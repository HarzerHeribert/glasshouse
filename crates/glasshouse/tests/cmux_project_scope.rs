//! GH-1745 — cmux session metadata cannot bypass project-scope validation.
//!
//! Phase 46's own entry for this line (`docs/product/evidence/phase-46.md`)
//! left it deliberately open: when it was written, cmux integration did not
//! exist in this crate at all. It exists now — `integrations::cmux::PaneRef`,
//! `NewSession::with_presentation_ref`, migration 20's `sessions.presentation_ref`
//! column, and `sessions_reject_foreign_project_insert` / `..._update`, the
//! same triggers `tests/project_isolation.rs` already proves for sessions
//! with no pane at all.
//!
//! Three questions, following that file's fixture shape (two real projects
//! sharing one data/config root) for the first and third, and the shipped
//! binary plus a fake `cmux` on `PATH` (`tests/cmux_presentation.rs`'s seam)
//! for the second, because only the running binary can show that cmux is
//! asked nothing.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use clap::Parser;
use rusqlite::Connection;

use glasshouse::session::{NewSession, ProjectSessions, SessionPresentation};
use glasshouse::{Cli, Runtime, bootstrap};

// -------------------------------------------------------------------------
// Fixture — two real projects, one shared data/config root. Copied from
// `tests/project_isolation.rs`'s `Fixture` rather than imported: integration
// test binaries do not share code with each other.
// -------------------------------------------------------------------------

struct Fixture {
    root: PathBuf,
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
        let runtime = bootstrap(&cli, &root).unwrap();
        Fixture {
            root,
            base: base.to_path_buf(),
            runtime,
        }
    }

    fn project_id(&self) -> &str {
        self.runtime.project().id().as_str()
    }

    fn sessions(&self) -> ProjectSessions {
        ProjectSessions::open(&self.runtime).unwrap()
    }

    fn raw_connection(&self) -> Connection {
        Connection::open(self.runtime.database_path()).unwrap()
    }

    /// Run the real `glasshouse` binary with this project as `cwd`, sharing
    /// this fixture's data/config root, and `PATH` led by a fake `cmux` that
    /// appends every invocation to `cmux_log` — the same seam
    /// `tests/cmux_presentation.rs` uses, reduced to what this file needs
    /// (`ping` and `identify`; nothing here should ever reach `workspace` or
    /// `send`).
    fn glasshouse_inside_cmux(&self, cmux_log: &Path, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_glasshouse"))
            .current_dir(&self.root)
            .arg("--data-dir")
            .arg(self.base.join("data"))
            .arg("--config-dir")
            .arg(self.base.join("config"))
            .env("PATH", path_with_fake_cmux(&self.base))
            .env("FAKE_CMUX_LOG", cmux_log)
            .env("CMUX_SOCKET_PATH", self.base.join("cmux.sock"))
            .env("CMUX_SURFACE_ID", "FAKE-SURFACE")
            .env("CMUX_WORKSPACE_ID", "FAKE-WORKSPACE")
            .env_remove("FAKE_CMUX_DEAD")
            .args(args)
            .output()
            .expect("the glasshouse binary must run")
    }
}

fn tempdir() -> tempfile::TempDir {
    tempfile::tempdir().unwrap()
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

fn both(output: &Output) -> String {
    format!("{}{}", stdout(output), stderr(output))
}

#[cfg(unix)]
fn install_executable(path: &Path, body: &str) {
    use std::os::unix::fs::PermissionsExt;

    std::fs::write(path, body).expect("write executable");
    let mut perms = std::fs::metadata(path).unwrap().permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(path, perms).unwrap();
}

/// A fake `cmux` that answers `ping` and `identify`, logs every call it
/// receives, and refuses anything else loudly — reduced from
/// `tests/cmux_presentation.rs`'s `FAKE_CMUX` to the two verbs this file's
/// tests could legitimately reach, so that any `workspace` or `send` call
/// (which none of these tests should ever cause) fails visibly rather than
/// quietly succeeding.
#[cfg(unix)]
const FAKE_CMUX: &str = r#"#!/bin/sh
printf '%s\n' "$*" >> "$FAKE_CMUX_LOG"
case "$1" in
  ping)
    echo PONG
    exit 0
    ;;
  identify)
    printf '{\n  "caller" : {\n    "surface_ref" : "surface:9",\n    "workspace_ref" : "workspace:7"\n  },\n  "focused" : {\n    "surface_ref" : "surface:1",\n    "workspace_ref" : "workspace:1"\n  }\n}\n'
    exit 0
    ;;
esac
echo "fake cmux: unsupported invocation in cmux_project_scope tests: $*" >&2
exit 2
"#;

#[cfg(unix)]
fn path_with_fake_cmux(base: &Path) -> std::ffi::OsString {
    let bin_dir = base.join("bin");
    std::fs::create_dir_all(&bin_dir).expect("create bin dir");
    install_executable(&bin_dir.join("cmux"), FAKE_CMUX);

    let mut paths = vec![bin_dir];
    paths.extend(
        std::env::var_os("PATH")
            .map(|path| std::env::split_paths(&path).collect::<Vec<_>>())
            .unwrap_or_default(),
    );
    std::env::join_paths(paths).expect("join PATH")
}

// -------------------------------------------------------------------------
// 1 — a session recorded with a pane belongs to its project like any other.
// -------------------------------------------------------------------------

/// A session carrying a `presentation_ref` is subject to the same physical
/// separation as every other session (`docs/product/evidence/phase-46.md`'s
/// line 1742 entry, `a_session_from_project_a_cannot_be_resumed_from_project_b`
/// in `tests/project_isolation.rs`): recording a pane changes nothing about
/// which project's database the row lives in.
#[test]
fn a_session_recorded_with_a_pane_belongs_to_its_project_like_any_other_session() {
    let tmp = tempdir();
    let alpha = Fixture::new(tmp.path(), "alpha");
    let beta = Fixture::new(tmp.path(), "beta");
    assert_ne!(
        alpha.project_id(),
        beta.project_id(),
        "fixture must use two distinct real projects"
    );

    let alpha_sessions = alpha.sessions();
    let record = alpha_sessions
        .store()
        .create(
            NewSession::embedded("claude-code")
                .with_presentation(SessionPresentation::External)
                .with_presentation_ref(Some("workspace:7".to_owned())),
        )
        .unwrap();
    assert_eq!(record.presentation_ref.as_deref(), Some("workspace:7"));

    // Alpha's own instance answers for it, pane and all.
    let reread = alpha_sessions.store().get(&record.id).unwrap().unwrap();
    assert_eq!(reread.presentation_ref.as_deref(), Some("workspace:7"));
    assert!(
        alpha_sessions
            .store()
            .list()
            .unwrap()
            .iter()
            .any(|session| session.id == record.id),
        "alpha's own listing must contain the session it just recorded"
    );

    // Beta's instance was never handed alpha's identifier by anything — the
    // pane reference travelled with the row, not around the project
    // boundary, so beta simply has never heard of this session.
    let beta_sessions = beta.sessions();
    assert!(
        beta_sessions.store().get(&record.id).unwrap().is_none(),
        "beta's database must not contain alpha's paned session at all"
    );
    assert!(
        !beta_sessions
            .store()
            .list()
            .unwrap()
            .iter()
            .any(|session| session.id == record.id),
        "beta's listing must not contain alpha's paned session"
    );
}

// -------------------------------------------------------------------------
// 2 — the reference is not a second identity.
// -------------------------------------------------------------------------

/// Knowing another project's session id (and, through it, its
/// `workspace:<n>`) does not let beta reach alpha's session. `focus_session`
/// resolves the id against beta's own database first — the same
/// `ProjectSessions::open(runtime)` every other command uses — and a session
/// beta has no row for is refused as `no session in this project` before
/// `cmux::detect` or any cmux subcommand ever runs. Modelled on
/// `tests/cmux_presentation.rs::focus_and_send_go_through_the_integration_and_the_door_is_preferred`,
/// which asserts the same call log for the same reason.
#[cfg(unix)]
#[test]
fn a_cmux_reference_is_not_a_second_identity_across_projects() {
    let tmp = tempdir();
    let base = tmp.path();
    let alpha = Fixture::new(base, "alpha");
    let beta = Fixture::new(base, "beta");

    // A real, externally-presented session, live in alpha only.
    let record = alpha
        .sessions()
        .store()
        .create(
            NewSession::embedded("claude-code")
                .with_presentation(SessionPresentation::External)
                .with_presentation_ref(Some("workspace:7".to_owned())),
        )
        .unwrap();

    let cmux_log = base.join("beta-focus.log");
    std::fs::write(&cmux_log, "").unwrap();

    // Beta is asked to focus alpha's session id, inside an environment where
    // cmux is present and would answer if asked.
    let output = beta.glasshouse_inside_cmux(&cmux_log, &["sessions", "focus", record.id.as_str()]);
    assert!(!output.status.success(), "{}", both(&output));
    assert!(
        stderr(&output).contains(&format!("no session `{}` in this project", record.id)),
        "{}",
        stderr(&output)
    );

    let calls = std::fs::read_to_string(&cmux_log).unwrap();
    assert!(
        calls.is_empty(),
        "beta must ask cmux nothing while resolving a session id it has no row for; \
         calls made: {calls:?}"
    );
}

// -------------------------------------------------------------------------
// 3 — the trigger is the floor, not the Rust.
// -------------------------------------------------------------------------

/// A `SessionStore` cannot be asked to insert a row with a forged
/// `project_id` — there is no such call. This test skips `SessionStore`
/// entirely and issues the `INSERT` straight against beta's database
/// connection, with a `presentation_ref` that would pass
/// `PaneRef::parse` if it ever reached that validation. It never does:
/// `sessions_reject_foreign_project_insert` fires first and the row never
/// lands.
#[test]
fn a_forged_insert_carrying_a_valid_looking_presentation_ref_is_refused_by_the_database_trigger() {
    let tmp = tempdir();
    let alpha = Fixture::new(tmp.path(), "alpha");
    let beta = Fixture::new(tmp.path(), "beta");

    let conn = beta.raw_connection();
    let result = conn.execute(
        "INSERT INTO sessions (id, project_id, harness, native_session_id, role, \
         lifecycle, presentation, presentation_ref, created_at, last_activity_at) \
         VALUES (?1, ?2, 'claude-code', ?3, 'normal', 'stopped', 'external', ?4, 10, 20)",
        rusqlite::params![
            "forged-session",
            alpha.project_id(),
            "native-1",
            "workspace:7"
        ],
    );

    let error = result.expect_err(
        "an INSERT naming a foreign project_id must be refused by the database trigger \
         itself, even though its presentation_ref is a well-formed cmux reference",
    );
    assert!(
        error
            .to_string()
            .contains("session belongs to a different project"),
        "{error}"
    );

    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sessions WHERE id = 'forged-session'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(count, 0, "a refused INSERT must leave no row behind");
}
