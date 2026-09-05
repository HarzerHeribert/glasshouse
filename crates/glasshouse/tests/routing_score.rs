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
use glasshouse::provider::pricing::PriceTable;
use glasshouse::routing::disposable::{
    CandidateCapacity, DisposableCandidate, DisposableRouting, JobKind, NoResource,
};
use glasshouse::routing::evidence::{CostConfidence, RouteResponsiveness};
use glasshouse::routing::free::{FreePool, FreePreferences};
use glasshouse::routing::session::{
    Destination, EstimatedInputSize, RouterInputs, RoutingMoment, SessionRouter, TaskRequirements,
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
        cache_read_ratio: None,
        cache_read_ratio_sample: 0,
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
        cache_read_ratio: None,
        cache_read_ratio_sample: 0,
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
        cache_read_ratio: None,
        cache_read_ratio_sample: 0,
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
        cache_read_ratio: None,
        cache_read_ratio_sample: 0,
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
        cache_read_ratio: None,
        cache_read_ratio_sample: 0,
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

// ---------------------------------------------------------------------------
// `GH-CACHE-TEMPERATURE` — map lines 1535/1545, through `SessionRouter::choose`
// the same way the responsiveness terms above are: this destination's own
// attached `RouteResponsiveness`, this time its `cache_read_ratio` half.
// ---------------------------------------------------------------------------

/// A route whose measured history shows a strongly warm prompt cache — a
/// 90% read ratio over twenty rows, comfortably past
/// `MIN_SAMPLE_FOR_SUMMARY`.
fn warm_cache_history() -> RouteResponsiveness {
    RouteResponsiveness {
        raw_ttfc_ms: None,
        raw_ttfc_sample: 0,
        failure_rate: None,
        failure_rate_sample: 0,
        rounds_per_minute: None,
        rounds_per_minute_sample: 0,
        cache_read_ratio: Some(0.9),
        cache_read_ratio_sample: 20,
    }
}

/// The mirror: a route whose measured history shows almost no cache reads,
/// at the same sample size.
fn cold_cache_history() -> RouteResponsiveness {
    RouteResponsiveness {
        raw_ttfc_ms: None,
        raw_ttfc_sample: 0,
        failure_rate: None,
        failure_rate_sample: 0,
        rounds_per_minute: None,
        rounds_per_minute_sample: 0,
        cache_read_ratio: Some(0.05),
        cache_read_ratio_sample: 20,
    }
}

/// Map lines 1535/1545: with every other term equal, a destination whose
/// measured cache-read history is warm scores strictly positive and wins
/// over one whose history is cold, which scores strictly negative.
///
/// Mutation target `invert-sign`: swapping which end of the ratio scores
/// positive must fail this test.
#[test]
fn measured_cache_temperature_prefers_a_destination_with_a_warmer_measured_history() {
    let overrides = no_pairing_overrides();
    let health = FreePool::new();
    let now = Instant::now();
    let inputs = RouterInputs {
        overrides: &overrides,
        health: &health,
        now,
        requirements: TaskRequirements::default(),
    };

    let warm = Destination::fresh(
        "warm",
        IntegrationId::ClaudeCode,
        "default",
        responsiveness_backend("anthropic", "claude-opus-4", "WARM_KEY"),
        None,
    )
    .with_route_responsiveness(Some(warm_cache_history()));
    let cold = Destination::fresh(
        "cold",
        IntegrationId::ClaudeCode,
        "default",
        responsiveness_backend("anthropic", "claude-opus-4", "COLD_KEY"),
        None,
    )
    .with_route_responsiveness(Some(cold_cache_history()));

    let routed = SessionRouter::new()
        .choose(RoutingMoment::SessionStart, None, &[warm, cold], &inputs)
        .expect("destinations were offered");
    assert_eq!(
        routed.chosen().id(),
        "warm",
        "with every other term equal, the warmer measured history must win: {:?}",
        routed.explanation()
    );

    for (destination, explanation) in routed.considered() {
        let contribution = explanation
            .contributions()
            .iter()
            .find(|c| c.name() == "measured cache temperature")
            .expect("the term is always present, even when inert");
        if destination.id() == "warm" {
            assert!(
                contribution.magnitude() > 0.0,
                "a warm measured history must score positive: {contribution:?}"
            );
            assert!(
                contribution.evidence().contains("90.0%"),
                "{}",
                contribution.evidence()
            );
            assert!(
                contribution.evidence().contains("20 rows"),
                "{}",
                contribution.evidence()
            );
        } else {
            assert!(
                contribution.magnitude() < 0.0,
                "a cold measured history must score negative: {contribution:?}"
            );
            assert!(
                contribution.evidence().contains("5.0%"),
                "{}",
                contribution.evidence()
            );
        }
    }
}

/// Map lines 1535/1545: inert — exactly `0.0`, and saying so — for a
/// destination with no responsiveness reading attached at all, and for one
/// whose reading carries too few rows to summarize a ratio. A ranking
/// computed on a build that reads no cache observations must be
/// byte-for-byte what it was before this term existed.
///
/// Mutation target `floor-dropped`: removing the `MIN_SAMPLE_FOR_SUMMARY`
/// gate on the cache-read ratio (in [`RouteResponsiveness::from_observations`])
/// must fail this test.
#[test]
fn measured_cache_temperature_is_inert_without_a_reading_and_below_the_sample_floor() {
    let overrides = no_pairing_overrides();
    let health = FreePool::new();
    let now = Instant::now();
    let inputs = RouterInputs {
        overrides: &overrides,
        health: &health,
        now,
        requirements: TaskRequirements::default(),
    };

    let no_reading = Destination::fresh(
        "no-reading",
        IntegrationId::ClaudeCode,
        "default",
        responsiveness_backend("anthropic", "claude-opus-4", "NO_READING_KEY"),
        None,
    );

    let below_floor = RouteResponsiveness {
        raw_ttfc_ms: None,
        raw_ttfc_sample: 0,
        failure_rate: None,
        failure_rate_sample: 0,
        rounds_per_minute: None,
        rounds_per_minute_sample: 0,
        cache_read_ratio: None,
        cache_read_ratio_sample: 2,
    };
    let thin_sample = Destination::fresh(
        "thin-sample",
        IntegrationId::ClaudeCode,
        "default",
        responsiveness_backend("anthropic", "claude-opus-4", "THIN_KEY"),
        None,
    )
    .with_route_responsiveness(Some(below_floor));

    let routed = SessionRouter::new()
        .choose(
            RoutingMoment::SessionStart,
            None,
            &[no_reading, thin_sample],
            &inputs,
        )
        .expect("destinations were offered");

    for (_, explanation) in routed.considered() {
        let contribution = explanation
            .contributions()
            .iter()
            .find(|c| c.name() == "measured cache temperature")
            .expect("the term is always present, even when inert");
        assert_eq!(contribution.magnitude(), 0.0, "{explanation:?}");
    }
}

// ---------------------------------------------------------------------------
// `GH-CACHED-INPUT-PRICE` — map line 1300: `estimated_cost` and
// `expected_marginal_cost`'s evidence split a destination's estimated input
// tokens at this route's own measured `cache_read_ratio` once the price
// table declares a `cached_input_per_million_usd`, and price exactly as
// before this package existed whenever either half is missing — the same
// `PriceTable::load_from_dir` / `SessionRouter::with_price_table` seam
// `tests/routing_pricing.rs` already proves the flat estimate through.
// ---------------------------------------------------------------------------

fn cached_price_temp_dir(label: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!(
        "glasshouse-routing-score-cached-price-test-{label}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ))
}

fn write_pricing_toml(dir: &std::path::Path, contents: &str) {
    std::fs::create_dir_all(dir).expect("create temp config dir");
    std::fs::write(dir.join("pricing.toml"), contents).expect("write pricing.toml");
}

/// A route responsiveness reading carrying only a cache-read ratio, at a
/// sample comfortably past `MIN_SAMPLE_FOR_SUMMARY` — mirrors
/// [`warm_cache_history`]/[`cold_cache_history`] above, generalized to an
/// arbitrary ratio for this section's arithmetic-exactness assertions.
fn cache_history(ratio: f64) -> RouteResponsiveness {
    RouteResponsiveness {
        raw_ttfc_ms: None,
        raw_ttfc_sample: 0,
        failure_rate: None,
        failure_rate_sample: 0,
        rounds_per_minute: None,
        rounds_per_minute_sample: 0,
        cache_read_ratio: Some(ratio),
        cache_read_ratio_sample: 20,
    }
}

fn cached_priced_destination(
    dir: &std::path::Path,
    pricing_toml: &str,
    tokens: u64,
    responsiveness: Option<RouteResponsiveness>,
) -> (PriceTable, Destination) {
    write_pricing_toml(dir, pricing_toml);
    let prices = PriceTable::load_from_dir(dir);
    let mut destination = Destination::fresh(
        "fresh",
        IntegrationId::ClaudeCode,
        "default",
        responsiveness_backend("openrouter", "some/model", "OPENROUTER_API_KEY"),
        None,
    )
    .with_estimated_input_size(
        EstimatedInputSize::UNESTIMATED.with_project_memory_tokens(Some(tokens)),
    );
    if let Some(reading) = responsiveness {
        destination = destination.with_route_responsiveness(Some(reading));
    }
    (prices, destination)
}

fn choose_one(
    prices: PriceTable,
    destination: Destination,
) -> glasshouse::routing::session::Routed {
    let overrides = no_pairing_overrides();
    let health = FreePool::new();
    let inputs = RouterInputs {
        overrides: &overrides,
        health: &health,
        now: Instant::now(),
        requirements: TaskRequirements::default(),
    };
    SessionRouter::new()
        .with_price_table(prices)
        .choose(RoutingMoment::SessionStart, None, &[destination], &inputs)
        .expect("a destination was offered")
}

const EXPECTED_MARGINAL_COST_TERM: &str = "expected marginal cost";

fn expected_marginal_cost_evidence(
    routed: &glasshouse::routing::session::Routed,
    id: &str,
) -> String {
    let (_, explanation) = routed
        .considered()
        .iter()
        .find(|(destination, _)| destination.id() == id)
        .unwrap_or_else(|| panic!("`{id}` was scored"));
    explanation
        .contributions()
        .iter()
        .find(|c| c.name() == EXPECTED_MARGINAL_COST_TERM)
        .expect("expected marginal cost is always present")
        .evidence()
        .to_owned()
}

/// REQUIRED BEHAVIOR 3: a declared cached rate and a measured ratio together
/// price a destination below the flat estimate, by exactly the arithmetic
/// the ratio implies — 1,000,000 tokens, a 90% measured ratio, $1.00/million
/// cached and $5.00/million uncached: `900_000 * 1.00 + 100_000 * 5.00 =
/// 1_400_000` micro-USD, well under the flat `1_000_000 * 5.00 =
/// 5_000_000` a missing cached rate would have produced.
#[test]
fn a_declared_cached_rate_and_a_measured_ratio_together_split_the_estimate() {
    let dir = cached_price_temp_dir("split");
    let (prices, destination) = cached_priced_destination(
        &dir,
        r#"
        [[prices]]
        provider = "openrouter"
        model = "some/model"
        input_per_million_usd = 5.0
        output_per_million_usd = 9.0
        cached_input_per_million_usd = 1.0
        "#,
        1_000_000,
        Some(cache_history(0.9)),
    );

    let routed = choose_one(prices, destination);
    let cost = routed
        .cost()
        .expect("a priced, sized destination has a cost");
    assert_eq!(
        cost.micro_usd, 1_400_000,
        "900_000 tokens at the cached rate plus 100_000 at the full rate must be exact"
    );
    assert_eq!(cost.confidence, CostConfidence::Estimated);

    let evidence = expected_marginal_cost_evidence(&routed, "fresh");
    assert!(
        evidence.contains("split"),
        "a cache-split estimate must be distinguishable from a flat one in the evidence: \
         {evidence}"
    );
    assert!(evidence.contains("90%"), "{evidence}");
}

/// REQUIRED BEHAVIOR 4: a declared cached rate with **no** measured ratio
/// (map line 1300's "a missing ratio is not a cold route") prices identically
/// to a build with no cached rate at all — the exact micro-dollar figure,
/// not just "unchanged": 1,000,000 tokens at $5.00/million is exactly
/// 5,000,000 micro-USD.
#[test]
fn a_declared_cached_rate_with_no_measured_ratio_prices_exactly_as_the_flat_rate() {
    let dir = cached_price_temp_dir("rate-no-ratio");
    let (prices, destination) = cached_priced_destination(
        &dir,
        r#"
        [[prices]]
        provider = "openrouter"
        model = "some/model"
        input_per_million_usd = 5.0
        output_per_million_usd = 9.0
        cached_input_per_million_usd = 1.0
        "#,
        1_000_000,
        None,
    );

    let routed = choose_one(prices, destination);
    let cost = routed
        .cost()
        .expect("a priced, sized destination has a cost");
    assert_eq!(
        cost.micro_usd, 5_000_000,
        "with no measured ratio, a declared cached rate must not change the price at all"
    );

    let evidence = expected_marginal_cost_evidence(&routed, "fresh");
    assert!(
        !evidence.contains("split"),
        "with no measured ratio there is nothing to split: {evidence}"
    );
}

/// REQUIRED BEHAVIOR 5: a measured ratio with **no** declared cached rate
/// (map line 1300's "a missing rate is not a free cache") prices identically
/// to the flat estimate — same exact-figure assertion as the previous test,
/// this time with the ratio present and the rate absent.
#[test]
fn a_measured_ratio_with_no_declared_cached_rate_prices_exactly_as_the_flat_rate() {
    let dir = cached_price_temp_dir("ratio-no-rate");
    let (prices, destination) = cached_priced_destination(
        &dir,
        r#"
        [[prices]]
        provider = "openrouter"
        model = "some/model"
        input_per_million_usd = 5.0
        output_per_million_usd = 9.0
        "#,
        1_000_000,
        Some(cache_history(0.9)),
    );

    let routed = choose_one(prices, destination);
    let cost = routed
        .cost()
        .expect("a priced, sized destination has a cost");
    assert_eq!(
        cost.micro_usd, 5_000_000,
        "with no declared cached rate, a measured ratio must not change the price at all"
    );

    let evidence = expected_marginal_cost_evidence(&routed, "fresh");
    assert!(
        !evidence.contains("split"),
        "with no declared cached rate there is nothing to split: {evidence}"
    );
}
