//! Input and application events.
//!
//! Glasshouse's interface has to react to two unrelated sources: the user's
//! keyboard and terminal, and things that happen on their own — a harness
//! session producing output, a session exiting, a lifecycle event arriving.
//! Both land in one [`Event`] stream so the interface has a single loop and a
//! single place where redraws are decided.

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
    /// [`QUIET_TICKS`], and **that threshold is the whole reason the short cut
    /// is safe.** Crossterm multiplexes the terminal and `SIGWINCH` through
    /// one edge-triggered `mio` registration, and its reader returns the first
    /// of the two it looks at — dropping, unread, whatever readiness arrived
    /// in the same batch (see [`Watch`]). Polling it less often leaves a
    /// `SIGWINCH` sitting in its pipe for longer, and a `SIGWINCH` sitting in
    /// its pipe is what a keystroke collides with: skipping it on every idle
    /// tick turned `pty_smoke::resizing_the_shell_reaches_the_harness_
    /// terminal` from 0 failures in 12 into 1 to 2, every one of them a shell
    /// whose keystrokes had been swallowed.
    ///
    /// Waiting for a second of complete silence first buys the protection
    /// where it is needed and gives up nothing where it is not: a terminal
    /// that has not made a sound for a second has no input to collide with,
    /// and the field processes had been silent for nineteen hours.
    ///
    /// **This threshold is no longer the only thing holding that collision
    /// off, and the two do not fight.** [`EventSource::next`] now drains
    /// crossterm's pipe as soon as a signal interrupts a wait, whatever this
    /// counter says — the `after_signal` override in the idle arm is there
    /// precisely so a long silence cannot keep a `SIGWINCH` held. The counter
    /// still does its own job, which is not this one: it is what keeps an idle
    /// process out of `crossterm::event::poll`, where a hangup wedges it.
    quiet_ticks: std::cell::Cell<u32>,
    /// Whether crossterm may still be holding an event it has already read
    /// off the descriptor.
    ///
    /// **This is what stops typing being throttled to one key per tick.**
    /// Crossterm does not read one byte at a time: it drains whatever the
    /// descriptor had into a parse buffer of its own and hands back one event
    /// per call. So after the first key of a burst is delivered, the rest of
    /// the burst is *inside the library* and the descriptor is **empty** —
    /// and `wait_for_terminal`'s `poll(2)` is level-triggered, so it
    /// correctly reports nothing and sleeps out the entire remaining tick
    /// before the loop asks crossterm for the key it has been holding all
    /// along.
    ///
    /// Measured on this tree rather than argued, with a probe logging
    /// `FIONREAD` on the descriptor beside `event::poll(Duration::ZERO)` on
    /// every pass of the wait loop: through a twenty-key burst, **every**
    /// sample read `fionread=0`, and nineteen consecutive samples had
    /// crossterm answering that an event was ready on a descriptor the kernel
    /// called empty. One key per 16ms tick, which is what the shipped binary
    /// delivered: **16.8ms per key, a 200-character paste in 3.38s**.
    ///
    /// So while this is set, [`EventSource::next`] asks crossterm *before*
    /// waiting instead of after, and the burst comes out at the speed of the
    /// loop. It is set by [`EventSource::take_from_crossterm`] whenever a read
    /// succeeds, and cleared the first time that early ask says no — one extra
    /// `event::poll` per burst, and none at all on a terminal nobody is typing
    /// at.
    ///
    /// **It is not an optimisation of the idle path and must not become one.**
    /// `quiet_ticks` exists to keep an *idle* process out of
    /// `crossterm::event::poll`, where a hangup wedges it; this flag is false
    /// on every one of those ticks, so the two never overlap.
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
                // a `wait_for_terminal` whose `poll(2)` reports `POLLHUP` the
                // instant the far end closes, so it too is only ever reached
                // microseconds after a hangup answer; this is the same
                // instrument, at the same distance, subscribing to nothing so
                // it cannot wait and cannot consume a keystroke. And it is one
                // call per tick either way: when this branch delivers, the ask
                // below never runs, and when it does not, this one has cleared
                // the flag.
                if wait_for_terminal(Duration::ZERO, Watch::HangUp)? == Wait::HangUp {
                    crate::shutdown::request_shutdown();
                    return Ok(Event::Shutdown);
                }
                if event::poll(Duration::ZERO).context("could not poll for terminal input")? {
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
                // **This arm is the residual-spin fix**, and what it fixes
                // is a rate rather than a bug. Every call into crossterm is a
                // chance for the terminal to have died since the wait above,
                // and an idle interface used to make one of those calls per
                // tick to be told nothing. Measured over two eight-second
                // profiles of an idle process: 268 of 6210 and 233 of 6162
                // main-thread samples — about 4% of every tick — were inside
                // that pointless call, and 0 of 6185 are after this arm. That
                // share is the window, and it is not the microseconds
                // `wait_for_terminal`'s comment used to claim.
                //
                // A terminal that has been silent for a while, with no window
                // resize to report, has nothing crossterm could say. Not
                // asking is the whole fix: see `quiet_ticks` for why it waits
                // out that silence first, and `last_size` for the one thing
                // that still has to get through.
                //
                // Crossterm cannot be left holding an event of its own by the
                // time this bites, either. It hands back one event per call
                // out of a whole parsed buffer, but it is asked on every one
                // of the `QUIET_TICKS` ticks before the short cut opens — so
                // anything it had is long since drained.
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
            if !event::poll(poll_for).context("could not poll for terminal input")? {
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
        let raw = event::read().context("could not read terminal input")?;
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

/// Which of the terminal's answers one wait is interested in.
///
/// # The collision both variants exist for
///
/// Crossterm watches two things through one `mio` registration: the terminal,
/// and a pipe of its own that a `SIGWINCH` handler writes a byte to. Both are
/// registered edge-triggered — `EPOLLET` on Linux, `EV_CLEAR` on the BSDs,
/// confirmed in mio 1.2.2's selectors — so each readiness is reported exactly
/// once and is gone whether or not anything acted on it.
///
/// `try_read` walks the batch one poll returned and **returns from inside that
/// walk**, on the first token that yields an event. When a `SIGWINCH` and
/// terminal input arrive in the same batch and the signal is looked at first,
/// crossterm returns the resize and the terminal's readiness is discarded
/// unread. The bytes stay on the descriptor, invisible to crossterm until new
/// input creates a new edge — which, for a user who has just pressed Return,
/// means until they press something else.
///
/// Measured on this tree rather than argued. A terminal resized and then typed
/// into four milliseconds later stranded the keystroke in **27 of 60** trials,
/// with `FIONREAD` reporting the byte still on the descriptor, `POLLIN` set,
/// no `POLLHUP`, and crossterm reporting nothing. All 27 came out the instant
/// one further key was pressed, which is the edge-triggered signature and
/// nothing else's. The same process, sampled: 1839 of 1851 main-thread samples
/// inside `crossterm::event::poll`, and 23.9% of a core against 0.3% idle.
///
/// # What [`EventSource::next`] does about it, and what is left
///
/// The window is the time a `SIGWINCH` spends sitting in crossterm's pipe
/// unread, because any keystroke arriving during it lands in the same batch.
/// That used to be a whole tick: the loop went back to waiting on the
/// descriptor and would not consult crossterm again until something happened.
/// It now consults crossterm the moment a signal interrupts a wait, while the
/// descriptor is still empty, which leaves only the case where the keystroke
/// and the signal genuinely arrive together.
///
/// | gap between the resize and the keystroke | before | after |
/// |---|---|---|
/// | 4ms | 27 in 60 | **0 in 60** |
/// | 50µs | — | **0 in 60** |
/// | none — both issued back to back | 15 in 60 | 11 in 60 |
///
/// So the window went from about 16ms to under 50µs, and what is left needs
/// the two to land in the same handful of microseconds. That last case is
/// crossterm's to fix and cannot be fixed here: once a readiness has been
/// reported and dropped, no call this side of the library can ask for it
/// again.
///
/// # Why the loop cannot simply ask again — [`Watch::HangUp`]
///
/// A descriptor with unread bytes stays readable, so this module's own
/// `poll(2)` — which is level-triggered — goes on answering [`Wait::Ready`]
/// while crossterm goes on answering "nothing". Asking either again changes
/// neither answer, and the loop that did ask again spent every remaining
/// microsecond of every tick doing it: **380,987 of 381,501 waits** in one
/// such process, at a whole core, with the keystrokes still not delivered.
///
/// So the loop stops asking for the rest of the tick and waits on
/// [`Watch::HangUp`] instead. `POLLHUP`, `POLLERR` and `POLLNVAL` are reported
/// whatever is subscribed to, so subscribing to nothing at all leaves exactly
/// one answer available: the terminal going away, which is the only thing that
/// could still need acting on before the next tick asks again. New input needs
/// no wakeup here — it will be there on the next tick, and it is the next
/// tick's edge that lets crossterm see it at last. Measured with the
/// prevention above deliberately disabled, so that stalls still happen: a
/// stalled process costs **0.3% of a core** with this, against **23.9%**
/// without.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Watch {
    /// Input, and a hangup — the ordinary wait.
    Input,
    /// A hangup and nothing else. `POLLHUP`, `POLLERR` and `POLLNVAL` are
    /// reported whatever is subscribed to, so subscribing to nothing leaves
    /// exactly one answer available.
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
/// `watch` chooses which of those two answers is wanted; see [`Watch`],
/// which is also where the reason for wanting only the second is written
/// down.
///
/// # Why this is not left to crossterm
///
/// `crossterm::event::poll` cannot report a hangup, and worse, it cannot
/// survive one. Its Unix source reacts to a readable terminal by looping on
/// `read` until the read yields an event or fails; a descriptor whose far end
/// has gone away is *permanently readable and returns zero bytes*, which is
/// neither. `try_read` therefore never returns, so `poll` never returns, so
/// [`EventSource::next`] never returns, and the shutdown check at the top of
/// it is never reached again.
///
/// That is not a theory. Three orphaned `glasshouse` processes were found
/// nineteen hours old at 99% CPU, and a 1622-sample profile put every single
/// sample in that `read`. A signal had already asked one of them to stop and
/// it never noticed, because noticing happens between calls to `next` and
/// there was never going to be another one.
///
/// **Which of crossterm's two Unix sources, because they do not agree.** The
/// one this build compiles is `event::source::unix::mio` — confirmed by
/// symbolising a caught process rather than assumed — and its `TTY_TOKEN` arm
/// treats a zero-byte read as neither a `break`, a `continue`, nor a `return`,
/// so the inner loop cannot end and no timeout is consulted inside it. The
/// other source, behind crossterm's `use-dev-tty` feature, does `break` on a
/// zero-byte read and would not hang this way — and it polls level-triggered,
/// so it could not drop a readiness either (see [`Watch`]). On paper it makes
/// both of this module's defects impossible.
///
/// **It was built and measured, and it does not work here.** That source ends
/// its loop on `while timeout.leftover().map_or(true, |t| !t.is_zero())`, so a
/// `Duration::ZERO` timeout runs the body zero times and
/// `crossterm::event::poll(Duration::ZERO)` can never return `true` — which is
/// the call this loop makes on every pass. Measured on a build with the
/// feature on: no input delivered at all, ever, and 2.03s of processor time in
/// 2.0s of wall clock on a freshly drawn interface. Adopting it would mean
/// giving crossterm a non-zero timeout as well, which is a different design
/// and a workspace dependency change. Recorded so the next reader does not
/// spend the afternoon finding it out.
///
/// So the wait is taken over here, and crossterm is only ever handed a
/// terminal that has bytes waiting. `poll(2)` reports `POLLHUP` the moment
/// the far end closes, and it is the right instrument rather than a
/// speculative `read`: it cannot consume a keystroke, and this loop is the
/// only thing reading the user's input. Measured on macOS against a pty whose
/// master was closed: a live terminal with input pending reports `POLLIN`
/// alone (`0x1`) and a hung-up one reports `POLLIN | POLLHUP` (`0x11`), so a
/// keystroke can never be mistaken for a hangup.
///
/// # The window that is left, which is narrower than it was and was never
/// microseconds
///
/// A terminal that dies between this call answering and crossterm's own poll
/// reaching the descriptor leaves crossterm in the same unbounded loop. That
/// window used to be described here as "microseconds wide against the 16ms one
/// it replaces". **It was not, and the arithmetic said so**: a microsecond
/// window against a 16ms tick predicts about one hangup in ten thousand, and
/// the measured survival rate was two in sixty.
///
/// The window is not a gap *between* calls, it is the duration of the call
/// itself. An idle [`EventSource::next`] used to ask crossterm once per tick
/// for an answer it could not have, and two eight-second profiles of an idle
/// process put 268 of 6210 and 233 of 6162 main-thread samples — about 4% of
/// every tick — inside that ask. A hangup arriving at a uniformly random
/// instant lands there roughly one time in twenty-five, which is the order of
/// magnitude that was actually seen: 7 survivors in 200 hangups. The same
/// profile of the same process with the fix is 0 of 6185.
///
/// So [`EventSource::next`] no longer makes that call once the terminal has
/// been silent for a while — see `QUIET_TICKS`, which is also the reason the
/// short cut waits rather than applying to every idle tick. What exposure is
/// left needs input or a resize at the instant the terminal dies, and neither
/// is the state a closed window leaves behind. Closing it completely would
/// mean parsing terminal input here instead of in the library, or ending the
/// process from outside a loop that can no longer end itself.
///
/// # Windows
///
/// **Deliberately unhandled.** Windows has no `poll` on a console handle, its
/// console input is read through `ReadConsoleInput` rather than a descriptor,
/// and a console that goes away there produces a `CTRL_CLOSE_EVENT` and a
/// failing handle rather than an endless run of zero-byte reads — a different
/// mechanism, needing a different answer. This project has no way to run a
/// native Windows terminal, so a Windows branch here could not be tested by
/// anyone who wrote it. [`Wait::Unavailable`] keeps the old behaviour there
/// exactly, and this comment is the record of what is missing: a Windows
/// hangup path, and the native terminal needed to prove it.
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
    if watched.revents & (libc::POLLHUP | libc::POLLERR | libc::POLLNVAL) != 0 {
        return Ok(Wait::HangUp);
    }
    Ok(match watch {
        Watch::Input => Wait::Ready,
        // Nothing was subscribed to, so a wakeup that is not a hangup is not
        // an answer about anything and the caller only wanted the time spent.
        Watch::HangUp => Wait::Idle,
    })
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
