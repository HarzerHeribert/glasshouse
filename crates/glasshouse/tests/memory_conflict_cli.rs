//! Capability map line 922 — "surface the conflict and ask whether the older
//! decision should be superseded when the conflict is material and cannot be
//! resolved from current evidence."
//!
//! # Why this enters through the binary
//!
//! `MemoryStore::resolve_conflict` has been fully implemented and tested
//! since Phase 22, and before this batch had **zero** non-test call sites:
//! `mark_conflicted` ships from an ordinary `memory search`
//! (`memory/search.rs`), and nothing in the shipped binary could settle what
//! it raised. Calling `resolve_conflict` directly and reading the row back
//! would prove the method works and nothing about whether an operator can
//! reach it — the two things that make this a capability are the door a
//! person types (`glasshouse memory conflicts` / `glasshouse memory
//! resolve`) and the fact that the shipped binary, not a test harness, is
//! what resolves the review. Practice §35: a caller every test bypasses is
//! not a caller.
//!
//! Seeding the conflicting pair goes straight through `memory::ProjectMemory`
//! and `MemoryStore::mark_conflicted`, the same way `memory_supersede_reason.rs`
//! seeds its pair: this file proves the CLI's listing and resolution doors,
//! not the raising half, which belongs to `memory/search.rs` and is out of
//! scope for this packet.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use rusqlite::Connection;

use glasshouse::cli::Cli;
use glasshouse::memory::{
    MemoryAuthority, MemoryId, MemoryKind, MemoryRecord, MemoryStatus, NewMemory, ProjectMemory,
};

struct Fixture {
    base: PathBuf,
    root: PathBuf,
}

impl Fixture {
    /// A bootstrapped project inside `base`, sharing `base`'s data and config
    /// roots — the shape `tests/memory_project_scope.rs` uses so that two
    /// fixtures over one `base` are two real projects on one machine, each
    /// with its own canonicalised root and its own `glasshouse.db`.
    fn new(base: &Path, name: &str) -> Self {
        let root = base.join("workspace").join(name);
        std::fs::create_dir_all(root.join(".git")).expect("create project root");
        std::fs::create_dir_all(base.join("config")).expect("create config dir");
        let root = std::fs::canonicalize(&root).expect("canonicalize the project root");
        Self {
            base: base.to_path_buf(),
            root,
        }
    }

    fn cli(&self) -> Cli {
        Cli {
            scope: Some(self.root.clone()),
            allow_unsafe_scope: false,
            data_dir: Some(self.base.join("data")),
            config_dir: Some(self.base.join("config")),
            log_level: None,
            log_file: None,
            log_stderr: false,
            command: None,
        }
    }

    fn runtime(&self) -> glasshouse::Runtime {
        glasshouse::bootstrap(&self.cli(), &self.root).expect("bootstrap")
    }

    fn project_id(&self) -> String {
        self.runtime().project().id().as_str().to_owned()
    }

    fn raw_connection(&self) -> Connection {
        Connection::open(self.runtime().database_path()).expect("open the raw database")
    }

    /// Record two memories and mark them conflicted through the store —
    /// exactly what `memory/search.rs::flag_contradictions` does on an
    /// ordinary search, without driving search itself.
    fn seed_conflicted_pair(
        &self,
        one_body: &str,
        one_authority: Option<MemoryAuthority>,
        other_body: &str,
    ) -> (MemoryId, MemoryId) {
        let runtime = self.runtime();
        let memory = ProjectMemory::open(&runtime).expect("open the project memory");
        let store = memory.store();
        let one = store
            .record(NewMemory {
                authority: one_authority,
                ..NewMemory::new(MemoryKind::Decision, one_body)
            })
            .expect("record the first memory");
        let other = store
            .record(NewMemory::new(MemoryKind::Decision, other_body))
            .expect("record the second memory");
        store
            .mark_conflicted(&one.id, &other.id)
            .expect("mark the pair conflicted");
        (one.id, other.id)
    }

    fn read(&self, id: &MemoryId) -> MemoryRecord {
        let runtime = self.runtime();
        let memory = ProjectMemory::open(&runtime).expect("open the project memory");
        let record = memory.store().get(id).expect("read the memory");
        record.expect("the memory exists")
    }

    fn glasshouse(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_glasshouse"))
            .arg("--scope")
            .arg(&self.root)
            .arg("--data-dir")
            .arg(self.base.join("data"))
            .arg("--config-dir")
            .arg(self.base.join("config"))
            .args(args)
            .output()
            .expect("the glasshouse binary must be runnable")
    }
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

/// Insert a `conflicted` memory row directly, bypassing `MemoryStore` and the
/// project-id trigger — the only way to plant a row belonging to another
/// project, modelling one that reached the file by a route the trigger never
/// saw (a restored backup, a hand-edited file, a build predating the guard).
/// Copied from `tests/memory_project_scope.rs::plant_foreign_memory`, which
/// proves the same shape for `with_status(NeedsReview, ..)`.
fn plant_foreign_conflicted_memory(conn: &Connection, id: &str, project_id: &str, body: &str) {
    conn.execute_batch("DROP TRIGGER memories_reject_foreign_project_insert;")
        .expect("drop the insert trigger");
    conn.execute(
        "INSERT INTO memories (id, project_id, kind, status, body, created_at, updated_at) \
         VALUES (?1, ?2, 'decision', 'conflicted', ?3, 0, 0)",
        rusqlite::params![id, project_id, body],
    )
    .expect("plant the foreign row");
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
    .expect("recreate the insert trigger");
}

/// The round trip that is impossible on `main`: before this batch the CLI had
/// no `conflicts` or `resolve` subcommand at all, so this test fails against
/// `main` on a missing-subcommand error, not on an assertion.
#[test]
fn resolving_a_conflict_from_the_cli_lists_it_then_settles_one_side() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let fixture = Fixture::new(tmp.path(), "workspace");
    let (one, other) = fixture.seed_conflicted_pair(
        "the gateway retries on every 5xx",
        None,
        "the gateway never retries a 5xx",
    );

    // Premise: both sides are actually conflicted before anything runs.
    assert_eq!(fixture.read(&one).status, MemoryStatus::Conflicted);
    assert_eq!(fixture.read(&other).status, MemoryStatus::Conflicted);

    let listed = fixture.glasshouse(&["memory", "conflicts"]);
    assert!(
        listed.status.success(),
        "`memory conflicts` must succeed:\nstdout: {}\nstderr: {}",
        stdout(&listed),
        stderr(&listed)
    );
    let rendered = stdout(&listed);
    assert!(
        rendered.contains(one.as_str()) && rendered.contains(other.as_str()),
        "both conflicted memories must be listed:\n{rendered}"
    );

    let resolved = fixture.glasshouse(&["memory", "resolve", one.as_str(), "active"]);
    assert!(
        resolved.status.success(),
        "`memory resolve <id> active` must succeed: {}",
        stderr(&resolved)
    );

    assert_eq!(
        fixture.read(&one).status,
        MemoryStatus::Active,
        "the resolved memory must be current again"
    );
    assert_eq!(
        fixture.read(&other).status,
        MemoryStatus::Conflicted,
        "the untouched side of the conflict must not change"
    );

    // And it drops out of the listing once resolved.
    let listed_again = fixture.glasshouse(&["memory", "conflicts"]);
    assert!(
        !stdout(&listed_again).contains(one.as_str()),
        "a resolved memory must no longer appear in the conflict listing"
    );
}

/// `ConflictResolver::Reviewed`, not `::Automatic`: a binding-authority
/// memory must resolve from the CLI, which is exactly the case `::Automatic`
/// would refuse (`MemoryStore::resolve_conflict`'s own documentation, and
/// `require_reviewed_for_high_impact`'s).
#[test]
fn a_binding_authority_memory_resolves_from_the_cli() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let fixture = Fixture::new(tmp.path(), "workspace");
    let (constrained, _other) = fixture.seed_conflicted_pair(
        "the API is versioned in the URL path",
        Some(MemoryAuthority::Constraint),
        "the API is versioned by header",
    );

    let resolved = fixture.glasshouse(&["memory", "resolve", constrained.as_str(), "active"]);
    assert!(
        resolved.status.success(),
        "a constraint-authority memory must resolve from the CLI, not be refused: {}",
        stderr(&resolved)
    );
    assert_eq!(fixture.read(&constrained).status, MemoryStatus::Active);
}

/// The unclassified case: `require_reviewed_for_high_impact` treats a `None`
/// authority as high-impact too, and it is the case `::Automatic` refuses
/// most often because most memories carry no authority at all.
#[test]
fn an_unclassified_authority_memory_resolves_from_the_cli() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let fixture = Fixture::new(tmp.path(), "workspace");
    let (unclassified, _other) = fixture.seed_conflicted_pair(
        "sessions expire after 30 minutes idle",
        None,
        "sessions expire after 2 hours idle",
    );
    assert_eq!(fixture.read(&unclassified).authority, None);

    let resolved = fixture.glasshouse(&["memory", "resolve", unclassified.as_str(), "superseded"]);
    assert!(
        resolved.status.success(),
        "an unclassified memory must resolve from the CLI, not be refused: {}",
        stderr(&resolved)
    );
    assert_eq!(fixture.read(&unclassified).status, MemoryStatus::Superseded);
}

/// A memory belonging to another project must never be listable or
/// resolvable through these commands, even when its full identifier is known
/// and typed exactly.
///
/// The premise is asserted first and is what makes this non-vacuous: beta's
/// own conflicted pair is what the listing is supposed to show, so an empty
/// listing cannot pass this test by accident.
#[test]
fn a_conflicted_memory_from_another_project_is_neither_listed_nor_resolvable() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let alpha = Fixture::new(tmp.path(), "alpha");
    let beta = Fixture::new(tmp.path(), "beta");

    let foreign_id = "a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4";
    let conn = beta.raw_connection();
    plant_foreign_conflicted_memory(
        &conn,
        foreign_id,
        &alpha.project_id(),
        "alpha's secret conflict must never be listed or resolved from beta",
    );
    drop(conn);

    let (own_one, _own_other) = beta.seed_conflicted_pair(
        "beta's own conflicted decision, side one",
        None,
        "beta's own conflicted decision, side two",
    );

    let listed = beta.glasshouse(&["memory", "conflicts"]);
    assert!(
        listed.status.success(),
        "`memory conflicts` must succeed: {}",
        stderr(&listed)
    );
    let rendered = stdout(&listed);
    assert!(
        rendered.contains(own_one.as_str()),
        "premise failed: beta's own conflicted memory is missing from its own listing: \
         {rendered}"
    );
    assert!(
        !rendered.contains(foreign_id),
        "the conflict listing returned a memory planted from another project:\n{rendered}"
    );
    assert!(
        !rendered.contains("alpha's secret conflict"),
        "another project's memory body reached beta's conflict listing:\n{rendered}"
    );

    let resolve_attempt = beta.glasshouse(&["memory", "resolve", foreign_id, "active"]);
    assert!(
        !resolve_attempt.status.success(),
        "resolving a foreign memory must be refused, not silently accepted"
    );

    // Refusing is not deleting: the planted row is still there, unchanged.
    let conn = beta.raw_connection();
    let planted_status: String = conn
        .query_row(
            "SELECT status FROM memories WHERE id = ?1",
            [foreign_id],
            |row| row.get(0),
        )
        .expect("the planted row must still exist");
    assert_eq!(
        planted_status, "conflicted",
        "a refused resolution must write nothing to the foreign row"
    );
}
