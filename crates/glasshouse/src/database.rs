//! The per-project SQLite database.
//!
//! Each project owns exactly one SQLite database file, physically separate
//! from every other project's file, at `<state_dir>/glasshouse.db`. It is the
//! only future home for that project's memory (the Phase 20 memory table will
//! live here). Nothing else in Glasshouse is allowed to open a database file
//! anywhere else: the path is derived from [`crate::Runtime`], never accepted
//! from a caller.
//!
//! The module deliberately stays small: a deterministic migration mechanism
//! (`schema_migrations`) plus the initial `project_metadata` table that binds
//! the database to one project identifier. No sessions, credentials, WAL
//! configuration, full-text search, or async wrappers — later phases add those
//! on this foundation when they need them.
//!
//! Safety properties enforced on every open:
//!
//! - A newly created database file is owner-only (`0600` on Unix).
//! - A final database path that is a symbolic link is refused by an explicit
//!   `symlink_metadata` check performed on every launch. This handles the
//!   ordinary case; it is an open-time check, not a guarantee about files
//!   being swapped while Glasshouse runs.
//! - Any other non-regular entry at the final database path (directory,
//!   device, FIFO, socket) is refused as well; nothing but a regular file is
//!   ever opened or created there.
//! - A connection that SQLite could only open read-only (for example a
//!   mode-0400 file) is refused instead of silently degrading to a session
//!   that cannot store anything.
//! - A database whose recorded project identifier differs from the active
//!   project is refused; it must have been copied across projects.
//! - A database written by a newer Glasshouse (higher schema version) is
//!   refused. Corrupt or too-new databases are never deleted or recreated:
//!   the user keeps their data and decides what to do.

use std::fs;
use std::path::{Path, PathBuf};

use rusqlite::{Connection, OpenFlags, OptionalExtension};

use crate::Runtime;

/// File name of the project database inside the project state directory.
pub(crate) const DATABASE_FILE_NAME: &str = "glasshouse.db";

/// The highest schema version this build knows how to migrate to.
///
/// Version 1 is the empty-but-initialized schema plus the `project_metadata`
/// table. Version 2 adds `sessions`. Later migrations are appended to
/// [`MIGRATIONS`], and this constant moves with them.
const SUPPORTED_SCHEMA_VERSION: i64 = 2;

/// Migration `index + 1` upgrades a database from schema version `index` to
/// version `index + 1`. Migrations run in order inside one transaction, so a
/// partially applied upgrade can never be observed.
///
/// Migrations are append-only. Editing one that has shipped would leave
/// already-migrated databases silently disagreeing with new ones, because the
/// recorded version would match while the schema did not.
const MIGRATIONS: [&str; SUPPORTED_SCHEMA_VERSION as usize] = [
    // 1: identity of the project this database belongs to. Memory (Phase 20)
    // and everything else project-scoped joins against these rows.
    "
    CREATE TABLE project_metadata (
        key TEXT PRIMARY KEY,
        value TEXT NOT NULL
    ) WITHOUT ROWID;
    ",
    // 2: Glasshouse session metadata.
    //
    // This is Glasshouse's own record of a session and is deliberately not
    // derived from any harness's session files: `native_session_id` is a
    // nullable *reference* to the harness's own identifier, never the source
    // of truth. A session exists here whether or not the harness kept a file,
    // and deleting the harness's history does not delete this row.
    //
    // The `CHECK` constraints keep the enum columns honest at the storage
    // layer, so a future writer cannot invent a lifecycle value that readers
    // would have to guess about.
    //
    // The two triggers are the structural half of the project-isolation rule.
    // Filtering by `project_id` in queries would be a convention any new query
    // could forget; a `BEFORE INSERT`/`BEFORE UPDATE` guard cannot be
    // forgotten, because SQLite enforces it against the binding in
    // `project_metadata` no matter which code writes the row. `IS NOT` rather
    // than `<>` is deliberate: if the binding row were somehow missing, the
    // subquery yields NULL and `<>` would silently evaluate to NULL and let
    // the write through, whereas `IS NOT` aborts. The guard fails closed.
    "
    CREATE TABLE sessions (
        id                TEXT PRIMARY KEY,
        project_id        TEXT NOT NULL,
        harness           TEXT NOT NULL,
        native_session_id TEXT,
        role              TEXT NOT NULL
            CHECK (role IN ('normal', 'orchestrator', 'worker')),
        lifecycle         TEXT NOT NULL
            CHECK (lifecycle IN ('starting', 'running', 'idle',
                                 'waiting_for_user', 'stopped', 'failed',
                                 'closed')),
        presentation      TEXT NOT NULL
            CHECK (presentation IN ('embedded', 'headless', 'external')),
        created_at        INTEGER NOT NULL,
        last_activity_at  INTEGER NOT NULL
    ) WITHOUT ROWID;

    CREATE INDEX sessions_by_last_activity
        ON sessions (last_activity_at DESC);

    -- A native session belongs to at most one Glasshouse session, which is
    -- what makes the column a mapping rather than a loose annotation. Scoped
    -- per harness because two harnesses may coincidentally use the same
    -- identifier format.
    --
    -- The `WHERE` clause is not what lets many sessions sit without a native
    -- identifier: SQLite already treats NULLs as distinct in a unique index,
    -- so they would never collide either way. It is here to keep the index
    -- from carrying an entry for every not-yet-identified session, and to say
    -- plainly that the constraint is about real identifiers. Sentinel values
    -- would break that — an empty-string default in place of NULL really
    -- would collide.
    CREATE UNIQUE INDEX sessions_native_id
        ON sessions (harness, native_session_id)
        WHERE native_session_id IS NOT NULL;

    CREATE TRIGGER sessions_reject_foreign_project_insert
    BEFORE INSERT ON sessions
    FOR EACH ROW
    WHEN NEW.project_id IS NOT (
        SELECT value FROM project_metadata WHERE key = 'project_id'
    )
    BEGIN
        SELECT RAISE(ABORT, 'session belongs to a different project');
    END;

    CREATE TRIGGER sessions_reject_foreign_project_update
    BEFORE UPDATE OF project_id ON sessions
    FOR EACH ROW
    WHEN NEW.project_id IS NOT (
        SELECT value FROM project_metadata WHERE key = 'project_id'
    )
    BEGIN
        SELECT RAISE(ABORT, 'session belongs to a different project');
    END;
    ",
];

pub(crate) const PROJECT_ID_KEY: &str = "project_id";

/// Everything that can go wrong while preparing a project database.
///
/// Every variant carries the database path in its message so an error is
/// actionable even when it surfaces far from where the path was known.
#[derive(Debug, thiserror::Error)]
pub(crate) enum DatabaseError {
    #[error(
        "project database `{path}` is a symbolic link; refusing to follow it \
         because the link target could change what Glasshouse reads and writes"
    )]
    Symlinked { path: PathBuf },
    #[error("project database `{path}` exists but is {actual}; refusing to use it as a database")]
    NotARegularFile { path: PathBuf, actual: &'static str },
    #[error("could not inspect project database `{path}`")]
    Inspect {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("could not create project database `{path}`")]
    Create {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("could not open project database `{path}`")]
    Open {
        path: PathBuf,
        #[source]
        source: rusqlite::Error,
    },
    #[error("could not configure project database `{path}`")]
    Configure {
        path: PathBuf,
        #[source]
        source: rusqlite::Error,
    },
    #[error(
        "project database `{path}` was opened read-only; Glasshouse cannot \
         store project memory in a database it cannot write to, so check the \
         file's permissions"
    )]
    ReadOnly { path: PathBuf },
    #[error("could not prepare the schema of project database `{path}`")]
    Sql {
        path: PathBuf,
        #[source]
        source: rusqlite::Error,
    },
    #[error(
        "project database `{path}` was written by a newer Glasshouse \
         (schema version {found}; this build supports up to {supported}); \
         refusing to guess how to read it"
    )]
    TooNew {
        path: PathBuf,
        found: i64,
        supported: i64,
    },
    #[error(
        "project database `{path}` belongs to project `{actual}`, not to the \
         active project `{expected}`; refusing to mix project memories"
    )]
    ProjectMismatch {
        path: PathBuf,
        expected: String,
        actual: String,
    },
    #[error(
        "project database `{path}` has project metadata but no project identifier; \
         refusing to adopt an unbound database because it may belong to another project"
    )]
    MissingProjectId { path: PathBuf },
}

/// Create or validate the project database of the given runtime's project.
///
/// Both the database path (`<state_dir>/glasshouse.db`) and the binding
/// project identifier are derived from `runtime`; no caller — inside or
/// outside the crate — can point this initializer at another file or bind it
/// to another project.
///
/// Called from `bootstrap`, so a successful [`crate::Runtime`] always has a
/// valid project database waiting in its state directory. On success the
/// connection is closed again; nothing holds it open between launches.
///
/// Use [`open`] instead when the caller actually needs to read or write.
pub(crate) fn ensure_ready(runtime: &Runtime) -> Result<(), DatabaseError> {
    // Dropping the connection closes it. Validation is the point of the call.
    open(runtime).map(drop)
}

/// Open the project database, applying every check [`ensure_ready`] applies,
/// and hand back the live connection.
///
/// This is the only way anything in Glasshouse obtains a usable connection, so
/// the symlink refusal, the read-only refusal, the project-identity check, and
/// the migrations are not steps a caller can skip or reorder. The path and the
/// binding identifier both come from `runtime`; neither is a parameter.
pub(crate) fn open(runtime: &Runtime) -> Result<Connection, DatabaseError> {
    let db_path = runtime.database_path();
    let project_id = runtime.project().id().as_str();

    prepare_file(&db_path)?;

    let mut conn = Connection::open_with_flags(
        &db_path,
        // No SQLITE_OPEN_CREATE: the file was just created above with the
        // right permissions, and if it vanished since then we want the open
        // to fail rather than silently recreate it.
        //
        // No SQLITE_OPEN_NOFOLLOW either, despite being offered: it makes
        // SQLite reject a symlink in *any* path component, not just the final
        // one, which breaks entirely legitimate locations such as macOS's
        // `/var` -> `/private/var`. A symlink at the final database path is
        // refused explicitly by `prepare_file` instead.
        OpenFlags::SQLITE_OPEN_READ_WRITE,
    )
    .map_err(|source| DatabaseError::Open {
        path: db_path.clone(),
        source,
    })?;

    // SQLITE_OPEN_READWRITE degrades silently to a read-only connection when
    // the file itself is not writable (e.g. mode 0400). That must not pass:
    // every later write — memory included — would fail far from this check.
    configure(&conn, &db_path)?;

    // Identity first, read-only: if an existing database is bound to another
    // project, refuse before any write is even attempted, so even a copied
    // database whose migration state looks stale or absent is left
    // byte-for-byte unmodified by the failed attempt.
    verify_identity(&conn, &db_path, project_id)?;

    // One BEGIN IMMEDIATE transaction from before the first schema statement
    // until after the project binding: concurrent first launches serialize on
    // SQLite's write lock instead of racing between "read version" and
    // "create table" or between "query binding" and "insert binding". Losers
    // of the lock wait here (bounded by the busy timeout), then see the
    // winner's committed state and proceed idempotently.
    let tx = conn
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .map_err(|source| DatabaseError::Sql {
            path: db_path.clone(),
            source,
        })?;

    migrate(&tx, &db_path)?;
    bind_project(&tx, &db_path, project_id)?;

    tx.commit().map_err(|source| DatabaseError::Sql {
        path: db_path.clone(),
        source,
    })?;

    Ok(conn)
}

/// Per-connection configuration that must hold before any work happens.
fn configure(conn: &Connection, db_path: &Path) -> Result<(), DatabaseError> {
    let configure_err = |source| DatabaseError::Configure {
        path: db_path.to_path_buf(),
        source,
    };

    // Bound wait instead of an immediate `database is locked` failure when
    // another Glasshouse process holds the write lock briefly.
    conn.busy_timeout(std::time::Duration::from_secs(5))
        .map_err(configure_err)?;

    if conn.is_readonly("main").map_err(configure_err)? {
        return Err(DatabaseError::ReadOnly {
            path: db_path.to_path_buf(),
        });
    }

    Ok(())
}

/// Refuse an existing metadata table that is unbound or belongs to a different
/// project. A genuinely brand-new database has no metadata table yet and passes
/// straight through to migration and [`bind_project`].
fn verify_identity(
    conn: &Connection,
    db_path: &Path,
    project_id: &str,
) -> Result<(), DatabaseError> {
    let sql_err = |source| DatabaseError::Sql {
        path: db_path.to_path_buf(),
        source,
    };

    let table_present: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master \
             WHERE type = 'table' AND name = 'project_metadata'",
            [],
            |row| row.get(0),
        )
        .map_err(sql_err)?;
    if table_present == 0 {
        return Ok(());
    }

    let stored: Option<String> = conn
        .query_row(
            "SELECT value FROM project_metadata WHERE key = ?1",
            [PROJECT_ID_KEY],
            |row| row.get(0),
        )
        .optional()
        .map_err(sql_err)?;

    match stored {
        Some(actual) if actual != project_id => Err(DatabaseError::ProjectMismatch {
            path: db_path.to_path_buf(),
            expected: project_id.to_owned(),
            actual,
        }),
        Some(_) => Ok(()),
        None => Err(DatabaseError::MissingProjectId {
            path: db_path.to_path_buf(),
        }),
    }
}

/// Inspect the final database path; refuse symlinks and non-regular entries.
///
/// Returns `Ok(false)` only when the path definitively does not exist (so the
/// caller should create it), `Ok(true)` when an existing regular file is
/// ready to open. Any other inspection failure — permission denied and
/// friends — is preserved with its source rather than being mistaken for
/// permission to create the file.
fn check_existing(db_path: &Path) -> Result<bool, DatabaseError> {
    let metadata = match fs::symlink_metadata(db_path) {
        Ok(metadata) => metadata,
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => {
            return Ok(false);
        }
        Err(source) => {
            return Err(DatabaseError::Inspect {
                path: db_path.to_path_buf(),
                source,
            });
        }
    };
    if metadata.file_type().is_symlink() {
        return Err(DatabaseError::Symlinked {
            path: db_path.to_path_buf(),
        });
    }
    if !metadata.is_file() {
        // Anything that is not a regular file — a directory, a device, a
        // FIFO, a socket — must not be opened as (or replaced by) a
        // database. Special files in particular could block or misbehave
        // when SQLite tries to read and write them.
        return Err(DatabaseError::NotARegularFile {
            path: db_path.to_path_buf(),
            actual: describe_entry(&metadata),
        });
    }
    // An existing regular file keeps whatever permissions it has;
    // like `create_state_dir`, this call neither widens nor narrows.
    Ok(true)
}

/// Human-readable kind of a final-path entry, for error messages.
fn describe_entry(metadata: &fs::Metadata) -> &'static str {
    let file_type = metadata.file_type();
    if file_type.is_dir() {
        "a directory"
    } else if file_type.is_file() {
        "a regular file"
    } else if file_type.is_symlink() {
        "a symbolic link"
    } else {
        "a special file (device, FIFO, socket, ...)"
    }
}

/// Make sure a regular file exists at `db_path`, created owner-only if new,
/// without following a symlink that may sit at the final component.
///
/// Only a definitive `NotFound` from the inspection counts as "absent"; any
/// other failure is preserved with its source instead of being mistaken for
/// permission to create the file. If creation loses an `AlreadyExists` race
/// with another Glasshouse process, the winning file is re-inspected — it
/// gets no free pass past the symlink refusal.
fn prepare_file(db_path: &Path) -> Result<(), DatabaseError> {
    match check_existing(db_path) {
        Ok(true) => return Ok(()),
        Ok(false) => {}
        Err(err) => return Err(err),
    }

    // Create the file ourselves instead of letting SQLite do it, because
    // SQLite would use plain `0644 &! umask` — world-readable, which no
    // project memory ever should be.
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    match options.open(db_path) {
        Ok(_) => Ok(()),
        Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => {
            // Lost the race; hold the winner to the same checks.
            check_existing(db_path).map(|_| ())
        }
        Err(source) => Err(DatabaseError::Create {
            path: db_path.to_path_buf(),
            source,
        }),
    }
}

/// Apply pending migrations deterministically and refuse anything this build
/// cannot handle.
///
/// Runs inside the caller's `BEGIN IMMEDIATE` transaction: the ledger is
/// created, read, and advanced under SQLite's write lock, so two concurrent
/// first launches can never interleave "read version 0" with "create table".
/// No commit happens here; [`ensure_ready`] commits once after the project
/// binding is also in place.
fn migrate(conn: &Connection, db_path: &Path) -> Result<(), DatabaseError> {
    let sql_err = |source| DatabaseError::Sql {
        path: db_path.to_path_buf(),
        source,
    };

    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS schema_migrations (
             version INTEGER PRIMARY KEY
         );",
    )
    .map_err(sql_err)?;

    let current: i64 = conn
        .query_row(
            "SELECT COALESCE(MAX(version), 0) FROM schema_migrations",
            [],
            |row| row.get(0),
        )
        .map_err(sql_err)?;

    if current > SUPPORTED_SCHEMA_VERSION {
        return Err(DatabaseError::TooNew {
            path: db_path.to_path_buf(),
            found: current,
            supported: SUPPORTED_SCHEMA_VERSION,
        });
    }

    for (index, script) in MIGRATIONS.iter().enumerate() {
        let version = index as i64 + 1;
        if version <= current {
            continue;
        }
        conn.execute_batch(script).map_err(sql_err)?;
        conn.execute(
            "INSERT INTO schema_migrations (version) VALUES (?1)",
            [version],
        )
        .map_err(sql_err)?;
    }

    Ok(())
}

/// Bind the database to the active project, or verify an existing binding.
///
/// Runs inside the caller's `BEGIN IMMEDIATE` transaction, so the
/// "query binding, then insert if absent" pair cannot interleave with another
/// launcher's. A stored identifier that differs from the active one means
/// this file was copied or moved across projects; opening it would silently
/// merge two projects' memories, so it is refused instead.
fn bind_project(conn: &Connection, db_path: &Path, project_id: &str) -> Result<(), DatabaseError> {
    let sql_err = |source| DatabaseError::Sql {
        path: db_path.to_path_buf(),
        source,
    };

    let stored: Option<String> = conn
        .query_row(
            "SELECT value FROM project_metadata WHERE key = ?1",
            [PROJECT_ID_KEY],
            |row| row.get(0),
        )
        .optional()
        .map_err(sql_err)?;

    match stored {
        Some(actual) if actual != project_id => Err(DatabaseError::ProjectMismatch {
            path: db_path.to_path_buf(),
            expected: project_id.to_owned(),
            actual,
        }),
        Some(_) => Ok(()),
        None => {
            conn.execute(
                "INSERT INTO project_metadata (key, value) VALUES (?1, ?2)",
                [PROJECT_ID_KEY, project_id],
            )
            .map_err(sql_err)?;
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Cli, Runtime};
    use clap::Parser;
    use std::path::PathBuf;

    /// A project rooted inside `base`'s `workspace/`, bootstrapped against
    /// `base`'s `data/` and `config/`. Fixtures sharing one `base` therefore
    /// share one GLASSHOUSE data/config root, like two real projects on one
    /// machine.
    struct Fixture {
        base: PathBuf,
        runtime: Runtime,
    }

    impl Fixture {
        fn new(base: &Path, name: &str) -> Self {
            let root = base.join("workspace").join(name);
            std::fs::create_dir_all(root.join(".git")).unwrap();
            let root = std::fs::canonicalize(&root).unwrap();

            let cli = Cli::try_parse_from([
                "glasshouse",
                "--data-dir",
                base.join("data").to_str().unwrap(),
                "--config-dir",
                base.join("config").to_str().unwrap(),
            ])
            .unwrap();
            let runtime = crate::bootstrap(&cli, &root).unwrap();
            Fixture {
                base: base.to_path_buf(),
                runtime,
            }
        }

        /// Bootstrap the same project again, exactly as a later launch would.
        fn rebootstrap(&self) -> anyhow::Result<Runtime> {
            let cli = Cli::try_parse_from([
                "glasshouse",
                "--data-dir",
                self.base.join("data").to_str().unwrap(),
                "--config-dir",
                self.base.join("config").to_str().unwrap(),
            ])
            .unwrap();
            crate::bootstrap(&cli, self.runtime.project().root())
        }
    }

    fn stored_project_id(db_path: &Path) -> String {
        let conn = Connection::open(db_path).unwrap();
        conn.query_row(
            "SELECT value FROM project_metadata WHERE key = 'project_id'",
            [],
            |row| row.get(0),
        )
        .unwrap()
    }

    fn schema_version(db_path: &Path) -> i64 {
        let conn = Connection::open(db_path).unwrap();
        conn.query_row("SELECT MAX(version) FROM schema_migrations", [], |row| {
            row.get(0)
        })
        .unwrap()
    }

    #[test]
    fn bootstrap_creates_the_project_database() {
        let tmp = tempfile::tempdir().unwrap();
        let fixture = Fixture::new(tmp.path(), "alpha");
        let db = fixture.runtime.database_path();

        assert_eq!(db.file_name().unwrap(), DATABASE_FILE_NAME);
        assert!(db.is_file());
        assert_eq!(
            stored_project_id(&db),
            fixture.runtime.project().id().as_str()
        );
        assert_eq!(schema_version(&db), SUPPORTED_SCHEMA_VERSION);
    }

    #[test]
    fn two_projects_sharing_one_data_root_get_separate_databases_and_ids() {
        let tmp = tempfile::tempdir().unwrap();
        // alpha and beta resolve against the SAME GLASSHOUSE data/config
        // root; only their project identities differ.
        let alpha = Fixture::new(tmp.path(), "alpha");
        let beta = Fixture::new(tmp.path(), "beta");

        let alpha_db = alpha.runtime.database_path();
        let beta_db = beta.runtime.database_path();

        // Both databases live under the one shared projects root...
        let projects_root = tmp.path().join("data").join("projects");
        assert_eq!(alpha_db.parent().unwrap().parent().unwrap(), projects_root);
        assert_eq!(beta_db.parent().unwrap().parent().unwrap(), projects_root);
        // ...yet in physically different files and directories.
        assert_ne!(alpha_db.parent(), beta_db.parent());
        assert_ne!(alpha_db, beta_db);

        // And each file records its own project, not its neighbour's.
        let alpha_id = stored_project_id(&alpha_db);
        let beta_id = stored_project_id(&beta_db);
        assert_ne!(alpha_id, beta_id);
        assert_eq!(alpha_id, alpha.runtime.project().id().as_str());
        assert_eq!(beta_id, beta.runtime.project().id().as_str());
    }

    #[test]
    fn reopening_is_idempotent() {
        let tmp = tempfile::tempdir().unwrap();
        let fixture = Fixture::new(tmp.path(), "alpha");
        let db = fixture.runtime.database_path();

        let id_before = stored_project_id(&db);
        let version_before = schema_version(&db);

        // Reopen through bootstrap several times: nothing may drift.
        for _ in 0..3 {
            let runtime = fixture.rebootstrap().unwrap();
            assert_eq!(runtime.database_path(), db);
        }

        assert_eq!(stored_project_id(&db), id_before);
        assert_eq!(schema_version(&db), version_before);
    }

    #[test]
    fn concurrent_first_bootstraps_serialize_on_one_database() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("workspace").join("solo");
        std::fs::create_dir_all(root.join(".git")).unwrap();
        let root = std::fs::canonicalize(&root).unwrap();

        const CALLERS: usize = 16;
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(CALLERS));
        let mut handles = Vec::new();
        for _ in 0..CALLERS {
            let barrier = std::sync::Arc::clone(&barrier);
            let root = root.clone();
            let data = tmp.path().join("data");
            let config = tmp.path().join("config");
            handles.push(std::thread::spawn(move || {
                // Release all callers at once so the very first creation of
                // the database file and schema is genuinely contended.
                barrier.wait();
                let cli = Cli::try_parse_from([
                    "glasshouse",
                    "--data-dir",
                    data.to_str().unwrap(),
                    "--config-dir",
                    config.to_str().unwrap(),
                ])
                .unwrap();
                crate::bootstrap(&cli, &root)
            }));
        }

        let mut results = Vec::new();
        for handle in handles {
            results.push(handle.join().expect("bootstrap thread panicked"));
        }
        for result in &results {
            result
                .as_ref()
                .expect("a concurrent first bootstrap failed");
        }

        // All callers agree on one physical database with one binding.
        let expected_db = results[0].as_ref().unwrap().database_path();
        let expected_id = results[0].as_ref().unwrap().project().id().as_str();
        for result in &results {
            let runtime = result.as_ref().unwrap();
            assert_eq!(runtime.database_path(), expected_db);
        }
        assert_eq!(schema_version(&expected_db), SUPPORTED_SCHEMA_VERSION);
        assert_eq!(stored_project_id(&expected_db), expected_id);

        let conn = Connection::open(&expected_db).unwrap();
        let bindings: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM project_metadata WHERE key = 'project_id'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(bindings, 1);
    }

    #[test]
    fn mismatched_copied_database_is_rejected() {
        let tmp = tempfile::tempdir().unwrap();
        let alpha = Fixture::new(tmp.path(), "alpha");
        let beta = Fixture::new(tmp.path(), "beta");

        // Copy alpha's whole database into beta's slot.
        std::fs::copy(alpha.runtime.database_path(), beta.runtime.database_path()).unwrap();

        let err = beta.rebootstrap().unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("belongs to project"), "{msg}");
        assert!(msg.contains(alpha.runtime.project().id().as_str()), "{msg}");
        assert!(
            msg.contains(beta.runtime.database_path().display().to_string().as_str()),
            "{msg}"
        );

        // The copy is left untouched for the user to decide about.
        assert_eq!(
            stored_project_id(&beta.runtime.database_path()),
            stored_project_id(&alpha.runtime.database_path())
        );
    }

    #[test]
    fn metadata_without_a_project_id_is_rejected_and_not_adopted() {
        let tmp = tempfile::tempdir().unwrap();
        let fixture = Fixture::new(tmp.path(), "alpha");
        let db = fixture.runtime.database_path();

        {
            let conn = Connection::open(&db).unwrap();
            conn.execute(
                "DELETE FROM project_metadata WHERE key = ?1",
                [PROJECT_ID_KEY],
            )
            .unwrap();
        }

        let err = fixture.rebootstrap().unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("no project identifier"), "{msg}");
        assert!(msg.contains(db.display().to_string().as_str()), "{msg}");

        // Rejection happens during the read-only identity preflight: the
        // missing binding is not silently recreated for the active project.
        let conn = Connection::open(&db).unwrap();
        let bindings: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM project_metadata WHERE key = ?1",
                [PROJECT_ID_KEY],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(bindings, 0);
        assert_eq!(schema_version(&db), SUPPORTED_SCHEMA_VERSION);
    }

    #[test]
    fn too_new_schema_is_rejected_and_not_recreated() {
        let tmp = tempfile::tempdir().unwrap();
        let fixture = Fixture::new(tmp.path(), "alpha");
        let db = fixture.runtime.database_path();

        {
            // What a newer Glasshouse would leave behind: this build's
            // migrations, plus one it has never heard of. Appending rather
            // than rewriting the existing rows keeps the fixture correct as
            // more migrations are added.
            let conn = Connection::open(&db).unwrap();
            conn.execute_batch("INSERT INTO schema_migrations (version) VALUES (99);")
                .unwrap();
        }

        let err = fixture.rebootstrap().unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("newer"), "{msg}");
        assert!(msg.contains("99"), "{msg}");
        assert!(msg.contains(db.display().to_string().as_str()), "{msg}");

        // Refused, not deleted or recreated: the too-new marker survives.
        assert!(db.is_file());
        assert_eq!(schema_version(&db), 99);
    }

    #[test]
    fn corrupt_database_is_refused_and_never_recreated() {
        let tmp = tempfile::tempdir().unwrap();
        let fixture = Fixture::new(tmp.path(), "alpha");
        let db = fixture.runtime.database_path();

        std::fs::write(&db, b"definitely not a sqlite database").unwrap();

        let err = fixture.rebootstrap().unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains(db.display().to_string().as_str()), "{msg}");

        // Still the same garbage bytes: nothing was silently wiped.
        assert_eq!(
            std::fs::read(&db).unwrap(),
            b"definitely not a sqlite database"
        );
    }

    #[test]
    fn directory_at_the_database_path_is_rejected_and_not_replaced() {
        let tmp = tempfile::tempdir().unwrap();
        let fixture = Fixture::new(tmp.path(), "alpha");
        let db = fixture.runtime.database_path();

        // Put a plain directory where the database belongs.
        std::fs::remove_file(&db).unwrap();
        std::fs::create_dir(&db).unwrap();

        let err = fixture.rebootstrap().unwrap_err();
        let msg = format!("{err:#}");
        assert!(
            msg.contains("not a regular file") || msg.contains("a directory"),
            "{msg}"
        );
        assert!(msg.contains(db.display().to_string().as_str()), "{msg}");

        // The directory is still there: nothing deleted or recreated it.
        assert!(db.is_dir());
    }

    #[test]
    fn foreign_database_with_pending_migrations_is_rejected_before_any_writes() {
        let tmp = tempfile::tempdir().unwrap();
        let alpha = Fixture::new(tmp.path(), "alpha");
        let beta = Fixture::new(tmp.path(), "beta");

        // Copy alpha's bound database into beta's slot, then make its
        // migration state look pending by dropping the migration ledger.
        // An implementation that migrated before checking identity would
        // recreate the ledger and write into this foreign database.
        std::fs::copy(alpha.runtime.database_path(), beta.runtime.database_path()).unwrap();
        {
            let conn = Connection::open(beta.runtime.database_path()).unwrap();
            conn.execute_batch("DROP TABLE schema_migrations;").unwrap();
        }

        let err = beta.rebootstrap().unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("belongs to project"), "{msg}");
        assert!(msg.contains(alpha.runtime.project().id().as_str()), "{msg}");
        assert!(
            msg.contains(beta.runtime.database_path().display().to_string().as_str()),
            "{msg}"
        );

        // The refusal happened before any schema work: the ledger is still
        // absent and the foreign binding untouched.
        let conn = Connection::open(beta.runtime.database_path()).unwrap();
        let ledger_present: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master \
                 WHERE type = 'table' AND name = 'schema_migrations'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(ledger_present, 0);
        assert_eq!(
            stored_project_id(&beta.runtime.database_path()),
            alpha.runtime.project().id().as_str()
        );
    }

    #[cfg(unix)]
    #[test]
    fn readonly_database_file_is_rejected() {
        use std::os::unix::fs::PermissionsExt;

        // Root can write to 0400 files regardless; the scenario does not
        // exist for that user, so the regression test says nothing there.
        if unsafe { libc::geteuid() } == 0 {
            return;
        }

        let tmp = tempfile::tempdir().unwrap();
        let fixture = Fixture::new(tmp.path(), "alpha");
        let db = fixture.runtime.database_path();

        std::fs::set_permissions(&db, fs::Permissions::from_mode(0o400)).unwrap();

        let err = fixture.rebootstrap().unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("read-only"), "{msg}");
        assert!(msg.contains(db.display().to_string().as_str()), "{msg}");

        // Restore so the temp directory can be cleaned up.
        std::fs::set_permissions(&db, fs::Permissions::from_mode(0o600)).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn new_database_file_is_owner_only() {
        use std::os::unix::fs::PermissionsExt;

        let tmp = tempfile::tempdir().unwrap();
        let fixture = Fixture::new(tmp.path(), "alpha");
        let mode = std::fs::metadata(fixture.runtime.database_path())
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600, "new database must be owner-only");
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_final_database_path_is_rejected() {
        use std::os::unix::fs::symlink;

        let tmp = tempfile::tempdir().unwrap();
        let fixture = Fixture::new(tmp.path(), "alpha");
        let db = fixture.runtime.database_path();

        // Replace the real database with a symlink to an unrelated file.
        let decoy = tmp.path().join("decoy.db");
        std::fs::write(&decoy, b"decoy").unwrap();
        std::fs::remove_file(&db).unwrap();
        symlink(&decoy, &db).unwrap();

        let err = fixture.rebootstrap().unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("symbolic link"), "{msg}");
        assert!(msg.contains(db.display().to_string().as_str()), "{msg}");

        // The symlink itself is left alone; nothing followed or replaced it.
        assert!(
            std::fs::symlink_metadata(&db)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert_eq!(std::fs::read(&decoy).unwrap(), b"decoy");
    }
}
