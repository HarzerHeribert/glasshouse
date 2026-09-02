use std::path::Path;

use rusqlite::{Connection, OptionalExtension};

use super::DatabaseError;
use super::schema::SUPPORTED_SCHEMA_VERSION;

mod v14_on;
mod v1_to_v13;

pub(super) use v1_to_v13::MIGRATIONS_V1_TO_V13;
use v14_on::MIGRATIONS_V14_ON;

/// Migration `index + 1` upgrades a database from schema version `index` to
/// version `index + 1`. Migrations run in order inside one transaction, so a
/// partially applied upgrade can never be observed.
///
/// Migrations are append-only. Editing one that has shipped would leave
/// already-migrated databases silently disagreeing with new ones, because the
/// recorded version would match while the schema did not.
pub(crate) const PROJECT_ID_KEY: &str = "project_id";

/// Refuse an existing metadata table that is unbound or belongs to a different
/// project. A genuinely brand-new database has no metadata table yet and passes
/// straight through to migration and [`bind_project`].
pub(super) fn verify_identity(
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

/// Apply pending migrations deterministically and refuse anything this build
/// cannot handle.
///
/// Runs inside the caller's `BEGIN IMMEDIATE` transaction: the ledger is
/// created, read, and advanced under SQLite's write lock, so two concurrent
/// first launches can never interleave "read version 0" with "create table".
/// No commit happens here; [`ensure_ready`] commits once after the project
/// binding is also in place.
pub(super) fn migrate(conn: &Connection, db_path: &Path) -> Result<(), DatabaseError> {
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

    for (index, script) in MIGRATIONS_V1_TO_V13
        .iter()
        .chain(MIGRATIONS_V14_ON.iter())
        .enumerate()
    {
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
pub(super) fn bind_project(
    conn: &Connection,
    db_path: &Path,
    project_id: &str,
) -> Result<(), DatabaseError> {
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
