//! Everything that reads across tables: the subscription-headroom estimate
//! and its replay accounting, the routing-consumption estimate pairs and
//! their accuracy, the effort-shadow classification shim, and the
//! responsiveness/separation measure — all of which join
//! `routing_observations` to `evaluation_observations` for a harness verdict
//! or an estimate row.

use super::*;

use rusqlite::params;

use crate::provider::quota::Confidence;

/// [`EvidenceLedger::effort_shadow`]'s deterministic ordering key for
/// [`TurnShape`]: tool-resume first, since that is the shape a clamp would
/// ever apply to.
fn turn_shape_rank(turn_shape: TurnShape) -> u8 {
    match turn_shape {
        TurnShape::ToolResume => 0,
        TurnShape::Prompt => 1,
    }
}

/// [`EvidenceLedger::effort_shadow`]'s ordering key for
/// `Option<EffortLevel>`: the ladder's own order, with "no effort carried"
/// last rather than first — it is not a rung below [`EffortLevel::Minimal`],
/// it is the absence of the field.
fn effort_level_rank(effort_level: Option<EffortLevel>) -> i8 {
    match effort_level {
        Some(EffortLevel::Minimal) => 0,
        Some(EffortLevel::Low) => 1,
        Some(EffortLevel::Medium) => 2,
        Some(EffortLevel::High) => 3,
        None => 4,
    }
}

/// [`EvidenceLedger::effort_shadow`]'s in-progress accumulator for one
/// `(turn_shape, effort_level)` group, folded from flat rows before becoming
/// an [`EffortShadowRow`] — a named type rather than a tuple so the fold in
/// [`EvidenceLedger::effort_shadow`] reads as fields, not positions.
struct EffortShadowGroup {
    turn_shape: TurnShape,
    effort_level: Option<EffortLevel>,
    output_tokens: Vec<i64>,
    completed: usize,
    failed: usize,
    unverdicted: usize,
}

impl EffortShadowGroup {
    fn new(turn_shape: TurnShape, effort_level: Option<EffortLevel>) -> Self {
        Self {
            turn_shape,
            effort_level,
            output_tokens: Vec::new(),
            completed: 0,
            failed: 0,
            unverdicted: 0,
        }
    }

    fn into_row(self) -> EffortShadowRow {
        let sample_count = self.output_tokens.len();
        let median_output_tokens =
            (sample_count >= MIN_SAMPLE_FOR_SUMMARY).then(|| median(self.output_tokens));
        EffortShadowRow {
            turn_shape: self.turn_shape,
            effort_level: self.effort_level,
            sample_count,
            median_output_tokens,
            completed: self.completed,
            failed: self.failed,
            unverdicted: self.unverdicted,
        }
    }
}

/// [`EvidenceLedger::effort_shadow`]'s verdict-subject vocabulary, spelled
/// once here rather than imported: [`crate::evaluation`]'s own
/// `TURN_COMPLETED`/`TURN_FAILED` constants are private to that module (see
/// its `turn_subject`), and the two agreeing is proven end to end by
/// `tests/effort_shadow.rs`'s launch-and-hook test rather than by a shared
/// symbol.
const EFFORT_SHADOW_VERDICT_COMPLETED: &str = "completed";
const EFFORT_SHADOW_VERDICT_FAILED: &str = "failed";

/// How recent a throttle must be to read as still-live pressure rather than
/// history the window happens to still hold, and how close a reset must sit
/// to count as imminent relief — map line 1245's "recency", one horizon for
/// both questions rather than a second invented number: an hour is the
/// shortest cadence window this project's own throttle producers actually
/// observe (`crate::gateway::session`'s own per-window limiters), so a
/// throttle or a reset outside it says nothing about the account's *current*
/// pressure.
pub const RECENT_SIGNAL_HORIZON_SECONDS: i64 = 3_600;

/// Map line 1249's second horizon — pressure that persists well past the
/// short window rather than a single accident. Three days, not a week or a
/// month: the one production caller queries rows only
/// [`CLASSIFICATION_EVIDENCE_WINDOW_SECONDS`] deep (seven days), and setting
/// this horizon at or past that bound would make
/// [`LongWindowPressure::NoPressure`] structurally unreachable — no query
/// could ever cover it, so the honest answer would always collapse to
/// [`LongWindowPressure::Undistinguished`]. Three days leaves the full
/// window room to actually prove an absence.
pub const LONG_SIGNAL_HORIZON_SECONDS: i64 = 3 * 24 * 3_600;

/// Map line 1248's anecdote guard: fewer than this many observed
/// throttle→success recoveries in window, and no reset window is learned at
/// all. Two is the floor at which a single unlucky pairing — a throttle
/// immediately followed, by coincidence, by an unrelated success — cannot be
/// the whole story behind the learned value.
pub const MIN_LEARNED_RESET_RECOVERIES: usize = 2;

/// Map line 1248 — whose reset reading, if any, informed this estimate's
/// "is a reset imminent" term. Kept off [`HeadroomBand`] itself (1250/1251's
/// own rule: no numeric field, no invented precision) and reported here
/// instead, so a consumer can label an inferred reading as what it is rather
/// than letting it render identically to the provider's own stated word.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResetBasis {
    /// No reset behaviour — stated or inferred — entered this estimate.
    Unknown,
    /// The caller's own authoritative reading: the provider's stated word,
    /// read from the gateway-quota cache. Never displaced by a learned
    /// value — see [`estimate_subscription_headroom`].
    Stated,
    /// No stated reading existed. Inferred from
    /// [`MIN_LEARNED_RESET_RECOVERIES`] or more throttle→success recoveries
    /// already in window.
    Learned,
}

/// Map line 1249 — whether the rows behind this estimate reach back far
/// enough to say anything about pressure beyond
/// [`RECENT_SIGNAL_HORIZON_SECONDS`], out to
/// [`LONG_SIGNAL_HORIZON_SECONDS`]. A third state, not a bucket guessed from
/// thin evidence: two rows an hour apart cannot tell a multi-hour window
/// from a monthly one, and the honest answer there is
/// [`Self::Undistinguished`] rather than a guessed [`Self::NoPressure`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LongWindowPressure {
    /// No informative row reached back far enough to say anything about the
    /// longer window — absence of evidence, not evidence of absence.
    Undistinguished,
    /// Coverage reached the long horizon and no throttle fell inside it.
    NoPressure,
    /// A throttle fell inside the long horizon, outside the short one:
    /// pressure the short window alone would miss entirely.
    Present,
}

/// Map lines 1244/1245/1246/1250/1251/1254's estimator output: never a bare
/// number.
///
/// # Why a band, never a percentage
///
/// [`crate::provider::quota::Percentage`] already refuses to label an
/// inferred capacity figure as exact (capability map line 1234); this type
/// goes one step further and carries no number at all, because none of its
/// inputs — accepted-request counts, throttle recency, session history — has
/// a natural denominator to divide by. A computed percentage would be a real
/// number glued to an invented scale, exactly what line 1251 forbids for
/// opaque token counts and what this type refuses to make representable for
/// the same reason.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HeadroomBand {
    /// A throttle inside [`RECENT_SIGNAL_HORIZON_SECONDS`] of `now`, with no
    /// reset imminent to relieve it.
    Exhausted,
    /// A throttle fell inside the window — recently, with a reset close
    /// behind it to soften the reading, or earlier and not repeated since.
    Low,
    /// Neither pressure nor activity was observed. A reset reading with
    /// nothing else behind it lands exactly here: real evidence the account
    /// is quota-bound, and none at all that it is under pressure right now.
    Moderate,
    /// Requests were accepted, or this project's own session history served
    /// this account, and no throttle fell in the window.
    Ample,
}

/// What kind of row [`estimate_subscription_headroom`] actually had to work
/// with — carried on the returned value so an opaque-limit account (map line
/// 1244: no token budget its provider will ever publish) and an account
/// whose rows happen to carry a token count render differently, without
/// either claiming more than the estimate has.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HeadroomBasis {
    /// No scoped row carried a token count. Accepted-request counts, throttle
    /// recency, reset behavior and session history are exactly what an
    /// opaque-limit account can supply, and this estimator asks nothing more
    /// of it.
    RequestActivity,
    /// At least one scoped row carried a token count. Recorded as a label
    /// only: map line 1251 forbids turning a raw count into a fictitious
    /// exact figure with no stated ceiling to divide it by, and this
    /// estimator does not duplicate the ceiling check
    /// [`crate::routing::Entitlement::spend_constraint`] already makes — a
    /// carried token count changes this label alone, never the band.
    TokenUsage,
}

/// Map line 1245's estimate, in full: a [`HeadroomBand`], the confidence it
/// is worth, what it was built from, and whose reading it is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SubscriptionHeadroomEstimate {
    pub band: HeadroomBand,
    /// Always [`Confidence::Low`] today — every signal behind this estimate
    /// is Glasshouse's own inference over its own recorded activity, never
    /// the provider's stated word. That is [`Confidence::Low`]'s own
    /// definition: *"derived, with no measurement of this quantity behind it
    /// at all."*
    pub confidence: Confidence,
    pub basis: HeadroomBasis,
    /// Map line 1246's keying rule, reused verbatim from
    /// [`recent_credential_throttles`]: `true` only when every informative
    /// row this estimate drew from named its own account; widened to
    /// provider scope the moment one does not.
    pub account_narrowed: bool,
    /// Map line 1248 — whose reset reading, if any, fed this estimate.
    pub reset_basis: ResetBasis,
    /// Map line 1249 — whether evidence separates short-window pressure
    /// from pressure that persists into the longer horizon.
    pub long_window_pressure: LongWindowPressure,
    /// Map line 1247's reachable half — the instant Glasshouse last detected
    /// a regime change for this provider (a stated ceiling that moved
    /// between two persisted gateway readings), if one has ever been
    /// recorded. `None` means the whole evidence window is still in play:
    /// no change was ever detected, or nothing here has looked for one.
    ///
    /// Always `None` out of [`estimate_subscription_headroom`] itself, which
    /// knows nothing about regime changes — its only production caller,
    /// [`crate::config::ResolvedEntitlement::with_telemetry`], is the one
    /// that floors the rows it passes in by this same instant and then
    /// stamps it here, so a rendered estimate can say which regime it
    /// describes.
    pub since_unix: Option<i64>,
}

/// Map line 1245's estimator, and lines 1244/1246/1250/1251/1254 with it —
/// see [`SubscriptionHeadroomEstimate`] and [`HeadroomBand`] for the type's
/// own honesty rules. No new table, no migration, no persisted estimator
/// state: every call re-derives the estimate from rows the caller already
/// holds.
///
/// Reads accepted-request counts and throttle events (narrowed to
/// `credential_label` only when **every** informative row names its
/// account; one contextless row widens to provider scope, map line 1246),
/// token usage (never turned into a figure, only recorded on
/// [`HeadroomBasis`], line 1251), reset behavior via `seconds_until_reset`
/// (line 1248: `None` falls back to a value learned from `scoped`'s own
/// throttle→success recoveries — see [`ResetBasis`] — never displacing a
/// real reading), and `recent_session_count` — none of them queried here,
/// all handed in by the caller.
///
/// `None` — unknown — when nothing at all is available: no informative row,
/// no session count, no reset reading. An account this genuinely unmeasured
/// is not "exhausted" and not "ample"; it is unmeasured, the 32B line-1239
/// discipline every other facet on `ResolvedEntitlement` already keeps.
// History: design-decisions.md, "Trims: routing module docs", routing/evidence/joins.rs `fn estimate_subscription_headroom`.
pub fn estimate_subscription_headroom(
    observations: &[RoutingObservation],
    provider: &str,
    credential_label: Option<&str>,
    now_unix: i64,
    seconds_until_reset: Option<i64>,
    recent_session_count: Option<usize>,
) -> Option<SubscriptionHeadroomEstimate> {
    let informative: Vec<&RoutingObservation> = observations
        .iter()
        .filter(|row| row.provider == provider)
        .filter(|row| row.outcome.is_some() && row.purpose.as_deref() != Some(CORRELATION_PURPOSE))
        .collect();

    let every_row_names_its_account =
        !informative.is_empty() && informative.iter().all(|row| row.quota_context.is_some());
    let account_narrowed = credential_label.is_some() && every_row_names_its_account;

    let scoped: Vec<&RoutingObservation> = if account_narrowed {
        informative
            .into_iter()
            .filter(|row| row.quota_context.as_deref() == credential_label)
            .collect()
    } else {
        informative
    };

    let accepted = scoped
        .iter()
        .filter(|row| row.outcome == Some(Outcome::Succeeded))
        .count();
    let most_recent_throttle_age = scoped
        .iter()
        .filter(|row| row.failure_class == Some(FailureClass::Throttle))
        .map(|row| now_unix.saturating_sub(row.observed_at_unix))
        .min();
    let carried_tokens = scoped
        .iter()
        .any(|row| row.input_tokens.is_some() || row.output_tokens.is_some());

    let session_count = recent_session_count.unwrap_or(0);

    if scoped.is_empty() && session_count == 0 && seconds_until_reset.is_none() {
        return None;
    }

    let recent_pressure =
        most_recent_throttle_age.is_some_and(|age| age <= RECENT_SIGNAL_HORIZON_SECONDS);
    let any_pressure = most_recent_throttle_age.is_some();
    let has_activity = accepted > 0 || session_count > 0;

    // Map line 1248: a stated reading is authoritative and is never
    // recomputed; only its absence opens the door to a learned fallback,
    // and even then only past the anecdote guard.
    let (effective_seconds_until_reset, reset_basis) = match seconds_until_reset {
        Some(seconds) => (Some(seconds), ResetBasis::Stated),
        None => match learn_reset_window_seconds(&scoped) {
            Some(window) => (Some(window), ResetBasis::Learned),
            None => (None, ResetBasis::Unknown),
        },
    };
    let reset_imminent = effective_seconds_until_reset
        .is_some_and(|seconds| (0..=RECENT_SIGNAL_HORIZON_SECONDS).contains(&seconds));

    // Map line 1249: positive evidence of long-window pressure needs no
    // full coverage of the horizon — one throttle out there is real
    // evidence regardless of how far back the rest of `scoped` reaches.
    // Its *absence* does, or the honest answer is "we did not look that
    // far", not "nothing happened".
    let long_window_pressure = {
        let present = scoped
            .iter()
            .filter(|row| row.failure_class == Some(FailureClass::Throttle))
            .map(|row| now_unix.saturating_sub(row.observed_at_unix))
            .any(|age| age > RECENT_SIGNAL_HORIZON_SECONDS && age <= LONG_SIGNAL_HORIZON_SECONDS);
        if present {
            LongWindowPressure::Present
        } else {
            let deepest_age = scoped
                .iter()
                .map(|row| now_unix.saturating_sub(row.observed_at_unix))
                .max();
            match deepest_age {
                Some(age) if age >= LONG_SIGNAL_HORIZON_SECONDS => LongWindowPressure::NoPressure,
                _ => LongWindowPressure::Undistinguished,
            }
        }
    };

    let band = match (recent_pressure, any_pressure, reset_imminent, has_activity) {
        (true, _, true, _) => HeadroomBand::Low,
        (true, _, false, _) => HeadroomBand::Exhausted,
        (false, true, _, _) => HeadroomBand::Low,
        (false, false, _, true) => HeadroomBand::Ample,
        (false, false, _, false) => HeadroomBand::Moderate,
    };

    Some(SubscriptionHeadroomEstimate {
        band,
        confidence: Confidence::Low,
        basis: if carried_tokens {
            HeadroomBasis::TokenUsage
        } else {
            HeadroomBasis::RequestActivity
        },
        account_narrowed,
        reset_basis,
        long_window_pressure,
        since_unix: None,
    })
}

/// Map line 1248's fallback window: the interval between a `Throttle` row
/// and the next `Succeeded` row after it in `scoped`, averaged across every
/// such recovery — `None` below [`MIN_LEARNED_RESET_RECOVERIES`] of them,
/// the anecdote rule stated in the packet this shipped from. Only ever
/// consulted by [`estimate_subscription_headroom`] when the caller supplied
/// no real `seconds_until_reset` at all.
fn learn_reset_window_seconds(scoped: &[&RoutingObservation]) -> Option<i64> {
    let mut ordered: Vec<&RoutingObservation> = scoped.to_vec();
    ordered.sort_by_key(|row| row.observed_at_unix);

    let mut recoveries = Vec::new();
    for (index, row) in ordered.iter().enumerate() {
        if row.failure_class != Some(FailureClass::Throttle) {
            continue;
        }
        if let Some(success) = ordered[index + 1..]
            .iter()
            .find(|later| later.outcome == Some(Outcome::Succeeded))
        {
            let recovery = success
                .observed_at_unix
                .saturating_sub(row.observed_at_unix);
            if recovery > 0 {
                recoveries.push(recovery);
            }
        }
    }

    if recoveries.len() < MIN_LEARNED_RESET_RECOVERIES {
        return None;
    }
    let sum: i64 = recoveries.iter().sum();
    Some(sum / recoveries.len() as i64)
}

/// [`EvidenceLedger::headroom_replay`]'s result — map line 1836, replaying
/// [`estimate_subscription_headroom`] against every throttle or exhaustion a
/// provider recorded, using only the rows that preceded it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct HeadroomReplayCounts {
    /// The replayed estimate's band was [`HeadroomBand::Low`] or
    /// [`HeadroomBand::Exhausted`] — the estimator would have warned.
    pub warned: usize,
    /// The replayed estimate's band was [`HeadroomBand::Moderate`] or
    /// [`HeadroomBand::Ample`] — the estimator would have missed it.
    pub missed: usize,
    /// [`estimate_subscription_headroom`] returned [`None`]: fewer rows
    /// came before this throttle than the estimator could read anything
    /// from at all.
    pub unestimable: usize,
    /// The median seconds from a throttle to this provider's first
    /// [`Outcome::Succeeded`] row after it — `None` when no throttle in the
    /// window was ever followed by one.
    pub observed_reset_lag_median_seconds: Option<i64>,
    /// How many throttles [`Self::observed_reset_lag_median_seconds`] is a
    /// median over.
    pub observed_reset_lag_sample_count: usize,
}

impl HeadroomReplayCounts {
    /// [`Self::warned`] + [`Self::missed`] + [`Self::unestimable`] — every
    /// throttle or exhaustion this replay scored, the denominator
    /// [`MIN_SAMPLE_FOR_SUMMARY`] gates the whole reading on.
    pub fn throttles(&self) -> usize {
        self.warned + self.missed + self.unestimable
    }
}

/// [`EvidenceLedger::output_estimate_accuracy`]'s result — map line 1855's
/// token half, one row per task class that carries at least one
/// [`crate::evaluation::EvaluationKind::RoutingConsumptionEstimated`] row in
/// the window.
#[derive(Debug, Clone, PartialEq)]
pub struct OutputEstimateAccuracy {
    /// The task class word the estimate row's own `subject` carries —
    /// [`crate::routing::request::TaskClass::as_str`].
    pub task_class: String,
    /// The median of *actual ÷ estimated* output tokens, `None` below
    /// [`MIN_SAMPLE_FOR_SUMMARY`] measured ratios.
    pub median_ratio: Option<f64>,
    /// How many sessions [`Self::median_ratio`] is a median over — an
    /// estimate row this reader could match to a summed actual.
    pub sample_count: usize,
    /// Sessions with an estimate row and no matching routing row (or one
    /// carrying no `output_tokens`) yet — never counted as a zero ratio.
    pub pending: usize,
}

/// One row of [`EvidenceLedger::effort_shadow`]'s per-`(turn_shape,
/// effort_level)` breakdown — capability map line 2039: the shadow
/// measurement `docs/product/design-decisions.md`'s *Carrying effort across a
/// translated pairing* asks for before any clamp is offered (*"Then the
/// measurement, then the clamp"*).
///
/// `sample_count`, `completed`, `failed` and `unverdicted` are counts and are
/// honest at any sample size — [`PurposeConsumption`]'s own convention.
/// `median_output_tokens` is a rate-shaped figure and sits behind the
/// standing floor every such figure on this ledger does ([`RoutingSummary`]'s
/// own doc comment): `None` below [`MIN_SAMPLE_FOR_SUMMARY`], never a median
/// nobody can trust.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EffortShadowRow {
    pub turn_shape: TurnShape,
    /// `None` is a real group: a translated exchange whose request asked for
    /// no effort at all, never folded into a rung of the ladder it did not
    /// carry.
    pub effort_level: Option<EffortLevel>,
    pub sample_count: usize,
    pub median_output_tokens: Option<i64>,
    /// How many of this group's exchanges had a session whose next
    /// [`crate::evaluation::EvaluationKind::TurnOutcomeObserved`] row said
    /// *completed*.
    pub completed: usize,
    /// ...said *failed*.
    pub failed: usize,
    /// Exchanges whose session recorded no `TurnOutcomeObserved` row at or
    /// after the exchange — never read from
    /// [`RoutingObservation::outcome`], which is a transport 2xx proxy and
    /// never a verdict (see that field's own doc comment).
    pub unverdicted: usize,
}

/// [`EvidenceLedger::effort_shadow`]'s result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EffortShadow {
    pub rows: Vec<EffortShadowRow>,
    /// Rows in the window whose `turn_shape` is `NULL` — a relayed exchange,
    /// or one written before migration 24's column existed. Counted, never
    /// folded into [`TurnShape::Prompt`].
    pub unread: usize,
}

/// [`EvidenceLedger::responsiveness_separation`]'s result — capability map
/// line 1850, one row per responsiveness figure.
#[derive(Debug, Clone, PartialEq)]
pub struct SeparationReport {
    /// Always exactly four, in the order line 1355 names them: raw TTFC,
    /// effective TTFC, TTFT, decode tokens/s.
    pub rows: Vec<SeparationMeasure>,
}

/// One figure's separation between usable and unusable agent turns —
/// map line 1850. *Separates*, never *predicts*: this is a comparison of
/// medians, not a claim of causation.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SeparationMeasure {
    pub measure: &'static str,
    pub usable_sample: usize,
    pub unusable_sample: usize,
    median_usable: Option<f64>,
    median_unusable: Option<f64>,
    median_all: Option<f64>,
}

impl SeparationMeasure {
    fn new(measure: &'static str, usable: Vec<f64>, unusable: Vec<f64>) -> Self {
        let usable_sample = usable.len();
        let unusable_sample = unusable.len();
        let all: Vec<f64> = usable.iter().chain(unusable.iter()).copied().collect();
        let median_usable = (usable_sample >= MIN_SAMPLE_FOR_SUMMARY).then(|| median_f64(usable));
        let median_unusable =
            (unusable_sample >= MIN_SAMPLE_FOR_SUMMARY).then(|| median_f64(unusable));
        let median_all = (!all.is_empty()).then(|| median_f64(all));
        Self {
            measure,
            usable_sample,
            unusable_sample,
            median_usable,
            median_unusable,
            median_all,
        }
    }

    pub fn median_usable(&self) -> Option<f64> {
        self.median_usable
    }

    pub fn median_unusable(&self) -> Option<f64> {
        self.median_unusable
    }

    /// `|median_unusable - median_usable| / median_all` — map line 1850's
    /// own formula. `None` when either side is below
    /// [`MIN_SAMPLE_FOR_SUMMARY`] ("not enough" on that side, per the
    /// ruling) or `median_all` is exactly `0.0`, never a divide-by-zero.
    pub fn separation(&self) -> Option<f64> {
        let usable = self.median_usable?;
        let unusable = self.median_unusable?;
        let all = self.median_all?;
        if all == 0.0 {
            return None;
        }
        Some((unusable - usable).abs() / all)
    }
}

fn median_f64(mut values: Vec<f64>) -> f64 {
    values.sort_by(|a, b| {
        a.partial_cmp(b)
            .expect("no NaN enters this ledger's figures")
    });
    values[values.len() / 2]
}

/// Effective TTFC (map line 1351) and its two supporting readings, computed
/// over any slice of routing observations the caller has already scoped to
/// the route it wants scored. This function does no filtering or grouping of
/// its own — the caller's slice **is** the scope, which is what lets one
/// function serve two callers: [`crate::routing::session::Destination`]'s
/// own reading over one `(provider, model)` pairing
/// (`GH-RESPONSIVENESS-TERMS` objective 2), and map line 1845's per-pairing-
/// class join over every row a session's class served
/// (`crate::evaluation`'s reader).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RouteResponsiveness {
    /// Mean `first_tool_call_ms` over the rows carrying one. Never a seconds
    /// fallback — 1347's own millisecond resolution is the point, and a
    /// mixed-resolution mean would misstate the very quantity this reads.
    pub raw_ttfc_ms: Option<f64>,
    pub raw_ttfc_sample: usize,
    /// The scope's transport-level failure rate — `failure_rate_aggregate`
    /// verbatim, `None` below [`MIN_SAMPLE_FOR_SUMMARY`].
    pub failure_rate: Option<f64>,
    pub failure_rate_sample: usize,
    /// Rounds begun ÷ minutes served, over rows carrying both a round count
    /// and a dispatch/completion pair — 1350's rate, recomputed from raw
    /// rows here rather than read off [`PurposeConsumption`], because this
    /// scope (one route, or one pairing class) is neither of that reader's
    /// two groupings.
    pub rounds_per_minute: Option<f64>,
    pub rounds_per_minute_sample: usize,
    /// Map lines 1535/1545: prompt-cache reads over total input tokens —
    /// [`TranslationSavings::cache_read_ratio`]'s own formula, recomputed
    /// here for the same reason [`Self::rounds_per_minute`] is: this scope
    /// is neither of that reader's two groupings. `None` below
    /// [`MIN_SAMPLE_FOR_SUMMARY`] rows carrying a known `input_tokens` —
    /// the same floor every *rate* on this ledger sits behind — never a
    /// ratio computed on too thin a sample.
    ///
    /// [`TranslationSavings::cache_read_ratio`]: super::TranslationSavings::cache_read_ratio
    pub cache_read_ratio: Option<f64>,
    /// Rows carrying a known `input_tokens` — every relayed row this scope
    /// held is excluded before this count, exactly as
    /// [`EvidenceLedger::translation_cache_savings`]'s own `WHERE
    /// input_tokens IS NOT NULL` excludes it at the SQL layer.
    pub cache_read_ratio_sample: usize,
}

impl RouteResponsiveness {
    pub fn from_observations(observations: &[RoutingObservation]) -> Self {
        let ttfc_values: Vec<f64> = observations
            .iter()
            .filter_map(|o| o.first_tool_call_ms)
            .map(|ms| ms as f64)
            .collect();
        let raw_ttfc_sample = ttfc_values.len();
        let raw_ttfc_ms = (!ttfc_values.is_empty())
            .then(|| ttfc_values.iter().sum::<f64>() / ttfc_values.len() as f64);

        let failure = failure_rate_aggregate(observations);
        let failure_rate_sample = failure
            .as_ref()
            .map(AggregateReading::sample_count)
            .unwrap_or(0);
        let failure_rate = failure.as_ref().map(|reading| *reading.value());

        let mut rounds_sum: i64 = 0;
        let mut serving_seconds_sum: i64 = 0;
        let mut rounds_per_minute_sample = 0usize;
        for observation in observations {
            if let (Some(rounds), Some(dispatched), Some(completed)) = (
                observation.tool_rounds,
                observation.dispatched_at_unix,
                observation.completed_at_unix,
            ) {
                rounds_sum += rounds;
                serving_seconds_sum += completed - dispatched;
                rounds_per_minute_sample += 1;
            }
        }
        let rounds_per_minute = (rounds_per_minute_sample > 0 && serving_seconds_sum > 0)
            .then(|| rounds_sum as f64 * 60.0 / serving_seconds_sum as f64);

        let mut input_tokens_sum: i64 = 0;
        let mut cached_input_tokens_sum: i64 = 0;
        let mut cache_read_ratio_sample = 0usize;
        for observation in observations {
            if let Some(input_tokens) = observation.input_tokens {
                input_tokens_sum += input_tokens;
                cached_input_tokens_sum += observation.cached_input_tokens.unwrap_or(0);
                cache_read_ratio_sample += 1;
            }
        }
        let cache_read_ratio = (cache_read_ratio_sample >= MIN_SAMPLE_FOR_SUMMARY
            && input_tokens_sum + cached_input_tokens_sum > 0)
            .then(|| {
                cached_input_tokens_sum as f64 / (input_tokens_sum + cached_input_tokens_sum) as f64
            });

        Self {
            raw_ttfc_ms,
            raw_ttfc_sample,
            failure_rate,
            failure_rate_sample,
            rounds_per_minute,
            rounds_per_minute_sample,
            cache_read_ratio,
            cache_read_ratio_sample,
        }
    }

    /// Map line 1351: `raw_ttfc_ms / (1 - failure_rate)`, defined only when
    /// both the TTFC sample and the failure-rate sample meet
    /// [`MIN_SAMPLE_FOR_SUMMARY`] and the failure rate is below 100% —
    /// `None` otherwise, never a clamped number.
    pub fn effective_ttfc_ms(&self) -> Option<f64> {
        if self.raw_ttfc_sample < MIN_SAMPLE_FOR_SUMMARY {
            return None;
        }
        if self.failure_rate_sample < MIN_SAMPLE_FOR_SUMMARY {
            return None;
        }
        let raw = self.raw_ttfc_ms?;
        let p = self.failure_rate?;
        if p >= 1.0 {
            return None;
        }
        Some(raw / (1.0 - p))
    }
}

impl EvidenceLedger {
    /// **Map line 1836.** Replays [`estimate_subscription_headroom`] against
    /// every throttle or exhaustion this provider recorded in the window,
    /// using only the rows that came *before* it — never the estimator's
    /// live inputs. `credential_label`, `seconds_until_reset` and
    /// `recent_session_count` are always absent here: this replay has no
    /// account narrowing to apply and no gateway-quota-cache reading to
    /// hand in, and pretending otherwise would score the estimator against
    /// evidence it never actually had at that moment. `estimate_subscription_headroom`
    /// itself is not modified; this calls it once per throttle.
    ///
    /// Paired with the *observed reset lag*: this ledger records no
    /// provider-stated wait ([`RoutingObservation`] carries no
    /// `retry_after`/reset field — that reading lives only in the gateway's
    /// quota-cache file, a different store [`Self`] does not open), so the
    /// only honest reset figure is the one actually observed — from a
    /// throttle at `t` to this provider's first [`Outcome::Succeeded`] row
    /// after `t`, in the same window.
    pub fn headroom_replay(
        &self,
        provider: &str,
        now_unix: i64,
        window_seconds: i64,
    ) -> Result<HeadroomReplayCounts, EvidenceLedgerError> {
        let rows = self.observations_in_window(now_unix, window_seconds)?;
        let provider_rows: Vec<&RoutingObservation> =
            rows.iter().filter(|row| row.provider == provider).collect();

        let mut warned = 0usize;
        let mut missed = 0usize;
        let mut unestimable = 0usize;
        let mut reset_lags: Vec<i64> = Vec::new();

        for row in &provider_rows {
            if !matches!(
                row.failure_class,
                Some(FailureClass::Throttle) | Some(FailureClass::ExhaustedQuota)
            ) {
                continue;
            }
            let t = row.observed_at_unix;
            let prior: Vec<RoutingObservation> = provider_rows
                .iter()
                .filter(|candidate| candidate.observed_at_unix < t)
                .map(|candidate| (*candidate).clone())
                .collect();
            match estimate_subscription_headroom(&prior, provider, None, t, None, None) {
                Some(estimate) => match estimate.band {
                    HeadroomBand::Low | HeadroomBand::Exhausted => warned += 1,
                    HeadroomBand::Moderate | HeadroomBand::Ample => missed += 1,
                },
                None => unestimable += 1,
            }
            if let Some(recovery) = provider_rows
                .iter()
                .filter(|candidate| candidate.observed_at_unix > t)
                .find(|candidate| candidate.outcome == Some(Outcome::Succeeded))
            {
                reset_lags.push(recovery.observed_at_unix - t);
            }
        }

        let observed_reset_lag_sample_count = reset_lags.len();
        let observed_reset_lag_median_seconds = if reset_lags.is_empty() {
            None
        } else {
            Some(median(reset_lags))
        };

        Ok(HeadroomReplayCounts {
            warned,
            missed,
            unestimable,
            observed_reset_lag_median_seconds,
            observed_reset_lag_sample_count,
        })
    }

    /// **Map line 1855, the token half.** Joins each
    /// [`crate::evaluation::EvaluationKind::RoutingConsumptionEstimated`]
    /// row in the window to the sum of `output_tokens` over this project's
    /// own routing rows carrying the same `session_id`, at or after the
    /// estimate row's own `observed_at` — the actual consumption the
    /// launch's estimate was a prediction *of*.
    ///
    /// [`crate::evaluation::EvaluationObservations`] and this ledger wrap
    /// separate [`Connection`]s onto the **same** project database file —
    /// [`Self::effort_shadow`] already reads `evaluation_observations` this
    /// way — so this is the same one-file, one-query join, scoped by
    /// [`Self::project_id`] on both sides, rather than opening a second
    /// ledger handle for a read this connection can already serve.
    ///
    /// A session with an estimate row and **no** matching routing row (or
    /// one whose `output_tokens` are all still `NULL`) has an unknown
    /// actual, never a fabricated zero — [`Self::consumption_by_purpose`]'s
    /// own rule for an absent sum. [`Self::output_estimate_accuracy`]
    /// reports it as *pending*.
    // History: design-decisions.md, "Trims: routing module docs", routing/evidence/joins.rs `fn estimate_pairs`.
    fn estimate_pairs(
        &self,
        now_unix: i64,
        window_seconds: i64,
    ) -> Result<Vec<(String, f64, Option<f64>)>, EvidenceLedgerError> {
        let earliest = now_unix.saturating_sub(window_seconds);
        let conn = self.lock();
        let mut statement = conn
            .prepare(
                "SELECT e.subject,
                        e.detail,
                        (SELECT SUM(r.output_tokens) FROM routing_observations r
                           WHERE r.project_id = ?1
                             AND r.session_id = e.session_id
                             AND r.observed_at >= e.observed_at
                             AND r.observed_at <= ?4
                             AND r.output_tokens IS NOT NULL) AS actual_sum
                 FROM evaluation_observations e
                 WHERE e.project_id = ?1
                   AND e.kind = ?5
                   AND e.observed_at >= ?2 AND e.observed_at <= ?4
                   AND e.session_id IS NOT NULL
                 ORDER BY e.subject, e.observed_at ASC",
            )
            .map_err(sql_err("read routing-consumption estimate pairs"))?;
        let rows = statement
            .query_map(
                params![
                    self.project_id,
                    earliest,
                    now_unix,
                    now_unix,
                    crate::evaluation::EvaluationKind::RoutingConsumptionEstimated.as_str(),
                ],
                |row| {
                    let subject: String = row.get(0)?;
                    let detail: Option<String> = row.get(1)?;
                    let actual_sum: Option<i64> = row.get(2)?;
                    Ok((subject, detail, actual_sum))
                },
            )
            .map_err(sql_err("read routing-consumption estimate pairs"))?;
        let mut out = Vec::new();
        for row in rows {
            let (subject, detail, actual_sum) =
                row.map_err(sql_err("read one routing-consumption estimate pair"))?;
            let Some(estimated) = detail.as_deref().and_then(|text| text.parse::<f64>().ok())
            else {
                continue;
            };
            out.push((subject, estimated, actual_sum.map(|tokens| tokens as f64)));
        }
        Ok(out)
    }

    /// **Map line 1855, the token half, rendered per task class.** The
    /// median of *actual ÷ estimated* output tokens over sessions whose
    /// launch recorded an estimate (`Self::estimate_pairs`), grouped by
    /// the task class the estimate names — `None` below
    /// [`MIN_SAMPLE_FOR_SUMMARY`] measured ratios, never a median guessed
    /// from too few of them, exactly as every other median on this ledger
    /// withholds.
    pub fn output_estimate_accuracy(
        &self,
        now_unix: i64,
        window_seconds: i64,
    ) -> Result<Vec<OutputEstimateAccuracy>, EvidenceLedgerError> {
        let pairs = self.estimate_pairs(now_unix, window_seconds)?;
        let mut by_class: std::collections::BTreeMap<String, (Vec<i64>, usize)> =
            std::collections::BTreeMap::new();
        for (subject, estimated, actual) in pairs {
            let entry = by_class.entry(subject).or_default();
            match actual {
                Some(actual) if estimated > 0.0 => {
                    // Scaled by 1000 and rounded so the fractional ratio can
                    // share this module's integer `median`, the same way
                    // every other ratio here is computed in Rust rather than
                    // in SQL.
                    let scaled = ((actual / estimated) * 1000.0).round() as i64;
                    entry.0.push(scaled);
                }
                _ => entry.1 += 1,
            }
        }
        Ok(by_class
            .into_iter()
            .map(|(task_class, (mut ratios, pending))| {
                let sample_count = ratios.len();
                let median_ratio = (sample_count >= MIN_SAMPLE_FOR_SUMMARY).then(|| {
                    ratios.sort_unstable();
                    median(std::mem::take(&mut ratios)) as f64 / 1000.0
                });
                OutputEstimateAccuracy {
                    task_class,
                    median_ratio,
                    sample_count,
                    pending,
                }
            })
            .collect())
    }

    /// Capability map line 1850: whether effective TTFC separates usable
    /// agent turns from unusable ones better than raw TTFC, TTFT or decode
    /// tokens per second — one [`SeparationMeasure`] per figure.
    ///
    /// Scoped to [`HARNESS_TURN_PURPOSE`] rows, the same restriction
    /// [`Self::translation_cache_savings`] applies, since only a translated
    /// exchange ever carries `first_tool_call_ms`, `first_token_ms` or a tool
    /// round to measure. The usable-turn verdict is [`Self::effort_shadow`]'s
    /// own subquery — the session's next
    /// [`crate::evaluation::EvaluationKind::TurnOutcomeObserved`] row at or
    /// after the exchange — never [`RoutingObservation::outcome`], a
    /// transport 2xx proxy and not a verdict; an exchange whose session
    /// recorded no such row is excluded from every measure here.
    ///
    /// **Effective TTFC is attached per row from its own route**, not
    /// computed per exchange: each row's contribution is its
    /// `(provider, model)`'s [`RouteResponsiveness::effective_ttfc_ms`] over
    /// this same window, computed once per route and read off for every row
    /// that route served. Raw TTFC, TTFT and decode tokens/s are each row's
    /// own figure.
    // History: design-decisions.md, "Trims: routing module docs", routing/evidence/joins.rs `fn responsiveness_separation`.
    pub fn responsiveness_separation(
        &self,
        now_unix: i64,
        window_seconds: i64,
    ) -> Result<SeparationReport, EvidenceLedgerError> {
        let earliest = now_unix.saturating_sub(window_seconds);
        let rows: Vec<(RoutingObservation, Option<String>)> = {
            let conn = self.lock();
            let mut statement = conn
                .prepare(
                    "SELECT r.*,
                            (SELECT e.subject FROM evaluation_observations AS e
                              WHERE e.kind = ?5
                                AND e.session_id = r.session_id
                                AND e.observed_at >= r.observed_at
                              ORDER BY e.observed_at ASC
                              LIMIT 1) AS verdict
                     FROM routing_observations AS r
                     WHERE r.project_id = ?1 AND r.observed_at >= ?2 AND r.observed_at <= ?3
                       AND r.purpose = ?4
                     ORDER BY r.observed_at ASC",
                )
                .map_err(sql_err(
                    "read observations for the responsiveness separation",
                ))?;
            let mapped = statement
                .query_map(
                    params![
                        self.project_id,
                        earliest,
                        now_unix,
                        HARNESS_TURN_PURPOSE,
                        crate::evaluation::EvaluationKind::TurnOutcomeObserved.as_str(),
                    ],
                    |row| {
                        let verdict: Option<String> = row.get("verdict")?;
                        Ok((row_to_observation(row)?, verdict))
                    },
                )
                .map_err(sql_err(
                    "read observations for the responsiveness separation",
                ))?;
            let mut rows = Vec::new();
            for row in mapped {
                let (observation, verdict) =
                    row.map_err(sql_err("read one responsiveness-separation row"))?;
                rows.push((observation?, verdict));
            }
            rows
        };

        let mut by_route: std::collections::BTreeMap<(String, String), Vec<RoutingObservation>> =
            std::collections::BTreeMap::new();
        for (observation, _) in &rows {
            by_route
                .entry((observation.provider.clone(), observation.model.clone()))
                .or_default()
                .push(observation.clone());
        }
        let route_effective_ttfc: std::collections::BTreeMap<(String, String), Option<f64>> =
            by_route
                .into_iter()
                .map(|(key, group)| {
                    let ttfc = RouteResponsiveness::from_observations(&group).effective_ttfc_ms();
                    (key, ttfc)
                })
                .collect();

        let mut usable = Vec::new();
        let mut unusable = Vec::new();
        for (observation, verdict) in &rows {
            match verdict.as_deref() {
                Some(EFFORT_SHADOW_VERDICT_COMPLETED) => usable.push(observation),
                Some(EFFORT_SHADOW_VERDICT_FAILED) => unusable.push(observation),
                _ => {}
            }
        }

        let raw_ttfc = |side: &[&RoutingObservation]| -> Vec<f64> {
            side.iter()
                .filter_map(|o| o.first_tool_call_ms)
                .map(|ms| ms as f64)
                .collect()
        };
        let effective_ttfc = |side: &[&RoutingObservation]| -> Vec<f64> {
            side.iter()
                .filter_map(|o| {
                    route_effective_ttfc
                        .get(&(o.provider.clone(), o.model.clone()))
                        .copied()
                        .flatten()
                })
                .collect()
        };
        let ttft = |side: &[&RoutingObservation]| -> Vec<f64> {
            side.iter()
                .filter_map(|o| o.first_token_ms)
                .map(|ms| ms as f64)
                .collect()
        };
        let decode_rate = |side: &[&RoutingObservation]| -> Vec<f64> {
            side.iter()
                .filter_map(|o| {
                    let output_tokens = o.output_tokens?;
                    let first_token_ms = o.first_token_ms?;
                    let completed_ms = o.completed_ms?;
                    if completed_ms < first_token_ms {
                        return None;
                    }
                    let decode_ms = completed_ms - first_token_ms;
                    if decode_ms <= 0 {
                        return None;
                    }
                    Some(output_tokens as f64 * 1000.0 / decode_ms as f64)
                })
                .collect()
        };

        Ok(SeparationReport {
            rows: vec![
                SeparationMeasure::new("raw TTFC", raw_ttfc(&usable), raw_ttfc(&unusable)),
                SeparationMeasure::new(
                    "effective TTFC",
                    effective_ttfc(&usable),
                    effective_ttfc(&unusable),
                ),
                SeparationMeasure::new("TTFT", ttft(&usable), ttft(&unusable)),
                SeparationMeasure::new(
                    "decode tokens/s",
                    decode_rate(&usable),
                    decode_rate(&unusable),
                ),
            ],
        })
    }

    /// [`EffortShadow`] — capability map line 2039's shadow measurement: per
    /// translated exchange this build recorded a turn shape and an
    /// output-token count for, whether the exchange's session's next
    /// harness-reported verdict was a completion, a failure, or nothing at
    /// all. Joined by migration 24's `session_id`, **never** by
    /// [`RoutingObservation::outcome`] — a transport 2xx proxy, not a
    /// verdict, per that field's own doc comment.
    ///
    /// **Two statements, not one.** The verdict is *the session's next
    /// [`crate::evaluation::EvaluationKind::TurnOutcomeObserved`] row at or
    /// after the exchange's `observed_at`* — a correlated subquery per
    /// candidate row — and this reader's median is computed in Rust from the
    /// raw sample, so the classified rows are fetched flat, with the verdict
    /// subquery inline, and folded here rather than in a `GROUP BY`.
    /// [`EffortShadow::unread`] is a second, simpler statement over the same
    /// window and purpose: a row whose `turn_shape` this reader could not
    /// decode is never folded into either turn shape, so its count comes
    /// from a query with no `output_tokens` filter at all — an unread row's
    /// tokens are unread for the same reason its shape is.
    ///
    /// Only [`HARNESS_TURN_PURPOSE`] rows with `output_tokens IS NOT NULL`
    /// enter a group.
    // History: design-decisions.md, "Trims: routing module docs", routing/evidence/joins.rs `fn effort_shadow`.
    pub fn effort_shadow(
        &self,
        now_unix: i64,
        window_seconds: i64,
    ) -> Result<EffortShadow, EvidenceLedgerError> {
        let earliest = now_unix.saturating_sub(window_seconds);
        let conn = self.lock();

        let unread: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM routing_observations
                 WHERE project_id = ?1 AND observed_at >= ?2 AND observed_at <= ?3
                   AND purpose = ?4 AND turn_shape IS NULL",
                params![self.project_id, earliest, now_unix, HARNESS_TURN_PURPOSE],
                |row| row.get(0),
            )
            .map_err(sql_err("read the effort shadow's unread count"))?;

        let mut statement = conn
            .prepare(
                "SELECT r.turn_shape AS turn_shape,
                        r.effort_level AS effort_level,
                        r.output_tokens AS output_tokens,
                        (SELECT e.subject FROM evaluation_observations AS e
                          WHERE e.kind = ?5
                            AND e.session_id = r.session_id
                            AND e.observed_at >= r.observed_at
                          ORDER BY e.observed_at ASC
                          LIMIT 1) AS verdict
                 FROM routing_observations AS r
                 WHERE r.project_id = ?1 AND r.observed_at >= ?2 AND r.observed_at <= ?3
                   AND r.purpose = ?4 AND r.output_tokens IS NOT NULL",
            )
            .map_err(sql_err("read the effort shadow's classified rows"))?;
        let mapped = statement
            .query_map(
                params![
                    self.project_id,
                    earliest,
                    now_unix,
                    HARNESS_TURN_PURPOSE,
                    crate::evaluation::EvaluationKind::TurnOutcomeObserved.as_str(),
                ],
                |row| {
                    let turn_shape_text: Option<String> = row.get("turn_shape")?;
                    let effort_level_text: Option<String> = row.get("effort_level")?;
                    let output_tokens: i64 = row.get("output_tokens")?;
                    let verdict: Option<String> = row.get("verdict")?;
                    Ok((turn_shape_text, effort_level_text, output_tokens, verdict))
                },
            )
            .map_err(sql_err("read the effort shadow's classified rows"))?;

        let mut groups: std::collections::BTreeMap<(u8, i8), EffortShadowGroup> =
            std::collections::BTreeMap::new();

        for row in mapped {
            let (turn_shape_text, effort_level_text, output_tokens, verdict) =
                row.map_err(sql_err("read one effort shadow row"))?;
            // A row this build cannot decode a turn shape for — `NULL`, or an
            // unrecognised future word — is never guessed into a shape: it is
            // already counted in `unread` above, and grouping it here would
            // count it twice under a shape it did not carry.
            let Some(turn_shape) = turn_shape_text.as_deref().and_then(TurnShape::from_stored)
            else {
                continue;
            };
            let effort_level = effort_level_text
                .as_deref()
                .and_then(EffortLevel::from_stored);
            let key = (turn_shape_rank(turn_shape), effort_level_rank(effort_level));
            let entry = groups
                .entry(key)
                .or_insert_with(|| EffortShadowGroup::new(turn_shape, effort_level));
            entry.output_tokens.push(output_tokens);
            match verdict.as_deref() {
                Some(EFFORT_SHADOW_VERDICT_COMPLETED) => entry.completed += 1,
                Some(EFFORT_SHADOW_VERDICT_FAILED) => entry.failed += 1,
                _ => entry.unverdicted += 1,
            }
        }

        let rows = groups
            .into_values()
            .map(EffortShadowGroup::into_row)
            .collect();

        Ok(EffortShadow {
            rows,
            unread: unread as usize,
        })
    }
}
