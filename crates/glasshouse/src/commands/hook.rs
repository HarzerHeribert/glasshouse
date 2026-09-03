//! `commands::hook` -- moved verbatim from `main.rs` (Phase 59 decomposition).

use glasshouse::Runtime;
use glasshouse::checkpoint::git::GitPosition;
use glasshouse::checkpoint::{Checkpoint, CheckpointReason, ProjectCheckpoints};
use glasshouse::config::{self, EffectiveConfig, UserConfig};
use glasshouse::events::{LifecycleEvent, MessageOrigin, Observation, TurnOutcome};
use glasshouse::session;
use glasshouse::session::api::SessionApi;
use glasshouse::session::{ProjectSessions, SessionId, SessionRuntime};

/// Record a lifecycle event a harness reported about one of its sessions.
///
/// # This function may never fail
///
/// It is run *by the harness*, inside the user's session, and Claude Code
/// treats a hook's non-zero exit as a veto: a `UserPromptSubmit` hook that
/// exits non-zero blocks the prompt outright, with the user's own words
/// echoed back at them and nothing sent. That was observed directly, not
/// assumed.
///
/// So every failure here is swallowed into the log. A database that cannot be
/// opened, a session that is not in it, an event nobody recognises — none of
/// them is worth costing the user a turn. Glasshouse's bookkeeping is never
/// more important than the session it is keeping books about.
pub(crate) fn report_hook(runtime: &Runtime, session: &str, event: &str) {
    report_hook_with(runtime, session, event, |id| {
        crate::commands::routing_classification::disposable_extraction_model(runtime, id)
    });
}

/// Phase 21/49: whether the automatic post-turn memory-extraction trigger may
/// run for this project — see
/// [`glasshouse::config::EffectiveConfig::memory_extraction_enabled`].
///
/// A configuration Glasshouse cannot read defaults to enabled, matching every
/// other read failure on this path: [`disposable_extraction_model`] falls
/// back the same way, for the same reason — a broken config file must not
/// silently and permanently turn off a working capability, and this trigger
/// already tolerates every other failure non-fatally (see
/// [`run_extraction`]'s own doc comment).
fn memory_extraction_enabled(runtime: &Runtime) -> bool {
    let Ok(user) = UserConfig::load(runtime.paths()) else {
        return true;
    };
    let project = config::load_project_config(runtime.project()).unwrap_or(None);
    EffectiveConfig::new(&user, project.as_ref())
        .memory_extraction_enabled()
        .value
}

/// Phase 19: whether Glasshouse may take a checkpoint automatically at a
/// task boundary — see
/// [`glasshouse::config::EffectiveConfig::automatic_checkpoint_enabled`].
///
/// A configuration Glasshouse cannot read defaults to enabled, matching
/// [`memory_extraction_enabled`]'s own fallback and for the same reason: a
/// broken config file must not silently and permanently turn off a working
/// capability, and this trigger already tolerates every other failure
/// non-fatally (see [`checkpoint_after_turn`]'s own doc comment).
fn automatic_checkpoint_enabled(runtime: &Runtime) -> bool {
    let Ok(user) = UserConfig::load(runtime.paths()) else {
        return true;
    };
    let project = config::load_project_config(runtime.project()).unwrap_or(None);
    EffectiveConfig::new(&user, project.as_ref())
        .automatic_checkpoint_enabled()
        .value
}

/// Take an automatic checkpoint for `id` at a task boundary, after a
/// completed turn.
///
/// # Nothing here can hurt the session
///
/// Matching [`run_extraction`]'s own policy for its neighbour: a
/// checkpoint that cannot be taken is logged and this returns. It never
/// propagates an error to [`report_hook_with`] and never blocks past a
/// synchronous read of a couple of small files and one write — there is no
/// model call here, so there is nothing to bound with a thread and a
/// timeout the way extraction needs.
///
/// # What it carries forward
///
/// A checkpoint's objective, state and next actions are authored —
/// Glasshouse does not know them and will not guess them from a session's
/// terminal output, for the same reason nothing else in this codebase reads
/// state out of scrollback. So this carries forward the handoff from the
/// session's most recent checkpoint, restamped with the current time and the
/// repository's current position — the same shape
/// `shell::checkpoint_task_boundaries` already uses in the interactive shell,
/// for the same reason. A session that has never had a checkpoint taken gets
/// nothing here, silently: there is no handoff to carry forward and nothing
/// honest to invent.
fn checkpoint_after_turn(runtime: &Runtime, id: &SessionId, harness: &str) {
    let outcome = (|| -> anyhow::Result<()> {
        let checkpoints = ProjectCheckpoints::open(runtime)?;
        let store = checkpoints.store();
        let Some(previous) = store.latest_for(id)? else {
            return Ok(());
        };
        let refreshed = Checkpoint::capture(
            id,
            harness,
            CheckpointReason::TaskBoundary,
            store.now(),
            runtime.project().root(),
            previous.checkpoint.handoff.clone(),
        );
        store.save(refreshed)?;
        Ok(())
    })();

    if let Err(err) = outcome {
        tracing::warn!(
            session = %id,
            error = %format!("{err:#}"),
            "could not take an automatic checkpoint"
        );
    }
}

/// Map line 1171: refresh a session's portable checkpoint just before the
/// harness compacts its own context, so the handoff a fresh window would
/// bootstrap from reflects where the repository actually stands rather than
/// wherever it stood at the last completed turn.
///
/// # Refresh, not a new kind of checkpoint
///
/// This mirrors [`checkpoint_after_turn`] in every respect but one: it
/// preserves `previous.checkpoint.reason` instead of stamping
/// [`CheckpointReason::TaskBoundary`]. A compaction is not a turn ending, so
/// stamping `TaskBoundary` would misdescribe why the checkpoint exists — and
/// `CheckpointReason` has exactly two variants, both pinned by a SQL `CHECK`,
/// so there is no third value honest enough to invent instead. What moves is
/// `created_at` and the Git position; the reason a person or agent already
/// gave the checkpoint does not change because the harness is about to
/// compact.
///
/// # `store.latest_for(id)?` returning `None` is the whole of "when practical"
///
/// A session that has never had a checkpoint taken gets nothing here,
/// silently — there is no previous handoff to carry forward and nothing
/// honest to invent, exactly as [`checkpoint_after_turn`] already declines.
///
/// # Nothing here can hurt the session
///
/// Same stance as its neighbour: a checkpoint that cannot be refreshed is
/// logged and this returns, never propagating an error back to the hook that
/// is running inside somebody's coding session.
fn checkpoint_before_compaction(runtime: &Runtime, id: &SessionId, harness: &str) {
    let outcome = (|| -> anyhow::Result<()> {
        let checkpoints = ProjectCheckpoints::open(runtime)?;
        let store = checkpoints.store();
        let Some(previous) = store.latest_for(id)? else {
            return Ok(());
        };
        let refreshed = Checkpoint::capture(
            id,
            harness,
            previous.checkpoint.reason,
            store.now(),
            runtime.project().root(),
            previous.checkpoint.handoff.clone(),
        );
        store.save(refreshed)?;
        Ok(())
    })();

    if let Err(err) = outcome {
        tracing::warn!(
            session = %id,
            error = %format!("{err:#}"),
            "could not refresh the checkpoint before compaction"
        );
    }
}

/// How long a hook process will wait for the harness to finish writing the
/// payload it is about to throw away.
///
/// # Why draining the payload needs a bound at all
///
/// [`report_hook_with`] drains its standard input so that a harness writing a
/// payload is not left writing into a closed pipe. Copying *to end of input*
/// is an **unbounded** wait, and the harness is the thing that decides when
/// that end arrives. A harness that writes nothing and never closes the pipe
/// parks this process there for as long as it lives — inside the user's
/// session, on the event Claude Code treats as a gate on the turn. That is
/// exactly what [`report_hook`]'s own doc comment says may never happen here.
///
/// Not hypothetical, and not Windows-specific either, though Windows is where
/// it was found: reached over an `ssh` channel whose far end never sees end of
/// input — which is how the local gate's Windows leg runs the suite, and which
/// its macOS leg avoids only because that one redirects from `/dev/null` — the
/// six tests that call this function block for ever, and every other test in
/// the target passes. Measured on both batch 50 and its own base commit, so
/// the wait is older than the batch that surfaced it.
///
/// # Why one second
///
/// Shorter than [`EXTRACTION_BOUND`] because there is far less on the other
/// side of it. The harness writes the payload as it starts this process, so a
/// live harness is finished before the first database is even open and the
/// normal cost of this wait is nothing at all. Any wait that reaches the bound
/// is already the pathological case, and the answer to it is to get on with
/// the bookkeeping rather than to keep waiting.
const PAYLOAD_DRAIN_BOUND: std::time::Duration = std::time::Duration::from_secs(1);

/// Run `work` on its own thread and stop waiting for it after `bound`,
/// reporting whether it finished in time.
///
/// # The abandoned thread is deliberate
///
/// Nothing here can stop a thread parked in a blocking read, and stopping it
/// is not the point: the point is that *this* thread may go on without it. The
/// work is left running, the process finishes what it was doing and exits, and
/// the operating system closes whatever handle the thread was waiting on.
///
/// [`run_extraction`] does the same thing by hand rather than
/// through this, because it needs the extraction's *outcome* back and not
/// merely the fact that it arrived.
pub(crate) fn abandon_after(
    bound: std::time::Duration,
    work: impl FnOnce() + Send + 'static,
) -> bool {
    let (finished, waiter) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        work();
        // A closed receiver means the bound expired and nobody is listening.
        // That is a normal outcome here, not an error.
        let _ = finished.send(());
    });
    waiter.recv_timeout(bound).is_ok()
}

/// [`report_hook`] with the extraction model supplied.
///
/// The model is the one thing on this path that does not exist yet — Phase 39
/// owns the provider interface, and [`NoExtractionModel`] is what production
/// passes until it does. Everything else here *is* the production path:
/// the session lookup, the translation, the event record, the state change
/// and the extraction call are all the shipped code, which is why the seam is
/// here and not one level up.
///
/// A factory rather than a reference, because extraction runs on its own
/// thread and needs something it can own.
///
/// It takes the session's *resolved* identifier because the routing decision
/// behind the model depends on it: capability map line 1290 lets the user
/// override protected-reserve protection for one named session, and
/// [`disposable_extraction_model`] can only honour that for the session it is
/// deciding for. `session` above is whatever the harness put on the command
/// line; the resolved id is what the user's configuration records.
pub(crate) fn report_hook_with(
    runtime: &Runtime,
    session: &str,
    event: &str,
    model: impl Fn(&glasshouse::session::SessionId) -> Box<dyn glasshouse::memory::ExtractionModel>,
) {
    // Codex writes its payload to the hook's stdin, and a process that never
    // reads it can leave the harness writing into a closed pipe. Glasshouse
    // has the event name and the session identifier from its own argv, so
    // the payload is drained to EOF and thrown away, unread and unparsed —
    // never deserialized, logged, or stored. See
    // `the_hook_command_never_reads_its_payload` below, and the
    // `docs/product/design-decisions.md` section this function implements.
    //
    // On its own thread, and abandoned at `PAYLOAD_DRAIN_BOUND`, because the
    // end of that input is the harness's decision and this process may not
    // wait on it for ever. See the constant for what the unbounded version
    // did.
    let drained = abandon_after(PAYLOAD_DRAIN_BOUND, || {
        let _ = std::io::copy(&mut std::io::stdin(), &mut std::io::sink());
    });
    if !drained {
        tracing::debug!(
            bound_ms = PAYLOAD_DRAIN_BOUND.as_millis(),
            "the harness had not closed this hook's input; going on without it"
        );
    }

    let outcome = (|| -> anyhow::Result<()> {
        let sessions = ProjectSessions::open(runtime)?;
        let store = sessions.store();
        let id = store.resolve_id(session)?;
        let record = store
            .get(&id)?
            .ok_or_else(|| anyhow::anyhow!("session `{id}` is not in this project"))?;

        // `observe`, not `lifecycle_for`. Two things follow from that and
        // both are capability lines:
        //
        // It preserves the raw observation in the debug log before
        // translating, so a harness that gained an event between releases
        // leaves a line naming what arrived — which is the difference between
        // a five-minute fix and a bisect, and is why the line is written
        // whether or not the event is recognised.
        //
        // And the observation is exactly two words: the integration slug from
        // this session's own record, and the event name from Glasshouse's own
        // argv. **The payload is not among them and cannot become one.** The
        // stream carrying the user's prompt and the model's last message was
        // drained into `io::sink()` above, unread; nothing downstream of here
        // has it to leak. See `the_hook_command_never_reads_its_payload`.
        let Some(translated) = session::lifecycle::observe(&record.harness, event) else {
            // Phase 21: *allow memory extraction to run before or around
            // native prompt compaction.*
            //
            // A compaction is not a `SessionLifecycle` state and has no
            // `LifecycleEvent`, so it lands here — in the arm for events that
            // translate to nothing — rather than beside the completed-turn
            // trigger below. No `lifecycle_events` row is written for it and
            // none can be: its `kind` is a SQL `CHECK` and `database`'s house
            // rule refuses to widen one. See
            // `session::lifecycle::precedes_native_compaction`.
            //
            // Gated by the same `memory_extraction` switch as the post-turn
            // trigger, and deliberately so: a user who turned automatic
            // extraction off turned it off, not "off except when the harness
            // compacts".
            if session::lifecycle::precedes_native_compaction(event) {
                // Capability map line 1159 — *"track the number of observed
                // compactions for a session when known"* — and this is the
                // only place in the shipped binary that knows one is coming.
                //
                // **Outside the `memory_extraction` gate, deliberately.**
                // That switch decides whether Glasshouse *does* something
                // about a compaction; the compaction happened either way, and
                // a count that silently stopped when a user turned extraction
                // off would be a number no reader could trust. It is also
                // ordered first, so a count is recorded even if extraction
                // takes the full `EXTRACTION_BOUND` and this process is torn
                // down by the harness while waiting.
                //
                // Best-effort: a compaction is the harness's business and a
                // hook that failed to write a counter must not fail the turn
                // over it, which is the same stance every other write on this
                // path takes.
                //
                // Gated on liveness for the reason `session::lifecycle::may_apply`
                // gates every lifecycle transition: a hook process outlives its
                // harness, and a `PreCompact` report arriving after the session
                // is recorded as finished must not move the record either —
                // `record_observed_compaction` itself has no such check (it is
                // an unconditional `UPDATE ... WHERE id = ?1`, by design, so a
                // session created before migration 16 still gets counted), so
                // the check belongs at this call site, the same way `may_apply`
                // belongs at the lifecycle-event call site below rather than
                // inside the write it guards.
                if record.lifecycle.is_live()
                    && let Err(err) = store.record_observed_compaction(&id)
                {
                    tracing::debug!(
                        error = %err,
                        session = %id,
                        "could not count an observed compaction"
                    );
                }
                if memory_extraction_enabled(runtime) {
                    // `hook_extraction`, not `run_extraction`: this is the
                    // trigger line 1174 is about, and a compaction that
                    // recorded nothing must say so where the person can read
                    // it rather than into a log that is off.
                    crate::commands::memory_extraction::hook_extraction(
                        runtime,
                        &id,
                        model(&id),
                        glasshouse::memory::ExtractionTrigger::BeforeCompaction,
                    );
                }
                // Map line 1171 — *"prefer creating or refreshing a portable
                // checkpoint before intentional compaction when practical"*.
                // Gated by `automatic_checkpoint`, the same independent
                // switch `checkpoint_after_turn` answers to below, and
                // deliberately **not** `memory_extraction`: checkpoints and
                // extraction are separate capabilities and turning one off
                // must leave the other exactly as it was.
                if automatic_checkpoint_enabled(runtime) {
                    checkpoint_before_compaction(runtime, &id, &record.harness);
                }
                return Ok(());
            }
            // An event this build does not recognise. Harnesses gain events
            // between releases, and guessing a state from an unfamiliar name
            // would be worse than ignoring it.
            tracing::debug!(event, "ignoring an unrecognised harness event");
            return Ok(());
        };

        // Phase 12's "record every translated lifecycle event with session ID
        // and timestamp", and Phase 18's "record lifecycle-hook events".
        // Recorded before the state change is decided, and independently of
        // whether one is applied at all: an event that arrived after the
        // session finished is still something that happened, and a log that
        // dropped it would be missing exactly the evidence somebody debugging
        // a late hook needs.
        crate::commands::resume::EventRecorder::open(runtime).record_observed(
            &id,
            translated.clone(),
            Observation::new(&record.harness, event),
        );

        // Phase 21: *allow memory extraction to run after task completion.*
        // Phase 19: *allow Glasshouse to request a checkpoint automatically
        // at selected task boundaries.*
        //
        // This is the one place a harness tells Glasshouse that a task
        // finished, and `TurnEnded { Completed }` is the only event that
        // carries that claim — `session::lifecycle::event_for` is its single
        // construction site, and a source-scanning test fails if a second one
        // appears. So this is where both triggers belong.
        //
        // Ordered **after** the event is recorded, on purpose: the log is the
        // material extraction reads, and a turn's own closing event should be
        // in it. Ordered **before** the state change for no reason at all
        // beyond it reading better; neither `run_extraction` nor
        // `checkpoint_after_turn` can fail in a way the rest of this function
        // could notice.
        //
        // The two triggers are gated independently — `memory_extraction` and
        // `automatic_checkpoint` are separate config fields, read by separate
        // `EffectiveConfig` methods — so turning one off leaves the other
        // exactly as it was.
        // Map lines 1834, 1835, 1845 and 1854's outcome half — and the whole
        // of what Glasshouse is allowed to learn about how a route turned
        // out. `TurnEnded` is the only event that carries a harness's own
        // verdict, `session::lifecycle::event_for` is its single construction
        // site, and **both** outcomes are recorded: a turn that ended badly
        // is a fact about the route as much as one that succeeded, and
        // counting only completions would make every ratio here a fraction of
        // an unstated denominator.
        //
        // A `SessionEnd`, a process exit and output going quiet all arrive
        // somewhere else or nowhere, and none of them writes a row. The
        // decision they belong to simply stays *unknown*, which is what the
        // readers count it as.
        //
        // Ordered **before** the extraction and checkpoint triggers below,
        // for the reason the compaction counter above is ordered first: those
        // run on their own thread up to `EXTRACTION_BOUND`, and this process
        // can be torn down by the harness while one is still going. A verdict
        // the harness actually stated must not be lost to work Glasshouse
        // chose to do about it.
        //
        // Map lines 1821 and 1831's proxy denominator — a second row, on
        // every session this arm reaches rather than only routed ones.
        // `record_routing_outcome` refuses a session with no routed
        // destination, so a door-spawned session (never routed) would
        // otherwise record nothing about how its turn went; `record_turn_outcome`
        // asks no routing question at all. Called first, so a session with no
        // routing decision still gets its outcome counted before the routed
        // call below returns early for it. Refusal register, *"Phase 51's
        // memory proxy — 1821 and 1831"*, ruling (b).
        if let LifecycleEvent::TurnEnded { outcome } = translated {
            // Map line 2393 — *"release a session's file claim automatically
            // when the relevant turn completes."* This is the only place in
            // the shipped binary that learns a turn ended, and a claim is
            // scoped to a turn, so this is where the release belongs.
            //
            // **Both outcomes release.** `TurnOutcome` is the harness's
            // verdict on its own turn; a turn that ended badly is a turn that
            // finished, and a claim outliving it would describe work nobody
            // is doing.
            //
            // Ordered first in this arm, ahead of the evaluation writes and
            // well ahead of extraction: it is one `DELETE`, and it is the one
            // write here that another *session* can observe. Extraction runs
            // on its own thread up to `EXTRACTION_BOUND` and this process can
            // be torn down by the harness while it does, which must not cost
            // a claim its release.
            //
            // Best-effort, like every other write on this path: a hook that
            // failed to release a claim must not fail the user's turn over
            // it, and `STALE_CLAIM_AFTER` is what bounds a claim this line
            // missed.
            match store.release_claims_of(&id) {
                Ok(0) => {}
                Ok(released) => {
                    tracing::debug!(session = %id, released, "released this turn's file claims");
                }
                Err(err) => tracing::debug!(
                    error = %err,
                    session = %id,
                    "could not release this session's file claims"
                ),
            }

            glasshouse::evaluation::record_turn_outcome(
                runtime,
                id.as_str(),
                outcome,
                glasshouse::evaluation::now_unix(),
            );
            glasshouse::evaluation::record_routing_outcome(
                runtime,
                id.as_str(),
                outcome,
                glasshouse::evaluation::now_unix(),
            );

            // Map lines 1149 and 1153 — *"after a successful Git commit"* and
            // *"record the relevant Git commit"*. Glasshouse installs no Git
            // hook: `.git/hooks` belongs to the user, `core.hooksPath` can
            // point anywhere, and nothing needs installing, because this
            // process already runs at every turn boundary and
            // `checkpoint::git` already reads HEAD out of `.git` without
            // spawning anything. A commit landing is therefore *HEAD is not
            // where this session last saw it*, and `note_head_commit` is that
            // comparison.
            //
            // **Outside the `memory_extraction` gate, deliberately**, for the
            // compaction counter's reason one arm up: that switch decides
            // whether Glasshouse *does* something about a boundary, and the
            // commit landed either way. A position recorded only while
            // extraction is enabled would make the switch's first turn back
            // on report a boundary spanning however long it was off.
            let landed = note_head_commit(runtime, &store, &id, record.last_seen_commit.as_deref());
            let completed = matches!(outcome, TurnOutcome::Completed);

            // One extraction per turn, and the more specific trigger wins.
            //
            // A completed turn that also landed a commit is **one** boundary
            // described two ways, not two boundaries: the same event window
            // is read either way, so running both would ask a model the same
            // question twice inside somebody's session and hand the second
            // answer to the duplicate check. `GitCommit` is the description
            // that carries more — it names the object, and line 1153 wants
            // that object on the memories — so it is the one recorded.
            //
            // A turn that ended badly still gets the Git trigger, and gets
            // nothing without one. `TurnOutcome` is the harness's verdict on
            // its *own* turn; a commit that landed is a fact about the
            // repository, and there is no reading of line 1149 in which a
            // commit becomes un-landed because the turn after it failed.
            if memory_extraction_enabled(runtime) {
                match (landed, completed) {
                    (Some(commit), _) => {
                        crate::commands::memory_extraction::hook_extraction(
                            runtime,
                            &id,
                            model(&id),
                            glasshouse::memory::ExtractionTrigger::GitCommit { commit },
                        );
                    }
                    (None, true) => {
                        crate::commands::memory_extraction::hook_extraction(
                            runtime,
                            &id,
                            model(&id),
                            glasshouse::memory::ExtractionTrigger::TaskCompleted,
                        );
                    }
                    (None, false) => {}
                }
            }
            if completed && automatic_checkpoint_enabled(runtime) {
                checkpoint_after_turn(runtime, &id, &record.harness);
            }
        }

        let Some(next) = translated.implied_state() else {
            // A translated event that says nothing about the session's state
            // — it is in the log and that is all it was ever going to do.
            return Ok(());
        };

        if !session::may_apply(record.lifecycle, next) {
            tracing::debug!(
                session = %id,
                from = record.lifecycle.as_str(),
                to = next.as_str(),
                "not applying a harness event to a session in this state"
            );
            return Ok(());
        }
        store.set_lifecycle(&id, next)?;
        tracing::info!(session = %id, event, state = next.as_str(), "harness reported an event");
        Ok(())
    })();

    if let Err(err) = outcome {
        tracing::warn!(error = %err, event, "could not record a harness event");
    }
}

/// Whether a commit landed since this session was last looked at, and record
/// where HEAD stands now — map line 1149.
///
/// Returns the **new** commit when it is a code-change boundary, and `None`
/// otherwise. Three different things produce `None` and they are not the
/// same, which is why they are separated here rather than at the call site:
///
/// - **the project is not a Git repository**, or HEAD cannot be read.
///   `GitPosition::detect` answers `None` for every such case by design, and
///   nothing is stored: a project with no repository has no code-change
///   boundaries to have.
/// - **nobody has looked before.** `previous` is `None` on a session whose
///   first turn this is, and on every session created before the column
///   existed. The position is recorded, and it is **not** a boundary: a
///   boundary is a *change*, and there is nothing here to have changed from.
///   Reporting the first turn of every session as a landed commit would make
///   the trigger fire hardest on sessions that have done nothing yet.
/// - **HEAD has not moved.** The ordinary case, and the one the comparison
///   exists for. Nothing is written, because nothing changed.
///
/// # A failed write is one debug line
///
/// Everything else on this path takes that stance and this is not more
/// important than the compaction counter beside it. The cost of the failure
/// is that the next turn re-reads the same position and calls it a boundary
/// once — a duplicate extraction the duplicate check already absorbs, which
/// is a far better failure than a hook that fell over inside somebody's
/// coding session.
fn note_head_commit(
    runtime: &Runtime,
    store: &glasshouse::session::SessionStore<'_>,
    id: &SessionId,
    previous: Option<&str>,
) -> Option<String> {
    let position = GitPosition::detect(runtime.project().root())?;
    if previous == Some(position.commit.as_str()) {
        return None;
    }

    if let Err(err) = store.record_seen_commit(id, &position.commit) {
        tracing::debug!(
            error = %err,
            session = %id,
            "could not record where HEAD stood for this session"
        );
    }

    // Recorded either way above; a boundary only when there was a position to
    // move *from*.
    previous.is_some().then_some(position.commit)
}

/// Send panic information to the log instead of to the user's terminal.
///
/// # Why a process-global is the right call *here*
///
/// `memory::extract` records the caveat that it cannot fix: it catches a
/// panicking extraction model with `catch_unwind`, but the **default panic
/// hook has already printed to stderr** by then. Setting a global from a
/// library module would be that module deciding something about every program
/// that links it, which is why it did not.
///
/// This is not a library module. It is the `glasshouse hook` command, a
/// process the harness runs **inside the user's session**, and whose stderr
/// the harness may show them. A Rust backtrace appearing in the middle of
/// somebody's coding session because a support job fell over is the same
/// defect as the hook failing: Glasshouse's bookkeeping is never more
/// important than the session it keeps books about.
///
/// The panic is not swallowed — it is logged, with the payload and the
/// location, where `--log-file` will show it.
pub(crate) fn install_quiet_panic_hook() {
    std::panic::set_hook(Box::new(|info| {
        let location = info
            .location()
            .map(|at| format!("{}:{}", at.file(), at.line()))
            .unwrap_or_else(|| "unknown location".to_owned());
        tracing::error!(location, panic = %info, "a glasshouse hook process panicked");
    }));
}

/// How long the coordination hook waits for Claude Code to finish writing
/// the `PreToolUse` payload.
///
/// [`PAYLOAD_DRAIN_BOUND`]'s value, and a stricter reason: the lifecycle hook
/// drains its input to be polite to the writer, whereas this one *needs* the
/// bytes and still may not wait for them. A payload that has not arrived in a
/// second is a payload this process goes on without — allowing, silently.
const EDIT_INTENT_READ_BOUND: std::time::Duration = PAYLOAD_DRAIN_BOUND;

/// Handle `edit-intent hook` — capability map lines 2402 to 2405.
///
/// Reads one `PreToolUse` event on stdin, records what the session is about
/// to change, compares it with other live sessions' claims, and writes the
/// hook response on stdout.
///
/// # It always allows, and that is the whole contract
///
/// `PreToolUse` is a **gate**: a `deny` here stops a user's `Edit` dead, and
/// a hook that exited non-zero would veto the tool call outright. Steering
/// decision 4 (design-decisions.md) rules soft coordination with an explicit
/// bypass and **no blocking**, so this function returns `()` rather than a
/// `Result` and every step below fails into the same quiet allowance:
///
/// - no `--session`, so nothing can be attributed → allow, silently;
/// - a payload that does not arrive, or does not parse → allow, silently;
/// - a read-only tool, or one naming no path inside the project → allow,
///   silently;
/// - a project database that cannot be opened, a session not in it, a claim
///   that cannot be written → allow, and say so only in the debug log.
///
/// A coordination layer that broke a user's edit because its own lookup
/// failed would be worse than no coordination at all.
///
/// # Order: read the claims, then take one
///
/// [`glasshouse::session::SessionStore::active_claims`] is read **before**
/// this session claims anything, so the answer cannot include a claim this
/// very call just wrote. It would be filtered by session identity anyway;
/// reading first means that invariant does not depend on the filter.
pub(crate) fn edit_intent_hook(runtime: &Runtime, session: Option<&str>) {
    let payload = read_pre_tool_use_payload();
    let conflict = edit_intent_conflict(runtime, session, payload.as_deref());
    print_edit_intent_response(conflict.as_deref());
}

/// The `PreToolUse` payload, or `None` if it did not arrive in time.
///
/// On its own thread and abandoned at [`EDIT_INTENT_READ_BOUND`], the shape
/// [`abandon_after`] exists for — with the bytes handed back through a
/// channel, because unlike the lifecycle hook's drain this one has a result
/// worth keeping.
fn read_pre_tool_use_payload() -> Option<Vec<u8>> {
    use std::io::Read as _;

    let (sender, receiver) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut input = Vec::new();
        let read = std::io::stdin().read_to_end(&mut input);
        // A closed receiver means the bound expired and nobody is listening,
        // which is a normal outcome here.
        let _ = sender.send(read.ok().map(|_| input));
    });
    match receiver.recv_timeout(EDIT_INTENT_READ_BOUND) {
        Ok(payload) => payload,
        Err(_) => {
            tracing::debug!(
                bound_ms = EDIT_INTENT_READ_BOUND.as_millis(),
                "edit intent: the harness had not finished writing this hook's input; \
                 allowing without recording"
            );
            None
        }
    }
}

/// One sentence naming the other sessions that already claimed a path this
/// call is about to change, or `None`.
///
/// Every failure answers `None`, which the caller renders as a quiet
/// allowance — see [`edit_intent_hook`]'s own doc for the list.
fn edit_intent_conflict(
    runtime: &Runtime,
    session: Option<&str>,
    payload: Option<&[u8]>,
) -> Option<String> {
    let session = session?;
    let payload = payload?;

    let event = match glasshouse::firewall::adapter::parse_pre_tool_use_event(payload) {
        Ok(event) => event,
        Err(err) => {
            tracing::debug!(
                error = %err,
                "edit intent: could not parse the PreToolUse event; allowing without recording"
            );
            return None;
        }
    };

    // The tool gate and the project gate, in that order and both from the
    // shipped functions the `file_touched` producer already uses: a
    // read-shaped tool records nothing even though its input carries a path,
    // and a path outside this project is dropped before it can be stored.
    let root = runtime.project().root();
    let mut paths: Vec<String> = Vec::new();
    for raw in glasshouse::firewall::adapter::edit_intent_paths(&event) {
        let Some(path) = crate::commands::context_firewall::project_relative_path(root, &raw)
        else {
            continue;
        };
        if !paths.contains(&path) {
            paths.push(path);
        }
    }
    if paths.is_empty() {
        return None;
    }

    let sessions = match ProjectSessions::open(runtime) {
        Ok(sessions) => sessions,
        Err(err) => {
            tracing::debug!(
                error = %format!("{err:#}"),
                "edit intent: the project database is unavailable; allowing without recording"
            );
            return None;
        }
    };
    let store = sessions.store();
    let id = match store.resolve_id(session) {
        Ok(id) => id,
        Err(err) => {
            tracing::debug!(
                error = %format!("{err:#}"),
                "edit intent: this hook's session is not in the project; allowing without \
                 recording"
            );
            return None;
        }
    };

    // Map line 2403's comparison, read before line 2402's write.
    let existing = store.active_claims().unwrap_or_else(|err| {
        tracing::debug!(
            error = %format!("{err:#}"),
            "edit intent: could not read this project's file claims; allowing, and \
             reporting no conflict"
        );
        Vec::new()
    });

    let mut notices: Vec<String> = Vec::new();
    for path in &paths {
        for claim in existing
            .iter()
            .filter(|claim| &claim.path == path && claim.session_id != id)
        {
            notices.push(format!(
                "{path} is already claimed by session {} (since {})",
                crate::commands::shared::short_id(&claim.session_id),
                crate::commands::shared::format_age(claim.claimed_at),
            ));
            // Map line 2414: the same collision this loop just told the
            // editing session about, told to the orchestrator too — one
            // delivery attempt per path, never a batch, so a conflict on
            // this path cannot be conflated with one on another (line 2415).
            notify_orchestrator_of_conflict(&store, path, &id, &claim.session_id);
        }
        // Map line 2402: the intent itself, recorded before the operation
        // runs. Per path rather than per batch, and best effort per path —
        // one claim that cannot be written must not take the others, and
        // none of them may change what this hook answers.
        if let Err(err) = store.claim_file(&id, path) {
            tracing::debug!(
                error = %format!("{err:#}"),
                path,
                "edit intent: could not record an edit intent"
            );
        }
    }

    if notices.is_empty() {
        return None;
    }
    tracing::info!(
        session = %id,
        conflicts = notices.len(),
        "edit intent: another session already claims a file this one is about to change"
    );
    // Map lines 2409-2410: every conflict this build can detect is a same-path
    // collision, so it is named as one — `OverlapKind::describe` is the one
    // place that wording lives, and `commands::sessions::claims_block` reads
    // it too rather than spelling it out a second time.
    Some(format!(
        "Glasshouse file coordination: {} ({}). This is advice, not a lock — the edit is \
         going ahead. Consider coordinating before overwriting shared work.",
        notices.join("; "),
        glasshouse::firewall::adapter::OverlapKind::DirectFile.describe(),
    ))
}

/// Map line 2414: tell this project's one unambiguous live orchestrator
/// about a direct file overlap [`edit_intent_conflict`] just detected on
/// `path`, between `editor` (this call's session) and `holder` (the session
/// whose existing claim it collided with).
///
/// Called once per colliding path, never once per hook call: line 2415's
/// granularity requirement is that a conflict on one path names only that
/// path, and a single call bundling every notice into one message would
/// have made a conflict on `src/a.rs` indistinguishable from one on
/// `src/b.rs` at the one reader — the orchestrator — that is supposed to
/// act on the difference.
///
/// # Ambiguity is reported, not guessed — the map's own architectural note
///
/// design-decisions.md's *A bounded file-coordination capability*: *"where
/// there is no unambiguous active orchestrator, surface that the conflict
/// could not be delivered rather than inventing a worker-ownership or push
/// subsystem."* Zero or more than one live orchestrator session is that
/// case, and this says so at `warn` — visible with `--log-level`/
/// `--log-file` the way every other diagnostic on this path is — rather
/// than picking a guess, broadcasting to every one, or the first row.
///
/// # No self-notification
///
/// The one live orchestrator being either `editor` or `holder` is not
/// "ambiguous" and is not logged as undeliverable: it is already a party to
/// this exact conflict and was told through the hook response itself (the
/// three channels [`glasshouse::firewall::adapter::pre_tool_use_response`]
/// writes), for the same reason
/// `edit_intent::a_session_does_not_conflict_with_its_own_claim` exists —
/// telling it again through a second channel would not be new information.
///
/// # Delivery reuses the Phase 15 wake-up seam, and why it is `debug`-only
/// here today
///
/// [`glasshouse::session::api::SessionApi::send_text`] is the delivery path
/// design-decisions.md names — *"Glasshouse already has an orchestrator
/// delivery path: the Phase 15 wake-up flow, `SessionApi::send_text`, and
/// `api/unix/events.rs`. Reuse it… do not design another transport."* This
/// function is that seam's caller from a new site: a `PreToolUse` hook
/// subprocess, which owns no pseudo-terminal of its own, so the
/// [`SessionRuntime`] it constructs starts empty and
/// [`SessionApi::send_text`] answers `NotLive` unless something else in
/// *this* process already holds the target session — nothing here does.
/// That is requirement 5's **best-effort** outcome, logged at `debug` and
/// never surfaced as the `warn` ambiguity gets: the recipient was resolved
/// correctly and the seam is wired for the moment a process that does hold
/// a live handle reaches it, or reads this claim itself. See this packet's
/// `packet_errors` for why that gap is recorded rather than closed with a
/// second transport.
fn notify_orchestrator_of_conflict(
    store: &glasshouse::session::SessionStore<'_>,
    path: &str,
    editor: &SessionId,
    holder: &SessionId,
) {
    let orchestrators = match store.live_orchestrators() {
        Ok(orchestrators) => orchestrators,
        Err(err) => {
            tracing::debug!(
                error = %format!("{err:#}"),
                "edit intent: could not read this project's orchestrator sessions"
            );
            return;
        }
    };

    let orchestrator = match orchestrators.as_slice() {
        [] => {
            tracing::warn!(
                path,
                "edit intent: a conflict on this path could not be delivered to an \
                 orchestrator — no live orchestrator session is running in this project"
            );
            return;
        }
        [only] => only,
        many => {
            tracing::warn!(
                path,
                candidates = many.len(),
                "edit intent: a conflict on this path could not be delivered to an \
                 orchestrator — more than one live orchestrator session is running in \
                 this project, and Glasshouse does not guess which one"
            );
            return;
        }
    };

    if &orchestrator.id == editor || &orchestrator.id == holder {
        return;
    }

    let text = format!(
        "Glasshouse file coordination: {path} has {} between session {} and session {}.",
        glasshouse::firewall::adapter::OverlapKind::DirectFile.describe(),
        crate::commands::shared::short_id(editor),
        crate::commands::shared::short_id(holder),
    );

    let mut live = SessionRuntime::new();
    let mut api = SessionApi::new(store, &mut live);
    match api.send_text(&orchestrator.id, &text, MessageOrigin::Machine) {
        Ok(()) => tracing::info!(
            orchestrator = %orchestrator.id,
            path,
            "edit intent: delivered a conflict notice to the orchestrator"
        ),
        Err(err) => tracing::debug!(
            error = %format!("{err:#}"),
            orchestrator = %orchestrator.id,
            path,
            "edit intent: could not deliver a conflict notice to the orchestrator"
        ),
    }
}

/// Write the `PreToolUse` hook response JSON to stdout — the protocol
/// channel here exactly as it is for `context-firewall hook`.
///
/// Not a `Result`: a response that could not be serialized would leave the
/// harness with no answer at all, which Claude Code reads as "no opinion"
/// and proceeds from. There is nothing here worth failing a tool call over,
/// so a serialization failure degrades to the bare allow literal.
fn print_edit_intent_response(conflict: Option<&str>) {
    let response = glasshouse::firewall::adapter::pre_tool_use_response(conflict);
    match serde_json::to_string(&response) {
        Ok(rendered) => println!("{rendered}"),
        Err(err) => {
            tracing::debug!(error = %err, "edit intent: could not render the hook response");
            println!(
                r#"{{"hookSpecificOutput":{{"hookEventName":"PreToolUse","permissionDecision":"allow"}}}}"#
            );
        }
    }
}
