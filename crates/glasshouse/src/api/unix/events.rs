use std::sync::Mutex;

use glasshouse::Runtime;
use glasshouse::events::{
    EventLog, GatewayFailure, LifecycleEvent, LoggedEvent, MessageOrigin, TurnOutcome,
};
use glasshouse::guardrails::{AssumptionStore, TransitionKind};
use glasshouse::session::api::{ApiError, SessionApi};
use glasshouse::session::{ProjectSessions, SessionId, SessionRuntime, SessionStore};

use super::assumptions::{guardrail_error_message, notification_json};
use super::{EventRecorder, api_error, lock};
use crate::api::protocol::Response;

/// The hard ceiling on how many events [`Request::Events`] returns in one
/// call, regardless of the `limit` a caller asks for — box 701's "bounded
/// output" requirement. A caller that has fallen behind by more than this
/// many events gets a `head` past what it can see in this response and
/// polls again rather than pulling the whole table in one line of JSON.
const MAX_EVENTS_LIMIT: usize = 1000;

/// This project's lifecycle events, harness-independent — capability map
/// line 701.
///
/// Incremental: `after` is the log position the caller has already consumed,
/// and `head` — the log's current position, returned even when `events` is
/// empty — is what it hands back next time. `limit` is capped at
/// [`MAX_EVENTS_LIMIT`] regardless of what is asked for.
///
/// Reads [`EventLog::since`], not `observed_since`: the caller is in another
/// process, so the harness-report filter that avoids double-counting an
/// in-process [`EventBus`] subscriber would instead delete every spawn,
/// intervention and exit no other process witnessed — see [`EventRecorder`].
///
/// Flushes first because recording is asynchronous ([`EventRecorder`]); the
/// wait is bounded and its failure ignored, so a slow writer only makes this
/// answer older, never absent — the caller's cursor brings it back next call.
// History: design-decisions.md, "Trims: api, events, harness and config module docs, second packet", crates/glasshouse/src/api/unix/events.rs `project_events`.
pub(super) fn project_events(
    runtime: &Runtime,
    after: i64,
    limit: usize,
    assumptions_after: i64,
    recorder: &EventRecorder,
) -> Response {
    recorder.flush();
    let bounded_limit = limit.min(MAX_EVENTS_LIMIT);

    // Two ledgers, one handle at a time (practice §65): the event log is
    // read and dropped before the assumption ledger is opened.
    let (events, head) = {
        let log = match EventLog::open(runtime) {
            Ok(log) => log,
            Err(err) => return Response::err(err.to_string()),
        };
        let events = match log.since(after, bounded_limit) {
            Ok(events) => events,
            Err(err) => return Response::err(err.to_string()),
        };
        let head = match log.head() {
            Ok(head) => head,
            Err(err) => return Response::err(err.to_string()),
        };
        (events, head)
    };

    // Phase 21K line 1050: a refuted premise and an exceeded budget ride
    // this verb rather than a new `lifecycle_events` kind — the design
    // ruling's fifth point, because that table's `CHECK` costs a rebuild
    // per value. Its own cursor, its own head.
    let (assumptions, assumptions_head) = {
        let ledger = match AssumptionStore::open(runtime) {
            Ok(ledger) => ledger,
            Err(err) => return Response::err(guardrail_error_message(&err)),
        };
        let notifications = match ledger.notifications_since(assumptions_after, bounded_limit) {
            Ok(notifications) => notifications,
            Err(err) => return Response::err(err.to_string()),
        };
        let head = match ledger.head() {
            Ok(head) => head,
            Err(err) => return Response::err(err.to_string()),
        };
        (notifications, head)
    };

    Response::ok(serde_json::json!({
        "events": events.iter().map(event_json).collect::<Vec<_>>(),
        "head": head,
        "assumptions": assumptions.iter().map(notification_json).collect::<Vec<_>>(),
        "assumptions_head": assumptions_head,
    }))
}

/// One logged event, translated for the door.
///
/// `kind` is [`LifecycleEvent::kind`]'s own word, never the harness's —
/// that is what capability map line 701 asks for. The harness that reported
/// it, when one did, appears only as the `harness` attribute; the harness's
/// own raw spelling of the event ([`glasshouse::events::Observation::event`])
/// and every hook payload field stay behind this door, matching the
/// guarantee `tests/session_hook.rs` already holds for the project database
/// itself — this handler exposes translated events, not raw adapter
/// observations. Every payload field a kind does not carry is `null`, never
/// `0` or `""` (§71).
fn event_json(logged: &LoggedEvent) -> serde_json::Value {
    let mut fields = serde_json::Map::new();
    fields.insert("seq".to_owned(), serde_json::json!(logged.seq));
    fields.insert(
        "session".to_owned(),
        serde_json::json!(logged.session.as_str()),
    );
    fields.insert("at".to_owned(), serde_json::json!(logged.at));
    fields.insert("kind".to_owned(), serde_json::json!(logged.event.kind()));
    fields.insert(
        "harness".to_owned(),
        serde_json::json!(logged.observed.as_ref().map(|o| o.harness.as_str())),
    );
    for key in [
        "outcome",
        "origin",
        "bytes",
        "exit_code",
        "exit_signal",
        "resource",
        "reason",
        "provider",
        "model",
        "cause",
    ] {
        fields.insert(key.to_owned(), serde_json::Value::Null);
    }

    match &logged.event {
        LifecycleEvent::TurnEnded { outcome } => {
            fields.insert(
                "outcome".to_owned(),
                serde_json::json!(turn_outcome_str(*outcome)),
            );
        }
        LifecycleEvent::TextDelivered { origin, bytes } => {
            fields.insert(
                "origin".to_owned(),
                serde_json::json!(message_origin_str(*origin)),
            );
            fields.insert("bytes".to_owned(), serde_json::json!(bytes));
        }
        LifecycleEvent::InterruptDelivered { origin } => {
            fields.insert(
                "origin".to_owned(),
                serde_json::json!(message_origin_str(*origin)),
            );
        }
        LifecycleEvent::ProcessExited { exit } => {
            fields.insert("exit_code".to_owned(), serde_json::json!(exit.code()));
            fields.insert("exit_signal".to_owned(), serde_json::json!(exit.signal()));
        }
        LifecycleEvent::GatewayUnhealthy { resource, reason } => {
            fields.insert("resource".to_owned(), serde_json::json!(resource));
            fields.insert(
                "reason".to_owned(),
                serde_json::json!(gateway_failure_str(*reason)),
            );
        }
        LifecycleEvent::GatewayBackendChanged {
            provider,
            model,
            cause,
        } => {
            fields.insert("provider".to_owned(), serde_json::json!(provider));
            fields.insert("model".to_owned(), serde_json::json!(model));
            fields.insert("cause".to_owned(), serde_json::json!(cause));
        }
        // Migration 26. The path is already repo-relative — the writer put
        // it through `normalize_observed_path` before the event existed — so
        // this puts a file name on the wire and never the project's absolute
        // location.
        LifecycleEvent::FileTouched { path } => {
            fields.insert("path".to_owned(), serde_json::json!(path));
        }
        LifecycleEvent::SessionStarted
        | LifecycleEvent::SessionResumed
        | LifecycleEvent::TurnStarted
        | LifecycleEvent::WaitingForUser
        | LifecycleEvent::OutputEnded => {}
    }

    serde_json::Value::Object(fields)
}

// ---------------------------------------------------------------------------
// The orchestrator wake-up flow — capability map Phase 15, lines 733-739.
// ---------------------------------------------------------------------------

/// How many log rows one pump reads. Bounded for the same reason
/// [`MAX_EVENTS_LIMIT`] is: a watch that has fallen a long way behind must
/// catch up over several ticks rather than pull the whole table into one
/// pass, and the cursor makes the next tick resume exactly where this one
/// stopped.
const WATCH_PUMP_LIMIT: usize = 256;

/// How many event kinds a completion summary names before it elides.
const SUMMARY_KINDS: usize = 6;

/// The one thing an orchestrator learns without asking — line 736's
/// "machine-originated message" — and therefore the one place this door
/// speaks first.
///
/// # This is a statement about the past, never about the present
///
/// Every field is read from one row of the durable event log and the row is
/// named by its own `seq`. That is deliberate and it is the whole design
/// ruling behind lines 740 and 748: an orchestrator woken by this knows that
/// *at log position `seq`* a harness reported a turn ending, and knows
/// nothing whatever about the session **now**. Anyone — a person, another
/// orchestrator, the harness itself — may have moved the session in the
/// meantime, and the only way to find out is to ask again
/// ([`Request::SessionState`], line 738).
///
/// A notification that carried a live state would be a lie with a timestamp
/// on it, and it would make the orchestrator's picture authoritative over
/// the user's, which is exactly backwards.
struct Completion {
    worker: SessionId,
    harness: Option<String>,
    outcome: TurnOutcome,
    seq: i64,
    at: i64,
    summary: String,
    /// Phase 21K line 1050: what the worker's assumption ledger recorded
    /// since this watch last delivered — how many premises were refuted, how
    /// many budgets exceeded, and the assumption identifiers. **Identifiers
    /// and counts only**: a claim is untrusted text and this line is typed
    /// into another agent's terminal, so the orchestrator that wants the
    /// words asks `list_assumptions` for them, labelled. `None` when the
    /// ledger could not be read, never a zero that means the same as
    /// nothing happened.
    assumptions: Option<AssumptionSummary>,
}

/// The counts a completion line carries — see [`Completion::assumptions`].
struct AssumptionSummary {
    refuted: usize,
    budget_exceeded: usize,
    ids: Vec<String>,
}

impl Completion {
    /// One line, and one line only.
    ///
    /// [`SessionApi::send_text`] appends the carriage return that submits
    /// this to the harness's line editor, so an embedded newline here would
    /// submit half a notification and leave the rest as the start of the
    /// next message. Every field below is either an integer, a session
    /// identifier, a harness slug, or [`Completion::summary`] — see
    /// [`summarize`] for why none of them can contain one.
    fn line(&self) -> String {
        let payload = serde_json::json!({
            "worker": self.worker.as_str(),
            "harness": self.harness,
            "outcome": turn_outcome_str(self.outcome),
            "seq": self.seq,
            "at": self.at,
            "summary": self.summary,
            "assumptions": self.assumptions.as_ref().map(|summary| serde_json::json!({
                "refuted": summary.refuted,
                "budget_exceeded": summary.budget_exceeded,
                "ids": summary.ids,
            })),
        });
        format!("glasshouse worker-completion {payload}")
    }
}

/// One registered interest — line 733.
///
/// `cursor` is the log position this watch has already consumed. It is the
/// entire dedup mechanism (line 739): a row is read exactly once, because
/// the cursor advances past **every** row the pump saw, matched or not, and
/// the log assigns `seq` monotonically from the database rather than from
/// any one process's counter.
pub(super) struct Watch {
    worker: SessionId,
    notify: SessionId,
    cursor: i64,
    /// The assumption ledger's position this watch has already delivered
    /// — the same dedup mechanism as `cursor`, over the other ledger.
    assumption_cursor: i64,
}

/// Every registered interest this door is currently holding.
///
/// # Lock order: `watches` before `live`, never the reverse
///
/// Two paths take both — [`watch_worker`], which validates a registration
/// against the runtime, and [`pump_watches`], which delivers through it.
/// Taking them in opposite orders is a deadlock that would strand the whole
/// door, and it is a deadlock that compiles and passes every test that does
/// not happen to interleave a registration with a delivery. So the order is
/// fixed here and stated at both sites.
pub(super) type Watches = Mutex<Vec<Watch>>;

/// Register interest in a worker's completions — line 733.
///
/// Both identifiers go through [`SessionApi`], so both are refused unless
/// they belong to this project: a watch is a standing instruction to type
/// into a session, and the one thing this door may never do is type into
/// another project's.
///
/// Three refusals, each because the alternative is worse than an error:
///
/// - **`notify` is not live in this process.** The runtime this door owns is
///   the only one it can write to (see this module's own doc comment), so an
///   orchestrator that was not spawned through this door has no terminal
///   here. Registering anyway would produce a watch that silently never
///   fires — the exact failure `scripts/worker-watch.sh` produced when a
///   finished worker was lost because nothing was really watching it.
/// - **`session` is the same as `notify`.** A session watching itself types
///   its own completions into itself, which is a loop with a keyboard.
/// - **an unknown or foreign session**, from `SessionApi` itself.
pub(super) fn watch_worker(
    runtime: &Runtime,
    store: &SessionStore<'_>,
    live: &Mutex<SessionRuntime>,
    watches: &Watches,
    session: &str,
    notify: &str,
) -> Response {
    let worker = SessionId::new(session.to_owned());
    let notify = SessionId::new(notify.to_owned());

    if worker == notify {
        return Response::err(format!(
            "session `{worker}` cannot be watched on its own behalf: a completion \
             would be typed back into the session that produced it"
        ));
    }

    // Lock order: `watches` first, then `live`. See [`Watches`].
    let mut registry = watches
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);

    {
        let mut guard = lock(live);
        let api = SessionApi::new(store, &mut guard);
        // `state` rather than `list`: it resolves through the same
        // project-scope check every other method starts with, and answers
        // for the one session asked about.
        if let Err(err) = api.state(&worker) {
            return Response::err(api_error(err));
        }
        if let Err(err) = api.state(&notify) {
            return Response::err(api_error(err));
        }
        if guard.get(&notify).is_none() {
            return Response::err(format!(
                "session `{notify}` is not live in this Glasshouse, so a completion \
                 notification would have nowhere to be delivered; an orchestrator \
                 must be a session this door holds"
            ));
        }
    }

    let from = match EventLog::open(runtime) {
        Ok(log) => match log.head() {
            Ok(head) => head,
            Err(err) => return Response::err(err.to_string()),
        },
        Err(err) => return Response::err(err.to_string()),
    };
    // The other ledger's head, read after the event log's handle is gone.
    // Nothing already recorded is replayed here either.
    let assumptions_from = match AssumptionStore::open(runtime) {
        Ok(ledger) => match ledger.head() {
            Ok(head) => head,
            Err(err) => return Response::err(err.to_string()),
        },
        Err(err) => return Response::err(guardrail_error_message(&err)),
    };

    // Idempotent per pair. A second registration replaces the first rather
    // than adding to it: two watches over one pair would deliver one
    // completion twice, which is precisely line 739's failure.
    if let Some(existing) = registry
        .iter_mut()
        .find(|watch| watch.worker == worker && watch.notify == notify)
    {
        existing.cursor = from;
        existing.assumption_cursor = assumptions_from;
    } else {
        registry.push(Watch {
            worker: worker.clone(),
            notify: notify.clone(),
            cursor: from,
            assumption_cursor: assumptions_from,
        });
    }

    Response::ok(serde_json::json!({
        "worker": worker.as_str(),
        "notify": notify.as_str(),
        "from": from,
        "assumptions_from": assumptions_from,
    }))
}

/// Whether any orchestrator has registered interest yet.
///
/// Peeked before anything is opened, so a door nobody is watching through
/// costs one uncontended mutex acquisition per tick and nothing else.
pub(super) fn watching(watches: &Watches) -> bool {
    !watches
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .is_empty()
}

/// The two database handles the wake-up pump reads through, opened **once**.
///
/// # Why this is not opened per tick
///
/// Both of these go through `database::open`, which does considerably more
/// than hand back a connection: it takes a `BEGIN IMMEDIATE` **write**
/// transaction and runs the migration ladder before returning. That is
/// exactly right once per process and badly wrong twenty times a second.
///
/// A pump that reopened on every 50ms tick would take SQLite's write lock
/// forty times a second for the whole life of the door — contending with the
/// very `glasshouse hook` processes that write the rows it is reading. Those
/// run *inside the user's own session*, and `report_hook`'s own doc comment
/// explains why a hook must never be made slow: Claude Code treats a hook's
/// failure as a veto, and the busy wait it would be pushed into is five
/// seconds. The door's bookkeeping is never more important than the session
/// it is keeping books about.
///
/// Opened lazily, on the first tick where a watch exists, so a door nobody
/// watches through holds no extra connection at all.
pub(super) struct WatchState {
    log: EventLog,
    sessions: ProjectSessions,
    /// For the assumption ledger, which is opened **per completion** rather
    /// than held: a completion is a per-turn event, not a per-tick one, so
    /// the argument above against reopening does not apply, and a third
    /// standing handle in this thread is not paid for.
    runtime: Runtime,
}

impl WatchState {
    pub(super) fn open(runtime: &Runtime) -> Option<Self> {
        let log = EventLog::open(runtime).ok()?;
        let sessions = ProjectSessions::open(runtime).ok()?;
        Some(Self {
            log,
            sessions,
            runtime: runtime.clone(),
        })
    }
}

/// Deliver any completion each watch has not yet seen — lines 734-737, 739.
///
/// Called from the door's own background tick: nothing outside `glasshouse
/// api serve` has to remember to call it.
///
/// Reads the log, not the bus: a turn ending is reported by the harness's
/// own lifecycle hook in a separate short-lived process (`glasshouse hook
/// <session> Stop`), and that row in the event log is the only place this
/// process can see it — the hook's process is gone before anyone could have
/// subscribed to anything.
///
/// The cursor advances past every observed row, not only the ones that
/// matched: `observed_since` returns every observed row for every worker, so
/// advancing only on a match would re-read the same unmatched rows forever.
/// Advancing past everything seen is what makes "read exactly once" a
/// property of the loop rather than of the filter.
// History: design-decisions.md, "Trims: api, events, harness and config module docs, second packet", crates/glasshouse/src/api/unix/events.rs `pump_watches`.
pub(super) fn pump_watches(state: &WatchState, live: &Mutex<SessionRuntime>, watches: &Watches) {
    // Lock order: `watches` first, then `live`. See [`Watches`].
    let mut registry = watches
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if registry.is_empty() {
        return;
    }

    let log = &state.log;
    let store = state.sessions.store();

    let mut dropped = Vec::new();
    for (index, watch) in registry.iter_mut().enumerate() {
        let rows = match log.observed_since(watch.cursor, WATCH_PUMP_LIMIT) {
            Ok(rows) => rows,
            Err(err) => {
                tracing::warn!(error = %err, "could not read the event log for a worker watch");
                continue;
            }
        };
        if rows.is_empty() {
            continue;
        }

        let mut completions = Vec::new();
        for row in &rows {
            watch.cursor = watch.cursor.max(row.seq);
            let LifecycleEvent::TurnEnded { outcome } = row.event else {
                continue;
            };
            if row.session != watch.worker {
                continue;
            }
            completions.push(Completion {
                worker: row.session.clone(),
                harness: row.observed.as_ref().map(|o| o.harness.clone()),
                outcome,
                seq: row.seq,
                at: row.at,
                summary: summarize(log, &row.session, row.seq, row.at),
                assumptions: None,
            });
        }
        if completions.is_empty() {
            continue;
        }

        // Phase 21K line 1050: name what the worker's ledger recorded since
        // this watch last delivered. One read per batch of completions, on
        // a handle opened for it and dropped before the runtime lock below.
        if let Some((summary, head)) =
            assumption_summary(&state.runtime, &watch.worker, watch.assumption_cursor)
        {
            watch.assumption_cursor = head;
            if let Some(last) = completions.last_mut() {
                last.assumptions = Some(summary);
            }
            for completion in completions.iter_mut().rev().skip(1) {
                completion.assumptions = Some(AssumptionSummary {
                    refuted: 0,
                    budget_exceeded: 0,
                    ids: Vec::new(),
                });
            }
        }

        let mut guard = lock(live);
        let mut api = SessionApi::new(&store, &mut guard);
        for completion in completions {
            // `MessageOrigin::Machine`, stated here rather than assumed
            // from the seam — line 736's "machine-originated message".
            // Glasshouse woke this orchestrator on its own initiative and no
            // request is involved, so there is no origin to carry and none to
            // be tempted by: this is the one delivery on this door that is
            // unambiguously not a person's, and it is pinned by
            // `tests/worker_access.rs` in both of the tests that watch a
            // handoff. There is no second write path into a session,
            // deliberately.
            match api.send_text(&watch.notify, &completion.line(), MessageOrigin::Machine) {
                Ok(()) => tracing::info!(
                    worker = %completion.worker,
                    notify = %watch.notify,
                    seq = completion.seq,
                    "delivered a worker completion to an orchestrator"
                ),
                // The orchestrator's own session ended. A watch that can
                // never be delivered again is dropped rather than retried
                // every tick for the life of the process.
                Err(ApiError::NotLive { .. }) | Err(ApiError::NotFound { .. }) => {
                    tracing::warn!(
                        notify = %watch.notify,
                        "dropping a worker watch: the session to notify is gone"
                    );
                    dropped.push(index);
                    break;
                }
                // Line 1719: a person is typing into the orchestrator right
                // now. This is the one delivery on this door with no caller
                // to refuse to, so it is **deferred** rather than refused —
                // the cursor is wound back to before this completion and the
                // next tick tries again, a few milliseconds later.
                //
                // Winding the cursor back is the whole of it, and it has to
                // be exact: the collection loop above advances `cursor` past
                // every row it read, so without this the completion would be
                // dropped for the life of the watch and an orchestrator would
                // wait forever for a worker that had already finished. Only
                // this completion and what follows it are re-read; the
                // completions already delivered in this batch have lower
                // sequence numbers and stay behind the cursor.
                Err(ApiError::UserHasTheKeyboard { .. }) => {
                    tracing::debug!(
                        notify = %watch.notify,
                        seq = completion.seq,
                        "holding a worker completion: a person is using the session to notify"
                    );
                    watch.cursor = completion.seq.saturating_sub(1);
                    break;
                }
                Err(err) => tracing::warn!(
                    notify = %watch.notify,
                    error = %err,
                    "could not deliver a worker completion"
                ),
            }
        }
    }

    for index in dropped.into_iter().rev() {
        registry.remove(index);
    }
}

/// The worker's refutations and exceeded budgets since `after`, and the
/// ledger's head to advance the watch to. `None` when the ledger cannot be
/// read — logged, and the completion says `null` rather than `0`.
fn assumption_summary(
    runtime: &Runtime,
    worker: &SessionId,
    after: i64,
) -> Option<(AssumptionSummary, i64)> {
    let ledger = match AssumptionStore::open(runtime) {
        Ok(ledger) => ledger,
        Err(err) => {
            tracing::warn!(error = %err, "could not open the assumption ledger for a worker watch");
            return None;
        }
    };
    let head = ledger.head().ok()?;
    let notifications = match ledger.notifications_for_session_since(
        worker.as_str(),
        after,
        WATCH_PUMP_LIMIT,
    ) {
        Ok(notifications) => notifications,
        Err(err) => {
            tracing::warn!(error = %err, "could not read the assumption ledger for a worker watch");
            return None;
        }
    };
    let mut summary = AssumptionSummary {
        refuted: 0,
        budget_exceeded: 0,
        ids: Vec::new(),
    };
    for notification in &notifications {
        match notification.transition.kind {
            TransitionKind::BudgetExceeded => summary.budget_exceeded += 1,
            _ => summary.refuted += 1,
        }
        if let Some(id) = &notification.transition.assumption_id {
            summary.ids.push(id.as_str().to_owned());
        }
    }
    Some((summary, head))
}

/// What Glasshouse actually observed about the turn that just ended — line
/// 737's "concise result summary".
///
/// Built only from [`LifecycleEvent::kind`]'s own words (a `&'static str`
/// from the eleven the enum defines) joined with an arrow, plus one integer
/// — no value read out of a hook payload, a session's scrollback, or a
/// harness's own event spelling can reach this string, because none of
/// those is in the type it is built from. A summary quoting a worker's
/// output would breach the boundary `tests/session_hook.rs` holds for the
/// project database.
///
/// It says the shape of the turn and how long it took —
/// `turn_started → waiting_for_user → turn_ended in 41s` — never what the
/// worker did, produced, or concluded; Glasshouse does not observe that
/// anywhere. An orchestrator that needs the result asks the worker (line
/// 738) or reads a checkpoint.
// History: design-decisions.md, "Trims: api, events, harness and config module docs, second packet", crates/glasshouse/src/api/unix/events.rs `summarize`.
fn summarize(log: &EventLog, session: &SessionId, seq: i64, at: i64) -> String {
    let history = match log.recent_for_session(session, SUMMARY_KINDS * 4) {
        Ok(history) => history,
        Err(err) => {
            tracing::warn!(session = %session, error = %err, "could not summarize a turn");
            return "no observed history for this turn".to_owned();
        }
    };

    // The turn is what happened after the previous `turn_ended` — the
    // harness's own boundary, not a guess at one.
    let start = history
        .iter()
        .rposition(|row| row.seq < seq && matches!(row.event, LifecycleEvent::TurnEnded { .. }))
        .map(|index| index + 1)
        .unwrap_or(0);
    let turn: Vec<&LoggedEvent> = history[start..]
        .iter()
        .filter(|row| row.seq <= seq)
        .collect();

    let elapsed = turn.first().map(|first| at - first.at).unwrap_or(0).max(0);

    let mut kinds: Vec<&'static str> = turn.iter().map(|row| row.event.kind()).collect();
    let elided = kinds.len().saturating_sub(SUMMARY_KINDS);
    if elided > 0 {
        // Keep the end of the turn, which is the part that says how it went.
        kinds.drain(..elided);
    }
    let shape = kinds.join(" → ");
    let shape = if elided > 0 {
        format!("… ({elided} earlier) → {shape}")
    } else {
        shape
    };

    format!("{shape} in {elapsed}s")
}

/// Matches `events::log`'s own private `outcome_sql` spelling, duplicated
/// rather than imported because that one is private to its own module (same
/// reasoning as [`describe_layer`]).
fn turn_outcome_str(outcome: TurnOutcome) -> &'static str {
    match outcome {
        TurnOutcome::Completed => "completed",
        TurnOutcome::Failed => "failed",
    }
}

/// Matches `events::log`'s own private `origin_sql` spelling.
fn message_origin_str(origin: MessageOrigin) -> &'static str {
    match origin {
        MessageOrigin::UserKeystroke => "user_keystroke",
        MessageOrigin::Machine => "machine",
    }
}

/// Matches `events::log`'s own private `gateway_reason_sql` spelling.
fn gateway_failure_str(reason: GatewayFailure) -> &'static str {
    match reason {
        GatewayFailure::Unreachable => "unreachable",
        GatewayFailure::TimedOut => "timed_out",
        GatewayFailure::Rejected => "rejected",
    }
}
