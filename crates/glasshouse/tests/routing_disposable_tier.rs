//! GH-CLASSIFY-CALLER — the fifth link, run through the highest production
//! entry point reachable without touching `main.rs` this round.
//!
//! # Why this test does not go through `main.rs`
//!
//! `main.rs`'s one production caller of [`DisposableRouting::choose`],
//! `disposable_extraction_model`, is invoked (as `report_hook_with`'s
//! `model()` closure) before `run_extraction_after_turn` reads this
//! session's events or builds its chunk — there is no request text at the
//! point the routing decision is made in the shipped binary today. That is
//! a real, checked finding, not an assumption: see this package's report
//! for the exact main.rs lines and the patch that would reorder them.
//!
//! So this proves the mechanism at the next entry point down instead:
//! [`glasshouse::memory::RoutedNoModel::new_for_request`], the same
//! `DisposableRouting::choose` wrapper `disposable_extraction_model` already
//! builds in production (`RoutedNoModel::new`) — `new_for_request` differs
//! only in classifying real text first, per this package's objective. It is
//! not yet called by `main.rs`, so per practice §35/§36 this does not by
//! itself prove a *production* caller varies the tier; it proves the
//! mechanism is correct and ready to be wired in by the patch the report
//! names.

use std::time::Instant;

use glasshouse::memory::{ExtractionModel, RoutedNoModel};
use glasshouse::provider::quota::{
    Capacity, CapacityBand, CapacityState, NativeAmount, Pool, Reading, ReadingSource,
    RemainingCapacityScore,
};
use glasshouse::routing::disposable::{
    AutomaticClassificationDecision, CandidateCapacity, DisposableCandidate, DisposableRouting,
    JobKind,
};
use glasshouse::routing::free::{FreePool, FreePreferences, FreeResource, WorkloadOutcome};
use glasshouse::routing::{Cost, CredentialId};
use glasshouse::secret::SecretRef;

fn credential(provider: &str) -> CredentialId {
    CredentialId::new(
        provider,
        SecretRef::Environment {
            var: format!("{}_API_KEY", provider.to_uppercase()),
        },
    )
}

/// One metered candidate, in the Reserve band with a distant reset (the same
/// shape `routing::disposable::tests::the_protected_reserve_policy_gates_the_metered_fallback`
/// and `tests/routing_score.rs`'s reserve test use) — the exact boundary
/// `provider::quota::evaluate_reserve_spend` denies at every tier but
/// `WorkloadTier::Heavy` once a reset is distant. No free candidate is
/// offered, so `choose` must reach the metered-fallback branch that carries
/// the literal this package replaces.
fn reserve_banded_candidate() -> DisposableCandidate {
    let capacity = CandidateCapacity::new()
        .with_band(Some(CapacityBand::Reserve))
        .with_seconds_until_reset(Some(7_200));
    DisposableCandidate::new(
        "openrouter",
        "a-reserved-model",
        credential("openrouter"),
        Cost::Metered,
    )
    .with_capacity(capacity)
}

/// The acceptance test itself: a trivial job and a demanding job, through the
/// same entry point (`RoutedNoModel::new_for_request`, in turn
/// `DisposableRouting::choose`), produce different outcomes for the
/// identical Reserve-band, distant-reset candidate — attributable only to
/// the classification, since nothing else about the call differs.
#[test]
fn a_trivial_and_a_demanding_job_get_different_outcomes_through_the_same_entry_point() {
    let routing = DisposableRouting::for_support_work(true, FreePreferences::new());

    let trivial = RoutedNoModel::new_for_request(
        JobKind::MemoryExtraction,
        "what is a mutex",
        &[reserve_banded_candidate()],
        &routing,
    );
    let trivial_description = trivial.describe();
    assert!(
        trivial_description.contains("protected-reserve policy denied every metered candidate"),
        "a leaf-tier classification must not justify spending the reserve: {trivial_description}"
    );

    let demanding = RoutedNoModel::new_for_request(
        JobKind::MemoryExtraction,
        "run cargo test and fix whatever fails",
        &[reserve_banded_candidate()],
        &routing,
    );
    let demanding_description = demanding.describe();
    assert!(
        demanding_description.contains("a-reserved-model"),
        "a heavy-tier classification must justify spending the reserve (map line 1290): \
         {demanding_description}"
    );
    assert!(
        !demanding_description.contains("protected-reserve policy denied every metered candidate"),
        "{demanding_description}"
    );

    // The two calls differ only in `request_text` — same job kind, same
    // candidate, same routing policy — so the diverging outcome is
    // attributable to the classification alone, not to some other input.
    assert_ne!(trivial_description, demanding_description);
}

/// Confidence at [`glasshouse::routing::classify::Confidence::Low`] escalates
/// the workload tier one step (`conservative_workload_tier`) rather than
/// leaving an ambiguous request at the cheapest tier — the same fail-closed
/// direction `MeteredUse`'s own doc comment describes. An empty request is
/// the heuristic's own worked example of this (Phase 35's evidence: `leaf
/// (conservative: standard)`), so it must not be treated identically to a
/// confidently-trivial one at this call site either.
#[test]
fn an_ambiguous_empty_request_does_not_get_the_confidently_trivial_outcome() {
    let routing = DisposableRouting::for_support_work(true, FreePreferences::new());

    let confidently_trivial = RoutedNoModel::new_for_request(
        JobKind::MemoryExtraction,
        "what is a mutex",
        &[reserve_banded_candidate()],
        &routing,
    );
    let ambiguous = RoutedNoModel::new_for_request(
        JobKind::MemoryExtraction,
        "",
        &[reserve_banded_candidate()],
        &routing,
    );

    // Both are denied at this Reserve-band/distant-reset candidate (neither
    // reaches Heavy), but the escalation still means they are not the same
    // raw tier — proven at `classify_heuristically`'s own level, since
    // `RoutedNoModel::describe` does not print a raw tier to assert on
    // directly.
    let trivial_tier = glasshouse::routing::classify::classify_heuristically("what is a mutex")
        .conservative_workload_tier();
    let ambiguous_tier =
        glasshouse::routing::classify::classify_heuristically("").conservative_workload_tier();
    assert_eq!(
        trivial_tier,
        glasshouse::routing::classify::WorkloadTier::Leaf
    );
    assert_eq!(
        ambiguous_tier,
        glasshouse::routing::classify::WorkloadTier::Standard
    );
    assert_ne!(trivial_tier, ambiguous_tier);

    // Both descriptions still deny this particular candidate — Standard is
    // not Heavy either — recorded so a reader does not mistake this test for
    // a second copy of the trivial-vs-demanding case above.
    assert!(
        confidently_trivial
            .describe()
            .contains("protected-reserve policy denied every metered candidate")
    );
    assert!(
        ambiguous
            .describe()
            .contains("protected-reserve policy denied every metered candidate")
    );
}

// ---------------------------------------------------------------------------
// GH-ROUTING-STICKINESS — map lines 1434, 1441, 1442.
//
// The first two here (1434) go through `DisposableRouting::choose` directly,
// the same production entry point `RoutedNoModel::new_for_request` above
// wraps for `JobKind::MemoryExtraction` — `choose` itself is unchanged in
// what calls it, only in what it eliminates before scoring.
//
// The last two (1441/1442) go through the new
// `DisposableRouting::choose_for_automatic_classification`, which is not yet
// called by `main.rs::automatic_classification_choice` — this package's
// report names the exact insertion point that would make it the production
// path for `glasshouse classify`'s automatic mode. These tests prove the
// mechanism this package built is correct, per practice §35/§36, not that a
// production caller varies stickiness yet.
// ---------------------------------------------------------------------------

/// A real, fully-measured remaining-capacity score at `percent` — the same
/// construction `tests/session_router.rs`'s own `capacity` helper uses,
/// because external tests cannot build a [`RemainingCapacityScore`] any other
/// way; its fields are private on purpose.
fn capacity_score(percent: i64) -> RemainingCapacityScore {
    const OBSERVED: i64 = 1_800_000_000;
    let measured = |value: i64| {
        Capacity::Measured(Reading::new(
            NativeAmount::whole(value, "tokens"),
            OBSERVED,
            ReadingSource::ResponseHeader("x-ratelimit".to_owned()),
        ))
    };
    CapacityState::metered_balance()
        .with_credits(
            Pool::inapplicable()
                .with_remaining(measured(percent))
                .with_limit(measured(100)),
        )
        .remaining_capacity_score()
        .expect("a fully-measured pool always yields a score")
}

/// Map line 1434: a free candidate that would otherwise win the user's own
/// free-resource order is removed outright once it is known to have zero
/// headroom, and the next available candidate is chosen instead — not merely
/// ranked below it.
#[test]
fn a_zero_headroom_candidate_is_not_selected_even_when_it_would_rank_first() {
    let routing = DisposableRouting::for_support_work(true, FreePreferences::new());
    let pool = FreePool::new();

    let exhausted =
        DisposableCandidate::new("alpha", "alpha-model", credential("alpha"), Cost::Free)
            .with_capacity(
                CandidateCapacity::new().with_remaining_capacity(Some(capacity_score(0))),
            );
    let healthy = DisposableCandidate::new("beta", "beta-model", credential("beta"), Cost::Free);

    let choice = routing
        .choose(
            JobKind::Classification,
            &[exhausted, healthy],
            &pool,
            Instant::now(),
            None,
        )
        .expect("a healthy free candidate remains after elimination");
    assert_eq!(
        choice.provider(),
        "beta",
        "the zero-headroom candidate ranked first in the user's order and must still lose"
    );
}

/// Map line 1434's honesty case: a candidate nothing is known about is not
/// eliminated, and it keeps its place in the user's own free-resource order —
/// the elimination step this package adds must not disturb ordering among
/// survivors.
#[test]
fn an_absent_capacity_reading_never_eliminates_a_candidate() {
    let routing = DisposableRouting::for_support_work(true, FreePreferences::new());
    let pool = FreePool::new();

    let unread = DisposableCandidate::new("alpha", "alpha-model", credential("alpha"), Cost::Free);
    let also_unread =
        DisposableCandidate::new("beta", "beta-model", credential("beta"), Cost::Free);

    let choice = routing
        .choose(
            JobKind::Classification,
            &[unread, also_unread],
            &pool,
            Instant::now(),
            None,
        )
        .expect("a free candidate is available");
    assert_eq!(
        choice.provider(),
        "alpha",
        "a candidate nothing is known about must still win the user's own free-resource \
         order — absence must never read as exhaustion"
    );
}

/// Map line 1442: two successive automatic-classification decisions inside
/// the sticky window return the same resource, even when the candidate order
/// changes between calls — proof the second call reused the retained pick
/// rather than re-running the full ranking, since a fresh ranking over the
/// changed order would have picked differently.
#[test]
fn two_decisions_inside_the_window_return_the_same_resource_without_reranking() {
    let routing = DisposableRouting::for_support_work(true, FreePreferences::new());
    let pool = FreePool::new();
    let now = Instant::now();
    let first_unix = 1_800_000_000;

    let alpha = DisposableCandidate::new("alpha", "alpha-model", credential("alpha"), Cost::Free);
    let beta = DisposableCandidate::new("beta", "beta-model", credential("beta"), Cost::Free);

    let first = routing
        .choose_for_automatic_classification(
            &[alpha.clone(), beta.clone()],
            &pool,
            now,
            first_unix,
            None,
            None,
        )
        .expect("a free candidate is available");
    let AutomaticClassificationDecision::Fresh(choice, pick) = first else {
        panic!("a call with no retained pick must make a fresh decision: {first:?}");
    };
    assert_eq!(choice.provider(), "alpha");

    // Reversed order: a fresh `choose` here would pick `beta` first. Reusing
    // the retained pick must still return `alpha`.
    let second = routing
        .choose_for_automatic_classification(
            &[beta.clone(), alpha.clone()],
            &pool,
            now,
            first_unix + 5,
            None,
            Some(pick.clone()),
        )
        .expect("the retained pick is still present and healthy");
    assert_eq!(
        second,
        AutomaticClassificationDecision::Retained(pick),
        "a retained pick inside the window must be reused, not re-ranked"
    );
}

/// Map line 1441: a retained pick whose provider has since become unhealthy
/// is not returned — stickiness must not outlive the healthiness it was
/// predicated on. A fresh decision is made instead.
#[test]
fn a_retained_pick_whose_provider_turned_unhealthy_is_not_returned() {
    let routing = DisposableRouting::for_support_work(true, FreePreferences::new());
    let mut pool = FreePool::new();
    let now = Instant::now();
    let first_unix = 1_800_000_000;

    let alpha = DisposableCandidate::new("alpha", "alpha-model", credential("alpha"), Cost::Free);
    let beta = DisposableCandidate::new("beta", "beta-model", credential("beta"), Cost::Free);

    let first = routing
        .choose_for_automatic_classification(
            &[alpha.clone(), beta.clone()],
            &pool,
            now,
            first_unix,
            None,
            None,
        )
        .expect("a free candidate is available");
    let AutomaticClassificationDecision::Fresh(choice, pick) = first else {
        panic!("a call with no retained pick must make a fresh decision: {first:?}");
    };
    assert_eq!(choice.provider(), "alpha");

    // Alpha's credential is rejected between calls — the same health signal
    // 1433 already reaches free candidates with.
    pool.observe(
        &FreeResource::new(credential("alpha"), "alpha-model"),
        WorkloadOutcome::CredentialRejected,
        now,
    );

    let second = routing
        .choose_for_automatic_classification(
            &[alpha.clone(), beta.clone()],
            &pool,
            now,
            first_unix + 5,
            None,
            Some(pick),
        )
        .expect("beta remains available");
    let AutomaticClassificationDecision::Fresh(choice, _) = second else {
        panic!(
            "a retained pick whose provider turned unhealthy must trigger a fresh decision: \
             {second:?}"
        );
    };
    assert_eq!(choice.provider(), "beta");
}

/// A missing or corrupt on-disk record must decide fresh rather than error —
/// `RoutingStickyCache::load` never surfaces a parse failure to a caller; see
/// `provider::telemetry::routing_sticky_cache_tests` for the cache-level
/// proof (acceptance test 5's `RoutingStickyCache::load` half). This is the
/// caller-facing half: passing `None` (what a failed load already collapses
/// to) behaves exactly like "decide fresh", never an error.
#[test]
fn a_caller_that_could_not_load_a_pick_still_decides_fresh_without_erroring() {
    let routing = DisposableRouting::for_support_work(true, FreePreferences::new());
    let pool = FreePool::new();

    let alpha = DisposableCandidate::new("alpha", "alpha-model", credential("alpha"), Cost::Free);

    let decision = routing
        .choose_for_automatic_classification(
            &[alpha],
            &pool,
            Instant::now(),
            1_800_000_000,
            None,
            None,
        )
        .expect("a missing retained pick must not fail the classification");
    assert!(matches!(
        decision,
        AutomaticClassificationDecision::Fresh(..)
    ));
}
