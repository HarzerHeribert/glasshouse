//! Phase 32E — burn rate and exhaustion forecasting: what the evidence
//! ledger's own rows say about how fast a constrained resource is being
//! spent, and whether it will reach its next reset. Capability map lines
//! 1274 and 1276–1283.
//!
//! Four public functions, each a mutation's target: [`task_class_request_rates`]
//! (1276), [`burn_rate`] (1277), [`forecast`] (1278–1279), and [`live_rows`]
//! (1282), which gates what the other three may see. Everything else is
//! read, not re-decided.
//!
//! No clock, no store, no socket: every function takes rows and a
//! `now_unix` the caller read, and returns a value.
//!
//! A **request** rate is the unit throughout — a completed request
//! produces a row whether or not anything measured its tokens.
//!
//! Every function returns `None`, never a figure, when its inputs are
//! insufficiently known.
// History: design-decisions.md, "Trims: routing/burn/mod.rs", module doc.

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
/// Two exclusions: rows before a **located** reset boundary, and rows
/// before the last gap wider than [`IDLE_GAP_SECONDS`]. A boundary is
/// located only for a **non-positive** `seconds_until_reset`
/// (`CapacityState::seconds_until_reset` returns it as-is, never clamped);
/// nothing in `crate::provider::quota` publishes a window length, so a
/// positive reset excludes nothing here rather than invent one. This is
/// the conservative direction: it can keep a row it might have dropped,
/// never drop one that is still evidence.
///
/// Rows are ordered by `observed_at` ascending — the ordering
/// `EvidenceLedger::consumption_in_window` guarantees — so the last gap
/// wider than the constant is a boundary, and only rows after it are live.
///
/// The result borrows: no row is copied, and a caller that wants the count
/// of what was excluded can compare lengths.
// History: design-decisions.md, "Trims: routing/burn/mod.rs", `live_rows` doc.
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
/// 2026-09-02 entry. One entry per class with at least one row that names
/// both a class and an output-token count, in [`TaskClass::ALL`]'s
/// declaration order; a class with no such row is **absent**, the same
/// convention [`task_class_request_rates`] keeps.
///
/// Restricted to `purpose = `[`HARNESS_TURN_PURPOSE`] rows — the gateway's
/// own served-exchange traffic — never `record_routing_latency`'s
/// routing-decision row, which carries a class but no tokens.
///
/// The window is `[now_unix - window_seconds, now_unix]`, a plain calendar
/// window rather than [`live_rows`]'s reset-and-idle-gap boundary: this
/// reader has no resource reset to bound against, and a caller passes rows
/// already windowed at the SQL layer by
/// [`super::evidence::EvidenceLedger::consumption_in_window`].
// History: design-decisions.md, "Trims: routing/burn/mod.rs", `output_tokens_by_class` doc.
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
mod tests;
