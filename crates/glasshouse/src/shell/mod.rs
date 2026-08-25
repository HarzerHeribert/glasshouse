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
//! The viewport shows the focused session's own screen, converted each tick
//! from its `vt100::Parser` into a [`state::ViewportGrid`] — see
//! [`build_viewport_grid`] — and drawn cell by cell by
//! [`view::render_viewport`]. The run loop is also the one place that
//! answers a session's cursor-position queries (see
//! [`crate::session::runtime::SessionRuntime::answer_terminal_queries`]'s doc
//! comment on why an embedded session must, unlike `session::attach`) and
//! that tells a session's pseudo-terminal and emulator the viewport's own
//! size rather than the terminal's outer one — see [`view::viewport_slot`].

pub mod state;
pub mod view;

use std::collections::HashMap;

use anyhow::Result;
use ratatui::layout::Rect;

use crate::Runtime;
use crate::config::{self, EffectiveConfig, UserConfig};
use crate::integrations::{Discovery, IntegrationId, IntegrationKind, IntegrationStatus};
use crate::launch::HarnessLaunch;
use crate::onboarding;
use crate::pty::TerminalSize;
use crate::session::{
    self, NewSession, ProjectSessions, RuntimeError, SessionId, SessionLifecycle, SessionRuntime,
};
use crate::tui::{AppEvent, DEFAULT_TICK, Event, EventSource, Screen};

pub use state::{
    Action, HarnessRow, IntegrationRow, Mode, Overlay, ProfileRow, ProfileSettingsEdit,
    ProviderRow, ProviderSettingsEdit, SettingsEdit, ShellState, ViewportGrid,
};

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
    // What each started session's harness index held for this project before
    // it ran — half the identity guard for a harness whose identifiers live
    // in one shared index, and the reason that read has to happen at start
    // rather than at exit. See `session::native_id::snapshot`. Kept in memory
    // rather than in the session record: it is scaffolding for one discovery,
    // meaningless once the session has ended, and a shell that dies mid-session
    // has nothing to capture anyway.
    let mut index_snapshots: HashMap<SessionId, session::native_id::IndexSnapshot> = HashMap::new();

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
                        viewport_terminal_size(&screen),
                        &mut index_snapshots,
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
                        Ok((harnesses, integrations, providers, profiles)) => {
                            state.open_settings(harnesses, integrations, providers, profiles);
                        }
                        Err(err) => {
                            tracing::warn!(error = %err, "could not open settings");
                            state.set_status(format!("could not open settings: {err:#}"));
                        }
                    },
                    Action::SaveUserSettings => {
                        let harness_edits = state.settings_edits();
                        let provider_edits = state.settings_provider_edits();
                        let profile_edits = state.settings_profile_edits();
                        if harness_edits.is_empty()
                            && provider_edits.is_empty()
                            && profile_edits.is_empty()
                        {
                            state.set_status("no settings changes to save");
                        } else if let Err(err) = save_user_settings(
                            runtime,
                            &harness_edits,
                            &provider_edits,
                            &profile_edits,
                        ) {
                            tracing::warn!(error = %err, "could not save user settings");
                            state.set_status(format!("could not save settings: {err:#}"));
                        } else {
                            state.set_status("saved to user configuration");
                            refresh_settings_after_save(runtime, &mut state);
                        }
                    }
                    Action::SaveProjectSettings => {
                        let harness_edits = state.settings_edits();
                        let provider_edits = state.settings_provider_edits();
                        let profile_edits = state.settings_profile_edits();
                        if harness_edits.is_empty()
                            && provider_edits.is_empty()
                            && profile_edits.is_empty()
                        {
                            state.set_status("no settings changes to save");
                        } else {
                            match save_project_settings(
                                runtime,
                                &harness_edits,
                                &provider_edits,
                                &profile_edits,
                            ) {
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
                    Action::ReopenOnboarding => {
                        // The wizard drives its own `Screen`, so this
                        // shell's must be released first and reacquired once
                        // it returns — the two never hold the terminal at
                        // once. Sessions already running keep running; only
                        // which screen is drawn changes for the moment the
                        // wizard has it.
                        drop(screen);
                        let outcome = reopen_onboarding(runtime);
                        screen = Screen::acquire()?;
                        match outcome {
                            Ok(onboarding::Outcome::Completed(_)) => {
                                state.set_status("setup wizard updated your configuration");
                            }
                            Ok(onboarding::Outcome::Cancelled) => {
                                state.set_status("setup wizard cancelled; nothing changed");
                            }
                            Err(err) => {
                                tracing::warn!(error = %err, "could not reopen the setup wizard");
                                state.set_status(format!("could not reopen setup: {err:#}"));
                            }
                        }
                        state.close_overlay();
                    }
                }
                sync_focus(&mut live, &state);
                if !matches!(action, Action::None) {
                    screen.draw(|frame| view::render(&state, frame))?;
                }
            }
            Event::Resize(cols, rows) => {
                screen.on_resize(cols, rows)?;
                if let Some(id) = live.focused().cloned() {
                    // The viewport's own inner size, not the terminal's outer
                    // one — see `view::viewport_slot`'s doc comment. A
                    // harness resized to the terminal's full size would draw
                    // for space Glasshouse's chrome has already claimed.
                    let slot = view::viewport_slot(Rect::new(0, 0, cols, rows));
                    if let Err(err) = live.resize(&id, TerminalSize::new(slot.height, slot.width)) {
                        tracing::warn!(session = %id, %err, "could not resize the focused session");
                    }
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

                // An embedded session has no real terminal behind it to
                // answer its own `ESC[6n` — Glasshouse is the terminal, so
                // Glasshouse must answer, every tick, or a harness waiting
                // on the reply hangs looking exactly like one that started
                // and did nothing. See `SessionRuntime::answer_terminal_queries`.
                live.answer_terminal_queries();

                let mut redraw = false;
                for (id, status) in live.poll_exits() {
                    let lifecycle = if status.success() {
                        SessionLifecycle::Stopped
                    } else {
                        SessionLifecycle::Failed
                    };
                    // The session is over, so this is the tightest the
                    // discovery window will ever be — see
                    // `session::native_id::capture`'s doc comment.
                    let index_before = index_snapshots.remove(&id).unwrap_or_default();
                    if let Ok(Some(record)) = sessions.store().get(&id) {
                        session::native_id::capture(
                            &sessions.store(),
                            &record,
                            runtime.project().root(),
                            &index_before,
                        );
                    }
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

                let grid = state
                    .active_session()
                    .and_then(|record| live.get(&record.id))
                    .map(|session| session.with_screen(build_viewport_grid))
                    .unwrap_or_default();
                if grid != *state.viewport_grid() {
                    state.set_viewport_grid(grid);
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

/// The viewport's own inner size, in the shape a freshly spawned session's
/// pseudo-terminal needs — not the terminal's outer size. See
/// `view::viewport_slot`'s doc comment.
fn viewport_terminal_size(screen: &Screen) -> TerminalSize {
    let outer = screen.size().unwrap_or_default();
    let slot = view::viewport_slot(Rect::new(0, 0, outer.cols, outer.rows));
    TerminalSize::new(slot.height, slot.width)
}

/// Convert `vt100`'s colour model to Ratatui's — the one place either module
/// needs to know about the other's colour type.
///
/// `Default` becomes `None`, meaning "inherit whatever is already there"
/// rather than any specific colour, so a cell whose fore/background was
/// never set keeps the terminal's own default instead of being forced to a
/// literal black or white.
fn convert_color(color: vt100::Color) -> Option<ratatui::style::Color> {
    match color {
        vt100::Color::Default => None,
        vt100::Color::Idx(index) => Some(ratatui::style::Color::Indexed(index)),
        vt100::Color::Rgb(r, g, b) => Some(ratatui::style::Color::Rgb(r, g, b)),
    }
}

/// The Ratatui style a single `vt100` cell should be drawn with.
fn cell_style(cell: &vt100::Cell) -> ratatui::style::Style {
    let mut style = ratatui::style::Style::default();
    if let Some(fg) = convert_color(cell.fgcolor()) {
        style = style.fg(fg);
    }
    if let Some(bg) = convert_color(cell.bgcolor()) {
        style = style.bg(bg);
    }
    let mut modifier = ratatui::style::Modifier::empty();
    if cell.bold() {
        modifier |= ratatui::style::Modifier::BOLD;
    }
    if cell.italic() {
        modifier |= ratatui::style::Modifier::ITALIC;
    }
    if cell.underline() {
        modifier |= ratatui::style::Modifier::UNDERLINED;
    }
    if cell.inverse() {
        modifier |= ratatui::style::Modifier::REVERSED;
    }
    style.add_modifier(modifier)
}

/// Walk a session's emulated screen into the [`ViewportGrid`]
/// [`view::render_viewport`] draws. The only place `vt100` and Ratatui's
/// colour and modifier types meet — see [`convert_color`].
fn build_viewport_grid(screen: &vt100::Screen) -> ViewportGrid {
    let (rows, cols) = screen.size();
    let mut cells = Vec::with_capacity(usize::from(rows) * usize::from(cols));
    for row in 0..rows {
        for col in 0..cols {
            let (text, style) = match screen.cell(row, col) {
                Some(cell) => (cell.contents().to_owned(), cell_style(cell)),
                None => (String::new(), ratatui::style::Style::default()),
            };
            cells.push((text, style));
        }
    }
    // vt100 tracks whether the cursor should be hidden (`ESC[?25l`) — a
    // harness that has hidden its own cursor should not get one drawn back
    // in for it.
    let cursor = (!screen.hide_cursor()).then(|| screen.cursor_position());
    ViewportGrid::new(rows, cols, cells, cursor)
}

/// Resolve a harness, record a new session, and start it — the same
/// selection seam `main.rs: launch_session` uses for `glasshouse launch`,
/// minus attaching to this process's own terminal: the shell attaches by
/// giving the session the viewport once its output starts arriving, instead.
///
/// `size` is the viewport's own inner size at the moment `n` was pressed —
/// see `view::viewport_slot`'s doc comment for why that, and not the
/// terminal's outer size, is what a harness must be told it has — rather
/// than the default `HarnessLaunch` would otherwise use: a harness TUI lays
/// itself out from the size it sees at startup, so starting it at the wrong
/// geometry and resizing afterwards would draw its first frame for space it
/// does not have — see `HarnessLaunch::size`'s doc comment, which names this
/// exact failure mode for the single-session `attach` path that this
/// mirrors.
fn start_session(
    app_runtime: &Runtime,
    live: &mut SessionRuntime,
    sessions: &ProjectSessions,
    size: TerminalSize,
    index_snapshots: &mut HashMap<SessionId, session::native_id::IndexSnapshot>,
) -> anyhow::Result<()> {
    let user = UserConfig::load(app_runtime.paths())?;
    let project_config = config::load_project_config(app_runtime.project())?;
    let effective = EffectiveConfig::new(&user, project_config.as_ref());
    let selection = session::select::select(None, effective)?;

    let store = sessions.store();
    let native = selection
        .assigns_native_session_id()
        .then(|| store.new_native_session_id())
        .transpose()?;
    let record = store.create(
        NewSession::embedded(selection.id().slug()).with_native_session_id(native.clone()),
    )?;

    // Before the harness runs — see the declaration of `index_snapshots` in
    // `run`, and `session::native_id::snapshot`.
    index_snapshots.insert(
        record.id.clone(),
        session::native_id::snapshot(&record.harness, app_runtime.project().root()),
    );

    tracing::info!(
        session = %record.id,
        harness = selection.id().slug(),
        executable = %selection.executable().path().display(),
        source = %selection.source(),
        "starting a session from the shell"
    );

    // No user arguments here: the shell's `n` opens a session, and anything
    // extra would be a Glasshouse invention rather than something asked for.
    let mut args = selection.start_args(native.as_deref(), Vec::<String>::new());
    // Best effort: a session that reports nothing is still a session, and is
    // a far smaller loss than refusing to start one the user asked for.
    let project_hooks_consent = effective.project_hooks(selection.id()).value;
    let hook_args = std::env::current_exe()
        .map_err(anyhow::Error::from)
        .and_then(|program| {
            let report = crate::harness::HookCommand::new(
                program,
                record.id.as_str(),
                app_runtime.session_dir(record.id.as_str()),
                app_runtime.project().root(),
                app_runtime.paths().data_dir(),
                app_runtime.paths().config_dir(),
            );
            selection.install_hooks(&report, project_hooks_consent)
        });
    match hook_args {
        Ok(Some(hook_args)) => {
            args.splice(0..0, hook_args);
        }
        Ok(None) => {}
        Err(err) => {
            tracing::warn!(session = %record.id, error = %err, "could not install lifecycle hooks");
        }
    }
    let launch = HarnessLaunch::new(selection.into_executable(), app_runtime.project())
        .args(args)
        .size(size);
    if let Err(err) = live.start(record.id.clone(), record.presentation, &launch) {
        // A session that never started will never be polled for its exit, so
        // its snapshot has nothing left to pair with.
        index_snapshots.remove(&record.id);
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

/// Reopen the first-run wizard for a "reconfigure" invocation from the
/// Settings overlay — Phase 2C: "Allow the onboarding wizard to be reopened
/// later from settings."
///
/// Loads `UserConfig` fresh from disk, not whatever unsaved Settings edits
/// happen to be staged in `state` — reopening the wizard and saving Settings
/// are two separate, independent write paths, exactly as they already are
/// for the user- versus project-level saves. [`onboarding::run`] seeds every
/// screen from what it is handed, so it shows the user's persisted choices
/// (including any previously configured provider) instead of a blank wizard.
fn reopen_onboarding(runtime: &Runtime) -> anyhow::Result<onboarding::Outcome> {
    let config = UserConfig::load(runtime.paths())?;
    let discovery = Discovery::run(runtime.project());
    onboarding::run(runtime, &discovery, config)
}

/// Every row every Settings section shows, in the order
/// [`build_settings`] returns them.
type SettingsRows = (
    Vec<HarnessRow>,
    Vec<IntegrationRow>,
    Vec<ProviderRow>,
    Vec<ProfileRow>,
);

/// Build the rows the Settings overlay shows, from a fresh [`Discovery`]
/// pass and the configuration currently on disk.
///
/// This is the only place that combines them: [`state::ShellState`] and its
/// `SettingsState` never run discovery or read a configuration file
/// themselves — that would put file I/O in `shell/state.rs`, which the
/// module keeps free of it by design.
fn build_settings(runtime: &Runtime) -> anyhow::Result<SettingsRows> {
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

    // Providers are atomic per name — see `ProviderRow::layer`'s own doc —
    // so each row's whole configuration and layer come from whichever table
    // actually holds that name, project winning over user, matching
    // `EffectiveConfig::configured_provider`.
    let mut providers = Vec::new();
    for name in effective.provider_names() {
        let found = project
            .as_ref()
            .and_then(|p| p.providers().get(&name))
            .map(|cfg| (cfg, config::Layer::Project))
            .or_else(|| {
                user.providers()
                    .get(&name)
                    .map(|cfg| (cfg, config::Layer::User))
            });
        if let Some((provider_config, layer)) = found {
            providers.push(ProviderRow {
                name,
                config: provider_config.clone(),
                layer,
            });
        }
    }

    // `EffectiveConfig::profile_names` also lists the implied Native
    // profile, which has no `ProfileConfig` behind it — see
    // `crate::profile::NATIVE_PROFILE_NAME`'s own doc — so the merge is
    // built directly from the two tables instead of reusing that method.
    let mut profile_names: std::collections::BTreeSet<String> =
        user.profiles().names().map(str::to_owned).collect();
    if let Some(project) = project.as_ref() {
        profile_names.extend(project.profiles().names().map(str::to_owned));
    }
    let mut profiles = Vec::new();
    for name in profile_names {
        let found = project
            .as_ref()
            .and_then(|p| p.profiles().get(&name))
            .map(|cfg| (cfg, config::Layer::Project))
            .or_else(|| {
                user.profiles()
                    .get(&name)
                    .map(|cfg| (cfg, config::Layer::User))
            });
        if let Some((profile_config, layer)) = found {
            profiles.push(ProfileRow {
                name,
                config: profile_config.clone(),
                layer,
            });
        }
    }

    Ok((harnesses, integrations, providers, profiles))
}

/// Re-read Settings' rows after a successful save and hand them to
/// [`state::ShellState::refresh_settings`], which is also what clears the
/// edits that just landed on disk. A failure here is not the save failing —
/// the write already succeeded — so it only costs a stale display, reported
/// the same non-fatal way as everything else in this module.
fn refresh_settings_after_save(runtime: &Runtime, state: &mut ShellState) {
    match build_settings(runtime) {
        Ok((harnesses, integrations, providers, profiles)) => {
            state.refresh_settings(harnesses, integrations, providers, profiles)
        }
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

/// Apply every pending provider edit onto `table` — an add/replace for
/// `Some`, a removal for `None`. Unlike [`apply_settings_edits`], each edit
/// already carries a complete [`config::ProviderConfig`], since every
/// provider edit in the Settings overlay produces (or removes) the whole
/// value rather than one field of it.
fn apply_provider_edits(table: &mut config::ProviderTable, edits: &[ProviderSettingsEdit]) {
    for edit in edits {
        match &edit.upsert {
            Some(provider_config) => table.set(edit.name.clone(), provider_config.clone()),
            None => {
                table.remove(&edit.name);
            }
        }
    }
}

/// The [`config::ProfileTable`] counterpart to [`apply_provider_edits`].
fn apply_profile_edits(table: &mut config::ProfileTable, edits: &[ProfileSettingsEdit]) {
    for edit in edits {
        match &edit.upsert {
            Some(profile_config) => table.set(edit.name.clone(), profile_config.clone()),
            None => {
                table.remove(&edit.name);
            }
        }
    }
}

/// Write every pending Settings edit to the user-level configuration file.
/// Never touches the project root — see the design decision's "writes
/// default to the user layer".
pub fn save_user_settings(
    runtime: &Runtime,
    harness_edits: &[SettingsEdit],
    provider_edits: &[ProviderSettingsEdit],
    profile_edits: &[ProfileSettingsEdit],
) -> anyhow::Result<()> {
    let mut config = UserConfig::load(runtime.paths())?;
    apply_settings_edits(config.integrations_mut(), harness_edits);
    apply_provider_edits(config.providers_mut(), provider_edits);
    apply_profile_edits(config.profiles_mut(), profile_edits);
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
    harness_edits: &[SettingsEdit],
    provider_edits: &[ProviderSettingsEdit],
    profile_edits: &[ProfileSettingsEdit],
) -> anyhow::Result<std::path::PathBuf> {
    let mut project_config = config::load_project_config(runtime.project())?.unwrap_or_default();
    apply_settings_edits(project_config.integrations_mut(), harness_edits);
    apply_provider_edits(project_config.providers_mut(), provider_edits);
    apply_profile_edits(project_config.profiles_mut(), profile_edits);
    config::write_project_config_with_consent(runtime.project(), &project_config)?;
    Ok(runtime
        .project()
        .display_root()
        .join(".glasshouse")
        .join("config.toml"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::style::{Color, Modifier};

    /// Colours, bold/inverse, and cursor position must all survive the walk
    /// from a `vt100::Screen` into a [`ViewportGrid`] — the design decision's
    /// invariant that "colours, cursor position and line wrapping survive a
    /// round trip through the emulator into Ratatui cells."
    #[test]
    fn colours_bold_inverse_and_cursor_position_survive_the_conversion() {
        let mut parser = vt100::Parser::new(3, 10, 0);
        // Bold, indexed red-on-blue "hi", then inverse-video "x".
        parser.process(b"\x1b[1;31;44mhi\x1b[0m\x1b[7mx\x1b[0m");
        parser.process(b"\x1b[2;3H"); // move to row 2, col 3 (1-based)

        let grid = build_viewport_grid(parser.screen());

        let (text, style) = grid.cell(0, 0).expect("cell (0,0) exists");
        assert_eq!(text, "h");
        assert_eq!(
            style.fg,
            Some(Color::Indexed(1)),
            "fg 31 is ANSI red, index 1"
        );
        assert_eq!(
            style.bg,
            Some(Color::Indexed(4)),
            "bg 44 is ANSI blue, index 4"
        );
        assert!(style.add_modifier.contains(Modifier::BOLD));

        let (_, inverse_style) = grid.cell(0, 2).expect("cell (0,2) exists");
        assert!(inverse_style.add_modifier.contains(Modifier::REVERSED));

        let (_, default_style) = grid.cell(1, 0).expect("cell (1,0) exists");
        assert_eq!(
            default_style.fg, None,
            "an untouched cell's colour must inherit, not be forced to a literal colour"
        );

        assert_eq!(
            grid.cursor(),
            Some((1, 2)),
            "vt100 reports zero-based; row 2 col 3 one-based is (1, 2)"
        );
    }

    /// A hidden cursor (`ESC[?25l`) must not be drawn back in.
    #[test]
    fn a_hidden_cursor_is_not_shown() {
        let mut parser = vt100::Parser::new(2, 5, 0);
        parser.process(b"\x1b[?25l");
        let grid = build_viewport_grid(parser.screen());
        assert_eq!(grid.cursor(), None);
    }

    /// Text that overruns a row must wrap onto the next one exactly as
    /// `vt100` lays it out — the grid is a direct walk of the screen, so this
    /// is really a proof that the walk visits cells in the right order.
    #[test]
    fn line_wrapping_is_preserved_in_the_grid() {
        let mut parser = vt100::Parser::new(2, 5, 0);
        parser.process(b"abcdefghij"); // 10 characters into a 5-wide screen
        assert!(
            parser.screen().row_wrapped(0),
            "the first row must have wrapped for this test to mean anything"
        );

        let grid = build_viewport_grid(parser.screen());
        let row = |r: u16| -> String {
            (0..5u16)
                .map(|c| grid.cell(r, c).expect("cell exists").0.clone())
                .collect()
        };
        assert_eq!(row(0), "abcde");
        assert_eq!(row(1), "fghij");
    }

    /// A screen with nothing drawn on it yet is still a valid, non-empty
    /// grid — every cell is blank, not absent — which is what lets the view
    /// tell "no live session" apart from "a live session with a blank
    /// screen".
    #[test]
    fn a_fresh_screen_converts_to_a_full_grid_of_blank_cells() {
        let parser = vt100::Parser::new(4, 10, 0);
        let grid = build_viewport_grid(parser.screen());
        assert!(!grid.is_empty());
        assert_eq!(grid.rows(), 4);
        assert_eq!(grid.cols(), 10);
        assert_eq!(grid.cell(0, 0).unwrap().0, "");
        assert_eq!(grid.cursor(), Some((0, 0)));
    }
}

/// Phase 2D: Settings' save/reload behaviour for Providers and Launch
/// Profiles, exercised through a real [`Runtime`] and real files — the
/// staging half (which row/edit changes when a key is pressed) is
/// `shell::state`'s own tests; this is the write half.
#[cfg(test)]
mod settings_persistence_tests {
    use super::*;
    use crate::config::{Layer as ConfigLayer, ProfileConfig, ProviderConfig};

    /// Bootstrap a `Runtime` over fresh, isolated data/config/workspace
    /// directories, matching `integrations::tests::bootstrapped_runtime`.
    fn bootstrapped_runtime() -> (tempfile::TempDir, tempfile::TempDir, crate::Runtime) {
        use clap::Parser;

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

    /// Acceptance 2: adding a provider from a built-in template persists it
    /// to the user layer, and it survives a reload.
    #[test]
    fn adding_a_provider_from_a_template_persists_to_the_user_layer_and_survives_reload() {
        let (_data, _workspace, runtime) = bootstrapped_runtime();

        let edit = ProviderSettingsEdit {
            name: "my-router".to_owned(),
            upsert: Some(ProviderConfig::new("openrouter")),
        };
        save_user_settings(&runtime, &[], &[edit], &[]).expect("save must succeed");

        // Reload from disk — a fresh read, not the in-memory value just
        // written — to prove this is a persistence test and not a tautology.
        let (harnesses, integrations, providers, profiles) =
            build_settings(&runtime).expect("settings must rebuild after the save");
        let _ = (harnesses, integrations, profiles);
        assert_eq!(providers.len(), 1);
        assert_eq!(providers[0].name, "my-router");
        assert_eq!(providers[0].config.template(), "openrouter");
        assert_eq!(providers[0].layer, ConfigLayer::User);

        // And directly against `UserConfig`, independent of `build_settings`.
        let reloaded = UserConfig::load(runtime.paths()).unwrap();
        assert_eq!(
            reloaded.providers().get("my-router").unwrap().template(),
            "openrouter"
        );
    }

    /// Acceptance 3: editing a provider's base URL persists, and the edited
    /// value is what a launch would actually use — proven by resolving the
    /// saved configuration into a real `Provider` and reading its protocol's
    /// base URL, the exact value `crate::launch` would send a harness to.
    #[test]
    fn an_edited_base_url_persists_and_is_what_a_launch_would_use() {
        let (_data, _workspace, runtime) = bootstrapped_runtime();

        let mut config = ProviderConfig::new("openrouter");
        config.set_base_url(Some("https://mirror.example.com/v1".to_owned()));
        let edit = ProviderSettingsEdit {
            name: "my-router".to_owned(),
            upsert: Some(config),
        };
        save_user_settings(&runtime, &[], &[edit], &[]).expect("save must succeed");

        let reloaded = UserConfig::load(runtime.paths()).unwrap();
        let effective = config::EffectiveConfig::new(&reloaded, None);
        let resolved = effective
            .configured_provider("my-router")
            .expect("the provider must resolve");
        let openai_chat = resolved
            .value
            .protocols
            .iter()
            .find(|p| p.protocol == crate::harness::WireProtocol::OpenAiChat)
            .expect("openrouter serves openai-chat");
        assert_eq!(
            openai_chat.base_url, "https://mirror.example.com/v1",
            "the edited base URL must be exactly what a launch would use"
        );
    }

    /// Acceptance 4 (the full write path): disabling a provider through
    /// `save_user_settings` persists the disabled state and every other
    /// field, and re-enabling needs no retyping.
    #[test]
    fn disabling_a_provider_through_the_save_path_persists_and_is_reversible() {
        let (_data, _workspace, runtime) = bootstrapped_runtime();

        let mut config = ProviderConfig::new("openrouter");
        config.set_base_url(Some("https://mirror.example.com/v1".to_owned()));
        config.set_enabled(false);
        let edit = ProviderSettingsEdit {
            name: "my-router".to_owned(),
            upsert: Some(config),
        };
        save_user_settings(&runtime, &[], &[edit], &[]).unwrap();

        let reloaded = UserConfig::load(runtime.paths()).unwrap();
        let provider = reloaded.providers().get("my-router").unwrap();
        assert!(!provider.enabled());
        assert_eq!(
            provider.base_url(),
            Some("https://mirror.example.com/v1"),
            "disabling must not touch other fields"
        );

        let mut re_enabled = provider.clone();
        re_enabled.set_enabled(true);
        let edit = ProviderSettingsEdit {
            name: "my-router".to_owned(),
            upsert: Some(re_enabled),
        };
        save_user_settings(&runtime, &[], &[edit], &[]).unwrap();
        let reloaded_again = UserConfig::load(runtime.paths()).unwrap();
        let provider_again = reloaded_again.providers().get("my-router").unwrap();
        assert!(provider_again.enabled());
        assert_eq!(
            provider_again.base_url(),
            Some("https://mirror.example.com/v1")
        );
    }

    /// Removing a provider through the save path actually removes the
    /// entry — the other half of acceptance 4.
    #[test]
    fn removing_a_provider_through_the_save_path_removes_it() {
        let (_data, _workspace, runtime) = bootstrapped_runtime();

        let edit = ProviderSettingsEdit {
            name: "my-router".to_owned(),
            upsert: Some(ProviderConfig::new("openrouter")),
        };
        save_user_settings(&runtime, &[], &[edit], &[]).unwrap();
        assert!(
            UserConfig::load(runtime.paths())
                .unwrap()
                .providers()
                .get("my-router")
                .is_some()
        );

        let removal = ProviderSettingsEdit {
            name: "my-router".to_owned(),
            upsert: None,
        };
        save_user_settings(&runtime, &[], &[removal], &[]).unwrap();
        assert!(
            UserConfig::load(runtime.paths())
                .unwrap()
                .providers()
                .get("my-router")
                .is_none()
        );
    }

    /// Acceptance 5 (the full write path): a duplicated launch profile is an
    /// independent entry once saved — editing the copy's stored
    /// configuration must never touch the original's file record.
    #[test]
    fn a_duplicated_profile_persists_as_an_independent_entry() {
        let (_data, _workspace, runtime) = bootstrapped_runtime();

        let original = ProfileConfig::new(crate::integrations::IntegrationId::ClaudeCode);
        save_user_settings(
            &runtime,
            &[],
            &[],
            &[ProfileSettingsEdit {
                name: "fast".to_owned(),
                upsert: Some(original),
            }],
        )
        .unwrap();

        let mut copy = ProfileConfig::new(crate::integrations::IntegrationId::ClaudeCode);
        copy.set_model(Some("claude-opus".to_owned()));
        save_user_settings(
            &runtime,
            &[],
            &[],
            &[ProfileSettingsEdit {
                name: "fast-copy".to_owned(),
                upsert: Some(copy),
            }],
        )
        .unwrap();

        let reloaded = UserConfig::load(runtime.paths()).unwrap();
        assert_eq!(reloaded.profiles().get("fast").unwrap().model(), None);
        assert_eq!(
            reloaded.profiles().get("fast-copy").unwrap().model(),
            Some("claude-opus")
        );
    }

    /// Acceptance 8: `save_user_settings` never touches the project root,
    /// and only `save_project_settings` — reached only after the Settings
    /// overlay's own explicit `W` confirmation (see `state`'s
    /// `shift_w_requires_a_separate_explicit_confirmation`) — writes
    /// `.glasshouse/config.toml`. This is the write half of that guarantee;
    /// the confirmation-gating half is `state`'s.
    #[test]
    fn saving_user_settings_never_creates_a_project_config_file() {
        let (_data, workspace, runtime) = bootstrapped_runtime();
        let project_config_path = workspace.path().join(".glasshouse").join("config.toml");

        let edit = ProviderSettingsEdit {
            name: "my-router".to_owned(),
            upsert: Some(ProviderConfig::new("openrouter")),
        };
        save_user_settings(&runtime, &[], &[edit], &[]).unwrap();

        assert!(
            !project_config_path.exists(),
            "a user-layer save must never create the project config file"
        );
    }

    /// The other side of acceptance 8: `save_project_settings` does write
    /// exactly `<project root>/.glasshouse/config.toml`, and the provider
    /// edit lands in the project layer, not the user layer.
    #[test]
    fn saving_project_settings_writes_the_project_layer_only() {
        let (_data, workspace, runtime) = bootstrapped_runtime();

        let edit = ProviderSettingsEdit {
            name: "my-router".to_owned(),
            upsert: Some(ProviderConfig::new("openrouter")),
        };
        let path = save_project_settings(&runtime, &[], &[edit], &[]).unwrap();

        assert!(path.exists());
        // Canonicalize before comparing: on macOS `TempDir` paths run through
        // `/tmp`, a symlink to `/private/tmp`, and the runtime's own scope
        // resolution follows it — a portability quirk of the test fixture,
        // not of `save_project_settings` itself.
        assert_eq!(
            std::fs::canonicalize(&path).unwrap(),
            std::fs::canonicalize(workspace.path().join(".glasshouse").join("config.toml"))
                .unwrap()
        );

        let project_config = config::load_project_config(runtime.project())
            .unwrap()
            .expect("the project config file must now exist");
        assert_eq!(
            project_config
                .providers()
                .get("my-router")
                .unwrap()
                .template(),
            "openrouter"
        );
        assert!(
            UserConfig::load(runtime.paths())
                .unwrap()
                .providers()
                .get("my-router")
                .is_none(),
            "a project-layer save must not also write the user layer"
        );
    }

    /// `build_settings` must read a disabled provider or profile back
    /// without panicking or dropping the disabled state — the same rows the
    /// Settings overlay renders.
    #[test]
    fn build_settings_reflects_a_disabled_provider_and_profile() {
        let (_data, _workspace, runtime) = bootstrapped_runtime();

        let mut provider = ProviderConfig::new("openrouter");
        provider.set_enabled(false);
        let mut profile = ProfileConfig::new(crate::integrations::IntegrationId::ClaudeCode);
        profile.set_enabled(false);

        save_user_settings(
            &runtime,
            &[],
            &[ProviderSettingsEdit {
                name: "my-router".to_owned(),
                upsert: Some(provider),
            }],
            &[ProfileSettingsEdit {
                name: "fast".to_owned(),
                upsert: Some(profile),
            }],
        )
        .unwrap();

        let (_, _, providers, profiles) = build_settings(&runtime).unwrap();
        assert!(!providers[0].config.enabled());
        assert!(!profiles[0].config.enabled());
    }
}
