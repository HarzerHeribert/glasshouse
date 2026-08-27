//! Phase 32D/32F: the normalized remaining-capacity score, its bands, and
//! the protected-reserve policy that reads them — exercised from outside the
//! crate, the same way `provider_discovery.rs` proves `provider::quota`'s
//! other guarantees.
//!
//! No test here reaches the network: every [`CapacityState`] is built by
//! hand from the public builder API, the same way `provider::quota`'s own
//! test module builds one.

use glasshouse::provider::quota::{
    Capacity, CapacityBand, CapacityBandThresholds, CapacityState, LimitingUnit, LimitingUnits,
    NativeAmount, Pool, RESET_DISTANT_SECONDS, RESET_IMMINENT_SECONDS, RateCeilings, Reading,
    ReadingSource, ReserveDecisionInputs, TokenBudget, evaluate_reserve_spend,
};
use glasshouse::routing::classify::WorkloadTier;

const OBSERVED: i64 = 1_800_000_000;

fn header(name: &str) -> ReadingSource {
    ReadingSource::ResponseHeader(name.to_owned())
}

fn measured(value: i64, unit: &str, source: ReadingSource) -> Capacity<NativeAmount> {
    Capacity::Measured(Reading::new(
        NativeAmount::whole(value, unit),
        OBSERVED,
        source,
    ))
}

fn pool(remaining: i64, limit: i64, unit: &str) -> Pool {
    Pool::inapplicable()
        .with_remaining(measured(remaining, unit, header("x-ratelimit-remaining")))
        .with_limit(measured(limit, unit, header("x-ratelimit-limit")))
}

fn remote_state() -> CapacityState {
    CapacityState::metered_balance()
}

// --- capability map line 1259: a score between zero and one, never a bare
// float ------------------------------------------------------------------

#[test]
fn a_remote_resource_with_nothing_measured_has_no_score() {
    let state = remote_state();
    assert!(
        state.remaining_capacity_score().is_none(),
        "no pool was ever read on both halves, so there is nothing to score"
    );
}

#[test]
fn a_subscription_has_no_score_because_its_pools_are_opaque_not_unmeasured() {
    let state = CapacityState::opaque_subscription();
    assert!(state.remaining_capacity_score().is_none());
}

#[test]
fn the_gateway_has_no_score_because_its_pools_are_delegated() {
    let state = CapacityState::delegated_to_upstream();
    assert!(state.remaining_capacity_score().is_none());
}

// --- capability map line 1260: the limiting dimension, not an average ----

#[test]
fn the_score_is_bound_by_the_tightest_dimension_not_averaged() {
    let state = remote_state()
        .with_tokens(TokenBudget::uniform(pool(9_000, 10_000, "tokens"))) // 90%
        .with_credits(pool(2, 100, "credits")); // 2%
    let score = state
        .remaining_capacity_score()
        .expect("both token and credit pools are measured");
    assert_eq!(score.dimension(), "credits");
    assert!(
        (score.fraction() - 0.02).abs() < 1e-9,
        "the binding dimension is the tightest one, not the average of 90% and 2%: {}",
        score.fraction()
    );
}

// --- capability map line 1261: short-window request pressure, widened ----

#[test]
fn a_tighter_per_minute_ceiling_becomes_visible_as_the_binding_dimension() {
    // The general request pool has plenty of headroom (900/1000 = 90%). The
    // per-minute rate ceiling has no "remaining" reading of its own — that
    // is the whole gap line 1261 names — so this pairs the same remaining
    // reading against the tighter of the two limits. 900 against a
    // 2000-wide per-minute ceiling is 45%, tighter than the general pool's
    // 90%, and must become the binding dimension.
    let state = remote_state()
        .with_requests(pool(900, 1_000, "requests"))
        .with_rate_ceilings(
            RateCeilings::uniform(Capacity::Inapplicable, Capacity::Inapplicable)
                .with_requests_per_minute(measured(2_000, "requests", header("ratelimit-limit"))),
        );
    let score = state
        .remaining_capacity_score()
        .expect("the requests pool and the per-minute ceiling are both measured");
    assert_eq!(score.dimension(), "requests per minute");
    assert!(
        (score.fraction() - (900.0 / 2_000.0)).abs() < 1e-6,
        "expected the tighter per-minute ceiling to bind: {}",
        score.fraction()
    );
}

#[test]
fn a_per_minute_ceiling_in_a_different_unit_is_never_paired() {
    // The remaining reading is in "requests"; the ceiling is stated in
    // "tokens" here on purpose, to prove commensurability is checked rather
    // than assumed.
    let state = remote_state()
        .with_requests(pool(50, 100, "requests"))
        .with_rate_ceilings(
            RateCeilings::uniform(Capacity::Inapplicable, Capacity::Inapplicable)
                .with_requests_per_minute(measured(40, "tokens", header("ratelimit-limit"))),
        );
    let score = state
        .remaining_capacity_score()
        .expect("the requests pool alone is measured on both halves");
    assert_eq!(
        score.dimension(),
        "requests",
        "an incommensurable ceiling must never be paired with the remaining reading"
    );
}

// --- capability map line 1267: local inference, honestly ------------------

#[test]
fn local_inference_scores_high_and_says_it_has_no_latency_evidence() {
    let state = CapacityState::unmetered_local();
    let score = state
        .remaining_capacity_score()
        .expect("local inference must not be unscoreable just because it has no pools");
    assert_eq!(score.fraction(), 1.0);
    let (_, _, source) = score
        .percent()
        .estimated()
        .expect("a score with no measurement behind it must never render as exact");
    assert!(
        source.contains("no latency") && source.contains("concurrency"),
        "the score must say plainly that it invented no measurement: {source}"
    );
}

#[test]
fn the_gateway_and_a_remote_provider_still_answer_the_local_special_case_apart() {
    // A resource whose LimitingUnits is None is exactly local inference;
    // Delegated (the gateway) and These (a remote provider) must not trigger
    // the same honest-high shortcut.
    assert!(
        CapacityState::delegated_to_upstream()
            .remaining_capacity_score()
            .is_none()
    );
    assert!(remote_state().remaining_capacity_score().is_none());
}

// --- capability map line 1266: confidence attenuates toward caution, never
// toward optimism -----------------------------------------------------------

#[test]
fn a_low_confidence_estimate_of_ninety_percent_does_not_outrank_a_high_confidence_measured_eighty()
{
    let low_confidence_ninety = remote_state().with_credits(
        Pool::inapplicable()
            .with_remaining(Capacity::Measured(Reading::new(
                NativeAmount::whole(90, "credits"),
                OBSERVED,
                ReadingSource::InferredEstimate("a heuristic guess about spend".to_owned()),
            )))
            .with_limit(measured(100, "credits", header("x-ratelimit-limit"))),
    );
    let high_confidence_eighty = remote_state().with_credits(pool(80, 100, "credits"));

    let low = low_confidence_ninety
        .remaining_capacity_score()
        .expect("both halves of the credits pool are present");
    let high = high_confidence_eighty
        .remaining_capacity_score()
        .expect("both halves of the credits pool are present");

    assert!(low.percent().estimated().is_some(), "expected an estimate");
    assert!(
        high.percent().exact().is_some(),
        "expected an exact reading"
    );
    assert!(
        low.fraction() > high.fraction(),
        "sanity: the raw figures really are 90% vs 80%"
    );
    assert!(
        low.routing_fraction() < high.routing_fraction(),
        "a low-confidence 90% estimate ({}) must not outrank a high-confidence measured 80% \
         ({}) for routing comparison",
        low.routing_fraction(),
        high.routing_fraction()
    );
}

#[test]
fn an_exact_reading_is_never_attenuated() {
    let state = remote_state().with_credits(pool(80, 100, "credits"));
    let score = state.remaining_capacity_score().unwrap();
    assert_eq!(score.routing_fraction(), score.fraction());
}

// --- capability map lines 1264/1265: reset proximity adjusts effective
// availability, never the measured remaining --------------------------------

#[test]
fn an_unknown_reset_leaves_the_effective_value_equal_to_the_routing_fraction() {
    let state = remote_state().with_credits(pool(10, 100, "credits"));
    let score = state.remaining_capacity_score().unwrap();
    assert_eq!(score.effective(None), score.routing_fraction());
}

#[test]
fn an_imminent_reset_raises_effective_availability_toward_full() {
    let state = remote_state().with_credits(pool(10, 100, "credits"));
    let score = state.remaining_capacity_score().unwrap();
    let raw = score.routing_fraction();
    let effective = score.effective(Some(0));
    assert!(
        effective > raw,
        "an imminent reset must raise effective availability above the raw score: {effective} \
         vs {raw}"
    );
    assert!(
        (effective - 1.0).abs() < 1e-9,
        "a reset happening now is maximally imminent"
    );
}

#[test]
fn a_distant_reset_does_not_raise_effective_availability_above_the_routing_fraction() {
    let state = remote_state().with_credits(pool(10, 100, "credits"));
    let score = state.remaining_capacity_score().unwrap();
    let raw = score.routing_fraction();
    let effective = score.effective(Some(RESET_DISTANT_SECONDS + 10_000));
    assert_eq!(
        effective, raw,
        "a distant reset must be treated exactly like no reset was known"
    );
}

#[test]
fn the_raw_score_is_never_mutated_by_computing_an_effective_value() {
    let state = remote_state().with_credits(pool(10, 100, "credits"));
    let score = state.remaining_capacity_score().unwrap();
    let before = score.fraction();
    let _ = score.effective(Some(0));
    let _ = score.effective(Some(RESET_DISTANT_SECONDS * 10));
    assert_eq!(score.fraction(), before);
}

// --- capability map line 1268: the score joins native units, never replaces
// them ------------------------------------------------------------------

#[test]
fn the_normalized_capacity_this_score_was_derived_from_still_carries_its_native_readings() {
    let state = remote_state().with_credits(pool(10, 100, "credits"));
    let (_, normalized) = state.normalized().expect("the credits pool is measured");
    assert_eq!(normalized.remaining().value().unit(), "credits");
    assert_eq!(normalized.limit().value().value(), 100);
}

// --- capability map line 1270: bands are an enum, thresholds fail closed --

#[test]
fn exhausted_sorts_lowest_and_plenty_highest() {
    assert!(CapacityBand::Exhausted < CapacityBand::Reserve);
    assert!(CapacityBand::Reserve < CapacityBand::Tight);
    assert!(CapacityBand::Tight < CapacityBand::Healthy);
    assert!(CapacityBand::Healthy < CapacityBand::Plenty);
}

#[test]
fn the_default_thresholds_classify_every_boundary_at_its_lower_edge() {
    let t = CapacityBandThresholds::DEFAULT;
    assert_eq!(t.band_for_percent(0), CapacityBand::Exhausted);
    assert_eq!(
        t.band_for_percent(t.exhausted_percent()),
        CapacityBand::Reserve
    );
    assert_eq!(t.band_for_percent(t.reserve_percent()), CapacityBand::Tight);
    assert_eq!(t.band_for_percent(t.tight_percent()), CapacityBand::Healthy);
    assert_eq!(
        t.band_for_percent(t.healthy_percent()),
        CapacityBand::Plenty
    );
    assert_eq!(t.band_for_percent(100), CapacityBand::Plenty);
}

#[test]
fn a_non_monotonic_set_of_thresholds_is_refused_rather_than_sorted() {
    // reserve (50) above tight (30) — not ascending.
    let result = CapacityBandThresholds::new(2, 50, 30, 70);
    assert!(
        result.is_err(),
        "a non-monotonic set of thresholds must be refused, not silently sorted into shape"
    );
}

#[test]
fn a_monotonic_set_of_thresholds_including_ties_is_accepted() {
    assert!(CapacityBandThresholds::new(0, 0, 50, 100).is_ok());
}

#[test]
fn a_resources_own_reserve_percentage_moves_where_the_reserve_band_begins() {
    let default = CapacityBandThresholds::DEFAULT;
    let widened = default.with_resource_reserve(30);
    assert_eq!(
        widened.band_for_percent(25),
        CapacityBand::Reserve,
        "a resource whose own protected reserve percent is 30 must be in Reserve at 25%, even \
         though the global default would have called that Tight"
    );
    assert_eq!(default.band_for_percent(25), CapacityBand::Tight);
}

#[test]
fn a_resources_reserve_percentage_may_widen_past_the_default_tight_boundary() {
    // A premium resource protecting most of its capacity is a legitimate
    // policy, not an error: everything up to 90% becomes Reserve, and
    // CapacityBand::Tight simply never fires for this resource.
    let widened = CapacityBandThresholds::DEFAULT.with_resource_reserve(90);
    assert_eq!(widened.band_for_percent(50), CapacityBand::Reserve);
    assert_eq!(widened.band_for_percent(89), CapacityBand::Reserve);
    assert_eq!(widened.band_for_percent(95), CapacityBand::Plenty);
}

// --- Phase 32F: reserve-spend policy functions, not a scheduler -----------

fn base_inputs() -> ReserveDecisionInputs {
    ReserveDecisionInputs {
        band: CapacityBand::Reserve,
        tier: WorkloadTier::Standard,
        cheaper_adequate_resource_exists: false,
        user_override: false,
        seconds_until_reset: None,
        task_nearly_complete: false,
    }
}

#[test]
fn line_1294_an_almost_complete_task_is_never_moved_for_a_reserve_threshold() {
    let inputs = ReserveDecisionInputs {
        band: CapacityBand::Exhausted,
        tier: WorkloadTier::Leaf,
        cheaper_adequate_resource_exists: true,
        user_override: false,
        seconds_until_reset: Some(RESET_DISTANT_SECONDS * 10),
        task_nearly_complete: true,
    };
    let decision = evaluate_reserve_spend(inputs);
    assert!(
        decision.is_allowed(),
        "an almost-complete task must never be denied solely because a reserve threshold was \
         crossed: {}",
        decision.reason()
    );
}

#[test]
fn line_1291_a_user_override_wins_even_in_the_reserve_band() {
    let inputs = ReserveDecisionInputs {
        user_override: true,
        ..base_inputs()
    };
    assert!(evaluate_reserve_spend(inputs).is_allowed());
}

#[test]
fn a_band_above_reserve_is_always_allowed_regardless_of_tier_or_alternatives() {
    let inputs = ReserveDecisionInputs {
        band: CapacityBand::Healthy,
        cheaper_adequate_resource_exists: true,
        ..base_inputs()
    };
    assert!(evaluate_reserve_spend(inputs).is_allowed());
}

#[test]
fn line_1292_an_imminent_reset_makes_the_policy_permissive() {
    let inputs = ReserveDecisionInputs {
        seconds_until_reset: Some(RESET_IMMINENT_SECONDS - 1),
        cheaper_adequate_resource_exists: true,
        ..base_inputs()
    };
    assert!(evaluate_reserve_spend(inputs).is_allowed());
}

#[test]
fn a_distant_reset_makes_the_policy_conservative_even_with_no_alternative() {
    let inputs = ReserveDecisionInputs {
        seconds_until_reset: Some(RESET_DISTANT_SECONDS + 1),
        cheaper_adequate_resource_exists: false,
        tier: WorkloadTier::Standard,
        ..base_inputs()
    };
    let decision = evaluate_reserve_spend(inputs);
    assert!(
        !decision.is_allowed(),
        "a distant reset must make the reserve policy strictly more conservative, denying even \
         with no cheaper alternative, unless the task needs the heavy tier: {}",
        decision.reason()
    );
}

#[test]
fn a_distant_reset_still_allows_heavy_tier_work() {
    let inputs = ReserveDecisionInputs {
        seconds_until_reset: Some(RESET_DISTANT_SECONDS + 1),
        tier: WorkloadTier::Heavy,
        ..base_inputs()
    };
    assert!(evaluate_reserve_spend(inputs).is_allowed());
}

#[test]
fn line_1290_heavy_tier_work_may_spend_the_reserve() {
    let inputs = ReserveDecisionInputs {
        tier: WorkloadTier::Heavy,
        cheaper_adequate_resource_exists: true,
        ..base_inputs()
    };
    assert!(evaluate_reserve_spend(inputs).is_allowed());
}

#[test]
fn line_1289_low_tier_work_does_not_spend_the_reserve_while_something_cheaper_is_adequate() {
    let inputs = ReserveDecisionInputs {
        tier: WorkloadTier::Leaf,
        cheaper_adequate_resource_exists: true,
        ..base_inputs()
    };
    let decision = evaluate_reserve_spend(inputs);
    assert!(!decision.is_allowed(), "{}", decision.reason());
}

#[test]
fn low_tier_work_may_spend_the_reserve_when_nothing_cheaper_is_adequate() {
    let inputs = ReserveDecisionInputs {
        tier: WorkloadTier::Leaf,
        cheaper_adequate_resource_exists: false,
        ..base_inputs()
    };
    assert!(evaluate_reserve_spend(inputs).is_allowed());
}

#[test]
fn line_1288_a_limiting_unit_can_be_named_for_a_metered_resource() {
    // Not a behavioural test of the decision function — a check that the
    // vocabulary evaluate_reserve_spend's callers reach for (limiting units,
    // bands) actually composes with a metered resource's own classification.
    let state = remote_state();
    assert!(state.limiting_units().includes(LimitingUnit::Credits));
    assert!(!matches!(state.limiting_units(), LimitingUnits::None));
}
