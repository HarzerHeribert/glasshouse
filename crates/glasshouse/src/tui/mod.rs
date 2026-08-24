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
pub struct Screen {
    terminal: ratatui::Terminal<CrosstermBackend<Stdout>>,
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

        Ok(Self {
            terminal,
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
        // `_guard` restores raw mode and the alternate screen after this.
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
