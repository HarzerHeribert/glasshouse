//! What the shell does when a session's process ends, comes back, or turns
//! out never to have been there.
//!
//! The invariant these three functions share: **nothing on screen and nothing
//! the keyboard reaches may outlive the process it describes.** A
//! [`crate::session::LiveSession`] deliberately outlives its process — the
//! runtime keeps it so scrollback and a crash report stay readable — so its
//! emulator still answers with a full grid of blanks and `Mode::Session`
//! still forwards keystrokes into a closed pty. Neither stops being true on
//! its own, so each is re-decided here on every tick — the keyboard against
//! the runtime's own liveness, the viewport against what is actually written
//! on the screen, which is not the same test and [`viewport_grid`] says why.
//!
//! Split out of `shell/mod.rs` rather than added to it: that file is at the
//! Phase 59 ceiling and is a dispatch and composition layer, and every
//! decision below is one the run loop only calls.

use std::collections::HashMap;

use crate::session::{SessionId, SessionPresentation, SessionRuntime};

use super::build_viewport_grid;
use super::state::{Action, Mode, ShellState, ViewportGrid, short_session_id};

/// A session whose process is gone keeps the viewport only while it still has
/// something written on it — this is the grid the run loop hands the renderer
/// each tick.
///
/// `poll_exits` records a session's status and leaves the session in the
/// runtime, and a dead session's emulator still reports its full size, which
/// `ViewportGrid::is_empty` — a geometry test — calls non-empty. A harness
/// that leaves its alternate screen on the way out (`/exit` in Claude Code)
/// therefore handed the renderer a full grid of blanks, and the landing
/// surface underneath was never drawn: the void a user meets after quitting
/// the harness.
///
/// **Emptiness, not liveness, is the test, and the difference is a harness's
/// last words.** An ordinary program prints its result and exits in the same
/// breath — the output arrives and the process ends between two ticks — so
/// refusing every dead session's grid erases the one frame that mattered.
/// Three `pty_smoke` tests read exactly that output back out of the shell.
/// So a dead session keeps the viewport while anything is on it, and gives it
/// up the moment there is not.
///
/// Headless is filtered for the older, sharper reason — such a session has no
/// viewport at all — and the runtime's presentation is the authority there,
/// not the stored record. See `view::render_viewport` for the last line of
/// defence against a grid that is merely stale.
pub(super) fn viewport_grid(state: &ShellState, live: &SessionRuntime) -> ViewportGrid {
    let Some(session) = state
        .active_session()
        .and_then(|record| live.get(&record.id))
        .filter(|session| session.presentation() != SessionPresentation::Headless)
    else {
        return ViewportGrid::default();
    };
    let grid = session.with_screen(build_viewport_grid);
    if session.is_running() || shows_anything(&grid) {
        grid
    } else {
        ViewportGrid::default()
    }
}

/// Whether any cell of `grid` carries something a user could read.
///
/// Walked only for a session that has already exited, so the cost is paid
/// once per dead session per tick rather than on every frame of every live
/// one — a running session is the viewport whatever is on its screen, since
/// a harness that has not drawn yet is about to.
fn shows_anything(grid: &ViewportGrid) -> bool {
    (0..grid.rows()).any(|row| {
        (0..grid.cols()).any(|col| {
            grid.cell(row, col)
                .is_some_and(|(text, _)| !text.trim().is_empty())
        })
    })
}

/// Session mode never survives the death of the process the keyboard is
/// writing to — whichever session that is.
///
/// The cursor is not the keyboard. `sync_focus` declines to move focus onto a
/// session that is not live (`RuntimeError::NotLive` is ignored on purpose),
/// so Tab-ing onto another record leaves keystrokes going to the session that
/// is still running. Testing `active_session` alone therefore left session
/// mode alive after that session exited, forwarding into a closed pty with no
/// visible way out. `focused` is the runtime's own answer to "where do bytes
/// go", so it decides; the cursor is still tested because a presented session
/// exiting is the ordinary case and need never have taken focus.
///
/// `focused` must be read **before** `poll_exits`, which moves focus off a
/// session it finds ended: asked afterwards it names the successor, and this
/// test could then never match. The run loop's `attached_before` is that
/// read, and it is why the parameter is passed in rather than taken from the
/// runtime here.
///
/// Returns whether the frame must be redrawn.
pub(super) fn note_exit(
    state: &mut ShellState,
    focused: Option<&SessionId>,
    exited: &SessionId,
) -> bool {
    let keyboard_was_there = focused == Some(exited)
        || state
            .active_session()
            .is_some_and(|record| &record.id == exited);
    if !keyboard_was_there || state.session_exited() != Action::Redraw {
        return false;
    }
    // Said out loud: being returned to control mode with no explanation is
    // the same complaint as not being returned at all.
    state.set_status(format!(
        "session `{}` exited — back in control mode",
        short_session_id(exited)
    ));
    true
}

/// The safety net: session mode with nothing live behind it returns to
/// control mode on the next tick, saying so.
///
/// [`note_exit`] enforces the same invariant on the exit the runtime
/// reported; this enforces it against the runtime's own answer every tick, so
/// the ways a session can stop being reachable *without* a reported exit —
/// a record whose harness belongs to another Glasshouse invocation and was
/// never in this runtime, a `close`, a start that failed after the mode
/// changed — all end the same way instead of leaving keystrokes going
/// nowhere. Costs one `Option` comparison per tick in control mode.
pub(super) fn heal_orphaned_session_mode(state: &mut ShellState, live: &SessionRuntime) -> bool {
    if state.mode() != Mode::Session {
        return false;
    }
    let attached = live
        .focused()
        .and_then(|id| live.get(id))
        .filter(|session| session.is_running());
    if attached.is_some() {
        return false;
    }
    let note = match live.focused() {
        Some(id) => format!(
            "session `{}` is no longer running — back in control mode",
            short_session_id(id)
        ),
        None => "no session has the keyboard — back in control mode".to_string(),
    };
    state.session_exited();
    state.set_status(note);
    true
}

/// A restart the user never asked for is never silent.
///
/// Phase 10A's tenth line restarts a session that exits unexpectedly, and
/// `poll_exits` then drops that id from the exits it returns so no consumer
/// treats a session that is running again as ended. The consequence is that
/// the shell is told nothing at all: the process the user was talking to died
/// and another took its place behind the same id and the same viewport. This
/// reads the runtime's own restart counter rather than re-deriving the event,
/// and reports each increase once. The restart policy is untouched; only the
/// silence is.
pub(super) struct RestartWatch {
    seen: HashMap<SessionId, u32>,
}

impl RestartWatch {
    pub(super) fn new() -> Self {
        Self {
            seen: HashMap::new(),
        }
    }

    /// The sessions restarted since the last call.
    ///
    /// Rebuilt from the runtime each time rather than updated in place, so a
    /// session that has been closed leaves no entry behind to be compared
    /// against a later session that reuses nothing but the map.
    pub(super) fn observe(&mut self, live: &SessionRuntime) -> Vec<SessionId> {
        let mut restarted = Vec::new();
        let mut current = HashMap::with_capacity(live.sessions().len());
        for session in live.sessions() {
            let count = session.restarts();
            if count > self.seen.get(session.id()).copied().unwrap_or(0) {
                restarted.push(session.id().clone());
            }
            current.insert(session.id().clone(), count);
        }
        self.seen = current;
        restarted
    }
}
