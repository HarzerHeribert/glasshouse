//! The readers that summarise `routing_observations` alone: per-route,
//! per-provider and per-session summaries, translation and consumption
//! savings, and the classification/support-work latency records. Nothing
//! here reads `evaluation_observations` or any other table — see `joins.rs`
//! for that, and see `signals.rs` for the throttle/correlation/credential
//! classification over an already-fetched observation slice.

use super::*;

use rusqlite::{OptionalExtension, Row, params};

use crate::config::pairing::{ObservationSource, ObservedEvidence};
use crate::harness::pairing::EvidenceKey;
use crate::provider::quota::{Freshness, ReadingSource};

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
/// 42 found and this package builds (practice §71): [`EvidenceLedger::recent`]
/// and [`EvidenceLedger::summarize`] both require the caller to already name
/// an identity via [`ObservationQuery`]; neither, nor anything else on this
/// ledger before [`EvidenceLedger::observed_identities`], can answer "which
/// identities exist at all."
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
    /// [`ObservationQuery::route`]'s own convention.
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
/// [`EvidenceLedger::consumption_by_purpose`] builds: every other reader on
/// this ledger requires the caller to already name an identity, and nothing
/// before this grouped by the columns that answer *what a call was for* and
/// *whether a harness was relaying it*.
///
/// `purpose` alone is not enough to separate coding-agent consumption from
/// everything else: `purpose` is `None` for every row no producer has
/// stamped, and today that is **both** every gateway relay exchange (line
/// 1464's own "coding-agent consumption", `crate::gateway::session`, which
/// always calls [`NewObservation::with_harness`]) **and** every
/// memory-extraction call (`crate::memory::extract::ModelCall::observation`,
/// which never does) — see [`NewObservation::with_purpose`]'s doc comment
/// for why extraction's rows are not back-filled with one. `harness_recorded`
/// is what tells those two `NULL`-purpose producers apart: `true` only when
/// every row in the group named a harness, which today means gateway rows
/// and gateway rows alone.
///
/// `sample_count` is a real `COUNT(*)`, always defined. The three token
/// fields are not: each is `None` when every row in the group left that
/// column `NULL`, which is a different fact from `Some(0)` and must stay
/// one — the hazard this whole aggregate exists to avoid rendering as a
/// number. A group that mixes counted and uncounted rows sums only what was
/// counted, exactly as [`NewObservation::with_tokens`] asks every producer to
/// leave absent counts absent rather than zeroed.
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
    /// real `COUNT(first_token_at)`. Only a **translated** exchange can ever
    /// supply it (`GH-STREAM-FIRST-EVENTS`, lines 1331/1332), so it is
    /// honestly `0` for every group whose rows are all relayed.
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
    /// `None` when none did. Line 1350's denominator for
    /// [`Self::tool_rounds_per_minute`], independent of whether those same
    /// rows counted a tool round.
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
    /// should trust. 1351's *purpose's failure rate*, the second half of
    /// [`Self::effective_ttfc_ms`].
    pub failure_rate: Option<f64>,
}

impl PurposeConsumption {
    /// Line 1350: tool rounds per minute of serving time, an outcome-adjacent
    /// measure — never folded into a quality score, this module's own header
    /// and `docs/product/design-decisions.md`'s *"Tool rounds and repairs on
    /// the translated path"* both keep that rule. `None` when either half is
    /// unrecorded or the group's summed serving time is `0`, never a
    /// fabricated rate.
    pub fn tool_rounds_per_minute(&self) -> Option<f64> {
        let rounds = self.tool_rounds?;
        let serving_seconds = self.serving_seconds?;
        if serving_seconds == 0 {
            return None;
        }
        Some(rounds as f64 * 60.0 / serving_seconds as f64)
    }

    /// Line 1349: decode tokens per second — output tokens over the time
    /// between the first real token and the end of the exchange, summed
    /// across exactly the rows that recorded all three.
    ///
    /// **A model-serving characteristic and not task progress**, which is the
    /// whole of what line 1349 asks for and the reason it is a method here
    /// and never a term in any score: a fast decode says the provider is
    /// serving quickly, not that the agent got anywhere. It is printed on its
    /// own line beside TTFC and TTFT (line 1355) rather than folded in with
    /// them.
    ///
    /// `None` when either half is unrecorded — a group of rows written
    /// before migration 25 has no `first_token_ms` at all, and there is no
    /// seconds fallback here on purpose: at one-second resolution the
    /// denominator is routinely `0` and the rate it produces is an artefact
    /// of the clock rather than a reading. `None` too when the summed decode
    /// time is `0`, never an infinite rate.
    pub fn decode_tokens_per_second(&self) -> Option<f64> {
        let output_tokens = self.decode_output_tokens?;
        let decode_ms = self.decode_ms?;
        if decode_ms <= 0 {
            return None;
        }
        Some(output_tokens as f64 * 1000.0 / decode_ms as f64)
    }

    /// Line 1351: effective TTFC, `mean_time_to_first_tool_call_ms` divided
    /// by one minus this group's own failure rate — the fifth figure line
    /// 1355 names, on a `PurposeConsumption` group rather than a single
    /// route. `None` unless both halves clear [`MIN_SAMPLE_FOR_SUMMARY`]
    /// (the TTFC figure's own [`Self::first_tool_call_ms_sample_count`], and
    /// [`Self::failure_rate_sample`] behind [`Self::failure_rate`]) and the
    /// failure rate is below 100% — never a clamped number.
    /// [`RouteResponsiveness::effective_ttfc_ms`] is the same formula over a
    /// raw observation slice; this is its `PurposeConsumption`-shaped
    /// sibling.
    pub fn effective_ttfc_ms(&self) -> Option<f64> {
        if self.first_tool_call_ms_sample_count < MIN_SAMPLE_FOR_SUMMARY {
            return None;
        }
        let raw = self.mean_time_to_first_tool_call_ms?;
        let p = self.failure_rate?;
        if p >= 1.0 {
            return None;
        }
        Some(raw / (1.0 - p))
    }
}

/// [`EvidenceLedger::translation_cache_savings`]'s result — map line 2034's
/// translation facet, one row per `(route, quota_context)` that carries at
/// least one [`HARNESS_TURN_PURPOSE`] row with `input_tokens` in the window.
///
/// `input_tokens` and `cached_input_tokens` are plain `i64`, not `Option`,
/// because this reader's own `WHERE input_tokens IS NOT NULL` (see the
/// method's doc comment) already excludes every relayed row before the
/// `GROUP BY` runs — a group that exists at all is, by construction, a
/// translated one, so there is nothing here for a `None` to distinguish.
/// `cached_input_tokens` is summed with SQL's `COALESCE(...,0)` for the same
/// reason [`RoutingOverhead`]'s own doc comment gives for leaving cached
/// tokens out of its spend sum elsewhere: a translated row can carry
/// `input_tokens` with no cache activity that turn, and that omission must
/// not turn the whole group's cache figure absent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TranslationSavings {
    /// `None` matches [`ObservationQuery::route`]'s own convention: these
    /// rows were recorded with no route, not "any route."
    pub route: Option<String>,
    /// The credential label `crate::gateway::session` stamps on every
    /// translated row — see `with_quota_context` there.
    pub quota_context: Option<String>,
    pub sample_count: usize,
    pub input_tokens: i64,
    pub cached_input_tokens: i64,
}

impl TranslationSavings {
    /// Prompt-cache reads over translated input tokens — `cached_input_tokens`
    /// of `input_tokens + cached_input_tokens`, `None` only when the
    /// denominator is `0`, which cannot happen for a group this reader's own
    /// `WHERE input_tokens IS NOT NULL` produced from at least one row, but
    /// is still handled rather than assumed away.
    pub fn cache_read_ratio(&self) -> Option<f64> {
        let denominator = self.input_tokens + self.cached_input_tokens;
        (denominator > 0).then(|| self.cached_input_tokens as f64 / denominator as f64)
    }
}

/// [`TranslationSavings`] grouped by the session that was served rather than
/// by the route and credential that served it — capability map line 2019's
/// *"show the per-session cache ratio beside the routing evidence"*, whose
/// producer is migration 24's `session_id`.
///
/// The same reader, the same window and the same `WHERE input_tokens IS NOT
/// NULL` filter as [`EvidenceLedger::translation_cache_savings`], so every
/// note on that type applies here unchanged. What differs is the grouping
/// key, and one consequence of it: `session_id` is nullable, so **one group
/// may have no session at all** — every translated row written by a gateway
/// nothing told which session it serves, and every row written before
/// migration 24. That group is a real reading about real exchanges and is
/// not dropped; it is [`Self::session_id`] `None`, and a renderer says so in
/// words rather than printing an empty name or a zero.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionTranslationSavings {
    /// `None` is *these rows name no session*, never "any session" — the
    /// convention [`TranslationSavings::route`] already follows.
    pub session_id: Option<String>,
    pub sample_count: usize,
    pub input_tokens: i64,
    pub cached_input_tokens: i64,
}

impl SessionTranslationSavings {
    /// [`TranslationSavings::cache_read_ratio`], per session.
    pub fn cache_read_ratio(&self) -> Option<f64> {
        let denominator = self.input_tokens + self.cached_input_tokens;
        (denominator > 0).then(|| self.cached_input_tokens as f64 / denominator as f64)
    }

    /// Whether this group carries at least [`MIN_SAMPLE_FOR_SUMMARY`]
    /// exchanges — the standing floor every *rate* on this ledger sits
    /// behind ([`AggregateReading`]'s own doc comment), applied to the ratio
    /// and to nothing else. The counts beside it are counts, not rates, and
    /// are honest at any sample size, exactly as
    /// [`PurposeConsumption::sample_count`] is.
    pub fn meets_sample_floor(&self) -> bool {
        self.sample_count >= MIN_SAMPLE_FOR_SUMMARY
    }
}

/// What this project's ledger holds about one `(provider, model)` **as a
/// routing-model classifier** — capability map lines 1422/1432 (does it
/// come back in the schema?) and 1421/1435 (how long does it take?) — read
/// from the [`CLASSIFICATION_PURPOSE`] rows alone.
///
/// Two counts and one median, each carrying its own denominator:
///
/// - `outcomes_recorded` is the number of rows that carry a parse outcome
///   at all — [`Outcome::Succeeded`] or [`Outcome::Failed`] — and `parsed`
///   is how many of those succeeded. A row with no outcome (written by a
///   build before the producer recorded one) counts in neither: it is not
///   evidence of reliability in either direction.
/// - `timed` is how many rows carry a duration, and `median_duration_ms`
///   is their median **only** once there are at least
///   [`MIN_SAMPLE_FOR_SUMMARY`] of them — the same floor every other figure
///   on this ledger sits behind. Below it the field is `None`, which a
///   consumer must read as *unmeasured*, never as fast.
///
/// **Resolution is one second.** `dispatched_at` and `completed_at` are
/// whole Unix seconds (this module's header, on line 1332's gap), so every
/// duration here is a multiple of 1000ms, and a ceiling compared against
/// this median is honest only to the second.
///
/// Not split by [`ContextState`]: a classification call is a fresh prompt
/// every time with nothing warm to keep, and its producer records
/// [`ContextState::Unknown`] on every row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClassificationRecord {
    pub provider: String,
    pub model: String,
    /// Rows carrying [`Outcome::Succeeded`] or [`Outcome::Failed`].
    pub outcomes_recorded: usize,
    /// Of those, the rows whose reply parsed as a classification.
    pub parsed: usize,
    /// Rows carrying a duration at all.
    pub timed: usize,
    /// The median of those durations, once there are enough to trust.
    pub median_duration_ms: Option<i64>,
}

impl ClassificationRecord {
    /// The share of outcome-carrying rows that parsed, or `None` when no
    /// row carries an outcome — a ratio over a zero denominator is not a
    /// reliability of `0`.
    pub fn parsed_fraction(&self) -> Option<f64> {
        (self.outcomes_recorded > 0).then(|| self.parsed as f64 / self.outcomes_recorded as f64)
    }
}

/// What this project's ledger holds about one `(provider, model)` as a
/// **support-work** resource's measured latency — capability map line 1539,
/// read from the [`EXTRACTION_PURPOSE`] rows alone.
/// [`ClassificationRecord`]'s sibling: the same floor and the same
/// one-second resolution (this module's header, on line 1332's gap), and
/// deliberately no outcome or parse fields — a disposable support-work call
/// has nothing to parse as a classification schema, so there is no
/// reliability axis to carry here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LatencyRecord {
    /// Rows carrying a duration at all.
    pub timed: usize,
    /// The median of those durations, once there are at least
    /// [`MIN_SAMPLE_FOR_SUMMARY`] of them. `None` below the floor — a
    /// consumer must read this as *unmeasured*, never as fast.
    pub median_duration_ms: Option<i64>,
}

/// Routing-model spend set against everything else — capability map line
/// 1465 — as one pure reading over
/// [`EvidenceLedger::consumption_by_purpose`]'s groups, so the arithmetic is
/// testable without a database and is rendered with its denominators rather
/// than as a bare ratio.
///
/// "Spend" is **tokens**, input plus output as the provider reported them,
/// because that is still the only currency this reading can rely on:
/// `cost_micro_usd` has one producer (map line 1307,
/// `main.rs::record_entitlement_fallback`), and it fires only on an
/// entitlement-fallback event — coding-agent spend routed through the
/// gateway relay, the volume this comparison exists to weigh, leaves the
/// column `NULL` exactly as before. Cached input tokens are left out of the
/// sum — providers disagree on whether they are already inside
/// `input_tokens`, and a sum that might double-count is worse than one that
/// names what it omits.
///
/// A `None` token figure means *no row in that side carried a count*, the
/// same convention [`PurposeConsumption`] keeps; a side that mixes counted
/// and uncounted rows sums only what was counted. [`Self::fraction`] is
/// `None` whenever either side is uncounted or the task side is zero, and
/// [`Self::exceeds`] never fires on an unmeasured comparison.
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

/// One [`EvidenceLedger::request_stats_by_harness`] row — map line 1951's
/// token/wall-clock/request-count half for one harness.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HarnessRequestStats {
    pub harness: String,
    /// Every `routing_observations` row this harness produced in the
    /// window, whether or not it carries timing or token data.
    pub requests: i64,
    /// `None` when no row in this window carries both `dispatched_at` and
    /// `completed_at` — never a fabricated zero.
    pub wall_clock: Option<WallClockSummary>,
    /// Rows carrying an `input_tokens` count — the relay path's rows never
    /// do (refusal register P1b), so this is `0` there, not [`Self::requests`].
    pub token_rows_present: i64,
    /// `input_tokens` summed over exactly [`Self::token_rows_present`] rows.
    /// A caller must print *"not exposed on `requests -
    /// token_rows_present` of `requests` exchanges"* rather than this sum
    /// alone whenever `token_rows_present < requests` (map line 1951's own
    /// mutation: printing `0` for an all-`NULL` group is refused).
    pub input_tokens_sum: i64,
    pub output_tokens_sum: i64,
}

impl HarnessRequestStats {
    fn from_rows(harness: String, rows: &[RoutingObservation]) -> Self {
        let durations: Vec<i64> = rows
            .iter()
            .filter_map(RoutingObservation::duration_ms)
            .collect();
        let wall_clock = (!durations.is_empty()).then(|| WallClockSummary {
            sample_count: durations.len() as i64,
            sum_ms: durations.iter().sum(),
            median_ms: median(durations.clone()),
        });
        let with_tokens: Vec<&RoutingObservation> = rows
            .iter()
            .filter(|observation| observation.input_tokens.is_some())
            .collect();
        Self {
            harness,
            requests: rows.len() as i64,
            wall_clock,
            token_rows_present: with_tokens.len() as i64,
            input_tokens_sum: with_tokens.iter().filter_map(|o| o.input_tokens).sum(),
            output_tokens_sum: with_tokens.iter().filter_map(|o| o.output_tokens).sum(),
        }
    }
}

/// [`HarnessRequestStats::wall_clock`] — `completed_at - dispatched_at`
/// over exactly the rows that carry both, matching
/// [`RoutingObservation::duration_ms`]'s own gap: neither timestamp is
/// invented for a row missing one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WallClockSummary {
    pub sample_count: i64,
    pub sum_ms: i64,
    pub median_ms: i64,
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

fn row_to_session_translation_savings(
    row: &Row<'_>,
) -> rusqlite::Result<SessionTranslationSavings> {
    let sample_count: i64 = row.get("sample_count")?;
    Ok(SessionTranslationSavings {
        session_id: row.get("session_id")?,
        sample_count: sample_count as usize,
        input_tokens: row.get("input_tokens")?,
        cached_input_tokens: row.get("cached_input_tokens")?,
    })
}

fn row_to_translation_savings(row: &Row<'_>) -> rusqlite::Result<TranslationSavings> {
    let sample_count: i64 = row.get("sample_count")?;
    Ok(TranslationSavings {
        route: row.get("route")?,
        quota_context: row.get("quota_context")?,
        sample_count: sample_count as usize,
        input_tokens: row.get("input_tokens")?,
        cached_input_tokens: row.get("cached_input_tokens")?,
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

/// How old an aggregate's most recent contributing observation may be before
/// [`ObservedEvidenceSource`] stops trusting it at full strength — map line
/// 1548's "stale windows count for less." This is distinct from the window
/// [`ObservedEvidenceSource::new`] is given: a row can sit comfortably inside
/// a wide `summarize` window (`crate::routing::interactive`'s own
/// `FAILOVER_EVIDENCE_WINDOW_SECONDS` is seven days) and still be the only
/// thing behind an aggregate that has not moved in days — the window decides
/// what is read at all, this decides how much the read result is trusted.
///
/// Provisional, like [`STALE_OBSERVATION_DISCOUNT`]: a day is long enough
/// that a routing decision inside the same working session trusts it fully,
/// and short enough that "stale" and "within the seven-day evidence window"
/// stay two different words rather than one.
const EVIDENCE_STALE_AFTER_SECONDS: i64 = 24 * 60 * 60;

/// How much a stale aggregate's effective sample count is discounted before
/// [`crate::config::pairing::evidence_signal`] — and, through
/// [`ObservedEvidence::reliable_observation_count`], the native-pairing
/// prior's own decay — ever sees it.
///
/// A fraction, never zero: line 1548 asks stale evidence to count for *less*,
/// not to vanish, and reducing all the way to zero would silently reproduce
/// the "no evidence at all" case this module already represents honestly
/// (an absent [`ObservedEvidence`], not a zeroed-out one — see
/// [`ObservedEvidenceSource::observed`]'s own empty-count fallback).
/// Provisional, tuned against nothing but being large enough to prove
/// against float rounding at [`MIN_SAMPLE_FOR_SUMMARY`]'s own boundary in a
/// test.
const STALE_OBSERVATION_DISCOUNT: f64 = 0.5;

/// [`ObservationSource`] for [`crate::config::pairing`]'s pairing prior —
/// design decision 6, replacing `NoObservations` with a real implementation
/// backed by this ledger.
///
/// A thin wrapper rather than `impl ObservationSource for EvidenceLedger`
/// directly, so the window this evidence is drawn from and the minimum
/// sample it requires are visible at the call site that constructs one,
/// rather than buried as constants only this module can see.
pub struct ObservedEvidenceSource<'a> {
    ledger: &'a EvidenceLedger,
    now_unix: i64,
    window_seconds: i64,
}

impl<'a> ObservedEvidenceSource<'a> {
    pub fn new(ledger: &'a EvidenceLedger, now_unix: i64, window_seconds: i64) -> Self {
        Self {
            ledger,
            now_unix,
            window_seconds,
        }
    }
}

impl ObservationSource for ObservedEvidenceSource<'_> {
    /// See this module's own header for the one gap in this match: `key`'s
    /// launch profile is not part of the query, because nothing this ledger
    /// stores carries one.
    ///
    /// `key.route().provider` is `None` for a first-party, non-gateway
    /// route — this ledger's one producer never records an observation for
    /// one of those (see this module's header), so there is nothing to look
    /// up and this answers `None` rather than guessing a provider.
    fn observed(&self, key: &EvidenceKey) -> Option<ObservedEvidence> {
        let provider = key.route().provider.as_deref()?;
        let route = key.route().protocol.map(|protocol| protocol.slug());
        let query = ObservationQuery {
            provider,
            model: key.model().label(),
            route,
            harness: Some(key.harness().slug()),
        };
        let summary = self
            .ledger
            .summarize(
                query,
                ContextState::Unknown,
                self.now_unix,
                self.window_seconds,
            )
            .ok()?;

        let task_success_rate = summary
            .failure_rate
            .as_ref()
            .map(|reading| 1.0 - reading.value());
        // Line 1548: a stale aggregate contributes less than a fresh one at
        // the same sample count, never a fabricated number — `task_success_rate`
        // above is untouched, only how many observations the rest of this
        // struct claims to stand on. See `EVIDENCE_STALE_AFTER_SECONDS` and
        // `STALE_OBSERVATION_DISCOUNT` for why these two numbers.
        let reliable_observation_count = summary
            .failure_rate
            .as_ref()
            .map(|reading| {
                let raw = reading.sample_count();
                match reading.freshness(self.now_unix, EVIDENCE_STALE_AFTER_SECONDS) {
                    Freshness::Fresh { .. } => raw,
                    Freshness::Stale { .. } => ((raw as f64) * STALE_OBSERVATION_DISCOUNT) as usize,
                }
            })
            .unwrap_or(0);

        if reliable_observation_count == 0 {
            return None;
        }

        Some(ObservedEvidence {
            reliable_observation_count,
            task_success_rate,
            // Not supplied by this ledger's one producer today — see this
            // module's own header. `None` rather than a guess.
            usable_tool_call_rate: None,
            repair_rate: None,
            // Requires `first_byte_at`, which this ledger's gateway producer
            // never records (see this module's header) — there is no honest
            // ratio to compute.
            effective_ttfc_ratio: None,
            reliability: None,
            user_override_signal: None,
        })
    }
}

impl EvidenceLedger {
    /// The most recent observations for one `(provider, model, route)`
    /// identity, newest first — the raw rows line 1335 requires to remain
    /// available beside [`Self::summarize`]'s aggregates, and the read
    /// `routing_observations_by_route_time` (migration 11's own index)
    /// exists to serve.
    ///
    /// `route` and `harness` match exactly, including `None`, which is
    /// deliberate: a route or harness recorded as unknown is a different fact
    /// from any named one, and this read must not conflate them.
    pub fn recent(
        &self,
        query: ObservationQuery<'_>,
        limit: usize,
    ) -> Result<Vec<RoutingObservation>, EvidenceLedgerError> {
        let conn = self.lock();
        let mut statement = conn
            .prepare(
                "SELECT * FROM routing_observations
                 WHERE provider = ?1 AND model = ?2
                   AND route IS ?3 AND harness IS ?4
                 ORDER BY observed_at DESC
                 LIMIT ?5",
            )
            .map_err(sql_err("read routing observations"))?;
        let rows = statement
            .query_map(
                params![
                    query.provider,
                    query.model,
                    query.route,
                    query.harness,
                    limit as i64
                ],
                row_to_observation,
            )
            .map_err(sql_err("read routing observations"))?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row.map_err(sql_err("read a routing observation"))??);
        }
        Ok(out)
    }

    /// **Map line 1629**'s reader: the most recent observations, newest
    /// first, whose `purpose` is [`CLASSIFICATION_PURPOSE`] or
    /// [`EXTRACTION_PURPOSE`] — *"which resource performed important memory
    /// extraction or classification for debugging"* — across every
    /// `(provider, model, route, harness)` identity at once.
    ///
    /// **Not [`Self::recent`].** That method requires the caller to already
    /// name one identity via [`ObservationQuery`], and the question this
    /// line asks is the opposite: which identity performed the work,
    /// unknown in advance. A purpose-filtered sibling fits where `recent`'s
    /// exact-identity shape does not.
    pub fn recent_support_work(
        &self,
        limit: usize,
    ) -> Result<Vec<RoutingObservation>, EvidenceLedgerError> {
        let conn = self.lock();
        let mut statement = conn
            .prepare(
                "SELECT * FROM routing_observations
                 WHERE project_id = ?1 AND purpose IN (?2, ?3)
                 ORDER BY observed_at DESC
                 LIMIT ?4",
            )
            .map_err(sql_err("read support-work routing observations"))?;
        let rows = statement
            .query_map(
                params![
                    self.project_id,
                    CLASSIFICATION_PURPOSE,
                    EXTRACTION_PURPOSE,
                    limit as i64
                ],
                row_to_observation,
            )
            .map_err(sql_err("read support-work routing observations"))?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row.map_err(sql_err("read a support-work routing observation"))??);
        }
        Ok(out)
    }

    /// **Map line 1951**'s token/wall-clock/request-count half, grouped by
    /// harness alone. `routing_observations.harness` is written directly by
    /// every producer
    /// (`crate::gateway::session::record_routing_observation`'s
    /// `.with_harness(...)`, and `main.rs`'s five `with_purpose` call
    /// sites), so this needs no join to `sessions` — unlike
    /// [`crate::evaluation::EvaluationObservations::outcomes_by_tier_and_harness`]'s
    /// outcome half, which has no harness of its own to read and joins
    /// `sessions.harness` instead.
    ///
    /// Reads the raw rows and folds them in Rust rather than aggregating in
    /// SQL — the same choice [`Self::route_correlations`] and
    /// [`Self::throttle_scopes`] make and for the same reason: the
    /// wall-clock median and the "rows without token data" split are
    /// decisions worth testing without a database, not SQL to get right
    /// once and never examine again.
    pub fn request_stats_by_harness(
        &self,
        from: i64,
        to: i64,
    ) -> Result<Vec<HarnessRequestStats>, EvidenceLedgerError> {
        let observations = {
            let conn = self.lock();
            let mut statement = conn
                .prepare(
                    "SELECT * FROM routing_observations
                     WHERE project_id = ?1 AND observed_at >= ?2 AND observed_at <= ?3
                     ORDER BY harness IS NULL, harness ASC, observed_at ASC",
                )
                .map_err(sql_err("read routing observations by harness"))?;
            let rows = statement
                .query_map(params![self.project_id, from, to], row_to_observation)
                .map_err(sql_err("read routing observations by harness"))?;
            let mut observations = Vec::new();
            for row in rows {
                observations.push(row.map_err(sql_err("read a routing observation"))??);
            }
            observations
        };

        let mut by_harness: std::collections::BTreeMap<String, Vec<RoutingObservation>> =
            std::collections::BTreeMap::new();
        for observation in observations {
            let harness = observation
                .harness
                .clone()
                .unwrap_or_else(|| UNKNOWN_HARNESS.to_owned());
            by_harness.entry(harness).or_default().push(observation);
        }

        Ok(by_harness
            .into_iter()
            .map(|(harness, rows)| HarnessRequestStats::from_rows(harness, &rows))
            .collect())
    }

    /// Rolling summaries for one `(provider, model, route, harness)`
    /// identity, within one [`ContextState`] bucket, computed from every
    /// observation newer than `now_unix - window_seconds` — capability map
    /// line 1341's decay: nothing older than the window contributes to the
    /// aggregate, but nothing is deleted from the table to make that true. A
    /// raw row outside the window is still readable through [`Self::recent`]
    /// for as long as it exists.
    pub fn summarize(
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
    /// Per provider rather than per [`ObservationQuery`] identity because
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

    /// Every pair of routes this project has observed failing or serving at
    /// the same moments, over the window ending at `now_unix` — lines 1370,
    /// 1373, 1374 and 1376's reader, and the one door
    /// `crate::gateway::session::SessionRouting::observe_exchange` reaches
    /// [`correlate_routes`] through.
    ///
    /// Reads every outcome-carrying row in the window in one pass and hands
    /// them to the pure function rather than joining in SQL: the overlap
    /// tolerance, the class match and the minimum are decisions, and a
    /// decision belongs where a test reaches it without a database. Rows
    /// with no outcome never inform a pair (see [`RouteCorrelation`]), so
    /// the query leaves them on disk.
    ///
    /// Called once per provider failure, not per exchange: a failover is a
    /// small minority of exchanges, and a full-window read at that moment
    /// costs less than keeping a correlation warm across every exchange that
    /// moved nothing.
    pub fn route_correlations(
        &self,
        now_unix: i64,
        window_seconds: i64,
    ) -> Result<RouteCorrelations, EvidenceLedgerError> {
        let earliest = now_unix.saturating_sub(window_seconds);
        let observations = {
            let conn = self.lock();
            let mut statement = conn
                .prepare(
                    "SELECT * FROM routing_observations
                     WHERE project_id = ?1
                       AND observed_at >= ?2 AND observed_at <= ?3
                       AND outcome IS NOT NULL
                     ORDER BY observed_at ASC",
                )
                .map_err(sql_err("read routing observations for correlation"))?;
            let rows = statement
                .query_map(
                    params![self.project_id, earliest, now_unix],
                    row_to_observation,
                )
                .map_err(sql_err("read routing observations for correlation"))?;
            let mut observations = Vec::new();
            for row in rows {
                observations.push(row.map_err(sql_err("read a routing observation"))??);
            }
            observations
        };
        Ok(correlate_routes(&observations))
    }

    /// Capability map line 1317's reader: [`classify_throttle_scopes`], fed
    /// every outcome-carrying row in the window ending at `now_unix` — the
    /// same query shape [`Self::route_correlations`] runs, for the same
    /// reason: the tolerance, the class match and the minimum are decisions,
    /// and a decision belongs where a test reaches it without a database.
    pub fn throttle_scopes(
        &self,
        now_unix: i64,
        window_seconds: i64,
    ) -> Result<ThrottleScopes, EvidenceLedgerError> {
        Ok(classify_throttle_scopes(
            &self.observations_in_window(now_unix, window_seconds)?,
        ))
    }

    /// Every outcome-carrying observation in the window ending at `now_unix`
    /// — the exact row set [`Self::throttle_scopes`] and
    /// [`Self::route_correlations`] classify, exposed for a caller that
    /// needs the rows themselves: map line 1965's entitlement telemetry
    /// resolver narrows them by provider and
    /// [`RoutingObservation::quota_context`]
    /// ([`recent_credential_throttles`]).
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
    /// cannot serve.
    ///
    /// # Why this is not `observations_in_window` with a flag
    ///
    /// [`Self::observations_in_window`] filters `outcome IS NOT NULL`
    /// because its callers classify *how exchanges went* — a throttle scope,
    /// a route correlation, a failure-class census — and a row with no
    /// recorded outcome is not evidence about that question.
    ///
    /// Capability map lines 1274 and 1276 ask a different question: how much
    /// of a resource was **consumed**. A request whose outcome nobody wrote
    /// down still consumed the request. And the one producer that carries a
    /// task class today — `main.rs::record_routing_latency`, which is the
    /// only caller holding a `crate::routing::request::RouterAnswer` — records no
    /// outcome at all, so every row line 1276 is about is invisible to the
    /// other read. Widening that read instead would silently change what
    /// four existing classifiers count, which is the opposite of what a new
    /// line is allowed to do.
    ///
    /// Ordered by `observed_at` ascending, like its sibling, because
    /// [`crate::routing::burn`] buckets by time and an idle gap is a property of
    /// consecutive rows.
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

    /// [`Self::summarize`] for whichever `(route, harness, context_state)`
    /// this `(provider, model)` was most recently observed under — additive,
    /// because a caller that only knows a routing selection's provider and
    /// model from configuration (never its route, harness or context-state
    /// bucket) cannot build the [`ObservationQuery`] [`Self::summarize`]
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

    /// Capability map line 1564's producer: the [`FailureClass`] of the
    /// **most recent** exchange this project recorded against `(provider,
    /// model)` within the window — `Ok(None)` when nothing was recorded, or
    /// the latest row carried no class (it succeeded, or a producer wrote a
    /// verdict without a kind).
    ///
    /// The latest row and not a count: line 1564 says *after* a clearly
    /// attributable failure, and "the last thing that happened on this
    /// backend" is the attribution this ledger can honestly make — rows
    /// carry no session id, so a count over the window would mix in every
    /// other session's exchanges. `main.rs`'s task-boundary `route` path
    /// reads it for the destination the work is on and hands it to
    /// `SessionRouter::with_retry_after`, which promotes one tier on a
    /// [`FailureClass::RequestIncompatibility`] or
    /// [`FailureClass::EmptyCompletion`] and on nothing else.
    ///
    /// Scoped to this ledger's `project_id`, like [`Self::observed_identities`].
    pub fn latest_failure_class_for_model(
        &self,
        provider: &str,
        model: &str,
        now_unix: i64,
        window_seconds: i64,
    ) -> Result<Option<FailureClass>, EvidenceLedgerError> {
        let earliest = now_unix.saturating_sub(window_seconds);
        let stored: Option<Option<String>> = {
            let conn = self.lock();
            conn.query_row(
                "SELECT failure_class
                 FROM routing_observations
                 WHERE project_id = ?1 AND provider = ?2 AND model = ?3
                   AND observed_at >= ?4 AND observed_at <= ?5
                 ORDER BY observed_at DESC, seq DESC
                 LIMIT 1",
                params![self.project_id, provider, model, earliest, now_unix],
                |row| row.get(0),
            )
            .optional()
            .map_err(sql_err("find the most recent failure class for a model"))?
        };
        match stored.flatten() {
            None => Ok(None),
            Some(text) => FailureClass::from_stored(&text).map(Some).ok_or(
                EvidenceLedgerError::UnknownAggregateValue {
                    column: "failure_class",
                    value: text,
                },
            ),
        }
    }

    /// The distinct `(provider, model, route, context_state)` identities
    /// this project has actually recorded within the last `window_seconds`,
    /// most recently active first — capability map lines 1762 and 1764, and
    /// the enumeration link batch 42 found missing (practice §71):
    /// [`Self::recent`] and [`Self::summarize`] both require the caller to
    /// already name an identity; this is the one method on this ledger that
    /// answers which identities exist at all.
    ///
    /// A `SELECT DISTINCT`, expressed as a `GROUP BY` with its own count and
    /// window — over columns `routing_observations` already has. No schema
    /// change, and [`Self::record`], [`Self::recent`], [`Self::summarize`]
    /// and [`ObservationQuery`] are all untouched. Bounded by `limit`, the
    /// same shape [`Self::recent`] takes: an unbounded listing over a
    /// growing table is a defect waiting for a busy project.
    ///
    /// Scoped to this ledger's own `project_id`, like every write this
    /// ledger makes — belt-and-suspenders alongside the physical per-project
    /// database file [`Self::open`] already guarantees, because this method,
    /// unlike [`Self::recent`] and [`Self::summarize`], reads across every
    /// identity in the table rather than one already-named one.
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
    /// Grouped by `purpose` first, so a routing model's own spend (`purpose
    /// = "classification"` today) never gets folded into anyone else's
    /// total; and, within the `NULL`-purpose rows every other producer
    /// leaves, split again by whether a harness was recorded, because that
    /// is what actually separates coding-agent consumption
    /// (`crate::gateway::session` always names a harness) from every other
    /// `NULL`-purpose producer (`crate::memory::extract` never does) — a
    /// distinction `purpose` alone cannot make. See [`PurposeConsumption`]'s
    /// own doc comment for why grouping on `purpose` alone would still fold
    /// two different producers together.
    ///
    /// `SUM(input_tokens)`, and its two siblings, are what SQLite's own
    /// aggregate already does correctly: it skips `NULL` inputs and answers
    /// `NULL` only when a group carried none at all, never `0` for an absent
    /// count. The row reader reads that straight into the `Option<i64>`
    /// [`PurposeConsumption`] declares, with no manual accumulate-and-default
    /// in between for a mutation to weaken.
    ///
    /// `first_byte_sample_count` is a genuine `COUNT(first_byte_at)`, so it
    /// is honestly `0` — not absent — for a group nothing timed, and
    /// `first_byte_ms_sample_count` is the same count over migration 25's
    /// measured offset. `mean_time_to_first_byte_ms` **prefers the offset**:
    /// each row contributes its own `first_byte_ms` when it has one and its
    /// `first_byte_at - dispatched_at` difference in milliseconds when it
    /// does not, so a window spanning the migration produces one mean over
    /// every timed row rather than two incomparable ones. It is `NULL`
    /// (`None`) exactly when no row offered either — SQLite's `AVG` over an
    /// empty set is already `NULL`, so there is no manual zero-guard here.
    /// `first_token_*`/`first_tool_call_*` are the identical triple.
    ///
    /// `decode_output_tokens` and `decode_ms` are line 1349's matched pair,
    /// summed over exactly the rows carrying `output_tokens`,
    /// `first_token_ms` and `completed_ms` with a non-negative gap — the one
    /// figure here with **no** seconds fallback, because at one-second
    /// resolution its denominator is routinely `0`. See
    /// [`PurposeConsumption::decode_tokens_per_second`].
    ///
    /// Scoped to this ledger's own `project_id`, like [`Self::observed_identities`]
    /// next door and for the same belt-and-suspenders reason: this reads
    /// across every row in the table rather than one already-named identity.
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

    /// [`TranslationSavings`] for every `(route, quota_context)` this ledger
    /// holds at least one translated row for, within one window — map line
    /// 2034's translation facet, and [`consumption_by_purpose`]'s sibling
    /// query rather than a filter over its output: that reader groups by
    /// `purpose` first and folds every route together, which is exactly the
    /// per-route/per-credential breakdown this line asks for and that one
    /// does not give.
    ///
    /// `purpose = HARNESS_TURN_PURPOSE` restricts to the gateway's own rows
    /// (map line 1330's stamp), and `input_tokens IS NOT NULL` is what
    /// separates a translated exchange from a relayed one **by
    /// construction**: `crate::gateway::session`'s doc comment (near line
    /// 485) and this module's own header (near line 89) both say a relayed
    /// exchange leaves all three token columns `NULL`, so a row that cleared
    /// this filter parsed a real reply body. A relayed row is never in this
    /// reader's denominator, which is the whole point of filtering in SQL
    /// rather than summing in Rust and hoping every caller remembers the
    /// same guard.
    ///
    /// [`consumption_by_purpose`]: Self::consumption_by_purpose
    pub fn translation_cache_savings(
        &self,
        now_unix: i64,
        window_seconds: i64,
    ) -> Result<Vec<TranslationSavings>, EvidenceLedgerError> {
        let earliest = now_unix.saturating_sub(window_seconds);
        let conn = self.lock();
        let mut statement = conn
            .prepare(
                "SELECT route,
                        quota_context,
                        COUNT(*) AS sample_count,
                        SUM(input_tokens) AS input_tokens,
                        SUM(COALESCE(cached_input_tokens, 0)) AS cached_input_tokens
                 FROM routing_observations
                 WHERE project_id = ?1 AND observed_at >= ?2 AND observed_at <= ?3
                   AND purpose = ?4 AND input_tokens IS NOT NULL
                 GROUP BY route, quota_context
                 ORDER BY route IS NULL, route ASC, quota_context IS NULL, quota_context ASC",
            )
            .map_err(sql_err("read translation cache savings"))?;
        let rows = statement
            .query_map(
                params![self.project_id, earliest, now_unix, HARNESS_TURN_PURPOSE],
                row_to_translation_savings,
            )
            .map_err(sql_err("read translation cache savings"))?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row.map_err(sql_err("read one route's translation cache savings"))?);
        }
        Ok(out)
    }

    /// [`Self::translation_cache_savings`] grouped by migration 24's
    /// `session_id` instead of by route and credential — capability map line
    /// 2019's per-session clause.
    ///
    /// Deliberately a second query rather than a second grouping column on
    /// the first: the two readings answer different questions (*which
    /// credential's traffic is cache-warm* and *which session's is*), a row
    /// belongs to exactly one group in each, and folding them into one
    /// `GROUP BY route, quota_context, session_id` would give a reader
    /// neither total without re-summing in Rust — which is the thing the
    /// existing reader's own doc comment says it filters in SQL to avoid.
    ///
    /// `session_id IS NULL` is a group, not an exclusion: see
    /// [`SessionTranslationSavings::session_id`]. Ordered with that group
    /// last, so a report's named sessions read first.
    pub fn session_translation_cache_savings(
        &self,
        now_unix: i64,
        window_seconds: i64,
    ) -> Result<Vec<SessionTranslationSavings>, EvidenceLedgerError> {
        let earliest = now_unix.saturating_sub(window_seconds);
        let conn = self.lock();
        let mut statement = conn
            .prepare(
                "SELECT session_id,
                        COUNT(*) AS sample_count,
                        SUM(input_tokens) AS input_tokens,
                        SUM(COALESCE(cached_input_tokens, 0)) AS cached_input_tokens
                 FROM routing_observations
                 WHERE project_id = ?1 AND observed_at >= ?2 AND observed_at <= ?3
                   AND purpose = ?4 AND input_tokens IS NOT NULL
                 GROUP BY session_id
                 ORDER BY session_id IS NULL, session_id ASC",
            )
            .map_err(sql_err("read per-session translation cache savings"))?;
        let rows = statement
            .query_map(
                params![self.project_id, earliest, now_unix, HARNESS_TURN_PURPOSE],
                row_to_session_translation_savings,
            )
            .map_err(sql_err("read per-session translation cache savings"))?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row.map_err(sql_err("read one session's translation cache savings"))?);
        }
        Ok(out)
    }

    /// [`Self::session_translation_cache_savings`] narrowed to one session
    /// and with no time window — capability map line 1760's evidence half:
    /// `sessions show <id> --debug` reads this to show what providers
    /// actually reported on this session's own translated exchanges, beside
    /// the router's own `prompt-cache state` estimate at launch
    /// ([`crate::routing::session::prompt_cache_state`]).
    ///
    /// A whole-session reading rather than a windowed one, deliberately:
    /// unlike [`Self::session_translation_cache_savings`]'s report over
    /// *recent* activity, one session's own exchanges are a bounded set
    /// already, and windowing them by recency would silently drop a
    /// session's earliest turns from its own evidence.
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

    /// [`ClassificationRecord`] for one `(provider, model)` over the last
    /// `window_seconds` — the reader for capability map lines 1422/1432 and
    /// 1421/1435, and the one that makes those quantities *measured* for
    /// `crate::routing::disposable`'s classification filters.
    ///
    /// Reads only rows whose `purpose` is [`CLASSIFICATION_PURPOSE`]: a
    /// model's gateway exchanges or extraction calls say nothing about how
    /// it behaves as a classifier, and folding them in would let a model
    /// that relays fine but never returns the schema look reliable.
    ///
    /// Scoped to this ledger's own `project_id`, like every read here that
    /// is not already keyed by a full identity.
    pub fn classification_record(
        &self,
        provider: &str,
        model: &str,
        now_unix: i64,
        window_seconds: i64,
    ) -> Result<ClassificationRecord, EvidenceLedgerError> {
        let earliest = now_unix.saturating_sub(window_seconds);
        let observations = {
            let conn = self.lock();
            let mut statement = conn
                .prepare(
                    "SELECT * FROM routing_observations
                     WHERE project_id = ?1 AND provider = ?2 AND model = ?3
                       AND purpose = ?4
                       AND observed_at >= ?5 AND observed_at <= ?6
                     ORDER BY observed_at ASC",
                )
                .map_err(sql_err("read classification observations"))?;
            let rows = statement
                .query_map(
                    params![
                        self.project_id,
                        provider,
                        model,
                        CLASSIFICATION_PURPOSE,
                        earliest,
                        now_unix
                    ],
                    row_to_observation,
                )
                .map_err(sql_err("read classification observations"))?;
            let mut observations = Vec::new();
            for row in rows {
                observations.push(row.map_err(sql_err("read a classification observation"))??);
            }
            observations
        };

        let outcomes_recorded = observations
            .iter()
            .filter(|o| matches!(o.outcome, Some(Outcome::Succeeded) | Some(Outcome::Failed)))
            .count();
        let parsed = observations
            .iter()
            .filter(|o| o.outcome == Some(Outcome::Succeeded))
            .count();
        let durations: Vec<i64> = observations
            .iter()
            .filter_map(RoutingObservation::duration_ms)
            .collect();
        let timed = durations.len();
        let median_duration_ms = (timed >= MIN_SAMPLE_FOR_SUMMARY).then(|| median(durations));

        Ok(ClassificationRecord {
            provider: provider.to_owned(),
            model: model.to_owned(),
            outcomes_recorded,
            parsed,
            timed,
            median_duration_ms,
        })
    }

    /// [`LatencyRecord`] for one `(provider, model)` over the last
    /// `window_seconds` — the reader for capability map line 1539, and
    /// [`Self::classification_record`]'s sibling over [`EXTRACTION_PURPOSE`]
    /// rows instead of [`CLASSIFICATION_PURPOSE`] ones: a support-work call's
    /// own latency says nothing about how it behaves as a classifier, and
    /// folding the two together would let a slow classifier's rows inflate a
    /// fast support-work resource's median or the reverse.
    ///
    /// Scoped to this ledger's own `project_id`, like every read here that
    /// is not already keyed by a full identity.
    pub fn support_work_latency(
        &self,
        provider: &str,
        model: &str,
        now_unix: i64,
        window_seconds: i64,
    ) -> Result<LatencyRecord, EvidenceLedgerError> {
        let earliest = now_unix.saturating_sub(window_seconds);
        let observations = {
            let conn = self.lock();
            let mut statement = conn
                .prepare(
                    "SELECT * FROM routing_observations
                     WHERE project_id = ?1 AND provider = ?2 AND model = ?3
                       AND purpose = ?4
                       AND observed_at >= ?5 AND observed_at <= ?6
                     ORDER BY observed_at ASC",
                )
                .map_err(sql_err("read support-work latency observations"))?;
            let rows = statement
                .query_map(
                    params![
                        self.project_id,
                        provider,
                        model,
                        EXTRACTION_PURPOSE,
                        earliest,
                        now_unix
                    ],
                    row_to_observation,
                )
                .map_err(sql_err("read support-work latency observations"))?;
            let mut observations = Vec::new();
            for row in rows {
                observations
                    .push(row.map_err(sql_err("read a support-work latency observation"))??);
            }
            observations
        };

        let durations: Vec<i64> = observations
            .iter()
            .filter_map(RoutingObservation::duration_ms)
            .collect();
        let timed = durations.len();
        let median_duration_ms = (timed >= MIN_SAMPLE_FOR_SUMMARY).then(|| median(durations));

        Ok(LatencyRecord {
            timed,
            median_duration_ms,
        })
    }
}
