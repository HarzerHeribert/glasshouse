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

/// `model`, with map line 488's withheld-credential notice on its
/// description when `withheld` is non-empty; `model` itself otherwise.
///
/// `describe` is what [`glasshouse::memory::ExtractionOutcome::model`]
/// stores and what [`lost_extraction_notice`] prints for an unavailable
/// model, so the sentence reaches the one stderr line a hook has without a
/// second channel. `complete` is delegated untouched.
pub(crate) fn noting_withheld_credentials(
    model: Box<dyn glasshouse::memory::ExtractionModel>,
    withheld: &[crate::commands::routing_classification::WithheldCredential],
) -> Box<dyn glasshouse::memory::ExtractionModel> {
    match crate::commands::routing_classification::withheld_credential_notice(withheld) {
        Some(notice) => Box::new(CredentialWithheld {
            inner: model,
            notice,
        }),
        None => model,
    }
}

struct CredentialWithheld {
    inner: Box<dyn glasshouse::memory::ExtractionModel>,
    notice: String,
}

impl glasshouse::memory::ExtractionModel for CredentialWithheld {
    fn describe(&self) -> String {
        format!("{}; {}", self.inner.describe(), self.notice)
    }

    fn complete(
        &self,
        prompt: &glasshouse::memory::extract::Prompt,
    ) -> Result<String, glasshouse::memory::ModelError> {
        self.inner.complete(prompt)
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
/// **memory commit**, whatever started it. One operation for map line
/// 1147's four triggers (lines 1148-1151): `Manual`, `TaskCompleted`,
/// `GitCommit`, `BeforeCompaction` — never a second pipeline, credential
/// screen, or duplicate check.
///
/// Returns `Option<ExtractionOutcome>`, not `()` or a `Result`: `None` means
/// preparation failed or the bound expired (both logged here), and every
/// failure of the extraction itself is a field on the outcome. The hook
/// path discards it either way — Phase 21's *"keep memory-extraction
/// failure non-fatal to the coding session"* — so a db/log open failure, a
/// model that is unavailable, refuses, answers rubbish, panics, or hangs
/// (bounded by [`EXTRACTION_BOUND`], then left running on its own thread as
/// the process exits) never reaches [`report_hook`].
///
/// A thread, not a plain call, because the bound is the whole point: this
/// codebase has no async runtime and `ExtractionModel` is deliberately
/// synchronous. Everything cheap runs before the thread starts, so only the
/// model call and the insert sit past the timeout.
///
/// History: design-decisions.md, "Trims: commands module docs", run_extraction.
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
/// out loud, because nothing reads the log there:
/// `logging::LogConfig::resolve` answers `Disabled` unless `GLASSHOUSE_LOG`
/// or a `--log-*` flag is given, and a harness spawning `glasshouse hook`
/// gives neither — measured 2026-08-31, a failed `PreCompact` model call
/// exited **0** with empty stderr, recording nothing, which is exactly the
/// silent-failure map line 1174 warns against.
///
/// So this writes one line to stderr on any failure, never stdout and
/// never a non-zero exit — the same distinction `main.rs`'s `run` draws for
/// the overridden safety refusal — and Phase 21's *"keep memory-extraction
/// failure non-fatal to the coding session"* stays true: the hook still
/// exits zero whatever extraction did.
///
/// Not used by `glasshouse memory commit` (`ExtractionTrigger::Manual` runs
/// in front of a watching person and prints its own report); this is the
/// wrapper for triggers that run inside somebody's session with nobody
/// watching.
///
/// History: design-decisions.md, "Trims: commands module docs", hook_extraction.
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
/// [`None`] when nothing was lost. Separated from [`hook_extraction`] so the
/// decision can be tested without a process.
///
/// Four cases, two silent: **no outcome at all** ([`run_extraction`]'s two
/// preparation failures and [`EXTRACTION_BOUND`] expiring) and **a
/// failure** (unavailable, refused, timed out, panicked, unreadable answer,
/// or the store unreadable for duplicate detection) are both said out
/// loud — `ExtractionFailure`'s `Display` is a fixed phrase, no provider
/// body reaches this line. **`NothingToExtract` stays silent**: there was
/// no activity to extract from, so a warning would fire on every empty
/// compaction and teach people to ignore it. **Rejections without a
/// failure** are silent unless *nothing* survived: dropping a duplicate or
/// a speculative memory is the mechanism working, not failing.
///
/// History: design-decisions.md, "Trims: commands module docs", lost_extraction_notice.
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
        // An unavailable model is the one failure whose cause is on the
        // outcome's own description — the routed rationale, and map line
        // 488's withheld-credential notice when a provider's key resolved
        // from nowhere this hook can see — so that line says more than the
        // fixed phrase. Every other failure keeps the fixed phrase alone.
        if matches!(
            failure,
            ExtractionFailure::Model(glasshouse::memory::ModelError::Unavailable)
        ) {
            return Some(format!(
                "memory extraction for `{trigger}` recorded nothing: {failure} ({})",
                outcome.model
            ));
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
/// routing evidence ledger — the first thing in this build that counts
/// tokens, because the gateway path never writes them
/// (`crate::gateway::ingress` relays a body it is designed never to parse)
/// while extraction already deserializes the whole reply. See
/// [`glasshouse::memory::extract::ModelCall::observation`] for what one row
/// carries and leaves empty.
///
/// Opened here, not beside the event log, for the reason [`evidence_ledger`]
/// carries too: an open `Mutex<Connection>` for the ledger's whole lifetime
/// blocks a later writer under Windows even when there is nothing to write.
/// So nothing opens until `observation()` has already said there is a row —
/// [`None`] under the default configuration, where extraction reaches no
/// provider.
///
/// A failure here is one log line, [`run_extraction`]'s own posture: no
/// caller of a hook process inside somebody's session should have an error
/// channel out of it.
///
/// History: design-decisions.md, "Trims: commands module docs", record_extraction_observation.
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
/// this project's `memory_files` — migration 17. `paths` is what the git
/// index said differed from the working tree when extraction began: an
/// observation about the **session**, not capability-map line 1139's *"the
/// files a memory explicitly references"* (this path's model input carries
/// no prose to reference from), so every row carries
/// [`glasshouse::memory::FileAssociation::Observed`].
///
/// Opened here, not beside the event log, for
/// [`record_extraction_observation`]'s reason: an open handle on a path
/// with nothing to write blocks a later writer under Windows, so the guard
/// comes first. Runs on the calling thread, not the extraction thread,
/// which outlives its bound and would otherwise open a second writable
/// handle at an unpredictable moment.
///
/// A failure here is one log line that counts lost associations and never
/// names the files: a file path is the user's own data.
///
/// History: design-decisions.md, "Trims: commands module docs", record_observed_files.
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
