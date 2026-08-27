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
//! # A rate, not a pass
//!
//! The first version of this file ran the scenario **once**. It passed on
//! every run, on both platforms, and three mutations killed it — and the
//! process was still spinning about one hangup in thirty. A separate harness
//! running the same scenario sixty times caught it twice, at `Rs+ 100.0` with
//! cumulative processor time equal to the process's whole lifetime.
//!
//! Nothing about the test was wrong; it simply cannot tell "fixed" from
//! "fixed 97% of the time", because a one-shot pass is consistent with a
//! residual rate of anything up to roughly one in three. So it now runs
//! [`TRIALS`] of them, and the rate measured by hand — too many trials to
//! afford in a gate — is recorded here rather than left to be inferred from
//! `ok`:
//!
//! | tree | survivors |
//! |---|---|
//! | before, by an earlier harness | 2 in 60 |
//! | before, measured again | 7 in 200 |
//! | before, through this test's own scenario | 2 in 120 |
//! | after | 0 in 400 |
//!
//! `0 in 400` bounds a residual rate below roughly 0.75% with 95% confidence.
//! It does **not** establish zero, and this file does not claim one.
//!
//! The mechanism behind those numbers is in `tui::event::wait_for_terminal`'s
//! doc comment: an idle interface spent about 4% of every tick inside a call
//! to crossterm it did not need to make, and a hangup landing inside that call
//! wedged crossterm exactly as before. It now makes no such call once the
//! terminal has been quiet — 0 samples of 6185, against 268 of 6210.
//!
//! # A second defect in the same loop, with its own rate
//!
//! `a_resize_does_not_swallow_the_keystroke_that_follows_it` is about a
//! different failure of the same event source, found while the one above was
//! being fixed and present on every tree before this one. Crossterm watches
//! the terminal and `SIGWINCH` through a single edge-triggered `mio`
//! registration and returns on the first of the two it looks at, throwing the
//! other's readiness away unread. A keystroke arriving while a resize is
//! waiting to be collected is therefore *discarded* — it stays on the
//! descriptor, invisible to crossterm until the user presses something else,
//! while the loop asks a level-triggered `poll` and an edge-triggered library
//! the same question forever and burns processor time doing it.
//!
//! Measured by hand with a harness that resizes the terminal and then types
//! into it, one process per trial, `--gap-ms` apart:
//!
//! | gap | before | after |
//! |---|---|---|
//! | 4ms | 27 in 60 | 0 in 60 |
//! | 50µs | — | 0 in 60 |
//! | back to back | 15 in 60 | 11 in 60 |
//!
//! So the window narrowed from about one tick to under fifty microseconds,
//! and what is left is the case where the two genuinely arrive together —
//! crossterm's to fix, and unreachable from this side of it. A process that
//! does hit it now costs 0.3% of a core rather than 23.9%, which is the other
//! half of the fix and is measured in `tui::event::Watch`'s own comment.
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

/// How many times the scenario is run.
///
/// **The point of the number, not padding.** The defect this file exists for
/// survived a single-trial version of it for a whole batch, because it fires
/// on a fraction of hangups rather than on all of them (see the rate table
/// above). N trials find a residual rate `p` with probability `1 - (1-p)^N`,
/// so this many catch a 1-in-3 every time, a 1-in-10 about four times in five,
/// and a 1-in-30 about two times in five. **Measured against the real thing**,
/// by deleting the fix and running this test sixteen times: it failed twice.
/// That is not enough on its own — which is why the by-hand `0 in 400` is
/// recorded above — but the gate runs the suite four times (two platforms, two
/// Rust versions), so 60 hangups per gate run makes a reverted fix far likelier
/// to be caught than missed.
///
/// The cost is roughly [`SETTLE`] plus a process start per trial, and `SETTLE`
/// cannot be shortened without testing a different scenario entirely — see its
/// own comment.
const TRIALS: usize = 15;

/// How long the interface is left alone before a resize is aimed at it in
/// `a_resize_does_not_swallow_the_keystroke_that_follows_it`.
///
/// Deliberately **shorter** than [`SETTLE`], and for the opposite reason.
/// That test wants the state a closed window leaves behind, which is silence.
/// This one wants the state a user is in — drawn, interactive, and about to
/// type — so it waits only long enough for startup to be over. Both reproduce
/// the same collision, because the window it needs is one tick wide either
/// way; this is the cheaper of the two and the one a person would recognise.
const TYPING_SETTLE: Duration = Duration::from_millis(400);

/// How long the keystroke gets to reach the interface after the resize.
///
/// An honest budget is one tick. This is three hundred of them, because a
/// stranded keystroke is stranded *until the next one* — no amount of waiting
/// turns a failure into a pass here, so the slack costs nothing but patience
/// on a loaded runner.
const KEYSTROKE_DEADLINE: Duration = Duration::from_secs(5);

/// How long after the resize the keystroke is sent, cycled trial by trial.
///
/// **Measured, not chosen for roundness.** The collision needs the `SIGWINCH`
/// to be sitting in crossterm's pipe when the keystroke lands, so the gap has
/// to be long enough for the signal to have been delivered and short enough
/// that the interface has not yet drained it. Against the unfixed tree the
/// stall rate by gap was 2 in 20 at no gap, 13 in 20 at 2ms, 15 in 20 at 4ms,
/// 8 in 20 at 8ms and 0 in 20 at 16ms — one tick, after which the pipe has
/// always been drained. The first of these sits on that peak.
///
/// **The second is here because a whole line of the fix is invisible at the
/// first, and it is also where the residual lives.** `wait_for_terminal`
/// passes an exactly-zero timeout straight through to `poll` instead of
/// rounding it up to a millisecond; rounding it up again reopens a
/// millisecond-wide window that a keystroke four milliseconds behind the
/// resize sails straight past. Measured: with that line reverted the harness
/// stalls **20 in 20** at half a millisecond and 2 in 20 at one.
///
/// But half a millisecond is also inside what is left of the defect, because
/// what is left is however long this process takes to be scheduled between the
/// signal and the poll — which on a loaded runner is not microseconds. Hence
/// [`MAX_STALLS`], and hence the two gaps being judged separately rather than
/// pooled.
const RESIZE_TO_KEYSTROKE: [Duration; 2] = [Duration::from_millis(4), Duration::from_micros(500)];

/// How many swallowed keystrokes each of [`RESIZE_TO_KEYSTROKE`]'s gaps
/// tolerates, index for index.
///
/// **Both numbers are measurements, and they differ because the two gaps ask
/// different questions.**
///
/// At 4ms the question is whether the fix is present at all, and the answer is
/// not supposed to be probabilistic: **0 stalls in 60 trials on a quiet
/// machine, and 0 in 48 with the rest of this file's tests running beside
/// them**. The 1 is slack for a runner slower than any seen here, not an
/// expectation.
///
/// At 500µs the question is whether one rounding line is still there, and the
/// answer has to be read past a residual this test cannot remove: 1 stall in
/// 96 trials of that gap under the same load, plus one in a full gate run.
/// Three is comfortably above that and far below the **8 in 8** the reverted
/// line produces, so the line stays proved without the residual failing a
/// gate.
const MAX_STALLS: [usize; 2] = [1, 3];

/// How many resize-then-type trials `a_resize_does_not_swallow_the_keystroke_
/// that_follows_it` runs.
///
/// **The number is the proof, not the loop.** The defect is a race and a
/// single trial cannot tell "fixed" from "fixed most of the time" (practice
/// §60). It fired in 27 of 60 trials on the unfixed tree at the first of
/// [`RESIZE_TO_KEYSTROKE`]'s gaps, so the eight trials that use that gap catch
/// a reverted fix about **94%** of the time on their own — `1 - 0.55^8 -
/// 8 * 0.45 * 0.55^7` — before the eight at the shorter gap add anything, and
/// the gate runs the suite four times. Even, so each gap gets half.
///
/// The by-hand rate, too many trials to afford here, is in this file's own
/// documentation above.
const RESIZE_TRIALS: usize = 16;

/// The size the terminal is changed to in the resize test, chosen only to be
/// different in both directions from the 40x120 it starts at.
const RESIZED_ROWS: u16 = 50;
const RESIZED_COLS: u16 = 100;

/// A column header the session overview draws and no other screen does.
///
/// Matched against the drawn text rather than an echoed byte: the interface is
/// in raw mode and echoes nothing, so the only evidence a key arrived is what
/// it made the interface do.
const OVERVIEW_HEADER: &str = "HARNESS";

/// How long `Shell::kill` keeps draining and reaping before it gives up.
///
/// See that method: the drain is the load-bearing half and this only stops it
/// being infinite.
const KILL_DEADLINE: Duration = Duration::from_secs(10);

const EXIT_POLL: Duration = Duration::from_millis(25);
const READ_POLL: Duration = Duration::from_millis(10);
/// How often the child's processor time is sampled while it is still alive.
/// Sampling costs a `/proc` read or a `ps`, so it is deliberately much slower
/// than the exit poll — the exit is what this test is usually waiting for.
const CPU_SAMPLE_INTERVAL: Duration = Duration::from_millis(250);

#[test]
fn a_terminal_that_goes_away_ends_the_interface_instead_of_spinning() {
    for trial in 1..=TRIALS {
        one_terminal_loss(trial);
    }
}

/// One trial: start the interface, let it settle, take its terminal away, and
/// require it gone without having burned processor time getting there.
///
/// `trial` appears in every failure message. A race that fails on the eleventh
/// run and not the first is worth being able to say so about.
fn one_terminal_loss(trial: usize) {
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
        "trial {trial}: glasshouse had already exited before its terminal was taken \
         away, so this run proves nothing about losing a terminal\n--- output ---\n{}\n--- end ---",
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
                "trial {trial}: the interface exited ({status:?}) after {took:?}, but burned \
                 {:?}s of processor time doing it — a wind-down should cost almost none",
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
                "trial {trial}: the interface was still running {HANGUP_DEADLINE:?} after its \
                 terminal went away, having burned {burned}. A terminal that is gone is an \
                 instruction to stop.\n--- what it had drawn ---\n{}\n--- end ---",
                child.output()
            );
        }

        std::thread::sleep(EXIT_POLL);
    }
}

/// The other half of the residual-spin fix, and the only thing that keeps it
/// from swallowing something.
///
/// That fix stops asking crossterm anything once the terminal has been silent
/// for about a second, because asking is what exposes the process to a hangup.
/// **But a window resize is not something the terminal says.** It arrives as
/// `SIGWINCH`, on a pipe of crossterm's own that only crossterm's poll
/// watches — so a loop that has stopped polling crossterm has also stopped
/// hearing about resizes, and an interface that has been idle for a second is
/// exactly the one a user is about to drag the corner of.
///
/// This is deterministic where the test above is a rate: the interface is left
/// alone well past the silence threshold, then its terminal is made a
/// different size, and it must redraw itself at that size. Without the watch
/// that notices, it redraws nothing at all — verified by mutation, not by
/// reasoning.
#[test]
fn a_resize_still_arrives_on_a_terminal_that_has_been_silent() {
    let fixture = Fixture::new();
    let mut child = fixture.start_shell();
    child.wait_for_first_frame();

    // Past `tui::event`'s silence threshold, which is what puts the interface
    // in the state where it is no longer consulting crossterm at all. Resizing
    // straight after the first frame would exercise the ordinary path and
    // prove nothing — the same trap `SETTLE` records for the test above.
    let settled = Instant::now() + SETTLE;
    while Instant::now() < settled {
        child.drain();
        std::thread::sleep(READ_POLL);
    }
    let drawn_before = child.drawn_so_far();

    child.resize(RESIZED_ROWS, RESIZED_COLS);

    // The interface draws its bottom row last, so a cursor move to the new
    // bottom row is proof it laid itself out at the new size rather than
    // merely repainting. Matched against the raw bytes on purpose: this is
    // about the geometry it drew at, which is carried by the escape sequences
    // and not by the text they place.
    let moved_to_new_bottom_row = format!("\x1b[{RESIZED_ROWS};");
    let deadline = Instant::now() + HANGUP_DEADLINE;
    while Instant::now() < deadline {
        child.drain();
        if child
            .drawn_since(drawn_before)
            .contains(&moved_to_new_bottom_row)
        {
            return;
        }
        std::thread::sleep(READ_POLL);
    }
    let after = child.drawn_since(drawn_before);
    child.kill();
    panic!(
        "the terminal was made {RESIZED_COLS}x{RESIZED_ROWS} and {HANGUP_DEADLINE:?} later the \
         interface had still never drawn a row {RESIZED_ROWS}. A resize that reaches nothing is \
         an interface drawing itself at a size its terminal no longer is.\n--- drawn since the \
         resize ---\n{after}\n--- end ---"
    );
}

/// A resize must not eat the key pressed just after it.
///
/// # The defect
///
/// Crossterm learns about the terminal and about `SIGWINCH` through one
/// edge-triggered `mio` registration, and its reader returns on the first of
/// the two it looks at — abandoning, unread, whatever readiness arrived in the
/// same batch. Look at the signal first and the keystroke that arrived with it
/// is discarded: the bytes stay on the descriptor, crossterm cannot see them
/// again until new input creates a new edge, and the interface meanwhile has a
/// descriptor its own `poll` calls readable and a library that says there is
/// nothing on it. It burns processor time in that state for as long as it
/// lasts — 23.9% of a core here against 0.3% idle, and a whole one on a tree
/// that consults crossterm less often — with the user's command sitting unread
/// in front of it. `tui::event`'s `Watch` carries the
/// mechanism and the numbers.
///
/// # Why it is written this way
///
/// The keystroke is `o`, which opens the session overview, because the
/// overview draws a header row no other screen does — so the assertion is
/// about what the interface *did*, not about what it echoed. The gaps between
/// the resize and the key are [`RESIZE_TO_KEYSTROKE`], two of them because one
/// line of the fix is only visible at the shorter; the trial count is
/// [`RESIZE_TRIALS`] and what each gap tolerates is [`MAX_STALLS`]. All three
/// carry their measurements in their own comments.
///
/// **It counts rather than stopping at the first stall**, because what it is
/// measuring is a rate (practice §60) and a residual it cannot remove lives at
/// the shorter gap. A test that failed on any single stall would be right
/// about the mechanism and wrong about this tree about one gate run in six —
/// measured, by running it eight times: one stall in 96 trials, at 500µs.
///
/// It runs against the real binary on a real pty for the same reason the test
/// above does: the collision is a property of the kernel's readiness
/// reporting, and no double has one.
#[test]
fn a_resize_does_not_swallow_the_keystroke_that_follows_it() {
    let mut stalls: [Vec<usize>; 2] = [Vec::new(), Vec::new()];
    let mut last_screen = String::new();
    for trial in 1..=RESIZE_TRIALS {
        let which = trial % RESIZE_TO_KEYSTROKE.len();
        if let Some(drawn) = one_resize_then_keystroke(trial, RESIZE_TO_KEYSTROKE[which]) {
            stalls[which].push(trial);
            last_screen = drawn;
        }
    }
    // Judged gap by gap rather than pooled: see `MAX_STALLS` for why one of
    // them asks about the fix and the other about a single line of it.
    for (which, swallowed) in stalls.iter().enumerate() {
        assert!(
            swallowed.len() <= MAX_STALLS[which],
            "{} of the {} keystrokes sent {:?} after a resize were swallowed by it, which is \
             more than the {} that gap tolerates — trials {swallowed:?}. A keystroke that \
             reaches nothing is a command the user typed and lost.\n--- what the last \
             swallowed trial had drawn since its resize ---\n{last_screen}\n--- end ---",
            swallowed.len(),
            RESIZE_TRIALS / RESIZE_TO_KEYSTROKE.len(),
            RESIZE_TO_KEYSTROKE[which],
            MAX_STALLS[which],
        );
    }
}

/// One trial. `None` when the keystroke arrived; the screen drawn since the
/// resize when it did not.
fn one_resize_then_keystroke(trial: usize, gap: Duration) -> Option<String> {
    let fixture = Fixture::new();
    let mut child = fixture.start_shell();
    child.wait_for_first_frame();

    let settled = Instant::now() + TYPING_SETTLE;
    while Instant::now() < settled {
        child.drain();
        std::thread::sleep(READ_POLL);
    }
    let drawn_before = child.drawn_so_far();

    // Exactly what a window manager dragging a corner does, and then exactly
    // what a person does next.
    child.resize(RESIZED_ROWS, RESIZED_COLS);
    std::thread::sleep(gap);
    child.type_key(b"o");

    let deadline = Instant::now() + KEYSTROKE_DEADLINE;
    while Instant::now() < deadline {
        child.drain();
        if child.drawn_since(drawn_before).contains(OVERVIEW_HEADER) {
            return None;
        }
        assert!(
            child.try_wait().is_none(),
            "trial {trial}: glasshouse exited instead of answering the keystroke, so this run \
             proves nothing about a keystroke arriving\n--- drawn since the resize ---\n{}\n\
             --- end ---",
            child.drawn_since(drawn_before)
        );
        std::thread::sleep(READ_POLL);
    }
    Some(child.drawn_since(drawn_before))
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

    /// Type at the terminal, exactly as a person does: the bytes go in at the
    /// master and come out of the child's standard input.
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
            "could not put {keys:?} into the terminal"
        );
    }

    /// Change the size of the terminal, exactly as a window manager dragging
    /// its corner does: the kernel records the new size and sends `SIGWINCH`.
    fn resize(&mut self, rows: u16, cols: u16) {
        self.master
            .as_ref()
            .expect("the terminal is still there")
            .resize(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .expect("resize the pty");
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

    /// End the child now, and do not come back until it is reaped.
    ///
    /// **`SIGKILL`, and not `portable_pty::Child::kill`, which sends
    /// `SIGHUP`.** Every caller of this is on a failure path or a scope exit,
    /// and a path that hangs is worse than no path at all — a gate waiting
    /// forever reports nothing, where a failed assertion reports a defect.
    /// Two ways `SIGHUP` hangs here, both observed:
    ///
    /// * a Glasshouse wedged in crossterm's unbounded read — the very defect
    ///   this file is about — never returns to the loop that would observe the
    ///   shutdown the signal requested, so it keeps spinning and `wait` never
    ///   returns;
    /// * a Glasshouse that *is* winding down blocks writing its last frame to
    ///   a pty whose master nobody is draining any more.
    ///
    /// **And the second of those is not answered by `SIGKILL` either**, which
    /// is what the drain below is for. A process already blocked in a write to
    /// a full pty cannot finish dying until somebody reads the other end, and
    /// this thread is the only reader there is — so a bare `wait()` here
    /// deadlocks the pair. Seen twice: once for eleven minutes in state `E`
    /// before `SIGKILL` was reached for at all, and again with `SIGKILL`
    /// already sent, by a trial that left an interface running and simply
    /// stopped reading it.
    ///
    /// The bound is belt and braces. A process that has not answered
    /// `SIGKILL` within it is an operating-system anomaly rather than
    /// anything this file can assert about, and returning lets the test say
    /// what it found instead of hanging the gate on it.
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
            std::thread::sleep(READ_POLL);
        }
    }

    fn output(&self) -> String {
        String::from_utf8_lossy(&self.output).into_owned()
    }

    /// How much has been drawn so far, as a mark to pass to [`Shell::drawn_since`].
    fn drawn_so_far(&self) -> usize {
        self.output.len()
    }

    /// Everything drawn since `mark`.
    ///
    /// **Counted in bytes and sliced in bytes**, which is why this exists at
    /// all rather than the caller indexing `output()`. That is a lossy UTF-8
    /// conversion of a stream this test reads in 4096-byte pieces: a piece
    /// that ends in the middle of one of the box-drawing characters the
    /// interface draws by the hundred becomes a replacement character, and the
    /// next read moves every byte offset after it. Slicing the converted
    /// string at an offset taken before that happened is a panic waiting for a
    /// badly-timed read.
    fn drawn_since(&self, mark: usize) -> String {
        String::from_utf8_lossy(&self.output[mark.min(self.output.len())..]).into_owned()
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
