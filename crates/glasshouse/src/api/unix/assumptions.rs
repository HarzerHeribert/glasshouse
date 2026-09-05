use std::sync::Mutex;

use glasshouse::Runtime;
use glasshouse::checkpoint::git::changed_paths;
use glasshouse::config::{self, EffectiveConfig, UserConfig};
use glasshouse::guardrails::{
    self, AppliedOverride, AssumptionStore, AssumptionView, ChangeFactors, GuardrailError,
    GuardrailMode, GuardrailResponse, NewAssumption, NewTransition, Origin, PromotionKind,
    RiskClass, Transition, TransitionKind,
};
use glasshouse::memory::{
    DecisionProvenance, MemoryAuthority, MemoryKind, NewMemory, ProjectMemory,
};
use glasshouse::session::api::SessionApi;
use glasshouse::session::{ProjectSessions, SessionId, SessionRuntime, SessionStore};

use super::checkpoints::request_checkpoint;
use super::memory::memory_error_message;
use super::{api_error, lock};
use crate::api::protocol::Response;

/// The most assumptions one [`Request::ListAssumptions`] returns — Phase
/// 21K. A caller may lower the ceiling and cannot raise it, like every other
/// bound on this door. Two hundred is more than any one session states and
/// small against a ledger that keeps a hundred thousand transitions.
const MAX_ASSUMPTIONS_LIMIT: usize = 200;

/// How many session-level guardrail events (gates, overrides, budgets) a
/// listing for one session carries beside its assumptions.
const SESSION_EVENTS_LIMIT: usize = 20;

// ---------------------------------------------------------------------------
// Phase 21K — the assumption guardrail, over the door
// ---------------------------------------------------------------------------

/// A ledger-side failure this door may put on the wire, as a message.
///
/// [`memory_error_message`]'s rule, for the same reason: `GuardrailError`
/// names identifiers and states and nothing else, while everything else
/// that can surface from opening the project database names the file's
/// absolute path, which does not leave here.
pub(super) fn guardrail_error_message(err: &anyhow::Error) -> String {
    match err.downcast_ref::<GuardrailError>() {
        Some(err) => err.to_string(),
        None => "could not open this project's assumption ledger".to_owned(),
    }
}

/// Resolve a session named on a guardrail request through [`SessionApi`] —
/// this project's, or refused — and hand back its identifier. `None` in,
/// `None` out: the verbs that accept no session write nothing keyed by one.
///
/// This is the door's own check, over and above the ledger's project-scope
/// trigger: the trigger refuses a *row* for another project, and this
/// refuses a *session* of another project before any row is attempted.
fn scoped_session(
    store: &SessionStore<'_>,
    live: &Mutex<SessionRuntime>,
    session: Option<&str>,
) -> Result<Option<SessionId>, String> {
    let Some(session) = session else {
        return Ok(None);
    };
    let id = SessionId::new(session.to_owned());
    let mut guard = lock(live);
    let api = SessionApi::new(store, &mut guard);
    api.state(&id).map_err(api_error)?;
    Ok(Some(id))
}

fn open_ledger(runtime: &Runtime) -> Result<AssumptionStore, String> {
    AssumptionStore::open(runtime).map_err(|err| guardrail_error_message(&err))
}

/// The configured policy — mode and blocking list with their layers — for
/// this project, with no per-task override yet.
fn guardrail_policy(runtime: &Runtime) -> Result<guardrails::Policy, String> {
    let user = UserConfig::load(runtime.paths()).map_err(|err| err.to_string())?;
    let project_config =
        config::load_project_config(runtime.project()).map_err(|err| err.to_string())?;
    Ok(EffectiveConfig::new(&user, project_config.as_ref()).guardrail_policy())
}

/// `Request::Preflight` — see the protocol's own doc comment for the
/// contract. Stateless without a session; with one, the gate is recorded,
/// a budget found exceeded is recorded and the open assumptions are listed
/// for re-evaluation, and a substantial change takes a checkpoint.
pub(super) fn preflight(
    runtime: &Runtime,
    sessions: &ProjectSessions,
    live: &Mutex<SessionRuntime>,
    session: Option<&str>,
    change: ChangeFactors,
) -> Response {
    let store = sessions.store();
    let session = match scoped_session(&store, live, session) {
        Ok(session) => session,
        Err(err) => return Response::err(err),
    };
    let mut policy = match guardrail_policy(runtime) {
        Ok(policy) => policy,
        Err(err) => return Response::err(err),
    };

    // The per-task override, from the session's own ledger — and the handle
    // is kept for the rows written below, then dropped before the
    // checkpoint path opens its own.
    let mut ledger = match &session {
        Some(_) => match open_ledger(runtime) {
            Ok(ledger) => Some(ledger),
            Err(err) => return Response::err(err),
        },
        None => None,
    };
    if let (Some(id), Some(ledger)) = (&session, &ledger) {
        match ledger.latest_override(id.as_str()) {
            Ok(Some((kind, row))) => {
                policy.override_ = Some(AppliedOverride {
                    kind,
                    origin: row.origin,
                    seq: row.seq,
                });
            }
            Ok(None) => {}
            Err(err) => return Response::err(err.to_string()),
        }
    }

    let answer = guardrails::preflight(&change, &policy);
    let description = change
        .description
        .as_deref()
        .map(|text| guardrails::sanitize(text, guardrails::MAX_DESCRIPTION_CHARS).text)
        .filter(|text| !text.is_empty());
    let mut result = match serde_json::to_value(&answer) {
        Ok(value) => value,
        Err(err) => return Response::err(err.to_string()),
    };
    result["session"] = serde_json::json!(session.as_ref().map(SessionId::as_str));
    result["description"] = serde_json::json!(description);

    if let (Some(id), Some(ledger)) = (&session, ledger.as_mut()) {
        // Line 1049: that a gate fired, and which factor fired it, as a row
        // a person can read back.
        let subject = format!(
            "{}/{}/{}",
            answer.risk,
            answer.factor.map_or("none", |factor| factor.as_str()),
            answer.verdict
        );
        match ledger.record_session_event(
            id.as_str(),
            TransitionKind::Gate,
            None,
            Origin::Glasshouse,
            Some(&subject),
            description.as_deref(),
        ) {
            Ok(row) => result["gate"]["seq"] = serde_json::json!(row.seq),
            Err(err) => return Response::err(err.to_string()),
        }

        // Line 1039 and 1050: an exceeded budget is recorded and notified,
        // and the session's open premises come back to be re-evaluated.
        if let Some(review) = &answer.budget
            && review.exceeded
        {
            let axis = review.exceeded_axis();
            let note = review
                .axes
                .iter()
                .filter(|line| line.exceeded)
                .map(|line| format!("{}: spent {} of {}", line.axis, line.spent, line.budget))
                .collect::<Vec<_>>()
                .join("; ");
            match ledger.record_session_event(
                id.as_str(),
                TransitionKind::BudgetExceeded,
                None,
                Origin::Glasshouse,
                axis.map(|axis| axis.as_str()),
                Some(&note),
            ) {
                Ok(row) => result["budget"]["seq"] = serde_json::json!(row.seq),
                Err(err) => return Response::err(err.to_string()),
            }
            match ledger.open_for_session(id.as_str()) {
                Ok(open) => {
                    result["re_evaluate"] =
                        serde_json::json!(open.iter().map(assumption_json).collect::<Vec<_>>());
                }
                Err(err) => return Response::err(err.to_string()),
            }
        }
    }
    drop(ledger);

    // Line 1036: a recoverable checkpoint before a high-risk change, through
    // the same path `TakeCheckpoint` uses, and said so in the answer. Not
    // under `off`, which disables the mechanism whole; a `skip` waives the
    // gate, not the recovery.
    if let Some(id) = &session
        && answer.risk == RiskClass::Substantial
        && policy.mode != GuardrailMode::Off
    {
        let objective = format!(
            "guardrail preflight before a substantial change: {}",
            description.as_deref().unwrap_or("(no description stated)")
        );
        let state = format!(
            "risk {} ({}), verdict {} — {}",
            answer.risk,
            answer.factor.map_or("none", |factor| factor.as_str()),
            answer.verdict,
            answer.gate.decided_by
        );
        let next_actions = answer
            .prompts
            .iter()
            .map(|prompt| prompt.ask.to_owned())
            .collect();
        match request_checkpoint(
            runtime,
            sessions,
            Some(id.as_str()),
            objective,
            state,
            Vec::new(),
            Vec::new(),
            Vec::new(),
            None,
            next_actions,
        ) {
            Response::Ok { result: checkpoint } => result["checkpoint"] = checkpoint,
            Response::Error { message } => result["checkpoint_error"] = serde_json::json!(message),
        }
    }

    Response::ok(result)
}

/// `Request::RecordAssumption`.
pub(super) fn record_assumption(
    runtime: &Runtime,
    store: &SessionStore<'_>,
    live: &Mutex<SessionRuntime>,
    mut new: NewAssumption,
) -> Response {
    let session = match scoped_session(store, live, new.session.as_deref()) {
        Ok(session) => session,
        Err(err) => return Response::err(err),
    };
    new.session = session.map(|id| id.as_str().to_owned());
    let mut ledger = match open_ledger(runtime) {
        Ok(ledger) => ledger,
        Err(err) => return Response::err(err),
    };
    let record = match ledger.record(new) {
        Ok(record) => record,
        Err(err) => return Response::err(err.to_string()),
    };
    match ledger.get(&record.id) {
        Ok(Some(view)) => Response::ok(assumption_json(&view)),
        Ok(None) => Response::err(format!(
            "assumption {} was not readable after recording",
            record.id
        )),
        Err(err) => Response::err(err.to_string()),
    }
}

/// `Request::UpdateAssumption`.
///
/// Three handles, one at a time (practice §65): the ledger to resolve and
/// read; the memory store, if a failed-approach record was asked for; the
/// ledger again to append. The memory is written first so the transition
/// can carry its identifier as `subject`. The one refusal the transition
/// itself raises — `waived_by_user` without a user origin — is the store's
/// (`AssumptionStore::transition`) and is not repeated here: a memory is
/// only ever written for a `refuted` state, so a refused waiver can leave
/// nothing behind. A copy of that check at this door survived its own
/// mutation, which is how it was found to be dead.
///
/// Line 1044: when the appended transition is a move to `refuted` or the
/// rollback/isolate response (`guardrails::transition_wants_preserve`), the
/// reply gains `preserve` — read from the session store the caller already
/// holds (`store.active_claims()`) and the working tree
/// (`checkpoint::git::changed_paths`), never a second store opened here.
/// Every other transition's reply is unchanged.
pub(super) fn update_assumption(
    runtime: &Runtime,
    store: &SessionStore<'_>,
    assumption: &str,
    mut transition: NewTransition,
    record_failed_approach: bool,
) -> Response {
    let (id, current) = match resolve_assumption(runtime, assumption) {
        Ok(found) => found,
        Err(err) => return Response::err(err),
    };

    let mut memory_id = None;
    if record_failed_approach {
        if transition.state != Some(guardrails::AssumptionState::Refuted) {
            return Response::err(
                GuardrailError::NotRefuted {
                    state: transition.state.unwrap_or(current.state),
                }
                .to_string(),
            );
        }
        let note = transition
            .note
            .as_deref()
            .map(|text| guardrails::sanitize(text, guardrails::MAX_FIELD_CHARS).text)
            .filter(|text| !text.is_empty());
        let record = &current.record;
        let mut body = format!(
            "Refuted premise: {}. Evidence at the time: {}.",
            record.claim, record.evidence
        );
        if let Some(note) = &note {
            body.push_str(" Refuted by: ");
            body.push_str(note);
            body.push('.');
        }
        let new = NewMemory::new(MemoryKind::FailedAttempt, body)
            .with_subject(Some(format!("refuted assumption {}", id.short())))
            .with_source_session(record.session_id.clone())
            .with_provenance(DecisionProvenance {
                rationale: Some(format!(
                    "task assumption {id} was refuted; kept as a failed approach so it is not \
                     repeated (capability map line 1019)"
                )),
                problem: Some(record.affected.clone()),
                assumptions: Some(record.claim.clone()),
                evidence: Some(match &note {
                    Some(note) => format!("{}; refuted by: {note}", record.evidence),
                    None => record.evidence.clone(),
                }),
                ..DecisionProvenance::default()
            });
        match write_memory(runtime, new) {
            Ok(stored) => memory_id = Some(stored),
            Err(err) => return Response::err(err),
        }
    }
    transition.subject = memory_id.clone();

    let mut ledger = match open_ledger(runtime) {
        Ok(ledger) => ledger,
        Err(err) => return Response::err(err),
    };
    let written = match ledger.transition(&id, transition) {
        Ok(written) => written,
        Err(err) => return Response::err(err.to_string()),
    };

    let preserve = if guardrails::transition_wants_preserve(written.state, written.response) {
        let claims = match store.active_claims() {
            Ok(claims) => claims,
            Err(err) => return Response::err(err.to_string()),
        };
        let changed = changed_paths(runtime.project().root());
        let session = SessionId::new(written.session_id.clone().unwrap_or_default());
        Some(guardrails::preserve_set(
            &claims,
            changed.as_deref(),
            &session,
        ))
    } else {
        None
    };

    match ledger.get(&id) {
        Ok(Some(view)) => {
            let mut result = serde_json::json!({
                "assumption": assumption_json(&view),
                "transition": transition_json(&written),
                "memory": memory_id,
            });
            if let Some(preserve) = preserve {
                result["preserve"] = serde_json::json!(preserve);
            }
            Response::ok(result)
        }
        Ok(None) => Response::err(format!("assumption {id} vanished while being updated")),
        Err(err) => Response::err(err.to_string()),
    }
}

/// `Request::ListAssumptions`.
pub(super) fn list_assumptions(
    runtime: &Runtime,
    store: &SessionStore<'_>,
    live: &Mutex<SessionRuntime>,
    session: Option<&str>,
    limit: usize,
) -> Response {
    let session = match scoped_session(store, live, session) {
        Ok(session) => session,
        Err(err) => return Response::err(err),
    };
    let ledger = match open_ledger(runtime) {
        Ok(ledger) => ledger,
        Err(err) => return Response::err(err),
    };
    let session = session.as_ref().map(SessionId::as_str);
    let views = match ledger.list(session, limit.min(MAX_ASSUMPTIONS_LIMIT)) {
        Ok(views) => views,
        Err(err) => return Response::err(err.to_string()),
    };
    let counts = match ledger.counts(session) {
        Ok(counts) => counts,
        Err(err) => return Response::err(err.to_string()),
    };
    let events = match session {
        Some(session) => match ledger.session_events(session, None, SESSION_EVENTS_LIMIT) {
            Ok(events) => events,
            Err(err) => return Response::err(err.to_string()),
        },
        None => Vec::new(),
    };
    let mut counted = serde_json::Map::new();
    for (state, count) in counts {
        counted.insert(state.as_str().to_owned(), serde_json::json!(count));
    }
    Response::ok(serde_json::json!({
        "session": session,
        "counts": counted,
        "assumptions": views.iter().map(assumption_json).collect::<Vec<_>>(),
        "events": events.iter().map(transition_json).collect::<Vec<_>>(),
    }))
}

/// `Request::PromoteAssumption` — line 1020's explicit promotion, and line
/// 1017's rule that it is refused for anything but a supported assumption.
pub(super) fn promote_assumption(
    runtime: &Runtime,
    assumption: &str,
    kind: PromotionKind,
    note: Option<String>,
    origin: Origin,
) -> Response {
    let (id, current) = match resolve_assumption(runtime, assumption) {
        Ok(found) => found,
        Err(err) => return Response::err(err),
    };
    if current.state != guardrails::AssumptionState::Supported {
        return Response::err(
            GuardrailError::NotSupported {
                id: id.as_str().to_owned(),
                state: current.state,
            }
            .to_string(),
        );
    }
    let note = note
        .as_deref()
        .map(|text| guardrails::sanitize(text, guardrails::MAX_FIELD_CHARS).text)
        .filter(|text| !text.is_empty());
    let (memory_kind, authority) = match kind {
        PromotionKind::Decision => (MemoryKind::Decision, Some(MemoryAuthority::Decision)),
        PromotionKind::Constraint => (MemoryKind::Constraint, Some(MemoryAuthority::Constraint)),
        // A finding is knowledge, not a rule: unclassified, so it is never
        // injected as binding until a person promotes its authority.
        PromotionKind::Finding => (MemoryKind::Finding, None),
    };
    let record = &current.record;
    let new = NewMemory::new(memory_kind, record.claim.clone())
        .with_subject(Some(record.affected.clone()))
        .with_authority(authority)
        .with_source_session(record.session_id.clone())
        .with_provenance(DecisionProvenance {
            rationale: Some(format!(
                "promoted from supported task assumption {id} (capability map line 1020){}",
                note.as_deref()
                    .map(|n| format!(": {n}"))
                    .unwrap_or_default()
            )),
            problem: Some(record.affected.clone()),
            evidence: Some(format!(
                "{} ({}, uncertainty {}); verification: {}",
                record.evidence, record.evidence_source, record.uncertainty, record.verification
            )),
            ..DecisionProvenance::default()
        });
    let memory_id = match write_memory(runtime, new) {
        Ok(stored) => stored,
        Err(err) => return Response::err(err),
    };

    let mut ledger = match open_ledger(runtime) {
        Ok(ledger) => ledger,
        Err(err) => return Response::err(err),
    };
    let restated = NewTransition::restate(origin)
        .with_subject(Some(memory_id.clone()))
        .with_note(Some(format!(
            "promoted as {kind}{}",
            note.as_deref()
                .map(|n| format!(": {n}"))
                .unwrap_or_default()
        )));
    match ledger.transition(&id, restated) {
        Ok(written) => Response::ok(serde_json::json!({
            "assumption": id.as_str(),
            "memory": memory_id,
            "kind": kind,
            "authority": authority.map(MemoryAuthority::as_str),
            "transition": transition_json(&written),
        })),
        Err(err) => Response::err(format!(
            "memory {memory_id} was written but the promotion could not be recorded: {err}"
        )),
    }
}

/// Resolve an identifier (or its leading part) and read the assumption, on
/// a handle that is dropped before this returns.
fn resolve_assumption(
    runtime: &Runtime,
    assumption: &str,
) -> Result<(guardrails::AssumptionId, AssumptionView), String> {
    let ledger = open_ledger(runtime)?;
    let id = ledger
        .resolve_id(assumption)
        .map_err(|err| err.to_string())?;
    let view = ledger
        .get(&id)
        .map_err(|err| err.to_string())?
        .ok_or_else(|| format!("no assumption `{id}` in this project"))?;
    Ok((id, view))
}

/// Write one memory through the existing store and hand back its id. The
/// store's admission guard and project scope apply unchanged.
fn write_memory(runtime: &Runtime, new: NewMemory) -> Result<String, String> {
    let memory = ProjectMemory::open(runtime).map_err(|err| memory_error_message(&err))?;
    let stored = memory.store().record(new).map_err(|err| err.to_string())?;
    Ok(stored.id.as_str().to_owned())
}

/// One assumption for the wire. Every free-text field goes through
/// [`guardrails::quote`]: this answer is read by an agent, so the stored
/// text is rendered with the same discipline as an injected memory.
fn assumption_json(view: &AssumptionView) -> serde_json::Value {
    let record = &view.record;
    serde_json::json!({
        "id": record.id.as_str(),
        "session": record.session_id,
        "created_at": record.created_at,
        "origin": record.origin,
        "state": view.state,
        "claim": guardrails::quote(&record.claim, guardrails::MAX_CLAIM_CHARS),
        "evidence": guardrails::quote(&record.evidence, guardrails::MAX_FIELD_CHARS),
        "evidence_source": record.evidence_source,
        "uncertainty": record.uncertainty,
        "affected": guardrails::quote(&record.affected, guardrails::MAX_FIELD_CHARS),
        "verification": guardrails::quote(&record.verification, guardrails::MAX_FIELD_CHARS),
        "transitions": view.transitions,
        "latest": transition_json(&view.latest),
    })
}

fn transition_json(transition: &Transition) -> serde_json::Value {
    serde_json::json!({
        "seq": transition.seq,
        "assumption": transition.assumption_id.as_ref().map(|id| id.as_str()),
        "session": transition.session_id,
        "at": transition.at,
        "kind": transition.kind,
        "state": transition.state,
        "origin": transition.origin,
        "subject": transition.subject,
        "response": transition.response.map(GuardrailResponse::as_str),
        "note": transition
            .note
            .as_deref()
            .map(|note| guardrails::quote(note, guardrails::MAX_FIELD_CHARS)),
    })
}

pub(super) fn notification_json(
    notification: &guardrails::store::Notification,
) -> serde_json::Value {
    let mut value = transition_json(&notification.transition);
    value["claim"] = serde_json::json!(
        notification
            .claim
            .as_deref()
            .map(|claim| guardrails::quote(claim, guardrails::MAX_CLAIM_CHARS))
    );
    value
}
