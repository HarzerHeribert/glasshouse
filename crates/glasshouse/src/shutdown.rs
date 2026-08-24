//! Clean shutdown and terminal restoration.
//!
//! Glasshouse puts the terminal into raw mode and the alternate screen while
//! the TUI runs. Leaving it that way after an exit, a panic, or a signal leaves
//! the user with an unusable shell, so restoration is centralised here and
//! wired into all three paths.

use std::io::Write;
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::{Context, Result};
use crossterm::terminal::{EnterAlternateScreen, LeaveAlternateScreen};
use crossterm::{cursor, execute, terminal};

/// True while the terminal is in raw mode / on the alternate screen.
static TERMINAL_ENGAGED: AtomicBool = AtomicBool::new(false);
/// Set when a signal has asked the application to wind down.
static SHUTDOWN_REQUESTED: AtomicBool = AtomicBool::new(false);

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
            restore_terminal();
            std::process::exit(EXIT_INTERRUPTED);
        }
        if SHUTDOWN_REQUESTED.swap(true, Ordering::SeqCst) {
            restore_terminal();
            std::process::exit(EXIT_INTERRUPTED);
        }
    })
    .context("could not install signal handler")
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

    #[test]
    fn restoring_an_unengaged_terminal_is_a_no_op() {
        // Must not touch the terminal or panic when the TUI never started.
        restore_terminal();
        restore_terminal();
        assert!(!TERMINAL_ENGAGED.load(Ordering::SeqCst));
    }
}
