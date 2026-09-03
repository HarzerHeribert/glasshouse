//! Capability map lines 2414–2416 — Phase 60's H slice: a direct file
//! overlap the edit-intent hook detects (lines 2409–2410) is told to this
//! project's one live orchestrator session, and Glasshouse says plainly when
//! there is nobody unambiguous to tell rather than guessing.
//!
//! # Everything here drives the shipped binary, and reads its log
//!
//! Practice §35: a caller every test bypasses is not a caller. Every test
//! spawns `glasshouse edit-intent hook` in its own process, exactly as
//! `tests/edit_intent.rs` does. The decision this file exists to prove —
//! which orchestrator got told, or that none unambiguous did — is never on
//! the `PreToolUse` answer: that channel reaches the two conflicting
//! sessions, not a third one. So every test enables `--log-level debug
//! --log-file <path>` and reads the log back, the same way a person running
//! `--log-file` would.

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use clap::Parser;

use glasshouse::session::{NewSession, ProjectSessions, SessionId, SessionLifecycle, SessionRole};
use glasshouse::{Cli, Runtime};

struct Fixture {
    _tmp: tempfile::TempDir,
    base: PathBuf,
    root: PathBuf,
    log: PathBuf,
    runtime: Runtime,
}

impl Fixture {
    fn new() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path().to_path_buf();
        let root = base.join("workspace");
        std::fs::create_dir_all(root.join(".git")).unwrap();
        std::fs::create_dir_all(root.join("src")).unwrap();
        let root = std::fs::canonicalize(&root).unwrap();
        let log = base.join("glasshouse.log");
        let runtime = bootstrap(&base, &root);
        Self {
            _tmp: tmp,
            base,
            root,
            log,
            runtime,
        }
    }

    /// `glasshouse edit-intent hook`, exactly as Claude Code runs it, with
    /// logging turned on so this test can read what the hook decided about
    /// delivery. Every hook process appends to the same log file.
    fn hook(&self, session: &SessionId, payload: &str) -> serde_json::Value {
        let mut command = Command::new(env!("CARGO_BIN_EXE_glasshouse"));
        command
            .current_dir(&self.root)
            .arg("--scope")
            .arg(&self.root)
            .arg("--data-dir")
            .arg(self.base.join("data"))
            .arg("--config-dir")
            .arg(self.base.join("config"))
            .arg("--log-level")
            .arg("debug")
            .arg("--log-file")
            .arg(&self.log)
            .args(["edit-intent", "hook", "--session", session.as_str()]);
        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("the glasshouse binary must be runnable");
        child
            .stdin
            .as_mut()
            .expect("stdin was piped")
            .write_all(payload.as_bytes())
            .expect("the hook must read its payload rather than closing the pipe");
        let output = child.wait_with_output().expect("the hook must exit");
        assert!(
            output.status.success(),
            "a PreToolUse hook that exits non-zero vetoes the tool call; stderr: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        serde_json::from_str(&String::from_utf8_lossy(&output.stdout)).unwrap_or_else(|err| {
            panic!(
                "the hook must answer with JSON ({err}): {:?}",
                String::from_utf8_lossy(&output.stdout)
            )
        })
    }

    /// Every line every `hook` call so far has logged, read back the way
    /// `--log-file` is meant to be read.
    fn log_text(&self) -> String {
        std::fs::read_to_string(&self.log).unwrap_or_default()
    }

    fn session(&self, role: SessionRole) -> SessionId {
        let sessions = ProjectSessions::open(&self.runtime).unwrap();
        let store = sessions.store();
        let record = store
            .create(NewSession::embedded("claude-code").with_role(role))
            .unwrap();
        store
            .set_lifecycle(&record.id, SessionLifecycle::Running)
            .unwrap();
        record.id
    }
}

fn bootstrap(base: &Path, root: &Path) -> Runtime {
    let cli = Cli::try_parse_from([
        "glasshouse",
        "--data-dir",
        base.join("data").to_str().unwrap(),
        "--config-dir",
        base.join("config").to_str().unwrap(),
    ])
    .unwrap();
    glasshouse::bootstrap(&cli, root).unwrap()
}

fn write_event(root: &Path, relative: &str) -> String {
    serde_json::json!({
        "hook_event_name": "PreToolUse",
        "tool_name": "Write",
        "tool_input": {
            "file_path": root.join(relative).display().to_string(),
            "content": "done",
        },
    })
    .to_string()
}

// ---------------------------------------------------------------------------
// 2414 — ambiguity is reported, not guessed.
// ---------------------------------------------------------------------------

/// Zero live orchestrator sessions: the conflict is real, and Glasshouse says
/// plainly it could not be delivered rather than staying silent about it.
#[test]
fn zero_live_orchestrators_reports_undeliverable() {
    let fixture = Fixture::new();
    let first = fixture.session(SessionRole::Normal);
    let second = fixture.session(SessionRole::Normal);
    let event = write_event(&fixture.root, "src/main.rs");

    fixture.hook(&first, &event);
    fixture.hook(&second, &event);

    let log = fixture.log_text();
    assert!(
        log.contains("no live orchestrator session is running in this project"),
        "{log}"
    );
}

/// **The mutation-killing test.** More than one live orchestrator session is
/// exactly as ambiguous as none — Glasshouse must say the conflict could not
/// be delivered rather than picking one of them as a guess. A mutation that
/// delivers to the first candidate instead removes this log line entirely,
/// which is what this test reads for.
#[test]
fn two_live_orchestrators_reports_undeliverable_rather_than_guessing() {
    let fixture = Fixture::new();
    let _orchestrator_a = fixture.session(SessionRole::Orchestrator);
    let _orchestrator_b = fixture.session(SessionRole::Orchestrator);
    let first = fixture.session(SessionRole::Normal);
    let second = fixture.session(SessionRole::Normal);
    let event = write_event(&fixture.root, "src/main.rs");

    fixture.hook(&first, &event);
    fixture.hook(&second, &event);

    let log = fixture.log_text();
    assert!(
        log.contains(
            "more than one live orchestrator session is running in this project, and \
             Glasshouse does not guess which one"
        ),
        "{log}"
    );
}

/// Exactly one live orchestrator, and it is neither of the two conflicting
/// sessions: the ambiguity path must not fire, and delivery must be
/// attempted through the real seam rather than skipped.
#[test]
fn one_unambiguous_orchestrator_is_attempted_not_reported_ambiguous() {
    let fixture = Fixture::new();
    let orchestrator = fixture.session(SessionRole::Orchestrator);
    let first = fixture.session(SessionRole::Normal);
    let second = fixture.session(SessionRole::Normal);
    let event = write_event(&fixture.root, "src/main.rs");

    fixture.hook(&first, &event);
    fixture.hook(&second, &event);

    let log = fixture.log_text();
    assert!(
        !log.contains("no live orchestrator session is running"),
        "exactly one candidate exists and must not be reported as none: {log}"
    );
    assert!(
        !log.contains("more than one live orchestrator session"),
        "exactly one candidate exists and must not be reported as ambiguous: {log}"
    );
    // Delivery is attempted through the real production seam
    // (`SessionApi::send_text`) and is expected to report `NotLive` here:
    // this hook runs as its own subprocess with no pseudo-terminal of its
    // own, so nothing in its freshly constructed runtime holds the
    // orchestrator's live session. See `notify_orchestrator_of_conflict`'s
    // own doc comment. The decisive fact this test reads is that the
    // correct, single recipient was resolved and an attempt was logged
    // against it.
    assert!(
        log.contains("could not deliver a conflict notice to the orchestrator")
            && log.contains(orchestrator.as_str()),
        "the one candidate must be named as the attempted recipient: {log}"
    );
}

/// No self-notification: the project's one live orchestrator is itself one
/// of the two conflicting sessions, and is not told about its own claim
/// through this second channel — it already has the fact, from being a
/// party to it.
#[test]
fn the_only_orchestrator_being_a_conflict_party_is_not_notified_again() {
    let fixture = Fixture::new();
    let orchestrator = fixture.session(SessionRole::Orchestrator);
    let other = fixture.session(SessionRole::Normal);
    let event = write_event(&fixture.root, "src/main.rs");

    // The orchestrator holds the first claim; the conflict is between it and
    // `other`.
    fixture.hook(&orchestrator, &event);
    fixture.hook(&other, &event);

    let log = fixture.log_text();
    assert!(
        !log.contains("no live orchestrator session is running")
            && !log.contains("more than one live orchestrator session"),
        "exactly one candidate exists; this is not the ambiguous case: {log}"
    );
    assert!(
        !log.contains("could not deliver a conflict notice to the orchestrator")
            && !log.contains("delivered a conflict notice to the orchestrator"),
        "the sole orchestrator is a party to its own conflict and must not be notified \
         through a second channel: {log}"
    );
}

// ---------------------------------------------------------------------------
// 2415 — granularity: a conflict on one path names only that path.
// ---------------------------------------------------------------------------

/// A conflict on one path is reported against that path, and a session
/// working on an unrelated, unclaimed path produces no delivery attempt that
/// names it. If the signal were task-wide rather than path-wide, the second
/// write's own path would appear in the same delivery attempt as the first.
#[test]
fn a_conflict_on_one_path_names_only_that_path() {
    let fixture = Fixture::new();
    let _orchestrator = fixture.session(SessionRole::Orchestrator);
    let first = fixture.session(SessionRole::Normal);
    let second = fixture.session(SessionRole::Normal);
    let third = fixture.session(SessionRole::Normal);

    fixture.hook(&first, &write_event(&fixture.root, "src/a.rs"));
    fixture.hook(&second, &write_event(&fixture.root, "src/a.rs"));
    // `third` writes an entirely different, unclaimed path: no conflict, no
    // delivery attempt at all for it.
    fixture.hook(&third, &write_event(&fixture.root, "src/b.rs"));

    let log = fixture.log_text();
    assert!(
        log.contains("path=src/a.rs") || log.contains("path=\"src/a.rs\""),
        "the conflicting path must be named: {log}"
    );
    assert!(
        !log.contains("src/b.rs"),
        "an unrelated, unclaimed path must never appear in a delivery attempt: {log}"
    );
}
