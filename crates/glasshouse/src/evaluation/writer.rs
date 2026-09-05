use crate::Runtime;

use super::{
    EvaluationKind, EvaluationObservations, EvaluationOutcome, FailoverPrevention, NewObservation,
    RetrievalScope, RoutingEvidence, RoutingTier, TURN_COMPLETED, TURN_FAILED, UNKNOWN_COST_CLASS,
};

/// Record that a memory search handed these memories back — the producer for
/// map lines 1822 and 1826, and — when `session_id` is carried — the
/// [`EvaluationKind::MemoryRetrieved`] half of map lines 1821 and 1831's own
/// proxy join (this reader block's own doc comment names the other half).
///
/// **This never fails a retrieval.** Memory search is on the user's path and
/// bookkeeping is not allowed to break it, so every error here is a
/// `tracing::warn!` and a return: the caller gets its results whether or not
/// the ledger could be written.
///
/// The database handle is opened here, and only here, and only when there is
/// something to record — practice §65's rule that a resource is acquired where
/// its consumer starts. A search that returned nothing opens nothing.
///
/// `session_id` is `None` whenever the caller has no session in scope —
/// never guessed. `GH-RETRIEVAL-ATTRIBUTION`'s two production callers today:
/// `main.rs::memory_search_grouped` passes `None` from the CLI's `memory
/// search` (no session to attribute a person's own command to) and from
/// `api::unix::query_memory` (the machine door's `QueryMemory` request
/// carries no session field to thread one from — see that caller's own doc
/// comment); `api::unix::deliver_memory` passes `Some` on every successful
/// launch-time injection, because that door already holds the `SessionId`
/// it is briefing.
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

/// Record the rationale behind one disposable-job routing decision — the
/// producer for [`EvaluationKind::DisposableRouteDecided`].
///
/// **This never fails a turn.** Its one caller is `glasshouse hook`, which
/// runs inside the user's coding session and whose non-zero exit Claude Code
/// treats as a veto on the user's prompt (see `main.rs::report_hook`). So
/// every error here is a `tracing::warn!` and a return, exactly as
/// [`record_memory_retrieval`] is, and for a sharper version of the same
/// reason: a retrieval that went uncounted cost a count, and a turn that went
/// unsent costs the user their words.
///
/// The handle is opened here, and only here, and only when there is something
/// to record — practice §65's rule that a resource is acquired where its
/// consumer starts. A decision with nothing to say about itself opens no
/// database.
///
/// # What is stored, and what is left absent
///
/// `subject` is the job kind's own name and `detail` is `rationale` verbatim:
/// the string the routing decision produced, not a re-derivation of it. The
/// caller passes what production already renders, so what the ledger holds is
/// what the decision said.
///
/// `routing_seq` is **absent, and stays absent.** This path makes no
/// `routing_observations` row — the disposable policy calls no model, so
/// there is no exchange to measure — and a `seq` pointing at some other
/// turn's measurement would be worse than no provenance at all. Map line
/// 1294's standing refusal is the rule: *a fabricated value here does not
/// degrade the policy, it inverts it.* `memory_id`, `feature` and `arm` are
/// absent for the same reason: this decision is about none of them.
pub fn record_disposable_route(
    runtime: &Runtime,
    job: crate::routing::disposable::JobKind,
    session_id: &str,
    rationale: &str,
    observed_at_unix: i64,
) {
    if rationale.trim().is_empty() {
        return;
    }

    let observation = NewObservation::new(EvaluationKind::DisposableRouteDecided)
        .with_subject(job.as_str())
        .with_session_id(session_id)
        .with_detail(rationale);

    let ledger = match EvaluationObservations::open(runtime) {
        Ok(ledger) => ledger,
        Err(err) => {
            tracing::warn!(
                error = %err,
                "could not open the evaluation ledger; the routing decision stands, but its \
                 rationale was not recorded"
            );
            return;
        }
    };
    if let Err(err) = ledger.record(observation, observed_at_unix) {
        tracing::warn!(
            error = %err,
            "could not record a disposable routing decision; the decision stands, but its \
             rationale was not recorded"
        );
    }
}

/// Encode `explanation`'s contributions as a compact JSON array of
/// `{"name", "magnitude", "evidence"}`, by hand.
///
/// **No general-purpose serializer here, deliberately.** This module's own
/// header says so: *"no `export`, no `to_json`, no `write_to`, no
/// serialization of an observation to anything outside the process"* — map
/// line 1856's other half, structural rather than advisory, and this
/// module's own pinning test fails the build the moment such a dependency
/// reappears here. `detail` is still a JSON string, because 1766 needs to
/// rank contributions by magnitude and a rendered sentence cannot be ranked
/// — but this ledger writes it itself rather than reaching for a crate
/// whose surface is far wider than one array of three fields.
fn encode_route_contributions(explanation: &crate::routing::RoutingExplanation) -> String {
    let mut out = String::from("[");
    for (index, contribution) in explanation.contributions().iter().enumerate() {
        if index > 0 {
            out.push(',');
        }
        out.push_str("{\"name\":");
        push_json_string(&mut out, contribution.name());
        out.push_str(",\"magnitude\":");
        push_json_number(&mut out, contribution.magnitude());
        out.push_str(",\"evidence\":");
        push_json_string(&mut out, contribution.evidence());
        out.push('}');
    }
    out.push(']');
    out
}

/// A JSON number cannot spell NaN or an infinity; a routing score never
/// produces either, but a value that somehow did degrades to `0` rather
/// than writing a `detail` [`route_contributions`] could not parse back.
fn push_json_number(out: &mut String, value: f64) {
    if value.is_finite() {
        out.push_str(&value.to_string());
    } else {
        out.push('0');
    }
}

/// A JSON string literal, escaped by hand — the same six escapes
/// [`route_contributions`]'s reader decodes.
fn push_json_string(out: &mut String, value: &str) {
    out.push('"');
    for ch in value.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                out.push_str(&format!("\\u{:04x}", c as u32));
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

/// Record why a launch's session-boundary routing chose the destination it
/// did — the producer for [`EvaluationKind::SessionRouteDecided`], map lines
/// 1757 and 1766.
///
/// Its callers are `main.rs::launch_session`'s same two routed exits
/// [`record_routed_session`] has — called right beside it, with the same
/// `session_id` and the same `observed_at_unix`.
///
/// **This never fails a launch**, exactly as [`record_routed_session`] does
/// not: it is on a person's own command path and a rationale row is not
/// worth a session.
///
/// # What is stored
///
/// `subject` is `destination_id`. `detail` is `explanation.contributions()`
/// as a compact JSON array of `{name, magnitude, evidence}`, in the
/// explanation's own order — built through this module's own
/// `encode_route_contributions`, never through `routing`'s own
/// [`crate::routing::RoutingExplanation::render`], because 1766 ranks by
/// magnitude and a rendered string cannot be ranked. An explanation with no
/// contributions still writes a row, `detail` `"[]"`: the decision happened
/// even when nothing weighed in.
pub fn record_session_route(
    runtime: &Runtime,
    session_id: &str,
    destination_id: &str,
    explanation: &crate::routing::RoutingExplanation,
    observed_at_unix: i64,
) {
    let detail = encode_route_contributions(explanation);

    let observation = NewObservation::new(EvaluationKind::SessionRouteDecided)
        .with_subject(destination_id)
        .with_session_id(session_id)
        .with_detail(detail);

    let ledger = match EvaluationObservations::open(runtime) {
        Ok(ledger) => ledger,
        Err(err) => {
            tracing::warn!(
                error = %err,
                "could not open the evaluation ledger; the session is routed, but its rationale \
                 was not recorded"
            );
            return;
        }
    };
    if let Err(err) = ledger.record(observation, observed_at_unix) {
        tracing::warn!(
            error = %err,
            "could not record a session's routing rationale; the session is routed, but its \
             rationale will not be shown"
        );
    }
}

/// Record a launch's own expected output-token size for the session it
/// produced — the producer for
/// [`EvaluationKind::RoutingConsumptionEstimated`], map line 1855's token
/// half.
///
/// Its callers are `main.rs::launch_session`'s same two routed exits
/// [`record_session_route`] has — called right beside it, with the same
/// `session_id` and the same `observed_at_unix`.
///
/// **Written only when there is a real median to write.** `median_output_tokens`
/// is the caller's own
/// [`crate::routing::burn::ClassOutput::median_output_tokens`] for this
/// launch's task class, already `None` below
/// [`crate::routing::evidence::MIN_SAMPLE_FOR_SUMMARY`] comparable rows — a
/// launch with no comparable rows for its class calls this with nothing to
/// write, and this records nothing rather than a fabricated zero. This
/// function does not re-derive the median itself: the caller already read
/// it from the same evidence ledger this row is about, and re-reading it
/// here would be a second, possibly different, read of the same window.
///
/// **This never fails a launch**, exactly as [`record_session_route`] does
/// not: it is on a person's own command path and an estimate row is not
/// worth a session.
///
/// # What is stored
///
/// `subject` is `task_class.as_str()`. `detail` is `median_output_tokens`,
/// rounded to the nearest whole token, as decimal text — never a raw float
/// string, so [`crate::routing::evidence::EvidenceLedger::output_estimate_accuracy`]
/// can parse it back without a locale-dependent format. `session_id` is the
/// session the decision produced.
pub fn record_routing_consumption_estimate(
    runtime: &Runtime,
    session_id: &str,
    task_class: crate::routing::request::TaskClass,
    median_output_tokens: f64,
    observed_at_unix: i64,
) {
    let observation = NewObservation::new(EvaluationKind::RoutingConsumptionEstimated)
        .with_subject(task_class.as_str())
        .with_session_id(session_id)
        .with_detail(format!("{}", median_output_tokens.round() as i64));

    let ledger = match EvaluationObservations::open(runtime) {
        Ok(ledger) => ledger,
        Err(err) => {
            tracing::warn!(
                error = %err,
                "could not open the evaluation ledger; the session is routed, but its expected \
                 output-token size was not recorded"
            );
            return;
        }
    };
    if let Err(err) = ledger.record(observation, observed_at_unix) {
        tracing::warn!(
            error = %err,
            "could not record a session's expected output-token size"
        );
    }
}

/// Record whether protected quota remained available for a high-tier launch
/// — the producer for [`EvaluationKind::ReserveAvailabilityObserved`], map
/// line 1837.
///
/// Its callers are `commands::launch`'s two routed exits, called beside
/// [`record_routing_consumption_estimate`] on the same `observed_at_unix`.
///
/// **Written only when the tier is [`crate::routing::classify::WorkloadTier::Heavy`]
/// or [`crate::routing::classify::WorkloadTier::Frontier`]** — the tiers the
/// reserve exists to protect. A launch classified at or below
/// [`crate::routing::classify::WorkloadTier::Standard`], or not classified at
/// all, returns without writing: *needed* is the line's own word, and a row
/// for routine support work would answer a question line 1837 does not ask.
///
/// # What is stored
///
/// `subject` is `band.map(CapacityBand::as_str).unwrap_or("unknown")` — the
/// destination's own band when the router read one, and the honest
/// `"unknown"` bucket when it did not, never a fabricated band. `detail` is
/// the tier word ([`RoutingTier::as_str`]); `session_id` is the launched
/// session's.
///
/// **This never fails a launch**, exactly as [`record_routing_consumption_estimate`]
/// does not: it is on a person's own command path and an evaluation row is
/// not worth a session.
pub fn record_reserve_availability(
    runtime: &Runtime,
    session_id: &str,
    tier: RoutingTier,
    band: Option<crate::provider::quota::CapacityBand>,
    observed_at_unix: i64,
) {
    use crate::routing::classify::WorkloadTier;

    let workload_tier = match tier {
        RoutingTier::Unclassified => None,
        RoutingTier::Classified { tier, .. } => Some(tier),
    };
    match workload_tier {
        Some(WorkloadTier::Heavy) | Some(WorkloadTier::Frontier) => {}
        _ => return,
    }

    let observation = NewObservation::new(EvaluationKind::ReserveAvailabilityObserved)
        .with_subject(band.map_or("unknown", crate::provider::quota::CapacityBand::as_str))
        .with_session_id(session_id)
        .with_detail(tier.as_str());

    let ledger = match EvaluationObservations::open(runtime) {
        Ok(ledger) => ledger,
        Err(err) => {
            tracing::warn!(
                error = %err,
                "could not open the evaluation ledger; the session is routed, but its \
                 protected-quota reading was not recorded"
            );
            return;
        }
    };
    if let Err(err) = ledger.record(observation, observed_at_unix) {
        tracing::warn!(
            error = %err,
            "could not record a high-tier launch's protected-quota reading"
        );
    }
}

/// Record one launch's session-boundary routing decision — the producer for
/// [`EvaluationKind::RoutingOverrideDecided`] and
/// [`EvaluationKind::RoutingContinuationDecided`], map lines 1829 and 1830.
///
/// **This never fails a launch.** Its one caller is
/// `main.rs::launch_session`, on the person's own command path, so every
/// error here is a `tracing::warn!` and a return, exactly as
/// [`record_disposable_route`] is.
///
/// The handle is opened here, and only here, and only when there is a routed
/// decision to record — practice §65's rule that a resource is acquired
/// where its consumer starts.
///
/// # What is stored, and what is left absent
///
/// Two rows, always together: `destination_id` and `fresh` are known the
/// instant a destination is chosen, so neither one is ever the "nothing
/// meaningful to say" case the way an empty rationale is for
/// [`record_disposable_route`]. `subject` carries the boolean-shaped fact
/// each line asks about and `detail` carries a destination id — never a file
/// path, prompt text, or credential.
///
/// `session_id` is left absent on both rows. A launch that continues an
/// existing session could name it, but a fresh launch has not minted one yet
/// at this point in `launch_session`, and a producer that filled the field on
/// one branch and not the other would make its absence look like a fact
/// about the decision rather than about when the row was written.
pub fn record_routing_decision(
    runtime: &Runtime,
    destination_id: &str,
    fresh: bool,
    overrode: Option<&str>,
    observed_at_unix: i64,
) {
    let mut override_observation = NewObservation::new(EvaluationKind::RoutingOverrideDecided)
        .with_subject(if overrode.is_some() {
            "overridden"
        } else {
            "automatic"
        });
    if let Some(automatic) = overrode {
        override_observation = override_observation.with_detail(automatic);
    }

    let continuation_observation = NewObservation::new(EvaluationKind::RoutingContinuationDecided)
        .with_subject(if fresh { "fresh" } else { "existing" })
        .with_detail(destination_id);

    let observations = [override_observation, continuation_observation];

    let ledger = match EvaluationObservations::open(runtime) {
        Ok(ledger) => ledger,
        Err(err) => {
            tracing::warn!(
                error = %err,
                "could not open the evaluation ledger; the routing decision stands, but it was \
                 not counted"
            );
            return;
        }
    };
    if let Err(err) = ledger.record_all(&observations, observed_at_unix) {
        tracing::warn!(
            error = %err,
            "could not record a routing decision; the decision stands, but it was not counted"
        );
    }
}

/// Which of the two [`crate::events::TurnOutcome`] a row records.
///
/// An exhaustive `match` at the single writer, for
/// [`EvaluationKind`]'s own reason: a third outcome added to that enum must
/// be a compile error here rather than a row silently recorded as one of the
/// two that already exist.
fn turn_subject(outcome: crate::events::TurnOutcome) -> &'static str {
    match outcome {
        crate::events::TurnOutcome::Completed => TURN_COMPLETED,
        crate::events::TurnOutcome::Failed => TURN_FAILED,
    }
}

/// Attribute a launch's routing decision to the session it produced — the
/// producer for [`EvaluationKind::RoutingCostClassObserved`] and
/// [`EvaluationKind::RoutingEvidenceObserved`], map lines 1835 and 1854.
///
/// Its callers are `main.rs::launch_session`'s two routed exits: the branch
/// that continues a warm session, where the destination's id *is* the session
/// id, and the branch that creates a fresh session record, called once that
/// record exists and its id is real.
///
/// **This never fails a launch**, exactly as [`record_routing_decision`]
/// never does, and for the same reason: it is on a person's own command path
/// and an evaluation row is not worth a session.
///
/// # What is stored
///
/// `cost` is [`None`] when no production fact states the destination's class
/// — a harness's own sign-in has no configured provider and no marked model —
/// and that is recorded as [`UNKNOWN_COST_CLASS`], its own bucket in every
/// reader here. `evidence` is whether the pool the router was handed held a
/// reading for this destination, which is the only thing about the router's
/// inputs that can be stated on this path.
///
/// Both rows carry ids and vocabulary words and nothing else: a destination
/// id, a session id, and one word from a closed list.
pub fn record_routed_session(
    runtime: &Runtime,
    session_id: &str,
    destination_id: &str,
    cost: Option<crate::routing::Cost>,
    evidence: RoutingEvidence,
    tier: RoutingTier,
    observed_at_unix: i64,
) {
    let class = NewObservation::new(EvaluationKind::RoutingCostClassObserved)
        .with_subject(cost.map_or(UNKNOWN_COST_CLASS, |cost| cost.as_str()))
        .with_session_id(session_id)
        .with_detail(destination_id);
    let evidence = NewObservation::new(EvaluationKind::RoutingEvidenceObserved)
        .with_subject(evidence.as_str())
        .with_session_id(session_id)
        .with_detail(destination_id);
    // Map line 1834's third row, written in the same call and therefore
    // through the same one handle: a tier that reached the ledger a moment
    // later would be a second open on a person's own launch path, which is
    // the whole of practice §65's finding.
    let mut tier_row = NewObservation::new(EvaluationKind::RoutingTierObserved)
        .with_subject(tier.as_str())
        .with_session_id(session_id);
    if let Some(stated) = tier.stated_tier() {
        tier_row = tier_row.with_detail(stated.as_str());
    }

    let ledger = match EvaluationObservations::open(runtime) {
        Ok(ledger) => ledger,
        Err(err) => {
            tracing::warn!(
                error = %err,
                "could not open the evaluation ledger; the session is routed, but its route \
                 was not attributed to it"
            );
            return;
        }
    };
    if let Err(err) = ledger.record_all(&[class, evidence, tier_row], observed_at_unix) {
        tracing::warn!(
            error = %err,
            "could not attribute a route to its session; the session is routed, but its route \
             will not be counted"
        );
    }
}

/// Record what the harness said about one turn of a routed session — the
/// producer for [`EvaluationKind::RoutingOutcomeObserved`], and the outcome
/// half of map lines 1834, 1835, 1845 and 1854.
///
/// Its one caller is `main.rs`'s `glasshouse hook` handler, on the arm that
/// has already translated the harness's event into
/// [`crate::events::LifecycleEvent::TurnEnded`]. **Nothing else may call it**,
/// because nothing else in this build holds a verdict a harness actually
/// stated: a process exit, output ending and a session going idle are all
/// silence, and silence is not an outcome.
///
/// # A session with no routing decision records nothing
///
/// [`EvaluationObservations::routed_destination`] answering [`None`] means
/// this session was never attributed to a route — it predates this build, or
/// it was created by a path that does not route — and there is nothing for an
/// outcome to be *about*. That is a `debug` line and no row, never a row
/// whose decision is invented.
///
/// # One handle, opened here, dropped here (practice §65)
///
/// The hook is a separate process the harness spawns on every event, and an
/// open SQLite handle is free on the developer's machine and billed on
/// Windows. The lookup and the write share the one handle this function
/// opens, and it is opened only after the caller has established that a turn
/// really ended.
pub fn record_routing_outcome(
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
    let destination = match ledger.routed_destination(session_id) {
        Ok(Some(destination)) => destination,
        Ok(None) => {
            tracing::debug!(
                session = session_id,
                "no routing decision is recorded for this session, so its turn outcome is \
                 not attributed to one"
            );
            return;
        }
        Err(err) => {
            tracing::warn!(
                error = %err,
                "could not read this session's routing decision; its turn outcome was not \
                 counted"
            );
            return;
        }
    };

    let mut observation = NewObservation::new(EvaluationKind::RoutingOutcomeObserved)
        .with_subject(turn_subject(outcome))
        .with_session_id(session_id);
    if !destination.is_empty() {
        observation = observation.with_detail(destination);
    }
    if let Err(err) = ledger.record(observation, observed_at_unix) {
        tracing::warn!(
            error = %err,
            "could not record a turn's outcome; the turn ended, but it was not counted"
        );
    }
}

/// Record what the harness said about one turn of **any** session that runs
/// the hook — the producer for [`EvaluationKind::TurnOutcomeObserved`], and
/// map lines 1821 and 1831's proxy denominator.
///
/// Its one caller is `main.rs`'s `glasshouse hook` handler, on the same
/// `TurnEnded` arm [`record_routing_outcome`] reads — called first, so a
/// session this ledger has never routed still gets an outcome row.
///
/// # Unlike `record_routing_outcome`, this asks no question about routing
///
/// [`record_routing_outcome`] refuses to write for a session with no routed
/// destination because that row is a claim about *the route*. This row
/// makes no claim about a route at all — it is the harness's verdict on the
/// session's turn, full stop — so it is written unconditionally, a
/// door-spawned session (never routed) included. Design ruling: refusal
/// register, *"Phase 51's memory proxy — 1821 and 1831"*, option (b).
///
/// # One handle, opened here, dropped here (practice §65)
///
/// Same reasoning as [`record_routing_outcome`]: the hook is a separate
/// process the harness spawns on every event, and the write shares the one
/// handle this function opens.
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

/// Record what the failure-domain term did to one gateway failover's ranking
/// — the producer for [`EvaluationKind::FailoverPrevented`], **map line
/// 1851**.
///
/// Its one caller is the sink `main.rs::launch_session` hands the gateway,
/// invoked from the exchange thread that ranked the failover. Nothing else
/// may call it: the comparison it records can only be made where both
/// rankings exist, which is inside
/// [`crate::routing::interactive::InteractiveRouting::on_provider_failure`],
/// and a row written from anywhere else would be an assertion rather than an
/// observation.
///
/// # One handle, opened here, dropped here (practice §65)
///
/// This runs on a gateway exchange thread inside somebody's coding session.
/// The handle is opened only once a failover has actually been decided —
/// which is a small minority of exchanges — and closed before this returns,
/// so no connection is held across the provider hop and none is opened at all
/// by the exchanges that fail over nowhere.
///
/// **This never fails an exchange.** Every error is one `warn` and a return,
/// exactly as [`record_routed_session`] and [`record_routing_outcome`] do,
/// and for the same reason: the session's own work outranks the books kept
/// about it.
pub fn record_failover_prevention(
    runtime: &Runtime,
    prevention: FailoverPrevention,
    displaced: Option<&str>,
    observed_at_unix: i64,
) {
    let mut observation =
        NewObservation::new(EvaluationKind::FailoverPrevented).with_subject(prevention.as_str());
    if let Some(displaced) = displaced {
        observation = observation.with_detail(displaced);
    }
    let ledger = match EvaluationObservations::open(runtime) {
        Ok(ledger) => ledger,
        Err(err) => {
            tracing::warn!(
                error = %err,
                "could not open the evaluation ledger; the failover happened, but what the \
                 failure-domain term did to it was not counted"
            );
            return;
        }
    };
    if let Err(err) = ledger.record(observation, observed_at_unix) {
        tracing::warn!(
            error = %err,
            "could not record what the failure-domain term did to a failover"
        );
    }
}

/// Record a person's or an agent's own verdict on a memory Glasshouse
/// retrieved — the producer for [`EvaluationKind::MemoryRated`], and
/// `glasshouse memory rate`'s one write. Returns the appended `seq`.
///
/// # This is allowed to fail loudly, unlike every producer above
///
/// [`record_memory_retrieval`] and its neighbours never fail a search or a
/// launch, because bookkeeping must not break the door it is counting. This
/// producer has no door to protect: it *is* the command, typed by a person
/// or issued by an agent as its own last act, and a rating that silently
/// failed to record would tell its caller their verdict was kept when it
/// was not. Its caller (`main.rs::memory_rate`) propagates this with `?` and
/// prints nothing but a failure.
///
/// `memory_id` is trusted to have already been resolved against this
/// project's own store — `glasshouse memory rate`'s project-isolation check
/// runs before this is ever called, the same way `memory_challenge` and
/// `memory_resolve_conflict` resolve an id before acting on it.
///
/// **Carries the scope of the retrieval it judges — map line 939.** Before
/// writing, this looks up the [`RetrievalScope`] of the retrieval the
/// rating is about (`EvaluationObservations`'s own private attribution
/// lookup) and copies it onto the row's own `subject`, so `false positives by
/// retrieval scope` can be read out per scope rather than only per memory.
/// Every verdict is attributed the same way — the scope is a fact about
/// which retrieval produced the memory being rated, not a judgement the
/// verdict itself makes. A memory this rating never saw retrieved carries no
/// scope. **A lookup failure fails the command exactly as a write failure
/// does** — this producer has no door to protect, per this function's own
/// header above.
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

/// Record an operator's or agent's own verdict on a session's route — the
/// producer for [`EvaluationKind::RoutingRated`], and `glasshouse rate-route`'s
/// one write. Returns the appended `seq`.
///
/// **This is allowed to fail loudly**, exactly as [`record_memory_rating`]
/// is and for the same reason: it *is* the command, typed by a person or
/// issued by an agent as its own last act, and a rating that silently failed
/// to record would tell its caller their verdict was kept when it was not.
/// Its caller (`commands::route::rate_route`) propagates this with `?` and
/// prints nothing but a failure.
///
/// # `session_id` must already have a routed destination
///
/// [`EvaluationObservations::routed_destination`] answering [`None`] means
/// this session was never attributed to a route, and this refuses rather
/// than writing a rating with no destination to carry as `subject` — *"one
/// cannot rate a route that was never taken"* (design decision, *"The
/// routing half of RC-B"*, 2026-09-05). This is the same lookup
/// [`record_routing_outcome`] already uses to decide whether to write at
/// all; here the answer decides whether to refuse the command outright.
pub fn record_route_rating(
    runtime: &Runtime,
    session_id: &str,
    verdict: EvaluationOutcome,
    note: Option<&str>,
    observed_at_unix: i64,
) -> anyhow::Result<i64> {
    let ledger = EvaluationObservations::open(runtime)?;
    let Some(destination) = ledger.routed_destination(session_id)? else {
        anyhow::bail!(
            "session `{session_id}` has no recorded route; cannot rate a route that was never \
             taken"
        );
    };

    let mut observation = NewObservation::new(EvaluationKind::RoutingRated)
        .with_subject(destination)
        .with_session_id(session_id)
        .with_outcome(verdict);
    if let Some(note) = note {
        observation = observation.with_detail(note);
    }
    Ok(ledger.record(observation, observed_at_unix)?)
}
