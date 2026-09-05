//! Clean shutdown and terminal restoration.
//!
//! Glasshouse puts the terminal into raw mode and the alternate screen while
//! the TUI runs. Leaving it that way after an exit, a panic, or a signal leaves
//! the user with an unusable shell, so restoration is centralised here and
//! wired into all three paths.

use std::io::Write;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};

use anyhow::{Context, Result};
use crossterm::terminal::{EnterAlternateScreen, LeaveAlternateScreen};
use crossterm::{cursor, execute, terminal};

/// True while the terminal is in raw mode / on the alternate screen.
static TERMINAL_ENGAGED: AtomicBool = AtomicBool::new(false);
/// True once the terminal has been engaged, and never false again.
///
/// [`TERMINAL_ENGAGED`] answers *"is there something to wind down right now"*.
/// This answers a different question — *"is this the kind of process that has
/// a wind-down at all"* — and the two stop agreeing at exactly the moment
/// [`interpret_signal`] is most likely to be asked. See its doc comment.
static TERMINAL_EVER_ENGAGED: AtomicBool = AtomicBool::new(false);
/// Set when a signal has asked the application to wind down.
static SHUTDOWN_REQUESTED: AtomicBool = AtomicBool::new(false);

/// One registered forced-exit cleanup: the identifier its guard removes by,
/// and the work itself.
type Cleanup = (u64, Box<dyn Fn() + Send>);

/// What to run on the forced-exit path, oldest registration first.
///
/// The forced path calls [`std::process::exit`], which does not unwind and
/// therefore runs no destructor. Anything owning an operating-system resource
/// that outlives this process — a harness in its own session, which no longer
/// receives a hangup when Glasshouse dies — is leaked unless it is torn down
/// here. See [`on_forced_exit`].
///
/// **A registry, not a single slot, and the difference is a real bug.** While
/// this held one `Option`, a second registration silently displaced the first
/// and dropping *either* guard unregistered the other — so two concurrent
/// sessions would have left one harness orphaned on Ctrl-C, and nothing would
/// have reported it. There is one caller today ([`crate::session::attach`]);
/// the shape is what stops the second one from being a silent regression.
static FORCED_EXIT_CLEANUP: Mutex<Vec<Cleanup>> = Mutex::new(Vec::new());

/// Source of the identifiers that let a guard remove its own entry and no
/// one else's. Monotonic and never reused, so a stale guard cannot evict a
/// live registration that happens to sit where it used to.
static NEXT_CLEANUP_ID: AtomicU64 = AtomicU64::new(0);

/// Exit code conventionally reported for a process terminated by SIGINT.
const EXIT_INTERRUPTED: i32 = 130;

/// Restore the terminal to its normal state.
///
/// Safe to call repeatedly and safe to call when the terminal was never
/// engaged; in that case it does nothing.
pub fn restore_terminal() {
    if !TERMINAL_ENGAGED.swap(false, Ordering::SeqCst) {
        return;
    }
    let mut out = std::io::stdout();
    let _ = terminal::disable_raw_mode();
    let _ = execute!(out, LeaveAlternateScreen, cursor::Show);
    let _ = out.flush();
}

/// True once a signal has requested shutdown. The TUI event loop polls this so
/// an interrupt ends the run through the normal exit path.
pub fn shutdown_requested() -> bool {
    SHUTDOWN_REQUESTED.load(Ordering::SeqCst)
}

/// Request shutdown programmatically.
pub fn request_shutdown() {
    SHUTDOWN_REQUESTED.store(true, Ordering::SeqCst);
}

/// Install the panic hook that restores the terminal before the panic message
/// is printed, so the report is readable and the shell is usable afterwards.
pub fn install_panic_hook() {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        restore_terminal();
        previous(info);
    }));
}

/// Install signal handling for interrupt and termination.
///
/// While the terminal is not engaged, a signal exits immediately, matching
/// what a user expects from a normal CLI command. While the TUI owns the
/// terminal the first signal asks for a graceful shutdown and a second one
/// forces the process down, restoring the terminal either way. "A second
/// one" counts **signals**, not shutdown requests — closing a terminal
/// delivers `SIGHUP` and a `POLLHUP` at the same instant, two observations
/// of one event, and counting requests instead once forced the process down
/// through `force_exit` (exit 130, no destructors) on roughly half of clean
/// hangups on macOS and all of them in a Linux container. The residue of
/// that race is answered in `interpret_signal` (named rather than linked:
/// it is private, and a public doc comment linking a private item fails
/// the gate).
///
/// History: design-decisions.md, "Trims: the remaining module docs, second
/// packet", `install_signal_handler`.
pub fn install_signal_handler() -> Result<()> {
    ctrlc::set_handler(|| match interpret_signal() {
        SignalMeaning::LeaveImmediately | SignalMeaning::StopWaiting => force_exit(),
        SignalMeaning::AskToStop => request_shutdown(),
    })
    .context("could not install signal handler")
}

/// What one signal means, given what has already happened.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SignalMeaning {
    /// Nothing owns the terminal, so there is nothing to wind down.
    LeaveImmediately,
    /// The first ask: let the interface stop and restore the terminal.
    AskToStop,
    /// A second ask, from someone who is done waiting for the first.
    StopWaiting,
}

/// Decide what a signal means, and record that it arrived.
///
/// This is the function that *asks* the policy; the handler above only acts
/// on the answer, kept separate because the distinction is worth a test on
/// its own. Asking [`TERMINAL_ENGAGED`] alone is wrong for the last few
/// milliseconds of every TUI run: `tui::event`'s `POLLHUP` detector and the
/// kernel's `SIGHUP` are two observations of one event, and if the detector
/// restores the terminal before the signal handler runs, the same event
/// reads as "nothing owns the terminal" and forces exit 130 having done
/// everything right — measured at 2.3% of clean hangups before the fix.
/// [`TERMINAL_EVER_ENGAGED`] tells the two apart: a process that has ever
/// held the terminal falls through to the ordinary counting rule, so the
/// hangup's `SIGHUP` is the first ask whichever side of the restore it
/// lands on. This costs exactly **one** signal — a `SIGTERM`/`SIGINT`
/// arriving after the terminal was given back is answered by
/// [`request_shutdown`] instead of forcing, and the next one forces as
/// always — which is why `tui::event`'s watchdog sends its `SIGTERM` twice.
///
/// History: design-decisions.md, "Trims: the remaining module docs, second
/// packet", `interpret_signal`.
fn interpret_signal() -> SignalMeaning {
    // Never held the terminal: an ordinary command, and a signal ends it now.
    if !TERMINAL_ENGAGED.load(Ordering::SeqCst) && !TERMINAL_EVER_ENGAGED.load(Ordering::SeqCst) {
        return SignalMeaning::LeaveImmediately;
    }
    if SIGNALS_SEEN.fetch_add(1, Ordering::SeqCst) > 0 {
        return SignalMeaning::StopWaiting;
    }
    SignalMeaning::AskToStop
}

/// Record that the terminal is now this process's to give back.
///
/// Both guards call this rather than storing [`TERMINAL_ENGAGED`] themselves,
/// so the latch [`interpret_signal`] depends on cannot be set by one of them
/// and forgotten by the other.
fn engage_terminal() {
    TERMINAL_EVER_ENGAGED.store(true, Ordering::SeqCst);
    TERMINAL_ENGAGED.store(true, Ordering::SeqCst);
}

/// How many signals have asked Glasshouse to stop.
///
/// Deliberately separate from [`SHUTDOWN_REQUESTED`]: that flag answers "should
/// Glasshouse stop", which anything may set, and this counts "how many times
/// has a *user or a kernel* asked", which only this handler may. Reading the
/// first to answer the second is the defect described on
/// [`install_signal_handler`].
static SIGNALS_SEEN: AtomicUsize = AtomicUsize::new(0);

/// Tear down what a skipped destructor would have, restore the terminal, and
/// end the process.
fn force_exit() -> ! {
    run_forced_exit_cleanup();
    restore_terminal();
    std::process::exit(EXIT_INTERRUPTED);
}

/// Register work to run if Glasshouse is forced down, and unregister it when
/// the returned guard drops.
///
/// The forced path exists so a second interrupt always works, even if the
/// normal one is not being serviced. It calls [`std::process::exit`], which
/// runs no destructor — so a harness started in its own session would simply
/// be left running, unreachable, with nothing to hang it up.
///
/// `cleanup` must be **best effort and non-blocking**. It runs while the
/// process is being torn down and while other threads may hold whatever it
/// wants to touch; a cleanup that waits for a lock could hang the very escape
/// hatch it belongs to. Use `try_lock` and give up rather than wait — failing
/// to clean up is no worse than today, whereas failing to exit is much worse.
///
/// `ctrlc` runs handlers on its own thread rather than in a real signal
/// context, so ordinary Rust — including taking a lock — is sound here.
pub fn on_forced_exit(cleanup: impl Fn() + Send + 'static) -> ForcedExitGuard {
    let id = NEXT_CLEANUP_ID.fetch_add(1, Ordering::SeqCst);
    if let Ok(mut registry) = FORCED_EXIT_CLEANUP.lock() {
        registry.push((id, Box::new(cleanup)));
    }
    ForcedExitGuard { id }
}

fn run_forced_exit_cleanup() {
    // `try_lock`, not `lock`: if the registry is momentarily held elsewhere,
    // skip the cleanup rather than risk never reaching the exit below.
    let Ok(registry) = FORCED_EXIT_CLEANUP.try_lock() else {
        return;
    };
    // Reverse registration order, which is the order destructors would have
    // run in had this path not skipped them. A later registration may depend
    // on an earlier one still being intact.
    for (_, cleanup) in registry.iter().rev() {
        // One bad callback must not stop the others, and must not unwind out
        // of `force_exit` before it reaches `std::process::exit`. That is this
        // module's own rule applied to the registry: failing to clean up is
        // survivable, failing to exit is not.
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(cleanup));
    }
}

/// Unregisters its [`on_forced_exit`] cleanup when dropped, so a callback
/// never outlives the thing it was meant to tear down.
#[derive(Debug)]
pub struct ForcedExitGuard {
    /// Identifies this guard's own entry. Dropping removes exactly this one,
    /// which is what makes a second registration safe.
    id: u64,
}

impl Drop for ForcedExitGuard {
    fn drop(&mut self) {
        if let Ok(mut registry) = FORCED_EXIT_CLEANUP.lock() {
            registry.retain(|(id, _)| *id != self.id);
        }
    }
}

/// RAII ownership of the terminal.
///
/// Dropping the guard restores the terminal, which covers normal returns and
/// unwinding alike.
#[derive(Debug)]
pub struct TerminalGuard {
    _private: (),
}

impl TerminalGuard {
    /// Enter raw mode and the alternate screen.
    pub fn acquire() -> Result<Self> {
        terminal::enable_raw_mode().context("could not enable raw terminal mode")?;
        // Store the flag immediately after raw mode is enabled, before
        // touching the alternate screen: a signal landing in that window
        // previously saw raw mode on but the flag still false, so
        // `restore_terminal` no-op'd and the shell was left unusable.
        engage_terminal();

        let mut out = std::io::stdout();
        if let Err(e) = execute!(out, EnterAlternateScreen, cursor::Hide) {
            // Route the failure through `restore_terminal` so the flag is
            // cleared and raw mode is disabled exactly once, rather than
            // duplicating that bookkeeping here.
            restore_terminal();
            return Err(e).context("could not enter the alternate screen");
        }
        Ok(Self { _private: () })
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        restore_terminal();
    }
}

/// RAII ownership of the terminal in raw mode only, with no alternate
/// screen.
///
/// This is what a session attached directly to the user's terminal needs.
/// [`TerminalGuard`] is wrong for that job: a harness draws its own TUI and
/// usually enters the alternate screen itself, so wrapping it in a second
/// one would strand the user watching their session vanish on exit. Raw
/// mode stops the local line discipline from buffering lines, echoing
/// keystrokes, and — the part that matters most — turning Ctrl-C into a
/// signal for *Glasshouse* rather than a `0x03` byte forwarded to the
/// harness. Restoration goes through the same [`restore_terminal`] and
/// `TERMINAL_ENGAGED` flag as [`TerminalGuard`], and that shared path emits
/// `LeaveAlternateScreen` unconditionally even though this guard never
/// entered one — deliberate, since the *harness* very likely did, and a
/// harness that dies without leaving it would otherwise strand the user.
///
/// History: design-decisions.md, "Trims: the remaining module docs, second
/// packet", `RawModeGuard`.
#[derive(Debug)]
pub struct RawModeGuard {
    _private: (),
}

impl RawModeGuard {
    /// Put the terminal into raw mode.
    pub fn acquire() -> Result<Self> {
        terminal::enable_raw_mode().context("could not enable raw terminal mode")?;
        engage_terminal();
        Ok(Self { _private: () })
    }
}

impl Drop for RawModeGuard {
    fn drop(&mut self) {
        restore_terminal();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `TERMINAL_ENGAGED` is process-global and more than one test here reads
    /// and writes it. Rust runs a crate's tests as threads of one process, so
    /// they have to take turns — `restoring_an_unengaged_terminal_is_a_no_op`
    /// asserts the flag is false, and the test below has to set it true. A
    /// poisoned lock is not interesting here: a test that already failed
    /// should not make its neighbours fail for a second reason.
    static TERMINAL_STATE: Mutex<()> = Mutex::new(());

    fn terminal_state_turn() -> std::sync::MutexGuard<'static, ()> {
        TERMINAL_STATE.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// A terminal that goes away is one event seen twice: `tui::event`'s own
    /// `POLLHUP` detector requests shutdown, and the kernel delivers `SIGHUP`
    /// at the same instant. Counting shutdown *requests* made the second
    /// observation look like a second interrupt, and the process was forced
    /// down without destructors — exit 130 where a clean 0 was available,
    /// measured at ten of ten attempts inside a Linux container.
    ///
    /// Both halves live in one test on purpose: `SIGNALS_SEEN` and
    /// `TERMINAL_ENGAGED` are process-global, and two tests racing them would
    /// corrupt each other exactly as the existing cleanup test says.
    #[test]
    fn a_shutdown_already_requested_elsewhere_is_not_a_second_interrupt() {
        let _turn = terminal_state_turn();
        let engaged_before = TERMINAL_ENGAGED.swap(true, Ordering::SeqCst);
        let ever_before = TERMINAL_EVER_ENGAGED.swap(false, Ordering::SeqCst);
        let requested_before = SHUTDOWN_REQUESTED.swap(true, Ordering::SeqCst);
        SIGNALS_SEEN.store(0, Ordering::SeqCst);

        // Shutdown is already requested — by the hangup detector, not by a
        // signal. The first signal to arrive is still the *first* signal.
        assert_eq!(
            interpret_signal(),
            SignalMeaning::AskToStop,
            "a shutdown requested by something other than a signal was counted as \
             a signal, so one hangup forced the process down without destructors"
        );

        // A real second signal still forces the process down; the impatient
        // second Ctrl-C this policy exists for must keep working.
        assert_eq!(interpret_signal(), SignalMeaning::StopWaiting);

        // A process that never held the terminal has nothing to wind down,
        // whatever has been counted. **`TERMINAL_EVER_ENGAGED` false is what
        // makes this the ordinary-command case** rather than a TUI that has
        // given its terminal back — see
        // `a_signal_that_arrives_after_the_terminal_was_given_back_does_not_force`,
        // which is the same two flags with the latch set.
        TERMINAL_ENGAGED.store(false, Ordering::SeqCst);
        assert_eq!(interpret_signal(), SignalMeaning::LeaveImmediately);

        SIGNALS_SEEN.store(0, Ordering::SeqCst);
        TERMINAL_ENGAGED.store(engaged_before, Ordering::SeqCst);
        TERMINAL_EVER_ENGAGED.store(ever_before, Ordering::SeqCst);
        SHUTDOWN_REQUESTED.store(requested_before, Ordering::SeqCst);
    }

    /// **The `SIGHUP`-after-restore race, at the seam where it is decided.**
    ///
    /// Closing a terminal is one event with two observers: `tui::event`'s
    /// `POLLHUP` detector, which winds the run down and gives the terminal
    /// back, and the `SIGHUP` the kernel delivers to the session. Whichever is
    /// scheduled second is looking at the same event — so both orderings have
    /// to reach the same answer, and while this read [`TERMINAL_ENGAGED`]
    /// alone they did not: the late one exited 130 from a run that had done
    /// everything right, on 2.3% of hangups.
    ///
    /// Driven here rather than only through a pty because the window is
    /// microseconds wide and the losing side of it cannot be *constructed*
    /// from outside the process — the rate is measured in
    /// `tests/terminal_loss.rs`, and this is the state that rate is made of
    /// (practice §60: the deterministic pair is what carries the claim, the
    /// rate is what says it is the right claim).
    #[test]
    fn a_signal_that_arrives_after_the_terminal_was_given_back_does_not_force() {
        let _turn = terminal_state_turn();
        let _flags = RestoreFlags::now();

        // The state a hangup leaves behind: the terminal was held, and the
        // wind-down the detector asked for has already given it back. No
        // signal has been counted, because a `POLLHUP` is not one.
        //
        // **`SHUTDOWN_REQUESTED` is deliberately not touched.** The hangup
        // detector does set it, and reading it here would answer the question
        // this test is asking with the very flag
        // [`install_signal_handler`]'s doc warns against treating as a signal
        // count. Leaving it alone proves the answer does not depend on it.
        TERMINAL_EVER_ENGAGED.store(true, Ordering::SeqCst);
        TERMINAL_ENGAGED.store(false, Ordering::SeqCst);
        SIGNALS_SEEN.store(0, Ordering::SeqCst);

        assert_eq!(
            interpret_signal(),
            SignalMeaning::AskToStop,
            "the `SIGHUP` that came with a hangup the interface has already \
             answered was read as `LeaveImmediately`, so a clean run was forced \
             down with exit {EXIT_INTERRUPTED}"
        );

        // And the escape hatch is untouched: a second signal still forces,
        // which is what `tui::event`'s watchdog relies on when it sends
        // `SIGTERM` twice to a process that has stopped listening.
        assert_eq!(
            interpret_signal(),
            SignalMeaning::StopWaiting,
            "a process that has given its terminal back must still be forceable"
        );
    }

    /// Puts the process-global signal flags back, **including when the test
    /// that moved them panicked**.
    ///
    /// Written while mutating `interpret_signal`: the mutation failed the test
    /// above at its first assertion, which skipped the restores that used to
    /// sit at the end of it, which left `SHUTDOWN_REQUESTED` set, which failed
    /// `tui::event`'s own `a_pending_shutdown_short_circuits_before_any_terminal_access`
    /// as a second and entirely misleading failure. A mutation's verdict has
    /// to be readable (practice §80), and a restore that only runs on the
    /// happy path is a restore that is missing exactly when it is needed.
    struct RestoreFlags {
        engaged: bool,
        ever_engaged: bool,
        requested: bool,
        signals: usize,
    }

    impl RestoreFlags {
        fn now() -> Self {
            Self {
                engaged: TERMINAL_ENGAGED.load(Ordering::SeqCst),
                ever_engaged: TERMINAL_EVER_ENGAGED.load(Ordering::SeqCst),
                requested: SHUTDOWN_REQUESTED.load(Ordering::SeqCst),
                signals: SIGNALS_SEEN.load(Ordering::SeqCst),
            }
        }
    }

    impl Drop for RestoreFlags {
        fn drop(&mut self) {
            TERMINAL_ENGAGED.store(self.engaged, Ordering::SeqCst);
            TERMINAL_EVER_ENGAGED.store(self.ever_engaged, Ordering::SeqCst);
            SHUTDOWN_REQUESTED.store(self.requested, Ordering::SeqCst);
            SIGNALS_SEEN.store(self.signals, Ordering::SeqCst);
        }
    }

    /// The forced-exit path skips destructors, so whatever a session
    /// registered has to be what actually runs — and it has to stop running
    /// once that session is gone, or a later forced exit would reach into a
    /// process that no longer exists.
    ///
    /// `force_exit` itself cannot be called here (it ends the process), so
    /// this drives `run_forced_exit_cleanup`, the part of it that does the
    /// work.
    #[test]
    fn forced_exit_cleanup_runs_while_registered_and_not_after() {
        use std::sync::Arc;
        use std::sync::atomic::AtomicUsize;

        let calls = Arc::new(AtomicUsize::new(0));

        // Nothing registered yet — every command other than an attached
        // session is in this state, and the forced path must simply do
        // nothing rather than fail. Checked here rather than in a test of its
        // own: both would touch the same global slot, and running in parallel
        // they would corrupt each other's count.
        run_forced_exit_cleanup();
        assert_eq!(calls.load(Ordering::SeqCst), 0);

        {
            let counter = Arc::clone(&calls);
            let _guard = on_forced_exit(move || {
                counter.fetch_add(1, Ordering::SeqCst);
            });

            run_forced_exit_cleanup();
            assert_eq!(
                calls.load(Ordering::SeqCst),
                1,
                "registered cleanup must run"
            );
        }

        // The guard has dropped with the session it belonged to.
        run_forced_exit_cleanup();
        assert_eq!(
            calls.load(Ordering::SeqCst),
            1,
            "cleanup must not outlive the guard that registered it"
        );

        // --- two registrations at once ---------------------------------
        //
        // Continued in this test rather than written as its own, for the
        // reason given above: `run_forced_exit_cleanup` fires *every*
        // registered callback, so a sibling test running in parallel would
        // drive this one's counters and both would be nonsense.
        //
        // While the registry was a single `Option`, everything below failed:
        // registering `second` displaced `first`, and dropping either guard
        // unregistered the other. Two concurrent sessions would have left a
        // real harness orphaned on Ctrl-C with nothing reporting it.
        let first = Arc::new(AtomicUsize::new(0));
        let second = Arc::new(AtomicUsize::new(0));

        let first_guard = {
            let c = Arc::clone(&first);
            on_forced_exit(move || {
                c.fetch_add(1, Ordering::SeqCst);
            })
        };
        let second_guard = {
            let c = Arc::clone(&second);
            on_forced_exit(move || {
                c.fetch_add(1, Ordering::SeqCst);
            })
        };

        run_forced_exit_cleanup();
        assert_eq!(
            (first.load(Ordering::SeqCst), second.load(Ordering::SeqCst)),
            (1, 1),
            "a second registration must not displace the first"
        );

        // Dropping the newer guard must leave the older one registered.
        drop(second_guard);
        run_forced_exit_cleanup();
        assert_eq!(
            (first.load(Ordering::SeqCst), second.load(Ordering::SeqCst)),
            (2, 1),
            "dropping one guard must unregister only its own cleanup"
        );

        drop(first_guard);
        run_forced_exit_cleanup();
        assert_eq!(
            (first.load(Ordering::SeqCst), second.load(Ordering::SeqCst)),
            (2, 1),
            "the registry must be empty once every guard has dropped"
        );
    }

    #[test]
    fn restoring_an_unengaged_terminal_is_a_no_op() {
        let _turn = terminal_state_turn();
        // Must not touch the terminal or panic when the TUI never started.
        restore_terminal();
        restore_terminal();
        assert!(!TERMINAL_ENGAGED.load(Ordering::SeqCst));
    }
}
