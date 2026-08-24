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
fn shell_command(script: &str, cwd: &std::path::Path) -> TerminalCommand {
    if cfg!(windows) {
        let comspec = std::env::var("COMSPEC").unwrap_or_else(|_| "cmd.exe".to_owned());
        TerminalCommand::new(comspec, cwd).arg("/C").arg(script)
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
        "set /p line= & echo got:%line%"
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
