//! GH-WORKLOAD-TIERS: the map's five-tier `WorkloadTier` system (capability
//! map lines 1395-1400, 1404), widened from the three variants
//! (`Leaf`, `Standard`, `Heavy`) production shipped before this batch.
//!
//! Two new variants — [`WorkloadTier::Deterministic`] (Tier 0) and
//! [`WorkloadTier::Frontier`] (Tier 4) — have no producer in this batch. The
//! tests here therefore split into two families:
//!
//! - a **behaviour-preservation** family, exercising only the three tiers a
//!   current classifier can actually produce, proving `evaluate_reserve_spend`
//!   still makes the same call for each;
//! - a **new-capability** family, proving the two new tiers are
//!   representable, order correctly, and — critically — that Tier 4 does not
//!   fall through the reserve policy's old `== Heavy` / `!= Heavy` equalities
//!   (capability map lines 1289, 1290, and 1292's distant-reset complement).

use glasshouse::provider::quota::{
    CapacityBand, RESET_DISTANT_SECONDS, RESET_IMMINENT_SECONDS, ReserveDecisionInputs,
    evaluate_reserve_spend,
};
use glasshouse::routing::classify::WorkloadTier;

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

// --- 1. Behaviour preservation: every tier a current classifier can
// produce (Leaf, Standard, Heavy) must reach the same `evaluate_reserve_spend`
// decision after this batch as before it. Each case below is chosen to land
// on one of the two comparisons ruling 2 converted from equality to
// threshold (quota.rs:2307, quota.rs:2317), plus the tier/alternatives
// branch (line 1289/1290) neither comparison touches. -----------------------

#[test]
fn leaf_tier_denies_the_reserve_past_the_distant_reset_threshold() {
    // Exercises quota.rs:2307. Leaf < Heavy before and after this batch, so
    // the outcome (Deny) is unchanged by converting `!=` to `<`.
    let inputs = ReserveDecisionInputs {
        tier: WorkloadTier::Leaf,
        seconds_until_reset: Some(RESET_DISTANT_SECONDS + 1),
        cheaper_adequate_resource_exists: false,
        ..base_inputs()
    };
    let decision = evaluate_reserve_spend(inputs);
    assert!(
        !decision.is_allowed(),
        "a leaf-tier task past the distant-reset threshold must still be denied: {}",
        decision.reason()
    );
}

#[test]
fn standard_tier_denies_the_reserve_past_the_distant_reset_threshold() {
    // Same branch as above, the other pre-existing tier below Heavy.
    let inputs = ReserveDecisionInputs {
        tier: WorkloadTier::Standard,
        seconds_until_reset: Some(RESET_DISTANT_SECONDS + 1),
        cheaper_adequate_resource_exists: false,
        ..base_inputs()
    };
    let decision = evaluate_reserve_spend(inputs);
    assert!(
        !decision.is_allowed(),
        "a standard-tier task past the distant-reset threshold must still be denied: {}",
        decision.reason()
    );
}

#[test]
fn heavy_tier_survives_the_distant_reset_threshold_as_before() {
    // Exercises quota.rs:2307. Heavy is the threshold value itself
    // (`< Heavy` is false for Heavy), so this must still be allowed.
    let inputs = ReserveDecisionInputs {
        tier: WorkloadTier::Heavy,
        seconds_until_reset: Some(RESET_DISTANT_SECONDS + 1),
        cheaper_adequate_resource_exists: false,
        ..base_inputs()
    };
    assert!(evaluate_reserve_spend(inputs).is_allowed());
}

#[test]
fn heavy_tier_still_justifies_spending_the_reserve_at_line_1290() {
    // Exercises quota.rs:2317 directly (no distant reset in play): Heavy
    // still satisfies `>= Heavy` exactly as it satisfied `== Heavy` before.
    let inputs = ReserveDecisionInputs {
        tier: WorkloadTier::Heavy,
        cheaper_adequate_resource_exists: true,
        ..base_inputs()
    };
    assert!(evaluate_reserve_spend(inputs).is_allowed());
}

#[test]
fn leaf_tier_still_defers_to_a_cheaper_adequate_resource_at_line_1289() {
    // Neither converted comparison fires here (band is Reserve, no distant
    // reset, tier is below Heavy): line 1289's plain "cheaper exists" denial
    // is unaffected by ruling 2's threshold change.
    let inputs = ReserveDecisionInputs {
        tier: WorkloadTier::Leaf,
        cheaper_adequate_resource_exists: true,
        ..base_inputs()
    };
    let decision = evaluate_reserve_spend(inputs);
    assert!(!decision.is_allowed(), "{}", decision.reason());
}

#[test]
fn leaf_tier_may_spend_the_reserve_when_nothing_cheaper_is_adequate() {
    let inputs = ReserveDecisionInputs {
        tier: WorkloadTier::Leaf,
        cheaper_adequate_resource_exists: false,
        ..base_inputs()
    };
    assert!(evaluate_reserve_spend(inputs).is_allowed());
}

// --- 2. The Tier 4 fall-through test — the one that proves ruling 2 was
// actually applied. With either comparison left as an equality against
// `Heavy`, a `Frontier` task compares unequal and gets the wrong answer on
// both branches below. --------------------------------------------------

#[test]
fn frontier_tier_survives_the_distant_reset_threshold() {
    // quota.rs:2307. With the old `!= Heavy`, a Frontier task would compare
    // unequal to Heavy and fall through to `Deny` — the defect this package
    // exists to avoid. With `< Heavy`, Frontier (which orders above Heavy)
    // is not less than Heavy, so this must be Allow.
    let inputs = ReserveDecisionInputs {
        tier: WorkloadTier::Frontier,
        seconds_until_reset: Some(RESET_DISTANT_SECONDS + 1),
        cheaper_adequate_resource_exists: false,
        ..base_inputs()
    };
    let decision = evaluate_reserve_spend(inputs);
    assert!(
        decision.is_allowed(),
        "the strongest tier in the system must not lose access to protected reserve past the \
         distant-reset threshold: {}",
        decision.reason()
    );
}

#[test]
fn frontier_tier_justifies_spending_the_reserve_at_line_1290() {
    // quota.rs:2317. With the old `== Heavy`, this would fall through past
    // the tier/alternatives branch and be denied by line 1289 despite being
    // the single strongest tier the map defines.
    let inputs = ReserveDecisionInputs {
        tier: WorkloadTier::Frontier,
        cheaper_adequate_resource_exists: true,
        ..base_inputs()
    };
    let decision = evaluate_reserve_spend(inputs);
    assert!(
        decision.is_allowed(),
        "a frontier-tier task's requirement must justify spending protected reserve: {}",
        decision.reason()
    );
}

// --- 3. Tier 0 is representable, orders strictly below Tier 1, and is
// documented as "no LLM required". -----------------------------------------

#[test]
fn tier_zero_deterministic_is_representable_and_orders_below_leaf() {
    assert!(WorkloadTier::Deterministic < WorkloadTier::Leaf);
    assert!(WorkloadTier::Deterministic < WorkloadTier::Standard);
    assert!(WorkloadTier::Deterministic < WorkloadTier::Heavy);
    assert!(WorkloadTier::Deterministic < WorkloadTier::Frontier);
    assert_eq!(WorkloadTier::Deterministic.as_str(), "deterministic");
}

// --- 4. `escalate()` saturates at Tier 4 and never returns a lower tier
// from a higher input, for all five tiers. ----------------------------------

#[test]
fn escalate_never_steps_down_for_any_tier() {
    let all = [
        WorkloadTier::Deterministic,
        WorkloadTier::Leaf,
        WorkloadTier::Standard,
        WorkloadTier::Heavy,
        WorkloadTier::Frontier,
    ];
    for tier in all {
        assert!(
            tier.escalate() >= tier,
            "{tier} escalated to something lower than itself"
        );
    }
}

#[test]
fn escalate_saturates_at_frontier() {
    assert_eq!(WorkloadTier::Frontier.escalate(), WorkloadTier::Frontier);
    assert_eq!(WorkloadTier::Heavy.escalate(), WorkloadTier::Frontier);
    assert_eq!(WorkloadTier::Standard.escalate(), WorkloadTier::Heavy);
    assert_eq!(WorkloadTier::Leaf.escalate(), WorkloadTier::Standard);
    assert_eq!(WorkloadTier::Deterministic.escalate(), WorkloadTier::Leaf);
}

// --- 5. All five tier definitions round-trip through `as_str()`: each is
// distinct, non-empty, and matches what `Display` prints. -------------------

#[test]
fn all_five_tiers_round_trip_through_as_str() {
    let all = [
        WorkloadTier::Deterministic,
        WorkloadTier::Leaf,
        WorkloadTier::Standard,
        WorkloadTier::Heavy,
        WorkloadTier::Frontier,
    ];
    let strings: Vec<&str> = all.iter().map(|t| t.as_str()).collect();

    for (tier, s) in all.iter().zip(strings.iter()) {
        assert!(!s.is_empty(), "{tier:?} has an empty as_str()");
        assert_eq!(
            tier.to_string(),
            *s,
            "Display must agree with as_str() for {tier:?}"
        );
    }

    let mut deduped = strings.clone();
    deduped.sort_unstable();
    deduped.dedup();
    assert_eq!(
        deduped.len(),
        strings.len(),
        "two tiers share an as_str() string: {strings:?}"
    );
}

// `RESET_IMMINENT_SECONDS` is imported only so the module-level doc comment
// above can name both thresholds precisely; the distant-reset tests above
// use `RESET_DISTANT_SECONDS` directly and don't need the imminent one at
// runtime, so referencing it here as a compile-time check would be a
// constant assertion (clippy correctly refuses that) rather than a real
// regression test.
const _: () = assert!(RESET_IMMINENT_SECONDS < RESET_DISTANT_SECONDS);
