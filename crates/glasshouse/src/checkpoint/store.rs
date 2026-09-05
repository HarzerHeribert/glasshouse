//! Where checkpoints are kept.
//!
//! In the project's own SQLite database, in a table of their own — *store
//! checkpoints separately from durable project memory* is Phase 19's line,
//! and `checkpoints` and `memories` being different tables with different
//! shapes is what satisfies it. They are different things: a memory is
//! durable project knowledge that outlives every session, and a checkpoint is
//! bounded handoff context for one session that is only interesting until
//! somebody picks the work up.
//!
//! # The document is the checkpoint
//!
//! One column holds the rendered [`Checkpoint`]; the three beside it —
//! session, timestamp, reason — exist because queries need them and are
//! written from the same value in one place. Nothing else is lifted out, so
//! there is nothing for a row and its own document to disagree about.

use std::fmt;
use std::sync::Arc;

use rusqlite::{Connection, OptionalExtension};

use crate::database::PROJECT_ID_KEY;
use crate::session::SessionId;
use crate::session::store::Clock;

use super::{Checkpoint, FormatError};

/// A stored checkpoint's identifier.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct CheckpointId(String);

impl CheckpointId {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Enough of an identifier to name a checkpoint in conversation, the same
    /// twelve characters `glasshouse sessions` prints for a session.
    pub fn short(&self) -> String {
        self.0.chars().take(12).collect()
    }
}

impl fmt::Display for CheckpointId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.pad(&self.0)
    }
}

/// One checkpoint as it came out of the database.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stored {
    pub id: CheckpointId,
    pub checkpoint: Checkpoint,
}

/// Why a checkpoint could not be stored or read.
#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("no checkpoint `{id}` in this project")]
    NotFound { id: CheckpointId },
    #[error(
        "`{prefix}` matches {} checkpoints ({}); use more of the identifier",
        .matches.len(),
        .matches.iter().map(CheckpointId::as_str).collect::<Vec<_>>().join(", ")
    )]
    AmbiguousPrefix {
        prefix: String,
        matches: Vec<CheckpointId>,
    },
    #[error("`{prefix}` is not a checkpoint identifier; identifiers are hexadecimal")]
    MalformedId { prefix: String },
    #[error("checkpoint `{id}` could not be read back")]
    Format {
        id: CheckpointId,
        #[source]
        source: FormatError,
    },
    #[error(
        "a checkpoint rendered to {size} bytes, past the {bound}-byte bound; \
         this is a defect in whatever built it, because storing one trims first"
    )]
    TooLarge { size: usize, bound: usize },
    #[error("the project database has no project identifier bound")]
    UnboundDatabase,
    #[error("could not {action} in the project database")]
    Sql {
        action: &'static str,
        #[source]
        source: rusqlite::Error,
    },
}

/// An open project database plus the checkpoints inside it.
///
/// The owning counterpart of [`CheckpointStore`], for a caller that wants the
/// checkpoints and nothing else — exactly as
/// [`crate::session::ProjectSessions`] is for sessions.
pub struct ProjectCheckpoints {
    conn: Connection,
    project_id: String,
    clock: Clock,
}

impl fmt::Debug for ProjectCheckpoints {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ProjectCheckpoints")
            .field("project_id", &self.project_id)
            .finish_non_exhaustive()
    }
}

impl ProjectCheckpoints {
    pub fn open(runtime: &crate::Runtime) -> anyhow::Result<Self> {
        Self::open_with_clock(runtime, Arc::new(crate::session::store::system_clock))
    }

    /// [`ProjectCheckpoints::open`] with the clock replaced, so a test can
    /// assert on exact timestamps rather than sleeping.
    pub fn open_with_clock(runtime: &crate::Runtime, clock: Clock) -> anyhow::Result<Self> {
        let conn = crate::database::open(runtime)?;
        let project_id = CheckpointStore::new(&conn)?.project_id().to_owned();
        Ok(Self {
            conn,
            project_id,
            clock,
        })
    }

    pub fn store(&self) -> CheckpointStore<'_> {
        CheckpointStore {
            conn: &self.conn,
            project_id: self.project_id.clone(),
            clock: Arc::clone(&self.clock),
        }
    }
}

/// The checkpoints of one project.
pub struct CheckpointStore<'a> {
    conn: &'a Connection,
    project_id: String,
    clock: Clock,
}

impl fmt::Debug for CheckpointStore<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CheckpointStore")
            .field("project_id", &self.project_id)
            .finish_non_exhaustive()
    }
}

impl<'a> CheckpointStore<'a> {
    pub fn new(conn: &'a Connection) -> Result<Self, StoreError> {
        Self::with_clock(conn, Arc::new(crate::session::store::system_clock))
    }

    /// [`CheckpointStore::new`] with the clock replaced.
    ///
    /// The project identifier is read from the database's own binding rather
    /// than accepted as an argument, for the reason
    /// [`crate::session::SessionStore::new`] gives.
    pub fn with_clock(conn: &'a Connection, clock: Clock) -> Result<Self, StoreError> {
        let project_id: Option<String> = conn
            .query_row(
                "SELECT value FROM project_metadata WHERE key = ?1",
                [PROJECT_ID_KEY],
                |row| row.get(0),
            )
            .optional()
            .map_err(|source| StoreError::Sql {
                action: "read the project identifier",
                source,
            })?;
        Ok(Self {
            project_id: project_id.ok_or(StoreError::UnboundDatabase)?,
            conn,
            clock,
        })
    }

    pub fn project_id(&self) -> &str {
        &self.project_id
    }

    /// Store a checkpoint, trimming it to the bound first.
    ///
    /// [`Checkpoint::fit`] is called here rather than trusted to have been
    /// called, so no caller can store an oversized checkpoint by forgetting —
    /// and the schema's own `CHECK` is still behind that, for anything that
    /// does not come through this function at all.
    ///
    /// Returns what was actually stored, which is the fitted checkpoint and
    /// not necessarily the one handed in. A caller that showed the user the
    /// original would be showing them something the project does not hold.
    pub fn save(&self, checkpoint: Checkpoint) -> Result<Stored, StoreError> {
        let checkpoint = checkpoint.fit();
        let id = CheckpointId(self.generate_id()?);
        let document = checkpoint.render();

        // `fit` has just run, so this cannot fire for any checkpoint built
        // through the ordinary path. It is here so that the one case it
        // cannot fix — metadata alone past the bound — surfaces as a sentence
        // naming the size rather than as `CHECK constraint failed`, which
        // would reach the user through a background writer with no context at
        // all.
        if document.len() > crate::database::MAX_CHECKPOINT_BYTES {
            return Err(StoreError::TooLarge {
                size: document.len(),
                bound: crate::database::MAX_CHECKPOINT_BYTES,
            });
        }

        // `seq` is computed inside the statement rather than read first and
        // passed in. SQLite takes the write lock at the start of an `INSERT`,
        // before the subquery reads, so `MAX(seq) + 1` is evaluated under the
        // same lock that will do the write and two concurrent writers cannot
        // both see the same maximum. A read-then-insert from Rust would have
        // exactly that race, and the window would be small enough to never
        // show up in a test.
        self.conn
            .execute(
                "INSERT INTO checkpoints (id, project_id, session_id, created_at, \
                 reason, document, seq) VALUES (?1, ?2, ?3, ?4, ?5, ?6, \
                 (SELECT COALESCE(MAX(seq), 0) + 1 FROM checkpoints))",
                rusqlite::params![
                    id.as_str(),
                    &self.project_id,
                    checkpoint.session.as_str(),
                    checkpoint.created_at,
                    checkpoint.reason.as_str(),
                    &document,
                ],
            )
            .map_err(|source| StoreError::Sql {
                action: "store a checkpoint",
                source,
            })?;

        Ok(Stored { id, checkpoint })
    }

    /// The current wall-clock reading, for a caller building a checkpoint to
    /// hand straight back to [`CheckpointStore::save`].
    ///
    /// Exposed so that the store's clock is the one a checkpoint is stamped
    /// with, rather than a second reading taken somewhere else.
    pub fn now(&self) -> i64 {
        (self.clock)()
    }

    /// One checkpoint by identifier, or `Ok(None)` if it is simply not here.
    pub fn get(&self, id: &CheckpointId) -> Result<Option<Stored>, StoreError> {
        let row: Option<(String, String)> = self
            .conn
            .query_row(
                "SELECT id, document FROM checkpoints WHERE id = ?1",
                [id.as_str()],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .map_err(|source| StoreError::Sql {
                action: "look a checkpoint up",
                source,
            })?;
        row.map(read_stored).transpose()
    }

    /// The most recent checkpoint for one session — proved end to end
    /// against a real harness process in `tests/checkpoint_portability.rs`,
    /// both for a worker that died and for one put back afterwards.
    ///
    /// Order is `checkpoints.seq DESC, id DESC` — write order, not clock
    /// order: `created_at` reads whole seconds and cannot separate two
    /// checkpoints written inside one, so `seq`, a counter
    /// [`CheckpointStore::save`] stamps inside the insert, breaks the tie
    /// instead. Unlike a clock, `seq` cannot step backwards (NTP, a resumed
    /// laptop), and unlike the pre-version-14 `id DESC`-only tiebreak on
    /// `randomblob` — measured a coin flip, 414 of 798 same-second pairs
    /// resolving to the older checkpoint — it is not one. `id DESC` remains
    /// only for rows that never went through `save` and carry `seq`'s schema
    /// default of 0, and rows written before version 14 keep the
    /// `(created_at, id)` order migration 14 backfilled them with.
    ///
    /// History: design-decisions.md, "Trims: config, checkpoint, evaluation and codex module docs", store.rs `latest_for`.
    pub fn latest_for(&self, session: &SessionId) -> Result<Option<Stored>, StoreError> {
        self.first(
            "SELECT id, document FROM checkpoints WHERE session_id = ?1 \
             ORDER BY seq DESC, id DESC LIMIT 1",
            &[&session.as_str()],
        )
    }

    /// The most recent checkpoint in the project, whichever session it
    /// belongs to.
    ///
    /// This is what `glasshouse checkpoint show` and
    /// `glasshouse launch --from-checkpoint latest` resolve, and it orders
    /// the same way [`CheckpointStore::latest_for`] does — see that
    /// function's own doc, which has the measurement. The counter is stamped
    /// per project rather than per session, so this is a total order across
    /// every session's checkpoints and not a merge of several.
    pub fn latest(&self) -> Result<Option<Stored>, StoreError> {
        self.first(
            "SELECT id, document FROM checkpoints ORDER BY seq DESC, id DESC LIMIT 1",
            &[],
        )
    }

    /// Every checkpoint in the project, most recent first.
    ///
    /// The same order [`CheckpointStore::latest`] resolves, so the head of
    /// this list and the answer to *"the latest one"* can never be different
    /// checkpoints.
    pub fn list(&self) -> Result<Vec<Stored>, StoreError> {
        let mut statement = self
            .conn
            .prepare("SELECT id, document FROM checkpoints ORDER BY seq DESC, id DESC")
            .map_err(|source| StoreError::Sql {
                action: "prepare the checkpoint list",
                source,
            })?;
        let rows = statement
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })
            .map_err(|source| StoreError::Sql {
                action: "list checkpoints",
                source,
            })?;

        let mut out = Vec::new();
        for row in rows {
            let row = row.map_err(|source| StoreError::Sql {
                action: "read a checkpoint row",
                source,
            })?;
            out.push(read_stored(row)?);
        }
        Ok(out)
    }

    /// Resolve a whole identifier, or the leading part of one.
    ///
    /// The same contract [`crate::session::SessionStore::resolve_id`] has, and
    /// for the same reason: a listing prints twelve characters, so twelve
    /// characters have to be usable. Ambiguity is refused and names its
    /// candidates; matching uses `substr` rather than `LIKE`, so a `%` typed
    /// by the user is a character and not a wildcard.
    pub fn resolve_id(&self, prefix: &str) -> Result<CheckpointId, StoreError> {
        let prefix = prefix.trim();
        if prefix.is_empty() || !prefix.chars().all(|c| c.is_ascii_hexdigit()) {
            return Err(StoreError::MalformedId {
                prefix: prefix.to_owned(),
            });
        }
        let prefix = prefix.to_ascii_lowercase();

        let mut statement = self
            .conn
            .prepare("SELECT id FROM checkpoints WHERE substr(id, 1, ?2) = ?1 ORDER BY id")
            .map_err(|source| StoreError::Sql {
                action: "prepare the checkpoint lookup",
                source,
            })?;
        let matches: Vec<CheckpointId> = statement
            .query_map(
                rusqlite::params![&prefix, i64::try_from(prefix.len()).unwrap_or(i64::MAX)],
                |row| row.get::<_, String>(0).map(CheckpointId),
            )
            .and_then(|rows| rows.collect::<Result<Vec<_>, _>>())
            .map_err(|source| StoreError::Sql {
                action: "look a checkpoint up by identifier",
                source,
            })?;

        match matches.as_slice() {
            [] => Err(StoreError::NotFound {
                id: CheckpointId(prefix),
            }),
            [only] => Ok(only.clone()),
            _ => Err(StoreError::AmbiguousPrefix { prefix, matches }),
        }
    }

    fn first(
        &self,
        sql: &str,
        params: &[&dyn rusqlite::ToSql],
    ) -> Result<Option<Stored>, StoreError> {
        let row: Option<(String, String)> = self
            .conn
            .query_row(sql, params, |row| Ok((row.get(0)?, row.get(1)?)))
            .optional()
            .map_err(|source| StoreError::Sql {
                action: "read the most recent checkpoint",
                source,
            })?;
        row.map(read_stored).transpose()
    }

    fn generate_id(&self) -> Result<String, StoreError> {
        self.conn
            .query_row("SELECT lower(hex(randomblob(16)))", [], |row| row.get(0))
            .map_err(|source| StoreError::Sql {
                action: "generate a checkpoint identifier",
                source,
            })
    }
}

fn read_stored((id, document): (String, String)) -> Result<Stored, StoreError> {
    let id = CheckpointId(id);
    let checkpoint = Checkpoint::parse(&document).map_err(|source| StoreError::Format {
        id: id.clone(),
        source,
    })?;
    Ok(Stored { id, checkpoint })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::checkpoint::{CheckpointReason, GitPosition, Handoff};
    use crate::{Cli, Runtime};
    use clap::Parser;
    use std::path::{Path, PathBuf};

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

        fn store(&self) -> CheckpointStore<'_> {
            CheckpointStore::new(&self.conn).unwrap()
        }

        fn reopen(&self) -> Connection {
            crate::database::open(&self.runtime).unwrap()
        }

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

    fn checkpoint(session: &str, at: i64, reason: CheckpointReason) -> Checkpoint {
        Checkpoint {
            session: SessionId::new(session),
            harness: "a-harness".to_owned(),
            reason,
            created_at: at,
            git: Some(GitPosition {
                branch: Some("main".to_owned()),
                commit: "0123456789abcdef0123456789abcdef01234567".to_owned(),
            }),
            working_tree: None,
            handoff: Handoff {
                objective: "the objective".to_owned(),
                implementation_state: "the state".to_owned(),
                next_actions: vec!["carry on".to_owned()],
                ..Handoff::default()
            },
            trimmed: false,
        }
    }

    /// A checkpoint written by one process is read back by another, whole.
    /// The reopen is the point: this proves what is on disk, not what is in
    /// memory.
    #[test]
    fn a_checkpoint_survives_the_process_that_wrote_it() {
        let tmp = tempfile::tempdir().unwrap();
        let fixture = Fixture::new(tmp.path(), "alpha");
        let written = fixture
            .store()
            .save(checkpoint("session-a", 100, CheckpointReason::Manual))
            .unwrap();

        let reopened = fixture.reopen();
        let store = CheckpointStore::new(&reopened).unwrap();
        let read_back = store.get(&written.id).unwrap().expect("still there");
        assert_eq!(read_back, written);
    }

    /// A stored row and the document inside it describe the same checkpoint.
    ///
    /// The three lifted-out columns exist for queries, and this is what keeps
    /// them from becoming a second, quietly diverging copy.
    #[test]
    fn a_stored_row_never_disagrees_with_its_own_document() {
        let tmp = tempfile::tempdir().unwrap();
        let fixture = Fixture::new(tmp.path(), "alpha");
        let written = fixture
            .store()
            .save(checkpoint(
                "session-a",
                4242,
                CheckpointReason::TaskBoundary,
            ))
            .unwrap();

        let (session, created_at, reason): (String, i64, String) = fixture
            .conn
            .query_row(
                "SELECT session_id, created_at, reason FROM checkpoints WHERE id = ?1",
                [written.id.as_str()],
                |row| Ok((row.get_unwrap(0), row.get_unwrap(1), row.get_unwrap(2))),
            )
            .unwrap();

        assert_eq!(session, written.checkpoint.session.as_str());
        assert_eq!(created_at, written.checkpoint.created_at);
        assert_eq!(reason, written.checkpoint.reason.as_str());
    }

    /// The most recent checkpoint for a session is the most recent one, and
    /// another session's is never it.
    #[test]
    fn the_latest_checkpoint_is_per_session() {
        let tmp = tempfile::tempdir().unwrap();
        let fixture = Fixture::new(tmp.path(), "alpha");
        let store = fixture.store();

        store
            .save(checkpoint("session-a", 100, CheckpointReason::Manual))
            .unwrap();
        let newest = store
            .save(checkpoint("session-a", 200, CheckpointReason::TaskBoundary))
            .unwrap();
        store
            .save(checkpoint("session-b", 300, CheckpointReason::Manual))
            .unwrap();

        assert_eq!(
            store
                .latest_for(&SessionId::new("session-a"))
                .unwrap()
                .unwrap(),
            newest
        );
        assert_eq!(
            store
                .latest_for(&SessionId::new("session-b"))
                .unwrap()
                .unwrap()
                .checkpoint
                .created_at,
            300
        );
        assert!(
            store
                .latest_for(&SessionId::new("session-c"))
                .unwrap()
                .is_none()
        );
        assert_eq!(store.list().unwrap().len(), 3);
        assert_eq!(store.latest().unwrap().unwrap().checkpoint.created_at, 300);
    }

    /// The store trims, so a caller cannot save an oversized checkpoint by
    /// forgetting to — and what comes back is what was stored, not what was
    /// handed in.
    #[test]
    fn saving_trims_rather_than_trusting_the_caller() {
        let tmp = tempfile::tempdir().unwrap();
        let fixture = Fixture::new(tmp.path(), "alpha");

        let mut huge = checkpoint("session-a", 100, CheckpointReason::Manual);
        huge.handoff.failed_approaches =
            (0..3000).map(|i| format!("failed approach {i}")).collect();

        let stored = fixture.store().save(huge.clone()).unwrap();
        assert!(stored.checkpoint.trimmed);
        assert!(stored.checkpoint.render().len() <= crate::checkpoint::MAX_BYTES);
        assert_ne!(
            stored.checkpoint.handoff.failed_approaches.len(),
            huge.handoff.failed_approaches.len(),
            "the returned checkpoint must describe what was stored"
        );
    }

    /// And the database refuses one regardless of what the store did, which
    /// is what makes the bound a property of the file rather than of one
    /// function.
    /// And the database refuses one regardless of what the store did, at
    /// exactly the documented bound — which is what holds the SQL literal to
    /// [`crate::checkpoint::MAX_BYTES`]. Both directions, because a `CHECK`
    /// that refused everything would pass a one-sided test.
    #[test]
    fn the_schema_enforces_exactly_the_documented_bound() {
        let tmp = tempfile::tempdir().unwrap();
        let fixture = Fixture::new(tmp.path(), "alpha");
        let project_id = fixture.runtime.project().id().as_str().to_owned();

        let insert = |id: &str, size: usize| {
            fixture.conn.execute(
                "INSERT INTO checkpoints (id, project_id, session_id, created_at, reason, document) \
                 VALUES (?1, ?2, 's', 1, 'manual', ?3)",
                rusqlite::params![id, &project_id, "x".repeat(size)],
            )
        };

        assert!(
            insert("at-the-bound", crate::checkpoint::MAX_BYTES).is_ok(),
            "a document of exactly the bound must be storable"
        );
        assert!(
            insert("over", crate::checkpoint::MAX_BYTES + 1).is_err(),
            "one byte past the bound must be refused by the schema itself"
        );
    }

    /// Checkpoints are project-scoped structurally, the way sessions and
    /// memories are: the database will not accept another project's row at
    /// all, so no future query has to remember to filter.
    #[test]
    fn the_database_refuses_a_checkpoint_from_another_project() {
        let tmp = tempfile::tempdir().unwrap();
        let fixture = Fixture::new(tmp.path(), "alpha");
        let other = fixture.sibling("beta");

        let result = fixture.conn.execute(
            "INSERT INTO checkpoints (id, project_id, session_id, created_at, reason, document) \
             VALUES ('x', ?1, 's', 1, 'manual', '{}')",
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

    /// The short form a listing prints resolves, and an ambiguous prefix is
    /// refused by name rather than resolved to whichever row sorted first.
    #[test]
    fn a_prefix_resolves_and_ambiguity_is_refused() {
        let tmp = tempfile::tempdir().unwrap();
        let fixture = Fixture::new(tmp.path(), "alpha");
        let store = fixture.store();
        let stored = store
            .save(checkpoint("session-a", 100, CheckpointReason::Manual))
            .unwrap();

        assert_eq!(store.resolve_id(&stored.id.short()).unwrap(), stored.id);
        assert!(matches!(
            store.resolve_id("zz"),
            Err(StoreError::MalformedId { .. })
        ));
        assert!(matches!(
            store.resolve_id("abcdef"),
            Err(StoreError::NotFound { .. })
        ));

        // Two rows sharing a prefix: the lookup must name both rather than
        // pick one.
        fixture
            .conn
            .execute_batch(
                "INSERT INTO checkpoints (id, project_id, session_id, created_at, reason, document)
                 SELECT 'aaaa1111', project_id, session_id, created_at, reason, document
                   FROM checkpoints LIMIT 1;
                 INSERT INTO checkpoints (id, project_id, session_id, created_at, reason, document)
                 SELECT 'aaaa2222', project_id, session_id, created_at, reason, document
                   FROM checkpoints LIMIT 1;",
            )
            .unwrap();
        assert!(matches!(
            store.resolve_id("aaaa"),
            Err(StoreError::AmbiguousPrefix { .. })
        ));
    }

    /// A document this build cannot read is reported against its identifier,
    /// never guessed at and never silently skipped in a listing.
    #[test]
    fn an_unreadable_document_names_the_checkpoint_it_belongs_to() {
        let tmp = tempfile::tempdir().unwrap();
        let fixture = Fixture::new(tmp.path(), "alpha");
        let project_id = fixture.runtime.project().id().as_str().to_owned();
        fixture
            .conn
            .execute(
                "INSERT INTO checkpoints (id, project_id, session_id, created_at, reason, document) \
                 VALUES ('deadbeef', ?1, 's', 1, 'manual', 'not json at all')",
                [&project_id],
            )
            .unwrap();

        let store = fixture.store();
        let error = store
            .get(&CheckpointId::new("deadbeef"))
            .expect_err("a malformed document must not be readable");
        match error {
            StoreError::Format { id, .. } => assert_eq!(id.as_str(), "deadbeef"),
            other => panic!("expected Format, got {other:?}"),
        }
    }

    /// A `document` column holding bytes that are not valid UTF-8 — the shape
    /// a single flipped bit in an otherwise-intact row produces, invisible to
    /// `PRAGMA integrity_check` — must be a reported error from every reader,
    /// never a panic that takes down every later invocation. `get` here
    /// stands in for `list`, `latest_for`, and `latest`, which route through
    /// the same `first` helper and the same conversion.
    #[test]
    fn a_hostile_document_column_is_a_reported_error_not_a_panic() {
        let tmp = tempfile::tempdir().unwrap();
        let fixture = Fixture::new(tmp.path(), "alpha");
        let project_id = fixture.runtime.project().id().as_str().to_owned();
        fixture
            .conn
            .execute(
                "INSERT INTO checkpoints (id, project_id, session_id, created_at, reason, document) \
                 VALUES ('c0ffee00', ?1, 's', 1, 'manual', CAST(x'7b22ff7d' AS TEXT))",
                [&project_id],
            )
            .unwrap();

        let store = fixture.store();
        let error = store
            .get(&CheckpointId::new("c0ffee00"))
            .expect_err("a hostile column must not panic the caller");
        assert!(
            matches!(error, StoreError::Sql { .. }),
            "expected Sql, got {error:?}"
        );

        let error = store
            .list()
            .expect_err("list must not panic on the same hostile row");
        assert!(
            matches!(error, StoreError::Sql { .. }),
            "expected Sql, got {error:?}"
        );
    }
}
