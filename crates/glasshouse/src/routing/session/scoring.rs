//! Scoring terms: each [`super::Contribution`] a candidate destination earns,
//! from harness/model pairing fit through cost and cache-locality terms.

use super::discovery::classify_destination;
use super::reserve::{TIER_FIT_BELOW_MOVED, entitlement_stickiness_note, tier_movement_note};
use super::*;

// ---------------------------------------------------------------------------
// The contributions. One public function each, so a mutation can zero
// exactly one of them.
// ---------------------------------------------------------------------------

/// Line 1595: what the harness's own capability fit for this destination
/// contributes.
///
/// Reads `classify`'s three **capability** axes and not its vendor class —
/// [`pairing_prior`], right below, reads that one. The axes vary with the
/// harness, which is exactly what makes this term able to separate a
/// candidate set: `crate::harness::adapter_for` returns a different adapter
/// per [`IntegrationId`], and each declares its own protocols.
pub fn harness_capability_fit(
    destination: &Destination,
    overrides: &pairing::PairingOverrides,
) -> Contribution {
    let pairing = classify_destination(destination, overrides);

    let (protocol_score, protocol_note) = match pairing.protocol_fit() {
        ProtocolFit::Native => (
            PROTOCOL_NATIVE_FIT,
            "speaks this route's wire protocol itself",
        ),
        ProtocolFit::Compatible => (
            PROTOCOL_COMPATIBLE_FIT,
            "does not speak this route's protocol, but the provider serves another one it does",
        ),
        ProtocolFit::Translated => (0.0, "reaches this route only through a translation adapter"),
        ProtocolFit::Incompatible => (0.0, "declares protocols and none of them is served here"),
        ProtocolFit::Unknown => (
            0.0,
            "declares no protocols, or the route named none — not a `no`",
        ),
    };

    let (behaviour_score, behaviour_note) = match pairing.model_behaviour() {
        ModelBehaviourFit::Verified => (0.0, "model behaviour established for this harness"),
        ModelBehaviourFit::Unverified => (
            0.0,
            "nobody established whether this model behaves the way this harness needs",
        ),
        ModelBehaviourFit::KnownAbsent => (
            MODEL_BEHAVIOUR_KNOWN_ABSENT,
            "this model is established **not** to behave the way this harness needs",
        ),
    };

    Contribution::new(
        "harness capability fit",
        protocol_score + behaviour_score,
        format!(
            "{}: protocol fit `{}` — {protocol_note}; model behaviour `{}` — {behaviour_note}",
            destination.harness().slug(),
            pairing.protocol_fit(),
            pairing.model_behaviour(),
        ),
    )
}

/// Lines 566, 1540, 1923: the vendor-native pairing's own soft starting
/// prior — see this module's header for why it is reachable here and not in
/// [`crate::routing::interactive`].
///
/// **Order of the two checks matters for the explanation a reader gets.** A
/// non-native pairing is inert regardless of how much local evidence exists
/// for it — it never had a prior to decay — so that check runs first and
/// reports plainly that this is not a vendor-native pairing. Only a
/// vendor-native pairing goes on to ask whether `PAIRING_PRIOR_EVIDENCE_THRESHOLD`
/// worth of local observations have replaced its starting assumption.
///
/// Never rejects: this term is a preference among candidates a hard
/// constraint has already admitted, and map line 1950's *"never rejecting
/// solely for being cross-vendor"* is a hard-constraint rule — no candidate
/// is refused on this axis anywhere in this module. `is_vendor_native`'s own
/// doc is the source of the "not a quality claim" wording repeated in both
/// branches below, and the map's first fixed architectural requirement is the
/// same claim at the product level.
pub fn pairing_prior(destination: &Destination, inputs: &RouterInputs<'_>) -> Contribution {
    let pairing = classify_destination(destination, inputs.overrides);
    let class = pairing.class();

    if !class.is_vendor_native() {
        return Contribution::new(
            "pairing prior",
            0.0,
            format!(
                "`{}` operating `{}` is a {class} pairing — inert: not a vendor-native pairing",
                destination.harness().slug(),
                destination.backend().model().label(),
            ),
        );
    }

    let observed = destination.pairing_prior_evidence();
    if observed >= PAIRING_PRIOR_EVIDENCE_THRESHOLD {
        return Contribution::new(
            "pairing prior",
            0.0,
            format!(
                "`{}` operating `{}` is a {class} pairing, but {observed} local observations \
                 have accumulated for it — observed evidence has replaced the starting prior",
                destination.harness().slug(),
                destination.backend().model().label(),
            ),
        );
    }

    Contribution::new(
        "pairing prior",
        PAIRING_PRIOR,
        format!(
            "`{}` operating `{}` is a {class} pairing — a starting assumption for a fresh \
             session with little local evidence, not a quality claim",
            destination.harness().slug(),
            destination.backend().model().label(),
        ),
    )
}

/// Line 1544: rounds per minute is supporting evidence, never a quality
/// score — clamped to a quarter of a full term's own `[-1.0, 1.0]` range, so
/// it can nudge a close decision but never outrank a full term on its own.
const TOOL_ROUNDS_MAGNITUDE_CEILING: f64 = 0.25;

/// Line 1542: the observed-reliability term's own ceiling — it may at most
/// equal what [`PAIRING_PRIOR`] gave, never exceed the starting assumption
/// it replaces.
const OBSERVED_RELIABILITY_MAGNITUDE_CEILING: f64 = PAIRING_PRIOR;

/// Whether this decision is tool-using work, and the reason — map line
/// 1352's own gate: TTFC is the responsiveness measure for tool-using agent
/// work, and a workload that is neither classified as one nor showing recent
/// tool activity on the session in hand is never scored on it.
///
/// Two tests, either sufficient: [`TaskRequirements::needs_tool_calls`] —
/// the classifier's own tool-using verdict
/// (`super::request::TaskClassification::requirements`'s
/// `needs_tool_calls: !hard_capabilities.is_empty()`, the same field
/// [`hard_constraint`] already gates a rejection on) — **or** the current
/// destination's own recent rows already carry a tool round, which is direct
/// evidence of tool use on this exact session even when no fresh
/// classification is in hand (a resumption, or `TaskRequirements::default()`).
fn tool_using_reason(
    inputs: &RouterInputs<'_>,
    current: Option<&Destination>,
) -> Option<&'static str> {
    if inputs.requirements.needs_tool_calls {
        return Some("the classified task needs tool calls");
    }
    let current_session_used_tools = current
        .and_then(Destination::route_responsiveness)
        .is_some_and(|reading| reading.rounds_per_minute_sample > 0);
    if current_session_used_tools {
        return Some("the current session's recent rows already carry a tool round");
    }
    None
}

/// Lines 1351/1352/1543: reliability-adjusted latency in route comparison —
/// a fast route that frequently fails is not ranked as genuinely fast.
///
/// Inert (`0.0`, naming why) for a workload that is not tool-using
/// ([`tool_using_reason`]), for a destination with no attached
/// [`Destination::route_responsiveness`], and for one whose effective TTFC
/// [`RouteResponsiveness::effective_ttfc_ms`] answers `None` — below
/// [`MIN_SAMPLE_FOR_SUMMARY`] on either half, or a failure rate at or above
/// 100%. Otherwise the candidate set's best (lowest) effective TTFC scores
/// `+1.0` and every other candidate scales by `best / own` — a route twice
/// as slow scores `0.5` — never negative, because a slower route is worth
/// less, not a defect to penalise past zero.
fn responsiveness(
    destination: &Destination,
    inputs: &RouterInputs<'_>,
    current: Option<&Destination>,
    best_effective_ttfc_ms: Option<f64>,
) -> Contribution {
    const TERM: &str = "responsiveness (effective TTFC)";
    let Some(reason) = tool_using_reason(inputs, current) else {
        return Contribution::new(
            TERM,
            0.0,
            "inert: this is not tool-using work — neither the classified task class nor the \
             current session's recent rows show tool use",
        );
    };
    let Some(reading) = destination.route_responsiveness() else {
        return Contribution::new(
            TERM,
            0.0,
            format!("inert: no responsiveness reading attached to this destination ({reason})"),
        );
    };
    let Some(own_effective_ttfc_ms) = reading.effective_ttfc_ms() else {
        return Contribution::new(
            TERM,
            0.0,
            format!(
                "inert: effective TTFC unmeasured for this route — {} rows carry a raw TTFC, {} \
                 rows carry a known outcome, and {MIN_SAMPLE_FOR_SUMMARY} of each are needed \
                 ({reason})",
                reading.raw_ttfc_sample, reading.failure_rate_sample,
            ),
        );
    };
    // `best_effective_ttfc_ms` is computed over the same candidates this
    // destination is being scored among, so `own_effective_ttfc_ms` is
    // always one of its own inputs and the ratio below is always in
    // `(0.0, 1.0]` — this candidate can score `1.0` (it IS the best) but
    // never negative, matching the objective's own rule.
    let Some(best) = best_effective_ttfc_ms else {
        return Contribution::new(
            TERM,
            0.0,
            format!(
                "inert: no candidate in this decision has a measured effective TTFC ({reason})"
            ),
        );
    };
    let magnitude = best / own_effective_ttfc_ms;
    Contribution::new(
        TERM,
        magnitude,
        format!(
            "{reason}: raw TTFC {:.0}ms, effective TTFC {own_effective_ttfc_ms:.0}ms at an \
             observed failure rate of {:.1}% (over {} rows) against the candidate set's best \
             effective TTFC of {best:.0}ms",
            reading.raw_ttfc_ms.unwrap_or_default(),
            reading.failure_rate.unwrap_or_default() * 100.0,
            reading.failure_rate_sample,
        ),
    )
}

/// Map line 1350's rate, priced as supporting evidence — never a full term,
/// never a substitute for [`responsiveness`], and printed inert whenever
/// [`PurposeConsumption::tool_rounds_per_minute`]'s own reasons for
/// withholding apply here too: no rows carrying both a round count and a
/// dispatch/completion pair.
///
/// [`PurposeConsumption::tool_rounds_per_minute`]: super::evidence::PurposeConsumption::tool_rounds_per_minute
fn tool_round_rate(destination: &Destination) -> Contribution {
    const TERM: &str = "tool rounds per minute";
    let Some(reading) = destination.route_responsiveness() else {
        return Contribution::new(
            TERM,
            0.0,
            "inert: no responsiveness reading attached to this destination — supporting \
             evidence, not a quality score",
        );
    };
    let Some(rate) = reading.rounds_per_minute else {
        return Contribution::new(
            TERM,
            0.0,
            format!(
                "inert: fewer than {MIN_SAMPLE_FOR_SUMMARY} rows carrying both a tool round and \
                 a dispatch/completion pair — supporting evidence, not a quality score"
            ),
        );
    };
    let magnitude = rate.clamp(
        -TOOL_ROUNDS_MAGNITUDE_CEILING,
        TOOL_ROUNDS_MAGNITUDE_CEILING,
    );
    Contribution::new(
        TERM,
        magnitude,
        format!(
            "{rate:.2} successful tool rounds/min over {} rows — supporting evidence, not a \
             quality score",
            reading.rounds_per_minute_sample
        ),
    )
}

/// Lines 1542/1923: observed pairing reliability replaces the same-vendor
/// prior once enough local observations exist — the term
/// [`pairing_prior`] itself says it yields to at
/// [`PAIRING_PRIOR_EVIDENCE_THRESHOLD`].
///
/// Inert for a non-vendor-native pairing (the prior never applied to it
/// either) and for a pairing below [`PAIRING_PRIOR_EVIDENCE_THRESHOLD`] local
/// observations (the starting prior is still active — this term has nothing
/// to replace yet). Once both gates clear **and** the attached
/// [`RouteResponsiveness::failure_rate`] itself meets
/// [`MIN_SAMPLE_FOR_SUMMARY`], the magnitude is `(1 - p - 0.5) * 0.4` clamped
/// to `±PAIRING_PRIOR` — so a perfectly reliable pairing (`p = 0`) scores
/// `+0.2`, matching what the prior itself gave a fresh session, and this
/// term can never exceed that ceiling.
fn observed_pairing_reliability(
    destination: &Destination,
    inputs: &RouterInputs<'_>,
) -> Contribution {
    const TERM: &str = "observed pairing reliability";
    let pairing = classify_destination(destination, inputs.overrides);
    let class = pairing.class();
    if !class.is_vendor_native() {
        return Contribution::new(
            TERM,
            0.0,
            format!(
                "`{}` operating `{}` is a {class} pairing — inert: not a vendor-native pairing, \
                 the same axis the pairing prior is inert on",
                destination.harness().slug(),
                destination.backend().model().label(),
            ),
        );
    }
    let observed = destination.pairing_prior_evidence();
    if observed < PAIRING_PRIOR_EVIDENCE_THRESHOLD {
        return Contribution::new(
            TERM,
            0.0,
            format!(
                "inert: only {observed} local observations for this pairing — fewer than \
                 {PAIRING_PRIOR_EVIDENCE_THRESHOLD}, so the starting prior is still active and \
                 there is nothing yet to replace it with"
            ),
        );
    }
    let Some(reading) = destination.route_responsiveness() else {
        return Contribution::new(
            TERM,
            0.0,
            "inert: no responsiveness reading attached to this destination, though \
             {observed} local observations exist for the pairing",
        );
    };
    let Some(p) = reading.failure_rate else {
        return Contribution::new(
            TERM,
            0.0,
            format!(
                "inert: fewer than {MIN_SAMPLE_FOR_SUMMARY} rows carry a known outcome for this \
                 route — the prior has yielded (0.0 at {observed} observations) but there is not \
                 yet an independent reliability signal to replace it with"
            ),
        );
    };
    let magnitude = ((1.0 - p - 0.5) * 0.4).clamp(
        -OBSERVED_RELIABILITY_MAGNITUDE_CEILING,
        OBSERVED_RELIABILITY_MAGNITUDE_CEILING,
    );
    Contribution::new(
        TERM,
        magnitude,
        format!(
            "`{}` operating `{}` is a {class} pairing with {observed} local observations and an \
             observed failure rate of {:.1}% (over {} rows) — this replaces the pairing prior, \
             which has already yielded at this evidence count",
            destination.harness().slug(),
            destination.backend().model().label(),
            p * 100.0,
            reading.failure_rate_sample,
        ),
    )
}

/// Map line 1382, joined to a task's hard capability requirements —
/// `GH-ROUTING-CAPABILITY`'s package, and `capability::axis_for`'s own
/// comparison function is what makes this ruling-1-safe: this function never
/// compares a task's tier to a resource's tier, only a resource's registry
/// entry to the specific axis a requirement names.
///
/// This is one of `TaskClassification::hard_capabilities`' two production
/// consumers — the other is `is_adequate`, which `session::hard_constraint`
/// asks the same question of to raise `HardConstraint::Capability` (map line
/// 1517). `requirements.hard_capabilities` is where a caller of
/// [`SessionRouter::choose`] attaches it: `main.rs`'s `launch_session` and
/// `route_recommendation` both build it from `classified.answer.requirements()`
/// on every classified launch; a caller with no task in hand still passes
/// `TaskRequirements::default()`, an empty list that contributes `0.0` here
/// and excludes nothing at the gate.
///
/// Reads `destination.harness()` the same way [`harness_capability_fit`]
/// does — the identity is already in hand at the point this term is
/// computed — and combines it with [`Destination::resource_facts`] through
/// [`capability::ResourceCapabilities::describe`]. No capability value and no
/// resource identity is matched here: this function only asks the registry a
/// question and applies the three named constants above, which is 1390's
/// answer — a new resource, a new harness, or a corrected axis changes
/// nothing in this function's body.
pub fn capability_fit(destination: &Destination, requirements: &TaskRequirements) -> Contribution {
    if requirements.hard_capabilities.is_empty() {
        return Contribution::new(
            "capability fit",
            0.0,
            "the task named no hard capability requirement, so this resource's capability \
             description contributes nothing",
        );
    }

    let harness_caps = crate::harness::adapter_for(destination.harness())
        .map(|adapter| adapter.describe().capabilities)
        .unwrap_or(HarnessCapabilities::UNVERIFIED);
    let resource =
        capability::ResourceCapabilities::describe(&harness_caps, destination.resource_facts());

    let mut magnitude = 0.0;
    let mut notes = Vec::with_capacity(requirements.hard_capabilities.len());
    for requirement in &requirements.hard_capabilities {
        let axis = capability::axis_for(*requirement);
        let (term, note) = match resource.axis(axis) {
            Declared::Verified {
                value: true,
                evidence,
            } => (
                CAPABILITY_ESTABLISHED_PRESENT,
                format!(
                    "`{}` needs {} and this resource's `{}` is established present ({evidence})",
                    destination.harness().slug(),
                    requirement.as_str(),
                    axis.name(),
                ),
            ),
            Declared::Verified {
                value: false,
                evidence,
            } => (
                CAPABILITY_ESTABLISHED_ABSENT,
                format!(
                    "`{}` needs {} and this resource's `{}` is established **absent** \
                     ({evidence})",
                    destination.harness().slug(),
                    requirement.as_str(),
                    axis.name(),
                ),
            ),
            Declared::Unverified => (
                CAPABILITY_UNVERIFIED,
                format!(
                    "`{}` needs {} and this resource's `{}` is not established — not a `no`",
                    destination.harness().slug(),
                    requirement.as_str(),
                    axis.name(),
                ),
            ),
        };
        magnitude += term;
        notes.push(note);
    }

    Contribution::new("capability fit", magnitude, notes.join("; "))
}

/// Map line 1531: how well this destination's established workload ceiling
/// fits the tier the work needs.
///
/// Only called when a tier is stated (`score` skips it otherwise, so a
/// launch that states no task renders exactly the explanation it always
/// has). `required` is the tier this decision **prefers** — the classified
/// tier, or the one a [`TierMovement`] moved it to. A destination below the
/// *classified* tier is never scored here — line 1516's gate in
/// `hard_constraint` removed it — so the fourth arm is reachable only after
/// an escalation: a destination established at the classified tier, kept
/// eligible because it can serve the work, and not the fit preferred now.
pub fn workload_tier_fit(destination: &Destination, required: WorkloadTier) -> Contribution {
    match destination.tier_ceiling() {
        Some(ceiling) if ceiling < required => Contribution::new(
            "workload tier fit",
            TIER_FIT_BELOW_MOVED,
            format!(
                "this decision escalated its preference to the `{required}` tier and `{}` is \
                 established to serve up to `{ceiling}` — still eligible for the classified \
                 tier, and not the fit preferred now (line 1559)",
                destination.id()
            ),
        ),
        Some(ceiling) if ceiling == required => Contribution::new(
            "workload tier fit",
            TIER_FIT_EXACT,
            format!(
                "the task needs the `{required}` tier and `{}` is established to serve exactly \
                 that",
                destination.id()
            ),
        ),
        Some(ceiling) => Contribution::new(
            "workload tier fit",
            TIER_FIT_HEADROOM,
            format!(
                "the task needs the `{required}` tier and `{}` is established to serve up to \
                 `{ceiling}` — it can, with headroom a cheaper resource would not spend",
                destination.id()
            ),
        ),
        None => Contribution::new(
            "workload tier fit",
            TIER_FIT_UNVERIFIED,
            format!(
                "the task needs the `{required}` tier and nothing has established `{}`'s \
                 ceiling — not a `no`",
                destination.id()
            ),
        ),
    }
}

/// Map line 1558: *"prefer the cheapest healthy candidate that satisfies the
/// required workload tier and hard capabilities."*
///
/// # What this term is, and what the three words before it already decide
///
/// The line names four properties, and three of them are already decided
/// before this function is reached, which is why it prices only the fourth:
///
/// - *satisfies the required workload tier* — `hard_constraint` has already
///   **removed** a destination whose established ceiling is below the
///   requirement, and [`workload_tier_fit`] prices the fit of what is left;
/// - *satisfies the hard capabilities* — [`capability_fit`] prices an
///   established-absent axis at `CAPABILITY_ESTABLISHED_ABSENT`, four times
///   this term's magnitude, so a cheap resource established to lack what the
///   task needs can never win on price;
/// - *healthy* — [`provider_health`] prices a refused credential and a
///   cooling-down provider, and its penalties are larger again.
///
/// So what is left for this term is the comparison the line is actually
/// about: two candidates the terms above could not separate, one of which
/// spends the user's money. It is `METERED_COST_PREFERENCE` for a metered
/// destination and `0.0` for a free one — a preference for the free
/// resource expressed as a cost on the paid one, so that a project with no
/// free resource configured is not scored as though every destination it has
/// were somehow deficient.
///
/// # Why it is only pushed when a tier was established
///
/// `score` pushes this term exactly where it pushes [`workload_tier_fit`]:
/// under `if let Some(required)`. The line's own subject is *"a candidate
/// that satisfies the required workload tier"*, and there is no required tier
/// until a task has been classified — so a launch or a `glasshouse route`
/// that states no task renders precisely the explanation it rendered before
/// this term existed, byte for byte. The same rule, and the same reason, as
/// the tier term beside it.
///
/// `Cost` is [`super::Backend::cost`], which `main.rs::destination_backend`
/// resolves through `ProviderConfig::cost_of` — the user's own `free_models`
/// list. Nothing here infers a price from a model's name.
pub fn cost_preference(destination: &Destination) -> Contribution {
    if destination.backend().cost().is_free() {
        return Contribution::new(
            "cost preference",
            0.0,
            format!(
                "`{}` is a zero-cost resource for this work — nothing is spent by preferring it",
                destination.id()
            ),
        );
    }
    Contribution::new(
        "cost preference",
        METERED_COST_PREFERENCE,
        format!(
            "`{}` is metered, so it is preferred only over candidates this decision's other \
             terms could not already separate it from (line 1558)",
            destination.id()
        ),
    )
}

/// Map line 1538: *"include expected marginal cost in candidate scoring."*
///
/// `Cost` — [`super::Cost`]'s own doc calls it *"whether using a model costs
/// the user anything at the margin"* — is still the only reading that can
/// ever make this term `0.0`: [`Cost::is_free`] returning `true` is a
/// **known** zero, never an unknown, and stays priced exactly as it always
/// was. Phase 32G's `PriceTable` (`crate::provider::pricing`) answers the
/// other half for a metered destination — a known per-million price, or an
/// honest unknown — but it changes only the **evidence**, never the
/// magnitude: there is still no per-call token estimate at this call site
/// (`SessionContextFacts` carries none), so a known price cannot yet be
/// converted into an actual expected dollar figure without inventing one.
/// Reporting the known rate is honest; reporting a dollar estimate from it
/// is map line 1298's job, once a size producer exists. A destination whose
/// price is unknown is priced identically to one whose price is known but
/// unconvertible — both metered, neither free — and the difference between
/// them is only ever textual, the same way [`AffinityFacet`]'s `known` and
/// `unknown` constructors both start every unattached facet as `0.0`.
///
/// **Pushed unconditionally**, unlike [`cost_preference`], because line 1538
/// names no workload-tier precondition the way line 1558 does. That is also
/// why it must stay inert exactly where [`cost_preference`] is active: once a
/// tier is established, [`cost_preference`] already prices the same `Cost`
/// reading as its own deliberately small tie-break (line 1558's own doc).
/// Pricing it again here would score the identical fact in the identical
/// direction a second time — the double-count this term exists to avoid, not
/// to add — so the two conditions partition rather than overlap: exactly one
/// of them ever prices a given candidate.
fn expected_marginal_cost(
    destination: &Destination,
    movement: Option<&TierMovement>,
    prices: &PriceTable,
    task_class: Option<TaskClass>,
    comparable_output: &[ClassOutput],
) -> Contribution {
    let known_price = destination
        .backend()
        .model()
        .name()
        .and_then(|model| prices.price_for(destination.backend().provider(), model));
    if movement.is_some() {
        let mut evidence = String::from(
            "a workload tier is established for this decision, so `cost preference` (line \
             1558) already prices free versus metered here — pricing it twice would double- \
             count the same reading",
        );
        // Map line 1301: the output half is not `cost_preference`'s reading
        // and is never priced twice by it, so it is said here even though
        // the input half's magnitude stands aside for the tier term.
        if !destination.backend().cost().is_free()
            && let Some(price) = known_price
        {
            evidence.push_str("; ");
            evidence.push_str(&expected_output_cost_evidence(
                task_class,
                comparable_output,
                price,
            ));
        }
        return Contribution::new("expected marginal cost", 0.0, evidence);
    }
    if destination.backend().cost().is_free() {
        return Contribution::new(
            "expected marginal cost",
            0.0,
            format!(
                "`{}` is a zero-cost resource for this work — nothing is spent by preferring it",
                destination.id()
            ),
        );
    }
    let (magnitude, mut evidence) = match known_price {
        Some(price) => match destination.estimated_input_size().total_tokens() {
            // Map line 1298: the rate and this decision's own input-size
            // estimate together become an actual dollar figure. The
            // **magnitude** still does not move — see this function's own
            // doc comment on why pricing it a second time here would
            // double-count `cost_preference` — only the evidence gains the
            // conversion this package exists to make possible.
            Some(tokens) => {
                let cost_usd = tokens as f64 * price.input_per_million_usd / 1_000_000.0;
                (
                    EXPECTED_MARGINAL_COST_PENALTY,
                    format!(
                        "`{}` is metered; its price is known — ${:.2} per million input \
                         tokens, ${:.2} per million output tokens — and this decision's own \
                         input-size estimate ({}) puts the expected marginal cost at roughly \
                         ${:.4} for this call; no workload tier is established yet to price it \
                         another way (line 1558 would once one is)",
                        destination.id(),
                        price.input_per_million_usd,
                        price.output_per_million_usd,
                        destination.estimated_input_size().describe(),
                        cost_usd,
                    ),
                )
            }
            // Unknown SIZE makes the cost unknown even when the price is
            // known — the rule this package adds. Priced identically to an
            // unpriced metered destination, never collapsed toward zero.
            None => (
                EXPECTED_MARGINAL_COST_PENALTY,
                format!(
                    "`{}` is metered; its price is known — ${:.2} per million input tokens, \
                     ${:.2} per million output tokens — but this decision's own input-size \
                     estimate could not measure any component for it, so there is nothing to \
                     convert that rate with; it is priced the same as an unpriced metered \
                     destination and no workload tier is established yet to price it another \
                     way (line 1558 would once one is)",
                    destination.id(),
                    price.input_per_million_usd,
                    price.output_per_million_usd,
                ),
            ),
        },
        // The magnitude here must stay `EXPECTED_MARGINAL_COST_PENALTY`, not
        // `0.0`: an unknown price is unknown, not free, and collapsing this
        // branch to the free branch's zero is exactly the fake-zero map line
        // 1305 forbids (see this module's own mutation coverage).
        None => (
            EXPECTED_MARGINAL_COST_PENALTY,
            format!(
                "`{}` is metered and its price is unknown — no provider price metadata names \
                 this provider/model, and no workload tier is established yet to price it \
                 another way (line 1558 would once one is); an unknown price is priced as \
                 metered, never as free",
                destination.id()
            ),
        ),
    };
    // Map line 1301, beside the input half above: never moves `magnitude`,
    // the same precedent line 1298 already set for it.
    if let Some(price) = known_price {
        evidence.push_str("; ");
        evidence.push_str(&expected_output_cost_evidence(
            task_class,
            comparable_output,
            price,
        ));
    }
    Contribution::new("expected marginal cost", magnitude, evidence)
}

/// Map line 1301's evidence half — `GH-TASK-CLASS-COST-JOIN`, joining the
/// launch's task class (`crate::gateway::session::SessionRouting::
/// serve_task_class`) with the median output size
/// [`super::burn::output_tokens_by_class`] read for it — appended to
/// [`expected_marginal_cost`]'s evidence wherever a price is already known.
/// **Never moves a magnitude**: this function returns text only, the same
/// precedent line 1298 set for the input half.
///
/// `task_class` is *this decision's* class — the classification
/// [`RouterAnswer::task_class`] gave it, when one was established; `None`
/// for a launch or `glasshouse route --task`-less run that classified
/// nothing, which reads as *no task class established* rather than
/// borrowing a class nobody named. `comparable_output` is the caller's own
/// window of [`ClassOutput`] readings; a class this decision names but the
/// window carries fewer than [`MIN_SAMPLE_FOR_SUMMARY`] rows for — including
/// none at all — is unmeasured, named with the floor, and never given an
/// invented size.
fn expected_output_cost_evidence(
    task_class: Option<TaskClass>,
    comparable_output: &[ClassOutput],
    price: crate::provider::pricing::ModelPrice,
) -> String {
    let Some(class) = task_class else {
        return "expected output size unmeasured (no task class established)".to_owned();
    };
    let comparable = comparable_output
        .iter()
        .find(|reading| reading.class == class);
    match comparable.and_then(|reading| reading.median_output_tokens) {
        Some(median_tokens) => {
            let samples = comparable.map_or(0, |reading| reading.samples);
            let cost_usd = median_tokens * price.output_per_million_usd / 1_000_000.0;
            format!(
                "recent comparable {class} tasks ({samples} in the window) produced a median \
                 of {median_tokens:.0} output tokens, putting expected output cost at roughly \
                 ${cost_usd:.4}"
            )
        }
        None => format!(
            "expected output size unmeasured (fewer than {MIN_SAMPLE_FOR_SUMMARY} comparable \
             {class} tasks recorded)"
        ),
    }
}

/// Line 1302: what a request pool's own scarcity is worth, read from
/// [`Allowance`]'s remaining count and [`Destination::burn_forecast`]'s
/// persisted rate — never recomputed from ledger rows, and never folded into
/// `expected_marginal_cost`'s magnitude: a reader sees two terms, one for
/// money and one for a scarce unit money does not price.
///
/// # Its own axis, never 1280's twice
///
/// [`super::pressure::exhaustion_forecast_pressure`] already prices the case
/// where a resource will not make it to its reset. This term is inert
/// whenever [`crate::routing::burn::ExhaustionForecast::exhausts_well_before_reset`]
/// says that term is already carrying the penalty for this destination's
/// resource — `phase-32g.md`'s 1302 entry: one forecast, priced once. What is
/// left for this term is the case beside it: a pool that will make its reset
/// but is being spent fast enough to be worth naming.
///
/// # Inert, and says so, in three cases
///
/// - the allowance is [`Allowance::TokenPriced`] — "how many requests are
///   left" has no answer for a resource priced per token, and pricing it
///   anyway is exactly the conflation `free.rs`'s own module doc warns
///   against;
/// - the pool's remaining count is not yet known, or the destination carries
///   no burn forecast at all (too few rows, no measured remaining amount, or
///   a non-positive rate — see [`crate::routing::burn::forecast`]);
/// - the forecast already exhausts well before the reset, which is the case
///   above.
pub fn request_pool_cost(destination: &Destination, pool: &FreePool) -> Contribution {
    let allowance = pool.allowance(destination.backend().credential());
    if !allowance.is_request_pool() {
        return Contribution::new(
            REQUEST_POOL_COST_TERM,
            0.0,
            "inert: this destination's allowance is priced per token, not by a request pool",
        );
    }
    let Allowance::RequestPool { remaining, .. } = allowance else {
        unreachable!("`is_request_pool` just confirmed this allowance is a request pool");
    };
    let Some(remaining) = remaining else {
        return Contribution::new(
            REQUEST_POOL_COST_TERM,
            0.0,
            "inert: this is a request pool, but its remaining count is not yet known",
        );
    };
    let Some(forecast) = destination.burn_forecast() else {
        return Contribution::new(
            REQUEST_POOL_COST_TERM,
            0.0,
            format!(
                "inert: {remaining} requests remain on this request pool, but no burn rate is \
                 known for it yet"
            ),
        );
    };
    if forecast.exhausts_well_before_reset() {
        return Contribution::new(
            REQUEST_POOL_COST_TERM,
            0.0,
            "inert: the exhaustion forecast term already prices this resource's scarcity — \
             pricing it here too would price the same forecast twice",
        );
    }
    let hours = (forecast.seconds_to_exhaustion as f64 / 3600.0).max(0.0);
    let magnitude = REQUEST_POOL_COST_PENALTY * REQUEST_POOL_COST_HALF_LIFE_HOURS
        / (REQUEST_POOL_COST_HALF_LIFE_HOURS + hours);
    Contribution::new(
        REQUEST_POOL_COST_TERM,
        magnitude,
        format!(
            "request pool has {remaining} requests remaining at an estimated {:.1} \
             requests/hour — about {hours:.1}h left at the current rate, over {} observations",
            forecast.requests_per_hour, forecast.rows
        ),
    )
}

/// Map line 1307: the marginal input cost this decision actually used, as a
/// monetary reading with its required confidence — never recomputed once
/// carried. [`SessionRouter::choose`] calls this exactly once, for the
/// destination it settled on, and the result travels on [`Routed`] to
/// whatever records it (`main.rs::record_entitlement_fallback`), rather than
/// being derived a second time at the writer from a `PriceTable` that may
/// have changed on disk since the decision was made.
///
/// Free is a known zero, regardless of size — nothing is spent whatever the
/// input turns out to be, the same certainty [`expected_marginal_cost`]'s
/// free branch reads. A metered destination needs **both** a known price
/// and a known size; either half missing answers `None` — never a
/// fabricated zero, matching map line 1307's own rule that unknown size or
/// unknown price means no cost row at all.
///
/// [`CostConfidence::Estimated`], always: every cost this function can
/// produce is built from the user's own `pricing.toml` and Glasshouse's own
/// token measurement, never a figure a provider reported — migration 11's
/// `CHECK` requires a label to be chosen, and this is the one that says so.
pub(super) fn estimated_cost(
    destination: &Destination,
    prices: &PriceTable,
) -> Option<ObservedCost> {
    if destination.backend().cost().is_free() {
        return Some(ObservedCost {
            micro_usd: 0,
            confidence: CostConfidence::Estimated,
        });
    }
    let price = destination
        .backend()
        .model()
        .name()
        .and_then(|model| prices.price_for(destination.backend().provider(), model))?;
    let tokens = destination.estimated_input_size().total_tokens()?;
    let micro_usd = (tokens as f64 * price.input_per_million_usd).round() as i64;
    Some(ObservedCost {
        micro_usd,
        confidence: CostConfidence::Estimated,
    })
}

/// The classification a decision acted on, as a zero-weight line in every
/// candidate's explanation — so a reader of `glasshouse route --task` or of
/// a launch sees who classified the work, what it was classed as, and
/// whether line 1459's conservative rules changed the answer, beside the
/// terms that answer then drove.
fn classification_note(answer: &RouterAnswer) -> Contribution {
    Contribution::new("task classification", 0.0, answer.explain())
}

/// Line 1597: what this destination does to provider-side prompt caching.
///
/// Two different questions, and the answer is the *worse* of them:
///
/// - a **fresh** session has no cached prefix at all, whatever its backend
///   is, because the cache is keyed by the conversation and there is no
///   conversation yet;
/// - an **existing** session's cached prefix survives only if the work stays
///   on the backend that built it, which is [`CacheLocality::between`]'s own
///   question and is answered by the one rule in Glasshouse that answers it.
///
/// `current` is what is serving the work now. `None` at a session start,
/// where there is nothing to move away from.
pub fn prompt_cache_state(
    destination: &Destination,
    current: Option<&Destination>,
) -> Contribution {
    if destination.is_fresh() {
        return Contribution::new(
            "prompt-cache state",
            0.0,
            "a fresh session starts with no cached prefix anywhere — there is no conversation \
             for a provider-side cache to have seen",
        );
    }

    let Some(current) = current else {
        // Deliberately `0.0`, and this is the correction that came out of
        // line 1594's own test failing.
        //
        // [`CacheLocality`] is defined as a comparison — `between(from, to)`
        // — and a session start has no `from`. Crediting an existing
        // destination with `Preserved` here would assert that a cached
        // prefix survived a move that was never made, and Glasshouse
        // observes neither a provider cache's presence nor its TTL (see
        // `WARM_SESSION_RELEVANCE_WINDOW_SECONDS`' own doc, which says those
        // expire in minutes). It would also double-count warmth, which
        // [`session_affinity`] already prices once.
        //
        // The consequence, stated rather than hidden: **line 1597's
        // contribution is inert at a session start** and live at a task
        // boundary, which is where the line's word "state" has a state to
        // compare against.
        return Contribution::new(
            "prompt-cache state",
            0.0,
            format!(
                "`{}` is not being moved from anything — a session start has no prior backend \
                 for a cached prefix to have survived, and Glasshouse observes neither a \
                 provider cache's presence nor its lifetime",
                destination.id()
            ),
        );
    };

    let locality = CacheLocality::between(current.backend(), destination.backend());
    let magnitude = match &locality {
        CacheLocality::Preserved => CACHE_PRESERVED,
        CacheLocality::LikelyLost(_) => CACHE_LIKELY_LOST,
        CacheLocality::Lost(_) => 0.0,
    };
    Contribution::new("prompt-cache state", magnitude, locality.to_string())
}

/// Map line 1951's producer, reduced to what map line 1952's term needs: a
/// success rate per (harness, task class), read once per decision the way
/// [`SessionRouter::with_price_table`] reads `pricing.toml` once per
/// decision — see [`SessionRouter::with_harness_efficiency`] for why this is
/// a builder on the router and not a field on [`RouterInputs`].
///
/// `InsufficientEvidence` rows carry nothing `harness_efficiency` can use
/// and are dropped rather than stored as a zero — the same rule
/// `TierOutcome::from_counts` applies to its own gate.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct HarnessEfficiencySummary {
    entries: Vec<HarnessClassEfficiency>,
}

#[derive(Debug, Clone, PartialEq)]
struct HarnessClassEfficiency {
    harness: String,
    task_class: String,
    successful: i64,
    sample_size: i64,
}

impl HarnessEfficiencySummary {
    /// The honest shape for a caller with no ledger open, and for every
    /// caller before this field existed — `harness_efficiency` reads it as
    /// inert.
    pub fn empty() -> Self {
        Self::default()
    }

    /// Built from [`crate::evaluation::EvaluationObservations::outcomes_by_tier_and_harness`]'s
    /// own rows.
    pub fn from_outcomes(rows: &[HarnessTierOutcome]) -> Self {
        let entries = rows
            .iter()
            .filter_map(|row| match row.outcome.verdict {
                TierOutcomeVerdict::Measured {
                    successful,
                    sample_size,
                    ..
                } => Some(HarnessClassEfficiency {
                    harness: row.harness.clone(),
                    task_class: row.outcome.bucket.clone(),
                    successful,
                    sample_size,
                }),
                TierOutcomeVerdict::InsufficientEvidence { .. } => None,
            })
            .collect();
        Self { entries }
    }

    /// `(successful, sample_size)` for one harness and task class, gated at
    /// [`MIN_SAMPLE_FOR_SUMMARY`] — the same floor
    /// `TierOutcome::from_counts` gated the row at before this summary was
    /// built, restated here because a caller comparing two rates must not
    /// trust one that never cleared it.
    fn measured(&self, harness: &str, task_class: &str) -> Option<(i64, i64)> {
        self.entries
            .iter()
            .find(|entry| entry.harness == harness && entry.task_class == task_class)
            .filter(|entry| entry.sample_size >= MIN_SAMPLE_FOR_SUMMARY as i64)
            .map(|entry| (entry.successful, entry.sample_size))
    }
}

/// The bucket [`crate::evaluation::EvaluationObservations::outcomes_by_tier_and_harness`]
/// stores for a classified task — [`RoutingTier::as_str`]'s own vocabulary,
/// rebuilt here from [`TaskRequirements::classification`] the way
/// `main.rs::routed_tier` rebuilds it from a `ClassifiedRouting` this module
/// never holds. `None` for a launch that stated no task — line 1952's own
/// precondition, and `harness_efficiency`'s first inert case.
fn task_class_bucket(requirements: &TaskRequirements) -> Option<String> {
    let answer = requirements.classification.as_ref()?;
    let tier = answer.required_tier();
    let escalated = tier != answer.stated_tier();
    Some(
        RoutingTier::Classified { tier, escalated }
            .as_str()
            .to_owned(),
    )
}

/// Map line 1952: prefer, for a stated task the user has not assigned a
/// harness to, the harness with the better observed efficiency for that
/// task class, and say why.
///
/// Inert (`0.0`, naming why) in every case preservation needs:
/// - no task classified — nothing to compare a rate within;
/// - fewer than [`MIN_SAMPLE_FOR_SUMMARY`] recorded outcomes for THIS
///   destination's harness and class;
/// - no OTHER candidate harness clears the same gate for this class —
///   which is what keeps this term from ever moving work off a harness the
///   user assigned: `launch_session`'s candidate set is already scoped to
///   one harness when the user named one, so there is no "other" to prefer
///   over it, and nothing here rebuilds that scoping.
///
/// Otherwise the magnitude is this destination's success rate minus the mean
/// success rate of the other candidate harnesses that also clear the gate
/// for this class — positive exactly for the harness with the better
/// observed rate, and clamped to `[-1.0, 1.0]`, strictly below a warm
/// session's `1.5` (line 1588): this is a preference among fresh starts, not
/// a reason to abandon a warm one.
pub(super) fn harness_efficiency(
    destination: &Destination,
    summary: &HarnessEfficiencySummary,
    requirements: &TaskRequirements,
    candidate_harnesses: &BTreeSet<&str>,
) -> Contribution {
    const TERM: &str = "harness efficiency";
    let Some(task_class) = task_class_bucket(requirements) else {
        return Contribution::new(
            TERM,
            0.0,
            "no task was classified; nothing to compare a harness's observed rate within",
        );
    };
    let harness = destination.harness().slug();
    let Some((successful, sample_size)) = summary.measured(harness, &task_class) else {
        return Contribution::new(
            TERM,
            0.0,
            format!(
                "inert: fewer than {MIN_SAMPLE_FOR_SUMMARY} recorded outcomes for `{harness}` on \
                 `{task_class}` tasks"
            ),
        );
    };
    let own_rate = successful as f64 / sample_size as f64;

    let other_rates: Vec<f64> = candidate_harnesses
        .iter()
        .filter(|&&other| other != harness)
        .filter_map(|&other| summary.measured(other, &task_class))
        .map(|(other_successful, other_sample)| other_successful as f64 / other_sample as f64)
        .collect();
    if other_rates.is_empty() {
        return Contribution::new(
            TERM,
            0.0,
            format!(
                "inert: no other harness offered here has {MIN_SAMPLE_FOR_SUMMARY} or more recorded \
                 outcomes on `{task_class}` tasks to compare `{harness}` against"
            ),
        );
    }
    let other_mean = other_rates.iter().sum::<f64>() / other_rates.len() as f64;
    let magnitude = (own_rate - other_mean).clamp(-1.0, 1.0);
    Contribution::new(
        TERM,
        magnitude,
        format!(
            "`{harness}` succeeded {successful} of {sample_size} recorded `{task_class}` tasks \
             ({:.0}% success), against {:.0}% for the other harness(es) considered here",
            own_rate * 100.0,
            other_mean * 100.0,
        ),
    )
}

/// Lines 1595 to 1600, in the order a reader compares them: what the harness
/// can do, what the session already holds, what the provider has cached, what
/// is left of the quota, how the provider has behaved, and what the move
/// costs.
#[allow(clippy::too_many_arguments)]
pub(super) fn score(
    destination: &Destination,
    current: Option<&Destination>,
    inputs: &RouterInputs<'_>,
    pressure: &PressureInputs<'_>,
    movement: Option<&TierMovement>,
    pool: &EntitlementPoolView,
    efficiency: &HarnessEfficiencySummary,
    candidate_harnesses: &BTreeSet<&str>,
    prices: &PriceTable,
    weights: &ScoreWeights,
    comparable_output: &[ClassOutput],
    best_effective_ttfc_ms: Option<f64>,
) -> RoutingExplanation {
    let mut explanation = RoutingExplanation::new();
    if let Some(answer) = &inputs.requirements.classification {
        explanation.push(classification_note(answer));
    }
    explanation.push(harness_capability_fit(destination, inputs.overrides));
    explanation.push(pairing_prior(destination, inputs));
    explanation.push(capability_fit(destination, &inputs.requirements));
    // `movement` is `Some` exactly when a tier was stated — `decide_tier_movement`
    // answers `None` otherwise — so the three terms under it keep the
    // preservation clause every tier term has kept: a launch that states no
    // task renders exactly what it rendered before any of them existed.
    if let Some(movement) = movement {
        explanation.push(workload_tier_fit(destination, movement.preferred_tier()));
        // Line 1558, pushed under the same condition and for the same
        // reason: "the cheapest candidate that satisfies the required
        // workload tier" has no subject until a tier has been required.
        explanation.push(cost_preference(destination));
        explanation.push(tier_movement_note(movement));
    }
    let affinity = session_affinity(destination, current, &inputs.requirements);
    let affinity_magnitude = affinity.magnitude();
    explanation.push(affinity);
    // Phase 56A lines 1953/1966–1969: the pool's own terms, right after the
    // affinity factor they share line 1966 with. Inert — every one of them,
    // saying so — for a candidate set that carries fewer than two configured
    // entitlements, which is what keeps a zero-or-one-entitlement user's
    // ranking byte-for-byte what it was.
    if let Some(note) = entitlement_stickiness_note(destination, pool, affinity_magnitude) {
        explanation.push(note);
    }
    explanation.push(entitlement_capacity(destination, pool));
    explanation.push(entitlement_reset_boundary(destination, pool));
    explanation.push(entitlement_throttling(destination, pool));
    explanation.push(entitlement_model_availability(destination, pool));
    // Map line 1952, right after the pool's own terms: both read the
    // candidate set beside the destination, and this term shares the pool
    // axis's own preservation clause — inert for a set this term finds
    // nothing to compare, byte-for-byte what the ranking was before it
    // existed.
    explanation.push(harness_efficiency(
        destination,
        efficiency,
        &inputs.requirements,
        candidate_harnesses,
    ));
    // Lines 1351/1352/1542/1543/1544, beside `harness_efficiency`: the
    // destination's own responsiveness and reliability reading, computed
    // over the candidate set already in hand here (`best_effective_ttfc_ms`)
    // and over this destination's own attached
    // `Destination::route_responsiveness`.
    explanation.push(responsiveness(
        destination,
        inputs,
        current,
        best_effective_ttfc_ms,
    ));
    explanation.push(tool_round_rate(destination));
    explanation.push(observed_pairing_reliability(destination, inputs));
    explanation.push(prompt_cache_state(destination, current));
    explanation.push(quota_pressure(destination, weights));
    // Phase 35D, lines 1570–1577: the band the quota reading falls in, and
    // what the scope's reserve policy makes of it — placed right after the
    // reading it qualifies, so a reader sees the percentage and the band
    // together.
    explanation.push(pressure::capacity_band_pressure(pressure));
    // Phase 32E line 1280: the forecast, right after the band it
    // strengthens — a reader sees "tight" and "and it may not reach its
    // reset" together, in that order. Inert and saying so for every
    // destination with no forecast, which is what keeps a ranking on a
    // build that reads no ledger byte-for-byte what it was.
    explanation.push(pressure::exhaustion_forecast_pressure(pressure));
    explanation.push(pressure::low_tier_spend(pressure));
    explanation.push(provider_health(
        destination,
        inputs.health,
        inputs.now,
        weights,
    ));
    explanation.push(cadence_availability(destination, inputs.health, inputs.now));
    explanation.push(switching_and_bootstrap_cost(destination, current));
    // Line 1538, pushed unconditionally (unlike `cost_preference` above) and
    // reading the un-shadowed parameter so it sees `None` exactly when the
    // block above did not run — see `expected_marginal_cost`'s own doc for
    // why the two must never both price a candidate.
    explanation.push(expected_marginal_cost(
        destination,
        movement,
        prices,
        inputs
            .requirements
            .classification
            .as_ref()
            .map(RouterAnswer::task_class),
        comparable_output,
    ));
    // Line 1302, beside the money term it must never be folded into: its own
    // axis, reading the same `inputs.health` `struggling` already reads and
    // the same `burn_forecast` the exhaustion term above already read.
    explanation.push(request_pool_cost(destination, inputs.health));
    explanation
}
