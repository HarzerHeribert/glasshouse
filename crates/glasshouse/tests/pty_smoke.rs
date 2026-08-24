//! Platform PTY smoke tests.
//!
//! These start a real interactive child process through the Glasshouse PTY
//! abstraction and check the four things every harness session depends on:
//! output streaming, keyboard input, window resize, and exit detection. They
//! are written to run unchanged on macOS, Linux, and native Windows so CI
//! proves the abstraction on each platform rather than only on the developer's.

use std::io::Read;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use glasshouse::pty::{ProcessSignal, PtyOutput, PtyProcess, TerminalCommand, TerminalSize};

/// Upper bound for any single wait in these tests. Generous enough for a loaded
/// CI runner, short enough that a genuine hang fails instead of stalling.
const TIMEOUT: Duration = Duration::from_secs(20);
const POLL: Duration = Duration::from_millis(25);

/// Build a command that runs `script` through the platform's shell.
///
/// `/V:ON` turns on delayed environment-variable expansion for cmd.exe. Any
/// script here that reads back a variable it just `set /p`'d into needs it:
/// cmd expands `%var%` at *parse* time, before `set /p` has run, so without
/// delayed expansion (`!var!`) a script would silently see the value from
/// before the read.
fn shell_command(script: &str, cwd: &std::path::Path) -> TerminalCommand {
    if cfg!(windows) {
        let comspec = std::env::var("COMSPEC").unwrap_or_else(|_| "cmd.exe".to_owned());
        TerminalCommand::new(comspec, cwd)
            .arg("/V:ON")
            .arg("/C")
            .arg(script)
    } else {
        TerminalCommand::new("/bin/sh", cwd).arg("-c").arg(script)
    }
}

/// Accumulates PTY output on a background thread, the way the session runtime
/// does, so tests can look at partial output while the child is still running.
struct Collector {
    buffer: Arc<Mutex<Vec<u8>>>,
    finished: Arc<AtomicBool>,
}

impl Collector {
    fn start(mut output: PtyOutput) -> Self {
        let buffer = Arc::new(Mutex::new(Vec::new()));
        let finished = Arc::new(AtomicBool::new(false));
        let thread_buffer = Arc::clone(&buffer);
        let thread_finished = Arc::clone(&finished);

        std::thread::spawn(move || {
            let mut chunk = [0u8; 4096];
            loop {
                match output.read(&mut chunk) {
                    // EOF, or the pty was torn down after the child exited.
                    Ok(0) | Err(_) => break,
                    Ok(n) => thread_buffer.lock().unwrap().extend_from_slice(&chunk[..n]),
                }
            }
            thread_finished.store(true, Ordering::SeqCst);
        });

        Self { buffer, finished }
    }

    fn text(&self) -> String {
        String::from_utf8_lossy(&self.buffer.lock().unwrap()).into_owned()
    }

    /// Wait until `needle` appears in the output, or fail with what was seen.
    fn expect(&self, needle: &str) {
        let deadline = Instant::now() + TIMEOUT;
        while Instant::now() < deadline {
            if self.text().contains(needle) {
                return;
            }
            if self.finished.load(Ordering::SeqCst) && self.text().contains(needle) {
                return;
            }
            std::thread::sleep(POLL);
        }
        panic!(
            "timed out waiting for {needle:?} in pty output.\n--- output ---\n{}\n--- end ---",
            self.text()
        );
    }
}

/// Wait for the process to exit, failing the test rather than hanging.
fn wait_for_exit(process: &mut PtyProcess) -> glasshouse::pty::ExitStatus {
    let deadline = Instant::now() + TIMEOUT;
    while Instant::now() < deadline {
        if let Some(status) = process.try_wait().expect("try_wait") {
            return status;
        }
        std::thread::sleep(POLL);
    }
    let _ = process.signal(ProcessSignal::Kill);
    panic!("child process did not exit within {TIMEOUT:?}");
}

#[test]
fn streams_output_and_reports_a_successful_exit() {
    let cwd = std::env::temp_dir();
    let (mut process, output) =
        PtyProcess::spawn(shell_command("echo glasshouse-ok", &cwd)).expect("spawn");
    let collector = Collector::start(output);

    collector.expect("glasshouse-ok");
    let status = wait_for_exit(&mut process);
    assert!(status.success(), "unexpected status: {status}");
    assert_eq!(status.code(), 0);
}

#[test]
fn reports_a_failing_exit_code() {
    let cwd = std::env::temp_dir();
    let (mut process, output) = PtyProcess::spawn(shell_command("exit 7", &cwd)).expect("spawn");
    let _collector = Collector::start(output);

    let status = wait_for_exit(&mut process);
    assert!(!status.success(), "expected failure, got: {status}");
    assert_eq!(status.code(), 7);
}

#[test]
fn forwards_input_to_an_interactive_child() {
    let cwd = std::env::temp_dir();
    let script = if cfg!(windows) {
        // `!line!` (delayed expansion, enabled via `/V:ON` in
        // `shell_command`) reads the value `set /p` just assigned; `%line%`
        // would read the value from before the assignment (here, unset).
        "set /p line= & echo got:!line!"
    } else {
        "read line; echo got:$line"
    };
    let (mut process, output) = PtyProcess::spawn(shell_command(script, &cwd)).expect("spawn");
    let collector = Collector::start(output);

    process.send_text("hello\r\n").expect("send_text");
    collector.expect("got:hello");
    wait_for_exit(&mut process);
}

#[test]
fn exit_is_detected_from_the_process_not_from_quiet_output() {
    let cwd = std::env::temp_dir();
    // A child that produces no output at all and then lingers: nothing in the
    // terminal stream distinguishes "thinking" from "finished".
    let script = if cfg!(windows) {
        "ping -n 2 127.0.0.1 > nul"
    } else {
        "sleep 0.4"
    };
    let (mut process, _output) = PtyProcess::spawn(shell_command(script, &cwd)).expect("spawn");

    assert!(
        process.try_wait().expect("try_wait").is_none(),
        "a running silent process must not look finished"
    );
    let status = wait_for_exit(&mut process);
    assert!(status.success(), "{status}");
}

#[test]
fn resize_reaches_the_operating_system() {
    let cwd = std::env::temp_dir();
    let script = if cfg!(windows) {
        "ping -n 20 127.0.0.1 > nul"
    } else {
        "sleep 20"
    };
    let (mut process, _output) =
        PtyProcess::spawn(shell_command(script, &cwd).size(TerminalSize::new(24, 80)))
            .expect("spawn");

    assert_eq!(
        process.os_size().expect("os_size"),
        TerminalSize::new(24, 80)
    );

    let resized = TerminalSize::new(40, 120);
    process.resize(resized).expect("resize");
    assert_eq!(process.size(), resized);
    assert_eq!(process.os_size().expect("os_size"), resized);

    process.signal(ProcessSignal::Kill).expect("kill");
    wait_for_exit(&mut process);
}

#[cfg(unix)]
#[test]
fn a_resize_is_visible_to_the_child_process() {
    let cwd = std::env::temp_dir();
    // The child reports its window size only after we resize and release it, so
    // the value it prints proves the kernel told the child, not just that
    // Glasshouse recorded a number.
    let (mut process, output) =
        PtyProcess::spawn(shell_command("read x; stty size", &cwd).size(TerminalSize::new(24, 80)))
            .expect("spawn");
    let collector = Collector::start(output);

    process.resize(TerminalSize::new(40, 120)).expect("resize");
    process.send_text("\n").expect("send_text");

    collector.expect("40 120");
    wait_for_exit(&mut process);
}

// Windows is deliberately not covered here. portable-pty creates the
// pseudoconsole with `PSEUDOCONSOLE_WIN32_INPUT_MODE`, under which conhost is
// documented to translate an incoming `0x03` byte into a Ctrl+C key event —
// so in principle `PtyProcess::interrupt` should work there too. But there is
// no cmd.exe (or portable `cmd /C` script) equivalent of "install a trap and
// prove it fired through the terminal, not some other path" the way a shell
// SIGINT trap does on Unix, and no Windows runner available here to check a
// candidate script actually behaves as expected. A test that cannot fail on
// a real regression is worse than no test, so this is left unverified rather
// than given a version that would pass vacuously.
#[cfg(unix)]
#[test]
fn interrupt_is_delivered_as_a_terminal_interrupt() {
    let cwd = std::env::temp_dir();
    // The shell installs a SIGINT trap, which only fires if the interrupt goes
    // through the terminal line discipline the way a real Ctrl-C does.
    let (mut process, output) = PtyProcess::spawn(shell_command(
        "trap 'echo caught-interrupt; exit 0' INT; echo ready; while true; do sleep 0.1; done",
        &cwd,
    ))
    .expect("spawn");
    let collector = Collector::start(output);

    collector.expect("ready");
    process.interrupt().expect("interrupt");

    collector.expect("caught-interrupt");
    wait_for_exit(&mut process);
}

#[test]
fn terminating_stops_a_long_running_process() {
    let cwd = std::env::temp_dir();
    let script = if cfg!(windows) {
        "ping -n 60 127.0.0.1 > nul"
    } else {
        "sleep 60"
    };
    let (mut process, _output) = PtyProcess::spawn(shell_command(script, &cwd)).expect("spawn");

    assert!(process.try_wait().expect("try_wait").is_none());
    process.signal(ProcessSignal::Terminate).expect("terminate");

    let status = wait_for_exit(&mut process);
    assert!(!status.success(), "terminated process reported: {status}");
}

#[test]
fn signalling_an_exited_process_is_reported_rather_than_misdirected() {
    let cwd = std::env::temp_dir();
    let (mut process, _output) = PtyProcess::spawn(shell_command("exit 0", &cwd)).expect("spawn");
    wait_for_exit(&mut process);

    // The pid may already have been recycled by the operating system, so this
    // must never turn into a signal aimed at an unrelated process.
    let err = process.signal(ProcessSignal::Kill).unwrap_err();
    assert!(matches!(err, glasshouse::pty::SignalError::AlreadyExited));
}

/// Regression test: `signal` used to trust a stale `exit_status` cache that
/// only a previous `wait`/`try_wait` call would have populated. Nothing
/// forced a poll of its own, so a process that exited without anyone ever
/// having polled it still looked "not yet known to be exited" and `signal`
/// would proceed to actually deliver a signal — to a pid the OS is free to
/// have already recycled for an unrelated process. This test deliberately
/// never calls `try_wait`/`wait` before signalling, unlike
/// `signalling_an_exited_process_is_reported_rather_than_misdirected` above,
/// which does (via `wait_for_exit`) and so would not have caught this bug.
#[test]
fn signalling_an_unpolled_but_exited_process_is_reported_rather_than_misdirected() {
    let cwd = std::env::temp_dir();
    let (mut process, _output) = PtyProcess::spawn(shell_command("exit 0", &cwd)).expect("spawn");

    // Give the child time to actually exit without ever polling it through
    // `PtyProcess` — that would populate `exit_status` and let even the old,
    // buggy check pass by accident. `exit 0` (and its cmd.exe equivalent)
    // finishes near-instantly, so a short fixed sleep comfortably covers it
    // on any CI runner while staying bounded.
    std::thread::sleep(Duration::from_secs(1));

    let err = process.signal(ProcessSignal::Kill).unwrap_err();
    assert!(matches!(err, glasshouse::pty::SignalError::AlreadyExited));
}

/// Regression test for the defect that mattered most: `signal` used to target
/// the terminal's *foreground* process group (`tcgetpgrp`) rather than the
/// child's own group. A harness that uses job control hands the terminal to a
/// descendant, so `Terminate` reported success while the session leader — the
/// harness itself — kept running. For a control plane whose only way to shut a
/// session down is this call, "reported success, nothing died" is the worst
/// possible failure.
///
/// `set -m` turns job control on, which is what puts the child `sleep` in a
/// different process group from the shell.
#[cfg(unix)]
#[test]
fn terminate_reaches_the_session_leader_under_job_control() {
    let cwd = std::env::temp_dir();
    let (mut process, _output) = PtyProcess::spawn(
        TerminalCommand::new("/bin/sh", &cwd)
            .arg("-c")
            .arg("set -m; sleep 60; sleep 60"),
    )
    .expect("spawn");

    // Let the shell reach the point where the inner `sleep` owns the terminal.
    let deadline = Instant::now() + Duration::from_secs(2);
    while Instant::now() < deadline {
        std::thread::sleep(POLL);
    }
    assert!(
        process.try_wait().expect("try_wait").is_none(),
        "the shell should still be running"
    );

    process.signal(ProcessSignal::Terminate).expect("terminate");

    let status = wait_for_exit(&mut process);
    assert!(
        !status.success(),
        "the shell survived Terminate and reported: {status}"
    );
}

/// Regression test: `PtyProcess` had no `Drop`, so a child that was still
/// running when its `PtyProcess` was dropped simply kept running,
/// unreachable, forever.
#[cfg(unix)]
#[test]
fn dropping_a_running_process_kills_it() {
    let cwd = std::env::temp_dir();
    let (process, _output) = PtyProcess::spawn(shell_command("sleep 60", &cwd)).expect("spawn");
    let pid = process.process_id().expect("pid") as libc::pid_t;

    drop(process);

    let deadline = Instant::now() + TIMEOUT;
    loop {
        // SAFETY: signal 0 delivers nothing; it only probes whether `pid`
        // still names a process this user could signal. A `kill` failure
        // with `ESRCH` means it is gone.
        let alive = unsafe { libc::kill(pid, 0) } == 0;
        if !alive {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "child (pid {pid}) was still alive {TIMEOUT:?} after its PtyProcess was dropped"
        );
        std::thread::sleep(POLL);
    }
}

/// Linux field-3-of-`/proc/<pid>/stat` state of `pid`, or `None` if the
/// kernel has no such process any more (which is not a zombie).
#[cfg(target_os = "linux")]
fn is_zombie(pid: u32) -> Option<bool> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    // Format is "pid (comm) state ...". `comm` can itself contain spaces or
    // parentheses, so split on the *last* ')' rather than splitting naively
    // on whitespace.
    let after_comm = stat.rsplit_once(')').expect("stat has a comm field").1;
    Some(after_comm.trim_start().starts_with('Z'))
}

/// Regression test: `PtyProcess` had no `Drop`, so a child that had already
/// exited but was never `wait`ed stayed a zombie forever — five spawn+drop
/// cycles during the original investigation left five permanent zombies.
#[cfg(target_os = "linux")]
#[test]
fn dropping_reaps_a_child_that_already_exited() {
    let cwd = std::env::temp_dir();
    let (process, _output) = PtyProcess::spawn(shell_command("exit 0", &cwd)).expect("spawn");
    let pid = process.process_id().expect("pid");

    // Give the child time to actually exit and become a zombie, without ever
    // polling it through `PtyProcess` — that would reap it itself via
    // `waitpid` and defeat the point of the test, which is that `Drop` has
    // to do the reaping.
    std::thread::sleep(Duration::from_secs(1));

    drop(process);

    match is_zombie(pid) {
        Some(true) => panic!("pid {pid} is still a zombie after PtyProcess was dropped"),
        Some(false) => panic!(
            "pid {pid} exists and is not a zombie; it was almost certainly recycled \
             for an unrelated process, which means the reap did not happen"
        ),
        // Reaped: the pid no longer names any process, zombie or otherwise.
        None => {}
    }
}

#[test]
fn the_child_starts_in_the_requested_working_directory() {
    let dir = tempfile::tempdir().expect("tempdir");
    let canonical = std::fs::canonicalize(dir.path()).expect("canonicalize");
    let script = if cfg!(windows) { "cd" } else { "pwd" };

    let (mut process, output) =
        PtyProcess::spawn(shell_command(script, &canonical)).expect("spawn");
    let collector = Collector::start(output);

    let expected = canonical
        .file_name()
        .expect("dir name")
        .to_string_lossy()
        .into_owned();
    collector.expect(&expected);
    wait_for_exit(&mut process);
}
