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
//! This is where a [`crate::session::SessionRuntime`] is actually owned: the
//! shell is the one place that holds several live harnesses at once and gives
//! one of them the keyboard. See
//! `.agent-runtime/design-shell-session-modes.md` for how the keyboard is
//! divided between Glasshouse and the focused session's PTY — [`state::Mode`]
//! is the switch it hangs on.
//!
//! The viewport shows the focused session's own scrollback once it has
//! produced any — raw bytes, not a rendered terminal, since Glasshouse does
//! not emulate one yet (see [`crate::session::runtime::Scrollback`]'s doc
//! comment). That emulation is Phase 5; this is only the plumbing that gets
//! keystrokes and output flowing in both directions.

pub mod state;
pub mod view;

use anyhow::Result;

use crate::Runtime;
use crate::config::{self, EffectiveConfig, UserConfig};
use crate::integrations::{Discovery, IntegrationId, IntegrationKind, IntegrationStatus};
use crate::launch::HarnessLaunch;
use crate::pty::TerminalSize;
use crate::session::{
    self, LiveSession, NewSession, ProjectSessions, RuntimeError, SessionLifecycle, SessionRuntime,
};
use crate::tui::{AppEvent, DEFAULT_TICK, Event, EventSource, Screen};

pub use state::{Action, HarnessRow, IntegrationRow, Mode, Overlay, SettingsEdit, ShellState};

/// Open the shell and run it until the user leaves.
///
/// Session *records* are read once at startup and re-read whenever the event
/// loop is nudged. The [`SessionRuntime`] built here starts out empty:
/// leaving the shell leaves every session it started exactly as it was — none
/// are stopped on the way out — and a session recorded on a previous run is
/// not automatically live again just because its row is in the bar; only `n`
/// or a resume starts a process.
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
    let mut live = SessionRuntime::new();

    // Acquired after the database work above, so a failure there leaves the
    // user's terminal untouched rather than flashing an alternate screen.
    let mut screen = Screen::acquire()?;
    let events = EventSource::new(DEFAULT_TICK);

    screen.draw(|frame| view::render(&state, frame))?;

    loop {
        match events.next()? {
            Event::Key(key) => {
                let action = state.handle_key(key);
                match &action {
                    Action::None | Action::Redraw => {}
                    Action::Quit => return Ok(()),
                    Action::Forward(bytes) => {
                        if let Err(err) = live.write_to_focused(bytes) {
                            tracing::warn!(
                                %err,
                                "could not forward a keystroke to the focused session"
                            );
                        }
                    }
                    Action::StartSession => match start_session(
                        runtime,
                        &mut live,
                        &sessions,
                        screen.size().unwrap_or_default(),
                    ) {
                        Ok(()) => {
                            if let Ok(records) = sessions.store().list() {
                                state.refresh(records);
                            }
                        }
                        Err(err) => {
                            tracing::warn!(error = %err, "could not start a session");
                            state.set_status(format!("could not start a session: {err:#}"));
                        }
                    },
                    Action::OpenSettings => match build_settings(runtime) {
                        Ok((harnesses, integrations)) => {
                            state.open_settings(harnesses, integrations);
                        }
                        Err(err) => {
                            tracing::warn!(error = %err, "could not open settings");
                            state.set_status(format!("could not open settings: {err:#}"));
                        }
                    },
                    Action::SaveUserSettings => {
                        let edits = state.settings_edits();
                        if edits.is_empty() {
                            state.set_status("no settings changes to save");
                        } else if let Err(err) = save_user_settings(runtime, &edits) {
                            tracing::warn!(error = %err, "could not save user settings");
                            state.set_status(format!("could not save settings: {err:#}"));
                        } else {
                            state.set_status("saved to user configuration");
                            refresh_settings_after_save(runtime, &mut state);
                        }
                    }
                    Action::SaveProjectSettings => {
                        let edits = state.settings_edits();
                        if edits.is_empty() {
                            state.set_status("no settings changes to save");
                        } else {
                            match save_project_settings(runtime, &edits) {
                                Ok(path) => {
                                    state.set_status(format!("saved to {}", path.display()));
                                    refresh_settings_after_save(runtime, &mut state);
                                }
                                Err(err) => {
                                    tracing::warn!(
                                        error = %err,
                                        "could not save project settings"
                                    );
                                    state.set_status(format!(
                                        "could not save project settings: {err:#}"
                                    ));
                                }
                            }
                        }
                    }
                }
                sync_focus(&mut live, &state);
                if !matches!(action, Action::None) {
                    screen.draw(|frame| view::render(&state, frame))?;
                }
            }
            Event::Resize(cols, rows) => {
                screen.on_resize(cols, rows)?;
                if let Some(id) = live.focused().cloned()
                    && let Err(err) = live.resize(&id, TerminalSize::new(rows, cols))
                {
                    tracing::warn!(session = %id, %err, "could not resize the focused session");
                }
                screen.draw(|frame| view::render(&state, frame))?;
            }
            Event::Tick => {
                // A signal is the only thing that ends the shell other than a
                // key, and it has to be noticed between keystrokes rather than
                // only when one arrives.
                if crate::shutdown::shutdown_requested() {
                    return Ok(());
                }

                let mut redraw = false;
                for (id, status) in live.poll_exits() {
                    let lifecycle = if status.success() {
                        SessionLifecycle::Stopped
                    } else {
                        SessionLifecycle::Failed
                    };
                    if let Err(err) = sessions.store().set_lifecycle(&id, lifecycle) {
                        tracing::warn!(session = %id, %err, "could not record a session's exit");
                    }
                    // Session mode with the just-exited session presented has
                    // nowhere left to send keystrokes.
                    if state.active_session().is_some_and(|record| record.id == id)
                        && state.session_exited() == Action::Redraw
                    {
                        redraw = true;
                    }
                }

                let text = state
                    .active_session()
                    .and_then(|record| live.get(&record.id))
                    .map(LiveSession::scrollback)
                    .unwrap_or_default();
                if text != state.viewport() {
                    state.set_viewport(text);
                    redraw = true;
                }

                if redraw {
                    screen.draw(|frame| view::render(&state, frame))?;
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

/// Bring the runtime's focus in line with whichever session the bar is
/// presenting.
///
/// `RuntimeError::NotLive` is ignored on purpose: a session the bar lists but
/// that is not running in this `Glasshouse` invocation (recorded on a past
/// run, say) is normal, not a bookkeeping failure. This never touches a
/// process — see [`SessionRuntime::focus`]'s doc comment — it only ever
/// changes which live session the keyboard reaches.
fn sync_focus(live: &mut SessionRuntime, state: &ShellState) {
    let Some(active) = state.active_session() else {
        return;
    };
    if live.focused() == Some(&active.id) {
        return;
    }
    match live.focus(&active.id) {
        Ok(()) | Err(RuntimeError::NotLive { .. }) => {}
        Err(err) => tracing::warn!(session = %active.id, %err, "could not focus a session"),
    }
}

/// Resolve a harness, record a new session, and start it — the same
/// selection seam `main.rs: launch_session` uses for `glasshouse launch`,
/// minus attaching to this process's own terminal: the shell attaches by
/// giving the session the viewport once its output starts arriving, instead.
///
/// `size` is the shell's own terminal size at the moment `n` was pressed, not
/// the default `HarnessLaunch` would otherwise use: a harness TUI lays itself
/// out from the size it sees at startup, so starting it at 24x80 and resizing
/// afterwards would draw its first frame for the wrong geometry — see
/// `HarnessLaunch::size`'s doc comment, which names this exact failure mode
/// for the single-session `attach` path that this mirrors.
fn start_session(
    app_runtime: &Runtime,
    live: &mut SessionRuntime,
    sessions: &ProjectSessions,
    size: TerminalSize,
) -> anyhow::Result<()> {
    let user = UserConfig::load(app_runtime.paths())?;
    let project_config = config::load_project_config(app_runtime.project())?;
    let selection =
        session::select::select(None, EffectiveConfig::new(&user, project_config.as_ref()))?;

    let store = sessions.store();
    let record = store.create(NewSession::embedded(selection.id().slug()))?;

    tracing::info!(
        session = %record.id,
        harness = selection.id().slug(),
        executable = %selection.executable().path().display(),
        source = %selection.source(),
        "starting a session from the shell"
    );

    let launch = HarnessLaunch::new(selection.into_executable(), app_runtime.project()).size(size);
    if let Err(err) = live.start(record.id.clone(), record.presentation, &launch) {
        if let Err(store_err) = store.set_lifecycle(&record.id, SessionLifecycle::Failed) {
            tracing::warn!(
                session = %record.id,
                error = %store_err,
                "could not record a failed session start"
            );
        }
        return Err(err);
    }

    Ok(())
}

/// Build the rows the Settings overlay shows, from a fresh [`Discovery`]
/// pass and the configuration currently on disk.
///
/// This is the only place that combines them: [`state::ShellState`] and its
/// `SettingsState` never run discovery or read a configuration file
/// themselves — that would put file I/O in `shell/state.rs`, which the
/// module keeps free of it by design.
fn build_settings(runtime: &Runtime) -> anyhow::Result<(Vec<HarnessRow>, Vec<IntegrationRow>)> {
    let discovery = Discovery::run(runtime.project());
    let user = UserConfig::load(runtime.paths())?;
    let project = config::load_project_config(runtime.project())?;
    let effective = EffectiveConfig::new(&user, project.as_ref());

    let mut harnesses = Vec::new();
    let mut integrations = Vec::new();
    for &id in IntegrationId::ALL {
        let detected = discovery.get(id);
        let is_detected = detected.is_some_and(|d| d.executable().is_some());
        match id.kind() {
            IntegrationKind::Harness => {
                let enabled = effective.enabled(id, false);
                let executable = effective.executable(id);
                harnesses.push(HarnessRow {
                    id,
                    detected: is_detected,
                    enabled: enabled.value,
                    enabled_layer: enabled.layer,
                    executable: executable.as_ref().map(|e| e.value.clone()),
                    executable_layer: executable.map(|e| e.layer),
                });
            }
            IntegrationKind::Multiplexer | IntegrationKind::LocalInference => {
                integrations.push(IntegrationRow {
                    id,
                    detected: is_detected,
                    status: detected.map_or(IntegrationStatus::NotFound, |d| d.status()),
                });
            }
        }
    }
    Ok((harnesses, integrations))
}

/// Re-read Settings' rows after a successful save and hand them to
/// [`state::ShellState::refresh_settings`], which is also what clears the
/// edits that just landed on disk. A failure here is not the save failing —
/// the write already succeeded — so it only costs a stale display, reported
/// the same non-fatal way as everything else in this module.
fn refresh_settings_after_save(runtime: &Runtime, state: &mut ShellState) {
    match build_settings(runtime) {
        Ok((harnesses, integrations)) => state.refresh_settings(harnesses, integrations),
        Err(err) => {
            tracing::warn!(error = %err, "could not refresh settings after saving");
        }
    }
}

/// Apply every pending Settings edit onto `table`, leaving any field an edit
/// never touched exactly as it was. This is what keeps a save from silently
/// promoting a value that was only ever a project or default layer into the
/// layer being written, when the user never actually changed it.
fn apply_settings_edits(table: &mut config::IntegrationTable, edits: &[SettingsEdit]) {
    for edit in edits {
        let entry = table.entry(edit.id);
        if let Some(enabled) = edit.enabled {
            entry.set_enabled(enabled);
        }
        if let Some(executable) = &edit.executable {
            entry.set_executable(executable.clone());
        }
    }
}

/// Write every pending Settings edit to the user-level configuration file.
/// Never touches the project root — see the design decision's "writes
/// default to the user layer".
pub fn save_user_settings(runtime: &Runtime, edits: &[SettingsEdit]) -> anyhow::Result<()> {
    let mut config = UserConfig::load(runtime.paths())?;
    apply_settings_edits(config.integrations_mut(), edits);
    config.save(runtime.paths())?;
    Ok(())
}

/// Write every pending Settings edit to
/// `<project root>/.glasshouse/config.toml` via
/// [`config::write_project_config_with_consent`] — the only writer. The
/// consent it requires is obtained by the Settings overlay's own `W`
/// confirmation, before [`Action::SaveProjectSettings`] is ever produced —
/// see `state::SettingsState`. Returns the path written, for the status
/// line.
pub fn save_project_settings(
    runtime: &Runtime,
    edits: &[SettingsEdit],
) -> anyhow::Result<std::path::PathBuf> {
    let mut project_config = config::load_project_config(runtime.project())?.unwrap_or_default();
    apply_settings_edits(project_config.integrations_mut(), edits);
    config::write_project_config_with_consent(runtime.project(), &project_config)?;
    Ok(runtime
        .project()
        .display_root()
        .join(".glasshouse")
        .join("config.toml"))
}
