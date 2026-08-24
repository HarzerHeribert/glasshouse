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
