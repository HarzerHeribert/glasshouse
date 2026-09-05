//! Reserve and pressure handling: entitlement capacity, throttling, tier
//! movement and the pool fallback a decision falls back across.

use super::discovery::{is_adequate, provider_available};
use super::*;

/// Line 1598: what is known about this destination's remaining quota.
///
/// `None` contributes exactly `0.0` and says so. That is not "assume full"
/// and not "assume empty": an unread resource is neither preferred nor
/// withheld, which is the same stance `glasshouse resources` takes when it
/// prints `unknown` rather than a number nobody read.
pub fn quota_pressure(destination: &Destination, weights: &ScoreWeights) -> Contribution {
    match destination.capacity() {
        Some(score) => Contribution::new(
            "known quota pressure",
            score.routing_fraction() * weights.quota_pressure_weight,
            format!(
                "{} remaining on `{}`, bound by {}",
                score.percent().render(),
                destination.backend().credential().label(),
                score.dimension()
            ),
        ),
        None => Contribution::new(
            "known quota pressure",
            0.0,
            format!(
                "nothing has been read about `{}`'s remaining quota — an unread resource is \
                 neither preferred nor withheld",
                destination.backend().credential().label()
            ),
        ),
    }
}

/// Line 1599: what has actually been observed about this destination's
/// provider.
///
/// Read from [`FreePool`], whose health half is keyed by credential **and**
/// model and is cost-agnostic — [`crate::routing::free::ResourceHealth`] counts
/// consecutive failures, cooldowns and credential rejections, none of which
/// is a statement about price. `crate::gateway::session`'s `observe_exchange`
/// is what puts real outcomes into it, from work that was going to happen
/// anyway, which is line 534's constraint and the reason nothing here probes.
pub fn provider_health(
    destination: &Destination,
    pool: &FreePool,
    now: Instant,
    weights: &ScoreWeights,
) -> Contribution {
    let resource = FreeResource::new(
        destination.backend().credential().clone(),
        destination.backend().model().label(),
    );
    let health = pool.health(&resource);

    if health.credential_was_rejected() {
        return Contribution::new(
            "provider health",
            weights.health_unavailable_penalty,
            format!(
                "`{}` was refused by its provider — waiting does not fix a revoked key",
                destination.backend().credential().label()
            ),
        );
    }
    if !health.is_available(now) {
        return Contribution::new(
            "provider health",
            weights.health_unavailable_penalty,
            format!(
                "`{}` is still cooling down after {} consecutive observed failures",
                destination.backend().credential().label(),
                health.consecutive_failures()
            ),
        );
    }
    let failures = health.consecutive_failures();
    if failures == 0 {
        return Contribution::new(
            "provider health",
            0.0,
            format!(
                "nothing has been observed against `{}` on `{}` — not a health claim, the \
                 absence of one",
                destination.backend().model().label(),
                destination.backend().credential().label()
            ),
        );
    }
    Contribution::new(
        "provider health",
        (f64::from(failures) * weights.health_failure_penalty).max(weights.health_penalty_floor),
        format!(
            "{failures} consecutive observed failures on `{}` that have not yet earned a \
             cooldown",
            destination.backend().credential().label()
        ),
    )
}

/// Line 1546: current cadence availability, scored separately from
/// [`provider_health`]'s general route health.
///
/// `provider_health` folds together a provider-declared wait and a cooldown
/// Glasshouse invented after repeated ordinary failures (Phase 9I line 534
/// deliberately keeps the invented kind probeable by real work); this term
/// reads only [`crate::routing::free::ResourceHealth::declared_wait_remaining`], which
/// is `None` for every case except a wait the destination's own provider is
/// currently inside. An invented cooldown, or a resource nothing has ever
/// been observed about, scores exactly the same here: inert.
pub fn cadence_availability(
    destination: &Destination,
    pool: &FreePool,
    now: Instant,
) -> Contribution {
    let resource = FreeResource::new(
        destination.backend().credential().clone(),
        destination.backend().model().label(),
    );
    let health = pool.health(&resource);

    // Capability map line 1366: which cadence, if any, this destination's
    // pool is holding, and where it came from — a stated window always wins
    // and a learned one names its own sample, so a reader can tell the two
    // apart rather than trusting a number with no provenance.
    let window_provenance = match pool.allowance(destination.backend().credential()) {
        Allowance::RequestPool {
            window: Some(Window::Stated { seconds }),
            ..
        } => format!("; window stated by the provider ({seconds}s)"),
        Allowance::RequestPool {
            window: Some(Window::Learned { seconds, sample }),
            ..
        } => format!("; window learned from {sample} throttles ({seconds}s)"),
        _ => String::new(),
    };

    match health.declared_wait_remaining(now) {
        Some(remaining) => Contribution::new(
            "cadence availability",
            CADENCE_DECLARED_WAIT_PENALTY,
            format!(
                "`{}` is inside a {}s wait its own provider declared{window_provenance}",
                destination.backend().credential().label(),
                remaining.as_secs()
            ),
        ),
        None => Contribution::new(
            "cadence availability",
            0.0,
            format!(
                "no provider-declared wait is in effect for `{}` — not a cadence claim, the \
                 absence of one{window_provenance}",
                destination.backend().credential().label()
            ),
        ),
    }
}

/// Line 1600: what it would cost to send the work here.
///
/// Two costs, added, because they are genuinely separate and a destination
/// can carry both:
///
/// - **bootstrap** — a fresh session must re-establish what a warm one
///   already holds. Line 1594's *"a good checkpoint exists"* is priced right
///   here: a good checkpoint carries the objective, the state and the next
///   actions, so it cuts the cost rather than removing it, because it does
///   not carry the conversation.
/// - **switching** — moving work off the harness or provider it is on now.
///   Harness costs more than provider: the harness holds the tools, the
///   permissions and the transcript.
///
/// Deliberately distinct from [`prompt_cache_state`], which prices the
/// *provider's* cache. This one prices what Glasshouse and the person have to
/// redo, and the two are different quantities that a single "switching cost"
/// term would have averaged into one unreadable number.
pub fn switching_and_bootstrap_cost(
    destination: &Destination,
    current: Option<&Destination>,
) -> Contribution {
    let mut magnitude = 0.0;
    let mut notes: Vec<String> = Vec::new();

    match destination.continuation() {
        Continuation::Fresh(Some(checkpoint)) if checkpoint.is_good() => {
            magnitude += BOOTSTRAP_COST_WITH_CHECKPOINT;
            notes.push(
                "a fresh session, booting from a good checkpoint — it carries the objective, the \
                 state and the next actions, and not the conversation"
                    .to_owned(),
            );
        }
        Continuation::Fresh(Some(checkpoint)) => {
            magnitude += BOOTSTRAP_COST;
            notes.push(format!(
                "a fresh session, and the available checkpoint is not a good one ({}{})",
                if checkpoint.has_next_actions() {
                    "next actions present"
                } else {
                    "no next actions"
                },
                if checkpoint.complete() {
                    ""
                } else {
                    ", and it was trimmed to fit"
                }
            ));
        }
        Continuation::Fresh(None) => {
            magnitude += BOOTSTRAP_COST;
            notes.push(
                "a fresh session with no checkpoint to boot from — its first turn is spent \
                 re-establishing context"
                    .to_owned(),
            );
        }
        Continuation::Existing(_) => {
            notes.push("an existing session pays no bootstrap cost".to_owned());
        }
    }

    if let Some(current) = current {
        if current.harness() != destination.harness() {
            magnitude += SWITCH_HARNESS_COST;
            notes.push(format!(
                "moving from `{}` to `{}` changes the harness, which holds the tools, the \
                 permissions and the transcript",
                current.harness().slug(),
                destination.harness().slug()
            ));
        } else if current.backend().provider() != destination.backend().provider() {
            magnitude += SWITCH_PROVIDER_COST;
            notes.push(format!(
                "moving from `{}` to `{}` changes the provider",
                current.backend().provider(),
                destination.backend().provider()
            ));
        } else {
            notes.push("no harness or provider change".to_owned());
        }
    }

    Contribution::new("switching and bootstrap cost", magnitude, notes.join("; "))
}

// ---------------------------------------------------------------------------
// Phase 56A step 3, lines 1953 and 1966-1969 — the entitlement pool's own
// terms: the pool enters the candidate set, and the score chooses among map
// line 1966's five factors (capacity band, time to reset, recent throttling,
// session affinity, model availability). Affinity is deliberately not a new
// term — it **is** [`session_affinity`], since a second number for the same
// warmth fact would be the double-count this module refuses everywhere;
// stickiness (line 1968) is that term's weight, not a second mechanism.
//
// An unknown facet contributes nothing and says so, and the terms are live
// only when the candidate set offers a choice of configured entitlements
// ([`EntitlementPoolView`], two or more) — a user with zero or one keeps a
// ranking byte-for-byte identical to today's, enforced structurally.
// ---------------------------------------------------------------------------
// History: design-decisions.md, "Trims: routing module docs", routing/session/reserve.rs entitlement-pool terms block.

/// Line 1966's capacity factor, by band. Plenty earns the most, and the
/// magnitudes are graded so that one band's step (0.15) is a *slight* lead —
/// the lead the distribution rule (line 1968) must be able to overcome —
/// while the drop into the reserve band is priced like the protected thing
/// it is.
const ENTITLEMENT_CAPACITY_PLENTY: f64 = 0.3;
const ENTITLEMENT_CAPACITY_HEALTHY: f64 = 0.15;
const ENTITLEMENT_CAPACITY_TIGHT: f64 = -0.15;
const ENTITLEMENT_CAPACITY_RESERVE: f64 = -0.5;

/// An entitlement whose band reads **exhausted** — line 1968's one capacity
/// case that must move even a warm session. Sized against this module's own
/// scale: it must clear a live zero-idle session's warmth (`1.5`), the hot
/// prompt-cache and intact-context facets a short-idle session can carry
/// (`0.4 + 0.3`), and the cold bootstrap the fresh sibling pays (`1.0`) —
/// `3.2` in all — because an account with nothing left cannot serve the next
/// turn, and staying warm on a resource that cannot answer is not warmth. It
/// deliberately does **not** also clear the task-identity facets (same task
/// `0.5` + touched files `0.6`): a session demonstrably mid-task on the
/// named files is not yanked by a band reading alone.
const ENTITLEMENT_EXHAUSTED_PENALTY: f64 = -3.5;

/// Line 1968's distribution half: what each **live** session already charged
/// to the same entitlement (elsewhere in this candidate set) costs a
/// candidate. One step must outweigh a *slight* capacity lead — one band
/// (`0.15`) — so two simultaneous fresh choices do not both land on the
/// entitlement that was marginally ahead, and must not outweigh a full
/// plenty-to-tight gap, so real capacity still decides when the pool is
/// genuinely uneven.
const ENTITLEMENT_IN_FLIGHT_LOAD: f64 = -0.2;

/// Line 1967, the burn half: what a low-band entitlement whose reset is
/// inside [`RESET_BURN_HORIZON_SECONDS`] earns. The user's instruction of
/// record (design-decisions §56A): *"A at 12% resetting in 1h20m and B at
/// 61% resetting in 4d ⇒ burn A"* — capacity that would expire unused is the
/// cheapest capacity there is, so the full weight of the strongest
/// established preference on this module's scale (`1.0`), enough to overturn
/// the reserve-band's own capacity penalty (`-0.5`) plus a healthy sibling's
/// lead (`+0.15`).
const RESET_BURN: f64 = 1.0;

/// Line 1967, the preserve half: what a low-band entitlement whose reset is
/// at or past [`RESET_PRESERVE_HORIZON_SECONDS`] pays — *"A at 12% resetting
/// in 4d ⇒ preserve A, route B"*. Smaller than the burn half on purpose: the
/// capacity factor already penalises the low band once, and this term adds
/// the map's own extra reason to look elsewhere, not a second copy of the
/// band.
const RESET_PRESERVE: f64 = -0.4;

/// Line 1967's "near". Two hours — comfortably containing the user's own
/// 1h20m example, and, on the scale of the multi-hour subscription windows
/// the 56A instruction describes, a remainder with under two hours to live
/// cannot plausibly be banked for later work. Deliberately **not**
/// `pressure::RESET_RELIEF_HORIZON_SECONDS` (300s): that figure prices an
/// API window's imminence, and reusing it here would make the user's own
/// example read as "distant".
pub(super) const RESET_BURN_HORIZON_SECONDS: i64 = 2 * 3600;

/// Line 1967's "far": a day or more away is fully "preserve" — the user's
/// four-day example with a wide margin — and between the two horizons the
/// term fades linearly, so a reset crossing either boundary does not jump.
pub(super) const RESET_PRESERVE_HORIZON_SECONDS: i64 = 24 * 3600;

/// Line 1966's throttling factor: per informative throttle recorded against
/// this entitlement in the evidence window, bounded. The same shape and
/// scale as [`HEALTH_FAILURE_PENALTY`], because both read observed
/// misbehaviour of the serving resource; account-scoped where 56A-2's
/// narrowing could honestly narrow it, and the evidence sentence says whose
/// count it is either way.
const ENTITLEMENT_THROTTLE_PENALTY: f64 = -0.2;
const ENTITLEMENT_THROTTLE_FLOOR: f64 = -0.6;

/// Line 1966's model-availability factor: the entitlement's declared
/// catalogue names the model this destination would serve — an established
/// fact, on the same scale as [`TIER_FIT_HEADROOM`]. The negative case is
/// not priced here at all: a declared list that does *not* name the model is
/// [`crate::routing::Entitlement::model_constraint`]'s hard refusal, and such a
/// candidate never reaches the score.
const ENTITLEMENT_MODEL_AVAILABLE: f64 = 0.2;

/// What the candidate set offers along the entitlement axis — the two
/// set-level facts the pool terms read, computed once per `choose` from the
/// eligible candidates because only the router holds the set (the same
/// reason the private `alternatives_for` helper lives here).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct EntitlementPoolView {
    /// Distinct **configured** entitlement names among the candidates — the
    /// pool gate. The synthesised harness-default entries do not count
    /// toward the gate (a user who configured nothing has no pool), but a
    /// candidate carrying one is still scored once the gate is open: the
    /// map's own pool names native sign-ins as members.
    configured: BTreeSet<String>,
    /// Live sessions per entitlement name, by destination id — line 1968's
    /// in-flight load, so a candidate can ask "who else is drawing on my
    /// account right now" without counting itself.
    live: BTreeMap<String, Vec<String>>,
}

impl EntitlementPoolView {
    /// Read the axis off a candidate set.
    pub fn of(destinations: &[Destination]) -> Self {
        let mut view = Self::default();
        for destination in destinations {
            let Some(entitlement) = destination.entitlement() else {
                continue;
            };
            if entitlement.is_configured() {
                view.configured.insert(entitlement.name().to_owned());
            }
            if let Continuation::Existing(warm) = destination.continuation()
                && warm.state == crate::config::pairing::WarmSessionState::Live
            {
                view.live
                    .entry(entitlement.name().to_owned())
                    .or_default()
                    .push(destination.id().to_owned());
            }
        }
        view
    }

    /// Whether the set carries two or more distinct configured entitlements
    /// — the gate every pool term checks first.
    pub fn offers_a_choice(&self) -> bool {
        self.configured.len() >= 2
    }

    pub fn configured_count(&self) -> usize {
        self.configured.len()
    }

    /// How many **live** sessions other than `excluding_id` are charged to
    /// `entitlement` in this candidate set.
    pub fn live_sessions_elsewhere(&self, entitlement: &str, excluding_id: &str) -> usize {
        self.live
            .get(entitlement)
            .map(|ids| ids.iter().filter(|id| *id != excluding_id).count())
            .unwrap_or(0)
    }

    /// The inert sentence every pool term renders when the gate is closed —
    /// one wording, so the explanation cannot say two different things about
    /// one fact.
    fn no_choice_evidence(&self) -> String {
        format!(
            "inert: the candidate set carries {} configured entitlement{} — the pool axis \
             separates nothing until there are two (lines 1953, 1966)",
            self.configured.len(),
            if self.configured.len() == 1 { "" } else { "s" },
        )
    }
}

/// The two gates every pool term shares: the set must offer a choice, and
/// the candidate must carry an entitlement at all. `Err` is the finished
/// inert contribution.
fn entitlement_axis<'a>(
    term: &'static str,
    destination: &'a Destination,
    pool: &EntitlementPoolView,
) -> Result<&'a crate::routing::Entitlement, Contribution> {
    if !pool.offers_a_choice() {
        return Err(Contribution::new(term, 0.0, pool.no_choice_evidence()));
    }
    match destination.entitlement() {
        Some(entitlement) => Ok(entitlement),
        None => Err(Contribution::new(
            term,
            0.0,
            "no entitlement describes this destination's resource — nothing to score on the \
             pool axis, and nothing is guessed",
        )),
    }
}

/// Line 1966's capacity factor, with line 1968's distribution built in: the
/// entitlement's own band, minus a load step per live session elsewhere in
/// the set already charged to the same account. The load half is what
/// spreads independent fresh choices across the pool — the second of two
/// simultaneous choices sees the first's live session and the marginally
/// leading account stops leading.
pub fn entitlement_capacity(destination: &Destination, pool: &EntitlementPoolView) -> Contribution {
    const TERM: &str = "entitlement capacity";
    let entitlement = match entitlement_axis(TERM, destination, pool) {
        Ok(entitlement) => entitlement,
        Err(inert) => return inert,
    };
    let Some(band) = entitlement.capacity_band() else {
        return Contribution::new(
            TERM,
            0.0,
            format!(
                "`{}`'s remaining capacity is unknown — nothing measured it, so it contributes \
                 nothing, never a guessed number",
                entitlement.name()
            ),
        );
    };
    if band == CapacityBand::Exhausted {
        return Contribution::new(
            TERM,
            ENTITLEMENT_EXHAUSTED_PENALTY,
            format!(
                "`{}` reads exhausted — an account with nothing left cannot serve the next \
                 turn, and this is the one band reading that outweighs a warm session's \
                 context (line 1968)",
                entitlement.name()
            ),
        );
    }
    let band_weight = match band {
        CapacityBand::Plenty => ENTITLEMENT_CAPACITY_PLENTY,
        CapacityBand::Healthy => ENTITLEMENT_CAPACITY_HEALTHY,
        CapacityBand::Tight => ENTITLEMENT_CAPACITY_TIGHT,
        CapacityBand::Reserve => ENTITLEMENT_CAPACITY_RESERVE,
        CapacityBand::Exhausted => unreachable!("returned above"),
    };
    let live_elsewhere = pool.live_sessions_elsewhere(entitlement.name(), destination.id());
    let magnitude = band_weight + live_elsewhere as f64 * ENTITLEMENT_IN_FLIGHT_LOAD;
    let mut evidence = format!(
        "`{}` is in the {band} band — the pool's capacity axis, read from 56A-2's telemetry",
        entitlement.name()
    );
    if live_elsewhere > 0 {
        use std::fmt::Write as _;
        let _ = write!(
            evidence,
            "; {live_elsewhere} live session{} in this set already draw{} on it, so independent \
             work spreads to a sibling account rather than piling on (line 1968)",
            if live_elsewhere == 1 { "" } else { "s" },
            if live_elsewhere == 1 { "s" } else { "" },
        );
    }
    Contribution::new(TERM, magnitude, evidence)
}

/// Line 1967 — the reset-boundary rule, as its own named term. Reads ONLY
/// the entitlement's reset facet and its capacity band: a **low** band
/// (tight or reserve) with a **near** reset is burned (its remainder would
/// otherwise expire), the same band with a **far** reset is preserved, and
/// the two fade into each other linearly between the horizons. A healthy
/// band has no scarce remainder to burn or preserve; an exhausted one has
/// nothing left to burn; an unknown reset contributes nothing.
pub fn entitlement_reset_boundary(
    destination: &Destination,
    pool: &EntitlementPoolView,
) -> Contribution {
    const TERM: &str = "reset boundary";
    let entitlement = match entitlement_axis(TERM, destination, pool) {
        Ok(entitlement) => entitlement,
        Err(inert) => return inert,
    };
    let Some(band) = entitlement.capacity_band() else {
        return Contribution::new(
            TERM,
            0.0,
            format!(
                "`{}`'s capacity band is unknown — this term reads the reset beside the band, \
                 and without the band nobody can say whether a remainder would expire",
                entitlement.name()
            ),
        );
    };
    let Some(seconds) = entitlement.seconds_until_reset() else {
        return Contribution::new(
            TERM,
            0.0,
            format!(
                "`{}`'s reset is unknown — an unknown reset contributes nothing (line 1967)",
                entitlement.name()
            ),
        );
    };
    if band == CapacityBand::Exhausted {
        return Contribution::new(
            TERM,
            0.0,
            format!(
                "`{}` reads exhausted — there is no remainder left to burn, and the capacity \
                 term already prices the band",
                entitlement.name()
            ),
        );
    }
    if band > CapacityBand::Tight {
        return Contribution::new(
            TERM,
            0.0,
            format!(
                "`{}` is in the {band} band — no scarce remainder to burn before a reset or to \
                 preserve past one",
                entitlement.name()
            ),
        );
    }
    let urgency = burn_urgency(seconds);
    let magnitude = RESET_BURN * urgency + RESET_PRESERVE * (1.0 - urgency);
    let evidence = if urgency >= 1.0 {
        format!(
            "`{}` is in the {band} band and resets in {seconds}s, inside the \
             {RESET_BURN_HORIZON_SECONDS}s burn horizon — its remainder would otherwise \
             expire, so it is burned aggressively (line 1967)",
            entitlement.name()
        )
    } else if urgency <= 0.0 {
        format!(
            "`{}` is in the {band} band and resets in {seconds}s, at or past the \
             {RESET_PRESERVE_HORIZON_SECONDS}s preserve horizon — a low remainder with a far \
             reset is preserved, and the work routes to a sibling (line 1967)",
            entitlement.name()
        )
    } else {
        format!(
            "`{}` is in the {band} band and resets in {seconds}s, between the burn and \
             preserve horizons — {:.0}% of the burn preference applies (line 1967)",
            entitlement.name(),
            urgency * 100.0
        )
    };
    Contribution::new(TERM, magnitude, evidence)
}

/// How much of the burn preference applies at `seconds` until reset: `1.0`
/// inside [`RESET_BURN_HORIZON_SECONDS`] (a reset already past is the
/// clearest reason of all to stop conserving), `0.0` at or past
/// [`RESET_PRESERVE_HORIZON_SECONDS`], linear between.
pub(super) fn burn_urgency(seconds: i64) -> f64 {
    if seconds <= 0 {
        // A reset already reached — or a stale reading from the deliberately
        // un-staled capacity cache whose window has since closed — means the
        // remainder has rolled over: there is nothing left to burn before it
        // expires, so it is no more urgent than a distant reset. Without this
        // guard a negative `seconds` slips under the burn horizon and scores
        // the maximum +1.0, making the router prefer the *stalest* account
        // over a fresh healthy one (caught by the 2026-08-31 investigation
        // swarm; it inverts this line's own intent).
        0.0
    } else if seconds <= RESET_BURN_HORIZON_SECONDS {
        1.0
    } else if seconds >= RESET_PRESERVE_HORIZON_SECONDS {
        0.0
    } else {
        1.0 - (seconds - RESET_BURN_HORIZON_SECONDS) as f64
            / (RESET_PRESERVE_HORIZON_SECONDS - RESET_BURN_HORIZON_SECONDS) as f64
    }
}

/// Line 1966's throttling factor: what the evidence window's informative
/// throttles against this entitlement cost it, account-scoped where 56A-2's
/// narrowing could honestly narrow the count and provider-wide otherwise —
/// and the evidence sentence says which, because a shared reading shown as a
/// per-account one would claim knowledge nothing measured.
pub fn entitlement_throttling(
    destination: &Destination,
    pool: &EntitlementPoolView,
) -> Contribution {
    const TERM: &str = "entitlement throttling";
    let entitlement = match entitlement_axis(TERM, destination, pool) {
        Ok(entitlement) => entitlement,
        Err(inert) => return inert,
    };
    let Some(throttling) = entitlement.throttling() else {
        return Contribution::new(
            TERM,
            0.0,
            format!(
                "`{}`'s recent throttling is unknown — nothing consulted the ledger for it, \
                 and \"none observed\" may only be said by a resolver that looked",
                entitlement.name()
            ),
        );
    };
    if throttling.throttled() == 0 {
        return Contribution::new(
            TERM,
            0.0,
            format!(
                "no recent throttle is recorded against `{}` ({})",
                entitlement.name(),
                throttling.scope_word()
            ),
        );
    }
    let magnitude = (throttling.throttled() as f64 * ENTITLEMENT_THROTTLE_PENALTY)
        .max(ENTITLEMENT_THROTTLE_FLOOR);
    Contribution::new(
        TERM,
        magnitude,
        format!(
            "{} recent throttle{} recorded against `{}` ({})",
            throttling.throttled(),
            if throttling.throttled() == 1 { "" } else { "s" },
            entitlement.name(),
            throttling.scope_word()
        ),
    )
}

/// Line 1966's model-availability factor. Only the established-positive case
/// carries weight: a declared catalogue naming this destination's model. The
/// established-negative case is [`crate::routing::Entitlement::model_constraint`]'s
/// hard refusal and never reaches a score; harness-decided, an unknown
/// facet, and a harness-picked model all contribute nothing and say why.
pub fn entitlement_model_availability(
    destination: &Destination,
    pool: &EntitlementPoolView,
) -> Contribution {
    const TERM: &str = "entitlement model availability";
    let entitlement = match entitlement_axis(TERM, destination, pool) {
        Ok(entitlement) => entitlement,
        Err(inert) => return inert,
    };
    let Some(models) = entitlement.models() else {
        return Contribution::new(
            TERM,
            0.0,
            format!(
                "`{}`'s model list is unknown — no catalogue was ever read, which is not a \
                 `no`",
                entitlement.name()
            ),
        );
    };
    match models {
        crate::routing::EntitlementModelsFacet::HarnessDecided => Contribution::new(
            TERM,
            0.0,
            format!(
                "`{}` is a native sign-in whose models the harness decides — nothing to check \
                 a destination's model against, and no list is invented",
                entitlement.name()
            ),
        ),
        crate::routing::EntitlementModelsFacet::Declared(declared) => {
            let Some(model) = destination.backend().model().name() else {
                return Contribution::new(
                    TERM,
                    0.0,
                    format!(
                        "the harness picks this destination's model itself, so `{}`'s declared \
                         list of {} constrains and earns nothing",
                        entitlement.name(),
                        declared.len()
                    ),
                );
            };
            if models.serves(model) {
                Contribution::new(
                    TERM,
                    ENTITLEMENT_MODEL_AVAILABLE,
                    format!(
                        "`{}`'s declared catalogue names `{model}` — established to serve this \
                         destination's model",
                        entitlement.name()
                    ),
                )
            } else {
                // Unreachable through `choose` — the hard constraint removed
                // such a candidate — but this is a public function, and a
                // direct caller is owed the honest sentence rather than a
                // panic.
                Contribution::new(
                    TERM,
                    0.0,
                    format!(
                        "`{}`'s declared catalogue does not name `{model}` — a hard entitlement \
                         constraint removes such a candidate before any score is taken",
                        entitlement.name()
                    ),
                )
            }
        }
    }
}

/// Line 1968's stickiness, said where a reader looks for it: a zero-weight
/// note on an existing session's explanation stating that keeping the
/// session on the entitlement holding its context is the `session affinity`
/// term's weight — one mechanism, priced once — and naming the two things
/// that do move it (a rule, through 56A-1's hard constraint; an exhausted
/// band, through [`entitlement_capacity`]). `None` for a fresh destination,
/// which holds no context to be sticky about, and outside a live pool.
pub(super) fn entitlement_stickiness_note(
    destination: &Destination,
    pool: &EntitlementPoolView,
    affinity_magnitude: f64,
) -> Option<Contribution> {
    if !pool.offers_a_choice() || destination.is_fresh() {
        return None;
    }
    let entitlement = destination.entitlement()?;
    Some(Contribution::new(
        "entitlement stickiness",
        0.0,
        format!(
            "`{}` holds this session's context, and keeping the session there is already \
             priced by the `session affinity` term ({affinity_magnitude:+.3}) — stickiness is \
             that term's weight, not a second mechanism; only a rule that now denies this work \
             (the hard entitlement constraint) or an exhausted capacity band moves it \
             (line 1968)",
            entitlement.name()
        ),
    ))
}

// ---------------------------------------------------------------------------
// Phase 35C, lines 1559–1565 — the tier-movement decision.
//
// Escalation and downgrade are one decision, made once per `choose` over the
// candidate set, and they act on the ranking through the terms that already
// exist: the tier the fit term prefers, the tier the gate admits, and the
// tier the pressure terms read. No second warmth signal, no second health
// signal, no second capacity signal — every input below is one the terms
// above already price, read the same way they read it. What is new is the
// *movement* itself: a named, explained, recordable change to which tier this
// decision prefers, and the rules for when it may happen.
//
// Every threshold is stated once, here, with its reason.
// ---------------------------------------------------------------------------

/// Line 1559's *"repeatedly fail"*: this many consecutive observed failures
/// on a candidate, short of the cooldown [`crate::routing::free::ResourceHealth`]
/// would already have imposed. Two, because once is an incident and the
/// health term prices it at [`HEALTH_FAILURE_PENALTY`]; twice is a pattern
/// the tier above should be preferred over.
const REPEATED_FAILURES: u32 = 2;

/// Line 1565's cap: how many tiers one routing decision may escalate by
/// itself, whatever the triggers. **One.** A malformed task that trips every
/// trigger at once still moves one step, the step is printed, and the
/// triggers that would have moved it further are named as capped — so the
/// premium resources it did not reach are reachable only by a person.
const MAX_ESCALATION_STEPS: usize = 1;

/// Line 1560's *"task failure would be expensive"*: a task stated at this
/// tier or above. [`WorkloadTier::Heavy`]'s own doc — difficult debugging,
/// architecture-sensitive changes, broad refactors — is where a wrong tier
/// costs a whole attempt rather than a turn.
const EXPENSIVE_FAILURE_TIER: WorkloadTier = WorkloadTier::Heavy;

/// Line 1562's *"routine support work"*, tier half: at or below this tier.
/// Above it the work is by definition not routine, and the map's own tier
/// descriptions say so.
const ROUTINE_SUPPORT_CEILING: WorkloadTier = WorkloadTier::Standard;

/// Line 1562's *"premium capacity is tight"*: every metered candidate with a
/// reading is in this band or worse. The same band `pressure::TIGHT_BAND_PENALTY`
/// starts at, so "tight" means one thing across the module.
const DOWNGRADE_PRESSURE_BAND: CapacityBand = CapacityBand::Tight;

/// Line 1563: the longest expected duration a downgrade may bet on. A task
/// that runs several turns below its tier and fails is redone **in full** on
/// the premium resource the downgrade meant to spare — the retry then costs
/// at least what was saved, plus every turn already spent. Only single-turn
/// work has a retry cheap enough to risk.
const DOWNGRADE_RETRY_TOLERANCE: DurationClass = DurationClass::SingleTurn;

/// Line 1561: the tier delta a warm session's affinity is weighed against —
/// what an exact tier fit is worth over headroom, the one step a downgrade
/// would trade a warm higher-tier session's context for. Not a new number:
/// the difference between two constants this module already prices the tier
/// with, so the comparison is on the module's own scale.
const WARM_CONTEXT_TIER_DELTA: f64 = TIER_FIT_EXACT - TIER_FIT_HEADROOM;

/// The fit of a destination established *below* the tier an escalation moved
/// the preference to — reachable only after an escalation, because the gate
/// still admits the classified tier. Zero: kept eligible (it can serve the
/// work), not preferred (the escalation exists to prefer something else),
/// and its own health terms say why it was passed over.
pub(super) const TIER_FIT_BELOW_MOVED: f64 = 0.0;

/// What asked for an escalation. Ordered as evaluated: a failure that
/// already happened outranks the set's health, which outranks the
/// classifier's uncertainty.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EscalationTrigger {
    /// Line 1564: the last exchange on the current destination ended in a
    /// model-capability failure.
    AttributableFailure(FailureClass),
    /// Line 1559: every candidate established at the classified tier is
    /// refused, cooling down, exhausted, or repeatedly failing.
    TierStruggling,
    /// Line 1560: deterministic heuristics rated the work heavy and no model
    /// confirmed it.
    HeuristicHeavy,
}

impl std::fmt::Display for EscalationTrigger {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::AttributableFailure(class) => write!(
                f,
                "the last exchange on the current destination ended in `{class}`, a \
                 model-capability failure (line 1564)"
            ),
            Self::TierStruggling => f.write_str(
                "every candidate established at the classified tier is refused, cooling down, \
                 exhausted or repeatedly failing (line 1559)",
            ),
            Self::HeuristicHeavy => f.write_str(
                "deterministic heuristics rated the work heavy and no model confirmed it — the \
                 verdict most expensive to be wrong about (line 1560)",
            ),
        }
    }
}

/// Why a stated tier stood. Every arm is a sentence a person reads in the
/// explanation, because "nothing moved" is only useful when it says why.
#[derive(Debug, Clone, PartialEq)]
pub enum HoldReason {
    /// No trigger fired. `retry_after` is what the router was told about the
    /// last exchange, kept so the explanation can say a health failure was
    /// seen and deliberately not promoted on.
    NoTrigger { retry_after: Option<FailureClass> },
    /// Triggers fired and the classified tier is already the top.
    AtTop { triggers: Vec<EscalationTrigger> },
    /// Triggers fired and no healthy candidate is established at the tier
    /// above — the preference stays rather than pointing at nothing.
    NoTarget {
        to: WorkloadTier,
        triggers: Vec<EscalationTrigger>,
    },
    /// Line 1563: routine work under pressure, not downgraded, because a
    /// failed retry would cost more than the downgrade saves.
    RetryCost { duration: DurationClass },
    /// Line 1561: routine work under pressure, not downgraded, because an
    /// existing higher-tier session's context outweighs the tier delta.
    WarmContext { session: String, affinity: f64 },
    /// Line 1562 wanted to downgrade and no free candidate adequate for the
    /// work is available to take it.
    NoFreeResource { to: WorkloadTier },
}

/// What one routing decision did to the tier it prefers — lines 1559–1565.
#[derive(Debug, Clone, PartialEq)]
pub enum TierMovement {
    /// The classified tier stands, and `reason` says why.
    Held {
        tier: WorkloadTier,
        reason: HoldReason,
    },
    /// One tier up (line 1565: never more). `trigger` is what moved it;
    /// `capped` is every further trigger that fired and was not applied.
    Escalated {
        from: WorkloadTier,
        to: WorkloadTier,
        trigger: EscalationTrigger,
        capped: Vec<EscalationTrigger>,
    },
    /// One tier down (line 1562). `target` is the free destination that made
    /// the downgrade worth taking — the existence proof, not the winner.
    Downgraded {
        from: WorkloadTier,
        to: WorkloadTier,
        target: String,
    },
}

impl TierMovement {
    /// The tier the work was classified at.
    pub fn classified(&self) -> WorkloadTier {
        match self {
            Self::Held { tier, .. } => *tier,
            Self::Escalated { from, .. } | Self::Downgraded { from, .. } => *from,
        }
    }

    /// The tier this decision prefers — what [`workload_tier_fit`] reads.
    pub fn preferred_tier(&self) -> WorkloadTier {
        match self {
            Self::Held { tier, .. } => *tier,
            Self::Escalated { to, .. } | Self::Downgraded { to, .. } => *to,
        }
    }

    /// The tier the hard gate admits — what line 1516's arm reads. A
    /// downgrade lowers it, so a cheaper resource the classified tier would
    /// have refused becomes eligible; an escalation **never** raises it,
    /// because a preference may not remove a candidate that can serve the
    /// work (design decision 1).
    pub fn gate_tier(&self) -> WorkloadTier {
        match self {
            Self::Downgraded { to, .. } => *to,
            Self::Held { .. } | Self::Escalated { .. } => self.classified(),
        }
    }

    /// The tier the pressure terms read: the **lower** of the classified and
    /// the preferred tier. A downgrade reaches the spending protections —
    /// a leaf-tier reading makes `low_tier_spend` and the reserve gate
    /// stricter, the fail-closed direction — and an escalation never
    /// reaches them: a preference for a stronger resource must not be what
    /// unlocks protected premium reserve for work the classifier did not
    /// rate that high (the hazard practice §79 records for exactly this
    /// input).
    pub fn pressure_tier(&self) -> WorkloadTier {
        self.classified().min(self.preferred_tier())
    }

    /// Whether the preference actually moved.
    pub fn fired(&self) -> bool {
        !matches!(self, Self::Held { .. })
    }

    /// One sentence a person reads — the whole decision and its reason.
    pub fn describe(&self) -> String {
        match self {
            Self::Held {
                tier,
                reason: HoldReason::NoTrigger { retry_after },
            } => {
                let mut out = format!(
                    "the classified `{tier}` tier stands — no escalation trigger fired and the \
                     work is not routine support work under premium pressure"
                );
                if let Some(class) = retry_after {
                    out.push_str(&format!(
                        "; the last exchange's `{class}` failure is a provider-health or quota \
                         fact, priced by provider health and not promoted on (line 1564)"
                    ));
                }
                out
            }
            Self::Held {
                tier,
                reason: HoldReason::AtTop { triggers },
            } => format!(
                "`{tier}` is the top of the scale, so it stands although {} asked to escalate",
                list(triggers)
            ),
            Self::Held {
                tier,
                reason: HoldReason::NoTarget { to, triggers },
            } => format!(
                "{} asked to escalate to `{to}` and no healthy candidate is established at that \
                 tier — the preference stays at `{tier}` rather than pointing at nothing",
                list(triggers)
            ),
            Self::Held {
                tier,
                reason: HoldReason::RetryCost { duration },
            } => format!(
                "routine work under premium pressure, kept at `{tier}`: it is expected to run \
                 {duration}, and a multi-turn task that fails below its tier is redone in full \
                 on the premium resource the downgrade meant to spare (line 1563)"
            ),
            Self::Held {
                tier,
                reason: HoldReason::WarmContext { session, affinity },
            } => format!(
                "routine work under premium pressure, kept at `{tier}`: `{session}` is an \
                 existing session established at or above it whose session affinity \
                 ({affinity:+.3}) outweighs the tier delta ({WARM_CONTEXT_TIER_DELTA:+.3}) a \
                 downgrade would trade its context for (line 1561)"
            ),
            Self::Held {
                tier,
                reason: HoldReason::NoFreeResource { to },
            } => format!(
                "routine work under premium pressure, kept at `{tier}`: no free candidate \
                 adequate for it and able to serve `{to}` is available to take it (line 1562)"
            ),
            Self::Escalated {
                from,
                to,
                trigger,
                capped,
            } => {
                let mut out = format!(
                    "escalated from `{from}` to `{to}` — {trigger}; one tier per decision \
                     (line 1565)"
                );
                if !capped.is_empty() {
                    out.push_str(&format!(
                        ", and {} would also have escalated and were capped",
                        list(capped)
                    ));
                }
                out
            }
            Self::Downgraded { from, to, target } => format!(
                "downgraded from `{from}` to `{to}` — routine support work, every metered \
                 candidate with a reading is in the {DOWNGRADE_PRESSURE_BAND} band or worse, \
                 and `{target}` is a free resource able to take it (line 1562)"
            ),
        }
    }
}

fn list(triggers: &[EscalationTrigger]) -> String {
    triggers
        .iter()
        .map(|trigger| format!("[{trigger}]"))
        .collect::<Vec<_>>()
        .join(" and ")
}

/// The tier one step down, when there is one a model serves.
/// [`WorkloadTier::Deterministic`] is *"should not require an LLM"*, so
/// nothing below [`WorkloadTier::Leaf`] is a place to route model work to.
fn one_tier_below(tier: WorkloadTier) -> Option<WorkloadTier> {
    match tier {
        WorkloadTier::Deterministic | WorkloadTier::Leaf => None,
        WorkloadTier::Standard => Some(WorkloadTier::Leaf),
        WorkloadTier::Heavy => Some(WorkloadTier::Standard),
        WorkloadTier::Frontier => Some(WorkloadTier::Heavy),
    }
}

/// Whether `destination` is one line 1559 would escalate away from: refused
/// or cooling down (the two facts [`provider_health`] prices at
/// [`HEALTH_UNAVAILABLE_PENALTY`]), exhausted (the band the pressure terms
/// read), or repeatedly failing ([`REPEATED_FAILURES`]).
fn struggling(destination: &Destination, inputs: &RouterInputs<'_>) -> bool {
    let health = inputs.health.health(&FreeResource::new(
        destination.backend().credential().clone(),
        destination.backend().model().label(),
    ));
    health.credential_was_rejected()
        || !health.is_available(inputs.now)
        || health.consecutive_failures() >= REPEATED_FAILURES
        || destination.capacity_facts().band() == Some(CapacityBand::Exhausted)
}

/// Lines 1559–1565 in one function: decide whether this decision moves the
/// tier it prefers, over the candidates that survived the capability
/// constraints and before the tier gate runs.
///
/// `None` when no tier was stated — no task, so nothing to move and nothing
/// to explain, the same preservation clause every tier term keeps.
///
/// Escalation is decided first and downgrade second, and they are exclusive:
/// a decision that escalates does not also ask whether it should have
/// downgraded. Within escalation the triggers are collected in a fixed
/// order, the first moves the tier by [`MAX_ESCALATION_STEPS`], and the rest
/// are reported as capped (line 1565). An escalation needs somewhere to go:
/// a candidate established at the tier above that is not itself struggling,
/// or the preference would point at nothing.
pub(super) fn decide_tier_movement(
    candidates: &[&Destination],
    inputs: &RouterInputs<'_>,
    retry_after: Option<FailureClass>,
) -> Option<TierMovement> {
    let from = inputs.requirements.minimum_tier?;
    let answer = inputs.requirements.classification.as_ref();

    // --- escalation triggers, in order ------------------------------------
    let mut triggers = Vec::new();
    if let Some(class @ (FailureClass::RequestIncompatibility | FailureClass::EmptyCompletion)) =
        retry_after
    {
        triggers.push(EscalationTrigger::AttributableFailure(class));
    }
    let at_tier: Vec<&Destination> = candidates
        .iter()
        .copied()
        .filter(|destination| destination.tier_ceiling() == Some(from))
        .collect();
    if !at_tier.is_empty()
        && at_tier
            .iter()
            .all(|destination| struggling(destination, inputs))
    {
        triggers.push(EscalationTrigger::TierStruggling);
    }
    if let Some(answer) = answer
        && answer.classification().source().is_heuristic()
        && answer.stated_tier() >= EXPENSIVE_FAILURE_TIER
        && !answer.is_conservative()
    {
        triggers.push(EscalationTrigger::HeuristicHeavy);
    }

    if !triggers.is_empty() {
        let steps = triggers.len().min(MAX_ESCALATION_STEPS);
        let mut to = from;
        for _ in 0..steps {
            to = to.escalate();
        }
        if to == from {
            return Some(TierMovement::Held {
                tier: from,
                reason: HoldReason::AtTop { triggers },
            });
        }
        let target_exists = candidates.iter().any(|destination| {
            destination
                .tier_ceiling()
                .is_some_and(|ceiling| ceiling >= to)
                && !struggling(destination, inputs)
        });
        if !target_exists {
            return Some(TierMovement::Held {
                tier: from,
                reason: HoldReason::NoTarget { to, triggers },
            });
        }
        let trigger = triggers[0];
        let capped = triggers[steps..].to_vec();
        return Some(TierMovement::Escalated {
            from,
            to,
            trigger,
            capped,
        });
    }

    let held = |reason| Some(TierMovement::Held { tier: from, reason });
    let no_trigger = HoldReason::NoTrigger { retry_after };

    // --- downgrade: routine support work under premium pressure -----------
    let Some(answer) = answer else {
        return held(no_trigger);
    };
    let routine = matches!(
        answer.task_class(),
        TaskClass::Question | TaskClass::Investigation
    ) && from <= ROUTINE_SUPPORT_CEILING;
    let Some(to) = one_tier_below(from).filter(|_| routine) else {
        return held(no_trigger);
    };
    // "Premium capacity is tight": every metered candidate with a reading
    // is in the tight band or worse, and at least one was read. An unread
    // resource is neither tight nor healthy, here as everywhere.
    let metered_bands: Vec<CapacityBand> = candidates
        .iter()
        .filter(|destination| !destination.backend().cost().is_free())
        .filter_map(|destination| destination.capacity_facts().band())
        .collect();
    if metered_bands.is_empty()
        || metered_bands
            .iter()
            .any(|band| *band > DOWNGRADE_PRESSURE_BAND)
    {
        return held(no_trigger);
    }
    // Somewhere to go: free, able to serve now, adequate for the task, and
    // not established below the tier the downgrade moves to.
    let Some(target) = candidates.iter().find(|destination| {
        destination.backend().cost().is_free()
            && provider_available(destination, inputs.health, inputs.now)
            && is_adequate(destination, &inputs.requirements).is_none()
            && destination
                .tier_ceiling()
                .is_none_or(|ceiling| ceiling >= to)
    }) else {
        return held(HoldReason::NoFreeResource { to });
    };
    // Line 1563: the retry brake.
    let duration = answer.expected_duration();
    if duration > DOWNGRADE_RETRY_TOLERANCE {
        return held(HoldReason::RetryCost { duration });
    }
    // Line 1561: the warm-context brake, on the affinity term itself.
    if let Some((session, affinity)) = candidates
        .iter()
        .filter(|destination| {
            !destination.is_fresh()
                && destination
                    .tier_ceiling()
                    .is_some_and(|ceiling| ceiling >= from)
        })
        .map(|destination| {
            (
                destination.id().to_owned(),
                // `current` is `None` here on purpose: this brake runs before a
                // destination is chosen, so there is no move whose cache locality
                // the prompt-cache facet could read. Leaving it unknown makes the
                // brake slightly LESS likely to hold, which is the conservative
                // side for a rule that keeps work on a premium session.
                session_affinity(destination, None, &inputs.requirements).magnitude(),
            )
        })
        .find(|(_, affinity)| *affinity >= WARM_CONTEXT_TIER_DELTA)
    {
        return held(HoldReason::WarmContext { session, affinity });
    }
    Some(TierMovement::Downgraded {
        from,
        to,
        target: target.id().to_owned(),
    })
}

/// The movement as a zero-weight line in every candidate's explanation, so
/// the terms it changed are read beside the reason they changed.
pub(super) fn tier_movement_note(movement: &TierMovement) -> Contribution {
    Contribution::new("tier movement", 0.0, movement.describe())
}

/// Why an override could not be honoured. Reported rather than swallowed —
/// a user who asked for a destination and silently got another one has been
/// lied to by a router whose whole product is an explanation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OverrideRefusal {
    /// The named id is not among the destinations offered.
    NoSuchDestination(String),
    /// The named destination exists and a hard constraint rejected it.
    Ineligible(String, HardConstraint),
    /// `Fresh` was asked for and every fresh destination was rejected, or
    /// none was offered.
    NoFreshDestination,
    /// `Hold` was asked for and the caller named no current destination.
    NothingToHold,
}

impl std::fmt::Display for OverrideRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoSuchDestination(id) => write!(
                f,
                "the override named `{id}`, which is not one of the destinations offered"
            ),
            Self::Ineligible(id, constraint) => write!(
                f,
                "the override named `{id}`, which a hard {constraint} constraint rejected — an \
                 override may overrule a ranking and not a fact about what can serve"
            ),
            Self::NoFreshDestination => f.write_str(
                "the override asked for a fresh session and no eligible fresh destination was \
                 offered",
            ),
            Self::NothingToHold => f.write_str(
                "the override asked to hold the current destination and no current destination \
                 was named",
            ),
        }
    }
}

// ---------------------------------------------------------------------------
// Map line 1970 — the tier-preserving fallback across the pool.
//
// The user's ruling of 2026-08-31 (`design-decisions.md` §Phase 56A, "Step
// 4's fallback order") is the instruction of record for everything in this
// block, and its shape was settled there too: *"line 1970's fallback becomes
// a post-ranking reselection over `Routed::considered()`'s already-complete
// list — which preserves additive, never a filter"*. Nothing here removes a
// candidate, changes a score, or reorders the ranking; it moves the winner,
// once, and says so.
// ---------------------------------------------------------------------------

/// Why map line 1970's fallback fired — the two triggers the line names and
/// no others.
///
/// There is deliberately no third variant for *scored badly*. A fallback is
/// not a preference: the ruling's whole risk is a silent downgrade, so the
/// trigger is a fact about the account (its capacity band reads exhausted,
/// or the ledger recorded a throttle against it) and never a comparison
/// between two accounts. An untriggered decision is byte-identical to the
/// one this router made before line 1970 existed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FallbackReason {
    /// The chosen entitlement's capacity band reads
    /// [`CapacityBand::Exhausted`].
    Exhausted,
    /// The evidence window recorded at least one informative throttle
    /// against the chosen entitlement.
    Throttled,
}

impl FallbackReason {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Exhausted => "exhausted",
            Self::Throttled => "throttled",
        }
    }
}

impl std::fmt::Display for FallbackReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Which step of the ruling's order matched — map line 1970's *"stated
/// order"*, as four steps rather than the line's three words, because the
/// ruling adds the constraint the line does not say and which is the part
/// that matters: **it is tier-preserving**.
///
/// > *"switch to another subscription always if same model or model of
/// > similar capability is available in another. You can't put a fable 5
/// > task and switch it to a nemotron v3 … If subscription model of
/// > capability is not available switch to api one - if available."*
///
/// [`Self::ORDER`] is that sentence, in order, and it is the only place the
/// order is written down: the reselection walks this slice and stops at the
/// first match, so a build that ranked an API-credit account above a
/// subscription one would have to reorder this constant to do it. That is
/// what the order mutation in this package's evidence changes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FallbackStep {
    /// Another **subscription** serving the **same model**.
    SubscriptionSameModel,
    /// Another **subscription** serving a model of the **same capability
    /// tier**. Unreachable until Phase 34F's axis lands — see
    /// [`super::same_capability_tier`].
    SubscriptionSameTier,
    /// An **API-credit** account serving the **same model**.
    ApiCreditsSameModel,
    /// An **API-credit** account serving a model of the **same capability
    /// tier**. Unreachable until Phase 34F's axis lands.
    ApiCreditsSameTier,
}

impl FallbackStep {
    /// The ruling's order, and the only statement of it in this build.
    pub const ORDER: [Self; 4] = [
        Self::SubscriptionSameModel,
        Self::SubscriptionSameTier,
        Self::ApiCreditsSameModel,
        Self::ApiCreditsSameTier,
    ];

    /// What a candidate must be paid for by to match this step.
    /// [`EntitlementSource::Unstated`] matches no step: an entry that names
    /// no backing is *listed, never matched, never charged*, and an order
    /// over subscriptions and API credits has no place for it.
    fn source(self) -> EntitlementSource {
        match self {
            Self::SubscriptionSameModel | Self::SubscriptionSameTier => {
                EntitlementSource::Subscription
            }
            Self::ApiCreditsSameModel | Self::ApiCreditsSameTier => EntitlementSource::ApiCredits,
        }
    }

    /// Whether this step accepts a move from `from` to `to` on the model
    /// axis — the ruling's tier-preserving constraint, and the whole of
    /// what stops a fallback from silently downgrading the work.
    ///
    /// **Unknown narrows, and never widens.** A pair Phase 34F's axis has
    /// not ranked reads [`TierRelation::Unknown`], which is `false` on the
    /// tier steps; so while the axis is absent the order collapses to its
    /// two same-model steps, and a fallback that cannot establish it is
    /// preserving the tier simply does not happen. The ruling is explicit
    /// that this is the safe direction: a fallback that silently downgrades
    /// the model *"is worse than a refusal, because the work continues and
    /// looks fine"*.
    fn accepts(self, from: &Destination, to: &Destination) -> bool {
        match self {
            Self::SubscriptionSameModel | Self::ApiCreditsSameModel => same_model(from, to),
            Self::SubscriptionSameTier | Self::ApiCreditsSameTier => {
                match (from.backend().model().name(), to.backend().model().name()) {
                    (Some(from_model), Some(to_model)) => {
                        from_model != to_model
                            && same_capability_tier(from.capability_tier(), to.capability_tier())
                                == TierRelation::Same
                    }
                    // A model the harness picks itself has no name for an
                    // axis to have ranked. Unknown, and unknown narrows.
                    _ => false,
                }
            }
        }
    }

    /// The clause a person reads inside the fallback's own sentence.
    pub fn describe(self) -> &'static str {
        match self {
            Self::SubscriptionSameModel => "another subscription serving the same model",
            Self::SubscriptionSameTier => {
                "another subscription serving a model of the same capability tier"
            }
            Self::ApiCreditsSameModel => "an API-credit account serving the same model",
            Self::ApiCreditsSameTier => {
                "an API-credit account serving a model of the same capability tier"
            }
        }
    }
}

/// One fallback map line 1970 made: which account the ranking had chosen,
/// which one the work went to instead, why, and which step of the order
/// matched.
///
/// Carried on [`Routed`] beside [`TierMovement`] and for its reason — the
/// decision is the router's, and what to do with the record is the caller's.
/// `glasshouse route` renders it (see [`Routed::render`]) and records
/// nothing, exactly as it does for a moved tier.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EntitlementFallback {
    from: String,
    to: String,
    from_destination: String,
    to_destination: String,
    reason: FallbackReason,
    step: FallbackStep,
}

impl EntitlementFallback {
    /// The entitlement the ranking had chosen and the fallback moved off.
    pub fn from(&self) -> &str {
        &self.from
    }

    /// The entitlement the work went to instead.
    pub fn to(&self) -> &str {
        &self.to
    }

    /// [`Destination::id`] of the candidate the ranking had chosen.
    pub fn from_destination(&self) -> &str {
        &self.from_destination
    }

    /// [`Destination::id`] the work went to instead.
    pub fn to_destination(&self) -> &str {
        &self.to_destination
    }

    pub fn reason(&self) -> FallbackReason {
        self.reason
    }

    pub fn step(&self) -> FallbackStep {
        self.step
    }

    /// The sentence a person reads, and the one a durable record should
    /// carry: both accounts, the trigger, and the step of the order that
    /// matched. One wording, so an explanation and a record cannot describe
    /// one fallback two ways.
    pub fn describe(&self) -> String {
        format!(
            "entitlement `{}` is {} — the work moved to `{}`, {} (map line 1970's order)",
            self.from,
            self.reason,
            self.to,
            self.step.describe()
        )
    }
}

/// Whether these two candidates would run the **same model** — map line
/// 1970's first and third steps.
///
/// Two named models are the same when their names are. Two
/// [`super::AssignedModel::HarnessDefault`] candidates are the same when
/// they are the **same harness on the same launch profile**, which is a
/// fact rather than a guess: the harness makes one choice for one profile,
/// so two accounts offered for that profile differ in the account and in
/// nothing else — and that is the pool's own commonest shape, the
/// `fresh:<harness>:<profile>@<account>` candidates line 1953 splits a
/// profile into. Anything else — one named and one harness-picked, or two
/// harness-picked candidates on different profiles — is **not established**
/// to be the same model and is therefore not the same model, on the ruling's
/// own direction.
fn same_model(from: &Destination, to: &Destination) -> bool {
    match (from.backend().model().name(), to.backend().model().name()) {
        (Some(from), Some(to)) => from == to,
        (None, None) => {
            from.harness() == to.harness() && from.launch_profile() == to.launch_profile()
        }
        _ => false,
    }
}

/// Whether this entitlement is in a state map line 1970 falls back **from**,
/// and which of the line's two triggers it is.
///
/// Exhaustion outranks throttling when both read true: a spent allowance is
/// the stronger fact and the one a person would name. An entitlement whose
/// band nobody read and whose ledger nobody consulted answers `None` —
/// *unknown is not exhausted*, the rule every other gate in this module
/// follows, and the one that keeps a build with no telemetry routing exactly
/// as it did before this function existed.
fn fallback_trigger(entitlement: &crate::routing::Entitlement) -> Option<FallbackReason> {
    if entitlement.capacity_band() == Some(CapacityBand::Exhausted) {
        return Some(FallbackReason::Exhausted);
    }
    if entitlement
        .throttling()
        .is_some_and(|throttling| throttling.throttled() > 0)
    {
        return Some(FallbackReason::Throttled);
    }
    None
}

/// Map line 1970's reselection: given the ranking and the index it settled
/// on, the index the work should move to instead and the record of why.
///
/// `ranked` is the already-ranked, already-gated list `Routed` keeps as
/// [`Routed::considered`], best first, so every candidate here has passed
/// every hard constraint, **map line 1971's rules included** — there is no
/// path to a candidate the gate removed, since the gate ran first.
///
/// `None` — no fallback — whenever: the candidate set carries fewer than
/// two configured entitlements (no pool to fall back across); the chosen
/// candidate carries no entitlement or one neither exhausted nor throttled
/// (the untriggered case, byte-identical to today's decision); or no step
/// of [`FallbackStep::ORDER`] matched a **healthy** candidate on a
/// **different** account — a sibling in the same state is not a refuge.
// History: design-decisions.md, "Trims: routing module docs", routing/session/reserve.rs `fn entitlement_fallback`.
pub fn entitlement_fallback(
    ranked: &[&Destination],
    chosen_index: usize,
    pool: &EntitlementPoolView,
) -> Option<(usize, EntitlementFallback)> {
    if !pool.offers_a_choice() {
        return None;
    }
    let from = ranked.get(chosen_index)?;
    let from_entitlement = from.entitlement()?;
    let reason = fallback_trigger(from_entitlement)?;

    for step in FallbackStep::ORDER {
        for (index, candidate) in ranked.iter().enumerate() {
            if index == chosen_index {
                continue;
            }
            let Some(entitlement) = candidate.entitlement() else {
                continue;
            };
            if entitlement.name() == from_entitlement.name() {
                continue;
            }
            if fallback_trigger(entitlement).is_some() {
                continue;
            }
            if entitlement.source() != step.source() {
                continue;
            }
            if !step.accepts(from, candidate) {
                continue;
            }
            return Some((
                index,
                EntitlementFallback {
                    from: from_entitlement.name().to_owned(),
                    to: entitlement.name().to_owned(),
                    from_destination: from.id().to_owned(),
                    to_destination: candidate.id().to_owned(),
                    reason,
                    step,
                },
            ));
        }
    }
    None
}
