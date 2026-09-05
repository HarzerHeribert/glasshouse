//! Glasshouse's own record of the sessions in one project.
//!
//! Deliberately *not* a view over a harness's session files: Claude Code,
//! Codex, and the rest each keep their own history in their own format, and
//! Glasshouse neither parses nor owns those files. The harness's own
//! identifier is recorded when known, as a nullable reference, so clearing
//! a harness's history never silently deletes Glasshouse's record.
//!
//! Every row carries the project identifier, enforced **structurally**, by
//! SQLite triggers created in migration 2, which abort any insert or
//! update whose `project_id` is not the identifier bound in
//! `project_metadata`; and **at the resume boundary**, by
//! [`SessionStore::open_for_resume`], which compares the stored identifier
//! against the active project before handing back anything a caller could
//! act on.
//!
//! History: design-decisions.md, "Trims: memory and session module docs", session/store/mod.rs module doc.

use std::fmt;
use std::path::PathBuf;
use std::sync::Arc;

use rusqlite::{Connection, OptionalExtension, Row};

use crate::database::PROJECT_ID_KEY;

use super::supervision::{self, ProcessIdentity, Supervision, SupervisionRefusal};

use record::{
    decode_assigned_model, decode_response_profile, encode_assigned_model, encode_response_profile,
};
// Named only by `tests.rs`, through this module's own `use super::*;` --
// unused outside `#[cfg(test)]`.
pub use claims::{FileClaim, STALE_CLAIM_AFTER};
pub use context::{
    AdvisoryCacheState, CacheState, CheckpointRecency, SessionContext, TaskContinuity,
};
pub use progress::{TASK_PROGRESS_EXPIRES_AFTER, TaskProgressDeclaration};
pub use record::{
    LabelError, NewSession, ResponseMechanism, ResumableSession, SessionDisposition, SessionId,
    SessionLifecycle, SessionName, SessionPairingClass, SessionPresentation, SessionProtocol,
    SessionPurpose, SessionRecord, SessionRole, SupervisionRecord,
};
#[cfg(test)]
use record::{MAX_SESSION_NAME, MAX_SESSION_PURPOSE};

/// Format 32 hex characters as an RFC 4122 version-4 UUID.
///
/// Six of the 128 bits are overwritten — four for the version, two for the
/// variant — which is what makes the result *valid* rather than merely
/// UUID-shaped, and leaves 122 random bits. A strict validator rejects an
/// 8-4-4-4-12 string whose version nibble is not `4`, and Glasshouse cannot
/// tell in advance which harnesses validate strictly.
///
/// Panics if `hex` is not exactly 32 hex characters; its only caller is the
/// SQL above, which cannot produce anything else.
fn uuid_v4_from_hex(hex: &str) -> String {
    assert_eq!(hex.len(), 32, "a 16-byte blob is 32 hex characters");
    let mut chars: Vec<char> = hex.chars().collect();
    // Version 4.
    chars[12] = '4';
    // Variant: the top two bits are `10`, so the nibble is one of 8, 9, a, b.
    chars[16] = match chars[16] {
        '0' | '4' | '8' | 'c' => '8',
        '1' | '5' | '9' | 'd' => '9',
        '2' | '6' | 'a' | 'e' => 'a',
        _ => 'b',
    };
    let s: String = chars.into_iter().collect();
    format!(
        "{}-{}-{}-{}-{}",
        &s[0..8],
        &s[8..12],
        &s[12..16],
        &s[16..20],
        &s[20..32]
    )
}

/// Whether a lifecycle change counts as the session having done something.
///
/// A marker rather than a `bool`, because the two call sites that differ are
/// three lines apart and `false` at a call site says nothing about what it
/// means. See [`SessionStore::write_lifecycle_locked`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Activity {
    Yes,
    No,
}

/// Whether this write is Glasshouse resuming a session, and may therefore
/// move a finished record back to a live state.
///
/// *"A finished session stays finished"* was written for one hazard: hook
/// processes are separate processes, and a slow one can deliver its event
/// after the harness it belongs to has exited, which would resurrect a
/// stopped session in the records. A genuine resume is not that case, so
/// the authority is a value only the resume boundary can supply, rather
/// than a property of the event or a loosening of
/// [`SessionLifecycle::is_live`] — which is unchanged, and which other
/// callers depend on. [`SessionStore::begin_resume`] is the only
/// constructor of [`Revival::Authorized`] in the crate, and it is
/// unreachable from the hook path: `glasshouse hook` never opens a resume
/// boundary.
///
/// History: design-decisions.md, "Trims: memory and session module docs", session/store/mod.rs Revival.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Revival {
    /// The default, and what every other caller passes: a finished session
    /// stays finished.
    Forbidden,
    /// Glasshouse is resuming this session itself.
    Authorized,
}

/// Failures a caller has to distinguish.
#[derive(Debug, thiserror::Error)]
pub enum SessionStoreError {
    #[error("no session `{id}` in this project")]
    NotFound { id: SessionId },
    #[error(
        "session `{id}` belongs to project `{actual}`, not to the active \
         project `{expected}`; refusing to resume another project's session"
    )]
    ForeignProject {
        id: SessionId,
        expected: String,
        actual: String,
    },
    #[error(
        "session `{id}` cannot be resumed because it is {disposition}; only a \
         stopped session with a recorded native session identifier can be \
         continued"
    )]
    NotResumable {
        id: SessionId,
        disposition: &'static str,
    },
    #[error(
        "`{prefix}` matches {} sessions ({}); use more of the identifier",
        .matches.len(),
        .matches.iter().map(SessionId::as_str).collect::<Vec<_>>().join(", ")
    )]
    AmbiguousPrefix {
        prefix: String,
        matches: Vec<SessionId>,
    },
    #[error("`{prefix}` is not a session identifier; identifiers are hexadecimal")]
    MalformedId { prefix: String },
    #[error(transparent)]
    Supervision(#[from] SupervisionRefusal),
    #[error("session `{id}` stored an unrecognized {column} value `{value}`")]
    UnknownValue {
        id: SessionId,
        column: &'static str,
        value: String,
    },
    #[error(
        "`{harness}` is {what}, not a harness; an interactive Glasshouse \
         session is always owned by a real harness, so there is no session \
         to record for one"
    )]
    NotAHarness { harness: String, what: &'static str },
    #[error(
        "`{harness}` is not a harness this build knows; a direct provider or \
         a gateway is a backend a harness talks to, never the owner of a \
         session"
    )]
    UnknownHarness { harness: String },
    #[error("session `{id}` is {lifecycle}, and a live session cannot be closed; stop it first")]
    StillLive {
        id: SessionId,
        lifecycle: SessionLifecycle,
    },
    #[error(
        "session `{id}` is {lifecycle}, and a session that has finished cannot claim a \
         file; a claim it took would be released again the moment anything read it"
    )]
    NotClaimable {
        id: SessionId,
        lifecycle: SessionLifecycle,
    },
    #[error(
        "`{path}` is not a path this project can claim; a claimed path is \
         repo-relative and inside the project, with no `..` component"
    )]
    ClaimPath { path: String },
    #[error(
        "session `{id}` is {lifecycle}, and a session that has finished has no \
         current task to be nearly complete; a declaration it made would be \
         honoured by nothing the moment anything read it"
    )]
    NotDeclarable {
        id: SessionId,
        lifecycle: SessionLifecycle,
    },
    #[error(transparent)]
    Label(#[from] LabelError),
    #[error("the project database has no project identifier bound")]
    UnboundDatabase,
    #[error("could not {action} in the project database")]
    Sql {
        action: &'static str,
        #[source]
        source: rusqlite::Error,
    },
}

/// Reads the wall clock, in seconds since the Unix epoch.
///
/// Injected rather than called directly so tests can assert on exact
/// timestamps instead of sleeping or accepting a range. Shared ownership
/// rather than a bare `fn` pointer because a useful test clock has to
/// *advance*, which means capturing state.
pub type Clock = Arc<dyn Fn() -> i64 + Send + Sync>;

/// Seconds since the Unix epoch.
///
/// Saturates rather than panicking on a clock set before 1970: a nonsensical
/// timestamp on one row is a far smaller problem than refusing to record a
/// session at all.
///
/// `pub(crate)` so that everything stamping a project-scoped record reads the
/// same clock: [`crate::checkpoint`] shares it rather than growing a second
/// one that could disagree with this one about what "now" is.
pub(crate) fn system_clock() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};

    match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(elapsed) => i64::try_from(elapsed.as_secs()).unwrap_or(i64::MAX),
        Err(_) => 0,
    }
}

const ALL_COLUMNS: &str = "id, project_id, harness, native_session_id, role, \
                           lifecycle, presentation, created_at, last_activity_at, \
                           launch_profile, backend_resource, model, pairing_class, \
                           protocol, response_profile, response_mechanism, \
                           display_name, purpose, source_session_id, \
                           observed_compactions, presentation_ref, last_seen_commit, \
                           entitlement";

/// An open project database plus the sessions inside it.
///
/// [`SessionStore`] borrows its connection so that one connection can back
/// several kinds of store as later phases add them. Callers that just want the
/// sessions — the CLI, and eventually the TUI — want something that owns the
/// connection instead, and this is it.
///
/// Opening goes through the crate's own `database::open` like everything
/// else, so the
/// symlink refusal, the read-only refusal, the project-identity check and the
/// migrations all still apply, and the path still comes from the runtime
/// rather than from a caller.
pub struct ProjectSessions {
    conn: Connection,
    project_id: String,
    clock: Clock,
    /// Where this project keeps a session's own files, so a quarantine can
    /// name what is still held. Carried rather than recomputed because
    /// `SessionStore` has no `Runtime` and must not grow one — it is a
    /// database-facing type, and the whole point of the split is that the
    /// paths come from the runtime.
    sessions_root: PathBuf,
}

impl fmt::Debug for ProjectSessions {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ProjectSessions")
            .field("project_id", &self.project_id)
            .finish_non_exhaustive()
    }
}

impl ProjectSessions {
    /// Open the active project's database and read its binding.
    pub fn open(runtime: &crate::Runtime) -> anyhow::Result<Self> {
        Self::open_with_clock(runtime, Arc::new(system_clock))
    }

    /// [`ProjectSessions::open`] with the clock replaced.
    ///
    /// # Supervision runs here — Phase 10A's second line
    ///
    /// *"Discover, on start, the sessions this project previously recorded
    /// whose processes are still running."* This is the door: `glasshouse
    /// launch`, `glasshouse resume`, `glasshouse sessions`, `glasshouse hook`
    /// and the interactive shell all open the project's sessions through
    /// here, and every one of them is a "start" in the sense the line means —
    /// a Glasshouse that is about to act on this project's sessions.
    ///
    /// Putting it in the shell alone would have missed the processes this
    /// phase exists because of: nobody was in the shell when they ran away.
    ///
    /// A failure to supervise is not a failure to open. Discovery reads the
    /// operating system, and an operating system that will not answer is a
    /// reason to say less, never a reason to refuse the user their session
    /// list.
    pub fn open_with_clock(runtime: &crate::Runtime, clock: Clock) -> anyhow::Result<Self> {
        let conn = crate::database::open(runtime)?;
        let project_id = SessionStore::with_clock(&conn, Arc::clone(&clock))?
            .project_id()
            .to_owned();
        let sessions = Self {
            conn,
            project_id,
            clock,
            sessions_root: runtime.session_dir(""),
        };
        sessions.supervise();
        Ok(sessions)
    }

    /// Reconcile every recorded live session against the machine, and tell the
    /// user about anything they have to decide.
    ///
    /// Phase 10A's eighth line — *"surface a quarantined session to the user
    /// with what is known about it and what it still holds"* — is the
    /// `eprintln!`. Standard error rather than standard output, because a
    /// script reading `glasshouse sessions` must keep getting the session
    /// list and nothing else; and it is written before any interface claims
    /// the terminal, because this runs at open.
    fn supervise(&self) {
        let store = self.store();
        let identity = supervision::ProcessIdentity::of_this_process();
        let now = (self.clock)();
        let report = match supervision::reconcile(&store, identity.as_ref(), now, &|id| {
            self.session_dir(id)
        }) {
            Ok(report) => report,
            Err(err) => {
                tracing::warn!(error = %err, "could not supervise this project's sessions");
                return;
            }
        };
        if let Some(described) = report.describe() {
            eprint!("{described}");
        }
        for session in report
            .adopted
            .iter()
            .chain(&report.quarantined)
            .chain(&report.lost)
            .chain(&report.never_ready)
        {
            tracing::info!(
                session = %session.id,
                harness = %session.harness,
                supervision = %session.supervision,
                reason = %session.reason,
                "supervision reached a conclusion about a recorded session"
            );
        }
    }

    /// Where Glasshouse keeps one session's own files.
    ///
    /// The same path [`crate::Runtime::session_dir`] produces; derived from
    /// the root captured at open so that this type needs no `Runtime` of its
    /// own.
    pub fn session_dir(&self, id: &SessionId) -> PathBuf {
        self.sessions_root.join(id.as_str())
    }

    /// The sessions in this project.
    pub fn store(&self) -> SessionStore<'_> {
        SessionStore {
            conn: &self.conn,
            project_id: self.project_id.clone(),
            clock: Arc::clone(&self.clock),
            sessions_root: self.sessions_root.clone(),
        }
    }
}

/// Session records for one project.
///
/// Borrows the connection rather than owning it, so the caller keeps control
/// of the database's lifetime and a single connection can back several stores
/// of different kinds as later phases add them.
pub struct SessionStore<'a> {
    conn: &'a Connection,
    project_id: String,
    clock: Clock,
    /// See [`ProjectSessions::sessions_root`]. Empty for a store opened
    /// straight over a connection, which is how the unit tests build one; a
    /// refusal then names the session directory relatively, which is still
    /// true and still useful.
    sessions_root: PathBuf,
}

impl fmt::Debug for SessionStore<'_> {
    /// Hand-written because [`Clock`] is a trait object with no `Debug`.
    /// Prints the project identifier — a hash of the canonical root, not a
    /// secret — and nothing about the connection.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SessionStore")
            .field("project_id", &self.project_id)
            .finish_non_exhaustive()
    }
}

impl<'a> SessionStore<'a> {
    /// Open the store over a connection produced by `database::open`.
    ///
    /// The project identifier is read from the database's own binding rather
    /// than accepted as an argument. That keeps the store honest about which
    /// project it is serving even if a caller is confused, and it means the
    /// identifier the store writes is by construction the identifier the
    /// triggers compare against.
    pub fn new(conn: &'a Connection) -> Result<Self, SessionStoreError> {
        Self::with_clock(conn, Arc::new(system_clock))
    }

    /// [`SessionStore::new`] with the clock replaced.
    pub fn with_clock(conn: &'a Connection, clock: Clock) -> Result<Self, SessionStoreError> {
        let project_id: Option<String> = conn
            .query_row(
                "SELECT value FROM project_metadata WHERE key = ?1",
                [PROJECT_ID_KEY],
                |row| row.get(0),
            )
            .optional()
            .map_err(|source| SessionStoreError::Sql {
                action: "read the project identifier",
                source,
            })?;

        Ok(Self {
            project_id: project_id.ok_or(SessionStoreError::UnboundDatabase)?,
            conn,
            clock,
            sessions_root: PathBuf::from("sessions"),
        })
    }

    /// The project every record in this store belongs to.
    pub fn project_id(&self) -> &str {
        &self.project_id
    }

    /// Where Glasshouse keeps one session's own files — see
    /// [`ProjectSessions::session_dir`], which is where the root comes from.
    pub fn session_dir(&self, id: &SessionId) -> PathBuf {
        self.sessions_root.join(id.as_str())
    }

    /// Start tracking a session.
    ///
    /// The identifier is generated by SQLite's own CSPRNG, which avoids a
    /// dependency and — more usefully — avoids the collision risk of anything
    /// derived from the clock, since sessions can be spawned in a burst.
    pub fn create(&self, new: NewSession) -> Result<SessionRecord, SessionStoreError> {
        let now = (self.clock)();
        // Line 646, and it is enforced here because this is the only door.
        // Refusing before an identifier is minted means a refused session
        // leaves nothing behind at all.
        require_owning_harness(&new.harness)?;

        // Phase 10A, first line. Recorded here because `create` is the only
        // door a session record comes through, so no future caller can start
        // a session Glasshouse would later be unable to identify.
        //
        // `None` is a real answer — a platform that will not name its
        // processes gets a session with no identity, and supervision then
        // refuses to conclude anything about it rather than guessing. That is
        // strictly better than a placeholder, which would match every other
        // placeholder on every other machine.
        let identity = supervision::ProcessIdentity::of_this_process();

        // Phase 10A, seventh line. A replacement must not be started while a
        // process nobody can account for still holds the same resources, and
        // the resource a *new* record can collide with is the harness's own
        // conversation. Checked before an identifier is minted, so a refused
        // session leaves nothing behind at all — `require_owning_harness`'s
        // argument, one line up, applied to the other refusal.
        if let Some(native) = new.native_session_id.as_deref() {
            self.refuse_if_quarantined_holds(&new.harness, native)?;
        }

        let id = SessionId(self.generate_id()?);

        let record = SessionRecord {
            id,
            project_id: self.project_id.clone(),
            harness: new.harness,
            native_session_id: new.native_session_id,
            role: new.role,
            lifecycle: SessionLifecycle::Starting,
            presentation: new.presentation,
            created_at: now,
            last_activity_at: now,
            launch_profile: new.launch_profile,
            backend_resource: new.backend_resource,
            model: new.model,
            pairing_class: new.pairing_class,
            protocol: new.protocol,
            response_profile: new.response_profile,
            response_mechanism: new.response_mechanism,
            // Two labels a person applies afterwards, never at creation: a
            // session Glasshouse named itself would be a name nobody chose.
            display_name: None,
            purpose: None,
            source_session_id: new.source_session_id,
            // `Some(0)`, never `None`. This build is counting from here on,
            // and a session it started that has compacted nothing has a
            // *measured* zero — which is the fact migration 16's nullable
            // column exists to keep apart from "nobody was counting". A
            // `None` written here would make the two indistinguishable for
            // every session Glasshouse ever starts, and the column would then
            // be carrying no information at all.
            observed_compactions: Some(0),
            presentation_ref: new.presentation_ref,
            // `None`, and this is the opposite call to the one above it for
            // the same reason. `observed_compactions` is `Some(0)` because
            // `create` can *measure* it — this build is counting from here.
            // Nothing here can measure a repository position: `create` has
            // the session's harness and role and no project root, and a
            // position guessed from the process's working directory would be
            // a claim about a repository nobody read. So the first hook that
            // does read one records it, and does not call it a boundary.
            last_seen_commit: None,
            // Unlike `last_seen_commit`, this one the caller CAN establish:
            // the launch path resolves and announces the serving entitlement
            // before anything is recorded, so it is supplied rather than
            // discovered later. `None` when that path established none, which
            // is a fact and not a gap.
            entitlement: new.entitlement,
        };

        self.conn
            .execute(
                "INSERT INTO sessions (id, project_id, harness, native_session_id, \
                 role, lifecycle, presentation, created_at, last_activity_at, \
                 launch_profile, backend_resource, model, pairing_class, protocol, \
                 response_profile, response_mechanism, process_id, \
                 process_started_at, process_host, supervision, source_session_id, \
                 observed_compactions, presentation_ref, last_seen_commit, \
                 entitlement) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, \
                 ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24, ?25)",
                rusqlite::params![
                    record.id.as_str(),
                    &record.project_id,
                    &record.harness,
                    &record.native_session_id,
                    record.role.as_str(),
                    record.lifecycle.as_str(),
                    record.presentation.as_str(),
                    record.created_at,
                    record.last_activity_at,
                    &record.launch_profile,
                    &record.backend_resource,
                    record.model.as_ref().map(encode_assigned_model),
                    record.pairing_class.map(SessionPairingClass::as_str),
                    record.protocol.map(SessionProtocol::as_str),
                    record
                        .response_profile
                        .as_ref()
                        .map(encode_response_profile),
                    record.response_mechanism.map(ResponseMechanism::as_str),
                    identity.as_ref().map(|identity| identity.pid),
                    identity.as_ref().map(|identity| identity.started_at_ms),
                    identity.as_ref().map(|identity| identity.host.as_str()),
                    // This Glasshouse started it and this Glasshouse is
                    // responsible for it. `owned` is the only conclusion
                    // `create` may reach; every other word in the vocabulary
                    // is something `supervision::reconcile` observed later,
                    // and writing one here would record an observation nobody
                    // made.
                    identity
                        .as_ref()
                        .map(|_| Supervision::Owned)
                        .map(Supervision::as_str),
                    record.source_session_id.as_ref().map(SessionId::as_str),
                    record.observed_compactions,
                    &record.presentation_ref,
                    &record.last_seen_commit,
                    &record.entitlement,
                ],
            )
            .map_err(|source| SessionStoreError::Sql {
                action: "record a new session",
                source,
            })?;

        Ok(record)
    }

    /// Mint an identifier for a harness that lets Glasshouse choose one.
    ///
    /// Formatted as an RFC 4122 version-4 UUID because that is what the
    /// harnesses which accept an assigned identifier demand — Claude Code
    /// refuses anything else outright ("Invalid session ID. Must be a valid
    /// UUID"). The randomness is SQLite's, the same source this store already
    /// uses for its own identifiers, so no second generator has to be trusted.
    ///
    /// Deliberately *not* derived from the Glasshouse session identifier.
    /// The two identifier spaces are independent by design — see
    /// [`SessionId`] — and a session's own name must stay meaningful after
    /// the harness's history is gone.
    pub fn new_native_session_id(&self) -> Result<String, SessionStoreError> {
        let hex: String = self
            .conn
            .query_row("SELECT lower(hex(randomblob(16)))", [], |row| row.get(0))
            .map_err(|source| SessionStoreError::Sql {
                action: "generate a native session identifier",
                source,
            })?;
        Ok(uuid_v4_from_hex(&hex))
    }

    fn generate_id(&self) -> Result<String, SessionStoreError> {
        self.conn
            .query_row("SELECT lower(hex(randomblob(16)))", [], |row| row.get(0))
            .map_err(|source| SessionStoreError::Sql {
                action: "generate a session identifier",
                source,
            })
    }

    /// Resolve a whole identifier, or the leading part of one, to exactly one
    /// session.
    ///
    /// A prefix is not a convenience here, it is a requirement: `glasshouse
    /// sessions` prints only the first twelve characters of an identifier, so
    /// the short form is the *only* one a user can copy from the screen. A
    /// resume command that demanded all thirty-two would be unusable with the
    /// identifiers Glasshouse itself shows.
    ///
    /// Ambiguity is refused rather than resolved — resuming the wrong session
    /// is worse than being asked to type four more characters — and the error
    /// names every candidate so the next attempt can succeed.
    ///
    /// Matching is done with `substr`, not `LIKE`: a `%` or `_` typed by the
    /// user would be a wildcard under `LIKE`, and `%` alone would silently
    /// match every session in the project.
    pub fn resolve_id(&self, prefix: &str) -> Result<SessionId, SessionStoreError> {
        let prefix = prefix.trim();
        if prefix.is_empty() || !prefix.chars().all(|c| c.is_ascii_hexdigit()) {
            return Err(SessionStoreError::MalformedId {
                prefix: prefix.to_owned(),
            });
        }
        let prefix = prefix.to_ascii_lowercase();

        let mut statement = self
            .conn
            .prepare("SELECT id FROM sessions WHERE substr(id, 1, ?2) = ?1 ORDER BY id")
            .map_err(|source| SessionStoreError::Sql {
                action: "prepare the session lookup",
                source,
            })?;
        let matches: Vec<SessionId> = statement
            .query_map(
                rusqlite::params![&prefix, i64::try_from(prefix.len()).unwrap_or(i64::MAX)],
                |row| row.get::<_, String>(0).map(SessionId),
            )
            .and_then(|rows| rows.collect::<Result<Vec<_>, _>>())
            .map_err(|source| SessionStoreError::Sql {
                action: "look a session up by identifier",
                source,
            })?;

        match matches.as_slice() {
            [] => Err(SessionStoreError::NotFound {
                id: SessionId(prefix),
            }),
            [only] => Ok(only.clone()),
            _ => Err(SessionStoreError::AmbiguousPrefix { prefix, matches }),
        }
    }

    /// Look one session up. `Ok(None)` means it is simply not here.
    pub fn get(&self, id: &SessionId) -> Result<Option<SessionRecord>, SessionStoreError> {
        self.conn
            .query_row(
                &format!("SELECT {ALL_COLUMNS} FROM sessions WHERE id = ?1"),
                [id.as_str()],
                |row| Ok(read_record(row)),
            )
            .optional()
            .map_err(|source| SessionStoreError::Sql {
                action: "look a session up",
                source,
            })?
            .transpose()
    }

    /// Every session in the project, most recently active first.
    pub fn list(&self) -> Result<Vec<SessionRecord>, SessionStoreError> {
        let mut statement = self
            .conn
            .prepare(&format!(
                "SELECT {ALL_COLUMNS} FROM sessions ORDER BY last_activity_at DESC, id ASC"
            ))
            .map_err(|source| SessionStoreError::Sql {
                action: "prepare the session list",
                source,
            })?;

        let rows = statement
            .query_map([], |row| Ok(read_record(row)))
            .map_err(|source| SessionStoreError::Sql {
                action: "list sessions",
                source,
            })?;

        let mut records = Vec::new();
        for row in rows {
            let record = row.map_err(|source| SessionStoreError::Sql {
                action: "read a session row",
                source,
            })?;
            records.push(record?);
        }
        Ok(records)
    }

    /// This project's live sessions carrying the orchestrator role — map line
    /// 2414's "which session is the orchestrator" query.
    ///
    /// Every candidate, not the first one found: whether the answer is
    /// exactly one, none, or several is the caller's decision — map line
    /// 2414's own architectural note is *"where there is no unambiguous
    /// active orchestrator, say the conflict could not be delivered rather
    /// than guessing a recipient"*, and a method that took the first row
    /// would have made that guess here instead, silently.
    pub fn live_orchestrators(&self) -> Result<Vec<SessionRecord>, SessionStoreError> {
        Ok(self
            .list()?
            .into_iter()
            .filter(|record| record.role == SessionRole::Orchestrator && record.lifecycle.is_live())
            .collect())
    }

    /// Move a session to a new lifecycle state, which also counts as activity.
    ///
    /// The single ordered path — Phase 10A's twelfth line: every lifecycle
    /// change in the shipped binary arrives here, from **separate
    /// operating-system processes** (the launch path, the shell's exit
    /// handling, `glasshouse hook`), and until this method took a
    /// transaction they raced in the classic read-check-write shape,
    /// producing a live state (`idle`) for a session whose process was
    /// already gone. `BEGIN IMMEDIATE` takes SQLite's write lock **before**
    /// the read, so the read and the write are one indivisible step and the
    /// losing writer sees the winner's state and declines.
    ///
    /// What it declines is one rule, [`super::lifecycle::may_apply`]'s: **a
    /// session that has finished may not be moved back to a live state.** A
    /// declined change returns the record as it stands rather than an
    /// error: the caller asked for something that is no longer true.
    ///
    /// History: design-decisions.md, "Trims: memory and session module docs", session/store/mod.rs set_lifecycle.
    pub fn set_lifecycle(
        &self,
        id: &SessionId,
        lifecycle: SessionLifecycle,
    ) -> Result<SessionRecord, SessionStoreError> {
        let action = "update a session's lifecycle";
        self.in_a_write_transaction(action, || {
            self.write_lifecycle_locked(id, lifecycle, Activity::Yes, Revival::Forbidden, action)
        })?;
        self.get(id)?
            .ok_or(SessionStoreError::NotFound { id: id.clone() })
    }

    /// **The only statement in this crate that moves a session's lifecycle**:
    /// one `UPDATE`, and everything else has to come through it —
    /// `one_statement_moves_a_sessions_lifecycle` fails if a second appears.
    ///
    /// Callers must already hold a write transaction — see
    /// [`SessionStore::in_a_write_transaction`], which is what makes the read
    /// below and the write after it one indivisible step.
    ///
    /// What it declines is one rule, [`super::lifecycle::may_apply`]'s: **a
    /// session that has finished may not be moved back to a live state.** A
    /// declined change leaves the record as it stands rather than
    /// erroring.
    ///
    /// History: design-decisions.md, "Trims: memory and session module docs", session/store/mod.rs write_lifecycle_locked.
    fn write_lifecycle_locked(
        &self,
        id: &SessionId,
        next: SessionLifecycle,
        activity: Activity,
        revival: Revival,
        action: &'static str,
    ) -> Result<(), SessionStoreError> {
        let current = self.read_lifecycle_locked(id, action)?;

        // A finished session stays finished — unless Glasshouse is the one
        // reopening it. See [`Revival`] for why that is a value the caller
        // supplies rather than something inferred from `next`: every caller
        // but [`SessionStore::begin_resume`] passes `Forbidden`, and the hook
        // path cannot reach the one that does not.
        if revival == Revival::Forbidden && !current.is_live() && next.is_live() {
            return Ok(());
        }

        // Whether the change counts as activity is decided *inside the one
        // statement*, rather than by having two statements. Closing a record
        // is something a person did to it, not something the session did —
        // see `SessionStore::close` — and stamping it would push a finished
        // session back to the top of a list ordered by when it last ran.
        self.conn
            .execute(
                "UPDATE sessions SET lifecycle = ?2, \
                 last_activity_at = CASE WHEN ?4 THEN ?3 ELSE last_activity_at END \
                 WHERE id = ?1",
                rusqlite::params![
                    id.as_str(),
                    next.as_str(),
                    (self.clock)(),
                    activity == Activity::Yes
                ],
            )
            .map_err(|source| SessionStoreError::Sql { action, source })?;
        Ok(())
    }

    /// A session's current lifecycle, read inside the caller's write
    /// transaction so that what it decides cannot be stale by the time it
    /// writes.
    fn read_lifecycle_locked(
        &self,
        id: &SessionId,
        action: &'static str,
    ) -> Result<SessionLifecycle, SessionStoreError> {
        let current: Option<String> = self
            .conn
            .query_row(
                "SELECT lifecycle FROM sessions WHERE id = ?1",
                [id.as_str()],
                |row| row.get(0),
            )
            .optional()
            .map_err(|source| SessionStoreError::Sql { action, source })?;
        let Some(current) = current else {
            return Err(SessionStoreError::NotFound { id: id.clone() });
        };
        SessionLifecycle::from_str(&current).ok_or(SessionStoreError::UnknownValue {
            id: id.clone(),
            column: "lifecycle",
            value: current,
        })
    }

    /// Run `body` with SQLite's write lock already held, and end the
    /// transaction on every path out.
    ///
    /// `IMMEDIATE`, not the default `DEFERRED`. A deferred transaction takes
    /// only a read lock until its first write and then has to *upgrade*; if
    /// another connection has committed in between, SQLite refuses the upgrade
    /// rather than waiting, and `busy_timeout` does not help because there is
    /// nothing to wait for — the read is already stale. Taking the write lock
    /// up front turns that failure into a wait, which is what makes several
    /// `glasshouse` processes writing to one session's record safe rather than
    /// merely lucky.
    fn in_a_write_transaction<T>(
        &self,
        action: &'static str,
        body: impl FnOnce() -> Result<T, SessionStoreError>,
    ) -> Result<T, SessionStoreError> {
        self.conn
            .execute_batch("BEGIN IMMEDIATE")
            .map_err(|source| SessionStoreError::Sql { action, source })?;
        let outcome = body();
        let ended = if outcome.is_ok() {
            self.conn.execute_batch("COMMIT")
        } else {
            self.conn.execute_batch("ROLLBACK")
        };
        let value = outcome?;
        ended.map_err(|source| SessionStoreError::Sql { action, source })?;
        Ok(value)
    }

    /// Everything supervision recorded about one session's process.
    ///
    /// # Why this is not five more fields on [`SessionRecord`]
    ///
    /// A `SessionRecord` is what a session *is*. Whether the process it was
    /// started in is still running is a fact about the machine right now, it
    /// changes without the record changing, and every caller that wants one
    /// wants it fresh. Folding it into the record would also have made a
    /// session's identity depend on a reading of the operating system, so two
    /// records of the same session taken a second apart would compare unequal.
    ///
    /// Returns [`SupervisionRecord::default`] — no identity, no conclusion —
    /// for a session recorded by a build that stored none. That is a real
    /// answer and callers treat it as one: nothing may be adopted, quarantined
    /// or declared stopped on the strength of an absent identity.
    pub fn supervision_of(&self, id: &SessionId) -> Result<SupervisionRecord, SessionStoreError> {
        let row = self
            .conn
            .query_row(
                "SELECT process_id, process_started_at, process_host, supervision, \
                 supervision_reason FROM sessions WHERE id = ?1",
                [id.as_str()],
                |row| {
                    Ok((
                        row.get::<_, Option<i64>>(0)?,
                        row.get::<_, Option<i64>>(1)?,
                        row.get::<_, Option<String>>(2)?,
                        row.get::<_, Option<String>>(3)?,
                        row.get::<_, Option<String>>(4)?,
                    ))
                },
            )
            .optional()
            .map_err(|source| SessionStoreError::Sql {
                action: "read a session's supervision",
                source,
            })?;

        let Some((pid, started_at, host, supervision, reason)) = row else {
            return Err(SessionStoreError::NotFound { id: id.clone() });
        };

        // The three identity columns are read together or not at all. A pid
        // without a start time is not an identity — it is exactly what Phase
        // 10A's first line stops Glasshouse trusting — and a start time
        // without a host is a number about a machine that may not be this one.
        // A partially recorded identity therefore reads as no identity, and
        // supervision refuses to conclude anything rather than guessing at the
        // missing part.
        let identity = match (pid, started_at, host) {
            (Some(pid), Some(started_at_ms), Some(host)) => {
                u32::try_from(pid).ok().map(|pid| ProcessIdentity {
                    pid,
                    started_at_ms,
                    host,
                })
            }
            _ => None,
        };

        let supervision = match supervision {
            None => None,
            Some(word) => Some(Supervision::from_str(&word).ok_or_else(|| {
                SessionStoreError::UnknownValue {
                    id: id.clone(),
                    column: "supervision",
                    value: word,
                }
            })?),
        };

        Ok(SupervisionRecord {
            identity,
            supervision,
            reason,
        })
    }

    /// Record what supervision concluded about a session's process, and — when
    /// the conclusion implies one — the lifecycle state that follows from it.
    ///
    /// Both writes go through the same transaction as every other lifecycle
    /// change, for the reason [`SessionStore::set_lifecycle`] gives: a
    /// supervision pass in one `glasshouse` process runs beside a hook in
    /// another, and a conclusion drawn from a read that is already stale is
    /// worse than no conclusion.
    ///
    /// **This never ends anything.** `Lost` is written because the process was
    /// observed to be gone, and `Quarantined` deliberately leaves the
    /// lifecycle alone: a quarantined session is neither stopped nor healthy,
    /// and overwriting its state with either would erase the whole distinction
    /// this phase is about.
    pub fn record_supervision(
        &self,
        id: &SessionId,
        supervision: Supervision,
        reason: &str,
        lifecycle: Option<SessionLifecycle>,
    ) -> Result<SessionRecord, SessionStoreError> {
        let action = "record a supervision conclusion";
        self.in_a_write_transaction(action, || {
            let changed = self
                .conn
                .execute(
                    "UPDATE sessions SET supervision = ?2, supervision_reason = ?3 \
                     WHERE id = ?1",
                    rusqlite::params![id.as_str(), supervision.as_str(), reason],
                )
                .map_err(|source| SessionStoreError::Sql { action, source })?;
            if changed == 0 {
                return Err(SessionStoreError::NotFound { id: id.clone() });
            }
            // Through the same one statement every other lifecycle change goes
            // through, inside the same transaction as the conclusion that
            // implied it — so a supervision pass and a hook in another process
            // cannot each half-apply.
            match lifecycle {
                Some(lifecycle) => self.write_lifecycle_locked(
                    id,
                    lifecycle,
                    Activity::Yes,
                    Revival::Forbidden,
                    action,
                ),
                None => Ok(()),
            }
        })?;
        self.get(id)?
            .ok_or(SessionStoreError::NotFound { id: id.clone() })
    }

    /// Refuse a new session that would take a conversation a quarantined
    /// process still holds — Phase 10A's seventh line.
    ///
    /// Scoped to the harness as well as the identifier, because the unique
    /// index that already exists is scoped that way: two harnesses may
    /// coincidentally spell an identifier the same, and refusing across that
    /// coincidence would refuse a start for no reason.
    fn refuse_if_quarantined_holds(
        &self,
        harness: &str,
        native_session_id: &str,
    ) -> Result<(), SessionStoreError> {
        let held: Option<String> = self
            .conn
            .query_row(
                "SELECT id FROM sessions WHERE harness = ?1 AND native_session_id = ?2 \
                 AND supervision = 'quarantined'",
                rusqlite::params![harness, native_session_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(|source| SessionStoreError::Sql {
                action: "check whether a quarantined session holds this conversation",
                source,
            })?;
        match held {
            None => Ok(()),
            Some(id) => Err(SupervisionRefusal::Quarantined {
                id: SessionId(id),
                holds: format!("the {harness} conversation `{native_session_id}`"),
                reason: "a process Glasshouse cannot account for was still running when \
                         that session was last examined"
                    .to_owned(),
            }
            .into()),
        }
    }

    /// Record the harness's own identifier, once it is known.
    pub fn set_native_session_id(
        &self,
        id: &SessionId,
        native_session_id: &str,
    ) -> Result<SessionRecord, SessionStoreError> {
        self.update(
            id,
            "UPDATE sessions SET native_session_id = ?2, last_activity_at = ?3 WHERE id = ?1",
            rusqlite::params![id.as_str(), native_session_id, (self.clock)()],
            "record a native session identifier",
        )
    }

    /// Note that something happened in a session, without changing its state.
    pub fn touch(&self, id: &SessionId) -> Result<SessionRecord, SessionStoreError> {
        self.update(
            id,
            "UPDATE sessions SET last_activity_at = ?2 WHERE id = ?1",
            rusqlite::params![id.as_str(), (self.clock)()],
            "record session activity",
        )
    }

    /// Count one compaction a harness said it was about to perform — map
    /// line 1159.
    ///
    /// A column, not an event: a compaction cannot join
    /// `LIFECYCLE_EVENT_KINDS` because that vocabulary is a SQL `CHECK` that
    /// SQLite cannot widen in place, so the count lives on the session row
    /// instead and the event log is left exactly as narrow as it was.
    ///
    /// `COALESCE`: a row recorded before migration 16 reads `NULL`, meaning
    /// *"nobody was counting"*, and its first observed compaction moves it
    /// to `1` rather than leaving it unknowable forever — from then on the
    /// number is a **lower bound**, since compactions before the upgrade
    /// cannot be recovered. For a session this build created the count is
    /// exact, because `create` starts it at a measured `0`.
    ///
    /// `last_activity_at` is untouched: a compaction is the harness
    /// reorganising what it holds, not the session doing work.
    ///
    /// History: design-decisions.md, "Trims: memory and session module docs", session/store/mod.rs record_observed_compaction.
    pub fn record_observed_compaction(
        &self,
        id: &SessionId,
    ) -> Result<SessionRecord, SessionStoreError> {
        self.update(
            id,
            "UPDATE sessions \
             SET observed_compactions = COALESCE(observed_compactions, 0) + 1 \
             WHERE id = ?1",
            rusqlite::params![id.as_str()],
            "count an observed compaction",
        )
    }

    /// Record where HEAD stands for this session — map line 1149.
    ///
    /// # It stores, and says nothing about whether a boundary happened
    ///
    /// Deliberately not `record_code_change_boundary`. This writes one
    /// column; whether the new position *is* a boundary is a comparison
    /// against [`SessionRecord::last_seen_commit`] that the caller has
    /// already made, and folding the comparison in here would put the
    /// decision behind a write and make "the position was learned" and "a
    /// commit landed" the same event. They are not: the first look at a
    /// session records a position and is not a boundary, because there is
    /// nothing to have changed from.
    ///
    /// # It is not activity
    ///
    /// `last_activity_at` is untouched, exactly as
    /// [`SessionStore::record_observed_compaction`] leaves it: a commit is
    /// something the *repository* did, and stamping it here would move a
    /// session up a list ordered by when it last ran on the strength of a
    /// `git commit` typed in another terminal.
    pub fn record_seen_commit(
        &self,
        id: &SessionId,
        commit: &str,
    ) -> Result<SessionRecord, SessionStoreError> {
        self.update(
            id,
            "UPDATE sessions SET last_seen_commit = ?2 WHERE id = ?1",
            rusqlite::params![id.as_str(), commit],
            "record where HEAD stood for a session",
        )
    }

    /// Everything Phase 30 can say about one session's context, as of now.
    ///
    /// `Ok(None)` for a session this project does not have, exactly as
    /// [`SessionStore::get`] answers.
    ///
    /// One function and not several: four of Phase 30's lines are answered
    /// by facts that already existed and were unreadable together — a
    /// caller assembling them itself would have to know that "recent
    /// checkpoint" is a comparison against `last_activity_at`. See
    /// [`SessionContext`], including its paragraph on the line that is
    /// **not** here.
    ///
    /// Reads two sibling tables and never writes them, so the project
    /// boundary is honoured by the query. Nothing here is stored: a stored
    /// `hot` is wrong the minute after it is written. Only
    /// [`SessionRecord::observed_compactions`] is durable, because a
    /// compaction leaves no trace anywhere else.
    /// History: design-decisions.md, "Trims: memory and session module docs", session/store/mod.rs context.
    pub fn context(&self, id: &SessionId) -> Result<Option<SessionContext>, SessionStoreError> {
        let Some(record) = self.get(id)? else {
            return Ok(None);
        };
        let now = (self.clock)();

        let newest_checkpoint: Option<i64> = self
            .conn
            .query_row(
                "SELECT MAX(created_at) FROM checkpoints \
                 WHERE project_id = ?1 AND session_id = ?2",
                rusqlite::params![&self.project_id, id.as_str()],
                |row| row.get(0),
            )
            .map_err(|source| SessionStoreError::Sql {
                action: "read a session's newest checkpoint",
                source,
            })?;

        // `MAX` over no rows is one row holding NULL, so the aggregate below
        // is read the same way: `COUNT(*)` is `0` and the conditional sum is
        // `0`, and the two together are what separates "no events at all"
        // from "events, no boundaries among them".
        let (observed_events, boundaries): (i64, i64) = self
            .conn
            .query_row(
                "SELECT COUNT(*), \
                        COALESCE(SUM(CASE WHEN kind = 'turn_ended' \
                                           AND turn_outcome = 'completed' \
                                          THEN 1 ELSE 0 END), 0) \
                   FROM lifecycle_events \
                  WHERE project_id = ?1 AND session_id = ?2",
                rusqlite::params![&self.project_id, id.as_str()],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .map_err(|source| SessionStoreError::Sql {
                action: "count a session's observed task boundaries",
                source,
            })?;

        Ok(Some(SessionContext {
            session: record.id.clone(),
            observed_compactions: record.observed_compactions,
            last_activity_at: record.last_activity_at,
            prompt_cache: AdvisoryCacheState::estimate(now, record.last_activity_at),
            checkpoint: match newest_checkpoint {
                None => CheckpointRecency::Never,
                Some(at) if at >= record.last_activity_at => CheckpointRecency::Current(at),
                Some(at) => CheckpointRecency::Stale(at),
            },
            task_continuity: match (observed_events, boundaries) {
                (0, _) => TaskContinuity::Unknown,
                (_, 0) => TaskContinuity::OneTask,
                (_, crossed) => TaskContinuity::BoundariesCrossed(crossed),
            },
        }))
    }

    /// Give a session a name of the user's own — line 650.
    ///
    /// # The native session identifier is not among the columns named here
    ///
    /// That is the whole of line 650: *"allow the user to rename a session
    /// without changing its native session ID"*. The identifier is what a
    /// harness is asked to continue from, so a rename that touched it would
    /// silently break resume, and nothing about the failure would point back
    /// at the rename. One `SET`, one column, and
    /// `renaming_a_session_leaves_its_native_identifier_alone` reads the
    /// identifier back afterwards rather than merely checking no error was
    /// returned.
    ///
    /// # And `last_activity_at` is not among them either
    ///
    /// Naming a session is something the *user* did, not something the
    /// session did. Stamping it as activity would move a finished session
    /// back to the top of a list ordered by when it last ran, which is the
    /// one question that list exists to answer.
    pub fn rename(
        &self,
        id: &SessionId,
        name: &SessionName,
    ) -> Result<SessionRecord, SessionStoreError> {
        self.update(
            id,
            "UPDATE sessions SET display_name = ?2 WHERE id = ?1",
            rusqlite::params![id.as_str(), name.as_str()],
            "rename a session",
        )
    }

    /// Take a session's name away again, leaving it identified by nothing but
    /// its identifiers.
    pub fn clear_name(&self, id: &SessionId) -> Result<SessionRecord, SessionStoreError> {
        self.update(
            id,
            "UPDATE sessions SET display_name = NULL WHERE id = ?1",
            rusqlite::params![id.as_str()],
            "clear a session's name",
        )
    }

    /// Tag a session with a lightweight purpose — line 651.
    ///
    /// A separate column and a separate type from the display name, so that
    /// tagging cannot rename and renaming cannot tag. Like a rename, it does
    /// not count as session activity.
    pub fn set_purpose(
        &self,
        id: &SessionId,
        purpose: &SessionPurpose,
    ) -> Result<SessionRecord, SessionStoreError> {
        self.update(
            id,
            "UPDATE sessions SET purpose = ?2 WHERE id = ?1",
            rusqlite::params![id.as_str(), purpose.as_str()],
            "tag a session",
        )
    }

    /// Record where a session is presented now — line 760, for a session
    /// that was recorded before it reached the place it is shown: a
    /// continued session picked up inside an external pane.
    ///
    /// Two columns, written together, because they are one fact: a
    /// presentation that names a pane and a pane without a presentation
    /// would each be half an answer. Like a rename, it does not count as
    /// session activity. The reference is stored as given — see
    /// [`SessionRecord::presentation_ref`] for why this module never
    /// interprets it.
    pub fn set_presentation(
        &self,
        id: &SessionId,
        presentation: SessionPresentation,
        presentation_ref: Option<&str>,
    ) -> Result<SessionRecord, SessionStoreError> {
        self.update(
            id,
            "UPDATE sessions SET presentation = ?2, presentation_ref = ?3 WHERE id = ?1",
            rusqlite::params![id.as_str(), presentation.as_str(), presentation_ref],
            "record where a session is presented",
        )
    }

    /// Remove a session's purpose tag.
    pub fn clear_purpose(&self, id: &SessionId) -> Result<SessionRecord, SessionStoreError> {
        self.update(
            id,
            "UPDATE sessions SET purpose = NULL WHERE id = ?1",
            rusqlite::params![id.as_str()],
            "clear a session's purpose",
        )
    }

    /// Retire Glasshouse's record of a session — line 654.
    ///
    /// It writes one column. `native_session_id` is untouched, and so is
    /// every harness file on disk, per line 654's *"without deleting the
    /// native provider history unless explicitly requested"* — nothing here
    /// is a request.
    ///
    /// A live session is refused: closing is filing a record away, and a
    /// record whose process is still running is not finished being
    /// written. Refusing names the state so the user knows to stop the
    /// session first.
    ///
    /// `last_activity_at` stays put: when the session last did something is
    /// a fact about the session, and when somebody filed it away is a
    /// different fact.
    ///
    /// History: design-decisions.md, "Trims: memory and session module docs", session/store/mod.rs close.
    pub fn close(&self, id: &SessionId) -> Result<SessionRecord, SessionStoreError> {
        // Through the same ordered path as every other lifecycle change —
        // Phase 10A's twelfth line. The liveness check and the write used to
        // be a read outside a transaction followed by a write, which is the
        // interleaving that line forbids: a hook process moving the session
        // back to `running` in between would leave a `closed` row that a live
        // harness kept updating. Reading under the write lock closes it.
        let action = "close a session record";
        self.in_a_write_transaction(action, || {
            let current = self.read_lifecycle_locked(id, action)?;
            if current.is_live() {
                return Err(SessionStoreError::StillLive {
                    id: id.clone(),
                    lifecycle: current,
                });
            }
            self.write_lifecycle_locked(
                id,
                SessionLifecycle::Closed,
                Activity::No,
                Revival::Forbidden,
                action,
            )
        })?;
        self.get(id)?
            .ok_or(SessionStoreError::NotFound { id: id.clone() })
    }

    fn update(
        &self,
        id: &SessionId,
        sql: &str,
        params: &[&dyn rusqlite::ToSql],
        action: &'static str,
    ) -> Result<SessionRecord, SessionStoreError> {
        let changed = self
            .conn
            .execute(sql, params)
            .map_err(|source| SessionStoreError::Sql { action, source })?;
        if changed == 0 {
            return Err(SessionStoreError::NotFound { id: id.clone() });
        }
        self.get(id)?
            .ok_or(SessionStoreError::NotFound { id: id.clone() })
    }

    /// Check that a session may be resumed here, and return what a resume
    /// needs.
    ///
    /// This is the enforcement point for the rule that one Glasshouse instance
    /// never continues another project's session. The stored project
    /// identifier is compared against the active one and a mismatch is an
    /// error, never a filtered-away row: the caller asked about a specific
    /// session and deserves to be told it belongs somewhere else, rather than
    /// being told it does not exist and left to wonder.
    ///
    /// The comparison is not made redundant by migration 2's triggers. Those
    /// decide what may be written; this decides what may be acted upon, and
    /// covers rows that arrived by any route the triggers did not police — a
    /// restored backup, a hand-edited file, a build whose schema predates the
    /// guard.
    pub fn open_for_resume(&self, id: &SessionId) -> Result<ResumableSession, SessionStoreError> {
        let record = self
            .get(id)?
            .ok_or_else(|| SessionStoreError::NotFound { id: id.clone() })?;

        if record.project_id != self.project_id {
            return Err(SessionStoreError::ForeignProject {
                id: id.clone(),
                expected: self.project_id.clone(),
                actual: record.project_id,
            });
        }

        // Phase 10A, lines five and seven, and they are asked *before* the
        // disposition question on purpose.
        //
        // A record whose process is verified still running is refused for
        // being still running, naming the process; a record held by something
        // Glasshouse cannot account for is refused for that, naming what is
        // held. Asking `disposition` first would answer both with *"still
        // running"* or *"closed"* — true of the record, useless about the
        // machine, and in the quarantine case actively misleading, because a
        // quarantined session is neither.
        supervision::guard_start(
            &record,
            &self.supervision_of(&record.id)?,
            supervision::ProcessIdentity::of_this_process().as_ref(),
            &|id| self.session_dir(id),
        )?;

        let disposition = record.disposition();
        if disposition != SessionDisposition::Resumable {
            return Err(SessionStoreError::NotResumable {
                id: id.clone(),
                disposition: match disposition {
                    SessionDisposition::Active => "still running",
                    SessionDisposition::Closed => "closed",
                    SessionDisposition::Failed => "failed",
                    SessionDisposition::Resumable => unreachable!("checked above"),
                },
            });
        }

        Ok(ResumableSession {
            id: record.id,
            harness: record.harness,
            native_session_id: record
                .native_session_id
                .expect("a resumable disposition requires a native session identifier"),
        })
    }

    /// Record that Glasshouse is resuming this session, moving it back to
    /// `Running`.
    ///
    /// Not `set_lifecycle`, which declines to move a finished record back to
    /// a live state and must keep declining: the two cases are told apart
    /// by **who is acting**. A resume is something Glasshouse does, at a
    /// boundary it opened deliberately; a late hook merely arrives. So this
    /// carries `Revival::Authorized` as a separate operation, rather than
    /// widening [`SessionLifecycle::is_live`] or `lifecycle::may_apply`.
    ///
    /// The disposition is checked again under the write lock, since
    /// [`SessionStore::open_for_resume`] reads outside a transaction: only a
    /// **stopped** record with a native identifier is resumable.
    /// The process identity is re-recorded here in one transaction with the
    /// lifecycle write: leaving the record naming the old process would
    /// make `supervision::reconcile` reach [`Verdict::Gone`] and undo the
    /// resume. `None` clears the columns rather than leaving old values.
    /// History: design-decisions.md, "Trims: memory and session module docs", session/store/mod.rs begin_resume.
    ///
    /// [`Verdict::Gone`]: super::supervision::Verdict::Gone
    pub fn begin_resume(
        &self,
        resumable: &ResumableSession,
    ) -> Result<SessionRecord, SessionStoreError> {
        let id = &resumable.id;
        let action = "record a session resume";
        // Asked before the write lock is taken. It reads the operating system
        // about *this* process, whose answer no other writer can change, and
        // the lock is for ordering writers rather than for holding a syscall.
        let identity = supervision::ProcessIdentity::of_this_process();
        self.in_a_write_transaction(action, || {
            let record = self
                .get(id)?
                .ok_or_else(|| SessionStoreError::NotFound { id: id.clone() })?;
            let disposition = record.disposition();
            if disposition != SessionDisposition::Resumable {
                return Err(SessionStoreError::NotResumable {
                    id: id.clone(),
                    disposition: match disposition {
                        SessionDisposition::Active => "still running",
                        SessionDisposition::Closed => "closed",
                        SessionDisposition::Failed => "failed",
                        SessionDisposition::Resumable => unreachable!("checked above"),
                    },
                });
            }

            // The other half of what `open_for_resume` already decided,
            // re-asked here for the same reason the disposition is: it read
            // outside a transaction, and a quarantine recorded in between
            // would otherwise be *overwritten* by the identity write below —
            // turning a session Glasshouse may not touch into one it owns.
            // Only the quarantine arm can fire, because a resumable record is
            // stopped and the duplicate refusal is about a live one; it is
            // still asked through `guard_start` so that a caller cannot check
            // one refusal and forget the other, which is what that function
            // exists for.
            supervision::guard_start(
                &record,
                &self.supervision_of(id)?,
                identity.as_ref(),
                &|id| self.session_dir(id),
            )?;

            self.write_identity_locked(id, identity.as_ref(), action)?;
            self.write_lifecycle_locked(
                id,
                SessionLifecycle::Running,
                Activity::Yes,
                Revival::Authorized,
                action,
            )
        })?;
        self.get(id)?
            .ok_or(SessionStoreError::NotFound { id: id.clone() })
    }

    /// Record the process a session is running in, replacing whatever was
    /// recorded before it.
    ///
    /// The write [`SessionStore::create`] makes as part of its `INSERT`, as an
    /// `UPDATE`, so that the other way a session becomes live can make it too.
    /// Callers must already hold a write transaction — the identity and the
    /// lifecycle it belongs to are one change, and a reader that could see
    /// half of it is the defect this exists to close.
    ///
    /// `supervision` is set to [`Supervision::Owned`] beside the identity,
    /// and the reason cleared: this Glasshouse is responsible for this
    /// process, and it is the only conclusion a writer that is not
    /// [`super::supervision::reconcile`] may reach.
    ///
    /// A `None` identity clears all four columns rather than half of them —
    /// [`SessionStore::supervision_of`] reads the three identity columns
    /// together or not at all, and a partially cleared row would be read as an
    /// identity built from whichever parts survived.
    ///
    /// History: design-decisions.md, "Trims: memory and session module docs", session/store/mod.rs write_identity_locked.
    fn write_identity_locked(
        &self,
        id: &SessionId,
        identity: Option<&ProcessIdentity>,
        action: &'static str,
    ) -> Result<(), SessionStoreError> {
        let changed = self
            .conn
            .execute(
                "UPDATE sessions SET process_id = ?2, process_started_at = ?3, \
                 process_host = ?4, supervision = ?5, supervision_reason = NULL \
                 WHERE id = ?1",
                rusqlite::params![
                    id.as_str(),
                    identity.map(|identity| identity.pid),
                    identity.map(|identity| identity.started_at_ms),
                    identity.map(|identity| identity.host.as_str()),
                    identity.map(|_| Supervision::Owned.as_str()),
                ],
            )
            .map_err(|source| SessionStoreError::Sql { action, source })?;
        if changed == 0 {
            return Err(SessionStoreError::NotFound { id: id.clone() });
        }
        Ok(())
    }
}

/// Build a record from a row, turning an unrecognized enum string into a
/// typed error rather than a panic or a silent default.
fn read_record(row: &Row<'_>) -> Result<SessionRecord, SessionStoreError> {
    // `row.get_unwrap` panics on any conversion failure, including a TEXT
    // column whose stored bytes are not valid UTF-8 -- which a single bit
    // flip in an otherwise untouched database file can produce without
    // `PRAGMA integrity_check` ever noticing, and which then crashes every
    // future command that lists or looks up a session. `col` reports that
    // the same way every other store in this crate reports a SQL failure.
    fn col<T: rusqlite::types::FromSql>(
        row: &Row<'_>,
        index: usize,
    ) -> Result<T, SessionStoreError> {
        row.get(index).map_err(|source| SessionStoreError::Sql {
            action: "read a session column",
            source,
        })
    }

    let id = SessionId(col::<String>(row, 0)?);

    fn decode<T>(
        id: &SessionId,
        column: &'static str,
        value: String,
        parsed: Option<T>,
    ) -> Result<T, SessionStoreError> {
        parsed.ok_or_else(|| SessionStoreError::UnknownValue {
            id: id.clone(),
            column,
            value,
        })
    }

    let role_text: String = col(row, 4)?;
    let lifecycle_text: String = col(row, 5)?;
    let presentation_text: String = col(row, 6)?;

    let role = decode(
        &id,
        "role",
        role_text.clone(),
        SessionRole::from_str(&role_text),
    )?;
    let lifecycle = decode(
        &id,
        "lifecycle",
        lifecycle_text.clone(),
        SessionLifecycle::from_str(&lifecycle_text),
    )?;
    let presentation = decode(
        &id,
        "presentation",
        presentation_text.clone(),
        SessionPresentation::from_str(&presentation_text),
    )?;

    // Each of these decodes through its own function and reports an
    // unrecognised value by name rather than defaulting. A row written by a
    // newer build is then a legible error naming the column and the value,
    // which is what a person needs; a silent default would report a session
    // as having run under something it did not.
    let model = optional(&id, "model", col(row, 11)?, decode_assigned_model)?;
    let pairing_class = optional(
        &id,
        "pairing_class",
        col(row, 12)?,
        SessionPairingClass::from_str,
    )?;
    let protocol = optional(&id, "protocol", col(row, 13)?, SessionProtocol::from_str)?;
    let response_profile = optional(
        &id,
        "response_profile",
        col(row, 14)?,
        decode_response_profile,
    )?;
    let response_mechanism = optional(
        &id,
        "response_mechanism",
        col(row, 15)?,
        ResponseMechanism::from_str,
    )?;
    // The two labels are stored as the person typed them, so a stored value
    // that no longer parses — a bound tightened in a later release — is
    // reported rather than shown truncated.
    let display_name = optional(&id, "display_name", col(row, 16)?, |value| {
        SessionName::parse(value).ok()
    })?;
    let purpose = optional(&id, "purpose", col(row, 17)?, |value| {
        SessionPurpose::parse(value).ok()
    })?;
    // Never decoded, only wrapped: an identifier does not fail to parse the
    // way an enum's stored word can.
    let source_session_id: Option<String> = col(row, 18)?;
    let source_session_id = source_session_id.map(SessionId);
    // Never decoded either, and deliberately read as an `Option` rather than
    // with a fallback: NULL is a fact this column carries — see
    // [`SessionRecord::observed_compactions`] — and `unwrap_or(0)` here would
    // erase it at the one point every reader in the crate passes through.
    let observed_compactions: Option<i64> = col(row, 19)?;
    // Opaque, like `source_session_id`: a reference the presenting backend
    // understands, stored and returned as given. Validating its shape here
    // would teach this module what a backend's references look like, which
    // is the one thing line 762 says it must not learn.
    let presentation_ref: Option<String> = col(row, 20)?;
    // Never decoded either: an object name is forty hex characters or it is
    // not one, and this column is only ever written from
    // `GitPosition::detect`, which already refuses anything else. Read as an
    // `Option` for `observed_compactions`' reason — NULL is the fact
    // "nobody has looked yet" and no fallback may erase it.
    let last_seen_commit: Option<String> = col(row, 21)?;
    // Opaque, like `presentation_ref`: the key of a `[entitlements.<name>]`
    // table, which is whatever a person typed in their own configuration
    // file, and this module does not know what names that file may hold. Read
    // as an `Option` for `observed_compactions`' reason — NULL is the fact
    // "the serving account was never established" and no fallback may erase
    // it into a name.
    let entitlement: Option<String> = col(row, 22)?;

    Ok(SessionRecord {
        id,
        project_id: col(row, 1)?,
        harness: col(row, 2)?,
        native_session_id: col(row, 3)?,
        role,
        lifecycle,
        presentation,
        created_at: col(row, 7)?,
        last_activity_at: col(row, 8)?,
        launch_profile: col(row, 9)?,
        backend_resource: col(row, 10)?,
        model,
        pairing_class,
        protocol,
        response_profile,
        response_mechanism,
        display_name,
        purpose,
        source_session_id,
        observed_compactions,
        presentation_ref,
        last_seen_commit,
        entitlement,
    })
}

/// Decode a nullable column, keeping NULL and "a value this build cannot read"
/// apart.
///
/// NULL is `Ok(None)` — the build that wrote the row recorded nothing. A
/// present value that does not decode is an error naming the column, never
/// `None`, because the two mean opposite things and a caller that saw `None`
/// for both would report a missing fact as a deliberate absence.
fn optional<T>(
    id: &SessionId,
    column: &'static str,
    stored: Option<String>,
    decode: impl FnOnce(&str) -> Option<T>,
) -> Result<Option<T>, SessionStoreError> {
    let Some(stored) = stored else {
        return Ok(None);
    };
    match decode(&stored) {
        Some(value) => Ok(Some(value)),
        None => Err(SessionStoreError::UnknownValue {
            id: id.clone(),
            column,
            value: stored,
        }),
    }
}

/// Refuse a session whose owner is not a real harness — line 646.
///
/// The catalogue is asked, not held: the question is answered by
/// [`super::owning_harness`], one module up, because Phase 6 line 294 keeps
/// adapter knowledge out of the session store, and
/// `harness::tests::the_session_model_depends_on_no_adapter` enforces it by
/// scanning this file.
///
/// Enforced **here** rather than at the caller because this is the only
/// door: a guard in `main.rs` would be a guard `shell::start_session` does
/// not have, and one any future caller could forget; a refusal in `create`
/// is one no caller can bypass.
///
/// History: design-decisions.md, "Trims: memory and session module docs", session/store/mod.rs require_owning_harness.
fn require_owning_harness(harness: &str) -> Result<(), SessionStoreError> {
    super::owning_harness(harness)
}

mod claims;
mod context;
mod progress;
mod record;
#[cfg(test)]
mod tests;
