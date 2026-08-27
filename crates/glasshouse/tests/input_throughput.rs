//! Typing into the interface must not be throttled to the tick.
//!
//! # The defect this exists for
//!
//! Between batch 26 and this one, a Glasshouse shell delivered **one keystroke
//! per 16ms tick** — about 59 characters a second, so a 200-character paste
//! took three and a half seconds and ordinary typing outran the interface.
//!
//! The cause is a disagreement between two correct components.
//! `tui::event::wait_for_terminal` waits on the terminal *descriptor*, and it
//! waits there before crossterm is consulted — deliberately, because a
//! terminal that has gone away is not something crossterm's own poll can
//! report or even survive. But crossterm does not read one byte at a time: it
//! drains whatever the descriptor had into a parse buffer of its own and hands
//! back one event per call. So once the first key of a burst has been
//! delivered, the rest of the burst is *inside crossterm* and the descriptor
//! is **empty** — and a level-triggered `poll(2)` on an empty descriptor
//! correctly reports nothing and sleeps out the whole remaining tick, with the
//! next keystroke sitting in the library the entire time.
//!
//! **Measured, not reasoned.** A probe logging `FIONREAD` on the descriptor
//! beside `crossterm::event::poll(Duration::ZERO)` on every pass of the wait
//! loop, through a twenty-key burst: *every* sample read `fionread=0`, and
//! nineteen consecutive samples had crossterm answering that an event was
//! ready on a descriptor the kernel called empty. The fix is
//! `EventSource::crossterm_may_hold_more` — while crossterm may still be
//! holding a burst, it is asked before the descriptor is waited on rather than
//! after.
//!
//! # Why this cannot be a unit test
//!
//! The throttle is a property of a real terminal descriptor, a real
//! level-triggered `poll(2)`, and a library's private buffer — none of which a
//! double has. Every real defect this event loop has had was found by running
//! the shipped binary on a real pty and none by a unit test, so this does the
//! same: it types at the binary and times how long the interface takes to act
//! on the last thing typed.
//!
//! # A number, not a pass
//!
//! The acceptance evidence is a rate from the same harness on both trees
//! (practice §34, §60). Measured on this development machine, one write of 200
//! filler keys followed by the key that opens the session overview, timed
//! until the overview is drawn:
//!
//! | tree | 201 keys delivered in | per key |
//! |---|---|---|
//! | before | 3.38s | 16.8ms |
//! | after | under 10ms | under 0.05ms |
//!
//! The before column is not a property of this machine: it is
//! [`BURST_KEYS`] + 1 ticks, and a tick is 16ms whatever the hardware, so the
//! unfixed tree cannot beat 3.2s however fast the machine. That is what makes
//! [`BURST_DEADLINE`] a floor rather than a timing guess — see its own comment.

#![cfg(unix)]

use std::os::fd::RawFd;
use std::time::{Duration, Instant};

use portable_pty::{CommandBuilder, MasterPty, PtySize, native_pty_system};

/// How long the shell gets to draw its first frame before this gives up.
/// Generous: a loaded container spawning a debug build is slow, and being slow
/// to start is not the thing under test.
const STARTUP_TIMEOUT: Duration = Duration::from_secs(30);

/// How long the interface is left alone before it is typed at.
///
/// Long enough for startup to be over and short enough to stay well inside
/// `tui::event`'s silence threshold, so the burst arrives at an interface in
/// the state a user leaves it in — drawn, interactive, and being typed at.
const SETTLE: Duration = Duration::from_millis(400);

/// How many filler keys go in before the one whose effect is watched for.
///
/// The packet's floor is 200 and this is the packet's floor. It is also about
/// what a pasted command line or a pasted path is, which is the case a user
/// meets: at one key per tick, this is three and a half seconds of a terminal
/// appearing to have hung.
const BURST_KEYS: usize = 200;

/// How long the whole burst gets to be acted on.
///
/// **A floor derived from the defect, not a timing guess.** The unfixed loop
/// delivers one key per `tui::DEFAULT_TICK`, and that tick is 16ms on every
/// machine, so `BURST_KEYS + 1` keys cannot be delivered in less than 3.2s
/// there — measured at 3.38s on this machine, and a *faster* machine does not
/// help, because what is being waited out is a sleep. The fixed loop delivers
/// the same burst in under 10ms here.
///
/// So a second sits three hundred times above what the fix costs and three and
/// a half times below what the defect costs, and neither margin is close. It
/// is deliberately not tighter: a loaded container is slow at everything
/// except sleeping for 16ms, which is the one thing it does at full speed.
const BURST_DEADLINE: Duration = Duration::from_secs(1);

/// How long the burst gets to be delivered at all before the trial is called a
/// failure rather than a slow pass.
///
/// Separate from [`BURST_DEADLINE`] so a tree that never delivers the keys is
/// reported as *that* rather than as a tree which was merely slow — and long
/// enough that the unfixed tree gets to finish and have its real number
/// quoted in the failure message.
const DELIVERY_DEADLINE: Duration = Duration::from_secs(30);

/// How many bursts are timed.
///
/// The throttle is not a race — it fired on every key of every burst, and the
/// unfixed tree's number is set by arithmetic rather than by scheduling — so
/// this is not §60's trial count. It is here because a *runner* is variable
/// even when the defect is not, and one sample cannot tell a fixed tree from a
/// lucky one. Every trial is asserted, and all of them are printed.
const TRIALS: usize = 3;

/// A column header the session overview draws and no other screen does.
///
/// Matched against the drawn text rather than an echoed byte: the interface is
/// in raw mode and echoes nothing, so the only evidence a key arrived is what
/// it made the interface do.
const OVERVIEW_HEADER: &str = "HARNESS";

/// The key that opens the session overview, sent last in the burst.
const OVERVIEW_KEY: u8 = b'o';

/// The filler the rest of the burst is made of, chosen because the shell's
/// default screen does nothing with it — so what is timed is the loop's
/// delivery rate and not the work each key causes.
const FILLER_KEY: u8 = b'x';

const KILL_DEADLINE: Duration = Duration::from_secs(10);
const READ_POLL: Duration = Duration::from_millis(1);
const EXIT_POLL: Duration = Duration::from_millis(25);

#[test]
fn a_burst_of_typing_is_not_delivered_one_key_per_tick() {
    let mut timings = Vec::new();
    for trial in 1..=TRIALS {
        timings.push(one_burst(trial));
    }
    // Printed rather than only asserted: the acceptance evidence for this file
    // is a number, and a reader of the gate log should not have to infer it
    // from `ok` (practice §60).
    println!(
        "{BURST_KEYS} filler keys + the overview key, delivered in: {}",
        timings
            .iter()
            .map(|took| format!("{:.1}ms", took.as_secs_f64() * 1000.0))
            .collect::<Vec<_>>()
            .join(", ")
    );
}

/// One trial: start the interface, let it settle, type a whole burst in one
/// write, and time how long it takes to act on the last key of it.
fn one_burst(trial: usize) -> Duration {
    let fixture = Fixture::new();
    let mut child = fixture.start_shell();
    child.wait_for_first_frame();

    let settled = Instant::now() + SETTLE;
    while Instant::now() < settled {
        child.drain();
        std::thread::sleep(READ_POLL);
    }
    let drawn_before = child.drawn_so_far();

    // One write, which is as fast as a pty accepts anything — a paste, or a
    // fast typist, or a script driving the terminal.
    let mut burst = vec![FILLER_KEY; BURST_KEYS];
    burst.push(OVERVIEW_KEY);
    let started = Instant::now();
    child.type_key(&burst);

    let deadline = started + DELIVERY_DEADLINE;
    while Instant::now() < deadline {
        child.drain();
        if child.drawn_since(drawn_before).contains(OVERVIEW_HEADER) {
            let took = started.elapsed();
            assert!(
                took <= BURST_DEADLINE,
                "trial {trial}: {} keys written in one go took {took:?} to be acted on — {:.1}ms \
                 a key. The interface is delivering roughly one keystroke per tick, which is a \
                 terminal that appears to have hung while somebody pastes into it.",
                burst.len(),
                took.as_secs_f64() * 1000.0 / burst.len() as f64,
            );
            return took;
        }
        assert!(
            child.try_wait().is_none(),
            "trial {trial}: glasshouse exited instead of answering the burst, so this run proves \
             nothing about how fast typing is delivered\n--- drawn since the burst ---\n{}\n\
             --- end ---",
            child.drawn_since(drawn_before)
        );
        std::thread::sleep(READ_POLL);
    }
    let drawn = child.drawn_since(drawn_before);
    child.kill();
    panic!(
        "trial {trial}: {} keys were written into the terminal and {DELIVERY_DEADLINE:?} later \
         the interface had still not acted on the last of them. Keys that reach nothing are the \
         user's typing, lost.\n--- drawn since the burst ---\n{drawn}\n--- end ---",
        burst.len()
    );
}

/// A project, a state directory and a user configuration, all thrown away with
/// the test.
struct Fixture {
    dir: tempfile::TempDir,
}

impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().expect("temp dir");
        for sub in ["project", "state", "config"] {
            std::fs::create_dir_all(dir.path().join(sub)).expect("fixture directory");
        }
        // Onboarding marked done, so the binary opens the session shell rather
        // than the first-run wizard. Both drive the same event source, but the
        // shell is the loop a user types into.
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
        // The same exact-snapshot environment the rest of this crate's pty
        // tests build: portable-pty's own defaults are not this process's
        // environment.
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
            // No cloned reader: on Unix that is a `dup` of the master, and a
            // reader thread holding one would keep the child's terminal alive
            // for as long as it lived. Output is drained here, non-blocking,
            // on this thread.
            master: Some(pair.master),
            fd,
            child,
            output: Vec::new(),
        }
    }
}

struct Shell {
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
            // or a broken pty. None of the three is something to wait on here.
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
                     say anything about how fast typing is delivered if the interface never came \
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

    /// Type at the terminal, exactly as a person or a paste does: the bytes go
    /// in at the master and come out of the child's standard input.
    fn type_key(&mut self, keys: &[u8]) {
        assert!(
            self.master.is_some(),
            "the terminal has been taken away; there is nothing to type at"
        );
        // SAFETY: `keys` is a live slice and its length is passed alongside
        // it. The descriptor is owned by the `MasterPty` this struct holds,
        // which is still alive (asserted above).
        let written =
            unsafe { libc::write(self.fd, keys.as_ptr().cast::<libc::c_void>(), keys.len()) };
        assert_eq!(
            usize::try_from(written).unwrap_or(0),
            keys.len(),
            "could not put {} keys into the terminal in one write",
            keys.len()
        );
    }

    fn try_wait(&mut self) -> Option<portable_pty::ExitStatus> {
        self.child.try_wait().expect("try_wait")
    }

    /// End the child now, and do not come back until it is reaped.
    ///
    /// `SIGKILL` rather than `portable_pty::Child::kill`, which sends
    /// `SIGHUP`: every caller is on a failure path or a scope exit, and the
    /// drain below is load-bearing — a process blocked writing its last frame
    /// to a pty nobody is reading cannot finish dying, and this thread is the
    /// only reader there is.
    fn kill(&mut self) {
        if let Some(pid) = self.child.process_id() {
            // SAFETY: `kill` takes no pointers. A pid that has already been
            // reaped fails with `ESRCH`, which is not an error here.
            unsafe {
                libc::kill(pid.cast_signed(), libc::SIGKILL);
            }
        }
        let deadline = Instant::now() + KILL_DEADLINE;
        while Instant::now() < deadline {
            if self.try_wait().is_some() {
                return;
            }
            self.drain();
            std::thread::sleep(EXIT_POLL);
        }
    }

    fn output(&self) -> String {
        String::from_utf8_lossy(&self.output).into_owned()
    }

    /// How much has been drawn so far, as a mark to pass to
    /// [`Shell::drawn_since`].
    fn drawn_so_far(&self) -> usize {
        self.output.len()
    }

    /// Everything drawn since `mark`.
    ///
    /// Counted and sliced in *bytes*, which is why this exists rather than the
    /// caller indexing `output()`: that is a lossy conversion of a stream read
    /// in 4096-byte pieces, and a piece ending mid-character moves every byte
    /// offset after it.
    fn drawn_since(&self, mark: usize) -> String {
        String::from_utf8_lossy(&self.output[mark.min(self.output.len())..]).into_owned()
    }
}

impl Drop for Shell {
    fn drop(&mut self) {
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
                    if ('\x40'..='\x7e').contains(&c) {
                        break;
                    }
                }
            }
            // OSC: runs until a bell or a string terminator.
            Some(']') => {
                while let Some(c) = chars.next() {
                    if c == '\x07' {
                        break;
                    }
                    if c == '\x1b' && chars.peek() == Some(&'\\') {
                        chars.next();
                        break;
                    }
                }
            }
            // Anything else is a two-byte sequence and is already consumed.
            _ => {}
        }
    }
    out
}
