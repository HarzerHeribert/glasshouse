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

use glasshouse::checkpoint::{Checkpoint, CheckpointReason, Handoff, ProjectCheckpoints, Stored};
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

    /// Run `glasshouse sessions show <session>` and return what it printed.
    ///
    /// The binary again, not `session_detail`: the claim under test is what a
    /// user is told about a session, and the printer is only half of that.
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
}

/// The value `glasshouse sessions show` printed on the line labelled `label`.
///
/// Parsed rather than matched as a padded substring: the column width is a
/// formatting decision that has moved before, and a test that encoded it
/// would fail for the wrong reason when it moves again.
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
/// completed turn reaches `run_extraction`, which always logs one of its two
/// `tracing::info!` lines: this fixture configures no provider, so
/// `RoutedModel` always fails and the "produced nothing" line is the one
/// that fires, but either is proof the trigger ran.
///
/// **The trigger is asserted as well as the attempt.** Those two log lines
/// used to name the completed turn in their own message text; there are two
/// triggers now (Phase 21's compaction line landed beside this one), so the
/// message is trigger-agnostic and the trigger travels as its own field.
/// Asserting only the message would leave this control unable to tell the
/// post-turn trigger from the compaction one.
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
        written.contains("memory extraction ran")
            || written.contains("memory extraction produced nothing"),
        "extraction left enabled must be attempted after a completed turn: {written}"
    );
    assert!(
        written.contains("trigger=task_completed"),
        "the attempt must name the trigger that made it, and this one is a completed task: \
         {written}"
    );
}

/// Map line 1791's memory-extraction half, the line itself.
/// `memory_extraction = false` planted in the user config layer — through
/// [`UserConfig::set_memory_extraction`] and [`UserConfig::save`], so this
/// test breaks if the key is ever renamed — makes
/// `memory_extraction_enabled(runtime)` false, and neither of
/// `run_extraction`'s log lines appears for the same completed turn the
/// premise test above drives.
///
/// The forbidden strings are kept identical to the ones the premise test
/// asserts *present*, and that is load-bearing: an absence assertion against
/// a string production no longer emits passes for the wrong reason, silently,
/// forever. The two lists are read together or not at all.
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
        "memory extraction ran",
        "memory extraction produced nothing",
        "trigger=task_completed",
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

/// A handoff a "user" authored before any automatic checkpoint runs — the
/// content an automatic checkpoint carries forward, never invents.
fn stub_handoff() -> Handoff {
    Handoff {
        objective: "close map line 802".to_owned(),
        implementation_state: "wiring the automatic-checkpoint trigger".to_owned(),
        decisions: vec!["mirror the memory_extraction switch".to_owned()],
        memory: Vec::new(),
        failed_approaches: Vec::new(),
        files: vec!["crates/glasshouse/src/main.rs".to_owned()],
        test_state: Some("session_hook tests pending".to_owned()),
        next_actions: vec!["run the mutation".to_owned()],
    }
}

/// Seed a manual checkpoint for `id`, the way a user's own `glasshouse
/// checkpoint save` would: an automatic checkpoint has nothing to carry
/// forward, and takes none, without one.
fn seed_checkpoint(runtime: &Runtime, id: &SessionId, harness: &str) -> Stored {
    let project_checkpoints = ProjectCheckpoints::open(runtime).unwrap();
    let store = project_checkpoints.store();
    store
        .save(Checkpoint::capture(
            id,
            harness,
            CheckpointReason::Manual,
            store.now(),
            runtime.project().root(),
            stub_handoff(),
        ))
        .unwrap()
}

/// Every checkpoint this project holds for one session, most recent first.
fn checkpoints_for(runtime: &Runtime, id: &SessionId) -> Vec<Stored> {
    let project_checkpoints = ProjectCheckpoints::open(runtime).unwrap();
    project_checkpoints
        .store()
        .list()
        .unwrap()
        .into_iter()
        .filter(|stored| &stored.checkpoint.session == id)
        .collect()
}

/// Map line 802's premise (§17): with the setting on — the default, so no
/// `automatic_checkpoint` key is written at all — a `Stop` for a completed
/// turn leaves behind a checkpoint that did not exist before, carrying
/// forward the handoff of the checkpoint the session already had. Asserted
/// on the stored checkpoint itself, not on an absence a no-op would also
/// produce.
///
/// This is the control for
/// [`automatic_checkpoint_disabled_in_user_config_is_not_attempted_while_the_hook_still_records_the_turn`]:
/// without it, that test's silence would be indistinguishable from a hook
/// that never takes an automatic checkpoint at all.
#[test]
fn automatic_checkpoint_left_enabled_takes_a_checkpoint_after_a_completed_turn() {
    let fixture = Fixture::new();
    let id = running_session(&fixture, "codex");
    let seeded = seed_checkpoint(&fixture.runtime, &id, "codex");

    assert!(fixture.hook(id.as_str(), "Stop", &payload()).success());

    let after = checkpoints_for(&fixture.runtime, &id);
    assert_eq!(
        after.len(),
        2,
        "a completed turn with automatic checkpoints on must leave behind one \
         new checkpoint beside the seeded one: {after:?}"
    );
    let taken = after
        .iter()
        .find(|stored| stored.id != seeded.id)
        .expect("a checkpoint that did not exist before the hook ran");
    assert_eq!(
        taken.checkpoint.reason,
        CheckpointReason::TaskBoundary,
        "an automatic checkpoint must record why it was taken"
    );
    assert_eq!(
        taken.checkpoint.handoff, seeded.checkpoint.handoff,
        "an automatic checkpoint carries the existing handoff forward rather than inventing one"
    );
}

/// Map line 802, the line itself. `automatic_checkpoint = false` planted in
/// the user config layer — through
/// [`UserConfig::set_automatic_checkpoint`] and [`UserConfig::save`], so this
/// test breaks if the key is ever renamed — makes
/// `automatic_checkpoint_enabled(runtime)` false, and the same completed turn
/// the premise test above drives leaves no new checkpoint behind.
///
/// The lifecycle event is still recorded: this proves the switch turned off
/// checkpoints specifically, not that the hook did nothing at all.
#[test]
fn automatic_checkpoint_disabled_in_user_config_is_not_attempted_while_the_hook_still_records_the_turn()
 {
    let fixture = Fixture::new();
    let id = running_session(&fixture, "codex");
    let seeded = seed_checkpoint(&fixture.runtime, &id, "codex");

    let mut user = UserConfig::load(fixture.runtime.paths())
        .expect("a fresh fixture has no config file yet, which loads as the default");
    user.set_automatic_checkpoint(Some(false));
    user.save(fixture.runtime.paths())
        .expect("the user config layer must be writable in the fixture's own tempdir");

    let before = fixture.log().len().unwrap();

    assert!(fixture.hook(id.as_str(), "Stop", &payload()).success());

    let after = checkpoints_for(&fixture.runtime, &id);
    assert_eq!(
        after.len(),
        1,
        "no checkpoint should be taken while automatic_checkpoint=false: {after:?}"
    );
    assert_eq!(
        after[0].id, seeded.id,
        "the only checkpoint must still be the seeded one"
    );

    // The switch turned off checkpoints, not the hook: the turn's own closing
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
        "the turn's own closing event must still be recorded even with checkpoints off: {recorded:?}"
    );
}

/// The two switches are independent: memory extraction disabled must not
/// disable automatic checkpoints. `automatic_checkpoint` is left at its
/// default (enabled) while `memory_extraction = false` is planted in the
/// user config layer, and the same completed turn still leaves behind a new
/// `task_boundary` checkpoint.
#[test]
fn automatic_checkpoint_still_runs_when_memory_extraction_is_disabled() {
    let fixture = Fixture::new();
    let id = running_session(&fixture, "codex");
    let seeded = seed_checkpoint(&fixture.runtime, &id, "codex");

    let mut user = UserConfig::load(fixture.runtime.paths())
        .expect("a fresh fixture has no config file yet, which loads as the default");
    user.set_memory_extraction(Some(false));
    user.save(fixture.runtime.paths())
        .expect("the user config layer must be writable in the fixture's own tempdir");

    assert!(fixture.hook(id.as_str(), "Stop", &payload()).success());

    let after = checkpoints_for(&fixture.runtime, &id);
    assert_eq!(
        after.len(),
        2,
        "checkpoints must still be taken with memory extraction off: {after:?}"
    );
    assert!(
        after.iter().any(|stored| stored.id != seeded.id
            && stored.checkpoint.reason == CheckpointReason::TaskBoundary),
        "an automatic checkpoint must still appear when only memory extraction is disabled: {after:?}"
    );
}

/// The Codex hook catalogue this build declares was read from the Codex this
/// machine has installed.
///
/// **Why this test exists.** `harness::codex::HOOK_EVENTS` is an *observed*
/// catalogue: Codex publishes no machine-readable list of its hook events,
/// and — verified on 0.150.1 — a `hooks.json` naming an event it does not
/// recognise is accepted in silence, with no diagnostic on any non-interactive
/// path. So a Codex release that renamed, removed, or re-scoped an event
/// would break a capability here and *nothing in this repository would say
/// so*. That is not hypothetical: between 0.149.1 and 0.150.1 the catalogue
/// gained `Interrupt`, and it was found by a person re-reading the review
/// screen by hand, not by a test.
///
/// The only thing left to hold Codex to, without a terminal, is the
/// provenance itself: the version the catalogue was read from must still be
/// the version installed. This would have failed the moment this machine
/// moved to 0.150.1.
///
/// It is deliberately **not** an assertion about `HOOK_EVENTS`'s contents.
/// Comparing a constant to itself is the vacuous shape this suite already
/// gets caught by; the claim here is about the world, and the only way to
/// satisfy it is to go and look.
///
/// Skips when Codex is not installed, so this is inert on CI and on any
/// machine without it — the check is for the development machine the
/// catalogue is read on.
#[test]
fn the_codex_hook_catalogue_was_read_from_the_installed_codex() {
    let Ok(codex) = glasshouse::platform::exec::resolve("codex") else {
        eprintln!("skipping: `codex` is not on PATH, so there is no catalogue to compare against");
        return;
    };

    let Ok(output) = Command::new(codex.path()).arg("--version").output() else {
        eprintln!("skipping: `codex --version` could not be run");
        return;
    };
    let reported = String::from_utf8_lossy(&output.stdout);
    // `codex --version` prints `codex-cli <version>`; take the last field so
    // a change to the product name ahead of it does not read as a drift.
    let Some(installed) = reported.split_whitespace().next_back() else {
        eprintln!("skipping: `codex --version` printed nothing to compare");
        return;
    };

    assert_eq!(
        installed,
        glasshouse::harness::codex::CATALOGUE_OBSERVED_VERSION,
        "Codex {installed} is installed, but this build's hook catalogue was read from \
         {}. The catalogue is observed rather than documented, so a version bump is the \
         only warning available that `PreCompact` — or any other event Glasshouse \
         subscribes to — may have been renamed or removed. Re-read Codex's hook review \
         screen, reconcile `harness::codex::HOOK_EVENTS` with what it shows, and only \
         then move `CATALOGUE_OBSERVED_VERSION`.",
        glasshouse::harness::codex::CATALOGUE_OBSERVED_VERSION
    );
}

// -------------------------------------------------------------------------
// A resumed session's harness is believed again.
//
// Found against a live Codex by `GH-CODEX-COMPACTION-PROBE`: a session that
// was quit and then continued by a second `glasshouse launch` kept
// `lifecycle = stopped` for the rest of its life, and every hook it sent
// afterwards was discarded. The state under test is therefore **a session
// that has been through a genuine resume**, not a session that received an
// event — see `resume_the_way_a_launch_does` for why the distinction is the
// whole defect.
// -------------------------------------------------------------------------

/// A recorded session that has stopped with something to resume to, which is
/// the only shape a resume can be performed on.
///
/// `set_native_session_id` is what makes the record `Resumable` rather than
/// `Closed`: without a native identifier there is nothing to resume *to*, and
/// `SessionRecord::disposition` says so.
fn stopped_resumable_session(fixture: &Fixture, harness: &str) -> SessionId {
    let id = running_session(fixture, harness);
    let sessions = ProjectSessions::open(&fixture.runtime).unwrap();
    let store = sessions.store();
    store.set_native_session_id(&id, "thread-77").unwrap();
    store.set_lifecycle(&id, SessionLifecycle::Stopped).unwrap();
    id
}

/// The two steps `main.rs::resume_session` performs, in its order, against
/// this project's own store.
///
/// This mirrors production rather than reaching past it: `open_for_resume` is
/// the resume boundary — it carries the project-isolation check, the
/// supervision guard and the disposition check — and the lifecycle write
/// immediately after it is the one the launch path makes before handing the
/// harness the conversation.
///
/// A test cannot drive the rest of `resume_session`, which spawns a real
/// harness executable and attaches a terminal to it; `pty_smoke.rs` covers
/// that half against a fake harness. What is reproduced here is the **store
/// state a genuine resume leaves behind**, which is what every later hook is
/// judged against.
fn resume_the_way_a_launch_does(fixture: &Fixture, id: &SessionId) {
    let sessions = ProjectSessions::open(&fixture.runtime).unwrap();
    let store = sessions.store();
    let resumable = store
        .open_for_resume(id)
        .expect("a stopped session with a native identifier is resumable");
    store
        .begin_resume(&resumable)
        .expect("the resume path records that the session is running again");
}

fn lifecycle_of(fixture: &Fixture, id: &SessionId) -> SessionLifecycle {
    ProjectSessions::open(&fixture.runtime)
        .unwrap()
        .store()
        .get(id)
        .unwrap()
        .unwrap()
        .lifecycle
}

/// **The defect.** A session that was stopped and then genuinely resumed must
/// believe its harness again.
///
/// Two assertions, and they fail in that order against the unfixed tree: the
/// resume itself must leave the record live, and a turn the harness reports
/// afterwards must move it. The second is the contract — *"when its harness
/// reports any lifecycle event, Glasshouse believes it"* — and the first is
/// why the second could not hold.
///
/// `Stop` rather than `UserPromptSubmit`, deliberately. A resumed session is
/// already `Running`, and `may_apply` refuses a transition to the state a
/// session is already in, so a `UserPromptSubmit` that changed nothing would
/// leave `Running` behind whether the hook was believed or discarded. `Stop`
/// means `Idle`, which the record can only be holding if the hook was applied.
///
/// # What this test cannot see, and where that half is proved
///
/// `running_session` creates the record with `SessionStore::create` **inside
/// this test process**, so the process identity on the row is the test
/// binary's own and verifies for the whole run. A real resume happens in a
/// process the creating `glasshouse` has already left — and until the resume
/// recorded an identity of its own, supervision verified that departed
/// process at the next `ProjectSessions::open`, concluded the session was
/// lost, and wrote `stopped` back over the resume roughly a millisecond
/// before the hook's own transition was refused against the state it had just
/// caused. Both assertions below hold against a tree with that defect,
/// because a live identity is exactly what the fixture supplies.
///
/// So this test is the contract and not the whole proof of it.
/// `tests/session_supervision.rs::a_resumed_sessions_hook_is_believed_rather_than_refused_by_its_own_arrival`
/// is the same claim where the identity is genuinely dead: the session there
/// is created by one `glasshouse launch` that exits, resumed by a second, and
/// the event is delivered by a real `glasshouse hook` process.
#[test]
fn a_resumed_session_believes_its_harness_again() {
    let fixture = Fixture::new();
    let id = stopped_resumable_session(&fixture, "codex");

    resume_the_way_a_launch_does(&fixture, &id);
    assert_eq!(
        lifecycle_of(&fixture, &id),
        SessionLifecycle::Running,
        "a genuine resume must leave the session live; a record still reading `stopped` is \
         what makes every later hook unbelievable"
    );

    assert!(fixture.hook(id.as_str(), "Stop", &payload()).success());

    assert_eq!(
        lifecycle_of(&fixture, &id),
        SessionLifecycle::Idle,
        "the harness reported a turn ending in a session Glasshouse itself resumed, and the \
         report was discarded"
    );
}

/// **The zombie defence, which must still hold.** A hook for a session that
/// stopped and was *not* resumed changes nothing about its state.
///
/// This is the case the rule was written for: hook processes are separate
/// processes and a slow one can deliver its event after the harness it
/// belongs to has exited. The sibling
/// `a_late_hook_is_recorded_without_reviving_a_finished_session` asserts the
/// same thing for one event; this one sweeps every event that implies a live
/// state, so a fix that reopened the door for any of them is caught here
/// rather than by whichever one happened to be written down.
#[test]
fn a_hook_for_a_session_that_was_never_resumed_is_still_refused() {
    let fixture = Fixture::new();
    let id = stopped_resumable_session(&fixture, "codex");

    for event in [
        "SessionStart",
        "UserPromptSubmit",
        "PermissionRequest",
        "Stop",
    ] {
        assert!(fixture.hook(id.as_str(), event, &payload()).success());
        assert_eq!(
            lifecycle_of(&fixture, &id),
            SessionLifecycle::Stopped,
            "`{event}` arriving for a session nobody resumed revived it"
        );
    }
}

/// A resume is not a permanent licence. Once a resumed session stops again,
/// the next late hook is refused exactly as the first incarnation's would
/// have been.
///
/// Without this, "was this session ever resumed?" would be the question the
/// gate asks, and a zombie from before the resume would be believed forever
/// after.
#[test]
fn a_session_that_stopped_after_being_resumed_is_finished_again() {
    let fixture = Fixture::new();
    let id = stopped_resumable_session(&fixture, "codex");
    resume_the_way_a_launch_does(&fixture, &id);

    ProjectSessions::open(&fixture.runtime)
        .unwrap()
        .store()
        .set_lifecycle(&id, SessionLifecycle::Stopped)
        .unwrap();

    assert!(fixture.hook(id.as_str(), "Stop", &payload()).success());
    assert_eq!(
        lifecycle_of(&fixture, &id),
        SessionLifecycle::Stopped,
        "a session that has stopped again is finished again, however many times it was resumed"
    );
}

/// A **failed** session is not resumable, so nothing can revive it.
///
/// `Failed` is not `Stopped`: the process ended badly, `disposition` reports
/// `Failed` rather than `Resumable`, and `open_for_resume` refuses it by
/// name. The resume path is therefore unreachable for such a record, and the
/// only thing that can arrive is a late hook — which is refused.
#[test]
fn a_failed_session_is_not_resumable_and_no_hook_revives_it() {
    let fixture = Fixture::new();
    let id = stopped_resumable_session(&fixture, "codex");
    let sessions = ProjectSessions::open(&fixture.runtime).unwrap();
    sessions
        .store()
        .set_lifecycle(&id, SessionLifecycle::Failed)
        .unwrap();

    let refusal = sessions
        .store()
        .open_for_resume(&id)
        .expect_err("a failed session has no resume boundary to cross");
    assert!(
        format!("{refusal}").contains("failed"),
        "the refusal must name why: {refusal}"
    );

    assert!(fixture.hook(id.as_str(), "Stop", &payload()).success());
    assert_eq!(
        lifecycle_of(&fixture, &id),
        SessionLifecycle::Failed,
        "a failed session was revived by a hook"
    );
}

/// A **closed** session is not resumable, so nothing can revive it.
///
/// `Closed` is not `Stopped` either: the user retired the record. It is the
/// one finished state a person chose, and `close` refuses to file a live
/// session away precisely so that the word keeps meaning that.
#[test]
fn a_closed_session_is_not_resumable_and_no_hook_revives_it() {
    let fixture = Fixture::new();
    let id = stopped_resumable_session(&fixture, "codex");
    let sessions = ProjectSessions::open(&fixture.runtime).unwrap();
    sessions.store().close(&id).unwrap();

    let refusal = sessions
        .store()
        .open_for_resume(&id)
        .expect_err("a closed session has no resume boundary to cross");
    assert!(
        format!("{refusal}").contains("closed"),
        "the refusal must name why: {refusal}"
    );

    assert!(fixture.hook(id.as_str(), "Stop", &payload()).success());
    assert_eq!(
        lifecycle_of(&fixture, &id),
        SessionLifecycle::Closed,
        "a closed session was revived by a hook"
    );
}

/// What the user sees. After a resume and a turn the harness reported,
/// `glasshouse sessions show` says the session is live rather than stopped.
///
/// Through the shipped binary and its own printer, because "the record is
/// right" and "the report is right" are two claims and this package was
/// raised against the second one: the probe's evidence was a listing that
/// kept saying `stopped` while Codex was answering prompts.
#[test]
fn a_resumed_sessions_turn_is_reported_as_live_rather_than_stopped() {
    let fixture = Fixture::new();
    let id = stopped_resumable_session(&fixture, "codex");
    resume_the_way_a_launch_does(&fixture, &id);

    assert!(
        fixture
            .hook(id.as_str(), "UserPromptSubmit", &payload())
            .success()
    );
    let working = fixture.sessions_show(id.as_str());
    assert_eq!(
        reported(&working, "lifecycle"),
        "running",
        "a resumed session taking a turn must not be reported as stopped:\n{working}"
    );

    assert!(fixture.hook(id.as_str(), "Stop", &payload()).success());
    let idle = fixture.sessions_show(id.as_str());
    assert_eq!(
        reported(&idle, "lifecycle"),
        "idle",
        "a resumed session's completed turn must be reported:\n{idle}"
    );
    assert_eq!(
        reported(&idle, "state"),
        "active",
        "the disposition a listing shows must follow the lifecycle:\n{idle}"
    );
}
