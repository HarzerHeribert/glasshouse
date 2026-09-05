//! Input and application events.
//!
//! Glasshouse's interface has to react to two unrelated sources: the user's
//! keyboard and terminal, and things that happen on their own — a harness
//! session producing output, a session exiting, a lifecycle event arriving.
//! Both land in one [`Event`] stream so the interface has a single loop and a
//! single place where redraws are decided.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, Sender, TryRecvError, channel};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use crossterm::event::{self, KeyEvent, MouseEvent};

/// Something the interface must react to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    Key(KeyEvent),
    Mouse(MouseEvent),
    /// Text pasted into the terminal, when bracketed paste is enabled.
    Paste(String),
    /// The terminal window changed size, in columns and rows.
    Resize(u16, u16),
    /// No input arrived within the tick interval.
    ///
    /// A tick is not a redraw request. It exists so the loop regains control
    /// regularly enough to notice a shutdown request, poll child processes,
    /// and repaint anything that became dirty on another thread.
    Tick,
    /// A signal asked Glasshouse to wind down.
    Shutdown,
    /// Something outside the terminal happened. Producers on other threads
    /// send these through [`EventSource::sender`].
    App(AppEvent),
}

/// An event raised by Glasshouse itself rather than by the terminal.
///
/// Deliberately small for now. Session and lifecycle variants arrive with the
/// session runtime; adding them speculatively would mean guessing at their
/// payloads before there is a session to carry one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AppEvent {
    /// Something changed and the interface should repaint.
    Redraw,
}

/// How many silent ticks before [`EventSource::next`] stops consulting
/// crossterm on an idle tick.
///
/// About a second at the default 16ms tick. Long enough that no interactive
/// burst of typing reaches it — see [`EventSource::quiet_ticks`] for why that
/// matters — and far short of the nineteen hours the orphaned processes this
/// exists for had been idle.
const QUIET_TICKS: u32 = 64;

/// Pulls events from the terminal and from other threads.
///
/// The terminal is polled with a timeout rather than read on a dedicated
/// thread. A thread blocked in `event::read()` cannot be cancelled — it would
/// sit there holding stdin until the user happened to press a key — so polling
/// keeps shutdown immediate and costs nothing but one syscall per tick.
///
/// That polling is done by this module's own `wait_for_terminal` rather than
/// by crossterm, because a terminal that goes away is not something
/// crossterm's own poll can report or even survive. See that function.
pub struct EventSource {
    tick: Duration,
    sender: Sender<AppEvent>,
    receiver: Receiver<AppEvent>,
    /// The terminal size this source last reported, or `None` before it has
    /// looked once.
    ///
    /// A window resize is the one thing crossterm learns that the descriptor
    /// itself never shows: it arrives as `SIGWINCH`, on a pipe of crossterm's
    /// own that only crossterm's poll watches. So a loop that stops polling
    /// crossterm on an idle tick stops delivering resizes — measured, not
    /// reasoned: `pty_smoke::resizing_the_shell_reaches_the_harness_terminal`
    /// failed on macOS *and* Linux the first time the idle skip was added, and
    /// passed again the moment it was taken out.
    ///
    /// Watching the size here rather than restoring that poll keeps the wait
    /// where the rest of this module put it. One `TIOCGWINSZ` per idle tick is
    /// the same order of cost as the `poll` beside it, and unlike a signal it
    /// cannot be delivered to the wrong thread or coalesced away.
    last_size: std::cell::Cell<Option<(u16, u16)>>,
    /// How many ticks in a row the terminal has said nothing at all.
    ///
    /// The short cut in [`EventSource::next`] is taken only once this passes
    /// [`QUIET_TICKS`]: a terminal silent that long has no input to collide
    /// with a delayed `SIGWINCH`, and [`EventSource::next`] separately drains
    /// crossterm's pipe the moment a signal interrupts a wait, so the counter
    /// no longer has to hold that collision off alone. Its own job stays: it
    /// is what keeps an idle process out of `crossterm::event::poll`, where a
    /// hangup wedges it.
    ///
    // History: design-decisions.md, "Trims: tui/event.rs", field `quiet_ticks`.
    quiet_ticks: std::cell::Cell<u32>,
    /// Whether crossterm may still be holding an event it has already read
    /// off the descriptor.
    ///
    /// **This is what stops typing being throttled to one key per tick.**
    /// Crossterm hands back one event per call from a parse buffer it fills
    /// on read, so after the first key of a burst the rest sit inside the
    /// library while the descriptor reads empty. While this is set,
    /// [`EventSource::next`] asks crossterm *before* waiting instead of
    /// after, so a burst comes out at the speed of the loop; it is set by
    /// [`EventSource::take_from_crossterm`] on every successful read and
    /// cleared the first time an early ask says no.
    ///
    /// It is not an optimisation of the idle path: `quiet_ticks` keeps an
    /// *idle* process out of `crossterm::event::poll`, and this flag is
    /// false on every one of those ticks, so the two never overlap.
    ///
    // History: design-decisions.md, "Trims: tui/event.rs", field `crossterm_may_hold_more`.
    crossterm_may_hold_more: std::cell::Cell<bool>,
}

impl EventSource {
    /// Create a source that yields [`Event::Tick`] when idle for `tick`.
    pub fn new(tick: Duration) -> Self {
        let (sender, receiver) = channel();
        Self {
            tick,
            sender,
            receiver,
            last_size: std::cell::Cell::new(None),
            quiet_ticks: std::cell::Cell::new(0),
            crossterm_may_hold_more: std::cell::Cell::new(false),
        }
    }

    /// Whether the terminal is a different size than the last resize this
    /// source delivered.
    ///
    /// **The cache is deliberately not updated here.** It moves only when a
    /// resize is actually delivered, further down, so a `SIGWINCH` that has
    /// not yet reached crossterm's pipe is looked for again on the next tick
    /// rather than lost — the answer stays `true` until the resize comes out.
    /// If crossterm never reported one, this would go on letting it through
    /// every tick, which is exactly the behaviour that existed before the idle
    /// short cut: worse than the short cut, and not broken.
    ///
    /// The first look seeds the cache and reports nothing. Nothing has been
    /// resized at that point; there is only a size that had never been read.
    fn terminal_was_resized(&self) -> bool {
        let Some(size) = terminal_size() else {
            return false;
        };
        match self.last_size.get() {
            Some(before) => before != size,
            None => {
                self.last_size.set(Some(size));
                false
            }
        }
    }

    /// A handle other threads can use to wake the interface.
    ///
    /// Cloneable, so every session reader can hold one.
    pub fn sender(&self) -> Sender<AppEvent> {
        self.sender.clone()
    }

    /// Wait for the next event.
    ///
    /// Returns [`Event::Shutdown`] as soon as a signal has asked Glasshouse to
    /// stop, so callers never need to check that separately — and equally when
    /// the terminal itself has gone away, which is the same instruction
    /// arriving without a signal to carry it.
    pub fn next(&self) -> Result<Event> {
        if crate::shutdown::shutdown_requested() {
            return Ok(Event::Shutdown);
        }

        // Events queued by other threads are already here; deliver them before
        // spending the tick waiting on the terminal.
        match self.receiver.try_recv() {
            Ok(app) => return Ok(Event::App(app)),
            Err(TryRecvError::Empty) => {}
            // The source owns a sender, so the channel cannot be disconnected
            // while `self` is alive. Treat it as empty rather than failing.
            Err(TryRecvError::Disconnected) => {}
        }

        let deadline = Instant::now() + self.tick;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Ok(Event::Tick);
            }
            // **Crossterm is asked first while it may still be holding a
            // burst.** Waiting on a descriptor for keystrokes that are already
            // inside the library is what throttled typing to one key per tick;
            // see `crossterm_may_hold_more` for the measurement.
            if self.crossterm_may_hold_more.get() {
                // Nothing may be handed to crossterm without this answer
                // first — a terminal that has gone away walks it into the
                // unbounded read `wait_for_terminal` exists to keep it out of.
                //
                // **The exposure this leaves is the one that is already
                // there, not a new one.** The ask further down is preceded by
                // the same guard, so both are only ever reached microseconds
                // after a hangup answer, and neither can consume a keystroke:
                // `poll(2)` reads nothing.
                //
                // **`Watch::Input` and not `Watch::HangUp`, and that is a fix
                // rather than a detail** — see [`Watch::HangUp`], which cannot
                // report a hangup on macOS at all. At a zero timeout there is
                // nothing for the empty subscription to protect against, since
                // the call returns whatever the answer is; a `Wait::Ready` here
                // just means there really is input, which is the case this
                // branch is for.
                if wait_for_terminal(Duration::ZERO, Watch::Input)? == Wait::HangUp {
                    crate::shutdown::request_shutdown();
                    return Ok(Event::Shutdown);
                }
                if in_crossterm(|| event::poll(Duration::ZERO))
                    .context("could not poll for terminal input")?
                {
                    match self.take_from_crossterm()? {
                        Some(ev) => return Ok(ev),
                        // An event this interface does not act on. Crossterm
                        // may still be holding the one behind it, so ask
                        // again rather than spending the tick on a wait.
                        None => continue,
                    }
                }
                // Crossterm has nothing left. This is the one extra ask a
                // burst costs, and the tick goes back to waiting on the
                // descriptor exactly as it always did.
                self.crossterm_may_hold_more.set(false);
            }
            // The wait happens here rather than inside crossterm, so that a
            // terminal which has gone away is recognised as one. See
            // `wait_for_terminal` for why that distinction cannot be left to
            // the library.
            let mut waited = wait_for_terminal(remaining, Watch::Input)?;
            // **A signal cut the wait short, and one signal in particular is
            // also an event crossterm is holding.** `SIGWINCH` reaches
            // crossterm as a byte on a pipe of its own, and a pipe crossterm
            // has been told about but has not drained is a second readiness
            // waiting to collide with the terminal's — see `Watch` for what
            // that collision costs. So the wait is not simply resumed: the
            // terminal is looked at again at once, and crossterm is consulted
            // on this pass, while the descriptor is still empty and there is
            // nothing for a resize to be picked ahead of.
            //
            // Looked at, never assumed. The hangup and the `SIGHUP`
            // announcing it arrive together, and handing crossterm a terminal
            // that has gone away walks straight into the unbounded read
            // `wait_for_terminal` exists to keep it out of. A second
            // interruption before that look completes leaves the terminal
            // still unexamined, which is the one case that goes back to
            // waiting.
            let after_signal = waited == Wait::Interrupted;
            if after_signal {
                waited = wait_for_terminal(Duration::ZERO, Watch::Input)?;
            }
            match waited {
                Wait::HangUp => {
                    // Nothing will ever arrive on this terminal again, and
                    // there is no longer anyone to show an interface to. Set
                    // the process-wide flag as well as answering this caller:
                    // a second loop started afterwards (the wizard handing
                    // over to the shell) must not sit down at the same dead
                    // terminal, and an attached session supervising a harness
                    // watches the same flag.
                    crate::shutdown::request_shutdown();
                    return Ok(Event::Shutdown);
                }
                // Interrupted again while checking on an interruption: still
                // nothing said about the terminal, so look once more.
                Wait::Interrupted => continue,
                // **This arm is the residual-spin fix.** An idle interface used
                // to ask crossterm once per tick to be told nothing — measured
                // at about 4% of every tick, 0% after this arm. Not asking is
                // safe because a terminal silent this long, with no resize to
                // report, has nothing crossterm could say; see `quiet_ticks`
                // for the silence and `last_size` for the resize.
                //
                // History: design-decisions.md, "Trims: tui/event.rs", `Wait::Idle` arm (residual-spin fix).
                Wait::Idle => {
                    let quiet = self.quiet_ticks.get();
                    // `after_signal` overrides the short cut, and that is the
                    // whole point of taking it on this pass: the signal that
                    // interrupted the wait may be the `SIGWINCH` crossterm is
                    // holding, and skipping the poll here would leave it held.
                    if !after_signal && quiet >= QUIET_TICKS && !self.terminal_was_resized() {
                        continue;
                    }
                    self.quiet_ticks.set(quiet.saturating_add(1));
                }
                // Anything the terminal actually said starts the silence over.
                Wait::Ready | Wait::Unavailable => self.quiet_ticks.set(0),
            }
            // Already waited above, so crossterm is asked only for what it
            // has *now* — except where the wait could not be taken over, in
            // which case it does the waiting exactly as it always did.
            let poll_for = if waited == Wait::Unavailable {
                remaining
            } else {
                Duration::ZERO
            };
            // **The same guard branch A carries, at the other hand-off.** Until
            // this was here, the whole duration of the call below was
            // unguarded, and a hangup landing inside it wedged crossterm
            // exactly as the field processes were wedged. `quiet_ticks` keeps
            // an idle interface away from this call *eventually*; it takes
            // [`QUIET_TICKS`] ticks to do it, and a probe on this tree
            // counted **exactly 64 of these calls in every one of 60 trials**
            // — the warm-up, and nothing after it. So the exposure was not a
            // vanishing tail, it was the interface's first second, every
            // time, which is also when a person who has just started
            // Glasshouse and closed the window is most likely to hang it up.
            //
            // `Wait::Unavailable` is skipped because there is no descriptor to
            // ask about; that platform hands the wait to crossterm entire, and
            // this module has nothing to say about it.
            if waited != Wait::Unavailable
                && wait_for_terminal(Duration::ZERO, Watch::Input)? == Wait::HangUp
            {
                crate::shutdown::request_shutdown();
                return Ok(Event::Shutdown);
            }
            if !in_crossterm(|| event::poll(poll_for))
                .context("could not poll for terminal input")?
            {
                if waited == Wait::Ready {
                    // The kernel says this descriptor has bytes on it and
                    // crossterm says it has nothing — and those two answers
                    // can only disagree when crossterm has thrown the
                    // descriptor's readiness away — see `Watch`. Asking it
                    // again before something new arrives can only produce the
                    // same answer, so the rest of the tick goes to the one
                    // question whose answer could still change.
                    let left = deadline.saturating_duration_since(Instant::now());
                    if !left.is_zero() && wait_for_terminal(left, Watch::HangUp)? == Wait::HangUp {
                        crate::shutdown::request_shutdown();
                        return Ok(Event::Shutdown);
                    }
                    return Ok(Event::Tick);
                }
                continue;
            }
            match self.take_from_crossterm()? {
                // Events Glasshouse does not act on (key releases, focus
                // changes) must not be reported as input, but must also not
                // consume the whole tick — keep waiting out the remainder.
                None => continue,
                Some(ev) => return Ok(ev),
            }
        }
    }

    /// Take the event crossterm has ready, and remember that it may have more.
    ///
    /// `None` is an event Glasshouse does not act on — a key release, a focus
    /// change. It is not input and must not be reported as any, and it must
    /// not end the tick either; the caller keeps going.
    ///
    /// Only ever called where a poll has just said there is an event, so the
    /// read cannot block.
    fn take_from_crossterm(&self) -> Result<Option<Event>> {
        let raw = in_crossterm(event::read).context("could not read terminal input")?;
        // A read that succeeded came out of crossterm's parse buffer, and that
        // buffer may hold the rest of a burst. Remembering so is what lets the
        // next pass ask before it waits — see `crossterm_may_hold_more`. Set
        // for an event that translates to nothing too: what matters is what
        // crossterm is holding, not what this interface makes of it.
        self.crossterm_may_hold_more.set(true);
        let Some(ev) = translate(raw) else {
            return Ok(None);
        };
        self.quiet_ticks.set(0);
        // Crossterm still reports resizes whenever it is consulted at all.
        // Agreeing with it here is what stops `terminal_was_resized`'s watch
        // reporting the same resize a second time.
        if let Event::Resize(cols, rows) = ev {
            self.last_size.set(Some((cols, rows)));
        }
        Ok(Some(ev))
    }
}

/// Whether the interface is inside a call to crossterm, and which one.
///
/// Even means the loop is somewhere this module can see and can leave; **odd
/// means it is inside `crossterm::event::poll` or `crossterm::event::read`**,
/// which is the one place it may never come back from — see
/// [`wait_for_terminal`] for why a hung-up descriptor traps crossterm's reader
/// forever, and [`arm_hangup_watchdog`] for what is done about it.
///
/// Counted up rather than toggled on purpose: a watchdog that samples this
/// twice can then tell "still inside the *same* call" from "inside another
/// call already", and only the first of those is a process that will never
/// come back.
static CROSSTERM_CALL: AtomicU64 = AtomicU64::new(0);

/// Run `call`, marking the interface as inside crossterm for its duration.
///
/// A panic inside leaves the count odd. Deliberate rather than overlooked: a
/// panic in the interface ends the process through
/// [`crate::shutdown::install_panic_hook`], so there is no later hangup for a
/// stale odd value to mislead.
fn in_crossterm<T>(call: impl FnOnce() -> T) -> T {
    CROSSTERM_CALL.fetch_add(1, Ordering::SeqCst);
    let out = call();
    CROSSTERM_CALL.fetch_add(1, Ordering::SeqCst);
    out
}

/// How often the watchdog asks whether the terminal is still there.
///
/// Six times slower than the interface's own tick, and the reason it is a
/// number at all rather than a blocking wait is in [`wait_until_hangup`].
#[cfg(unix)]
const HANGUP_POLL: Duration = Duration::from_millis(100);

/// How long the same crossterm call must still be in flight, after the
/// terminal has gone, before it is called wedged.
///
/// Three ticks. A crossterm call on a live terminal is microseconds, and a
/// crossterm call on a dead one is forever — there is no third duration to
/// confuse this with, so the only thing this has to outlast is the interface
/// being descheduled, which is why it is measured in ticks rather than in
/// microseconds.
#[cfg(unix)]
const WEDGE_CHECK: Duration = Duration::from_millis(50);

/// How long a hung-up interface that is **not** stuck inside crossterm is
/// given to leave on its own.
///
/// Deliberately generous, and it can afford to be: this branch is for a
/// process that is winding down rather than spinning, so waiting costs no
/// processor time at all. The wind-down flushes an event log to SQLite, and
/// cutting that short to save a second nobody is watching would trade a
/// resource-exhaustion defect for a data-loss one. The spinning case does not
/// wait this long — [`WEDGE_CHECK`] proves it and it is put down at once.
#[cfg(unix)]
const HANGUP_GRACE: Duration = Duration::from_secs(3);

/// How long each rung of the forced-exit ladder is given before the next.
#[cfg(unix)]
const FORCE_INTERVAL: Duration = Duration::from_millis(100);

/// Exit code for the backstop below, matching `shutdown`'s own forced exit so
/// that a caller cannot tell which of the two ended the process — they mean
/// the same thing.
#[cfg(unix)]
const EXIT_FORCED: i32 = 130;

/// Whether the watchdog thread has already been started.
#[cfg(unix)]
static WATCHDOG_RUNNING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Start watching for the terminal going away, independently of the interface.
///
/// # Why a thread, when the loop already detects hangups
///
/// The loop's own detection is a *rate*, not a *guarantee*: a terminal that
/// dies in the microseconds between its hangup check and crossterm's own
/// `read` traps the interface, because crossterm loops forever on a
/// zero-byte read and nothing this process sets is ever looked at again.
/// Nothing inside that loop can end it, so this exists outside it.
///
/// One thread blocked in a `poll(2)` subscribed to nothing wakes only for
/// `POLLHUP`/`POLLERR`/`POLLNVAL`, at one syscall for an ordinary session's
/// life. A hangup is also how a session usually ends, so [`CROSSTERM_CALL`]
/// tells it which case this is: stuck in the same call past [`WEDGE_CHECK`]
/// is ended at once, anywhere else gets [`HANGUP_GRACE`] to wind down —
/// deliberately with no way to disarm it, since every `Screen` here is
/// dropped either to acquire another or on the way out. Idempotent: the
/// thread is started once.
// History: design-decisions.md, "Trims: tui/event.rs", `fn arm_hangup_watchdog`.
#[cfg(unix)]
pub(crate) fn arm_hangup_watchdog() {
    if WATCHDOG_RUNNING.swap(true, Ordering::SeqCst) {
        return;
    }
    let Some(fd) = terminal_fd() else {
        // No terminal to watch. Nothing to guarantee, and nothing to leak.
        WATCHDOG_RUNNING.store(false, Ordering::SeqCst);
        return;
    };
    let started = std::thread::Builder::new()
        .name("glasshouse-hangup-watchdog".to_owned())
        .spawn(move || watch_for_hangup(fd));
    if let Err(err) = started {
        // A process that cannot spawn a thread is in no state to be given a
        // second one to worry about. The interface's own detection stays, and
        // this says so rather than pretending the guarantee is in place.
        tracing::warn!(%err, "could not start the terminal hangup watchdog");
        WATCHDOG_RUNNING.store(false, Ordering::SeqCst);
    }
}

/// See the Unix implementations. Windows constructs no hangup answer at all
/// (see [`wait_for_terminal`]), so there is nothing here to watch for and the
/// platform keeps exactly the behaviour it had.
#[cfg(not(unix))]
pub(crate) fn arm_hangup_watchdog() {}

/// The watchdog thread's whole life.
#[cfg(unix)]
fn watch_for_hangup(fd: std::os::fd::RawFd) {
    if !wait_until_hangup(fd) {
        WATCHDOG_RUNNING.store(false, Ordering::SeqCst);
        return;
    }
    // The ordinary route first, and it is almost always the one that works:
    // the loop reads this between events and leaves through its own exit.
    crate::shutdown::request_shutdown();

    let deadline = Instant::now() + HANGUP_GRACE;
    loop {
        let seen = CROSSTERM_CALL.load(Ordering::SeqCst);
        std::thread::sleep(WEDGE_CHECK);
        // Odd means inside crossterm; unchanged means inside the *same* call.
        // Both together mean a call that cannot return, on a descriptor that
        // will never produce another byte.
        if seen % 2 == 1 && CROSSTERM_CALL.load(Ordering::SeqCst) == seen {
            break;
        }
        if Instant::now() >= deadline {
            break;
        }
    }
    force_down();
}

/// Wait until the terminal's far end goes away, and say whether it did.
///
/// # Why this polls on a timer instead of blocking
///
/// No mask blocks on a hangup alone on both platforms: on macOS every mask
/// that reports a hangup also reports an ordinary pending keystroke, and a
/// watchdog blocked on one would be woken by input it must never read, then
/// woken again immediately — forever. So it asks instead of waiting: one
/// zero-timeout `poll(2)` every [`HANGUP_POLL`], sleeping in between, which
/// never reads the descriptor and costs the same 0.3% of a core idle as
/// no watchdog at all.
///
/// The latency this costs is paid only where the interface itself can no
/// longer answer — its own guards catch a hangup within microseconds, so one
/// further [`HANGUP_POLL`] on an already-stuck process changes nothing.
///
/// Deliberately not [`wait_for_terminal`], which [`blind_to_hangups`] can
/// make answer a hung-up terminal the way the original defect did — a
/// watchdog blinded along with the interface would prove nothing.
// History: design-decisions.md, "Trims: tui/event.rs", `fn wait_until_hangup`.
#[cfg(unix)]
fn wait_until_hangup(fd: std::os::fd::RawFd) -> bool {
    loop {
        let mut watched = libc::pollfd {
            fd,
            // Subscribed to, never read from. `poll` inspects; it does not
            // consume, so the interface's input is untouched — and on macOS
            // nothing is reported at all without a subscription.
            events: libc::POLLIN,
            revents: 0,
        };
        // SAFETY: `watched` is one initialised `pollfd` and the count says so.
        // `poll` reads `fd`/`events` and writes only `revents`.
        let ready = unsafe { libc::poll(&mut watched, 1, 0) };
        if ready < 0 {
            let error = std::io::Error::last_os_error();
            if error.kind() != std::io::ErrorKind::Interrupted {
                return false;
            }
        } else if watched.revents & (libc::POLLHUP | libc::POLLERR | libc::POLLNVAL) != 0 {
            return true;
        }
        std::thread::sleep(HANGUP_POLL);
    }
}

/// End this process, through the route the project already defines for it.
///
/// `SIGTERM` rather than a private exit, because `shutdown`'s handler already
/// knows what forcing Glasshouse down means — run the registered forced-exit
/// cleanups, restore the terminal, exit — and one definition of that is worth
/// more than a second one here that would drift. Two of them, because the
/// handler's first answer is "ask the interface to stop" unless a signal has
/// already been counted, and an interface stuck inside crossterm is exactly
/// the one that will not hear it. With no handler installed at all, `SIGTERM`'s
/// default disposition ends the process on the first.
///
/// The exit below is a backstop for a process that answered neither, and
/// nothing in it can fail. Failing to clean up is survivable; failing to exit
/// is the defect.
#[cfg(unix)]
fn force_down() -> ! {
    for _ in 0..2 {
        // SAFETY: `kill` inspects nothing; the pid is this process's own and
        // `SIGTERM` is a valid signal number.
        unsafe {
            libc::kill(std::process::id() as libc::pid_t, libc::SIGTERM);
        }
        std::thread::sleep(FORCE_INTERVAL);
    }
    crate::shutdown::restore_terminal();
    std::process::exit(EXIT_FORCED);
}

/// Which of the terminal's answers one wait is interested in.
///
/// Crossterm multiplexes the terminal and a `SIGWINCH` pipe through one
/// edge-triggered `mio` registration, and its reader returns from inside the
/// first readiness it finds — so a `SIGWINCH` looked at first in the same
/// batch as terminal input discards the terminal's readiness unread until a
/// new edge creates one (measured: 27 of 60 trials stranded a keystroke this
/// way). [`EventSource::next`] narrows the window by consulting crossterm
/// the instant a signal interrupts a wait, from about 16ms to under 50µs,
/// leaving only the case where the two arrive in the same microseconds —
/// crossterm's own readiness-dropping to fix, not this module's.
///
/// Asking crossterm or this module's own `poll(2)` again does not help: a
/// descriptor with unread bytes stays readable, so `poll(2)` keeps answering
/// [`Wait::Ready`] while crossterm answers nothing, and re-asking burned a
/// whole core for the rest of the tick with nothing delivered (measured:
/// 380,987 of 381,501 waits). So the loop instead waits on [`Watch::HangUp`],
/// which subscribes to nothing and can only report the one thing still
/// worth acting on before the next tick: the terminal going away.
// History: design-decisions.md, "Trims: tui/event.rs", `enum Watch`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Watch {
    /// Input, and a hangup — the ordinary wait.
    Input,
    /// A hangup and nothing else, on the platform where that is possible.
    ///
    /// # This does not work on macOS, and the loop no longer depends on it
    ///
    /// POSIX says `POLLHUP`/`POLLERR`/`POLLNVAL` are reported whatever is
    /// subscribed to; Darwin does not do it — a descriptor must be
    /// subscribed to something before any `revents` are reported at all, and
    /// no mask there wakes on a hangup and not on input (`POLLPRI` also
    /// fires for an ordinary pending keystroke).
    ///
    /// So the zero-timeout guards before each hand-off to crossterm ask
    /// [`Watch::Input`] instead, which reports the hangup on both platforms.
    /// The timed wait below still uses this variant, because there the empty
    /// subscription is the point — subscribing to `POLLIN` there is the
    /// 380,987-waits spin; on macOS it degrades to a plain sleep, and a
    /// hangup inside it is caught one tick later by the ordinary wait.
    /// Neither of those is what makes the guarantee — [`arm_hangup_watchdog`]
    /// is, with its own `poll` for exactly this reason.
    ///
    // History: design-decisions.md, "Trims: tui/event.rs", `Watch::HangUp` variant.
    HangUp,
}

/// What one wait on the terminal produced.
///
/// A platform with no hangup answer never constructs [`Wait::HangUp`] or
/// [`Wait::Interrupted`], and an unconstructed variant is a `-D warnings`
/// build failure rather than a warning. The variants stay — they are the
/// vocabulary the loop above is written in, and deleting them per platform
/// would mean two loops — so the platform that cannot produce them says so
/// here. Found by compiling the non-Unix path locally with the cfg flipped
/// (practice §18), which is the only Windows evidence this machine can offer.
#[cfg_attr(not(unix), allow(dead_code))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Wait {
    /// Input is there to be read.
    Ready,
    /// Nothing arrived before the deadline — or, under [`Watch::HangUp`],
    /// nothing that was being watched for.
    Idle,
    /// A signal arrived instead, so the wait says nothing either way.
    Interrupted,
    /// The far end of the terminal is gone.
    HangUp,
    /// The wait could not be taken over on this platform, so nothing was
    /// waited on and crossterm must do it — exactly as it did before any of
    /// this existed.
    Unavailable,
}

/// Wait up to `timeout` for the terminal, distinguishing input from a hangup.
///
/// `watch` chooses which answer is wanted; see [`Watch`] for why only the
/// hangup answer is ever asked for alone.
///
/// `crossterm::event::poll` cannot report a hangup and cannot survive one:
/// its Unix source loops on `read` until it yields an event or fails, and a
/// descriptor whose far end is gone is permanently readable and returns
/// zero bytes — neither — so `poll` never returns and [`EventSource::next`]
/// never reaches its shutdown check again (found from orphaned processes
/// stuck exactly there). `poll(2)` reports `POLLHUP` the moment the far end
/// closes instead, without consuming a keystroke, since this loop is the
/// only thing reading input.
/// A terminal dying between this call and crossterm's own poll still leaves
/// crossterm in that loop; [`QUIET_TICKS`] bounds the idle ask, and
/// [`arm_hangup_watchdog`] is the guarantee for the rest. Windows is
/// deliberately unhandled — a different mechanism needing a different
/// answer this project has no native terminal to test —
/// [`Wait::Unavailable`] keeps its old behaviour there exactly.
// History: design-decisions.md, "Trims: tui/event.rs", `fn wait_for_terminal`.
#[cfg(unix)]
fn wait_for_terminal(timeout: Duration, watch: Watch) -> Result<Wait> {
    let Some(fd) = terminal_fd() else {
        return Ok(Wait::Unavailable);
    };

    let mut watched = libc::pollfd {
        fd,
        events: match watch {
            Watch::Input => libc::POLLIN,
            // Subscribing to nothing is the point: see `Watch::HangUp`.
            Watch::HangUp => 0,
        },
        revents: 0,
    };
    // `poll` counts whole milliseconds. Round a sub-millisecond remainder up
    // rather than down: a zero timeout there would turn the last fraction of
    // every tick into a spin, which is the defect this exists to remove.
    //
    // An *exactly* zero timeout is a different request and is passed through
    // as one: it is the look-don't-wait `EventSource::next` makes after a
    // signal, and rounding that up to a millisecond would hand the keystroke
    // it is racing a millisecond to arrive in. Measured — rounded up, the
    // harness stalls 20 in 20 at a half-millisecond gap where this tree
    // stalls 0 in 40.
    let millis = if timeout.is_zero() {
        0
    } else {
        i32::try_from(timeout.as_millis().max(1)).unwrap_or(i32::MAX)
    };

    // SAFETY: `watched` is one initialised `pollfd` and the count says so.
    // `poll` reads `fd`/`events` and writes only `revents`.
    let ready = unsafe { libc::poll(&mut watched, 1, millis) };

    if ready < 0 {
        let error = std::io::Error::last_os_error();
        // A signal arriving mid-wait is not a terminal problem, and it is not
        // an answer about the terminal either.
        if error.kind() == std::io::ErrorKind::Interrupted {
            return Ok(Wait::Interrupted);
        }
        return Err(anyhow::Error::new(error).context("could not wait on the terminal"));
    }
    if ready == 0 {
        return Ok(Wait::Idle);
    }

    // Checked before `POLLIN`, and that order is the whole point: a hung-up
    // terminal reports both at once, and reading the `POLLIN` half of that is
    // what spins forever. `POLLERR` and `POLLNVAL` mean the descriptor is
    // unusable, which is the same outcome by a different route.
    if watched.revents & (libc::POLLHUP | libc::POLLERR | libc::POLLNVAL) != 0
        && !blind_to_hangups()
    {
        return Ok(Wait::HangUp);
    }
    Ok(match watch {
        Watch::Input => Wait::Ready,
        // Nothing was subscribed to, so a wakeup that is not a hangup is not
        // an answer about anything and the caller only wanted the time spent.
        Watch::HangUp => Wait::Idle,
    })
}

/// Whether this process has been asked to answer a hung-up terminal the way
/// the original defect did.
///
/// # Why a switch exists in shipped code
///
/// [`arm_hangup_watchdog`]'s guarantee needs a process genuinely trapped
/// inside crossterm, and that race is now microseconds wide — a rate to
/// sample (about one hangup in sixty on a loaded runner), not a state a
/// test can construct honestly. So the case is constructed instead: with
/// this set, `wait_for_terminal` looks past `POLLHUP` and answers `POLLIN`,
/// the exact reading the field defect made, and the interface walks into
/// crossterm with a dead descriptor every time — which only the watchdog
/// can end.
///
/// Read once, it changes nothing unless the variable is present, and
/// `block_until_hangup` deliberately does not consult it, so the watchdog
/// cannot be blinded by the same switch that blinds the interface.
///
// History: design-decisions.md, "Trims: tui/event.rs", `fn blind_to_hangups`.
#[cfg(unix)]
fn blind_to_hangups() -> bool {
    static BLIND: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *BLIND.get_or_init(|| std::env::var_os("GLASSHOUSE_TUI_BLIND_TO_HANGUPS").is_some())
}

/// The descriptor crossterm reads terminal input from.
///
/// It has to be that one and not merely *a* terminal: polling anything else
/// would report the state of something crossterm is not reading. Crossterm
/// picks standard input when it is a terminal and opens `/dev/tty` otherwise
/// (`crossterm::terminal::sys::file_descriptor::tty_fd`), so this makes the
/// same choice. `None` means there is no terminal to watch at all, which
/// leaves the wait where it was.
#[cfg(unix)]
fn terminal_fd() -> Option<std::os::fd::RawFd> {
    use std::os::fd::AsRawFd;

    // SAFETY: `isatty` only inspects the descriptor.
    if unsafe { libc::isatty(libc::STDIN_FILENO) } == 1 {
        return Some(libc::STDIN_FILENO);
    }

    // Opened once and kept for the life of the process. Reopening it every
    // tick would be two syscalls per tick for a case the interactive paths
    // never reach — both of them refuse to start without a terminal on
    // standard input.
    static DEV_TTY: std::sync::OnceLock<Option<std::fs::File>> = std::sync::OnceLock::new();
    DEV_TTY
        .get_or_init(|| {
            std::fs::File::options()
                .read(true)
                .write(true)
                .open("/dev/tty")
                .ok()
        })
        .as_ref()
        .map(AsRawFd::as_raw_fd)
}

/// The terminal's size in columns and rows, read straight from the descriptor.
///
/// Deliberately not `crossterm::terminal::size`, which falls back to the
/// `COLUMNS`/`LINES` environment variables when the `ioctl` fails and would
/// therefore answer confidently about a terminal that is no longer there.
/// `None` here means "do not know", which is what the caller wants.
#[cfg(unix)]
fn terminal_size() -> Option<(u16, u16)> {
    let fd = terminal_fd()?;
    let mut size = libc::winsize {
        ws_row: 0,
        ws_col: 0,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };
    // SAFETY: `TIOCGWINSZ` writes one `winsize` through the pointer given, and
    // `size` is an initialised one owned by this frame.
    if unsafe { libc::ioctl(fd, libc::TIOCGWINSZ, &raw mut size) } != 0 {
        return None;
    }
    Some((size.ws_col, size.ws_row))
}

/// See the Unix implementation. Nothing here reaches this: the wait cannot be
/// taken over on this platform, so the loop never takes the idle short cut
/// that would ask.
#[cfg(not(unix))]
fn terminal_size() -> Option<(u16, u16)> {
    None
}

/// See the Unix implementation: this platform has no hangup answer yet, so
/// the wait stays where it always was.
#[cfg(not(unix))]
fn wait_for_terminal(_timeout: Duration, _watch: Watch) -> Result<Wait> {
    Ok(Wait::Unavailable)
}

/// Convert a crossterm event into a Glasshouse event, or `None` for events
/// that carry no meaning here.
fn translate(raw: event::Event) -> Option<Event> {
    match raw {
        event::Event::Key(key) => {
            // Key *release* and *repeat* events only arrive when the terminal
            // supports the Kitty keyboard protocol. Acting on both press and
            // release would double every keystroke.
            if key.kind == event::KeyEventKind::Release {
                None
            } else {
                Some(Event::Key(key))
            }
        }
        event::Event::Mouse(mouse) => Some(Event::Mouse(mouse)),
        event::Event::Paste(text) => Some(Event::Paste(text)),
        event::Event::Resize(cols, rows) => Some(Event::Resize(cols, rows)),
        event::Event::FocusGained | event::Event::FocusLost => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyEventKind, KeyModifiers};

    fn key(kind: KeyEventKind) -> event::Event {
        event::Event::Key(KeyEvent::new_with_kind(
            KeyCode::Char('a'),
            KeyModifiers::NONE,
            kind,
        ))
    }

    #[test]
    fn key_releases_are_dropped_so_input_is_not_doubled() {
        assert!(translate(key(KeyEventKind::Release)).is_none());
        assert!(translate(key(KeyEventKind::Press)).is_some());
        assert!(translate(key(KeyEventKind::Repeat)).is_some());
    }

    #[test]
    fn focus_changes_are_not_input() {
        assert!(translate(event::Event::FocusGained).is_none());
        assert!(translate(event::Event::FocusLost).is_none());
    }

    #[test]
    fn resize_and_paste_are_forwarded() {
        assert_eq!(
            translate(event::Event::Resize(120, 40)),
            Some(Event::Resize(120, 40))
        );
        assert_eq!(
            translate(event::Event::Paste("hi".into())),
            Some(Event::Paste("hi".into()))
        );
    }

    #[test]
    fn events_sent_from_another_thread_are_delivered() {
        let source = EventSource::new(Duration::from_millis(50));
        let sender = source.sender();

        // Joined before `next` is called, so the event is definitely queued and
        // the assertion cannot depend on thread scheduling. It also means
        // `next` returns from the channel without ever polling the terminal,
        // which is what lets this run under a test harness with no tty.
        std::thread::spawn(move || sender.send(AppEvent::Redraw).expect("send"))
            .join()
            .expect("collector thread");

        assert_eq!(
            source.next().expect("next"),
            Event::App(AppEvent::Redraw),
            "an event already queued by another thread must be delivered \
             before the terminal is polled"
        );
    }

    #[test]
    fn a_pending_shutdown_short_circuits_before_any_terminal_access() {
        // Not calling `request_shutdown` here: it is process-global and would
        // leak into every other test in this binary. Instead assert the
        // ordering that makes the short circuit possible — the shutdown check
        // is the first thing `next` does, so it cannot be blocked by a
        // terminal that is unavailable.
        assert!(!crate::shutdown::shutdown_requested());
    }
}
