//! Opening Settings never stops the interface.
//!
//! The invariant: no keystroke handler calls [`super::build_settings`].
//! That builder runs [`crate::integrations::Discovery`], which **spawns every
//! enabled harness binary and waits for each one's version output** — measured
//! here at 1.05–1.49s with six Node CLIs enabled, and reported at about four
//! seconds on a warm machine. On the thread that reads keys and draws frames
//! that is a frozen terminal: the artwork stops, nothing repaints, and the
//! interface is indistinguishable from a hung process. The same defect class
//! `spawn_provider_probe` and `spawn_event_tail` already exist for, and this
//! follows their shape rather than inventing a second one — a thread, a
//! channel, a wake-up, and a drain in the run loop.
//!
//! **What the user sees between the keypress and the rows is a note, not an
//! empty Settings.** Settings is an editor addressed by cursor position:
//! rows arriving after it opened would slide under the user's fingers, so a
//! keystroke aimed at one harness lands on another. An empty harness list is
//! also indistinguishable from "nothing is installed", which is a false
//! answer the user can act on — disabling a harness they have. A note names
//! what is being waited for and leaves every row correct on arrival.

use std::sync::mpsc::{Receiver, Sender};

use super::{Mode, Overlay, SettingsRows, ShellState};
use crate::Runtime;
use crate::tui::AppEvent;

/// Where the interface was when Settings was asked for.
///
/// The invariant it guards: a build started for the screen the user was
/// looking at is not allowed to take over a different one. Between the
/// keypress and the rows the user can open another overlay or focus a
/// session, and an overlay that appears over the top of that answers a
/// question nobody is still asking.
/// Why the rows were asked for, and so what to do when they land.
///
/// The same slow build serves both, and the difference is one call: opening
/// puts the overlay up, refreshing replaces the rows under an overlay that is
/// already there and clears the edits that were just written.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Purpose {
    Open,
    Refresh,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) struct Placement {
    mode: Mode,
    overlay: Option<Overlay>,
    purpose: Purpose,
}

impl Placement {
    pub(super) fn of(state: &ShellState, purpose: Purpose) -> Self {
        Self {
            mode: state.mode(),
            overlay: state.overlay(),
            purpose,
        }
    }

    /// Where the rows land is the request's business, not the screen's, so
    /// this is compared apart from the placement itself.
    fn matches_screen(self, state: &ShellState) -> bool {
        self.mode == state.mode() && self.overlay == state.overlay()
    }
}

/// Start building Settings' rows on a thread of its own, and say so.
///
/// A request already in flight is not duplicated: pressing `s` again
/// re-records the placement instead, so a user who wandered off and asked
/// again still gets the overlay when the one running build lands.
pub(super) fn request_settings(
    runtime: &Runtime,
    state: &mut ShellState,
    pending: &mut Option<Placement>,
    results: &Sender<anyhow::Result<SettingsRows>>,
    wake: &Sender<AppEvent>,
    purpose: Purpose,
) {
    if pending.is_some() {
        *pending = Some(Placement::of(state, purpose));
        state.set_status("settings: still checking harness versions…");
        return;
    }

    // Cloned rather than borrowed: `Runtime` is plain owned data, and the
    // thread outlives this call by design.
    let runtime = runtime.clone();
    let results = results.clone();
    let wake = wake.clone();

    let started = std::thread::Builder::new()
        .name("glasshouse-settings".to_owned())
        .spawn(move || {
            let rows = super::build_settings(&runtime);
            // A send failure means the shell has already gone — the rows are
            // dropped, correct for an overlay nobody can be shown.
            if results.send(rows).is_ok() {
                let _ = wake.send(AppEvent::Redraw);
            }
        });

    match started {
        Ok(_handle) => {
            *pending = Some(Placement::of(state, purpose));
            state.set_status("settings: checking harness versions…");
        }
        Err(err) => {
            // Reported rather than silently retried on this thread, which is
            // the thread this whole module exists to keep free.
            tracing::warn!(error = %err, "could not start the settings build");
            state.set_status(format!("could not open settings: {err}"));
        }
    }
}

/// Hand a finished build to the overlay. Returns whether a frame is owed.
///
/// The rows are applied only where they were asked for; a build that lands
/// after the user moved elsewhere is dropped, and the request is over either
/// way — nothing is left in flight for a result that already arrived.
pub(super) fn drain_settings(
    inbox: &Receiver<anyhow::Result<SettingsRows>>,
    state: &mut ShellState,
    pending: &mut Option<Placement>,
) -> bool {
    let mut redraw = false;
    while let Ok(rows) = inbox.try_recv() {
        let asked_from = pending.take();
        match rows {
            Ok((harnesses, integrations, providers, profiles, routing, memory)) => {
                let Some(asked_from) = asked_from.filter(|p| p.matches_screen(state)) else {
                    tracing::debug!("settings finished building after the user moved on");
                    continue;
                };
                match asked_from.purpose {
                    Purpose::Open => {
                        state.open_settings_with_routing(
                            harnesses,
                            integrations,
                            providers,
                            profiles,
                            routing,
                            memory,
                        );
                        // Replaces the "checking…" note, which is no longer true.
                        state.set_status("settings ready");
                    }
                    Purpose::Refresh => {
                        state.refresh_settings_with_routing(
                            harnesses,
                            integrations,
                            providers,
                            profiles,
                            routing,
                            memory,
                        );
                    }
                }
                redraw = true;
            }
            Err(err) => {
                tracing::warn!(error = %err, "could not open settings");
                state.set_status(format!("could not open settings: {err:#}"));
                redraw = true;
            }
        }
    }
    redraw
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shell::{MemoryRow, RoutingRow};

    fn shell_state() -> ShellState {
        ShellState::new("demo", "/tmp/demo", "0.0.0", Vec::new())
    }

    fn rows() -> SettingsRows {
        (
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            RoutingRow::defaults(Vec::new()),
            MemoryRow::defaults(),
        )
    }

    /// The defect this module exists for: `s` must not build Settings on the
    /// thread that draws. Measured before the fix at 1.05–1.49s of a terminal
    /// receiving no bytes at all.
    #[test]
    fn the_open_settings_arm_does_not_build_settings_on_the_drawing_thread() {
        let source = include_str!("mod.rs").replace("\r\n", "\n");
        let start = source
            .find("Action::OpenSettings =>")
            .expect("the run loop still has an OpenSettings arm");
        let end = source[start..]
            .find("Action::OpenProjectOverview =>")
            .expect("OpenProjectOverview still follows OpenSettings")
            + start;
        let arm = &source[start..end];
        assert!(
            arm.contains("request_settings"),
            "the arm must still hand the build to this module, or this scan watches nothing"
        );
        assert!(
            !arm.contains("build_settings("),
            "`build_settings` runs every harness binary; calling it from a key handler \
             freezes the terminal for as long as the slowest harness takes to answer"
        );
    }

    #[test]
    fn rows_that_arrive_where_they_were_asked_for_open_settings() {
        let mut state = shell_state();
        let mut pending = Some(Placement::of(&state, Purpose::Open));
        let (tx, rx) = std::sync::mpsc::channel();
        tx.send(Ok(rows())).unwrap();

        assert!(drain_settings(&rx, &mut state, &mut pending));
        assert_eq!(state.overlay(), Some(Overlay::Settings));
        assert!(pending.is_none(), "the request is answered");
    }

    /// The guard, and the reason a placement is recorded at all: the overlay
    /// must not appear over whatever the user opened while waiting.
    #[test]
    fn rows_that_arrive_after_the_user_moved_on_are_dropped() {
        let mut state = shell_state();
        // Asked for from the Overview; the user has since left it.
        let mut pending = Some(Placement {
            mode: state.mode(),
            overlay: Some(Overlay::Overview),
            purpose: Purpose::Open,
        });
        let (tx, rx) = std::sync::mpsc::channel();
        tx.send(Ok(rows())).unwrap();

        assert!(!drain_settings(&rx, &mut state, &mut pending));
        assert_eq!(
            state.overlay(),
            None,
            "nothing may open over the new screen"
        );
        assert!(pending.is_none(), "the request is over either way");
    }

    /// A real runtime, isolated to two temporary directories, the same way
    /// `shell::tests` builds one. Needed only because `request_settings`
    /// takes one — the case below returns before it is read.
    fn isolated_runtime() -> (tempfile::TempDir, tempfile::TempDir, Runtime) {
        use clap::Parser as _;

        let data = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(workspace.path().join(".git")).unwrap();
        let cli = crate::Cli::try_parse_from([
            "glasshouse",
            "--data-dir",
            data.path().to_str().unwrap(),
            "--config-dir",
            data.path().to_str().unwrap(),
        ])
        .unwrap();
        let runtime = crate::bootstrap(&cli, workspace.path()).unwrap();
        (data, workspace, runtime)
    }

    /// Holding `s` down must not spawn a thread per repeat, each one running
    /// every harness binary again.
    #[test]
    fn a_second_press_while_a_build_is_running_starts_no_second_build() {
        let (_data, _workspace, runtime) = isolated_runtime();
        let mut state = shell_state();
        let already = Placement::of(&state, Purpose::Open);
        let mut pending = Some(already);
        let (results, _inbox) = std::sync::mpsc::channel();
        let (wake, wake_inbox) = std::sync::mpsc::channel();

        request_settings(
            &runtime,
            &mut state,
            &mut pending,
            &results,
            &wake,
            Purpose::Open,
        );

        assert_eq!(
            pending,
            Some(already),
            "a repeat press re-records the placement rather than starting a build"
        );
        assert_eq!(
            state.status(),
            Some("settings: still checking harness versions\u{2026}")
        );
        assert!(
            wake_inbox.try_recv().is_err(),
            "no second build means no second wake-up"
        );
    }
}
