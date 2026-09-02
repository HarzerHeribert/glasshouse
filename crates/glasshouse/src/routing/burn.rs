//! Phase 32E — burn rate and exhaustion forecasting: what the evidence
//! ledger's own rows say about how fast a constrained resource is being
//! spent, and whether it will reach its next reset.
//!
//! Capability map lines 1274 and 1276–1283.
//!
//! # What this module decides, and what it deliberately reuses
//!
//! Four readings, each a public function so a mutation can zero exactly one
//! of them (the same shape `super::pressure`'s two terms take, and for the
//! same reason):
//!
//! - [`task_class_request_rates`] — line 1276. A short moving average of
//!   requests consumed per task class, over the rows migration 23 made able
//!   to carry one.
//! - [`burn_rate`] — line 1277. Requests per hour against one resource,
//!   keyed the way [`super::evidence::recent_credential_throttles`] keys a
//!   credential: provider, narrowed by
//!   [`super::evidence::RoutingObservation::quota_context`] when the caller
//!   names one.
//! - [`forecast`] — lines 1278 and 1279. Time-to-exhaustion, and whether
//!   that lands before the resource's own reset.
//! - [`live_rows`] — line 1282. Which rows the three above are allowed to
//!   see at all.
//!
//! Everything else is *read*, not re-decided: the remaining amount is
//! `crate::provider::quota::Capacity<NativeAmount>` exactly as the provider
//! stated it, and the reset is
//! `crate::provider::quota::CapacityState::seconds_until_reset` computed
//! against the caller's clock.
//!
//! # Purity
//!
//! No clock, no store, no socket — `super::pressure`'s discipline, restated
//! because this module is the one most tempted to break it. Every function
//! here takes rows and a `now_unix` the caller read, and returns a value.
//! Nothing opens a ledger, and nothing can widen the `project_id` scope
//! `EvidenceLedger::consumption_in_window` already applied: this module
//! never sees a connection.
//!
//! # Nothing here parses a response body
//!
//! A **request** rate is the unit throughout, because a completed request
//! produces a row whether or not anything measured its tokens. A token rate
//! is offered only from rows whose `input_tokens`/`output_tokens` are
//! already `Some` — written by a *translated* gateway exchange, which parsed
//! its own response for its own reasons — and is [`None`] otherwise. Line
//! 1275, token consumption per task class, is now served the same way: since
//! `GH-TASK-CLASS-COST-JOIN` every served row of a classified launch carries
//! its `task_class`, and since Phase 56 a translated exchange carries its
//! token counts, so [`task_class_request_rates`] can read a token rate over
//! rows that exist. `crate::gateway::ingress` remains structurally unable to
//! carry a token count, and a relayed row stays uncounted for exactly that
//! reason — this module still never invents one from a ratio.
//!
//! # A forecast that is not known is absent, never a number
//!
//! Every function returns `None` rather than a figure when its inputs are
//! insufficiently known — too few rows, a remaining amount that is a
//! percentage rather than a count, a unit that is not requests, a burn rate
//! of zero. This is the same stance `super::pressure` takes for an unread
//! resource: neither preferred nor withheld. A `None` here makes
//! `super::pressure::exhaustion_forecast_pressure` inert and makes
//! `crate::shell`'s capacity line print exactly what it printed before this
//! module existed.

use super::evidence::{HARNESS_TURN_PURPOSE, MIN_SAMPLE_FOR_SUMMARY, RoutingObservation};
use super::request::TaskClass;
use crate::provider::quota::{Capacity, NativeAmount, UnitScale};

// ---------------------------------------------------------------------------
// The constants every reading is measured against.
// ---------------------------------------------------------------------------

/// Line 1277's floor: how many rows a burn rate must rest on before it is
/// stated at all.
///
/// Eight, and the reasoning is the shape of the estimator rather than a
/// round number. The rate is a [`median`] over [`BUCKET_SECONDS`] buckets;
/// a median over fewer than a handful of buckets is the middle of a very
/// short list and moves as much as a mean would, which is the failure line
/// 1281 names. It
/// is deliberately a count of **rows** and not of buckets, because a caller
/// can see how many rows it passed in and cannot easily see how they fell.
///
/// The same figure as [`super::evidence::MIN_SAMPLE_FOR_SUMMARY`] would be a
/// coincidence, not a shared definition: that constant guards a latency
/// summary. This one is not derived from it.
pub const MIN_ROWS_FOR_BURN_RATE: usize = 8;

/// Line 1282's idle gap. Consecutive rows further apart than this end the
/// window: only rows *after* the last such gap are live.
///
/// Six hours. Long enough that an ordinary night, a lunch, or a meeting does
/// not throw away a working day's evidence; short enough that a burst from
/// yesterday afternoon cannot forecast this morning. The line's own words
/// are *"long idle periods"*, and the thing being protected against is
/// concrete: a rate computed over a window that is mostly silence reports a
/// resource as comfortable while it is being spent hard right now.
pub const IDLE_GAP_SECONDS: i64 = 6 * 60 * 60;

/// The bucket a rate is counted in before [`median`] is taken across
/// buckets — line 1281's *"robust rolling statistic"* needs something to be
/// rolling over.
///
/// Five minutes. Short enough that a burst occupies one or two buckets and
/// is outvoted by the quiet ones around it; long enough that an ordinary
/// interactive rate puts more than one row in a bucket, so the median is not
/// simply a list of ones and zeroes.
pub const BUCKET_SECONDS: i64 = 5 * 60;

/// The unit a remaining amount must be stated in for [`forecast`] to divide
/// it by a request rate.
///
/// A remaining amount in `"tokens"` divided by requests per hour is not a
/// time; `NativeAmount::commensurable_with` already refuses that mixture for
/// a percentage, and this refuses it for a forecast. The provider's own word
/// is what is compared, both spellings that have been observed, and nothing
/// is converted.
pub const REQUEST_UNITS: [&str; 2] = ["requests", "request"];

/// One hour in seconds — the denominator every rate here is stated in, named
/// so no call site writes `3600.0` and means something else by it.
pub const SECONDS_PER_HOUR: f64 = 3600.0;

// ---------------------------------------------------------------------------
// Which rows a reading may see — line 1282.
// ---------------------------------------------------------------------------

/// Line 1282: the rows that are still evidence about *now*.
///
/// Two exclusions, and each one has a defect it prevents:
///
/// 1. **Before a reset boundary this build can actually locate.** Rows spent
///    against a quota that has since been given back would forecast the
///    exhaustion of capacity that no longer applies. But the *only* reset
///    fact any caller here has is `seconds_until_reset`, and nothing in
///    `crate::provider::quota` publishes a window **length** — so the
///    previous turn cannot be derived from the next one without inventing a
///    period nobody stated, which is exactly the fabrication this module
///    refuses everywhere else.
///
///    So one boundary is located and one only: a **non-positive**
///    `seconds_until_reset`, which `CapacityState::seconds_until_reset`
///    returns as-is rather than clamping, means the window turned
///    `-seconds` ago and that instant *is* the boundary. A positive reset
///    excludes nothing on this ground, and rows are then bounded only by
///    the caller's own window and by the idle gap below. This is the
///    conservative direction: it can keep a row it might have dropped, and
///    it can never drop a row that is still evidence.
/// 2. **Before an idle gap longer than [`IDLE_GAP_SECONDS`].** Rows are
///    ordered by `observed_at` ascending (the ordering
///    `EvidenceLedger::consumption_in_window` guarantees); the last gap
///    wider than the constant is a boundary, and only rows after it are
///    live.
///
/// The result borrows: no row is copied, and a caller that wants the count
/// of what was excluded can compare lengths.
pub fn live_rows(
    rows: &[RoutingObservation],
    now_unix: i64,
    seconds_until_reset: Option<i64>,
) -> Vec<&RoutingObservation> {
    let after_reset: Vec<&RoutingObservation> =
        match last_reset_boundary(now_unix, seconds_until_reset) {
            Some(boundary) => rows
                .iter()
                .filter(|row| row.observed_at_unix >= boundary)
                .collect(),
            None => rows.iter().collect(),
        };

    // The last idle gap wins: an hour of work after eight hours of silence
    // is described by the hour.
    let mut start = 0usize;
    for index in 1..after_reset.len() {
        let gap = after_reset[index]
            .observed_at_unix
            .saturating_sub(after_reset[index - 1].observed_at_unix);
        if gap > IDLE_GAP_SECONDS {
            start = index;
        }
    }
    after_reset[start..].to_vec()
}

/// When the resource's window last turned, if that can be derived.
///
/// `None` when no reset is known at all — nothing to exclude, and this
/// function will not invent a boundary from a window length nobody stated.
/// A reset already in the past (a negative `seconds_until_reset`, which
/// `CapacityState::seconds_until_reset` returns as-is rather than clamping)
/// is itself the boundary: the window turned, and everything before it is
/// spent against the old quota.
fn last_reset_boundary(now_unix: i64, seconds_until_reset: Option<i64>) -> Option<i64> {
    let seconds = seconds_until_reset?;
    if seconds <= 0 {
        return Some(now_unix.saturating_add(seconds));
    }
    None
}

// ---------------------------------------------------------------------------
// The robust statistic — line 1281.
// ---------------------------------------------------------------------------

/// How many rows fell in each [`BUCKET_SECONDS`] bucket of the span the rows
/// cover, oldest bucket first.
///
/// Empty buckets are counted as zero and kept: a quiet five minutes is
/// evidence about the rate, and dropping it would make a bursty resource
/// look uniformly busy — the same defect from the other side that line 1281
/// names.
fn bucket_counts(rows: &[&RoutingObservation], now_unix: i64) -> Vec<f64> {
    let Some(first) = rows.first() else {
        return Vec::new();
    };
    let start = first.observed_at_unix;
    let end = now_unix.max(rows.last().map_or(start, |row| row.observed_at_unix));
    let span = end.saturating_sub(start).max(1);
    let bucket_count = (span / BUCKET_SECONDS + 1) as usize;
    let mut counts = vec![0f64; bucket_count];
    for row in rows {
        let offset = row.observed_at_unix.saturating_sub(start).max(0);
        let index = ((offset / BUCKET_SECONDS) as usize).min(bucket_count - 1);
        counts[index] += 1.0;
    }
    counts
}

/// The median of a non-empty list — line 1281's robust statistic.
///
/// **A median and not a mean, and that is the whole point of the line.** A
/// mean over bucket counts is moved by a single busy bucket in proportion to
/// how busy it was: one bucket with fifty requests raises a mean over ten
/// buckets by five requests per bucket, and the forecast built on it reports
/// a resource as exhausting five times sooner than the steady rate warrants.
/// A median moves by at most one position no matter how large that bucket
/// is. An even-length list averages the two middle values, which is the
/// ordinary definition and is still bounded by its neighbours.
///
/// `None` for an empty list — never a zero, which a caller could not tell
/// from a genuinely idle resource.
pub fn median(values: &[f64]) -> Option<f64> {
    if values.is_empty() {
        return None;
    }
    let mut sorted = values.to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).expect("a bucket count is never NaN"));
    let middle = sorted.len() / 2;
    if sorted.len() % 2 == 1 {
        Some(sorted[middle])
    } else {
        Some((sorted[middle - 1] + sorted[middle]) / 2.0)
    }
}

/// The median bucket count expressed as a rate per hour.
fn median_rate_per_hour(rows: &[&RoutingObservation], now_unix: i64) -> Option<f64> {
    let counts = bucket_counts(rows, now_unix);
    let median = median(&counts)?;
    Some(median * SECONDS_PER_HOUR / BUCKET_SECONDS as f64)
}

// ---------------------------------------------------------------------------
// Line 1276 — requests per task class.
// ---------------------------------------------------------------------------

/// One task class's share of the recent request rate — [`task_class_request_rates`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ClassRate {
    /// Which class. Rows carrying no class at all are in no `ClassRate`:
    /// see [`super::evidence::NewObservation::with_task_class`].
    pub class: TaskClass,
    /// The robust rolling estimate, requests per hour.
    pub requests_per_hour: f64,
    /// How many rows this class contributed. A caller showing the figure
    /// should show this too — a rate over three rows is a different claim
    /// from a rate over three hundred.
    pub rows: usize,
    /// Line 1275: tokens per hour for this class, from
    /// `token_rate_per_hour` over the same live, per-class rows — `None`
    /// when none of them carries a token count, the same convention
    /// [`BurnRate::tokens_per_hour`] keeps for one resource.
    pub tokens_per_hour: Option<f64>,
    /// How many of this class's rows carried a token count — never gated by
    /// [`MIN_ROWS_FOR_BURN_RATE`] here, so a caller can apply that floor to
    /// the token figure independently of the request figure.
    pub token_rows: usize,
}

/// Line 1276: a short moving average of requests consumed per task class.
///
/// One entry per class that has at least one live row, in
/// [`TaskClass::ALL`]'s declaration order so two calls over the same rows
/// render identically. Classes with no rows are **absent** rather than
/// present with a zero: "nothing of this kind was routed" and "this kind was
/// routed at a rate of zero" are the same number and different facts, and
/// only the first is true.
///
/// The average is a [`median`] of per-[`BUCKET_SECONDS`] counts — line
/// 1281's robust statistic, not an arithmetic mean over raw per-request
/// sizes.
///
/// Line 1275 rides along on the same per-class rows: each `ClassRate` also
/// carries a token rate, `None` when none of the class's rows carries a
/// count — see `token_rate_per_hour`.
pub fn task_class_request_rates(
    rows: &[RoutingObservation],
    now_unix: i64,
    seconds_until_reset: Option<i64>,
) -> Vec<ClassRate> {
    let live = live_rows(rows, now_unix, seconds_until_reset);
    TaskClass::ALL
        .into_iter()
        .filter_map(|class| {
            let of_class: Vec<&RoutingObservation> = live
                .iter()
                .copied()
                .filter(|row| row.task_class == Some(class))
                .collect();
            if of_class.is_empty() {
                return None;
            }
            let requests_per_hour = median_rate_per_hour(&of_class, now_unix)?;
            let (tokens_per_hour, token_rows) = token_rate_per_hour(&of_class, now_unix);
            Some(ClassRate {
                class,
                requests_per_hour,
                rows: of_class.len(),
                tokens_per_hour,
                token_rows,
            })
        })
        .collect()
}

/// The token half of [`burn_rate`] and [`task_class_request_rates`], in one
/// place so the two readings cannot drift: the sum of `input_tokens` plus
/// `output_tokens` over the rows that carry either, divided by the span
/// those rows cover — never bucketed or medianed, because a token total is
/// not a per-request count that a single burst could distort the way line
/// 1281 is about.
///
/// `(None, 0)` when no row in `rows` carries a token count. The count
/// returned alongside is always the number of rows the sum rests on, whether
/// or not that clears any floor a caller applies — [`burn_rate`] does not
/// gate on it, and [`task_class_request_rates`]'s caller in
/// `crate::shell::build_project_overview_capacity` gates the per-class token
/// figure on it the same way it already gates the per-class request figure.
fn token_rate_per_hour(rows: &[&RoutingObservation], now_unix: i64) -> (Option<f64>, usize) {
    let measured: Vec<&RoutingObservation> = rows
        .iter()
        .copied()
        .filter(|row| row.input_tokens.is_some() || row.output_tokens.is_some())
        .collect();
    if measured.is_empty() {
        return (None, 0);
    }
    let span = span_seconds(&measured, now_unix);
    let total: i64 = measured
        .iter()
        .map(|row| row.input_tokens.unwrap_or(0) + row.output_tokens.unwrap_or(0))
        .sum();
    (
        Some(total as f64 * SECONDS_PER_HOUR / span as f64),
        measured.len(),
    )
}

// ---------------------------------------------------------------------------
// Map line 1301 (`GH-TASK-CLASS-COST-JOIN`) — output tokens per task class.
// ---------------------------------------------------------------------------

/// One task class's recent output-token size, for
/// [`output_tokens_by_class`] — the sibling of [`ClassRate`], read by
/// `super::session::expected_marginal_cost` instead of by
/// `super::pressure`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ClassOutput {
    /// Which class. A class with no row in the window at all is in no
    /// `ClassOutput` — see [`output_tokens_by_class`]'s own doc.
    pub class: TaskClass,
    /// How many rows this class contributed, whether or not that clears the
    /// floor. A caller naming an unmeasured size should still say how many
    /// rows fell short of it, never a silent absence.
    pub samples: usize,
    /// The median output-token count over this class's rows, or `None` when
    /// `samples` is below [`MIN_SAMPLE_FOR_SUMMARY`] — a size withheld,
    /// never estimated from too few rows to trust.
    pub median_output_tokens: Option<f64>,
}

/// Map line 1301: the output-token half of the join this phase's census
/// named missing — `docs/product/evidence/phase-32g.md`'s Censused
/// 2026-09-02 entry. One entry per class with at least one row in the
/// window that names both a class and an output-token count, in
/// [`TaskClass::ALL`]'s declaration order; a class with no such row at all
/// is **absent**, the same convention [`task_class_request_rates`] keeps for
/// its own rate.
///
/// Restricted to `purpose = `[`HARNESS_TURN_PURPOSE`] rows: this is the
/// gateway's own served-exchange traffic, the same rows
/// [`super::evidence::NewObservation::with_task_class`]'s own doc names as
/// what this reader counts — never `record_routing_latency`'s
/// routing-decision row, which carries a class but no tokens and would only
/// ever contribute nothing here.
///
/// The window is `[now_unix - window_seconds, now_unix]`, read off each
/// row's own `observed_at_unix` — a plain calendar window rather than
/// [`live_rows`]'s reset-and-idle-gap boundary, because this reader has no
/// resource reset to bound against and a caller here passes rows straight
/// from [`super::evidence::EvidenceLedger::consumption_in_window`] with the
/// same window already applied at the SQL layer; the second check here is
/// what lets this function also be exercised directly, over a hand-built
/// row list, without a ledger in the loop at all.
pub fn output_tokens_by_class(
    rows: &[RoutingObservation],
    now_unix: i64,
    window_seconds: i64,
) -> Vec<ClassOutput> {
    let earliest = now_unix.saturating_sub(window_seconds);
    TaskClass::ALL
        .into_iter()
        .filter_map(|class| {
            let sizes: Vec<f64> = rows
                .iter()
                .filter(|row| {
                    row.purpose.as_deref() == Some(HARNESS_TURN_PURPOSE)
                        && row.task_class == Some(class)
                        && row.observed_at_unix >= earliest
                        && row.observed_at_unix <= now_unix
                })
                .filter_map(|row| row.output_tokens)
                .map(|tokens| tokens as f64)
                .collect();
            if sizes.is_empty() {
                return None;
            }
            let samples = sizes.len();
            let median_output_tokens = (samples >= MIN_SAMPLE_FOR_SUMMARY)
                .then(|| median(&sizes))
                .flatten();
            Some(ClassOutput {
                class,
                samples,
                median_output_tokens,
            })
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Line 1277 — the burn rate of one resource.
// ---------------------------------------------------------------------------

/// Which resource a burn rate is about — the identity
/// [`super::evidence::recent_credential_throttles`] already uses, named so a
/// call site cannot transpose the two strings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResourceKey<'a> {
    /// `RoutingObservation::provider`, matched exactly.
    pub provider: &'a str,
    /// `RoutingObservation::quota_context` — the
    /// `super::CredentialId::label` shape the gateway stamps on each row.
    /// `None` counts every row of the provider, which is the honest answer
    /// for a resource with no credential of its own; `Some` narrows to that
    /// account **only when every candidate row names an account**, exactly
    /// as `recent_credential_throttles` decides it, so a partially-attributed
    /// history reports the provider-wide rate rather than a fraction of it.
    pub quota_context: Option<&'a str>,
}

/// How fast one resource is being spent — [`burn_rate`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BurnRate {
    /// Requests per hour, the robust rolling estimate.
    pub requests_per_hour: f64,
    /// Tokens per hour, **only** from rows that already carry a token count.
    /// `None` when no live row does, which is the overwhelming majority of
    /// windows in this build: a relayed exchange writes NULLs, and this
    /// module parses nothing to fill them. See the module header.
    pub tokens_per_hour: Option<f64>,
    /// How many live rows the estimate rests on.
    pub rows: usize,
    /// Whether `rows` are this account's own rather than the provider-wide
    /// set — the same fact `CredentialThrottles::account_narrowed` reports,
    /// and for the same reason: a figure attributed to an account nobody can
    /// prove it belongs to is a wrong number wearing a right label.
    pub account_narrowed: bool,
}

/// Line 1277: the current burn rate of the resource `key` names.
///
/// `None` when fewer than [`MIN_ROWS_FOR_BURN_RATE`] live rows match — not a
/// zero, which a caller could not tell from an idle resource, and not a
/// figure over three rows, which is the overreaction line 1281 forbids from
/// the other direction.
pub fn burn_rate(
    rows: &[RoutingObservation],
    key: ResourceKey<'_>,
    now_unix: i64,
    seconds_until_reset: Option<i64>,
) -> Option<BurnRate> {
    let live = live_rows(rows, now_unix, seconds_until_reset);
    let of_provider: Vec<&RoutingObservation> = live
        .into_iter()
        .filter(|row| row.provider == key.provider)
        .collect();

    let every_row_names_its_account =
        !of_provider.is_empty() && of_provider.iter().all(|row| row.quota_context.is_some());
    let (matched, account_narrowed) = match key.quota_context {
        Some(label) if every_row_names_its_account => (
            of_provider
                .iter()
                .copied()
                .filter(|row| row.quota_context.as_deref() == Some(label))
                .collect::<Vec<&RoutingObservation>>(),
            true,
        ),
        _ => (of_provider, false),
    };

    if matched.len() < MIN_ROWS_FOR_BURN_RATE {
        return None;
    }
    let requests_per_hour = median_rate_per_hour(&matched, now_unix)?;

    // Tokens, only where a row already carried them. A row with neither
    // field contributes nothing; a row with one contributes that one.
    let (tokens_per_hour, _) = token_rate_per_hour(&matched, now_unix);

    Some(BurnRate {
        requests_per_hour,
        tokens_per_hour,
        rows: matched.len(),
        account_narrowed,
    })
}

/// The span the rows cover, at least one second so no division here can be
/// by zero — the guard the packet's cross-platform section asks for, stated
/// once rather than at each call.
fn span_seconds(rows: &[&RoutingObservation], now_unix: i64) -> i64 {
    let Some(first) = rows.first() else {
        return 1;
    };
    let start = first.observed_at_unix;
    let end = now_unix.max(rows.last().map_or(start, |row| row.observed_at_unix));
    end.saturating_sub(start).max(1)
}

// ---------------------------------------------------------------------------
// Lines 1278 and 1279 — time to exhaustion, and the reset.
// ---------------------------------------------------------------------------

/// What is forecast about one resource — [`forecast`].
///
/// `Copy`, because `super::session::Destination` carries one and
/// `super::pressure::PressureInputs` is `Copy`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ExhaustionForecast {
    /// The burn rate the forecast rests on, requests per hour.
    pub requests_per_hour: f64,
    /// Line 1278: how long the measured remaining amount lasts at that rate.
    pub seconds_to_exhaustion: i64,
    /// Line 1279: whether the resource reaches its next reset. `None` when
    /// the reset is not known — never `true`, which would be a promise built
    /// on an absence.
    pub survives_until_reset: Option<bool>,
    /// The reset this was compared against, when there was one.
    pub seconds_until_reset: Option<i64>,
    /// How many live rows the burn rate rests on.
    pub rows: usize,
}

impl ExhaustionForecast {
    /// Line 1280's test: does this exhaust *well before* its reset?
    ///
    /// `false` when there is no reset to be well before — an unknown reset
    /// earns a destination nothing and costs it nothing, `super::pressure`'s
    /// own stance for an unread resource.
    pub fn exhausts_well_before_reset(&self) -> bool {
        let Some(reset) = self.seconds_until_reset else {
            return false;
        };
        if reset <= 0 {
            return false;
        }
        (self.seconds_to_exhaustion as f64) < WELL_BEFORE_RESET_FRACTION * reset as f64
    }
}

/// Line 1280's *"well before"*, as a fraction of the time left until the
/// reset.
///
/// A half. The line asks for a routing preference to drop for a resource
/// that will not make it to its reset — but a forecast that lands a minute
/// short of the reset is inside this estimator's own error bars, and moving
/// work away from a resource on that basis would be the overreaction line
/// 1281 is about, arriving through a different door. Half the remaining
/// window is a margin the median's own tolerance comfortably fits inside: at
/// this threshold the rate would have to be wrong by a factor of two before
/// the resource actually survives, and the penalty is small enough
/// ([`super::pressure::EXHAUSTION_FORECAST_PENALTY`]) that being wrong costs
/// a preference rather than a refusal.
pub const WELL_BEFORE_RESET_FRACTION: f64 = 0.5;

/// Lines 1278 and 1279: when the resource `key` names runs out, and whether
/// that is before its reset.
///
/// `None` — *insufficiently known*, and the words are the line's own — in
/// every one of these cases, none of which is answered with a number:
///
/// - fewer than [`MIN_ROWS_FOR_BURN_RATE`] live rows ([`burn_rate`] says so);
/// - `remaining` is not [`Capacity::Measured`]. A percentage is not a
///   count: `remaining_capacity_score` can say "12% left" for a resource
///   whose native ceiling nobody published, and 12% of an unknown number
///   divided by a rate is not a duration. `Inapplicable`, `ProviderOpaque`,
///   `Unmeasured` and `DelegatedUpstream` are all equally absent here;
/// - the measured amount is not stated in a whole count of requests
///   ([`REQUEST_UNITS`]). Tokens over requests-per-hour is not a time;
/// - the burn rate is zero or negative. Nothing is being spent, so nothing
///   exhausts — and the division that would produce an infinity never runs.
///
/// `survives_until_reset` is `None` whenever `seconds_until_reset` is,
/// which keeps line 1279's own hedge: an unknown reset produces no verdict.
pub fn forecast(
    rows: &[RoutingObservation],
    key: ResourceKey<'_>,
    remaining: &Capacity<NativeAmount>,
    now_unix: i64,
    seconds_until_reset: Option<i64>,
) -> Option<ExhaustionForecast> {
    let rate = burn_rate(rows, key, now_unix, seconds_until_reset)?;
    let amount = measured_requests(remaining)?;
    if rate.requests_per_hour <= 0.0 {
        return None;
    }
    let seconds = (amount as f64 / rate.requests_per_hour) * SECONDS_PER_HOUR;
    if !seconds.is_finite() || seconds < 0.0 {
        return None;
    }
    let seconds_to_exhaustion = seconds as i64;
    let survives_until_reset = seconds_until_reset.map(|reset| seconds_to_exhaustion >= reset);
    Some(ExhaustionForecast {
        requests_per_hour: rate.requests_per_hour,
        seconds_to_exhaustion,
        survives_until_reset,
        seconds_until_reset,
        rows: rate.rows,
    })
}

/// The remaining amount as a whole count of requests, or `None`.
///
/// Three conditions, and each one is a different way of not knowing: it must
/// have been read at all ([`Capacity::Measured`]), it must be a whole count
/// rather than millionths of one ([`UnitScale::Whole`]), and the provider's
/// own word for the unit must be a request ([`REQUEST_UNITS`]). A negative
/// count is refused too — a provider that publishes one has published a
/// deficit, not a remaining amount.
fn measured_requests(remaining: &Capacity<NativeAmount>) -> Option<i64> {
    let amount = remaining.reading()?.value();
    if amount.scale() != UnitScale::Whole {
        return None;
    }
    if !REQUEST_UNITS.contains(&amount.unit()) {
        return None;
    }
    let value = amount.value();
    if value < 0 { None } else { Some(value) }
}

// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::routing::evidence::{ContextState, Outcome};

    const NOW: i64 = 1_800_000_000;

    /// A row carrying only what this module reads. Built by hand rather than
    /// through the ledger because these are unit tests of arithmetic — the
    /// ledger's own round trip is `crate::database`'s migration-23 test.
    fn row(
        provider: &str,
        quota_context: Option<&str>,
        observed_at_unix: i64,
        class: Option<TaskClass>,
    ) -> RoutingObservation {
        RoutingObservation {
            seq: observed_at_unix,
            project_id: "p".to_owned(),
            observed_at_unix,
            provider: provider.to_owned(),
            model: "m".to_owned(),
            route: None,
            quota_context: quota_context.map(str::to_owned),
            harness: None,
            purpose: None,
            dispatched_at_unix: None,
            first_byte_at_unix: None,
            first_token_at_unix: None,
            first_tool_call_at_unix: None,
            completed_at_unix: None,
            input_tokens: None,
            output_tokens: None,
            cached_input_tokens: None,
            cost: None,
            tool_rounds: None,
            retries: None,
            repairs: None,
            failovers: None,
            outcome: Some(Outcome::Succeeded),
            failure_class: None,
            task_class: class,
            // Migration 24's three columns. This module reads none of them;
            // they are here because the struct literal must be complete.
            session_id: None,
            effort_level: None,
            turn_shape: None,
            context_state: ContextState::Unknown,
        }
    }

    /// `count` rows spread one per `spacing` seconds, ending `ends_ago`
    /// seconds before [`NOW`].
    fn steady(
        provider: &str,
        count: usize,
        spacing: i64,
        ends_ago: i64,
        class: Option<TaskClass>,
    ) -> Vec<RoutingObservation> {
        (0..count)
            .map(|i| {
                let at = NOW - ends_ago - (count as i64 - 1 - i as i64) * spacing;
                row(provider, Some("acct-a"), at, class)
            })
            .collect()
    }

    fn measured_requests_amount(value: i64) -> Capacity<NativeAmount> {
        Capacity::Measured(crate::provider::quota::Reading::new(
            NativeAmount::whole(value, "requests"),
            NOW,
            crate::provider::quota::ReadingSource::ProviderEndpoint(
                "https://example/usage".to_owned(),
            ),
        ))
    }

    // --- line 1281: the robust statistic --------------------------------

    /// **Line 1281's killer.** One bucket carrying a burst forty times the
    /// steady rate must not move the estimate the way a mean would.
    ///
    /// The two lists share every steady row and differ only by the burst, so
    /// nothing but the statistic can explain a difference. The mean is
    /// computed here from the same buckets, so the assertion is not against
    /// a hard-coded number that a future constant change would falsify — it
    /// is against the naive statistic itself, which is exactly the thing the
    /// mutation swaps in.
    #[test]
    fn one_outlier_bucket_moves_the_median_far_less_than_it_moves_a_mean() {
        // Twelve five-minute buckets, two requests each.
        let mut steady_rows: Vec<RoutingObservation> = Vec::new();
        for bucket in 0..12i64 {
            for slot in 0..2i64 {
                steady_rows.push(row(
                    "p",
                    Some("acct-a"),
                    NOW - 3600 + bucket * BUCKET_SECONDS + slot * 60,
                    None,
                ));
            }
        }
        let mut bursty = steady_rows.clone();
        // One bucket with eighty extra requests — an outlier by any reading.
        for extra in 0..80i64 {
            bursty.push(row(
                "p",
                Some("acct-a"),
                NOW - 3600 + 5 * BUCKET_SECONDS + extra % 300,
                None,
            ));
        }
        bursty.sort_by_key(|r| r.observed_at_unix);

        let steady_refs: Vec<&RoutingObservation> = steady_rows.iter().collect();
        let bursty_refs: Vec<&RoutingObservation> = bursty.iter().collect();

        let steady_median = median_rate_per_hour(&steady_refs, NOW).unwrap();
        let bursty_median = median_rate_per_hour(&bursty_refs, NOW).unwrap();

        let mean = |rows: &[&RoutingObservation]| {
            let counts = bucket_counts(rows, NOW);
            let sum: f64 = counts.iter().sum();
            sum / counts.len() as f64 * SECONDS_PER_HOUR / BUCKET_SECONDS as f64
        };
        let steady_mean = mean(&steady_refs);
        let bursty_mean = mean(&bursty_refs);

        let median_shift = (bursty_median - steady_median).abs();
        let mean_shift = (bursty_mean - steady_mean).abs();

        assert!(
            median_shift < mean_shift / 4.0,
            "the robust statistic must absorb the outlier: median moved {median_shift:.1} \
             req/h, a mean moved {mean_shift:.1} req/h"
        );
        assert!(
            mean_shift > 50.0,
            "the fixture must contain an outlier a mean would actually notice, moved \
             {mean_shift:.1} req/h"
        );
    }

    /// The median itself, on the two shapes a bucket list takes.
    #[test]
    fn the_median_is_the_middle_and_averages_the_two_middles_when_even() {
        assert_eq!(median(&[]), None);
        assert_eq!(median(&[3.0]), Some(3.0));
        assert_eq!(median(&[9.0, 1.0, 2.0]), Some(2.0));
        assert_eq!(median(&[1.0, 2.0, 3.0, 100.0]), Some(2.5));
    }

    // --- lines 1278 and 1279 --------------------------------------------

    /// **Line 1278's killer.** A percentage without a native count is
    /// *insufficiently known*: no forecast, never a fabricated one.
    ///
    /// Every non-`Measured` state is asserted, because they mean different
    /// things and a reader must not be able to fix one and miss three.
    #[test]
    fn time_to_exhaustion_is_absent_without_a_measured_request_unit_amount() {
        let rows = steady("p", 20, 120, 0, None);
        let key = ResourceKey {
            provider: "p",
            quota_context: None,
        };
        // The rate itself is established — so a `None` below is about the
        // remaining amount and nothing else.
        assert!(burn_rate(&rows, key, NOW, Some(7200)).is_some());

        for absent in [
            Capacity::Inapplicable,
            Capacity::ProviderOpaque,
            Capacity::Unmeasured,
            Capacity::DelegatedUpstream,
        ] {
            assert_eq!(
                forecast(&rows, key, &absent, NOW, Some(7200)),
                None,
                "{} is not a count, so it cannot be divided by a rate",
                absent.as_str()
            );
        }

        // A measured amount in the wrong unit is equally absent: tokens over
        // requests-per-hour is not a time.
        let tokens = Capacity::Measured(crate::provider::quota::Reading::new(
            NativeAmount::whole(500, "tokens"),
            NOW,
            crate::provider::quota::ReadingSource::ProviderEndpoint(
                "https://example/usage".to_owned(),
            ),
        ));
        assert_eq!(forecast(&rows, key, &tokens, NOW, Some(7200)), None);

        // And with a real request count it answers.
        let forecast = forecast(&rows, key, &measured_requests_amount(60), NOW, Some(7200))
            .expect("a measured request count and an established rate is a forecast");
        assert!(forecast.seconds_to_exhaustion > 0);
    }

    /// The arithmetic itself: sixty requests left at thirty an hour is two
    /// hours, and the rate is the one `burn_rate` reported.
    #[test]
    fn time_to_exhaustion_is_the_remaining_count_over_the_rate() {
        // One request every two minutes: 30/hour by the median of buckets
        // holding 2.5 each — spacing is chosen so every bucket holds the
        // same count and the median is unambiguous.
        let rows = steady("p", 24, 120, 0, None);
        let key = ResourceKey {
            provider: "p",
            quota_context: None,
        };
        let rate = burn_rate(&rows, key, NOW, None).unwrap();
        let forecast = forecast(&rows, key, &measured_requests_amount(60), NOW, None).unwrap();
        let expected = (60.0 / rate.requests_per_hour * SECONDS_PER_HOUR) as i64;
        assert_eq!(forecast.seconds_to_exhaustion, expected);
        assert_eq!(forecast.requests_per_hour, rate.requests_per_hour);
    }

    /// **Line 1279's killer.** The verdict is a comparison against the
    /// reset, and it is absent when either side is.
    #[test]
    fn survives_until_reset_compares_against_the_reset_and_is_none_without_one() {
        let rows = steady("p", 24, 120, 0, None);
        let key = ResourceKey {
            provider: "p",
            quota_context: None,
        };
        let remaining = measured_requests_amount(60);

        let unknown_reset = forecast(&rows, key, &remaining, NOW, None).unwrap();
        assert_eq!(
            unknown_reset.survives_until_reset, None,
            "an unknown reset produces no verdict — never a `true` built on an absence"
        );

        let to_exhaustion = unknown_reset.seconds_to_exhaustion;

        // A reset that arrives before exhaustion: it survives.
        let survives = forecast(&rows, key, &remaining, NOW, Some(to_exhaustion - 600)).unwrap();
        assert_eq!(survives.survives_until_reset, Some(true));

        // A reset that arrives after exhaustion: it does not.
        let does_not = forecast(&rows, key, &remaining, NOW, Some(to_exhaustion + 600)).unwrap();
        assert_eq!(does_not.survives_until_reset, Some(false));
    }

    /// Line 1280's own threshold, on the type that carries it.
    #[test]
    fn well_before_is_half_the_window_and_is_false_without_a_reset() {
        let base = ExhaustionForecast {
            requests_per_hour: 10.0,
            seconds_to_exhaustion: 1000,
            survives_until_reset: Some(false),
            seconds_until_reset: Some(3000),
            rows: 20,
        };
        assert!(base.exhausts_well_before_reset(), "1000 < 0.5 * 3000");

        let marginal = ExhaustionForecast {
            seconds_until_reset: Some(1900),
            ..base
        };
        assert!(
            !marginal.exhausts_well_before_reset(),
            "1000 is not well before 1900 — inside the estimator's own tolerance"
        );

        let no_reset = ExhaustionForecast {
            seconds_until_reset: None,
            survives_until_reset: None,
            ..base
        };
        assert!(!no_reset.exhausts_well_before_reset());
    }

    // --- line 1277 ------------------------------------------------------

    /// The floor: too little history is no rate, never a zero and never a
    /// figure over three rows.
    #[test]
    fn a_burn_rate_below_the_minimum_row_count_is_absent() {
        let key = ResourceKey {
            provider: "p",
            quota_context: None,
        };
        let thin = steady("p", MIN_ROWS_FOR_BURN_RATE - 1, 120, 0, None);
        assert_eq!(burn_rate(&thin, key, NOW, None), None);

        let enough = steady("p", MIN_ROWS_FOR_BURN_RATE, 120, 0, None);
        assert!(burn_rate(&enough, key, NOW, None).is_some());
    }

    /// The key is the provider, narrowed by `quota_context` exactly as
    /// `recent_credential_throttles` narrows a credential — including its
    /// refusal to narrow when the history is only partly attributed.
    #[test]
    fn the_burn_rate_is_keyed_by_provider_and_narrowed_by_the_account() {
        // Twenty rows for `p`, eight of them relabelled to a second
        // account, so BOTH the narrowed and the wide set clear
        // `MIN_ROWS_FOR_BURN_RATE` — otherwise a `None` here would be about
        // the floor rather than about the key.
        let mut rows = steady("p", 20, 120, 0, None);
        rows.extend(steady("other", 12, 120, 0, None));
        for row in rows.iter_mut().filter(|r| r.provider == "p").take(8) {
            row.quota_context = Some("acct-b".to_owned());
        }

        let wide = burn_rate(
            &rows,
            ResourceKey {
                provider: "p",
                quota_context: None,
            },
            NOW,
            None,
        )
        .unwrap();
        assert_eq!(wide.rows, 20, "another provider's rows are not this one's");
        assert!(!wide.account_narrowed);

        let narrowed = burn_rate(
            &rows,
            ResourceKey {
                provider: "p",
                quota_context: Some("acct-a"),
            },
            NOW,
            None,
        )
        .unwrap();
        assert_eq!(narrowed.rows, 12);
        assert!(narrowed.account_narrowed);

        // One unattributed row and the narrowing is refused wholesale — a
        // fraction of a partly-labelled history is a wrong number wearing a
        // right label.
        rows[0].quota_context = None;
        let refused = burn_rate(
            &rows,
            ResourceKey {
                provider: "p",
                quota_context: Some("acct-a"),
            },
            NOW,
            None,
        )
        .unwrap();
        assert!(!refused.account_narrowed);
        assert_eq!(refused.rows, 20);
    }

    /// A token rate exists only where rows already carry token counts —
    /// nothing here parses a body, and a window of relayed exchanges has
    /// none.
    #[test]
    fn a_token_rate_exists_only_where_rows_already_carry_tokens() {
        let key = ResourceKey {
            provider: "p",
            quota_context: None,
        };
        let untokened = steady("p", 12, 120, 0, None);
        assert_eq!(
            burn_rate(&untokened, key, NOW, None)
                .unwrap()
                .tokens_per_hour,
            None
        );

        let mut tokened = untokened.clone();
        for row in &mut tokened {
            row.input_tokens = Some(100);
            row.output_tokens = Some(50);
        }
        let rate = burn_rate(&tokened, key, NOW, None).unwrap().tokens_per_hour;
        assert!(rate.is_some_and(|value| value > 0.0));
    }

    // --- line 1282 ------------------------------------------------------

    /// **Line 1282's killer, first half.** Rows before an idle gap longer
    /// than the constant contribute nothing.
    #[test]
    fn rows_before_a_long_idle_gap_are_excluded() {
        // A burst yesterday, silence, then a quiet hour now.
        let mut rows = steady("p", 40, 30, IDLE_GAP_SECONDS + 7200, None);
        rows.extend(steady("p", 12, 300, 0, None));
        rows.sort_by_key(|r| r.observed_at_unix);

        let live = live_rows(&rows, NOW, None);
        assert_eq!(
            live.len(),
            12,
            "only the rows after the gap are still evidence about now"
        );

        // And the rate the caller sees is the recent one, not yesterday's.
        let key = ResourceKey {
            provider: "p",
            quota_context: None,
        };
        let rate = burn_rate(&rows, key, NOW, None).unwrap();
        assert_eq!(rate.rows, 12);
    }

    /// **Line 1282's killer, second half.** Rows from before the resource's
    /// own window turned are spent against a quota that no longer exists.
    #[test]
    fn rows_before_the_last_reset_boundary_are_excluded() {
        // The window turned an hour ago: `seconds_until_reset` is negative,
        // which `CapacityState::seconds_until_reset` returns as-is.
        let turned_ago = -3600;
        let mut rows = steady("p", 10, 120, 7200, None); // before the turn
        rows.extend(steady("p", 12, 120, 0, None)); // after it
        rows.sort_by_key(|r| r.observed_at_unix);

        let live = live_rows(&rows, NOW, Some(turned_ago));
        assert_eq!(
            live.len(),
            12,
            "capacity given back at the reset is not capacity this rate should count"
        );
        // With no reset known, nothing is excluded on that ground.
        assert_eq!(live_rows(&rows, NOW, None).len(), 22);
    }

    // --- line 1276 ------------------------------------------------------

    /// Per-class rates: one entry per class that has rows, in declaration
    /// order, and no entry at all for a class nothing was routed as.
    #[test]
    fn task_class_rates_name_only_the_classes_that_have_rows() {
        let mut rows = steady("p", 12, 120, 0, Some(TaskClass::CodeModification));
        rows.extend(steady("p", 6, 240, 0, Some(TaskClass::Question)));
        // Rows carrying no class at all join no bucket.
        rows.extend(steady("p", 20, 60, 0, None));
        rows.sort_by_key(|r| r.observed_at_unix);

        let rates = task_class_request_rates(&rows, NOW, None);
        let classes: Vec<TaskClass> = rates.iter().map(|rate| rate.class).collect();
        assert_eq!(
            classes,
            vec![TaskClass::Question, TaskClass::CodeModification],
            "declaration order, and only the classes with rows"
        );
        assert_eq!(rates[0].rows, 6);
        assert_eq!(rates[1].rows, 12);
        assert!(rates.iter().all(|rate| rate.requests_per_hour > 0.0));

        assert!(
            task_class_request_rates(&[], NOW, None).is_empty(),
            "no rows is no rates, never five zeroes"
        );
    }

    /// The per-class rate obeys line 1282 too: it is computed over live rows
    /// and not over the whole window.
    #[test]
    fn task_class_rates_see_only_live_rows() {
        let mut rows = steady(
            "p",
            30,
            30,
            IDLE_GAP_SECONDS + 7200,
            Some(TaskClass::ShellWork),
        );
        rows.extend(steady("p", 9, 300, 0, Some(TaskClass::ShellWork)));
        rows.sort_by_key(|r| r.observed_at_unix);

        let rates = task_class_request_rates(&rows, NOW, None);
        assert_eq!(rates.len(), 1);
        assert_eq!(rates[0].rows, 9);
    }

    /// **Line 1275's killer.** A class whose rows all carry token counts
    /// gets a token rate; a class whose rows carry none gets `None` — never
    /// a `0` a caller could mistake for a genuinely idle class.
    #[test]
    fn task_class_rates_carry_a_token_rate_only_where_rows_carry_tokens() {
        let mut tokened = steady("p", 12, 120, 0, Some(TaskClass::CodeModification));
        for row in &mut tokened {
            row.input_tokens = Some(100);
            row.output_tokens = Some(50);
        }
        let mut rows = tokened;
        rows.extend(steady("p", 8, 120, 0, Some(TaskClass::Question)));
        rows.sort_by_key(|r| r.observed_at_unix);

        let rates = task_class_request_rates(&rows, NOW, None);
        let by_class = |class: TaskClass| {
            rates
                .iter()
                .find(|rate| rate.class == class)
                .unwrap_or_else(|| panic!("{class:?} missing from {rates:?}"))
        };

        let tokened_rate = by_class(TaskClass::CodeModification);
        assert!(
            tokened_rate
                .tokens_per_hour
                .is_some_and(|value| value > 0.0),
            "every row of this class carries tokens: {tokened_rate:?}"
        );
        assert_eq!(tokened_rate.token_rows, 12);

        let untokened_rate = by_class(TaskClass::Question);
        assert_eq!(
            untokened_rate.tokens_per_hour, None,
            "no row of this class carries a token count, so the rate is absent, not zero"
        );
        assert_eq!(untokened_rate.token_rows, 0);
    }

    /// Mixed rows within one class: only the token-carrying rows contribute
    /// to that class's token rate, and the class's request rate is
    /// unaffected either way.
    #[test]
    fn task_class_rates_token_axis_counts_only_the_counted_rows_of_its_own_class() {
        let mut rows = steady("p", 12, 120, 0, Some(TaskClass::ShellWork));
        for row in rows.iter_mut().take(5) {
            row.input_tokens = Some(40);
            row.output_tokens = Some(10);
        }

        let rates = task_class_request_rates(&rows, NOW, None);
        assert_eq!(rates.len(), 1);
        let rate = &rates[0];
        assert_eq!(rate.rows, 12, "the request count is unaffected by tokens");
        assert_eq!(rate.token_rows, 5, "only the five rows that carry tokens");
        assert!(rate.tokens_per_hour.is_some_and(|value| value > 0.0));
    }

    /// A zero or negative burn rate is "no forecast", not an infinity and
    /// not a panic — the guard the packet's cross-platform section names.
    #[test]
    fn a_rate_of_zero_produces_no_forecast_rather_than_an_infinity() {
        // Every row far enough in the past that `now` is many empty buckets
        // later: the median bucket is empty, so the rate is zero.
        let rows = steady("p", 12, 60, 4 * 3600, None);
        let key = ResourceKey {
            provider: "p",
            quota_context: None,
        };
        let rate = burn_rate(&rows, key, NOW, None).unwrap();
        assert_eq!(rate.requests_per_hour, 0.0);
        assert_eq!(
            forecast(&rows, key, &measured_requests_amount(60), NOW, None),
            None,
            "nothing is being spent, so nothing exhausts"
        );
    }
}
