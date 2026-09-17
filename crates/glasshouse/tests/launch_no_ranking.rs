//! GH-GLASSHOUSE-LAUNCH-NO-RANKING — what remains of capability map line
//! 1712 after Phase 37's automatic ranking and Phase 34D's `--task`
//! classification left `glasshouse launch` (design-decisions.md, 2026-09-16,
//! "Glasshouse never decides which model is used"): no destination flag
//! opens a fresh session, `--to <id>` continues exactly that session, and
//! neither takes a ranking decision or writes one.
//!
//! Replaces `tests/launch_classification.rs`, which drove the same binary
//! to prove the ranking and the classification that fed it; both are gone
//! from this path, so that file's tests went with them — see this packet's
//! own report for the accounting of what else changed.

#![cfg(unix)]

use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::Command;

use clap::Parser;
use glasshouse::{Cli, Runtime};

struct Fixture {
    _tmp: tempfile::TempDir,
    base: PathBuf,
    root: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let tmp = tempfile::tempdir().expect("tempdir");
        let base = tmp.path().to_path_buf();
        let root = base.join("workspace");
        std::fs::create_dir_all(root.join(".git")).expect("create project root");
        let root = std::fs::canonicalize(&root).expect("canonicalize project root");

        let bin_dir = base.join("bin");
        std::fs::create_dir_all(&bin_dir).expect("create bin dir");
        let harness_path = bin_dir.join("fake-claude");
        std::fs::write(&harness_path, "#!/bin/sh\nexit 0\n").expect("write fake harness");
        let mut perms = std::fs::metadata(&harness_path).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&harness_path, perms).unwrap();
        let escaped = harness_path.display().to_string().replace('\\', "\\\\");

        let config_dir = base.join("config");
        std::fs::create_dir_all(&config_dir).expect("create config dir");
        std::fs::write(
            config_dir.join("config.toml"),
            format!(
                "version = 1\n\n\
                 [integrations.claude-code]\nenabled = true\nexecutable = \"{escaped}\"\n"
            ),
        )
        .expect("write user config");

        Self {
            _tmp: tmp,
            base,
            root,
        }
    }

    /// Run the binary and hand back everything it said, asserting success.
    fn run(&self, args: &[&str]) -> String {
        let output = Command::new(env!("CARGO_BIN_EXE_glasshouse"))
            .arg("--scope")
            .arg(&self.root)
            .arg("--data-dir")
            .arg(self.base.join("data"))
            .arg("--config-dir")
            .arg(self.base.join("config"))
            .args(args)
            .env("PATH", self.base.join("empty-path"))
            .output()
            .expect("the glasshouse binary must be runnable");
        let said = format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            output.status.success(),
            "`glasshouse {}` failed:\n{said}",
            args.join(" ")
        );
        said
    }

    fn runtime(&self) -> Runtime {
        let cli = Cli::try_parse_from([
            "glasshouse",
            "--data-dir",
            self.base.join("data").to_str().unwrap(),
            "--config-dir",
            self.base.join("config").to_str().unwrap(),
        ])
        .unwrap();
        glasshouse::bootstrap(&cli, &self.root).unwrap()
    }

    /// Every recorded session id, oldest first — read directly rather than
    /// through `ProjectSessions::open`, which is what this file drives.
    fn session_ids(&self) -> Vec<String> {
        let conn = rusqlite::Connection::open(self.runtime().database_path()).unwrap();
        let mut statement = conn
            .prepare("SELECT id FROM sessions ORDER BY created_at ASC, id ASC")
            .unwrap();
        let rows = statement
            .query_map([], |row| row.get::<_, String>(0))
            .unwrap();
        rows.map(Result::unwrap).collect()
    }
}

/// REQUIRED BEHAVIOR 1: `glasshouse launch` with no destination flag opens a
/// fresh session, every time — never the automatic continuation Phase 37's
/// ranking used to choose among this project's warm sessions. Two bare
/// launches must record two sessions, each the harness's own new session id.
#[test]
fn no_flags_opens_a_fresh_session_every_time() {
    let fixture = Fixture::new();

    fixture.run(&["launch", "claude-code", "--headless"]);
    let first = fixture.session_ids();
    assert_eq!(
        first.len(),
        1,
        "the first launch must record exactly one session: {first:?}"
    );

    fixture.run(&["launch", "claude-code", "--headless"]);
    let second = fixture.session_ids();
    assert_eq!(
        second.len(),
        2,
        "a second bare launch must start a fresh session rather than continuing the \
         first one — nothing ranks this project's warm sessions any more: {second:?}"
    );
    assert_ne!(
        second[0], second[1],
        "the two launches must be two distinct session ids: {second:?}"
    );
}

/// REQUIRED BEHAVIOR 2: `--to <id>` continues exactly the session it names —
/// no second session is recorded. The ranking-decision evaluation kinds a
/// launch used to write when a ranking chose or displaced a destination
/// (`RoutingOverrideDecided`, `RoutingContinuationDecided`,
/// `SessionRouteDecided`, `FailoverPrevented`) are gone from
/// `EvaluationKind` entirely along with the ranking that wrote them, so
/// there is nothing left here to assert empty.
///
/// Mutation (packet §16/§9): make `--to <session>` fall back to a fresh
/// session instead of continuing it, and this test must fail — both on the
/// session-count assertion (a second row appears) and on the harness-id
/// assertion (`said` no longer names the original id).
#[test]
fn to_continues_exactly_the_named_session_and_writes_no_ranking_decision() {
    let fixture = Fixture::new();

    fixture.run(&["launch", "claude-code", "--headless"]);
    let id = {
        let ids = fixture.session_ids();
        assert_eq!(
            ids.len(),
            1,
            "expected exactly one recorded session after the first launch: {ids:?}"
        );
        ids.into_iter().next().unwrap()
    };

    let said = fixture.run(&["launch", "claude-code", "--headless", "--to", &id]);
    assert!(
        said.contains(&id),
        "the launch must say which session it continued: {said}"
    );

    let after = fixture.session_ids();
    assert_eq!(
        after,
        vec![id],
        "`--to <id>` must continue that session rather than recording a second one: \
         {after:?}"
    );
}
