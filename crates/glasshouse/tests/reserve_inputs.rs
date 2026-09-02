//! GH-RESERVE-INPUTS: the three inputs the protected-reserve policy could not
//! observe — capability map lines 1289, 1290 and 1294.
//!
//! `provider::quota::evaluate_reserve_spend` has always taken six inputs and
//! `routing::disposable` has always passed three of them as the literal
//! `false`. This file is about what happened to those three:
//!
//! - **1289** — *"allow high-tier tasks to consume protected reserve when
//!   their capability requirement justifies it"*. Closed on the tier, which
//!   is the scale a capability *requirement* is expressed on here. The
//!   hard-capability set (`TaskClassification::hard_capabilities`) is
//!   deliberately **not** a second input beside it, and
//!   `the_hard_capability_set_is_not_a_reserve_input` is the test that says
//!   so in code rather than only in a doc comment.
//! - **1290** — *"allow the user to override reserve protection for a
//!   specific task or session"*. `user_override` now has a producer:
//!   `ReserveOverride`, which is a *scope* and not a switch.
//! - **1294** — refused. Nothing in this build can observe that a task is
//!   almost complete, and `nothing_in_this_build_produces_task_nearly_complete`
//!   is the evidence rather than an assertion that it is hard.
//!
//! Every test that exercises the override goes through
//! `DisposableRouting::choose`, which is the function the shipped binary
//! calls (`main.rs::disposable_extraction_model` →
//! `memory::RoutedModel::new` → here). Practice §35: a policy proven only
//! against `evaluate_reserve_spend` in isolation would be proven below the
//! production entry point.

use glasshouse::config::{EffectiveConfig, ProjectConfig, UserConfig};
use glasshouse::provider::quota::{
    CapacityBand, RESET_DISTANT_SECONDS, RESET_IMMINENT_SECONDS, ReserveDecisionInputs,
    evaluate_reserve_spend,
};
use glasshouse::routing::classify::{HardCapability, WorkloadTier, classify_heuristically};
use glasshouse::routing::disposable::{
    CandidateCapacity, DisposableCandidate, DisposableRouting, JobKind, NoResource, ReserveOverride,
};
use glasshouse::routing::free::{FreePool, FreePreferences};
use glasshouse::routing::{Cost, CredentialId};
use glasshouse::secret::SecretRef;

const OVERRIDDEN: &str = "session-the-user-named";
const NOT_OVERRIDDEN: &str = "session-the-user-did-not-name";

fn credential(provider: &str) -> CredentialId {
    CredentialId::new(
        provider,
        SecretRef::Environment {
            var: format!("{}_API_KEY", provider.to_uppercase()),
        },
    )
}

/// One metered candidate in the Reserve band with a distant reset — the shape
/// `evaluate_reserve_spend` denies for everything below the heavy tier. No
/// free candidate is offered anywhere in this file, so `choose` always
/// reaches the metered-fallback branch where the reserve policy runs.
fn reserve_banded_candidate() -> DisposableCandidate {
    DisposableCandidate::new(
        "openrouter",
        "a-reserved-model",
        credential("openrouter"),
        Cost::Metered,
    )
    .with_capacity(
        CandidateCapacity::new()
            .with_band(Some(CapacityBand::Reserve))
            .with_seconds_until_reset(Some(RESET_DISTANT_SECONDS + 1)),
    )
}

/// Route one disposable job through the production seam and say whether the
/// metered candidate survived the reserve gate.
fn reserve_allows(routing: &DisposableRouting) -> bool {
    match routing.choose(
        JobKind::MemoryExtraction,
        &[reserve_banded_candidate()],
        &FreePool::new(),
        std::time::Instant::now(),
        None,
    ) {
        Ok(choice) => {
            assert_eq!(choice.model(), "a-reserved-model");
            true
        }
        Err(NoResource::ProtectedReserveDenied { .. }) => false,
        Err(other) => panic!("the reserve policy was not what decided this: {other}"),
    }
}

// --- 1. No regression: every decision reachable today is unchanged ----------

/// The reserve policy restated from the map's own words, independently of
/// `evaluate_reserve_spend`'s code.
///
/// Deliberately a second implementation rather than a recorded snapshot of
/// the first: a snapshot taken from the function under test rescales with any
/// change to it, which is §80 case 6's failure — a test whose expectation is
/// derived from the thing being mutated cannot detect that thing changing.
/// This one is derived from lines 1288-1292 and 1294 as written on the map,
/// so it disagrees the moment the code does.
fn what_the_map_says(inputs: &ReserveDecisionInputs) -> bool {
    if inputs.task_nearly_complete {
        return true; // 1294
    }
    if inputs.user_override {
        return true; // 1290
    }
    if inputs.band > CapacityBand::Reserve {
        return true; // not in the protected band at all
    }
    if let Some(seconds) = inputs.seconds_until_reset {
        if seconds <= RESET_IMMINENT_SECONDS {
            return true; // 1291
        }
        if seconds >= RESET_DISTANT_SECONDS && inputs.tier < WorkloadTier::Heavy {
            return false; // 1292
        }
    }
    if inputs.tier >= WorkloadTier::Heavy {
        return true; // 1289
    }
    !inputs.cheaper_adequate_resource_exists // 1288
}

/// Acceptance test 1. Every combination of the inputs a caller can present
/// today — with `user_override` and `task_nearly_complete` at the `false`
/// they were before this package, which is the state every existing caller is
/// still in — decides exactly as it did.
#[test]
fn every_reserve_decision_reachable_before_this_package_is_unchanged() {
    let bands = [
        CapacityBand::Exhausted,
        CapacityBand::Reserve,
        CapacityBand::Tight,
        CapacityBand::Healthy,
        CapacityBand::Plenty,
    ];
    let tiers = [
        WorkloadTier::Deterministic,
        WorkloadTier::Leaf,
        WorkloadTier::Standard,
        WorkloadTier::Heavy,
        WorkloadTier::Frontier,
    ];
    let resets = [
        None,
        Some(0),
        Some(RESET_IMMINENT_SECONDS),
        Some(RESET_IMMINENT_SECONDS + 1),
        Some(RESET_DISTANT_SECONDS - 1),
        Some(RESET_DISTANT_SECONDS),
        Some(RESET_DISTANT_SECONDS + 1),
    ];

    let mut checked = 0;
    for band in bands {
        for tier in tiers {
            for cheaper in [false, true] {
                for seconds_until_reset in resets {
                    let inputs = ReserveDecisionInputs {
                        band,
                        tier,
                        cheaper_adequate_resource_exists: cheaper,
                        user_override: false,
                        seconds_until_reset,
                        task_nearly_complete: false,
                    };
                    let decision = evaluate_reserve_spend(inputs);
                    assert_eq!(
                        decision.is_allowed(),
                        what_the_map_says(&inputs),
                        "band={band}, tier={tier}, cheaper={cheaper}, \
                         reset={seconds_until_reset:?}: {}",
                        decision.reason()
                    );
                    checked += 1;
                }
            }
        }
    }
    assert_eq!(
        checked,
        bands.len() * tiers.len() * 2 * resets.len(),
        "the sweep did not cover what it claims to"
    );
}

// --- 2. The override applies where the user said, and nowhere else ----------

/// Acceptance test 2, first half. The user named this session, so its
/// background work may spend the protected reserve that would otherwise be
/// denied — through `DisposableRouting::choose`, the function the binary
/// calls.
#[test]
fn the_override_grants_the_reserve_for_the_session_the_user_named() {
    let routing = DisposableRouting::for_support_work(true, FreePreferences::new())
        .with_reserve_override(
            ReserveOverride::for_sessions([OVERRIDDEN]).deciding_for(OVERRIDDEN),
        );

    assert!(
        reserve_allows(&routing),
        "the user overrode reserve protection for this session; its work must be allowed to \
         spend the reserve (map line 1290)"
    );
}

/// **Acceptance test 2, second half — the one that matters.** The same
/// configured override, deciding for a session the user never named, must
/// change nothing at all.
///
/// This is the test that distinguishes line 1290 from "the reserve is off":
/// a `bool` producer would pass the half above and fail here.
#[test]
fn the_override_does_not_reach_a_session_the_user_did_not_name() {
    let routing = DisposableRouting::for_support_work(true, FreePreferences::new())
        .with_reserve_override(
            ReserveOverride::for_sessions([OVERRIDDEN]).deciding_for(NOT_OVERRIDDEN),
        );

    assert!(
        !reserve_allows(&routing),
        "an override the user granted to `{OVERRIDDEN}` must not spend protected reserve on \
         `{NOT_OVERRIDDEN}`'s behalf"
    );
}

/// The two halves above, in one call each, on the identical candidate and the
/// identical configured override — so the difference in outcome is
/// attributable to the session being decided for and to nothing else.
#[test]
fn the_same_override_decides_two_sessions_differently() {
    let granted = ReserveOverride::for_sessions([OVERRIDDEN]);
    let named = DisposableRouting::for_support_work(true, FreePreferences::new())
        .with_reserve_override(granted.clone().deciding_for(OVERRIDDEN));
    let other = DisposableRouting::for_support_work(true, FreePreferences::new())
        .with_reserve_override(granted.deciding_for(NOT_OVERRIDDEN));

    assert!(reserve_allows(&named));
    assert!(!reserve_allows(&other));
}

/// There is no value of this type that means "every session". The empty
/// override, an override that named sessions but is deciding for none, and an
/// override whose set is empty all refuse, so no caller can reach a global
/// kill switch by construction.
#[test]
fn no_reserve_override_means_everywhere() {
    for (what, over) in [
        ("the default", ReserveOverride::none()),
        (
            "an empty set",
            ReserveOverride::for_sessions(Vec::<String>::new()),
        ),
        (
            "an empty set, deciding for a session",
            ReserveOverride::for_sessions(Vec::<String>::new()).deciding_for(OVERRIDDEN),
        ),
        (
            "a named set with no session to decide for",
            ReserveOverride::for_sessions([OVERRIDDEN]),
        ),
    ] {
        assert!(!over.applies(), "{what} must not override anything");
        assert_eq!(over.granted_session(), None, "{what}");
        let routing = DisposableRouting::for_support_work(true, FreePreferences::new())
            .with_reserve_override(over);
        assert!(!reserve_allows(&routing), "{what} allowed the reserve");
    }
}

/// A policy built without ever mentioning the override behaves exactly as it
/// did before this package — the arriving type is a no-op for every caller
/// that predates it.
#[test]
fn a_routing_policy_that_names_no_override_is_unchanged() {
    let routing = DisposableRouting::for_support_work(true, FreePreferences::new());
    assert_eq!(routing.reserve_override(), &ReserveOverride::none());
    assert!(!reserve_allows(&routing));
}

/// The override is *visible*, not silent: a spend of protected reserve names
/// the session the user granted it for, in the routing explanation the
/// choice carries home (map line 1293).
#[test]
fn a_granted_override_names_its_session_in_the_explanation() {
    let routing = DisposableRouting::for_support_work(true, FreePreferences::new())
        .with_reserve_override(
            ReserveOverride::for_sessions([OVERRIDDEN]).deciding_for(OVERRIDDEN),
        );

    let choice = routing
        .choose(
            JobKind::MemoryExtraction,
            &[reserve_banded_candidate()],
            &FreePool::new(),
            std::time::Instant::now(),
            None,
        )
        .expect("the override must allow this candidate");

    let rendered = choice.explanation().render();
    assert!(
        rendered.contains(OVERRIDDEN),
        "the explanation must name the session the override was granted for: {rendered}"
    );
    assert!(
        rendered.contains("overrode reserve protection"),
        "the explanation must say what happened: {rendered}"
    );
}

// --- 3. The branch this package made reachable for the first time ----------

/// Ruling 1: giving `user_override` a producer makes
/// `evaluate_reserve_spend`'s first-and-only-previously-dead override branch
/// reachable, so it gets a test of its own.
///
/// The inputs are the *most* denied combination the policy has — Reserve
/// band, deterministic tier, a cheaper adequate resource available, and a
/// reset further away than the distant threshold. Every one of those is a
/// reason to deny, and the override outranks all of them, which is what
/// "override" means.
#[test]
fn the_user_override_branch_outranks_every_automatic_denial() {
    let denied = ReserveDecisionInputs {
        band: CapacityBand::Reserve,
        tier: WorkloadTier::Deterministic,
        cheaper_adequate_resource_exists: true,
        user_override: false,
        seconds_until_reset: Some(RESET_DISTANT_SECONDS + 1),
        task_nearly_complete: false,
    };
    assert!(
        !evaluate_reserve_spend(denied).is_allowed(),
        "the control case must be denied, or the test below proves nothing"
    );

    let overridden = ReserveDecisionInputs {
        user_override: true,
        ..denied
    };
    let decision = evaluate_reserve_spend(overridden);
    assert!(decision.is_allowed(), "{}", decision.reason());
    assert!(
        decision.reason().contains("1290"),
        "the reason must cite the line it answers, because it is what a user reads: {}",
        decision.reason()
    );
}

/// The override does not outrank line 1294's guard, which is the precedence
/// `evaluate_reserve_spend`'s own doc comment states. Both allow, but the
/// reason names 1294 — a user who overrode reserve for a session has not
/// thereby made every allowance in it their doing.
#[test]
fn the_almost_complete_guard_still_answers_before_the_override() {
    let decision = evaluate_reserve_spend(ReserveDecisionInputs {
        band: CapacityBand::Reserve,
        tier: WorkloadTier::Leaf,
        cheaper_adequate_resource_exists: true,
        user_override: true,
        seconds_until_reset: Some(RESET_DISTANT_SECONDS + 1),
        task_nearly_complete: true,
    });
    assert!(decision.is_allowed());
    assert!(
        decision.reason().contains("1294"),
        "line 1294 answers first: {}",
        decision.reason()
    );
}

// --- 4. Line 1289, and the input that is deliberately absent ---------------

/// Line 1289's own branch, at the boundary Phase 34A set. Not a re-proof of
/// that package's thresholds — those are `tests/workload_tiers.rs`'s, and
/// this packet may not change them — but the statement this line is closed
/// on: a task at or above the heavy tier spends the reserve, a task below it
/// does not, all else identical.
#[test]
fn the_tier_is_what_decides_whether_a_capability_requirement_justifies_the_reserve() {
    let base = ReserveDecisionInputs {
        band: CapacityBand::Reserve,
        tier: WorkloadTier::Standard,
        cheaper_adequate_resource_exists: true,
        user_override: false,
        seconds_until_reset: None,
        task_nearly_complete: false,
    };
    assert!(!evaluate_reserve_spend(base).is_allowed());

    for tier in [WorkloadTier::Heavy, WorkloadTier::Frontier] {
        let decision = evaluate_reserve_spend(ReserveDecisionInputs { tier, ..base });
        assert!(
            decision.is_allowed(),
            "{tier} must justify spending the reserve: {}",
            decision.reason()
        );
        assert!(
            decision.reason().contains("1289"),
            "the reason must cite line 1289: {}",
            decision.reason()
        );
    }
}

/// Why `TaskClassification::hard_capabilities` is not a second reserve input
/// beside the tier, as a test rather than only as a doc comment — practice
/// §79's rule that a refused wiring is written where the wiring would be
/// attempted.
///
/// A `HardCapability` names something a *harness* must be wired for, so it is
/// by construction not satisfiable by spending a stronger model's quota.
/// `cat the README` is the worked example: it carries a real hard capability
/// requirement — repository access — and is still leaf-tier work. Feeding its
/// capability set into the reserve decision would spend protected premium
/// reserve on it, which inverts what the reserve protects.
#[test]
fn the_hard_capability_set_is_not_a_reserve_input() {
    let hard_but_trivial = classify_heuristically("cat the README");
    assert!(
        hard_but_trivial
            .hard_capabilities()
            .contains(&HardCapability::RepositoryAccess),
        "this request is the worked example only if it really does carry a hard capability          requirement"
    );
    assert!(
        hard_but_trivial.conservative_workload_tier() < WorkloadTier::Heavy,
        "…and only if the tier still reads it as work a stronger model is not needed for: {}",
        hard_but_trivial.conservative_workload_tier()
    );

    let decision = evaluate_reserve_spend(ReserveDecisionInputs {
        band: CapacityBand::Reserve,
        tier: hard_but_trivial.conservative_workload_tier(),
        cheaper_adequate_resource_exists: true,
        user_override: false,
        seconds_until_reset: None,
        task_nearly_complete: false,
    });
    assert!(
        !decision.is_allowed(),
        "a hard capability requirement must not buy protected reserve — it names something no \
         model choice can supply: {}",
        decision.reason()
    );
}

// --- 5. Line 1294, refused, with the evidence ------------------------------

/// Acceptance test 4, for the line this package refuses.
///
/// `task_nearly_complete` is honoured by the consumer — the test above proves
/// that branch works — and is set by nothing. This scans the shipped crate's
/// own source for a producer and finds only the literal `false`, which is the
/// refusal stated where a reader can check it rather than in a report they
/// cannot.
///
/// Practice §14: the sources are normalised for line endings first, because a
/// Windows checkout reads this file with CRLF and a scan that assumed `\n`
/// would fail there for a reason that has nothing to do with the claim.
#[test]
fn nothing_in_this_build_produces_task_nearly_complete() {
    let sources = [
        (
            "routing/disposable.rs",
            include_str!("../src/routing/disposable.rs"),
        ),
        (
            "provider/quota.rs",
            include_str!("../src/provider/quota.rs"),
        ),
        ("main.rs", include_str!("../src/main.rs")),
    ];

    let mut assignments = Vec::new();
    for (name, source) in sources {
        for (number, line) in source.replace("\r\n", "\n").lines().enumerate() {
            let line = line.trim();
            if line.starts_with("//") {
                continue;
            }
            if line.starts_with("task_nearly_complete:") {
                assignments.push((name, number + 1, line.to_owned()));
            }
        }
    }

    assert!(
        !assignments.is_empty(),
        "the scan found no construction site at all, so it is checking nothing"
    );
    for (name, number, line) in &assignments {
        assert_eq!(
            line, "task_nearly_complete: false,",
            "{name}:{number} produces a value for line 1294's input. That is only correct if \
             something in this build can genuinely observe that a task is almost complete; a \
             turn count or an elapsed-time threshold cannot, and this field is the first branch \
             the policy takes."
        );
    }
}

/// The other half of the refusal: the signal does not arrive because
/// Glasshouse's event vocabulary has no way to express it. Every lifecycle
/// event is binary and retrospective, and the two that come closest carry doc
/// comments saying in as many words that they are not statements about the
/// work.
///
/// If a future batch adds a progress-bearing event, this test fails and the
/// refusal is due a re-reading — which is the point of asserting it.
#[test]
fn the_event_vocabulary_cannot_express_almost_complete() {
    let events = include_str!("../src/events/mod.rs").replace("\r\n", "\n");
    let vocabulary = events
        .split("pub enum LifecycleEvent {")
        .nth(1)
        .expect("LifecycleEvent must exist")
        .split("\n}")
        .next()
        .expect("LifecycleEvent must have a closing brace")
        .to_owned();

    assert!(
        vocabulary.contains("TurnEnded"),
        "the slice is not the enum body; the scan below would check nothing"
    );
    for absent in ["progress", "Progress", "percent", "remaining", "complete"] {
        assert!(
            !vocabulary
                .lines()
                .filter(|line| !line.trim_start().starts_with("///"))
                .any(|line| line.contains(absent)),
            "`{absent}` appears in the lifecycle vocabulary; capability map line 1294's refusal \
             was recorded against a vocabulary that could not express task progress"
        );
    }
}

// --- 6. The configuration that carries the override -----------------------

/// The setting is a list of sessions, layered project over user over the
/// empty default like every other list on `EffectiveConfig`.
#[test]
fn the_reserve_override_setting_layers_project_over_user() {
    let mut user = UserConfig::default();
    user.routing_mut()
        .set_reserve_override_sessions(Some(vec![OVERRIDDEN.to_owned()]));

    let effective = EffectiveConfig::new(&user, None);
    assert_eq!(effective.reserve_override_sessions().value, [OVERRIDDEN]);

    let mut project = ProjectConfig::default();
    project
        .routing_mut()
        .set_reserve_override_sessions(Some(vec![NOT_OVERRIDDEN.to_owned()]));
    let effective = EffectiveConfig::new(&user, Some(&project));
    assert_eq!(
        effective.reserve_override_sessions().value,
        [NOT_OVERRIDDEN],
        "the project layer wins outright rather than being unioned with the user's"
    );

    let untouched = UserConfig::default();
    let effective = EffectiveConfig::new(&untouched, None);
    assert!(
        effective.reserve_override_sessions().value.is_empty(),
        "a user who has never run `glasshouse sessions reserve` overrides nothing"
    );
}

/// The setting survives a round trip through the file format, and a
/// configuration that never mentioned it still serialises without a
/// `[routing]` table — the `is_unset` contract every other field on that
/// table keeps.
#[test]
fn the_reserve_override_setting_round_trips_through_the_config_file() {
    let dir = tempfile::tempdir().unwrap();
    let cli = <glasshouse::Cli as clap::Parser>::try_parse_from([
        "glasshouse",
        "--data-dir",
        dir.path().to_str().unwrap(),
        "--config-dir",
        dir.path().to_str().unwrap(),
    ])
    .unwrap();
    let workspace = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(workspace.path().join(".git")).unwrap();
    let runtime = glasshouse::bootstrap(&cli, workspace.path()).unwrap();

    let untouched = UserConfig::default();
    untouched.save(runtime.paths()).unwrap();
    let written = std::fs::read_to_string(runtime.paths().user_config_file()).unwrap();
    assert!(
        !written.contains("reserve_override_sessions"),
        "an untouched configuration must not grow the setting: {written}"
    );

    let mut user = UserConfig::load(runtime.paths()).unwrap();
    user.routing_mut()
        .set_reserve_override_sessions(Some(vec![OVERRIDDEN.to_owned()]));
    user.save(runtime.paths()).unwrap();

    let reloaded = UserConfig::load(runtime.paths()).unwrap();
    assert_eq!(
        reloaded.routing().reserve_override_sessions(),
        Some([OVERRIDDEN.to_owned()].as_slice())
    );
}
