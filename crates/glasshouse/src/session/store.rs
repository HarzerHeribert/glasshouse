//! Glasshouse's own record of the sessions in one project.
//!
//! This is deliberately *not* a view over a harness's session files. Claude
//! Code, Codex, and the rest each keep their own history in their own format
//! in their own directory, and Glasshouse neither parses nor owns those files.
//! What it keeps here is the metadata it needs to list, resume, and reason
//! about sessions: which harness, when it started, when it was last active,
//! what role it plays, where it is presented, and what state it is in. The
//! harness's own identifier is recorded when it is known, as a nullable
//! reference — so a session survives in this table whether or not the harness
//! kept anything, and clearing a harness's history never silently deletes
//! Glasshouse's record of what happened.
//!
//! # Project isolation
//!
//! Every row carries the project identifier, and it is enforced in two places
//! on purpose:
//!
//! - **Structurally**, by SQLite triggers created in migration 2, which abort
//!   any insert or update whose `project_id` is not the identifier bound in
//!   `project_metadata`. No query in this module — or any future one — has to
//!   remember to filter, because a foreign row cannot be written at all.
//! - **At the resume boundary**, by [`SessionStore::open_for_resume`], which
//!   compares the stored identifier against the active project before handing
//!   back anything a caller could act on.
//!
//! The second check is not redundant with the first. The trigger governs what
//! this database will accept from now on; the resume check governs what
//! Glasshouse will *act on*, including rows that predate a guard, arrived
//! through a restored backup, or were written by a build whose triggers
//! differed. A resume is the one operation that takes a stored identity and
//! turns it back into a running process, so it verifies rather than assumes.

use std::fmt;
use std::sync::Arc;

use rusqlite::{Connection, OptionalExtension, Row};

use crate::database::PROJECT_ID_KEY;

/// A Glasshouse session identifier.
///
/// Distinct from any harness's native identifier, which is recorded
/// separately: Glasshouse names its own sessions so that a session remains
/// identifiable before a harness has produced an identifier, and after the
/// harness's own history is gone.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SessionId(String);

impl SessionId {
    /// Wrap an identifier that already exists, such as one read back from the
    /// database or supplied on the command line.
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for SessionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.pad(&self.0)
    }
}

/// What a session is being used for.
///
/// The orchestrator role is a tag on an ordinary session, never a separate
/// kind of thing: an orchestrator is a real harness in a real terminal that
/// the user can enter, exactly like a worker.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionRole {
    Normal,
    Orchestrator,
    Worker,
}

/// Where a session's terminal is shown.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionPresentation {
    /// Inside Glasshouse's own TUI viewport.
    Embedded,
    /// Running with no visible viewport. Still a real session the user can
    /// bring to the front — not a hidden agent loop.
    Headless,
    /// Presented by something else, such as a cmux pane.
    External,
}

/// The state of the process behind a session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionLifecycle {
    /// Spawned, not yet known to be serving.
    Starting,
    /// Working on something.
    Running,
    /// Alive with nothing in flight.
    Idle,
    /// Alive and blocked on the user, which is different from idle and is only
    /// recorded when the harness says so rather than being guessed from
    /// silence.
    WaitingForUser,
    /// The process ended without an error worth flagging.
    Stopped,
    /// The process ended badly.
    Failed,
    /// The user retired the Glasshouse record. Closing does not touch the
    /// harness's own history.
    Closed,
}

/// The coarse categories a session list has to distinguish.
///
/// Derived from [`SessionLifecycle`] plus whether a native identifier was ever
/// recorded, rather than stored as its own column: two columns that can
/// disagree about the same fact eventually do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionDisposition {
    /// A live process.
    Active,
    /// No live process, but enough recorded to start the harness again where
    /// it left off.
    Resumable,
    /// Over, with nothing to go back to.
    Closed,
    /// Over because something went wrong.
    Failed,
}

macro_rules! sql_enum {
    ($ty:ty { $($variant:ident => $text:literal),+ $(,)? }) => {
        impl $ty {
            /// The value stored in SQLite. The schema's `CHECK` constraint
            /// lists exactly these strings, so adding a variant here without
            /// a migration makes writes fail loudly rather than silently
            /// storing something readers cannot interpret.
            pub fn as_str(self) -> &'static str {
                match self {
                    $(Self::$variant => $text,)+
                }
            }

            fn from_str(value: &str) -> Option<Self> {
                match value {
                    $($text => Some(Self::$variant),)+
                    _ => None,
                }
            }
        }

        impl fmt::Display for $ty {
            /// `pad`, not `write_str`: a `Display` that writes straight to the
            /// formatter silently ignores width and alignment, so
            /// `{:<12}` in a table would produce ragged columns.
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.pad(self.as_str())
            }
        }
    };
}

sql_enum!(SessionRole {
    Normal => "normal",
    Orchestrator => "orchestrator",
    Worker => "worker",
});

sql_enum!(SessionPresentation {
    Embedded => "embedded",
    Headless => "headless",
    External => "external",
});

sql_enum!(SessionLifecycle {
    Starting => "starting",
    Running => "running",
    Idle => "idle",
    WaitingForUser => "waiting_for_user",
    Stopped => "stopped",
    Failed => "failed",
    Closed => "closed",
});

impl SessionLifecycle {
    /// True while a process is expected to exist.
    ///
    /// A full `match` rather than `matches!`, which imposes no exhaustiveness:
    /// a new variant must be classified here instead of defaulting to "not
    /// live".
    pub fn is_live(self) -> bool {
        match self {
            Self::Starting | Self::Running | Self::Idle | Self::WaitingForUser => true,
            Self::Stopped | Self::Failed | Self::Closed => false,
        }
    }
}

/// One stored session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionRecord {
    pub id: SessionId,
    /// The project this session belongs to. Always the active project for any
    /// record this module hands out.
    pub project_id: String,
    /// The harness that operates the session, as an
    /// [`crate::integrations::IntegrationId`] string.
    pub harness: String,
    /// The harness's own identifier, once one is known.
    pub native_session_id: Option<String>,
    pub role: SessionRole,
    pub lifecycle: SessionLifecycle,
    pub presentation: SessionPresentation,
    /// Seconds since the Unix epoch.
    pub created_at: i64,
    /// Seconds since the Unix epoch.
    pub last_activity_at: i64,
    /// The launch profile this session ran under, by name. `None` means a
    /// session recorded before this column existed — a different fact from a
    /// session that ran the Native profile, which is recorded as
    /// `Some("native")`. A reference only: profiles themselves are
    /// configuration (see [`crate::profile`] and [`crate::config`]), never
    /// project memory.
    pub launch_profile: Option<String>,
    /// The resolved backend resource's [`crate::profile::BackendResource::slug`],
    /// recorded for the same reason and with the same `None` meaning as
    /// `launch_profile`.
    pub backend_resource: Option<String>,
}

impl SessionRecord {
    /// Which of the four categories a session list has to separate.
    ///
    /// A stopped session counts as resumable only when a native identifier was
    /// recorded, because that identifier is the entire mechanism by which a
    /// harness is asked to continue rather than start fresh. Without one there
    /// is nothing to resume *to*, so it is reported as closed instead — better
    /// than offering the user a resume that could only ever produce a blank
    /// session wearing an old session's name.
    pub fn disposition(&self) -> SessionDisposition {
        // Every variant is listed and there is no `_` arm, so adding a
        // lifecycle state is a compile error here rather than a silent
        // classification. An earlier version led with `lifecycle if
        // lifecycle.is_live()`; a guarded arm does not count towards
        // exhaustiveness, so it needed a wildcard, and a new variant would
        // have quietly become `Active`.
        match self.lifecycle {
            SessionLifecycle::Starting
            | SessionLifecycle::Running
            | SessionLifecycle::Idle
            | SessionLifecycle::WaitingForUser => SessionDisposition::Active,
            SessionLifecycle::Failed => SessionDisposition::Failed,
            SessionLifecycle::Stopped if self.native_session_id.is_some() => {
                SessionDisposition::Resumable
            }
            SessionLifecycle::Stopped | SessionLifecycle::Closed => SessionDisposition::Closed,
        }
    }
}

/// What a caller supplies to start tracking a session.
///
/// There is no field for a credential, a token, or a provider key, and there
/// is no column for one either. Provider secrets belong in the operating
/// system's own secret storage; the project database is checked into nothing
/// and backed up casually, so it must never become a place a secret can end
/// up by accident.
#[derive(Debug, Clone)]
pub struct NewSession {
    pub harness: String,
    pub role: SessionRole,
    pub presentation: SessionPresentation,
    /// Usually `None`: most harnesses only reveal an identifier once they are
    /// running.
    pub native_session_id: Option<String>,
    /// The launch profile this session is starting under, by name. See
    /// [`SessionRecord::launch_profile`] for what `None` means.
    pub launch_profile: Option<String>,
    /// The resolved backend resource, as
    /// [`crate::profile::BackendResource::slug`]. See
    /// [`SessionRecord::backend_resource`] for what `None` means.
    pub backend_resource: Option<String>,
}

impl NewSession {
    /// A normal embedded session, which is what starting a harness from the
    /// TUI produces.
    pub fn embedded(harness: impl Into<String>) -> Self {
        Self {
            harness: harness.into(),
            role: SessionRole::Normal,
            presentation: SessionPresentation::Embedded,
            native_session_id: None,
            launch_profile: None,
            backend_resource: None,
        }
    }

    pub fn with_role(mut self, role: SessionRole) -> Self {
        self.role = role;
        self
    }

    pub fn with_presentation(mut self, presentation: SessionPresentation) -> Self {
        self.presentation = presentation;
        self
    }

    /// Record the harness's native identifier from the start.
    ///
    /// Only for a harness that lets Glasshouse *assign* one: the identifier
    /// is then known before the process exists, so a session that dies during
    /// startup still has one, and nothing has to be discovered afterwards.
    pub fn with_native_session_id(mut self, native: Option<String>) -> Self {
        self.native_session_id = native;
        self
    }

    /// Record which launch profile this session is starting under.
    pub fn with_launch_profile(mut self, launch_profile: Option<String>) -> Self {
        self.launch_profile = launch_profile;
        self
    }

    /// Record the resolved backend resource this session is starting with.
    pub fn with_backend_resource(mut self, backend_resource: Option<String>) -> Self {
        self.backend_resource = backend_resource;
        self
    }
}

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

/// Everything a resume needs, once the record has been proven to belong here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResumableSession {
    pub id: SessionId,
    pub harness: String,
    /// Never `None`: a record without one is refused as not resumable.
    pub native_session_id: String,
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
    #[error("session `{id}` stored an unrecognized {column} value `{value}`")]
    UnknownValue {
        id: SessionId,
        column: &'static str,
        value: String,
    },
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
                           launch_profile, backend_resource";

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
    pub fn open_with_clock(runtime: &crate::Runtime, clock: Clock) -> anyhow::Result<Self> {
        let conn = crate::database::open(runtime)?;
        let project_id = SessionStore::with_clock(&conn, Arc::clone(&clock))?
            .project_id()
            .to_owned();
        Ok(Self {
            conn,
            project_id,
            clock,
        })
    }

    /// The sessions in this project.
    pub fn store(&self) -> SessionStore<'_> {
        SessionStore {
            conn: &self.conn,
            project_id: self.project_id.clone(),
            clock: Arc::clone(&self.clock),
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
        })
    }

    /// The project every record in this store belongs to.
    pub fn project_id(&self) -> &str {
        &self.project_id
    }

    /// Start tracking a session.
    ///
    /// The identifier is generated by SQLite's own CSPRNG, which avoids a
    /// dependency and — more usefully — avoids the collision risk of anything
    /// derived from the clock, since sessions can be spawned in a burst.
    pub fn create(&self, new: NewSession) -> Result<SessionRecord, SessionStoreError> {
        let now = (self.clock)();
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
        };

        self.conn
            .execute(
                "INSERT INTO sessions (id, project_id, harness, native_session_id, \
                 role, lifecycle, presentation, created_at, last_activity_at, \
                 launch_profile, backend_resource) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
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

    /// Move a session to a new lifecycle state, which also counts as activity.
    pub fn set_lifecycle(
        &self,
        id: &SessionId,
        lifecycle: SessionLifecycle,
    ) -> Result<SessionRecord, SessionStoreError> {
        self.update(
            id,
            "UPDATE sessions SET lifecycle = ?2, last_activity_at = ?3 WHERE id = ?1",
            rusqlite::params![id.as_str(), lifecycle.as_str(), (self.clock)()],
            "update a session's lifecycle",
        )
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
}

/// Build a record from a row, turning an unrecognized enum string into a
/// typed error rather than a panic or a silent default.
fn read_record(row: &Row<'_>) -> Result<SessionRecord, SessionStoreError> {
    let id = SessionId(row.get_unwrap::<_, String>(0));

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

    let role_text: String = row.get_unwrap(4);
    let lifecycle_text: String = row.get_unwrap(5);
    let presentation_text: String = row.get_unwrap(6);

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

    Ok(SessionRecord {
        id,
        project_id: row.get_unwrap(1),
        harness: row.get_unwrap(2),
        native_session_id: row.get_unwrap(3),
        role,
        lifecycle,
        presentation,
        created_at: row.get_unwrap(7),
        last_activity_at: row.get_unwrap(8),
        launch_profile: row.get_unwrap(9),
        backend_resource: row.get_unwrap(10),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Cli, Runtime};
    use clap::Parser;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicI64, Ordering};

    /// A bootstrapped project with an open connection to its database, which
    /// is what every caller of this module will have.
    struct Fixture {
        base: PathBuf,
        runtime: Runtime,
        conn: Connection,
    }

    impl Fixture {
        fn new(base: &Path, name: &str) -> Self {
            let root = base.join("workspace").join(name);
            std::fs::create_dir_all(root.join(".git")).unwrap();
            let root = std::fs::canonicalize(&root).unwrap();
            let runtime = bootstrap_at(base, &root);
            let conn = crate::database::open(&runtime).unwrap();
            Self {
                base: base.to_path_buf(),
                runtime,
                conn,
            }
        }

        fn store(&self) -> SessionStore<'_> {
            SessionStore::new(&self.conn).unwrap()
        }

        /// A store whose clock returns `start`, then `start + step` on each
        /// later call, so a test can assert exact timestamps.
        fn store_with_ticking_clock(&self, start: i64, step: i64) -> SessionStore<'_> {
            let next = AtomicI64::new(start);
            let clock: Clock = Arc::new(move || next.fetch_add(step, Ordering::SeqCst));
            SessionStore::with_clock(&self.conn, clock).unwrap()
        }

        /// Reopen the database the way a later launch would, proving what is
        /// on disk rather than what is in memory.
        fn reopen(&self) -> Connection {
            crate::database::open(&self.runtime).unwrap()
        }

        fn project_id(&self) -> &str {
            self.runtime.project().id().as_str()
        }

        /// A second project sharing this machine's data/config root.
        fn sibling(&self, name: &str) -> Runtime {
            let root = self.base.join("workspace").join(name);
            std::fs::create_dir_all(root.join(".git")).unwrap();
            let root = std::fs::canonicalize(&root).unwrap();
            bootstrap_at(&self.base, &root)
        }
    }

    fn bootstrap_at(base: &Path, root: &Path) -> Runtime {
        let cli = Cli::try_parse_from([
            "glasshouse",
            "--data-dir",
            base.join("data").to_str().unwrap(),
            "--config-dir",
            base.join("config").to_str().unwrap(),
        ])
        .unwrap();
        crate::bootstrap(&cli, root).unwrap()
    }

    /// Insert a row directly, bypassing [`SessionStore`] entirely.
    ///
    /// Used to plant a row belonging to another project, which is exactly what
    /// the schema's trigger exists to prevent — so the trigger is dropped for
    /// the insert and restored afterwards. That models the real threat the
    /// resume check answers: a row that reached the file by some route the
    /// trigger never saw, such as a restored backup or an older build.
    fn plant_foreign_row(conn: &Connection, id: &str, project_id: &str, native: Option<&str>) {
        conn.execute_batch("DROP TRIGGER sessions_reject_foreign_project_insert;")
            .unwrap();
        conn.execute(
            "INSERT INTO sessions (id, project_id, harness, native_session_id, role, \
             lifecycle, presentation, created_at, last_activity_at) \
             VALUES (?1, ?2, 'claude-code', ?3, 'normal', 'stopped', 'embedded', 10, 20)",
            rusqlite::params![id, project_id, native],
        )
        .unwrap();
        conn.execute_batch(
            "CREATE TRIGGER sessions_reject_foreign_project_insert
             BEFORE INSERT ON sessions
             FOR EACH ROW
             WHEN NEW.project_id IS NOT (
                 SELECT value FROM project_metadata WHERE key = 'project_id'
             )
             BEGIN
                 SELECT RAISE(ABORT, 'session belongs to a different project');
             END;",
        )
        .unwrap();
    }

    // ---------------------------------------------------------------
    // Phase 1 line 90 — reject a cross-project resume.
    // ---------------------------------------------------------------

    /// The capability, stated as a contract: given a session record whose
    /// project identifier differs from the active project's, when a caller
    /// tries to resume it, Glasshouse refuses and names both projects, while
    /// leaving the record untouched.
    #[test]
    fn resuming_a_session_belonging_to_another_project_is_refused() {
        let tmp = tempfile::tempdir().unwrap();
        let fixture = Fixture::new(tmp.path(), "alpha");
        let other = fixture.sibling("beta");
        let other_id = other.project().id().as_str();
        assert_ne!(
            other_id,
            fixture.project_id(),
            "fixture must use two projects"
        );

        plant_foreign_row(&fixture.conn, "planted", other_id, Some("native-1"));

        let store = fixture.store();
        let error = store
            .open_for_resume(&SessionId::new("planted"))
            .expect_err("a session from another project must never be resumable");

        match &error {
            SessionStoreError::ForeignProject {
                id,
                expected,
                actual,
            } => {
                assert_eq!(id.as_str(), "planted");
                assert_eq!(expected, fixture.project_id());
                assert_eq!(actual, other_id);
            }
            other => panic!("expected ForeignProject, got {other:?}"),
        }

        // Naming both projects is the point: "not found" would send the user
        // hunting for a session that is sitting right there.
        let message = error.to_string();
        assert!(
            message.contains(other_id),
            "message must name the owning project: {message}"
        );
        assert!(
            message.contains(fixture.project_id()),
            "message must name the active project: {message}"
        );

        // Refusing is not deleting. The record is still exactly as planted.
        let still_there: String = fixture
            .conn
            .query_row(
                "SELECT project_id FROM sessions WHERE id = 'planted'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(still_there, other_id);
    }

    /// The structural half: the database itself refuses to store a session
    /// belonging to another project, so no future query has to remember to
    /// filter by project.
    #[test]
    fn the_database_refuses_to_store_a_session_from_another_project() {
        let tmp = tempfile::tempdir().unwrap();
        let fixture = Fixture::new(tmp.path(), "alpha");
        let other = fixture.sibling("beta");

        let result = fixture.conn.execute(
            "INSERT INTO sessions (id, project_id, harness, role, lifecycle, \
             presentation, created_at, last_activity_at) \
             VALUES ('x', ?1, 'claude-code', 'normal', 'starting', 'embedded', 1, 1)",
            [other.project().id().as_str()],
        );

        let Err(error) = result else {
            panic!("the trigger must abort an insert for another project");
        };
        assert!(
            error.to_string().contains("different project"),
            "unexpected error: {error}"
        );
    }

    /// Same guard on the update path: a row cannot be *moved* to another
    /// project after the fact.
    #[test]
    fn a_stored_session_cannot_be_reassigned_to_another_project() {
        let tmp = tempfile::tempdir().unwrap();
        let fixture = Fixture::new(tmp.path(), "alpha");
        let other = fixture.sibling("beta");
        let record = fixture
            .store()
            .create(NewSession::embedded("claude-code"))
            .unwrap();

        let result = fixture.conn.execute(
            "UPDATE sessions SET project_id = ?2 WHERE id = ?1",
            rusqlite::params![record.id.as_str(), other.project().id().as_str()],
        );

        let Err(error) = result else {
            panic!("the trigger must abort a reassignment");
        };
        assert!(
            error.to_string().contains("different project"),
            "unexpected error: {error}"
        );
    }

    /// The guard fails closed: with no binding row to compare against, the
    /// trigger aborts rather than letting the write through. `<>` against a
    /// NULL subquery would have evaluated to NULL and allowed it.
    #[test]
    fn a_session_write_is_refused_when_the_project_binding_is_missing() {
        let tmp = tempfile::tempdir().unwrap();
        let fixture = Fixture::new(tmp.path(), "alpha");
        let project_id = fixture.project_id().to_owned();

        fixture
            .conn
            .execute("DELETE FROM project_metadata WHERE key = 'project_id'", [])
            .unwrap();

        let result = fixture.conn.execute(
            "INSERT INTO sessions (id, project_id, harness, role, lifecycle, \
             presentation, created_at, last_activity_at) \
             VALUES ('x', ?1, 'claude-code', 'normal', 'starting', 'embedded', 1, 1)",
            [&project_id],
        );
        assert!(
            result.is_err(),
            "an unbound database must accept no session rows"
        );
    }

    /// The permitted case, so the refusals above are not simply "resume never
    /// works".
    #[test]
    fn a_stopped_session_of_this_project_can_be_resumed() {
        let tmp = tempfile::tempdir().unwrap();
        let fixture = Fixture::new(tmp.path(), "alpha");
        let store = fixture.store();

        let record = store.create(NewSession::embedded("codex")).unwrap();
        store
            .set_native_session_id(&record.id, "thread-77")
            .unwrap();
        store
            .set_lifecycle(&record.id, SessionLifecycle::Stopped)
            .unwrap();

        let resumable = store.open_for_resume(&record.id).unwrap();
        assert_eq!(
            resumable,
            ResumableSession {
                id: record.id,
                harness: "codex".to_owned(),
                native_session_id: "thread-77".to_owned(),
            }
        );
    }

    #[test]
    fn resuming_an_unknown_session_says_so() {
        let tmp = tempfile::tempdir().unwrap();
        let fixture = Fixture::new(tmp.path(), "alpha");
        let error = fixture
            .store()
            .open_for_resume(&SessionId::new("nope"))
            .expect_err("an unknown session cannot be resumed");
        assert!(
            matches!(error, SessionStoreError::NotFound { .. }),
            "got {error:?}"
        );
    }

    #[test]
    fn a_live_session_is_not_resumable() {
        let tmp = tempfile::tempdir().unwrap();
        let fixture = Fixture::new(tmp.path(), "alpha");
        let store = fixture.store();
        let record = store.create(NewSession::embedded("claude-code")).unwrap();
        store.set_native_session_id(&record.id, "native-1").unwrap();
        store
            .set_lifecycle(&record.id, SessionLifecycle::Running)
            .unwrap();

        let error = store
            .open_for_resume(&record.id)
            .expect_err("a running session is not resumable");
        assert!(
            matches!(&error, SessionStoreError::NotResumable { disposition, .. } if *disposition == "still running"),
            "got {error:?}"
        );
    }

    /// Without a native identifier there is nothing to resume *to*, so
    /// offering a resume would produce a blank session wearing an old name.
    #[test]
    fn a_stopped_session_with_no_native_identifier_is_not_resumable() {
        let tmp = tempfile::tempdir().unwrap();
        let fixture = Fixture::new(tmp.path(), "alpha");
        let store = fixture.store();
        let record = store.create(NewSession::embedded("claude-code")).unwrap();
        store
            .set_lifecycle(&record.id, SessionLifecycle::Stopped)
            .unwrap();

        let error = store
            .open_for_resume(&record.id)
            .expect_err("nothing to resume to");
        assert!(
            matches!(error, SessionStoreError::NotResumable { .. }),
            "got {error:?}"
        );
    }

    // ---------------------------------------------------------------
    // Phase 2 line 183 — metadata independent of native session files.
    // ---------------------------------------------------------------

    /// The record is Glasshouse's own: it is complete before the harness has
    /// produced any identifier, it survives a reopen, and nothing about it is
    /// read from a harness's files.
    #[test]
    fn a_session_is_recorded_and_survives_a_reopen_with_no_harness_involved() {
        let tmp = tempfile::tempdir().unwrap();
        let fixture = Fixture::new(tmp.path(), "alpha");

        let created = fixture
            .store_with_ticking_clock(1_700_000_000, 0)
            .create(
                NewSession::embedded("claude-code")
                    .with_role(SessionRole::Orchestrator)
                    .with_presentation(SessionPresentation::External),
            )
            .unwrap();
        assert!(
            created.native_session_id.is_none(),
            "no harness has spoken yet"
        );

        // A different connection to the same file, as a later launch makes.
        let reopened = fixture.reopen();
        let store = SessionStore::new(&reopened).unwrap();
        let read_back = store
            .get(&created.id)
            .unwrap()
            .expect("the record is on disk");
        assert_eq!(read_back, created);
    }

    // ---------------------------------------------------------------
    // Phase 2 line 184 — Glasshouse ID <-> native harness ID mapping.
    // ---------------------------------------------------------------

    #[test]
    fn a_native_session_identifier_can_be_attached_later_and_read_back() {
        let tmp = tempfile::tempdir().unwrap();
        let fixture = Fixture::new(tmp.path(), "alpha");
        let store = fixture.store();

        let record = store.create(NewSession::embedded("claude-code")).unwrap();
        let updated = store.set_native_session_id(&record.id, "sess-abc").unwrap();

        assert_eq!(updated.native_session_id.as_deref(), Some("sess-abc"));
        assert_eq!(
            updated.id, record.id,
            "the Glasshouse identifier never changes"
        );
        assert_eq!(
            store
                .get(&record.id)
                .unwrap()
                .unwrap()
                .native_session_id
                .as_deref(),
            Some("sess-abc")
        );
    }

    /// A mapping, not an annotation: one native session cannot be claimed by
    /// two Glasshouse sessions, or a resume would not know which to continue.
    #[test]
    fn one_native_session_cannot_map_to_two_glasshouse_sessions() {
        let tmp = tempfile::tempdir().unwrap();
        let fixture = Fixture::new(tmp.path(), "alpha");
        let store = fixture.store();

        let first = store.create(NewSession::embedded("claude-code")).unwrap();
        let second = store.create(NewSession::embedded("claude-code")).unwrap();
        store.set_native_session_id(&first.id, "shared").unwrap();

        let error = store
            .set_native_session_id(&second.id, "shared")
            .expect_err("the same native session must not be claimed twice");
        assert!(
            matches!(error, SessionStoreError::Sql { .. }),
            "got {error:?}"
        );
    }

    /// Scoped per harness, so two harnesses that happen to use the same
    /// identifier format do not collide.
    #[test]
    fn two_harnesses_may_use_the_same_native_identifier() {
        let tmp = tempfile::tempdir().unwrap();
        let fixture = Fixture::new(tmp.path(), "alpha");
        let store = fixture.store();

        let claude = store.create(NewSession::embedded("claude-code")).unwrap();
        let codex = store.create(NewSession::embedded("codex")).unwrap();
        store.set_native_session_id(&claude.id, "1").unwrap();
        store.set_native_session_id(&codex.id, "1").unwrap();

        assert_eq!(store.list().unwrap().len(), 2);
    }

    /// Sessions awaiting a native identifier must coexist freely.
    ///
    /// SQLite's unique indexes treat NULLs as distinct, so this holds today
    /// without help from the index's `WHERE` clause. The test earns its place
    /// by pinning the behaviour against the obvious future refactor: making
    /// the column `NOT NULL DEFAULT ''` would make every unidentified session
    /// collide with the next one.
    #[test]
    fn many_sessions_may_have_no_native_identifier_at_once() {
        let tmp = tempfile::tempdir().unwrap();
        let fixture = Fixture::new(tmp.path(), "alpha");
        let store = fixture.store();

        for _ in 0..3 {
            store.create(NewSession::embedded("claude-code")).unwrap();
        }
        assert_eq!(store.list().unwrap().len(), 3);
    }

    // ---------------------------------------------------------------
    // Phase 2 line 185 — harness, times, role, lifecycle, project id.
    // ---------------------------------------------------------------

    /// Every field the capability names, asserted by value rather than by
    /// "it round-trips".
    #[test]
    fn every_required_field_is_persisted() {
        let tmp = tempfile::tempdir().unwrap();
        let fixture = Fixture::new(tmp.path(), "alpha");

        let record = fixture
            .store_with_ticking_clock(1_600_000_000, 0)
            .create(NewSession::embedded("codex").with_role(SessionRole::Worker))
            .unwrap();

        assert_eq!(record.harness, "codex");
        assert_eq!(record.role, SessionRole::Worker);
        assert_eq!(record.lifecycle, SessionLifecycle::Starting);
        assert_eq!(record.project_id, fixture.project_id());
        assert_eq!(record.created_at, 1_600_000_000);
        assert_eq!(record.last_activity_at, 1_600_000_000);
        assert!(!record.id.as_str().is_empty());
    }

    #[test]
    fn every_role_and_lifecycle_value_round_trips() {
        let tmp = tempfile::tempdir().unwrap();
        let fixture = Fixture::new(tmp.path(), "alpha");
        let store = fixture.store();

        for role in [
            SessionRole::Normal,
            SessionRole::Orchestrator,
            SessionRole::Worker,
        ] {
            let record = store
                .create(NewSession::embedded("claude-code").with_role(role))
                .unwrap();
            assert_eq!(store.get(&record.id).unwrap().unwrap().role, role);

            for lifecycle in [
                SessionLifecycle::Starting,
                SessionLifecycle::Running,
                SessionLifecycle::Idle,
                SessionLifecycle::WaitingForUser,
                SessionLifecycle::Stopped,
                SessionLifecycle::Failed,
                SessionLifecycle::Closed,
            ] {
                store.set_lifecycle(&record.id, lifecycle).unwrap();
                assert_eq!(store.get(&record.id).unwrap().unwrap().lifecycle, lifecycle);
            }
        }
    }

    /// Activity time is what a session list sorts and ages by, so it has to
    /// move independently of creation time.
    #[test]
    fn activity_time_advances_while_creation_time_stays_put() {
        let tmp = tempfile::tempdir().unwrap();
        let fixture = Fixture::new(tmp.path(), "alpha");
        let store = fixture.store_with_ticking_clock(1_000, 10);

        let record = store.create(NewSession::embedded("claude-code")).unwrap();
        assert_eq!(record.created_at, 1_000);

        let touched = store.touch(&record.id).unwrap();
        assert_eq!(touched.created_at, 1_000, "creation time is immutable");
        assert_eq!(touched.last_activity_at, 1_010);

        let moved = store
            .set_lifecycle(&record.id, SessionLifecycle::Running)
            .unwrap();
        assert_eq!(
            moved.last_activity_at, 1_020,
            "a state change counts as activity"
        );
        assert_eq!(moved.created_at, 1_000);
    }

    #[test]
    fn sessions_are_listed_most_recently_active_first() {
        let tmp = tempfile::tempdir().unwrap();
        let fixture = Fixture::new(tmp.path(), "alpha");
        let store = fixture.store_with_ticking_clock(500, 10);

        let first = store.create(NewSession::embedded("claude-code")).unwrap();
        let second = store.create(NewSession::embedded("codex")).unwrap();
        store.touch(&first.id).unwrap();

        let listed: Vec<_> = store.list().unwrap().into_iter().map(|r| r.id).collect();
        assert_eq!(listed, vec![first.id, second.id]);
    }

    #[test]
    fn touching_an_unknown_session_reports_it_missing_rather_than_inventing_one() {
        let tmp = tempfile::tempdir().unwrap();
        let fixture = Fixture::new(tmp.path(), "alpha");
        let error = fixture
            .store()
            .touch(&SessionId::new("ghost"))
            .expect_err("no such session");
        assert!(
            matches!(error, SessionStoreError::NotFound { .. }),
            "got {error:?}"
        );
        assert_eq!(
            fixture.store().list().unwrap().len(),
            0,
            "nothing was created"
        );
    }

    // ---------------------------------------------------------------
    // Phase 2 line 186 — presentation mode.
    // ---------------------------------------------------------------

    #[test]
    fn every_presentation_mode_is_persisted() {
        let tmp = tempfile::tempdir().unwrap();
        let fixture = Fixture::new(tmp.path(), "alpha");
        let store = fixture.store();

        for presentation in [
            SessionPresentation::Embedded,
            SessionPresentation::Headless,
            SessionPresentation::External,
        ] {
            let record = store
                .create(NewSession::embedded("claude-code").with_presentation(presentation))
                .unwrap();
            assert_eq!(
                store.get(&record.id).unwrap().unwrap().presentation,
                presentation,
                "presentation must survive a round trip"
            );
        }
    }

    // ---------------------------------------------------------------
    // Phase 2 line 187 — active / resumable / closed / failed.
    // ---------------------------------------------------------------

    #[test]
    fn the_four_dispositions_are_distinguishable_from_stored_metadata() {
        let tmp = tempfile::tempdir().unwrap();
        let fixture = Fixture::new(tmp.path(), "alpha");
        let store = fixture.store();

        let make = |lifecycle: SessionLifecycle, native: Option<&str>| {
            let record = store.create(NewSession::embedded("claude-code")).unwrap();
            if let Some(native) = native {
                store.set_native_session_id(&record.id, native).unwrap();
            }
            store.set_lifecycle(&record.id, lifecycle).unwrap()
        };

        assert_eq!(
            make(SessionLifecycle::Starting, None).disposition(),
            SessionDisposition::Active
        );
        assert_eq!(
            make(SessionLifecycle::Running, None).disposition(),
            SessionDisposition::Active
        );
        assert_eq!(
            make(SessionLifecycle::Idle, None).disposition(),
            SessionDisposition::Active
        );
        assert_eq!(
            make(SessionLifecycle::WaitingForUser, None).disposition(),
            SessionDisposition::Active
        );
        assert_eq!(
            make(SessionLifecycle::Stopped, Some("n1")).disposition(),
            SessionDisposition::Resumable
        );
        assert_eq!(
            make(SessionLifecycle::Stopped, None).disposition(),
            SessionDisposition::Closed,
            "stopped with nothing to resume to is over, not resumable"
        );
        assert_eq!(
            make(SessionLifecycle::Closed, None).disposition(),
            SessionDisposition::Closed
        );
        assert_eq!(
            make(SessionLifecycle::Failed, Some("n2")).disposition(),
            SessionDisposition::Failed,
            "a failure stays visible as a failure even with a native id"
        );
    }

    // ---------------------------------------------------------------
    // Phase 2 line 188 — no provider credentials in the project database.
    // ---------------------------------------------------------------

    /// The whole schema, locked to an explicit list.
    ///
    /// Fuzzy name matching would be worse than useless here: `project_metadata`
    /// legitimately has a column called `key`, and a credential column could
    /// just as easily be called `value`. Pinning the exact schema instead means
    /// any new column fails this test until someone updates the list, and that
    /// is the moment to ask what the new column can hold.
    ///
    /// **What this test can and cannot prove.** It proves no column exists
    /// whose *purpose* is to hold a credential, and that adding one is a
    /// deliberate act somebody has to write down here. It does not prove a
    /// credential can never be stored: `memories.subject` and `memories.body`
    /// are free text, and free text can hold anything.
    ///
    /// That gap is real and is not closed by widening this list. It is closed
    /// on the **producer** side — Phase 21's memory extractor must never be
    /// fed, and must never emit, credential material, and that is an explicit
    /// acceptance condition of Phase 21 rather than something inherited by
    /// assumption. Recorded when migration 4 added the memory tables and the
    /// worker adding them declined to certify otherwise.
    ///
    /// **Migration 6's twelve new columns, and the answer this test exists to
    /// force.** Two of them are integers: `source_event_first` and
    /// `source_event_last` are positions in `lifecycle_events.seq`, and an
    /// `INTEGER` column cannot hold a credential — there is no question to
    /// ask about those two.
    ///
    /// The other ten **can**. `rationale`, `problem`, `assumptions`,
    /// `scale_assumptions`, `security_assumptions`,
    /// `compatibility_assumptions`, `operational_assumptions`, `evidence`
    /// and `source_excerpt` are free text a producer chooses, exactly like
    /// `subject` and `body`, and `source_excerpt` is the sharpest of the ten
    /// because it is *verbatim session text* rather than a model's
    /// paraphrase — a decision quoted from a session that discussed
    /// configuring a provider is precisely where a key would appear.
    /// (`project_phase` is the eleventh and the one exception: migration 6
    /// gives it a `CHECK` over five fixed words, so it is not free text.)
    ///
    /// So the answer for migration 6 is the same as migration 4's and it is
    /// written down rather than inherited: **this test does not certify
    /// them.** The control is on the producer side, and it covers the new
    /// fields *without being extended*, which is the property worth having:
    /// `memory::extract::schema::judge` screens each emitted element whole,
    /// over its serialized text, **before reading any field of it**, so a
    /// field the contract gained yesterday is screened today. That ordering
    /// is why the coverage is automatic, and it is a Phase 21 acceptance
    /// condition rather than a convention.
    ///
    /// **Migration 5's twenty new columns, judged one at a time.** Nineteen
    /// hold a value drawn from a fixed set or from Glasshouse's own machinery
    /// — a kind, an origin, an exit code, a signal name, a backend resource
    /// slug, an integration slug, a harness event name from an adapter's own
    /// constant list — and none of them is free text a caller chooses.
    /// `checkpoints.document` is the twentieth and it **is** free text, for
    /// the same reason `memories.body` is: a person writes a handoff. The same
    /// limit therefore applies to it and is recorded here rather than glossed
    /// — it is closed on the producer side, by whoever authors a checkpoint,
    /// and this test does not and cannot certify it.
    #[test]
    fn the_project_database_schema_has_nowhere_to_put_a_credential() {
        let tmp = tempfile::tempdir().unwrap();
        let fixture = Fixture::new(tmp.path(), "alpha");

        let mut statement = fixture
            .conn
            .prepare(
                "SELECT m.name, p.name FROM sqlite_master m \
                 JOIN pragma_table_info(m.name) p \
                 WHERE m.type = 'table' AND m.name NOT LIKE 'sqlite_%' \
                 ORDER BY m.name, p.cid",
            )
            .unwrap();
        let columns: Vec<String> = statement
            .query_map([], |row| {
                Ok(format!(
                    "{}.{}",
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?
                ))
            })
            .unwrap()
            .map(Result::unwrap)
            .collect();

        assert_eq!(
            columns,
            vec![
                "checkpoints.id",
                "checkpoints.project_id",
                "checkpoints.session_id",
                "checkpoints.created_at",
                "checkpoints.reason",
                "checkpoints.document",
                "lifecycle_events.seq",
                "lifecycle_events.project_id",
                "lifecycle_events.session_id",
                "lifecycle_events.at",
                "lifecycle_events.kind",
                "lifecycle_events.turn_outcome",
                "lifecycle_events.origin",
                "lifecycle_events.bytes",
                "lifecycle_events.exit_code",
                "lifecycle_events.exit_signal",
                "lifecycle_events.resource",
                "lifecycle_events.gateway_reason",
                "lifecycle_events.gateway_provider",
                "lifecycle_events.gateway_model",
                "lifecycle_events.gateway_cause",
                "lifecycle_events.observed_harness",
                "lifecycle_events.observed_event",
                "memories.id",
                "memories.project_id",
                "memories.kind",
                "memories.authority",
                "memories.status",
                "memories.subject",
                "memories.body",
                "memories.source_session_id",
                "memories.source_commit",
                "memories.superseded_by",
                "memories.created_at",
                "memories.updated_at",
                "memories.source_event_first",
                "memories.source_event_last",
                "memories.rationale",
                "memories.project_phase",
                "memories.problem",
                "memories.assumptions",
                "memories.scale_assumptions",
                "memories.security_assumptions",
                "memories.compatibility_assumptions",
                "memories.operational_assumptions",
                "memories.evidence",
                "memories.source_excerpt",
                "memories_fts.subject",
                "memories_fts.body",
                "memories_fts.rationale",
                "memories_fts_config.k",
                "memories_fts_config.v",
                "memories_fts_data.id",
                "memories_fts_data.block",
                "memories_fts_docsize.id",
                "memories_fts_docsize.sz",
                "memories_fts_idx.segid",
                "memories_fts_idx.term",
                "memories_fts_idx.pgno",
                "project_metadata.key",
                "project_metadata.value",
                "schema_migrations.version",
                "sessions.id",
                "sessions.project_id",
                "sessions.harness",
                "sessions.native_session_id",
                "sessions.role",
                "sessions.lifecycle",
                "sessions.presentation",
                "sessions.created_at",
                "sessions.last_activity_at",
                "sessions.launch_profile",
                "sessions.backend_resource",
            ],
            "the project database schema changed; confirm the new column cannot \
             hold a provider credential before updating this list"
        );
    }

    // ---------------------------------------------------------------
    // Phase 9A — a launch profile is a reference here, never a definition.
    // ---------------------------------------------------------------

    /// The database schema has exactly a reference column for the profile a
    /// session ran under, and no table defining what a profile *is* —
    /// profiles are configuration, resolved in `crate::config`/
    /// `crate::profile`, never project memory.
    #[test]
    fn no_launch_profile_definition_is_stored_in_the_project_database() {
        let tmp = tempfile::tempdir().unwrap();
        let fixture = Fixture::new(tmp.path(), "alpha");

        let mut statement = fixture
            .conn
            .prepare(
                "SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%' \
                 ORDER BY name",
            )
            .unwrap();
        let tables: Vec<String> = statement
            .query_map([], |row| row.get(0))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        assert_eq!(
            tables,
            vec![
                "checkpoints",
                "lifecycle_events",
                "memories",
                "memories_fts",
                "memories_fts_config",
                "memories_fts_data",
                "memories_fts_docsize",
                "memories_fts_idx",
                "project_metadata",
                "schema_migrations",
                "sessions",
            ],
            "no table defining launch profiles may exist in the project database"
        );

        let record = fixture
            .store()
            .create(
                NewSession::embedded("claude-code")
                    .with_launch_profile(Some("native".to_owned()))
                    .with_backend_resource(Some("native".to_owned())),
            )
            .unwrap();
        assert_eq!(record.launch_profile.as_deref(), Some("native"));
        assert_eq!(record.backend_resource.as_deref(), Some("native"));

        let read_back = fixture.store().get(&record.id).unwrap().unwrap();
        assert_eq!(read_back.launch_profile.as_deref(), Some("native"));
        assert_eq!(read_back.backend_resource.as_deref(), Some("native"));
    }

    /// Building a session without naming a profile leaves both columns NULL
    /// rather than inventing a value — the same "None means not recorded"
    /// rule the rest of this table already follows for `native_session_id`.
    #[test]
    fn a_session_with_no_recorded_profile_leaves_both_columns_null() {
        let tmp = tempfile::tempdir().unwrap();
        let fixture = Fixture::new(tmp.path(), "alpha");
        let record = fixture
            .store()
            .create(NewSession::embedded("claude-code"))
            .unwrap();
        assert_eq!(record.launch_profile, None);
        assert_eq!(record.backend_resource, None);
    }

    /// An existing version-2 database gains the two launch-profile columns on
    /// the next launch, with every existing session's data intact and both
    /// new columns `NULL` — a session recorded before this migration ran is a
    /// different fact from one that ran the Native profile, so NULL must
    /// stay NULL rather than default to `"native"`.
    #[test]
    fn upgrading_a_version_2_database_preserves_every_existing_session() {
        let tmp = tempfile::tempdir().unwrap();
        let fixture = Fixture::new(tmp.path(), "alpha");
        let store = fixture.store();

        let record = store
            .create(NewSession::embedded("claude-code").with_role(SessionRole::Worker))
            .unwrap();
        store.set_native_session_id(&record.id, "native-1").unwrap();
        store
            .set_lifecycle(&record.id, SessionLifecycle::Stopped)
            .unwrap();

        // Roll the database back to what version 2 left behind: drop what
        // migrations 3 and 4 added, and forget that they ran.
        //
        // `DELETE ... WHERE version = 3` is what this said while 3 was the
        // highest migration, and it stopped working the moment 4 existed. The
        // runner resumes from `MAX(version)`, so deleting only row 3 leaves a
        // *hole* — max is still 4, nothing re-applies, and the test failed
        // later and confusingly with "no such column: launch_profile". Roll
        // back a contiguous range, or do not roll back at all.
        //
        // Everything a later migration created has to go with the rows that
        // record it, or the re-run fails on `table … already exists` instead —
        // which is the same trap wearing the opposite coat, and is exactly how
        // migration 5 announced itself here.
        fixture
            .conn
            .execute_batch(
                "ALTER TABLE sessions DROP COLUMN launch_profile;
                 ALTER TABLE sessions DROP COLUMN backend_resource;
                 DROP TABLE IF EXISTS memories_fts;
                 DROP TABLE IF EXISTS memories;
                 DROP TABLE IF EXISTS lifecycle_events;
                 DROP TABLE IF EXISTS checkpoints;
                 DELETE FROM schema_migrations WHERE version >= 3;",
            )
            .unwrap();

        let reopened = fixture.reopen();
        let version: i64 = reopened
            .query_row("SELECT MAX(version) FROM schema_migrations", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(
            version, 7,
            "the launch must have applied migrations 3, 4, 5, 6 and 7"
        );

        let migrated_store = SessionStore::new(&reopened).unwrap();
        let migrated = migrated_store
            .get(&record.id)
            .unwrap()
            .expect("the pre-migration session must survive");
        assert_eq!(migrated.id, record.id);
        assert_eq!(migrated.harness, "claude-code");
        assert_eq!(migrated.role, SessionRole::Worker);
        assert_eq!(migrated.native_session_id.as_deref(), Some("native-1"));
        assert_eq!(migrated.lifecycle, SessionLifecycle::Stopped);
        assert_eq!(migrated.created_at, record.created_at);
        assert_eq!(
            migrated.launch_profile, None,
            "a pre-migration session has no recorded profile — never a guessed default"
        );
        assert_eq!(migrated.backend_resource, None);
    }

    /// `project_metadata` is a key/value table, which is the one place a
    /// credential could be smuggled in without a schema change. Its keys are
    /// pinned too.
    #[test]
    fn project_metadata_holds_only_the_project_identifier() {
        let tmp = tempfile::tempdir().unwrap();
        let fixture = Fixture::new(tmp.path(), "alpha");
        fixture
            .store()
            .create(NewSession::embedded("claude-code"))
            .unwrap();

        let mut statement = fixture
            .conn
            .prepare("SELECT key FROM project_metadata ORDER BY key")
            .unwrap();
        let keys: Vec<String> = statement
            .query_map([], |row| row.get(0))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        assert_eq!(keys, vec!["project_id"]);
    }

    // ---------------------------------------------------------------
    // Storage-layer integrity.
    // ---------------------------------------------------------------

    /// The `CHECK` constraints are the reason readers can trust the enum
    /// columns, so verify they actually reject nonsense.
    #[test]
    fn the_schema_rejects_enum_values_it_does_not_define() {
        let tmp = tempfile::tempdir().unwrap();
        let fixture = Fixture::new(tmp.path(), "alpha");
        let project_id = fixture.project_id().to_owned();

        for (column, bad) in [
            ("role", "admin"),
            ("lifecycle", "probably_fine"),
            ("presentation", "invisible"),
        ] {
            let mut values = std::collections::HashMap::from([
                ("role", "normal"),
                ("lifecycle", "starting"),
                ("presentation", "embedded"),
            ]);
            values.insert(column, bad);

            let result = fixture.conn.execute(
                "INSERT INTO sessions (id, project_id, harness, role, lifecycle, \
                 presentation, created_at, last_activity_at) \
                 VALUES (?1, ?2, 'claude-code', ?3, ?4, ?5, 1, 1)",
                rusqlite::params![
                    format!("bad-{column}"),
                    &project_id,
                    values["role"],
                    values["lifecycle"],
                    values["presentation"],
                ],
            );
            assert!(result.is_err(), "`{column}` must reject `{bad}`");
        }
    }

    /// A value that somehow got past the constraint must surface as a typed
    /// error naming the column, never a panic or a silent default.
    #[test]
    fn an_unrecognized_stored_enum_value_is_reported_rather_than_guessed() {
        let tmp = tempfile::tempdir().unwrap();
        let fixture = Fixture::new(tmp.path(), "alpha");
        let record = fixture
            .store()
            .create(NewSession::embedded("claude-code"))
            .unwrap();

        // Rebuild the table without its CHECK constraints to model a database
        // written by a future build that knows a lifecycle this one does not.
        fixture
            .conn
            .execute_batch(
                "PRAGMA writable_schema = ON;
                 UPDATE sqlite_master
                    SET sql = replace(sql, \"CHECK (lifecycle IN ('starting', 'running', 'idle',\
\n                                 'waiting_for_user', 'stopped', 'failed',\
\n                                 'closed'))\", '')
                  WHERE type = 'table' AND name = 'sessions';
                 PRAGMA writable_schema = OFF;",
            )
            .unwrap();
        let reopened = fixture.reopen();
        reopened
            .execute(
                "UPDATE sessions SET lifecycle = 'hibernating' WHERE id = ?1",
                [record.id.as_str()],
            )
            .unwrap();

        let store = SessionStore::new(&reopened).unwrap();
        let error = store
            .get(&record.id)
            .expect_err("an unknown lifecycle must not be guessed");
        match error {
            SessionStoreError::UnknownValue { column, value, .. } => {
                assert_eq!(column, "lifecycle");
                assert_eq!(value, "hibernating");
            }
            other => panic!("expected UnknownValue, got {other:?}"),
        }
    }

    /// Identifiers come from SQLite's CSPRNG rather than the clock, because
    /// sessions get spawned in bursts.
    #[test]
    fn generated_session_identifiers_are_unique_within_a_burst() {
        let tmp = tempfile::tempdir().unwrap();
        let fixture = Fixture::new(tmp.path(), "alpha");
        // A frozen clock: any identifier derived from time would collide.
        let store = fixture.store_with_ticking_clock(42, 0);

        let ids: std::collections::HashSet<_> = (0..64)
            .map(|_| {
                store
                    .create(NewSession::embedded("claude-code"))
                    .unwrap()
                    .id
            })
            .collect();
        assert_eq!(ids.len(), 64, "identifiers must not collide");
    }

    /// An existing version-1 database gains the sessions table on the next
    /// launch without losing its project binding.
    #[test]
    fn a_version_one_database_migrates_forward_keeping_its_binding() {
        let tmp = tempfile::tempdir().unwrap();
        let fixture = Fixture::new(tmp.path(), "alpha");
        let project_id = fixture.project_id().to_owned();

        // Wind the database back to what version 1 left behind.
        //
        // The deleted range must stay contiguous to the newest migration: the
        // runner resumes from `MAX(version)`, so leaving a higher row behind
        // makes it believe there is nothing to do. See the sibling test.
        fixture
            .conn
            .execute_batch(
                "DROP TRIGGER sessions_reject_foreign_project_insert;
                 DROP TRIGGER sessions_reject_foreign_project_update;
                 DROP TABLE sessions;
                 DROP TABLE IF EXISTS memories_fts;
                 DROP TABLE IF EXISTS memories;
                 DROP TABLE IF EXISTS lifecycle_events;
                 DROP TABLE IF EXISTS checkpoints;
                 DELETE FROM schema_migrations WHERE version >= 2;",
            )
            .unwrap();
        drop(fixture.reopen());

        let reopened = fixture.reopen();
        let version: i64 = reopened
            .query_row("SELECT MAX(version) FROM schema_migrations", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(
            version, 7,
            "the launch must have applied migrations 2, 3, 4, 5, 6 and 7"
        );

        let store = SessionStore::new(&reopened).unwrap();
        assert_eq!(store.project_id(), project_id, "the binding survived");
        let record = store.create(NewSession::embedded("claude-code")).unwrap();
        assert_eq!(record.project_id, project_id);
    }

    /// Two projects on one machine keep entirely separate session lists —
    /// separate files, not a shared file with a filter.
    #[test]
    fn two_projects_have_independent_session_lists() {
        let tmp = tempfile::tempdir().unwrap();
        let alpha = Fixture::new(tmp.path(), "alpha");
        let beta = Fixture::new(tmp.path(), "beta");

        alpha
            .store()
            .create(NewSession::embedded("claude-code"))
            .unwrap();
        alpha.store().create(NewSession::embedded("codex")).unwrap();
        beta.store()
            .create(NewSession::embedded("claude-code"))
            .unwrap();

        assert_ne!(alpha.runtime.database_path(), beta.runtime.database_path());
        assert_eq!(alpha.store().list().unwrap().len(), 2);
        assert_eq!(beta.store().list().unwrap().len(), 1);
    }

    /// The store refuses to work against a database with no project bound,
    /// rather than defaulting to something and writing rows nobody can place.
    #[test]
    fn the_store_refuses_an_unbound_database() {
        let tmp = tempfile::tempdir().unwrap();
        let fixture = Fixture::new(tmp.path(), "alpha");
        fixture
            .conn
            .execute("DELETE FROM project_metadata WHERE key = 'project_id'", [])
            .unwrap();

        let error = SessionStore::new(&fixture.conn).expect_err("an unbound database is unusable");
        assert!(
            matches!(error, SessionStoreError::UnboundDatabase),
            "got {error:?}"
        );
    }

    /// The injected clock is the one every test above uses, so the real one
    /// needs its own check that it returns sane epoch seconds rather than,
    /// say, nanoseconds or zero.
    #[test]
    fn the_default_clock_returns_plausible_epoch_seconds() {
        let first = system_clock();
        let second = system_clock();
        assert!(
            second >= first,
            "the wall clock must not run backwards mid-test"
        );
        assert!(
            first > 1_600_000_000,
            "the clock must return seconds since the epoch"
        );
        assert!(
            first < 32_000_000_000,
            "seconds, not milliseconds or nanoseconds"
        );
    }

    // --- resolving an identifier ----------------------------------------

    #[test]
    fn a_whole_identifier_resolves_to_its_session() {
        let tmp = tempfile::tempdir().unwrap();
        let fixture = Fixture::new(tmp.path(), "alpha");
        let store = fixture.store();
        let record = store.create(NewSession::embedded("claude-code")).unwrap();
        assert_eq!(store.resolve_id(record.id.as_str()).unwrap(), record.id);
    }

    #[test]
    fn the_short_form_the_listing_prints_is_enough_to_resolve() {
        // `glasshouse sessions` prints twelve characters and nothing else, so
        // twelve characters have to be usable. If they were not, the only
        // identifier a user can see would be the one they cannot use.
        let tmp = tempfile::tempdir().unwrap();
        let fixture = Fixture::new(tmp.path(), "alpha");
        let store = fixture.store();
        let record = store.create(NewSession::embedded("claude-code")).unwrap();

        let short: String = record.id.as_str().chars().take(12).collect();
        assert_eq!(store.resolve_id(&short).unwrap(), record.id);
    }

    #[test]
    fn an_ambiguous_prefix_is_refused_and_names_its_candidates() {
        // Resuming the wrong session is worse than being asked to type more.
        let tmp = tempfile::tempdir().unwrap();
        let fixture = Fixture::new(tmp.path(), "alpha");
        let store = fixture.store();
        let first = store.create(NewSession::embedded("claude-code")).unwrap();
        let second = store.create(NewSession::embedded("codex")).unwrap();

        // Every identifier shares the empty prefix; the shortest prefix both
        // share is found by comparison so the test does not depend on the
        // random values.
        let shared: String = first
            .id
            .as_str()
            .chars()
            .zip(second.id.as_str().chars())
            .take_while(|(a, b)| a == b)
            .map(|(a, _)| a)
            .collect();
        let ambiguous = shared;
        if ambiguous.is_empty() {
            // Two identifiers with no shared prefix: use a one-character one
            // that both cannot share, and assert the exact-match path instead.
            assert_eq!(store.resolve_id(first.id.as_str()).unwrap(), first.id);
            return;
        }

        match store.resolve_id(&ambiguous) {
            Err(SessionStoreError::AmbiguousPrefix { matches, .. }) => {
                assert!(matches.contains(&first.id));
                assert!(matches.contains(&second.id));
            }
            other => panic!("expected an ambiguous prefix, got {other:?}"),
        }
    }

    #[test]
    fn an_unknown_identifier_is_not_found() {
        let tmp = tempfile::tempdir().unwrap();
        let fixture = Fixture::new(tmp.path(), "alpha");
        let store = fixture.store();
        store.create(NewSession::embedded("claude-code")).unwrap();
        assert!(matches!(
            store.resolve_id("ffffffffffffffffffffffffffffffff"),
            Err(SessionStoreError::NotFound { .. })
        ));
    }

    #[test]
    fn a_wildcard_cannot_be_smuggled_into_the_lookup() {
        // Identifiers are matched with `substr`, not `LIKE`. Under `LIKE`, a
        // bare `%` would match every session in the project, and resuming
        // "whichever one came first" is exactly the wrong answer.
        let tmp = tempfile::tempdir().unwrap();
        let fixture = Fixture::new(tmp.path(), "alpha");
        let store = fixture.store();
        store.create(NewSession::embedded("claude-code")).unwrap();

        for hostile in ["%", "_", "%%", "a%", "' OR 1=1 --"] {
            assert!(
                matches!(
                    store.resolve_id(hostile),
                    Err(SessionStoreError::MalformedId { .. })
                ),
                "`{hostile}` was not refused"
            );
        }
    }

    #[test]
    fn an_empty_identifier_is_refused() {
        let tmp = tempfile::tempdir().unwrap();
        let fixture = Fixture::new(tmp.path(), "alpha");
        let store = fixture.store();
        assert!(matches!(
            store.resolve_id("   "),
            Err(SessionStoreError::MalformedId { .. })
        ));
    }

    // --- assigned native identifiers -------------------------------------

    #[test]
    fn a_minted_native_identifier_is_a_valid_version_4_uuid() {
        let tmp = tempfile::tempdir().unwrap();
        let fixture = Fixture::new(tmp.path(), "alpha");
        let store = fixture.store();
        for _ in 0..64 {
            let id = store.new_native_session_id().unwrap();
            assert_eq!(id.len(), 36, "{id}");
            let groups: Vec<&str> = id.split('-').collect();
            assert_eq!(
                groups.iter().map(|g| g.len()).collect::<Vec<_>>(),
                vec![8, 4, 4, 4, 12],
                "{id}"
            );
            assert!(
                id.chars().all(|c| c.is_ascii_hexdigit() || c == '-'),
                "{id}"
            );
            // The two things a strict validator checks beyond the shape.
            assert_eq!(groups[2].chars().next(), Some('4'), "version nibble: {id}");
            assert!(
                matches!(groups[3].chars().next(), Some('8' | '9' | 'a' | 'b')),
                "variant nibble: {id}"
            );
        }
    }

    #[test]
    fn minted_native_identifiers_do_not_repeat() {
        let tmp = tempfile::tempdir().unwrap();
        let fixture = Fixture::new(tmp.path(), "alpha");
        let store = fixture.store();
        let mut seen = std::collections::HashSet::new();
        for _ in 0..256 {
            assert!(
                seen.insert(store.new_native_session_id().unwrap()),
                "a minted identifier repeated"
            );
        }
    }

    #[test]
    fn the_uuid_formatter_only_overwrites_the_version_and_variant() {
        // Every other nibble survives, so the identifier keeps 122 bits of
        // the randomness it was given rather than being quietly reshaped.
        let hex = "0123456789abcdef0123456789abcdef";
        let uuid = uuid_v4_from_hex(hex);
        assert_eq!(uuid, "01234567-89ab-4def-8123-456789abcdef");

        let plain: String = uuid.chars().filter(|c| *c != '-').collect();
        let differences = hex
            .chars()
            .zip(plain.chars())
            .enumerate()
            .filter(|(_, (a, b))| a != b)
            .map(|(i, _)| i)
            .collect::<Vec<_>>();
        assert_eq!(differences, vec![12, 16], "only these two nibbles may move");
    }

    #[test]
    fn a_session_can_be_recorded_with_its_native_identifier_from_the_start() {
        // The point of assignment: the record carries the identifier before
        // the harness has produced any output at all, so a session that dies
        // during startup is still resumable rather than anonymous.
        let tmp = tempfile::tempdir().unwrap();
        let fixture = Fixture::new(tmp.path(), "alpha");
        let store = fixture.store();
        let native = store.new_native_session_id().unwrap();
        let record = store
            .create(
                NewSession::embedded("claude-code").with_native_session_id(Some(native.clone())),
            )
            .unwrap();
        assert_eq!(record.native_session_id.as_deref(), Some(native.as_str()));

        let read_back = store.get(&record.id).unwrap().expect("the session");
        assert_eq!(read_back.native_session_id, Some(native));
    }
}

#[cfg(test)]
mod display_tests {
    use super::*;

    /// A `Display` impl that writes straight to the formatter ignores width,
    /// which turns any aligned listing into ragged columns. Cheap to get
    /// wrong, invisible in a round-trip test, so it gets its own check.
    #[test]
    fn stored_values_honour_format_width_so_listings_align() {
        assert_eq!(format!("[{:<10}]", SessionRole::Normal), "[normal    ]");
        assert_eq!(
            format!("[{:<10}]", SessionRole::Orchestrator),
            "[orchestrator]"
        );
        assert_eq!(
            format!("[{:<10}]", SessionPresentation::Embedded),
            "[embedded  ]"
        );
        assert_eq!(
            format!("[{:<20}]", SessionLifecycle::WaitingForUser),
            "[waiting_for_user    ]"
        );
        assert_eq!(format!("[{:<6}]", SessionId::new("ab")), "[ab    ]");
    }
}
