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
use crate::checkpoint::{Checkpoint, CheckpointReason, CheckpointStore, ProjectCheckpoints};
use crate::config::{self, EffectiveConfig, UserConfig};
use crate::events::{EventBus, EventLog, EventLogSink, LifecycleEvent, ProcessExit, RecordedEvent};
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
    Action, HarnessRow, IntegrationRow, KnowledgeSection, MemoryDetail, MemoryRow,
    MemorySettingsEdit, Mode, ModelRefresh, Overlay, OverviewState, ProbeKind, ProfileRow,
    ProfileSettingsEdit, ProviderNotice, ProviderProbeIntent, ProviderProbeResult, ProviderRow,
    ProviderSettingsEdit, ReachabilityCheck, RouteEvidenceRow, RouteHealthRow, RoutingRow,
    RoutingSettingsEdit, SettingsEdit, ShellState, ViewportGrid,
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
    // The one normalized lifecycle stream, owned here and shared with the
    // session runtime, so that everything the runtime publishes reaches this
    // shell's consumers — and, through the sink below, the project's durable
    // log.
    let events = EventBus::new();
    let event_log = attach_event_log(runtime, &events);
    // Drained every tick. Publishing never waits on this, by construction:
    // the queue is bounded and the oldest events are dropped if a viewport
    // stops draining — see `crate::events::bus`.
    let event_stream = events.subscribe();

    let checkpoints = ProjectCheckpoints::open(runtime)?;

    let mut live = SessionRuntime::with_event_bus(
        crate::session::runtime::DEFAULT_SCROLLBACK_BYTES,
        events.clone(),
    );
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
    // And where a harness's own reports come back. Same shape, same reason:
    // reading them means reading SQLite, and a reader can be made to wait on
    // whoever holds the write lock. On the drawing thread that is a frozen
    // interface, which is a defect this project has already shipped once.
    let (reported, reported_inbox) = std::sync::mpsc::channel::<Vec<RecordedEvent>>();
    spawn_event_tail(runtime, &reported, &events.sender());

    screen.draw(|frame| view::render(&state, frame))?;

    // Every `return` below leaves through this, so the last few events reach
    // the database rather than dying with the writer thread.
    let _flush = FlushOnLeaving(event_log);

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
                    Action::ResumeSession(id) => {
                        let name = state::short_session_id(id);
                        match resume_session(
                            runtime,
                            &mut live,
                            &sessions,
                            id,
                            viewport_terminal_size(&screen),
                        ) {
                            Ok(()) => {
                                if let Ok(records) = sessions.store().list() {
                                    state.refresh(records);
                                }
                                state.set_status(format!("resumed session `{name}`"));
                            }
                            Err(err) => {
                                tracing::warn!(session = %id, error = %err, "could not resume a session");
                                state.set_status(format!("could not resume `{name}`: {err:#}"));
                            }
                        }
                    }
                    Action::OpenSettings => match build_settings(runtime) {
                        Ok((harnesses, integrations, providers, profiles, routing, memory)) => {
                            state.open_settings_with_routing(
                                harnesses,
                                integrations,
                                providers,
                                profiles,
                                routing,
                                memory,
                            );
                        }
                        Err(err) => {
                            tracing::warn!(error = %err, "could not open settings");
                            state.set_status(format!("could not open settings: {err:#}"));
                        }
                    },
                    Action::OpenProjectOverview => {
                        let resources = build_project_overview_capacity(runtime);
                        match build_project_overview_memory(runtime) {
                            Ok(memory) => {
                                state.open_project_overview(
                                    memory.decisions,
                                    memory.todos,
                                    memory.todos_omitted,
                                    resources,
                                    None,
                                );
                            }
                            Err(err) => {
                                tracing::warn!(
                                    error = %err,
                                    "could not read project memory for the overview"
                                );
                                state.open_project_overview(
                                    Vec::new(),
                                    Vec::new(),
                                    0,
                                    resources,
                                    Some(format!("project memory unavailable: {err:#}")),
                                );
                            }
                        }
                    }
                    Action::OpenProjectKnowledge => match build_project_knowledge_memory(runtime) {
                        Ok(memory) => {
                            state.open_project_knowledge(
                                memory.decisions,
                                memory.constraints,
                                memory.features,
                                memory.failed_attempts,
                                memory.todos,
                                None,
                            );
                        }
                        Err(err) => {
                            tracing::warn!(
                                error = %err,
                                "could not read project memory for the knowledge view"
                            );
                            state.open_project_knowledge(
                                KnowledgeSection::default(),
                                KnowledgeSection::default(),
                                KnowledgeSection::default(),
                                KnowledgeSection::default(),
                                KnowledgeSection::default(),
                                Some(format!("project memory unavailable: {err:#}")),
                            );
                        }
                    },
                    Action::OpenRouteEvidence => match build_route_evidence_table(runtime) {
                        Ok(rows) => {
                            state.open_route_evidence(rows, None);
                        }
                        Err(err) => {
                            tracing::warn!(
                                error = %err,
                                "could not read the routing evidence ledger for the route table"
                            );
                            state.open_route_evidence(
                                Vec::new(),
                                Some(format!("routing evidence unavailable: {err:#}")),
                            );
                        }
                    },
                    Action::OpenRouteHealth => {
                        state.open_route_health(build_route_health_table(runtime));
                    }
                    Action::OpenProjectMemory => match build_project_memory_view(runtime) {
                        Ok(memory) => {
                            state.open_project_memory(memory, None);
                        }
                        Err(err) => {
                            tracing::warn!(
                                error = %err,
                                "could not read project memory for the project-memory view"
                            );
                            state.open_project_memory(
                                KnowledgeSection::default(),
                                Some(format!("project memory unavailable: {err:#}")),
                            );
                        }
                    },
                    Action::SaveUserSettings => {
                        let harness_edits = state.settings_edits();
                        let provider_edits = state.settings_provider_edits();
                        let profile_edits = state.settings_profile_edits();
                        let routing_edit = state.settings_routing_edit();
                        let memory_edit = state.settings_memory_edit();
                        if harness_edits.is_empty()
                            && provider_edits.is_empty()
                            && profile_edits.is_empty()
                            && routing_edit.is_none()
                            && memory_edit.is_none()
                        {
                            state.set_status("no settings changes to save");
                        } else if let Err(err) = save_user_settings_with_routing(
                            runtime,
                            &harness_edits,
                            &provider_edits,
                            &profile_edits,
                            routing_edit.as_ref(),
                            memory_edit.as_ref(),
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
                        let routing_edit = state.settings_routing_edit();
                        let memory_edit = state.settings_memory_edit();
                        if harness_edits.is_empty()
                            && provider_edits.is_empty()
                            && profile_edits.is_empty()
                            && routing_edit.is_none()
                            && memory_edit.is_none()
                        {
                            state.set_status("no settings changes to save");
                        } else {
                            match save_project_settings_with_routing(
                                runtime,
                                &harness_edits,
                                &provider_edits,
                                &profile_edits,
                                routing_edit.as_ref(),
                                memory_edit.as_ref(),
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
                let exits = live.poll_exits();
                let any_exited = !exits.is_empty();
                for (id, status) in exits {
                    // `ProcessExit` owns this classification and is the only
                    // place it lives. It used to be computed inline here as
                    // well, which is two definitions of "did it crash" — and
                    // two definitions of that eventually disagree about a
                    // signal, which is the case that comes up least often and
                    // costs the most when it is wrong.
                    let lifecycle = ProcessExit::from_status(&status).session_state();
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
                // Phase 11 line 688: a session's disposition can only turn
                // `Resumable` here, on exit, and nothing else ever refreshes
                // `state.sessions()` on its own — the session bar and
                // overview otherwise only re-read the store on specific
                // triggers (starting a session, `AppEvent::Redraw`), and an
                // exit noticed on this tick is neither. Without this, `r`
                // pressed against a session that exited moments ago would
                // refuse it as "still running" against a record this loop
                // had already, correctly, marked `Stopped` underneath it.
                if any_exited && state.refresh(sessions.store().list()?) == Action::Redraw {
                    redraw = true;
                }

                // Everything that happened since the last tick, from both
                // sides of the one stream: what this process published, and
                // what a harness reported to a hook process the interface
                // never sees. Drained on the interface's own thread — never
                // on the one reading a pseudo-terminal, which is the whole
                // point of the bus being a queue rather than a callback.
                let mut recorded = event_stream.drain();
                recorded.extend(reported_inbox.try_iter().flatten());
                // The consumer that makes this a delivery rather than a
                // path: the overview's activity view shows these, and a user
                // pressing `o` sees what their sessions have been doing. A
                // drain whose result went nowhere would be a delivery path
                // with nothing at the end of it, which is the state this
                // capability sat in until now.
                if state.note_events(&recorded) == Action::Redraw {
                    redraw = true;
                }
                if !recorded.is_empty()
                    && checkpoint_task_boundaries(
                        &checkpoints.store(),
                        &sessions,
                        runtime,
                        &recorded,
                        &mut state,
                    )
                {
                    redraw = true;
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

/// Send this shell's lifecycle events to the project's durable log as well.
///
/// Best effort by construction, and the direction of the trade is the point:
/// a project whose database cannot be opened loses event history and keeps
/// its sessions. Refusing to open the interface because a diagnostic log
/// could not be attached would be Glasshouse's bookkeeping mattering more
/// than the sessions it keeps books about, which it never does.
///
/// The sink queues behind a writer thread rather than writing inline — see
/// [`crate::events::log`]. Publishing happens on whichever thread produced
/// the event, and one of those is the thread draining a pseudo-terminal.
fn attach_event_log(runtime: &Runtime, events: &EventBus) -> Option<std::sync::Arc<EventLogSink>> {
    match EventLog::open(runtime) {
        Ok(log) => {
            let sink = EventLogSink::spawn(log);
            events.attach_sink(
                std::sync::Arc::clone(&sink) as std::sync::Arc<dyn crate::events::EventSink>
            );
            Some(sink)
        }
        Err(err) => {
            tracing::warn!(
                error = %format!("{err:#}"),
                "could not open the project event log; this session's events will not be recorded"
            );
            None
        }
    }
}

/// Watch the project's event log for what a harness reported to a hook.
///
/// # Why the interface cannot simply subscribe to the bus for these
///
/// A lifecycle hook runs as its **own short-lived process** — that is how
/// every supported harness reports, and it is why `glasshouse hook` exists at
/// all. Its events are minted on that process's bus and it exits. Nothing on
/// this process's bus ever sees them, so an interface that only subscribed
/// would show a session's own keystrokes and never once show it finishing a
/// turn.
///
/// The project's event log is the seam between the two, because it is the one
/// ordering both processes write into. This reads it and delivers what it
/// finds through the same channel `spawn_provider_probe` uses, for the same
/// reason: **reading it means reading SQLite, and a reader waits on whoever
/// holds the write lock.** On the drawing thread that is a frozen interface —
/// the exact defect class this project shipped once already, in a settings
/// screen that made a blocking call where the terminal was being painted.
///
/// It starts from the log's current head rather than its beginning: opening
/// the interface should show what happens next, not replay a week.
fn spawn_event_tail(
    runtime: &Runtime,
    reported: &std::sync::mpsc::Sender<Vec<RecordedEvent>>,
    wake: &std::sync::mpsc::Sender<AppEvent>,
) {
    /// How often the log is asked what is new.
    ///
    /// Far slower than the interface's own tick: this is a database query,
    /// and a harness event arriving a quarter of a second later than it
    /// happened is imperceptible next to the turn it belongs to.
    const POLL: std::time::Duration = std::time::Duration::from_millis(250);
    /// Most rows to take in one pass, so a log that grew while Glasshouse was
    /// closed cannot arrive as one enormous message.
    const BATCH: usize = 256;

    let log = match EventLog::open(runtime) {
        Ok(log) => log,
        Err(err) => {
            tracing::warn!(
                error = %format!("{err:#}"),
                "could not read the project event log; harness reports will not reach the interface"
            );
            return;
        }
    };
    let reported = reported.clone();
    let wake = wake.clone();

    // Not joined and not stopped explicitly. It ends by itself when the
    // channel's receiver goes, which happens when `run` returns — the same
    // lifetime `spawn_provider_probe`'s thread has, and for the same reason:
    // there is nothing to clean up but a `Sender`.
    let started = std::thread::Builder::new()
        .name("glasshouse-event-tail".to_owned())
        .spawn(move || {
            let mut after = log.head().unwrap_or(0);
            loop {
                match log.observed_since(after, BATCH) {
                    Ok(fresh) if !fresh.is_empty() => {
                        after = fresh.last().map(|event| event.seq).unwrap_or(after);
                        let batch: Vec<RecordedEvent> = fresh
                            .into_iter()
                            .map(|event| event.into_recorded())
                            .collect();
                        // A send failure means the interface has gone. That
                        // is the end of this thread's job, not an error.
                        if reported.send(batch).is_err() {
                            return;
                        }
                        let _ = wake.send(AppEvent::Redraw);
                    }
                    Ok(_) => {}
                    Err(err) => {
                        // One unreadable poll must not end the watch: a busy
                        // database is the ordinary case this exists to absorb.
                        tracing::debug!(%err, "could not read the project event log");
                    }
                }
                std::thread::sleep(POLL);
            }
        });
    if let Err(err) = started {
        tracing::warn!(%err, "could not start the event-log reader");
    }
}

/// Waits briefly for the event log's writer to catch up on the way out.
///
/// A guard rather than a call, because `shell::run` returns from several
/// places and the one that would get forgotten is whichever is added next.
///
/// Bounded, for the reason [`crate::shutdown`] gives about its own cleanup:
/// failing to record the last few events is survivable, and failing to give
/// the user their terminal back is not.
struct FlushOnLeaving(Option<std::sync::Arc<EventLogSink>>);

impl Drop for FlushOnLeaving {
    fn drop(&mut self) {
        const BOUND: std::time::Duration = std::time::Duration::from_millis(500);
        if let Some(sink) = &self.0
            && !sink.flush(BOUND)
        {
            tracing::warn!(
                dropped = sink.dropped(),
                "the event log did not finish writing before the shell closed"
            );
        }
    }
}

/// Take an automatic checkpoint for every session whose turn just ended.
///
/// # What "automatically" can and cannot mean here
///
/// A checkpoint's objective, state and next actions are authored — Glasshouse
/// does not know them and will not guess them from a session's terminal
/// output, for the same reason nothing else in this codebase reads state out
/// of scrollback. So an automatic checkpoint **carries forward the handoff the
/// user last wrote for that session**, restamped with the current time and the
/// repository's current position.
///
/// That is worth doing and is not a substitute for writing one: it keeps the
/// most recent checkpoint fresh as of the last task boundary, so a session
/// that dies leaves a handoff describing where the repository actually was
/// rather than where it was an hour ago. A session whose user has never taken
/// a checkpoint gets nothing, silently, because the alternative is inventing
/// one.
///
/// Returns whether anything worth repainting happened.
fn checkpoint_task_boundaries(
    checkpoints: &CheckpointStore<'_>,
    sessions: &ProjectSessions,
    runtime: &Runtime,
    recorded: &[RecordedEvent],
    state: &mut ShellState,
) -> bool {
    let mut noted = false;
    for event in recorded {
        // A turn ending is the task boundary Glasshouse actually detects.
        // Nothing else in the stream is one: a process exiting says the
        // harness is gone, not that the work finished, and that distinction
        // is the whole of `crate::events`'s doc comment.
        if !matches!(event.event(), LifecycleEvent::TurnEnded { .. }) {
            continue;
        }
        let id = event.session();
        let previous = match checkpoints.latest_for(id) {
            Ok(Some(previous)) => previous,
            // No handoff has ever been written for this session, so there is
            // nothing to carry forward and nothing honest to invent.
            Ok(None) => continue,
            Err(err) => {
                tracing::warn!(session = %id, %err, "could not read a session's checkpoint");
                continue;
            }
        };

        let harness = match sessions.store().get(id) {
            Ok(Some(record)) => record.harness,
            // The checkpoint's own record of which harness wrote it is the
            // fallback, and it is the right one: it is what was true when the
            // handoff was authored.
            _ => previous.checkpoint.harness.clone(),
        };

        let refreshed = Checkpoint::capture(
            id,
            &harness,
            CheckpointReason::TaskBoundary,
            checkpoints.now(),
            runtime.project().root(),
            previous.checkpoint.handoff.clone(),
        );
        match checkpoints.save(refreshed) {
            Ok(stored) => {
                state.set_status(format!(
                    "checkpointed `{}` at a turn boundary ({})",
                    state::short_session_id(id),
                    stored.id.short()
                ));
                noted = true;
            }
            Err(err) => {
                tracing::warn!(session = %id, %err, "could not take an automatic checkpoint");
            }
        }
    }
    noted
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

    // Phase 9A line 368. The shell's quick-open resolves no launch profile
    // and no response request of its own — there is no surface here to ask
    // for either, see `HarnessSelection::install_hooks`'s own doc comment —
    // so both are the implied defaults a session gets when nobody asked for
    // anything else: the `Native` profile, and the `Interactive` role. That
    // is still six real facts, not four blanks: a session opened with `n`
    // now records the same kind of answer `glasshouse launch` does for an
    // unadorned `glasshouse launch <harness>`, rather than `-` for every
    // column `main.rs::launch_session` fills in.
    let launch_profile = crate::profile::LaunchProfile::native(selection.id());
    let pairing = {
        use crate::harness::Declared;
        use crate::harness::pairing::{PairingQuery, ServingRoute, classify};
        use crate::routing::AssignedModel;

        // The same fallback `main.rs::session_pairing` builds for a `Native`
        // profile: `pairing_queries` never lists it, so a configured-pairing
        // lookup here would always miss anyway — see that function's own doc
        // comment for why the implied profile needs no lookup at all.
        let query = PairingQuery {
            harness: launch_profile.harness,
            model: AssignedModel::HarnessDefault,
            route: ServingRoute {
                provider: None,
                gateway: None,
                protocol: None,
            },
            tool_calls: Declared::Unverified,
            provider_protocols: Vec::new(),
        };
        classify(&query, &effective.pairing_overrides())
    };
    let response_profile =
        effective.response_profile(&config::response::ResponseRequest::default());
    for problem in response_profile.problems() {
        // `eprintln!` would corrupt the alternate-screen viewport this
        // process owns while the shell is running — this is the diagnostic
        // channel every other shell warning already uses.
        tracing::warn!(problem, "could not read part of the response profile");
    }
    let response_application =
        crate::harness::response::apply(selection.adapter(), response_profile.resolved());

    // The presentation is recorded before the process exists and is then the
    // single source of truth for it: `live.start` below is handed
    // `record.presentation`, so a session's stored presentation and its
    // running one cannot disagree.
    let record = store.create(
        NewSession::embedded(selection.id().slug())
            .with_presentation(presentation)
            .with_native_session_id(native.clone())
            .with_launch_profile(Some(launch_profile.name.clone()))
            .with_backend_resource(Some(launch_profile.backend.slug()))
            .with_model(Some(pairing.model().clone()))
            .with_pairing_class(Some(session::session_pairing_class(pairing.class())))
            .with_protocol(Some(session::session_protocol(pairing.route().protocol)))
            .with_response_profile(Some(response_profile.resolved().profile()))
            .with_response_mechanism(Some(session::session_response_mechanism(
                response_application.mechanism(),
            ))),
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
    // `install_session_document` rather than `install_hooks`: the latter is
    // the narrower mechanism that exists precisely because this call site
    // used to resolve no response profile — see its own doc comment, which
    // this is the fix for. Hooks and the response profile now share one
    // document exactly as `main.rs::launch_session`'s already does.
    let document_args = std::env::current_exe()
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
            selection.install_session_document(
                &report,
                project_hooks_consent,
                &response_application,
            )
        });
    match document_args {
        Ok(document) => {
            args.splice(0..0, document.args);
        }
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

/// Reopen a recorded session, embedded in this shell — Phase 11 line 688,
/// "allow the user to resume any compatible stopped session from the
/// overview".
///
/// This is `main.rs::resume_session`'s embedded counterpart, cut down to
/// what the overview's key needs and mirroring `start_session` above rather
/// than that CLI path: the shell keeps every live harness inside its own
/// [`SessionRuntime`] rather than handing the terminal away with
/// `session::attach`, so this calls `live.start` exactly as a fresh session
/// does, with the resumed session's own recorded id.
///
/// # What this does not do
///
/// It does not re-resolve the session's launch profile overlay the way
/// `main.rs::resume_session` does — that machinery
/// (`resolve_resume_overlay`) is private to `main.rs`, which this package may
/// not edit (see the packet's `FORBIDDEN FILES`), so a resumed session here
/// runs on a plain resume invocation with no regenerated provider
/// configuration. A session resumed from the overview that needs its
/// original overlay reapplied is a gap for the next package, not a silent
/// approximation — recorded in this phase's evidence rather than hidden.
///
/// It also cannot record [`LifecycleEvent::SessionResumed`]:
/// [`SessionRuntime::start`] always publishes `SessionStarted`, and that is
/// `session/runtime.rs`, also outside this package's `FORBIDDEN FILES`. The
/// activity feed will therefore say "session started" for a resume, which is
/// the same finding one layer down — the event model already draws this
/// exact distinction (see `describe_event`'s doc comment), and the seam that
/// would let `SessionRuntime::start` publish the right one is not this
/// package's to open.
fn resume_session(
    app_runtime: &Runtime,
    live: &mut SessionRuntime,
    sessions: &ProjectSessions,
    id: &SessionId,
    size: TerminalSize,
) -> anyhow::Result<()> {
    let store = sessions.store();
    // The store's own gate, not a second copy of it: `open_for_resume`
    // refuses a session that belongs to another project, is still running,
    // or was never given a native identifier to resume to — the same check
    // `main.rs::resume_session` relies on, so an overview resume and a CLI
    // one refuse exactly the same sessions for exactly the same reasons.
    let resumable = store.open_for_resume(id)?;
    let record = store
        .get(&resumable.id)?
        .expect("open_for_resume already proved this session's record exists");

    let user = UserConfig::load(app_runtime.paths())?;
    let project_config = config::load_project_config(app_runtime.project())?;
    let effective = EffectiveConfig::new(&user, project_config.as_ref());
    // The record's own harness, not whatever is configured now — resuming a
    // Codex conversation in Claude Code would be nonsense, and this is the
    // same rule `main.rs::resume_session`'s doc comment states for the CLI
    // path.
    let selection = session::select::select(Some(resumable.harness.as_str()), effective)?;

    let Some(mut args) = selection.resume_args(&resumable.native_session_id, Vec::<String>::new())
    else {
        anyhow::bail!(
            "{} has no resume mechanism Glasshouse has verified, so this session cannot be \
             reopened",
            selection.id().display_name()
        );
    };

    let response_request = config::response::ResponseRequest {
        role: Some(config::response::ResponseRequest::role_for(record.role)),
        ..config::response::ResponseRequest::default()
    };
    let response_profile = effective.response_profile(&response_request);
    for problem in response_profile.problems() {
        tracing::warn!(problem, "could not read part of the response profile");
    }
    let response_application =
        crate::harness::response::apply(selection.adapter(), response_profile.resolved());

    let project_hooks_consent = effective.project_hooks(selection.id()).value;
    let document_args = std::env::current_exe()
        .map_err(anyhow::Error::from)
        .and_then(|program| {
            let report = crate::harness::HookCommand::new(
                program,
                resumable.id.as_str(),
                app_runtime.session_dir(resumable.id.as_str()),
                app_runtime.project().root(),
                app_runtime.paths().data_dir(),
                app_runtime.paths().config_dir(),
            );
            selection.install_session_document(
                &report,
                project_hooks_consent,
                &response_application,
            )
        });
    match document_args {
        Ok(document) => {
            args.splice(0..0, document.args);
        }
        Err(err) => {
            tracing::warn!(session = %resumable.id, error = %err, "could not install lifecycle hooks");
        }
    }

    // `open_for_resume` already proved this session's process exited, but its
    // `LiveSession` is still sitting in `live` — `SessionRuntime` never drops
    // one on its own, and `get`/`focus`/`interrupt`/`send_text` all resolve
    // the *first* entry with a given id. Starting a fresh process under this
    // same id without forgetting the dead one first would leave every one of
    // those calls silently talking to the exited process's frozen screen
    // instead of the one just started. `close` is best-effort: `NotLive`
    // here would only mean the entry was already gone, which is fine.
    let _ = live.close(&resumable.id);

    let launch = HarnessLaunch::new(selection.into_executable(), app_runtime.project())
        .args(args)
        .size(size);
    if let Err(err) = live.start(resumable.id.clone(), record.presentation, &launch) {
        if let Err(store_err) = store.set_lifecycle(&resumable.id, SessionLifecycle::Failed) {
            tracing::warn!(
                session = %resumable.id,
                error = %store_err,
                "could not record a failed session resume"
            );
        }
        return Err(err);
    }

    if let Err(err) = store.set_lifecycle(&resumable.id, SessionLifecycle::Running) {
        tracing::warn!(session = %resumable.id, %err, "could not record a session resume");
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
    RoutingRow,
    MemoryRow,
);

/// Current binding memory (decisions and constraints) and unresolved todos,
/// summarized into display lines for [`state::ShellState::open_project_overview`].
///
/// Reading `crate::memory` is file I/O this module deliberately does not
/// hold in `shell/state.rs`, exactly like [`build_settings`] and
/// `EffectiveConfig`. `binding` and `snapshot` are otherwise only exercised
/// by `tests/memory_authority.rs` and `tests/memory_snapshot.rs` — this is
/// their first production caller.
struct ProjectOverviewMemory {
    decisions: Vec<String>,
    todos: Vec<String>,
    todos_omitted: usize,
}

/// How many current binding memories (decisions and constraints) the
/// overview shows. Generous, because this is a summary a person reads once
/// in a while rather than a paginated list — see [`ProjectOverviewMemory`].
const PROJECT_OVERVIEW_DECISION_LIMIT: usize = 20;
const PROJECT_OVERVIEW_TODO_LIMIT: usize = 20;
const PROJECT_OVERVIEW_BODY_CHARS: usize = 96;

fn build_project_overview_memory(runtime: &Runtime) -> anyhow::Result<ProjectOverviewMemory> {
    use crate::memory::MemoryKind;
    use crate::memory::ProjectMemory;
    use crate::memory::snapshot::{SnapshotBudget, snapshot};

    let memory = ProjectMemory::open(runtime)?;
    let store = memory.store();

    let decisions = store
        .binding(PROJECT_OVERVIEW_DECISION_LIMIT)?
        .into_iter()
        .map(|record| summarize_memory_line(record.kind, record.subject.as_deref(), &record.body))
        .collect();

    let budget = SnapshotBudget::new(PROJECT_OVERVIEW_TODO_LIMIT, PROJECT_OVERVIEW_BODY_CHARS);
    let snap = snapshot(&store, &budget)?;
    let (todos, todos_omitted) = match snap.section(MemoryKind::Todo) {
        Some(section) => (
            section
                .entries
                .iter()
                .map(|entry| {
                    summarize_memory_line(MemoryKind::Todo, entry.subject.as_deref(), &entry.body)
                })
                .collect(),
            section.omitted,
        ),
        None => (Vec::new(), 0),
    };

    Ok(ProjectOverviewMemory {
        decisions,
        todos,
        todos_omitted,
    })
}

/// Map lines 1657, 1658, 1659, 1660 and 1663: what Glasshouse has observed
/// about this project's own configured resources, one line per resource —
/// the project overview's condensed sibling of `glasshouse resources`'s full
/// report, read the same way `main.rs::resources_report` reads it: the same
/// [`crate::provider::resources::observed_capacity`] over the same on-disk
/// [`crate::provider::telemetry::GatewayQuotaCache`], no network call.
///
/// Scoped to [`EffectiveConfig::provider_names`] rather than the full
/// [`crate::provider::registry::registry`] catalog: that accessor's own doc
/// comment says "a provider only exists here because a user or project
/// explicitly configured one" — exactly the behavioral contract's "configured
/// resources", and the same set `main.rs::disposable_candidates` already
/// scores a real routing decision over, which is what makes
/// [`resource_capacity_line`]'s reserve note more than a hypothetical: it
/// mirrors the identical `with_resource_reserve` fold
/// `main.rs::disposable_candidate_capacity` builds for that decision.
///
/// Line 1661 — the currently selected routing model and its recent latency —
/// is deliberately absent: Phase 34B has no routing-model role in this
/// build, so there is nothing to name.
///
/// # Cannot fail visibly
///
/// A configuration Glasshouse cannot read becomes one honest line rather
/// than an empty section blocking the rest of the overlay. Reading
/// `crate::config` and the gateway-quota cache is file I/O this module
/// deliberately does not hold in `shell/state.rs` — the same split
/// [`build_project_overview_memory`] keeps.
fn build_project_overview_capacity(runtime: &Runtime) -> Vec<String> {
    use crate::provider::registry::ResourceKind;
    use crate::provider::resources::{GatheredTelemetry, observed_capacity};
    use crate::provider::telemetry::GatewayQuotaCache;

    let user = match UserConfig::load(runtime.paths()) {
        Ok(user) => user,
        Err(err) => return vec![format!("  resource configuration unavailable: {err:#}")],
    };
    let project_config = match config::load_project_config(runtime.project()) {
        Ok(project_config) => project_config,
        Err(err) => return vec![format!("  resource configuration unavailable: {err:#}")],
    };
    let effective = EffectiveConfig::new(&user, project_config.as_ref());

    let providers = effective.provider_names();
    if providers.is_empty() {
        return Vec::new();
    }

    let now_unix = crate::provider::cache::now_unix_seconds();
    let telemetry =
        GatheredTelemetry::new().gather_gateway_quota(&GatewayQuotaCache::new(runtime.paths()));
    let base_thresholds = effective.capacity_band_thresholds().value;

    providers
        .into_iter()
        .map(|provider| {
            let kind = ResourceKind::from_direct_provider(&provider);
            let state = observed_capacity(&kind, &effective, &telemetry, now_unix);
            let reserve_percent = effective.reserve_percent(&provider).value.get();
            let thresholds = base_thresholds.with_resource_reserve(reserve_percent);
            resource_capacity_line(
                &kind.label(),
                &state,
                &thresholds,
                reserve_percent,
                now_unix,
            )
        })
        .collect()
}

/// One line describing what Glasshouse currently believes about `label`'s
/// capacity — the pure formatting half of
/// [`build_project_overview_capacity`], split out so every case (measured,
/// estimated, manual, unknown, reset present or absent, reserve engaged or
/// not) is testable directly against a hand-built
/// [`crate::provider::quota::CapacityState`] rather than only through real
/// configuration files and an on-disk cache.
///
/// `thresholds` must already carry this resource's own protected reserve —
/// see `crate::provider::resources`'s private `capacity_band_thresholds_for`,
/// which this mirrors rather than calls: that function lives in
/// `provider/resources.rs`, outside this package's partition this round.
///
/// # Line 1659, precisely
///
/// [`crate::provider::quota::TelemetryClass::Authoritative`] and
/// [`crate::provider::quota::TelemetryClass::Observed`] both collapse to
/// `"measured"` here — line 1659 names four words, not the five
/// [`crate::provider::quota`] itself tracks, and both are real readings
/// nobody inferred. [`crate::provider::quota::TelemetryClass::Estimated`]
/// and [`crate::provider::quota::TelemetryClass::Manual`] keep their own
/// words, and no reading at all is `"unknown"` — never a number.
///
/// # Line 1663, precisely
///
/// A reserve note is shown only at or below
/// [`crate::provider::quota::CapacityBand::Reserve`] — the exact boundary
/// `crate::provider::quota::evaluate_reserve_spend` itself
/// gates on (`inputs.band > CapacityBand::Reserve` trivially allows every
/// request; at or below it, the reserve policy actually runs and can deny
/// one). Above that boundary the reserve has influenced nothing this round,
/// so nothing about it is shown.
fn resource_capacity_line(
    label: &str,
    state: &crate::provider::quota::CapacityState,
    thresholds: &crate::provider::quota::CapacityBandThresholds,
    reserve_percent: u8,
    now_unix: i64,
) -> String {
    use crate::provider::quota::{CapacityBand, TelemetryClass};

    let reset_note = match state.seconds_until_reset(now_unix) {
        Some(seconds) => format!(", reset in {seconds}s"),
        None => String::new(),
    };

    let Some(score) = state.remaining_capacity_score() else {
        // No pool normalized to a percentage, but the resource's own plan or
        // rate ceilings may still carry a class worth naming — a manually
        // configured plan, say. `state.telemetry_class()` answers that;
        // `None` here is the genuine "unknown" case line 1657/1658 name.
        let class_word = match state.telemetry_class() {
            None => "unknown",
            Some(TelemetryClass::Authoritative | TelemetryClass::Observed) => "measured",
            Some(TelemetryClass::Estimated) => "estimated",
            Some(TelemetryClass::Manual) => "manual",
        };
        return format!("  {label}  capacity {class_word}{reset_note}");
    };

    let band = score.band(thresholds);
    // `RemainingCapacityScore::percent` is only ever `Exact` (this displayed
    // value came from the provider itself) or `Estimated` (anything weaker
    // fed into it) — never `Manual` or absent, since a score exists here.
    // That is line 1659's "measured" vs "estimated" distinction exactly, and
    // deliberately not `state.telemetry_class()`, which answers the whole
    // resource's *best* source across every pool and would report
    // "measured" even when the one number actually shown is an estimate.
    let (class_word, digits) = match score.percent().exact() {
        Some(percent) => ("measured", percent),
        None => (
            "estimated",
            score
                .percent()
                .estimated()
                .map(|(percent, _, _)| percent)
                .expect("a Percentage is always Exact or Estimated"),
        ),
    };

    let reserve_note = if band <= CapacityBand::Reserve {
        format!("; protected reserve {reserve_percent}% is limiting routing here")
    } else {
        String::new()
    };

    format!("  {label}  {band} {digits}% [{class_word}]{reset_note}{reserve_note}")
}

/// One display line: the memory's kind, and its subject if it has one or its
/// body cut to [`PROJECT_OVERVIEW_BODY_CHARS`] otherwise.
///
/// Prefers the subject over the body when both exist because the subject is
/// already the producer's own summary (`MemoryRecord::subject`'s doc
/// comment) — cutting the body instead would show less of a memory that
/// already told us how to describe it concisely.
fn summarize_memory_line(
    kind: crate::memory::MemoryKind,
    subject: Option<&str>,
    body: &str,
) -> String {
    let text = subject.unwrap_or(body);
    let char_count = text.chars().count();
    if char_count <= PROJECT_OVERVIEW_BODY_CHARS {
        format!("{kind}: {text}")
    } else {
        let cut: String = text.chars().take(PROJECT_OVERVIEW_BODY_CHARS).collect();
        format!("{kind}: {cut}…")
    }
}

/// How many entries [`build_project_knowledge_memory`] shows per section —
/// the same generous default [`PROJECT_OVERVIEW_DECISION_LIMIT`] uses, for
/// the same reason: a summary read occasionally, not a paginated list.
const PROJECT_KNOWLEDGE_SECTION_LIMIT: usize = 20;
/// Ceiling for one [`crate::memory::MemoryStore::with_status`] fetch before
/// [`knowledge_section`] applies its own per-kind display limit. Generous
/// enough that no real project's per-status memory count approaches it —
/// this bounds one query, not the section shown on screen.
const PROJECT_KNOWLEDGE_FETCH_LIMIT: usize = 10_000;

/// Every kind of durable project knowledge, grouped and formatted for
/// [`state::ShellState::open_project_knowledge`] — Phase 25, map lines
/// 1098-1107.
struct ProjectKnowledgeMemory {
    decisions: KnowledgeSection,
    constraints: KnowledgeSection,
    features: KnowledgeSection,
    failed_attempts: KnowledgeSection,
    todos: KnowledgeSection,
}

/// Read every section the project-knowledge view shows, from the current
/// project's memory database.
///
/// Reading `crate::memory` is file I/O this module deliberately does not
/// hold in `shell/state.rs` — the same split [`build_project_overview_memory`]
/// keeps. `MemoryStore` has no single "everything, by kind" query, so each
/// section is built by [`knowledge_section`] against the public
/// `with_status`/kind filter, exactly the surface
/// [`build_project_overview_memory`] already uses.
fn build_project_knowledge_memory(runtime: &Runtime) -> anyhow::Result<ProjectKnowledgeMemory> {
    use crate::memory::{MemoryKind, ProjectMemory};

    let memory = ProjectMemory::open(runtime)?;
    let store = memory.store();

    // Map lines 1100-1102: active decisions, known constraints, and
    // implemented-or-planned features are all *current* project knowledge —
    // `MemoryStatus::is_current` is the one test for that, so a superseded,
    // rejected or invalidated record of any of these three kinds never
    // reaches its section (acceptance test 3).
    let decisions = knowledge_section(&store, MemoryKind::Decision, |status| status.is_current())?;
    let constraints =
        knowledge_section(&store, MemoryKind::Constraint, |status| status.is_current())?;
    let features = knowledge_section(&store, MemoryKind::Feature, |status| status.is_current())?;

    // Map line 1104: *unresolved*, not merely *current* —
    // `MemoryStatus::is_open_work` also keeps a todo under review or in
    // conflict, which `is_current` alone would drop, and excludes a resolved
    // one exactly like `is_current` does.
    let todos = knowledge_section(&store, MemoryKind::Todo, |status| status.is_open_work())?;

    // Map line 1103: failed approaches get a dedicated *historical* section,
    // deliberately unfiltered by status — the record of what was tried and
    // did not work is the point, including one a newer memory has since
    // superseded (map line 1106 is how that supersession is named).
    let failed_attempts = knowledge_section(&store, MemoryKind::FailedAttempt, |_| true)?;

    Ok(ProjectKnowledgeMemory {
        decisions,
        constraints,
        features,
        failed_attempts,
        todos,
    })
}

/// Every memory of `kind` whose status satisfies `include`, most recently
/// updated first, formatted and capped at
/// [`PROJECT_KNOWLEDGE_SECTION_LIMIT`].
///
/// `MemoryStore::binding` filters by authority, not kind, and
/// `memory::snapshot::snapshot` only ever returns
/// [`crate::memory::MemoryStatus::Active`] records — neither fits a section
/// that needs one specific kind across a caller-chosen set of statuses. So
/// this walks [`crate::memory::MemoryStatus::ALL`] through the public
/// [`crate::memory::MemoryStore::with_status`] and keeps what matches both
/// `kind` and `include`: the same public surface, used the way a caller
/// outside `memory/**` is meant to combine it.
fn knowledge_section(
    store: &crate::memory::MemoryStore<'_>,
    kind: crate::memory::MemoryKind,
    include: impl Fn(crate::memory::MemoryStatus) -> bool,
) -> anyhow::Result<KnowledgeSection> {
    let mut matched: Vec<crate::memory::MemoryRecord> = crate::memory::MemoryStatus::ALL
        .iter()
        .copied()
        .filter(|status| include(*status))
        .map(|status| store.with_status(status, PROJECT_KNOWLEDGE_FETCH_LIMIT))
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .flatten()
        .filter(|record| record.kind == kind)
        .collect();
    matched.sort_by(|a, b| {
        b.updated_at
            .cmp(&a.updated_at)
            .then_with(|| a.id.as_str().cmp(b.id.as_str()))
    });

    let omitted = matched
        .len()
        .saturating_sub(PROJECT_KNOWLEDGE_SECTION_LIMIT);
    let shown: Vec<crate::memory::MemoryRecord> = matched
        .into_iter()
        .take(PROJECT_KNOWLEDGE_SECTION_LIMIT)
        .collect();
    let lines = shown.iter().map(knowledge_line).collect();
    let details = shown.iter().map(knowledge_detail).collect();

    Ok(KnowledgeSection {
        lines,
        details,
        omitted,
    })
}

/// One display line: [`summarize_memory_line`]'s kind-and-text line, with a
/// trailing supersession note when [`crate::memory::MemoryRecord::superseded_by`]
/// names a successor.
///
/// Map line 1106: said in words *when a supersession relationship exists*,
/// and silent otherwise — never a placeholder like "none" or an empty
/// column, which is why this is one conditional push rather than always
/// appending a (possibly empty) field.
fn knowledge_line(record: &crate::memory::MemoryRecord) -> String {
    let mut line = summarize_memory_line(record.kind, record.subject.as_deref(), &record.body);
    if let Some(successor) = &record.superseded_by {
        line.push_str(&format!(" — superseded by {successor}"));
    }
    line
}

/// Map line 1105's drill-down data for one memory: its rationale, source
/// session, source commit and lifecycle state, straight off
/// [`crate::memory::MemoryRecord`]'s own fields.
///
/// `rationale` comes from `record.provenance.rationale` rather than the
/// whole [`crate::memory::DecisionProvenance`] — the line names only the
/// rationale, not the five kinds of recorded assumption sitting beside it,
/// and showing those here would be answering a question the box does not
/// ask. `lifecycle` uses [`crate::memory::MemoryStatus`]'s own `Display`
/// (`"active"`, `"superseded"`, and so on) rather than inventing a second
/// vocabulary for the same fact.
fn knowledge_detail(record: &crate::memory::MemoryRecord) -> MemoryDetail {
    MemoryDetail {
        rationale: record.provenance.rationale.clone(),
        source_session: record.source_session_id.clone(),
        source_commit: record.source_commit.clone(),
        lifecycle: record.status.to_string(),
    }
}

/// One display line for the project-memory view: [`knowledge_line`]'s
/// kind-and-text line, prefixed with the memory's lifecycle status.
///
/// [`build_project_knowledge_memory`]'s five sections each already imply a
/// single status by construction — the active-decisions section is
/// `is_current`, the todos section is `is_open_work`, and so on — so
/// `knowledge_line` alone is enough there: which section an entry is in
/// already says its status. This view has exactly one list spanning every
/// [`crate::memory::MemoryStatus`] at once, so the status has to be said on
/// the line rather than implied by where the entry sits — map line 234's
/// "at least its kind and its status".
fn memory_view_line(record: &crate::memory::MemoryRecord) -> String {
    format!("[{}] {}", record.status, knowledge_line(record))
}

/// Every memory record in this project — every
/// [`crate::memory::MemoryKind`], at every [`crate::memory::MemoryStatus`],
/// most recently updated first — for [`state::Action::OpenProjectMemory`].
/// Map line 234.
///
/// [`build_project_knowledge_memory`]'s unfiltered sibling: that function
/// calls [`knowledge_section`] once per curated kind, each restricted to a
/// status predicate that makes it "current knowledge". This view has no
/// predicate and no per-kind split — every kind, including
/// [`crate::memory::MemoryKind::Finding`], which `build_project_knowledge_memory`
/// never queries for at all, and every status, including one
/// `is_current`/`is_open_work` would drop. Reading `crate::memory` is file
/// I/O this module deliberately does not hold in `shell/state.rs` — the same
/// split [`build_project_knowledge_memory`] keeps.
fn build_project_memory_view(runtime: &Runtime) -> anyhow::Result<KnowledgeSection> {
    use crate::memory::{MemoryKind, MemoryStatus, ProjectMemory};

    let memory = ProjectMemory::open(runtime)?;
    let store = memory.store();

    let by_status: Vec<Vec<crate::memory::MemoryRecord>> = MemoryStatus::ALL
        .iter()
        .copied()
        .map(|status| store.with_status(status, PROJECT_KNOWLEDGE_FETCH_LIMIT))
        .collect::<Result<Vec<_>, _>>()?;

    // Every kind this project's memory has — the whole point of this view
    // next to `ProjectKnowledge`'s curated sections. `MemoryStatus::ALL`
    // above already returns every kind at each status; this keeps the
    // inclusion explicit rather than resting on the absence of a filter.
    let kinds: &[MemoryKind] = MemoryKind::ALL;
    let mut matched: Vec<crate::memory::MemoryRecord> = by_status
        .into_iter()
        .flatten()
        .filter(|record| kinds.contains(&record.kind))
        .collect();
    matched.sort_by(|a, b| {
        b.updated_at
            .cmp(&a.updated_at)
            .then_with(|| a.id.as_str().cmp(b.id.as_str()))
    });

    let omitted = matched
        .len()
        .saturating_sub(PROJECT_KNOWLEDGE_SECTION_LIMIT);
    let shown: Vec<crate::memory::MemoryRecord> = matched
        .into_iter()
        .take(PROJECT_KNOWLEDGE_SECTION_LIMIT)
        .collect();
    let lines = shown.iter().map(memory_view_line).collect();
    let details = shown.iter().map(knowledge_detail).collect();

    Ok(KnowledgeSection {
        lines,
        details,
        omitted,
    })
}

/// How many identities [`build_route_evidence_table`] shows — the same
/// generous, read-occasionally default [`PROJECT_KNOWLEDGE_SECTION_LIMIT`]
/// uses.
const ROUTE_EVIDENCE_ROW_LIMIT: usize = 20;

/// How far back [`build_route_evidence_table`] looks for observed
/// identities. A week — long enough that a project worked on across a normal
/// week still sees its own routing activity, short enough that an identity
/// nobody has exercised in a month quietly ages out of the table rather than
/// accumulating in it forever. Provisional, like the constants
/// `crate::routing::evidence`'s own module names for the same reason.
const ROUTE_EVIDENCE_WINDOW_SECONDS: i64 = 7 * 24 * 60 * 60;

/// Read the routing evidence ledger's own distinct identities — Phase 47
/// lines 1762 and 1764, closed after batch 42 found the ledger could not
/// enumerate identities at all (practice §71).
/// [`crate::routing::evidence::EvidenceLedger::observed_identities`] is this
/// package's one additive method and the whole of what makes this function
/// possible; see its own doc comment for why `recent`/`summarize` alone
/// could not answer this. Reading the ledger is file I/O this module
/// deliberately does not hold in `shell/state.rs` — the same split
/// [`build_project_overview_memory`] keeps.
fn build_route_evidence_table(runtime: &Runtime) -> anyhow::Result<Vec<RouteEvidenceRow>> {
    use crate::routing::evidence::EvidenceLedger;

    let ledger = EvidenceLedger::open(runtime)?;
    let now = crate::provider::cache::now_unix_seconds();
    let identities =
        ledger.observed_identities(now, ROUTE_EVIDENCE_WINDOW_SECONDS, ROUTE_EVIDENCE_ROW_LIMIT)?;
    Ok(identities
        .into_iter()
        .map(|identity| {
            let (window_start_unix, window_end_unix) = identity.window();
            let sample_count = identity.sample_count();
            RouteEvidenceRow {
                provider: identity.provider,
                model: identity.model,
                route: identity.route,
                context_state: identity.context_state.as_str().to_owned(),
                sample_count,
                window_start_unix,
                window_end_unix,
            }
        })
        .collect())
}

/// Read what a local gateway has observed about each free resource — Phase 47
/// map line 1765, *"show route health, immediate availability, cadence, quota
/// reset, and failure-domain evidence as separate concepts"*.
///
/// # Why this can be read from the interactive shell at all
///
/// The shell process has no gateway and no router in it: [`run`] takes only a
/// [`Runtime`], and the gateway is started in `main.rs`'s `launch_session`,
/// which is a different invocation. So none of this can come from live router
/// state. It does not have to: `crate::gateway::mod`'s accept loop already
/// writes both of these caches to disk on every forwarded exchange
/// (`GatewayQuotaCache::store` and `GatewayHealthCache::store`), for exactly
/// this reason — `glasshouse resources` is a separate process too, and reads
/// them back the same way. This is that same seam, used by a second reader.
///
/// # Never fails, and that is the caches' own contract
///
/// Both loads are documented as returning no error ever: absent, unreadable,
/// truncated, or written by another format version all mean *nothing was
/// observed*, which is a complete and honest answer. There is consequently no
/// note for [`ShellState::open_route_health`] to carry, unlike
/// [`build_route_evidence_table`], whose ledger really can fail to open.
///
/// # Scope: this is installation-wide, and the view says so
///
/// Both caches live under [`crate::paths::RuntimePaths::data_dir`], keyed by
/// provider — **not** under `project_state_dir`. They describe providers, and
/// providers are configured at the user level, so a reading written while a
/// gateway served one project is visible to every project's shell. That is
/// the same scope `glasshouse resources` already prints, and it is labelled
/// in the view rather than left for a reader to assume. Nothing project-scoped
/// is read here at all: no project database is opened by this function.
fn build_route_health_table(runtime: &Runtime) -> Vec<RouteHealthRow> {
    use crate::provider::telemetry::{GatewayHealthCache, GatewayQuotaCache};
    use crate::routing::domain::FailureDomain;

    let now_unix = crate::provider::cache::now_unix_seconds();
    let quota: std::collections::HashMap<
        String,
        (crate::provider::telemetry::RateLimitHeaders, i64),
    > = GatewayQuotaCache::new(runtime.paths())
        .load_all()
        .into_iter()
        .map(|(provider, headers, observed_at)| (provider, (headers, observed_at)))
        .collect();

    let mut rows = Vec::new();
    for (provider, readings) in GatewayHealthCache::new(runtime.paths()).load_all() {
        // Concept 5's only honest signal. `FailureDomain::between` compares
        // two `Backend`s and neither cache stores one, so what is available
        // here is the identity that comparison would use — the provider name
        // — applied to the resources actually observed under it. The
        // vocabulary comes from the enum itself so this can never drift into
        // a second spelling, and `Independent` is unreachable by
        // construction: there is no branch below that produces it.
        let peers = readings.len().saturating_sub(1);
        let domain = if peers > 0 {
            FailureDomain::Shared
        } else {
            FailureDomain::Unknown
        };
        let stated = quota.get(&provider);
        for reading in readings {
            rows.push(RouteHealthRow {
                provider: provider.clone(),
                credential_label: reading.credential_label.clone(),
                model: reading.model.clone(),
                consecutive_failures: reading.consecutive_failures,
                credential_rejected: reading.credential_rejected,
                // The producer's own decision, asked rather than re-derived
                // from the two fields above.
                available_now: reading.is_available(now_unix),
                cooling_down_until_unix: reading.cooling_down_until_unix,
                stated_limit: stated.and_then(|(headers, _)| headers.limit()),
                stated_window_seconds: stated.and_then(|(headers, _)| headers.window_seconds()),
                quota_resets_at_unix: stated
                    .and_then(|(headers, observed_at)| headers.resets_at_unix(*observed_at)),
                failure_domain: domain.as_str().to_owned(),
                failure_domain_peers: peers,
            });
        }
    }
    rows
}

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

    let routing_model = effective.routing_model();
    let max_latency = effective.max_router_latency();
    let max_cost = effective.max_router_cost();
    let prefer_free = effective.prefer_free_routing();
    let premium_reserve = effective.premium_reserve();
    // Phase 9I line 536: the user's own order, disabled list and pin over the
    // free pool, layered exactly like every routing preference beside them.
    let free_order = effective.free_resource_order();
    let free_disabled = effective.free_resource_disabled();
    let free_pin = effective.free_resource_pin();
    let configured_providers = providers.iter().map(|row| row.name.clone()).collect();
    let routing = RoutingRow::new(
        routing_model,
        max_latency,
        max_cost,
        prefer_free,
        premium_reserve,
        configured_providers,
    )
    .with_free_preferences(free_order, free_disabled, free_pin);
    let memory = MemoryRow::new(effective.memory_extraction_enabled());

    Ok((
        harnesses,
        integrations,
        providers,
        profiles,
        routing,
        memory,
    ))
}

/// Re-read Settings' rows after a successful save and hand them to
/// [`state::ShellState::refresh_settings`], which is also what clears the
/// edits that just landed on disk. A failure here is not the save failing —
/// the write already succeeded — so it only costs a stale display, reported
/// the same non-fatal way as everything else in this module.
fn refresh_settings_after_save(runtime: &Runtime, state: &mut ShellState) {
    match build_settings(runtime) {
        Ok((harnesses, integrations, providers, profiles, routing, memory)) => state
            .refresh_settings_with_routing(
                harnesses,
                integrations,
                providers,
                profiles,
                routing,
                memory,
            ),
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

fn apply_routing_edit(table: &mut config::RoutingConfig, edit: &RoutingSettingsEdit) {
    if let Some(model) = &edit.model {
        table.set_model(Some(model.clone()));
    }
    if let Some(value) = edit.max_latency {
        table.set_max_router_latency(Some(value));
    }
    if let Some(value) = edit.max_cost {
        table.set_max_marginal_cost(Some(value));
    }
    if let Some(value) = edit.prefer_free {
        table.set_prefer_free(Some(value));
    }
    if let Some(value) = edit.premium_reserve {
        table.set_premium_reserve(Some(value));
    }
    // Phase 9I line 536. The pin is a double `Option` because, unlike every
    // preference above it, "no pin" is a state a user can choose explicitly
    // rather than merely not having touched.
    if let Some(value) = &edit.free_order {
        table.set_free_resource_order(Some(value.clone()));
    }
    if let Some(value) = &edit.free_disabled {
        table.set_free_resource_disabled(Some(value.clone()));
    }
    if let Some(value) = &edit.free_pin {
        table.set_free_resource_pin(value.clone());
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
    save_user_settings_with_routing(
        runtime,
        harness_edits,
        provider_edits,
        profile_edits,
        None,
        None,
    )
}

/// User-level save including independently staged Routing fields and the
/// Memory field.
pub fn save_user_settings_with_routing(
    runtime: &Runtime,
    harness_edits: &[SettingsEdit],
    provider_edits: &[ProviderSettingsEdit],
    profile_edits: &[ProfileSettingsEdit],
    routing_edit: Option<&RoutingSettingsEdit>,
    memory_edit: Option<&MemorySettingsEdit>,
) -> anyhow::Result<()> {
    let mut config = UserConfig::load(runtime.paths())?;
    apply_settings_edits(config.integrations_mut(), harness_edits);
    apply_provider_edits(config.providers_mut(), provider_edits);
    apply_profile_edits(config.profiles_mut(), profile_edits);
    if let Some(edit) = routing_edit {
        apply_routing_edit(config.routing_mut(), edit);
    }
    if let Some(value) = memory_edit.and_then(|edit| edit.memory_extraction) {
        config.set_memory_extraction(Some(value));
    }
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
    save_project_settings_with_routing(
        runtime,
        harness_edits,
        provider_edits,
        profile_edits,
        None,
        None,
    )
}

/// Project-level counterpart to [`save_user_settings_with_routing`]. The
/// caller reaches this only after the same explicit `W` confirmation as all
/// other project Settings edits.
pub fn save_project_settings_with_routing(
    runtime: &Runtime,
    harness_edits: &[SettingsEdit],
    provider_edits: &[ProviderSettingsEdit],
    profile_edits: &[ProfileSettingsEdit],
    routing_edit: Option<&RoutingSettingsEdit>,
    memory_edit: Option<&MemorySettingsEdit>,
) -> anyhow::Result<std::path::PathBuf> {
    let mut project_config = config::load_project_config(runtime.project())?.unwrap_or_default();
    apply_settings_edits(project_config.integrations_mut(), harness_edits);
    apply_provider_edits(project_config.providers_mut(), provider_edits);
    apply_profile_edits(project_config.profiles_mut(), profile_edits);
    if let Some(edit) = routing_edit {
        apply_routing_edit(project_config.routing_mut(), edit);
    }
    if let Some(value) = memory_edit.and_then(|edit| edit.memory_extraction) {
        project_config.set_memory_extraction(Some(value));
    }
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
        let (harnesses, integrations, providers, profiles, _, _) =
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

        let (harnesses, integrations, providers, profiles, _, _) =
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

        let (_, _, providers, _, _, _) = build_settings(&runtime).expect("settings open");
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

        let (harnesses, integrations, providers, profiles, _, _) =
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
        let (_, _, reopened, _, _, _) = build_settings(&runtime).expect("settings reopen");
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

        let (_, _, providers, profiles, _, _) = build_settings(&runtime).unwrap();
        assert!(!providers[0].config.enabled());
        assert!(!profiles[0].config.enabled());
    }
}

/// Phase 41: the project overview reads real binding memory and real
/// unresolved todos through [`build_project_overview_memory`] — the
/// production function `Action::OpenProjectOverview`'s handler calls, not a
/// helper that re-implements the query. `MemoryStore::binding` and
/// `memory::snapshot::snapshot` had no other production caller before this
/// (only `tests/memory_authority.rs` and `tests/memory_snapshot.rs`
/// exercised them), so the overview is what makes them reachable at all.
#[cfg(test)]
mod project_overview_tests {
    use super::*;
    use crate::memory::{MemoryAuthority, MemoryKind, NewMemory, ProjectMemory};

    /// Bootstrap a `Runtime` over fresh, isolated data/config/workspace
    /// directories, matching `settings_persistence_tests::bootstrapped_runtime`.
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

    /// A project with no memory at all gets empty, honest sections — not an
    /// error. `ProjectMemory::open` creates the database on first use, so
    /// "no memory yet" and "could not read memory" must not collapse into
    /// the same outcome.
    #[test]
    fn a_project_with_no_memory_yet_reports_empty_sections_not_an_error() {
        let (_data, _workspace, runtime) = bootstrapped_runtime();

        let memory = build_project_overview_memory(&runtime).expect("must not fail");
        assert!(memory.decisions.is_empty());
        assert!(memory.todos.is_empty());
        assert_eq!(memory.todos_omitted, 0);
    }

    /// A recorded, active constraint and decision both come back from the
    /// real `MemoryStore::binding` call, and a memory with no authority
    /// classification (`None`, never presented as a rule) does not.
    #[test]
    fn active_decisions_and_constraints_are_read_through_the_real_binding_query() {
        let (_data, _workspace, runtime) = bootstrapped_runtime();
        let memory = ProjectMemory::open(&runtime).expect("open");
        let store = memory.store();

        store
            .record(
                NewMemory::new(MemoryKind::Constraint, "the local gate must run alone")
                    .with_authority(Some(MemoryAuthority::Constraint)),
            )
            .unwrap();
        store
            .record(
                NewMemory::new(MemoryKind::Decision, "sonnet closes phase 41")
                    .with_authority(Some(MemoryAuthority::Decision)),
            )
            .unwrap();
        // Never classified, so `binding()` must not return it — see
        // `MemoryStore::binding`'s own doc comment.
        store
            .record(NewMemory::new(
                MemoryKind::Finding,
                "an unclassified finding",
            ))
            .unwrap();

        let overview = build_project_overview_memory(&runtime).expect("must not fail");
        assert_eq!(overview.decisions.len(), 2);
        assert!(
            overview
                .decisions
                .iter()
                .any(|line| line.contains("the local gate must run alone"))
        );
        assert!(
            overview
                .decisions
                .iter()
                .any(|line| line.contains("sonnet closes phase 41"))
        );
        assert!(
            overview
                .decisions
                .iter()
                .all(|line| !line.contains("an unclassified finding"))
        );
    }

    /// A resolved todo is queryable but must never be presented as open work
    /// — `MemoryStatus::is_open_work`'s own contract, proven here through the
    /// same `snapshot` call the overview uses.
    #[test]
    fn only_unresolved_todos_are_shown() {
        let (_data, _workspace, runtime) = bootstrapped_runtime();
        let memory = ProjectMemory::open(&runtime).expect("open");
        let store = memory.store();

        store
            .record(NewMemory::new(MemoryKind::Todo, "wire the shell into main"))
            .unwrap();
        let resolved = store
            .record(NewMemory::new(MemoryKind::Todo, "already done"))
            .unwrap();
        store
            .set_status(&resolved.id, crate::memory::MemoryStatus::Resolved)
            .unwrap();

        let overview = build_project_overview_memory(&runtime).expect("must not fail");
        assert_eq!(overview.todos.len(), 1);
        assert!(overview.todos[0].contains("wire the shell into main"));
        assert!(
            overview
                .todos
                .iter()
                .all(|line| !line.contains("already done"))
        );
    }

    /// The `p` key opens the overlay through the real run-loop action, and
    /// the overlay carries the memory the run loop read — not a
    /// hand-constructed fixture.
    #[test]
    fn opening_the_project_overview_shows_real_memory() {
        let (_data, _workspace, runtime) = bootstrapped_runtime();
        let memory = ProjectMemory::open(&runtime).expect("open");
        memory
            .store()
            .record(
                NewMemory::new(MemoryKind::Constraint, "never run ci-local beside cargo")
                    .with_authority(Some(MemoryAuthority::Constraint)),
            )
            .unwrap();

        let mut state = state::ShellState::new(
            "glasshouse",
            runtime.project().display_root(),
            "test",
            Vec::new(),
        );
        assert_eq!(
            state.handle_key(crossterm::event::KeyEvent::from(
                crossterm::event::KeyCode::Char('p')
            )),
            state::Action::OpenProjectOverview
        );

        let built = build_project_overview_memory(&runtime).expect("must not fail");
        state.open_project_overview(
            built.decisions,
            built.todos,
            built.todos_omitted,
            Vec::new(),
            None,
        );

        assert_eq!(state.overlay(), Some(state::Overlay::ProjectOverview));
        let overview = state.project_overview().expect("open");
        assert!(
            overview
                .decisions()
                .iter()
                .any(|line| line.contains("never run ci-local beside cargo"))
        );
    }
}

/// Phase 41 lines 1657-1660 and 1663: [`resource_capacity_line`]'s honesty
/// rules, each proven directly against a hand-built
/// [`crate::provider::quota::CapacityState`] — the same construction
/// technique `provider::quota`'s own tests use, entirely through public
/// constructors, so every case is fast and needs no runtime or on-disk
/// config — plus one test that goes through
/// [`build_project_overview_capacity`]'s real config-file and
/// gateway-quota-cache reads, so the formatter is proven reachable from a
/// real configured provider and not only from a hand-built fixture
/// (practice §35).
#[cfg(test)]
mod project_overview_capacity_tests {
    use super::*;
    use crate::provider::quota::{
        Capacity, CapacityBandThresholds, CapacityState, NativeAmount, Pool, Reading,
        ReadingSource, WindowCapacity, WindowShape, Windows,
    };

    const NOW: i64 = 1_800_000_000;

    /// A `requests` pool whose remaining and limit both came from a
    /// provider's own response header — [`ReadingSource::ResponseHeader`],
    /// which is the only [`crate::provider::quota::TelemetryClass::Authoritative`]
    /// producer, so [`crate::provider::quota::Percentage::exact`] answers
    /// `Some` for it.
    fn measured_requests_pool(remaining: i64, limit: i64) -> Pool {
        Pool::unmeasured()
            .with_limit(Capacity::Measured(Reading::new(
                NativeAmount::whole(limit, "requests"),
                NOW,
                ReadingSource::ResponseHeader("x-ratelimit-limit-requests".to_owned()),
            )))
            .with_remaining(Capacity::Measured(Reading::new(
                NativeAmount::whole(remaining, "requests"),
                NOW,
                ReadingSource::ResponseHeader("x-ratelimit-remaining-requests".to_owned()),
            )))
    }

    /// The same pool, with `remaining` inferred rather than read from the
    /// provider — [`ReadingSource::InferredEstimate`], so the combined
    /// percentage can never be [`crate::provider::quota::Percentage::Exact`].
    fn estimated_requests_pool(remaining: i64, limit: i64) -> Pool {
        Pool::unmeasured()
            .with_limit(Capacity::Measured(Reading::new(
                NativeAmount::whole(limit, "requests"),
                NOW,
                ReadingSource::ResponseHeader("x-ratelimit-limit-requests".to_owned()),
            )))
            .with_remaining(Capacity::Measured(Reading::new(
                NativeAmount::whole(remaining, "requests"),
                NOW,
                ReadingSource::InferredEstimate("recent usage".to_owned()),
            )))
    }

    fn with_reset(state: CapacityState, seconds_from_now: i64) -> CapacityState {
        let windows = Windows::uniform(Pool::unmeasured(), Capacity::Unmeasured).with_rolling(
            WindowCapacity::uniform(
                WindowShape::Rolling,
                Pool::unmeasured(),
                Capacity::Unmeasured,
            )
            .with_resets_at(Capacity::Measured(Reading::new(
                NOW + seconds_from_now,
                NOW,
                ReadingSource::ResponseHeader("x-ratelimit-reset-requests".to_owned()),
            ))),
        );
        state.with_windows(windows)
    }

    /// Map lines 1658 and 1659: a measured reading renders its band and the
    /// literal word `"measured"`.
    #[test]
    fn a_measured_reading_renders_its_band_and_says_measured() {
        let state = CapacityState::metered_balance().with_requests(measured_requests_pool(82, 100));
        let line = resource_capacity_line(
            "openrouter (remote)",
            &state,
            &CapacityBandThresholds::DEFAULT,
            20,
            NOW,
        );
        assert!(line.contains("82%"), "{line}");
        assert!(line.contains("[measured]"), "{line}");
        assert!(line.contains("plenty"), "{line}");
    }

    /// Map line 1659: the same resource with only an estimated reading
    /// renders the estimate labelled as one, and the two renderings differ —
    /// the mutation this test kills is `remove-validation` dropping the
    /// measured/estimated label.
    #[test]
    fn a_measured_and_an_estimated_reading_of_the_same_resource_render_differently() {
        let thresholds = CapacityBandThresholds::DEFAULT;
        let measured =
            CapacityState::metered_balance().with_requests(measured_requests_pool(82, 100));
        let estimated =
            CapacityState::metered_balance().with_requests(estimated_requests_pool(82, 100));

        let measured_line =
            resource_capacity_line("openrouter (remote)", &measured, &thresholds, 20, NOW);
        let estimated_line =
            resource_capacity_line("openrouter (remote)", &estimated, &thresholds, 20, NOW);

        assert!(measured_line.contains("[measured]"), "{measured_line}");
        assert!(estimated_line.contains("[estimated]"), "{estimated_line}");
        assert_ne!(measured_line, estimated_line);
    }

    /// Map lines 1658 and 1659: a resource with no telemetry at all renders
    /// `"unknown"` and no number anywhere — the mutation this test kills is
    /// `accept-stale-state` rendering an unknown capacity as a number.
    #[test]
    fn no_telemetry_renders_unknown_with_no_number_at_all() {
        let state = CapacityState::metered_balance();
        let line = resource_capacity_line(
            "openrouter (remote)",
            &state,
            &CapacityBandThresholds::DEFAULT,
            20,
            NOW,
        );
        assert!(line.contains("unknown"), "{line}");
        assert!(
            !line.chars().any(|c| c.is_ascii_digit()),
            "must show no number at all: {line}"
        );
    }

    /// Map line 1660: a constrained resource — one whose reset time is
    /// actually known — renders it.
    #[test]
    fn a_constrained_resource_with_a_known_reset_shows_it() {
        let state = with_reset(
            CapacityState::metered_balance().with_requests(measured_requests_pool(82, 100)),
            3600,
        );
        let line = resource_capacity_line(
            "openrouter (remote)",
            &state,
            &CapacityBandThresholds::DEFAULT,
            20,
            NOW,
        );
        assert!(line.contains("reset in 3600s"), "{line}");
    }

    /// Map line 1660: the same resource with no reset ever read renders
    /// none — the two renderings the acceptance test asks to differ.
    #[test]
    fn an_unconstrained_resource_shows_no_reset() {
        let state = CapacityState::metered_balance().with_requests(measured_requests_pool(82, 100));
        let line = resource_capacity_line(
            "openrouter (remote)",
            &state,
            &CapacityBandThresholds::DEFAULT,
            20,
            NOW,
        );
        assert!(!line.contains("reset"), "{line}");
    }

    /// Map line 1663: a reserve that currently gates routing — this
    /// resource's band, folded with its reserve percentage, has crossed into
    /// [`crate::provider::quota::CapacityBand::Reserve`], the exact boundary
    /// `crate::provider::quota::evaluate_reserve_spend` itself stops
    /// trivially allowing at — appears.
    #[test]
    fn a_reserve_that_currently_gates_routing_appears() {
        // 10% is below `CapacityBandThresholds::DEFAULT`'s 15% reserve
        // boundary, so the band is `Reserve` and the policy actually runs.
        let state = CapacityState::metered_balance().with_requests(measured_requests_pool(10, 100));
        let line = resource_capacity_line(
            "openrouter (remote)",
            &state,
            &CapacityBandThresholds::DEFAULT,
            20,
            NOW,
        );
        assert!(line.contains("protected reserve 20%"), "{line}");
        assert!(line.contains("limiting routing"), "{line}");
    }

    /// Map line 1663: a reserve that has influenced nothing — this
    /// resource's band is well above `Reserve` — does not appear. The
    /// mutation this test kills is `invert-condition` showing a reserve that
    /// influenced nothing.
    #[test]
    fn a_reserve_that_influences_nothing_does_not_appear() {
        let state = CapacityState::metered_balance().with_requests(measured_requests_pool(80, 100));
        let line = resource_capacity_line(
            "openrouter (remote)",
            &state,
            &CapacityBandThresholds::DEFAULT,
            20,
            NOW,
        );
        assert!(!line.contains("reserve"), "{line}");
    }

    /// Bootstrap a `Runtime` over fresh, isolated data/config/workspace
    /// directories, matching `project_overview_tests::bootstrapped_runtime`.
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

    /// A project with no configured provider shows no resource lines rather
    /// than the full, unconfigured `provider::registry` catalog — the
    /// behavioral contract's "configured resources", read through
    /// [`EffectiveConfig::provider_names`].
    #[test]
    fn no_configured_providers_yields_no_resource_lines() {
        let (_data, _workspace, runtime) = bootstrapped_runtime();
        let lines = build_project_overview_capacity(&runtime);
        assert!(lines.is_empty(), "{lines:?}");
    }

    /// [`build_project_overview_capacity`] reached through its real callers
    /// — a real configured provider on disk, and a real planted
    /// [`crate::provider::telemetry::GatewayQuotaCache`] reading, the same
    /// on-disk bridge `main.rs::resources_report` and
    /// `main.rs::disposable_candidate_capacity` already read — not a
    /// hand-built [`crate::provider::quota::CapacityState`] a test
    /// constructed itself (practice §35).
    #[test]
    fn build_project_overview_capacity_reads_a_real_configured_provider_and_a_real_planted_reading()
    {
        let (_data, _workspace, runtime) = bootstrapped_runtime();

        let mut user = UserConfig::load(runtime.paths()).unwrap();
        let provider = crate::config::ProviderConfig::new("openai-compatible");
        user.providers_mut()
            .set("overview-capacity-test-provider", provider);
        user.save(runtime.paths()).unwrap();

        let now_unix = crate::provider::cache::now_unix_seconds();
        crate::provider::telemetry::GatewayQuotaCache::new(runtime.paths()).store(
            "overview-capacity-test-provider",
            &crate::provider::telemetry::RateLimitHeaders::read(vec![
                ("x-ratelimit-limit-requests", "100"),
                ("x-ratelimit-remaining-requests", "82"),
            ]),
            now_unix,
        );

        let lines = build_project_overview_capacity(&runtime);
        assert_eq!(lines.len(), 1, "{lines:?}");
        assert!(
            lines[0].contains("overview-capacity-test-provider"),
            "{lines:?}"
        );
        assert!(lines[0].contains("82%"), "{lines:?}");
        assert!(lines[0].contains("[measured]"), "{lines:?}");
    }
}

/// Phase 25: the project-knowledge view reads every kind of durable project
/// memory through [`build_project_knowledge_memory`] — the production
/// function `Action::OpenProjectKnowledge`'s handler calls, not a helper
/// that re-implements the query (practice §35). Map lines 1098-1107.
#[cfg(test)]
mod project_knowledge_tests {
    use super::*;
    use crate::memory::{MemoryKind, MemoryStatus, NewMemory, ProjectMemory};

    /// Same bootstrap `project_overview_tests` uses — an isolated, real
    /// on-disk project database, not a fixture that reimplements the query.
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

    /// A project with no memory at all gets five empty, honest sections —
    /// not an error (map line 1098's empty-state half).
    #[test]
    fn a_project_with_no_knowledge_yet_reports_empty_sections_not_an_error() {
        let (_data, _workspace, runtime) = bootstrapped_runtime();

        let memory = build_project_knowledge_memory(&runtime).expect("must not fail");
        for section in [
            &memory.decisions,
            &memory.constraints,
            &memory.features,
            &memory.failed_attempts,
            &memory.todos,
        ] {
            assert!(section.lines.is_empty());
            assert_eq!(section.omitted, 0);
        }
    }

    /// Map line 1100, and acceptance test 3: a superseded decision is
    /// history, not active knowledge, so it must not appear in the active
    /// decisions section — only the memory that replaced it does.
    #[test]
    fn a_superseded_decision_does_not_appear_among_active_decisions() {
        let (_data, _workspace, runtime) = bootstrapped_runtime();
        let memory = ProjectMemory::open(&runtime).expect("open");
        let store = memory.store();

        let old = store
            .record(NewMemory::new(
                MemoryKind::Decision,
                "ship the old approach",
            ))
            .unwrap();
        let new = store
            .record(NewMemory::new(
                MemoryKind::Decision,
                "ship the replacement approach",
            ))
            .unwrap();
        store.supersede(&old.id, &new.id).unwrap();

        let built = build_project_knowledge_memory(&runtime).expect("must not fail");
        assert!(
            built
                .decisions
                .lines
                .iter()
                .any(|line| line.contains("ship the replacement approach"))
        );
        assert!(
            built
                .decisions
                .lines
                .iter()
                .all(|line| !line.contains("ship the old approach"))
        );
    }

    /// Map lines 1101 and 1102: known constraints and implemented-or-planned
    /// features are filtered to current knowledge the same way decisions
    /// are — a superseded record of either kind does not reach its section.
    #[test]
    fn constraints_and_features_are_filtered_to_current_the_same_way_decisions_are() {
        let (_data, _workspace, runtime) = bootstrapped_runtime();
        let memory = ProjectMemory::open(&runtime).expect("open");
        let store = memory.store();

        store
            .record(NewMemory::new(
                MemoryKind::Constraint,
                "the local gate must run alone",
            ))
            .unwrap();
        let old_constraint = store
            .record(NewMemory::new(MemoryKind::Constraint, "an old constraint"))
            .unwrap();
        let new_constraint = store
            .record(NewMemory::new(MemoryKind::Constraint, "its replacement"))
            .unwrap();
        store
            .supersede(&old_constraint.id, &new_constraint.id)
            .unwrap();

        store
            .record(NewMemory::new(MemoryKind::Feature, "the knowledge view"))
            .unwrap();
        let old_feature = store
            .record(NewMemory::new(MemoryKind::Feature, "an old feature plan"))
            .unwrap();
        let new_feature = store
            .record(NewMemory::new(MemoryKind::Feature, "the revised plan"))
            .unwrap();
        store.supersede(&old_feature.id, &new_feature.id).unwrap();

        let built = build_project_knowledge_memory(&runtime).expect("must not fail");
        assert_eq!(built.constraints.lines.len(), 2);
        assert!(
            built
                .constraints
                .lines
                .iter()
                .all(|line| !line.contains("an old constraint"))
        );
        assert_eq!(built.features.lines.len(), 2);
        assert!(
            built
                .features
                .lines
                .iter()
                .all(|line| !line.contains("an old feature plan"))
        );
    }

    /// Map line 1104, and acceptance test 3's other half: a resolved todo is
    /// queryable but must never be presented as unresolved work.
    #[test]
    fn a_resolved_todo_does_not_appear_among_unresolved_todos() {
        let (_data, _workspace, runtime) = bootstrapped_runtime();
        let memory = ProjectMemory::open(&runtime).expect("open");
        let store = memory.store();

        store
            .record(NewMemory::new(MemoryKind::Todo, "wire the knowledge view"))
            .unwrap();
        let resolved = store
            .record(NewMemory::new(MemoryKind::Todo, "already done"))
            .unwrap();
        store
            .set_status(&resolved.id, MemoryStatus::Resolved)
            .unwrap();

        let built = build_project_knowledge_memory(&runtime).expect("must not fail");
        assert_eq!(built.todos.lines.len(), 1);
        assert!(built.todos.lines[0].contains("wire the knowledge view"));
        assert!(
            built
                .todos
                .lines
                .iter()
                .all(|line| !line.contains("already done"))
        );
    }

    /// Map line 1104 turns on `MemoryStatus::is_open_work`, not
    /// `is_current`: a todo under review is not `Active`, but it is still
    /// open work and must still count as unresolved. This is what would
    /// distinguish the two predicates if `knowledge_section`'s todos call
    /// were quietly narrowed to `is_current`.
    #[test]
    fn a_todo_marked_needs_review_still_counts_as_unresolved() {
        let (_data, _workspace, runtime) = bootstrapped_runtime();
        let memory = ProjectMemory::open(&runtime).expect("open");
        let store = memory.store();

        let todo = store
            .record(NewMemory::new(MemoryKind::Todo, "revisit after the audit"))
            .unwrap();
        store
            .set_status(&todo.id, MemoryStatus::NeedsReview)
            .unwrap();

        let built = build_project_knowledge_memory(&runtime).expect("must not fail");
        assert!(
            built
                .todos
                .lines
                .iter()
                .any(|line| line.contains("revisit after the audit"))
        );
    }

    /// Map lines 1103 and 1106: failed approaches are shown regardless of
    /// status — including one a newer memory has superseded — and the
    /// superseded one names its successor while the current one stays
    /// silent about supersession, since it has none.
    #[test]
    fn failed_approaches_are_shown_regardless_of_status_and_name_their_successor() {
        let (_data, _workspace, runtime) = bootstrapped_runtime();
        let memory = ProjectMemory::open(&runtime).expect("open");
        let store = memory.store();

        let old = store
            .record(NewMemory::new(
                MemoryKind::FailedAttempt,
                "tried a global lock, it deadlocked",
            ))
            .unwrap();
        let new = store
            .record(NewMemory::new(
                MemoryKind::FailedAttempt,
                "tried per-project locks instead, still fails under load",
            ))
            .unwrap();
        store.supersede(&old.id, &new.id).unwrap();

        let built = build_project_knowledge_memory(&runtime).expect("must not fail");
        assert_eq!(built.failed_attempts.lines.len(), 2);

        let old_line = built
            .failed_attempts
            .lines
            .iter()
            .find(|line| line.contains("tried a global lock"))
            .expect("the superseded failed attempt is still shown");
        assert!(
            old_line.contains(&format!("superseded by {}", new.id)),
            "must name its successor: {old_line}"
        );

        let new_line = built
            .failed_attempts
            .lines
            .iter()
            .find(|line| line.contains("tried per-project locks"))
            .expect("the current failed attempt is shown");
        assert!(
            !new_line.contains("superseded by"),
            "has no successor, so must say nothing: {new_line}"
        );
    }

    /// The `k` key opens the overlay through the real run-loop action, and
    /// the overlay carries the memory the run loop read — not a
    /// hand-constructed fixture.
    #[test]
    fn opening_the_project_knowledge_view_shows_real_memory() {
        let (_data, _workspace, runtime) = bootstrapped_runtime();
        let memory = ProjectMemory::open(&runtime).expect("open");
        memory
            .store()
            .record(NewMemory::new(
                MemoryKind::Constraint,
                "never run ci-local beside cargo",
            ))
            .unwrap();

        let mut state = state::ShellState::new(
            "glasshouse",
            runtime.project().display_root(),
            "test",
            Vec::new(),
        );
        assert_eq!(
            state.handle_key(crossterm::event::KeyEvent::from(
                crossterm::event::KeyCode::Char('k')
            )),
            state::Action::OpenProjectKnowledge
        );

        let built = build_project_knowledge_memory(&runtime).expect("must not fail");
        state.open_project_knowledge(
            built.decisions,
            built.constraints,
            built.features,
            built.failed_attempts,
            built.todos,
            None,
        );

        assert_eq!(state.overlay(), Some(state::Overlay::ProjectKnowledge));
        let knowledge = state.project_knowledge().expect("open");
        assert!(
            knowledge
                .constraints()
                .lines
                .iter()
                .any(|line| line.contains("never run ci-local beside cargo"))
        );
    }

    /// Map line 1105: [`knowledge_detail`] carries the real rationale,
    /// source session and source commit a memory was recorded with —
    /// through [`build_project_knowledge_memory`], the production function,
    /// not a hand-built fixture.
    #[test]
    fn build_project_knowledge_memory_carries_real_provenance_for_the_detail_view() {
        use crate::memory::DecisionProvenance;

        let (_data, _workspace, runtime) = bootstrapped_runtime();
        let memory = ProjectMemory::open(&runtime).expect("open");
        memory
            .store()
            .record(
                NewMemory::new(MemoryKind::Decision, "adopt the drill-down view")
                    .with_source_session(Some("sess_01AAAAAAAAAAAAAAAAAAAAAAAA"))
                    .with_source_commit(Some("d34db33f"))
                    .with_provenance(DecisionProvenance {
                        rationale: Some("answers one question at a time".to_owned()),
                        ..Default::default()
                    }),
            )
            .unwrap();

        let built = build_project_knowledge_memory(&runtime).expect("must not fail");
        assert_eq!(built.decisions.lines.len(), 1);
        assert_eq!(built.decisions.details.len(), 1);
        let detail = &built.decisions.details[0];
        assert_eq!(
            detail.rationale.as_deref(),
            Some("answers one question at a time")
        );
        assert_eq!(
            detail.source_session.as_deref(),
            Some("sess_01AAAAAAAAAAAAAAAAAAAAAAAA")
        );
        assert_eq!(detail.source_commit.as_deref(), Some("d34db33f"));
        assert_eq!(detail.lifecycle, "active");
    }

    /// Map line 1105's honesty half, at the query layer: a memory recorded
    /// with no rationale, no source session and no source commit produces a
    /// [`MemoryDetail`] with `None` in each of those fields — never an
    /// empty string standing in for "not recorded".
    #[test]
    fn build_project_knowledge_memory_leaves_unrecorded_provenance_as_none() {
        let (_data, _workspace, runtime) = bootstrapped_runtime();
        let memory = ProjectMemory::open(&runtime).expect("open");
        memory
            .store()
            .record(NewMemory::new(
                MemoryKind::Todo,
                "wire the knowledge view into main",
            ))
            .unwrap();

        let built = build_project_knowledge_memory(&runtime).expect("must not fail");
        assert_eq!(built.todos.details.len(), 1);
        let detail = &built.todos.details[0];
        assert_eq!(detail.rationale, None);
        assert_eq!(detail.source_session, None);
        assert_eq!(detail.source_commit, None);
        assert_eq!(detail.lifecycle, MemoryStatus::Active.to_string());
    }
}

/// Map line 234: the project-memory view reads every kind of durable
/// project memory, at every status, through [`build_project_memory_view`] —
/// the production function `Action::OpenProjectMemory`'s handler calls, not
/// a helper that re-implements the query (practice §35).
#[cfg(test)]
mod project_memory_tests {
    use super::*;
    use crate::memory::{MemoryKind, MemoryStatus, NewMemory, ProjectMemory};

    /// Same bootstrap `project_knowledge_tests` uses — an isolated, real
    /// on-disk project database, not a fixture that reimplements the query.
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

    /// A project with no memory at all gets one empty, honest section — not
    /// an error.
    #[test]
    fn a_project_with_no_memory_yet_reports_an_empty_section_not_an_error() {
        let (_data, _workspace, runtime) = bootstrapped_runtime();

        let memory = build_project_memory_view(&runtime).expect("must not fail");
        assert!(memory.lines.is_empty());
        assert_eq!(memory.omitted, 0);
    }

    /// The whole point of this view next to `ProjectKnowledge`: a `Finding`
    /// record has no section in `build_project_knowledge_memory` at all, but
    /// it must appear here.
    #[test]
    fn a_finding_record_appears_in_the_project_memory_view() {
        let (_data, _workspace, runtime) = bootstrapped_runtime();
        let memory = ProjectMemory::open(&runtime).expect("open");
        memory
            .store()
            .record(NewMemory::new(
                MemoryKind::Finding,
                "the local gate must run alone",
            ))
            .unwrap();

        let built = build_project_memory_view(&runtime).expect("must not fail");
        assert!(
            built
                .lines
                .iter()
                .any(|line| line.contains("the local gate must run alone")),
            "{:?}",
            built.lines
        );

        let knowledge = build_project_knowledge_memory(&runtime).expect("must not fail");
        for section in [
            &knowledge.decisions,
            &knowledge.constraints,
            &knowledge.features,
            &knowledge.failed_attempts,
            &knowledge.todos,
        ] {
            assert!(
                section
                    .lines
                    .iter()
                    .all(|line| !line.contains("the local gate must run alone")),
                "a Finding must not reach any ProjectKnowledge section: {:?}",
                section.lines
            );
        }
    }

    /// Unlike `build_project_knowledge_memory`'s five sections, this view is
    /// not filtered by status: a superseded decision — invisible to the
    /// active-decisions section — is still shown here, with its status said
    /// on the line rather than implied by which section it is in.
    #[test]
    fn a_superseded_record_still_appears_here_with_its_status_on_the_line() {
        let (_data, _workspace, runtime) = bootstrapped_runtime();
        let memory = ProjectMemory::open(&runtime).expect("open");
        let store = memory.store();

        let old = store
            .record(NewMemory::new(
                MemoryKind::Decision,
                "ship the old approach",
            ))
            .unwrap();
        let new = store
            .record(NewMemory::new(
                MemoryKind::Decision,
                "ship the replacement approach",
            ))
            .unwrap();
        store.supersede(&old.id, &new.id).unwrap();

        let built = build_project_memory_view(&runtime).expect("must not fail");
        let old_line = built
            .lines
            .iter()
            .find(|line| line.contains("ship the old approach"))
            .expect("the superseded decision must still be shown");
        assert!(
            old_line.contains(&format!("[{}]", MemoryStatus::Superseded)),
            "its status must be said on the line: {old_line}"
        );
    }

    /// The `M` key opens the overlay through the real run-loop action, and
    /// the overlay carries the memory the run loop read — not a
    /// hand-constructed fixture.
    #[test]
    fn opening_the_project_memory_view_shows_real_memory() {
        let (_data, _workspace, runtime) = bootstrapped_runtime();
        let memory = ProjectMemory::open(&runtime).expect("open");
        memory
            .store()
            .record(NewMemory::new(MemoryKind::Finding, "placeholder"))
            .unwrap();

        let mut state = state::ShellState::new(
            "glasshouse",
            runtime.project().display_root(),
            "test",
            Vec::new(),
        );
        assert_eq!(
            state.handle_key(crossterm::event::KeyEvent::from(
                crossterm::event::KeyCode::Char('M')
            )),
            state::Action::OpenProjectMemory
        );

        let built = build_project_memory_view(&runtime).expect("must not fail");
        state.open_project_memory(built, None);

        assert_eq!(state.overlay(), Some(state::Overlay::ProjectMemory));
        let shown = state.project_memory().expect("open");
        assert!(
            shown
                .memory()
                .lines
                .iter()
                .any(|line| line.contains("placeholder"))
        );
    }
}

/// Phase 47 lines 1762 and 1764: the route-evidence table reads real
/// recorded routing observations through [`build_route_evidence_table`] —
/// the production function `Action::OpenRouteEvidence`'s handler calls, not
/// a helper that re-implements
/// `routing::evidence::EvidenceLedger::observed_identities` (practice §35),
/// the one method that can answer which identities exist at all
/// (practice §71).
#[cfg(test)]
mod route_evidence_tests {
    use super::*;
    use crate::routing::evidence::{EvidenceLedger, NewObservation};

    /// Same bootstrap `project_overview_tests` and `project_knowledge_tests`
    /// use — an isolated, real on-disk project database.
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

    /// A project with no routing evidence at all gets an honest, empty
    /// table — not an error. `EvidenceLedger::open` creates the database on
    /// first use, so "no evidence yet" and "could not read the ledger" must
    /// not collapse into the same outcome, the same rule
    /// `a_project_with_no_memory_yet_reports_empty_sections_not_an_error`
    /// proves for the project overview.
    #[test]
    fn a_project_with_no_routing_evidence_yet_reports_an_empty_table_not_an_error() {
        let (_data, _workspace, runtime) = bootstrapped_runtime();
        let rows = build_route_evidence_table(&runtime).expect("must not fail");
        assert!(rows.is_empty());
    }

    /// Real recorded observations, through the production `EvidenceLedger`,
    /// come back as distinct rows with their real sample counts — not a
    /// fixture standing in for the ledger. Acceptance test 4.
    #[test]
    fn two_recorded_identities_come_back_as_two_rows_with_real_sample_counts() {
        let (_data, _workspace, runtime) = bootstrapped_runtime();
        let ledger = EvidenceLedger::open(&runtime).expect("open");
        // `build_route_evidence_table` windows against the real wall clock
        // (`ROUTE_EVIDENCE_WINDOW_SECONDS`), so observations here must be
        // recorded near real "now" — a small fixed epoch like `1_000` would
        // fall outside every window this function ever queries.
        let now = crate::provider::cache::now_unix_seconds();
        ledger
            .record(
                NewObservation::new("anyrouter", "claude-opus-4-1")
                    .with_route(Some("anthropic-messages")),
                now - 20,
            )
            .unwrap();
        ledger
            .record(
                NewObservation::new("anyrouter", "claude-opus-4-1")
                    .with_route(Some("anthropic-messages")),
                now - 10,
            )
            .unwrap();
        ledger
            .record(NewObservation::new("openai-router", "gpt-5"), now)
            .unwrap();

        let rows = build_route_evidence_table(&runtime).expect("must not fail");
        assert_eq!(rows.len(), 2);
        let anyrouter = rows
            .iter()
            .find(|row| row.provider == "anyrouter")
            .expect("anyrouter row");
        let openai = rows
            .iter()
            .find(|row| row.provider == "openai-router")
            .expect("openai row");
        assert_eq!(anyrouter.sample_count, 2);
        assert_eq!(openai.sample_count, 1);
        assert_ne!(
            anyrouter.sample_count, openai.sample_count,
            "two identities with different counts must render differently"
        );
    }

    /// Line 1764: a row recorded with no context state — the honest default
    /// every real production row has today, since
    /// `NewObservation::with_context_state` has zero non-test callers (see
    /// `routing::evidence`'s own module header) — comes back labelled
    /// `"unknown"`, never blank and never upgraded to a measurement.
    #[test]
    fn a_row_with_no_recorded_context_state_reads_unknown() {
        let (_data, _workspace, runtime) = bootstrapped_runtime();
        let ledger = EvidenceLedger::open(&runtime).expect("open");
        let now = crate::provider::cache::now_unix_seconds();
        ledger
            .record(NewObservation::new("anyrouter", "m"), now)
            .unwrap();

        let rows = build_route_evidence_table(&runtime).expect("must not fail");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].context_state, "unknown");
    }

    /// The `r` key opens the overlay through the real run-loop action, and
    /// the overlay carries the rows the run loop actually read — not a
    /// hand-constructed fixture (practice §35).
    #[test]
    fn opening_the_route_evidence_table_shows_real_recorded_observations() {
        let (_data, _workspace, runtime) = bootstrapped_runtime();
        let ledger = EvidenceLedger::open(&runtime).expect("open");
        let now = crate::provider::cache::now_unix_seconds();
        ledger
            .record(NewObservation::new("anyrouter", "claude-opus-4-1"), now)
            .unwrap();

        let mut state = state::ShellState::new(
            "glasshouse",
            runtime.project().display_root(),
            "test",
            Vec::new(),
        );
        assert_eq!(
            state.handle_key(crossterm::event::KeyEvent::from(
                crossterm::event::KeyCode::Char('r')
            )),
            state::Action::OpenRouteEvidence
        );

        let rows = build_route_evidence_table(&runtime).expect("must not fail");
        state.open_route_evidence(rows, None);

        assert_eq!(state.overlay(), Some(state::Overlay::RouteEvidence));
        let evidence = state.route_evidence().expect("open");
        assert!(
            evidence
                .rows()
                .iter()
                .any(|row| row.provider == "anyrouter")
        );
    }
}

/// Phase 47 line 1765: the route-health view reads real gateway telemetry
/// through [`build_route_health_table`] — the production function
/// `Action::OpenRouteHealth`'s handler calls.
///
/// These write through the *production* cache writers
/// (`GatewayHealthCache::store` and `GatewayQuotaCache::store`, the same two
/// calls `gateway::mod`'s accept loop makes) and read back through the
/// production builder, rather than hand-building a `RouteHealthRow`. A test
/// that constructed the row itself would leave the builder deletable without
/// anything noticing, which is practice §35's exact failure.
#[cfg(test)]
mod route_health_tests {
    use super::*;
    use crate::provider::telemetry::{
        GatewayHealthCache, GatewayHealthReading, GatewayQuotaCache, RateLimitHeaders,
    };

    /// The same bootstrap `route_evidence_tests` uses. `data_dir` is where
    /// both telemetry caches live, so pointing it at a temporary directory is
    /// what keeps these tests from reading the developer's own installation.
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

    fn reading(
        model: &str,
        consecutive_failures: u32,
        cooling_down_until_unix: Option<i64>,
        credential_rejected: bool,
    ) -> GatewayHealthReading {
        GatewayHealthReading {
            credential_label: "anyrouter/ANYROUTER_API_KEY".to_owned(),
            model: model.to_owned(),
            consecutive_failures,
            cooling_down_until_unix,
            credential_rejected,
        }
    }

    /// A fresh installation has observed nothing, and that is a complete
    /// answer rather than an error — the caches' own fail-soft contract,
    /// which is also why this builder returns no `Result`.
    #[test]
    fn an_installation_with_no_gateway_telemetry_yields_no_rows() {
        let (_data, _workspace, runtime) = bootstrapped_runtime();
        assert!(build_route_health_table(&runtime).is_empty());
    }

    /// The five concepts come back as five *separate* fields, read through
    /// the production caches. The fixture is deliberately one where they
    /// disagree — no failures, yet unavailable, and paced — because that is
    /// the case a single collapsed status word cannot represent.
    #[test]
    fn the_five_concepts_survive_the_process_boundary_as_separate_fields() {
        let (_data, _workspace, runtime) = bootstrapped_runtime();
        let now = crate::provider::cache::now_unix_seconds();

        GatewayHealthCache::new(runtime.paths()).store(
            "anyrouter",
            &[reading("claude-opus-4-1", 0, Some(now + 300), true)],
            now,
        );
        GatewayQuotaCache::new(runtime.paths()).store(
            "anyrouter",
            &RateLimitHeaders::read([
                ("ratelimit-limit", "300"),
                ("ratelimit-remaining", "12"),
                ("ratelimit-reset", "1800"),
            ]),
            now,
        );

        let rows = build_route_health_table(&runtime);
        assert_eq!(rows.len(), 1);
        let row = &rows[0];

        // 1. route health — a streak and a flag, both preserved.
        assert_eq!(row.consecutive_failures, 0);
        assert!(row.credential_rejected);
        // 2. immediate availability — the producer's own answer, and it
        //    disagrees with the zero failure streak above.
        assert!(
            !row.available_now,
            "a refused credential is unavailable even with no failure streak"
        );
        // 3. cadence — Glasshouse's own pacing and the provider's window,
        //    two different facts kept apart.
        assert_eq!(row.cooling_down_until_unix, Some(now + 300));
        assert_eq!(row.stated_limit, Some(300));
        assert_eq!(row.stated_window_seconds, None);
        // 4. quota reset — the provider's own clock, a different instant
        //    from the cooldown above.
        assert_eq!(row.quota_resets_at_unix, Some(now + 1_800));
        assert_ne!(
            row.quota_resets_at_unix, row.cooling_down_until_unix,
            "the provider's reset and Glasshouse's cooldown are two clocks"
        );
        // 5. failure-domain evidence — one observed resource, so nothing is
        //    known to share its domain, and never `independent`.
        assert_eq!(row.failure_domain, "unknown");
        assert_eq!(row.failure_domain_peers, 0);
    }

    /// A provider with nothing stated leaves the three provider-sourced
    /// concepts `None` — the shape the view turns into `unknown`. A default
    /// of zero here is the defect this assertion exists to catch, because it
    /// would reach the screen as a measurement.
    #[test]
    fn a_provider_that_stated_no_headers_leaves_every_stated_field_none() {
        let (_data, _workspace, runtime) = bootstrapped_runtime();
        let now = crate::provider::cache::now_unix_seconds();
        GatewayHealthCache::new(runtime.paths()).store(
            "openrouter",
            &[reading("some-free-model", 2, None, false)],
            now,
        );

        let rows = build_route_health_table(&runtime);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].stated_limit, None);
        assert_eq!(rows[0].stated_window_seconds, None);
        assert_eq!(rows[0].quota_resets_at_unix, None);
        assert_eq!(rows[0].cooling_down_until_unix, None);
        // Route health is still real: the streak crossed the boundary.
        assert_eq!(rows[0].consecutive_failures, 2);
    }

    /// Failure-domain evidence is about a *pair*, and the only signal this
    /// build has is the provider. Two resources behind one provider are
    /// `shared`; each is `unknown` with respect to the other provider, and
    /// nothing anywhere is ever `independent`.
    #[test]
    fn two_resources_on_one_provider_are_shared_and_never_independent() {
        let (_data, _workspace, runtime) = bootstrapped_runtime();
        let now = crate::provider::cache::now_unix_seconds();
        let health = GatewayHealthCache::new(runtime.paths());
        health.store(
            "anyrouter",
            &[
                reading("model-a", 0, None, false),
                reading("model-b", 1, None, false),
            ],
            now,
        );
        health.store("openrouter", &[reading("model-c", 0, None, false)], now);

        let rows = build_route_health_table(&runtime);
        assert_eq!(rows.len(), 3);
        for row in &rows {
            assert_ne!(
                row.failure_domain, "independent",
                "nothing in this build can establish independence"
            );
        }
        let anyrouter: Vec<_> = rows.iter().filter(|r| r.provider == "anyrouter").collect();
        assert_eq!(anyrouter.len(), 2);
        for row in &anyrouter {
            assert_eq!(row.failure_domain, "shared");
            assert_eq!(row.failure_domain_peers, 1);
        }
        let lone = rows
            .iter()
            .find(|r| r.provider == "openrouter")
            .expect("openrouter row");
        assert_eq!(lone.failure_domain, "unknown");
        assert_eq!(lone.failure_domain_peers, 0);
    }

    /// The `h` key reaches this builder through the real run-loop action, and
    /// the overlay carries the rows the builder actually read — not a
    /// hand-constructed fixture (practice §35).
    #[test]
    fn opening_the_route_health_view_shows_real_gateway_telemetry() {
        let (_data, _workspace, runtime) = bootstrapped_runtime();
        let now = crate::provider::cache::now_unix_seconds();
        GatewayHealthCache::new(runtime.paths()).store(
            "anyrouter",
            &[reading("claude-opus-4-1", 3, None, false)],
            now,
        );

        let mut state = state::ShellState::new(
            "glasshouse",
            runtime.project().display_root(),
            "test",
            Vec::new(),
        );
        assert_eq!(
            state.handle_key(crossterm::event::KeyEvent::from(
                crossterm::event::KeyCode::Char('h')
            )),
            state::Action::OpenRouteHealth
        );

        state.open_route_health(build_route_health_table(&runtime));

        assert_eq!(state.overlay(), Some(state::Overlay::RouteHealth));
        let health = state.route_health().expect("open");
        let row = health
            .rows()
            .iter()
            .find(|row| row.provider == "anyrouter")
            .expect("the observed resource must reach the overlay");
        assert_eq!(row.consecutive_failures, 3);
    }

    /// The isolation invariant, asserted rather than assumed: this builder
    /// opens **no project database at all**. It reads two provider-keyed
    /// cache directories under the installation's data directory, so there is
    /// no project predicate for it to get wrong — and a future edit that
    /// started reading project rows here would have to delete this test.
    #[test]
    fn the_builder_reads_no_project_scoped_store() {
        let source = include_str!("mod.rs");
        let start = source
            .find("fn build_route_health_table(")
            .expect("the function must exist");
        // Ended at the next item at column zero, read with `str::lines` so a
        // CRLF checkout cannot defeat it (practice §14).
        let body: String = source[start..]
            .lines()
            .skip(1)
            .take_while(|line| !line.starts_with('}'))
            .collect::<Vec<_>>()
            .join("\n");
        for forbidden in [
            "MemoryStore",
            "EvidenceLedger",
            "ProjectSessions",
            "EventLog",
            "project_state_dir",
            "Connection",
        ] {
            assert!(
                !body.contains(forbidden),
                "build_route_health_table must not reach a project-scoped store, \
                 but names `{forbidden}`:\n{body}"
            );
        }
        assert!(
            body.contains("data_dir") || body.contains("GatewayHealthCache::new"),
            "the builder must read the installation-wide telemetry caches:\n{body}"
        );
    }
}

/// Phase 9A line 368's shell half: `start_session` — the TUI's `n` key — must
/// record the same six facts `main.rs::launch_session` does, not `-` for
/// every one of them.
///
/// These call [`start_session`] itself, the production function, against a
/// real [`SessionRuntime`] and a fake installed harness — the same shape
/// `tests/events_lifecycle.rs` already uses to drive `SessionRuntime` outside
/// a real terminal. A test that resolved the six facts by hand instead would
/// prove nothing about whether `start_session` actually calls the code that
/// resolves them.
#[cfg(test)]
mod native_session_facts_tests {
    use super::*;

    /// A [`Runtime`] whose config directory already names one installed,
    /// harmless harness — a shell script that exits immediately, exactly like
    /// `tests/session_model.rs`'s fake `claude-code`.
    fn runtime_with_fake_claude_code() -> (tempfile::TempDir, tempfile::TempDir, crate::Runtime) {
        use clap::Parser;

        let data = tempfile::tempdir().expect("tempdir");
        let workspace = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir_all(workspace.path().join(".git")).expect("create .git");
        let workspace_root =
            std::fs::canonicalize(workspace.path()).expect("canonicalize workspace root");

        let bin_dir = data.path().join("bin");
        std::fs::create_dir_all(&bin_dir).expect("create bin dir");
        let harness = install_fake_claude_code(&bin_dir);
        let escaped = harness.display().to_string().replace('\\', "\\\\");
        std::fs::write(
            data.path().join("config.toml"),
            format!(
                "version = 1\n\n\
                 [integrations.claude-code]\nenabled = true\nexecutable = \"{escaped}\"\n"
            ),
        )
        .expect("write user config");

        let cli = crate::Cli::try_parse_from([
            "glasshouse",
            "--data-dir",
            data.path().to_str().unwrap(),
            "--config-dir",
            data.path().to_str().unwrap(),
        ])
        .unwrap();
        let runtime = crate::bootstrap(&cli, &workspace_root).unwrap();
        (data, workspace, runtime)
    }

    #[cfg(unix)]
    fn install_fake_claude_code(bin_dir: &std::path::Path) -> std::path::PathBuf {
        use std::os::unix::fs::PermissionsExt;

        let path = bin_dir.join("fake-claude");
        std::fs::write(&path, "#!/bin/sh\nexit 0\n").expect("write fake harness");
        let mut perms = std::fs::metadata(&path).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&path, perms).unwrap();
        path
    }

    #[cfg(windows)]
    fn install_fake_claude_code(bin_dir: &std::path::Path) -> std::path::PathBuf {
        let path = bin_dir.join("fake-claude.cmd");
        std::fs::write(&path, "@echo off\r\nexit /b 0\r\n").expect("write fake harness");
        path
    }

    #[test]
    fn starting_a_session_from_the_shell_records_all_six_facts() {
        let (_data, _workspace, runtime) = runtime_with_fake_claude_code();
        let sessions = ProjectSessions::open(&runtime).expect("open project sessions");
        let mut live = SessionRuntime::new();
        let mut index_snapshots = HashMap::new();

        start_session(
            &runtime,
            &mut live,
            &sessions,
            SessionPresentation::Embedded,
            TerminalSize::new(24, 80),
            &mut index_snapshots,
        )
        .expect("starting a session from the shell must succeed");

        let records = sessions.store().list().expect("list sessions");
        assert_eq!(records.len(), 1, "exactly one session must be recorded");
        let record = &records[0];

        // Line 368's own words: "the resolved harness, backend resource,
        // model, protocol, pairing class, and response profile" — six facts,
        // and this used to record none of them.
        assert_eq!(
            record.launch_profile.as_deref(),
            Some(crate::profile::NATIVE_PROFILE_NAME),
            "the implied Native profile is still a profile, and must be named"
        );
        assert_eq!(
            record.backend_resource.as_deref(),
            Some("native"),
            "record: {record:?}"
        );
        assert!(record.model.is_some(), "record: {record:?}");
        assert!(record.pairing_class.is_some(), "record: {record:?}");
        assert!(record.protocol.is_some(), "record: {record:?}");
        assert!(record.response_profile.is_some(), "record: {record:?}");
        assert!(record.response_mechanism.is_some(), "record: {record:?}");
    }
}
