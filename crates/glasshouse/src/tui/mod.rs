//! Terminal user interface runtime.
//!
//! This module owns the terminal while Glasshouse is interactive: it puts it
//! into raw mode and the alternate screen, gives callers a Ratatui drawing
//! surface, and restores everything on the way out — including after a panic
//! or a signal, through [`crate::shutdown`].
//!
//! It deliberately holds no application state. Screens (the first-run wizard,
//! the session shell, settings) own their own state and drive their own loop
//! against [`Screen`]; keeping the runtime state-free is what lets the wizard
//! run before the main interface exists without either knowing about the
//! other.

pub mod event;

use std::io::{Stdout, Write, stdout};
use std::mem::ManuallyDrop;
use std::time::Duration;

use anyhow::{Context, Result};
use crossterm::event::{
    DisableBracketedPaste, EnableBracketedPaste, KeyCode, KeyEvent, KeyModifiers,
};
use crossterm::execute;
use ratatui::Frame;
use ratatui::backend::CrosstermBackend;

pub use event::{AppEvent, Event, EventSource};

use crate::pty::TerminalSize;
use crate::shutdown::TerminalGuard;

/// How long the interface waits for input before regaining control.
///
/// Short enough that a session's output does not visibly lag behind the
/// process producing it, long enough that an idle Glasshouse is not spinning.
pub const DEFAULT_TICK: Duration = Duration::from_millis(16);

/// The terminal Glasshouse draws on.
///
/// Dropping a `Screen` restores the terminal. The [`TerminalGuard`] it holds
/// does the work, so the same restoration happens on a normal return, an
/// error, a panic, and a signal.
///
/// `terminal` is wrapped in [`ManuallyDrop`] so [`Screen`]'s own `Drop` can
/// take it and drop it under a caught panic — see that impl for why.
pub struct Screen {
    terminal: ManuallyDrop<ratatui::Terminal<CrosstermBackend<Stdout>>>,
    _guard: TerminalGuard,
}

impl Screen {
    /// Take over the terminal.
    pub fn acquire() -> Result<Self> {
        let guard = TerminalGuard::acquire()?;

        // Bracketed paste lets a multi-line paste arrive as one event instead
        // of a burst of keystrokes. Enabled after the guard so that failing
        // here still restores the terminal.
        let mut out = stdout();
        if let Err(e) = execute!(out, EnableBracketedPaste) {
            tracing::debug!(error = %e, "terminal does not support bracketed paste");
        }

        let backend = CrosstermBackend::new(stdout());
        let terminal =
            ratatui::Terminal::new(backend).context("could not initialise the terminal")?;

        // Armed here rather than beside the event loop, because this is where
        // the terminal is actually taken over and the watchdog's question is
        // "does Glasshouse still owe someone an interface?" rather than "is a
        // loop running?" — it has to outlast the loop to cover the wind-down
        // after it. A screen may be acquired, dropped and acquired again (the
        // wizard handing over to the shell); arming is idempotent, and there
        // is deliberately no disarm. See `event::arm_hangup_watchdog`.
        event::arm_hangup_watchdog();

        Ok(Self {
            terminal: ManuallyDrop::new(terminal),
            _guard: guard,
        })
    }

    /// Draw one frame.
    pub fn draw(&mut self, render: impl FnOnce(&mut Frame)) -> Result<()> {
        self.terminal
            .draw(render)
            .context("could not draw to the terminal")?;
        Ok(())
    }

    /// Current terminal size.
    pub fn size(&self) -> Result<TerminalSize> {
        let area = self
            .terminal
            .size()
            .context("could not read the terminal size")?;
        Ok(TerminalSize::new(area.height, area.width))
    }

    /// Tell Ratatui the terminal was resized, discarding its cached buffer.
    ///
    /// Ratatui compares each frame against the previous one to decide what to
    /// redraw. After a resize that cached buffer describes a different
    /// geometry, so it has to be thrown away or the next frame will be drawn
    /// from a stale diff.
    pub fn on_resize(&mut self, cols: u16, rows: u16) -> Result<()> {
        self.terminal
            .resize(ratatui::layout::Rect::new(0, 0, cols, rows))
            .context("could not apply the new terminal size")?;
        Ok(())
    }
}

impl Drop for Screen {
    fn drop(&mut self) {
        let mut out = stdout();
        let _ = execute!(out, DisableBracketedPaste);
        let _ = out.flush();

        // SAFETY: `self.terminal` is taken exactly once, here, in the one
        // place a `Screen` is ever dropped.
        let terminal = unsafe { ManuallyDrop::take(&mut self.terminal) };
        drop_terminal_tolerantly(terminal);
        // `_guard` restores raw mode and the alternate screen after this.
    }
}

/// Drop a Ratatui terminal without letting its own `Drop` impl's panic
/// escape.
///
/// Ratatui's own `Drop for Terminal` shows the cursor if Ratatui last left it
/// hidden, and panics — an `eprintln!` behind an `.expect` — if that write
/// fails, which it does once the terminal is gone. A `Screen` that already
/// returned cleanly from its own loop must still leave the process exiting
/// 0, so that panic is caught here instead of being let out: this drops the
/// terminal under `catch_unwind` rather than reaching into Ratatui to fix it
/// there — Glasshouse does not own that crate.
///
/// A free function taking the terminal by value, rather than inline in
/// [`Screen`]'s `Drop`, so `dropping_a_terminal_that_writes_on_drop_does_not_
/// panic` below can prove this without a real terminal, a real pty, or the
/// signal race described on that test.
fn drop_terminal_tolerantly<B: ratatui::backend::Backend>(terminal: ratatui::Terminal<B>) {
    if std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| drop(terminal))).is_err() {
        tracing::debug!(
            "the terminal's own drop panicked on a terminal that had already gone away"
        );
    }
}

/// True for the key combinations that mean "leave" at a Glasshouse prompt.
///
/// Ctrl-C is included because at a Glasshouse-owned screen there is no harness
/// to hand it to. Once a native session owns the input, Ctrl-C belongs to that
/// session instead, and this must not be consulted.
pub fn is_quit_key(key: &KeyEvent) -> bool {
    matches!(key.code, KeyCode::Esc)
        || (key.modifiers.contains(KeyModifiers::CONTROL)
            && matches!(key.code, KeyCode::Char('c') | KeyCode::Char('C')))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A writer that behaves normally until told not to, then panics on
    /// every write — standing in for what a hung-up terminal does to
    /// Ratatui's real `Drop for Terminal`, without needing a real pty, a
    /// real signal, or the shared `SIGHUP` race described on the test below
    /// (this packet's own report has the detail: closing a real terminal
    /// races Glasshouse's own signal handling, and on this development
    /// machine that race — not either terminal-loss defect — decided most
    /// attempts).
    ///
    /// A panic here rather than an `io::Error`, and deliberately not the
    /// same shape as the real bug: the real one is Ratatui's own
    /// `Drop for Terminal` swallowing a failed write and then panicking
    /// itself, via `eprintln!`'s `.expect`, when it reports that failure —
    /// and reproducing *that* exact path needs the write to reach a broken
    /// process-wide standard error, which cargo's own multi-threaded test
    /// harness shares across every test in this binary and is not safe to
    /// break for one of them. `drop_terminal_tolerantly` does not know or
    /// care why the terminal's `Drop` panicked, only that it might — so a
    /// writer that panics directly proves the thing that actually matters
    /// here (a panic unwinding out of `Terminal<B>`'s own drop does not
    /// escape) without needing the real trigger.
    struct PanicsOnWrite(std::rc::Rc<std::cell::Cell<bool>>);

    impl Write for PanicsOnWrite {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            assert!(!self.0.get(), "the terminal is gone");
            Ok(buf.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            assert!(!self.0.get(), "the terminal is gone");
            Ok(())
        }
    }

    /// Build a terminal over a [`PanicsOnWrite`], draw one frame while it
    /// still works (leaving the cursor hidden, same as most screens —
    /// Ratatui hides it whenever a frame does not call
    /// `set_cursor_position`), then arm the panic. Returns the terminal
    /// primed to panic on the very next write its own `Drop` makes, which is
    /// exactly the write `Drop for Terminal` makes trying to show that
    /// hidden cursor again.
    ///
    /// A fixed viewport, not `Screen`'s own `Viewport::Fullscreen`: the
    /// fullscreen viewport asks the backend for the real terminal size on
    /// construction and on every draw, which `PanicsOnWrite` cannot answer
    /// and a `cargo test` process usually has none of anyway. Fixed
    /// sidesteps that entirely, and nothing about which viewport is in use
    /// changes whether the cursor was left hidden — the one thing this is
    /// about.
    fn primed_to_panic_on_drop() -> ratatui::Terminal<CrosstermBackend<PanicsOnWrite>> {
        let armed = std::rc::Rc::new(std::cell::Cell::new(false));
        let mut terminal = ratatui::Terminal::with_options(
            CrosstermBackend::new(PanicsOnWrite(armed.clone())),
            ratatui::TerminalOptions {
                viewport: ratatui::Viewport::Fixed(ratatui::layout::Rect::new(0, 0, 10, 10)),
            },
        )
        .expect("construct a terminal over a writer that still works");
        terminal
            .draw(|_frame| {})
            .expect("draw while the writer still works");
        armed.set(true);
        terminal
    }

    /// Confirms this test file's own premise before relying on it below: a
    /// `Terminal` left with its cursor hidden really does write on drop, so
    /// `primed_to_panic_on_drop` panics unwrapped. If a future Ratatui
    /// change stopped writing there — the write being the point, not the
    /// panic — this, not the test that depends on catching one, is what
    /// would fail, which is the more useful place for that to show up.
    #[test]
    fn a_terminal_left_with_its_cursor_hidden_writes_again_on_drop() {
        let terminal = primed_to_panic_on_drop();
        let panicked = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| drop(terminal)));
        assert!(
            panicked.is_err(),
            "dropping a terminal left with its cursor hidden did not write to the backend — \
             the premise `drop_terminal_tolerantly` is proven against no longer holds"
        );
    }

    /// The acceptance test for defect 2, proven directly against the
    /// mechanism rather than through a real terminal: see
    /// `primed_to_panic_on_drop` and `PanicsOnWrite`'s doc comment for why a
    /// real pty cannot reliably prove this on its own.
    #[test]
    fn dropping_a_terminal_that_writes_on_drop_does_not_panic() {
        drop_terminal_tolerantly(primed_to_panic_on_drop());
    }

    #[test]
    fn quit_keys_are_escape_and_ctrl_c() {
        assert!(is_quit_key(&KeyEvent::new(
            KeyCode::Esc,
            KeyModifiers::NONE
        )));
        assert!(is_quit_key(&KeyEvent::new(
            KeyCode::Char('c'),
            KeyModifiers::CONTROL
        )));
        assert!(!is_quit_key(&KeyEvent::new(
            KeyCode::Char('c'),
            KeyModifiers::NONE
        )));
        assert!(!is_quit_key(&KeyEvent::new(
            KeyCode::Enter,
            KeyModifiers::NONE
        )));
    }
}
