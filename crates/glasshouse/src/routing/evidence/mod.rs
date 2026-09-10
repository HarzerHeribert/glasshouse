//! Phase 33A — the project-local routing evidence ledger.
//!
//! An append-oriented record of what actually happened on a routed turn
//! (line 1329), stored in `routing_observations` (`crate::database` migration
//! 11), plus rolling summaries computed **on read** from those raw rows
//! (line 1335) rather than replacing them. Every summary carries its own
//! source, window, sample size, freshness and confidence (line 1339, and see
//! [`AggregateReading`]) and stays [`None`] — "unknown" — when the sample is
//! too small to support a routing decision (line 1340), never a wide error
//! bar around a guess.
//!
//! Two producers write these rows, each honest about what it can see:
//! [`crate::gateway::session::SessionRouting`] relays a harness's own request
//! and so leaves most timing, token and outcome columns `NULL` unless a
//! translated exchange's own seam already decoded them; `crate::memory::extract`
//! builds and decodes its own request and so can fill the token columns the
//! gateway cannot. A column this build cannot honestly read stays `NULL` —
//! "the build that wrote this row recorded nothing here" — never a guessed or
//! interpolated value.
// History: design-decisions.md, "Trims: routing/evidence/mod.rs", module doc.

use std::sync::Mutex;

use rusqlite::{Connection, Row};

use crate::provider::quota::ReadingSource;

/// Everything that can go wrong reading or writing the evidence ledger.
#[derive(Debug, thiserror::Error)]
pub enum EvidenceLedgerError {
    #[error("the project database has no project identifier bound")]
    UnboundDatabase,
    #[error("routing observation {seq} stored an unrecognized {column} value `{value}`")]
    UnknownValue {
        seq: i64,
        column: &'static str,
        value: String,
    },
    #[error(
        "an observed routing identity grouped by (provider, model, route, context_state) stored an unrecognized {column} value `{value}`"
    )]
    UnknownAggregateValue { column: &'static str, value: String },
    #[error("could not {action} in the routing evidence ledger")]
    Sql {
        action: &'static str,
        #[source]
        source: rusqlite::Error,
    },
}

fn sql_err(action: &'static str) -> impl Fn(rusqlite::Error) -> EvidenceLedgerError {
    move |source| EvidenceLedgerError::Sql { action, source }
}

fn median(mut values: Vec<i64>) -> i64 {
    values.sort_unstable();
    values[values.len() / 2]
}

/// An open project database plus the routing observations inside it.
///
/// Owns its connection behind a [`Mutex`] rather than borrowing one, unlike
/// [`crate::memory::MemoryStore`] and
/// [`crate::checkpoint::store::CheckpointStore`]: this ledger's production
/// writer (`crate::gateway::session::SessionRouting`) is called from a fresh
/// thread per connection (`crate::gateway::mod::accept_loop`'s own "giving
/// each connection a thread"), so the store this module hands the gateway
/// must be safe to hold behind one shared `Arc` and written from many threads
/// at once. A single [`Connection`] behind a [`Mutex`] is the same answer
/// [`crate::gateway::session::SessionRouting`]'s own `State` gives for the
/// same reason.
pub struct EvidenceLedger {
    conn: Mutex<Connection>,
    project_id: String,
}

impl std::fmt::Debug for EvidenceLedger {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EvidenceLedger")
            .field("project_id", &self.project_id)
            .finish_non_exhaustive()
    }
}

fn failure_rate_aggregate(observations: &[RoutingObservation]) -> Option<AggregateReading<f64>> {
    let with_outcome: Vec<&RoutingObservation> = observations
        .iter()
        .filter(|o| matches!(o.outcome, Some(Outcome::Succeeded) | Some(Outcome::Failed)))
        .collect();
    if with_outcome.len() < MIN_SAMPLE_FOR_SUMMARY {
        return None;
    }
    let failed = with_outcome
        .iter()
        .filter(|o| matches!(o.outcome, Some(Outcome::Failed)))
        .count();
    let window_start = with_outcome.first()?.observed_at_unix;
    let window_end = with_outcome.last()?.observed_at_unix;
    Some(AggregateReading::new(
        failed as f64 / with_outcome.len() as f64,
        window_start,
        window_end,
        with_outcome.len(),
        ReadingSource::LocalObservation("gateway exchange failure count".to_owned()),
    ))
}

/// `pub(crate)`: [`crate::evaluation`]'s map-line-1845 join reads
/// `routing_observations` directly (the same database file, a second
/// connection — see that module's own doc comment) and reuses this row
/// decoder rather than re-deriving [`RoutingObservation`]'s parsing.
pub(crate) fn row_to_observation(
    row: &Row<'_>,
) -> rusqlite::Result<Result<RoutingObservation, EvidenceLedgerError>> {
    let seq: i64 = row.get("seq")?;

    let outcome_text: Option<String> = row.get("outcome")?;
    let outcome = match outcome_text {
        None => None,
        Some(text) => match Outcome::from_stored(&text) {
            Some(outcome) => Some(outcome),
            None => {
                return Ok(Err(EvidenceLedgerError::UnknownValue {
                    seq,
                    column: "outcome",
                    value: text,
                }));
            }
        },
    };

    let failure_class_text: Option<String> = row.get("failure_class")?;
    let failure_class = match failure_class_text {
        None => None,
        Some(text) => match FailureClass::from_stored(&text) {
            Some(class) => Some(class),
            None => {
                return Ok(Err(EvidenceLedgerError::UnknownValue {
                    seq,
                    column: "failure_class",
                    value: text,
                }));
            }
        },
    };

    // Migration 23, and deliberately not `failure_class`'s shape above: an
    // unrecognised word is `None`, not an `UnknownValue`. See the migration's
    // own doc comment -- a class is a bucketing input to an average, and a
    // future build's sixth class must not break an older build's burn rate.
    let task_class_text: Option<String> = row.get("task_class")?;
    let task_class = task_class_text
        .as_deref()
        .and_then(super::request::TaskClass::from_stored);

    // Migration 24, and `task_class`'s arm above rather than
    // `failure_class`'s, for the reason that migration's own doc comment
    // gives: both stored words are bucketing inputs to a ratio, so a word a
    // future build invents must lower no reader here rather than failing the
    // whole row for an older build. `session_id` needs no arm of its own —
    // it is an opaque identifier with no vocabulary to fail against.
    let effort_level_text: Option<String> = row.get("effort_level")?;
    let effort_level = effort_level_text
        .as_deref()
        .and_then(EffortLevel::from_stored);

    let turn_shape_text: Option<String> = row.get("turn_shape")?;
    let turn_shape = turn_shape_text.as_deref().and_then(TurnShape::from_stored);

    let context_text: String = row.get("context_state")?;
    let Some(context_state) = ContextState::from_stored(&context_text) else {
        return Ok(Err(EvidenceLedgerError::UnknownValue {
            seq,
            column: "context_state",
            value: context_text,
        }));
    };

    let cost_micro_usd: Option<i64> = row.get("cost_micro_usd")?;
    let cost_confidence_text: Option<String> = row.get("cost_confidence")?;
    let cost = match (cost_micro_usd, cost_confidence_text) {
        (None, _) => None,
        (Some(micro_usd), Some(text)) => match CostConfidence::from_stored(&text) {
            Some(confidence) => Some(ObservedCost {
                micro_usd,
                confidence,
            }),
            None => {
                return Ok(Err(EvidenceLedgerError::UnknownValue {
                    seq,
                    column: "cost_confidence",
                    value: text,
                }));
            }
        },
        // Migration 11's own `CHECK` refuses this combination on the way in;
        // reaching it means a row was written by something that bypassed the
        // schema, and this reader reports it rather than guessing a
        // confidence nobody stated.
        (Some(_), None) => {
            return Ok(Err(EvidenceLedgerError::UnknownValue {
                seq,
                column: "cost_confidence",
                value: "absent".to_owned(),
            }));
        }
    };

    Ok(Ok(RoutingObservation {
        seq,
        project_id: row.get("project_id")?,
        observed_at_unix: row.get("observed_at")?,
        provider: row.get("provider")?,
        model: row.get("model")?,
        route: row.get("route")?,
        quota_context: row.get("quota_context")?,
        harness: row.get("harness")?,
        purpose: row.get("purpose")?,
        dispatched_at_unix: row.get("dispatched_at")?,
        first_byte_at_unix: row.get("first_byte_at")?,
        first_token_at_unix: row.get("first_token_at")?,
        first_tool_call_at_unix: row.get("first_tool_call_at")?,
        completed_at_unix: row.get("completed_at")?,
        // Migration 25. No vocabulary to fail against and no arm of their
        // own: an integer column reads back as the integer it holds, and
        // `NULL` is the *this producer did not measure* every other optional
        // column on this row already means.
        first_byte_ms: row.get("first_byte_ms")?,
        first_token_ms: row.get("first_token_ms")?,
        first_tool_call_ms: row.get("first_tool_call_ms")?,
        completed_ms: row.get("completed_ms")?,
        input_tokens: row.get("input_tokens")?,
        output_tokens: row.get("output_tokens")?,
        cached_input_tokens: row.get("cached_input_tokens")?,
        cost,
        tool_rounds: row.get("tool_rounds")?,
        retries: row.get("retries")?,
        repairs: row.get("repairs")?,
        failovers: row.get("failovers")?,
        outcome,
        failure_class,
        task_class,
        session_id: row.get("session_id")?,
        effort_level,
        turn_shape,
        context_state,
    }))
}

impl EvidenceLedger {
    fn lock(&self) -> std::sync::MutexGuard<'_, Connection> {
        self.conn
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

mod joins;
mod ledger;
mod readers;
mod signals;
#[cfg(test)]
mod tests;
mod vocabulary;

pub use joins::{
    EffortShadow, EffortShadowRow, HeadroomBand, HeadroomBasis, HeadroomReplayCounts,
    LONG_SIGNAL_HORIZON_SECONDS, LongWindowPressure, MIN_LEARNED_RESET_RECOVERIES,
    OutputEstimateAccuracy, RECENT_SIGNAL_HORIZON_SECONDS, ResetBasis, RouteResponsiveness,
    SeparationMeasure, SeparationReport, SubscriptionHeadroomEstimate,
    estimate_subscription_headroom,
};
pub use readers::{
    ClassificationRecord, HarnessRequestStats, LatencyRecord, ObservationQuery,
    ObservedEvidenceSource, ObservedIdentity, PurposeConsumption, RoutingOverhead, RoutingSummary,
    SessionTranslationSavings, TranslationSavings, WallClockSummary,
};
pub use signals::{
    CorrelationVerdict, CredentialCost, CredentialSpend, CredentialThrottles, RouteCorrelation,
    RouteCorrelations, RouteIdentity, ThrottleScope, ThrottleScopes, classify_throttle_scope,
    classify_throttle_scopes, correlate_routes, estimated_context_tokens, recent_credential_cost,
    recent_credential_spend, recent_credential_throttles,
};
pub use vocabulary::*;
