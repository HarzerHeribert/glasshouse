use crate::Runtime;

use super::{
    EvaluationKind, EvaluationObservations, EvaluationOutcome, NewObservation, RetrievalScope,
    TURN_COMPLETED, TURN_FAILED,
};

/// Record that a memory search handed these memories back — the producer for
/// map lines 1822 and 1826, and — when `session_id` is carried — the
/// [`EvaluationKind::MemoryRetrieved`] half of map lines 1821 and 1831's own
/// proxy join (this reader block's own doc comment names the other half).
///
/// **This never fails a retrieval.** Memory search is on the user's path and
/// bookkeeping is not allowed to break it, so every error here is a
/// `tracing::warn!` and a return. The database handle is opened here, and
/// only here, and only when there is something to record (practice §65).
/// `session_id` is `None` whenever the caller has no session in scope —
/// never guessed.
///
/// History: design-decisions.md, "Trims: the remaining module docs, second
/// packet", `record_memory_retrieval`.
pub fn record_memory_retrieval<'a>(
    runtime: &Runtime,
    scope: RetrievalScope,
    memory_ids: impl IntoIterator<Item = &'a str>,
    session_id: Option<&str>,
    observed_at_unix: i64,
) {
    let observations: Vec<NewObservation> = memory_ids
        .into_iter()
        .map(|id| {
            let mut observation = NewObservation::new(EvaluationKind::MemoryRetrieved)
                .with_subject(scope.as_str())
                .with_memory_id(id);
            if let Some(session_id) = session_id {
                observation = observation.with_session_id(session_id);
            }
            observation
        })
        .collect();
    if observations.is_empty() {
        return;
    }

    let ledger = match EvaluationObservations::open(runtime) {
        Ok(ledger) => ledger,
        Err(err) => {
            tracing::warn!(
                error = %err,
                "could not open the evaluation ledger; the retrieval stands, \
                 but it was not counted"
            );
            return;
        }
    };
    if let Err(err) = ledger.record_all(&observations, observed_at_unix) {
        tracing::warn!(
            error = %err,
            "could not record a memory retrieval; the retrieval stands, but it \
             was not counted"
        );
    }
}

/// Record that a memory search on a production door matched nothing at all —
/// the miss counterpart of [`record_memory_retrieval`], and the producer map
/// line 1865 needs: *"do not add vector retrieval until FTS5 retrieval
/// failures are observed and recorded in real projects."*
///
/// **This never fails a search or a launch**, for the same reason
/// [`record_memory_retrieval`] does not: bookkeeping is not allowed to break
/// the door it is counting. Every error here is a `tracing::warn!` and a
/// return.
///
/// The database handle is opened here, and only here — practice §65's rule
/// that a resource is acquired where its consumer starts, applied to a door
/// that returned nothing rather than one that returned something. Every
/// caller of this function must have already dropped its memory connection
/// before calling it, for the same reason [`record_memory_retrieval`]'s own
/// callers do.
pub fn record_memory_retrieval_miss(
    runtime: &Runtime,
    scope: RetrievalScope,
    observed_at_unix: i64,
) {
    let ledger = match EvaluationObservations::open(runtime) {
        Ok(ledger) => ledger,
        Err(err) => {
            tracing::warn!(
                error = %err,
                "could not open the evaluation ledger; the retrieval miss was \
                 not counted"
            );
            return;
        }
    };
    let observation =
        NewObservation::new(EvaluationKind::MemoryRetrievalMiss).with_subject(scope.as_str());
    if let Err(err) = ledger.record(observation, observed_at_unix) {
        tracing::warn!(
            error = %err,
            "could not record a memory retrieval miss"
        );
    }
}

/// Record a person's or an agent's own verdict on a memory Glasshouse
/// retrieved — the producer for [`EvaluationKind::MemoryRated`], and
/// `glasshouse memory rate`'s one write. Returns the appended `seq`. Unlike
/// [`record_memory_retrieval`] and its neighbours, this is allowed to fail
/// loudly: it has no door to protect, it *is* the command, and a rating that
/// silently failed to record would tell its caller their verdict was kept
/// when it was not. `memory_id` is trusted to have already been resolved
/// against this project's own store. Before writing, this looks up the
/// [`RetrievalScope`] of the retrieval the rating is about and copies it
/// onto the row's own `subject` (map line 939), so `false positives by
/// retrieval scope` can be read out per scope; a lookup failure fails the
/// command exactly as a write failure does.
///
/// History: design-decisions.md, "Trims: the remaining module docs, second
/// packet", `record_memory_rating`.
pub fn record_memory_rating(
    runtime: &Runtime,
    memory_id: &str,
    verdict: EvaluationOutcome,
    session_id: Option<&str>,
    note: Option<&str>,
    observed_at_unix: i64,
) -> anyhow::Result<i64> {
    let ledger = EvaluationObservations::open(runtime)?;
    let scope = ledger.most_recent_retrieval_scope(memory_id, session_id)?;

    let mut observation = NewObservation::new(EvaluationKind::MemoryRated)
        .with_memory_id(memory_id)
        .with_outcome(verdict);
    if let Some(scope) = scope {
        observation = observation.with_subject(scope);
    }
    if let Some(session_id) = session_id {
        observation = observation.with_session_id(session_id);
    }
    if let Some(note) = note {
        observation = observation.with_detail(note);
    }
    Ok(ledger.record(observation, observed_at_unix)?)
}

/// Record that `glasshouse memory revalidate` ran — the producer for
/// [`EvaluationKind::MemoryRevalidated`], map line 1824's own denominator.
/// Its one caller (`main.rs::memory_revalidate`) calls this after the store
/// has already written the outcome, so a ledger failure here can never leave
/// a revalidation half-applied.
///
/// **Never fails the command**, the same shape [`record_memory_retrieval`]
/// and its neighbours use rather than [`record_memory_rating`]'s: the store
/// mutation is the real act and has already succeeded by the time this runs,
/// so a bookkeeping error here must not turn a successful `memory revalidate`
/// into a failed command exit.
///
/// `outcome` is the CLI's own word (`reaffirmed`, `needs-review`,
/// `superseded` or `invalidated`), stored verbatim as `subject` — this
/// producer does not judge whether the revalidation was correct, only that
/// it happened.
pub fn record_memory_revalidation(
    runtime: &Runtime,
    memory_id: &str,
    outcome: &str,
    observed_at_unix: i64,
) {
    let observation = NewObservation::new(EvaluationKind::MemoryRevalidated)
        .with_memory_id(memory_id)
        .with_subject(outcome);
    let ledger = match EvaluationObservations::open(runtime) {
        Ok(ledger) => ledger,
        Err(err) => {
            tracing::warn!(
                error = %err,
                "could not open the evaluation ledger; the revalidation stands, but it was not \
                 counted"
            );
            return;
        }
    };
    if let Err(err) = ledger.record(observation, observed_at_unix) {
        tracing::warn!(
            error = %err,
            "could not record that a memory revalidation happened"
        );
    }
}

/// Which of the two [`crate::events::TurnOutcome`] a row records.
///
/// An exhaustive `match` at the single writer, for [`EvaluationKind`]'s own
/// reason: a third outcome added to that enum must be a compile error here
/// rather than a row silently recorded as one of the two that already exist.
fn turn_subject(outcome: crate::events::TurnOutcome) -> &'static str {
    match outcome {
        crate::events::TurnOutcome::Completed => TURN_COMPLETED,
        crate::events::TurnOutcome::Failed => TURN_FAILED,
    }
}

/// Record what the harness said about one turn of **any** session that runs
/// the hook — the producer for [`EvaluationKind::TurnOutcomeObserved`], and
/// map lines 1821 and 1831's proxy denominator. Its one caller is `main.rs`'s
/// `glasshouse hook` handler, on the `TurnEnded` arm.
///
/// History: design-decisions.md, "Trims: the remaining module docs, second
/// packet", `record_turn_outcome`.
pub fn record_turn_outcome(
    runtime: &Runtime,
    session_id: &str,
    outcome: crate::events::TurnOutcome,
    observed_at_unix: i64,
) {
    let ledger = match EvaluationObservations::open(runtime) {
        Ok(ledger) => ledger,
        Err(err) => {
            tracing::warn!(
                error = %err,
                "could not open the evaluation ledger; the turn ended, but its outcome was \
                 not counted"
            );
            return;
        }
    };
    let observation = NewObservation::new(EvaluationKind::TurnOutcomeObserved)
        .with_subject(turn_subject(outcome))
        .with_session_id(session_id);
    if let Err(err) = ledger.record(observation, observed_at_unix) {
        tracing::warn!(
            error = %err,
            "could not record a turn's outcome; the turn ended, but it was not counted"
        );
    }
}

/// What a hook-triggered memory extraction did, for
/// [`record_memory_extraction`] to describe.
///
/// Not `Option<&ExtractionOutcome>` alone: a run that never produced an
/// outcome is one of two structurally distinct causes — `run_extraction`'s
/// own preparation failure, or its bound expiring — and `ExtractionOutcome`
/// has no field that could carry the difference for a run that never made
/// one. The caller already knows which, from its own elapsed time against its
/// own bound, so it states it here rather than this ledger guessing.
#[derive(Debug)]
pub enum ExtractionObservation<'a> {
    /// Extraction produced an outcome — stored, rejected, or a named failure
    /// all count as "ran" here; `outcome.failure` carries which.
    Ran(&'a crate::memory::extract::ExtractionOutcome),
    /// `run_extraction` answered [`None`]: preparation failed before a model
    /// was ever asked, or the binary crate's hook-side bound expired while
    /// waiting on one. `bound_expired` is `true` only for the second case.
    NoOutcome { bound_expired: bool },
}

/// Record how one hook-triggered memory extraction ended — the producer for
/// [`EvaluationKind::MemoryExtractionObserved`], dogfooding 2026-09-06 finding
/// 4: extraction routed to a resource and then nothing durable said whether
/// the model answered, timed out, or returned nothing worth storing.
///
/// **This never fails a turn**: its
/// one caller is the binary crate's `commands::memory_extraction::hook_extraction`,
/// on the harness's own gate, so every error here is a `tracing::warn!` and a
/// return. `subject` is `trigger`, the [`crate::memory::extract::ExtractionTrigger::as_str`]
/// word; `detail` is built here from [`ExtractionObservation`] and
/// `elapsed_ms` — the model's own rendered description plus counts
/// (`.len()`, never the items themselves) for a run with no failure, the
/// model description plus [`crate::memory::extract::ExtractionFailure`]'s
/// fixed `Display` phrase for one with a failure, or `"no outcome"` and which
/// of preparation failing or the bound expiring it was, for a run that
/// produced neither. **No memory body, activity line, provider response body
/// or credential value ever reaches `detail`**: nothing here reads a memory's
/// text, a rejection's rendered message, or an activity line — only lengths,
/// a fixed phrase, a rendered model description and a duration.
///
/// One row per hook-triggered extraction, whatever it did — **including
/// `NothingToExtract`**, which the hook's own stderr notice stays silent for
/// on purpose (a warning on every empty compaction would teach people to
/// ignore it) but which this ledger still records, so a reader can tell
/// "nothing to extract" from "extraction never ran". Never called from
/// `glasshouse memory commit` (`ExtractionTrigger::Manual` prints its own
/// report in front of a person watching; this row is for the triggers
/// nobody is watching).
pub fn record_memory_extraction(
    runtime: &Runtime,
    session_id: &str,
    trigger: &str,
    observation: ExtractionObservation<'_>,
    elapsed_ms: u128,
    observed_at_unix: i64,
) {
    let detail = match observation {
        ExtractionObservation::Ran(outcome) => match &outcome.failure {
            None => format!(
                "{}: recorded {}, lowered {}, speculative {}, duplicates {}, rejected {}; \
                 {elapsed_ms} ms",
                outcome.model,
                outcome.stored(),
                outcome.lowered.len(),
                outcome.speculative,
                outcome.duplicates,
                outcome.rejected.len(),
            ),
            Some(failure) => format!("{}: {failure}; {elapsed_ms} ms", outcome.model),
        },
        ExtractionObservation::NoOutcome { bound_expired } => {
            let reason = if bound_expired {
                "the bound expired"
            } else {
                "preparation failed"
            };
            format!("no outcome ({reason}): {elapsed_ms} ms")
        }
    };

    let observation = NewObservation::new(EvaluationKind::MemoryExtractionObserved)
        .with_subject(trigger)
        .with_session_id(session_id)
        .with_detail(detail);

    let ledger = match EvaluationObservations::open(runtime) {
        Ok(ledger) => ledger,
        Err(err) => {
            tracing::warn!(
                error = %err,
                "could not open the evaluation ledger; this extraction's outcome was not counted"
            );
            return;
        }
    };
    if let Err(err) = ledger.record(observation, observed_at_unix) {
        tracing::warn!(
            error = %err,
            "could not record a memory extraction's outcome"
        );
    }
}
