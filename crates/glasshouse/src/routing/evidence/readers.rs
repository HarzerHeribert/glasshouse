//! The readers that survive over `routing_observations` after the
//! 2026-09-16 ruling deleted the router: per-identity, per-provider and
//! per-session/per-purpose consumption summaries, and the per-session
//! cached-input share `glasshouse cost` and the shell's meters read.

use super::*;

use rusqlite::{OptionalExtension, Row, params};

use crate::provider::quota::ReadingSource;

/// Rolling summaries for one `(provider, model, route)` identity, within one
/// [`ContextState`] bucket — capability map line 1337's separation kept all
/// the way through the aggregate, never blended back together.
///
/// Every field is `None` — "unknown" — below [`MIN_SAMPLE_FOR_SUMMARY`], per
/// line 1340; `crate::config::pairing::evidence_signal`'s own convention is
/// that an absent field contributes nothing to a routing decision, which is
/// exactly the composition this type is built to support.
#[derive(Debug, Clone, PartialEq)]
pub struct RoutingSummary {
    pub provider: String,
    pub model: String,
    pub route: Option<String>,
    pub context_state: ContextState,

    /// Median exchange duration, in milliseconds — capability map line 1339's
    /// "median."
    pub median_duration_ms: Option<AggregateReading<i64>>,
    /// 95th-percentile exchange duration, in milliseconds — line 1339's
    /// "tail latency."
    pub tail_duration_ms: Option<AggregateReading<i64>>,
    /// Exponentially-weighted moving average of exchange duration, in
    /// milliseconds — line 1339's "exponentially weighted averages."
    pub ewma_duration_ms: Option<AggregateReading<f64>>,
    /// Fraction of observations with a known outcome that were
    /// [`Outcome::Failed`] — line 1339's "failure rates."
    pub failure_rate: Option<AggregateReading<f64>>,
    /// How many of this identity's exchanges in the window fell into each
    /// [`FailureClass`], with their denominator — lines 1316 and 1365. Counts
    /// rather than rates, so **not** withheld below [`MIN_SAMPLE_FOR_SUMMARY`]
    /// like the four aggregates above; see [`FailureClassCounts`]' own doc.
    pub failure_classes: FailureClassCounts,
}

/// How much weight [`ewma`] gives the most recent observation.
///
/// A third, chosen so that roughly the last five observations dominate the
/// average — matching [`MIN_SAMPLE_FOR_SUMMARY`] rather than an unrelated
/// number, so "how many observations before this project trusts a figure"
/// and "how many observations that figure actually weighs" tell a consistent
/// story.
const EWMA_ALPHA: f64 = 1.0 / (MIN_SAMPLE_FOR_SUMMARY as f64);

fn p95(mut values: Vec<i64>) -> i64 {
    values.sort_unstable();
    let index = ((values.len() - 1) * 95) / 100;
    values[index]
}

/// The oldest-first EWMA of `values`, seeded with the first observation.
fn ewma(values: &[i64]) -> f64 {
    let mut iter = values.iter();
    let Some(&first) = iter.next() else {
        return 0.0;
    };
    let mut acc = first as f64;
    for &value in iter {
        acc = EWMA_ALPHA * value as f64 + (1.0 - EWMA_ALPHA) * acc;
    }
    acc
}

/// The identity a group of observations is read back by — `provider`,
/// `model`, `route` and `harness`, matching migration 11's own index and
/// capability map line 1338's "materially different" set. Bundled into one
/// type so [`EvidenceLedger::recent`] and [`EvidenceLedger::summarize`] stay
/// under this crate's argument-count lint rather than each taking four
/// separate identity parameters beside their own.
#[derive(Debug, Clone, Copy)]
pub struct ObservationQuery<'a> {
    pub provider: &'a str,
    pub model: &'a str,
    /// `None` matches rows recorded with no route, not "any route."
    pub route: Option<&'a str>,
    /// `None` matches rows recorded with no harness, not "any harness."
    pub harness: Option<&'a str>,
}

/// One `(provider, model, route)` identity that actually has rows in
/// `routing_observations` within a queried window, grouped further by
/// [`ContextState`] — capability map line 1762's route-evidence table and
/// line 1764's "which of warm, cold or unknown," and the missing link batch
/// 42 found and this package builds (practice §71): every other reader on
/// this ledger requires the caller to already name an identity via its own
/// query type; nothing before [`EvidenceLedger::observed_identities`] can
/// answer "which identities exist at all."
///
/// `context_state` is part of the group, not a value chosen or averaged
/// across it — the same separation [`RoutingSummary`] keeps for the same
/// reason (line 1337) — so an identity that genuinely has both warm and
/// unknown rows gets one row per state here rather than one row picking a
/// winner.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObservedIdentity {
    pub provider: String,
    pub model: String,
    /// `None` means these rows were recorded with no route, matching
    /// `ObservationQuery::route`'s own convention.
    pub route: Option<String>,
    pub context_state: ContextState,
    sample_count: usize,
    window_start_unix: i64,
    window_end_unix: i64,
}

impl ObservedIdentity {
    /// How many raw `routing_observations` rows this identity was counted
    /// from, within the queried window — a real `COUNT(*)` over recorded
    /// rows, never an estimate and never rounded up to look confident.
    pub fn sample_count(&self) -> usize {
        self.sample_count
    }

    /// The observation window this count was drawn from, as
    /// `(earliest_unix, latest_unix)` — the same shape
    /// [`AggregateReading::window`] returns, for the same reason: a count
    /// with no window attached invites reading it as "ever," which it is
    /// not.
    pub fn window(&self) -> (i64, i64) {
        (self.window_start_unix, self.window_end_unix)
    }
}

/// Request and token consumption for one `(purpose, harness_recorded)`
/// group, within a queried window — capability map line 1464's "measure
/// routing-model token and request consumption separately from coding-agent
/// consumption," and the absent aggregate
/// [`EvidenceLedger::consumption_by_purpose`] builds.
///
/// `purpose` alone cannot separate coding-agent consumption from everything
/// else: it is `None` for every row no producer has stamped, which today is
/// both every gateway relay exchange and every memory-extraction call.
/// `harness_recorded` tells those two `NULL`-purpose producers apart: `true`
/// only when every row in the group named a harness, which today means
/// gateway rows alone.
///
/// `sample_count` is a real `COUNT(*)`, always defined. The three token
/// fields are not: each is `None` when every row in the group left that
/// column `NULL`, a different fact from `Some(0)` that must stay one — the
/// hazard this aggregate exists to avoid rendering as a number. A group
/// mixing counted and uncounted rows sums only what was counted, as
/// [`NewObservation::with_tokens`] asks every producer to leave absent
/// counts absent rather than zeroed.
// History: design-decisions.md, "Trims: routing module docs", routing/evidence/readers.rs `struct PurposeConsumption` doc.
#[derive(Debug, Clone, PartialEq)]
pub struct PurposeConsumption {
    pub purpose: Option<String>,
    pub harness_recorded: bool,
    pub sample_count: usize,
    pub input_tokens: Option<i64>,
    pub output_tokens: Option<i64>,
    pub cached_input_tokens: Option<i64>,
    /// How many rows in this group carried a `first_byte_at` — a real
    /// `COUNT(first_byte_at)`, always defined and honestly `0` when none did.
    /// Line 1331's gateway producer is the only writer that can ever supply
    /// this column, so today it is nonzero only for the coding-agent group.
    pub first_byte_sample_count: usize,
    /// How many rows in this group carried migration 25's `first_byte_ms` —
    /// the *measured* offset, as against the second-resolution difference
    /// [`Self::first_byte_sample_count`] counts.
    ///
    /// Two counts rather than one, because the mean beside them is computed
    /// over both kinds of row and a reader must be able to say which it is
    /// looking at: `0` here with a nonzero
    /// [`Self::first_byte_sample_count`] means every row in this group
    /// predates migration 25, and the figure is a seconds difference wearing
    /// millisecond units. `main.rs::render_routing_cost` prints *(seconds
    /// only)* for exactly that case.
    pub first_byte_ms_sample_count: usize,
    /// The mean time to first byte, in milliseconds — migration 25's
    /// `first_byte_ms` for each row that carries one, and the
    /// `first_byte_at - dispatched_at` difference in milliseconds for each
    /// row that does not. `None` when neither was available for any row in
    /// the group, never a fabricated duration for a group nothing timed.
    pub mean_time_to_first_byte_ms: Option<f64>,
    /// [`Self::first_byte_sample_count`]'s sibling for `first_token_at` — a
    /// real `COUNT(first_token_at)`.
    ///
    /// **A relayed exchange can supply it too, since 2026-09-03.** This said
    /// *only a translated exchange can ever* supply it, which was true until
    /// the user approved the gateway reading usage and timing out of
    /// supported relayed bodies (lines 1331/1332; `GH-STREAM-FIRST-EVENTS`
    /// then `GH-RELAY-USAGE`). Two conditions still leave it `0`: a protocol
    /// slug whose usage spelling is unknown, and a **non-streamed** delivery
    /// — a document arrives as one body, so the moment a marker crosses the
    /// seam measures how fast the socket drained rather than when the
    /// provider produced it, and deriving one timestamp from another is the
    /// estimate that approval withholds.
    pub first_token_sample_count: usize,
    /// [`Self::first_byte_ms_sample_count`]'s sibling for `first_token_ms`.
    pub first_token_ms_sample_count: usize,
    /// The mean time to first token, in milliseconds, under
    /// [`Self::mean_time_to_first_byte_ms`]'s own two-source rule — line
    /// 1348's TTFT, kept as a measure of generation responsiveness and
    /// never presented as agent productivity.
    pub mean_time_to_first_token_ms: Option<f64>,
    /// [`Self::first_byte_sample_count`]'s sibling for `first_tool_call_at`.
    pub first_tool_call_sample_count: usize,
    /// [`Self::first_byte_ms_sample_count`]'s sibling for
    /// `first_tool_call_ms`.
    pub first_tool_call_ms_sample_count: usize,
    /// The mean time to the first tool call, in milliseconds, under
    /// [`Self::mean_time_to_first_byte_ms`]'s own two-source rule — line
    /// 1347's TTFC, the responsiveness measure for tool-using work.
    pub mean_time_to_first_tool_call_ms: Option<f64>,
    /// Output tokens summed over exactly the rows that carried all three of
    /// `output_tokens`, `first_token_ms` and `completed_ms` with the
    /// completion not before the first token — line 1349's numerator, and
    /// `None` when no row in the group carried all three.
    ///
    /// Summed under the same filter as [`Self::decode_ms`] so the two are a
    /// matched pair over one set of rows; a rate built from a numerator and
    /// a denominator drawn from different rows would be a number about no
    /// exchange that happened.
    pub decode_output_tokens: Option<i64>,
    /// Milliseconds of decode time summed over exactly the rows
    /// [`Self::decode_output_tokens`] sums — `completed_ms - first_token_ms`
    /// each, line 1349's denominator.
    pub decode_ms: Option<i64>,
    /// How many tool-use rounds the responses in this group began —
    /// `SUM(tool_rounds)`, `None` when no row in the group ever counted one
    /// (`SUM` over an all-`NULL` column is already `NULL`, so there is no
    /// manual zero-guard here either, unlike the `AVG(CASE …)` pairs above).
    /// Line 1334's last two quantities, `GH-TOOL-ROUNDS-ON-TRANSLATED`.
    pub tool_rounds: Option<i64>,
    /// [`Self::tool_rounds`]'s sibling: `SUM(repairs)`, the harness's own
    /// report of a previous round's failure, under the same `None` rule.
    pub repairs: Option<i64>,
    /// The group's summed exchange duration, in seconds —
    /// `SUM(completed_at - dispatched_at)` over the rows that carried both,
    /// `None` when none did.
    pub serving_seconds: Option<i64>,
    /// How many of this group's rows carry a known outcome
    /// (`succeeded`/`failed`) — the same test `failure_rate_aggregate`
    /// applies to a raw slice, computed here in SQL over the group instead.
    /// Line 1351's own rate floor sits behind [`Self::failure_rate`], not
    /// this count, which is honest at any size.
    pub failure_rate_sample: usize,
    /// The fraction of [`Self::failure_rate_sample`] that failed —
    /// [`MIN_SAMPLE_FOR_SUMMARY`]'s standing rate floor applied here as it is
    /// everywhere else on this ledger: `None` below it, never a rate nobody
    /// should trust.
    pub failure_rate: Option<f64>,
}

/// A session's cached-input share over its own translated exchanges —
/// capability map line 2019's *"show the per-session cache ratio beside the
/// routing evidence"*, whose producer is migration 24's `session_id`. Built
/// by [`EvidenceLedger::cached_share_for_session`].
///
/// `session_id` is nullable at the row level, but every value this type is
/// actually built with names the one session it was queried for; `None`
/// here would mean the query itself matched no rows, which
/// [`EvidenceLedger::cached_share_for_session`] already turns into `Ok(None)`
/// before constructing one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionTranslationSavings {
    pub session_id: Option<String>,
    pub sample_count: usize,
    pub input_tokens: i64,
    pub cached_input_tokens: i64,
}

impl SessionTranslationSavings {
    /// The share of this session's input tokens a translated exchange
    /// reported as cached, or `None` when neither side of the ratio is
    /// counted.
    pub fn cache_read_ratio(&self) -> Option<f64> {
        let denominator = self.input_tokens + self.cached_input_tokens;
        (denominator > 0).then(|| self.cached_input_tokens as f64 / denominator as f64)
    }
}

/// Routing-model spend set against everything else — capability map line
/// 1465 — as one pure reading over
/// [`EvidenceLedger::consumption_by_purpose`]'s groups, so the arithmetic is
/// testable without a database and is rendered with its denominators rather
/// than as a bare ratio.
///
/// "Spend" is **tokens**, since that is still the only currency this reading
/// can rely on: `cost_micro_usd` has one producer (map line 1307), and it
/// fires only on an entitlement-fallback event, leaving coding-agent spend's
/// column `NULL`. Cached input tokens are left out of the sum — providers
/// disagree on whether they are already inside `input_tokens`, and a sum
/// that might double-count is worse than one that names what it omits.
///
/// A `None` token figure means *no row in that side carried a count*, the
/// same convention [`PurposeConsumption`] keeps; a side that mixes counted
/// and uncounted rows sums only what was counted. [`Self::fraction`] is
/// `None` whenever either side is uncounted or the task side is zero, and
/// [`Self::exceeds`] never fires on an unmeasured comparison.
// History: design-decisions.md, "Trims: routing module docs", routing/evidence/readers.rs `struct RoutingOverhead` doc.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RoutingOverhead {
    /// Rows whose `purpose` is [`CLASSIFICATION_PURPOSE`].
    pub classification_requests: usize,
    pub classification_tokens: Option<i64>,
    /// Every other row the ledger holds in the window — gateway exchanges,
    /// memory extraction, anything a later producer stamps with another
    /// purpose.
    ///
    /// **This stays the line-1466 denominator and keeps its meaning**, and
    /// the four fields below are its breakdown rather than a partition that
    /// replaces it: `extraction + routing_latency + tier_movement + coding_agent +
    /// unstamped == task_requests` exactly, by construction.
    pub task_requests: usize,
    pub task_tokens: Option<i64>,
    /// Rows whose `purpose` is [`EXTRACTION_PURPOSE`] — capability map line
    /// 1832's *"memory-extraction cost, separately from interactive coding
    /// cost"*. Stamped from the build this constant landed in; earlier
    /// extraction rows are in [`Self::unstamped_requests`] and are never
    /// moved here.
    pub extraction_requests: usize,
    pub extraction_tokens: Option<i64>,
    /// Rows whose `purpose` is [`ROUTING_LATENCY_PURPOSE`] — line 1833's
    /// *request consumption* half for the routing model's own decision
    /// timing. These carry no tokens by construction, so a token figure here
    /// is honestly absent rather than zero.
    pub routing_latency_requests: usize,
    pub routing_latency_tokens: Option<i64>,
    /// Rows whose `purpose` is [`TIER_ESCALATION_PURPOSE`] or
    /// [`TIER_DOWNGRADE_PURPOSE`] — line 1566's record of the session
    /// router moving the tier it prefers. No tokens by construction, for
    /// [`ROUTING_LATENCY_PURPOSE`]'s reason.
    pub tier_movement_requests: usize,
    pub tier_movement_tokens: Option<i64>,
    /// Rows whose `purpose` is [`ENTITLEMENT_FALLBACK_EXHAUSTED_PURPOSE`]
    /// or [`ENTITLEMENT_FALLBACK_THROTTLED_PURPOSE`] — line 1970's record
    /// of the broker leaving an account. No tokens by construction, for
    /// [`ROUTING_LATENCY_PURPOSE`]'s reason.
    pub entitlement_fallback_requests: usize,
    pub entitlement_fallback_tokens: Option<i64>,
    /// Rows whose `purpose` is [`CONTEXT_FIREWALL_REDUCTION_PURPOSE`],
    /// [`CONTEXT_FIREWALL_BYPASS_PURPOSE`], or
    /// [`CONTEXT_FIREWALL_EXPANSION_PURPOSE`] — map lines 1987 and 1988's
    /// telemetry. No tokens by construction, for the reason
    /// [`CONTEXT_FIREWALL_REDUCTION_PURPOSE`]'s own doc comment gives: this
    /// purpose's producer never writes an estimate into a column documented
    /// as a provider's own report.
    pub context_firewall_requests: usize,
    pub context_firewall_tokens: Option<i64>,
    /// The gateway relay's own traffic, and today nothing else: rows whose
    /// `purpose` is [`HARNESS_TURN_PURPOSE`], plus rows no producer stamped
    /// that **did** name a harness — the same traffic, from before the build
    /// that added the constant. This is *"interactive coding cost"* as lines
    /// 1832 and 1833 use the phrase, and it is the one side of the
    /// separation this build cannot count in tokens:
    /// `crate::gateway::ingress` relays a body it is designed never to
    /// parse, so every one of these rows leaves all three token columns
    /// `NULL`. The request count is real; the token figure is absent, and
    /// must render as absent.
    pub coding_agent_requests: usize,
    pub coding_agent_tokens: Option<i64>,
    /// Everything none of the four named buckets claims — today exactly the
    /// rows written before this build stamped a purpose (no `purpose`, no
    /// harness), which is every memory-extraction call the previous builds
    /// recorded.
    ///
    /// **Its own bucket precisely so those rows are neither re-labelled nor
    /// silently counted as somebody else's spend.** A `purpose` a later
    /// build writes and this one does not know would also land here, which
    /// is visible degradation rather than a wrong attribution.
    pub unstamped_requests: usize,
    pub unstamped_tokens: Option<i64>,
}

/// Fold one group's counts into one bucket, keeping an absent token count
/// absent.
///
/// `Some(0)` and `None` are different facts here — the whole reason
/// [`PurposeConsumption`]'s token fields are `Option` — so a bucket only
/// becomes counted once a group that carried a count reaches it.
fn add_consumption(bucket: (&mut usize, &mut Option<i64>), requests: usize, tokens: Option<i64>) {
    let (count, total) = bucket;
    *count += requests;
    if let Some(tokens) = tokens {
        *total = Some(total.unwrap_or(0) + tokens);
    }
}

impl RoutingOverhead {
    pub fn from_consumption(groups: &[PurposeConsumption]) -> Self {
        let mut overhead = Self::default();
        for group in groups {
            let tokens = match (group.input_tokens, group.output_tokens) {
                (None, None) => None,
                (input, output) => Some(input.unwrap_or(0) + output.unwrap_or(0)),
            };
            // The named bucket this group belongs to. `harness_recorded` is
            // what tells the two `NULL`-purpose producers apart — see
            // [`PurposeConsumption`]'s own doc comment — so an unstamped row
            // that named a harness is the coding agent's, and one that named
            // none is a row written before this build stamped a purpose.
            // [`HARNESS_TURN_PURPOSE`] is the same coding-agent traffic,
            // stamped explicitly from the build that added the constant
            // onward — the two guards below are one bucket across the
            // stamped/unstamped boundary, not two different facts.
            let named = match group.purpose.as_deref() {
                Some(CLASSIFICATION_PURPOSE) => (
                    &mut overhead.classification_requests,
                    &mut overhead.classification_tokens,
                ),
                Some(EXTRACTION_PURPOSE) => (
                    &mut overhead.extraction_requests,
                    &mut overhead.extraction_tokens,
                ),
                // Line 1852's rows: one per steered failover, no tokens and
                // no request to any model. Not spend on either side of line
                // 1466's comparison, so neither a bucket nor the denominator
                // — see `CORRELATION_PURPOSE`'s own doc comment.
                Some(CORRELATION_PURPOSE) => continue,
                Some(ROUTING_LATENCY_PURPOSE) => (
                    &mut overhead.routing_latency_requests,
                    &mut overhead.routing_latency_tokens,
                ),
                Some(TIER_ESCALATION_PURPOSE | TIER_DOWNGRADE_PURPOSE) => (
                    &mut overhead.tier_movement_requests,
                    &mut overhead.tier_movement_tokens,
                ),
                Some(
                    ENTITLEMENT_FALLBACK_EXHAUSTED_PURPOSE | ENTITLEMENT_FALLBACK_THROTTLED_PURPOSE,
                ) => (
                    &mut overhead.entitlement_fallback_requests,
                    &mut overhead.entitlement_fallback_tokens,
                ),
                Some(
                    CONTEXT_FIREWALL_REDUCTION_PURPOSE
                    | CONTEXT_FIREWALL_BYPASS_PURPOSE
                    | CONTEXT_FIREWALL_EXPANSION_PURPOSE,
                ) => (
                    &mut overhead.context_firewall_requests,
                    &mut overhead.context_firewall_tokens,
                ),
                Some(HARNESS_TURN_PURPOSE) | None if group.harness_recorded => (
                    &mut overhead.coding_agent_requests,
                    &mut overhead.coding_agent_tokens,
                ),
                _ => (
                    &mut overhead.unstamped_requests,
                    &mut overhead.unstamped_tokens,
                ),
            };
            add_consumption(named, group.sample_count, tokens);
            // Line 1466's denominator is *everything that is not the routing
            // model*, and it keeps that meaning: the four buckets above,
            // minus classification, sum to exactly this.
            if group.purpose.as_deref() != Some(CLASSIFICATION_PURPOSE) {
                add_consumption(
                    (&mut overhead.task_requests, &mut overhead.task_tokens),
                    group.sample_count,
                    tokens,
                );
            }
        }
        overhead
    }

    /// Classification tokens as a fraction of task tokens, when both sides
    /// were counted and the task side is not zero.
    pub fn fraction(&self) -> Option<f64> {
        let classification = self.classification_tokens?;
        let task = self.task_tokens?;
        (task > 0).then(|| classification as f64 / task as f64)
    }

    /// Capability map line 1466: whether routing's own spend has crossed
    /// `threshold` of the task spend it exists to protect. `false` whenever
    /// [`Self::fraction`] is `None` — an unmeasured comparison is not a
    /// warning.
    pub fn exceeds(&self, threshold: f64) -> bool {
        self.fraction().is_some_and(|fraction| fraction > threshold)
    }
}

fn duration_aggregate(
    observations: &[RoutingObservation],
    reduce: fn(Vec<i64>) -> i64,
    what: &'static str,
) -> Option<AggregateReading<i64>> {
    let durations: Vec<i64> = observations
        .iter()
        .filter_map(RoutingObservation::duration_ms)
        .collect();
    if durations.len() < MIN_SAMPLE_FOR_SUMMARY {
        return None;
    }
    let window_start = observations
        .iter()
        .filter(|o| o.duration_ms().is_some())
        .map(|o| o.observed_at_unix)
        .min()?;
    let window_end = observations
        .iter()
        .filter(|o| o.duration_ms().is_some())
        .map(|o| o.observed_at_unix)
        .max()?;
    let sample_count = durations.len();
    Some(AggregateReading::new(
        reduce(durations),
        window_start,
        window_end,
        sample_count,
        ReadingSource::LocalObservation(what.to_owned()),
    ))
}

fn ewma_duration_aggregate(observations: &[RoutingObservation]) -> Option<AggregateReading<f64>> {
    let with_duration: Vec<&RoutingObservation> = observations
        .iter()
        .filter(|o| o.duration_ms().is_some())
        .collect();
    if with_duration.len() < MIN_SAMPLE_FOR_SUMMARY {
        return None;
    }
    let durations: Vec<i64> = with_duration
        .iter()
        .filter_map(|o| o.duration_ms())
        .collect();
    let window_start = with_duration.first()?.observed_at_unix;
    let window_end = with_duration.last()?.observed_at_unix;
    Some(AggregateReading::new(
        ewma(&durations),
        window_start,
        window_end,
        durations.len(),
        ReadingSource::LocalObservation(
            "exponentially weighted gateway exchange duration".to_owned(),
        ),
    ))
}

fn failure_class_counts(observations: &[RoutingObservation]) -> FailureClassCounts {
    let mut counts = FailureClassCounts::default();
    for observation in observations {
        counts.record(observation.outcome, observation.failure_class);
    }
    counts
}

/// No enum on this row to fall through on, unlike [`row_to_identity`] next
/// door — `purpose` is a free-form nullable `TEXT` with no vocabulary this
/// module enforces, so there is no unrecognized value to reject, and a plain
/// [`rusqlite::Result`] is honest about that.
fn row_to_purpose_consumption(row: &Row<'_>) -> rusqlite::Result<PurposeConsumption> {
    let sample_count: i64 = row.get("sample_count")?;
    let first_byte_sample_count: i64 = row.get("first_byte_sample_count")?;
    let first_byte_ms_sample_count: i64 = row.get("first_byte_ms_sample_count")?;
    let first_token_sample_count: i64 = row.get("first_token_sample_count")?;
    let first_token_ms_sample_count: i64 = row.get("first_token_ms_sample_count")?;
    let first_tool_call_sample_count: i64 = row.get("first_tool_call_sample_count")?;
    let first_tool_call_ms_sample_count: i64 = row.get("first_tool_call_ms_sample_count")?;
    Ok(PurposeConsumption {
        purpose: row.get("purpose")?,
        harness_recorded: row.get("harness_recorded")?,
        sample_count: sample_count as usize,
        input_tokens: row.get("input_tokens")?,
        output_tokens: row.get("output_tokens")?,
        cached_input_tokens: row.get("cached_input_tokens")?,
        first_byte_sample_count: first_byte_sample_count as usize,
        first_byte_ms_sample_count: first_byte_ms_sample_count as usize,
        mean_time_to_first_byte_ms: row.get("mean_time_to_first_byte_ms")?,
        first_token_sample_count: first_token_sample_count as usize,
        first_token_ms_sample_count: first_token_ms_sample_count as usize,
        mean_time_to_first_token_ms: row.get("mean_time_to_first_token_ms")?,
        first_tool_call_sample_count: first_tool_call_sample_count as usize,
        first_tool_call_ms_sample_count: first_tool_call_ms_sample_count as usize,
        mean_time_to_first_tool_call_ms: row.get("mean_time_to_first_tool_call_ms")?,
        decode_output_tokens: row.get("decode_output_tokens")?,
        decode_ms: row.get("decode_ms")?,
        tool_rounds: row.get("tool_rounds")?,
        repairs: row.get("repairs")?,
        serving_seconds: row.get("serving_seconds")?,
        failure_rate_sample: {
            let failure_rate_sample: i64 = row.get("failure_rate_sample")?;
            failure_rate_sample as usize
        },
        failure_rate: {
            let failure_rate_sample: i64 = row.get("failure_rate_sample")?;
            if failure_rate_sample as usize >= MIN_SAMPLE_FOR_SUMMARY {
                let failed_count: i64 = row.get("failed_count")?;
                Some(failed_count as f64 / failure_rate_sample as f64)
            } else {
                None
            }
        },
    })
}

fn row_to_identity(
    row: &Row<'_>,
) -> rusqlite::Result<Result<ObservedIdentity, EvidenceLedgerError>> {
    let context_text: String = row.get("context_state")?;
    let Some(context_state) = ContextState::from_stored(&context_text) else {
        return Ok(Err(EvidenceLedgerError::UnknownAggregateValue {
            column: "context_state",
            value: context_text,
        }));
    };
    let sample_count: i64 = row.get("sample_count")?;
    Ok(Ok(ObservedIdentity {
        provider: row.get("provider")?,
        model: row.get("model")?,
        route: row.get("route")?,
        context_state,
        sample_count: sample_count as usize,
        window_start_unix: row.get("window_start")?,
        window_end_unix: row.get("window_end")?,
    }))
}

impl EvidenceLedger {
    /// Rolling summaries for one `(provider, model, route, harness)`
    /// identity, within one [`ContextState`] bucket, computed from every
    /// observation newer than `now_unix - window_seconds` — capability map
    /// line 1341's decay: nothing older than the window contributes to the
    /// aggregate, but nothing is deleted from the table to make that true.
    /// [`Self::summarize_latest_for_model`]'s own private helper, kept as a
    /// method rather than a free function because it is the one place this
    /// crate builds a [`RoutingSummary`] from a named identity.
    fn summarize(
        &self,
        query: ObservationQuery<'_>,
        context_state: ContextState,
        now_unix: i64,
        window_seconds: i64,
    ) -> Result<RoutingSummary, EvidenceLedgerError> {
        let earliest = now_unix.saturating_sub(window_seconds);
        let observations = {
            let conn = self.lock();
            let mut statement = conn
                .prepare(
                    "SELECT * FROM routing_observations
                     WHERE provider = ?1 AND model = ?2
                       AND route IS ?3 AND harness IS ?4
                       AND context_state = ?5
                       AND observed_at >= ?6 AND observed_at <= ?7
                     ORDER BY observed_at ASC",
                )
                .map_err(sql_err("read routing observations"))?;
            let rows = statement
                .query_map(
                    params![
                        query.provider,
                        query.model,
                        query.route,
                        query.harness,
                        context_state.as_str(),
                        earliest,
                        now_unix
                    ],
                    row_to_observation,
                )
                .map_err(sql_err("read routing observations"))?;
            let mut observations = Vec::new();
            for row in rows {
                observations.push(row.map_err(sql_err("read a routing observation"))??);
            }
            observations
        };

        Ok(RoutingSummary {
            provider: query.provider.to_owned(),
            model: query.model.to_owned(),
            route: query.route.map(str::to_owned),
            context_state,
            median_duration_ms: duration_aggregate(
                &observations,
                median,
                "median gateway exchange duration",
            ),
            tail_duration_ms: duration_aggregate(
                &observations,
                p95,
                "p95 gateway exchange duration",
            ),
            ewma_duration_ms: ewma_duration_aggregate(&observations),
            failure_rate: failure_rate_aggregate(&observations),
            failure_classes: failure_class_counts(&observations),
        })
    }

    /// Every provider's [`FailureClassCounts`] over the window ending at
    /// `now_unix` — capability map lines 1316 and 1365's reader, at the grain
    /// `glasshouse resources` renders: one entry per provider, across every
    /// model, route, harness and context state it was observed under.
    ///
    /// Per provider rather than per `ObservationQuery` identity because
    /// the question these two lines ask — *is this provider throttling me,
    /// out of quota, or unwell?* — is about the resource, and
    /// `crate::provider::resources` keys its health rendering by provider
    /// name exactly as [`crate::provider::telemetry::GatewayHealthCache`]
    /// does. Blending across context states is harmless here because these
    /// are counts of failures, not the latency figures line 1337 forbids
    /// averaging across a cache boundary.
    ///
    /// One `GROUP BY` rather than a row-by-row read: the ledger may hold a
    /// long session's every exchange, and a report should not pull each of
    /// them into memory to count nine buckets.
    pub fn failure_classes_by_provider(
        &self,
        now_unix: i64,
        window_seconds: i64,
    ) -> Result<std::collections::BTreeMap<String, FailureClassCounts>, EvidenceLedgerError> {
        let earliest = now_unix.saturating_sub(window_seconds);
        let conn = self.lock();
        let mut statement = conn
            .prepare(
                "SELECT provider, outcome, failure_class, COUNT(*) AS n
                 FROM routing_observations
                 WHERE observed_at >= ?1 AND observed_at <= ?2
                 GROUP BY provider, outcome, failure_class",
            )
            .map_err(sql_err("count routing failures by class"))?;
        let rows = statement
            .query_map(params![earliest, now_unix], |row| {
                let provider: String = row.get("provider")?;
                let outcome: Option<String> = row.get("outcome")?;
                let class: Option<String> = row.get("failure_class")?;
                let n: i64 = row.get("n")?;
                Ok((provider, outcome, class, n))
            })
            .map_err(sql_err("count routing failures by class"))?;

        let mut out: std::collections::BTreeMap<String, FailureClassCounts> = Default::default();
        for row in rows {
            let (provider, outcome, class, n) =
                row.map_err(sql_err("count routing failures by class"))?;
            // A stored value this build does not recognise is reported, not
            // guessed at — the same refusal `row_to_observation` makes. A
            // grouped row has no single `seq` to name, so `-1` says so.
            let outcome = match outcome {
                None => None,
                Some(text) => Some(Outcome::from_stored(&text).ok_or_else(|| {
                    EvidenceLedgerError::UnknownValue {
                        seq: -1,
                        column: "outcome",
                        value: text,
                    }
                })?),
            };
            let class = match class {
                None => None,
                Some(text) => Some(FailureClass::from_stored(&text).ok_or_else(|| {
                    EvidenceLedgerError::UnknownValue {
                        seq: -1,
                        column: "failure_class",
                        value: text,
                    }
                })?),
            };
            let counts = out.entry(provider).or_default();
            for _ in 0..n.max(0) {
                counts.record(outcome, class);
            }
        }
        Ok(out)
    }

    /// Every outcome-carrying observation in the window ending at `now_unix`,
    /// for a caller that needs the raw rows: map line 1965's entitlement
    /// telemetry resolver narrows them by provider and
    /// [`RoutingObservation::quota_context`].
    pub fn observations_in_window(
        &self,
        now_unix: i64,
        window_seconds: i64,
    ) -> Result<Vec<RoutingObservation>, EvidenceLedgerError> {
        let earliest = now_unix.saturating_sub(window_seconds);
        let conn = self.lock();
        let mut statement = conn
            .prepare(
                "SELECT * FROM routing_observations
                 WHERE project_id = ?1
                   AND observed_at >= ?2 AND observed_at <= ?3
                   AND outcome IS NOT NULL
                 ORDER BY observed_at ASC",
            )
            .map_err(sql_err("read routing observations in a window"))?;
        let rows = statement
            .query_map(
                params![self.project_id, earliest, now_unix],
                row_to_observation,
            )
            .map_err(sql_err("read routing observations in a window"))?;
        let mut observations = Vec::new();
        for row in rows {
            observations.push(row.map_err(sql_err("read a routing observation"))??);
        }
        Ok(observations)
    }

    /// Every observation in the window ending at `now_unix`, **whether or
    /// not it carries an outcome** — the row set a *consumption* reader
    /// needs, and the one [`Self::observations_in_window`] deliberately
    /// cannot serve, since it filters `outcome IS NOT NULL` for its own
    /// callers, which classify *how exchanges went* and treat a row with no
    /// recorded outcome as no evidence about that question.
    ///
    /// Capability map lines 1274 and 1276 ask how much of a resource was
    /// **consumed** — a request whose outcome nobody wrote down still
    /// consumed the request — and the one producer that carries a task
    /// class today (`main.rs::record_routing_latency`) records no outcome at
    /// all, so widening `observations_in_window` instead would silently
    /// change what four existing classifiers count.
    ///
    /// Ordered by `observed_at` ascending, like its sibling, because a
    /// caller bucketing by time reads an idle gap as a property of
    /// consecutive rows.
    // History: design-decisions.md, "Trims: routing module docs", routing/evidence/readers.rs `fn consumption_in_window`.
    pub fn consumption_in_window(
        &self,
        now_unix: i64,
        window_seconds: i64,
    ) -> Result<Vec<RoutingObservation>, EvidenceLedgerError> {
        let earliest = now_unix.saturating_sub(window_seconds);
        let conn = self.lock();
        let mut statement = conn
            .prepare(
                "SELECT * FROM routing_observations
                 WHERE project_id = ?1
                   AND observed_at >= ?2 AND observed_at <= ?3
                 ORDER BY observed_at ASC",
            )
            .map_err(sql_err("read routing consumption in a window"))?;
        let rows = statement
            .query_map(
                params![self.project_id, earliest, now_unix],
                row_to_observation,
            )
            .map_err(sql_err("read routing consumption in a window"))?;
        let mut observations = Vec::new();
        for row in rows {
            observations.push(row.map_err(sql_err("read a routing observation"))??);
        }
        Ok(observations)
    }

    /// `Self::summarize` for whichever `(route, harness, context_state)`
    /// this `(provider, model)` was most recently observed under — additive,
    /// because a caller that only knows a routing selection's provider and
    /// model from configuration (never its route, harness or context-state
    /// bucket) cannot build the `ObservationQuery` `Self::summarize`
    /// requires, the same gap [`Self::observed_identities`] closed for
    /// listing rather than summarizing (practice §71). This picks the single
    /// most recently active identity for the pair and summarizes exactly
    /// that one — never blended across context states, matching every other
    /// summary this ledger returns.
    ///
    /// `Ok(None)` means no observation exists for this `(provider, model)` at
    /// all, within the window. That is a different fact from
    /// [`RoutingSummary`]'s own `None` fields (observed, but below
    /// [`MIN_SAMPLE_FOR_SUMMARY`]) — a caller that only wants "is there a
    /// figure to show" can treat both the same way, but one that wants to say
    /// *why* there is not should keep them apart.
    pub fn summarize_latest_for_model(
        &self,
        provider: &str,
        model: &str,
        now_unix: i64,
        window_seconds: i64,
    ) -> Result<Option<RoutingSummary>, EvidenceLedgerError> {
        let earliest = now_unix.saturating_sub(window_seconds);
        let identity = {
            let conn = self.lock();
            conn.query_row(
                "SELECT route, harness, context_state
                 FROM routing_observations
                 WHERE project_id = ?1 AND provider = ?2 AND model = ?3
                   AND observed_at >= ?4 AND observed_at <= ?5
                 ORDER BY observed_at DESC
                 LIMIT 1",
                params![self.project_id, provider, model, earliest, now_unix],
                |row| {
                    let route: Option<String> = row.get(0)?;
                    let harness: Option<String> = row.get(1)?;
                    let context_state: String = row.get(2)?;
                    Ok((route, harness, context_state))
                },
            )
            .optional()
            .map_err(sql_err(
                "find the most recently observed identity for a model",
            ))?
        };
        let Some((route, harness, context_text)) = identity else {
            return Ok(None);
        };
        let Some(context_state) = ContextState::from_stored(&context_text) else {
            return Err(EvidenceLedgerError::UnknownAggregateValue {
                column: "context_state",
                value: context_text,
            });
        };
        let query = ObservationQuery {
            provider,
            model,
            route: route.as_deref(),
            harness: harness.as_deref(),
        };
        Ok(Some(self.summarize(
            query,
            context_state,
            now_unix,
            window_seconds,
        )?))
    }

    /// The distinct `(provider, model, route, context_state)` identities
    /// this project has actually recorded within the last `window_seconds`,
    /// most recently active first — capability map lines 1762 and 1764, and
    /// the enumeration link batch 42 found missing (practice §71): every
    /// other reader on this ledger requires the caller to already name an
    /// identity; this is the one method on this ledger that answers which
    /// identities exist at all.
    ///
    /// A `SELECT DISTINCT`, expressed as a `GROUP BY` with its own count and
    /// window — over columns `routing_observations` already has. No schema
    /// change. Bounded by `limit`: an unbounded listing over a growing table
    /// is a defect waiting for a busy project.
    ///
    /// Scoped to this ledger's own `project_id`, like every write this
    /// ledger makes — belt-and-suspenders alongside the physical per-project
    /// database file [`Self::open`] already guarantees, because this method
    /// reads across every identity in the table rather than one
    /// already-named one.
    pub fn observed_identities(
        &self,
        now_unix: i64,
        window_seconds: i64,
        limit: usize,
    ) -> Result<Vec<ObservedIdentity>, EvidenceLedgerError> {
        let earliest = now_unix.saturating_sub(window_seconds);
        let conn = self.lock();
        let mut statement = conn
            .prepare(
                "SELECT provider, model, route, context_state,
                        COUNT(*) AS sample_count,
                        MIN(observed_at) AS window_start,
                        MAX(observed_at) AS window_end
                 FROM routing_observations
                 WHERE project_id = ?1 AND observed_at >= ?2 AND observed_at <= ?3
                 GROUP BY provider, model, route, context_state
                 ORDER BY window_end DESC, provider ASC, model ASC, route ASC, context_state ASC
                 LIMIT ?4",
            )
            .map_err(sql_err("read observed routing identities"))?;
        let rows = statement
            .query_map(
                params![self.project_id, earliest, now_unix, limit as i64],
                row_to_identity,
            )
            .map_err(sql_err("read observed routing identities"))?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row.map_err(sql_err("read an observed routing identity"))??);
        }
        Ok(out)
    }

    /// [`PurposeConsumption`] for every `(purpose, harness_recorded)` group
    /// this ledger holds a row for, within one window — capability map line
    /// 1464, and the aggregate this module's own header says nothing
    /// computes yet.
    ///
    /// Grouped by `purpose` first, so a routing model's own spend never
    /// folds into anyone else's total; within the `NULL`-purpose rows every
    /// other producer leaves, split again by whether a harness was
    /// recorded — the distinction `purpose` alone cannot make between
    /// coding-agent and other `NULL`-purpose consumption.
    ///
    /// `SUM(input_tokens)` and its siblings rely on SQLite's own aggregate
    /// skipping `NULL` inputs and answering `NULL` (never `0`) for a group
    /// with none, read straight into `Option<i64>` with no manual
    /// accumulate-and-default to weaken. `mean_time_to_first_byte_ms`
    /// **prefers migration 25's measured offset** per row, falling back to
    /// `first_byte_at - dispatched_at` only when a row lacks it, so a window
    /// spanning the migration produces one mean rather than two incomparable
    /// ones; `first_token_*`/`first_tool_call_*` are the identical triple.
    /// `decode_output_tokens`/`decode_ms` (line 1349's pair) have **no**
    /// seconds fallback, since at one-second resolution the denominator is
    /// routinely `0`.
    ///
    /// Scoped to this ledger's own `project_id`, like [`Self::observed_identities`].
    // History: design-decisions.md, "Trims: routing module docs", routing/evidence/readers.rs `fn consumption_by_purpose`.
    pub fn consumption_by_purpose(
        &self,
        now_unix: i64,
        window_seconds: i64,
    ) -> Result<Vec<PurposeConsumption>, EvidenceLedgerError> {
        let earliest = now_unix.saturating_sub(window_seconds);
        let conn = self.lock();
        let mut statement = conn
            .prepare(
                "SELECT purpose,
                        (harness IS NOT NULL) AS harness_recorded,
                        COUNT(*) AS sample_count,
                        SUM(input_tokens) AS input_tokens,
                        SUM(output_tokens) AS output_tokens,
                        SUM(cached_input_tokens) AS cached_input_tokens,
                        COUNT(first_byte_at) AS first_byte_sample_count,
                        COUNT(first_byte_ms) AS first_byte_ms_sample_count,
                        AVG(
                            CASE
                                WHEN first_byte_ms IS NOT NULL
                                THEN CAST(first_byte_ms AS REAL)
                                WHEN first_byte_at IS NOT NULL AND dispatched_at IS NOT NULL
                                THEN CAST(first_byte_at - dispatched_at AS REAL) * 1000
                            END
                        ) AS mean_time_to_first_byte_ms,
                        COUNT(first_token_at) AS first_token_sample_count,
                        COUNT(first_token_ms) AS first_token_ms_sample_count,
                        AVG(
                            CASE
                                WHEN first_token_ms IS NOT NULL
                                THEN CAST(first_token_ms AS REAL)
                                WHEN first_token_at IS NOT NULL AND dispatched_at IS NOT NULL
                                THEN CAST(first_token_at - dispatched_at AS REAL) * 1000
                            END
                        ) AS mean_time_to_first_token_ms,
                        COUNT(first_tool_call_at) AS first_tool_call_sample_count,
                        COUNT(first_tool_call_ms) AS first_tool_call_ms_sample_count,
                        AVG(
                            CASE
                                WHEN first_tool_call_ms IS NOT NULL
                                THEN CAST(first_tool_call_ms AS REAL)
                                WHEN first_tool_call_at IS NOT NULL AND dispatched_at IS NOT NULL
                                THEN CAST(first_tool_call_at - dispatched_at AS REAL) * 1000
                            END
                        ) AS mean_time_to_first_tool_call_ms,
                        SUM(
                            CASE
                                WHEN output_tokens IS NOT NULL
                                 AND first_token_ms IS NOT NULL
                                 AND completed_ms IS NOT NULL
                                 AND completed_ms >= first_token_ms
                                THEN output_tokens
                            END
                        ) AS decode_output_tokens,
                        SUM(
                            CASE
                                WHEN output_tokens IS NOT NULL
                                 AND first_token_ms IS NOT NULL
                                 AND completed_ms IS NOT NULL
                                 AND completed_ms >= first_token_ms
                                THEN completed_ms - first_token_ms
                            END
                        ) AS decode_ms,
                        SUM(tool_rounds) AS tool_rounds,
                        SUM(repairs) AS repairs,
                        SUM(
                            CASE
                                WHEN completed_at IS NOT NULL AND dispatched_at IS NOT NULL
                                THEN completed_at - dispatched_at
                            END
                        ) AS serving_seconds,
                        COUNT(CASE WHEN outcome IN ('succeeded', 'failed') THEN 1 END)
                            AS failure_rate_sample,
                        SUM(CASE WHEN outcome = 'failed' THEN 1 ELSE 0 END) AS failed_count
                 FROM routing_observations
                 WHERE project_id = ?1 AND observed_at >= ?2 AND observed_at <= ?3
                 GROUP BY purpose, harness_recorded
                 ORDER BY purpose IS NULL, purpose ASC, harness_recorded DESC",
            )
            .map_err(sql_err("read routing consumption by purpose"))?;
        let rows = statement
            .query_map(
                params![self.project_id, earliest, now_unix],
                row_to_purpose_consumption,
            )
            .map_err(sql_err("read routing consumption by purpose"))?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row.map_err(sql_err("read one purpose's routing consumption"))?);
        }
        Ok(out)
    }

    /// A session-scoped translation cache reading, with no time window —
    /// capability map line 1760's evidence half: `sessions show <id> --debug`
    /// reads this to show what providers actually reported on this session's
    /// own translated exchanges.
    ///
    /// A whole-session reading rather than a windowed one, deliberately: one
    /// session's own exchanges are a bounded set already, and windowing them
    /// by recency would silently drop a session's earliest turns from its
    /// own evidence.
    ///
    /// `Ok(None)` is *no translated exchange has reported cached-input
    /// tokens for this session* — a session started before migration 24, a
    /// session served only by relayed exchanges (which never carry
    /// `input_tokens`, this module's own header), or a session with no
    /// exchanges at all. Never a session with a zero share: a session that
    /// warmed nothing still has a `sample_count` and a real `0%`, and this
    /// return is reserved for having nothing to report at all.
    pub fn cached_share_for_session(
        &self,
        session_id: &str,
    ) -> Result<Option<SessionTranslationSavings>, EvidenceLedgerError> {
        let conn = self.lock();
        let (sample_count, input_tokens, cached_input_tokens): (i64, i64, i64) = conn
            .query_row(
                "SELECT COUNT(*) AS sample_count,
                        COALESCE(SUM(input_tokens), 0) AS input_tokens,
                        COALESCE(SUM(cached_input_tokens), 0) AS cached_input_tokens
                 FROM routing_observations
                 WHERE project_id = ?1 AND session_id = ?2
                   AND purpose = ?3 AND input_tokens IS NOT NULL",
                params![self.project_id, session_id, HARNESS_TURN_PURPOSE],
                |row| {
                    Ok((
                        row.get("sample_count")?,
                        row.get("input_tokens")?,
                        row.get("cached_input_tokens")?,
                    ))
                },
            )
            .map_err(sql_err("read a session's cached-input share"))?;
        if sample_count == 0 {
            return Ok(None);
        }
        Ok(Some(SessionTranslationSavings {
            session_id: Some(session_id.to_owned()),
            sample_count: sample_count as usize,
            input_tokens,
            cached_input_tokens,
        }))
    }
}
