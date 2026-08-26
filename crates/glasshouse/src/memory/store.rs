//! The `memories` table, and the only way to read or write it.
//!
//! # Project isolation
//!
//! Enforced in three independent places, for the reason each is different:
//!
//! - **At the file**, because [`ProjectMemory::open`] goes through
//!   `database::open`, which derives the path from the runtime and
//!   refuses a database bound to another project outright.
//! - **At the row**, by the two SQLite triggers migration 4 creates. A query
//!   can forget to filter by `project_id`; a `BEFORE INSERT` / `BEFORE UPDATE`
//!   guard cannot be forgotten, and it holds against any writer, including one
//!   written later by someone who never read this module.
//! - **At the read boundary**, by [`MemoryStore::get`], which compares the
//!   stored identifier against the active project before handing a record
//!   back.
//!
//! The third is not redundant with the second, for the reason
//! [`crate::session::store`] gives about resume: the trigger governs what this
//! database will *accept* from now on, while the boundary check governs what
//! Glasshouse will *act on* — including a row that predates a guard, arrived
//! through a restored backup, or was written by a build whose triggers
//! differed. Retrieval is the operation that turns a stored row into something
//! an agent will treat as true, so it verifies rather than assumes.
//!
//! # No credentials
//!
//! There is no column here for a token, a key, or a provider secret, and there
//! is no field for one either. The project database is checked into nothing
//! and backed up casually; the operating system's secret storage
//! ([`crate::secret`]) is where a credential lives. `body` is free text an
//! extractor produced, which is exactly why nothing may *route* a credential
//! into it.

use std::fmt;
use std::sync::Arc;

use rusqlite::{Connection, OptionalExtension, Row};

use crate::database::PROJECT_ID_KEY;

use super::policy::{MemoryRefusal, admit};

/// A durable memory's identifier, unique inside one project.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct MemoryId(String);

impl MemoryId {
    /// Wrap an identifier that already exists, such as one read back from the
    /// database or supplied on the command line.
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for MemoryId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.pad(&self.0)
    }
}

/// What sort of thing was remembered — Phase 20's six kinds.
///
/// Independent of [`MemoryAuthority`], which says how binding it is, and of
/// [`MemoryStatus`], which says where it sits in its lifecycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum MemoryKind {
    /// An implementation or architecture choice that was accepted.
    Decision,
    /// A limit the project currently works within.
    Constraint,
    /// Something the project has or plans to have.
    Feature,
    /// Something established by investigation, which rediscovering would cost.
    Finding,
    /// An approach that was tried and did not work. Kept precisely so it is
    /// not tried again.
    FailedAttempt,
    /// Work that is known to be outstanding.
    Todo,
}

/// How binding a memory is — Phase 21A's seven authority classes.
///
/// Stored from Phase 20 onwards so that Phase 21A adds *classification* rather
/// than a migration. Nothing classifies yet, which is why the column and this
/// field are optional: `None` means no authority has been assigned, a distinct
/// fact from every one of the seven classes. Retrieval must treat `None`
/// conservatively — see [`MemoryAuthority::is_binding`], which `None` is not.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum MemoryAuthority {
    /// Must not be violated without explicit review.
    Invariant,
    /// A currently binding technical, security, legal, compatibility or
    /// product limit.
    Constraint,
    /// An accepted choice that may later be revisited.
    Decision,
    /// A desired direction that must not force unnecessary complexity.
    Preference,
    /// A belief that still requires validation.
    Hypothesis,
    /// Exploratory. Must never be injected as a binding instruction.
    Idea,
    /// Useful for understanding the project; must not direct current work.
    Historical,
}

impl MemoryAuthority {
    /// Whether a memory of this authority may be presented to an agent as a
    /// rule rather than as context.
    ///
    /// A full `match`: a new authority class must be classified here instead
    /// of defaulting to either side.
    pub fn is_binding(self) -> bool {
        match self {
            Self::Invariant | Self::Constraint | Self::Decision => true,
            Self::Preference | Self::Hypothesis | Self::Idea | Self::Historical => false,
        }
    }
}

/// Where a memory sits in its lifecycle — Phase 20's six statuses, plus
/// Phase 22's conflict state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum MemoryStatus {
    /// Current knowledge. The only status normal retrieval returns.
    Active,
    /// Replaced by a newer memory. Kept, never deleted: the history is what
    /// stops a future agent resurrecting the same idea.
    Superseded,
    /// Considered and turned down. Searchable as historical knowledge so the
    /// rejection is not rediscovered as a fresh proposal.
    Rejected,
    /// A todo that was completed. Queryable, but never presented as open work.
    Resolved,
    /// Something changed that may invalidate this; a person or a stronger
    /// agent has to look.
    NeedsReview,
    /// A known invalidation condition occurred. Historical evidence, never a
    /// current instruction.
    Invalidated,
    /// Phase 22: this memory contradicts another current memory and the
    /// conflict could not be resolved automatically.
    Conflicted,
}

impl MemoryStatus {
    /// Whether a memory with this status is current project knowledge.
    ///
    /// Only [`MemoryStatus::Active`] is. Everything else is history — still
    /// stored, still searchable when history is asked for explicitly, never
    /// returned by a default search.
    pub fn is_current(self) -> bool {
        match self {
            Self::Active => true,
            Self::Superseded
            | Self::Rejected
            | Self::Resolved
            | Self::NeedsReview
            | Self::Invalidated
            | Self::Conflicted => false,
        }
    }

    /// Whether a [`MemoryKind::Todo`] with this status is still open work.
    ///
    /// A resolved todo remains queryable — Phase 22 requires it — and this is
    /// what keeps it from being *presented* as something still to do.
    pub fn is_open_work(self) -> bool {
        match self {
            Self::Active | Self::NeedsReview | Self::Conflicted => true,
            Self::Superseded | Self::Rejected | Self::Resolved | Self::Invalidated => false,
        }
    }
}

macro_rules! sql_enum {
    ($ty:ty { $($variant:ident => $text:literal),+ $(,)? }) => {
        impl $ty {
            /// The value stored in SQLite. Migration 4's `CHECK` constraint
            /// lists exactly these strings, so adding a variant here without a
            /// migration makes writes fail loudly rather than silently storing
            /// something readers cannot interpret.
            pub fn as_str(self) -> &'static str {
                match self {
                    $(Self::$variant => $text,)+
                }
            }

            /// Parse the value stored in SQLite, which is also the spelling
            /// a user types on the command line.
            ///
            /// Deliberately not named `from_str`: that name belongs to
            /// [`std::str::FromStr`], and a public inherent method wearing it
            /// would be picked up by `.parse()`-shaped expectations it does not
            /// satisfy.
            pub fn from_stored(value: &str) -> Option<Self> {
                match value {
                    $($text => Some(Self::$variant),)+
                    _ => None,
                }
            }

            /// Every variant, in schema order. Used by the CLI's help text and
            /// by round-trip tests; a variant missing here is a variant no
            /// user-facing surface can name.
            pub const ALL: &'static [Self] = &[$(Self::$variant,)+];
        }

        impl fmt::Display for $ty {
            /// `pad`, not `write_str`: a `Display` that writes straight to the
            /// formatter silently ignores width and alignment, so `{:<14}` in
            /// a table would produce ragged columns.
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.pad(self.as_str())
            }
        }
    };
}

sql_enum!(MemoryKind {
    Decision => "decision",
    Constraint => "constraint",
    Feature => "feature",
    Finding => "finding",
    FailedAttempt => "failed_attempt",
    Todo => "todo",
});

sql_enum!(MemoryAuthority {
    Invariant => "invariant",
    Constraint => "constraint",
    Decision => "decision",
    Preference => "preference",
    Hypothesis => "hypothesis",
    Idea => "idea",
    Historical => "historical",
});

sql_enum!(MemoryStatus {
    Active => "active",
    Superseded => "superseded",
    Rejected => "rejected",
    Resolved => "resolved",
    NeedsReview => "needs_review",
    Invalidated => "invalidated",
    Conflicted => "conflicted",
});

/// One stored memory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryRecord {
    pub id: MemoryId,
    /// The project this memory belongs to. Always the active project for any
    /// record this module hands out.
    pub project_id: String,
    pub kind: MemoryKind,
    /// `None` means no authority has been assigned — see [`MemoryAuthority`].
    pub authority: Option<MemoryAuthority>,
    pub status: MemoryStatus,
    /// A concise subject, when the producer had one. `None` is not an empty
    /// subject: it records that none was available, which Phase 20 explicitly
    /// allows ("when available").
    pub subject: Option<String>,
    /// The durable body. Never empty; the admission guard refuses that.
    pub body: String,
    /// The session this memory was extracted from, when known.
    ///
    /// A *reference*, not a foreign key, for the same reason
    /// `sessions.native_session_id` is: a memory may be extracted from a
    /// session Glasshouse never recorded, and deleting a session's record must
    /// not delete what was learned in it.
    pub source_session_id: Option<String>,
    /// The Git commit the project was at when this was learned, when known.
    pub source_commit: Option<String>,
    /// The memory that replaced this one, when a direct supersession
    /// relationship is known. `None` on a superseded memory means it was
    /// retired without a single identifiable successor.
    pub superseded_by: Option<MemoryId>,
    /// Seconds since the Unix epoch.
    pub created_at: i64,
    /// Seconds since the Unix epoch.
    pub updated_at: i64,
}

impl MemoryRecord {
    /// Whether this memory is current project knowledge that may be retrieved
    /// by default.
    pub fn is_current(&self) -> bool {
        self.status.is_current()
    }

    /// Whether this memory should be presented as work still outstanding.
    ///
    /// Only a todo ever can be: Phase 22's requirement is specifically that a
    /// resolved todo stays queryable without appearing open, and nothing else
    /// is "open work" to begin with.
    pub fn is_open_todo(&self) -> bool {
        self.kind == MemoryKind::Todo && self.status.is_open_work()
    }
}

/// What a caller supplies to record a memory.
///
/// There is no field for a credential, a token, or a provider key, and no
/// column for one either — see the module documentation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewMemory {
    pub kind: MemoryKind,
    pub body: String,
    pub subject: Option<String>,
    /// Left `None` until Phase 21A classifies. Never guessed here: an
    /// unclassified memory that defaulted to a class would be a promotion
    /// nobody made.
    pub authority: Option<MemoryAuthority>,
    pub source_session_id: Option<String>,
    pub source_commit: Option<String>,
}

impl NewMemory {
    /// A memory of one kind with one body. Everything else is optional
    /// because Phase 20 stores it "when available", and absent must stay
    /// distinguishable from empty.
    pub fn new(kind: MemoryKind, body: impl Into<String>) -> Self {
        Self {
            kind,
            body: body.into(),
            subject: None,
            authority: None,
            source_session_id: None,
            source_commit: None,
        }
    }

    /// Record a concise subject.
    ///
    /// A subject that is empty or whitespace is stored as `None` rather than
    /// as `Some("")`: "no subject was available" and "the subject is the empty
    /// string" are the same fact, and only one of them should be
    /// representable.
    pub fn with_subject(mut self, subject: Option<impl Into<String>>) -> Self {
        self.subject = subject
            .map(Into::into)
            .filter(|value| !value.trim().is_empty());
        self
    }

    /// Record the authority class, once something has classified it.
    pub fn with_authority(mut self, authority: Option<MemoryAuthority>) -> Self {
        self.authority = authority;
        self
    }

    /// Record the session this was extracted from.
    pub fn with_source_session(mut self, session: Option<impl Into<String>>) -> Self {
        self.source_session_id = session
            .map(Into::into)
            .filter(|value| !value.trim().is_empty());
        self
    }

    /// Record the Git commit the project was at.
    pub fn with_source_commit(mut self, commit: Option<impl Into<String>>) -> Self {
        self.source_commit = commit
            .map(Into::into)
            .filter(|value| !value.trim().is_empty());
        self
    }
}

/// Failures a caller has to distinguish.
#[derive(Debug, thiserror::Error)]
pub enum MemoryStoreError {
    #[error("no memory `{id}` in this project")]
    NotFound { id: MemoryId },
    #[error(
        "memory `{id}` belongs to project `{actual}`, not to the active \
         project `{expected}`; refusing to read another project's memory"
    )]
    ForeignProject {
        id: MemoryId,
        expected: String,
        actual: String,
    },
    /// The admission guard refused the memory. See [`MemoryRefusal`].
    #[error(transparent)]
    Refused(#[from] MemoryRefusal),
    #[error(
        "a memory cannot supersede itself (`{id}`); supersession records that \
         one memory replaced another"
    )]
    SelfSupersession { id: MemoryId },
    #[error("memory `{id}` stored an unrecognized {column} value `{value}`")]
    UnknownValue {
        id: MemoryId,
        column: &'static str,
        value: String,
    },
    #[error(
        "memory `{id}` carries {impact} authority, so its conflict may not be \
         resolved automatically; a person or a stronger agent has to decide"
    )]
    ReviewRequired { id: MemoryId, impact: &'static str },
    #[error("the project database has no project identifier bound")]
    UnboundDatabase,
    #[error("could not {action} in the project database")]
    Sql {
        action: &'static str,
        #[source]
        source: rusqlite::Error,
    },
}

/// Who is resolving a memory conflict.
///
/// Phase 22 requires "human or stronger-agent review before automatically
/// resolving ambiguous high-impact memory conflicts", so the caller has to say
/// which it is. There is no default: an argument that could be omitted would
/// be omitted, and the omission would always fall on the automatic side.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConflictResolver {
    /// An agent acting on its own judgment, mid-task.
    Automatic,
    /// A person, or an agent the user has put in a review role. Permitted to
    /// resolve any conflict.
    Reviewed,
}

/// Reads the wall clock, in seconds since the Unix epoch.
///
/// Injected rather than called directly so tests can assert on exact
/// timestamps instead of sleeping or accepting a range. Shared ownership
/// rather than a bare `fn` pointer because a useful test clock has to
/// *advance*, which means capturing state.
///
/// Deliberately a second declaration of the same shape as
/// `crate::session::store::Clock` rather than a reuse of it: the two modules
/// are being built by two concurrent batches, and a shared alias would couple
/// them for no benefit beyond removing one line. Worth unifying once both have
/// landed.
pub type Clock = Arc<dyn Fn() -> i64 + Send + Sync>;

/// Seconds since the Unix epoch.
///
/// Saturates rather than panicking on a clock set before 1970: a nonsensical
/// timestamp on one row is a far smaller problem than refusing to record what
/// the project learned.
fn system_clock() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};

    match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(elapsed) => i64::try_from(elapsed.as_secs()).unwrap_or(i64::MAX),
        Err(_) => 0,
    }
}

/// Every column of `memories`, in the order [`row_to_record`] reads them.
pub(super) const ALL_COLUMNS: &str = "id, project_id, kind, authority, status, subject, body, \
                                      source_session_id, source_commit, superseded_by, \
                                      created_at, updated_at";

/// An open project database plus the memories inside it.
///
/// Owns its connection, for callers — the CLI, and eventually the TUI — that
/// want one value to hold. [`MemoryStore`] borrows a connection instead, so a
/// single connection can back several kinds of store.
pub struct ProjectMemory {
    conn: Connection,
    project_id: String,
    clock: Clock,
}

impl fmt::Debug for ProjectMemory {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ProjectMemory")
            .field("project_id", &self.project_id)
            .finish_non_exhaustive()
    }
}

impl ProjectMemory {
    /// Open the active project's database and read its binding.
    ///
    /// The path comes from `runtime` and nowhere else, so there is no argument
    /// a caller could pass to reach another project's file. Every check
    /// `database::open` performs — the symlink refusal, the read-only
    /// refusal, the project-identity check, the migrations — applies here
    /// because this is the same door.
    pub fn open(runtime: &crate::Runtime) -> anyhow::Result<Self> {
        Self::open_with_clock(runtime, Arc::new(system_clock))
    }

    /// [`ProjectMemory::open`] with the clock replaced.
    pub fn open_with_clock(runtime: &crate::Runtime, clock: Clock) -> anyhow::Result<Self> {
        let conn = crate::database::open(runtime)?;
        let project_id = MemoryStore::with_clock(&conn, Arc::clone(&clock))?
            .project_id()
            .to_owned();
        Ok(Self {
            conn,
            project_id,
            clock,
        })
    }

    /// The memories in this project.
    pub fn store(&self) -> MemoryStore<'_> {
        MemoryStore {
            conn: &self.conn,
            project_id: self.project_id.clone(),
            clock: Arc::clone(&self.clock),
        }
    }
}

/// Memory records for one project.
pub struct MemoryStore<'a> {
    conn: &'a Connection,
    project_id: String,
    clock: Clock,
}

impl fmt::Debug for MemoryStore<'_> {
    /// Hand-written because [`Clock`] is a trait object with no `Debug`.
    /// Prints the project identifier — a hash of the canonical root, not a
    /// secret — and nothing about the connection or its contents.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MemoryStore")
            .field("project_id", &self.project_id)
            .finish_non_exhaustive()
    }
}

impl<'a> MemoryStore<'a> {
    /// Open the store over a connection produced by
    /// `database::open`.
    ///
    /// The project identifier is read from the database's own binding rather
    /// than accepted as an argument, so the identifier this store writes is by
    /// construction the identifier the triggers compare against, even if a
    /// caller is confused about which project it is in.
    pub fn new(conn: &'a Connection) -> Result<Self, MemoryStoreError> {
        Self::with_clock(conn, Arc::new(system_clock))
    }

    /// [`MemoryStore::new`] with the clock replaced.
    pub fn with_clock(conn: &'a Connection, clock: Clock) -> Result<Self, MemoryStoreError> {
        let project_id: Option<String> = conn
            .query_row(
                "SELECT value FROM project_metadata WHERE key = ?1",
                [PROJECT_ID_KEY],
                |row| row.get(0),
            )
            .optional()
            .map_err(|source| MemoryStoreError::Sql {
                action: "read the project identifier",
                source,
            })?;

        Ok(Self {
            project_id: project_id.ok_or(MemoryStoreError::UnboundDatabase)?,
            conn,
            clock,
        })
    }

    /// The project every record in this store belongs to.
    pub fn project_id(&self) -> &str {
        &self.project_id
    }

    /// The connection, for the search and snapshot surfaces in this module.
    pub(super) fn connection(&self) -> &Connection {
        self.conn
    }

    /// Store a memory, if it is one.
    ///
    /// Goes through the admission guard first, so there is no route into the
    /// table that skips Phase 20's prohibitions. A refused memory leaves no
    /// row and no identifier behind.
    ///
    /// The identifier is generated by SQLite's own CSPRNG rather than from the
    /// clock: extraction produces memories in bursts, and anything
    /// time-derived would collide.
    pub fn record(&self, new: NewMemory) -> Result<MemoryRecord, MemoryStoreError> {
        admit(&new)?;

        let now = (self.clock)();
        let record = MemoryRecord {
            id: MemoryId(self.generate_id()?),
            project_id: self.project_id.clone(),
            kind: new.kind,
            authority: new.authority,
            // Everything starts current. A memory is only ever moved out of
            // `Active` by something that knows why.
            status: MemoryStatus::Active,
            subject: new.subject,
            body: new.body,
            source_session_id: new.source_session_id,
            source_commit: new.source_commit,
            superseded_by: None,
            created_at: now,
            updated_at: now,
        };

        self.conn
            .execute(
                "INSERT INTO memories (id, project_id, kind, authority, status, subject, \
                 body, source_session_id, source_commit, superseded_by, created_at, updated_at) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
                rusqlite::params![
                    record.id.as_str(),
                    &record.project_id,
                    record.kind.as_str(),
                    record.authority.map(MemoryAuthority::as_str),
                    record.status.as_str(),
                    &record.subject,
                    &record.body,
                    &record.source_session_id,
                    &record.source_commit,
                    record.superseded_by.as_ref().map(MemoryId::as_str),
                    record.created_at,
                    record.updated_at,
                ],
            )
            .map_err(|source| MemoryStoreError::Sql {
                action: "record a memory",
                source,
            })?;

        Ok(record)
    }

    fn generate_id(&self) -> Result<String, MemoryStoreError> {
        self.conn
            .query_row("SELECT lower(hex(randomblob(16)))", [], |row| row.get(0))
            .map_err(|source| MemoryStoreError::Sql {
                action: "generate a memory identifier",
                source,
            })
    }

    /// One memory in full, by identifier — Phase 26's `memory.get`.
    ///
    /// Verifies the stored project identifier before returning anything, which
    /// is the read boundary the module documentation describes. A row bound to
    /// another project is an error, never `None`: silently reporting "no such
    /// memory" would hide the fact that a foreign row is sitting in this
    /// project's file.
    pub fn get(&self, id: &MemoryId) -> Result<Option<MemoryRecord>, MemoryStoreError> {
        let record: Option<MemoryRecord> = self
            .conn
            .query_row(
                &format!("SELECT {ALL_COLUMNS} FROM memories WHERE id = ?1"),
                [id.as_str()],
                row_to_record,
            )
            .optional()
            .map_err(|source| MemoryStoreError::Sql {
                action: "read a memory",
                source,
            })?
            .transpose()?;

        match record {
            Some(record) if record.project_id != self.project_id => {
                Err(MemoryStoreError::ForeignProject {
                    id: record.id,
                    expected: self.project_id.clone(),
                    actual: record.project_id,
                })
            }
            other => Ok(other),
        }
    }

    /// Resolve a whole identifier, or the leading part of one, to exactly one
    /// memory.
    ///
    /// A prefix is a requirement rather than a convenience, for the reason
    /// `glasshouse sessions` established: listings print a short form, so the
    /// short form is the only one a user can copy from the screen. Ambiguity
    /// is refused and every candidate named, and matching uses `substr` rather
    /// than `LIKE` so a `%` typed by the user is a character, not a wildcard
    /// that would match every memory in the project.
    pub fn resolve_id(&self, prefix: &str) -> Result<MemoryId, MemoryStoreError> {
        let prefix = prefix.trim().to_ascii_lowercase();
        if prefix.is_empty() || !prefix.chars().all(|c| c.is_ascii_hexdigit()) {
            return Err(MemoryStoreError::NotFound {
                id: MemoryId(prefix),
            });
        }

        let mut statement = self
            .conn
            .prepare("SELECT id FROM memories WHERE substr(id, 1, ?2) = ?1 ORDER BY id")
            .map_err(|source| MemoryStoreError::Sql {
                action: "prepare the memory lookup",
                source,
            })?;
        let matches: Vec<MemoryId> = statement
            .query_map(
                rusqlite::params![&prefix, i64::try_from(prefix.len()).unwrap_or(i64::MAX)],
                |row| row.get::<_, String>(0).map(MemoryId),
            )
            .and_then(|rows| rows.collect::<Result<Vec<_>, _>>())
            .map_err(|source| MemoryStoreError::Sql {
                action: "look a memory up by identifier",
                source,
            })?;

        match matches.as_slice() {
            [only] => Ok(only.clone()),
            _ => Err(MemoryStoreError::NotFound {
                id: MemoryId(prefix),
            }),
        }
    }

    /// Record that one memory replaced another — Phase 22.
    ///
    /// The older memory becomes [`MemoryStatus::Superseded`] and names its
    /// successor. Nothing is deleted: the history is the point, and the
    /// superseding identifier is what stops a later agent resurrecting the old
    /// decision without knowing why it went.
    ///
    /// Both identifiers are checked against this project first, so a
    /// supersession can never be recorded across projects even if a caller
    /// somehow held a foreign identifier.
    pub fn supersede(
        &self,
        old: &MemoryId,
        replacement: &MemoryId,
    ) -> Result<MemoryRecord, MemoryStoreError> {
        if old == replacement {
            return Err(MemoryStoreError::SelfSupersession { id: old.clone() });
        }
        // `get` carries the project check, so both ends are verified before
        // anything is written.
        self.get(old)?
            .ok_or_else(|| MemoryStoreError::NotFound { id: old.clone() })?;
        self.get(replacement)?
            .ok_or_else(|| MemoryStoreError::NotFound {
                id: replacement.clone(),
            })?;

        self.conn
            .execute(
                "UPDATE memories SET status = ?2, superseded_by = ?3, updated_at = ?4 \
                 WHERE id = ?1",
                rusqlite::params![
                    old.as_str(),
                    MemoryStatus::Superseded.as_str(),
                    replacement.as_str(),
                    (self.clock)(),
                ],
            )
            .map_err(|source| MemoryStoreError::Sql {
                action: "record a supersession",
                source,
            })?;

        self.get(old)?
            .ok_or_else(|| MemoryStoreError::NotFound { id: old.clone() })
    }

    /// Move a memory to another lifecycle status.
    ///
    /// Setting [`MemoryStatus::Superseded`] this way is allowed and leaves
    /// `superseded_by` alone — Phase 22 asks for the successor's identifier
    /// only "when a direct supersession relationship is known", so retiring a
    /// memory without one is a real state rather than a missing field.
    ///
    /// Moving *away* from `Superseded` clears the successor, because a memory
    /// that is active again has not been replaced by anything; the schema's
    /// `CHECK` would refuse the inconsistent row anyway, and clearing it here
    /// means the caller gets the intended state instead of an error.
    pub fn set_status(
        &self,
        id: &MemoryId,
        status: MemoryStatus,
    ) -> Result<MemoryRecord, MemoryStoreError> {
        self.get(id)?
            .ok_or_else(|| MemoryStoreError::NotFound { id: id.clone() })?;

        let keep_successor = status == MemoryStatus::Superseded;
        self.conn
            .execute(
                "UPDATE memories \
                 SET status = ?2, \
                     superseded_by = CASE WHEN ?3 THEN superseded_by ELSE NULL END, \
                     updated_at = ?4 \
                 WHERE id = ?1",
                rusqlite::params![id.as_str(), status.as_str(), keep_successor, (self.clock)()],
            )
            .map_err(|source| MemoryStoreError::Sql {
                action: "change a memory's status",
                source,
            })?;

        self.get(id)?
            .ok_or_else(|| MemoryStoreError::NotFound { id: id.clone() })
    }

    /// Every memory with the given status, most recently updated first.
    ///
    /// The ordering is the `memories_by_status_updated` index read directly.
    pub fn with_status(
        &self,
        status: MemoryStatus,
        limit: usize,
    ) -> Result<Vec<MemoryRecord>, MemoryStoreError> {
        let mut statement = self
            .conn
            .prepare(&format!(
                "SELECT {ALL_COLUMNS} FROM memories WHERE status = ?1 \
                 ORDER BY updated_at DESC, id ASC LIMIT ?2"
            ))
            .map_err(|source| MemoryStoreError::Sql {
                action: "prepare the memory listing",
                source,
            })?;
        let rows = statement
            .query_map(
                rusqlite::params![status.as_str(), i64::try_from(limit).unwrap_or(i64::MAX)],
                row_to_record,
            )
            .and_then(|rows| rows.collect::<Result<Vec<_>, _>>())
            .map_err(|source| MemoryStoreError::Sql {
                action: "list memories",
                source,
            })?;
        rows.into_iter().collect()
    }

    /// Record that two current memories contradict each other — Phase 22.
    ///
    /// Both move to [`MemoryStatus::Conflicted`], which is what makes the
    /// contradiction *visible*: a conflicted memory is not current knowledge,
    /// so a default search stops returning either of them as if it were
    /// settled truth, and a caller that asks for them gets a status that says
    /// why. Neither is deleted and neither is silently preferred, because
    /// picking one is exactly the judgment this state exists to defer.
    ///
    /// Both identifiers are checked against this project before anything is
    /// written.
    pub fn mark_conflicted(
        &self,
        one: &MemoryId,
        other: &MemoryId,
    ) -> Result<(MemoryRecord, MemoryRecord), MemoryStoreError> {
        if one == other {
            return Err(MemoryStoreError::SelfSupersession { id: one.clone() });
        }
        let first = self.set_status(one, MemoryStatus::Conflicted)?;
        let second = self.set_status(other, MemoryStatus::Conflicted)?;
        Ok((first, second))
    }

    /// Settle a conflicted memory, if the caller is allowed to.
    ///
    /// A [`ConflictResolver::Automatic`] caller may settle a conflict only on
    /// a memory that is *not* high-impact. High-impact means an authority that
    /// [`MemoryAuthority::is_binding`] — an invariant, a constraint, or an
    /// accepted decision — **and also** an authority that has not been
    /// classified at all.
    ///
    /// Unclassified counting as high-impact is the conservative direction and
    /// is deliberate. `None` means nobody has judged how binding this memory
    /// is; treating "unknown" as "safe to resolve without review" would make
    /// every memory recorded before Phase 21A's classifier automatically
    /// resolvable, which is precisely the silent promotion the map's
    /// "treat uncertain authority classification conservatively" line forbids.
    /// Fail closed: an unknown authority needs review.
    ///
    /// A [`ConflictResolver::Reviewed`] caller may settle anything. The
    /// judgment is theirs; this method only records it.
    pub fn resolve_conflict(
        &self,
        id: &MemoryId,
        outcome: MemoryStatus,
        by: ConflictResolver,
    ) -> Result<MemoryRecord, MemoryStoreError> {
        let record = self
            .get(id)?
            .ok_or_else(|| MemoryStoreError::NotFound { id: id.clone() })?;

        if by == ConflictResolver::Automatic
            && let Some(impact) = high_impact_reason(record.authority)
        {
            return Err(MemoryStoreError::ReviewRequired {
                id: record.id,
                impact,
            });
        }

        self.set_status(id, outcome)
    }

    /// How many memories this project holds, by status.
    pub fn count(&self, status: MemoryStatus) -> Result<i64, MemoryStoreError> {
        self.conn
            .query_row(
                "SELECT COUNT(*) FROM memories WHERE status = ?1",
                [status.as_str()],
                |row| row.get(0),
            )
            .map_err(|source| MemoryStoreError::Sql {
                action: "count memories",
                source,
            })
    }
}

/// Build a record from a row of [`ALL_COLUMNS`].
///
/// The outer `Result` is SQLite's; the inner one is this module's, because an
/// unrecognized enum string is a data problem rather than a database problem
/// and deserves an error that names the column and the value. Nothing here
/// substitutes a default for an unreadable value: a memory whose kind cannot
/// be interpreted must not silently become a `finding`.
pub(super) fn row_to_record(
    row: &Row<'_>,
) -> rusqlite::Result<Result<MemoryRecord, MemoryStoreError>> {
    let id = MemoryId(row.get("id")?);
    let kind_text: String = row.get("kind")?;
    let status_text: String = row.get("status")?;
    let authority_text: Option<String> = row.get("authority")?;

    let Some(kind) = MemoryKind::from_stored(&kind_text) else {
        return Ok(Err(MemoryStoreError::UnknownValue {
            id,
            column: "kind",
            value: kind_text,
        }));
    };
    let Some(status) = MemoryStatus::from_stored(&status_text) else {
        return Ok(Err(MemoryStoreError::UnknownValue {
            id,
            column: "status",
            value: status_text,
        }));
    };
    let authority = match authority_text {
        None => None,
        Some(text) => match MemoryAuthority::from_stored(&text) {
            Some(authority) => Some(authority),
            None => {
                return Ok(Err(MemoryStoreError::UnknownValue {
                    id,
                    column: "authority",
                    value: text,
                }));
            }
        },
    };

    Ok(Ok(MemoryRecord {
        id,
        project_id: row.get("project_id")?,
        kind,
        authority,
        status,
        subject: row.get("subject")?,
        body: row.get("body")?,
        source_session_id: row.get("source_session_id")?,
        source_commit: row.get("source_commit")?,
        superseded_by: row.get::<_, Option<String>>("superseded_by")?.map(MemoryId),
        created_at: row.get("created_at")?,
        updated_at: row.get("updated_at")?,
    }))
}

/// Why a memory counts as high-impact for Phase 22's review requirement, or
/// `None` if it does not.
///
/// Returns the word used in the refusal, so the error says *which* of the two
/// reasons applied rather than making the reader guess.
fn high_impact_reason(authority: Option<MemoryAuthority>) -> Option<&'static str> {
    match authority {
        // Nobody has judged how binding this is. Unknown is not safe.
        None => Some("unclassified"),
        Some(authority) if authority.is_binding() => Some(authority.as_str()),
        Some(_) => None,
    }
}
