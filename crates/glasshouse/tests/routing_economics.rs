//! GH-ROUTING-ECONOMICS — the routing-model selector's evidence readers and
//! the economics of routing itself. Capability map lines 1420, 1421, 1422,
//! 1423, 1427, 1432, 1435, 1437, 1438, 1463, 1465, 1466 and 1795.
//!
//! # Two levels, on purpose
//!
//! The policy tests below enter through
//! [`DisposableRouting::choose_for_automatic_classification`], the exact
//! function `main.rs::automatic_classification_choice` calls, with the facts
//! a caller attaches to a candidate built by hand. They prove what each
//! filter and preference does with a measured quantity and with an
//! unmeasured one.
//!
//! The binary tests run `glasshouse classify` and `glasshouse resources` as
//! processes against a canned OpenAI chat-completions endpoint on loopback
//! and against rows planted in the project's own ledgers. They prove the
//! wiring: that the ledger's record actually reaches the selector, that the
//! fallback chain is walked by the shipped binary, that a remote model is
//! never contacted under local-only, and that the economics block reads real
//! rows. Practice §35: a filter no test feeds through the production entry
//! point is, to the suite, not applied.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use clap::Parser;
use glasshouse::config::{
    EffectiveConfig, FreeResourceRef, Layer, Layered, ProjectConfig, ProviderConfig,
    RoutingModelChoice, UserConfig,
};
use glasshouse::evaluation::{
    EvaluationKind, EvaluationObservations, NewObservation as NewEvaluation, interactive_hours,
};
use glasshouse::provider::quota::{
    Capacity, CapacityState, NativeAmount, Pool, RateCeilings, Reading, ReadingSource,
    RemainingCapacityScore,
};
use glasshouse::provider::registry::Locality;
use glasshouse::routing::disposable::{
    AutomaticClassificationDecision, CLASSIFICATION_RELIABILITY_MIN_OBSERVATIONS,
    CandidateCapacity, ClassificationPolicy, DisposableCandidate, DisposableChoice,
    DisposableRouting, NoResource,
};
use glasshouse::routing::evidence::{
    CLASSIFICATION_PURPOSE, ClassificationRecord, EvidenceLedger, MIN_SAMPLE_FOR_SUMMARY,
    NewObservation, ObservationQuery, Outcome, PurposeConsumption,
    ROUTING_OVERHEAD_WARNING_FRACTION, RoutingObservation, RoutingOverhead,
};
use glasshouse::routing::free::{FreePool, FreePreferences, FreeResourceKey};
use glasshouse::routing::{Cost, CredentialId};
use glasshouse::secret::SecretRef;
use glasshouse::session::{NewSession, ProjectSessions, SessionLifecycle};
use glasshouse::{Cli, Runtime};

// ===========================================================================
// Policy-level helpers.
// ===========================================================================

fn credential(provider: &str) -> CredentialId {
    CredentialId::new(
        provider,
        SecretRef::Environment {
            var: format!("{}_API_KEY", provider.to_uppercase().replace('-', "_")),
        },
    )
}

fn free(provider: &str, model: &str) -> DisposableCandidate {
    DisposableCandidate::new(provider, model, credential(provider), Cost::Free)
}

fn metered(provider: &str, model: &str) -> DisposableCandidate {
    DisposableCandidate::new(provider, model, credential(provider), Cost::Metered)
}

/// A classification record as the ledger would hand it back: `parsed` of
/// `outcomes` calls in the schema, and a median latency only when one is
/// given — in which case it is backed by exactly the minimum sample, the
/// smallest count the ledger will ever attach a median to.
fn record(
    provider: &str,
    model: &str,
    parsed: usize,
    outcomes: usize,
    median_ms: Option<i64>,
) -> ClassificationRecord {
    ClassificationRecord {
        provider: provider.to_owned(),
        model: model.to_owned(),
        outcomes_recorded: outcomes,
        parsed,
        timed: if median_ms.is_some() {
            MIN_SAMPLE_FOR_SUMMARY
        } else {
            0
        },
        median_duration_ms: median_ms,
    }
}

/// A remaining-capacity score bound by the per-minute request ceiling —
/// the shape `provider::quota::CapacityState::remaining_capacity_score`
/// produces when the general request pool's remaining count is paired
/// against a `requests_per_minute` ceiling in the same unit and nothing
/// tighter is known. External tests cannot build a
/// [`RemainingCapacityScore`] any other way; its fields are private.
fn rpm_bound_score(remaining: i64, ceiling: i64) -> RemainingCapacityScore {
    const OBSERVED: i64 = 1_800_000_000;
    let measured = |value: i64| {
        Capacity::Measured(Reading::new(
            NativeAmount::whole(value, "requests"),
            OBSERVED,
            ReadingSource::ResponseHeader("x-ratelimit".to_owned()),
        ))
    };
    let score = CapacityState::metered_balance()
        .with_requests(Pool::inapplicable().with_remaining(measured(remaining)))
        .with_rate_ceilings(
            RateCeilings::uniform(Capacity::Unmeasured, Capacity::Unmeasured)
                .with_requests_per_minute(measured(ceiling)),
        )
        .remaining_capacity_score()
        .expect("a remaining count against a per-minute ceiling yields a score");
    assert_eq!(
        score.dimension(),
        "requests per minute",
        "this helper exists to build an RPM-bound score"
    );
    score
}

/// A score bound by a token pool rather than by requests per minute.
fn token_bound_score(percent: i64) -> RemainingCapacityScore {
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

fn support_routing() -> DisposableRouting {
    DisposableRouting::for_support_work(true, FreePreferences::new())
}

/// One fresh automatic-classification decision — the production entry
/// point, with no retained pick and an empty health pool.
fn decide(
    routing: &DisposableRouting,
    candidates: &[DisposableCandidate],
) -> Result<DisposableChoice, NoResource> {
    match routing.choose_for_automatic_classification(
        candidates,
        &FreePool::new(),
        Instant::now(),
        1_800_000_000,
        None,
        None,
    )? {
        AutomaticClassificationDecision::Fresh(choice, _) => Ok(choice),
        AutomaticClassificationDecision::Retained(choice) => {
            panic!("no retained pick was supplied, yet one was reused: {choice:?}")
        }
    }
}

fn rendered(choice: &DisposableChoice) -> String {
    choice.explanation().render()
}

// ===========================================================================
// Map lines 1422 / 1432 — structured-output reliability.
// ===========================================================================

/// **REQUIRED BEHAVIOR 1.** Three parsed of ten is excluded with the ratio
/// in the reason; one of one is not excluded and is explained as unproven.
#[test]
fn an_unreliable_candidate_is_excluded_and_an_unproven_one_is_not() {
    let unreliable = free("alpha", "alpha-model").with_classification_record(Some(record(
        "alpha",
        "alpha-model",
        3,
        10,
        None,
    )));
    let unproven = free("beta", "beta-model").with_classification_record(Some(record(
        "beta",
        "beta-model",
        1,
        1,
        None,
    )));

    let choice = decide(&support_routing(), &[unreliable, unproven])
        .expect("the unproven candidate remains");
    assert_eq!(
        choice.provider(),
        "beta",
        "the unreliable candidate was listed first and must still lose: {}",
        rendered(&choice)
    );

    let explanation = rendered(&choice);
    assert!(
        explanation.contains(
            "alpha-model on alpha: only 3 of 10 classification calls came back in the schema (30%)"
        ),
        "the exclusion must name the ratio:\n{explanation}"
    );
    assert!(
        explanation.contains("below the 80% reliability floor"),
        "the exclusion must name the floor it fell under:\n{explanation}"
    );
    assert!(
        explanation.contains("unproven, not unreliable: 1 of 1 classification calls parsed"),
        "the winner's note must say it is unproven rather than reliable:\n{explanation}"
    );
    assert!(
        explanation.contains(&format!(
            "fewer than the {CLASSIFICATION_RELIABILITY_MIN_OBSERVATIONS} needed"
        )),
        "the note must say how many observations the floor waits for:\n{explanation}"
    );
}

/// The floor is a floor: four of five parsed sits *at* it and stays
/// admitted, so the boundary is exact rather than "roughly reliable".
#[test]
fn a_candidate_exactly_at_the_reliability_floor_is_admitted() {
    let at_floor = free("alpha", "alpha-model").with_classification_record(Some(record(
        "alpha",
        "alpha-model",
        4,
        5,
        None,
    )));
    let choice = decide(&support_routing(), &[at_floor]).expect("4 of 5 is at the 80% floor");
    assert_eq!(choice.provider(), "alpha");
    assert!(
        rendered(&choice).contains(
            "4 of 5 classification calls came back in the schema (80%), at or above the 80% floor"
        ),
        "{}",
        rendered(&choice)
    );
}

/// Map line 1422 on its own words: among free candidates the user has not
/// ranked, the one whose measured record is more reliable is preferred —
/// both are above the floor, so only the preference can separate them.
#[test]
fn a_more_reliable_candidate_is_preferred_among_unranked_free_candidates() {
    let at_floor = free("alpha", "alpha-model").with_classification_record(Some(record(
        "alpha",
        "alpha-model",
        8,
        10,
        None,
    )));
    let flawless = free("beta", "beta-model").with_classification_record(Some(record(
        "beta",
        "beta-model",
        10,
        10,
        None,
    )));

    let choice = decide(&support_routing(), &[at_floor, flawless]).expect("both are admissible");
    assert_eq!(
        choice.provider(),
        "beta",
        "the more reliable candidate was listed second and must still win: {}",
        rendered(&choice)
    );
    assert!(
        rendered(&choice).contains("10 of 10 classification calls came back in the schema (100%) — more reliable is preferred"),
        "{}",
        rendered(&choice)
    );
}

// ===========================================================================
// Map lines 1421 / 1435 — latency.
// ===========================================================================

/// **REQUIRED BEHAVIOR 2.** A ceiling of 800ms excludes a candidate whose
/// measured median is 1200ms, and the explanation names both numbers.
#[test]
fn a_slow_candidate_is_excluded_by_the_configured_latency_ceiling() {
    let routing = support_routing()
        .with_classification_policy(ClassificationPolicy::new().with_max_latency_ms(Some(800)));
    let slow = free("alpha", "alpha-model").with_classification_record(Some(record(
        "alpha",
        "alpha-model",
        5,
        5,
        Some(1200),
    )));
    let unmeasured = free("beta", "beta-model");

    let choice = decide(&routing, &[slow, unmeasured]).expect("the unmeasured candidate remains");
    assert_eq!(choice.provider(), "beta", "{}", rendered(&choice));
    let explanation = rendered(&choice);
    assert!(
        explanation.contains("median classification latency 1200ms")
            && explanation.contains("exceeds the 800ms ceiling"),
        "the exclusion must name both the median and the ceiling:\n{explanation}"
    );
}

/// Map line 1421: among free candidates the user has not ranked, a
/// measured lower median is preferred — the preference acts on the pick,
/// not only on the explanation.
#[test]
fn a_faster_candidate_is_preferred_among_unranked_free_candidates() {
    let slower = free("alpha", "alpha-model").with_classification_record(Some(record(
        "alpha",
        "alpha-model",
        5,
        5,
        Some(3000),
    )));
    let faster = free("beta", "beta-model").with_classification_record(Some(record(
        "beta",
        "beta-model",
        5,
        5,
        Some(0),
    )));

    let choice = decide(&support_routing(), &[slower, faster]).expect("both are admissible");
    assert_eq!(
        choice.provider(),
        "beta",
        "the faster candidate was listed second and must still win: {}",
        rendered(&choice)
    );
    assert!(
        rendered(&choice)
            .contains("median 0ms over 5 timed classification calls — lower is preferred"),
        "{}",
        rendered(&choice)
    );
}

// ===========================================================================
// Map line 1420 — requests-per-minute headroom.
// ===========================================================================

/// **REQUIRED BEHAVIOR (acceptance 3).** More request-per-minute headroom
/// scores higher and the explanation says so.
#[test]
fn more_request_headroom_scores_higher_and_says_so() {
    let tight = free("alpha", "alpha-model").with_capacity(
        CandidateCapacity::new().with_remaining_capacity(Some(rpm_bound_score(10, 100))),
    );
    let roomy = free("beta", "beta-model").with_capacity(
        CandidateCapacity::new().with_remaining_capacity(Some(rpm_bound_score(90, 100))),
    );

    let choice = decide(&support_routing(), &[tight, roomy]).expect("both have headroom");
    assert_eq!(
        choice.provider(),
        "beta",
        "the roomier candidate was listed second and must still win: {}",
        rendered(&choice)
    );
    let explanation = rendered(&choice);
    assert!(
        explanation.contains(
            "requests-per-minute headroom — 90% of the per-minute request ceiling remains"
        ),
        "the explanation must say the headroom preferred it:\n{explanation}"
    );
}

/// A candidate bound by something other than requests per minute does not
/// get an RPM preference invented for it.
#[test]
fn a_candidate_bound_by_another_dimension_has_an_inert_rpm_preference() {
    let by_tokens = free("alpha", "alpha-model").with_capacity(
        CandidateCapacity::new().with_remaining_capacity(Some(token_bound_score(90))),
    );
    let choice = decide(&support_routing(), &[by_tokens]).expect("admissible");
    let explanation = rendered(&choice);
    assert!(
        explanation.contains("+0.000  requests-per-minute headroom — this candidate is bound by"),
        "{explanation}"
    );
    assert!(
        explanation.contains("rather than by requests per minute — this preference is inert"),
        "{explanation}"
    );
}

// ===========================================================================
// Map line 1437 / 1438 — the preferences that wait on the requirements.
// ===========================================================================

/// Map line 1437: free capacity is preferred *after* the latency
/// requirement is satisfied — a free candidate that fails the ceiling does
/// not keep a metered one that meets it from being chosen.
#[test]
fn a_free_candidate_is_preferred_only_after_the_latency_ceiling_is_satisfied() {
    let routing = support_routing()
        .with_classification_policy(ClassificationPolicy::new().with_max_latency_ms(Some(800)));
    let slow_free = free("alpha", "alpha-model").with_classification_record(Some(record(
        "alpha",
        "alpha-model",
        5,
        5,
        Some(3000),
    )));
    let quick_metered = metered("beta", "beta-model").with_classification_record(Some(record(
        "beta",
        "beta-model",
        5,
        5,
        Some(0),
    )));

    let choice = decide(&routing, &[slow_free, quick_metered])
        .expect("the metered candidate meets every requirement");
    assert_eq!(choice.provider(), "beta", "{}", rendered(&choice));
    assert_eq!(choice.cost(), Cost::Metered);
    assert!(
        rendered(&choice).contains(
            "excluded candidate — alpha-model on alpha: median classification latency 3000ms"
        ),
        "{}",
        rendered(&choice)
    );

    // And with the ceiling satisfied, the free candidate wins as it always
    // has — the preference is intact, only its precondition is new.
    let quick_free = free("alpha", "alpha-model").with_classification_record(Some(record(
        "alpha",
        "alpha-model",
        5,
        5,
        Some(0),
    )));
    let quick_metered = metered("beta", "beta-model").with_classification_record(Some(record(
        "beta",
        "beta-model",
        5,
        5,
        Some(0),
    )));
    let choice = decide(&routing, &[quick_metered, quick_free]).expect("free is available");
    assert_eq!(choice.cost(), Cost::Free);
}

/// Map line 1438: a local candidate is preferred over an equally adequate
/// remote one the user has not ranked — and never over one the user *has*
/// ranked, which is the scoring invariant `choose` documents.
#[test]
fn a_local_candidate_is_preferred_over_an_equally_adequate_remote_one() {
    let remote = free("alpha", "alpha-model").with_locality(Locality::Remote);
    let local = free("ollama", "local-model").with_locality(Locality::Local);

    let choice =
        decide(&support_routing(), &[remote.clone(), local.clone()]).expect("both are admissible");
    assert_eq!(
        choice.provider(),
        "ollama",
        "the local candidate was listed second and must still win: {}",
        rendered(&choice)
    );
    assert!(
        rendered(&choice).contains("locality — local inference — preferred"),
        "{}",
        rendered(&choice)
    );

    let user_ranked_remote_first = DisposableRouting::for_support_work(
        true,
        FreePreferences::new().with_order(vec![FreeResourceKey::new("alpha", "alpha-model")]),
    );
    let choice = decide(&user_ranked_remote_first, &[remote, local]).expect("both are admissible");
    assert_eq!(
        choice.provider(),
        "alpha",
        "a candidate the user ranked is never displaced by a preference: {}",
        rendered(&choice)
    );
}

// ===========================================================================
// Map line 1427 — local only, at the policy.
// ===========================================================================

/// Under `local_only`, a remote candidate and one whose locality nobody
/// stated are both excluded, the refusal names why, and a local candidate
/// is the only thing that can be chosen.
#[test]
fn local_only_admits_no_remote_or_unstated_candidate_and_says_why() {
    let routing = support_routing()
        .with_classification_policy(ClassificationPolicy::new().with_local_only(true));
    let remote = free("alpha", "alpha-model").with_locality(Locality::Remote);
    let unstated = free("gamma", "gamma-model");

    let refusal = decide(&routing, &[remote.clone(), unstated.clone()])
        .expect_err("nothing local was offered");
    let NoResource::ClassificationRequirementsExcludedAll { reasons } = &refusal else {
        panic!("expected every candidate excluded, got {refusal:?}");
    };
    assert_eq!(reasons.len(), 2, "{reasons:?}");
    assert!(
        reasons[0].contains(
            "alpha-model on alpha: remote, and classification is confined to local models"
        ),
        "{reasons:?}"
    );
    assert!(
        reasons[1].contains("gamma-model on gamma: its locality was not stated"),
        "{reasons:?}"
    );
    let sentence = refusal.to_string();
    assert!(
        sentence.contains("so no model was asked"),
        "the refusal must say why no model was asked: {sentence}"
    );

    let local = free("ollama", "local-model").with_locality(Locality::Local);
    let choice = decide(&routing, &[remote, unstated, local]).expect("the local candidate serves");
    assert_eq!(choice.provider(), "ollama");
    assert!(
        rendered(&choice).contains("admitted under classification_local_only"),
        "{}",
        rendered(&choice)
    );
}

// ===========================================================================
// The honesty rule — unmeasured quantities never filter.
// ===========================================================================

/// **REQUIRED BEHAVIOR 6 (acceptance 7).** A candidate nothing was measured
/// about passes every filter, and the explanation names each inert
/// requirement and preference as inert rather than as satisfied.
#[test]
fn unmeasured_quantities_are_inert_and_named() {
    let routing = support_routing()
        .with_classification_policy(ClassificationPolicy::new().with_max_latency_ms(Some(800)));
    let unread = free("alpha", "alpha-model");
    let barely_read =
        free("beta", "beta-model").with_classification_record(Some(ClassificationRecord {
            provider: "beta".to_owned(),
            model: "beta-model".to_owned(),
            outcomes_recorded: 2,
            parsed: 0,
            timed: 2,
            median_duration_ms: None,
        }));

    let choice = decide(&routing, &[unread.clone(), barely_read.clone()])
        .expect("nothing measured means nothing excluded");
    assert_eq!(
        choice.provider(),
        "alpha",
        "with nothing measured the caller's order stands: {}",
        rendered(&choice)
    );
    let explanation = rendered(&choice);
    for expected in [
        "reliability floor — no classification history was read for this candidate — the 80% floor is inert; unproven, not unreliable",
        "structured-output reliability — no reliability figure yet (0 of 5 outcome-carrying classification calls) — this preference is inert",
        "latency ceiling — no latency figure yet (0 of 5 timed classification calls) — the 800ms ceiling is inert",
        "classification latency — no latency figure yet (0 of 5 timed classification calls) — this preference is inert",
        "requests-per-minute headroom — no requests-per-minute reading for this provider — this preference is inert",
        "locality — locality not stated by the caller — this preference is inert",
    ] {
        assert!(
            explanation.contains(expected),
            "missing `{expected}` in:\n{explanation}"
        );
    }
    assert!(
        !explanation.contains("excluded candidate"),
        "nothing may be excluded on an unmeasured quantity:\n{explanation}"
    );

    // Zero parsed of two is not "0% reliable" — it is below the count the
    // floor waits for, so `beta` is admitted too, and wins when listed
    // first.
    let choice = decide(&routing, &[barely_read, unread]).expect("still nothing excluded");
    assert_eq!(choice.provider(), "beta", "{}", rendered(&choice));
    assert!(
        rendered(&choice).contains("unproven, not unreliable: 0 of 2 classification calls parsed"),
        "{}",
        rendered(&choice)
    );
}

/// Map line 1441's invariant, extended: a retained pick that no longer
/// meets a classification requirement is not reused — the requirements are
/// applied before the retained pick is even looked for.
#[test]
fn a_retained_pick_that_no_longer_meets_the_requirements_is_not_reused() {
    let routing = support_routing();
    let pool = FreePool::new();
    let now = Instant::now();
    let alpha = free("alpha", "alpha-model");
    let beta = free("beta", "beta-model");

    let first = routing
        .choose_for_automatic_classification(
            &[alpha.clone(), beta.clone()],
            &pool,
            now,
            1_800_000_000,
            None,
            None,
        )
        .expect("a free candidate is available");
    let AutomaticClassificationDecision::Fresh(choice, pick) = first else {
        panic!("first decision must be fresh: {first:?}");
    };
    assert_eq!(choice.provider(), "alpha");

    let alpha_now_unreliable =
        alpha.with_classification_record(Some(record("alpha", "alpha-model", 1, 10, None)));
    let second = routing
        .choose_for_automatic_classification(
            &[alpha_now_unreliable, beta],
            &pool,
            now,
            1_800_000_005,
            None,
            Some(pick),
        )
        .expect("beta remains");
    let AutomaticClassificationDecision::Fresh(choice, _) = second else {
        panic!("an excluded retained pick must not be reused: {second:?}");
    };
    assert_eq!(choice.provider(), "beta", "{}", rendered(&choice));
}

// ===========================================================================
// Map lines 1463 / 1465 / 1466 — the pure readers.
// ===========================================================================

fn consumption(
    purpose: Option<&str>,
    harness_recorded: bool,
    calls: usize,
    input: Option<i64>,
    output: Option<i64>,
) -> PurposeConsumption {
    PurposeConsumption {
        purpose: purpose.map(str::to_owned),
        harness_recorded,
        sample_count: calls,
        input_tokens: input,
        output_tokens: output,
        cached_input_tokens: None,
        first_byte_sample_count: 0,
        mean_time_to_first_byte_ms: None,
        first_token_sample_count: 0,
        mean_time_to_first_token_ms: None,
        first_tool_call_sample_count: 0,
        mean_time_to_first_tool_call_ms: None,
    }
}

/// Map lines 1465 and 1466 at the reader: classification tokens are kept
/// apart from every other group, the fraction carries both denominators,
/// and an uncounted side never produces a fraction or a warning.
#[test]
fn routing_overhead_is_read_with_its_denominators_and_never_from_an_uncounted_side() {
    let overhead = RoutingOverhead::from_consumption(&[
        consumption(Some(CLASSIFICATION_PURPOSE), false, 4, Some(200), Some(100)),
        consumption(None, true, 2, Some(1000), Some(500)),
        consumption(None, false, 1, Some(300), Some(200)),
    ]);
    assert_eq!(overhead.classification_requests, 4);
    assert_eq!(overhead.classification_tokens, Some(300));
    assert_eq!(overhead.task_requests, 3);
    assert_eq!(overhead.task_tokens, Some(2000));
    assert_eq!(overhead.fraction(), Some(0.15));
    assert!(overhead.exceeds(ROUTING_OVERHEAD_WARNING_FRACTION));
    assert!(!overhead.exceeds(0.15), "the line is strict");

    let uncounted_task = RoutingOverhead::from_consumption(&[
        consumption(Some(CLASSIFICATION_PURPOSE), false, 4, Some(200), Some(100)),
        consumption(None, true, 2, None, None),
    ]);
    assert_eq!(uncounted_task.task_requests, 2);
    assert_eq!(uncounted_task.task_tokens, None);
    assert_eq!(uncounted_task.fraction(), None);
    assert!(
        !uncounted_task.exceeds(0.0),
        "an unmeasured comparison must never warn"
    );

    let empty = RoutingOverhead::from_consumption(&[]);
    assert_eq!(empty.fraction(), None);
    assert!(!empty.exceeds(0.0));
}

/// Map line 1463's denominator: an interactive hour is an epoch-aligned
/// hour a session's activity span touches, clipped to the window.
#[test]
fn interactive_hours_count_the_hours_a_session_touched() {
    let hour = 3600;
    let t0 = 1_800_000_000 / hour * hour;
    let from = t0;
    let to = t0 + 24 * hour;

    // One session, one minute, one hour.
    assert_eq!(interactive_hours([(t0 + 10, t0 + 70)], from, to), 1);
    // Two sessions in the same hour are still one interactive hour.
    assert_eq!(
        interactive_hours([(t0 + 10, t0 + 70), (t0 + 100, t0 + 200)], from, to),
        1
    );
    // A span crossing an hour boundary touches two.
    assert_eq!(
        interactive_hours([(t0 + hour - 5, t0 + hour + 5)], from, to),
        2
    );
    // A span before the window contributes nothing; one straddling it is clipped.
    assert_eq!(interactive_hours([(t0 - 2 * hour, t0 - hour)], from, to), 0);
    assert_eq!(interactive_hours([(t0 - hour, t0 + 5)], from, to), 1);
    // No spans, no hours.
    assert_eq!(interactive_hours(Vec::<(i64, i64)>::new(), from, to), 0);
}

// ===========================================================================
// Map lines 1423 / 1795 / 1427 — configuration layering.
// ===========================================================================

/// The fallback chain and the local-only confinement layer project over
/// user over their defaults, per field, like every other `[routing]`
/// value.
#[test]
fn the_fallback_chain_and_local_only_layer_project_over_user() {
    let mut user = UserConfig::default();
    user.routing_mut()
        .set_model_fallback(Some(vec![FreeResourceRef::new("alpha", "alpha-model")]))
        .set_classification_local_only(Some(false));

    let effective = EffectiveConfig::new(&user, None);
    assert_eq!(
        effective.routing_model_fallback(),
        Layered::new(
            vec![FreeResourceRef::new("alpha", "alpha-model")],
            Layer::User
        )
    );
    assert_eq!(
        effective.classification_local_only(),
        Layered::new(false, Layer::User)
    );

    let mut project = ProjectConfig::default();
    project
        .routing_mut()
        .set_model_fallback(Some(vec![FreeResourceRef::new("ollama", "local-model")]))
        .set_classification_local_only(Some(true));
    let effective = EffectiveConfig::new(&user, Some(&project));
    assert_eq!(
        effective.routing_model_fallback(),
        Layered::new(
            vec![FreeResourceRef::new("ollama", "local-model")],
            Layer::Project
        )
    );
    assert_eq!(
        effective.classification_local_only(),
        Layered::new(true, Layer::Project)
    );

    let unset = UserConfig::default();
    let effective = EffectiveConfig::new(&unset, None);
    assert_eq!(
        effective.routing_model_fallback(),
        Layered::new(Vec::new(), Layer::Default)
    );
    assert_eq!(
        effective.classification_local_only(),
        Layered::new(false, Layer::Default)
    );
}

// ===========================================================================
// The shipped binary, against a canned endpoint and planted ledgers.
// ===========================================================================

const CREDENTIAL_VAR: &str = "GLASSHOUSE_TEST_ONLY_ROUTING_ECONOMICS_KEY";
const CREDENTIAL: &str = "sk-fabricated-test-value-not-a-real-credential";
const ROUTE: &str = "openai-chat";
const QUESTION: &str = "what is a monad";

/// A classification no run of `classify_heuristically` can produce for
/// [`QUESTION`] — `frontier` has no heuristic producer, so a report saying
/// so is a fact only a model answer can explain.
const MODEL_ANSWER: &str = r#"{
  "needs_repo_context": true,
  "needs_code_modification": true,
  "needs_shell_execution": true,
  "needs_browser_interaction": true,
  "complexity": "complex",
  "likely_multi_turn": true,
  "workload_tier": "frontier",
  "safe_for_disposable_model": false,
  "warm_context": "prefer_warm",
  "confidence": "high"
}"#;

/// One request as it arrived on the wire.
#[derive(Debug, Clone)]
struct Seen {
    body: String,
}

enum Answer {
    Content(String),
}

/// A canned OpenAI chat-completions endpoint that decides its answer from
/// the request body and remembers every request it was sent — the same
/// shape `tests/classification_call.rs` uses, so *"the request arrived,
/// naming this model"* stays a claim about the wire.
struct FakeModel {
    address: SocketAddr,
    seen: Arc<Mutex<Vec<Seen>>>,
    stop: Arc<AtomicBool>,
}

impl FakeModel {
    fn start(responder: impl Fn(&str) -> Answer + Send + Sync + 'static) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("loopback must bind");
        listener
            .set_nonblocking(true)
            .expect("the accept loop polls its stop flag");
        let address = listener.local_addr().unwrap();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let thread_seen = Arc::clone(&seen);
        let thread_stop = Arc::clone(&stop);
        std::thread::spawn(move || {
            while !thread_stop.load(Ordering::SeqCst) {
                match listener.accept() {
                    Ok((stream, _)) => {
                        let _ = stream.set_nonblocking(false);
                        serve(stream, &thread_seen, &responder);
                    }
                    Err(ref err) if err.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(std::time::Duration::from_millis(5));
                    }
                    Err(_) => break,
                }
            }
        });
        Self {
            address,
            seen,
            stop,
        }
    }

    fn base_url(&self) -> String {
        format!("http://{}/v1", self.address)
    }

    fn requests(&self) -> Vec<Seen> {
        self.seen.lock().unwrap().clone()
    }
}

impl Drop for FakeModel {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
    }
}

fn serve(
    mut stream: TcpStream,
    seen: &Arc<Mutex<Vec<Seen>>>,
    responder: &(impl Fn(&str) -> Answer + ?Sized),
) {
    let mut reader = BufReader::new(stream.try_clone().expect("the stream clones"));
    let mut request_line = String::new();
    if reader.read_line(&mut request_line).is_err() || request_line.is_empty() {
        return;
    }
    let mut length = 0usize;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).is_err() {
            return;
        }
        let line = line.trim_end_matches(['\r', '\n']);
        if line.is_empty() {
            break;
        }
        if let Some((name, value)) = line.split_once(':')
            && name.trim().eq_ignore_ascii_case("content-length")
        {
            length = value.trim().parse().unwrap_or(0);
        }
    }
    let mut body = vec![0u8; length];
    if reader.read_exact(&mut body).is_err() {
        return;
    }
    let body = String::from_utf8_lossy(&body).into_owned();
    seen.lock().unwrap().push(Seen { body: body.clone() });

    let Answer::Content(content) = responder(&body);
    let document = serde_json::json!({
        "choices": [{ "message": { "role": "assistant", "content": content } }],
        "usage": { "prompt_tokens": 314, "completion_tokens": 15 }
    })
    .to_string();
    let response = format!(
        "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\
         connection: close\r\n\r\n{document}",
        document.len()
    );
    let _ = stream.write_all(response.as_bytes());
    let _ = stream.flush();
}

/// A project, a private data directory, and the binary run against them.
struct Fixture {
    _tmp: tempfile::TempDir,
    base: PathBuf,
    root: PathBuf,
    runtime: Runtime,
}

struct Ran {
    stdout: String,
    stderr: String,
    status: std::process::ExitStatus,
}

impl Fixture {
    fn new() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path().to_path_buf();
        let root = base.join("workspace");
        std::fs::create_dir_all(root.join(".git")).unwrap();
        let root = std::fs::canonicalize(&root).unwrap();
        let runtime = bootstrap(&base, &root);
        Self {
            _tmp: tmp,
            base,
            root,
            runtime,
        }
    }

    fn config(&self) -> UserConfig {
        UserConfig::load(self.runtime.paths()).unwrap()
    }

    fn save(&self, user: UserConfig) {
        user.save(self.runtime.paths()).unwrap();
    }

    fn add_provider(&self, name: &str, model: &str, base_url: &str) {
        let mut user = self.config();
        let mut provider = ProviderConfig::new("openai-compatible");
        provider.set_base_url(Some(base_url.to_owned()));
        provider.set_credential_env(vec![CREDENTIAL_VAR.to_owned()]);
        provider.set_free_models(vec![model.to_owned()]);
        user.providers_mut().set(name, provider);
        self.save(user);
    }

    fn edit_routing(&self, edit: impl FnOnce(&mut glasshouse::config::RoutingConfig)) {
        let mut user = self.config();
        edit(user.routing_mut());
        self.save(user);
    }

    fn run(&self, args: &[&str]) -> Ran {
        let output = Command::new(env!("CARGO_BIN_EXE_glasshouse"))
            .env(CREDENTIAL_VAR, CREDENTIAL)
            .arg("--scope")
            .arg(&self.root)
            .arg("--data-dir")
            .arg(self.base.join("data"))
            .arg("--config-dir")
            .arg(self.base.join("config"))
            .args(args)
            .output()
            .expect("the glasshouse binary must be runnable");
        Ran {
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
            status: output.status,
        }
    }

    fn classify(&self, text: &str) -> Ran {
        self.run(&["classify", text])
    }

    fn observations(&self, provider: &str, model: &str) -> Vec<RoutingObservation> {
        EvidenceLedger::open(&self.runtime)
            .unwrap()
            .recent(
                ObservationQuery {
                    provider,
                    model,
                    route: Some(ROUTE),
                    harness: None,
                },
                16,
            )
            .unwrap()
    }

    /// Plant one classification row as the producer would have written it.
    fn plant_classification(&self, provider: &str, model: &str, outcome: Outcome, at: i64) {
        EvidenceLedger::open(&self.runtime)
            .unwrap()
            .record(
                NewObservation::new(provider, model)
                    .with_route(Some(ROUTE))
                    .with_purpose(Some(CLASSIFICATION_PURPOSE))
                    .with_timing(Some(at), Some(at))
                    .with_tokens(Some(50), Some(50), None)
                    .with_outcome(outcome),
                at,
            )
            .unwrap();
    }
}

fn bootstrap(base: &Path, root: &Path) -> Runtime {
    let cli = Cli::try_parse_from([
        "glasshouse",
        "--data-dir",
        base.join("data").to_str().unwrap(),
        "--config-dir",
        base.join("config").to_str().unwrap(),
    ])
    .unwrap();
    glasshouse::bootstrap(&cli, root).unwrap()
}

fn source_line(stdout: &str) -> &str {
    stdout
        .lines()
        .find(|line| line.starts_with("source"))
        .unwrap_or_else(|| panic!("no source line in:\n{stdout}"))
}

/// The reader's own floor: a classification median is withheld below
/// [`MIN_SAMPLE_FOR_SUMMARY`] timed rows and attached at it, and a row
/// with no recorded outcome counts toward neither reliability side.
#[test]
fn the_ledger_withholds_a_classification_median_below_the_sample_floor() {
    let fixture = Fixture::new();
    let now = glasshouse::provider::cache::now_unix_seconds();
    let ledger = EvidenceLedger::open(&fixture.runtime).unwrap();
    let window = glasshouse::routing::evidence::CLASSIFICATION_EVIDENCE_WINDOW_SECONDS;

    // A row from a build that recorded neither outcome nor timing.
    ledger
        .record(
            NewObservation::new("alpha-runner", "alpha-model")
                .with_route(Some(ROUTE))
                .with_purpose(Some(CLASSIFICATION_PURPOSE)),
            now - 1000,
        )
        .unwrap();
    for i in 0..(MIN_SAMPLE_FOR_SUMMARY - 1) {
        fixture.plant_classification(
            "alpha-runner",
            "alpha-model",
            Outcome::Succeeded,
            now - 500 + i as i64,
        );
    }
    let record = ledger
        .classification_record("alpha-runner", "alpha-model", now, window)
        .unwrap();
    assert_eq!(record.outcomes_recorded, MIN_SAMPLE_FOR_SUMMARY - 1);
    assert_eq!(record.parsed, MIN_SAMPLE_FOR_SUMMARY - 1);
    assert_eq!(record.timed, MIN_SAMPLE_FOR_SUMMARY - 1);
    assert_eq!(
        record.median_duration_ms, None,
        "below the sample floor there is no median, only a count"
    );

    fixture.plant_classification("alpha-runner", "alpha-model", Outcome::Failed, now - 100);
    let record = ledger
        .classification_record("alpha-runner", "alpha-model", now, window)
        .unwrap();
    assert_eq!(record.outcomes_recorded, MIN_SAMPLE_FOR_SUMMARY);
    assert_eq!(record.parsed, MIN_SAMPLE_FOR_SUMMARY - 1);
    assert_eq!(record.timed, MIN_SAMPLE_FOR_SUMMARY);
    assert_eq!(
        record.median_duration_ms,
        Some(0),
        "at the floor the median is attached, at the ledger's one-second resolution"
    );

    // Rows under another purpose say nothing about the classifier.
    ledger
        .record(
            NewObservation::new("alpha-runner", "alpha-model")
                .with_route(Some(ROUTE))
                .with_outcome(Outcome::Failed),
            now - 50,
        )
        .unwrap();
    let record = ledger
        .classification_record("alpha-runner", "alpha-model", now, window)
        .unwrap();
    assert_eq!(record.outcomes_recorded, MIN_SAMPLE_FOR_SUMMARY);
}

/// **REQUIRED BEHAVIOR 3 (acceptance 4).** A chain of two: the first's
/// endpoint returns garbage, the second's a valid classification. The
/// classification is the second's, both attempts left a row carrying its
/// parse outcome, the label names the walk, and — the invariant — the
/// chain entry naming the model that already failed is not retried.
#[test]
fn a_fallback_chain_is_walked_once_and_every_attempt_is_recorded() {
    let model = FakeModel::start(|body| {
        if body.contains("alpha-model") {
            Answer::Content("this is not a classification".to_owned())
        } else {
            Answer::Content(MODEL_ANSWER.to_owned())
        }
    });
    let fixture = Fixture::new();
    fixture.add_provider("alpha-runner", "alpha-model", &model.base_url());
    fixture.add_provider("beta-runner", "beta-model", &model.base_url());
    fixture.edit_routing(|routing| {
        routing.set_model(Some(RoutingModelChoice::Pinned {
            provider: "alpha-runner".to_owned(),
            model: "alpha-model".to_owned(),
        }));
        // The first entry names the pinned model itself: it must be
        // skipped, never retried.
        routing.set_model_fallback(Some(vec![
            FreeResourceRef::new("alpha-runner", "alpha-model"),
            FreeResourceRef::new("beta-runner", "beta-model"),
        ]));
    });

    let ran = fixture.classify(QUESTION);
    assert!(ran.status.success(), "{}", ran.stderr);
    assert!(
        ran.stdout.contains("frontier"),
        "the classification must be the second model's:\n{}",
        ran.stdout
    );
    let source = source_line(&ran.stdout);
    assert!(
        source
            .contains("beta-runner/beta-model via openai-chat, after alpha-model on alpha-runner:"),
        "the label must name the walk: {source}"
    );
    assert!(
        !ran.stderr
            .contains("deterministic heuristics answered instead"),
        "the chain answered, so no degrade was needed:\n{}",
        ran.stderr
    );

    let requests = model.requests();
    assert_eq!(
        requests.len(),
        2,
        "exactly one call per distinct model; the duplicate entry must not be retried: {requests:?}"
    );
    assert!(requests[0].body.contains("alpha-model"));
    assert!(requests[1].body.contains("beta-model"));

    let alpha = fixture.observations("alpha-runner", "alpha-model");
    assert_eq!(
        alpha.len(),
        1,
        "the failed attempt must still leave its row"
    );
    assert_eq!(alpha[0].outcome, Some(Outcome::Failed));
    assert_eq!(alpha[0].purpose.as_deref(), Some(CLASSIFICATION_PURPOSE));
    assert!(
        alpha[0].duration_ms().is_some(),
        "the row must carry the clock either side of the call: {:?}",
        alpha[0]
    );
    let beta = fixture.observations("beta-runner", "beta-model");
    assert_eq!(beta.len(), 1);
    assert_eq!(beta[0].outcome, Some(Outcome::Succeeded));
    assert!(beta[0].duration_ms().is_some());
}

/// Without a chain, a failing model degrades exactly as it always did.
#[test]
fn without_a_chain_a_parse_failure_degrades_to_the_heuristic_as_before() {
    let model = FakeModel::start(|_| Answer::Content("not json".to_owned()));
    let fixture = Fixture::new();
    fixture.add_provider("alpha-runner", "alpha-model", &model.base_url());
    fixture.edit_routing(|routing| {
        routing.set_model(Some(RoutingModelChoice::Pinned {
            provider: "alpha-runner".to_owned(),
            model: "alpha-model".to_owned(),
        }));
    });
    let ran = fixture.classify(QUESTION);
    assert!(ran.status.success());
    assert!(
        source_line(&ran.stdout).contains("deterministic heuristics"),
        "{}",
        ran.stdout
    );
    assert!(
        ran.stderr
            .contains("deterministic heuristics answered instead"),
        "{}",
        ran.stderr
    );
    assert!(
        !ran.stderr.contains("alpha-model on alpha-runner:"),
        "a single failure is reported bare, as before this package:\n{}",
        ran.stderr
    );
    assert_eq!(model.requests().len(), 1);
}

/// **REQUIRED BEHAVIOR 4 (acceptance 5).** With only remote candidates and
/// `classification_local_only = true`, no request leaves the process — in
/// automatic mode, through the fallback chain, and for a pinned remote
/// model — the heuristic classifies, and the explanation says why.
#[test]
fn local_only_never_sends_a_remote_request() {
    let model = FakeModel::start(|_| Answer::Content(MODEL_ANSWER.to_owned()));
    let fixture = Fixture::new();
    fixture.add_provider("remote-runner", "remote-model", &model.base_url());
    fixture.add_provider("other-runner", "other-model", &model.base_url());
    fixture.edit_routing(|routing| {
        routing.set_model(Some(RoutingModelChoice::Automatic));
        routing.set_classification_local_only(Some(true));
        routing.set_model_fallback(Some(vec![FreeResourceRef::new(
            "other-runner",
            "other-model",
        )]));
    });

    let ran = fixture.classify(QUESTION);
    assert!(ran.status.success(), "{}", ran.stderr);
    assert!(
        source_line(&ran.stdout).contains("deterministic heuristics"),
        "{}",
        ran.stdout
    );
    assert!(
        ran.stderr
            .contains("classification is confined to local models")
            && ran
                .stderr
                .contains("deterministic heuristics answered instead"),
        "the degrade must say why no model was asked:\n{}",
        ran.stderr
    );
    assert!(
        model.requests().is_empty(),
        "a request left the process under local-only: {:?}",
        model.requests()
    );

    // The pinned form: the user named a remote model *and* confined
    // classification to local ones. Privacy wins, and the chain (also
    // remote) is not walked either.
    fixture.edit_routing(|routing| {
        routing.set_model(Some(RoutingModelChoice::Pinned {
            provider: "remote-runner".to_owned(),
            model: "remote-model".to_owned(),
        }));
    });
    let ran = fixture.classify(QUESTION);
    assert!(ran.status.success(), "{}", ran.stderr);
    assert!(source_line(&ran.stdout).contains("deterministic heuristics"));
    assert!(ran.stderr.contains("no request was sent"), "{}", ran.stderr);
    assert!(
        model.requests().is_empty(),
        "a pinned remote model was contacted under local-only: {:?}",
        model.requests()
    );

    // `glasshouse resources` names the same refusal in the routing-model
    // block rather than pretending a model would be selected.
    fixture.edit_routing(|routing| {
        routing.set_model(Some(RoutingModelChoice::Automatic));
    });
    let ran = fixture.run(&["resources", "--no-harness"]);
    assert!(ran.status.success(), "{}", ran.stderr);
    assert!(
        ran.stdout.contains("would select    nothing — every configured candidate was excluded by a classification requirement"),
        "{}",
        ran.stdout
    );
    assert!(model.requests().is_empty());
}

/// **The §35 test for the reliability filter.** The ledger's own record —
/// three parsed of ten for the model the user ranked first — reaches the
/// shipped binary's selector and keeps that model from being asked. Fails
/// on a build where `main.rs` never reads the ledger back into the
/// candidate, whatever the policy tests above prove.
#[test]
fn a_candidate_the_ledger_shows_unreliable_is_not_asked_by_the_shipped_binary() {
    let model = FakeModel::start(|_| Answer::Content(MODEL_ANSWER.to_owned()));
    let fixture = Fixture::new();
    fixture.add_provider("alpha-runner", "alpha-model", &model.base_url());
    fixture.add_provider("zeta-runner", "zeta-model", &model.base_url());
    fixture.edit_routing(|routing| {
        routing.set_model(Some(RoutingModelChoice::Automatic));
        routing.set_free_resource_order(Some(vec![FreeResourceRef::new(
            "alpha-runner",
            "alpha-model",
        )]));
    });
    let now = glasshouse::provider::cache::now_unix_seconds();
    for i in 0..10 {
        let outcome = if i < 3 {
            Outcome::Succeeded
        } else {
            Outcome::Failed
        };
        fixture.plant_classification("alpha-runner", "alpha-model", outcome, now - 600 + i);
    }

    let ran = fixture.classify(QUESTION);
    assert!(ran.status.success(), "{}", ran.stderr);
    assert!(ran.stdout.contains("frontier"), "{}", ran.stdout);
    let requests = model.requests();
    assert_eq!(requests.len(), 1, "{requests:?}");
    assert!(
        requests[0].body.contains("zeta-model") && !requests[0].body.contains("alpha-model"),
        "the model the ledger shows unreliable must not be the one asked: {}",
        requests[0].body
    );

    let ran = fixture.run(&["resources", "--no-harness", "--verbose"]);
    assert!(ran.status.success(), "{}", ran.stderr);
    assert!(
        ran.stdout
            .contains("would select    zeta-model on zeta-runner"),
        "{}",
        ran.stdout
    );
    assert!(
        ran.stdout.contains("excluded candidate — alpha-model on alpha-runner: only 3 of 10 classification calls came back in the schema (30%)"),
        "the diagnostic must name the exclusion and its ratio:\n{}",
        ran.stdout
    );
}

/// **REQUIRED BEHAVIOR 5 (acceptance 6).** `glasshouse resources` prints
/// decisions per interactive hour, classification spend against other
/// spend, and a warning once the fraction is crossed — every figure with
/// its denominator.
#[test]
fn resources_reports_routing_overhead_with_denominators_and_warns_past_the_fraction() {
    let fixture = Fixture::new();
    let now = glasshouse::provider::cache::now_unix_seconds();

    // Four classification calls at 100 tokens each, against two other calls
    // at 1000 tokens each: 400 over 2000 is 20%, above the 10% line.
    for i in 0..4 {
        fixture.plant_classification(
            "alpha-runner",
            "alpha-model",
            Outcome::Succeeded,
            now - 60 + i,
        );
    }
    {
        let ledger = EvidenceLedger::open(&fixture.runtime).unwrap();
        for i in 0..2 {
            ledger
                .record(
                    NewObservation::new("anyrouter", "a-coding-model")
                        .with_route(Some(ROUTE))
                        .with_harness(Some("claude-code"))
                        .with_tokens(Some(600), Some(400), None),
                    now - 30 + i,
                )
                .unwrap();
        }
    }
    {
        let evaluation = EvaluationObservations::open(&fixture.runtime).unwrap();
        for i in 0..3 {
            evaluation
                .record(
                    NewEvaluation::new(EvaluationKind::RoutingContinuationDecided)
                        .with_subject("fresh")
                        .with_detail("destination"),
                    now - 20 + i,
                )
                .unwrap();
        }
    }
    {
        let sessions = ProjectSessions::open(&fixture.runtime).unwrap();
        let store = sessions.store();
        let record = store.create(NewSession::embedded("claude-code")).unwrap();
        store
            .set_lifecycle(&record.id, SessionLifecycle::Running)
            .unwrap();
    }

    let ran = fixture.run(&["resources", "--no-harness"]);
    assert!(ran.status.success(), "{}", ran.stderr);
    let block = ran
        .stdout
        .split_once("ROUTING ECONOMICS")
        .unwrap_or_else(|| panic!("no ROUTING ECONOMICS block in:\n{}", ran.stdout))
        .1;
    for expected in [
        "decisions       3 routing decisions over 1 interactive hours — 3.0 per interactive hour",
        "an interactive hour is a wall-clock hour in which a session record shows activity",
        "routing spend   400 tokens over 4 classification calls",
        "task spend      2000 tokens over 2 other calls",
        "overhead        20.0% of task spend — warns above 10%",
        "warning         routing is consuming 20.0% of the task spend it exists to protect, above the 10% line",
    ] {
        assert!(
            block.contains(expected),
            "missing `{expected}` in:\n{block}"
        );
    }
}

/// The economics block never fabricates: a project with nothing recorded
/// prints its zero denominators and *not comparable*, never `0%`, and the
/// command still succeeds.
#[test]
fn resources_with_nothing_recorded_prints_denominators_and_no_warning() {
    let fixture = Fixture::new();
    let ran = fixture.run(&["resources", "--no-harness"]);
    assert!(ran.status.success(), "{}", ran.stderr);
    let block = ran
        .stdout
        .split_once("ROUTING ECONOMICS")
        .unwrap_or_else(|| panic!("no ROUTING ECONOMICS block in:\n{}", ran.stdout))
        .1;
    assert!(
        block.contains("decisions       0 routing decisions over 0 interactive hours — no interactive hour in the window, so no rate"),
        "{block}"
    );
    assert!(
        block.contains("routing spend   tokens not counted over 0 classification calls"),
        "{block}"
    );
    assert!(
        block.contains("overhead        not comparable — no classification call in the window carried a token count"),
        "{block}"
    );
    assert!(!block.contains("warning"), "{block}");
    assert!(!block.contains("0%"), "a fabricated zero fraction: {block}");
}
