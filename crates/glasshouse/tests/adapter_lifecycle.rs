//! A session waiting on the user is observably waiting — end to end, through
//! the process a harness actually runs.
//!
//! # Why this file exists when the translation is already unit-tested
//!
//! `session::lifecycle` has had `"PermissionRequest" => WaitingForUser` for
//! some time, and its own unit tests assert it by calling `lifecycle_for`
//! directly. Those tests are true and they prove the wrong thing: practice
//! §35 is precisely about a suite that enters *below* the production entry
//! point, where the call the shipped binary makes could be deleted without
//! anything failing. `lifecycle_for` is not what Claude Code runs. What
//! Claude Code runs is `glasshouse hook --session <id> --event
//! PermissionRequest`, as a separate process, and every link between that
//! process and a user seeing the word "waiting" is untested until something
//! spawns it.
//!
//! So these tests spawn the built binary, exactly as `session_hook.rs` does
//! and for the same reason.
//!
//! # What "observable" is taken to mean
//!
//! Not "a row changed". Three separate readers must be able to tell a
//! permission-waiting session from an idle one:
//!
//! - the session store, which every listing reads;
//! - the event log, which an observer tailing the project reads;
//! - the disposition, which decides whether a session is offered as live.
//!
//! A change visible to only the first of those would leave a session that
//! *is* waiting looking finished to two of the three things that ask.

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use glasshouse::events::{EventLog, LifecycleEvent};
use glasshouse::harness::{Declared, all};
use glasshouse::integrations::IntegrationId;
use glasshouse::session::{
    NewSession, ProjectSessions, SessionDisposition, SessionId, SessionLifecycle,
};
use glasshouse::{Cli, Runtime};

use clap::Parser;

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

    /// Run `glasshouse hook` as its own process, with a payload on stdin —
    /// the way a harness runs it and the only way that proves anything.
    fn hook(&self, session: &str, event: &str) -> std::process::ExitStatus {
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
            .write_all(PAYLOAD.as_bytes())
            .expect("the handler must read its payload rather than closing the pipe");
        child
            .wait_with_output()
            .expect("the hook must finish")
            .status
    }

    fn lifecycle_of(&self, id: &SessionId) -> SessionLifecycle {
        ProjectSessions::open(&self.runtime)
            .unwrap()
            .store()
            .get(id)
            .unwrap()
            .expect("the session is recorded")
            .lifecycle
    }

    fn log(&self) -> EventLog {
        EventLog::open(&self.runtime).unwrap()
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

/// The shape Claude Code sends. Its contents are irrelevant here — the
/// handler drains the stream unread — but the process must be given one, so
/// that this test exercises the same code path a real hook does.
const PAYLOAD: &str = r#"{"session_id":"native-1","hook_event_name":"PermissionRequest"}"#;

fn running_session(fixture: &Fixture, harness: &str) -> SessionId {
    let sessions = ProjectSessions::open(&fixture.runtime).unwrap();
    let store = sessions.store();
    let record = store.create(NewSession::embedded(harness)).unwrap();
    store
        .set_lifecycle(&record.id, SessionLifecycle::Running)
        .unwrap();
    record.id
}

/// **Line 308.** Claude Code asking for permission leaves the session
/// observably waiting, to all three readers.
#[test]
fn a_claude_code_permission_request_makes_the_session_observably_waiting() {
    let fixture = Fixture::new();
    let id = running_session(&fixture, "claude-code");

    assert!(
        fixture.hook(id.as_str(), "PermissionRequest").success(),
        "a hook must always exit zero: Claude Code treats a non-zero exit as a veto on the turn"
    );

    // 1. The store, which every listing reads.
    assert_eq!(
        fixture.lifecycle_of(&id),
        SessionLifecycle::WaitingForUser,
        "the session the harness said it was blocked on the user for is not recorded as waiting"
    );

    // 2. The event log, which an observer tailing the project reads. A
    //    listing and a tail are different subscribers and a state that
    //    reached only one of them is half-observable.
    let logged = fixture.log().for_session(&id).unwrap();
    assert!(
        logged
            .iter()
            .any(|entry| entry.event == LifecycleEvent::WaitingForUser),
        "nothing in the event log says this session is waiting for the user: {logged:?}"
    );

    // 3. The disposition, which decides whether a session is offered as live.
    //    Waiting is emphatically not finished — a session blocked on a
    //    permission prompt is the one the user most needs offered back to
    //    them.
    let record = ProjectSessions::open(&fixture.runtime)
        .unwrap()
        .store()
        .get(&id)
        .unwrap()
        .unwrap();
    assert_eq!(
        record.disposition(),
        SessionDisposition::Active,
        "a session waiting on a permission prompt is still live"
    );
}

/// The discriminating half, and the reason the test above is not satisfied by
/// "any event moves the session somewhere".
///
/// `Stop` and `PermissionRequest` arrive through the same process, over the
/// same argv, from the same harness. If they did not land in different states
/// the first test would pass against a build that mapped every report to one
/// state — and a user would be told a session was finished while it sat on an
/// unanswered prompt. **Waiting for the user is not idle**, and this is where
/// that sentence is enforced rather than asserted.
#[test]
fn waiting_for_a_permission_prompt_is_not_the_same_state_as_a_finished_turn() {
    let fixture = Fixture::new();

    let waiting = running_session(&fixture, "claude-code");
    assert!(
        fixture
            .hook(waiting.as_str(), "PermissionRequest")
            .success()
    );

    let finished = running_session(&fixture, "claude-code");
    assert!(fixture.hook(finished.as_str(), "Stop").success());

    let waiting_state = fixture.lifecycle_of(&waiting);
    let finished_state = fixture.lifecycle_of(&finished);

    assert_eq!(waiting_state, SessionLifecycle::WaitingForUser);
    assert_eq!(finished_state, SessionLifecycle::Idle);
    assert_ne!(
        waiting_state, finished_state,
        "a permission prompt and a completed turn collapsed into one state, so a session \
         blocked on the user is indistinguishable from one that has finished"
    );
}

/// The document Glasshouse installs actually asks for the event.
///
/// `PermissionRequest` reaching `session::lifecycle` proves the translation.
/// It does not prove that any harness was ever *told* to report it — an
/// adapter that quietly dropped it from its reported events would leave the
/// translation perfectly correct and permanently unreached, which is the
/// dead-path shape §35 describes.
///
/// **This test read the wrong constant on its first draft, and a mutation
/// caught it.** `describe().hooks.verified_events` is `HOOK_EVENTS` — the
/// catalogue of events Claude Code *supports* — while the settings document
/// Glasshouse writes is built from `REPORTED_EVENTS`, the subset it actually
/// subscribes to. Deleting `PermissionRequest` from the second left the first
/// untouched, so the assertion passed against a build that would never have
/// received a single permission report. §80 case 3: the site mutated was not
/// the site the test read, and the pass was meaningless.
///
/// So this goes through [`glasshouse::harness::HarnessAdapter::hook_installation`],
/// which is what `session::HarnessSelection` calls to produce the file, and
/// asserts on the **rendered bytes** — what Claude Code parses — rather than
/// on any list an adapter keeps beside them.
#[test]
fn the_settings_document_glasshouse_installs_subscribes_to_permission_requests() {
    let claude = all()
        .find(|adapter| adapter.id() == IntegrationId::ClaudeCode)
        .expect("Claude Code has an adapter");

    // The harness declaring it supports the event is a necessary condition
    // and, on its own, not the thing in question — see the note above.
    let Declared::Verified { value: hooks, .. } = claude.describe().hooks else {
        panic!("Claude Code declares no hook mechanism, so nothing can report anything");
    };
    assert!(
        hooks.verified_events.contains(&"PermissionRequest"),
        "Claude Code is no longer declared to support `PermissionRequest` at all: {:?}",
        hooks.verified_events
    );

    let tmp = tempfile::tempdir().unwrap();
    let report = glasshouse::harness::HookCommand::new(
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
        installation.events.contains(&"PermissionRequest"),
        "the installation Glasshouse builds no longer subscribes to `PermissionRequest`, so no \
         permission report would ever be sent: {:?}",
        installation.events
    );
    assert!(
        installation.contents.contains("PermissionRequest"),
        "the settings document Claude Code will actually parse does not mention \
         `PermissionRequest`:\n{}",
        installation.contents
    );
}

/// **Lines 340 and 341, held open honestly.**
///
/// Antigravity CLI 1.1.21 does expose a structured lifecycle event stream —
/// `--output-format stream-json` emits typed `init`, `step_update` and
/// `result` events, and running it is how that was established. But that
/// stream is print-mode only, and Glasshouse starts Antigravity as an
/// interactive session ([`glasshouse::harness::antigravity`]'s `start` is a
/// bare invocation), so the stream is not on the session Glasshouse runs.
/// Its `hooks.json` mechanism, which *would* be, was observed to load and
/// never observed to execute.
///
/// So this adapter must keep claiming nothing. The test is here because the
/// tempting next step — declaring the hook mechanism because the
/// specification for it is sitting right there in the binary — would produce
/// an adapter that installs a hook document for events no one has seen fire,
/// and a session whose lifecycle silently never updates. An `Unverified` that
/// a later change quietly fills in is exactly what this project's declaration
/// type exists to prevent, so the absence is pinned rather than left to
/// discipline.
#[test]
fn antigravity_claims_no_hook_channel_it_has_not_been_seen_to_use() {
    let antigravity = all()
        .find(|adapter| adapter.id() == IntegrationId::Antigravity)
        .expect("Antigravity has an adapter");

    assert!(
        !antigravity.describe().hooks.is_verified(),
        "Antigravity now declares a hook mechanism. Its `hooks.json` file is loaded by the CLI \
         (`hooks_manager.go` reports loading it) but no hook was observed to execute in any \
         print-mode run, including one that made three tool calls. If a hook has since been \
         seen to fire, say where in the declaration and delete this test — do not just make it \
         pass"
    );
}
