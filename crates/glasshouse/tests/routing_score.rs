//! Phase 35B — candidate scoring, map lines 1530–1554 — exercised from
//! outside the crate, entirely through `DisposableRouting`'s public API, the
//! same way `tests/capacity_score.rs` exercises `provider::quota` and
//! `tests/pairing_prior.rs` exercises the pairing prior.
//!
//! Every test here proves something about the **shipped binary's actual
//! decision path**, not a hand-built stand-in: `DisposableRouting::choose`
//! is the exact function `main.rs`'s `disposable_extraction_model` calls
//! (through `memory::RoutedModel::new`) to route `glasshouse memory
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

use glasshouse::harness::pairing::PairingOverrides;
use glasshouse::integrations::IntegrationId;
use glasshouse::routing::disposable::{
    CandidateCapacity, DisposableCandidate, DisposableRouting, JobKind, NoResource,
};
use glasshouse::routing::evidence::RouteResponsiveness;
use glasshouse::routing::free::{FreePool, FreePreferences};
use glasshouse::routing::session::{
    Destination, RouterInputs, RoutingMoment, SessionRouter, TaskRequirements,
};
use glasshouse::routing::{AssignedModel, Backend, Cost, CredentialId, ToolSemantics};
use glasshouse::secret::SecretRef;
use std::collections::BTreeMap;
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

// ---------------------------------------------------------------------------
// `GH-RESPONSIVENESS-TERMS` — map lines 1351/1352/1542/1543/1544, through
// `SessionRouter::choose` rather than `DisposableRouting`: a disposable job
// carries no harness or ledger reading (this file's own header explains
// why), so the responsiveness terms are proved at the session router's own
// seam, the same way `tests/pairing_prior.rs`'s second half proves
// `config::pairing`'s prior through `InteractiveRouting`.
// ---------------------------------------------------------------------------

fn responsiveness_backend(provider: &str, model: &str, var: &str) -> Backend {
    Backend::new(
        provider,
        "anthropic-messages",
        AssignedModel::named(model),
        CredentialId::new(
            provider,
            SecretRef::Environment {
                var: var.to_owned(),
            },
        ),
        Cost::Metered,
        ToolSemantics::Verified,
    )
}

fn no_pairing_overrides() -> PairingOverrides {
    PairingOverrides::from_parts("no configuration", BTreeMap::new(), BTreeMap::new())
}

fn tool_using_requirements() -> TaskRequirements {
    TaskRequirements {
        needs_tool_calls: true,
        ..TaskRequirements::default()
    }
}

/// A route that answers fast but fails half the time — `raw_ttfc_ms: 800`,
/// `failure_rate: 0.5`, both at ten observations (twice
/// `MIN_SAMPLE_FOR_SUMMARY`). Effective TTFC: `800 / (1 - 0.5) = 1600`.
fn fast_and_flaky() -> RouteResponsiveness {
    RouteResponsiveness {
        raw_ttfc_ms: Some(800.0),
        raw_ttfc_sample: 10,
        failure_rate: Some(0.5),
        failure_rate_sample: 10,
        rounds_per_minute: None,
        rounds_per_minute_sample: 0,
    }
}

/// A route that answers slower but never fails — `raw_ttfc_ms: 1200`,
/// `failure_rate: 0.0`, same sample size. Effective TTFC:
/// `1200 / (1 - 0.0) = 1200` — **lower** than [`fast_and_flaky`]'s 1600,
/// which is the whole point: reliability-adjusted, the "slower" route is
/// genuinely faster.
fn slower_and_sound() -> RouteResponsiveness {
    RouteResponsiveness {
        raw_ttfc_ms: Some(1200.0),
        raw_ttfc_sample: 10,
        failure_rate: Some(0.0),
        failure_rate_sample: 10,
        rounds_per_minute: None,
        rounds_per_minute_sample: 0,
    }
}

/// Map lines 1351/1352/1543: a fast route that frequently fails is not
/// ranked as genuinely fast. The sound candidate's effective TTFC (1200ms)
/// beats the flaky candidate's (1600ms) even though its *raw* TTFC (1200ms)
/// is worse than the flaky one's (800ms) — so a term that used raw TTFC
/// alone would rank these two the other way around.
///
/// Mutation target `effective-is-raw`: dropping the `/ (1 - p)` division
/// must fail this test (the flaky candidate would then win on raw TTFC
/// alone).
#[test]
fn a_reliable_slower_route_beats_a_fast_flaky_one_on_effective_ttfc_for_tool_using_work() {
    let overrides = no_pairing_overrides();
    let health = FreePool::new();
    let now = Instant::now();
    let inputs = RouterInputs {
        overrides: &overrides,
        health: &health,
        now,
        requirements: tool_using_requirements(),
    };

    let flaky = Destination::fresh(
        "flaky",
        IntegrationId::ClaudeCode,
        "default",
        responsiveness_backend("anthropic", "claude-opus-4", "FLAKY_KEY"),
        None,
    )
    .with_route_responsiveness(Some(fast_and_flaky()));
    let sound = Destination::fresh(
        "sound",
        IntegrationId::ClaudeCode,
        "default",
        responsiveness_backend("anthropic", "claude-opus-4", "SOUND_KEY"),
        None,
    )
    .with_route_responsiveness(Some(slower_and_sound()));

    let routed = SessionRouter::new()
        .choose(RoutingMoment::SessionStart, None, &[flaky, sound], &inputs)
        .expect("destinations were offered");
    assert_eq!(
        routed.chosen().id(),
        "sound",
        "reliability-adjusted, the sound route is genuinely faster: {:?}",
        routed.explanation()
    );

    let contribution = routed
        .explanation()
        .contributions()
        .iter()
        .find(|c| c.name() == "responsiveness (effective TTFC)")
        .expect("the winner carries the responsiveness term");
    assert_eq!(
        contribution.magnitude(),
        1.0,
        "the winner is the candidate set's own best"
    );
    assert!(
        contribution.evidence().contains("raw TTFC 1200ms"),
        "the evidence must name the raw figure: {}",
        contribution.evidence()
    );
    assert!(
        contribution.evidence().contains("effective TTFC 1200ms"),
        "the evidence must name the effective figure: {}",
        contribution.evidence()
    );
}

/// The identical pair, scored for a non-tool-using task: the responsiveness
/// term is inert on both candidates, and the un-adjusted ranking decides —
/// nothing here asserts which candidate wins, only that the term contributed
/// nothing and said why.
#[test]
fn responsiveness_is_inert_for_a_non_tool_using_task() {
    let overrides = no_pairing_overrides();
    let health = FreePool::new();
    let now = Instant::now();
    let inputs = RouterInputs {
        overrides: &overrides,
        health: &health,
        now,
        requirements: TaskRequirements::default(),
    };

    let flaky = Destination::fresh(
        "flaky",
        IntegrationId::ClaudeCode,
        "default",
        responsiveness_backend("anthropic", "claude-opus-4", "FLAKY_KEY"),
        None,
    )
    .with_route_responsiveness(Some(fast_and_flaky()));
    let sound = Destination::fresh(
        "sound",
        IntegrationId::ClaudeCode,
        "default",
        responsiveness_backend("anthropic", "claude-opus-4", "SOUND_KEY"),
        None,
    )
    .with_route_responsiveness(Some(slower_and_sound()));

    let routed = SessionRouter::new()
        .choose(RoutingMoment::SessionStart, None, &[flaky, sound], &inputs)
        .expect("destinations were offered");

    for (_, explanation) in routed.considered() {
        let contribution = explanation
            .contributions()
            .iter()
            .find(|c| c.name() == "responsiveness (effective TTFC)")
            .expect("the term is always present, even when inert");
        assert_eq!(contribution.magnitude(), 0.0, "{explanation:?}");
        assert!(
            contribution.evidence().contains("not tool-using work"),
            "{}",
            contribution.evidence()
        );
    }
}

/// Below `MIN_SAMPLE_FOR_SUMMARY` on either half, the term is inert and
/// names the reason — never a clamped or fabricated figure.
///
/// Mutation target `floor-dropped`: removing the `MIN_SAMPLE_FOR_SUMMARY`
/// gate on the responsiveness term must fail this test.
#[test]
fn responsiveness_is_inert_below_the_sample_floor() {
    let overrides = no_pairing_overrides();
    let health = FreePool::new();
    let now = Instant::now();
    let inputs = RouterInputs {
        overrides: &overrides,
        health: &health,
        now,
        requirements: tool_using_requirements(),
    };

    // Each half of the formula is below the floor on its own — a mutation
    // that dropped only one of the two gates in `effective_ttfc_ms` must
    // still fail this test, so the raw-TTFC sample is below the floor while
    // the failure-rate sample clears it, and vice versa is proved by
    // `responsiveness_is_inert_below_the_sample_floor_on_the_failure_rate_half`.
    let below_floor = RouteResponsiveness {
        raw_ttfc_ms: Some(800.0),
        raw_ttfc_sample: 2,
        failure_rate: Some(0.5),
        failure_rate_sample: 10,
        rounds_per_minute: None,
        rounds_per_minute_sample: 0,
    };
    let only_candidate = Destination::fresh(
        "only",
        IntegrationId::ClaudeCode,
        "default",
        responsiveness_backend("anthropic", "claude-opus-4", "ONLY_KEY"),
        None,
    )
    .with_route_responsiveness(Some(below_floor));

    let routed = SessionRouter::new()
        .choose(
            RoutingMoment::SessionStart,
            None,
            &[only_candidate],
            &inputs,
        )
        .expect("a destination was offered");
    let contribution = routed
        .explanation()
        .contributions()
        .iter()
        .find(|c| c.name() == "responsiveness (effective TTFC)")
        .expect("the term is always present");
    assert_eq!(contribution.magnitude(), 0.0);
    assert!(
        contribution
            .evidence()
            .contains("effective TTFC unmeasured"),
        "{}",
        contribution.evidence()
    );
}

/// [`responsiveness_is_inert_below_the_sample_floor`]'s other half: the
/// raw-TTFC sample clears the floor while the failure-rate sample does not
/// — still inert, proving the second gate independently of the first.
#[test]
fn responsiveness_is_inert_below_the_sample_floor_on_the_failure_rate_half() {
    let overrides = no_pairing_overrides();
    let health = FreePool::new();
    let now = Instant::now();
    let inputs = RouterInputs {
        overrides: &overrides,
        health: &health,
        now,
        requirements: tool_using_requirements(),
    };

    let below_floor = RouteResponsiveness {
        raw_ttfc_ms: Some(800.0),
        raw_ttfc_sample: 10,
        failure_rate: Some(0.5),
        failure_rate_sample: 2,
        rounds_per_minute: None,
        rounds_per_minute_sample: 0,
    };
    let only_candidate = Destination::fresh(
        "only",
        IntegrationId::ClaudeCode,
        "default",
        responsiveness_backend("anthropic", "claude-opus-4", "ONLY_KEY"),
        None,
    )
    .with_route_responsiveness(Some(below_floor));

    let routed = SessionRouter::new()
        .choose(
            RoutingMoment::SessionStart,
            None,
            &[only_candidate],
            &inputs,
        )
        .expect("a destination was offered");
    let contribution = routed
        .explanation()
        .contributions()
        .iter()
        .find(|c| c.name() == "responsiveness (effective TTFC)")
        .expect("the term is always present");
    assert_eq!(contribution.magnitude(), 0.0);
    assert!(
        contribution
            .evidence()
            .contains("effective TTFC unmeasured"),
        "{}",
        contribution.evidence()
    );
}

/// Map line 1544: rounds per minute is supporting evidence, and its `±0.25`
/// ceiling must never let it outrank a candidate a full term ahead — plant
/// the magnitudes: `provider health` alone gives one candidate a full `+1.0`
/// it does not share, and the rounds-per-minute term's own `+0.25` (its own
/// maximum) cannot close that gap.
///
/// Mutation target `rounds-unbounded`: removing the `±0.25` clamp must fail
/// this test.
#[test]
fn tool_rounds_per_minute_never_outranks_a_candidate_a_full_term_ahead() {
    let overrides = no_pairing_overrides();
    let now = Instant::now();

    // A very high rounds-per-minute rate on the destination that is
    // otherwise disadvantaged, so the test is a real comparison rather than
    // one where the two terms happen to point the same way.
    let very_high_rounds = RouteResponsiveness {
        raw_ttfc_ms: None,
        raw_ttfc_sample: 0,
        failure_rate: None,
        failure_rate_sample: 0,
        rounds_per_minute: Some(1000.0),
        rounds_per_minute_sample: 10,
    };

    // Session affinity's own "warmth" facet (line 1596) — a resumable warm
    // session is worth `+0.750` (`tests/session_router.rs`'s own
    // `a_warm_existing_session_outweighs_a_better_resourced_fresh_one`), a
    // real full-term-scale magnitude the ±0.25 ceiling must not be able to
    // bridge.
    let disadvantaged = Destination::fresh(
        "disadvantaged",
        IntegrationId::ClaudeCode,
        "default",
        responsiveness_backend("anthropic", "claude-opus-4", "DISADVANTAGED_KEY"),
        None,
    )
    .with_route_responsiveness(Some(very_high_rounds));

    let ahead = Destination::existing(
        "ahead",
        IntegrationId::ClaudeCode,
        "default",
        responsiveness_backend("anthropic", "claude-opus-4", "AHEAD_KEY"),
        glasshouse::config::pairing::WarmSession {
            state: glasshouse::config::pairing::WarmSessionState::Live,
            idle_seconds: 0,
        },
    );

    let health = FreePool::new();
    let inputs = RouterInputs {
        overrides: &overrides,
        health: &health,
        now,
        requirements: TaskRequirements::default(),
    };

    let routed = SessionRouter::new()
        .choose(
            RoutingMoment::SessionStart,
            None,
            &[disadvantaged, ahead],
            &inputs,
        )
        .expect("destinations were offered");
    assert_eq!(
        routed.chosen().id(),
        "ahead",
        "a quarter-term supporting signal must not overturn a full term: {:?}",
        routed.explanation()
    );
}
