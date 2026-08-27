//! The interface must end when its terminal does.
//!
//! # The defect this exists for
//!
//! Three `glasshouse` processes were found on a developer's machine nineteen
//! hours after the terminals that started them had closed, each burning a
//! whole core, the machine at 501% CPU. A 1622-sample profile put every
//! sample in one `read` on the terminal descriptor.
//!
//! A descriptor whose far end has gone away is *permanently readable and
//! returns zero bytes*. Crossterm's Unix event source reacts to a readable
//! terminal by looping on `read` until it yields an event; zero bytes is
//! never an event, so `try_read` never returns, so `poll` never returns, and
//! `EventSource::next` never returns to the shutdown check at the top of
//! itself. A signal had already asked one of those processes to stop. It
//! never found out.
//!
//! # Why this cannot be a unit test
//!
//! The condition is a property of a real pseudo-terminal — the kernel's, not
//! a double's — and the loop that hangs is inside a library, reached only
//! through a running process. So this starts the shipped binary on a real
//! pty, closes the master, and watches what the process does. Nothing short
//! of that would have caught the defect, and nothing short of that proves it
//! is gone.
//!
//! # Its own file, deliberately
//!
//! Not in `pty_smoke.rs`, which carries a known Linux flake under load
//! (practice §34). A new proof should not inherit an old reputation: when
//! this file fails it should be believed.
//!
//! # A related exit-code defect proved elsewhere, on purpose
//!
//! `Screen::drop` (`tui::mod`) can still panic on the way out here — Ratatui's
//! own `Drop for Terminal` writes to show a cursor it left hidden, and panics
//! if that write fails, which it does once the terminal is gone (packet
//! HANGUP-FOLLOWUP, defect 2). The test below deliberately does not check an
//! exit code because of it.
//!
//! A real end-to-end proof of the fix was attempted here first and pulled
//! back out: closing a real terminal also delivers `SIGHUP`, which races
//! `crate::shutdown`'s own signal handling against the clean-shutdown path
//! this file's test proves — a pre-existing defect, independent of both of
//! this packet's, reported rather than fixed. On this development machine
//! that race decided anywhere from roughly half to (in a container, every
//! single time observed) *all* attempts, so an end-to-end test of the
//! exit-code fix cannot be made to fail reliably on the unfixed tree here.
//! `tui::mod`'s own tests prove it instead, directly against the drop
//! mechanism, without a real terminal, pty, or signal in the way.

#![cfg(unix)]

use std::os::fd::RawFd;
use std::time::{Duration, Instant};

use portable_pty::{CommandBuilder, MasterPty, PtySize, native_pty_system};

/// How long the shell gets to draw its first frame before this gives up.
/// Generous: a loaded container spawning a debug build is slow, and being
/// slow to start is not the thing under test.
const STARTUP_TIMEOUT: Duration = Duration::from_secs(30);

/// How long the process gets to be gone after its terminal is.
///
/// The interface's tick is 16ms and the wind-down is a return through two
/// stack frames, so the honest budget is milliseconds; this is three orders
/// of magnitude of slack for a loaded runner. It is also the wall-clock half
/// of the CPU claim below — a process that exits inside this window cannot
/// have burned more than this window's worth of CPU, whatever the sampler
/// managed to observe.
const HANGUP_DEADLINE: Duration = Duration::from_secs(5);

/// How much processor time the process may consume between losing its
/// terminal and exiting.
///
/// The defect's signature is not "exits late", it is "runs flat out": the
/// field processes accumulated 19 hours of CPU in 19 hours of wall clock. A
/// wind-down that returns through a loop and drops a database handle costs
/// far less than this.
const MAX_CPU_AFTER_HANGUP: f64 = 0.5;

/// How long the interface is left alone, drawn and idle, before its terminal
/// is taken away.
///
/// **Not padding, and measured rather than guessed.** Without it this test
/// closed the master about 30ms after the first frame, and on Linux that
/// passed whether or not the fix was present: a Glasshouse still finishing
/// its startup turns the hangup into an error that propagates out of the
/// interface and ends the process by another route entirely, at 0.01s of CPU.
/// Left to settle for this long first, the same tree without the fix burns
/// 5.01s of processor time in 5.02s of wall clock — the defect, on Linux, in
/// this test. The field processes were idle for nineteen hours; an idle
/// interface is the state under test.
const SETTLE: Duration = Duration::from_millis(1500);

const EXIT_POLL: Duration = Duration::from_millis(25);
const READ_POLL: Duration = Duration::from_millis(10);
/// How often the child's processor time is sampled while it is still alive.
/// Sampling costs a `/proc` read or a `ps`, so it is deliberately much slower
/// than the exit poll — the exit is what this test is usually waiting for.
const CPU_SAMPLE_INTERVAL: Duration = Duration::from_millis(250);

#[test]
fn a_terminal_that_goes_away_ends_the_interface_instead_of_spinning() {
    let fixture = Fixture::new();
    let mut child = fixture.start_shell();

    // Prove the interface is actually up before taking its terminal away.
    // Without this the test could be passed by a Glasshouse that exited on
    // startup for some unrelated reason — which is exactly the failure a
    // bare "did it exit?" assertion cannot tell from success.
    child.wait_for_first_frame();

    // Let it reach the state the field processes were in: drawn, idle, and
    // doing nothing but waiting for input. See `SETTLE`.
    let settled = Instant::now() + SETTLE;
    while Instant::now() < settled {
        child.drain();
        std::thread::sleep(READ_POLL);
    }

    // Drawing a frame proves it *was* alive; this proves it still is. Without
    // it, a Glasshouse that drew one frame and then quit for its own reasons
    // would be indistinguishable from one that quit because its terminal went
    // away — the test would pass while measuring nothing.
    assert!(
        child.try_wait().is_none(),
        "glasshouse had already exited before its terminal was taken away, so this run \
         proves nothing about losing a terminal\n--- output ---\n{}\n--- end ---",
        child.output()
    );

    let cpu_before = child.cpu_seconds();
    let hangup = Instant::now();
    child.close_terminal();

    let mut cpu_after = cpu_before;
    let mut next_sample = hangup + CPU_SAMPLE_INTERVAL;
    let deadline = hangup + HANGUP_DEADLINE;
    loop {
        if let Some(status) = child.try_wait() {
            let took = hangup.elapsed();
            // Not asserting an exit *code*. The interface returns normally,
            // but on the way out Ratatui's `Terminal` drop tries to show the
            // cursor and prints to standard error when it cannot — and on a
            // terminal that has gone away, printing to standard error is
            // itself a panic. That is Ratatui's business and it happens after
            // this loop has already returned; the process is gone either way,
            // which is what was in question.
            let cpu = cpu_after
                .zip(cpu_before)
                .map(|(after, before)| after - before);
            assert!(
                cpu.is_none_or(|burned| burned <= MAX_CPU_AFTER_HANGUP),
                "the interface exited ({status:?}) after {took:?}, but burned {:?}s of \
                 processor time doing it — a wind-down should cost almost none",
                cpu.unwrap_or_default(),
            );
            return;
        }

        if Instant::now() >= next_sample {
            next_sample += CPU_SAMPLE_INTERVAL;
            if let Some(sampled) = child.cpu_seconds() {
                cpu_after = Some(sampled);
            }
        }

        if Instant::now() >= deadline {
            let cpu = cpu_after
                .zip(cpu_before)
                .map(|(after, before)| after - before);
            let burned = match cpu {
                Some(seconds) => format!(
                    "{seconds:.2}s of processor time in {:.2}s of wall clock",
                    hangup.elapsed().as_secs_f64()
                ),
                None => "an unmeasurable amount of processor time".to_owned(),
            };
            child.kill();
            panic!(
                "the interface was still running {HANGUP_DEADLINE:?} after its terminal \
                 went away, having burned {burned}. A terminal that is gone is an \
                 instruction to stop.\n--- what it had drawn ---\n{}\n--- end ---",
                child.output()
            );
        }

        std::thread::sleep(EXIT_POLL);
    }
}

/// A project, a state directory and a user configuration, all thrown away
/// with the test.
struct Fixture {
    dir: tempfile::TempDir,
}

impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().expect("temp dir");
        for sub in ["project", "state", "config"] {
            std::fs::create_dir_all(dir.path().join(sub)).expect("fixture directory");
        }
        // Onboarding marked done, so the binary opens the session shell
        // rather than the first-run wizard. Both drive the same event source
        // — one `EventSource::new(DEFAULT_TICK)` each — but the shell is the
        // loop the field sample was taken in.
        std::fs::write(
            dir.path().join("config").join("config.toml"),
            "version = 1\n\n[onboarding]\ncompleted = true\n",
        )
        .expect("write user config");
        Self { dir }
    }

    fn start_shell(&self) -> Shell {
        let pair = native_pty_system()
            .openpty(PtySize {
                rows: 40,
                cols: 120,
                pixel_width: 0,
                pixel_height: 0,
            })
            .expect("open pty");

        let mut command = CommandBuilder::new(env!("CARGO_BIN_EXE_glasshouse"));
        // The same exact-snapshot environment `pty::TerminalCommand` builds,
        // for the same reason: portable-pty's own defaults are not this
        // process's environment.
        command.env_clear();
        for (key, value) in std::env::vars_os() {
            command.env(key, value);
        }
        command.cwd(self.dir.path());
        command.args([
            "--scope",
            &self.dir.path().join("project").display().to_string(),
            "--data-dir",
            &self.dir.path().join("state").display().to_string(),
            "--config-dir",
            &self.dir.path().join("config").display().to_string(),
        ]);

        let child = pair.slave.spawn_command(command).expect("spawn glasshouse");
        // The last slave descriptor this process holds. Keeping it would keep
        // the terminal half-alive from the child's point of view.
        drop(pair.slave);

        let fd = pair
            .master
            .as_raw_fd()
            .expect("a Unix pty master has a descriptor");
        set_nonblocking(fd);

        Shell {
            // **No cloned reader, and that is the whole trick.** On Unix a
            // cloned reader is a `dup` of the master descriptor, and the
            // child's terminal is gone only when *every* master descriptor is
            // closed. A reader thread holding a dup would block in `read`
            // forever waiting for an end-of-file that its own descriptor was
            // preventing — the test would deadlock instead of hanging up.
            // So output is drained here, non-blocking, on this thread.
            master: Some(pair.master),
            fd,
            child,
            output: Vec::new(),
        }
    }
}

struct Shell {
    /// `None` once the terminal has been taken away.
    master: Option<Box<dyn MasterPty + Send>>,
    fd: RawFd,
    child: Box<dyn portable_pty::Child + Send + Sync>,
    output: Vec<u8>,
}

impl Shell {
    /// Read whatever the child has written, without waiting for more.
    fn drain(&mut self) {
        if self.master.is_none() {
            return;
        }
        let mut buffer = [0u8; 4096];
        loop {
            // SAFETY: `buffer` is a live, initialised array and its length is
            // passed alongside it. The descriptor is owned by the `MasterPty`
            // this struct holds, which is still alive (checked above).
            let read = unsafe {
                libc::read(
                    self.fd,
                    buffer.as_mut_ptr().cast::<libc::c_void>(),
                    buffer.len(),
                )
            };
            if read > 0 {
                #[allow(clippy::cast_sign_loss)]
                self.output.extend_from_slice(&buffer[..read as usize]);
                continue;
            }
            // Zero is end-of-file and a negative is either "nothing waiting"
            // or a broken pty. None of the three is something to wait on
            // here; the caller's deadline decides what happens next.
            return;
        }
    }

    /// Wait until the interface has drawn itself, or fail saying what it did
    /// instead.
    fn wait_for_first_frame(&mut self) {
        let banner = format!("glasshouse {}", glasshouse::VERSION);
        let deadline = Instant::now() + STARTUP_TIMEOUT;
        while Instant::now() < deadline {
            self.drain();
            if strip_terminal_sequences(&self.output()).contains(&banner) {
                return;
            }
            if let Some(status) = self.try_wait() {
                panic!(
                    "glasshouse exited ({status:?}) before drawing anything — this test cannot \
                     say anything about a terminal going away if the interface never came \
                     up.\n--- output ---\n{}\n--- end ---",
                    self.output()
                );
            }
            std::thread::sleep(READ_POLL);
        }
        panic!(
            "glasshouse never drew {banner:?} within {STARTUP_TIMEOUT:?}\n--- output ---\n{}\n\
             --- end ---",
            self.output()
        );
    }

    /// Take the terminal away, exactly as closing the pane that started it
    /// does: every master descriptor closed, and no signal sent.
    ///
    /// **No signal on purpose.** Signals are not the gap here — the field
    /// processes all died on `SIGTERM`, so Glasshouse's signal handling works
    /// fine. The hole is the hangup nobody announces, and sending anything
    /// would test the path that already worked.
    fn close_terminal(&mut self) {
        self.drain();
        drop(self.master.take());
    }

    fn try_wait(&mut self) -> Option<portable_pty::ExitStatus> {
        self.child.try_wait().expect("try_wait")
    }

    fn kill(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }

    fn output(&self) -> String {
        String::from_utf8_lossy(&self.output).into_owned()
    }

    /// Processor time the child has used so far, in seconds, or `None` where
    /// this platform will not say.
    fn cpu_seconds(&self) -> Option<f64> {
        self.child.process_id().and_then(cpu_seconds_of)
    }
}

impl Drop for Shell {
    fn drop(&mut self) {
        // A failed assertion leaves this scope by unwinding, and a process
        // spinning at 100% CPU is precisely what must not be left behind by a
        // test *about* processes spinning at 100% CPU.
        if self.try_wait().is_none() {
            self.kill();
        }
    }
}

fn set_nonblocking(fd: RawFd) {
    // SAFETY: `fd` is owned by a live `MasterPty`; both calls only read and
    // replace its status flags.
    unsafe {
        let flags = libc::fcntl(fd, libc::F_GETFL);
        assert!(flags >= 0, "could not read the pty master's flags");
        assert!(
            libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) >= 0,
            "could not make the pty master non-blocking"
        );
    }
}

/// Processor time used by `pid`, in seconds.
///
/// Linux is read straight out of `/proc` because a `rust` container has no
/// `ps`; everywhere else `ps` is the portable answer. `None` where neither
/// works, which weakens the measurement to the wall-clock bound rather than
/// failing a test over its own instrument.
#[cfg(target_os = "linux")]
fn cpu_seconds_of(pid: u32) -> Option<f64> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    // The second field is the executable name in parentheses and may contain
    // spaces and parentheses of its own, so fields are counted from after the
    // last `)` rather than from the start.
    let rest = &stat[stat.rfind(')')? + 1..];
    let fields: Vec<&str> = rest.split_whitespace().collect();
    // `rest` starts at field 3 (state), so utime and stime — fields 14 and 15
    // — are at offsets 11 and 12.
    let utime: f64 = fields.get(11)?.parse().ok()?;
    let stime: f64 = fields.get(12)?.parse().ok()?;
    // SAFETY: `sysconf` reads a system constant and takes no pointers.
    let ticks = unsafe { libc::sysconf(libc::_SC_CLK_TCK) };
    if ticks <= 0 {
        return None;
    }
    #[allow(clippy::cast_precision_loss)]
    Some((utime + stime) / ticks as f64)
}

#[cfg(not(target_os = "linux"))]
fn cpu_seconds_of(pid: u32) -> Option<f64> {
    let out = std::process::Command::new("ps")
        .args(["-o", "time=", "-p", &pid.to_string()])
        .output()
        .ok()?;
    parse_ps_time(String::from_utf8_lossy(&out.stdout).trim())
}

/// Parse `ps -o time=`, which prints `MM:SS.ss`, `HH:MM:SS` or a mix of the
/// two depending on how long the process has been running.
#[cfg(not(target_os = "linux"))]
fn parse_ps_time(text: &str) -> Option<f64> {
    if text.is_empty() {
        return None;
    }
    let mut seconds = 0.0;
    for (place, part) in text.rsplit(':').enumerate() {
        let value: f64 = part.parse().ok()?;
        seconds += value * 60_f64.powi(i32::try_from(place).ok()?);
    }
    Some(seconds)
}

/// Remove ANSI escape sequences so a match is against what was drawn rather
/// than against the cursor moves that drew it.
fn strip_terminal_sequences(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\x1b' {
            out.push(c);
            continue;
        }
        match chars.next() {
            // CSI: parameters and intermediates, then a final byte.
            Some('[') => {
                for c in chars.by_ref() {
                    if ('\u{40}'..='\u{7e}').contains(&c) {
                        break;
                    }
                }
            }
            // OSC: a payload terminated by BEL or by ST (`ESC \`).
            Some(']') => {
                while let Some(c) = chars.next() {
                    if c == '\u{7}' {
                        break;
                    }
                    if c == '\x1b' && chars.peek() == Some(&'\\') {
                        chars.next();
                        break;
                    }
                }
            }
            // Anything else is a two-byte sequence; both bytes are dropped.
            _ => {}
        }
    }
    out
}
