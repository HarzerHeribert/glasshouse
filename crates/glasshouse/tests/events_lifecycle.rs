//! The lifecycle event stream against real child processes.
//!
//! Every real defect this project has found came from running something in a
//! real terminal rather than from a unit test, and the properties here are
//! exactly the kind a unit test flatters: *one dying worker does not take the
//! others*, *a stalled consumer does not stall a harness*, *a quiet process is
//! never reported as having finished its work*. Each of those is trivially
//! true against a stub and is the whole question against a pty.
//!
//! So these start real executables through the sanctioned
//! [`glasshouse::launch::HarnessLaunch`] seam — no explicit working directory
//! or program appears anywhere below — and drive them through
//! [`SessionRuntime`], the same type `shell::run` owns in the shipped
//! binary.

#![cfg(any(unix, windows))]

use std::time::{Duration, Instant};

use glasshouse::Project;
use glasshouse::events::{EventBus, LifecycleEvent, MessageOrigin, ProcessExit, task_outcome};
use glasshouse::launch::HarnessLaunch;
use glasshouse::platform::exec;
use glasshouse::session::{
    LiveSession, SessionId, SessionLifecycle, SessionPresentation, SessionRuntime,
};

/// Long enough for a child to start and die on a loaded machine, short enough
/// that a hang is a failure rather than a wait.
const TIMEOUT: Duration = Duration::from_secs(20);
const POLL: Duration = Duration::from_millis(20);

/// A project and a directory to drop fake harnesses into.
///
/// A real `Project` discovered from a real (empty) `.git` directory, so every
/// `HarnessLaunch` built from it derives a real, project-bound working
/// directory rather than a stand-in.
struct Fixture {
    _tmp: tempfile::TempDir,
    project: Project,
    bin_dir: std::path::PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let tmp = tempfile::tempdir().expect("tempdir");
        let project_dir = tmp.path().join("proj");
        std::fs::create_dir_all(project_dir.join(".git")).expect("create project");
        let bin_dir = tmp.path().join("bin");
        std::fs::create_dir_all(&bin_dir).expect("create bin dir");
        let project = Project::discover(&project_dir, None, false).expect("discover project");
        Self {
            _tmp: tmp,
            project,
            bin_dir,
        }
    }

    fn launch(&self, path: &std::path::Path) -> HarnessLaunch<'_> {
        let resolved = exec::resolve_explicit(path).expect("resolve fake harness");
        HarnessLaunch::new(resolved, &self.project)
    }
}

/// A harness that announces itself and then dies badly.
///
/// Unix kills itself with `SIGKILL`, which is the shape of a real crash and
/// produces a signal in the exit status. Windows has no signals, so it exits
/// with a non-zero code; both are [`ProcessExit::is_crash`], and the
/// assertions below are written against that rather than against whichever
/// spelling this machine happens to produce.
#[cfg(unix)]
fn install_crashing_harness(bin_dir: &std::path::Path, name: &str) -> std::path::PathBuf {
    unix_script(bin_dir, name, "#!/bin/sh\necho STARTED\nkill -9 $$\n")
}

#[cfg(windows)]
fn install_crashing_harness(bin_dir: &std::path::Path, name: &str) -> std::path::PathBuf {
    windows_script(bin_dir, name, "@echo off\r\necho STARTED\r\nexit /b 3\r\n")
}

/// A harness that says one thing and leaves on its own terms.
#[cfg(unix)]
fn install_quiet_harness(bin_dir: &std::path::Path, name: &str) -> std::path::PathBuf {
    unix_script(bin_dir, name, "#!/bin/sh\necho STARTED\nexit 0\n")
}

#[cfg(windows)]
fn install_quiet_harness(bin_dir: &std::path::Path, name: &str) -> std::path::PathBuf {
    windows_script(bin_dir, name, "@echo off\r\necho STARTED\r\nexit /b 0\r\n")
}

/// A harness that reads a line and echoes it back, then keeps waiting.
///
/// Short output on purpose: ConPTY renders into a fixed-width screen buffer
/// and reflows anything wider, so a test that parses long lines through a pty
/// passes on Unix and fails on Windows for reasons that have nothing to do
/// with the code under test.
#[cfg(unix)]
fn install_echo_harness(bin_dir: &std::path::Path, name: &str) -> std::path::PathBuf {
    unix_script(
        bin_dir,
        name,
        "#!/bin/sh\necho STARTED\nwhile IFS= read -r line; do echo \"GOT:$line\"; done\n",
    )
}

#[cfg(windows)]
fn install_echo_harness(bin_dir: &std::path::Path, name: &str) -> std::path::PathBuf {
    windows_script(
        bin_dir,
        name,
        "@echo off\r\necho STARTED\r\n:loop\r\nset \"line=\"\r\nset /p line=\r\n\
         if defined line echo GOT:%line%\r\ngoto loop\r\n",
    )
}

#[cfg(unix)]
fn unix_script(bin_dir: &std::path::Path, name: &str, body: &str) -> std::path::PathBuf {
    use std::os::unix::fs::PermissionsExt;

    let path = bin_dir.join(name);
    std::fs::write(&path, body).expect("write fake harness");
    let mut perms = std::fs::metadata(&path).unwrap().permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&path, perms).unwrap();
    path
}

#[cfg(windows)]
fn windows_script(bin_dir: &std::path::Path, name: &str, body: &str) -> std::path::PathBuf {
    let path = bin_dir.join(format!("{name}.cmd"));
    std::fs::write(&path, body).expect("write fake harness");
    path
}

/// Drive the runtime the way `shell::run`'s tick does until `done` is
/// satisfied, or fail with what was seen.
///
/// `answer_terminal_queries` is in the loop because it is in the production
/// tick: an embedded session has no real terminal behind it, so a harness
/// waiting on `ESC[6n` waits forever unless Glasshouse answers. Leaving it
/// out would make these tests hang on Windows for a reason unrelated to what
/// they assert.
fn drive(
    runtime: &mut SessionRuntime,
    what: &str,
    mut done: impl FnMut(&mut SessionRuntime) -> bool,
) {
    let deadline = Instant::now() + TIMEOUT;
    loop {
        runtime.answer_terminal_queries();
        runtime.poll_exits();
        if done(runtime) {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for {what}; sessions: {runtime:?}"
        );
        std::thread::sleep(POLL);
    }
}

fn exited(runtime: &mut SessionRuntime, id: &SessionId) -> bool {
    runtime.get(id).is_some_and(|s| s.exit().is_some())
}

/// Phase 45: "Preserve terminal output and event history after a worker
/// crashes."
///
/// Neither belongs to the process that died — the scrollback is Glasshouse's
/// own buffer and the history is the project's bus — so a crash costs
/// neither. The session is deliberately left in the runtime after it exits;
/// removing it is the only way to lose the output, and `poll_exits` does not.
#[test]
fn a_crashed_worker_leaves_its_output_and_its_event_history_behind() {
    let fixture = Fixture::new();
    let harness = install_crashing_harness(&fixture.bin_dir, "crasher");
    let mut runtime = SessionRuntime::new();
    let id = SessionId::new("crashed-session");

    runtime
        .start(
            id.clone(),
            SessionPresentation::Embedded,
            &fixture.launch(&harness),
        )
        .expect("start the crashing harness");

    drive(&mut runtime, "the harness to crash", |runtime| {
        exited(runtime, &id)
    });

    let report = runtime
        .crash_report(&id)
        .expect("a harness that died badly must produce a crash report");

    assert!(report.exit.is_crash(), "{:?} is not a crash", report.exit);
    assert_eq!(report.exit.session_state(), SessionLifecycle::Failed);
    assert!(
        report.output.contains("STARTED"),
        "the crashed worker's terminal output must survive it; got {:?}",
        report.output
    );
    assert!(
        report
            .history
            .iter()
            .any(|recorded| recorded.event() == &LifecycleEvent::SessionStarted),
        "the event history must survive too: {:?}",
        report.history
    );
    assert!(
        report
            .history
            .iter()
            .any(|recorded| matches!(recorded.event(), LifecycleEvent::ProcessExited { .. })),
        "including the exit itself: {:?}",
        report.history
    );

    // On Unix a crash really is a signal. The *name* is deliberately not
    // asserted: portable-pty passes the platform's own description through,
    // and macOS says `Killed: 9` where another Unix says something else. Both
    // satisfy the property under test — the status carries a signal — and
    // pinning one machine's spelling is how a green local run turns into a
    // red job on a platform nobody looked at.
    #[cfg(unix)]
    assert!(
        report.exit.signal().is_some(),
        "a signalled death must carry its signal: {:?}",
        report.exit
    );

    // And the standing rule holds through a crash: nothing here says the work
    // was finished, because no harness ever said so.
    assert_eq!(task_outcome(&report.history), None);
}

/// Phase 45: "Ensure one failed worker cannot terminate unrelated sessions or
/// the entire Glasshouse instance."
///
/// Three live sessions, one killed. The other two must still be running, must
/// still accept input, and must still produce output afterwards — a runtime
/// that survived the poll but could no longer be used would satisfy a weaker
/// test and be useless.
#[test]
fn one_worker_crashing_leaves_unrelated_sessions_running() {
    let fixture = Fixture::new();
    let crasher = install_crashing_harness(&fixture.bin_dir, "crasher");
    let survivor = install_echo_harness(&fixture.bin_dir, "echoer");

    let mut runtime = SessionRuntime::new();
    let doomed = SessionId::new("doomed");
    let alpha = SessionId::new("alpha");
    let beta = SessionId::new("beta");

    for (id, program) in [(&alpha, &survivor), (&doomed, &crasher), (&beta, &survivor)] {
        runtime
            .start(
                id.clone(),
                SessionPresentation::Embedded,
                &fixture.launch(program),
            )
            .expect("start a session");
    }

    drive(&mut runtime, "the doomed session to die", |runtime| {
        exited(runtime, &doomed)
    });

    for id in [&alpha, &beta] {
        let session = runtime.get(id).expect("an unrelated session was removed");
        assert!(
            session.is_running(),
            "session `{id}` was taken down by an unrelated crash"
        );
    }

    // Still steerable, not merely still listed. This is the half a weaker
    // test would miss.
    //
    // `\r`, not `\n`, and not because Windows likes carriage returns: it is
    // what both production senders already append — `shell::run`'s
    // `format!("{text}\r")` and `SessionApi::send_text` — because that is the
    // byte a terminal's line editor treats as submit. A Windows console never
    // completes a `set /p` on `\n`, so a test that writes its own terminator
    // was exercising a line nothing in the binary sends.
    for id in [&alpha, &beta] {
        runtime
            .send_text_from(id, "ping\r", MessageOrigin::Machine)
            .expect("an unrelated session must still accept input");
    }
    for id in [&alpha, &beta] {
        drive(&mut runtime, "the survivors to answer", |runtime| {
            runtime
                .get(id)
                .map(LiveSession::scrollback)
                .unwrap_or_default()
                .contains("GOT:ping")
        });
    }

    // And exactly one exit was reported, not three.
    let exits = runtime
        .events()
        .history()
        .into_iter()
        .filter(|recorded| matches!(recorded.event(), LifecycleEvent::ProcessExited { .. }))
        .collect::<Vec<_>>();
    assert_eq!(exits.len(), 1, "exactly one session ended: {exits:?}");
    assert_eq!(exits[0].session(), &doomed);
}

/// Phase 12: "Deliver lifecycle events to the TUI without blocking the
/// harness process."
///
/// The mechanism that would break this is not obvious from the bus's own
/// tests: a session's reader thread publishes, and a reader that waits stops
/// draining the pseudo-terminal, whose buffer then fills, and the harness
/// blocks on `write`. So this fills a subscriber's queue to its bound, never
/// drains it, and then requires the harness to keep talking.
#[test]
fn a_stalled_event_consumer_does_not_stall_a_live_harness() {
    let fixture = Fixture::new();
    let harness = install_echo_harness(&fixture.bin_dir, "echoer");

    let bus = EventBus::new();
    // Room for one, and nobody will ever drain it.
    let stalled = bus.subscribe_with_capacity(1);
    let mut runtime = SessionRuntime::with_event_bus(64 * 1024, bus.clone());
    let id = SessionId::new("live");

    runtime
        .start(
            id.clone(),
            SessionPresentation::Embedded,
            &fixture.launch(&harness),
        )
        .expect("start the harness");

    drive(&mut runtime, "the harness to start", |runtime| {
        runtime
            .get(&id)
            .map(LiveSession::scrollback)
            .unwrap_or_default()
            .contains("STARTED")
    });

    assert_eq!(stalled.pending(), 1, "the consumer's queue is full");

    // `\r` for the reason `one_worker_crashing_leaves_unrelated_sessions_running`
    // records: it is the terminator every production sender appends, and the
    // only one a Windows console accepts as submit.
    for round in 0..40 {
        runtime
            .send_text_from(&id, &format!("r{round}\r"), MessageOrigin::Machine)
            .expect("the harness must still be writable");
    }
    drive(&mut runtime, "the harness to answer the last round", |rt| {
        rt.get(&id)
            .map(LiveSession::scrollback)
            .unwrap_or_default()
            .contains("GOT:r39")
    });

    assert!(
        runtime.get(&id).is_some_and(LiveSession::is_running),
        "the harness must still be alive after a consumer stopped consuming"
    );
    assert_eq!(
        stalled.pending(),
        1,
        "the stalled queue never grew past its bound"
    );
    assert!(
        stalled.dropped() > 0,
        "events were dropped rather than made to wait"
    );
}

/// The capability map's standing rule, end to end: *do not infer successful
/// task completion solely because a child process became quiet.*
///
/// This harness does exactly what a finished one looks like from outside. It
/// prints, goes silent, and exits zero. Every observable signal is present
/// except the only one that means anything — a harness saying a turn ended —
/// and Glasshouse must therefore report that it does not know.
#[test]
fn a_quiet_harness_that_exits_cleanly_is_never_reported_as_having_finished() {
    let fixture = Fixture::new();
    let harness = install_quiet_harness(&fixture.bin_dir, "quiet");
    let mut runtime = SessionRuntime::new();
    let id = SessionId::new("quiet-session");

    runtime
        .start(
            id.clone(),
            SessionPresentation::Embedded,
            &fixture.launch(&harness),
        )
        .expect("start the quiet harness");

    drive(&mut runtime, "the harness to exit", |runtime| {
        exited(runtime, &id)
    });
    drive(&mut runtime, "its output to end", |runtime| {
        runtime.get(&id).is_some_and(LiveSession::output_ended)
    });

    let history = runtime.events().history_for(&id);

    // The whole quiet-completion picture really is present…
    assert!(
        history
            .iter()
            .any(|recorded| recorded.event() == &LifecycleEvent::OutputEnded),
        "the output really did end: {history:?}"
    );
    let exit = history
        .iter()
        .find_map(|recorded| match recorded.event() {
            LifecycleEvent::ProcessExited { exit } => Some(exit.clone()),
            _ => None,
        })
        .expect("the process really did exit");
    assert!(!exit.is_crash(), "and it exited on its own terms: {exit:?}");
    assert_eq!(exit.session_state(), SessionLifecycle::Stopped);

    // …and it still does not add up to a finished task.
    assert_eq!(
        task_outcome(&history),
        None,
        "a quiet, clean exit is not a harness saying the work finished"
    );
    assert!(
        runtime.crash_report(&id).is_none(),
        "and a session that left on its own terms did not crash"
    );
}

/// A deliberate close is not a crash.
///
/// `SessionRuntime::close` signals `Kill`, which on Unix produces exactly the
/// exit status a crash produces. It is not one, and the difference is
/// structural rather than a matter of inspecting the status: `close` removes
/// the session before it signals, so there is nothing left for `poll_exits`
/// to report or for a crash report to be built from.
#[test]
fn closing_a_session_is_not_reported_as_a_crash() {
    let fixture = Fixture::new();
    let harness = install_echo_harness(&fixture.bin_dir, "echoer");
    let mut runtime = SessionRuntime::new();
    let id = SessionId::new("closed");

    runtime
        .start(
            id.clone(),
            SessionPresentation::Embedded,
            &fixture.launch(&harness),
        )
        .expect("start the harness");
    drive(&mut runtime, "the harness to start", |runtime| {
        runtime
            .get(&id)
            .map(LiveSession::scrollback)
            .unwrap_or_default()
            .contains("STARTED")
    });

    runtime.close(&id).expect("close the session");

    assert!(runtime.get(&id).is_none(), "close forgets the session");
    assert!(runtime.crash_report(&id).is_none());
    assert!(
        runtime.poll_exits().is_empty(),
        "a closed session produces no exit to report"
    );
    assert!(
        !runtime
            .events()
            .history_for(&id)
            .iter()
            .any(|recorded| matches!(recorded.event(), LifecycleEvent::ProcessExited { .. })),
        "and no exit event: a deliberate close is not a crash"
    );
}

// -------------------------------------------------------------------------
// A restart is not an ending: what `poll_exits` may report about a session
// whose harness it put back.
// -------------------------------------------------------------------------

/// A harness that comes up, stays up long enough to be believed, and then
/// dies badly.
///
/// The delay is what makes this shape different from `install_crashing_harness`
/// above: `consider_restart` deliberately leaves a harness that was never
/// healthy alone, so a restart can only be provoked by outliving
/// `HEALTHY_AFTER` first. Unix signals itself for the same reason the crashing
/// harness does — it is the shape of a real crash — and Windows, which has no
/// signals, exits non-zero.
#[cfg(unix)]
fn install_healthy_then_crashing_harness(
    bin_dir: &std::path::Path,
    name: &str,
) -> std::path::PathBuf {
    unix_script(
        bin_dir,
        name,
        "#!/bin/sh\necho STARTED\nsleep 3\nkill -9 $$\n",
    )
}

#[cfg(windows)]
fn install_healthy_then_crashing_harness(
    bin_dir: &std::path::Path,
    name: &str,
) -> std::path::PathBuf {
    // `ping` rather than `timeout`, which refuses to run when its input is
    // not a console it recognises; four pings is a little over three seconds.
    windows_script(
        bin_dir,
        name,
        "@echo off\r\necho STARTED\r\nping -n 4 127.0.0.1 > nul\r\nexit /b 3\r\n",
    )
}

/// A session that was restarted is not an exit any consumer may act on.
///
/// `poll_exits` notices the death, publishes `ProcessExited`, and then
/// `consider_restart` puts the harness back — and the entry must not survive
/// that. Every consumer of the returned vector treats an entry as terminal:
/// `shell::run` writes `ProcessExit::session_state()` into the durable record
/// and runs `session::native_id::capture`, and `supervision::guard_start`
/// returns `Ok` for any record whose lifecycle is not live — so a reported
/// restart leaves a record that offers a conversation whose harness is still
/// holding it.
///
/// The assertion is made *inside* the poll loop, against the runtime as it
/// stands at the moment of the report, because that is the moment the
/// consumer acts in.
#[test]
fn a_restarted_session_is_not_reported_as_an_exit() {
    let fixture = Fixture::new();
    let harness = install_healthy_then_crashing_harness(&fixture.bin_dir, "healthy-crasher");
    let bystander_harness = install_echo_harness(&fixture.bin_dir, "bystander");
    let mut runtime = SessionRuntime::new();
    let id = SessionId::new("restarted-session");
    let bystander = SessionId::new("bystander-session");

    // Started first, so it is the session the focus fix-up below would fall
    // to — `poll_exits` reassigns focus to the *first* running non-headless
    // session, not to the one that ended.
    runtime
        .start(
            bystander.clone(),
            SessionPresentation::Embedded,
            &fixture.launch(&bystander_harness),
        )
        .expect("start a bystander session");
    runtime
        .start(
            id.clone(),
            SessionPresentation::Embedded,
            &fixture.launch(&harness),
        )
        .expect("start the harness that dies after coming up");
    runtime
        .focus(&id)
        .expect("the crashing session holds the keyboard");

    // Only a harness that has once been healthy is restarted at all, so this
    // wait is what makes the rest of the test about a restart rather than
    // about a start that did not work.
    drive(
        &mut runtime,
        "the harness to be verified healthy",
        |runtime| runtime.get(&id).is_some_and(LiveSession::verified_healthy),
    );

    let deadline = Instant::now() + TIMEOUT;
    loop {
        runtime.answer_terminal_queries();
        for (ended, status) in runtime.poll_exits() {
            if ended != id {
                continue;
            }
            // Exactly what `shell::run` computes from an entry before it
            // writes it to the record.
            let lifecycle = ProcessExit::from_status(&status).session_state();
            assert!(
                !runtime.get(&id).is_some_and(LiveSession::is_running),
                "`poll_exits` reported session `{id}` as ended ({lifecycle:?}) while its \
                 harness is running again; a consumer writes that lifecycle to the \
                 durable record, and nothing refuses a resume of a record that is not live"
            );
        }
        if runtime
            .get(&id)
            .is_some_and(|session| session.restarts() >= 1)
        {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "the harness never crashed and was never put back; sessions: {runtime:?}"
        );
        std::thread::sleep(POLL);
    }

    let session = runtime.get(&id).expect("a restarted session is still held");
    assert!(
        session.is_running(),
        "the restarted harness must be alive: {:?}",
        session.exit()
    );
    assert!(
        runtime.poll_exits().iter().all(|(ended, _)| ended != &id),
        "and it must stay absent from later polls while it keeps running"
    );
    assert_eq!(
        runtime.focused(),
        Some(&id),
        "a crash-and-restart must not move the keyboard to a different harness"
    );

    // The death itself is not hidden — only the claim that the session is
    // over. The history is where a crash-and-restart is legible.
    assert!(
        runtime
            .events()
            .history_for(&id)
            .iter()
            .any(|recorded| matches!(recorded.event(), LifecycleEvent::ProcessExited { .. })),
        "the exit event must still have been published for the death"
    );

    runtime.close(&id).expect("close the restarted session");
}

/// The other half: a session that stays exited is reported exactly as before.
///
/// A harness that dies before it was ever healthy is not restarted — Phase
/// 10A's third exclusion — so its exit is terminal, and dropping it from
/// `poll_exits`' return would lose the only report the record is written
/// from.
#[test]
fn a_session_that_stays_exited_is_still_reported() {
    let fixture = Fixture::new();
    let harness = install_crashing_harness(&fixture.bin_dir, "stays-dead");
    let mut runtime = SessionRuntime::new();
    let id = SessionId::new("not-restarted");

    runtime
        .start(
            id.clone(),
            SessionPresentation::Embedded,
            &fixture.launch(&harness),
        )
        .expect("start the crashing harness");

    let deadline = Instant::now() + TIMEOUT;
    let status = loop {
        runtime.answer_terminal_queries();
        if let Some((_, status)) = runtime
            .poll_exits()
            .into_iter()
            .find(|(ended, _)| ended == &id)
        {
            break status;
        }
        assert!(
            Instant::now() < deadline,
            "`poll_exits` never reported the exit of a session nothing put back; \
             sessions: {runtime:?}"
        );
        std::thread::sleep(POLL);
    };

    let exit = ProcessExit::from_status(&status);
    assert!(exit.is_crash(), "{exit:?} is not a crash");
    assert!(
        !exit.session_state().is_live(),
        "the lifecycle a consumer records for it must not be a live one"
    );

    let session = runtime.get(&id).expect("the session is still held");
    assert!(!session.is_running(), "and the session really is over");
    assert_eq!(
        session.restarts(),
        0,
        "a harness that was never healthy must not have been restarted"
    );
    assert!(
        runtime.poll_exits().iter().all(|(ended, _)| ended != &id),
        "an exit is reported exactly once"
    );
}
