//! The **read** side of memory's project boundary.
//!
//! `tests/project_isolation.rs` proves that every state-changing
//! `MemoryStore` primitive refuses a memory planted from another project and
//! writes nothing to it. That hardening (Phase 21G) added
//! `AND project_id = ?N` to five `UPDATE memories` statements, on the ground
//! that a leading `self.get(id)?` guard is "one line a future edit can drop,
//! and the failure is silent."
//!
//! The same argument applies to reads, and the reads were not covered.
//! [`MemoryStore::with_status`] is the query behind
//! `glasshouse memory revalidate --list` (`main.rs::memory_revalidate_list`)
//! and behind the shell's project-knowledge panel
//! (`shell/mod.rs`, two call sites) — three production consumers, none of
//! which passes an identifier through `get`, so nothing else in the call
//! chain checks scope. A listing query is not protected by a leading guard at
//! all: there is no identifier to guard.
//!
//! This file is the regression evidence for that boundary, in the direction
//! that actually renders a foreign memory's *body* on a user's screen.

use std::path::{Path, PathBuf};

use clap::Parser;
use rusqlite::Connection;

use glasshouse::memory::{MemoryKind, MemoryStatus, NewMemory, ProjectMemory, ReviewReason};
use glasshouse::{Cli, Runtime, bootstrap};

/// A bootstrapped project inside `base`, sharing `base`'s data and config
/// roots — the same shape `tests/project_isolation.rs` uses, so that two
/// fixtures over one `base` are two real projects on one machine, each with
/// its own canonicalised root and its own `glasshouse.db`.
struct Fixture {
    runtime: Runtime,
}

impl Fixture {
    fn new(base: &Path, name: &str) -> Self {
        let root: PathBuf = base.join("workspace").join(name);
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
        let runtime = bootstrap(&cli, &root).unwrap();
        Fixture { runtime }
    }

    fn project_id(&self) -> &str {
        self.runtime.project().id().as_str()
    }

    fn memory(&self) -> ProjectMemory {
        ProjectMemory::open(&self.runtime).unwrap()
    }

    fn raw_connection(&self) -> Connection {
        Connection::open(self.runtime.database_path()).unwrap()
    }
}

/// Insert a memory row directly, bypassing `MemoryStore` and the project-id
/// trigger — the only way to plant a row belonging to another project, which
/// is exactly what the trigger exists to prevent. Models a row that reached
/// the file by a route the trigger never saw: a restored backup, a
/// hand-edited file, a build whose schema predates the guard.
///
/// Takes a `status` because the listing this file exercises selects *by*
/// status; `project_isolation.rs`'s own helper always plants `active`, which
/// `with_status(NeedsReview, ..)` would never have returned regardless of
/// scoping, and a test that cannot fail is not a test.
fn plant_foreign_memory(conn: &Connection, id: &str, project_id: &str, status: &str, body: &str) {
    conn.execute_batch("DROP TRIGGER memories_reject_foreign_project_insert;")
        .unwrap();
    conn.execute(
        "INSERT INTO memories (id, project_id, kind, status, body, review_reason, \
         created_at, updated_at) \
         VALUES (?1, ?2, 'finding', ?3, ?4, 'project_state', 0, 0)",
        rusqlite::params![id, project_id, status, body],
    )
    .unwrap();
    conn.execute_batch(
        "CREATE TRIGGER memories_reject_foreign_project_insert
         BEFORE INSERT ON memories
         FOR EACH ROW
         WHEN NEW.project_id IS NOT (
             SELECT value FROM project_metadata WHERE key = 'project_id'
         )
         BEGIN
             SELECT RAISE(ABORT, 'memory belongs to a different project');
         END;",
    )
    .unwrap();
}

/// `MemoryStore::with_status` — the review queue — must not return a memory
/// belonging to another project, and `MemoryStore::count` must not count one.
///
/// The premise is asserted first and it is what makes this non-vacuous: beta's
/// *own* needs-review memory is in the queue and is counted, so an empty
/// result cannot pass this test. The planted row is given the **same status**
/// the query selects for, so scoping is the only thing that can exclude it.
///
/// Failure mode this guards, and why an error return is not the shape here:
/// `with_status` never takes an identifier, so there is no `self.get(id)?` to
/// stand in front of it. The `WHERE` clause is the entire boundary, and if it
/// omits `project_id` the foreign memory's **body** is printed by
/// `glasshouse memory revalidate --list` and rendered in the shell's
/// project-knowledge panel.
#[test]
fn the_review_queue_and_the_status_count_never_reach_a_memory_planted_from_another_project() {
    let tmp = tempfile::tempdir().unwrap();
    let alpha = Fixture::new(tmp.path(), "alpha");
    let beta = Fixture::new(tmp.path(), "beta");

    let conn = beta.raw_connection();
    plant_foreign_memory(
        &conn,
        "planted-memory",
        alpha.project_id(),
        "needs_review",
        "alpha's secret body must never be listed from beta",
    );
    drop(conn);

    let beta_memory = beta.memory();
    let beta_store = beta_memory.store();

    let own = beta_store
        .record(NewMemory::new(MemoryKind::Finding, "beta's own memory"))
        .unwrap();
    beta_store
        .mark_for_review(&own.id, ReviewReason::ProjectState)
        .unwrap();

    // Premise: the queue is not empty, and beta's own memory is the thing in
    // it. Without this the assertions below would pass against a store that
    // returned nothing at all.
    let queue = beta_store
        .with_status(MemoryStatus::NeedsReview, 20)
        .unwrap();
    let ids: Vec<&str> = queue.iter().map(|record| record.id.as_str()).collect();
    assert!(
        ids.contains(&own.id.as_str()),
        "premise failed: beta's own needs-review memory is missing from its own queue: {ids:?}"
    );

    assert!(
        !ids.contains(&"planted-memory"),
        "the review queue returned a memory planted from another project: {ids:?}"
    );
    for record in &queue {
        assert_eq!(
            record.project_id,
            beta.project_id(),
            "the review queue returned a row bound to another project"
        );
        assert!(
            !record.body.contains("alpha's secret body"),
            "another project's memory body reached beta's review queue"
        );
    }

    assert_eq!(
        beta_store.count(MemoryStatus::NeedsReview).unwrap(),
        1,
        "the needs-review count must be beta's own memory alone, not beta's plus alpha's"
    );

    // Refusing to list is not deleting: the planted row is still there, so
    // this asserts a scoping boundary rather than a destructive cleanup.
    let conn = beta.raw_connection();
    let planted_status: String = conn
        .query_row(
            "SELECT status FROM memories WHERE id = 'planted-memory'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(planted_status, "needs_review");
}
