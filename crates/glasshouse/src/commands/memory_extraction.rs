//! `commands::memory_extraction` -- moved verbatim from `main.rs` (Phase 59 decomposition).

use glasshouse::Runtime;
use glasshouse::checkpoint::WorkingTreeStatus;
use glasshouse::events::EventLog;
use glasshouse::session::SessionId;

/// The extraction model Glasshouse has in production, which is none.
///
/// Phase 21 has two separate lines here and they are separate on purpose:
/// *"allow memory extraction to run after task completion"* is about the
/// **trigger**, and *"allow a configurable cheap or local model to perform
/// memory extraction"* is about the **model**. The trigger is built; the
/// model is Phase 39's disposable-job provider and does not exist.
///
/// So extraction really does run after every completed turn, and it really
/// does report `no extraction model is available` every time — which is
/// exactly the shape [`glasshouse::memory::ExtractionOutcome`] exists to
/// carry, and exactly the failure Phase 21's *"keep memory-extraction failure
/// non-fatal to the coding session"* is about. Naming itself plainly matters
/// as much as it does for `glasshouse memory extract`: a log line saying a
/// model ran when none did would be worse than no line.
pub(crate) struct NoExtractionModel;

impl glasshouse::memory::ExtractionModel for NoExtractionModel {
    fn describe(&self) -> String {
        "none configured (Phase 39 supplies the provider)".to_owned()
    }

    fn complete(
        &self,
        _prompt: &glasshouse::memory::extract::Prompt,
    ) -> Result<String, glasshouse::memory::ModelError> {
        Err(glasshouse::memory::ModelError::Unavailable)
    }
}

/// How long a hook process will wait for extraction before going on without
/// it.
///
/// The number is chosen against what is on the other side of it: this process
/// is run **by the harness, inside the user's session**, and Claude Code
/// treats a hook's exit as a gate on the turn. A model that hangs must
/// therefore cost the user a bounded pause and not an open-ended one.
///
/// Deliberately not "however long the model takes". Extraction is a support
/// job; a coding session waiting on one has the relationship backwards.
pub(crate) const EXTRACTION_BOUND: std::time::Duration = std::time::Duration::from_secs(5);

/// Run memory extraction over what this session has done — Phase 29's
/// **memory commit**, whatever started it.
///
/// # One operation, four triggers, and no second pipeline
///
/// Map line 1147 asks for *"a lightweight memory commit operation that
/// extracts durable project knowledge from recently completed work"* and
/// lines 1148-1151 ask for four ways to start one. This function is that
/// operation, and `trigger` is the whole of the difference between them:
/// `Manual` from `glasshouse memory commit`, `TaskCompleted` and `GitCommit`
/// from the `TurnEnded` arm of [`report_hook_with`], `BeforeCompaction` from
/// its `PreCompact` arm. A second extraction path for any of them would be a
/// second answer to what is worth remembering, a second credential screen and
/// a second duplicate check.
///
/// # The outcome is returned, and the hook path still ignores it
///
/// `Option<ExtractionOutcome>` rather than `()` so `glasshouse memory commit`
/// can print what its run actually did. It is not an error channel and does
/// not become one: `None` means the *preparation* failed or the bound expired
/// — both already logged here — and every failure of the extraction itself is
/// a field on the outcome, never a `Result`. The hook path discards it, which
/// is why nothing about its posture changes.
///
/// # Nothing here can hurt the session, and that is the design
///
/// Phase 21: *"keep memory-extraction failure non-fatal to the coding
/// session."* Four different failures are absorbed here and none of them
/// reaches [`report_hook`]:
///
/// - the project database will not open, or the event log will not read —
///   logged, and the function returns;
/// - the model is unavailable, refuses, or answers rubbish —
///   [`glasshouse::memory::Extractor::run`] has no error channel at all and
///   describes it on the outcome;
/// - the model **panics** — caught inside `run`, reported as an outcome;
/// - the model **hangs** — the work is on its own thread and this waits
///   [`EXTRACTION_BOUND`], then leaves it behind. The thread dies when the
///   process exits moments later, having written nothing: the store is only
///   touched after the model answers.
///
/// # Why a thread and not just a call
///
/// The only thing that buys is the bound, and the bound is the whole point.
/// This codebase has no async runtime and [`glasshouse::memory::ExtractionModel`]
/// is deliberately synchronous, so a thread is the mechanism; `ExtractionModel`
/// is `Send + Sync` for precisely this reason.
///
/// Everything cheap happens before the thread starts — opening the database,
/// reading a bounded window of the log, scrubbing and bounding the chunk — so
/// what is on the far side of the bound is the model call and the insert, and
/// a timeout means the model, not Glasshouse.
pub(crate) fn run_extraction(
    runtime: &Runtime,
    id: &SessionId,
    model: Box<dyn glasshouse::memory::ExtractionModel>,
    trigger: glasshouse::memory::ExtractionTrigger,
) -> Option<glasshouse::memory::ExtractionOutcome> {
    use glasshouse::memory::extract::chunk::ChunkLimits;
    use glasshouse::memory::extract::lifecycle::{EVENT_WINDOW, chunk_for_session};
    use glasshouse::memory::{Extractor, ProjectMemory};

    let prepared = (|| -> anyhow::Result<_> {
        let log = EventLog::open(runtime)?;
        let events = log.recent_for_session(id, EVENT_WINDOW)?;
        let memory = ProjectMemory::open(runtime)?;
        Ok((memory, events))
    })();

    let (memory, events) = match prepared {
        Ok(prepared) => prepared,
        Err(err) => {
            tracing::warn!(
                session = %id,
                error = %format!("{err:#}"),
                "could not read this session's history for memory extraction"
            );
            return None;
        }
    };

    // The commit is still deliberately not *read* here. `checkpoint::git`
    // knows how to find one and this process does not need to: a memory's
    // commit is "where the project was when this was learned", and a hook
    // process runs while the user's tree is mid-edit. `glasshouse memory
    // extract` takes the session's activity from a person who knows; this
    // path takes what the log holds and claims nothing more.
    //
    // Map line 1153 — *"record the relevant Git commit with memories produced
    // from a code-change boundary"* — is the one case where that objection
    // does not apply, and it does not apply because nothing is read. A
    // `GitCommit` trigger **is** a commit: the caller compared HEAD against
    // what this session had already seen, found it moved, and the object that
    // moved it is the trigger's own payload. So the commit recorded on these
    // memories is the boundary that caused the run, not a reading taken at an
    // arbitrary moment during it — which is exactly the distinction the
    // paragraph above refuses to blur. Every other trigger still carries
    // `None`.
    let chunk = chunk_for_session(id, &events, trigger.commit(), ChunkLimits::default());

    // The **working tree**, though, is read here, and mid-edit is exactly why.
    //
    // The refusal above is about a *commit*: "where the project was when this
    // was learned" is a poor answer while somebody is still editing. The same
    // sentence argues the other way for the dirty set — mid-edit is the state
    // in which what differs from the index is most informative and a commit
    // least — so this reads it and records it under its own name, `observed`,
    // rather than claiming the memory referenced anything.
    //
    // Read here, before the thread starts, for the reason this function's own
    // doc gives about everything else cheap: the model call can take seconds,
    // and the set this associates memories with should be the one that was
    // true when extraction began rather than whatever the user has typed
    // since. `WorkingTreeStatus::detect` opens two small files and no
    // database.
    let observed_files = WorkingTreeStatus::detect(runtime.project().root())
        .map(|status| status.changed_files)
        .unwrap_or_default();

    let (tx, rx) = std::sync::mpsc::channel();
    let session = id.clone();
    // Cloned rather than moved: `ExtractionTrigger` stopped being `Copy` when
    // `GitCommit` gained its commit, and the log lines below name the trigger
    // after the thread has taken its own.
    let thread_trigger = trigger.clone();
    std::thread::spawn(move || {
        let store = memory.store();
        let outcome = Extractor::new(&store, model.as_ref()).run(&chunk, thread_trigger);
        // A closed receiver means the bound expired and nobody is listening.
        // That is a normal outcome here, not an error.
        let _ = tx.send(outcome);
        drop(session);
    });

    match rx.recv_timeout(EXTRACTION_BOUND) {
        Ok(outcome) => {
            match &outcome.failure {
                None => tracing::info!(
                    session = %id,
                    trigger = %trigger,
                    model = outcome.model,
                    stored = outcome.stored(),
                    duplicates = outcome.duplicates,
                    speculative = outcome.speculative,
                    rejected = outcome.rejected.len(),
                    "memory extraction ran"
                ),
                Some(failure) => tracing::info!(
                    session = %id,
                    trigger = %trigger,
                    model = outcome.model,
                    reason = %failure,
                    "memory extraction produced nothing"
                ),
            }
            // After the log line and outside the `failure` match on purpose:
            // a reply that failed the extraction contract still cost whatever
            // the provider says it cost, and a ledger that recorded only the
            // runs that worked would under-report exactly the calls worth
            // knowing about.
            record_extraction_observation(runtime, &outcome);
            record_observed_files(runtime, &outcome.recorded, &observed_files);
            // Map line 1769, opt-in: never fails and never changes `outcome`
            // — the extraction it describes has already run and already
            // been reported.
            if crate::commands::routing_classification::memory_extraction_diagnostics_enabled(
                runtime,
            ) {
                glasshouse::memory::extract::diagnostics::append_diagnostics(runtime, &outcome);
            }
            Some(outcome)
        }
        Err(_) => {
            tracing::warn!(
                session = %id,
                trigger = %trigger,
                bound_ms = EXTRACTION_BOUND.as_millis(),
                "memory extraction did not finish within its bound; the session is unaffected"
            );
            None
        }
    }
}

/// [`run_extraction`] on a hook's path, where a lost memory has to be said
/// out loud.
///
/// # Why this exists at all, when `run_extraction` already logs every failure
///
/// Because on this path nothing reads the log. `logging::LogConfig::resolve`
/// answers [`glasshouse::logging::LogSink::Disabled`] unless `GLASSHOUSE_LOG`
/// is set or a `--log-*` flag is given, and a harness spawning
/// `glasshouse hook` gives neither — so `run_extraction`'s
/// `"memory extraction produced nothing"` and its bound-expiry `warn!` are
/// both written to a subscriber that was never installed. Measured
/// 2026-08-31: a `PreCompact` hook whose model call failed exited **0**, with
/// **empty stderr**, having recorded nothing.
///
/// That is the precise thing capability map line 1174 is about. *"Record
/// enough pre-compaction durable memory that important project decisions do
/// not depend solely on a lossy native compact summary"* is not satisfied by
/// a trigger that fires, fails, and says nothing: the person then believes
/// their decisions were captured and goes on to compact, which is worse than
/// knowing they were not.
///
/// # Why stderr, and why one line
///
/// `main.rs`'s own [`run`] already draws this distinction for the overridden
/// safety refusal, three lines into the program and for exactly this reason:
/// *"logging is off by default, so a `tracing::warn!` there can go completely
/// unseen … it always gets a line on stderr, log or no log."* A memory the
/// compaction trigger was supposed to record and did not is user-facing in
/// the same sense.
///
/// Stderr and not stdout, and never a non-zero exit: Claude Code reads a
/// hook's exit code as a gate on the turn, and Phase 21's *"keep
/// memory-extraction failure non-fatal to the coding session"* is unchanged
/// by this. The hook still exits zero whatever extraction did.
///
/// Not used by `glasshouse memory commit`: that trigger is
/// [`glasshouse::memory::ExtractionTrigger::Manual`], it runs in front of a
/// person who is watching, and it prints its own report. This is the wrapper
/// for the triggers that run inside somebody's session with nobody watching.
pub(crate) fn hook_extraction(
    runtime: &Runtime,
    id: &SessionId,
    model: Box<dyn glasshouse::memory::ExtractionModel>,
    trigger: glasshouse::memory::ExtractionTrigger,
) {
    // Read before the call, because `run_extraction` takes the trigger.
    let named = trigger.as_str();
    let outcome = run_extraction(runtime, id, model, trigger);
    if let Some(notice) = lost_extraction_notice(named, outcome.as_ref()) {
        eprintln!("glasshouse: warning: {notice}");
        eprintln!(
            "glasshouse: the coding session is unaffected; this project's durable memory is not \
             updated for this boundary"
        );
    }
}

/// What to tell the person about an extraction that recorded nothing, or
/// [`None`] when nothing was lost.
///
/// Separated from [`hook_extraction`] so the decision can be tested without a
/// process: what this returns is the whole of the difference between a silent
/// loss and an observable one.
///
/// # The four cases, and why two of them are silent
///
/// - **no outcome at all.** [`run_extraction`] answers `None` for its two
///   preparation failures and for [`EXTRACTION_BOUND`] expiring. All three
///   are losses — a boundary went by and nothing was written — and the reason
///   is in a log that, on this path, does not exist.
/// - **a failure.** The model was unavailable, refused, timed out, panicked,
///   answered something the contract could not read, or the store could not
///   be read for duplicate detection. Each is a memory that should exist and
///   does not, and [`glasshouse::memory::extract::ExtractionFailure`]'s `Display` is a
///   fixed phrase by construction — no provider body reaches this line.
/// - **[`glasshouse::memory::extract::ExtractionFailure::NothingToExtract`] is
///   deliberately silent.** There was no session activity to extract from, so
///   there is no memory to have lost. A warning here would fire on every
///   compaction of a session that had not done anything yet, and a warning
///   that cries wolf is how the real one gets ignored.
/// - **rejections without a failure.** The model answered and some of what it
///   proposed did not survive the contract. Said out loud when *nothing*
///   survived, and silent when something did: a run that stored two memories
///   and rejected a third lost nothing a person needs to act on, and
///   duplicates and speculative drops are the mechanism working rather than
///   failing.
pub(crate) fn lost_extraction_notice(
    trigger: &str,
    outcome: Option<&glasshouse::memory::ExtractionOutcome>,
) -> Option<String> {
    use glasshouse::memory::extract::ExtractionFailure;

    let Some(outcome) = outcome else {
        return Some(format!(
            "memory extraction for `{trigger}` did not finish and recorded nothing (it was cut \
             off at its {}s bound, or this session's history could not be read)",
            EXTRACTION_BOUND.as_secs()
        ));
    };

    if let Some(failure) = &outcome.failure {
        // The one failure that is not a loss.
        if matches!(failure, ExtractionFailure::NothingToExtract) {
            return None;
        }
        return Some(format!(
            "memory extraction for `{trigger}` recorded nothing: {failure}"
        ));
    }

    if outcome.stored() == 0 && !outcome.rejected.is_empty() {
        return Some(format!(
            "memory extraction for `{trigger}` recorded nothing: the model answered, and all {} \
             of the memories it proposed were rejected",
            outcome.rejected.len()
        ));
    }

    None
}

/// What the extraction model reported the call cost, into this project's
/// routing evidence ledger.
///
/// # This is the first thing in this build that counts tokens
///
/// `routing_observations` has carried `input_tokens`, `output_tokens` and
/// `cached_input_tokens` since migration 11 and nothing has ever written
/// one: `crate::gateway::ingress` relays a response body it is designed
/// never to parse, so the gateway producer leaves all three `NULL` and says
/// so in its own module header. Memory extraction is the other path —
/// Glasshouse builds the request itself and already deserializes the whole
/// reply — so the counts come from a document that was parsed anyway. See
/// [`glasshouse::memory::extract::ModelCall::observation`] for exactly what
/// one row carries and what it deliberately leaves empty.
///
/// # Why the ledger is opened here and not beside the event log
///
/// The same finding [`evidence_ledger`] carries, one path over.
/// [`glasshouse::routing::evidence::EvidenceLedger`] holds `Mutex<Connection>`
/// — an open SQLite handle for its whole lifetime — and a handle opened on a
/// path that turns out to have nothing to write blocks a later writer under
/// Windows' mandatory `LockFileEx` while being invisible under POSIX advisory
/// locks. So nothing is opened until `observation()` has already said there
/// is a row: that is [`None`] for every run that reached no provider, which
/// is every run under the default configuration, where extraction chooses a
/// resource and calls nothing at all.
///
/// # A failure here is one log line
///
/// [`run_extraction`]'s own posture, for its own reason: this is a hook
/// process running inside somebody's coding session, and Glasshouse's
/// bookkeeping is never more important than the session it keeps books
/// about. There is no error channel out of this function because no caller
/// should have one.
fn record_extraction_observation(
    runtime: &Runtime,
    outcome: &glasshouse::memory::ExtractionOutcome,
) {
    let Some(observation) = outcome.observation() else {
        return;
    };
    // Capability map line 1832. `ModelCall::observation` deliberately leaves
    // `purpose` unwritten — its own doc comment records that it fills no
    // column with a nearby value — so the stamp is applied here, by the
    // producer that knows what this call was *for*, the same way
    // `record_classification_observation` and `record_routing_latency` stamp
    // theirs.
    //
    // **Only rows written from here on.** Every extraction row already on
    // disk keeps its `NULL`, and the rendering counts those as *unstamped*
    // rather than re-labelling them: `NewObservation::with_purpose`'s own doc
    // comment is the rule, and back-filling would make "this build recorded
    // nothing here" indistinguishable from "this build recorded a purpose".
    let observation = observation.with_purpose(Some(
        crate::commands::routing_classification::EXTRACTION_PURPOSE,
    ));
    let ledger = match glasshouse::routing::evidence::EvidenceLedger::open(runtime) {
        Ok(ledger) => ledger,
        Err(err) => {
            tracing::warn!(
                error = %err,
                "routing evidence ledger unavailable; what this extraction cost is not recorded"
            );
            return;
        }
    };
    if let Err(err) = ledger.record(observation, glasshouse::provider::cache::now_unix_seconds()) {
        tracing::warn!(
            error = %err,
            "could not record what memory extraction cost"
        );
    }
}

/// Which files were being worked on when these memories were learned, into
/// this project's `memory_files` — migration 17.
///
/// # This records an observation and not a reference, deliberately
///
/// `paths` is what the git index said differed from the working tree when
/// extraction began. It says *"this was learned while that file was being
/// worked on"*, which is a fact about the **session**: three memories out of a
/// session that dirtied twenty files get all sixty pairs, and each pair is
/// true. It is emphatically not capability-map line 1139's *"the files a
/// memory explicitly references"* — on this path the model's input carries no
/// prose at all, so a model asked to name files here would be fabricating from
/// an empty input, and line 1294's rule is that a fabricated value inverts the
/// policy rather than degrading it. Every row therefore carries
/// [`glasshouse::memory::FileAssociation::Observed`].
///
/// # Why the store is opened here and not beside the event log
///
/// [`record_extraction_observation`]'s finding, one function over, for the
/// same reason: an open SQLite handle on a path that turns out to have
/// nothing to write blocks a later writer under Windows' mandatory
/// `LockFileEx` while being invisible under POSIX advisory locks (practice
/// §65). So the guard comes first and nothing is opened at all when there is
/// no row — which is every extraction that stored nothing, and every one run
/// against a clean tree.
///
/// This deliberately runs on the calling thread rather than inside the
/// extraction thread: the thread outlives its bound, and a write started
/// there after the process has already decided to move on would be a second
/// writable handle appearing at an unpredictable moment.
///
/// # A failure here is one log line
///
/// [`run_extraction`]'s posture, and the path is not named in it: a file path
/// is the user's own data, so the log says how many associations were lost
/// and never which files they were about.
fn record_observed_files(
    runtime: &Runtime,
    recorded: &[glasshouse::memory::MemoryId],
    paths: &[String],
) {
    if recorded.is_empty() || paths.is_empty() {
        return;
    }
    let memory = match glasshouse::memory::ProjectMemory::open(runtime) {
        Ok(memory) => memory,
        Err(err) => {
            tracing::warn!(
                error = %format!("{err:#}"),
                "project memory unavailable; which files this session was \
                 working on is not recorded"
            );
            return;
        }
    };
    match memory.store().record_observed_files(recorded, paths) {
        Ok(written) => tracing::debug!(
            memories = recorded.len(),
            files = paths.len(),
            rows = written,
            "recorded which files were being worked on"
        ),
        Err(err) => tracing::warn!(
            error = %err,
            "could not record which files were being worked on"
        ),
    }
}

/// A model's reply read from a file, for `glasshouse memory extract`.
///
/// [`describe`](glasshouse::memory::ExtractionModel::describe) says plainly
/// that nothing was called, and that string is stored on the outcome and
/// printed on every run. An evaluation run must never be mistaken later for
/// evidence that a model performed extraction — that capability is Phase 39's
/// and is not built.
pub(crate) struct ReplyFromFile(pub(crate) String);

impl glasshouse::memory::ExtractionModel for ReplyFromFile {
    fn describe(&self) -> String {
        "file (evaluation harness; no model was called)".to_owned()
    }

    fn complete(
        &self,
        _prompt: &glasshouse::memory::extract::Prompt,
    ) -> Result<String, glasshouse::memory::ModelError> {
        Ok(self.0.clone())
    }
}
