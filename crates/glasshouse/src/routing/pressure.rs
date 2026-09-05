//! Phase 35D — routing under subscription pressure: what a destination's
//! capacity **band** and the nearness of its reset do to the session
//! router's ranking, and the reserve policy the scope of work runs under.
//!
//! Capability map lines 1570–1577, 1606/1612: [`capacity_band_pressure`]
//! penalises a premium destination in the **tight** band (less so the
//! nearer its reset), and routes a **reserve**-band one to the
//! protected-reserve policy this build already has; [`low_tier_spend`]
//! keeps a premium destination under pressure off low-tier work while a
//! healthy zero-cost alternative is among the candidates. Everything else
//! is *read*, not re-decided, from existing sources — inventing a second
//! copy of any of them would be two scales for one question.
//!
//! What is tunable is configuration, never a hierarchy in this source:
//! `routing.capacity_band_thresholds`, a provider's `reserve_percent`, and
//! `routing.reserve.*` — `tests/subscription_pressure.rs` scans this file
//! for those names and for the absence of any provider's or model's.
//!
//! Every contribution below has a test in `tests/subscription_pressure.rs`
//! holding two destinations that differ **only** in its axis and resolving
//! differently; a term that cannot separate anything (no reading, unknown
//! tier, a zero-cost destination) contributes exactly `0.0` and says why —
//! neither "assume healthy" nor "assume exhausted".
//!
//! "Premium" is one fact, [`super::Cost`]: whether the destination costs
//! the user anything at the margin, fail-closed to [`super::Cost::Metered`]
//! for anything not marked free. The *shape* of the quota is not consulted.
//!
//! No clock, no store, no socket, no name: **no provider, model or harness
//! is named in this file** — the policy is tunable only through
//! configuration, and line 1612 is enforced by a test that scans this
//! source.
// History: design-decisions.md, "Trims: routing module docs", routing/pressure.rs module doc.

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::provider::quota::{
    CapacityBand, RESET_DISTANT_SECONDS, RESET_IMMINENT_SECONDS, ReserveDecision,
    ReserveDecisionInputs, evaluate_reserve_spend,
};

use super::Contribution;
use super::classify::WorkloadTier;

// ---------------------------------------------------------------------------
// Weights. On the scale `super::session` already established: a live warm
// session at zero idle is worth 1.5, a cold bootstrap costs 1.0, and a
// required capability established absent costs 0.4. Every magnitude below is
// placed against those three on purpose, and the placement is the decision.
// ---------------------------------------------------------------------------

/// Line 1570. What being in the tight band costs a premium destination — the
/// full penalty, before reset relief.
///
/// Placed **below** the cost of a required capability established absent
/// (`0.4` in `super::session`), so a tight but adequate destination still
/// beats an alternative established to lack something the task needs —
/// *adequate* is the line's own word, and it is enforced by this ordering
/// rather than by a second capability model here. And placed far below what a
/// live warm session is worth (`1.5`), so that tightness alone never moves
/// warm work — line 1572, which is a statement about this magnitude and needs
/// no term of its own: "high-value" is the warmth and checkpoint quality the
/// router already weighs, not a new guess.
pub const TIGHT_BAND_PENALTY: f64 = -0.35;

/// Lines 1571 and 1606. What a reserve-band destination costs when the
/// scope's reserve policy **denies** the spend.
///
/// Above a live warm session's worth (`1.5`), so a denied reserve can move
/// even warm work to a comparable alternative — the reserve is protected
/// *for* high-tier work, which means lighter work leaves it when it can.
/// Below warmth plus a cold bootstrap (`2.5`), so it never sends work to a
/// session that would have to start from nothing: a reserve breach chooses
/// between destinations that can both carry the work, it does not throw the
/// work away.
pub const RESERVE_DENIED_PENALTY: f64 = -2.0;

/// Line 1575. What spending a premium destination under pressure on
/// low-tier work costs while a healthy zero-cost alternative could do it.
///
/// Above warmth plus a cold bootstrap (`2.5`) on purpose: this is the one
/// term the packet's own contract lets outweigh a warm session — *"a warm
/// existing session on a tight subscription still beats a cold fresh
/// alternative **unless** the alternative is adequate and the task is low
/// tier"* — because a leaf task neither needs the warm context nor deserves
/// the subscription.
pub const LOW_TIER_SPEND_PENALTY: f64 = -3.0;

/// Lines 1573 and 1574. A reset within this many seconds waives the tight
/// penalty entirely: capacity that would otherwise expire unused is spent
/// freely. The same figure as
/// [`crate::provider::quota::RESET_IMMINENT_SECONDS`], deliberately — one
/// definition of "imminent" across the reserve policy, the effective-capacity
/// score and this term, so a reset cannot be imminent to one and not another.
pub const RESET_RELIEF_HORIZON_SECONDS: i64 = RESET_IMMINENT_SECONDS;

/// Line 1573's other end. A reset this many seconds away or further relieves
/// nothing — the same figure as
/// [`crate::provider::quota::RESET_DISTANT_SECONDS`], for the reason above.
/// Between the horizon and this, relief fades linearly, so a destination
/// crossing either boundary does not jump.
pub const RESET_RELIEF_FADE_SECONDS: i64 = RESET_DISTANT_SECONDS;

/// Line 1280. What a destination forecast to exhaust **well before** its
/// next reset costs.
///
/// Twice [`TIGHT_BAND_PENALTY`], and deliberately below a live warm
/// session's worth (`1.5` in `super::session`) for that constant's own
/// stated reason: a forecast is an estimate over a median of bucket counts,
/// and an estimate must not be able to throw away warm context on its own.
/// Above tightness because it is a strictly stronger statement — the band
/// says *this resource is low*, the forecast says *this resource is low and
/// is being spent fast enough to run out before it is given back*, which is
/// the fact line 1280 asks the ranking to react to.
///
/// It does not stack surprisingly: a destination that earns this has almost
/// always earned [`TIGHT_BAND_PENALTY`] too, and the two together
/// (`-1.05`) still sit below warmth, so the pair moves cold work and leaves
/// warm work where it is. That is the same ordering line 1572 fixed for the
/// band alone.
pub const EXHAUSTION_FORECAST_PENALTY: f64 = 2.0 * TIGHT_BAND_PENALTY;

/// Line 1575's "low tier": at or below this tier. [`WorkloadTier::Leaf`]'s
/// own doc — *"a disposable, free, or local model is expected to be
/// sufficient"* — is the definition, read rather than restated;
/// [`WorkloadTier::Standard`] is "an ordinary interactive model" and is not
/// low.
pub const LOW_TIER_CEILING: WorkloadTier = WorkloadTier::Leaf;

// ---------------------------------------------------------------------------
// The values a destination carries and the caller computes.
// ---------------------------------------------------------------------------

/// What the caller read about a destination's capacity band and reset —
/// the two inputs lines 1570–1574 are about, carried on the destination so
/// this module reads no telemetry of its own.
///
/// Both halves are `None` when nothing has been read, and the terms below
/// are inert for such a destination and say so. The caller resolves them
/// exactly as `main.rs::disposable_candidate_capacity` does for the
/// disposable router: [`crate::provider::quota::RemainingCapacityScore::band`]
/// against the configured thresholds with the provider's own reserve
/// percentage applied, and
/// [`crate::provider::quota::CapacityState::seconds_until_reset`] against
/// the caller's clock.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct CapacityFacts {
    band: Option<CapacityBand>,
    seconds_until_reset: Option<i64>,
}

impl CapacityFacts {
    /// Nothing read: the honest floor for a destination whose provider has
    /// no cached reading, and the value every destination starts with.
    pub const UNREAD: Self = Self {
        band: None,
        seconds_until_reset: None,
    };

    pub fn new(band: Option<CapacityBand>, seconds_until_reset: Option<i64>) -> Self {
        Self {
            band,
            seconds_until_reset,
        }
    }

    pub fn band(&self) -> Option<CapacityBand> {
        self.band
    }

    pub fn seconds_until_reset(&self) -> Option<i64> {
        self.seconds_until_reset
    }
}

/// What the reserve band means for work in one scope — capability map line
/// 1577, the value `routing.reserve.interactive` and
/// `routing.reserve.background` take.
///
/// Two values and no third. `protect` puts a reserve-band destination to
/// Phase 32F's policy and penalises a denial; `spend` says this scope's work
/// may use the reserve, and the band then costs only what tightness costs.
/// There is deliberately no `exclude`: on the launch path a destination
/// removed from the ranking is not replaced by a refusal, the launch simply
/// proceeds under its requested profile with no routing announcement at all
/// — a silence indistinguishable from "nothing was excluded", which is the
/// defect this project keeps finding (practice §68's family). A penalty stays
/// in the explanation; an exclusion would vanish from it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ReservePolicy {
    /// The reserve is protected for this scope: a reserve-band destination is
    /// admitted only when the reserve policy allows the spend, and penalised
    /// by [`RESERVE_DENIED_PENALTY`] otherwise. The default, because a
    /// spending protection fails closed.
    #[default]
    Protect,
    /// The reserve is not protected for this scope. The band still costs what
    /// the tight band costs — being in reserve is tighter than tight — but no
    /// denial is issued.
    Spend,
}

impl ReservePolicy {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Protect => "protect",
            Self::Spend => "spend",
        }
    }
}

impl fmt::Display for ReservePolicy {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Which kind of work a routing decision is for — line 1577's two scopes.
///
/// The session router (`super::session`) is always [`Self::Interactive`]:
/// every one of its production callers is a person's own launch, resume or
/// `route` diagnostic. [`Self::Background`] is the disposable router's scope
/// (`super::disposable`, whose reserve call site is the one place a
/// background job's reserve decision is taken), and it is a variant here so
/// the policy selection is one function that both could call, not so this
/// module could guess which scope a caller meant.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReserveScope {
    /// A person's own session: `glasshouse launch`, `resume`, `route`.
    Interactive,
    /// A support job Glasshouse runs on its own behalf — memory extraction,
    /// classification.
    Background,
}

impl ReserveScope {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Interactive => "interactive",
            Self::Background => "background",
        }
    }
}

impl fmt::Display for ReserveScope {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Both scopes' reserve policies, as configuration resolved them — line
/// 1577's whole content: the two are separate fields so a user can say that
/// their own work may spend the reserve while background jobs may not, or
/// the reverse.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ReservePolicies {
    pub interactive: ReservePolicy,
    pub background: ReservePolicy,
}

impl ReservePolicies {
    /// The policy `scope` runs under. The only place the selection is made.
    pub fn for_scope(&self, scope: ReserveScope) -> ReservePolicy {
        match scope {
            ReserveScope::Interactive => self.interactive,
            ReserveScope::Background => self.background,
        }
    }
}

/// What the router found among the **other** candidates, computed once per
/// destination from the set it is ranking — the two set-level facts lines
/// 1575 and 1571 (through line 1288) need, which no destination can know
/// about itself.
///
/// Each half names the destination it found so the explanation can point at
/// it rather than assert that "an alternative exists". `None` is "no such
/// candidate", and both terms are inert on it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Alternatives {
    healthy_free_adequate: Option<String>,
    cheaper_adequate: Option<String>,
}

impl Alternatives {
    /// No other candidate qualifies for either half.
    pub fn none() -> Self {
        Self::default()
    }

    /// Another candidate that is zero-cost, currently available by its
    /// provider's observed health, whose band (if read) is at least healthy,
    /// and that is adequate for the task's hard capabilities — line 1575's
    /// *"another adequate zero-cost resource is healthy"*.
    #[must_use]
    pub fn with_healthy_free_adequate(mut self, id: impl Into<String>) -> Self {
        self.healthy_free_adequate = Some(id.into());
        self
    }

    /// Another adequate, currently available candidate outside the reserve
    /// band — line 1288's *cheaper adequate resource*, in the currency Phase
    /// 32F chose for it: reserve capacity, not money. A **read** band above
    /// reserve counts; so does a zero-cost candidate, which the disposable
    /// router never has to consult at its reserve site because it tries free
    /// resources first, and which this router — with no such staging — must.
    /// An unread band on a metered candidate does not count: it might be deep
    /// in its own reserve, Phase 32F's own refusal 1. And a candidate its
    /// provider is refusing or cooling down does not count either, whatever
    /// its band: it cannot serve *instead*, which is the whole of the word.
    #[must_use]
    pub fn with_cheaper_adequate(mut self, id: impl Into<String>) -> Self {
        self.cheaper_adequate = Some(id.into());
        self
    }

    pub fn healthy_free_adequate(&self) -> Option<&str> {
        self.healthy_free_adequate.as_deref()
    }

    pub fn cheaper_adequate(&self) -> Option<&str> {
        self.cheaper_adequate.as_deref()
    }
}

/// Everything the two terms read about one destination, gathered into one
/// value so a call site cannot transpose two booleans.
#[derive(Debug, Clone, Copy)]
pub struct PressureInputs<'a> {
    /// Whether this destination costs the user at the margin —
    /// [`super::Backend::cost`] is metered. The fact that decides "premium";
    /// see the module header.
    pub premium: bool,
    /// What was read about its band and reset.
    pub facts: CapacityFacts,
    /// The task's required tier, when the caller established one. `None` is
    /// "not established", and it never earns a destination anything: the
    /// reserve gate treats it conservatively (line 1459) and the low-tier
    /// term is inert on it.
    pub tier: Option<WorkloadTier>,
    /// Whether this destination continues an existing session — read only
    /// for the explanation, so a tight warm session can say why it is kept
    /// (line 1572).
    pub existing: bool,
    /// What the router found among the other candidates.
    pub alternatives: &'a Alternatives,
    /// Both scopes' policies, and which scope this decision is for.
    pub policies: ReservePolicies,
    pub scope: ReserveScope,
    /// Whether the user overrode reserve protection for this destination —
    /// line 1290, scoped at the caller through
    /// [`super::disposable::ReserveOverride`]: true only for an existing
    /// session the user named.
    pub user_override: bool,
    /// Whether this destination's session had its current task **declared**
    /// nearly complete — lines 1294 and 1610, scoped at the caller exactly
    /// as `user_override` above is: true only for an existing session
    /// somebody declared, and only while that declaration is inside its
    /// horizon.
    ///
    /// `false` is *nothing declared*, which is what every caller that
    /// declares nothing carries and what makes this field's arrival a no-op
    /// for them. It is never derived from a turn count, an elapsed time or
    /// any other observable — see
    /// [`crate::provider::quota::ReserveDecisionInputs::task_nearly_complete`]
    /// for why a proxy inverts the reserve policy rather than approximating
    /// it.
    pub task_nearly_complete: bool,
    /// Phase 32E line 1280: what [`super::burn::forecast`] made of this
    /// destination's resource, when it could make anything of it at all.
    ///
    /// `None` is *insufficiently known* and it is the value every caller
    /// that reads no ledger carries — [`exhaustion_forecast_pressure`] is
    /// inert on it and says so, which is what keeps a ranking with no
    /// forecast byte-for-byte what it was. See
    /// [`super::burn::forecast`]'s own doc for the four ways it answers
    /// `None`, none of which is a number.
    pub forecast: Option<super::burn::ExhaustionForecast>,
}

// ---------------------------------------------------------------------------
// The two contributions.
// ---------------------------------------------------------------------------

const BAND_TERM: &str = "capacity band";
const LOW_TIER_TERM: &str = "low-tier spend";
const FORECAST_TERM: &str = "exhaustion forecast";

/// Lines 1570, 1571, 1573, 1574 and 1577: what this destination's capacity
/// band, its reset, and the scope's reserve policy contribute.
///
/// - not premium, or nothing read → `0.0`, named inert;
/// - plenty or healthy → `0.0`, nothing to conserve;
/// - tight → [`TIGHT_BAND_PENALTY`], relieved by [`reset_relief`];
/// - reserve or exhausted → under a `spend` policy, the tight penalty; under
///   `protect`, [`reserve_verdict`]: an allowed spend costs what tightness
///   costs and says why it was allowed, a denied one costs
///   [`RESERVE_DENIED_PENALTY`] and says why it was denied.
pub fn capacity_band_pressure(inputs: &PressureInputs<'_>) -> Contribution {
    if !inputs.premium {
        return Contribution::new(
            BAND_TERM,
            0.0,
            "inert: a zero-cost resource has no premium capacity to conserve",
        );
    }
    let Some(band) = inputs.facts.band() else {
        return Contribution::new(
            BAND_TERM,
            0.0,
            "inert: no capacity reading has been cached for this destination's provider, so \
             its band is unknown — neither preferred nor withheld",
        );
    };
    match band {
        CapacityBand::Plenty | CapacityBand::Healthy => Contribution::new(
            BAND_TERM,
            0.0,
            format!("in the {band} band — nothing to conserve"),
        ),
        CapacityBand::Tight => tight_penalty(inputs, band, None),
        CapacityBand::Reserve | CapacityBand::Exhausted => reserve_band(inputs, band),
    }
}

/// Line 1280: what a forecast of exhaustion before the next reset
/// contributes.
///
/// `super::burn::forecast` answers `None` for a resource with too few rows,
/// no measured request-unit remaining amount, no known reset, or a zero
/// burn rate — the four ways line 1278's *"sufficiently known"* is not
/// met — and every one contributes exactly `0.0`, keeping a ranking with no
/// forecast identical to what it was before this module existed
/// (`a_destination_with_no_forecast_ranks_exactly_as_it_did`).
///
/// [`super::burn::WELL_BEFORE_RESET_FRACTION`] exists because a forecast
/// landing just short of the reset is inside the estimator's own tolerance,
/// and reacting to it would be line 1281's overreaction wearing a different
/// hat: a resource forecast to exhaust after the reset, or in the last half
/// of the window before it, contributes `0.0` and names the figures it was
/// comparing.
///
/// The evidence text says *estimated* and *at the current rate*, never
/// *will* — line 1283's restraint, same as `crate::shell`'s capacity line.
// History: design-decisions.md, "Trims: routing module docs", routing/pressure.rs `fn exhaustion_forecast_pressure`.
pub fn exhaustion_forecast_pressure(inputs: &PressureInputs<'_>) -> Contribution {
    let Some(forecast) = inputs.forecast else {
        return Contribution::new(
            FORECAST_TERM,
            0.0,
            "inert: no exhaustion forecast is sufficiently known for this resource",
        );
    };
    let hours = forecast.seconds_to_exhaustion as f64 / 3600.0;
    if !forecast.exhausts_well_before_reset() {
        let reset_note = match forecast.seconds_until_reset {
            Some(reset) => format!(", and its reset is {:.1}h away", reset as f64 / 3600.0),
            None => String::new(),
        };
        return Contribution::new(
            FORECAST_TERM,
            0.0,
            format!(
                "inert: estimated to last about {hours:.1}h at the current rate of \
                 {:.1} requests/hour{reset_note} — not forecast to exhaust well before \
                 its reset",
                forecast.requests_per_hour
            ),
        );
    }
    let reset = forecast
        .seconds_until_reset
        .expect("`exhausts_well_before_reset` is false without a reset");
    Contribution::new(
        FORECAST_TERM,
        EXHAUSTION_FORECAST_PENALTY,
        format!(
            "estimated to last about {hours:.1}h at the current rate of {:.1} requests/hour \
             over {} observations, and its reset is {:.1}h away — it may not reach that \
             reset, so this resource is preferred less",
            forecast.requests_per_hour,
            forecast.rows,
            reset as f64 / 3600.0
        ),
    )
}

/// Line 1575: what spending this destination on low-tier work contributes
/// while a healthy zero-cost alternative could do it.
///
/// Every condition the line names is a separate inert arm with its own
/// reason, so a reader can tell "the task was not low tier" from "no free
/// alternative was available" from "nothing was read".
pub fn low_tier_spend(inputs: &PressureInputs<'_>) -> Contribution {
    if !inputs.premium {
        return Contribution::new(
            LOW_TIER_TERM,
            0.0,
            "inert: a zero-cost resource is what line 1575 prefers, not what it withholds",
        );
    }
    let Some(tier) = inputs.tier else {
        return Contribution::new(
            LOW_TIER_TERM,
            0.0,
            "inert: the task's tier is not established, so no low-tier claim can be made \
             about it",
        );
    };
    if tier > LOW_TIER_CEILING {
        return Contribution::new(
            LOW_TIER_TERM,
            0.0,
            format!(
                "inert: the task is {tier}-tier, above the {LOW_TIER_CEILING} ceiling this term \
                 applies to"
            ),
        );
    }
    let Some(band) = inputs.facts.band() else {
        return Contribution::new(
            LOW_TIER_TERM,
            0.0,
            "inert: no capacity reading has been cached for this destination's provider, so \
             whether it is under pressure is unknown",
        );
    };
    if band > CapacityBand::Tight {
        return Contribution::new(
            LOW_TIER_TERM,
            0.0,
            format!(
                "inert: in the {band} band — not under pressure, so low-tier work exhausts \
                 nothing here"
            ),
        );
    }
    let Some(alternative) = inputs.alternatives.healthy_free_adequate() else {
        return Contribution::new(
            LOW_TIER_TERM,
            0.0,
            format!(
                "inert: a {tier}-tier task and the {band} band, but no healthy zero-cost \
                 candidate adequate for the task is available to take it"
            ),
        );
    };
    Contribution::new(
        LOW_TIER_TERM,
        LOW_TIER_SPEND_PENALTY,
        format!(
            "a {tier}-tier task, and this destination is in the {band} band while `{alternative}` \
             is a healthy zero-cost resource adequate for it — premium capacity is not spent on \
             work a free resource can do (line 1575)"
        ),
    )
}

/// How much of the tight penalty a known reset waives, `0.0` (none) to `1.0`
/// (all) — lines 1573 and 1574 as one number.
///
/// `None` is `0.0`: no reset known, no relief, which is the identity
/// [`crate::provider::quota::RemainingCapacityScore::effective`] also keeps
/// for an unknown reset rather than fabricating one. A reset already past
/// (`<= 0`) is full relief, not an error — a window that just turned is the
/// clearest reason to stop conserving, and this module has no clock with
/// which to have noticed the turn any sooner.
pub fn reset_relief(seconds_until_reset: Option<i64>) -> f64 {
    let Some(seconds) = seconds_until_reset else {
        return 0.0;
    };
    if seconds <= RESET_RELIEF_HORIZON_SECONDS {
        1.0
    } else if seconds >= RESET_RELIEF_FADE_SECONDS {
        0.0
    } else {
        1.0 - (seconds - RESET_RELIEF_HORIZON_SECONDS) as f64
            / (RESET_RELIEF_FADE_SECONDS - RESET_RELIEF_HORIZON_SECONDS) as f64
    }
}

/// The tight-band arm, also reached by an admitted reserve spend and by a
/// `spend` policy: [`TIGHT_BAND_PENALTY`] scaled down by [`reset_relief`],
/// with the evidence saying which of the three it was and what the reset did.
fn tight_penalty(
    inputs: &PressureInputs<'_>,
    band: CapacityBand,
    admitted: Option<&str>,
) -> Contribution {
    let relief = reset_relief(inputs.facts.seconds_until_reset());
    let magnitude = TIGHT_BAND_PENALTY * (1.0 - relief);

    let mut evidence = format!("in the {band} band");
    match inputs.facts.seconds_until_reset() {
        None => evidence
            .push_str(" with no reset known, so the full conservation penalty applies (line 1570)"),
        Some(seconds) if relief >= 1.0 => {
            let _ = write_fmt(
                &mut evidence,
                format_args!(
                    ", resetting in {seconds}s — within the {RESET_RELIEF_HORIZON_SECONDS}s relief \
                     horizon, so the conservation penalty is waived: capacity that would expire \
                     unused is spent freely (lines 1573, 1574)"
                ),
            );
        }
        Some(seconds) if relief > 0.0 => {
            let _ = write_fmt(
                &mut evidence,
                format_args!(
                    ", resetting in {seconds}s, so the conservation penalty is reduced by \
                     {:.0}% toward the {RESET_RELIEF_HORIZON_SECONDS}s horizon (line 1573)",
                    relief * 100.0
                ),
            );
        }
        Some(seconds) => {
            let _ = write_fmt(
                &mut evidence,
                format_args!(
                    ", resetting in {seconds}s — beyond the {RESET_RELIEF_FADE_SECONDS}s fade, so \
                     the full conservation penalty applies (line 1570)"
                ),
            );
        }
    }
    if let Some(reason) = admitted {
        let _ = write_fmt(
            &mut evidence,
            format_args!(
                "; the {} reserve policy is `{}` and the spend is admitted — {reason}",
                inputs.scope,
                inputs.policies.for_scope(inputs.scope)
            ),
        );
    }
    if inputs.existing {
        let _ = write_fmt(
            &mut evidence,
            format_args!(
                "; this continues an existing session, and band pressure alone ({TIGHT_BAND_PENALTY:+.2} \
                 at most) is worth less than its warmth, so tightness by itself does not move \
                 the work (line 1572)"
            ),
        );
    }
    Contribution::new(BAND_TERM, magnitude, evidence)
}

/// The reserve-band arm: the scope's policy, then Phase 32F's verdict.
fn reserve_band(inputs: &PressureInputs<'_>, band: CapacityBand) -> Contribution {
    let policy = inputs.policies.for_scope(inputs.scope);
    match policy {
        ReservePolicy::Spend => tight_penalty(
            inputs,
            band,
            Some("this scope's work may spend the reserve (line 1577)"),
        ),
        ReservePolicy::Protect => {
            let verdict = reserve_verdict(
                band,
                inputs.tier,
                inputs.alternatives.cheaper_adequate().is_some(),
                inputs.user_override,
                inputs.facts.seconds_until_reset(),
                inputs.task_nearly_complete,
            );
            match verdict {
                ReserveDecision::Allow { reason } => tight_penalty(inputs, band, Some(&reason)),
                ReserveDecision::Deny { reason } => Contribution::new(
                    BAND_TERM,
                    RESERVE_DENIED_PENALTY,
                    format!(
                        "in the {band} band, and the {} reserve policy is `{policy}`, which denies \
                         the spend — {reason}{} (lines 1571, 1577)",
                        inputs.scope,
                        match inputs.alternatives.cheaper_adequate() {
                            Some(id) => format!("; `{id}` is the cheaper adequate alternative"),
                            None => String::new(),
                        }
                    ),
                ),
            }
        }
    }
}

/// Whether a reserve-band spend is justified — Phase 32F's own
/// [`evaluate_reserve_spend`] when the task's tier is established, and its
/// precedence with every tier-dependent branch taken **conservatively** when
/// it is not.
///
/// Line 1459's conservative-on-low-confidence rule extends to an absent
/// tier: the reserve exists *for* high-tier work (line 1571), so an
/// unestablished tier is not admitted on the tier branch — the same outcome
/// the lowest tier gets, with a reason naming the tier unknown rather than
/// claiming the task doesn't need the heavy tier. Every other branch (the
/// user's override, an imminent reset, no cheaper adequate resource) admits
/// the spend regardless of tier, same as Phase 32F. A test holds the
/// unknown-tier verdict equal to the lowest tier's across every input.
///
/// Line 1610 (*"avoid migrating a nearly completed task solely to preserve
/// quota"*) is line 1294's guard from Phase 38, turning on **solely**: a
/// second reason may only come from the party that knows, so
/// `task_nearly_complete` is a **declaration** carried in by the caller
/// through `PressureInputs::task_nearly_complete`, never inferred here —
/// `design-decisions.md`'s *"A task is never 'nearly complete'"* on its own,
/// since a turn-count or elapsed-time proxy would invert the protection.
// History: design-decisions.md, "Trims: routing module docs", routing/pressure.rs `fn reserve_verdict`.
pub fn reserve_verdict(
    band: CapacityBand,
    tier: Option<WorkloadTier>,
    cheaper_adequate_resource_exists: bool,
    user_override: bool,
    seconds_until_reset: Option<i64>,
    task_nearly_complete: bool,
) -> ReserveDecision {
    if let Some(tier) = tier {
        return evaluate_reserve_spend(ReserveDecisionInputs {
            band,
            tier,
            cheaper_adequate_resource_exists,
            user_override,
            seconds_until_reset,
            // Lines 1294 and 1610, declared by the caller and never derived
            // here — see above.
            task_nearly_complete,
        });
    }

    // The unknown-tier copy below reproduces `evaluate_reserve_spend`'s
    // precedence, so the declaration leads here too. It has to: this guard
    // outranks every other signal precisely because it is about work already
    // in flight, and whether the router managed to establish that work's
    // tier says nothing about whether somebody declared it nearly done. A
    // copy that started at the override would quietly withhold the
    // protection from every task whose tier was not classified — line
    // 1459's conservatism applied to the one branch it was never about.
    if task_nearly_complete {
        return ReserveDecision::Allow {
            reason: "this session's operator declared its current task nearly complete, so the \
                     reserve threshold is not the sole reason to move the work (lines 1294, \
                     1610)"
                .to_owned(),
        };
    }

    if user_override {
        return ReserveDecision::Allow {
            reason: "the user explicitly overrode reserve protection for this session (line \
                     1290)"
                .to_owned(),
        };
    }
    if band > CapacityBand::Reserve {
        return ReserveDecision::Allow {
            reason: format!(
                "the resource is in the {band} band, which has not crossed into its protected \
                 reserve"
            ),
        };
    }
    if let Some(seconds) = seconds_until_reset {
        if seconds <= RESET_IMMINENT_SECONDS {
            return ReserveDecision::Allow {
                reason: format!(
                    "the quota resets in {seconds}s, within the reserve policy's imminent \
                     window; conserving now buys little (line 1291)"
                ),
            };
        }
        if seconds >= RESET_DISTANT_SECONDS {
            return ReserveDecision::Deny {
                reason: format!(
                    "the next reset is {seconds}s away, past the reserve policy's distant \
                     threshold, and the task's tier is not established — not known to be \
                     heavy-tier work, so treated as not justifying the reserve (lines 1292, \
                     1459)"
                ),
            };
        }
    }
    if cheaper_adequate_resource_exists {
        return ReserveDecision::Deny {
            reason: "a cheaper adequate resource exists and the task's tier is not established \
                     — not known to require the heavy tier, so protected reserve is not spent \
                     on it (lines 1288, 1459)"
                .to_owned(),
        };
    }
    ReserveDecision::Allow {
        reason: "no cheaper adequate resource exists, so spending protected reserve is the \
                 least-bad option available"
            .to_owned(),
    }
}

/// `String::write_fmt` without importing the trait at every call site.
fn write_fmt(out: &mut String, args: fmt::Arguments<'_>) -> fmt::Result {
    use fmt::Write as _;
    out.write_fmt(args)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn inputs<'a>(
        facts: CapacityFacts,
        tier: Option<WorkloadTier>,
        alternatives: &'a Alternatives,
    ) -> PressureInputs<'a> {
        PressureInputs {
            premium: true,
            facts,
            tier,
            existing: false,
            alternatives,
            policies: ReservePolicies::default(),
            scope: ReserveScope::Interactive,
            user_override: false,
            task_nearly_complete: false,
            forecast: None,
        }
    }

    // --- reset relief -------------------------------------------------------

    #[test]
    fn relief_is_full_within_the_horizon_none_beyond_the_fade_and_linear_between() {
        assert_eq!(reset_relief(None), 0.0);
        assert_eq!(reset_relief(Some(-5)), 1.0);
        assert_eq!(reset_relief(Some(RESET_RELIEF_HORIZON_SECONDS)), 1.0);
        assert_eq!(reset_relief(Some(RESET_RELIEF_FADE_SECONDS)), 0.0);
        let midpoint = (RESET_RELIEF_HORIZON_SECONDS + RESET_RELIEF_FADE_SECONDS) / 2;
        let relief = reset_relief(Some(midpoint));
        assert!((relief - 0.5).abs() < 1e-9, "midpoint relief {relief}");
        assert!(reset_relief(Some(600)) > reset_relief(Some(1800)));
    }

    // --- the unknown-tier copy of the precedence cannot drift ---------------

    /// `reserve_verdict(.., None, ..)` mirrors `evaluate_reserve_spend` with
    /// the lowest tier on every input this module can hand it. Only the
    /// reasons differ, and they are meant to.
    ///
    /// **The declared-task-progress axis is in the sweep for a reason.** The
    /// unknown-tier arm is a hand-written copy of the precedence, so lines
    /// 1294 and 1610's guard has to be re-implemented in it; a copy that
    /// omitted the branch would withhold the protection from exactly the
    /// tasks whose tier the router could not establish, and nothing else in
    /// the suite compares the two arms input for input.
    #[test]
    fn an_unknown_tier_decides_exactly_as_the_lowest_tier_would() {
        for band in [
            CapacityBand::Exhausted,
            CapacityBand::Reserve,
            CapacityBand::Tight,
            CapacityBand::Plenty,
        ] {
            for cheaper in [false, true] {
                for user_override in [false, true] {
                    for declared in [false, true] {
                        for reset in [None, Some(0), Some(60), Some(1800), Some(7200)] {
                            let unknown = reserve_verdict(
                                band,
                                None,
                                cheaper,
                                user_override,
                                reset,
                                declared,
                            );
                            let lowest = evaluate_reserve_spend(ReserveDecisionInputs {
                                band,
                                tier: WorkloadTier::Deterministic,
                                cheaper_adequate_resource_exists: cheaper,
                                user_override,
                                seconds_until_reset: reset,
                                task_nearly_complete: declared,
                            });
                            assert_eq!(
                                unknown.is_allowed(),
                                lowest.is_allowed(),
                                "band {band}, cheaper {cheaper}, override {user_override}, \
                                 declared {declared}, reset {reset:?}: unknown said \
                                 {unknown:?}, lowest said {lowest:?}"
                            );
                        }
                    }
                }
            }
        }
    }

    /// The unknown-tier arm's own words, so a caller reading the reason sees
    /// that the second reason was a **declaration** and not something
    /// Glasshouse worked out — lines 1294 and 1610 both turn on *solely*.
    #[test]
    fn a_declared_task_is_kept_at_an_unknown_tier_and_the_reason_says_who_said_so() {
        let verdict = reserve_verdict(CapacityBand::Reserve, None, true, false, Some(7200), true);
        assert!(verdict.is_allowed(), "{}", verdict.reason());
        assert!(
            verdict.reason().contains("declared"),
            "the reason must name the declaration: {}",
            verdict.reason()
        );
        // Without the declaration the same inputs are a denial, so the
        // assertion above is about the declaration and not about the band.
        let undeclared =
            reserve_verdict(CapacityBand::Reserve, None, true, false, Some(7200), false);
        assert!(!undeclared.is_allowed(), "{}", undeclared.reason());
    }

    #[test]
    fn an_unknown_tier_is_named_in_the_denial_rather_than_called_light() {
        let verdict = reserve_verdict(CapacityBand::Reserve, None, true, false, None, false);
        assert!(!verdict.is_allowed());
        assert!(
            verdict.reason().contains("not established"),
            "{}",
            verdict.reason()
        );
        assert!(
            !verdict.reason().contains("does not require"),
            "{}",
            verdict.reason()
        );
    }

    // --- the two terms, at the pure level -----------------------------------

    #[test]
    fn a_zero_cost_destination_is_inert_for_both_terms_and_says_so() {
        let alternatives = Alternatives::none();
        let mut inputs = inputs(
            CapacityFacts::new(Some(CapacityBand::Reserve), Some(10)),
            Some(WorkloadTier::Leaf),
            &alternatives,
        );
        inputs.premium = false;
        for term in [capacity_band_pressure(&inputs), low_tier_spend(&inputs)] {
            assert_eq!(term.magnitude(), 0.0, "{}", term.evidence());
            assert!(term.evidence().starts_with("inert"), "{}", term.evidence());
        }
    }

    #[test]
    fn a_spend_policy_costs_the_reserve_band_only_what_tightness_costs() {
        let alternatives = Alternatives::none().with_cheaper_adequate("other");
        let mut inputs = inputs(
            CapacityFacts::new(Some(CapacityBand::Reserve), None),
            Some(WorkloadTier::Standard),
            &alternatives,
        );
        inputs.policies = ReservePolicies {
            interactive: ReservePolicy::Spend,
            background: ReservePolicy::Protect,
        };
        let interactive = capacity_band_pressure(&inputs);
        assert_eq!(interactive.magnitude(), TIGHT_BAND_PENALTY);
        assert!(
            interactive.evidence().contains("`spend`"),
            "{}",
            interactive.evidence()
        );

        inputs.scope = ReserveScope::Background;
        let background = capacity_band_pressure(&inputs);
        assert_eq!(background.magnitude(), RESERVE_DENIED_PENALTY);
        assert!(
            background
                .evidence()
                .contains("background reserve policy is `protect`"),
            "{}",
            background.evidence()
        );
    }

    #[test]
    fn the_admitted_reserve_spend_carries_its_reason_and_the_tight_shaped_penalty() {
        let alternatives = Alternatives::none().with_cheaper_adequate("other");
        let inputs = inputs(
            CapacityFacts::new(Some(CapacityBand::Reserve), None),
            Some(WorkloadTier::Heavy),
            &alternatives,
        );
        let term = capacity_band_pressure(&inputs);
        assert_eq!(term.magnitude(), TIGHT_BAND_PENALTY);
        assert!(term.evidence().contains("admitted"), "{}", term.evidence());
        assert!(term.evidence().contains("line 1289"), "{}", term.evidence());
    }

    #[test]
    fn the_user_override_admits_a_reserve_spend_for_the_named_session() {
        let alternatives = Alternatives::none().with_cheaper_adequate("other");
        let mut inputs = inputs(
            CapacityFacts::new(Some(CapacityBand::Reserve), None),
            Some(WorkloadTier::Leaf),
            &alternatives,
        );
        assert_eq!(
            capacity_band_pressure(&inputs).magnitude(),
            RESERVE_DENIED_PENALTY
        );
        inputs.user_override = true;
        let term = capacity_band_pressure(&inputs);
        assert_eq!(term.magnitude(), TIGHT_BAND_PENALTY);
        assert!(term.evidence().contains("line 1290"), "{}", term.evidence());
    }

    #[test]
    fn the_low_tier_term_names_which_condition_left_it_inert() {
        let free = Alternatives::none().with_healthy_free_adequate("free");
        let none = Alternatives::none();
        let tight = CapacityFacts::new(Some(CapacityBand::Tight), None);

        let live = low_tier_spend(&inputs(tight, Some(WorkloadTier::Leaf), &free));
        assert_eq!(live.magnitude(), LOW_TIER_SPEND_PENALTY);
        assert!(live.evidence().contains("`free`"), "{}", live.evidence());

        let cases = [
            (inputs(tight, None, &free), "not established"),
            (
                inputs(tight, Some(WorkloadTier::Standard), &free),
                "above the leaf ceiling",
            ),
            (
                inputs(CapacityFacts::UNREAD, Some(WorkloadTier::Leaf), &free),
                "no capacity reading",
            ),
            (
                inputs(
                    CapacityFacts::new(Some(CapacityBand::Healthy), None),
                    Some(WorkloadTier::Leaf),
                    &free,
                ),
                "not under pressure",
            ),
            (
                inputs(tight, Some(WorkloadTier::Leaf), &none),
                "no healthy zero-cost",
            ),
        ];
        for (inputs, expected) in cases {
            let term = low_tier_spend(&inputs);
            assert_eq!(term.magnitude(), 0.0, "{}", term.evidence());
            assert!(term.evidence().starts_with("inert"), "{}", term.evidence());
            assert!(term.evidence().contains(expected), "{}", term.evidence());
        }
    }
}
