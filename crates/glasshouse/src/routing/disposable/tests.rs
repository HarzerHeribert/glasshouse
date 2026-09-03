use super::classification::{CLASSIFICATION_PREFERENCE_WEIGHT, LATENCY_PREFERENCE_WEIGHT};
use super::*;
use crate::routing::evidence::LatencyRecord;
use crate::routing::free::{FreeResource, FreeResourceKey, WorkloadOutcome};
use crate::routing::{Entitlement, EntitlementRules};
use crate::secret::SecretRef;
use std::time::Duration;

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

/// `routing/disposable` is a directory since Phase 59 (`GH-DECOMP-DISPOSABLE`);
/// the boundary scan below reads every production file of it, joined, the same
/// way `routing::mod::tests::session_source` does for `session/`.
fn disposable_source() -> String {
    [
        include_str!("mod.rs"),
        include_str!("candidates.rs"),
        include_str!("classification.rs"),
    ]
    .join("\n")
}

fn production_code(source: &str) -> String {
    source
        .split("#[cfg(test)]")
        .next()
        .expect("split always yields at least one part")
        .lines()
        .filter(|line| !line.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Line 533's type-level half, checked rather than asserted in a comment:
/// a disposable choice offers no way to become an interactive assignment.
#[test]
fn a_disposable_choice_cannot_become_an_interactive_assignment() {
    let code = production_code(&disposable_source());
    for forbidden in ["Assignment", "InteractiveRouting", "TurnRouting"] {
        assert!(
            !code.contains(forbidden),
            "routing/disposable.rs names `{forbidden}`: the two policy classes Phase 9I \
             line 533 requires to stay separate have started to share types"
        );
    }
}

/// Line 530, and line 531 with it: a user-marked free model is preferred
/// for support work over a metered one.
#[test]
fn support_work_prefers_a_free_model_over_a_metered_one() {
    let routing = DisposableRouting::for_support_work(false, FreePreferences::new());
    let choice = routing
        .choose(
            JobKind::MemoryExtraction,
            &[
                metered("openrouter", "an-expensive-model"),
                free("openrouter", "nvidia/nemotron-nano-9b-v2:free"),
            ],
            &FreePool::new(),
            Instant::now(),
            None,
        )
        .expect("a free model is configured");

    assert_eq!(choice.model(), "nvidia/nemotron-nano-9b-v2:free");
    assert_eq!(choice.cost(), Cost::Free);
    assert_eq!(choice.reason(), UseReason::QuotaPreservation);
}

/// The citation on `choose`'s doc comment: scoring ranks candidates for
/// the explanation it hands back, but the free loop never consults
/// `score` to pick a *winner* — it walks the user's own configured order
/// and returns the first one `pool` says is available. This constructs
/// two free candidates whose scoring order and user order **disagree**
/// (the second-listed one has far more remaining capacity, so `score`
/// would rank it first) and asserts the winner still follows the user's
/// order, not the score.
#[test]
fn scoring_never_reorders_the_existing_free_selection() {
    use crate::provider::quota::{
        Capacity, CapacityState, NativeAmount, Pool, Reading, ReadingSource,
    };

    const OBSERVED: i64 = 1_800_000_000;
    let measured = |value: i64, unit: &str| {
        Capacity::Measured(Reading::new(
            NativeAmount::whole(value, unit),
            OBSERVED,
            ReadingSource::ResponseHeader("x-ratelimit".to_owned()),
        ))
    };
    let score_of = |remaining: i64, limit: i64| {
        CapacityState::metered_balance()
            .with_credits(
                Pool::inapplicable()
                    .with_remaining(measured(remaining, "tokens"))
                    .with_limit(measured(limit, "tokens")),
            )
            .remaining_capacity_score()
            .expect("both halves of the credits pool are measured")
    };

    // Low remaining capacity, but first in the user's own order.
    let first_choice = free("openrouter", "first-choice-model")
        .with_capacity(CandidateCapacity::new().with_remaining_capacity(Some(score_of(10, 100))));
    // High remaining capacity — the one `score` alone would prefer.
    let scoring_would_prefer = free("openrouter", "scoring-would-prefer-model")
        .with_capacity(CandidateCapacity::new().with_remaining_capacity(Some(score_of(90, 100))));

    let preferences =
        FreePreferences::new().with_order(vec![first_choice.key(), scoring_would_prefer.key()]);
    let routing = DisposableRouting::for_support_work(true, preferences);

    let choice = routing
        .choose(
            JobKind::MemoryExtraction,
            // Listed with the scoring-preferred candidate first, so a
            // regression that let scoring drive selection would also
            // have list order working in its favor.
            &[scoring_would_prefer.clone(), first_choice.clone()],
            &FreePool::new(),
            Instant::now(),
            None,
        )
        .expect("both candidates are free and available");

    assert_eq!(
        choice.model(),
        first_choice.model(),
        "scoring favors `{}` (higher remaining capacity), but the user's own order names \
         `{}` first — scoring must never override the user's free-resource order",
        scoring_would_prefer.model(),
        first_choice.model()
    );
}

/// Map line 1539's own words: among free candidates, the expected-latency
/// term must never be why one wins. Both candidates carry a latency
/// figure — the second listed is the one the term alone would prefer —
/// and the winner must still be whichever the user's own order and
/// availability name, exactly as
/// `scoring_never_reorders_the_existing_free_selection` proves for the
/// existing capacity term.
#[test]
fn the_expected_latency_term_never_reorders_the_free_selection() {
    let first_choice = free("openrouter", "first-choice-model").with_latency(Some(LatencyRecord {
        timed: MIN_SAMPLE_FOR_SUMMARY,
        median_duration_ms: Some(5_000),
    }));
    // The term alone would prefer this candidate — its median is far
    // lower — but it is listed second and the user named neither.
    let latency_would_prefer =
        free("openrouter", "latency-would-prefer-model").with_latency(Some(LatencyRecord {
            timed: MIN_SAMPLE_FOR_SUMMARY,
            median_duration_ms: Some(0),
        }));

    let routing = DisposableRouting::for_support_work(true, FreePreferences::new());
    let choice = routing
        .choose(
            JobKind::MemoryExtraction,
            &[first_choice.clone(), latency_would_prefer.clone()],
            &FreePool::new(),
            Instant::now(),
            None,
        )
        .expect("both candidates are free and available");

    assert_eq!(
        choice.model(),
        first_choice.model(),
        "the expected-latency term favors `{}` (lower median), but the free loop must \
         still return the first available candidate in the order it was handed — the \
         term ranks the metered fallback and informs the explanation, never the free \
         selection",
        latency_would_prefer.model()
    );
}

/// Unit: the expected-latency term's arithmetic on fixed inputs — the
/// same formula and the same weight classification latency's own term
/// uses (map line 1421), so a reader who trusts one can trust the other
/// without re-deriving it.
#[test]
fn the_expected_latency_term_uses_classification_latencys_formula_and_weight() {
    let candidate = metered("openrouter", "a-metered-model").with_latency(Some(LatencyRecord {
        timed: MIN_SAMPLE_FOR_SUMMARY,
        median_duration_ms: Some(500),
    }));
    let routing = DisposableRouting::for_support_work(true, FreePreferences::new());
    let choice = routing
        .choose(
            JobKind::MemoryExtraction,
            &[candidate],
            &FreePool::new(),
            Instant::now(),
            None,
        )
        .expect("the sole metered candidate is admitted at the default Plenty band");

    let magnitude = choice
        .explanation()
        .contributions()
        .iter()
        .find(|contribution| contribution.name() == "expected latency")
        .expect("the term is always rendered, measured or not")
        .magnitude();
    let expected = LATENCY_PREFERENCE_WEIGHT / (1.0 + 500.0 / 1000.0);
    assert!(
        (magnitude - expected).abs() < 1e-9,
        "expected {expected}, got {magnitude}"
    );
    assert!((expected - CLASSIFICATION_PREFERENCE_WEIGHT / 1.5).abs() < 1e-9);
}

/// Line 539, the acceptance condition: an automated run finds no free
/// resource and **fails** rather than buying one.
#[test]
fn glasshouses_own_run_refuses_a_metered_resource_without_an_opt_in() {
    let routing = DisposableRouting::for_glasshouses_own_run(
        MeteredUse::for_automated_run(|_| None),
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
        .expect_err("a test run must not spend the user's money");

    assert!(matches!(
        err,
        NoResource::NoFreeResourceAndMeteredWithheld { .. }
    ));
    assert!(err.to_string().contains(MeteredUse::OPT_IN_VAR));
}

/// And the opt-in works, so the capability is "never without an explicit
/// opt-in" rather than "never".
#[test]
fn an_explicit_opt_in_lets_an_automated_run_use_a_metered_resource() {
    let routing = DisposableRouting::for_glasshouses_own_run(
        MeteredUse::for_automated_run(|var| {
            (var == MeteredUse::OPT_IN_VAR).then(|| "1".to_owned())
        }),
        FreePreferences::new(),
    );
    let choice = routing
        .choose(
            JobKind::Evaluation,
            &[metered("openrouter", "an-expensive-model")],
            &FreePool::new(),
            Instant::now(),
            None,
        )
        .expect("an explicit opt-in permits it");
    assert_eq!(choice.cost(), Cost::Metered);
}

/// The fail-closed reading of the opt-in: anything but `1` spends
/// nothing.
#[test]
fn only_the_exact_opt_in_value_counts() {
    for value in ["", "0", "true", "yes", "TRUE", " 1"] {
        let use_ = MeteredUse::for_automated_run(|_| Some(value.to_owned()));
        assert_eq!(
            use_,
            MeteredUse::Withheld,
            "`{value}` must not be read as an opt-in"
        );
    }
}

/// An automated run cannot be handed ordinary support work's permission.
#[test]
fn an_automated_run_cannot_inherit_permitted() {
    let routing =
        DisposableRouting::for_glasshouses_own_run(MeteredUse::Permitted, FreePreferences::new());
    assert_eq!(routing.metered_use(), &MeteredUse::Withheld);
}

/// Line 540: the three reasons, produced by the policy that chose.
#[test]
fn a_choice_says_why_the_free_resource_is_the_one_being_used() {
    let now = Instant::now();

    let asked = DisposableRouting::for_support_work(true, FreePreferences::new())
        .choose(
            JobKind::Classification,
            &[free("openrouter", "a-free-model")],
            &FreePool::new(),
            now,
            None,
        )
        .expect("configured");
    assert_eq!(asked.reason(), UseReason::UserPreference);

    let mut pool = FreePool::new();
    let first = free("openrouter", "first-free-model");
    for _ in 0..2 {
        pool.observe(
            &FreeResource::new(first.credential().clone(), first.model()),
            WorkloadOutcome::CapacityFailure,
            now,
        );
    }
    let fell_back = DisposableRouting::for_support_work(true, FreePreferences::new())
        .choose(
            JobKind::Classification,
            &[first, free("openrouter", "second-free-model")],
            &pool,
            now,
            None,
        )
        .expect("the second free model can serve");
    assert_eq!(fell_back.model(), "second-free-model");
    assert_eq!(fell_back.reason(), UseReason::Fallback);
    assert!(fell_back.describe().contains("fallback"));
}

/// Line 536: a pin is not a preference to fall back from.
#[test]
fn a_pinned_free_resource_that_cannot_serve_fails_the_job() {
    let now = Instant::now();
    let pinned = free("openrouter", "the-pinned-model");
    let mut pool = FreePool::new();
    for _ in 0..2 {
        pool.observe(
            &FreeResource::new(pinned.credential().clone(), pinned.model()),
            WorkloadOutcome::RateLimited {
                retry_after: Some(Duration::from_secs(300)),
            },
            now,
        );
    }

    let routing = DisposableRouting::for_support_work(
        true,
        FreePreferences::new()
            .with_pin(Some(FreeResourceKey::new("openrouter", "the-pinned-model"))),
    );
    let err = routing
        .choose(
            JobKind::Reranking,
            &[pinned, free("openrouter", "another-free-model")],
            &pool,
            now,
            None,
        )
        .expect_err("a pin does not fall back");
    assert!(matches!(err, NoResource::PinnedResourceUnavailable { .. }));
}

/// Line 536: a disabled resource is not chosen for any reason.
#[test]
fn a_disabled_free_resource_is_never_chosen() {
    let routing = DisposableRouting::for_support_work(
        true,
        FreePreferences::new()
            .with_disabled(vec![FreeResourceKey::new("openrouter", "banned-model")]),
    );
    let choice = routing
        .choose(
            JobKind::Classification,
            &[
                free("openrouter", "banned-model"),
                free("nous", "allowed-model"),
            ],
            &FreePool::new(),
            Instant::now(),
            None,
        )
        .expect("one free model is allowed");
    assert_eq!(choice.model(), "allowed-model");
}

/// Characterizes `Self::finish`, extracted from three copies of the same
/// "attach refusal notes, build the choice" sequence: whichever arm of
/// `choose` wins still carries every entitlement refusal note on its
/// explanation, with the note's own wording (map line 1947) unchanged.
/// Pins the winner via the pin arm, since it is the cheapest of the three
/// arms to reach directly.
#[test]
fn a_winning_choice_still_carries_every_entitlement_refusal_note() {
    let denied = free("openrouter", "denied-model").with_entitlement(Some(Entitlement::new(
        "no-extraction",
        EntitlementRules::UNRESTRICTED.deny_job_kinds([JobKind::MemoryExtraction]),
    )));
    let pinned = free("openrouter", "the-pinned-model");

    let routing = DisposableRouting::for_support_work(
        true,
        FreePreferences::new()
            .with_pin(Some(FreeResourceKey::new("openrouter", "the-pinned-model"))),
    );
    let choice = routing
        .choose(
            JobKind::MemoryExtraction,
            &[denied, pinned],
            &FreePool::new(),
            Instant::now(),
            None,
        )
        .expect("the pinned candidate is available");

    assert_eq!(choice.model(), "the-pinned-model");
    let note = choice
        .explanation()
        .contributions()
        .iter()
        .find(|contribution| contribution.name() == "entitlement rule")
        .expect("the denied candidate's refusal note travels with the winner");
    assert!(
        note.evidence()
            .contains("denied-model on openrouter is not a candidate"),
        "unexpected refusal note text: {}",
        note.evidence()
    );
}

/// Map line 1530 and 1554: the winning candidate's explanation names
/// real, inspectable contributions — not an opaque number — and line
/// 1553's structural separation shows up as a hard-constraint-shaped
/// input (cost/eligibility) never being blended into the same magnitude
/// as a soft one (order, capacity, reset).
#[test]
fn the_winning_candidate_carries_a_named_inspectable_explanation() {
    let routing = DisposableRouting::for_support_work(true, FreePreferences::new());
    let choice = routing
        .choose(
            JobKind::MemoryExtraction,
            &[free("openrouter", "a-free-model")],
            &FreePool::new(),
            Instant::now(),
            None,
        )
        .expect("configured");

    let names: Vec<&str> = choice
        .explanation()
        .contributions()
        .iter()
        .map(|c| c.name())
        .collect();
    assert!(names.contains(&"cost"));
    assert!(names.contains(&"user free-resource order"));
    assert!(names.contains(&"normalized remaining capacity"));
    assert!(names.contains(&"time until quota reset"));
    assert!(choice.describe().contains("normalized remaining capacity"));
}

/// Map line 1536 and 1549: when a caller supplies real capacity and
/// reset data for a candidate, it reaches the explanation with a real
/// magnitude — not the `0.0` absence contribution.
#[test]
fn real_capacity_and_reset_data_reach_the_explanation() {
    use crate::provider::quota::{
        Capacity, CapacityState, NativeAmount, Pool, Reading, ReadingSource,
    };

    const OBSERVED: i64 = 1_800_000_000;
    let measured = |value: i64, unit: &str| {
        Capacity::Measured(Reading::new(
            NativeAmount::whole(value, unit),
            OBSERVED,
            ReadingSource::ResponseHeader("x-ratelimit".to_owned()),
        ))
    };
    let state = CapacityState::metered_balance().with_credits(
        Pool::inapplicable()
            .with_remaining(measured(40, "tokens"))
            .with_limit(measured(100, "tokens")),
    );
    let scored = state
        .remaining_capacity_score()
        .expect("both halves of the credits pool are measured");

    let capacity = CandidateCapacity::new()
        .with_remaining_capacity(Some(scored))
        .with_seconds_until_reset(Some(120));
    let routing = DisposableRouting::for_support_work(true, FreePreferences::new());
    let choice = routing
        .choose(
            JobKind::MemoryExtraction,
            &[free("openrouter", "a-free-model").with_capacity(capacity)],
            &FreePool::new(),
            Instant::now(),
            None,
        )
        .expect("configured");

    let capacity_line = choice
        .explanation()
        .contributions()
        .iter()
        .find(|c| c.name() == "normalized remaining capacity")
        .expect("a capacity contribution is always present");
    assert!(
        capacity_line.magnitude() > 0.0,
        "real capacity data must produce a nonzero contribution, not the absence default"
    );
    assert!(capacity_line.evidence().contains("credits"));
    assert!(capacity_line.evidence().contains("40%"));

    let reset_line = choice
        .explanation()
        .contributions()
        .iter()
        .find(|c| c.name() == "time until quota reset")
        .expect("a reset contribution is always present");
    assert!(reset_line.evidence().contains("120"));
}

/// Map line 1550: Phase 32F's protected-reserve policy is a real gate on
/// the metered-fallback path, proven with the actual production
/// function — not a stand-in. A distant, known reset with a Reserve-band
/// candidate is denied; the same candidate with no reset knowledge at
/// all is allowed, because `evaluate_reserve_spend` treats "no cheaper
/// alternative and no distant reset" as the least-bad option.
#[test]
fn the_protected_reserve_policy_gates_the_metered_fallback() {
    use crate::provider::quota::CapacityBand;

    let denied_capacity = CandidateCapacity::new()
        .with_band(Some(CapacityBand::Reserve))
        .with_seconds_until_reset(Some(7_200));
    let routing = DisposableRouting::for_support_work(true, FreePreferences::new());
    let err = routing
        .choose(
            JobKind::MemoryExtraction,
            &[metered("openrouter", "a-reserved-model").with_capacity(denied_capacity)],
            &FreePool::new(),
            Instant::now(),
            None,
        )
        .expect_err("a distant reset on a Reserve-band candidate must be denied");
    assert!(matches!(err, NoResource::ProtectedReserveDenied { .. }));
    assert!(err.to_string().contains("a-reserved-model"));

    let allowed = routing
        .choose(
            JobKind::MemoryExtraction,
            &[metered("openrouter", "an-unread-model")],
            &FreePool::new(),
            Instant::now(),
            None,
        )
        .expect("a candidate nothing has been read about is not withheld by reserve policy");
    assert_eq!(allowed.model(), "an-unread-model");
    assert!(
        allowed
            .explanation()
            .contributions()
            .iter()
            .any(|c| c.name() == "protected-reserve policy" && c.evidence().contains("allowed"))
    );
}

/// §35: mutate the call, not the callee. Deleting the reserve check from
/// the metered-fallback path (treating every decision as allowed) must
/// make a named test fail — proving the gate in `choose` is a real
/// caller of `evaluate_reserve_spend`, not decoration around one.
///
/// This test does not mutate source; it exists so that mutating
/// `evaluate_reserve_spend`'s call in `choose` (deleting the
/// `if !decision.is_allowed() { ... continue; }` guard) is guaranteed to
/// flip `the_protected_reserve_policy_gates_the_metered_fallback` from
/// pass to fail — recorded here so a future reader can find the killed
/// mutation's evidence without re-deriving it. See this package's report
/// for the actual mutation run.
#[test]
fn a_metered_candidate_with_no_reserve_data_is_never_denied() {
    let routing = DisposableRouting::for_support_work(true, FreePreferences::new());
    let choice = routing
        .choose(
            JobKind::MemoryExtraction,
            &[metered("openrouter", "plain-metered-model")],
            &FreePool::new(),
            Instant::now(),
            None,
        )
        .expect("no capacity data defaults to the least protective band, so nothing denies it");
    assert_eq!(choice.model(), "plain-metered-model");
}

/// GH-RESERVE-RESET, map lines 1291 and 1292: reset distance, not the
/// band alone, decides the outcome. `the_protected_reserve_policy_gates_the_metered_fallback`
/// only ever drives a distant reset, so nothing in the suite watched the
/// imminent branch of `evaluate_reserve_spend` before this test — the
/// orchestrator proved that gap by mutating `reset_urgency`'s distant arm
/// from `0.0` to `1.0` and watching 37 tests, including that one, stay
/// green.
///
/// Both candidates share the same Reserve band, the same model identity
/// and the same everything else `evaluate_reserve_spend` reads; only
/// `seconds_until_reset` moves from [`RESET_IMMINENT_SECONDS`] to
/// [`RESET_DISTANT_SECONDS`] (referenced by name per §17's premise
/// discipline, not copied as a literal, so a change to either constant
/// moves this test with it).
#[test]
fn reset_distance_alone_flips_the_protected_reserve_decision() {
    use crate::provider::quota::{CapacityBand, RESET_DISTANT_SECONDS, RESET_IMMINENT_SECONDS};

    let base = CandidateCapacity::new().with_band(Some(CapacityBand::Reserve));
    let imminent_capacity = base
        .clone()
        .with_seconds_until_reset(Some(RESET_IMMINENT_SECONDS));
    let distant_capacity = base
        .clone()
        .with_seconds_until_reset(Some(RESET_DISTANT_SECONDS));

    // Assert the premise (§17): the two inputs actually differ, and the
    // only thing they differ in is `seconds_until_reset` — strip that one
    // field back out of each and they become equal, so the band (and
    // every other field `evaluate_reserve_spend` could read) never moved.
    assert_ne!(
        imminent_capacity, distant_capacity,
        "the two capacities must actually differ for this test to prove anything"
    );
    assert_eq!(
        imminent_capacity.clone().with_seconds_until_reset(None),
        distant_capacity.clone().with_seconds_until_reset(None),
        "band and every other field besides seconds_until_reset must be identical"
    );

    let routing = DisposableRouting::for_support_work(true, FreePreferences::new());

    let allowed = routing
        .choose(
            JobKind::MemoryExtraction,
            &[metered("openrouter", "same-reserved-model").with_capacity(imminent_capacity)],
            &FreePool::new(),
            Instant::now(),
            None,
        )
        .expect("a reset within RESET_IMMINENT_SECONDS permits spending the reserve (line 1291)");
    assert_eq!(allowed.model(), "same-reserved-model");

    let denied = routing
        .choose(
            JobKind::MemoryExtraction,
            &[metered("openrouter", "same-reserved-model").with_capacity(distant_capacity)],
            &FreePool::new(),
            Instant::now(),
            None,
        )
        .expect_err(
            "a reset at RESET_DISTANT_SECONDS denies the same Reserve-band candidate (line 1292)",
        );
    assert!(matches!(denied, NoResource::ProtectedReserveDenied { .. }));
}

/// Two metered candidates whose *bands* differ, built so that the one in
/// the protected reserve would otherwise **win** — the fixture the two
/// tests below share.
///
/// # Why the reserved candidate is the better-scoring one, and why that
/// is not a contrived configuration
///
/// `score` never reads `band`; the magnitude it reads is
/// `remaining_capacity`. The two are genuinely independent inputs,
/// because a band is a percentage compared against **that provider's own
/// thresholds** — `EffectiveConfig::reserve_percent(provider)` — and
/// `phase-32d`/`phase-32f` already ruled that a user may widen one
/// provider's reserve past the global `Tight` boundary as a legitimate
/// policy. So "60% left, and that is inside the reserve its owner asked
/// for" beside "30% left, and that is plenty by its owner's thresholds"
/// is exactly the configuration those rulings describe, not an invention
/// of this test.
///
/// It matters because it is what makes the tests non-vacuous: without
/// the reserve gate the higher-scoring reserved candidate is chosen, so
/// any test that saw the *other* one chosen has watched the gate act and
/// not the scorer.
fn reserved_and_unreserved_pair(
    reserved_reset: Option<i64>,
) -> (DisposableCandidate, DisposableCandidate) {
    use crate::provider::quota::{
        Capacity, CapacityBand, CapacityState, NativeAmount, Pool, Reading, ReadingSource,
    };

    const OBSERVED: i64 = 1_800_000_000;
    let percent_remaining = |value: i64| {
        let measured = |amount: i64| {
            Capacity::Measured(Reading::new(
                NativeAmount::whole(amount, "tokens"),
                OBSERVED,
                ReadingSource::ResponseHeader("x-ratelimit".to_owned()),
            ))
        };
        CapacityState::metered_balance()
            .with_credits(
                Pool::inapplicable()
                    .with_remaining(measured(value))
                    .with_limit(measured(100)),
            )
            .remaining_capacity_score()
            .expect("both halves of the credits pool are measured")
    };

    let reserved = metered("openrouter", "a-reserved-model").with_capacity(
        CandidateCapacity::new()
            .with_band(Some(CapacityBand::Reserve))
            .with_remaining_capacity(Some(percent_remaining(60)))
            .with_seconds_until_reset(reserved_reset),
    );
    let unreserved = metered("anyrouter", "a-plentiful-model").with_capacity(
        CandidateCapacity::new()
            .with_band(Some(CapacityBand::Plenty))
            .with_remaining_capacity(Some(percent_remaining(30))),
    );
    (reserved, unreserved)
}

/// Capability map line 1288 — *"avoid spending protected reserve on
/// low-tier work while cheaper adequate resources exist"* — at the one
/// production caller of `evaluate_reserve_spend`.
///
/// The whole line lives in one input, and that input was a hardcoded
/// `false` until this package: with it, the policy's
/// cheaper-alternative branch is unreachable from production, and the
/// line is not a missing mechanism but an unfed one.
///
/// **Premise first (§17), and it is the same candidate both times.** A
/// Reserve-band candidate *alone* is allowed — nothing cheaper exists, so
/// spending the reserve is the least-bad option — and is chosen. Put an
/// unreserved candidate beside it and the reserved one is refused, so the
/// unreserved one is chosen instead, although it scores strictly lower.
/// Only the presence of the sibling moved.
///
/// Deleting this test loses the only proof that
/// `cheaper_adequate_resource_exists` carries a real value; restoring the
/// constant must fail it.
#[test]
fn line_1288_an_unreserved_sibling_denies_the_reserve_to_low_tier_work() {
    let routing = DisposableRouting::for_support_work(true, FreePreferences::new());
    let (reserved, unreserved) = reserved_and_unreserved_pair(None);

    let alone = routing
        .choose(
            JobKind::MemoryExtraction,
            std::slice::from_ref(&reserved),
            &FreePool::new(),
            Instant::now(),
            None,
        )
        .expect("with nothing cheaper available, spending the reserve is the least-bad option");
    assert_eq!(
        alone.model(),
        "a-reserved-model",
        "the premise: this candidate is chosen when it is the only one"
    );

    let beside_a_cheaper_one = routing
        .choose(
            JobKind::MemoryExtraction,
            &[reserved.clone(), unreserved.clone()],
            &FreePool::new(),
            Instant::now(),
            None,
        )
        .expect("the unreserved candidate can serve, so the job is not refused");
    assert_eq!(
        beside_a_cheaper_one.model(),
        "a-plentiful-model",
        "a resource outside its protected reserve is adequate and cheaper in the currency \
         this policy protects, so leaf-tier work must not spend the reserve (line 1288)"
    );
    assert!(
        beside_a_cheaper_one
            .explanation()
            .contributions()
            .iter()
            .any(|c| c.name() == "protected-reserve policy" && c.evidence().contains("allowed")),
        "the chosen candidate still records the reserve decision that let it through"
    );

    // The order of the candidate list must not decide this: the same two
    // resources the other way round answer the same.
    let reversed = routing
        .choose(
            JobKind::MemoryExtraction,
            &[unreserved, reserved],
            &FreePool::new(),
            Instant::now(),
            None,
        )
        .expect("the unreserved candidate can serve, whichever end of the list it is on");
    assert_eq!(reversed.model(), "a-plentiful-model");
}

/// The two ways a sibling is **not** a cheaper adequate resource, which
/// are the two ways the change for line 1288 could have made the policy
/// refuse work it should do.
///
/// - **Equally reserved.** Two candidates both inside their protected
///   reserve are not alternatives to each other. If they were, a user
///   whose every metered resource is in its reserve band would get
///   `ProtectedReserveDenied` for all of them instead of the least-bad
///   spend `evaluate_reserve_spend`'s tail is written to allow. A
///   `>=` where the predicate says `>` produces exactly that, and this
///   test is what catches it.
/// - **Unread.** A resource nothing has been observed about may be deep
///   in its own reserve; withholding a spend on the strength of it would
///   invent the judgement the input exists to avoid. `None` is not
///   "outside the reserve" — deliberately the opposite of `choose`'s own
///   `unwrap_or(CapacityBand::Plenty)` for the candidate *being judged*,
///   and both are the same refusal to let an unobserved band decide
///   anything.
#[test]
fn an_equally_reserved_or_unread_sibling_is_not_a_cheaper_alternative() {
    use crate::provider::quota::CapacityBand;

    let routing = DisposableRouting::for_support_work(true, FreePreferences::new());
    let (reserved, _) = reserved_and_unreserved_pair(None);
    let also_reserved = metered("anyrouter", "another-reserved-model")
        .with_capacity(CandidateCapacity::new().with_band(Some(CapacityBand::Reserve)));
    let unread = metered("anyrouter", "an-unread-model");

    assert_eq!(
        routing
            .choose(
                JobKind::MemoryExtraction,
                &[reserved.clone(), also_reserved],
                &FreePool::new(),
                Instant::now(),
                None,
            )
            .expect(
                "when every metered resource is inside its reserve, spending one is still \
                 the least-bad option — they are not alternatives to each other"
            )
            .model(),
        "a-reserved-model",
        "the better-scoring reserved candidate is chosen, and neither denies the other"
    );

    assert_eq!(
        routing
            .choose(
                JobKind::MemoryExtraction,
                &[reserved, unread],
                &FreePool::new(),
                Instant::now(),
                None,
            )
            .expect("a candidate nothing has been read about denies nothing")
            .model(),
        "a-reserved-model",
        "an unobserved band is not evidence that a cheaper adequate resource exists"
    );
}

/// Capability map line 1291 — *"allow reserve policy to become more
/// permissive shortly before a known quota reset"* — which needs line
/// 1288's input to be observable at all.
///
/// `phase-32f.md` recorded exactly why this line stayed open after its
/// own mechanism was built and tested: `evaluate_reserve_spend`'s tail
/// denies only when `cheaper_adequate_resource_exists`, and otherwise
/// falls through to `Allow`, so with that input nailed to `false` the
/// imminent-reset branch's `Allow` and the default `Allow` were **the
/// same decision** and "more permissive" could not be seen. Disabling
/// the branch outright changed nothing; the orchestrator ran that
/// mutation and it SURVIVED.
///
/// With a real sibling the two decisions come apart. Same pair of
/// candidates, same bands, same scores: at a reset the policy calls
/// imminent the reserved candidate is spent, and at a reset it does not
/// the cheaper one is taken instead. **Reset distance is the only field
/// that moves**, asserted below rather than claimed.
///
/// The far case is deliberately *between* [`RESET_IMMINENT_SECONDS`] and
/// [`RESET_DISTANT_SECONDS`], so that the denial comes from line 1288's
/// branch and not from the distant-reset branch
/// `reset_distance_alone_flips_the_protected_reserve_decision` already
/// covers — two different lines, kept apart.
#[test]
fn line_1291_an_imminent_reset_makes_the_policy_spend_a_reserve_it_would_otherwise_keep() {
    use crate::provider::quota::{RESET_DISTANT_SECONDS, RESET_IMMINENT_SECONDS};

    let (imminent, unreserved) = reserved_and_unreserved_pair(Some(RESET_IMMINENT_SECONDS));
    let midway = (RESET_IMMINENT_SECONDS + RESET_DISTANT_SECONDS) / 2;
    let (not_imminent, _) = reserved_and_unreserved_pair(Some(midway));

    // Assert the premise (§17): the reserved candidate's two forms differ
    // in `seconds_until_reset` and in nothing else — strip that one field
    // from both and they are the same candidate, so band, capacity score
    // and identity have provably not moved.
    assert_ne!(imminent, not_imminent);
    assert_eq!(
        imminent
            .clone()
            .with_capacity(imminent.capacity.clone().with_seconds_until_reset(None)),
        not_imminent
            .clone()
            .with_capacity(not_imminent.capacity.clone().with_seconds_until_reset(None)),
        "only seconds_until_reset may differ between the two reserved candidates"
    );
    assert!(
        midway > RESET_IMMINENT_SECONDS && midway < RESET_DISTANT_SECONDS,
        "the far case must fall short of the distant-reset branch, so that this test is \
         about line 1291 and not about line 1292"
    );

    let routing = DisposableRouting::for_support_work(true, FreePreferences::new());

    let spent = routing
        .choose(
            JobKind::MemoryExtraction,
            &[imminent, unreserved.clone()],
            &FreePool::new(),
            Instant::now(),
            None,
        )
        .expect("configured");
    assert_eq!(
        spent.model(),
        "a-reserved-model",
        "conserving buys little when the quota is about to reset, so the policy becomes \
         permissive and spends the reserve (line 1291)"
    );

    let kept = routing
        .choose(
            JobKind::MemoryExtraction,
            &[not_imminent, unreserved],
            &FreePool::new(),
            Instant::now(),
            None,
        )
        .expect("configured");
    assert_eq!(
        kept.model(),
        "a-plentiful-model",
        "with the same cheaper sibling and a reset that is not imminent, the reserve is \
         kept — which is what makes the case above 'more permissive' rather than 'always \
         permissive'"
    );
}

/// GH-CLASSIFY-CALLER, the fifth link: a real [`TaskClassification`]
/// reaching `choose`'s metered-fallback path must change the outcome, not
/// merely be accepted and ignored. Reuses the exact Reserve-band,
/// distant-reset candidate `the_protected_reserve_policy_gates_the_metered_fallback`
/// denies at the fixed [`WorkloadTier::Leaf`] this policy used before a
/// classification existed to ask — the same candidate, the same band, the
/// same reset, only the classification differs, so any change in the
/// outcome is attributable to `classification` alone.
///
/// `classify_heuristically`'s two production examples from Phase 35's own
/// evidence: "what is a mutex" (leaf, confidence medium, no escalation)
/// and "run cargo test and fix whatever fails" (heavy, confidence
/// medium) — line 2307/2317 of `provider::quota::evaluate_reserve_spend`
/// denies every tier but heavy once a reset is distant, so this is the
/// exact boundary a policy stuck on `WorkloadTier::Leaf` could never
/// cross.
#[test]
fn a_real_classification_changes_the_metered_fallback_outcome_at_the_same_call_site() {
    use crate::provider::quota::CapacityBand;
    use crate::routing::classify::classify_heuristically;

    let reserve_capacity = || {
        CandidateCapacity::new()
            .with_band(Some(CapacityBand::Reserve))
            .with_seconds_until_reset(Some(7_200))
    };
    let routing = DisposableRouting::for_support_work(true, FreePreferences::new());

    let trivial = classify_heuristically("what is a mutex");
    assert_eq!(trivial.conservative_workload_tier(), WorkloadTier::Leaf);
    let denied = routing
        .choose(
            JobKind::MemoryExtraction,
            &[metered("openrouter", "a-reserved-model").with_capacity(reserve_capacity())],
            &FreePool::new(),
            Instant::now(),
            Some(&trivial),
        )
        .expect_err("a leaf-tier classification must not justify spending the reserve");
    assert!(matches!(denied, NoResource::ProtectedReserveDenied { .. }));

    let demanding = classify_heuristically("run cargo test and fix whatever fails");
    assert_eq!(demanding.conservative_workload_tier(), WorkloadTier::Heavy);
    let allowed = routing
        .choose(
            JobKind::MemoryExtraction,
            &[metered("openrouter", "a-reserved-model").with_capacity(reserve_capacity())],
            &FreePool::new(),
            Instant::now(),
            Some(&demanding),
        )
        .expect("a heavy-tier classification justifies spending the reserve (line 1290)");
    assert_eq!(allowed.model(), "a-reserved-model");
}
