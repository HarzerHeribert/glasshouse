//! `glasshouse hook`, run as a real process, exactly as a harness runs it.
//!
//! This is the production path and there is no seam short of it: a lifecycle
//! hook is a **separate process** the harness spawns, handed an event name on
//! its argv and a payload on its standard input. Everything interesting about
//! it is about that process — that it records what it was told, that it
//! records it *durably* because its own memory is gone a millisecond later,
//! that it exits zero whatever happens, and above all that it never reads the
//! conversation it was handed.
//!
//! So these tests spawn the built binary. A unit test calling an internal
//! function would prove none of those four things.

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use glasshouse::config::UserConfig;
use glasshouse::events::{EventLog, LifecycleEvent, TurnOutcome};
use glasshouse::session::{NewSession, ProjectSessions, SessionId, SessionLifecycle};
use glasshouse::{Cli, Runtime};

use clap::Parser;

/// A project with its own data and config roots, as every test here needs.
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

    /// Run `glasshouse hook`, writing `payload` to its standard input the way
    /// a harness does, and return its exit status.
    fn hook(&self, session: &str, event: &str, payload: &str) -> std::process::ExitStatus {
        self.hook_logging(session, event, payload, None)
    }

    /// [`Fixture::hook`] with debug logging sent to a file, so a test can read
    /// what the handler actually wrote to the log.
    fn hook_logging(
        &self,
        session: &str,
        event: &str,
        payload: &str,
        log_file: Option<&Path>,
    ) -> std::process::ExitStatus {
        let mut command = Command::new(env!("CARGO_BIN_EXE_glasshouse"));
        if let Some(log_file) = log_file {
            command.arg("--log-level").arg("debug");
            command.arg("--log-file").arg(log_file);
        }
        let mut child = command
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
        let output = child.wait_with_output().expect("the hook must finish");
        // Kept for the leak scan below: whatever the handler printed is one of
        // the places a payload field could surface.
        assert!(
            output.stdout.is_empty(),
            "a hook must print nothing on standard output: {:?}",
            String::from_utf8_lossy(&output.stdout)
        );
        output.status
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

/// A recorded session in the `Running` state, which is what a harness
/// reporting a turn is reporting about.
fn running_session(fixture: &Fixture, harness: &str) -> SessionId {
    let sessions = ProjectSessions::open(&fixture.runtime).unwrap();
    let store = sessions.store();
    let record = store.create(NewSession::embedded(harness)).unwrap();
    store
        .set_lifecycle(&record.id, SessionLifecycle::Running)
        .unwrap();
    record.id
}

/// A payload of exactly the shape both supported harnesses send, with the two
/// fields that are the conversation itself carrying values a scan can find.
const PROMPT: &str = "PAYLOAD-PROMPT-a1b2c3-MUST-NEVER-BE-STORED";
const REPLY: &str = "PAYLOAD-REPLY-d4e5f6-MUST-NEVER-BE-STORED";

fn payload() -> String {
    format!(
        r#"{{"session_id":"native-1","transcript_path":"/somewhere/rollout.jsonl",
            "hook_event_name":"Stop","permission_mode":"auto","cwd":"/somewhere",
            "model":"a-model","turn_id":"t-1","stop_hook_active":false,
            "prompt":"{PROMPT}","last_assistant_message":"{REPLY}"}}"#
    )
}

/// **The capability, end to end.** A harness reports an event; Glasshouse
/// records it in the project's own log with the session it happened to, a
/// timestamp, and the harness's own word for it kept beside its own
/// normalized reading.
#[test]
fn a_reported_event_is_recorded_with_its_session_timestamp_and_raw_observation() {
    let fixture = Fixture::new();
    let id = running_session(&fixture, "codex");

    let before = fixture.log().len().unwrap();
    assert!(fixture.hook(id.as_str(), "Stop", &payload()).success());

    let recorded = fixture.log().for_session(&id).unwrap();
    assert_eq!(
        recorded.len() as u64,
        fixture.log().len().unwrap() - before,
        "the hook recorded something for a different session"
    );
    let event = recorded.last().expect("the hook must have recorded one");

    assert_eq!(event.session, id, "the session the event happened to");
    assert!(
        event.at > 1_600_000_000,
        "a real timestamp, not a zero: {}",
        event.at
    );
    assert_eq!(
        event.event,
        LifecycleEvent::TurnEnded {
            outcome: TurnOutcome::Completed
        },
        "`Stop` is a turn that finished"
    );

    // The raw observation, beside the normalized reading and distinguishable
    // from it — which is the whole of Phase 18's first fixed requirement.
    let observed = event
        .observed
        .as_ref()
        .expect("a translated harness report must carry the report it came from");
    // Read out of the session's own record rather than from anything this
    // hook invocation said: `glasshouse hook` is given a session identifier
    // and an event name, never a harness.
    assert_eq!(observed.harness, "codex");
    assert_eq!(
        observed.event, "Stop",
        "the harness's own spelling, not Glasshouse's"
    );
}

/// **The rule the design decision exists for.** The payload carries the
/// user's prompt and the model's reply; the handler drains that stream
/// unread, so neither can reach the project database — and the database has
/// no column either could go in.
///
/// Asserted over the whole file's bytes rather than over one table, because
/// the claim is about the project's stored state and not about a query
/// somebody remembered to write.
#[test]
fn no_field_of_a_hook_payload_reaches_the_project_database() {
    let fixture = Fixture::new();
    let id = running_session(&fixture, "codex");

    for event in [
        "UserPromptSubmit",
        "Stop",
        "PermissionRequest",
        "PreCompact",
    ] {
        assert!(fixture.hook(id.as_str(), event, &payload()).success());
    }

    let bytes = std::fs::read(fixture.runtime.database_path()).unwrap();
    let text = String::from_utf8_lossy(&bytes);
    for forbidden in [
        PROMPT,
        REPLY,
        "transcript_path",
        "rollout.jsonl",
        "permission_mode",
        "native-1",
    ] {
        assert!(
            !text.contains(forbidden),
            "`{forbidden}` from the hook payload reached the project database"
        );
    }

    // And the control: the scan can find something, so its silence above
    // means absence rather than a scan that reads nothing.
    assert!(
        text.contains("codex"),
        "the scan found nothing at all, so it is proving nothing"
    );
}

/// An event this build does not recognise still leaves the session alone and
/// still exits zero — and records nothing, because there is nothing
/// normalized to record.
#[test]
fn an_unrecognised_event_records_nothing_and_still_exits_zero() {
    let fixture = Fixture::new();
    let id = running_session(&fixture, "codex");
    let before = fixture.log().len().unwrap();

    for unknown in ["PreCompact", "PreToolUse", "Notification"] {
        let status = fixture.hook(id.as_str(), unknown, "{}");
        assert!(
            status.success(),
            "a hook must always exit zero; `{unknown}` gave {status:?}"
        );
    }

    assert_eq!(
        fixture.log().len().unwrap(),
        before,
        "an event this build does not model must not become a row"
    );
    let sessions = ProjectSessions::open(&fixture.runtime).unwrap();
    assert_eq!(
        sessions.store().get(&id).unwrap().unwrap().lifecycle,
        SessionLifecycle::Running,
        "an unfamiliar event must leave the session exactly as it was"
    );
}

/// A hook that arrives after its session has finished is **recorded and not
/// applied**, and those are deliberately two different decisions.
///
/// Hook processes outlive their harness, so a late one must not revive a
/// stopped session in the records. It is still something that happened,
/// though, and dropping it would remove exactly the evidence somebody
/// debugging a late hook is looking for.
#[test]
fn a_late_hook_is_recorded_without_reviving_a_finished_session() {
    let fixture = Fixture::new();
    let id = running_session(&fixture, "codex");
    let sessions = ProjectSessions::open(&fixture.runtime).unwrap();
    sessions
        .store()
        .set_lifecycle(&id, SessionLifecycle::Stopped)
        .unwrap();

    assert!(
        fixture
            .hook(id.as_str(), "UserPromptSubmit", "{}")
            .success()
    );

    assert_eq!(
        sessions.store().get(&id).unwrap().unwrap().lifecycle,
        SessionLifecycle::Stopped,
        "a hook arriving after the session ended must not bring it back"
    );
    let recorded = fixture.log().for_session(&id).unwrap();
    assert!(
        recorded
            .iter()
            .any(|event| event.event == LifecycleEvent::TurnStarted),
        "the late report is still a thing that happened and belongs in the log: {recorded:?}"
    );
}

/// A hook against a session this project has never heard of exits zero and
/// records nothing.
///
/// It must never fail: Claude Code treats a `UserPromptSubmit` hook's
/// non-zero exit as a veto and blocks the user's prompt outright. Glasshouse's
/// bookkeeping is never worth costing somebody a turn.
#[test]
fn a_hook_for_an_unknown_session_costs_the_user_nothing() {
    let fixture = Fixture::new();
    let before = fixture.log().len().unwrap();

    let status = fixture.hook("abcdef123456", "Stop", &payload());
    assert!(status.success(), "a hook must always exit zero: {status:?}");
    assert_eq!(fixture.log().len().unwrap(), before);
}

/// **"Preserve raw adapter event payloads in debug logs when useful for
/// troubleshooting", and the standing decision that bounds it.**
///
/// The two are in tension and the resolution is in
/// `docs/product/design-decisions.md`: what an adapter hands Glasshouse is an
/// integration slug and an event name, so that is what is preserved. The
/// conversation the payload also carries is drained unread and reaches
/// nothing. Both halves are asserted here, over the real log file the real
/// binary wrote, because a claim about a debug log can only be checked by
/// reading one.
///
/// The unrecognised event is the case the line exists *for*: a harness gained
/// an event between releases and Glasshouse ignores it, which is correct
/// behaviour and completely invisible without this line.
#[test]
fn the_debug_log_preserves_the_raw_observation_and_none_of_the_payload() {
    let fixture = Fixture::new();
    let id = running_session(&fixture, "codex");
    let log_file = fixture.base.join("hook.log");

    for event in ["Stop", "SomethingThisBuildHasNeverHeardOf"] {
        assert!(
            fixture
                .hook_logging(id.as_str(), event, &payload(), Some(&log_file))
                .success()
        );
    }

    let written = std::fs::read_to_string(&log_file).expect("the hook must have written a log");

    for expected in [
        "raw harness observation",
        "harness=\"codex\"",
        "event=\"Stop\"",
        // Written whether or not the event is recognised — the whole point.
        "event=\"SomethingThisBuildHasNeverHeardOf\"",
    ] {
        assert!(
            written.contains(expected),
            "the debug log does not preserve `{expected}`:\n{written}"
        );
    }

    // And not one field of the payload, which is where the user's own words
    // and the model's reply live.
    for forbidden in [
        PROMPT,
        REPLY,
        "transcript_path",
        "rollout.jsonl",
        "permission_mode",
        "stop_hook_active",
        "native-1",
    ] {
        assert!(
            !written.contains(forbidden),
            "`{forbidden}` from the hook payload reached the debug log:\n{written}"
        );
    }
}

/// The interface's window onto another process, and why it is a window rather
/// than the whole wall.
///
/// `observed_since` is what the shell's reader thread polls. It returns
/// **only** harness-reported rows, because everything the interface itself
/// publishes already reaches its consumers on its own event bus — so a reader
/// that took every row would show half of them twice. The filter is a rule
/// about where an event came from, and this is what holds it to that.
#[test]
fn the_tail_returns_harness_reports_and_not_the_interfaces_own_events() {
    let fixture = Fixture::new();
    let id = running_session(&fixture, "codex");

    // An event Glasshouse observed itself, written exactly as the shell's
    // sink writes one: no observation.
    let bus = glasshouse::events::EventBus::new();
    let own = bus.publish(&id, LifecycleEvent::SessionStarted);
    fixture.log().append(&own, None).unwrap();

    // And one a harness reported, through the real hook process.
    assert!(fixture.hook(id.as_str(), "Stop", &payload()).success());

    let tailed = fixture.log().observed_since(0, 256).unwrap();
    assert!(
        !tailed.is_empty(),
        "the tail found nothing at all, so it is proving nothing"
    );
    for event in &tailed {
        assert!(
            event.observed.is_some(),
            "the tail returned an event the interface already publishes itself: {event:?}"
        );
    }
    assert!(
        tailed.iter().any(|event| event.event
            == LifecycleEvent::TurnEnded {
                outcome: TurnOutcome::Completed
            }),
        "the harness's report did not reach the tail: {tailed:?}"
    );

    // And the whole log does hold the interface's own event, so the filter
    // above is selecting rather than the row simply being absent.
    assert!(
        fixture
            .log()
            .all()
            .unwrap()
            .iter()
            .any(|event| event.event == LifecycleEvent::SessionStarted),
        "the control row is missing, so the filter test proves nothing"
    );
}

/// Map line 1791's memory-extraction half, premise. Left enabled — the
/// default, so no `memory_extraction` key is written at all — a `Stop` for a
/// completed turn reaches `run_extraction_after_turn`, which always logs one
/// of its two `tracing::info!` lines (`main.rs:1648` or `main.rs:1657`): this
/// fixture configures no provider, so `RoutedNoModel` always fails and the
/// "produced nothing" line is the one that fires, but either is proof the
/// trigger ran.
///
/// This is the control for
/// [`memory_extraction_disabled_in_user_config_is_not_attempted_while_the_hook_still_records_the_turn`]:
/// without it, that test's silence would be indistinguishable from a hook
/// that never attempts extraction at all.
#[test]
fn memory_extraction_left_enabled_is_attempted_after_a_completed_turn() {
    let fixture = Fixture::new();
    let id = running_session(&fixture, "codex");
    let log_file = fixture.base.join("extraction-enabled.log");

    assert!(
        fixture
            .hook_logging(id.as_str(), "Stop", &payload(), Some(&log_file))
            .success()
    );

    let written = std::fs::read_to_string(&log_file).expect("the hook must have written a log");
    assert!(
        written.contains("memory extraction ran after a completed task")
            || written.contains("memory extraction after a completed task produced nothing"),
        "extraction left enabled must be attempted after a completed turn: {written}"
    );
}

/// Map line 1791's memory-extraction half, the line itself.
/// `memory_extraction = false` planted in the user config layer — through
/// [`UserConfig::set_memory_extraction`] and [`UserConfig::save`], so this
/// test breaks if the key is ever renamed — makes
/// `memory_extraction_enabled(runtime)` false, and neither of
/// `run_extraction_after_turn`'s log lines appears for the same completed
/// turn the premise test above drives.
///
/// The lifecycle event is still recorded: this proves the switch turned off
/// extraction specifically, not that the hook did nothing at all.
#[test]
fn memory_extraction_disabled_in_user_config_is_not_attempted_while_the_hook_still_records_the_turn()
 {
    let fixture = Fixture::new();
    let id = running_session(&fixture, "codex");

    let mut user = UserConfig::load(fixture.runtime.paths())
        .expect("a fresh fixture has no config file yet, which loads as the default");
    user.set_memory_extraction(Some(false));
    user.save(fixture.runtime.paths())
        .expect("the user config layer must be writable in the fixture's own tempdir");

    let log_file = fixture.base.join("extraction-disabled.log");
    let before = fixture.log().len().unwrap();

    assert!(
        fixture
            .hook_logging(id.as_str(), "Stop", &payload(), Some(&log_file))
            .success()
    );

    let written = std::fs::read_to_string(&log_file).expect("the hook must have written a log");
    for forbidden in [
        "memory extraction ran after a completed task",
        "memory extraction after a completed task produced nothing",
    ] {
        assert!(
            !written.contains(forbidden),
            "`{forbidden}` was logged even though memory_extraction=false:\n{written}"
        );
    }

    // The switch turned off extraction, not the hook: the turn's own closing
    // event is still recorded for this session.
    let recorded = fixture.log().for_session(&id).unwrap();
    assert_eq!(
        recorded.len() as u64,
        fixture.log().len().unwrap() - before,
        "the hook recorded something for a different session"
    );
    assert!(
        recorded.iter().any(|event| event.event
            == LifecycleEvent::TurnEnded {
                outcome: TurnOutcome::Completed
            }),
        "the turn's own closing event must still be recorded even with extraction off: {recorded:?}"
    );
}
