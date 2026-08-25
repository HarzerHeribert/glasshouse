//! The main interactive interface.
//!
//! The shell is what `glasshouse` opens with no arguments: a persistent top bar
//! naming the project and its canonical root, a session bar listing the
//! project's sessions, a viewport reserved for the active session's terminal,
//! and a session overview a keystroke away.
//!
//! Split the same way the first-run wizard is — [`state`] answers keys without
//! drawing, [`view`] draws without deciding anything — so the interesting
//! behaviour is testable without a terminal, and the run loop below stays small
//! enough to read in one sitting.
//!
//! What the shell deliberately does **not** do yet: embed a live harness
//! terminal in the viewport. That is Phase 5, and pretending otherwise would
//! mean drawing a convincing empty terminal for a session that is not attached.
//! The viewport reserves the space and says what will fill it.

pub mod state;
pub mod view;

use anyhow::Result;

use crate::Runtime;
use crate::session::ProjectSessions;
use crate::tui::{AppEvent, DEFAULT_TICK, Event, EventSource, Screen};

pub use state::{Action, Overlay, ShellState};

/// Open the shell and run it until the user leaves.
///
/// Sessions are read once at startup and re-read whenever the event loop is
/// nudged. Nothing here starts or stops a process: leaving the shell leaves
/// every session exactly as it was, which is why the record is durable in the
/// first place.
pub fn run(runtime: &Runtime) -> Result<()> {
    let sessions = ProjectSessions::open(runtime)?;
    let records = sessions.store().list()?;

    let project = runtime.project();
    let mut state = ShellState::new(
        project.name(),
        project.display_root(),
        crate::VERSION,
        records,
    );

    // Acquired after the database work above, so a failure there leaves the
    // user's terminal untouched rather than flashing an alternate screen.
    let mut screen = Screen::acquire()?;
    let events = EventSource::new(DEFAULT_TICK);

    screen.draw(|frame| view::render(&state, frame))?;

    loop {
        match events.next()? {
            Event::Key(key) => match state.handle_key(key) {
                Action::None => {}
                Action::Redraw => screen.draw(|frame| view::render(&state, frame))?,
                Action::Quit => return Ok(()),
            },
            Event::Resize(cols, rows) => {
                screen.on_resize(cols, rows)?;
                screen.draw(|frame| view::render(&state, frame))?;
            }
            Event::Tick => {
                // A signal is the only thing that ends the shell other than a
                // key, and it has to be noticed between keystrokes rather than
                // only when one arrives.
                if crate::shutdown::shutdown_requested() {
                    return Ok(());
                }
            }
            Event::Shutdown => return Ok(()),
            Event::App(AppEvent::Redraw) => {
                // Something outside the terminal changed. Re-read the records
                // rather than trusting the sender to describe what moved; the
                // list is small and the alternative is a second source of truth.
                if state.refresh(sessions.store().list()?) == Action::Redraw {
                    screen.draw(|frame| view::render(&state, frame))?;
                }
            }
            Event::Paste(_) | Event::Mouse(_) => {}
        }
    }
}
