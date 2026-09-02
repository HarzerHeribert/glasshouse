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
//! (`schema_migrations`), the `project_metadata` table that binds the database
//! to one project identifier, and the tables later phases have needed —
//! `sessions` and, from version 4, `memories` with its FTS5 index. It holds no
//! credentials, no WAL configuration, and no async wrappers; what a table
//! *means* lives with the module that owns it ([`crate::session::store`],
//! [`crate::memory`]), and only the schema itself lives here.
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

use std::path::{Path, PathBuf};

use rusqlite::{Connection, OpenFlags};

use crate::Runtime;

/// File name of the project database inside the project state directory.
pub(crate) const DATABASE_FILE_NAME: &str = "glasshouse.db";

mod bootstrap;
mod migrations;
mod schema;
#[cfg(test)]
mod tests;

pub(crate) use migrations::PROJECT_ID_KEY;
#[cfg(test)]
pub(crate) use schema::MEMORY_FILE_PROVENANCE;
pub(crate) use schema::{EVALUATION_KINDS, LIFECYCLE_EVENT_KINDS, MAX_CHECKPOINT_BYTES};

use bootstrap::prepare_file;
use migrations::{bind_project, migrate, verify_identity};

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
    #[error(
        "project database `{path}` exists but is empty (zero bytes); a genuinely new \
         project has no database file at all, so an existing file this size means it was \
         likely truncated — by a crashed copy, an interrupted restore, or a disk-full \
         write. Restore it from a backup, or delete the file deliberately if you want to \
         start this project fresh"
    )]
    EmptyExisting { path: PathBuf },
    #[error(
        "could not publish the newly created project database at `{path}`: its \
         finished private copy `{private}` could not be linked into place"
    )]
    Publish {
        path: PathBuf,
        private: PathBuf,
        #[source]
        source: std::io::Error,
    },
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

    // Either the database was already there, or this call created one
    // privately and published it whole. Whichever it was, what is at
    // `db_path` from here on is a complete, migrated, project-bound
    // database — never one in the making.
    prepare_file(&db_path, project_id)?;

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
    // until after the project binding, so that "read version" and
    // "create table" — and "query binding" and "insert binding" — cannot
    // interleave with another launcher's. A *first* migration no longer runs
    // here at all (`prepare_file` ran it on a private file before this path
    // existed), so what serializes here is only ever an upgrade or a no-op
    // over an already-complete database, which is what `configure`'s five
    // second busy timeout is sized for.
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
    // another Glasshouse process holds the write lock briefly. Five seconds is
    // enough because of what can be behind that lock on this path: an upgrade
    // or a no-op over a database that is already complete. A *first* migration
    // — the one unbounded piece of work, growing with every migration this
    // build gains — never runs on this file; `prepare_file` runs it on a
    // private copy nobody else can see, and publishes the result whole.
    conn.busy_timeout(std::time::Duration::from_secs(5))
        .map_err(configure_err)?;

    if conn.is_readonly("main").map_err(configure_err)? {
        return Err(DatabaseError::ReadOnly {
            path: db_path.to_path_buf(),
        });
    }

    Ok(())
}
