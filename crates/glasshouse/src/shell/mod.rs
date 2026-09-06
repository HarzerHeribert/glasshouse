//! The main interactive interface: what `glasshouse` opens with no
//! arguments — a persistent top bar naming the project and its canonical
//! root, a session bar listing the project's sessions, a viewport reserved
//! for the active session's terminal, and a session overview a keystroke
//! away. Split the same way the first-run wizard is — [`state`] answers keys
//! without drawing, [`view`] draws without deciding anything.
//! This is where a [`crate::session::SessionRuntime`] is actually owned: the
//! shell holds several live harnesses at once and gives one of them the
//! keyboard — see `.agent-runtime/design-shell-session-modes.md` and
//! [`state::Mode`]. The viewport is the focused session's own screen,
//! converted each tick by `build_viewport_grid` into a
//! [`state::ViewportGrid`] and drawn by `view::render_viewport`; the run
//! loop also answers a session's cursor-position queries and tells its PTY
//! and emulator the viewport's own size, not the terminal's outer one — see
//! [`view::viewport_slot`].

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
    ProviderSettingsEdit, ReachabilityCheck, RouteDecisionRow, RouteEvidenceRow, RouteHealthRow,
    RoutingRow, RoutingSettingsEdit, SettingsEdit, ShellState, ViewportGrid,
};

/// Open the shell and run it until the user leaves. Session *records* are
/// read once at startup and re-read whenever the event loop is nudged. The
/// [`SessionRuntime`] built here starts out empty: leaving the shell leaves
/// every session it started exactly as it was, and a recorded session is
/// not automatically live again just because its row is in the bar; only
/// `n` or a resume starts a process.
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
    // The one normalized lifecycle stream, shared with the session runtime
    // and, through the sink below, the project's durable log.
    let events = EventBus::new();
    let event_log = attach_event_log(runtime, &events);
    // Drained every tick; publishing never waits on it — the queue is bounded
    // and oldest events drop if a viewport stops draining. See `crate::events::bus`.
    let event_stream = events.subscribe();

    let checkpoints = ProjectCheckpoints::open(runtime)?;

    let mut live = SessionRuntime::with_event_bus(
        crate::session::runtime::DEFAULT_SCROLLBACK_BYTES,
        events.clone(),
    );
    // What each session's harness index held before it ran — half the
    // identity guard for a shared-index harness; must be read at start, not
    // exit. See `session::native_id::snapshot`. Kept in memory only: it is
    // meaningless once the session ends, and a shell that dies mid-session has
    // nothing to capture anyway.
    let mut index_snapshots: HashMap<SessionId, session::native_id::IndexSnapshot> = HashMap::new();

    // Acquired after the database work above, so a failure there leaves the
    // user's terminal untouched rather than flashing an alternate screen.
    let mut screen = Screen::acquire()?;
    let events = EventSource::new(DEFAULT_TICK);
    // Where a provider probe's answer comes back — the request runs on its
    // own thread (`spawn_provider_probe`), off the thread drawing the terminal.
    let (probe_results, probe_inbox) = std::sync::mpsc::channel::<ProviderProbeResult>();
    // Where a harness's own reports come back — reading them means reading
    // SQLite, and a reader can wait on the writer; on the drawing thread that
    // is a frozen interface, a defect this project has already shipped once.
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
                                    // No viewport, so `N` would look like a
                                    // no-op — `render_viewport`'s placeholder
                                    // says so on every frame.
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
                        let routing = build_project_overview_routing(runtime);
                        match build_project_overview_memory(runtime) {
                            Ok(memory) => {
                                state.open_project_overview(
                                    memory.decisions,
                                    memory.todos,
                                    memory.todos_omitted,
                                    resources,
                                    routing,
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
                                    routing,
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
                    Action::OpenRouteDecisions => match build_route_decision_table(runtime) {
                        Ok(rows) => {
                            state.open_route_decisions(rows, None);
                        }
                        Err(err) => {
                            tracing::warn!(
                                error = %err,
                                "could not read the evaluation ledger for the routing-decisions view"
                            );
                            state.open_route_decisions(
                                Vec::new(),
                                Some(format!("routing decisions unavailable: {err:#}")),
                            );
                        }
                    },
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
                        // `ProbeTimeouts::default()` and nothing else;
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
                        // The wizard drives its own `Screen`; released here,
                        // reacquired below — the two never hold it at once.
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
                    // Inner size, not the terminal's outer one — see
                    // `view::viewport_slot` — or a harness draws over chrome.
                    let slot = view::viewport_slot(Rect::new(0, 0, cols, rows));
                    if let Err(err) = live.resize(&id, TerminalSize::new(slot.height, slot.width)) {
                        tracing::warn!(session = %id, %err, "could not resize the focused session");
                    }
                }
                screen.draw(|frame| view::render(&state, frame))?;
            }
            Event::Tick => {
                // A signal is the only thing besides a key that ends the
                // shell, and must be noticed between keystrokes.
                if crate::shutdown::shutdown_requested() {
                    return Ok(());
                }

                // An embedded session has no real terminal to answer its own
                // `ESC[6n` — Glasshouse must, every tick, or a harness
                // waiting on the reply hangs looking like it did nothing.
                live.answer_terminal_queries();

                let mut redraw = false;
                let exits = live.poll_exits();
                let any_exited = !exits.is_empty();
                for (id, status) in exits {
                    // `ProcessExit` owns this classification, the only place
                    // it lives — two copies of "did it crash" can disagree.
                    let lifecycle = ProcessExit::from_status(&status).session_state();
                    // The session is over, so this is the tightest the
                    // discovery window will ever be — see `native_id::capture`.
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
                // Phase 11 line 688: disposition only turns `Resumable` here,
                // on exit — without this, `r` on a session that just exited
                // would still read "still running".
                if any_exited && state.refresh(sessions.store().list()?) == Action::Redraw {
                    redraw = true;
                }

                // Both sides of the one stream, drained here, never on the
                // thread reading a pseudo-terminal.
                let mut recorded = event_stream.drain();
                recorded.extend(reported_inbox.try_iter().flatten());
                // The overview's activity view is the consumer.
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

                // Drained here too, so a probe result can never be stranded
                // by a wake-up that raced a tick.
                if drain_provider_probes(&probe_inbox, &mut state) {
                    redraw = true;
                }
                // Otherwise the in-flight line draws once and looks hung.
                if state.provider_probe_in_flight() {
                    redraw = true;
                }

                // Headless is skipped here too, though `render_viewport`
                // already refuses to draw one — see design-decisions.md,
                // "Trims: `shell/mod.rs`" for the mutation that proved this
                // load-bearing anyway. The runtime's presentation is the
                // authority, not the stored record.
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
                // Re-read the records rather than trusting the sender to
                // describe what moved — the list is small.
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
/// Best effort: a project whose database cannot be opened loses event
/// history and keeps its sessions. The sink queues behind a writer thread
/// rather than writing inline — see [`crate::events::log`] — because
/// publishing happens on whichever thread produced the event, and one of
/// those is the thread draining a pseudo-terminal.
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
/// A lifecycle hook runs as its own short-lived process and cannot be seen
/// by simply subscribing to this process's bus — why, in design-decisions.md,
/// "Trims: `shell/mod.rs`". Reads on its own thread rather than inline, on
/// the same [`spawn_provider_probe`] reasoning: reading it means reading
/// SQLite, and a reader can wait on the writer, which on the drawing thread
/// is a frozen interface — a defect class this project has shipped once.
/// Starts from the log's current head, not its beginning: opening the
/// interface should show what happens next, not replay a week.
fn spawn_event_tail(
    runtime: &Runtime,
    reported: &std::sync::mpsc::Sender<Vec<RecordedEvent>>,
    wake: &std::sync::mpsc::Sender<AppEvent>,
) {
    /// How often the log is asked what is new — far slower than the
    /// interface's own tick, since a quarter-second-late harness event is
    /// imperceptible next to the turn it belongs to.
    const POLL: std::time::Duration = std::time::Duration::from_millis(250);
    /// Most rows to take in one pass, so a log that grew while closed cannot
    /// arrive as one enormous message.
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

    // Not joined and not stopped explicitly: it ends when the channel's
    // receiver goes, which is when `run` returns — the same lifetime
    // `spawn_provider_probe`'s thread has, for the same reason.
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
/// A guard rather than a call, because `shell::run` returns from several
/// places and the one that would get forgotten is whichever is added next.
/// Bounded, on [`crate::shutdown`]'s own reasoning: losing the last few
/// events is survivable, and not returning the user's terminal is not.
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
/// A checkpoint's objective, state and next actions are authored — Glasshouse
/// will not guess them from terminal output. So an automatic checkpoint
/// **carries forward the handoff the user last wrote**, restamped with the
/// current time and repository position, rather than leaving it stale. A
/// session whose user never took a checkpoint gets nothing, silently.
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
        // A turn ending is the task boundary Glasshouse detects — a process
        // exiting says the harness is gone, not that the work finished.
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

/// Interrupt one session, whether or not it is the one on screen. Does
/// **not** focus it, change which session the bar presents, or move its
/// recorded lifecycle — an exit from the interrupt is noticed by
/// `poll_exits` on the next tick, from the operating system. Whether the
/// byte becomes a signal is the platform's business: `PtyProcess::interrupt`
/// writes `ETX`, and it is the Unix line discipline — or ConPTY's Win32
/// input mode — that turns it into a process-group interrupt.
fn interrupt_session(live: &mut SessionRuntime, state: &mut ShellState, id: &SessionId) {
    let name = state::short_session_id(id);
    match live.interrupt(id) {
        Ok(()) => state.set_status(format!("interrupted session `{name}`")),
        // Refused out loud: the runtime knows things the records do not — a
        // session that exited since the last poll still reads as live here.
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
/// [`RuntimeError`]'s own `Display` names the session in full — right for a
/// log line, wrong for a status note that already named it, long enough to
/// clip in the popup. Found by running the shipped binary.
fn refusal_reason(err: &RuntimeError) -> String {
    match err {
        RuntimeError::NotLive { .. } => "it is not running in this Glasshouse".to_owned(),
        RuntimeError::Exited { .. } => "it has already exited".to_owned(),
        RuntimeError::Headless { .. } => "it is headless and has no viewport".to_owned(),
        // Names neither the session (already named) nor the pasted line.
        RuntimeError::LineTooLong { bytes, limit, .. } => {
            format!("that line is {bytes} bytes and its terminal takes at most {limit} in one line")
        }
        RuntimeError::Io { source, .. } => source.to_string(),
    }
}

/// Send one line to a session, whether or not it is the one on screen.
/// A carriage return is appended because this is a *line*: a bare `\r` is
/// what a real Enter key delivers, so this arrives indistinguishable from
/// something typed. Does not touch focus — a line arriving in a background
/// session must never pull the user out of the one they are working in.
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

/// Bring the runtime's focus in line with whichever session the bar shows.
/// `RuntimeError::NotLive` is ignored on purpose: a session the bar lists
/// but that is not running in this invocation is normal. Never touches a
/// process — see [`SessionRuntime::focus`] — only changes which live
/// session the keyboard reaches.
fn sync_focus(live: &mut SessionRuntime, state: &ShellState) {
    let Some(active) = state.active_session() else {
        return;
    };
    if live.focused() == Some(&active.id) {
        return;
    }
    match live.focus(&active.id) {
        Ok(()) | Err(RuntimeError::NotLive { .. }) => {}
        // A headless session has no viewport to bring forward — the bar
        // moving onto one leaves the keyboard where it was rather than
        // logging a failure on every key; see `ShellState::enter_session_mode`.
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
/// needs to know about the other's colour type. `Default` becomes `None`,
/// meaning "inherit whatever is already there", so a cell whose
/// fore/background was never set keeps the terminal's own default instead
/// of being forced to literal black or white.
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
/// selection seam `main.rs: launch_session` uses, minus attaching to this
/// process's own terminal: the shell gives the session the viewport once its
/// output arrives instead. `presentation` is the only difference between `n`
/// and `N` — everything else is shared, so a headless session is an ordinary
/// one not shown. `size` is the viewport's own inner size at the moment `n`
/// was pressed, not the terminal's outer size — see `view::viewport_slot`
/// and `HarnessLaunch::size`: a harness TUI lays itself out from the size it
/// sees at startup, so the wrong geometry draws its first frame short.
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

    // Phase 9A line 368. The shell's quick-open resolves no launch profile or
    // response request of its own, so both take the implied defaults: the
    // `Native` profile and the `Interactive` role — the same kind of answer
    // `glasshouse launch <harness>` records unadorned, not `-` for every
    // column `main.rs::launch_session` fills in.
    let launch_profile = crate::profile::LaunchProfile::native(selection.id());
    let pairing = {
        use crate::harness::Declared;
        use crate::harness::pairing::{PairingQuery, ServingRoute, classify};
        use crate::routing::AssignedModel;

        // The same fallback `main.rs::session_pairing` builds for `Native`:
        // `pairing_queries` never lists it, so a lookup here would always
        // miss anyway.
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
        // process owns — the diagnostic channel every shell warning uses.
        tracing::warn!(problem, "could not read part of the response profile");
    }
    let response_application =
        crate::harness::response::apply(selection.adapter(), response_profile.resolved());

    // Recorded before the process exists and is the single source of truth:
    // `live.start` below gets `record.presentation`, so it cannot disagree.
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

    // Before the harness runs — see `index_snapshots` in `run`.
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
    // `install_session_document` rather than `install_hooks`: hooks and the
    // response profile now share one document, exactly as
    // `main.rs::launch_session`'s already does.
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
    let mut launch = HarnessLaunch::new(selection.into_executable(), app_runtime.project())
        .args(args)
        .size(size)
        .without_provider_credentials(&effective);
    // Map lines 1973 and 488: the scrubs `launch_session` applies — the child
    // inherits neither another entitlement's credential variable from this
    // process's environment nor any configured provider's.
    let entitlement =
        match effective.entitlement_for(launch_profile.harness, &launch_profile.backend) {
            Ok(entitlement) => entitlement,
            Err(err) => {
                tracing::warn!(
                    session = %record.id,
                    error = %err,
                    "could not resolve the serving entitlement for the credential scrub"
                );
                None
            }
        };
    for var in effective.foreign_entitlement_credential_vars(entitlement.as_ref().map(|e| e.name()))
    {
        launch = launch.env_remove(var);
    }
    let launch = launch;
    if let Err(err) = live.start(record.id.clone(), record.presentation, &launch) {
        // Never polled for its exit, so its snapshot has nothing to pair with.
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
/// "allow the user to resume any compatible stopped session from the overview".
/// `main.rs::resume_session`'s embedded counterpart, mirroring
/// `start_session` above: the shell keeps every live harness in its own
/// [`SessionRuntime`] rather than handing the terminal away, so this calls
/// `live.start` with the resumed id. **Gaps, recorded in this phase's
/// evidence, not silent approximations** (both outside `FORBIDDEN FILES`):
/// no re-resolved launch profile overlay, so no regenerated provider
/// configuration; and no [`LifecycleEvent::SessionResumed`] since
/// `SessionRuntime::start` always publishes `SessionStarted`.
fn resume_session(
    app_runtime: &Runtime,
    live: &mut SessionRuntime,
    sessions: &ProjectSessions,
    id: &SessionId,
    size: TerminalSize,
) -> anyhow::Result<()> {
    let store = sessions.store();
    // The store's own gate, not a second copy of it — the same check
    // `main.rs::resume_session` relies on, so an overview and a CLI resume
    // refuse exactly the same sessions for exactly the same reasons.
    let resumable = store.open_for_resume(id)?;
    let record = store
        .get(&resumable.id)?
        .expect("open_for_resume already proved this session's record exists");

    let user = UserConfig::load(app_runtime.paths())?;
    let project_config = config::load_project_config(app_runtime.project())?;
    let effective = EffectiveConfig::new(&user, project_config.as_ref());
    // The record's own harness, not whatever is configured now — resuming a
    // Codex conversation in Claude Code would be nonsense; same rule
    // `main.rs::resume_session` states for the CLI path.
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

    // `open_for_resume` proved the process exited, but its `LiveSession` is
    // still in `live` — `SessionRuntime` never drops one on its own, and
    // `get`/`focus`/`interrupt`/`send_text` resolve the *first* entry with a
    // given id. Without this, a fresh process under the same id would leave
    // those calls talking to the exited process's frozen screen. Best-effort:
    // `NotLive` here only means the entry was already gone.
    let _ = live.close(&resumable.id);

    // Map lines 1973 and 488 on the resume path — the same scrubs
    // `start_session` applies above. The record's own launch profile, with
    // the same Native fallback, resolved before `selection` is consumed
    // below.
    let resume_profile = record
        .launch_profile
        .as_deref()
        .and_then(|name| effective.launch_profile(name, selection.id()).ok())
        .map(|layered| layered.value)
        .unwrap_or_else(|| crate::profile::LaunchProfile::native(selection.id()));
    let entitlement =
        match effective.entitlement_for(resume_profile.harness, &resume_profile.backend) {
            Ok(entitlement) => entitlement,
            Err(err) => {
                tracing::warn!(
                    session = %resumable.id,
                    error = %err,
                    "could not resolve the serving entitlement for the credential scrub"
                );
                None
            }
        };
    let mut launch = HarnessLaunch::new(selection.into_executable(), app_runtime.project())
        .args(args)
        .size(size)
        .without_provider_credentials(&effective);
    for var in effective.foreign_entitlement_credential_vars(entitlement.as_ref().map(|e| e.name()))
    {
        launch = launch.env_remove(var);
    }
    let launch = launch;
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
/// Loads `UserConfig` fresh from disk, not whatever unsaved edits are staged
/// in `state` — reopening the wizard and saving Settings are separate write
/// paths, so it shows the user's persisted choices, not a blank wizard.
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
/// Reading `crate::memory` is file I/O this module deliberately keeps out of
/// `shell/state.rs`, like [`build_settings`] and `EffectiveConfig`.
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
/// the condensed sibling of `glasshouse resources`'s full report, read the
/// same way: [`crate::provider::resources::observed_capacity`], no network
/// call. Scoped to [`EffectiveConfig::provider_names`], not the full
/// registry — the same set `main.rs::disposable_candidates` scores a
/// routing decision over. Line 1661 is [`build_project_overview_routing`],
/// split out because it reads a database this function never opens.
/// Cannot fail visibly: an unreadable configuration becomes one honest line,
/// the same file-I/O split [`build_project_overview_memory`] keeps.
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

    // **Line 1283's producer.** The rows a burn rate counts, read once for
    // every provider below. Fail-soft: a ledger that cannot be opened leaves
    // the forecast honestly absent — never an error, never a guess.
    let consumption = crate::routing::evidence::EvidenceLedger::open(runtime)
        .and_then(|ledger| {
            Ok(ledger.consumption_in_window(
                now_unix,
                crate::routing::evidence::CLASSIFICATION_EVIDENCE_WINDOW_SECONDS,
            )?)
        })
        .map_err(|err| {
            tracing::debug!(
                error = %err,
                "could not read the routing evidence ledger for the capacity overview's forecasts"
            );
        })
        .ok();

    let mut lines: Vec<String> = providers
        .into_iter()
        .map(|provider| {
            let kind = ResourceKind::from_direct_provider(&provider);
            let state = observed_capacity(&kind, &effective, &telemetry, now_unix);
            let reserve_percent = effective.reserve_percent(&provider).value.get();
            let thresholds = base_thresholds.with_resource_reserve(reserve_percent);
            let seconds_until_reset = state.seconds_until_reset(now_unix);
            // Keyed provider-wide (`quota_context: None`): this overview is
            // per configured provider, and names no credential of it.
            let forecast = consumption.as_ref().and_then(|rows| {
                crate::routing::burn::forecast(
                    rows,
                    crate::routing::burn::ResourceKey {
                        provider: &provider,
                        quota_context: None,
                    },
                    state.requests().remaining(),
                    now_unix,
                    seconds_until_reset,
                )
            });
            resource_capacity_line(
                &kind.label(),
                &state,
                &thresholds,
                reserve_percent,
                now_unix,
                forecast,
            )
        })
        .collect();

    // Line 1276: the same rows, read once more for the moving average per
    // task class. Absent entirely, never a zero, when no class has enough
    // live rows — gated at `MIN_ROWS_FOR_BURN_RATE`, the same minimum
    // `burn_rate` enforces for the per-resource line above.
    if let Some(rows) = consumption.as_ref() {
        let rates = crate::routing::burn::task_class_request_rates(rows, now_unix, None);
        let printable: Vec<_> = rates
            .iter()
            .filter(|rate| rate.rows >= crate::routing::burn::MIN_ROWS_FOR_BURN_RATE)
            .collect();
        if !printable.is_empty() {
            // Line 1275: the same floor, on `token_rows` independently of
            // the request figure — `tokens not counted` rather than a
            // fabricated `0 tok/h`.
            let by_class = printable
                .iter()
                .map(|rate| {
                    let tokens = if rate.token_rows >= crate::routing::burn::MIN_ROWS_FOR_BURN_RATE
                    {
                        format!(
                            "~{:.0} tok/h",
                            rate.tokens_per_hour.expect(
                                "token_rows at or above the floor means tokens_per_hour is Some"
                            )
                        )
                    } else {
                        "tokens not counted".to_owned()
                    };
                    format!(
                        "{} ~{:.1}/h, {tokens}",
                        rate.class.as_str(),
                        rate.requests_per_hour
                    )
                })
                .collect::<Vec<_>>()
                .join(" · ");
            lines.push(format!(
                "  requests by task class (recent, estimated)  {by_class}"
            ));
        }
    }

    lines
}

/// Map line 1661: the routing model currently selected to classify work, and
/// its most recent observed latency — the first production reader
/// `EvidenceLedger::summarize`'s duration fields have had outside a test.
/// Model: [`EffectiveConfig::routing_model_resolution`], the same live
/// answer map line 1680 reports — a `Pinned` choice naming a since-removed
/// provider does not read as "selected" when nothing routes through it.
/// Latency: only [`crate::config::RoutingModelResolution::Pinned`] names an
/// identity the ledger can query; `Automatic`/`Heuristics` classify without
/// one, so the line says so rather than showing an average attributed to a
/// name that did not earn it (ruling 3). Cannot fail visibly: degrades to
/// one honest line, the same shape [`build_project_overview_capacity`] uses.
fn build_project_overview_routing(runtime: &Runtime) -> String {
    use crate::config::RoutingModelResolution;
    use crate::routing::evidence::EvidenceLedger;

    let user = match UserConfig::load(runtime.paths()) {
        Ok(user) => user,
        Err(err) => return format!("  routing model  unavailable: {err:#}"),
    };
    let project_config = match config::load_project_config(runtime.project()) {
        Ok(project_config) => project_config,
        Err(err) => return format!("  routing model  unavailable: {err:#}"),
    };
    let effective = EffectiveConfig::new(&user, project_config.as_ref());
    let resolution = effective.routing_model_resolution().value;
    let label = routing_resolution_label(&resolution);

    let latency = match &resolution {
        RoutingModelResolution::Pinned { provider, model } => {
            let now_unix = crate::provider::cache::now_unix_seconds();
            match EvidenceLedger::open(runtime) {
                Ok(ledger) => match ledger.summarize_latest_for_model(
                    provider,
                    model,
                    now_unix,
                    ROUTE_EVIDENCE_WINDOW_SECONDS,
                ) {
                    Ok(summary) => routing_latency_phrase(summary.as_ref()),
                    Err(err) => format!("unavailable: {err:#}"),
                },
                Err(err) => format!("unavailable: {err:#}"),
            }
        }
        RoutingModelResolution::Automatic | RoutingModelResolution::Heuristics(_) => {
            "not applicable — no single model is selected".to_owned()
        }
    };

    format!("  routing model  {label}, recent latency {latency}")
}

/// The short label for what will actually classify a request right now — the
/// first pure half of [`build_project_overview_routing`], testable without a
/// config file. Matches `shell::view::render_routing`'s word choice for
/// [`crate::config::RoutingModelChoice::Automatic`]/`Deterministic`/`Pinned`
/// exactly, plus the one thing a *resolution* can say that a raw choice
/// cannot: which fallback, if any, is actually in effect right now.
fn routing_resolution_label(resolution: &crate::config::RoutingModelResolution) -> String {
    use crate::config::{RoutingFallback, RoutingModelResolution};

    match resolution {
        RoutingModelResolution::Automatic => "automatic".to_owned(),
        RoutingModelResolution::Pinned { provider, model } => format!("{provider}:{model}"),
        RoutingModelResolution::Heuristics(RoutingFallback::NotConfigured) => {
            "deterministic heuristics (none configured)".to_owned()
        }
        RoutingModelResolution::Heuristics(RoutingFallback::DeterministicChosen) => {
            "deterministic heuristics".to_owned()
        }
        RoutingModelResolution::Heuristics(RoutingFallback::ProviderNotConfigured {
            provider,
            ..
        }) => format!("deterministic heuristics (`{provider}` no longer configured)"),
    }
}

/// One phrase naming a queried model's most recent latency, or exactly why
/// there is none — the second pure half of [`build_project_overview_routing`],
/// testable directly against a hand-built
/// [`crate::routing::evidence::RoutingSummary`].
/// Ruling 1: `None` is never `0`. `summary` being absent and
/// `summary.median_duration_ms` being absent (below the minimum sample) both
/// read the same honest "unknown" here — a caller downstream does not need
/// to tell the two apart; `summarize_latest_for_model` keeps that
/// distinction for one that does.
fn routing_latency_phrase(summary: Option<&crate::routing::evidence::RoutingSummary>) -> String {
    let Some(median) = summary.and_then(|s| s.median_duration_ms.as_ref()) else {
        return "unknown — not enough observations yet".to_owned();
    };
    let tail = summary
        .and_then(|s| s.tail_duration_ms.as_ref())
        .map(|reading| format!(", p95 {}ms", reading.value()));
    format!(
        "median {}ms{} ({} sample(s))",
        median.value(),
        tail.unwrap_or_default(),
        median.sample_count()
    )
}

/// One line describing what Glasshouse currently believes about `label`'s
/// capacity — the pure formatting half of
/// [`build_project_overview_capacity`], testable directly against a
/// hand-built [`crate::provider::quota::CapacityState`].
/// `thresholds` must already carry this resource's own protected reserve —
/// mirrors `crate::provider::resources`'s private
/// `capacity_band_thresholds_for` rather than calling it (outside this
/// package's partition this round). Lines 1659 and 1663's exact wording and
/// gating are in design-decisions.md, "Trims: `shell/mod.rs`".
fn resource_capacity_line(
    label: &str,
    state: &crate::provider::quota::CapacityState,
    thresholds: &crate::provider::quota::CapacityBandThresholds,
    reserve_percent: u8,
    now_unix: i64,
    forecast: Option<crate::routing::burn::ExhaustionForecast>,
) -> String {
    use crate::provider::quota::{CapacityBand, TelemetryClass};

    let reset_note = match state.seconds_until_reset(now_unix) {
        Some(seconds) => format!(", reset in {seconds}s"),
        None => String::new(),
    };
    let forecast_note = forecast_note(forecast);

    let Some(score) = state.remaining_capacity_score() else {
        // No pool normalized to a percentage, but a manually configured plan
        // may still carry a class worth naming. `None` here is the genuine
        // "unknown" case line 1657/1658 name.
        let class_word = match state.telemetry_class() {
            None => "unknown",
            Some(TelemetryClass::Authoritative | TelemetryClass::Observed) => "measured",
            Some(TelemetryClass::Estimated) => "estimated",
            Some(TelemetryClass::Manual) => "manual",
        };
        return format!("  {label}  capacity {class_word}{reset_note}{forecast_note}");
    };

    let band = score.band(thresholds);
    // `RemainingCapacityScore::percent` is only ever `Exact` or `Estimated`
    // here, never `Manual` or absent — line 1659's distinction exactly, and
    // deliberately not `state.telemetry_class()`, which answers the whole
    // resource's *best* source and would say "measured" even when the one
    // number shown is an estimate.
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

    format!("  {label}  {band} {digits}% [{class_word}]{reset_note}{reserve_note}{forecast_note}")
}

/// **Line 1283**: an exhaustion forecast rendered as an *estimate*, never as
/// a promise — real error bars a reader will act on, so every sentence is
/// hedged in the text itself. Full wording rationale in design-decisions.md,
/// "Trims: `shell/mod.rs`". `""` when there is no forecast, which the
/// property `a_resource_with_no_forecast_prints_exactly_what_it_printed_before`
/// pins.
fn forecast_note(forecast: Option<crate::routing::burn::ExhaustionForecast>) -> String {
    let Some(forecast) = forecast else {
        return String::new();
    };
    let hours = forecast.seconds_to_exhaustion as f64 / 3600.0;
    let reach = match forecast.survives_until_reset {
        Some(false) => ", and may not reach its reset at the current rate",
        Some(true) => ", which at the current rate would carry it past its reset",
        None => "",
    };
    format!(
        "; estimated to last about {hours:.1}h at the current rate \
         ({:.1} requests/hour over {} observations){reach}",
        forecast.requests_per_hour, forecast.rows
    )
}

/// One display line: the memory's kind, and its subject if it has one or its
/// body cut to [`PROJECT_OVERVIEW_BODY_CHARS`] otherwise.
/// Prefers the subject over the body when both exist: the subject is already
/// the producer's own summary (`MemoryRecord::subject`), so cutting the body
/// instead would show less of a memory that already told us how to describe
/// it concisely.
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
/// the same generous default [`PROJECT_OVERVIEW_DECISION_LIMIT`] uses.
const PROJECT_KNOWLEDGE_SECTION_LIMIT: usize = 20;
/// Ceiling for one [`crate::memory::MemoryStore::with_status`] fetch before
/// [`knowledge_section`] applies its own per-kind display limit — bounds one
/// query, not the section shown on screen.
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
/// project's memory database. `MemoryStore` has no single "everything, by
/// kind" query, so each section is built by [`knowledge_section`] against
/// the public `with_status`/kind filter.
fn build_project_knowledge_memory(runtime: &Runtime) -> anyhow::Result<ProjectKnowledgeMemory> {
    use crate::memory::{MemoryKind, ProjectMemory};

    let memory = ProjectMemory::open(runtime)?;
    let store = memory.store();

    // Map lines 1100-1102: current project knowledge — `is_current` excludes
    // a superseded, rejected or invalidated record (acceptance test 3).
    let decisions = knowledge_section(&store, MemoryKind::Decision, |status| status.is_current())?;
    let constraints =
        knowledge_section(&store, MemoryKind::Constraint, |status| status.is_current())?;
    let features = knowledge_section(&store, MemoryKind::Feature, |status| status.is_current())?;

    // Map line 1104: *unresolved*, not merely *current* — `is_open_work`
    // also keeps a todo under review or in conflict.
    let todos = knowledge_section(&store, MemoryKind::Todo, |status| status.is_open_work())?;

    // Map line 1103: failed approaches get a dedicated *historical* section,
    // unfiltered by status, including one a newer memory has since
    // superseded (map line 1106 names that supersession).
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
/// `MemoryStore::binding` filters by authority, not kind, and
/// `memory::snapshot::snapshot` only ever returns
/// [`crate::memory::MemoryStatus::Active`] records — neither fits a section
/// needing one specific kind across a caller-chosen set of statuses. So this
/// walks [`crate::memory::MemoryStatus::ALL`] through the public
/// [`crate::memory::MemoryStore::with_status`] and keeps what matches both.
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
/// Map line 1106: said in words when a supersession exists, silent
/// otherwise — never a placeholder like "none".
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
/// `rationale` comes from `record.provenance.rationale`, not the whole
/// [`crate::memory::DecisionProvenance`] — the line names only the
/// rationale, not the recorded assumptions beside it. `lifecycle` uses
/// [`crate::memory::MemoryStatus`]'s own `Display` rather than inventing a
/// second vocabulary for the same fact.
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
/// [`build_project_knowledge_memory`]'s five sections each already imply a
/// single status by construction, so `knowledge_line` alone is enough there.
/// This view spans every [`crate::memory::MemoryStatus`] at once, so the
/// status has to be said on the line — map line 234's "at least its kind and
/// its status".
fn memory_view_line(record: &crate::memory::MemoryRecord) -> String {
    format!("[{}] {}", record.status, knowledge_line(record))
}

/// Every memory record in this project — every
/// [`crate::memory::MemoryKind`], at every [`crate::memory::MemoryStatus`],
/// most recently updated first — for [`state::Action::OpenProjectMemory`].
/// Map line 234.
/// [`build_project_knowledge_memory`]'s unfiltered sibling: no predicate and
/// no per-kind split, so every kind (including
/// [`crate::memory::MemoryKind::Finding`], never queried there) and every
/// status (including one `is_current`/`is_open_work` would drop) appears.
fn build_project_memory_view(runtime: &Runtime) -> anyhow::Result<KnowledgeSection> {
    use crate::memory::{MemoryKind, MemoryStatus, ProjectMemory};

    let memory = ProjectMemory::open(runtime)?;
    let store = memory.store();

    let by_status: Vec<Vec<crate::memory::MemoryRecord>> = MemoryStatus::ALL
        .iter()
        .copied()
        .map(|status| store.with_status(status, PROJECT_KNOWLEDGE_FETCH_LIMIT))
        .collect::<Result<Vec<_>, _>>()?;

    // Every kind, explicit rather than resting on the absence of a filter.
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
/// generous default [`PROJECT_KNOWLEDGE_SECTION_LIMIT`] uses.
const ROUTE_EVIDENCE_ROW_LIMIT: usize = 20;

/// How far back [`build_route_evidence_table`] looks for observed
/// identities. A week: long enough a project still sees its own routing
/// activity, short enough an identity nobody has exercised in a month ages
/// out. Provisional, like `crate::routing::evidence`'s own constants.
const ROUTE_EVIDENCE_WINDOW_SECONDS: i64 = 7 * 24 * 60 * 60;

/// Read the routing evidence ledger's own distinct identities — Phase 47
/// lines 1762 and 1764, closed after batch 42 found the ledger could not
/// enumerate identities at all (practice §71).
/// [`crate::routing::evidence::EvidenceLedger::observed_identities`] is this
/// package's one additive method and the whole of what makes this possible.
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

/// How many decisions the routing-decisions view shows.
/// Smaller than [`ROUTE_EVIDENCE_ROW_LIMIT`] on purpose: a row here is a
/// whole rationale, not one line, so twenty would be several screens nobody
/// scrolls. Ten is a few days of ordinary use.
const ROUTE_DECISION_ROW_LIMIT: usize = 10;

/// Read the disposable-routing rationales `glasshouse hook` recorded.
/// [`crate::evaluation::EvaluationObservations::recent_of_kind`] is this
/// package's one additive read: `recent` alone is an unkeyed listing over
/// every kind, so a project that has searched its memory recently would
/// otherwise show retrievals here instead of routing decisions.
/// **Project scope is the store's, not this function's**: the ledger opens
/// from [`Runtime`] alone, and migration 15's triggers refuse a row naming
/// any other `project_id`. **Nothing is derived** — every field is the
/// stored column, and a row recording no session or rationale arrives as
/// `None`, never an empty string.
fn build_route_decision_table(runtime: &Runtime) -> anyhow::Result<Vec<RouteDecisionRow>> {
    use crate::evaluation::{EvaluationKind, EvaluationObservations};

    let ledger = EvaluationObservations::open(runtime)?;
    let decisions = ledger.recent_of_kind(
        EvaluationKind::DisposableRouteDecided,
        ROUTE_DECISION_ROW_LIMIT,
    )?;
    Ok(decisions
        .into_iter()
        .map(|observation| RouteDecisionRow {
            observed_at_unix: observation.observed_at,
            // `subject` is the job kind's own name, written by the producer.
            // A row that recorded none says so rather than being drawn as a
            // decision about nothing in particular.
            job: observation
                .subject
                .unwrap_or_else(|| "(no job recorded)".to_owned()),
            session_id: observation.session_id,
            rationale: observation.detail,
        })
        .collect())
}

/// Read what a local gateway has observed about each free resource — Phase 47
/// map line 1765, *"show route health, immediate availability, cadence, quota
/// reset, and failure-domain evidence as separate concepts"*.
/// The shell has no gateway or router of its own — [`run`] takes only a
/// [`Runtime`] — but `crate::gateway::mod`'s accept loop, in a different
/// invocation, already writes both caches to disk on every forwarded
/// exchange; `glasshouse resources` reads them back the same way. Never
/// fails: absent, unreadable or old-format all mean *nothing was observed*.
/// **Scope is installation-wide, and the view says so**: both caches live
/// under [`crate::paths::RuntimePaths::data_dir`], keyed by provider, **not**
/// `project_state_dir` — visible to every project's shell.
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
        // two `Backend`s and neither cache stores one, so this uses the
        // identity that comparison would use — the provider name. The
        // vocabulary comes from the enum itself, and `Independent` is
        // unreachable by construction.
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
/// This is the only place that combines them: [`state::ShellState`] and its
/// `SettingsState` never run discovery or read a configuration file — that
/// would put file I/O in `shell/state.rs`, which the module keeps free of.
fn build_settings(runtime: &Runtime) -> anyhow::Result<SettingsRows> {
    let discovery = Discovery::run(runtime.project());
    // **Phase 9D line 3, and the whole of it.** Reads the model catalogue off
    // disk — no fetch, no expiry check, no network fallback on a miss. The
    // type that does this cannot make a request at all.
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

    // Providers are atomic per name — see `ProviderRow::layer` — so each
    // row's configuration and layer come from whichever table holds that
    // name, project winning over user, matching
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
    // `crate::profile::NATIVE_PROFILE_NAME` — so this merges the two tables
    // directly instead of reusing that method.
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
    // Phase 9I line 536: the user's order, disabled list and pin over the
    // free pool, layered like every routing preference beside them.
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
/// [`state::ShellState::refresh_settings`], which also clears the landed
/// edits. A failure here is not the save failing, so it only costs a stale
/// display.
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
/// **The whole lifetime of the value is this function**: taken out of the
/// Settings overlay (which no longer holds it), moved into
/// [`crate::secret::native::NativeSecretStore::store`], and dropped at the
/// closing brace. Never logged, never put in a status line, never returned —
/// every `set_status` below names the provider and the store, nothing else.
/// The same rule [`crate::profile::resolve`] follows for a launch's credential.
fn store_provider_credential(state: &mut ShellState) {
    let Some((provider, value)) = state.take_provider_credential_entry() else {
        return;
    };

    let store = secret::native::PreferNativeSecretStore::detect();
    let native = match store.native() {
        Ok(native) => native,
        Err(reason) => {
            // Line 2: an unavailable native store is reported plainly, and
            // the user is told what Glasshouse will read instead.
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
    // `crate::profile::resolve` asks with.
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
/// thread of its own** — `ureq` is blocking, and calling it from the thread
/// that reads keys and draws frames would hang the terminal until
/// [`discovery::TOTAL_TIMEOUT`] for a wedged endpoint (Phase 9E shipped
/// exactly this bug once, found by running the binary, not a test). The
/// worker resolves the credential immediately before the request and
/// nowhere else, makes one request bounded by
/// [`discovery::ProbeTimeouts::default`], sends the outcome down `results`,
/// and nudges the event loop. Not joined or tracked: bounded by its own
/// timeouts, holding nothing the shell needs back.
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
            // Resolved here, not in `state`: the last possible moment before
            // it is needed, off the drawing thread, and the `Secret` lives
            // only as long as this closure. The same store a launch would use.
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

            // A send failure means the shell has already gone — the answer
            // is simply dropped, correct for a question nobody awaits.
            if results.send(result).is_ok() {
                let _ = wake.send(AppEvent::Redraw);
            }
        })
        .map_or_else(
            |err| {
                // Reported rather than silently retried on this thread.
                tracing::warn!(error = %err, "could not start a provider probe");
                state.set_status(format!("could not start the provider request: {err}"));
            },
            |_handle| (),
        );
}

/// One probe, start to finish, with nothing that touches the terminal.
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
                // told about survives a restart. A write failure is reported
                // as a failed refresh rather than swallowed.
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
    // Every result waiting, not just the first — two providers can have
    // requests outstanding at once.
    while let Ok(result) = inbox.try_recv() {
        if state.apply_provider_probe_result(result) == Action::Redraw {
            redraw = true;
        }
    }
    redraw
}

/// Delete the selected provider's stored credential — line 3.
/// Both halves, and both reported: the item leaves the OS store, and the
/// reference leaves the provider's configuration. Deleting one that is not
/// there is **not** an error — see
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

    // Runs regardless: a reference to a missing credential should still go.
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
/// never touched exactly as it was — this is what keeps a save from silently
/// promoting a value the user never actually changed.
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
/// already carries a complete [`config::ProviderConfig`]: a provider edit
/// produces or removes the whole value, never one field of it.
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
    // Phase 9I line 536: the pin is a double `Option` because "no pin" is a
    // state a user can choose explicitly, unlike every preference above it.
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
/// consent it requires comes from the Settings overlay's own `W`
/// confirmation, before [`Action::SaveProjectSettings`] is produced. Returns
/// the path written, for the status line.
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
#[path = "tests/mod_tests.rs"]
mod tests;
