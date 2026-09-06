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

/// Record the rationale behind one disposable-job routing decision — the
/// producer for [`EvaluationKind::DisposableRouteDecided`].
///
/// **This never fails a turn.** Its one caller is `glasshouse hook`, whose
/// non-zero exit Claude Code treats as a veto on the user's prompt, so every
/// error here is a `tracing::warn!` and a return — a sharper version of
/// [`record_memory_retrieval`]'s reason, since a turn that went unsent costs
/// the user their words. `subject` is the job kind's own name and `detail`
/// is `rationale` verbatim. `routing_seq`, `memory_id`, `feature` and `arm`
/// stay absent: this path makes no `routing_observations` row (the
/// disposable policy calls no model), and map line 1294's rule is that a
/// fabricated value here would invert the policy rather than merely
/// degrade it.
///
/// History: design-decisions.md, "Trims: the remaining module docs, second
/// packet", `record_disposable_route`.
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
/// 1757 and 1766. Its callers are `main.rs::launch_session`'s same two
/// routed exits [`record_routed_session`] has. `subject` is `destination_id`;
/// `detail` is `explanation.contributions()` as a compact JSON array of
/// `{name, magnitude, evidence}`, built through this module's own
/// `encode_route_contributions` rather than
/// [`crate::routing::RoutingExplanation::render`], because 1766 ranks by
/// magnitude and a rendered string cannot be ranked. This never fails a
/// launch: it is on a person's own command path and a rationale row is not
/// worth a session.
///
/// History: design-decisions.md, "Trims: the remaining module docs, second
/// packet", `record_session_route`.
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
/// half. Its callers are `main.rs::launch_session`'s same two routed exits
/// [`record_session_route`] has. Written only when there is a real median:
/// `median_output_tokens` is the caller's own
/// [`crate::routing::burn::ClassOutput::median_output_tokens`], already
/// `None` below [`crate::routing::evidence::MIN_SAMPLE_FOR_SUMMARY`]
/// comparable rows, and this records nothing rather than a fabricated zero.
/// `subject` is `task_class.as_str()`; `detail` is the rounded token count as
/// decimal text, never a raw float, so
/// [`crate::routing::evidence::EvidenceLedger::output_estimate_accuracy`]
/// parses it back without a locale-dependent format. Never fails a launch.
///
/// History: design-decisions.md, "Trims: the remaining module docs, second
/// packet", `record_routing_consumption_estimate`.
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
/// line 1837. Its callers are `commands::launch`'s two routed exits, called
/// beside [`record_routing_consumption_estimate`]. Written only when the
/// tier is [`crate::routing::classify::WorkloadTier::Heavy`] or `::Frontier`
/// — the tiers the reserve exists to protect; a launch at or below
/// `::Standard`, or unclassified, returns without writing, since *needed* is
/// the line's own word. `subject` is the destination's band or the honest
/// `"unknown"`, never a fabricated one; `detail` is the tier word. Never
/// fails a launch, for the same reason its neighbours do not.
///
/// History: design-decisions.md, "Trims: the remaining module docs, second
/// packet", `record_reserve_availability`.
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
/// Its one caller is `main.rs::launch_session`, on the person's own command
/// path, so this never fails a launch, exactly as
/// [`record_disposable_route`]. Two rows, always together: `subject` carries
/// the boolean-shaped fact each line asks about and `detail` carries a
/// destination id, never a file path, prompt text, or credential.
/// `session_id` is left absent on both — a fresh launch has not minted one
/// yet at this point, and filling it on one branch and not the other would
/// make its absence look like a fact about the decision rather than about
/// when the row was written.
///
/// History: design-decisions.md, "Trims: the remaining module docs, second
/// packet", `record_routing_decision`.
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
/// [`EvaluationKind::RoutingEvidenceObserved`], map lines 1835 and 1854. Its
/// callers are `main.rs::launch_session`'s two routed exits. Never fails a
/// launch, exactly as [`record_routing_decision`]. `cost` is [`None`] when no
/// production fact states the destination's class, recorded as
/// [`UNKNOWN_COST_CLASS`]; `evidence` is whether the pool the router was
/// handed held a reading for this destination. Both rows carry only ids and
/// vocabulary words.
///
/// History: design-decisions.md, "Trims: the remaining module docs, second
/// packet", `record_routed_session`.
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
/// half of map lines 1834, 1835, 1845 and 1854. Its one caller is `main.rs`'s
/// `glasshouse hook` handler, on the arm that has already translated the
/// harness's event into [`crate::events::LifecycleEvent::TurnEnded`] —
/// nothing else may call it, because nothing else in this build holds a
/// verdict a harness actually stated. [`EvaluationObservations::routed_destination`]
/// answering [`None`] means this session was never attributed to a route,
/// and that is a `debug` line and no row, never an invented decision. The
/// hook is a separate process spawned on every event, so the lookup and the
/// write share the one handle this function opens (practice §65) — never a
/// second, background handle.
///
/// History: design-decisions.md, "Trims: the remaining module docs, second
/// packet", `record_routing_outcome`.
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
/// map lines 1821 and 1831's proxy denominator. Its one caller is `main.rs`'s
/// `glasshouse hook` handler, on the same `TurnEnded` arm
/// [`record_routing_outcome`] reads — called first, so a session this ledger
/// has never routed still gets an outcome row. Unlike
/// [`record_routing_outcome`], this makes no claim about a route, so it
/// writes unconditionally, a door-spawned session included (refusal
/// register, *"Phase 51's memory proxy — 1821 and 1831"*, option (b)).
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

/// Record what the failure-domain term did to one gateway failover's ranking
/// — the producer for [`EvaluationKind::FailoverPrevented`], **map line
/// 1851**. Its one caller is the sink `main.rs::launch_session` hands the
/// gateway, invoked from the exchange thread that ranked the failover —
/// nothing else may call it, since the comparison can only be made inside
/// [`crate::routing::interactive::InteractiveRouting::on_provider_failure`].
/// The handle is opened only once a failover has actually been decided and
/// closed before this returns, so no connection is held across the provider
/// hop. Never fails an exchange, exactly as [`record_routed_session`] and
/// [`record_routing_outcome`].
///
/// History: design-decisions.md, "Trims: the remaining module docs, second
/// packet", `record_failover_prevention`.
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
/// **This never fails a turn**, exactly as [`record_disposable_route`]: its
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
