use super::*;
use crate::events::LifecycleEvent;
use crate::launch::HarnessLaunch;
use crate::platform::exec;
use crate::session::{NewSession, SessionPresentation};
use crate::{Cli, Runtime};
use clap::Parser;
use rusqlite::Connection;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicI64, Ordering};
use std::time::{Duration, Instant};

const TIMEOUT: Duration = Duration::from_secs(15);
const POLL: Duration = Duration::from_millis(20);

/// A bootstrapped project with an open connection to its database.
///
/// Mirrors `session::store`'s own test fixture: this module needs
/// `crate::database::open`, which is crate-private, so the pattern is
/// reproduced here rather than imported from a private test module.
struct Fixture {
    base: PathBuf,
    runtime: Runtime,
    conn: Connection,
}

impl Fixture {
    fn new(base: &Path, name: &str) -> Self {
        let root = base.join("workspace").join(name);
        std::fs::create_dir_all(root.join(".git")).unwrap();
        let root = std::fs::canonicalize(&root).unwrap();
        let runtime = bootstrap_at(base, &root);
        let conn = crate::database::open(&runtime).unwrap();
        Self {
            base: base.to_path_buf(),
            runtime,
            conn,
        }
    }

    fn store(&self) -> SessionStore<'_> {
        SessionStore::new(&self.conn).unwrap()
    }

    fn store_with_ticking_clock(&self, start: i64, step: i64) -> SessionStore<'_> {
        let next = AtomicI64::new(start);
        let clock: crate::session::store::Clock =
            std::sync::Arc::new(move || next.fetch_add(step, Ordering::SeqCst));
        SessionStore::with_clock(&self.conn, clock).unwrap()
    }

    fn project_id(&self) -> &str {
        self.runtime.project().id().as_str()
    }

    /// A second project sharing this machine's data/config root.
    fn sibling(&self, name: &str) -> Runtime {
        let root = self.base.join("workspace").join(name);
        std::fs::create_dir_all(root.join(".git")).unwrap();
        let root = std::fs::canonicalize(&root).unwrap();
        bootstrap_at(&self.base, &root)
    }
}

fn bootstrap_at(base: &Path, root: &Path) -> Runtime {
    let cli = Cli::try_parse_from([
        "glasshouse",
        "--data-dir",
        base.join("data").to_str().unwrap(),
        "--config-dir",
        base.join("config").to_str().unwrap(),
    ])
    .unwrap();
    crate::bootstrap(&cli, root).unwrap()
}

/// Insert a row directly, bypassing [`SessionStore`] entirely, to model a
/// session bearing another project's identifier however it got there.
/// See `session::store`'s own test module for the fuller explanation.
fn plant_foreign_row(conn: &Connection, id: &str, project_id: &str, native: Option<&str>) {
    conn.execute_batch("DROP TRIGGER sessions_reject_foreign_project_insert;")
        .unwrap();
    conn.execute(
        "INSERT INTO sessions (id, project_id, harness, native_session_id, role, \
         lifecycle, presentation, created_at, last_activity_at) \
         VALUES (?1, ?2, 'claude-code', ?3, 'normal', 'stopped', 'embedded', 10, 20)",
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

/// A harness that reads one line and echoes it back, so a real send can
/// be observed reaching a real terminal.
#[cfg(unix)]
fn install_echo_harness(bin_dir: &Path) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let path = bin_dir.join("echo-harness");
    std::fs::write(&path, "#!/bin/sh\nread line\necho \"got:$line\"\n").unwrap();
    let mut perms = std::fs::metadata(&path).unwrap().permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&path, perms).unwrap();
    path
}

#[cfg(windows)]
fn install_echo_harness(bin_dir: &Path) -> PathBuf {
    let path = bin_dir.join("echo-harness.cmd");
    std::fs::write(
        &path,
        "@echo off\r\nsetlocal enabledelayedexpansion\r\nset /p line=\r\necho got:!line!\r\n",
    )
    .unwrap();
    path
}

/// A harness that announces itself and then stays alive, for tests that
/// interrupt it or write to it.
///
/// The `READY` line matters: on Windows the child does not start until
/// something answers ConPTY's `ESC[6n` query (see `drive`'s doc comment),
/// so a harness that produced no output could still be sitting unstarted
/// at the handshake when a test acts on it. `READY` in the scrollback is
/// proof the process the test is about to interrupt is the real one.
///
/// The sleep is minutes, not seconds, and that is load-bearing, found by
/// a mutation that should have failed and did not: a shorter sleep can
/// finish **on its own** inside a test's own wait window, and a test
/// that only checks "is it dead yet" cannot tell that apart from a real
/// interrupt. Dropping the interrupt delivery entirely (see this
/// module's mutation record) still passed against a 30-second sleep,
/// because the process outlived the interrupt but not the test's own
/// patience. A sleep long enough to outlast any reasonable wait removes
/// that escape.
#[cfg(unix)]
fn install_sleepy_harness(bin_dir: &Path) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let path = bin_dir.join("sleepy-harness");
    std::fs::write(&path, "#!/bin/sh\necho READY\nsleep 300\n").unwrap();
    let mut perms = std::fs::metadata(&path).unwrap().permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&path, perms).unwrap();
    path
}

/// The Windows equivalent of a shell's `trap ... INT`, and why there is no
/// `sleepy-harness.cmd` beside the one above any more.
///
/// There was one, and
/// [`interrupting_through_the_api_is_recorded_as_machine_initiated`] used
/// it: a `.cmd` running `timeout /t 300 /nobreak`, whose reaction to the
/// interrupt was read as a `^C` appearing in the session's scrollback.
/// That marker went quiet, and `test (windows)` on `655bbc0` failed with
/// *"timed out waiting for the interrupted harness to react"* after the
/// full 45 seconds — while `pty_smoke::interrupt_is_delivered_as_a_terminal_interrupt`
/// and `pty_smoke::an_interrupt_reaches_an_unfocused_session_and_leaves_it_running`
/// **passed in the same run**, both of them proving a real `CTRL_C_EVENT`
/// reaching a real Windows child through this very seam.
///
/// So the interrupt was never the thing that failed; the marker was. It
/// could not have been anything else, because it is unobservable in both
/// directions: a `^C` echo is the *console's* reaction and depends on the
/// input mode `timeout` happens to have set when the byte lands, and a
/// child that died of the interrupt produces no echo either — the check
/// cannot tell "nothing arrived" from "everything arrived and cmd.exe ate
/// the echo".
///
/// This is the marker those two passing tests use instead, and it is the
/// one `pty_smoke` already wrote down the reasoning for: a child that
/// installs a real `SetConsoleCtrlHandler`, says so, returns *handled*,
/// and keeps running. No `cmd.exe` script can do that, so the harness is
/// this same test binary re-entered — which is why it lives here and not
/// beside the shell scripts.
///
/// Inert unless `GLASSHOUSE_INTERRUPT_TRAP` is set, so it costs the
/// ordinary suite one immediately-returning test.
#[cfg(windows)]
static CAUGHT_CONSOLE_INTERRUPT: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

/// Returning non-zero means *handled*, which is what stops the default
/// action — ending the process — and is the whole point: interrupting is
/// not killing.
#[cfg(windows)]
unsafe extern "system" fn note_console_interrupt(event: u32) -> i32 {
    const CTRL_C_EVENT: u32 = 0;
    if event == CTRL_C_EVENT {
        CAUGHT_CONSOLE_INTERRUPT.store(true, std::sync::atomic::Ordering::SeqCst);
        1
    } else {
        0
    }
}

#[cfg(windows)]
unsafe extern "system" {
    fn SetConsoleCtrlHandler(
        handler: Option<unsafe extern "system" fn(u32) -> i32>,
        add: i32,
    ) -> i32;
}

/// Start the trap child described by `CAUGHT_CONSOLE_INTERRUPT` as a
/// session's harness: this same test binary, re-entered with the console
/// control handler installed.
///
/// **Compiled on every platform and called only on Windows**, which is
/// deliberate. Practice §18 says compile the other platform's path, and
/// the one part of this that genuinely cannot be — the handler itself,
/// whose `SetConsoleCtrlHandler` has no symbol to link against off
/// Windows — is a copy of code `pty_smoke` already runs green there. This
/// half is new, so it is written where a macOS `cargo test` typechecks it.
#[cfg_attr(not(windows), allow(dead_code))]
fn start_the_interrupt_trap_child(fixture: &Fixture, live: &mut SessionRuntime, id: SessionId) {
    let exe = std::env::current_exe().expect("current exe");
    let resolved = exec::resolve_explicit(&exe).expect("resolve this test binary");
    let launch = HarnessLaunch::new(resolved, fixture.runtime.project())
        .args([
            "--exact",
            "session::api::tests::windows_interrupt_trap_child",
            "--nocapture",
            "--test-threads=1",
        ])
        .env("GLASSHOUSE_INTERRUPT_TRAP", "1");
    live.start(id, SessionPresentation::Embedded, &launch)
        .expect("start the trap child as a harness");
}

/// See [`CAUGHT_CONSOLE_INTERRUPT`].
#[cfg(windows)]
#[test]
fn windows_interrupt_trap_child() {
    use std::io::Write as _;

    if std::env::var_os("GLASSHOUSE_INTERRUPT_TRAP").is_none() {
        return;
    }
    assert_ne!(
        unsafe { SetConsoleCtrlHandler(Some(note_console_interrupt), 1) },
        0,
        "could not install a console control handler"
    );

    // Printed only after the handler is installed, so a caller that has
    // seen this marker knows the interrupt cannot hit the default action.
    println!("TRAP-READY");
    std::io::stdout().flush().expect("flush");

    let deadline = Instant::now() + Duration::from_secs(60);
    let mut announced = false;
    while Instant::now() < deadline {
        if !announced && CAUGHT_CONSOLE_INTERRUPT.load(std::sync::atomic::Ordering::SeqCst) {
            println!("CAUGHT-INTERRUPT");
            std::io::stdout().flush().expect("flush");
            announced = true;
        }
        std::thread::sleep(POLL);
    }
}

/// A harness that echoes every line it reads, forever, so more than one
/// send can be observed reaching a real, still-running child.
#[cfg(unix)]
fn install_looping_echo_harness(bin_dir: &Path) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let path = bin_dir.join("looping-echo-harness");
    std::fs::write(
        &path,
        "#!/bin/sh\nwhile IFS= read -r line; do echo \"got:$line\"; done\n",
    )
    .unwrap();
    let mut perms = std::fs::metadata(&path).unwrap().permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&path, perms).unwrap();
    path
}

#[cfg(windows)]
fn install_looping_echo_harness(bin_dir: &Path) -> PathBuf {
    let path = bin_dir.join("looping-echo-harness.cmd");
    std::fs::write(
        &path,
        "@echo off\r\nsetlocal enabledelayedexpansion\r\n:loop\r\nset \"line=\"\r\nset /p line=\r\necho got:!line!\r\ngoto loop\r\n",
    )
    .unwrap();
    path
}

fn start_live_session(
    fixture: &Fixture,
    live: &mut SessionRuntime,
    id: SessionId,
    bin_dir: &Path,
    install: impl FnOnce(&Path) -> PathBuf,
) {
    let harness_path = install(bin_dir);
    let resolved = exec::resolve_explicit(&harness_path).unwrap();
    let launch = HarnessLaunch::new(resolved, fixture.runtime.project());
    live.start(id, SessionPresentation::Embedded, &launch)
        .unwrap();
}

/// Drive the runtime the way an owner of one does, until `done` is
/// satisfied, and fail with what was actually seen.
///
/// `answer_terminal_queries` is in the loop because it is in every
/// production tick — `shell::run`'s and the headless launch loop in
/// `main.rs` both call it — and on Windows it is not a nicety. ConPTY
/// asks `ESC[6n` while bringing the pseudo-console up and **does not let
/// the child start** until something replies; Glasshouse is the terminal
/// for an embedded session, so nothing else can. Probed on the Windows
/// ARM64 CI VM: a harness whose first act was to write a file outside the
/// pty had still not written it three seconds after spawn, and the entire
/// scrollback was the one unanswered query. Answering produced the file,
/// the child's echo, and the buffered input, in that order.
///
/// So a wait loop that only accumulates output is modelling an owner of a
/// [`SessionRuntime`] that cannot exist. This mirrors
/// `tests/events_lifecycle.rs`'s `drive` for the same reason.
fn drive(live: &mut SessionRuntime, what: &str, mut done: impl FnMut(&mut SessionRuntime) -> bool) {
    let deadline = Instant::now() + TIMEOUT;
    loop {
        live.answer_terminal_queries();
        live.poll_exits();
        if done(live) {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for {what}; sessions: {live:?}"
        );
        std::thread::sleep(POLL);
    }
}

#[test]
fn listing_returns_every_session_in_this_project_most_recent_first() {
    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path(), "alpha");
    let store = fixture.store_with_ticking_clock(500, 10);

    let first = store.create(NewSession::embedded("claude-code")).unwrap();
    let second = store.create(NewSession::embedded("codex")).unwrap();
    store.touch(&first.id).unwrap();

    let mut live = SessionRuntime::new();
    let api = SessionApi::new(&store, &mut live);

    let listed: Vec<_> = api.list().unwrap().into_iter().map(|r| r.id).collect();
    assert_eq!(listed, vec![first.id, second.id]);
}

#[test]
fn state_reports_what_the_store_recorded() {
    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path(), "alpha");
    let store = fixture.store();
    let record = store.create(NewSession::embedded("claude-code")).unwrap();

    store
        .set_lifecycle(&record.id, SessionLifecycle::WaitingForUser)
        .unwrap();

    let mut live = SessionRuntime::new();
    let api = SessionApi::new(&store, &mut live);

    assert_eq!(
        api.state(&record.id).unwrap(),
        SessionLifecycle::WaitingForUser,
        "waiting for the user and idle are different states"
    );

    store
        .set_lifecycle(&record.id, SessionLifecycle::Idle)
        .unwrap();
    assert_eq!(api.state(&record.id).unwrap(), SessionLifecycle::Idle);
}

#[test]
fn sending_text_to_a_live_session_reaches_its_terminal() {
    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path(), "alpha");
    let bin_dir = tmp.path().join("bin");
    std::fs::create_dir_all(&bin_dir).unwrap();
    let store = fixture.store();
    let record = store.create(NewSession::embedded("claude-code")).unwrap();

    let mut live = SessionRuntime::new();
    start_live_session(
        &fixture,
        &mut live,
        record.id.clone(),
        &bin_dir,
        install_echo_harness,
    );

    {
        let mut api = SessionApi::new(&store, &mut live);
        api.send_text(&record.id, "hello", MessageOrigin::Machine)
            .unwrap();
    }

    drive(&mut live, "the harness to echo the line back", |live| {
        live.get(&record.id)
            .map(|session| session.scrollback().contains("got:hello"))
            .unwrap_or(false)
    });
}

/// **Capability map line 1719, the expiry half.**
///
/// `tests/user_control.rs` proves the refusal through the shipped binary,
/// which is where a rule about a person at a keyboard belongs. It cannot
/// prove that the window *ends* without sleeping through
/// [`crate::session::runtime::USER_INPUT_PRECEDENCE`], and a test that slept ten
/// seconds to observe a constant would be paid for on every run of this
/// suite forever.
///
/// So the clock is the argument. `note_user_input` takes the moment as a
/// parameter and every production caller passes `Instant::now()` — this
/// passes a moment on the far side of the window, through **the same
/// call the binary makes**, rather than through a `#[cfg(test)]` door
/// beside it.
#[test]
fn a_machine_message_is_delivered_once_the_persons_window_has_passed() {
    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path(), "alpha");
    let bin_dir = tmp.path().join("bin");
    std::fs::create_dir_all(&bin_dir).unwrap();
    let store = fixture.store();
    let record = store.create(NewSession::embedded("claude-code")).unwrap();

    let mut live = SessionRuntime::new();
    start_live_session(
        &fixture,
        &mut live,
        record.id.clone(),
        &bin_dir,
        install_looping_echo_harness,
    );

    // A person, a moment further back than the window is wide. The
    // window has closed, so the machine line is delivered — and
    // delivered to a real process rather than merely permitted, which
    // the harness's own echo is what proves.
    //
    // This half runs **first** because the mark never moves backwards
    // (see [`SessionRuntime::note_user_input`]): a test that stamped
    // `now` and then backdated would be asserting against a rule that
    // deliberately ignores the second call.
    live.note_user_input(
        &record.id,
        Instant::now() - crate::session::runtime::USER_INPUT_PRECEDENCE - Duration::from_secs(1),
    );
    {
        let mut api = SessionApi::new(&store, &mut live);
        api.send_text(&record.id, "after-the-window", MessageOrigin::Machine)
            .expect("the window has passed, so the machine line is delivered");
    }
    drive(&mut live, "the harness to echo the machine line", |live| {
        live.get(&record.id)
            .map(|session| session.scrollback().contains("got:after-the-window"))
            .unwrap_or(false)
    });

    // The same person, now. The machine is refused, and the refusal
    // names a time — asserted so the half above cannot pass on a build
    // where nothing is ever refused.
    live.note_user_input(&record.id, Instant::now());
    {
        let mut api = SessionApi::new(&store, &mut live);
        match api.send_text(&record.id, "too-soon", MessageOrigin::Machine) {
            Err(ApiError::UserHasTheKeyboard { seconds, .. }) => assert!(
                seconds > 0,
                "a refusal naming no remaining time reads as a bug"
            ),
            other => panic!(
                "a machine line must be refused while a person holds the keyboard, and \
                 this was {other:?}"
            ),
        }
    }
}

/// The mark never moves backwards, so a person typing steadily keeps the
/// keyboard rather than losing it to the age of their first line.
///
/// Written because the obvious implementation — assign whatever arrives —
/// is wrong in exactly one direction and in a way no timing test would
/// catch: two calls in the natural order both extend the window, and only
/// an out-of-order pair distinguishes "keep the later" from "keep the
/// last".
#[test]
fn an_older_keystroke_does_not_shorten_the_window_a_newer_one_opened() {
    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path(), "alpha");
    let bin_dir = tmp.path().join("bin");
    std::fs::create_dir_all(&bin_dir).unwrap();
    let store = fixture.store();
    let record = store.create(NewSession::embedded("claude-code")).unwrap();

    let mut live = SessionRuntime::new();
    start_live_session(
        &fixture,
        &mut live,
        record.id.clone(),
        &bin_dir,
        install_looping_echo_harness,
    );

    let now = Instant::now();
    live.note_user_input(&record.id, now);
    live.note_user_input(
        &record.id,
        now - crate::session::runtime::USER_INPUT_PRECEDENCE - Duration::from_secs(1),
    );

    let mut api = SessionApi::new(&store, &mut live);
    assert!(
        matches!(
            api.send_text(&record.id, "still-refused", MessageOrigin::Machine),
            Err(ApiError::UserHasTheKeyboard { .. })
        ),
        "the stale mark must not have replaced the live one"
    );
}

/// The load-bearing test of the batch: a machine-sent line and a typed
/// one must never be merged in Glasshouse's own event log, even though
/// the harness on the other end cannot tell them apart at all.
#[test]
fn a_machine_sent_line_is_recorded_separately_from_a_keystroke() {
    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path(), "alpha");
    let bin_dir = tmp.path().join("bin");
    std::fs::create_dir_all(&bin_dir).unwrap();
    let store = fixture.store();
    let record = store.create(NewSession::embedded("claude-code")).unwrap();

    let mut live = SessionRuntime::new();
    start_live_session(
        &fixture,
        &mut live,
        record.id.clone(),
        &bin_dir,
        install_looping_echo_harness,
    );

    {
        let mut api = SessionApi::new(&store, &mut live);
        api.send_text(&record.id, "machine-line", MessageOrigin::Machine)
            .unwrap();
    }
    // A dead-at-handshake child (never past ConPTY's `ESC[6n`, see
    // `drive`'s doc comment) would never echo this: this line is proof
    // the machine send reached a real, running process, not just
    // Glasshouse's own event log.
    drive(&mut live, "the harness to echo the machine line", |live| {
        live.get(&record.id)
            .map(|session| session.scrollback().contains("got:machine-line"))
            .unwrap_or(false)
    });

    assert!(live.write_to_focused(b"keystroke-line\r").unwrap());
    drive(
        &mut live,
        "the harness to echo the keystroke line",
        |live| {
            live.get(&record.id)
                .map(|session| session.scrollback().contains("got:keystroke-line"))
                .unwrap_or(false)
        },
    );

    let history = live.events().history_for(&record.id);
    let origins: Vec<MessageOrigin> = history
        .iter()
        .filter_map(|recorded| match recorded.event() {
            LifecycleEvent::TextDelivered { origin, .. } => Some(*origin),
            _ => None,
        })
        .collect();

    assert_eq!(
        origins,
        vec![MessageOrigin::Machine, MessageOrigin::UserKeystroke],
        "a machine-sent line and a keystroke must be recorded with distinct origins, in order"
    );
}

/// The seam records the origin it is told, and no longer decides one.
///
/// The distinction from
/// [`a_machine_sent_line_is_recorded_separately_from_a_keystroke`] is the
/// whole point: that test writes one line through this API and one
/// through the keyboard, so it would pass just as well against the old
/// hard-wired `MessageOrigin::Machine`. **Both** lines here go through
/// [`SessionApi::send_text`], on the same session, against the same
/// running process — so the only thing that can make their recorded
/// origins differ is the argument, which is exactly the property the
/// control door needs and did not have.
///
/// Ordered, not merely set-equal: the second row is the person's, and a
/// seam that stamped every write with its first caller's origin would
/// produce two identical rows here.
#[test]
fn the_seam_records_whichever_origin_its_caller_states() {
    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path(), "alpha");
    let bin_dir = tmp.path().join("bin");
    std::fs::create_dir_all(&bin_dir).unwrap();
    let store = fixture.store();
    let record = store.create(NewSession::embedded("claude-code")).unwrap();

    let mut live = SessionRuntime::new();
    start_live_session(
        &fixture,
        &mut live,
        record.id.clone(),
        &bin_dir,
        install_looping_echo_harness,
    );

    // Deliberately the same length, so `bytes` cannot be what tells the
    // two rows apart — the same care `tests/worker_access.rs` takes at
    // the door.
    const BY_GLASSHOUSE: &str = "sent-by-machine";
    const BY_A_PERSON: &str = "sent-by-persons";
    assert_eq!(BY_GLASSHOUSE.len(), BY_A_PERSON.len());

    for (text, origin) in [
        (BY_GLASSHOUSE, MessageOrigin::Machine),
        (BY_A_PERSON, MessageOrigin::UserKeystroke),
    ] {
        {
            let mut api = SessionApi::new(&store, &mut live);
            api.send_text(&record.id, text, origin).unwrap();
        }
        // Echoed by a real process before the next line is sent: the
        // rows below are about two deliveries that demonstrably
        // happened, and the wait also fixes their order.
        drive(&mut live, "the harness to echo the line back", |live| {
            live.get(&record.id)
                .map(|session| session.scrollback().contains(&format!("got:{text}")))
                .unwrap_or(false)
        });
    }

    let history = live.events().history_for(&record.id);
    let delivered: Vec<(MessageOrigin, usize)> = history
        .iter()
        .filter_map(|recorded| match recorded.event() {
            LifecycleEvent::TextDelivered { origin, bytes } => Some((*origin, *bytes)),
            _ => None,
        })
        .collect();

    assert_eq!(
        delivered,
        vec![
            (MessageOrigin::Machine, BY_GLASSHOUSE.len() + 1),
            (MessageOrigin::UserKeystroke, BY_A_PERSON.len() + 1),
        ],
        "both lines went through `SessionApi::send_text` and differ only in \
         the origin their caller stated, so the recorded origins must \
         differ too — a seam that decides the origin itself cannot record \
         a person's intervention through the control door: {history:?}"
    );
}

#[test]
fn interrupting_through_the_api_is_recorded_as_machine_initiated() {
    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path(), "alpha");
    let bin_dir = tmp.path().join("bin");
    std::fs::create_dir_all(&bin_dir).unwrap();
    let store = fixture.store();
    let record = store.create(NewSession::embedded("claude-code")).unwrap();

    let mut live = SessionRuntime::new();
    #[cfg(unix)]
    start_live_session(
        &fixture,
        &mut live,
        record.id.clone(),
        &bin_dir,
        install_sleepy_harness,
    );
    // A different harness on Windows, for the reason
    // `CAUGHT_CONSOLE_INTERRUPT` gives: nothing a `cmd.exe` script can do
    // reports a console control event, so the harness is this test binary
    // re-entered with a real handler installed.
    #[cfg(windows)]
    start_the_interrupt_trap_child(&fixture, &mut live, record.id.clone());

    // Wait for the harness's own marker, not just for the runtime to say
    // the session started: on Windows the child does not run until
    // something answers ConPTY's handshake (see `drive`'s doc comment),
    // so without this a dead-at-handshake child would still "pass" the
    // interrupt below — nothing would be there to receive it, and nothing
    // here would notice. On Windows the marker also means the console
    // control handler is installed, which has to be true before the
    // interrupt arrives or this would prove the opposite of what it
    // claims.
    const HARNESS_IS_UP: &str = if cfg!(windows) { "TRAP-READY" } else { "READY" };
    drive(&mut live, "the harness to announce itself", |live| {
        live.get(&record.id)
            .map(|session| session.scrollback().contains(HARNESS_IS_UP))
            .unwrap_or(false)
    });
    assert!(
        live.get(&record.id).unwrap().is_running(),
        "the harness must be a real, still-running process before it is interrupted"
    );

    {
        let mut api = SessionApi::new(&store, &mut live);
        api.interrupt(&record.id, MessageOrigin::Machine).unwrap();
    }

    // Each harness reports the interrupt the only way its platform can,
    // and both are proof a dead child could never produce -- as opposed
    // to the event log entry alone, which the API writes whether or not
    // anything was listening.
    //
    // On Unix the shell's `sleep` simply dies: no trap, default action.
    // On Windows the trap child says `CAUGHT-INTERRUPT` from inside its
    // console control handler, so what is observed is a real
    // `CTRL_C_EVENT` rather than a byte the child happened to read, and
    // the child is still running when it says it. See
    // [`CAUGHT_CONSOLE_INTERRUPT`] for what this replaced and why.
    //
    // A longer deadline than `drive`'s shared `TIMEOUT`, not a shorter
    // one: this waits on a child's own reaction to a control event under
    // whatever load the rest of the suite is putting on the same machine,
    // and that reaction has been observed to take longer than 15s here
    // specifically when many pty-heavy tests are running at once
    // (§34/§40's standing timing debt, not a defect in the wait condition
    // itself).
    //
    // Whether the child is still alive is in the failure message because
    // the last time this timed out it was not, and that was the whole
    // question: `SessionRuntime`'s `Debug` says only how many sessions
    // there are, so the gate could not tell "the interrupt never arrived"
    // from "it arrived and killed the harness". The *scrollback* stays
    // out of it — this file's error text does not carry a session's
    // terminal contents.
    {
        let deadline = Instant::now() + Duration::from_secs(45);
        loop {
            live.answer_terminal_queries();
            live.poll_exits();
            let reacted = live
                .get(&record.id)
                .map(|session| {
                    if cfg!(windows) {
                        session.scrollback().contains("CAUGHT-INTERRUPT")
                    } else {
                        !session.is_running()
                    }
                })
                .unwrap_or(false);
            if reacted {
                break;
            }
            let still_running = live
                .get(&record.id)
                .map(|session| session.is_running())
                .unwrap_or(false);
            assert!(
                Instant::now() < deadline,
                "timed out waiting for the interrupted harness to react to the interrupt; \
                 sessions: {live:?}; harness still running: {still_running}"
            );
            std::thread::sleep(POLL);
        }
    }

    let history = live.events().history_for(&record.id);
    assert!(
        history.iter().any(|recorded| matches!(
            recorded.event(),
            LifecycleEvent::InterruptDelivered {
                origin: MessageOrigin::Machine
            }
        )),
        "expected a machine-initiated interrupt in the history: {history:?}"
    );
}

#[test]
fn messaging_a_session_from_another_project_is_refused_by_name() {
    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path(), "alpha");
    let other = fixture.sibling("beta");
    let other_id = other.project().id().as_str().to_owned();
    assert_ne!(
        other_id,
        fixture.project_id(),
        "fixture must use two projects"
    );

    plant_foreign_row(&fixture.conn, "planted", &other_id, None);

    let store = fixture.store();
    let mut live = SessionRuntime::new();
    let mut api = SessionApi::new(&store, &mut live);

    let error = api
        .send_text(&SessionId::new("planted"), "hi", MessageOrigin::Machine)
        .expect_err("a session from another project must never be messaged");

    match &error {
        ApiError::ForeignProject {
            id,
            expected,
            actual,
        } => {
            assert_eq!(id.as_str(), "planted");
            assert_eq!(expected, fixture.project_id());
            assert_eq!(actual, &other_id);
        }
        other => panic!("expected ForeignProject, got {other:?}"),
    }
}

#[test]
fn every_api_call_refuses_a_foreign_session() {
    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path(), "alpha");
    let other = fixture.sibling("beta");
    let other_id = other.project().id().as_str().to_owned();

    plant_foreign_row(&fixture.conn, "planted", &other_id, None);
    let id = SessionId::new("planted");

    let store = fixture.store();
    let mut live = SessionRuntime::new();
    let mut api = SessionApi::new(&store, &mut live);

    assert!(
        matches!(api.state(&id), Err(ApiError::ForeignProject { .. })),
        "state must refuse a foreign session"
    );
    assert!(
        matches!(
            api.send_text(&id, "hi", MessageOrigin::Machine),
            Err(ApiError::ForeignProject { .. })
        ),
        "send_text must refuse a foreign session"
    );
    assert!(
        matches!(
            api.interrupt(&id, MessageOrigin::Machine),
            Err(ApiError::ForeignProject { .. })
        ),
        "interrupt must refuse a foreign session"
    );
    assert!(
        matches!(
            api.recent_output(&id, 100),
            Err(ApiError::ForeignProject { .. })
        ),
        "recent_output must refuse a foreign session"
    );

    let listed = api.list().unwrap();
    assert!(
        listed.iter().all(|record| record.id != id),
        "list must never surface a session from another project"
    );
}

#[test]
fn a_foreign_session_that_is_not_running_is_refused_as_foreign_not_as_dead() {
    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path(), "alpha");
    let other = fixture.sibling("beta");
    let other_id = other.project().id().as_str().to_owned();

    // Planted with no native session id and lifecycle 'stopped': not
    // live in this runtime either, so a check that asked liveness first
    // would answer NotLive instead of the true answer.
    plant_foreign_row(&fixture.conn, "planted", &other_id, None);
    let id = SessionId::new("planted");

    let store = fixture.store();
    let mut live = SessionRuntime::new();
    let mut api = SessionApi::new(&store, &mut live);

    let error = api
        .send_text(&id, "hi", MessageOrigin::Machine)
        .expect_err("a foreign, non-running session must still be refused");
    assert!(
        matches!(error, ApiError::ForeignProject { .. }),
        "expected ForeignProject, got {error:?} — scope must be checked before liveness"
    );
}

#[test]
fn recent_output_returns_the_tail_and_never_splits_a_character() {
    let text = "héllo";
    assert_eq!(
        tail(text, 100),
        "héllo",
        "shorter than max_bytes returns everything"
    );
    assert_eq!(tail(text, 5), "éllo");
    assert_eq!(
        tail(text, 4),
        "llo",
        "a boundary that would split é drops the whole character rather than mangling it"
    );
    assert_eq!(tail(text, 0), "");
    for cut in 0..=text.len() {
        assert!(
            std::str::from_utf8(tail(text, cut).as_bytes()).is_ok(),
            "tail({cut}) produced invalid UTF-8"
        );
    }
}
