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
//! `build_viewport_grid` — and drawn cell by cell by
//! `view::render_viewport`. The run loop is also the one place that
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
use crate::provider::cache::{ModelCache, ModelCatalogue};
use crate::provider::discovery::{self, ModelFetch, ProbeRequest};
use crate::pty::TerminalSize;
use crate::secret;
use crate::secret::SecretStore as _;
use crate::session::{
    self, NewSession, ProjectSessions, RuntimeError, SessionId, SessionLifecycle,
    SessionPresentation, SessionRuntime,
};
use crate::tui::{AppEvent, DEFAULT_TICK, Event, EventSource, Screen};

pub use state::{
    Action, HarnessRow, IntegrationRow, Mode, ModelRefresh, Overlay, OverviewState, ProbeKind,
    ProfileRow, ProfileSettingsEdit, ProviderNotice, ProviderProbeIntent, ProviderProbeResult,
    ProviderRow, ProviderSettingsEdit, ReachabilityCheck, SettingsEdit, ShellState, ViewportGrid,
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
    // Where a provider probe's answer comes back. The request itself is made
    // on a thread of its own — see `spawn_provider_probe` — and this is the
    // seam that keeps it off the thread drawing the terminal.
    let (probe_results, probe_inbox) = std::sync::mpsc::channel::<ProviderProbeResult>();

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
                    Action::StartSession | Action::StartHeadlessSession => {
                        let presentation = if matches!(action, Action::StartHeadlessSession) {
                            SessionPresentation::Headless
                        } else {
                            SessionPresentation::Embedded
                        };
                        match start_session(
                            runtime,
                            &mut live,
                            &sessions,
                            presentation,
                            viewport_terminal_size(&screen),
                            &mut index_snapshots,
                        ) {
                            Ok(()) => {
                                if let Ok(records) = sessions.store().list() {
                                    state.refresh(records);
                                }
                                if presentation == SessionPresentation::Headless {
                                    // A headless session draws no viewport by
                                    // design, so `N` would otherwise be a key
                                    // that appeared to do nothing. The
                                    // viewport placeholder says so on every
                                    // frame — see `view::render_viewport` —
                                    // which is what carries the message on a
                                    // terminal too narrow to leave room for
                                    // this note beside the key bindings.
                                    state.set_status("started a headless session — `o` lists it");
                                }
                            }
                            Err(err) => {
                                tracing::warn!(error = %err, "could not start a session");
                                state.set_status(format!("could not start a session: {err:#}"));
                            }
                        }
                    }
                    Action::InterruptSession(id) => interrupt_session(&mut live, &mut state, id),
                    Action::SendSessionText { id, text } => {
                        send_session_text(&mut live, &mut state, id, text);
                    }
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
                    Action::StoreProviderCredential => {
                        store_provider_credential(&mut state);
                    }
                    Action::RunProviderProbe => {
                        // `ProbeTimeouts::default()` and nothing else. The
                        // parameter exists so a test can bound a hanging
                        // endpoint in under a second instead of waiting out
                        // `RESPONSE_TIMEOUT`; the values production uses are
                        // asserted by `provider::discovery`'s own
                        // `the_default_timeouts_are_the_named_constants_and_none_is_unset`,
                        // and that this call site passes the default is
                        // asserted by `the_run_loop_probes_with_the_default_timeouts`.
                        spawn_provider_probe(
                            runtime,
                            &mut state,
                            &probe_results,
                            &events.sender(),
                            discovery::ProbeTimeouts::default(),
                        );
                    }
                    Action::DeleteProviderCredential => {
                        delete_provider_credential(&mut state);
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

                // A probe's answer normally arrives with its own wake-up —
                // the worker sends `AppEvent::Redraw` — but it is drained
                // here too, so a result can never be stranded by a wake-up
                // that raced a tick.
                if drain_provider_probes(&probe_inbox, &mut state) {
                    redraw = true;
                }
                // A request is outstanding, so keep repainting. Without this
                // the in-flight line would be drawn once and then sit there
                // looking exactly like the hang it exists to rule out.
                if state.provider_probe_in_flight() {
                    redraw = true;
                }

                // A headless session is skipped, and this is deliberately
                // **not** where the guarantee lives: `view::render_viewport`
                // refuses to draw one, which is what holds even for a grid
                // that is merely stale. Removing the filter below therefore
                // changes nothing anyone can see, and a mutation proving that
                // was run rather than assumed.
                //
                // It stays for two things a test cannot observe. It keeps
                // `state.viewport_grid()` an honest description of what is on
                // screen rather than a screen that is deliberately not shown;
                // and without it a headless session producing output would
                // make the grid differ every tick, so the shell would repaint
                // continuously for a session nobody is looking at.
                //
                // The runtime's own presentation is the authority — it is
                // what `focus` refuses on — rather than the stored record,
                // which can be about a session no longer running in this
                // Glasshouse at all.
                let grid = state
                    .active_session()
                    .and_then(|record| live.get(&record.id))
                    .filter(|session| session.presentation() != SessionPresentation::Headless)
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
                let probed = drain_provider_probes(&probe_inbox, &mut state);
                if state.refresh(sessions.store().list()?) == Action::Redraw || probed {
                    screen.draw(|frame| view::render(&state, frame))?;
                }
            }
            Event::Paste(_) | Event::Mouse(_) => {}
        }
    }
}

/// Interrupt one session, whether or not it is the one on screen.
///
/// The whole point of the capability, so it is worth being explicit about
/// what this does **not** do: it does not focus the session, does not change
/// which session the bar presents, and does not move the session's recorded
/// lifecycle. A harness that handles the interrupt keeps running; one that
/// exits because of it is noticed by `poll_exits` on the next tick, from the
/// operating system rather than inferred here — the same rule that keeps
/// session state out of terminal output everywhere else.
///
/// Whether the byte becomes a signal is the platform's business:
/// `PtyProcess::interrupt` writes `ETX` into the session's terminal, and it
/// is the Unix line discipline — or ConPTY's Win32 input mode — that turns
/// that into an interrupt for the process group. Nothing here is
/// platform-specific.
fn interrupt_session(live: &mut SessionRuntime, state: &mut ShellState, id: &SessionId) {
    let name = state::short_session_id(id);
    match live.interrupt(id) {
        Ok(()) => state.set_status(format!("interrupted session `{name}`")),
        // Refused out loud, never silently: the runtime knows things the
        // records do not — a session that exited since the last poll still
        // reads as live in the store, and `ShellState` can only see the store.
        Err(err) => {
            tracing::warn!(session = %id, %err, "could not interrupt a session");
            state.set_status(format!(
                "cannot interrupt `{name}`: {}",
                refusal_reason(&err)
            ));
        }
    }
}

/// Why the runtime refused, *without* the session it refused about.
///
/// [`RuntimeError`]'s own `Display` names the session in full, which is right
/// for a log line and wrong for a status note: the note has already named it,
/// in the short form the overview's rows use, and a sentence carrying a
/// twelve-character identifier and a thirty-two-character one is long enough
/// to be clipped by the popup it is drawn in. Found by running the shipped
/// binary — the refusal was correct and unreadable.
fn refusal_reason(err: &RuntimeError) -> String {
    match err {
        RuntimeError::NotLive { .. } => "it is not running in this Glasshouse".to_owned(),
        RuntimeError::Exited { .. } => "it has already exited".to_owned(),
        RuntimeError::Headless { .. } => "it is headless and has no viewport".to_owned(),
        RuntimeError::Io { source, .. } => source.to_string(),
    }
}

/// Send one line to a session, whether or not it is the one on screen.
///
/// A carriage return is appended because this is a *line*: a bare `\r` is
/// exactly what a real Enter key delivers to a terminal, which is what
/// `state::encode` sends in session mode, so text arriving this way is
/// indistinguishable to the harness from text somebody typed.
///
/// `SessionRuntime::send_text` does not touch focus, and neither does this —
/// a line arriving in a background session must never pull the user out of
/// the one they are working in.
fn send_session_text(
    live: &mut SessionRuntime,
    state: &mut ShellState,
    id: &SessionId,
    text: &str,
) {
    let name = state::short_session_id(id);
    match live.send_text(id, &format!("{text}\r")) {
        Ok(()) => state.set_status(format!("sent a line to session `{name}`")),
        Err(err) => {
            tracing::warn!(session = %id, %err, "could not send text to a session");
            state.set_status(format!("cannot send to `{name}`: {}", refusal_reason(&err)));
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
        // A headless session has no viewport to bring forward — that is what
        // makes it headless. The bar moving onto one leaves the keyboard
        // exactly where it was rather than logging a failure on every key,
        // and the user is told the moment they try to enter it; see
        // `ShellState::enter_session_mode`.
        Err(RuntimeError::Headless { .. }) => {}
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
/// `presentation` is the only difference between `n` and `N`. Everything
/// else — harness selection, the recorded session, hooks, the launch — is
/// deliberately shared, so a headless session is an ordinary session that is
/// not shown rather than a second kind of thing.
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
    presentation: SessionPresentation,
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
    // The presentation is recorded before the process exists and is then the
    // single source of truth for it: `live.start` below is handed
    // `record.presentation`, so a session's stored presentation and its
    // running one cannot disagree.
    let record = store.create(
        NewSession::embedded(selection.id().slug())
            .with_presentation(presentation)
            .with_native_session_id(native.clone()),
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
    // **Phase 9D line 3, and the whole of it.** Opening Settings reads the
    // model catalogue off disk. It does not fetch, it does not check an
    // expiry, and it does not fall back to the network on a miss — a provider
    // with no cache simply shows none until someone presses `m`. The type
    // that does this cannot make a request at all, which is a stronger
    // guarantee than remembering not to.
    let model_cache = ModelCache::new(runtime.paths());
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
            let models = model_cache.load(&name);
            providers
                .push(ProviderRow::new(name, provider_config.clone(), layer).with_models(models));
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

/// Write the credential the user just typed into the OS's own secure store.
///
/// # The whole lifetime of the value is this function
///
/// It is taken out of the Settings overlay (which no longer holds it), moved
/// into [`crate::secret::native::NativeSecretStore::store`], and dropped at
/// the closing brace. It is never logged, never put in a status line, and
/// never returned: every `set_status` below names the provider and the store,
/// and nothing else. That is the same rule
/// [`crate::profile::resolve`] follows for the one credential a launch mints.
fn store_provider_credential(state: &mut ShellState) {
    let Some((provider, value)) = state.take_provider_credential_entry() else {
        return;
    };

    let store = secret::native::PreferNativeSecretStore::detect();
    let native = match store.native() {
        Ok(native) => native,
        Err(reason) => {
            // Line 2: an unavailable native store is reported plainly, and
            // the user is told what Glasshouse will read instead rather than
            // being left to guess.
            state.set_status(format!(
                "cannot store a credential: {} — Glasshouse will read {} instead",
                reason.reason(),
                store.describe()
            ));
            return;
        }
    };

    // Filed under the variable name the provider already declares, so the
    // stored credential is found by exactly the reference
    // `crate::profile::resolve` asks with — see
    // `secret::native`'s "a reference names a credential".
    let Some(var) = state.provider_credential_variable(&provider) else {
        state.set_status(format!(
            "`{provider}` names no credential variable to store a credential under"
        ));
        return;
    };
    let reference = secret::native::os_credential_for_variable(&var);

    match native.store(&reference, &value) {
        Ok(()) => {
            let stored = config::StoredCredentialRef::new(secret::native::SERVICE, &var);
            state.record_provider_credential_stored(&provider, stored);
            state.set_status(format!(
                "stored `{provider}`'s credential for {var} in {} — save with `w` to record it",
                native.describe()
            ));
        }
        Err(err) => {
            tracing::warn!(provider = %provider, error = %err, "could not store a credential");
            state.set_status(format!("could not store the credential: {err}"));
        }
    }
}

/// Make the provider request the Settings overlay just planned, **on a
/// thread of its own**.
///
/// # This is the whole point of the batch
///
/// `ureq` is a blocking client. Calling it from here — the thread that reads
/// keys and draws frames — would stop both for as long as the provider took
/// to answer, which for a wedged endpoint is until
/// [`discovery::TOTAL_TIMEOUT`]. A terminal that has stopped repainting and
/// stopped accepting keys is a hung terminal from the user's side, and the
/// fact that it would have come back in twenty seconds is invisible while it
/// is happening. Phase 9E shipped exactly this class of bug once already, and
/// it was found by running the binary rather than by any test.
///
/// So the request goes to a worker thread, which:
///
/// 1. resolves the credential — the first reference in the intent that
///    answers — immediately before the request and nowhere else;
/// 2. makes exactly one request, bounded by
///    [`discovery::ProbeTimeouts::default`];
/// 3. sends the outcome back down `results`;
/// 4. and nudges the event loop, so the answer is drawn the moment it lands
///    rather than at the next tick.
///
/// The thread is deliberately not joined and not tracked. It is bounded by
/// its own timeouts, it holds nothing the shell needs back, and a user who
/// quits while a probe is outstanding should not wait for a provider to
/// answer before their terminal is returned to them.
fn spawn_provider_probe(
    runtime: &Runtime,
    state: &mut ShellState,
    results: &std::sync::mpsc::Sender<ProviderProbeResult>,
    wake: &std::sync::mpsc::Sender<AppEvent>,
    timeouts: discovery::ProbeTimeouts,
) {
    let Some(intent) = state.take_provider_probe_intent() else {
        return;
    };

    let cache = ModelCache::new(runtime.paths());
    let results = results.clone();
    let wake = wake.clone();

    std::thread::Builder::new()
        .name(format!("glasshouse-probe-{}", intent.provider))
        .spawn(move || {
            // Resolved here and not in `state`: this is the last possible
            // moment before the value is needed, it happens off the drawing
            // thread, and the `Secret` it produces lives only as long as this
            // closure. The store a launch would use, so a key in the Keychain
            // is a key this probe can send.
            let store = secret::native::PreferNativeSecretStore::detect();
            let credential = intent
                .secret_refs
                .iter()
                .find_map(|reference| store.resolve(reference));

            let request = ProbeRequest::new(
                intent.provider.clone(),
                intent.protocol,
                intent.base_url.clone(),
                intent.target,
                intent.headers.clone(),
                credential,
            );
            let result = run_provider_probe(&intent, &request, &cache, timeouts);

            // A send failure means the shell has already gone. Nothing to
            // report to and nothing to clean up — the answer is simply
            // dropped, which is the correct outcome for a question nobody is
            // waiting on any more.
            if results.send(result).is_ok() {
                let _ = wake.send(AppEvent::Redraw);
            }
        })
        .map_or_else(
            |err| {
                // A thread that will not start is reported rather than
                // silently retried on this one, which is the failure mode
                // this function exists to prevent.
                tracing::warn!(error = %err, "could not start a provider probe");
                state.set_status(format!("could not start the provider request: {err}"));
            },
            |_handle| (),
        );
}

/// One probe, start to finish, with nothing that touches the terminal.
///
/// Split out from the thread body so it can be called directly by a test,
/// which is what makes the timeout and the cache-write assertions possible
/// without a `Screen`.
fn run_provider_probe(
    intent: &ProviderProbeIntent,
    request: &ProbeRequest,
    cache: &ModelCache,
    timeouts: discovery::ProbeTimeouts,
) -> ProviderProbeResult {
    let endpoint = request.url();
    match intent.kind {
        ProbeKind::Connectivity => ProviderProbeResult {
            provider: intent.provider.clone(),
            notice: ProviderNotice::Reachability(ReachabilityCheck::Answered {
                protocol: intent.protocol.slug(),
                base_url: intent.base_url.clone(),
                endpoint,
                outcome: discovery::connectivity(request, timeouts),
            }),
            catalogue: None,
        },
        ProbeKind::ModelRefresh => match discovery::model_catalogue(request, timeouts) {
            ModelFetch::Catalogue(models) => {
                let catalogue = ModelCatalogue::new(
                    intent.provider.clone(),
                    intent.base_url.clone(),
                    endpoint.clone(),
                    crate::provider::cache::now_unix_seconds(),
                    models,
                );
                // Written before it is reported, so a catalogue the user is
                // told about is one that survives a restart. A write failure
                // is reported as a failed refresh rather than swallowed: a
                // list that vanishes on the next start would be worse than
                // one that never appeared.
                match cache.store(&catalogue) {
                    Ok(_) => ProviderProbeResult {
                        provider: intent.provider.clone(),
                        notice: ProviderNotice::Models(ModelRefresh::Refreshed {
                            count: catalogue.len(),
                            fetched_at: catalogue.fetched_at(),
                            endpoint,
                        }),
                        catalogue: Some(catalogue),
                    },
                    Err(err) => ProviderProbeResult {
                        provider: intent.provider.clone(),
                        notice: ProviderNotice::Models(ModelRefresh::Failed(format!(
                            "fetched {} models but could not cache them: {err}",
                            catalogue.len()
                        ))),
                        catalogue: None,
                    },
                }
            }
            ModelFetch::NotACatalogue { status, reason } => ProviderProbeResult {
                provider: intent.provider.clone(),
                notice: ProviderNotice::Models(ModelRefresh::Failed(format!(
                    "{endpoint} answered {status}, but {reason}"
                ))),
                catalogue: None,
            },
            ModelFetch::Probe(outcome) => ProviderProbeResult {
                provider: intent.provider.clone(),
                notice: ProviderNotice::Models(ModelRefresh::Failed(format!(
                    "{endpoint}: {}",
                    view::describe_probe_outcome(&outcome)
                ))),
                catalogue: None,
            },
        },
    }
}

/// Hand every finished probe to the overlay. Returns whether anything
/// changed and a frame is owed.
fn drain_provider_probes(
    inbox: &std::sync::mpsc::Receiver<ProviderProbeResult>,
    state: &mut ShellState,
) -> bool {
    let mut redraw = false;
    // Every result waiting, not just the first: two providers can have
    // requests outstanding at once, and a loop that took one per tick would
    // make the second look slower than it was.
    while let Ok(result) = inbox.try_recv() {
        if state.apply_provider_probe_result(result) == Action::Redraw {
            redraw = true;
        }
    }
    redraw
}

/// Delete the selected provider's stored credential — line 3.
///
/// Both halves, and both reported: the item leaves the OS store, and the
/// reference leaves the provider's configuration. Deleting one that is not
/// there is **not** an error; it is already the desired state, and saying so
/// is better than raising — see
/// [`crate::secret::native::Deletion::AlreadyAbsent`].
fn delete_provider_credential(state: &mut ShellState) {
    let Some((provider, references)) = state.selected_provider_stored_credentials() else {
        return;
    };

    let store = secret::native::PreferNativeSecretStore::detect();
    let native = match store.native() {
        Ok(native) => native,
        Err(reason) => {
            state.set_status(format!(
                "cannot delete a stored credential: {} — nothing is stored to delete",
                reason.reason()
            ));
            return;
        }
    };

    let mut removed = 0usize;
    for reference in &references {
        match native.delete(reference) {
            Ok(secret::native::Deletion::Removed) => removed += 1,
            Ok(secret::native::Deletion::AlreadyAbsent) => {}
            Err(err) => {
                tracing::warn!(
                    provider = %provider,
                    error = %err,
                    "could not delete a stored credential"
                );
                state.set_status(format!("could not delete the stored credential: {err}"));
                return;
            }
        }
    }

    // The configuration half runs whether or not the store held anything:
    // a reference to a credential that is not there is exactly the record
    // that should go.
    state.record_provider_credential_cleared(&provider);
    state.set_status(if removed > 0 {
        format!(
            "removed `{provider}`'s credential from {} and dropped the reference — \
             save with `w`",
            native.describe()
        )
    } else {
        format!("`{provider}` had nothing stored in {}", native.describe())
    });
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
    // --- Phase 9D: the network never touches the drawing thread ----------
    use crate::provider::cache::{ModelCatalogue, ModelEntry};
    use crate::provider::discovery::{ProbeTarget, ProbeTimeouts};
    use crate::provider::fixture::FixtureProvider;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    /// Short enough that a hanging endpoint is bounded inside a test run,
    /// and far longer than a loopback round trip.
    fn quick_timeouts() -> ProbeTimeouts {
        ProbeTimeouts {
            connect: std::time::Duration::from_millis(500),
            response: std::time::Duration::from_millis(400),
            total: std::time::Duration::from_millis(900),
        }
    }

    fn press(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    /// A shell with Settings open on one provider pointed at `base_url`,
    /// with a credential in the environment so the preconditions pass.
    fn settings_open_on(base_url: &str, var: &str) -> ShellState {
        let mut config = ProviderConfig::new("openrouter");
        config.set_base_url(Some(base_url.to_owned()));
        config.set_credential_env(vec![var.to_owned()]);
        let rows = vec![ProviderRow::new("router", config, ConfigLayer::User)];

        let mut state = ShellState::new("glasshouse", "/work", crate::VERSION, Vec::new());
        state.open_settings(Vec::new(), Vec::new(), rows, Vec::new());
        // Tab to the Providers section: Harnesses, Integrations, Providers.
        state.handle_key(press(KeyCode::Tab));
        state.handle_key(press(KeyCode::Tab));
        state
    }

    /// **Acceptance test 3, through the production spawn path, and the
    /// single most important test in this batch.**
    ///
    /// The fixture accepts the connection and then never writes a byte and
    /// never closes — a wedged provider, not a refused one. A refused
    /// connection is the easy case and proves almost nothing: it comes back
    /// in microseconds whether or not anyone remembered a timeout.
    ///
    /// Two things are asserted, and both matter:
    ///
    /// 1. **The interface stayed alive.** While the request is outstanding
    ///    the main thread — the one that in production reads keys and draws
    ///    frames — keeps handling keystrokes and rendering, and it does so
    ///    many times. Under the bug this batch exists to prevent, the very
    ///    first of those would have blocked until the timeout expired.
    /// 2. **The request came back bounded**, reported as a timeout rather
    ///    than as an unreachable host, because "your network is slow" and
    ///    "your URL is wrong" are different problems.
    #[test]
    fn a_provider_that_accepts_and_never_answers_never_blocks_the_drawing_thread() {
        const VAR: &str = "GLASSHOUSE_SHELL_TEST_ONLY_HANGING_PROBE_VAR";
        // SAFETY: `VAR` is unique to this test and removed again below.
        unsafe {
            std::env::set_var(VAR, "sk-fabricated-test-value-not-a-real-credential");
        }
        let (_data, _workspace, runtime) = bootstrapped_runtime();
        let fixture = FixtureProvider::hanging();
        let mut state = settings_open_on(&fixture.base_url(), VAR);

        assert_eq!(
            state.handle_key(press(KeyCode::Char('t'))),
            Action::RunProviderProbe
        );

        let (results, inbox) = std::sync::mpsc::channel();
        let (wake, wake_inbox) = std::sync::mpsc::channel();
        let started = std::time::Instant::now();
        spawn_provider_probe(&runtime, &mut state, &results, &wake, quick_timeouts());
        unsafe {
            std::env::remove_var(VAR);
        }

        // The loop the run loop would be running. Every iteration is work
        // the drawing thread does *while the request is outstanding*; under
        // the bug, iteration one would not have returned.
        let mut frames = 0usize;
        let mut answer = None;
        while started.elapsed() < std::time::Duration::from_secs(5) {
            assert!(
                state.provider_probe_in_flight() || answer.is_some(),
                "a request that has not come back must still be reported as in flight"
            );
            // A real keystroke, answered while the socket is open.
            state.handle_key(press(if frames.is_multiple_of(2) {
                KeyCode::Down
            } else {
                KeyCode::Up
            }));
            let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(100, 30))
                .expect("a test terminal");
            terminal
                .draw(|frame| view::render(&state, frame))
                .expect("a frame is drawn while the request is outstanding");
            frames += 1;

            if let Ok(result) = inbox.try_recv() {
                answer = Some(result);
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        let elapsed = started.elapsed();

        let answer = answer.expect("the probe must come back, bounded by its own timeout");
        assert!(
            elapsed < std::time::Duration::from_secs(5),
            "the probe must be bounded by its timeout, not by the peer; took {elapsed:?}"
        );
        assert!(
            frames > 5,
            "the interface must have kept drawing while the request was outstanding; \
             it managed {frames} frames in {elapsed:?}"
        );
        assert_eq!(
            fixture.connections(),
            1,
            "the probe must really have connected — a refused connection would prove \
             nothing about a stall"
        );

        match &answer.notice {
            ProviderNotice::Reachability(ReachabilityCheck::Answered { outcome, .. }) => assert!(
                matches!(
                    outcome,
                    crate::provider::discovery::ProbeOutcome::TimedOut { .. }
                ),
                "a stall must be reported as a timeout, not as an unreachable host: {outcome:?}"
            ),
            other => panic!("expected a connectivity answer, got {other:?}"),
        }

        // And the answer reaches the state, clearing the in-flight marker.
        assert_eq!(state.apply_provider_probe_result(answer), Action::Redraw);
        assert!(!state.provider_probe_in_flight());

        // The worker nudged the event loop, so the answer is drawn when it
        // lands rather than at the next tick.
        assert!(
            wake_inbox.try_recv().is_ok(),
            "a finished probe must wake the interface"
        );
    }

    /// **Acceptance test 5.** Starting with a cached catalogue issues no
    /// network request at all.
    ///
    /// Asserted on the fixture seeing **zero connections**, not on elapsed
    /// time. A timing assertion would pass on a fast machine no matter what
    /// the code did; a connection counter cannot.
    #[test]
    fn opening_settings_with_a_cached_catalogue_opens_no_connection_at_all() {
        let (_data, _workspace, runtime) = bootstrapped_runtime();
        let fixture = FixtureProvider::answering(
            "HTTP/1.1 200 OK",
            "",
            r#"{"data":[{"id":"should/never/be/fetched"}]}"#,
        );

        let mut config = ProviderConfig::new("openrouter");
        config.set_base_url(Some(fixture.base_url()));
        save_user_settings(
            &runtime,
            &[],
            &[ProviderSettingsEdit {
                name: "router".to_owned(),
                upsert: Some(config),
            }],
            &[],
        )
        .expect("the provider is configured");

        // A catalogue already on disk, as a previous run's refresh would
        // have left it.
        let cache = ModelCache::new(runtime.paths());
        cache
            .store(&ModelCatalogue::new(
                "router",
                fixture.base_url(),
                format!("{}/models", fixture.base_url()),
                1_787_336_476,
                vec![ModelEntry::new("cached/one"), ModelEntry::new("cached/two")],
            ))
            .expect("the cache is written");

        let (harnesses, integrations, providers, profiles) =
            build_settings(&runtime).expect("settings open");

        assert_eq!(
            fixture.connections(),
            0,
            "opening Settings made a network request; Phase 9D line 3 exists to stop \
             Glasshouse querying a remote catalogue on every start"
        );

        let row = providers
            .iter()
            .find(|row| row.name == "router")
            .expect("the row");
        let models = row.models.as_ref().expect("the cached catalogue is loaded");
        assert_eq!(models.len(), 2);
        assert_eq!(models.fetched_at(), 1_787_336_476);
        assert_eq!(models.models()[0].id(), "cached/one");
        assert!(
            !models.models().iter().any(|m| m.id().contains("never")),
            "the list must be the cached one, not one the fixture served"
        );

        // Rendering it opens nothing either — a renderer that fetched
        // lazily would be the same bug wearing a different hat.
        let mut state = ShellState::new("glasshouse", "/work", crate::VERSION, Vec::new());
        state.open_settings(harnesses, integrations, providers, profiles);
        state.handle_key(press(KeyCode::Tab));
        state.handle_key(press(KeyCode::Tab));
        let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(200, 40))
            .expect("a test terminal");
        terminal
            .draw(|frame| view::render(&state, frame))
            .expect("a frame");
        assert_eq!(
            fixture.connections(),
            0,
            "drawing a cached model list must not fetch one"
        );
    }

    /// A provider with no cache is simply a provider with no models. It does
    /// **not** become a fetch — the counterpart to the test above, so "zero
    /// connections" cannot be passing because nothing was configured.
    #[test]
    fn a_provider_with_no_cached_catalogue_fetches_nothing_on_open_either() {
        let (_data, _workspace, runtime) = bootstrapped_runtime();
        let fixture = FixtureProvider::answering("HTTP/1.1 200 OK", "", r#"{"data":[]}"#);

        let mut config = ProviderConfig::new("openrouter");
        config.set_base_url(Some(fixture.base_url()));
        save_user_settings(
            &runtime,
            &[],
            &[ProviderSettingsEdit {
                name: "router".to_owned(),
                upsert: Some(config),
            }],
            &[],
        )
        .expect("configured");

        let (_, _, providers, _) = build_settings(&runtime).expect("settings open");
        assert_eq!(fixture.connections(), 0);
        assert!(
            providers[0].models.is_none(),
            "no cache means no models, never an implicit fetch"
        );
    }

    /// **Acceptance test 4, end to end.** A manual refresh fetches, replaces
    /// the cache on disk, moves the timestamp, and survives a reopen — which
    /// is what "cached" has to mean.
    #[test]
    fn a_manual_refresh_writes_the_catalogue_to_disk_and_a_reopen_finds_it() {
        const VAR: &str = "GLASSHOUSE_SHELL_TEST_ONLY_REFRESH_VAR";
        // SAFETY: `VAR` is unique to this test and removed again below.
        unsafe {
            std::env::set_var(VAR, "sk-fabricated-test-value-not-a-real-credential");
        }
        let (_data, _workspace, runtime) = bootstrapped_runtime();
        let fixture = FixtureProvider::answering(
            "HTTP/1.1 200 OK",
            "",
            r#"{"data":[{"id":"vendor/a"},{"id":"vendor/b"},{"id":"vendor/c"}]}"#,
        );

        let mut config = ProviderConfig::new("openrouter");
        config.set_base_url(Some(fixture.base_url()));
        config.set_credential_env(vec![VAR.to_owned()]);
        save_user_settings(
            &runtime,
            &[],
            &[ProviderSettingsEdit {
                name: "router".to_owned(),
                upsert: Some(config),
            }],
            &[],
        )
        .expect("configured");

        // A stale catalogue, so this proves a replacement rather than a
        // first write.
        let cache = ModelCache::new(runtime.paths());
        cache
            .store(&ModelCatalogue::new(
                "router",
                fixture.base_url(),
                format!("{}/models", fixture.base_url()),
                1_000,
                vec![ModelEntry::new("stale/one")],
            ))
            .expect("stale cache written");

        let (harnesses, integrations, providers, profiles) =
            build_settings(&runtime).expect("settings open");
        let mut state = ShellState::new("glasshouse", "/work", crate::VERSION, Vec::new());
        state.open_settings(harnesses, integrations, providers, profiles);
        state.handle_key(press(KeyCode::Tab));
        state.handle_key(press(KeyCode::Tab));

        assert_eq!(
            state.handle_key(press(KeyCode::Char('m'))),
            Action::RunProviderProbe
        );
        let (results, inbox) = std::sync::mpsc::channel();
        let (wake, _wake_inbox) = std::sync::mpsc::channel();
        spawn_provider_probe(&runtime, &mut state, &results, &wake, quick_timeouts());
        unsafe {
            std::env::remove_var(VAR);
        }

        let result = inbox
            .recv_timeout(std::time::Duration::from_secs(5))
            .expect("the refresh must come back");
        assert_eq!(fixture.requests().len(), 1, "exactly one request, no other");
        assert_eq!(
            fixture.requests()[0].target,
            "/models",
            "the model list, at the path the provider's own base URL names"
        );

        let fetched_at = match &result.notice {
            ProviderNotice::Models(ModelRefresh::Refreshed {
                count, fetched_at, ..
            }) => {
                assert_eq!(*count, 3);
                *fetched_at
            }
            other => panic!("expected a refreshed catalogue, got {other:?}"),
        };
        assert!(
            fetched_at > 1_000,
            "the timestamp must move forward on a refresh, or a stale list looks fresh"
        );
        state.apply_provider_probe_result(result);

        // On disk, and found by a completely fresh read — the thing that
        // makes the next start silent.
        let (_, _, reopened, _) = build_settings(&runtime).expect("settings reopen");
        let models = reopened[0].models.as_ref().expect("a cached catalogue");
        assert_eq!(models.len(), 3);
        assert_eq!(models.fetched_at(), fetched_at);
        assert!(
            !models.models().iter().any(|m| m.id() == "stale/one"),
            "a refresh replaces the cached list; it must never append to it"
        );
        assert_eq!(
            fixture.requests().len(),
            1,
            "and reopening Settings must not have fetched again"
        );
    }

    /// **Acceptance test 2, end to end.** A provider answering `401` is
    /// reported as reachable-but-rejected.
    #[test]
    fn a_provider_answering_401_is_reported_as_reachable_but_rejected() {
        const VAR: &str = "GLASSHOUSE_SHELL_TEST_ONLY_REJECTED_VAR";
        // SAFETY: `VAR` is unique to this test and removed again below.
        unsafe {
            std::env::set_var(VAR, "sk-fabricated-test-value-not-a-real-credential");
        }
        let (_data, _workspace, runtime) = bootstrapped_runtime();
        let fixture = FixtureProvider::answering(
            "HTTP/1.1 401 Unauthorized",
            "",
            r#"{"error":{"message":"Authentication parameter not received in Header"}}"#,
        );
        let mut state = settings_open_on(&fixture.base_url(), VAR);
        state.handle_key(press(KeyCode::Char('t')));

        let (results, inbox) = std::sync::mpsc::channel();
        let (wake, _wake_inbox) = std::sync::mpsc::channel();
        spawn_provider_probe(&runtime, &mut state, &results, &wake, quick_timeouts());
        unsafe {
            std::env::remove_var(VAR);
        }

        let result = inbox
            .recv_timeout(std::time::Duration::from_secs(5))
            .expect("the probe comes back");
        match &result.notice {
            ProviderNotice::Reachability(ReachabilityCheck::Answered { outcome, .. }) => {
                assert_eq!(
                    outcome,
                    &crate::provider::discovery::ProbeOutcome::Rejected { status: 401 }
                );
            }
            other => panic!("expected a rejection, got {other:?}"),
        }
        state.apply_provider_probe_result(result);

        let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(200, 40))
            .expect("a test terminal");
        terminal
            .draw(|frame| view::render(&state, frame))
            .expect("a frame");
        let text: String = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(ratatui::buffer::Cell::symbol)
            .collect();
        assert!(
            text.contains("reachable, but it did not accept the credential"),
            "the user must be told which of the two problems they have: {text}"
        );
    }

    /// The run loop probes with the production timeouts and nothing else.
    ///
    /// A source scan, in the same idiom as `secret`'s own. The parameter that
    /// makes the tests above fast is also a parameter someone could quietly
    /// widen at the one call site that matters, and that call site is not
    /// otherwise reachable from a test without a real terminal.
    #[test]
    fn the_run_loop_probes_with_the_default_timeouts() {
        assert!(
            run_loop_passes_the_default_timeouts(include_str!("mod.rs")),
            "the run loop must pass the default timeouts, not values of its own"
        );
    }

    /// Whether the run loop's own call to `spawn_provider_probe` passes
    /// [`discovery::ProbeTimeouts::default`].
    ///
    /// # Scanned by lines, deliberately
    ///
    /// The first version of this searched for the literal
    /// `"spawn_provider_probe(\n"`. That is a **multi-line literal**, and on a
    /// checkout where Git converts line endings the source
    /// [`include_str!`] hands back contains `\r\n`, so the search finds
    /// nothing and the scan fails by *panicking* rather than by asserting.
    /// Windows CI went red on exactly that, for a test that has nothing to do
    /// with platforms — the second time this repository has paid for the same
    /// mistake, which is why the practice file has a section about it.
    ///
    /// [`str::lines`] strips the carriage return, so this is CRLF-agnostic by
    /// construction rather than by remembering. See
    /// `the_scan_finds_the_call_whatever_the_line_endings_are`, which proves
    /// it against a CRLF copy of this very file — an LF checkout never
    /// exercises the broken path, so without that control the fix would be
    /// untested precisely where it was needed.
    fn run_loop_passes_the_default_timeouts(source: &str) -> bool {
        let production = source.split("#[cfg(test)]").next().unwrap_or(source);
        let lines: Vec<&str> = production.lines().collect();
        let call = lines.iter().position(|line| {
            let trimmed = line.trim();
            // The call site, not the `fn spawn_provider_probe(` definition
            // line and not a single-line test call.
            trimmed == "spawn_provider_probe("
        });
        let Some(call) = call else { return false };
        lines
            .iter()
            .skip(call)
            .take(12)
            .any(|line| line.contains("discovery::ProbeTimeouts::default()"))
    }

    /// The control that keeps the scan above honest.
    ///
    /// Both sides are built from a **normalised** base rather than from
    /// whatever `include_str!` happened to produce, because an assertion whose
    /// input varies with the checkout is a flake generator that will find the
    /// environment you did not test on.
    #[test]
    fn the_scan_finds_the_call_whatever_the_line_endings_are() {
        let normalised = include_str!("mod.rs").replace("\r\n", "\n");
        let crlf = normalised.replace('\n', "\r\n");
        assert!(
            run_loop_passes_the_default_timeouts(&normalised),
            "the scan must find the call in an LF checkout"
        );
        assert!(
            run_loop_passes_the_default_timeouts(&crlf),
            "the scan must find the call in a CRLF checkout — this is the assertion \
             Windows CI failed on"
        );
        // And it must be capable of saying no, or the two above prove nothing.
        assert!(
            !run_loop_passes_the_default_timeouts("fn main() {}\n"),
            "a source with no such call must not report that the call is correct"
        );
    }

    /// A credential reaches the provider's `authorization` header and no
    /// other surface the run loop touches — including the cache file it
    /// writes, which is a new place on disk for one to end up.
    ///
    /// `!contains`, never `assert_eq!`, on the raw bytes.
    #[test]
    fn a_planted_credential_reaches_the_header_and_not_the_cache_file() {
        const VAR: &str = "GLASSHOUSE_SHELL_TEST_ONLY_LEAK_VAR";
        const VALUE: &str = "sk-planted-run-loop-credential-9d";
        // SAFETY: `VAR` is unique to this test and removed again below.
        unsafe {
            std::env::set_var(VAR, VALUE);
        }
        let (_data, _workspace, runtime) = bootstrapped_runtime();
        let fixture =
            FixtureProvider::answering("HTTP/1.1 200 OK", "", r#"{"data":[{"id":"vendor/a"}]}"#);

        let mut config = ProviderConfig::new("openrouter");
        config.set_base_url(Some(fixture.base_url()));
        config.set_credential_env(vec![VAR.to_owned()]);
        let rows = vec![ProviderRow::new("router", config, ConfigLayer::User)];
        let mut state = ShellState::new("glasshouse", "/work", crate::VERSION, Vec::new());
        state.open_settings(Vec::new(), Vec::new(), rows, Vec::new());
        state.handle_key(press(KeyCode::Tab));
        state.handle_key(press(KeyCode::Tab));
        state.handle_key(press(KeyCode::Char('m')));

        let (results, inbox) = std::sync::mpsc::channel();
        let (wake, _wake_inbox) = std::sync::mpsc::channel();
        spawn_provider_probe(&runtime, &mut state, &results, &wake, quick_timeouts());
        let result = inbox
            .recv_timeout(std::time::Duration::from_secs(5))
            .expect("the refresh comes back");
        // Removed only once the worker has finished with it: the credential
        // is resolved on that thread, at the moment of use, so unsetting the
        // variable any earlier is a race with the code under test rather
        // than a tidy-up.
        unsafe {
            std::env::remove_var(VAR);
        }

        // It really was sent — otherwise every `!contains` below would pass
        // for the wrong reason.
        let sent = fixture.requests();
        assert_eq!(
            sent[0].header("authorization"),
            Some(format!("Bearer {VALUE}").as_str())
        );

        assert!(!format!("{result:?}").contains(VALUE), "a probe result");
        state.apply_provider_probe_result(result);
        assert!(
            !format!("{:?}", state.settings().unwrap().providers()).contains(VALUE),
            "a provider row"
        );

        // The cache file on disk, byte for byte.
        let path = ModelCache::new(runtime.paths()).path_for("router");
        let bytes = std::fs::read(&path).expect("the refresh wrote a cache file");
        assert!(
            !bytes.is_empty(),
            "and it is not empty, so this checks something"
        );
        assert!(
            !String::from_utf8_lossy(&bytes).contains(VALUE),
            "a credential reached the cache file at {}",
            path.display()
        );

        // And the whole rendered screen.
        let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(400, 60))
            .expect("a test terminal");
        terminal
            .draw(|frame| view::render(&state, frame))
            .expect("a frame");
        let text: String = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(ratatui::buffer::Cell::symbol)
            .collect();
        assert!(!text.contains(VALUE), "a credential was rendered on screen");
    }

    /// A probe whose target is the bare base URL appends no path.
    ///
    /// The `ollama` template's model list is `Unverified`, so a connectivity
    /// test of it asks the base URL itself rather than guessing `/models` —
    /// and the fixture is what proves no path was invented.
    #[test]
    fn a_provider_with_no_established_model_list_is_probed_at_its_base_url() {
        let (_data, _workspace, runtime) = bootstrapped_runtime();
        let fixture = FixtureProvider::answering("HTTP/1.1 200 OK", "", "ok");

        let mut config = ProviderConfig::new("ollama");
        config.set_base_url(Some(format!("{}/v1", fixture.base_url())));
        let rows = vec![ProviderRow::new("local", config, ConfigLayer::User)];
        let mut state = ShellState::new("glasshouse", "/work", crate::VERSION, Vec::new());
        state.open_settings(Vec::new(), Vec::new(), rows, Vec::new());
        state.handle_key(press(KeyCode::Tab));
        state.handle_key(press(KeyCode::Tab));
        state.handle_key(press(KeyCode::Char('t')));

        let (results, inbox) = std::sync::mpsc::channel();
        let (wake, _wake_inbox) = std::sync::mpsc::channel();
        spawn_provider_probe(&runtime, &mut state, &results, &wake, quick_timeouts());
        inbox
            .recv_timeout(std::time::Duration::from_secs(5))
            .expect("the probe comes back");

        assert_eq!(
            fixture.requests()[0].target,
            "/v1",
            "a provider with no established model list must be asked for its base URL, \
             never a path nobody read from its documentation"
        );
    }

    /// `ProbeTarget` is chosen from the provider's own declaration, and the
    /// two templates that bracket the choice are asserted by name.
    #[test]
    fn the_probe_target_follows_whether_a_model_list_was_established() {
        const VAR: &str = "GLASSHOUSE_SHELL_TEST_ONLY_TARGET_MATRIX_VAR";
        // SAFETY: `VAR` is unique to this test and removed again below. It
        // exists because the preconditions are checked before a target is
        // chosen, so a template whose credential variable is unset would
        // never reach the line under test.
        unsafe {
            std::env::set_var(VAR, "sk-fabricated-test-value-not-a-real-credential");
        }
        for (template, expected) in [
            ("openrouter", ProbeTarget::ModelList),
            ("litellm", ProbeTarget::ModelList),
            ("ollama", ProbeTarget::BaseUrl),
            ("nvidia", ProbeTarget::BaseUrl),
        ] {
            let mut config = ProviderConfig::new(template);
            config.set_base_url(Some("http://127.0.0.1:1/v1".to_owned()));
            config.set_credential_env(vec![VAR.to_owned()]);
            let rows = vec![ProviderRow::new("p", config, ConfigLayer::User)];
            let mut state = ShellState::new("glasshouse", "/work", crate::VERSION, Vec::new());
            state.open_settings(Vec::new(), Vec::new(), rows, Vec::new());
            state.handle_key(press(KeyCode::Tab));
            state.handle_key(press(KeyCode::Tab));
            state.handle_key(press(KeyCode::Char('t')));
            let intent = state
                .take_provider_probe_intent()
                .unwrap_or_else(|| panic!("{template} must plan a probe"));
            assert_eq!(intent.target, expected, "for the {template} template");
        }
        unsafe {
            std::env::remove_var(VAR);
        }
    }

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
