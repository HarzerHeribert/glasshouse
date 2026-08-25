//! Platform PTY smoke tests.
//!
//! These start a real interactive child process through the Glasshouse PTY
//! abstraction and check the four things every harness session depends on:
//! output streaming, keyboard input, window resize, and exit detection. They
//! are written to run unchanged on macOS, Linux, and native Windows so CI
//! proves the abstraction on each platform rather than only on the developer's.
//!
//! # Answering the terminal's own questions
//!
//! Windows' ConPTY asks the other end of the pseudo-console "where is the
//! cursor?" (`ESC[6n`, a Device Status Report query) as part of its startup
//! handshake, and nothing proceeds until something replies with
//! `ESC[<row>;<col>R`. A real terminal emulator answers this as a matter of
//! course; a test harness that only accumulates bytes does not, so the
//! console host stalls and every child spawned through ConPTY hangs before
//! producing a single byte of output. See the constraint recorded on
//! [`glasshouse::pty`]'s module doc, which is the product-level version of
//! this same fact for the terminal-emulation layer that will eventually sit
//! downstream of this PTY code.
//!
//! [`Session`] below is this harness acting like a real terminal: it watches
//! its own output for that query and answers it, on every platform (a
//! harness TUI can issue the same query on Unix, and there is no reason for
//! this test double to behave differently there), so these tests exercise
//! input, output, resize, and exit the same way on Windows as everywhere
//! else instead of merely not hanging.

use std::io::Read;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use glasshouse::Project;
use glasshouse::launch::HarnessLaunch;
use glasshouse::platform::{exec, paths};
use glasshouse::pty::{ProcessSignal, PtyOutput, PtyProcess, TerminalCommand, TerminalSize};
use glasshouse::session::{
    LiveSession, RuntimeError, Scrollback, SessionId, SessionPresentation, SessionRuntime,
};

/// Upper bound for any single wait in these tests. Generous enough for a loaded
/// CI runner, short enough that a genuine hang fails instead of stalling.
const TIMEOUT: Duration = Duration::from_secs(20);
const POLL: Duration = Duration::from_millis(25);

/// How long [`Session::spawn`] waits for a freshly spawned child's startup
/// Device Status Report query before giving up on one arriving.
///
/// This exists for the one test that deliberately polls nothing before acting
/// on the process --
/// `signalling_an_unpolled_but_exited_process_is_reported_rather_than_misdirected`,
/// which must not poll, because polling is exactly what it is proving `signal`
/// does for itself. Without an answered handshake a ConPTY child never gets
/// past startup, so it would never reach the `exit 0` that test depends on.
///
/// The wait ends as soon as a query has been answered, so a healthy runner
/// pays almost nothing and a loaded one is not cut off early. Only Windows
/// performs this handshake at all; every other platform skips the wait
/// entirely rather than burning it on every spawn in this file waiting for
/// something that is never coming.
#[cfg(windows)]
const SETTLE: Duration = Duration::from_secs(5);
#[cfg(not(windows))]
const SETTLE: Duration = Duration::ZERO;

/// The ANSI Device Status Report query for cursor position. Windows' ConPTY
/// sends this, unprompted, as part of bringing up a pseudo-console; see the
/// module doc above and on [`glasshouse::pty`].
const DSR_CURSOR_POSITION_QUERY: &[u8] = b"\x1b[6n";

/// Reply to [`DSR_CURSOR_POSITION_QUERY`], in the `ESC[<row>;<col>R` format a
/// terminal is expected to send back. Row 1, column 1 is an arbitrary but
/// valid position -- nothing in these tests inspects where the cursor is
/// reported to be, only that *a* well-formed reply arrives so the handshake
/// completes.
const DSR_CURSOR_POSITION_REPLY: &[u8] = b"\x1b[1;1R";

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
        // `/D` ignores AutoRun registry entries, `/Q` turns command echoing
        // off: both keep unrelated output out of the child's stream.
        TerminalCommand::new(comspec, cwd)
            .arg("/D")
            .arg("/Q")
            .arg("/V:ON")
            .arg("/C")
            .arg(script)
    } else {
        TerminalCommand::new("/bin/sh", cwd).arg("-c").arg(script)
    }
}

/// Remove ANSI escape sequences from `text`, leaving the child's real output.
///
/// Two shapes matter here, and leaving either one in poisons a match:
///
/// - **CSI** — `ESC [`, then parameter/intermediate bytes, then one final byte
///   in `0x40..=0x7e`. ConPTY interleaves these constantly, its startup
///   `ESC[6n` query among them.
/// - **OSC** — `ESC ]`, then a payload, terminated by `BEL` or by ST
///   (`ESC \`). `cmd.exe` opens by setting the window title this way, and on
///   `windows-latest` that sequence arrives glued directly to the front of the
///   path the child prints:
///
///   ```text
///   ESC]0;C:\Windows\system32\cmd.exeBELC:\Users\...\proj
///   ```
///
///   Stripping only CSI leaves `ESC]0;...cmd.exeBEL` welded to the real path,
///   so the line names no directory that exists and the match fails — which
///   looked exactly like a launch that had started in the wrong place, and
///   was not.
fn strip_terminal_sequences(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\x1b' {
            out.push(c);
            continue;
        }
        match chars.peek() {
            Some('[') => {
                chars.next();
                // Drop bytes up to and including the sequence's final byte. A
                // truncated sequence at end-of-input is dropped with the rest.
                let mut terminated = false;
                for b in chars.by_ref() {
                    if ('\u{40}'..='\u{7e}').contains(&b) {
                        terminated = true;
                        break;
                    }
                }
                if !terminated {
                    break;
                }
            }
            Some(']') => {
                chars.next();
                let mut terminated = false;
                while let Some(b) = chars.next() {
                    if b == '\u{7}' {
                        terminated = true;
                        break;
                    }
                    // ST, the other legal terminator, is two characters.
                    if b == '\x1b' {
                        if chars.peek() == Some(&'\\') {
                            chars.next();
                            terminated = true;
                        }
                        break;
                    }
                }
                if !terminated {
                    break;
                }
            }
            // A lone ESC introducing neither is left alone rather than
            // guessed at.
            _ => out.push(c),
        }
    }
    out
}

/// Streaming counter for occurrences of a fixed byte pattern across a
/// sequence of chunks delivered in whatever sizes the reader happens to hand
/// over.
///
/// PTY reads split wherever the kernel (or ConPTY) feels like it, so a scan
/// that only looks inside one `read()` call's buffer can miss a match whose
/// bytes landed on either side of a chunk boundary. This keeps just enough
/// state between calls to `feed` -- how many leading bytes of `pattern` are
/// currently matched, per the standard KMP prefix-function technique -- to
/// detect a match that straddles a boundary without rescanning or retaining
/// the bytes already consumed.
struct PatternScanner {
    pattern: &'static [u8],
    /// `failure[i]` is the length of the longest proper prefix of
    /// `pattern[..=i]` that is also a suffix of it -- the standard KMP
    /// failure function. Used to resume matching after a mismatch without
    /// backtracking over bytes already consumed.
    failure: Vec<usize>,
    /// How many leading bytes of `pattern` match the tail of everything fed
    /// so far.
    matched: usize,
}

impl PatternScanner {
    fn new(pattern: &'static [u8]) -> Self {
        debug_assert!(!pattern.is_empty());
        let mut failure = vec![0usize; pattern.len()];
        let mut k = 0;
        for i in 1..pattern.len() {
            while k > 0 && pattern[k] != pattern[i] {
                k = failure[k - 1];
            }
            if pattern[k] == pattern[i] {
                k += 1;
            }
            failure[i] = k;
        }
        Self {
            pattern,
            failure,
            matched: 0,
        }
    }

    /// Feed the next chunk of bytes, returning how many additional complete
    /// matches of `pattern` were found (including more than one, if `chunk`
    /// happens to contain several back to back).
    fn feed(&mut self, chunk: &[u8]) -> usize {
        let mut found = 0;
        for &byte in chunk {
            while self.matched > 0 && self.pattern[self.matched] != byte {
                self.matched = self.failure[self.matched - 1];
            }
            if self.pattern[self.matched] == byte {
                self.matched += 1;
            }
            if self.matched == self.pattern.len() {
                found += 1;
                self.matched = self.failure[self.matched - 1];
            }
        }
        found
    }
}

/// Accumulates PTY output on a background thread, the way the session runtime
/// does, so tests can look at partial output while the child is still
/// running -- and counts how many times the child's startup handshake query
/// has appeared in that stream, so [`Session`] knows how many replies it
/// still owes.
struct Collector {
    buffer: Arc<Mutex<Vec<u8>>>,
    dsr_seen: Arc<AtomicUsize>,
}

impl Collector {
    fn start(mut output: PtyOutput) -> Self {
        let buffer = Arc::new(Mutex::new(Vec::new()));
        let dsr_seen = Arc::new(AtomicUsize::new(0));
        let thread_buffer = Arc::clone(&buffer);
        let thread_dsr_seen = Arc::clone(&dsr_seen);

        std::thread::spawn(move || {
            let mut scanner = PatternScanner::new(DSR_CURSOR_POSITION_QUERY);
            let mut chunk = [0u8; 4096];
            loop {
                match output.read(&mut chunk) {
                    // EOF, or the pty was torn down after the child exited.
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        let bytes = &chunk[..n];
                        thread_buffer.lock().unwrap().extend_from_slice(bytes);
                        let found = scanner.feed(bytes);
                        if found > 0 {
                            thread_dsr_seen.fetch_add(found, Ordering::SeqCst);
                        }
                    }
                }
            }
        });

        Self { buffer, dsr_seen }
    }

    fn text(&self) -> String {
        String::from_utf8_lossy(&self.buffer.lock().unwrap()).into_owned()
    }
}

/// Owns both halves of a PTY-backed child -- the writable [`PtyProcess`] and
/// the [`Collector`] draining its output -- so the waiting loops below can do
/// what a real terminal does: see the child's startup handshake query in the
/// output stream and write the reply back, instead of only ever accumulating
/// bytes no one answers.
struct Session {
    process: PtyProcess,
    collector: Collector,
    /// How many [`DSR_CURSOR_POSITION_REPLY`] replies have already been
    /// written back, compared against `collector.dsr_seen` on every poll so
    /// each query is answered exactly once.
    answered: usize,
}

impl Session {
    fn spawn(command: TerminalCommand) -> Self {
        let (process, output) = PtyProcess::spawn(command).expect("spawn");
        Self::from_parts(process, output)
    }

    /// Start a harness through the sanctioned [`HarnessLaunch`] seam — the
    /// same `PtyProcess` machinery, entered the way production code enters
    /// it.
    fn spawn_harness(launch: &HarnessLaunch<'_>) -> Self {
        let (process, output) = launch.spawn().expect("harness spawn");
        Self::from_parts(process, output)
    }

    fn from_parts(process: PtyProcess, output: PtyOutput) -> Self {
        let collector = Collector::start(output);
        let mut session = Self {
            process,
            collector,
            answered: 0,
        };
        // Wait for the startup handshake, but only until it has been
        // answered -- see `SETTLE`. A caller that goes on to poll output or
        // wait for exit would have answered it anyway; this is for the one
        // that does neither.
        let deadline = Instant::now() + SETTLE;
        loop {
            session.answer_pending_queries();
            if session.answered > 0 || Instant::now() >= deadline {
                break;
            }
            std::thread::sleep(POLL);
        }
        session
    }

    /// Reply to any Device Status Report queries seen in the output so far
    /// that have not already been answered -- exactly what a real terminal
    /// emulator does the moment it sees one go by.
    fn answer_pending_queries(&mut self) {
        let seen = self.collector.dsr_seen.load(Ordering::SeqCst);
        while self.answered < seen {
            // Best effort: if the write fails the child is presumably gone
            // already, which the next `try_wait` will report on its own.
            let _ = self.process.write_input(DSR_CURSOR_POSITION_REPLY);
            self.answered += 1;
        }
    }

    fn send(&mut self, text: &str) {
        self.process.send_text(text).expect("send_text");
    }

    /// Wait until `needle` appears in the output, answering any handshake
    /// queries along the way, or fail with what was seen.
    fn expect(&mut self, needle: &str) {
        let deadline = Instant::now() + TIMEOUT;
        while Instant::now() < deadline {
            self.answer_pending_queries();
            if self.collector.text().contains(needle) {
                return;
            }
            std::thread::sleep(POLL);
        }
        panic!(
            "timed out waiting for {needle:?} in pty output.\n--- output ---\n{}\n--- end ---",
            self.collector.text()
        );
    }

    /// Wait for the process to exit, answering any handshake queries along
    /// the way, failing the test rather than hanging.
    fn wait_for_exit(&mut self) -> glasshouse::pty::ExitStatus {
        let deadline = Instant::now() + TIMEOUT;
        while Instant::now() < deadline {
            self.answer_pending_queries();
            if let Some(status) = self.process.try_wait().expect("try_wait") {
                return status;
            }
            std::thread::sleep(POLL);
        }
        let _ = self.process.signal(ProcessSignal::Kill);
        panic!("child process did not exit within {TIMEOUT:?}");
    }

    /// Direct access to the underlying process, for resize/interrupt/signal
    /// calls that are not about waiting on anything.
    fn process(&mut self) -> &mut PtyProcess {
        &mut self.process
    }

    /// The output captured so far. Cross-platform by design: any test that
    /// needs to inspect what the child actually printed uses this, not just
    /// Unix-only ones.
    fn output(&self) -> String {
        self.collector.text()
    }
}

#[test]
fn streams_output_and_reports_a_successful_exit() {
    let cwd = std::env::temp_dir();
    let mut session = Session::spawn(shell_command("echo glasshouse-ok", &cwd));

    session.expect("glasshouse-ok");
    let status = session.wait_for_exit();
    assert!(status.success(), "unexpected status: {status}");
    assert_eq!(status.code(), 0);
}

#[test]
fn reports_a_failing_exit_code() {
    let cwd = std::env::temp_dir();
    let mut session = Session::spawn(shell_command("exit 7", &cwd));

    let status = session.wait_for_exit();
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
    let mut session = Session::spawn(shell_command(script, &cwd));

    session.send("hello\r\n");
    session.expect("got:hello");
    session.wait_for_exit();
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
    let mut session = Session::spawn(shell_command(script, &cwd));

    assert!(
        session.process().try_wait().expect("try_wait").is_none(),
        "a running silent process must not look finished"
    );
    let status = session.wait_for_exit();
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
    let mut session = Session::spawn(shell_command(script, &cwd).size(TerminalSize::new(24, 80)));

    assert_eq!(
        session.process().os_size().expect("os_size"),
        TerminalSize::new(24, 80)
    );

    let resized = TerminalSize::new(40, 120);
    session.process().resize(resized).expect("resize");
    assert_eq!(session.process().size(), resized);
    assert_eq!(session.process().os_size().expect("os_size"), resized);

    session.process().signal(ProcessSignal::Kill).expect("kill");
    session.wait_for_exit();
}

#[cfg(unix)]
#[test]
fn a_resize_is_visible_to_the_child_process() {
    let cwd = std::env::temp_dir();
    // The child reports its window size only after we resize and release it, so
    // the value it prints proves the kernel told the child, not just that
    // Glasshouse recorded a number.
    let mut session =
        Session::spawn(shell_command("read x; stty size", &cwd).size(TerminalSize::new(24, 80)));

    session
        .process()
        .resize(TerminalSize::new(40, 120))
        .expect("resize");
    session.send("\n");

    session.expect("40 120");
    session.wait_for_exit();
}

// Windows is deliberately not covered here. portable-pty creates the
// pseudoconsole with `PSEUDOCONSOLE_WIN32_INPUT_MODE`, under which conhost is
// documented to translate an incoming `0x03` byte into a Ctrl+C key event --
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
    let mut session = Session::spawn(shell_command(
        "trap 'echo caught-interrupt; exit 0' INT; echo ready; while true; do sleep 0.1; done",
        &cwd,
    ));

    session.expect("ready");
    session.process().interrupt().expect("interrupt");

    session.expect("caught-interrupt");
    session.wait_for_exit();
}

#[test]
fn terminating_stops_a_long_running_process() {
    let cwd = std::env::temp_dir();
    let script = if cfg!(windows) {
        "ping -n 60 127.0.0.1 > nul"
    } else {
        "sleep 60"
    };
    let mut session = Session::spawn(shell_command(script, &cwd));

    assert!(session.process().try_wait().expect("try_wait").is_none());
    session
        .process()
        .signal(ProcessSignal::Terminate)
        .expect("terminate");

    let status = session.wait_for_exit();
    assert!(!status.success(), "terminated process reported: {status}");
}

#[test]
fn signalling_an_exited_process_is_reported_rather_than_misdirected() {
    let cwd = std::env::temp_dir();
    let mut session = Session::spawn(shell_command("exit 0", &cwd));
    session.wait_for_exit();

    // The pid may already have been recycled by the operating system, so this
    // must never turn into a signal aimed at an unrelated process.
    let err = session.process().signal(ProcessSignal::Kill).unwrap_err();
    assert!(matches!(err, glasshouse::pty::SignalError::AlreadyExited));
}

/// Regression test: `signal` used to trust a stale `exit_status` cache that
/// only a previous `wait`/`try_wait` call would have populated. Nothing
/// forced a poll of its own, so a process that exited without anyone ever
/// having polled it still looked "not yet known to be exited" and `signal`
/// would proceed to actually deliver a signal -- to a pid the OS is free to
/// have already recycled for an unrelated process. This test deliberately
/// never calls `try_wait`/`wait` before signalling, unlike
/// `signalling_an_exited_process_is_reported_rather_than_misdirected` above,
/// which does (via `wait_for_exit`) and so would not have caught this bug.
///
/// This also deliberately never calls `expect`/`wait_for_exit`, so the only
/// thing standing between this test and ConPTY's startup handshake stalling
/// the child forever on Windows is the bounded settle window in
/// `Session::spawn` -- see `SETTLE`'s doc comment.
#[test]
fn signalling_an_unpolled_but_exited_process_is_reported_rather_than_misdirected() {
    let cwd = std::env::temp_dir();
    let mut session = Session::spawn(shell_command("exit 0", &cwd));

    // Give the child time to actually exit without ever polling it through
    // `PtyProcess` -- that would populate `exit_status` and let even the old,
    // buggy check pass by accident. `exit 0` (and its cmd.exe equivalent)
    // finishes near-instantly, so a short fixed sleep comfortably covers it
    // on any CI runner while staying bounded.
    std::thread::sleep(Duration::from_secs(1));

    let err = session.process().signal(ProcessSignal::Kill).unwrap_err();
    assert!(matches!(err, glasshouse::pty::SignalError::AlreadyExited));
}

/// Regression test for the defect that mattered most: `signal` used to target
/// the terminal's *foreground* process group (`tcgetpgrp`) rather than the
/// child's own group. A harness that uses job control hands the terminal to a
/// descendant, so `Terminate` reported success while the session leader -- the
/// harness itself -- kept running. For a control plane whose only way to shut a
/// session down is this call, "reported success, nothing died" is the worst
/// possible failure.
///
/// `set -m` turns job control on, which is what puts the child `sleep` in a
/// different process group from the shell.
#[cfg(unix)]
#[test]
fn terminate_reaches_the_session_leader_under_job_control() {
    let cwd = std::env::temp_dir();
    let mut session = Session::spawn(
        TerminalCommand::new("/bin/sh", &cwd)
            .arg("-c")
            .arg("set -m; sleep 60; sleep 60"),
    );

    // Let the shell reach the point where the inner `sleep` owns the terminal.
    let deadline = Instant::now() + Duration::from_secs(2);
    while Instant::now() < deadline {
        std::thread::sleep(POLL);
    }
    assert!(
        session.process().try_wait().expect("try_wait").is_none(),
        "the shell should still be running"
    );

    session
        .process()
        .signal(ProcessSignal::Terminate)
        .expect("terminate");

    let status = session.wait_for_exit();
    assert!(
        !status.success(),
        "the shell survived Terminate and reported: {status}"
    );
}

/// Regression test: `PtyProcess` had no `Drop`, so a child that was still
/// running when its `PtyProcess` was dropped simply kept running,
/// unreachable, forever.
///
/// Deliberately built from `PtyProcess::spawn` directly rather than
/// `Session`: this test cares only about `Drop`'s effect on the process, and
/// keeping a `Collector` thread reading the discarded `PtyOutput` for the
/// rest of the test would be an unrelated, unused reader racing the drop
/// this test exists to check.
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
/// exited but was never `wait`ed stayed a zombie forever -- five spawn+drop
/// cycles during the original investigation left five permanent zombies.
///
/// Deliberately built from `PtyProcess::spawn` directly rather than
/// `Session`, for the same reason as `dropping_a_running_process_kills_it`
/// above: no reader should be racing the drop this test exists to check.
#[cfg(target_os = "linux")]
#[test]
fn dropping_reaps_a_child_that_already_exited() {
    let cwd = std::env::temp_dir();
    let (process, _output) = PtyProcess::spawn(shell_command("exit 0", &cwd)).expect("spawn");
    let pid = process.process_id().expect("pid");

    // Give the child time to actually exit and become a zombie, without ever
    // polling it through `PtyProcess` -- that would reap it itself via
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

    let mut session = Session::spawn(shell_command(script, &canonical));

    let expected = canonical
        .file_name()
        .expect("dir name")
        .to_string_lossy()
        .into_owned();
    session.expect(&expected);
    session.wait_for_exit();
}

/// Write a fake installed harness into `bin_dir` and return its path.
///
/// Windows: a `.cmd` script, so the resolver classifies it as
/// `WindowsScript` and the launch really goes through `cmd.exe /D /C`.
#[cfg(windows)]
fn install_fake_harness(bin_dir: &std::path::Path) -> std::path::PathBuf {
    let path = bin_dir.join("fake-harness.cmd");
    std::fs::write(&path, "@echo off\r\ncd\r\n").expect("write fake harness");
    path
}

/// Unix: a plain executable shell script printing its physical cwd.
#[cfg(unix)]
fn install_fake_harness(bin_dir: &std::path::Path) -> std::path::PathBuf {
    use std::os::unix::fs::PermissionsExt;

    let path = bin_dir.join("fake-harness");
    std::fs::write(&path, "#!/bin/sh\nexec /bin/pwd -P\n").expect("write fake harness");
    let mut perms = std::fs::metadata(&path).unwrap().permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&path, perms).unwrap();
    path
}

/// The harness launch seam end to end, with a *fake installed harness*: a
/// real executable file on disk is resolved through
/// [`glasshouse::platform::exec::resolve_explicit`], launched through
/// [`HarnessLaunch`] (the sanctioned production route — no explicit cwd or
/// program appears anywhere in this test), and the child's own report of its
/// working directory must denote exactly the project root — compared with
/// `platform::paths::same_file`, i.e. by asking the filesystem, not by weak
/// basename or string matching.
///
/// On Windows the fake harness is a `.cmd` script, so this exercises
/// `ResolvedExecutable::spawn_command`'s `WindowsScript` branch (`cmd.exe
/// /D /C <script>`) for real. The project and the fake install live in a
/// tempdir while the Glasshouse process stays wherever the test runner put
/// it — no global cwd mutation — and a sanity check asserts those two
/// locations really are different.
#[test]
fn a_fake_installed_harness_launches_inside_the_discovered_project_root() {
    let tmp = tempfile::tempdir().expect("tempdir");

    // The project and the fake harness install are separate directories,
    // both distinct from the Glasshouse process's own cwd.
    let project_dir = tmp.path().join("proj");
    std::fs::create_dir_all(project_dir.join(".git")).expect("create project");
    let bin_dir = tmp.path().join("bin");
    std::fs::create_dir_all(&bin_dir).expect("create bin dir");

    // Install the fake harness. On Windows a `.cmd` script is what makes the
    // resolver classify it as WindowsScript; elsewhere a plain executable
    // shell script that prints its physical working directory. Split into
    // cfg-attributed helpers so each platform compiles only its own branch
    // (`cfg!` would still type-check both).
    let harness_path = install_fake_harness(&bin_dir);

    let resolved = exec::resolve_explicit(&harness_path).expect("resolve fake harness");
    let project = Project::discover(&project_dir, None, false).expect("discover project");

    // Windows canonicalization returns a verbatim identity path. This makes
    // the CI smoke prove that HarnessLaunch did not merely receive an already
    // display-safe path: the child can start only after display_root strips
    // this prefix at the process boundary.
    #[cfg(windows)]
    assert!(
        project.root().to_string_lossy().starts_with(r"\\?\"),
        "expected a verbatim canonical project root on Windows: {}",
        project.root().display()
    );

    // Sanity: the project root is genuinely elsewhere from the process cwd,
    // so only a correctly derived child cwd can match below.
    let process_cwd = std::env::current_dir().expect("process cwd");
    assert!(
        !paths::same_file(&process_cwd, project.root()),
        "test setup is degenerate: the process already runs in the project root"
    );

    let launch = HarnessLaunch::new(resolved, &project);
    let mut session = Session::spawn_harness(&launch);

    // Poll for the reported path *before* waiting for exit, the way
    // `Session::expect` polls: reading the collector only once after exit can
    // race its background thread draining the final PTY bytes (and on Windows
    // the PTY may not even be closed by then).
    let deadline = Instant::now() + TIMEOUT;
    let mut reported = None;
    while Instant::now() < deadline {
        session.answer_pending_queries();
        // Parse the *stripped* stream: ConPTY may emit CSI sequences (its
        // `ESC[6n` startup query among them) directly adjacent to the path
        // the child printed, and those bytes would poison a same_file match.
        let clean = strip_terminal_sequences(&session.output());
        reported = clean
            .lines()
            .map(str::trim)
            .find(|line| {
                !line.is_empty() && paths::same_file(std::path::Path::new(line), project.root())
            })
            .map(str::to_owned);
        if reported.is_some() {
            break;
        }
        std::thread::sleep(POLL);
    }
    let Some(reported) = reported else {
        panic!(
            "the fake harness never reported a directory naming the project root.\n\
             --- raw output ---\n{}\n--- end ---",
            session.output()
        );
    };

    // The path has been observed; now the process is allowed to finish.
    let status = session.wait_for_exit();
    assert!(status.success(), "fake harness reported: {status}");

    // Belt and braces: the line that matched must still resolve to the same
    // location from this process right now.
    assert!(
        paths::same_file(std::path::Path::new(&reported), project.root()),
        "reported cwd `{reported}` does not denote `{}`",
        project.root().display()
    );
}

#[test]
fn a_direct_executable_launches_through_the_harness_seam() {
    // The `WindowsScript` branch is covered by the fake `.cmd` harness above;
    // this covers the other one. It matters for the same reason that branch
    // needed fixing: resolving an executable canonicalizes it, and on Windows
    // canonical means verbatim (`\\?\C:\...`). `cmd.exe` rejects that form
    // outright, and nothing here had ever confirmed that `CreateProcess`
    // accepts it — so a native `.exe` harness rested on an assumption no test
    // had checked.
    //
    // The Glasshouse binary is itself a real Direct executable with a real
    // `--version`, so it serves as the harness: no fixture to compile, and on
    // Windows its resolved path is genuinely verbatim.
    let tmp = tempfile::tempdir().expect("tempdir");
    let project_dir = tmp.path().join("proj");
    std::fs::create_dir_all(project_dir.join(".git")).expect("create project");
    let project = Project::discover(&project_dir, None, false).expect("discover project");

    let exe = exec::resolve_explicit(std::path::Path::new(env!("CARGO_BIN_EXE_glasshouse")))
        .expect("resolve the glasshouse binary");
    assert_eq!(exe.kind(), exec::LaunchKind::Direct);

    let launch = HarnessLaunch::new(exe, &project).arg("--version");
    let mut session = Session::spawn_harness(&launch);

    session.expect(glasshouse::VERSION);
    let status = session.wait_for_exit();
    assert!(status.success(), "unexpected status: {status}");
}

#[test]
fn stripper_rescues_a_path_welded_to_a_cmd_title_sequence() {
    // Byte for byte what `windows-latest` actually produced, including the
    // CSI preamble and the OSC window-title sequence cmd.exe emits on
    // startup, with the child's real output glued straight onto the end.
    let raw = "\x1b[6n\x1b[?9001h\x1b[?1004h\x1b[m\x1b]0;C:\\Windows\\system32\\cmd.exe\x07\
               \x1b[?25hC:\\Users\\runneradmin\\AppData\\Local\\Temp\\.tmp1\\proj\r\n";
    assert_eq!(
        strip_terminal_sequences(raw),
        "C:\\Users\\runneradmin\\AppData\\Local\\Temp\\.tmp1\\proj\r\n"
    );

    // The other legal OSC terminator is ST (`ESC \`), not BEL.
    assert_eq!(strip_terminal_sequences("\x1b]0;title\x1b\\after"), "after");
    // An unterminated OSC swallows the rest rather than emitting garbage.
    assert_eq!(strip_terminal_sequences("keep\x1b]0;never-ends"), "keep");
}

#[test]
fn stripper_rescues_a_path_adjacent_to_a_conpty_query() {
    // Exactly the poisoning case: ConPTY's cursor-position query glued onto
    // the front of the child's own printed path must come out as that path.
    assert_eq!(
        strip_terminal_sequences("\x1b[6nC:\\proj\r\n"),
        "C:\\proj\r\n"
    );
    assert_eq!(
        strip_terminal_sequences("\x1b[6n/var/folders/x/proj\n"),
        "/var/folders/x/proj\n"
    );
}

#[test]
fn stripper_removes_ordinary_sequences_and_preserves_text() {
    // Colour, cursor movement, and other ordinary CSI sequences disappear;
    // the surrounding text is untouched.
    assert_eq!(
        strip_terminal_sequences("before\x1b[1;32mbright\x1b[0mafter\x1b[2J\ndone"),
        "beforebrightafter\ndone"
    );
    // Text with no escape sequences at all passes through unchanged...
    assert_eq!(strip_terminal_sequences("plain text\n"), "plain text\n");
    // ...as does a lone ESC that does not introduce a CSI sequence.
    assert_eq!(strip_terminal_sequences("a\x1bb"), "a\x1bb");
}

/// Proves the responder itself works, not just that these tests stopped
/// timing out: a shell child raises the exact query ConPTY raises, reads
/// back exactly the bytes `Session` wrote, and echoes them into its own
/// output for the collector to see -- so this fails if `Session` ever stops
/// detecting or answering the query, even though no platform this suite runs
/// on has a real ConPTY to raise one itself.
///
/// `stty raw -echo` matters: a pty defaults to canonical (line-buffered)
/// mode, which would hold the reply bytes in the kernel's line discipline
/// until a newline arrived and would also echo Glasshouse's write back into
/// the child's own read -- neither of which has anything to do with what is
/// under test here. Real ConPTY sessions do not have this wrinkle (Windows
/// console input handling is not Unix termios), so this is purely a
/// property of the Unix surrogate, not something `Session` itself needs to
/// account for.
#[cfg(unix)]
#[test]
fn the_responder_answers_a_query_the_child_itself_raises() {
    let cwd = std::env::temp_dir();
    let script = "stty raw -echo; printf '\\033[6n'; od -An -v -tx1 -N 6 | tr -d ' \\n'";
    let mut session = Session::spawn(shell_command(script, &cwd));

    // The 6 raw bytes of `DSR_CURSOR_POSITION_REPLY` (1b 5b 31 3b 31 52),
    // hex-dumped by the child with no separators.
    session.expect("1b5b313b3152");
    session.wait_for_exit();
    assert!(
        session.output().contains("1b5b313b3152"),
        "child did not see the reply bytes: {}",
        session.output()
    );
}

#[test]
fn pattern_scanner_finds_a_query_within_one_chunk() {
    let mut scanner = PatternScanner::new(DSR_CURSOR_POSITION_QUERY);
    assert_eq!(scanner.feed(b"hello \x1b[6n world"), 1);
}

/// The case that actually matters: PTY reads split wherever they like, so a
/// query can land with some bytes in one `read()` and the rest in the next.
#[test]
fn pattern_scanner_finds_a_query_split_across_chunks() {
    let mut scanner = PatternScanner::new(DSR_CURSOR_POSITION_QUERY);
    assert_eq!(scanner.feed(b"leading \x1b["), 0);
    assert_eq!(scanner.feed(b"6n trailing"), 1);
}

#[test]
fn pattern_scanner_finds_a_query_split_byte_by_byte() {
    let mut scanner = PatternScanner::new(DSR_CURSOR_POSITION_QUERY);
    let mut total = 0;
    for &byte in DSR_CURSOR_POSITION_QUERY {
        total += scanner.feed(&[byte]);
    }
    assert_eq!(total, 1);
}

#[test]
fn pattern_scanner_counts_multiple_queries_in_one_chunk() {
    let mut scanner = PatternScanner::new(DSR_CURSOR_POSITION_QUERY);
    assert_eq!(scanner.feed(b"\x1b[6n\x1b[6n\x1b[6n"), 3);
}

#[test]
fn pattern_scanner_does_not_false_positive_on_a_near_miss() {
    let mut scanner = PatternScanner::new(DSR_CURSOR_POSITION_QUERY);
    // `ESC [ 6 0 n` is a different report (window size) that merely shares a
    // prefix with the cursor-position query; it must not count as a match.
    assert_eq!(scanner.feed(b"\x1b[60n"), 0);
}

#[test]
fn pattern_scanner_recovers_after_a_partial_match_that_fails() {
    let mut scanner = PatternScanner::new(DSR_CURSOR_POSITION_QUERY);
    // Starts to match (`ESC [ 6`), then diverges, then a real query follows
    // right after -- the scanner must not get stuck thinking it is still
    // mid-match from the failed attempt.
    assert_eq!(scanner.feed(b"\x1b[6X\x1b[6n"), 1);
}

/// Write a fake harness that prints a fixed marker, its physical working
/// directory, and then exits with `exit_code`.
///
/// Windows gets a `.cmd` script so the launch really goes through
/// `cmd.exe /D /C`, exactly as a real npm-installed harness shim would.
#[cfg(windows)]
fn install_marker_harness(
    bin_dir: &std::path::Path,
    name: &str,
    marker: &str,
    exit_code: u8,
) -> std::path::PathBuf {
    let path = bin_dir.join(format!("{name}.cmd"));
    std::fs::write(
        &path,
        format!("@echo off\r\necho {marker}\r\ncd\r\nexit /b {exit_code}\r\n"),
    )
    .expect("write fake harness");
    path
}

#[cfg(unix)]
fn install_marker_harness(
    bin_dir: &std::path::Path,
    name: &str,
    marker: &str,
    exit_code: u8,
) -> std::path::PathBuf {
    use std::os::unix::fs::PermissionsExt;

    let path = bin_dir.join(name);
    std::fs::write(
        &path,
        format!("#!/bin/sh\necho {marker}\n/bin/pwd -P\nexit {exit_code}\n"),
    )
    .expect("write fake harness");
    let mut perms = std::fs::metadata(&path).unwrap().permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&path, perms).unwrap();
    path
}

/// The whole production consumer, end to end: the real `glasshouse` binary,
/// running `glasshouse launch`, inside a real pseudo-terminal.
///
/// This is the test that makes the Phase 1 working-directory claim about
/// *production* rather than about a mechanism a test drove by hand. Nothing
/// here constructs a `HarnessLaunch`, a `TerminalCommand`, or a working
/// directory: it sets up configuration on disk, runs the shipped executable,
/// and reads what the harness itself reports.
///
/// Four separate claims are proved at once, each of which would otherwise
/// need its own scaffolding:
///
/// 1. **Project binding** — the harness's own report of its working
///    directory denotes the project root, compared with `same_file` (asking
///    the filesystem) rather than by string equality. Glasshouse itself is
///    deliberately run from a *different* directory, so inheriting a cwd
///    cannot produce a pass.
/// 2. **Project-over-user executable precedence** — two different fake
///    harnesses are installed and configured, a decoy at the user level and
///    the real one at the project level. The decoy prints an unmistakable
///    marker, so a precedence bug cannot pass silently: it fails loudly with
///    the decoy's own output as the evidence.
/// 3. **Exit propagation** — the harness exits with a distinctive code and
///    Glasshouse must exit with the same one, not with a generic success or
///    failure.
/// 4. **The terminal bridge works at all** — output reaches the terminal
///    through the attach pumps, which is the only reason claims 1 and 2 are
///    observable from out here.
#[test]
fn the_launch_command_opens_the_configured_harness_inside_the_project_root() {
    /// Exit code the fake harness ends with. Deliberately not 0 or 1, so a
    /// generic success or generic failure cannot be mistaken for propagation.
    const HARNESS_EXIT_CODE: u8 = 7;
    const DECOY_MARKER: &str = "GLASSHOUSE-DECOY-HARNESS-RAN";
    const REAL_MARKER: &str = "GLASSHOUSE-REAL-HARNESS-RAN";

    let tmp = tempfile::tempdir().expect("tempdir");
    let project_dir = tmp.path().join("proj");
    std::fs::create_dir_all(project_dir.join(".git")).expect("create project");
    let bin_dir = tmp.path().join("bin");
    std::fs::create_dir_all(&bin_dir).expect("create bin dir");
    let state_dir = tmp.path().join("state");
    let config_dir = tmp.path().join("config");
    std::fs::create_dir_all(&state_dir).expect("create state dir");
    std::fs::create_dir_all(&config_dir).expect("create config dir");

    // The decoy must never run. It is configured at the user level, which the
    // project level has to override.
    let decoy = install_marker_harness(&bin_dir, "decoy-harness", DECOY_MARKER, 0);
    let real = install_marker_harness(&bin_dir, "real-harness", REAL_MARKER, HARNESS_EXIT_CODE);

    // TOML needs its backslashes escaped, which matters only on Windows but
    // is harmless everywhere.
    let toml_path = |p: &std::path::Path| p.display().to_string().replace('\\', "\\\\");

    std::fs::write(
        config_dir.join("config.toml"),
        format!(
            "version = 1\n\n[integrations.claude-code]\nenabled = true\nexecutable = \"{}\"\n",
            toml_path(&decoy)
        ),
    )
    .expect("write user config");

    std::fs::create_dir_all(project_dir.join(".glasshouse")).expect("create project config dir");
    std::fs::write(
        project_dir.join(".glasshouse").join("config.toml"),
        format!(
            "version = 1\n\n[integrations.claude-code]\nenabled = true\nexecutable = \"{}\"\n",
            toml_path(&real)
        ),
    )
    .expect("write project config");

    // Glasshouse runs from somewhere that is emphatically not the project, so
    // only a working directory it derived from the project can match below.
    let elsewhere = std::env::temp_dir();
    assert!(
        !paths::same_file(&elsewhere, &project_dir),
        "test setup is degenerate: Glasshouse would already run in the project root"
    );

    let command = TerminalCommand::new(env!("CARGO_BIN_EXE_glasshouse"), &elsewhere)
        .arg("--scope")
        .arg(&project_dir)
        .arg("--data-dir")
        .arg(&state_dir)
        .arg("--config-dir")
        .arg(&config_dir)
        .arg("launch")
        .arg("claude-code");

    let mut session = Session::spawn(command);

    // Poll for the harness's own report while it is still running, the same
    // way the other launch smoke does: reading only after exit can race the
    // collector draining the last bytes.
    let deadline = Instant::now() + TIMEOUT;
    let mut reported = None;
    while Instant::now() < deadline {
        session.answer_pending_queries();
        let clean = strip_terminal_sequences(&session.output());
        if clean.contains(REAL_MARKER) {
            reported = clean
                .lines()
                .map(str::trim)
                .find(|line| {
                    !line.is_empty() && paths::same_file(std::path::Path::new(line), &project_dir)
                })
                .map(str::to_owned);
            if reported.is_some() {
                break;
            }
        }
        std::thread::sleep(POLL);
    }

    let output = session.output();
    assert!(
        !strip_terminal_sequences(&output).contains(DECOY_MARKER),
        "the user-level decoy executable ran, so the project level did not take \
         precedence.\n--- output ---\n{output}\n--- end ---"
    );
    let Some(reported) = reported else {
        panic!(
            "`glasshouse launch` never started a harness reporting the project root.\n\
             --- output ---\n{output}\n--- end ---"
        );
    };
    assert!(
        paths::same_file(std::path::Path::new(&reported), &project_dir),
        "reported cwd `{reported}` does not denote `{}`",
        project_dir.display()
    );

    let status = session.wait_for_exit();
    assert_eq!(
        status.code(),
        u32::from(HARNESS_EXIT_CODE),
        "glasshouse did not propagate the harness's exit code; it reported: {status}"
    );
}

/// Glasshouse's own session record, written by the real binary during a real
/// session and read back by a second process.
///
/// The store has thorough unit tests, but those construct a runtime in-process.
/// This is the part they cannot show: that `glasshouse launch` actually records
/// what it started, that the outcome of the harness reaches the record, and
/// that a later `glasshouse sessions` reads it back off disk. Without this,
/// "Glasshouse persists session metadata" would rest on machinery no shipped
/// command exercises.
#[test]
fn launching_a_harness_records_a_session_that_a_later_command_reads_back() {
    const OK_MARKER: &str = "GLASSHOUSE-SESSION-OK";
    const BAD_MARKER: &str = "GLASSHOUSE-SESSION-BAD";
    /// Neither 0 nor 1, so a generic failure cannot be mistaken for this one.
    const FAILING_EXIT: u8 = 9;

    let tmp = tempfile::tempdir().expect("tempdir");
    let project_dir = tmp.path().join("proj");
    std::fs::create_dir_all(project_dir.join(".git")).expect("create project");
    let bin_dir = tmp.path().join("bin");
    std::fs::create_dir_all(&bin_dir).expect("create bin dir");
    let state_dir = tmp.path().join("state");
    let config_dir = tmp.path().join("config");
    std::fs::create_dir_all(&state_dir).expect("create state dir");
    std::fs::create_dir_all(&config_dir).expect("create config dir");

    let good = install_marker_harness(&bin_dir, "good-harness", OK_MARKER, 0);
    let bad = install_marker_harness(&bin_dir, "bad-harness", BAD_MARKER, FAILING_EXIT);

    let toml_path = |p: &std::path::Path| p.display().to_string().replace('\\', "\\\\");
    std::fs::write(
        config_dir.join("config.toml"),
        format!(
            "version = 1\n\n\
             [integrations.claude-code]\nenabled = true\nexecutable = \"{}\"\n\n\
             [integrations.codex]\nenabled = true\nexecutable = \"{}\"\n",
            toml_path(&good),
            toml_path(&bad)
        ),
    )
    .expect("write user config");

    let base_args = |extra: &[&str]| {
        let mut args: Vec<String> = vec![
            "--scope".into(),
            project_dir.display().to_string(),
            "--data-dir".into(),
            state_dir.display().to_string(),
            "--config-dir".into(),
            config_dir.display().to_string(),
        ];
        args.extend(extra.iter().map(|s| (*s).to_owned()));
        args
    };

    // Nothing has run yet, so the listing must say so rather than inventing a
    // row or failing on an empty table.
    let empty = std::process::Command::new(env!("CARGO_BIN_EXE_glasshouse"))
        .args(base_args(&["sessions"]))
        .output()
        .expect("run glasshouse sessions");
    let empty_text = String::from_utf8_lossy(&empty.stdout);
    assert!(
        empty_text.contains("No sessions recorded"),
        "a fresh project should report no sessions, got:\n{empty_text}"
    );

    // A session that succeeds, and one that fails, each in a real terminal.
    for (harness, expected_exit) in [("claude-code", 0u32), ("codex", u32::from(FAILING_EXIT))] {
        let command = TerminalCommand::new(env!("CARGO_BIN_EXE_glasshouse"), tmp.path())
            .args(base_args(&["launch", harness]));
        // `wait_for_exit` answers the ConPTY startup handshake while it polls,
        // so nothing here has to babysit the terminal.
        let mut session = Session::spawn(command);
        let status = session.wait_for_exit();
        assert_eq!(
            status.code(),
            expected_exit,
            "`glasshouse launch {harness}` reported: {status}\n--- output ---\n{}\n--- end ---",
            session.output()
        );
    }

    let listed = std::process::Command::new(env!("CARGO_BIN_EXE_glasshouse"))
        .args(base_args(&["sessions"]))
        .output()
        .expect("run glasshouse sessions");
    let text = String::from_utf8_lossy(&listed.stdout);

    // Both sessions are there, written by one process and read by another,
    // which is the whole point: the record outlives the session.
    let rows: Vec<&str> = text
        .lines()
        .skip(1)
        .filter(|line| !line.trim().is_empty())
        .collect();
    assert_eq!(rows.len(), 2, "expected one row per session, got:\n{text}");

    let claude_row = rows
        .iter()
        .find(|row| row.contains("claude-code"))
        .unwrap_or_else(|| panic!("no row for the successful session:\n{text}"));
    let codex_row = rows
        .iter()
        .find(|row| row.contains("codex"))
        .unwrap_or_else(|| panic!("no row for the failed session:\n{text}"));

    // The harness's outcome reached the record, and so did its identity.
    //
    // This row used to read `closed`, because nothing gave a session a native
    // identifier and a stopped session without one has nothing to resume to.
    // Glasshouse now assigns Claude Code its identifier before the process
    // exists, so a cleanly stopped session is genuinely resumable — the first
    // time any session reaches that disposition in production.
    //
    // Codex still reads `failed` below rather than `resumable`: it names its
    // own sessions, so Glasshouse has nothing to record for it yet, and a
    // failed session is failed regardless.
    assert!(
        claude_row.contains("resumable"),
        "a cleanly stopped session with an assigned identifier should read as \
         resumable:\n{claude_row}"
    );
    assert!(
        codex_row.contains("failed"),
        "a harness that exited {FAILING_EXIT} should read as failed:\n{codex_row}"
    );

    // Columns line up, which is the only reason a listing is readable at all.
    for row in &rows {
        assert!(
            row.contains("normal") && row.contains("embedded"),
            "row is missing role/presentation columns:\n{row}"
        );
    }
}

/// The shell itself, in a real terminal, driven by real keystrokes.
///
/// The view has unit tests against `TestBackend`, but those prove only that a
/// pure function draws into a buffer. This is the part they cannot show: that
/// running the shipped binary with no arguments opens the interface, that the
/// project root is on screen, that the keyboard actually moves between
/// sessions and opens the overview, and that leaving restores the terminal
/// instead of stranding the user in the alternate screen.
#[test]
fn the_shell_opens_in_a_real_terminal_and_answers_the_keyboard() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let project_dir = tmp.path().join("proj");
    std::fs::create_dir_all(project_dir.join(".git")).expect("create project");
    let bin_dir = tmp.path().join("bin");
    std::fs::create_dir_all(&bin_dir).expect("create bin dir");
    let state_dir = tmp.path().join("state");
    let config_dir = tmp.path().join("config");
    std::fs::create_dir_all(&state_dir).expect("create state dir");
    std::fs::create_dir_all(&config_dir).expect("create config dir");

    let good = install_marker_harness(&bin_dir, "shell-harness", "SHELL-HARNESS-RAN", 0);
    let toml_path = |p: &std::path::Path| p.display().to_string().replace('\\', "\\\\");

    // Onboarding already done, or the wizard would own the terminal instead of
    // the shell.
    std::fs::write(
        config_dir.join("config.toml"),
        format!(
            "version = 1\n\n\
             [onboarding]\ncompleted = true\n\n\
             [integrations.claude-code]\nenabled = true\nexecutable = \"{}\"\n\n\
             [integrations.codex]\nenabled = true\nexecutable = \"{}\"\n",
            toml_path(&good),
            toml_path(&good)
        ),
    )
    .expect("write user config");

    let base: Vec<String> = vec![
        "--scope".into(),
        project_dir.display().to_string(),
        "--data-dir".into(),
        state_dir.display().to_string(),
        "--config-dir".into(),
        config_dir.display().to_string(),
    ];

    // Two recorded sessions, so the session bar and navigation have something
    // real to move between.
    for harness in ["claude-code", "codex"] {
        let mut args = base.clone();
        args.push("launch".into());
        args.push(harness.into());
        let mut session = Session::spawn(
            TerminalCommand::new(env!("CARGO_BIN_EXE_glasshouse"), tmp.path()).args(args),
        );
        session.wait_for_exit();
    }

    let mut shell = Session::spawn(
        TerminalCommand::new(env!("CARGO_BIN_EXE_glasshouse"), tmp.path()).args(base),
    );

    // The canonical project root is the thing the whole isolation model rests
    // on, so it has to be on screen. Match on the last component: the shell
    // truncates from the left when the terminal is narrow, and the tail is the
    // part that identifies the project.
    let leaf = project_dir
        .file_name()
        .expect("project dir has a name")
        .to_string_lossy()
        .into_owned();
    shell.expect(&leaf);

    // Assert against the root *field* specifically, not just "the leaf appears
    // somewhere on screen". The project's name and its root's last component
    // are the same string, so a bare `contains` stayed green even with the root
    // blanked out — found by mutating `render_root` and watching this test pass.
    //
    // There is no line structure to search: a full-screen Ratatui app positions
    // the cursor rather than emitting newlines, so after stripping the escape
    // sequences the whole frame is one run of text. The label and the path are
    // still written contiguously, which is enough to anchor on.
    let screen = strip_terminal_sequences(&shell.output());
    let after_label = screen.find("root ").map(|at| &screen[at + "root ".len()..]);
    let Some(after_label) = after_label else {
        panic!("the shell never drew the root field:\n--- screen ---\n{screen}\n--- end ---");
    };
    let field: String = after_label.chars().take(200).collect();
    assert!(
        field.contains(&leaf),
        "the root field must show the project root; found `{field}`\n         --- screen ---\n{screen}\n--- end ---"
    );
    assert!(
        screen.contains("glasshouse"),
        "the shell must name itself:\n{screen}"
    );

    // The keyboard drives it: open the overview, leave it, then quit.
    shell.send("o");
    shell.expect("HARNESS");

    shell.send("\x1b"); // Escape leaves the overlay, not Glasshouse.
    shell.send("\t"); // ...and the shell is still alive to answer Tab.
    shell.send("q");

    let status = shell.wait_for_exit();
    assert!(
        status.success(),
        "the shell should exit cleanly on `q`, got: {status}\n--- output ---\n{}\n--- end ---",
        shell.output()
    );

    // Leaving must put the terminal back. A shell that exits still on the
    // alternate screen leaves the user staring at a dead frame.
    let output = shell.output();
    assert!(
        output.contains("\x1b[?1049l") || !output.contains("\x1b[?1049h"),
        "the alternate screen was entered and never left:\n{output:?}"
    );
}

// ---------------------------------------------------------------------------
// SessionRuntime: several live harness sessions at once
// ---------------------------------------------------------------------------
//
// Everything above exercises one child process through the raw PTY layer.
// What follows exercises `glasshouse::session::runtime::SessionRuntime`,
// which holds several such children at once, each in its own pseudo-terminal
// with its own reader thread filling its own bounded `Scrollback`. See that
// module's doc comment for the properties these tests exist to prove: output
// is never lost while a session is unfocused, and exit is detected from the
// process rather than from output going quiet.
//
// `SessionRuntime` answers the ConPTY startup handshake itself for embedded
// sessions -- but only when whoever owns the runtime calls
// `answer_terminal_queries`, because the reader thread cannot write to the
// child. In the shipped product that caller is `shell::run`'s tick. These tests
// own the runtime directly, so they answer on its behalf, the same way
// `Session` above does for the raw `PtyProcess` tests.
//
// `an_embedded_session_answers_the_cursor_position_query_itself` is the test
// that proves the production responder works; this helper exists so the other
// runtime tests are not each blocked on a handshake they are not about.
#[derive(Default)]
struct DsrTracker(std::collections::HashMap<SessionId, usize>);

impl DsrTracker {
    /// Reply to any newly seen queries in `id`'s scrollback. Safe to call on
    /// every poll of every session in play: on any platform other than
    /// Windows the query never appears, so this is just a cheap scan of text
    /// that never matches.
    fn answer(&mut self, runtime: &mut SessionRuntime, id: &SessionId) {
        let Some(session) = runtime.get(id) else {
            return;
        };
        let query = std::str::from_utf8(DSR_CURSOR_POSITION_QUERY).expect("query is ascii");
        let seen = session.with_scrollback(|scrollback| scrollback.text().matches(query).count());
        let answered = self.0.entry(id.clone()).or_insert(0);
        while *answered < seen {
            let reply = std::str::from_utf8(DSR_CURSOR_POSITION_REPLY).expect("reply is ascii");
            // Best effort: if the session has already exited there is
            // nothing left to answer for, and the next poll will simply see
            // no further queries either.
            let _ = runtime.send_text(id, reply);
            *answered += 1;
        }
    }

    fn has_replied(&self, id: &SessionId) -> bool {
        self.0.get(id).is_some_and(|count| *count > 0)
    }
}

/// Wait until `id`'s first ConPTY handshake query has been answered, the way
/// `Session::spawn`'s internal wait does for the raw `PtyProcess` tests above.
///
/// Needed only by tests that write input to a session before ever waiting for
/// output from it: without this, a `send_text` issued before the handshake
/// completes would race the reply this same test still owes on Windows.
/// Elsewhere (waiting for output, waiting for exit) the polling loop answers
/// the query as a side effect, so callers that only ever wait need not call
/// this separately.
fn settle(runtime: &mut SessionRuntime, id: &SessionId, dsr: &mut DsrTracker) {
    let deadline = Instant::now() + SETTLE;
    loop {
        dsr.answer(runtime, id);
        if dsr.has_replied(id) || Instant::now() >= deadline {
            return;
        }
        std::thread::sleep(POLL);
    }
}

/// Wait until `needle` appears in `id`'s scrollback (after stripping terminal
/// control sequences, the same way `Session::expect` does above), answering
/// the ConPTY handshake along the way via `dsr`. Returns the stripped
/// scrollback so callers can make further assertions against it. Panics with
/// the raw scrollback if `needle` never appears within `TIMEOUT`.
fn wait_for_text(
    runtime: &mut SessionRuntime,
    id: &SessionId,
    dsr: &mut DsrTracker,
    needle: &str,
) -> String {
    let deadline = Instant::now() + TIMEOUT;
    loop {
        dsr.answer(runtime, id);
        let raw = runtime
            .get(id)
            .map(LiveSession::scrollback)
            .unwrap_or_default();
        let clean = strip_terminal_sequences(&raw);
        if clean.contains(needle) {
            return clean;
        }
        if Instant::now() >= deadline {
            panic!(
                "timed out waiting for {needle:?} in session `{id}`'s scrollback.\n\
                 --- scrollback ---\n{raw}\n--- end ---"
            );
        }
        std::thread::sleep(POLL);
    }
}

/// A project and an install directory for the runtime tests to drop fake
/// harnesses into, built exactly the way
/// `a_direct_executable_launches_through_the_harness_seam` builds its own: a
/// real `Project` discovered from a real (empty) `.git` directory, so every
/// `HarnessLaunch` built from it derives a real, project-bound working
/// directory rather than a stand-in.
struct RuntimeFixture {
    _tmp: tempfile::TempDir,
    project: Project,
    bin_dir: std::path::PathBuf,
}

impl RuntimeFixture {
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

    /// A `HarnessLaunch` for the executable at `path`, resolved through the
    /// same seam production code uses -- no explicit cwd or program appears
    /// anywhere in a test that uses this.
    fn launch(&self, path: &std::path::Path) -> HarnessLaunch<'_> {
        let resolved = exec::resolve_explicit(path).expect("resolve fake harness");
        HarnessLaunch::new(resolved, &self.project)
    }
}

/// Write a fake installed harness that reads one line from its input and
/// echoes it back prefixed with `GOT:` -- used to prove that specific
/// keystrokes reached a specific session.
#[cfg(windows)]
fn install_echo_harness(bin_dir: &std::path::Path, name: &str) -> std::path::PathBuf {
    let path = bin_dir.join(format!("{name}.cmd"));
    // Plain sequential lines, not one line joined with `&`: cmd.exe parses
    // and executes each line of a script *file* in turn, so `%line%` on the
    // line after `set /p` already sees the value just read -- no delayed
    // expansion needed here, unlike `shell_command`'s single-line form.
    std::fs::write(&path, "@echo off\r\nset /p line=\r\necho GOT:%line%\r\n")
        .expect("write echo harness");
    path
}

#[cfg(unix)]
fn install_echo_harness(bin_dir: &std::path::Path, name: &str) -> std::path::PathBuf {
    use std::os::unix::fs::PermissionsExt;

    let path = bin_dir.join(name);
    std::fs::write(&path, "#!/bin/sh\nread line\necho GOT:$line\n").expect("write echo harness");
    let mut perms = std::fs::metadata(&path).unwrap().permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&path, perms).unwrap();
    path
}

/// Write a fake installed harness that prints nothing at all and exits with
/// `exit_code` -- used to prove that exit detection depends on the process
/// itself, never on anything appearing in its output.
#[cfg(windows)]
fn install_silent_harness(
    bin_dir: &std::path::Path,
    name: &str,
    exit_code: u8,
) -> std::path::PathBuf {
    let path = bin_dir.join(format!("{name}.cmd"));
    std::fs::write(&path, format!("@echo off\r\nexit /b {exit_code}\r\n"))
        .expect("write silent harness");
    path
}

#[cfg(unix)]
fn install_silent_harness(
    bin_dir: &std::path::Path,
    name: &str,
    exit_code: u8,
) -> std::path::PathBuf {
    use std::os::unix::fs::PermissionsExt;

    let path = bin_dir.join(name);
    std::fs::write(&path, format!("#!/bin/sh\nexit {exit_code}\n")).expect("write silent harness");
    let mut perms = std::fs::metadata(&path).unwrap().permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&path, perms).unwrap();
    path
}

/// Write a fake installed harness that stays alive doing nothing for roughly
/// `seconds` -- used where a test needs a session it can still observe as
/// running after acting on a different one.
#[cfg(windows)]
fn install_sleep_harness(
    bin_dir: &std::path::Path,
    name: &str,
    seconds: u32,
) -> std::path::PathBuf {
    let path = bin_dir.join(format!("{name}.cmd"));
    std::fs::write(
        &path,
        format!("@echo off\r\nping -n {seconds} 127.0.0.1 > nul\r\n"),
    )
    .expect("write sleep harness");
    path
}

#[cfg(unix)]
fn install_sleep_harness(
    bin_dir: &std::path::Path,
    name: &str,
    seconds: u32,
) -> std::path::PathBuf {
    use std::os::unix::fs::PermissionsExt;

    let path = bin_dir.join(name);
    std::fs::write(&path, format!("#!/bin/sh\nsleep {seconds}\n")).expect("write sleep harness");
    let mut perms = std::fs::metadata(&path).unwrap().permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&path, perms).unwrap();
    path
}

/// Write a fake installed harness that prints `lines` short lines and exits
/// -- enough output, at any reasonable `lines` count, to overflow a small
/// scrollback bound many times over.
#[cfg(windows)]
fn install_flood_harness(bin_dir: &std::path::Path, name: &str, lines: u32) -> std::path::PathBuf {
    let path = bin_dir.join(format!("{name}.cmd"));
    std::fs::write(
        &path,
        format!(
            "@echo off\r\nfor /L %%i in (1,1,{lines}) do echo flood-line-%%i-0123456789012345678901234567890123456789\r\n"
        ),
    )
    .expect("write flood harness");
    path
}

#[cfg(unix)]
fn install_flood_harness(bin_dir: &std::path::Path, name: &str, lines: u32) -> std::path::PathBuf {
    use std::os::unix::fs::PermissionsExt;

    let path = bin_dir.join(name);
    std::fs::write(
        &path,
        format!(
            "#!/bin/sh\ni=0\nwhile [ $i -lt {lines} ]; do\n  echo \"flood-line-$i-0123456789012345678901234567890123456789\"\n  i=$((i+1))\ndone\n"
        ),
    )
    .expect("write flood harness");
    let mut perms = std::fs::metadata(&path).unwrap().permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&path, perms).unwrap();
    path
}

/// Two sessions started in one runtime run at the same time, each filling its
/// own scrollback: one session's output must never appear in the other's,
/// which is the entire reason `SessionRuntime` gives each session its own
/// `Scrollback` instead of sharing one stream.
#[test]
fn two_sessions_run_concurrently_with_independent_scrollback() {
    const MARKER_A: &str = "GLASSHOUSE-RUNTIME-MARKER-A";
    const MARKER_B: &str = "GLASSHOUSE-RUNTIME-MARKER-B";

    let fixture = RuntimeFixture::new();
    let harness_a = install_marker_harness(&fixture.bin_dir, "runtime-a", MARKER_A, 0);
    let harness_b = install_marker_harness(&fixture.bin_dir, "runtime-b", MARKER_B, 0);
    let launch_a = fixture.launch(&harness_a);
    let launch_b = fixture.launch(&harness_b);

    let mut runtime = SessionRuntime::new();
    let id_a = SessionId::new("runtime-a");
    let id_b = SessionId::new("runtime-b");
    runtime
        .start(id_a.clone(), SessionPresentation::Embedded, &launch_a)
        .expect("start a");
    runtime
        .start(id_b.clone(), SessionPresentation::Embedded, &launch_b)
        .expect("start b");

    let mut dsr = DsrTracker::default();
    let text_a = wait_for_text(&mut runtime, &id_a, &mut dsr, MARKER_A);
    let text_b = wait_for_text(&mut runtime, &id_b, &mut dsr, MARKER_B);

    assert!(
        !text_a.contains(MARKER_B),
        "session a's scrollback leaked session b's marker:\n{text_a}"
    );
    assert!(
        !text_b.contains(MARKER_A),
        "session b's scrollback leaked session a's marker:\n{text_b}"
    );

    runtime.close(&id_a).expect("close a");
    runtime.close(&id_b).expect("close b");
}

/// Sending text to a session that does not have focus still reaches it: focus
/// is only a statement about which session the keyboard currently reaches,
/// never about which sessions are allowed to receive anything at all -- see
/// `SessionRuntime::send_text`'s doc comment.
#[test]
fn an_unfocused_session_still_receives_sent_text() {
    let fixture = RuntimeFixture::new();
    let holder_harness = install_sleep_harness(&fixture.bin_dir, "focus-holder", 20);
    let echo_harness = install_echo_harness(&fixture.bin_dir, "unfocused-echo");
    let holder_launch = fixture.launch(&holder_harness);
    let echo_launch = fixture.launch(&echo_harness);

    let mut runtime = SessionRuntime::new();
    let id_holder = SessionId::new("focus-holder");
    let id_target = SessionId::new("unfocused-target");
    runtime
        .start(
            id_holder.clone(),
            SessionPresentation::Embedded,
            &holder_launch,
        )
        .expect("start holder");
    runtime
        .start(
            id_target.clone(),
            SessionPresentation::Embedded,
            &echo_launch,
        )
        .expect("start target");

    let mut dsr = DsrTracker::default();
    settle(&mut runtime, &id_holder, &mut dsr);
    settle(&mut runtime, &id_target, &mut dsr);

    runtime.focus(&id_holder).expect("focus holder");
    assert_eq!(runtime.focused(), Some(&id_holder));

    runtime
        .send_text(&id_target, "hello\r\n")
        .expect("send to the unfocused session");
    wait_for_text(&mut runtime, &id_target, &mut dsr, "GOT:hello");

    assert_eq!(
        runtime.focused(),
        Some(&id_holder),
        "sending text to another session must not move focus"
    );

    runtime.close(&id_holder).expect("close holder");
    runtime.close(&id_target).expect("close target");
}

/// Moving focus back and forth changes nothing about either process: both
/// keep the same pid and keep running throughout, because focus only records
/// which session the keyboard reaches and never touches a process -- see
/// `SessionRuntime::focus`'s doc comment.
#[test]
fn focus_changes_nothing_but_focus() {
    let fixture = RuntimeFixture::new();
    let harness_a = install_sleep_harness(&fixture.bin_dir, "steady-a", 20);
    let harness_b = install_sleep_harness(&fixture.bin_dir, "steady-b", 20);
    let launch_a = fixture.launch(&harness_a);
    let launch_b = fixture.launch(&harness_b);

    let mut runtime = SessionRuntime::new();
    let id_a = SessionId::new("steady-a");
    let id_b = SessionId::new("steady-b");
    runtime
        .start(id_a.clone(), SessionPresentation::Embedded, &launch_a)
        .expect("start a");
    runtime
        .start(id_b.clone(), SessionPresentation::Embedded, &launch_b)
        .expect("start b");

    let mut dsr = DsrTracker::default();
    settle(&mut runtime, &id_a, &mut dsr);
    settle(&mut runtime, &id_b, &mut dsr);

    let pid_a = runtime.get(&id_a).and_then(LiveSession::process_id);
    let pid_b = runtime.get(&id_b).and_then(LiveSession::process_id);

    for _ in 0..5 {
        runtime.focus(&id_a).expect("focus a");
        runtime.focus(&id_b).expect("focus b");
    }
    runtime.focus(&id_a).expect("focus a again");

    assert_eq!(runtime.get(&id_a).and_then(LiveSession::process_id), pid_a);
    assert_eq!(runtime.get(&id_b).and_then(LiveSession::process_id), pid_b);
    assert!(runtime.get(&id_a).expect("a present").is_running());
    assert!(runtime.get(&id_b).expect("b present").is_running());
    assert_eq!(runtime.focused(), Some(&id_a));

    runtime.close(&id_a).expect("close a");
    runtime.close(&id_b).expect("close b");
}

/// A headless session runs exactly like any other -- it still fills its own
/// scrollback -- but has no viewport to bring forward: `SessionRuntime::focus`
/// must refuse it with `RuntimeError::Headless` rather than silently doing
/// nothing, and it must never take focus on its own at start either.
#[test]
fn a_headless_session_runs_but_cannot_be_focused() {
    const MARKER: &str = "GLASSHOUSE-HEADLESS-MARKER";

    let fixture = RuntimeFixture::new();
    let harness = install_marker_harness(&fixture.bin_dir, "headless", MARKER, 0);
    let launch = fixture.launch(&harness);

    let mut runtime = SessionRuntime::new();
    let id = SessionId::new("headless-session");
    runtime
        .start(id.clone(), SessionPresentation::Headless, &launch)
        .expect("start headless");

    assert_eq!(
        runtime.focused(),
        None,
        "a headless session must never take focus on its own"
    );
    assert!(matches!(
        runtime.focus(&id),
        Err(RuntimeError::Headless { .. })
    ));

    let mut dsr = DsrTracker::default();
    let text = wait_for_text(&mut runtime, &id, &mut dsr, MARKER);
    assert!(text.contains(MARKER));

    runtime.close(&id).expect("close headless");
}

/// A session's exit is detected from the process itself, never inferred from
/// its output: this harness prints nothing at all, and
/// `SessionRuntime::poll_exits` must still notice it ended and report exactly
/// the exit code it used -- proving exit detection does not depend on output
/// the way it would if it merely watched for the pty to go quiet.
#[test]
fn exit_is_detected_with_no_output_at_all() {
    const SILENT_EXIT_CODE: u8 = 42;

    let fixture = RuntimeFixture::new();
    let harness = install_silent_harness(&fixture.bin_dir, "silent", SILENT_EXIT_CODE);
    let launch = fixture.launch(&harness);

    let mut runtime = SessionRuntime::new();
    let id = SessionId::new("silent-session");
    runtime
        .start(id.clone(), SessionPresentation::Embedded, &launch)
        .expect("start silent");

    let mut dsr = DsrTracker::default();
    settle(&mut runtime, &id, &mut dsr);

    let deadline = Instant::now() + TIMEOUT;
    let mut ended = None;
    while ended.is_none() && Instant::now() < deadline {
        for (ended_id, status) in runtime.poll_exits() {
            if ended_id == id {
                ended = Some(status);
            }
        }
        if ended.is_none() {
            std::thread::sleep(POLL);
        }
    }
    let status = ended.unwrap_or_else(|| {
        let scrollback = runtime
            .get(&id)
            .map(LiveSession::scrollback)
            .unwrap_or_default();
        panic!(
            "poll_exits never reported session `{id}` exiting.\n\
             --- scrollback ---\n{scrollback}\n--- end ---"
        );
    });

    assert_eq!(status.code(), u32::from(SILENT_EXIT_CODE));

    runtime.close(&id).expect("close silent");
}

/// The scrollback stays within its configured bound even under real output
/// from a real process: `SessionRuntime::with_scrollback_bytes` caps memory
/// per session regardless of how much a harness prints, and the discarded
/// count grows to say so.
#[test]
fn scrollback_stays_bounded_under_real_output() {
    const CAP: usize = 2048;

    let fixture = RuntimeFixture::new();
    // 3000 lines of ~55 bytes each is well over 100KB -- dozens of times CAP.
    let harness = install_flood_harness(&fixture.bin_dir, "flood", 3000);
    let launch = fixture.launch(&harness);

    let mut runtime = SessionRuntime::with_scrollback_bytes(CAP);
    let id = SessionId::new("flood-session");
    runtime
        .start(id.clone(), SessionPresentation::Embedded, &launch)
        .expect("start flood");

    let mut dsr = DsrTracker::default();
    let deadline = Instant::now() + TIMEOUT;
    loop {
        dsr.answer(&mut runtime, &id);
        let len = runtime
            .get(&id)
            .map(|session| session.with_scrollback(Scrollback::len))
            .unwrap_or(0);
        assert!(
            len <= CAP,
            "scrollback exceeded its cap mid-stream: {len} > {CAP}"
        );
        if runtime.poll_exits().iter().any(|(ended, _)| ended == &id) {
            break;
        }
        assert!(Instant::now() < deadline, "flood harness never exited");
        std::thread::sleep(POLL);
    }

    let (len, dropped) = runtime
        .get(&id)
        .map(|session| session.with_scrollback(|s| (s.len(), s.dropped())))
        .expect("session still present");
    assert!(len <= CAP, "scrollback grew past its bound: {len} > {CAP}");
    assert!(dropped > 0, "expected some output to have been discarded");

    runtime.close(&id).expect("close flood");
}

/// Closing one session removes only that one: the runtime keeps the others
/// running untouched, and if the closed session held focus it moves to a
/// live, focusable survivor rather than vanishing -- see the focus-recovery
/// logic in `SessionRuntime::close`.
#[test]
fn closing_one_session_leaves_the_others_running() {
    let fixture = RuntimeFixture::new();
    let harness_a = install_sleep_harness(&fixture.bin_dir, "closing-a", 20);
    let harness_b = install_sleep_harness(&fixture.bin_dir, "surviving-b", 20);
    let launch_a = fixture.launch(&harness_a);
    let launch_b = fixture.launch(&harness_b);

    let mut runtime = SessionRuntime::new();
    let id_a = SessionId::new("closing-a");
    let id_b = SessionId::new("surviving-b");
    runtime
        .start(id_a.clone(), SessionPresentation::Embedded, &launch_a)
        .expect("start a");
    runtime
        .start(id_b.clone(), SessionPresentation::Embedded, &launch_b)
        .expect("start b");

    let mut dsr = DsrTracker::default();
    settle(&mut runtime, &id_a, &mut dsr);
    settle(&mut runtime, &id_b, &mut dsr);

    assert_eq!(
        runtime.focused(),
        Some(&id_a),
        "the first focusable session should hold focus"
    );

    runtime.close(&id_a).expect("close a");

    assert_eq!(runtime.len(), 1);

    // Ask the operating system, repeatedly, over a window long enough for a
    // signal sent during `close` to have landed. `is_running()` alone cannot
    // show this: it reads the status cached by the last `poll_exits`, so a
    // survivor that had just been killed would still report itself running.
    // Mutating `close` to kill every session left this test green until the
    // poll below was added.
    let deadline = Instant::now() + Duration::from_millis(500);
    while Instant::now() < deadline {
        let ended = runtime.poll_exits();
        assert!(
            !ended.iter().any(|(id, _)| id == &id_b),
            "closing one session killed a different one: {ended:?}"
        );
        std::thread::sleep(POLL);
    }

    let survivor = runtime.get(&id_b).expect("survivor still present");
    assert!(
        survivor.is_running(),
        "the surviving session must still be running"
    );
    assert_eq!(
        runtime.focused(),
        Some(&id_b),
        "focus should move to the surviving session"
    );

    runtime.close(&id_b).expect("close b");
}

/// `SessionRuntime::write_to_focused` reaches whichever session currently
/// holds focus: it is the path every real keystroke from a real terminal
/// takes, so it has to actually reach the child process, not just record that
/// it tried.
#[test]
fn keystrokes_reach_the_focused_session() {
    let fixture = RuntimeFixture::new();
    let harness = install_echo_harness(&fixture.bin_dir, "focused-echo");
    let launch = fixture.launch(&harness);

    let mut runtime = SessionRuntime::new();
    let id = SessionId::new("focused-echo-session");
    runtime
        .start(id.clone(), SessionPresentation::Embedded, &launch)
        .expect("start echo");

    let mut dsr = DsrTracker::default();
    settle(&mut runtime, &id, &mut dsr);
    assert_eq!(runtime.focused(), Some(&id));

    let wrote = runtime
        .write_to_focused(b"hello\r\n")
        .expect("write to the focused session");
    assert!(
        wrote,
        "write_to_focused should report it had a session to send bytes to"
    );

    wait_for_text(&mut runtime, &id, &mut dsr, "GOT:hello");

    runtime.close(&id).expect("close echo");
}

/// A keystroke typed into the real shell reaching a real harness, and the
/// harness's answer coming back to the viewport.
///
/// Unix only, and the reason is worth recording rather than hiding behind the
/// `cfg`. The Windows fake harness reads its input with `set /p`, which wants a
/// CRLF, while a real Enter key is a bare carriage return and that is what
/// `encode` sends. Making `encode` emit CRLF to satisfy it would be wrong: it
/// would give every Unix harness a spurious extra newline per keystroke. The
/// forwarding path itself is covered on Windows by
/// `keystrokes_reach_the_focused_session`, which drives the same
/// `write_to_focused` at the runtime layer. Whether a bare carriage return
/// satisfies a real Windows harness is an open question recorded in the
/// handoff — the harnesses Glasshouse actually targets read raw input and
/// accept it, but that is reasoning, not evidence.
///
/// Everything else about the wiring is unit-tested against a `ShellState` with
/// no processes behind it. This is the one test that proves the whole chain
/// exists in the shipped binary: `glasshouse` with no arguments opens the
/// shell, `n` starts a real harness in a real pseudo-terminal, session mode
/// hands the keyboard to it, the bytes arrive, and its reply is drained into
/// the session's scrollback and drawn.
///
/// It also proves the mode split does what it is for: `q` is typed while in
/// session mode and must reach the harness rather than quitting Glasshouse.
#[cfg(unix)]
#[test]
fn a_keystroke_typed_into_the_shell_reaches_a_real_harness_and_comes_back() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let project_dir = tmp.path().join("proj");
    std::fs::create_dir_all(project_dir.join(".git")).expect("create project");
    let bin_dir = tmp.path().join("bin");
    std::fs::create_dir_all(&bin_dir).expect("create bin dir");
    let state_dir = tmp.path().join("state");
    let config_dir = tmp.path().join("config");
    std::fs::create_dir_all(&state_dir).expect("create state dir");
    std::fs::create_dir_all(&config_dir).expect("create config dir");

    // Reads one line and echoes it back with a prefix, so the reply proves the
    // input travelled the whole way rather than being echoed by the terminal.
    let harness = install_echo_harness(&bin_dir, "echoing");
    let toml_path = |p: &std::path::Path| p.display().to_string().replace('\\', "\\\\");
    std::fs::write(
        config_dir.join("config.toml"),
        format!(
            "version = 1\n\n[onboarding]\ncompleted = true\n\n\
             [integrations.claude-code]\nenabled = true\nexecutable = \"{}\"\n",
            toml_path(&harness)
        ),
    )
    .expect("write user config");

    let mut shell = Session::spawn(
        TerminalCommand::new(env!("CARGO_BIN_EXE_glasshouse"), tmp.path()).args([
            "--scope".to_owned(),
            project_dir.display().to_string(),
            "--data-dir".to_owned(),
            state_dir.display().to_string(),
            "--config-dir".to_owned(),
            config_dir.display().to_string(),
        ]),
    );

    // The shell is up once it has drawn the project root.
    shell.expect("root ");

    // `n` starts a real harness; the session bar names it once it exists.
    shell.send("n");
    shell.expect("claude-code");

    // Enter session mode, then type. `q` is deliberately part of the payload:
    // in session mode it belongs to the harness, and if the mode split were
    // wrong it would quit Glasshouse instead and the expect below would time
    // out on a dead process.
    shell.send("\r");
    shell.expect("ctrl-]");
    shell.send("quiet\r");

    shell.expect("GOT:quiet");

    // Leave session mode and quit through Glasshouse's own binding.
    shell.send("\x1d");
    shell.send("q");

    let status = shell.wait_for_exit();
    assert!(
        status.success(),
        "the shell should still exit cleanly on `q` after a session: {status}\n\
         --- output ---\n{}\n--- end ---",
        shell.output()
    );
}

/// The shell's mode machinery, in a real terminal, on every platform.
///
/// Separated from the round-trip above so Windows keeps coverage of the part
/// that does not depend on how a fake `.cmd` harness reads its input: the shell
/// opens, `n` starts a real harness, Enter hands the keyboard to it, `Ctrl-]`
/// takes it back, and `q` then quits Glasshouse rather than reaching the
/// harness. If the escape chord did not match — as it did not, before a real
/// terminal was used to check — `q` would go to the harness and this would hang.
#[test]
fn the_shell_enters_and_leaves_session_mode_in_a_real_terminal() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let project_dir = tmp.path().join("proj");
    std::fs::create_dir_all(project_dir.join(".git")).expect("create project");
    let bin_dir = tmp.path().join("bin");
    std::fs::create_dir_all(&bin_dir).expect("create bin dir");
    let state_dir = tmp.path().join("state");
    let config_dir = tmp.path().join("config");
    std::fs::create_dir_all(&state_dir).expect("create state dir");
    std::fs::create_dir_all(&config_dir).expect("create config dir");

    // Stays alive, so the escape chord is genuinely what returns control rather
    // than the session ending underneath the test.
    let harness = install_sleep_harness(&bin_dir, "lingering", 20);
    let toml_path = |p: &std::path::Path| p.display().to_string().replace('\\', "\\\\");
    std::fs::write(
        config_dir.join("config.toml"),
        format!(
            "version = 1\n\n[onboarding]\ncompleted = true\n\n\
             [integrations.claude-code]\nenabled = true\nexecutable = \"{}\"\n",
            toml_path(&harness)
        ),
    )
    .expect("write user config");

    let mut shell = Session::spawn(
        TerminalCommand::new(env!("CARGO_BIN_EXE_glasshouse"), tmp.path()).args([
            "--scope".to_owned(),
            project_dir.display().to_string(),
            "--data-dir".to_owned(),
            state_dir.display().to_string(),
            "--config-dir".to_owned(),
            config_dir.display().to_string(),
        ]),
    );

    shell.expect("root ");
    shell.send("n");
    shell.expect("claude-code");
    shell.send("\r");
    shell.expect("ctrl-]");

    // Back to control mode, then quit. `q` only quits if the escape landed.
    shell.send("\x1d");
    shell.send("q");

    let status = shell.wait_for_exit();
    assert!(
        status.success(),
        "the shell did not quit after leaving session mode, so the escape chord \
         never matched: {status}\n--- output ---\n{}\n--- end ---",
        shell.output()
    );
}

/// A resize of Glasshouse's own terminal reaching the harness's.
///
/// The mechanism (`PtyProcess::resize`) has its own tests, and the shell calls
/// it on every `Event::Resize`, but nothing proved the two were joined up. This
/// asks the harness itself, twice, through the keystroke path proved above:
/// `stty size` before and after the outer terminal changes shape. If the event
/// were dropped anywhere between Crossterm and the child's pseudo-terminal, the
/// second answer would equal the first.
///
/// Unix only: `stty` is the portable way to ask a terminal its size from
/// inside a shell, and Windows has no equivalent a `.cmd` harness can run.
#[cfg(unix)]
#[test]
fn resizing_the_shell_reaches_the_harness_terminal() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let project_dir = tmp.path().join("proj");
    std::fs::create_dir_all(project_dir.join(".git")).expect("create project");
    let state_dir = tmp.path().join("state");
    let config_dir = tmp.path().join("config");
    std::fs::create_dir_all(&state_dir).expect("create state dir");
    std::fs::create_dir_all(&config_dir).expect("create config dir");

    // A plain shell is the harness: it can be asked its terminal size.
    //
    // Registered as Codex, not Claude Code: Glasshouse assigns Claude Code a
    // native session identifier, and `/bin/sh --session-id <uuid>` prints its
    // usage instead of running. Codex names its own sessions, so it is started
    // bare — which is what this test needs, since it is about resize reaching
    // the child and not about arguments.
    std::fs::write(
        config_dir.join("config.toml"),
        "version = 1\n\n[onboarding]\ncompleted = true\n\n\
         [integrations.codex]\nenabled = true\nexecutable = \"/bin/sh\"\n",
    )
    .expect("write user config");

    let start = TerminalSize::new(24, 80);
    let mut shell = Session::spawn(
        TerminalCommand::new(env!("CARGO_BIN_EXE_glasshouse"), tmp.path())
            .size(start)
            .args([
                "--scope".to_owned(),
                project_dir.display().to_string(),
                "--data-dir".to_owned(),
                state_dir.display().to_string(),
                "--config-dir".to_owned(),
                config_dir.display().to_string(),
            ]),
    );

    shell.expect("root ");
    shell.send("n");
    shell.expect("codex");
    shell.send("\r");
    shell.expect("ctrl-]");

    // The harness's view of its own terminal, before anything moves. It is
    // smaller than Glasshouse's, because the viewport is inset by the top bar,
    // session bar, status bar and border — the exact figures are the shell's
    // business, so this only asserts that they change.
    shell.send("stty size > /tmp/gh-size-1 2>&1\r");
    let first = read_when_written("/tmp/gh-size-1", &mut shell);

    // Now change the outer terminal, exactly as a window manager would.
    shell
        .process()
        .resize(TerminalSize::new(40, 120))
        .expect("resize the shell's terminal");

    // Give the SIGWINCH time to become a Crossterm event, be handled, and
    // reach the child's pseudo-terminal before asking.
    let settle = Instant::now() + Duration::from_millis(600);
    while Instant::now() < settle {
        shell.answer_pending_queries();
        std::thread::sleep(POLL);
    }

    shell.send("stty size > /tmp/gh-size-2 2>&1\r");
    let second = read_when_written("/tmp/gh-size-2", &mut shell);

    assert_ne!(
        first.trim(),
        second.trim(),
        "the harness saw the same terminal size before and after the shell was \
         resized, so the event never reached its pseudo-terminal"
    );

    shell.send("\x1d");
    let back = Instant::now() + Duration::from_millis(300);
    while Instant::now() < back {
        shell.answer_pending_queries();
        std::thread::sleep(POLL);
    }
    shell.send("q");
    let status = shell.wait_for_exit();
    assert!(
        status.success(),
        "the shell must still quit while a session is live: {status}"
    );
    let _ = std::fs::remove_file("/tmp/gh-size-1");
    let _ = std::fs::remove_file("/tmp/gh-size-2");
}

/// Wait for a file the harness is writing, keeping the session serviced while
/// polling so a handshake query never blocks it.
#[cfg(unix)]
fn read_when_written(path: &str, session: &mut Session) -> String {
    let _ = std::fs::remove_file(path);
    let deadline = Instant::now() + TIMEOUT;
    while Instant::now() < deadline {
        session.answer_pending_queries();
        if let Ok(text) = std::fs::read_to_string(path)
            && !text.trim().is_empty()
        {
            return text;
        }
        std::thread::sleep(POLL);
    }
    panic!(
        "the harness never wrote {path}\n--- shell output ---\n{}\n--- end ---",
        session.output()
    );
}

/// An environment override reaching a real child, not just the command builder.
///
/// `TerminalCommand`'s `env`/`env_remove` ordering has unit tests, but those
/// inspect the builder's own record of what it would do. This spawns a real
/// process and asks it what it actually received, which is the difference
/// between "the struct holds the right value" and "the child got it" — the
/// same gap that hid a Windows-only launch defect earlier in this project.
///
/// Both halves are exercised without touching this process's own environment:
/// one variable is set, and a second is set and then removed, so the removal
/// is observable in the child without a `set_var` that would race the other
/// tests sharing this process.
#[test]
fn an_environment_override_reaches_a_real_child() {
    const KEPT: &str = "GLASSHOUSE_ENV_KEPT";
    const DROPPED: &str = "GLASSHOUSE_ENV_DROPPED";
    const VALUE: &str = "reached-the-child";
    const GONE: &str = "should-not-survive";

    let cwd = std::env::temp_dir();
    let script = if cfg!(windows) {
        format!("echo kept=%{KEPT}% dropped=[%{DROPPED}%]")
    } else {
        format!("echo kept=${KEPT} dropped=[${DROPPED}]")
    };

    let command = shell_command(&script, &cwd)
        .env(KEPT, VALUE)
        .env(DROPPED, GONE)
        .env_remove(DROPPED);
    let mut session = Session::spawn(command);

    session.expect("kept=");
    let status = session.wait_for_exit();
    assert!(status.success(), "{status}");

    let output = strip_terminal_sequences(&session.output());
    assert!(
        output.contains(&format!("kept={VALUE}")),
        "the override never reached the child:\n{output}"
    );
    assert!(
        !output.contains(GONE),
        "a variable removed after being set still reached the child:\n{output}"
    );
}

/// An embedded session answers the cursor-position query itself.
///
/// This is the rule `session::attach` inverts. `attach` is a pass-through and
/// must never answer `ESC[6n` — the user's real terminal does, and a second
/// reply would reach the harness as input. An embedded session has no real
/// terminal behind it: Glasshouse owns the buffer and redraws it, so if nothing
/// answers, a harness that waits for the reply waits forever and looks exactly
/// like one that started and did nothing.
///
/// The harness here asks, then reads back precisely the six bytes of a reply
/// for a fresh screen (`ESC[1;1R`) and writes them to a file, so the test can
/// assert on the actual bytes rather than on the absence of a hang.
///
/// Unix only: it needs `printf`/`head -c`, and a `.cmd` harness has no
/// equivalent way to read a fixed byte count from its own terminal.
#[cfg(unix)]
#[test]
fn an_embedded_session_answers_the_cursor_position_query_itself() {
    use std::os::unix::fs::PermissionsExt;

    let fixture = RuntimeFixture::new();
    let reply_path = fixture.bin_dir.join("reply.bin");
    let path = fixture.bin_dir.join("asks-where-it-is");
    std::fs::write(
        &path,
        // `stty raw`: a Device Status Report reply carries no newline, and a
        // pseudo-terminal in canonical mode delivers nothing until one arrives,
        // so a cooked read would block forever no matter who answered. `-echo`
        // keeps the reply out of the session's own output.
        format!(
            "#!/bin/sh\nstty raw -echo\nprintf '\\033[6n'\nhead -c 6 > '{}'\nstty sane\n",
            reply_path.display()
        ),
    )
    .expect("write harness");
    let mut perms = std::fs::metadata(&path).unwrap().permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&path, perms).unwrap();

    let launch = fixture.launch(&path);
    let mut runtime = SessionRuntime::new();
    let id = SessionId::new("asks");
    runtime
        .start(id.clone(), SessionPresentation::Embedded, &launch)
        .expect("start");

    // The interface's tick does exactly this: drain exits, answer queries.
    // Wait for six bytes, not for the file: the shell's redirection creates it
    // the instant the harness starts, long before `head` has read anything.
    let deadline = Instant::now() + TIMEOUT;
    while Instant::now() < deadline {
        runtime.answer_terminal_queries();
        runtime.poll_exits();
        if std::fs::metadata(&reply_path).map(|m| m.len()).unwrap_or(0) >= 6 {
            break;
        }
        std::thread::sleep(POLL);
    }

    let reply = std::fs::read(&reply_path).unwrap_or_else(|_| {
        panic!(
            "the harness never received a reply to its cursor-position query\n\
             bytes written so far: {:?}\n--- its output ---\n{}\n--- end ---",
            std::fs::metadata(&reply_path).map(|m| m.len()),
            runtime
                .get(&id)
                .map(LiveSession::scrollback)
                .unwrap_or_default()
        )
    });

    assert_eq!(
        reply, b"\x1b[1;1R",
        "expected a Device Status Report for a fresh screen, got {reply:?}"
    );

    runtime.close(&id).expect("close");
}

/// A harness that reports the arguments it was given, so a test can read the
/// command line Glasshouse actually built rather than the one it meant to.
#[cfg(unix)]
fn install_argv_harness(bin_dir: &std::path::Path, name: &str) -> std::path::PathBuf {
    use std::os::unix::fs::PermissionsExt;

    let path = bin_dir.join(name);
    std::fs::write(&path, "#!/bin/sh\necho \"ARGV:$*\"\nexit 0\n").expect("write fake harness");
    let mut perms = std::fs::metadata(&path).unwrap().permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&path, perms).unwrap();
    path
}

/// Glasshouse assigns Claude Code its native session identifier, and records
/// the same one it handed over.
///
/// Both halves matter and only together. An identifier on the command line
/// that was never recorded cannot be resumed; a recorded identifier the
/// harness never received names a conversation that does not exist. This runs
/// the shipped binary, reads the argument list from the harness itself, and
/// then reads the record back through the same store production writes to.
///
/// Unix only for the harness script; the assignment itself is
/// platform-independent and covered by the adapter's own tests everywhere.
#[cfg(unix)]
#[test]
fn a_claude_code_session_is_launched_and_recorded_under_one_identifier() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let project_dir = tmp.path().join("proj");
    std::fs::create_dir_all(project_dir.join(".git")).expect("create project");
    let state_dir = tmp.path().join("state");
    let config_dir = tmp.path().join("config");
    let bin_dir = tmp.path().join("bin");
    for dir in [&state_dir, &config_dir, &bin_dir] {
        std::fs::create_dir_all(dir).expect("create dir");
    }

    let harness = install_argv_harness(&bin_dir, "fake-claude");
    std::fs::write(
        config_dir.join("config.toml"),
        format!(
            "version = 1\n\n[integrations.claude-code]\nenabled = true\nexecutable = \"{}\"\n",
            harness.display()
        ),
    )
    .expect("write user config");

    let mut session = Session::spawn(
        TerminalCommand::new(env!("CARGO_BIN_EXE_glasshouse"), tmp.path())
            .arg("--scope")
            .arg(&project_dir)
            .arg("--data-dir")
            .arg(&state_dir)
            .arg("--config-dir")
            .arg(&config_dir)
            .arg("launch")
            .arg("claude-code"),
    );

    let deadline = Instant::now() + TIMEOUT;
    let mut argv = None;
    while Instant::now() < deadline {
        session.answer_pending_queries();
        let clean = strip_terminal_sequences(&session.output());
        if let Some(line) = clean.lines().find(|line| line.contains("ARGV:")) {
            argv = Some(line.trim().to_owned());
            break;
        }
        std::thread::sleep(POLL);
    }
    let argv = argv.unwrap_or_else(|| {
        panic!(
            "the harness never reported its arguments\n--- output ---\n{}\n--- end ---",
            session.output()
        )
    });
    let _ = session.wait_for_exit();

    // What Glasshouse put on the command line.
    let handed_over = argv
        .split("--session-id")
        .nth(1)
        .unwrap_or_else(|| panic!("no --session-id in the harness's arguments: {argv}"))
        .split_whitespace()
        .next()
        .unwrap_or_else(|| panic!("--session-id had no value: {argv}"))
        .to_owned();
    assert_eq!(handed_over.len(), 36, "not a UUID: {handed_over}");

    // What Glasshouse wrote down, read back the way production reads it.
    use clap::Parser as _;
    let cli = glasshouse::Cli::try_parse_from([
        "glasshouse",
        "--scope",
        project_dir.to_str().unwrap(),
        "--data-dir",
        state_dir.to_str().unwrap(),
        "--config-dir",
        config_dir.to_str().unwrap(),
    ])
    .expect("cli");
    let runtime = glasshouse::bootstrap(&cli, tmp.path()).expect("bootstrap");
    let sessions = glasshouse::session::ProjectSessions::open(&runtime).expect("open sessions");
    let records = sessions.store().list().expect("list sessions");
    assert_eq!(
        records.len(),
        1,
        "expected exactly one session: {records:?}"
    );

    assert_eq!(
        records[0].native_session_id.as_deref(),
        Some(handed_over.as_str()),
        "the identifier Glasshouse handed the harness is not the one it recorded"
    );
}

/// The whole resume path, through the shipped binary: launch a session, let it
/// stop, then reopen it and check the harness was handed the same
/// conversation.
///
/// This is what Phase 6's adapter and Phase 7's assigned identifier were both
/// for, and nothing before it could be tested end to end — until an adapter
/// existed there was no resume command to run, and until an identifier was
/// assigned there was never anything to resume to.
#[cfg(unix)]
#[test]
fn a_recorded_session_is_resumed_under_the_identifier_it_was_given() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let project_dir = tmp.path().join("proj");
    std::fs::create_dir_all(project_dir.join(".git")).expect("create project");
    let state_dir = tmp.path().join("state");
    let config_dir = tmp.path().join("config");
    let bin_dir = tmp.path().join("bin");
    for dir in [&state_dir, &config_dir, &bin_dir] {
        std::fs::create_dir_all(dir).expect("create dir");
    }

    let harness = install_argv_harness(&bin_dir, "fake-claude");
    std::fs::write(
        config_dir.join("config.toml"),
        format!(
            "version = 1\n\n[integrations.claude-code]\nenabled = true\nexecutable = \"{}\"\n",
            harness.display()
        ),
    )
    .expect("write user config");

    let glasshouse = |args: &[&str]| {
        let mut command = TerminalCommand::new(env!("CARGO_BIN_EXE_glasshouse"), tmp.path())
            .arg("--scope")
            .arg(&project_dir)
            .arg("--data-dir")
            .arg(&state_dir)
            .arg("--config-dir")
            .arg(&config_dir);
        for arg in args {
            command = command.arg(arg);
        }
        command
    };

    let read_argv = |session: &mut Session| -> String {
        let deadline = Instant::now() + TIMEOUT;
        while Instant::now() < deadline {
            session.answer_pending_queries();
            let clean = strip_terminal_sequences(&session.output());
            if let Some(line) = clean.lines().find(|line| line.contains("ARGV:")) {
                return line.trim().to_owned();
            }
            std::thread::sleep(POLL);
        }
        panic!(
            "the harness never reported its arguments\n--- output ---\n{}\n--- end ---",
            session.output()
        )
    };

    // Start one, and let it finish.
    let mut first = Session::spawn(glasshouse(&["launch", "claude-code"]));
    let started = read_argv(&mut first);
    let _ = first.wait_for_exit();

    let assigned = started
        .split("--session-id")
        .nth(1)
        .unwrap_or_else(|| panic!("no --session-id when launching: {started}"))
        .split_whitespace()
        .next()
        .expect("an identifier")
        .to_owned();

    // The short form is the only identifier the listing shows, so it is the
    // one a user would type — and therefore the one this test uses.
    let mut listing = Session::spawn(glasshouse(&["sessions"]));
    let _ = listing.wait_for_exit();
    let text = strip_terminal_sequences(&listing.output());
    let row = text
        .lines()
        .find(|line| line.contains("resumable"))
        .unwrap_or_else(|| panic!("no resumable session in the listing:\n{text}"));
    let short = row.split_whitespace().next().expect("an identifier column");
    assert_eq!(short.len(), 12, "the listing's short form changed: {row}");

    // Reopen it.
    let mut second = Session::spawn(glasshouse(&["resume", short]));
    let resumed = read_argv(&mut second);
    let _ = second.wait_for_exit();

    assert!(
        resumed.contains("--resume"),
        "resuming did not use the harness's own resume mechanism: {resumed}"
    );
    assert!(
        resumed.contains(&assigned),
        "resumed a different conversation:\n  assigned {assigned}\n  resumed  {resumed}"
    );
    assert!(
        !resumed.contains("--session-id"),
        "a resumed session must not also be assigned a fresh identifier: {resumed}"
    );
}

/// Resuming something this project never recorded is refused, and says so.
#[cfg(unix)]
#[test]
fn resuming_an_unknown_session_is_refused() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let project_dir = tmp.path().join("proj");
    std::fs::create_dir_all(project_dir.join(".git")).expect("create project");
    let state_dir = tmp.path().join("state");
    let config_dir = tmp.path().join("config");
    for dir in [&state_dir, &config_dir] {
        std::fs::create_dir_all(dir).expect("create dir");
    }

    let mut session = Session::spawn(
        TerminalCommand::new(env!("CARGO_BIN_EXE_glasshouse"), tmp.path())
            .arg("--scope")
            .arg(&project_dir)
            .arg("--data-dir")
            .arg(&state_dir)
            .arg("--config-dir")
            .arg(&config_dir)
            .arg("resume")
            .arg("ffffffffffff"),
    );
    let status = session.wait_for_exit();
    assert!(!status.success(), "resuming nothing must not succeed");

    let text = strip_terminal_sequences(&session.output());
    assert!(
        text.contains("no session"),
        "the refusal must say what was wrong:\n{text}"
    );
}

/// A session with nothing to resume to is refused, by name, rather than
/// reopened as something blank.
///
/// Codex names its own sessions, so Glasshouse has no identifier for one it
/// started — which makes a cleanly stopped Codex session `closed`, not
/// `resumable`. Reopening it would produce a fresh, empty conversation
/// wearing an old session's identity, which is precisely what the store's
/// resume guard exists to prevent.
///
/// This is the test that reaches that guard on the production path: an
/// unknown identifier is refused earlier, by the resolver, so it proves
/// nothing about the guard at all.
#[cfg(unix)]
#[test]
fn resuming_a_session_with_no_conversation_is_refused() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let project_dir = tmp.path().join("proj");
    std::fs::create_dir_all(project_dir.join(".git")).expect("create project");
    let state_dir = tmp.path().join("state");
    let config_dir = tmp.path().join("config");
    let bin_dir = tmp.path().join("bin");
    for dir in [&state_dir, &config_dir, &bin_dir] {
        std::fs::create_dir_all(dir).expect("create dir");
    }

    let harness = install_argv_harness(&bin_dir, "fake-codex");
    std::fs::write(
        config_dir.join("config.toml"),
        format!(
            "version = 1\n\n[integrations.codex]\nenabled = true\nexecutable = \"{}\"\n",
            harness.display()
        ),
    )
    .expect("write user config");

    let glasshouse = |args: &[&str]| {
        let mut command = TerminalCommand::new(env!("CARGO_BIN_EXE_glasshouse"), tmp.path())
            .arg("--scope")
            .arg(&project_dir)
            .arg("--data-dir")
            .arg(&state_dir)
            .arg("--config-dir")
            .arg(&config_dir);
        for arg in args {
            command = command.arg(arg);
        }
        command
    };

    let mut first = Session::spawn(glasshouse(&["launch", "codex"]));
    let deadline = Instant::now() + TIMEOUT;
    while Instant::now() < deadline {
        first.answer_pending_queries();
        if strip_terminal_sequences(&first.output()).contains("ARGV:") {
            break;
        }
        std::thread::sleep(POLL);
    }
    let _ = first.wait_for_exit();

    let mut listing = Session::spawn(glasshouse(&["sessions"]));
    let _ = listing.wait_for_exit();
    let text = strip_terminal_sequences(&listing.output());
    let row = text
        .lines()
        .find(|line| line.contains("codex"))
        .unwrap_or_else(|| panic!("no codex session in the listing:\n{text}"));
    assert!(
        row.contains("closed"),
        "a harness that names its own sessions leaves nothing to resume to: {row}"
    );
    let short = row.split_whitespace().next().expect("an identifier column");

    let mut second = Session::spawn(glasshouse(&["resume", short]));
    let status = second.wait_for_exit();
    assert!(
        !status.success(),
        "resuming a session with no conversation must not succeed"
    );

    let refusal = strip_terminal_sequences(&second.output());
    assert!(
        refusal.contains("closed"),
        "the refusal must say why the session cannot be resumed:\n{refusal}"
    );
    assert!(
        !refusal.contains("ARGV:"),
        "the harness must never be started for a session with nothing to resume:\n{refusal}"
    );
}

/// A harness that stays alive long enough to be observed, and reports the
/// arguments it was given.
#[cfg(unix)]
fn install_lingering_harness(bin_dir: &std::path::Path, name: &str) -> std::path::PathBuf {
    use std::os::unix::fs::PermissionsExt;

    let path = bin_dir.join(name);
    std::fs::write(&path, "#!/bin/sh\necho \"ARGV:$*\"\nsleep 20\nexit 0\n")
        .expect("write fake harness");
    let mut perms = std::fs::metadata(&path).unwrap().permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&path, perms).unwrap();
    path
}

/// The hooks Glasshouse installs move a live session's state.
///
/// The command line is not re-derived here: it is read out of the settings
/// document Glasshouse generated and run exactly as written, through a shell,
/// which is how Claude Code runs it. That makes this a test of the quoting as
/// much as of the reporting — an executable path with a space in it would
/// break the hook and nothing else would notice.
///
/// No model turn is involved. Claude Code's own firing of these hooks is
/// verified separately by a runtime probe recorded in the evidence ledger;
/// what is proved here is every part Glasshouse owns.
#[cfg(unix)]
#[test]
fn an_installed_hook_moves_the_session_state() {
    use clap::Parser as _;

    let tmp = tempfile::tempdir().expect("tempdir");
    let project_dir = tmp.path().join("proj");
    std::fs::create_dir_all(project_dir.join(".git")).expect("create project");
    let state_dir = tmp.path().join("state");
    let config_dir = tmp.path().join("config");
    let bin_dir = tmp.path().join("bin");
    for dir in [&state_dir, &config_dir, &bin_dir] {
        std::fs::create_dir_all(dir).expect("create dir");
    }

    let harness = install_lingering_harness(&bin_dir, "fake-claude");
    std::fs::write(
        config_dir.join("config.toml"),
        format!(
            "version = 1\n\n[integrations.claude-code]\nenabled = true\nexecutable = \"{}\"\n",
            harness.display()
        ),
    )
    .expect("write user config");

    let mut session = Session::spawn(
        TerminalCommand::new(env!("CARGO_BIN_EXE_glasshouse"), tmp.path())
            .arg("--scope")
            .arg(&project_dir)
            .arg("--data-dir")
            .arg(&state_dir)
            .arg("--config-dir")
            .arg(&config_dir)
            .arg("launch")
            .arg("claude-code"),
    );

    // Wait until the harness is up and has told us its arguments.
    let deadline = Instant::now() + TIMEOUT;
    let mut argv = None;
    while Instant::now() < deadline {
        session.answer_pending_queries();
        let clean = strip_terminal_sequences(&session.output());
        if let Some(line) = clean.lines().find(|line| line.contains("ARGV:")) {
            argv = Some(line.trim().to_owned());
            break;
        }
        std::thread::sleep(POLL);
    }
    let argv = argv.unwrap_or_else(|| {
        panic!(
            "the harness never started\n--- output ---\n{}\n--- end ---",
            session.output()
        )
    });
    assert!(
        argv.contains("--settings"),
        "Glasshouse did not install any hooks: {argv}"
    );

    let cli = glasshouse::Cli::try_parse_from([
        "glasshouse",
        "--scope",
        project_dir.to_str().unwrap(),
        "--data-dir",
        state_dir.to_str().unwrap(),
        "--config-dir",
        config_dir.to_str().unwrap(),
    ])
    .expect("cli");
    let runtime = glasshouse::bootstrap(&cli, tmp.path()).expect("bootstrap");

    let record = {
        let sessions = glasshouse::session::ProjectSessions::open(&runtime).expect("sessions");
        let records = sessions.store().list().expect("list");
        records.into_iter().next().expect("one session")
    };
    assert_eq!(
        record.lifecycle,
        glasshouse::session::SessionLifecycle::Running,
        "a launched harness should be running"
    );

    // The settings document Glasshouse wrote, read back and used verbatim.
    let settings_path = runtime
        .session_dir(record.id.as_str())
        .join("claude-settings.json");
    let settings = std::fs::read_to_string(&settings_path)
        .unwrap_or_else(|err| panic!("no settings at {}: {err}", settings_path.display()));

    let command = settings
        .lines()
        .find(|line| line.contains("PermissionRequest"))
        .and(
            settings
                .split("\"PermissionRequest\"")
                .nth(1)
                .and_then(|rest| rest.split("\"command\": \"").nth(1))
                .and_then(|rest| rest.split('"').next()),
        )
        .unwrap_or_else(|| panic!("no PermissionRequest command in:\n{settings}"))
        .replace("\\\"", "\"")
        .replace("\\\\", "\\");

    let status = std::process::Command::new("/bin/sh")
        .arg("-c")
        .arg(&command)
        .status()
        .expect("run the hook command");
    assert!(
        status.success(),
        "a hook must always succeed, or Claude Code treats it as a veto: {command}"
    );

    let after = {
        let sessions = glasshouse::session::ProjectSessions::open(&runtime).expect("sessions");
        sessions
            .store()
            .get(&record.id)
            .expect("get")
            .expect("the session")
    };
    assert_eq!(
        after.lifecycle,
        glasshouse::session::SessionLifecycle::WaitingForUser,
        "the harness said it was asking permission, and the record did not follow"
    );

    session.send("\x03");
    let _ = session.wait_for_exit();
}

/// A hook that cannot do its job still succeeds.
///
/// Claude Code treats a hook's non-zero exit as a veto: a `UserPromptSubmit`
/// hook that exits non-zero blocks the prompt outright, with the user's words
/// echoed back and nothing sent. That was observed directly against the real
/// binary, which is why this is a test and not a preference.
///
/// So every way a report can fail — a session that is not there, a database
/// that cannot be opened, an event nobody recognises — has to end in exit 0.
/// Glasshouse's bookkeeping is never worth costing the user a turn.
#[cfg(unix)]
#[test]
fn a_hook_that_cannot_report_still_exits_zero() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let project_dir = tmp.path().join("proj");
    std::fs::create_dir_all(project_dir.join(".git")).expect("create project");
    let state_dir = tmp.path().join("state");
    let config_dir = tmp.path().join("config");
    for dir in [&state_dir, &config_dir] {
        std::fs::create_dir_all(dir).expect("create dir");
    }

    let cases: [(&str, &str); 3] = [
        // A session this project has never heard of.
        ("ffffffffffffffffffffffffffffffff", "Stop"),
        // Something that is not an identifier at all.
        ("not-an-identifier", "Stop"),
        // An event this build does not recognise.
        ("ffffffffffffffffffffffffffffffff", "SomeFutureEvent"),
    ];

    for (session, event) in cases {
        let status = std::process::Command::new(env!("CARGO_BIN_EXE_glasshouse"))
            .args([
                "--scope",
                project_dir.to_str().unwrap(),
                "--data-dir",
                state_dir.to_str().unwrap(),
                "--config-dir",
                config_dir.to_str().unwrap(),
                "hook",
                "--session",
                session,
                "--event",
                event,
            ])
            .status()
            .expect("run the hook");
        assert!(
            status.success(),
            "reporting `{event}` for `{session}` exited {status}; Claude Code would have \
             treated that as a veto and blocked the user's prompt"
        );
    }
}

/// The real Claude Code TUI, drawn inside Glasshouse's viewport.
///
/// Opt-in: set `GLASSHOUSE_PROBE_REAL_HARNESS=1`. Without it this skips, so an
/// ordinary `cargo test` never starts somebody's real coding agent. It submits
/// nothing and costs no model turn — it starts a session, reads the screen,
/// and leaves.
///
/// This is the only honest way to check that a harness's own interface
/// survives the round trip through `vt100` into Ratatui cells: a fake harness
/// proves the pipe works, not that a real TUI is legible at the other end.
#[cfg(unix)]
#[test]
fn the_real_claude_code_interface_appears_in_the_viewport() {
    if std::env::var("GLASSHOUSE_PROBE_REAL_HARNESS").as_deref() != Ok("1") {
        eprintln!("skipping: set GLASSHOUSE_PROBE_REAL_HARNESS=1 to run the real-harness probe");
        return;
    }
    let Ok(claude) = glasshouse::platform::exec::resolve("claude") else {
        eprintln!("skipping: `claude` is not on PATH");
        return;
    };

    let tmp = tempfile::tempdir().expect("tempdir");
    let state_dir = tmp.path().join("state");
    let config_dir = tmp.path().join("config");
    for dir in [&state_dir, &config_dir] {
        std::fs::create_dir_all(dir).expect("create dir");
    }
    // The project is this repository, which the user's Claude Code already
    // trusts; a fresh directory would meet the workspace-trust prompt instead
    // of the interface under test.
    let project_dir = std::env::var("CARGO_MANIFEST_DIR").expect("manifest dir");

    std::fs::write(
        config_dir.join("config.toml"),
        format!(
            "version = 1\n\n[onboarding]\ncompleted = true\n\n\
             [integrations.claude-code]\nenabled = true\nexecutable = \"{}\"\n",
            claude.path().display()
        ),
    )
    .expect("write user config");

    let mut shell = Session::spawn(
        TerminalCommand::new(env!("CARGO_BIN_EXE_glasshouse"), tmp.path())
            .size(TerminalSize::new(40, 120))
            .args([
                "--scope".to_owned(),
                project_dir,
                "--data-dir".to_owned(),
                state_dir.display().to_string(),
                "--config-dir".to_owned(),
                config_dir.display().to_string(),
            ]),
    );

    shell.expect("root ");

    // The harness's own version string, asked of the harness itself. It is
    // the one thing on the opening screen that Glasshouse's chrome can never
    // produce — an earlier version of this test looked for "Claude Code" and
    // passed against Glasshouse's own error message, which is the whole
    // reason this asserts on something specific instead.
    let version = std::process::Command::new(claude.path())
        .arg("--version")
        .output()
        .ok()
        .and_then(|out| String::from_utf8(out.stdout).ok())
        .and_then(|text| text.split_whitespace().next().map(str::to_owned))
        .expect("the harness reports a version");
    assert!(
        version.starts_with(char::is_numeric),
        "unexpected version format: {version}"
    );

    // Nothing Glasshouse draws by itself carries the harness's version, so if
    // it is on screen before a session exists the assertion below would prove
    // nothing. An earlier revision of this test did exactly that.
    assert!(
        !strip_terminal_sequences(&shell.output()).contains(&version),
        "`{version}` is on screen before any session started, so matching it proves nothing"
    );

    shell.send("n");

    let deadline = Instant::now() + Duration::from_secs(45);
    let mut seen = false;
    while Instant::now() < deadline {
        shell.answer_pending_queries();
        // Spaces are collapsed out of the captured screen, so the version is
        // matched rather than any phrase around it.
        if strip_terminal_sequences(&shell.output()).contains(&version) {
            seen = true;
            break;
        }
        std::thread::sleep(POLL);
    }
    assert!(
        seen,
        "the harness's own version `{version}` never appeared in the viewport\n\
         --- screen ---\n{}\n--- end ---",
        strip_terminal_sequences(&shell.output())
    );

    shell.send("\x1d");
    shell.send("q");
    let _ = shell.wait_for_exit();
}
