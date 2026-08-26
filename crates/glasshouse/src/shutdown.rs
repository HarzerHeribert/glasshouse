//! Clean shutdown and terminal restoration.
//!
//! Glasshouse puts the terminal into raw mode and the alternate screen while
//! the TUI runs. Leaving it that way after an exit, a panic, or a signal leaves
//! the user with an unusable shell, so restoration is centralised here and
//! wired into all three paths.

use std::io::Write;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use anyhow::{Context, Result};
use crossterm::terminal::{EnterAlternateScreen, LeaveAlternateScreen};
use crossterm::{cursor, execute, terminal};

/// True while the terminal is in raw mode / on the alternate screen.
static TERMINAL_ENGAGED: AtomicBool = AtomicBool::new(false);
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
/// While the terminal is not engaged, a signal exits immediately, matching what
/// a user expects from a normal CLI command. While the TUI owns the terminal
/// the first signal asks for a graceful shutdown and a second one forces the
/// process down, restoring the terminal either way.
pub fn install_signal_handler() -> Result<()> {
    ctrlc::set_handler(|| {
        if !TERMINAL_ENGAGED.load(Ordering::SeqCst) {
            force_exit();
        }
        if SHUTDOWN_REQUESTED.swap(true, Ordering::SeqCst) {
            force_exit();
        }
    })
    .context("could not install signal handler")
}

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
        TERMINAL_ENGAGED.store(true, Ordering::SeqCst);

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
/// one would put the session's own output on a screen that is thrown away
/// when Glasshouse leaves it — the user would watch their session scroll by
/// and then see it vanish on exit.
///
/// Raw mode is what the session genuinely does need: it stops the local line
/// discipline from buffering lines, echoing keystrokes, and — the part that
/// matters most — turning Ctrl-C into a signal for *Glasshouse*. In raw mode
/// Ctrl-C arrives as a plain `0x03` byte that gets forwarded to the harness,
/// which is exactly where it belongs while a session owns the terminal.
///
/// Restoration goes through the same [`restore_terminal`] and
/// `TERMINAL_ENGAGED` flag as [`TerminalGuard`], so a panic or a signal
/// restores a raw-mode session just as reliably as a full-screen one.
///
/// That shared path also emits `LeaveAlternateScreen` on the way out, which
/// this guard never entered. That is deliberate, not an oversight to tidy
/// away: the *harness* very likely entered one, and a harness that dies
/// without leaving it would otherwise strand the user on a screen they
/// cannot get off. Sending it unconditionally repairs that case and costs
/// nothing in the case where no alternate screen was ever entered.
#[derive(Debug)]
pub struct RawModeGuard {
    _private: (),
}

impl RawModeGuard {
    /// Put the terminal into raw mode.
    pub fn acquire() -> Result<Self> {
        terminal::enable_raw_mode().context("could not enable raw terminal mode")?;
        TERMINAL_ENGAGED.store(true, Ordering::SeqCst);
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
        // Must not touch the terminal or panic when the TUI never started.
        restore_terminal();
        restore_terminal();
        assert!(!TERMINAL_ENGAGED.load(Ordering::SeqCst));
    }
}
