//! Map line 310 — *"Record Claude compaction events when they can be observed
//! reliably."*
//!
//! # The premise, settled empirically before any code changed
//!
//! `session/lifecycle.rs`'s `precedes_native_compaction` doc comment used to
//! read *"Claude Code's observed catalogue has no compaction event at
//! all"* — a 2.1.245 reading (2026-08-25). Claude Code 2.1.257, installed on
//! this machine on 2026-09-01, does not agree: its own binary carries a
//! `### Hook Events` table naming `PreCompact` and `PostCompact`, real
//! functions that dispatch them (`executePreCompactHooks`), and log strings
//! (`"compaction blocked by PreCompact hook"`) that only make sense if the
//! event is real.
//!
//! That was then run, not just read. A settings document declaring a
//! `PreCompact` command (the same shape `harness::mod::hooks_document`
//! renders — no `matcher`, one `{type, command, timeout}` entry) was passed to
//! the real `claude` binary with `--print --input-format=stream-json
//! --output-format=stream-json --settings <that file>`, one ordinary turn was
//! sent, then a literal `/compact`. The hook ran: its stdin payload was
//! `{"session_id":"<the --session-id given>","transcript_path":"...",
//! "cwd":"...","prompt_id":"...","hook_event_name":"PreCompact",
//! "trigger":"manual","custom_instructions":null}`, and the stream's own
//! `system status` event carried a `compact_result`. The one thing that was
//! missing was `harness::claude_code::REPORTED_EVENTS` ever asking for it —
//! `session::lifecycle::precedes_native_compaction` already matched the
//! string, because Codex has sent it since Phase 8.
//!
//! # What this file proves instead
//!
//! Not that live Claude Code fires the hook — the probe above did that, once,
//! by hand, and cannot be a `cargo test` because CI has no `claude` binary.
//! This file proves the shipped chain **`glasshouse hook` actually walks**:
//! the generated settings document now names `PreCompact`, and a `PreCompact`
//! report for a Claude Code session increments `observed_compactions` and is
//! readable through `glasshouse sessions show` — the same surface
//! `docs/product/evidence/phase-7.md`'s other entries treat as the shipped
//! reader. Practice §35: a caller every test bypasses is not a caller, so
//! everything below spawns the built binary exactly as a harness does.

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use clap::Parser;

use glasshouse::harness::{Declared, HookCommand, all};
use glasshouse::integrations::IntegrationId;
use glasshouse::session::{NewSession, ProjectSessions, SessionId, SessionLifecycle};
use glasshouse::{Cli, Runtime};

struct Fixture {
    _tmp: tempfile::TempDir,
    base: PathBuf,
    root: PathBuf,
    runtime: Runtime,
}

impl Fixture {
    fn new() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path().to_path_buf();
        let root = base.join("workspace");
        std::fs::create_dir_all(root.join(".git")).unwrap();
        let root = std::fs::canonicalize(&root).unwrap();
        let runtime = bootstrap(&base, &root);
        Self {
            _tmp: tmp,
            base,
            root,
            runtime,
        }
    }

    /// Run `glasshouse hook`, exactly as a harness runs it: a separate
    /// process, the event on argv, a payload on stdin.
    fn hook(&self, session: &str, event: &str, payload: &str) -> std::process::ExitStatus {
        let mut child = Command::new(env!("CARGO_BIN_EXE_glasshouse"))
            .arg("--scope")
            .arg(&self.root)
            .arg("--data-dir")
            .arg(self.base.join("data"))
            .arg("--config-dir")
            .arg(self.base.join("config"))
            .arg("hook")
            .arg("--session")
            .arg(session)
            .arg("--event")
            .arg(event)
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
            .expect("the handler must read its payload rather than closing the pipe");
        child
            .wait_with_output()
            .expect("the hook must finish")
            .status
    }

    /// Run `glasshouse sessions show <session>` and return what it printed —
    /// the shipped surface a person reads, not the store underneath it.
    fn sessions_show(&self, session: &str) -> String {
        let output = Command::new(env!("CARGO_BIN_EXE_glasshouse"))
            .arg("--scope")
            .arg(&self.root)
            .arg("--data-dir")
            .arg(self.base.join("data"))
            .arg("--config-dir")
            .arg(self.base.join("config"))
            .arg("sessions")
            .arg("show")
            .arg(session)
            .output()
            .expect("the glasshouse binary must be runnable");
        assert!(
            output.status.success(),
            "`sessions show` failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).expect("the report is text")
    }

    fn observed_compactions(&self, id: &SessionId) -> Option<i64> {
        ProjectSessions::open(&self.runtime)
            .unwrap()
            .store()
            .get(id)
            .unwrap()
            .expect("the session must still be recorded")
            .observed_compactions
    }
}

/// The value `glasshouse sessions show` printed on the line labelled `label`.
///
/// Parsed rather than matched as a padded substring: the column width
/// (`main.rs::session_detail`'s `{label:<19}`) is a formatting decision, not
/// part of this capability's contract.
fn reported(report: &str, label: &str) -> String {
    report
        .lines()
        .find_map(|line| line.strip_prefix(label))
        .map(|rest| rest.trim().to_owned())
        .unwrap_or_else(|| panic!("no `{label}` line in the report:\n{report}"))
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

fn running_session(fixture: &Fixture, harness: &str) -> SessionId {
    let sessions = ProjectSessions::open(&fixture.runtime).unwrap();
    let store = sessions.store();
    let record = store.create(NewSession::embedded(harness)).unwrap();
    store
        .set_lifecycle(&record.id, SessionLifecycle::Running)
        .unwrap();
    record.id
}

const PAYLOAD: &str =
    r#"{"session_id":"native-1","hook_event_name":"PreCompact","trigger":"auto"}"#;

// ---------------------------------------------------------------------------
// The document Glasshouse installs actually asks Claude Code for the event.
// ---------------------------------------------------------------------------

/// Mirrors `adapter_lifecycle.rs`'s
/// `the_settings_document_glasshouse_installs_subscribes_to_permission_requests`,
/// for the same documented reason: `HOOK_EVENTS` is the catalogue Claude Code
/// *supports*, `REPORTED_EVENTS` is what Glasshouse actually subscribes to,
/// and a test that reads the first while the defect is in the second passes
/// for nothing. So this asserts on the rendered document bytes — what Claude
/// Code parses — not on a list an adapter keeps beside them.
#[test]
fn the_settings_document_glasshouse_installs_for_claude_code_subscribes_to_precompact() {
    let claude = all()
        .find(|adapter| adapter.id() == IntegrationId::ClaudeCode)
        .expect("Claude Code has an adapter");

    let Declared::Verified { value: hooks, .. } = claude.describe().hooks else {
        panic!("Claude Code declares no hook mechanism, so nothing can report anything");
    };
    assert!(
        hooks.verified_events.contains(&"PreCompact"),
        "Claude Code is not declared to support `PreCompact` at all: {:?}",
        hooks.verified_events
    );

    let tmp = tempfile::tempdir().unwrap();
    let report = HookCommand::new(
        tmp.path().join("glasshouse"),
        "session-1",
        tmp.path().join("hooks"),
        tmp.path(),
        tmp.path().join("data"),
        tmp.path().join("config"),
    );
    let installation = claude
        .hook_installation(&report)
        .expect("Claude Code installs a hook document");

    assert!(
        installation.events.contains(&"PreCompact"),
        "the installation Glasshouse builds does not subscribe to `PreCompact`, so no \
         compaction would ever be reported: {:?}",
        installation.events
    );
    assert!(
        installation.contents.contains("PreCompact"),
        "the settings document Claude Code will actually parse does not mention \
         `PreCompact`:\n{}",
        installation.contents
    );
}

// ---------------------------------------------------------------------------
// The end-to-end path: a PreCompact report for a live Claude Code session
// increments the count, and it is readable through the surface that reads it.
// ---------------------------------------------------------------------------

/// **The capability, end to end, for the harness map line 310 names.**
///
/// Not a test that the constant is in an array: a `PreCompact` report is
/// walked through the real `glasshouse hook` process, and both the store and
/// `glasshouse sessions show` — the reader a person actually runs — agree the
/// count moved.
#[test]
fn a_precompact_report_for_a_claude_code_session_increments_observed_compactions_and_is_readable_through_sessions_show()
 {
    let fixture = Fixture::new();
    let id = running_session(&fixture, "claude-code");

    assert_eq!(
        fixture.observed_compactions(&id),
        Some(0),
        "a session this build created starts at a measured zero, not unknown"
    );
    assert_eq!(
        reported(&fixture.sessions_show(id.as_str()), "compactions"),
        "0"
    );

    assert!(fixture.hook(id.as_str(), "PreCompact", PAYLOAD).success());

    assert_eq!(fixture.observed_compactions(&id), Some(1));
    assert_eq!(
        reported(&fixture.sessions_show(id.as_str()), "compactions"),
        "1",
        "`glasshouse sessions show` must reflect the same count the store holds"
    );

    assert!(fixture.hook(id.as_str(), "PreCompact", PAYLOAD).success());
    assert_eq!(
        fixture.observed_compactions(&id),
        Some(2),
        "a second compaction on the same session must add to the count, not replace it"
    );
}

/// One session's compaction must never increment another's.
#[test]
fn one_claude_code_sessions_compaction_does_not_touch_another() {
    let fixture = Fixture::new();
    let counted = running_session(&fixture, "claude-code");
    let untouched = running_session(&fixture, "claude-code");

    assert!(
        fixture
            .hook(counted.as_str(), "PreCompact", PAYLOAD)
            .success()
    );

    assert_eq!(fixture.observed_compactions(&counted), Some(1));
    assert_eq!(
        fixture.observed_compactions(&untouched),
        Some(0),
        "a compaction reported for one session must not be counted against another"
    );
}

/// A `PreCompact` report naming a session this project has never heard of
/// must cost the user nothing — the same non-negotiable every other event on
/// this path already honours, because Claude Code treats a hook's non-zero
/// exit as a veto on the user's own turn.
#[test]
fn a_precompact_report_for_an_unknown_session_costs_the_user_nothing() {
    let fixture = Fixture::new();
    let status = fixture.hook(
        "0000000000000000000000000000000000000000000000000000000000000000",
        "PreCompact",
        PAYLOAD,
    );
    assert!(status.success(), "a hook must always exit zero: {status:?}");
}

/// A `PreCompact` report arriving after a session has finished must be
/// discarded, not counted — `lifecycle.rs` already documents that a hook from
/// a process which outlived its harness must not bring a session back, and a
/// compaction count moving on a finished session is the same defect wearing a
/// different field.
#[test]
fn a_precompact_report_for_an_already_finished_claude_code_session_is_discarded() {
    let fixture = Fixture::new();
    let id = running_session(&fixture, "claude-code");
    ProjectSessions::open(&fixture.runtime)
        .unwrap()
        .store()
        .set_lifecycle(&id, SessionLifecycle::Stopped)
        .unwrap();

    assert!(fixture.hook(id.as_str(), "PreCompact", PAYLOAD).success());

    assert_eq!(
        fixture.observed_compactions(&id),
        Some(0),
        "a hook arriving after the session ended must not move its compaction count"
    );
}

/// A regression pin for the map line's own two-branch shape: a harness that
/// never compacts is unaffected by any of this. `Stop` still stamps nothing
/// onto `observed_compactions`, exactly as it did before this box.
#[test]
fn a_completed_turn_does_not_move_the_compaction_count() {
    let fixture = Fixture::new();
    let id = running_session(&fixture, "claude-code");

    let stop_payload = r#"{"session_id":"native-1","hook_event_name":"Stop"}"#;
    assert!(fixture.hook(id.as_str(), "Stop", stop_payload).success());

    assert_eq!(
        fixture.observed_compactions(&id),
        Some(0),
        "an ordinary turn ending must never be counted as a compaction"
    );
}
