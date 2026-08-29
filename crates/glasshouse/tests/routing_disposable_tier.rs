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

use glasshouse::memory::{ExtractionModel, RoutedNoModel};
use glasshouse::provider::quota::CapacityBand;
use glasshouse::routing::disposable::{
    CandidateCapacity, DisposableCandidate, DisposableRouting, JobKind,
};
use glasshouse::routing::free::FreePreferences;
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
