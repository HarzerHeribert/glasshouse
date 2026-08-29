//! Phase 35B — candidate scoring, map lines 1530–1554 — exercised from
//! outside the crate, entirely through `DisposableRouting`'s public API, the
//! same way `tests/capacity_score.rs` exercises `provider::quota` and
//! `tests/pairing_prior.rs` exercises the pairing prior.
//!
//! Every test here proves something about the **shipped binary's actual
//! decision path**, not a hand-built stand-in: `DisposableRouting::choose`
//! is the exact function `main.rs`'s `disposable_extraction_model` calls
//! (through `memory::RoutedNoModel::new`) to route `glasshouse memory
//! extract`, and `crates/glasshouse/src/main.rs`'s own test module has two
//! tests — `disposable_extraction_model_prefers_a_configured_free_model_and_names_the_reason`
//! and `disposable_extraction_model_reflects_real_cached_capacity_telemetry`
//! — that call `disposable_extraction_model` itself and assert on the
//! explanation this file proves the shape of. See this package's report for
//! the mutation that ties the two together: deleting the scorer's call from
//! `choose`'s free-candidate branch fails four named tests, two of them
//! reached only through `main.rs`.
//!
//! Phase 35B's honest scope, stated once here rather than per test: of the
//! map's 25 lines, this package closes the scorer's own shape (1530, 1532,
//! 1553, 1554), normalized remaining capacity (1536), time until quota reset
//! (1549), protected-reserve policy (1550, though with **no reachable
//! production caller** — see below), and user preference/pinning (1552).
//! Harness-model pairing (1540, 1541 and the boxes that depend on them) does
//! not close: `DisposableRouting`'s candidates carry no harness at all — a
//! disposable job is Glasshouse's own internal call, never run inside one of
//! the ten `IntegrationId` coding harnesses `harness::pairing::PairingQuery`
//! requires — so there is no real value to hand `config::pairing::native_pairing_prior_contribution`
//! without inventing one. Everything else (session affinity, cache
//! temperature, provider health, marginal cost, latency, tool-round
//! throughput, failure-domain diversity, session-switching cost) has no
//! source in this build and is left open with the reason named in the
//! report.

use glasshouse::routing::disposable::{
    CandidateCapacity, DisposableCandidate, DisposableRouting, JobKind, NoResource,
};
use glasshouse::routing::free::{FreePool, FreePreferences};
use glasshouse::routing::{Cost, CredentialId};
use glasshouse::secret::SecretRef;
use std::time::Instant;

fn credential(provider: &str) -> CredentialId {
    CredentialId::new(
        provider,
        SecretRef::Environment {
            var: format!("{}_API_KEY", provider.to_uppercase()),
        },
    )
}

fn free(provider: &str, model: &str) -> DisposableCandidate {
    DisposableCandidate::new(provider, model, credential(provider), Cost::Free)
}

fn metered(provider: &str, model: &str) -> DisposableCandidate {
    DisposableCandidate::new(provider, model, credential(provider), Cost::Metered)
}

/// Map line 1530: every eligible candidate is scored by an inspectable,
/// named function — not one opaque blended number. Map line 1554: the
/// winner's explanation names the reasons it won.
#[test]
fn the_chosen_candidates_explanation_names_every_real_contribution() {
    let routing = DisposableRouting::for_support_work(true, FreePreferences::new());
    let choice = routing
        .choose(
            JobKind::MemoryExtraction,
            &[free("openrouter", "nvidia/nemotron-nano-9b-v2:free")],
            &FreePool::new(),
            Instant::now(),
            None,
        )
        .expect("a free model is configured");

    let names: Vec<&str> = choice
        .explanation()
        .contributions()
        .iter()
        .map(|c| c.name())
        .collect();
    assert!(names.contains(&"cost"), "{names:?}");
    assert!(names.contains(&"user free-resource order"), "{names:?}");
    assert!(
        names.contains(&"normalized remaining capacity"),
        "{names:?}"
    );
    assert!(names.contains(&"time until quota reset"), "{names:?}");
    assert!(
        choice
            .describe()
            .contains("nvidia/nemotron-nano-9b-v2:free")
    );
}

/// Map line 1553: hard eligibility (whether `MeteredUse` permits spending
/// this candidate at all) and soft preference (order, capacity, reset) are
/// never blended into one score — a withheld metered candidate is refused
/// outright, never merely down-weighted, matching Phase 9I line 539's own
/// acceptance condition.
#[test]
fn an_automated_run_refuses_a_metered_candidate_rather_than_scoring_it_low() {
    let routing = DisposableRouting::for_glasshouses_own_run(
        glasshouse::routing::disposable::MeteredUse::for_automated_run(|_| None),
        FreePreferences::new(),
    );
    let err = routing
        .choose(
            JobKind::Evaluation,
            &[metered("openrouter", "an-expensive-model")],
            &FreePool::new(),
            Instant::now(),
            None,
        )
        .expect_err("an automated run must never spend money without an explicit opt-in");
    assert!(matches!(
        err,
        NoResource::NoFreeResourceAndMeteredWithheld { .. }
    ));
}

/// Map line 1552: a user's pin is an explicit high-priority policy input,
/// applied as a hard rule before any candidate is scored — the same design
/// decision Phase 9J's `PairingPreference::Pin` already made, proved here at
/// `DisposableRouting`'s own seam.
#[test]
fn a_user_pin_is_reported_as_the_reason_ranking_never_ran() {
    let now = Instant::now();
    let pinned_key = glasshouse::routing::free::FreeResourceKey::new("openrouter", "pinned-model");
    let routing = DisposableRouting::for_support_work(
        true,
        FreePreferences::new().with_pin(Some(pinned_key)),
    );
    let choice = routing
        .choose(
            JobKind::MemoryExtraction,
            &[
                free("openrouter", "pinned-model"),
                free("openrouter", "another-model"),
            ],
            &FreePool::new(),
            now,
            None,
        )
        .expect("the pinned resource is available");
    assert_eq!(choice.model(), "pinned-model");
    assert!(
        choice
            .explanation()
            .contributions()
            .iter()
            .any(|c| c.name() == "user pin"),
        "{:?}",
        choice.explanation()
    );
}

/// Map line 1550: Phase 32F's protected-reserve policy — the same
/// `evaluate_reserve_spend` function `tests/capacity_score.rs` proves in
/// isolation — is a real gate on the metered-fallback path here too,
/// reached with `WorkloadTier::Leaf` (a disposable job is definitionally
/// leaf-tier work) and `cheaper_adequate_resource_exists: false` (reaching
/// this branch at all already proved no free resource could serve).
///
/// **This gate has no reachable production caller today.** `main.rs`'s own
/// `disposable_candidates` — the only production builder of
/// `DisposableCandidate`s — enumerates a provider's *free* models only; it
/// never constructs a metered one, so `choose`'s metered-fallback branch,
/// though real and mutation-tested here, is never actually reached from
/// `glasshouse memory extract`. Map line 1550 therefore stays open — see
/// this package's report.
#[test]
fn the_reserve_policy_denies_a_distant_reset_on_a_reserve_band_candidate() {
    let capacity = CandidateCapacity::new()
        .with_band(Some(glasshouse::provider::quota::CapacityBand::Reserve))
        .with_seconds_until_reset(Some(7_200));
    let routing = DisposableRouting::for_support_work(true, FreePreferences::new());
    let err = routing
        .choose(
            JobKind::MemoryExtraction,
            &[metered("openrouter", "a-reserved-model").with_capacity(capacity)],
            &FreePool::new(),
            Instant::now(),
            None,
        )
        .expect_err("a distant reset on a Reserve-band candidate must be denied");
    assert!(matches!(err, NoResource::ProtectedReserveDenied { .. }));
}
