//! Phase 46 — security and contamination tests.
//!
//! Six capability-map boxes, each an isolation guarantee the codebase already
//! implements and already has a production caller for (see
//! `docs/product/evidence/phase-1.md` and `database.rs`'s project-isolation
//! triggers). This file is the missing regression evidence for it.
//!
//! Every test here builds **two real, canonicalised project roots sharing one
//! `--data-dir`/`--config-dir`**, exactly like `tests/memory_store.rs` and
//! `tests/memory_extract.rs` already do — "two fixtures over one `base` are
//! two real projects on one machine." A single tempdir with two arbitrary
//! subdirectories would not exercise `project::root_safety`'s refusals the
//! same way; this shape does, because `Fixture::new` always gives each
//! project its own real `.git` root canonicalised by `Project::discover`.
//!
//! Two boxes this phase also names — cmux session metadata (Phase 17) and MCP
//! operations (Phase 43) — are **not** covered here. Neither surface exists
//! anywhere in this crate (`docs/product/capability-map.md` carries both
//! phases at 0/10), so a test claiming to prove either bound would be
//! exercising nothing and would read as coverage forever.

use std::path::{Path, PathBuf};

use clap::Parser;
use rusqlite::Connection;

use glasshouse::memory::extract::chunk::{ChunkLimits, SessionChunk};
use glasshouse::memory::extract::{
    ExtractionModel, ExtractionOutcome, ExtractionTrigger, Extractor, ModelError, Prompt,
};
use glasshouse::memory::{
    MemoryId, MemoryKind, MemoryStatus, MemoryStoreError, NewMemory, ProjectMemory,
};
use glasshouse::project::ScopeError;
use glasshouse::session::{
    NewSession, ProjectSessions, ResumableSession, SessionId, SessionLifecycle, SessionStoreError,
};
use glasshouse::{Cli, Project, Runtime, bootstrap};

// -------------------------------------------------------------------------
// Fixture — two real projects, one shared data/config root.
// -------------------------------------------------------------------------

/// A bootstrapped project inside `base`, sharing `base`'s data and config
/// roots. Two fixtures over one `base` are two real projects on one machine,
/// each with its own canonicalised root and its own `glasshouse.db`.
struct Fixture {
    root: PathBuf,
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
        let runtime = bootstrap(&cli, &root).unwrap();
        Fixture { root, runtime }
    }

    fn project_id(&self) -> &str {
        self.runtime.project().id().as_str()
    }

    fn memory(&self) -> ProjectMemory {
        ProjectMemory::open(&self.runtime).unwrap()
    }

    fn sessions(&self) -> ProjectSessions {
        ProjectSessions::open(&self.runtime).unwrap()
    }

    /// A second, independent connection to this project's own database file
    /// — the same file `database::open` would open, reached the only way an
    /// external test can: through the path `Runtime` already makes public,
    /// not through any crate-private door.
    fn raw_connection(&self) -> Connection {
        Connection::open(self.runtime.database_path()).unwrap()
    }
}

fn tempdir() -> tempfile::TempDir {
    tempfile::tempdir().unwrap()
}

/// Insert a memory row directly, bypassing `MemoryStore` and the project-id
/// trigger entirely — the only way to plant a row belonging to another
/// project, which is exactly what the trigger exists to prevent. Models a
/// row that reached the file by some route the trigger never saw: a restored
/// backup, a hand-edited file, a build whose schema predates the guard.
fn plant_foreign_memory(conn: &Connection, id: &str, project_id: &str, body: &str) {
    conn.execute_batch("DROP TRIGGER memories_reject_foreign_project_insert;")
        .unwrap();
    conn.execute(
        "INSERT INTO memories (id, project_id, kind, status, body, created_at, updated_at) \
         VALUES (?1, ?2, 'finding', 'active', ?3, 0, 0)",
        rusqlite::params![id, project_id, body],
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

/// The session-table analogue of [`plant_foreign_memory`], copied from the
/// same trigger `database.rs` installs for `sessions` (and from the unit test
/// `session::store::tests::plant_foreign_row`, which proves the same shape
/// from inside the crate).
fn plant_foreign_session(conn: &Connection, id: &str, project_id: &str, native: &str) {
    conn.execute_batch("DROP TRIGGER sessions_reject_foreign_project_insert;")
        .unwrap();
    conn.execute(
        "INSERT INTO sessions (id, project_id, harness, native_session_id, role, \
         lifecycle, presentation, created_at, last_activity_at) \
         VALUES (?1, ?2, 'codex', ?3, 'normal', 'stopped', 'embedded', 10, 20)",
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

// -------------------------------------------------------------------------
// Line 1741 — one project database cannot be queried through another
// project's Glasshouse instance.
// -------------------------------------------------------------------------

/// Two halves of the same guarantee: the honest case, reached by simply
/// running two real projects side by side, and the defence-in-depth case,
/// reached by planting a foreign row the way a restored backup or an older
/// build might.
///
/// Mutation (kills both halves): in `memory/store.rs::MemoryStore::get`,
/// change `record.project_id != self.project_id` to a condition that never
/// holds (e.g. `false`). Restores to green with `cp` + `touch`.
#[test]
fn one_project_database_cannot_be_queried_through_another_projects_glasshouse_instance() {
    let tmp = tempdir();
    let alpha = Fixture::new(tmp.path(), "alpha");
    let beta = Fixture::new(tmp.path(), "beta");
    assert_ne!(
        alpha.project_id(),
        beta.project_id(),
        "fixture must use two distinct real projects"
    );

    let alpha_record = alpha
        .memory()
        .store()
        .record(NewMemory::new(MemoryKind::Finding, "alpha-only fact"))
        .unwrap();
    let beta_record = beta
        .memory()
        .store()
        .record(NewMemory::new(MemoryKind::Finding, "beta-only fact"))
        .unwrap();

    // Honest case: beta's own instance was never handed alpha's identifier by
    // anything, so it simply has never heard of it — physical separation,
    // not a filtered query.
    assert!(
        beta.memory()
            .store()
            .get(&alpha_record.id)
            .unwrap()
            .is_none(),
        "beta's database must not contain alpha's row at all"
    );
    assert!(
        alpha
            .memory()
            .store()
            .get(&beta_record.id)
            .unwrap()
            .is_none(),
        "alpha's database must not contain beta's row at all"
    );
    // And each instance still answers for its own record.
    assert_eq!(
        alpha
            .memory()
            .store()
            .get(&alpha_record.id)
            .unwrap()
            .unwrap()
            .body,
        "alpha-only fact"
    );

    // Defence-in-depth case: a row that reached beta's file despite the
    // insert trigger (a tampered backup, say) must still be refused when
    // read, never silently served as beta's own.
    let conn = beta.raw_connection();
    plant_foreign_memory(
        &conn,
        "planted-memory",
        alpha.project_id(),
        "should never surface",
    );
    drop(conn);

    let error = beta
        .memory()
        .store()
        .get(&MemoryId::new("planted-memory"))
        .expect_err("a foreign row must never be returned as this project's own");
    match &error {
        MemoryStoreError::ForeignProject {
            id,
            expected,
            actual,
        } => {
            assert_eq!(id.as_str(), "planted-memory");
            assert_eq!(expected, beta.project_id());
            assert_eq!(actual, alpha.project_id());
        }
        other => panic!("expected ForeignProject, got {other:?}"),
    }
}

// -------------------------------------------------------------------------
// Line 1742 — a session from project A cannot be resumed from project B.
// -------------------------------------------------------------------------

/// As phase-1's own evidence notes for the identical mechanism at the
/// session layer: the cross-project case cannot be reached end to end
/// through the binary, because the schema's `BEFORE INSERT` trigger refuses
/// to store such a row in the first place. Reaching the comparison in
/// `open_for_resume` at all requires planting a row by tampering — which is
/// the point: it is defence in depth, not the only line.
///
/// Mutation (kills both halves): in `session/store.rs::open_for_resume`,
/// change `record.project_id != self.project_id` to a condition that never
/// holds. Restores to green with `cp` + `touch`.
#[test]
fn a_session_from_project_a_cannot_be_resumed_from_project_b() {
    let tmp = tempdir();
    let alpha = Fixture::new(tmp.path(), "alpha");
    let beta = Fixture::new(tmp.path(), "beta");

    // The permitted case first, so the refusals below are not merely "resume
    // never works" — assert the premise per practice §17.
    let alpha_sessions = alpha.sessions();
    let alpha_store = alpha_sessions.store();
    let record = alpha_store.create(NewSession::embedded("codex")).unwrap();
    alpha_store
        .set_native_session_id(&record.id, "thread-77")
        .unwrap();
    alpha_store
        .set_lifecycle(&record.id, SessionLifecycle::Stopped)
        .unwrap();
    let resumed = alpha_store.open_for_resume(&record.id).unwrap();
    assert_eq!(
        resumed,
        ResumableSession {
            id: record.id.clone(),
            harness: "codex".to_owned(),
            native_session_id: "thread-77".to_owned(),
        }
    );

    // Honest case: beta's database was never handed alpha's session at all.
    let beta_sessions = beta.sessions();
    let beta_store = beta_sessions.store();
    let error = beta_store
        .open_for_resume(&record.id)
        .expect_err("beta must never resume a session it has no record of");
    assert!(
        matches!(error, SessionStoreError::NotFound { .. }),
        "{error:?}"
    );

    // Defence-in-depth case: a foreign row planted directly into beta's file.
    let conn = beta.raw_connection();
    plant_foreign_session(&conn, "planted-session", alpha.project_id(), "native-1");
    drop(conn);

    let error = beta_store
        .open_for_resume(&SessionId::new("planted-session"))
        .expect_err("a session from another project must never be resumable");
    match &error {
        SessionStoreError::ForeignProject {
            id,
            expected,
            actual,
        } => {
            assert_eq!(id.as_str(), "planted-session");
            assert_eq!(expected, beta.project_id());
            assert_eq!(actual, alpha.project_id());
        }
        other => panic!("expected ForeignProject, got {other:?}"),
    }
    let message = error.to_string();
    assert!(message.contains(alpha.project_id()) && message.contains(beta.project_id()));

    // Refusing is not deleting: the planted record is untouched.
    let conn = beta.raw_connection();
    let still_there: String = conn
        .query_row(
            "SELECT project_id FROM sessions WHERE id = 'planted-session'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(still_there, alpha.project_id());
}

// -------------------------------------------------------------------------
// Line 1743 — canonicalized paths cannot escape the project root through
// parent-directory traversal.
// -------------------------------------------------------------------------

/// `ProjectScope::resolve` has exactly one production caller today —
/// `config::project_config_path`, which only ever feeds it a fixed constant
/// (`.glasshouse/config.toml`), never caller-controlled input — so there is
/// no route through that caller to exercise a traversal attempt with. This
/// test goes straight at the guard through `Project::scope()`, the same
/// public accessor and the identical code that caller runs, using a real
/// sibling project as the target so "escape" means reaching an actual
/// second project's actual file, not merely a path outside some tempdir.
///
/// Mutation (kills this and the symlink test below, line 1744): in
/// `project/scope.rs::ProjectScope::resolve`, change the final
/// `platform::is_within(&self.root, &resolved)` check to always `true`.
/// Restores to green with `cp` + `touch`.
#[test]
fn canonicalized_paths_cannot_escape_the_project_root_through_parent_directory_traversal() {
    let tmp = tempdir();
    let workspace = tmp.path().join("workspace");
    let root_a = workspace.join("alpha");
    let root_b = workspace.join("beta");
    std::fs::create_dir_all(&root_a).unwrap();
    std::fs::create_dir_all(&root_b).unwrap();
    std::fs::write(root_a.join("inside.txt"), "a").unwrap();
    std::fs::write(root_b.join("secret.txt"), "b").unwrap();

    let project_a = Project::discover(&root_a, Some(Path::new(".")), false).unwrap();
    let scope_a = project_a.scope();

    // Assert the premise (practice §17): a normal in-project path really
    // does resolve, so the refusals below prove something.
    let resolved = scope_a.resolve("inside.txt").unwrap();
    assert!(resolved.ends_with("inside.txt"));

    // Every one of these names a real file, in a real sibling project, one
    // `..` away — not merely a string shaped like an escape.
    for traversal in [
        "../beta/secret.txt",
        "../beta",
        "sub/../../beta/secret.txt",
        "..",
    ] {
        let err = scope_a.resolve(traversal).unwrap_err();
        assert!(
            matches!(
                err,
                ScopeError::Traversal { .. } | ScopeError::OutsideProject { .. }
            ),
            "traversal `{traversal}` was not refused: {err}"
        );
    }
}

// -------------------------------------------------------------------------
// Line 1744 — symlink targets outside the project root are rejected by
// Glasshouse-controlled file operations.
// -------------------------------------------------------------------------

/// Unlike line 1743, this box has a real dynamic production caller:
/// `config::load_project_config` / `write_project_config_with_consent`, both
/// of which resolve `.glasshouse/config.toml` through `ProjectScope::resolve`
/// before touching disk. Planting `.glasshouse` itself as a symlink pointing
/// at a real sibling project is the scenario the guard's own doc comment
/// names — the guard resolves symlinks *before* the containment check, so a
/// symlink inside the project cannot be used to reach outside it.
///
/// Unix-only: `std::os::windows::fs::symlink_dir` needs a privilege this
/// sandbox does not reliably have, and `resolve` runs the identical code on
/// every platform (see `project::scope`'s own cross-platform unit tests), so
/// this test proves nothing about Windows and is gated `#[cfg(unix)]`
/// accordingly.
///
/// Mutation: shared with line 1743's test above — same final containment
/// check in `ProjectScope::resolve`.
#[cfg(unix)]
#[test]
fn symlink_targets_outside_the_project_root_are_rejected_by_project_config_io() {
    let tmp = tempdir();
    let workspace = tmp.path().join("workspace");
    let root_a = workspace.join("alpha");
    let root_b = workspace.join("beta");
    std::fs::create_dir_all(&root_a).unwrap();
    std::fs::create_dir_all(&root_b).unwrap();

    // `.glasshouse` itself is a symlink escaping into a real second project.
    std::os::unix::fs::symlink(&root_b, root_a.join(".glasshouse")).unwrap();
    let project_a = Project::discover(&root_a, Some(Path::new(".")), false).unwrap();

    let err =
        glasshouse::config::write_project_config_with_consent(&project_a, &Default::default())
            .unwrap_err();
    assert!(
        matches!(err, glasshouse::config::ConfigError::Scope(_)),
        "{err:?}"
    );
    // Critically, the write must not have gone through to beta's directory.
    assert!(!root_b.join("config.toml").exists());

    let err = glasshouse::config::load_project_config(&project_a).unwrap_err();
    assert!(
        matches!(err, glasshouse::config::ConfigError::Scope(_)),
        "{err:?}"
    );

    // Assert the premise: without the symlink, the same call succeeds and
    // really does write inside alpha.
    std::fs::remove_file(root_a.join(".glasshouse")).unwrap();
    let project_a = Project::discover(&root_a, Some(Path::new(".")), false).unwrap();
    glasshouse::config::write_project_config_with_consent(&project_a, &Default::default()).unwrap();
    assert!(root_a.join(".glasshouse/config.toml").is_file());
}

// -------------------------------------------------------------------------
// Line 1747 — memory extraction cannot write into another project's
// database.
// -------------------------------------------------------------------------

/// Answers with a fixed reply, the same fake model `tests/memory_extract.rs`
/// uses.
struct Canned(String);

impl ExtractionModel for Canned {
    fn describe(&self) -> String {
        "fake/canned".to_owned()
    }
    fn complete(&self, _prompt: &Prompt) -> Result<String, ModelError> {
        Ok(self.0.clone())
    }
}

fn one_memory_reply(body: &str) -> String {
    format!(
        r#"{{"memories": [{{"kind":"finding","authority":"constraint",
             "disposition":"accepted","support":"established",
             "confidence":"certain","body":"{body}"}}]}}"#
    )
}

fn run_extraction(fixture: &Fixture, model: &dyn ExtractionModel, body: &str) -> ExtractionOutcome {
    let memory = fixture.memory();
    let store = memory.store();
    let chunk = SessionChunk::build(
        "session-x",
        Some("deadbee"),
        [format!("we decided: {body}")],
        ChunkLimits::default(),
    );
    Extractor::new(&store, model).run(&chunk, ExtractionTrigger::Manual)
}

/// Extraction always opens its store through `ProjectMemory::open(runtime)`
/// (`main.rs`'s `glasshouse memory extract` does exactly this — see
/// `ProjectMemory::open` call sites) — there is no path argument and no
/// project argument anywhere in the pipeline, so there is no way to hand it
/// another project's database to begin with. The positive half proves that
/// structurally; the mutation proves the database itself would refuse a
/// mis-tagged write even if the structural guarantee were ever broken.
///
/// Mutation: in `memory/store.rs::MemoryStore::record`, change
/// `project_id: self.project_id.clone()` to a fixed wrong string. The insert
/// then hits `memories_reject_foreign_project_insert` and `record()` returns
/// `Err`, so `outcome.recorded` goes empty where it was one — the named
/// assertion below fails. Restores to green with `cp` + `touch`.
#[test]
fn memory_extraction_only_ever_writes_into_its_own_projects_database() {
    let tmp = tempdir();
    let alpha = Fixture::new(tmp.path(), "alpha");
    let beta = Fixture::new(tmp.path(), "beta");

    let model = Canned(one_memory_reply("alpha-specific finding"));
    let outcome = run_extraction(&alpha, &model, "alpha-specific finding");
    assert_eq!(
        outcome.recorded.len(),
        1,
        "extraction against alpha must record into alpha: {outcome:?}"
    );

    let alpha_count = alpha.memory().store().count(MemoryStatus::Active).unwrap();
    let beta_count = beta.memory().store().count(MemoryStatus::Active).unwrap();
    assert_eq!(alpha_count, 1, "alpha must hold the extracted memory");
    assert_eq!(
        beta_count, 0,
        "beta must not gain a row from a's extraction run"
    );
}

// -------------------------------------------------------------------------
// Line 1748 — a project-state deletion removes only that project's
// Glasshouse state.
// -------------------------------------------------------------------------

/// There is no `glasshouse` subcommand that deletes a project's state today
/// (confirmed: no `remove_dir_all` anywhere under `crates/glasshouse/src`
/// touches a project directory), so "a project-state deletion" here is what
/// the module doc of `project/mod.rs` promises regardless of who performs
/// it: state is "keyed by a `ProjectId`... physically separated" — deletion
/// by any means (an operator, a future housekeeping command, a person
/// running `rm -rf`) can only ever reach the one directory it names.
///
/// Mutation: in `paths.rs::RuntimePaths::project_state_dir`, change
/// `self.projects_dir().join(project_id)` to ignore `project_id` (e.g. join
/// a fixed name). Under that mutation `Fixture::new` for the second project
/// fails outright during `bootstrap` (`DatabaseError::ProjectMismatch`,
/// since the shared directory is already bound to the first project's
/// identifier) — the isolation guard failing exactly where it must: the two
/// projects can no longer even coexist, so there is nothing left to delete
/// independently. Restores to green with `cp` + `touch`.
#[test]
fn deleting_one_projects_state_leaves_a_sibling_projects_state_intact() {
    let tmp = tempdir();
    let alpha = Fixture::new(tmp.path(), "alpha");
    let beta = Fixture::new(tmp.path(), "beta");

    let alpha_record = alpha
        .memory()
        .store()
        .record(NewMemory::new(MemoryKind::Finding, "alpha fact"))
        .unwrap();
    let beta_record = beta
        .memory()
        .store()
        .record(NewMemory::new(MemoryKind::Finding, "beta fact"))
        .unwrap();

    let alpha_state = alpha.runtime.state_dir().to_path_buf();
    let beta_state = beta.runtime.state_dir().to_path_buf();
    assert_ne!(alpha_state, beta_state);
    assert!(!alpha_state.starts_with(&beta_state));
    assert!(!beta_state.starts_with(&alpha_state));
    assert!(alpha_state.is_dir());
    assert!(beta_state.is_dir());

    // The deletion under test.
    std::fs::remove_dir_all(&alpha_state).unwrap();
    assert!(!alpha_state.exists());
    // The project's own workspace (its Git checkout) is untouched either way
    // — Glasshouse state and the project's own files are different trees.
    assert!(alpha.root.is_dir());

    // Beta's directory, database and file must be entirely unaffected.
    assert!(beta_state.is_dir());
    let reopened = beta
        .memory()
        .store()
        .get(&beta_record.id)
        .unwrap()
        .expect("beta's memory must survive alpha's deletion");
    assert_eq!(reopened.body, "beta fact");
    let _ = alpha_record; // proven gone by construction; kept for symmetry.
}
