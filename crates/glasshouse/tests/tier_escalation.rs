//! Phase 35C lines 1559–1566 — capacity-aware tier escalation and downgrade,
//! entered the way production enters it.
//!
//! Two halves, for the reason `tests/subscription_pressure.rs` gives. The
//! first goes through [`SessionRouter::choose`] with hand-built destinations
//! and, for every rule, holds a candidate set on which the *existing* terms
//! would have chosen differently — so a rule that changed no winner would
//! fail its own test rather than pass as decoration (practice §35, §79). Each
//! rule also has a control: the same set with the trigger removed, on which
//! nothing moves and the explanation says why.
//!
//! The second half runs the shipped binary, because nothing in the first
//! half can fail on a build where `main.rs` stops announcing or recording a
//! movement — and those two calls are the whole of lines 1565 and 1566.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::Instant;

use clap::Parser;
use glasshouse::config::pairing::{WarmSession, WarmSessionState};
use glasshouse::harness::pairing::PairingOverrides;
use glasshouse::integrations::IntegrationId;
use glasshouse::provider::quota::CapacityBand;
use glasshouse::routing::classify::{
    ClassificationSource, Complexity, Confidence, DurationClass, TaskClassification,
    WarmContextValue, WorkloadTier, classify_heuristically,
};
use glasshouse::routing::evidence::{
    EvidenceLedger, FailureClass, NewObservation, ObservationQuery, Outcome,
};
use glasshouse::routing::free::{FreePool, FreeResource, WorkloadOutcome};
use glasshouse::routing::pressure::CapacityFacts;
use glasshouse::routing::request::{AnswerProvenance, HeuristicReason, RouterAnswer};
use glasshouse::routing::session::{
    Destination, EscalationTrigger, HoldReason, Routed, RouterInputs, RoutingMoment, SessionRouter,
    TaskRequirements, TierMovement,
};
use glasshouse::routing::{AssignedModel, Backend, Cost, CredentialId, ToolSemantics};
use glasshouse::secret::SecretRef;
use glasshouse::{Cli, Runtime};

// ===========================================================================
// Half one — through `SessionRouter::choose`.
// ===========================================================================

const PROTOCOL: &str = "anthropic-messages";
const HARNESS: IntegrationId = IntegrationId::ClaudeCode;

/// A task `classify_heuristically` reads as **standard**-tier code
/// modification at medium confidence — the same text `tests/tier_ceiling.rs`
/// uses, written out so a classifier change fails these tests rather than
/// rescaling with them.
const STANDARD_REPO_TASK: &str = "refactor the launch profile handling in this project";

/// A task the same heuristics read as **heavy** (`run ` is a shell keyword)
/// at medium confidence, from heuristics and no model — line 1560's trigger.
const HEAVY_SHELL_TASK: &str = "run the tests in this project";

fn backend(id: &str, cost: Cost) -> Backend {
    let provider = format!("{id}-provider");
    Backend::new(
        provider.clone(),
        PROTOCOL,
        AssignedModel::named("the-same-model"),
        CredentialId::new(
            provider.clone(),
            SecretRef::Environment {
                var: format!("{}_KEY", provider.to_uppercase().replace('-', "_")),
            },
        ),
        cost,
        ToolSemantics::Verified,
    )
}

fn fresh(id: &str, ceiling: Option<WorkloadTier>, cost: Cost) -> Destination {
    Destination::fresh(id, HARNESS, "profile", backend(id, cost), None).with_tier_ceiling(ceiling)
}

fn existing(id: &str, ceiling: Option<WorkloadTier>, cost: Cost, warm: WarmSession) -> Destination {
    Destination::existing(id, HARNESS, "profile", backend(id, cost), warm)
        .with_tier_ceiling(ceiling)
}

fn live() -> WarmSession {
    WarmSession {
        state: WarmSessionState::Live,
        idle_seconds: 0,
    }
}

fn resource_of(destination: &Destination) -> FreeResource {
    FreeResource::new(
        destination.backend().credential().clone(),
        destination.backend().model().label(),
    )
}

fn tight() -> CapacityFacts {
    CapacityFacts::new(Some(CapacityBand::Tight), None)
}

/// Deterministic heuristics' own answer for `text`, exactly as `main.rs`'s
/// `heuristic_answer` builds it.
fn heuristic(text: &str) -> TaskRequirements {
    RouterAnswer::new(
        classify_heuristically(text),
        AnswerProvenance::Heuristic(HeuristicReason::NoRoutingModel),
    )
    .requirements()
}

/// A routing **model**'s answer: a standard-tier pure question — routine
/// support work above the leaf tier, which heuristics never produce (they
/// rate every question leaf) and which is therefore line 1562's whole
/// reachable case. `duration` is what the model stated about how long it
/// runs; `None` derives single-turn from the fields.
fn model_rated_standard_question(duration: Option<DurationClass>) -> TaskRequirements {
    let classification = TaskClassification::new(
        false,
        false,
        false,
        false,
        Complexity::Trivial,
        false,
        WorkloadTier::Standard,
        false,
        WarmContextValue::PreferStrongerCold,
        Confidence::High,
        ClassificationSource::Model {
            label: "the-routing-model".to_owned(),
        },
    )
    .with_duration(duration);
    RouterAnswer::new(
        classification,
        AnswerProvenance::Model {
            label: "the-routing-model".to_owned(),
        },
    )
    .requirements()
}

/// The same heavy shell task a **model** confirmed — line 1560's control.
fn model_confirmed_heavy() -> TaskRequirements {
    let classification = TaskClassification::new(
        true,
        true,
        true,
        false,
        Complexity::Complex,
        true,
        WorkloadTier::Heavy,
        false,
        WarmContextValue::PreferWarm,
        Confidence::High,
        ClassificationSource::Model {
            label: "the-routing-model".to_owned(),
        },
    );
    RouterAnswer::new(
        classification,
        AnswerProvenance::Model {
            label: "the-routing-model".to_owned(),
        },
    )
    .requirements()
}

struct World {
    overrides: PairingOverrides,
    health: FreePool,
    now: Instant,
}

impl World {
    fn new() -> Self {
        Self {
            overrides: PairingOverrides::default(),
            health: FreePool::new(),
            now: Instant::now(),
        }
    }

    /// A candidate that can still serve but is doing badly — `struggling`'s
    /// own definition (`session.rs:3328`): `REPEATED_FAILURES` (2)
    /// consecutive failures that stated no wait, which crosses into an
    /// *invented* cooldown (line 534) but never a rejection. Ruling
    /// 2026-09-02 (`GH-CANDIDATE-GEN`, line 1518): a struggling candidate
    /// must still pass the hard-constraint gate — only a rejected credential
    /// or a provider-*declared* cooldown is hard-excluded, which is a
    /// different fact ("unavailable") than this one ("struggling").
    fn struggle(&mut self, destination: &Destination) {
        for _ in 0..2 {
            self.health.observe(
                &resource_of(destination),
                WorkloadOutcome::CapacityFailure,
                self.now,
            );
        }
    }

    fn choose(
        &self,
        router: SessionRouter,
        current: Option<&Destination>,
        destinations: &[Destination],
        requirements: TaskRequirements,
    ) -> Routed {
        let inputs = RouterInputs {
            overrides: &self.overrides,
            health: &self.health,
            now: self.now,
            requirements,
        };
        let moment = if current.is_some() {
            RoutingMoment::TaskBoundary
        } else {
            RoutingMoment::SessionStart
        };
        router
            .choose(moment, current, destinations, &inputs)
            .expect("destinations were offered")
    }
}

fn movement(routed: &Routed) -> &TierMovement {
    routed
        .movement()
        .expect("a tier was stated, so the router decided a movement")
}

/// The magnitude `id`'s explanation carries for `term`.
fn term(routed: &Routed, id: &str, term: &str) -> f64 {
    let (_, explanation) = routed
        .considered()
        .iter()
        .find(|(destination, _)| destination.id() == id)
        .unwrap_or_else(|| panic!("`{id}` was not scored:\n{}", routed.render_overview()));
    explanation
        .contributions()
        .iter()
        .find(|contribution| contribution.name() == term)
        .unwrap_or_else(|| panic!("`{id}` has no `{term}` term:\n{}", explanation.render()))
        .magnitude()
}

fn rejected(routed: &Routed, id: &str) -> bool {
    routed
        .rejected()
        .iter()
        .any(|(destination, _)| destination.id() == id)
}

// --- 1559 -------------------------------------------------------------------

/// **Line 1559.** When every candidate established at the classified tier is
/// struggling, the preference moves one tier up — and the winner changes.
///
/// The frontier-ceiling candidate is offered *first*: without the movement
/// it and the heavy one tie on headroom (`+0.200` each) and caller order
/// picks it. With the movement the heavy one is the exact fit (`+0.400`) and
/// wins although it is offered second. The struggling candidate stays
/// scored, not rejected — an escalation is a preference, never a gate.
#[test]
fn every_struggling_candidate_at_the_classified_tier_escalates_the_preference_one_step() {
    for (label, struggling) in [
        (
            "repeatedly failing without rejection",
            fresh("c-standard", Some(WorkloadTier::Standard), Cost::Metered),
        ),
        (
            "in the exhausted band",
            fresh("c-standard", Some(WorkloadTier::Standard), Cost::Metered)
                .with_capacity_facts(CapacityFacts::new(Some(CapacityBand::Exhausted), None)),
        ),
    ] {
        let mut world = World::new();
        // Ruling 2026-09-02 (GH-CANDIDATE-GEN, line 1518): a rejected
        // credential is UNAVAILABLE and hard-excluded, not struggling —
        // this fixture must still pass the gate.
        if label.starts_with("repeatedly") {
            world.struggle(&struggling);
        }
        let destinations = [
            fresh("a-frontier", Some(WorkloadTier::Frontier), Cost::Metered),
            fresh("b-heavy", Some(WorkloadTier::Heavy), Cost::Metered),
            struggling,
        ];
        let routed = world.choose(
            SessionRouter::new(),
            None,
            &destinations,
            heuristic(STANDARD_REPO_TASK),
        );

        assert_eq!(
            movement(&routed),
            &TierMovement::Escalated {
                from: WorkloadTier::Standard,
                to: WorkloadTier::Heavy,
                trigger: EscalationTrigger::TierStruggling,
                capped: Vec::new(),
            },
            "the only standard-tier candidate is {label}:\n{}",
            routed.render_overview()
        );
        assert_eq!(
            routed.chosen().id(),
            "b-heavy",
            "{label}:\n{}",
            routed.render_overview()
        );
        assert_eq!(
            term(&routed, "b-heavy", "workload tier fit"),
            0.4,
            "{label}"
        );
        assert_eq!(
            term(&routed, "a-frontier", "workload tier fit"),
            0.2,
            "{label}"
        );
        assert_eq!(
            term(&routed, "c-standard", "workload tier fit"),
            0.0,
            "kept eligible and not preferred ({label}):\n{}",
            routed.render_overview()
        );
        assert!(
            !rejected(&routed, "c-standard"),
            "an escalation must never reject the candidate it moved away from ({label})"
        );
        assert!(
            routed
                .render()
                .contains("tier         escalated from `standard` to `heavy`"),
            "the movement is a heading, not only a term ({label}):\n{}",
            routed.render()
        );
    }
}

/// **Line 1559's control.** The same set with the standard-tier candidate
/// healthy: nothing moves, it wins on its exact fit, and the held movement
/// says no trigger fired.
#[test]
fn a_healthy_candidate_at_the_classified_tier_holds_the_preference() {
    let world = World::new();
    let destinations = [
        fresh("a-frontier", Some(WorkloadTier::Frontier), Cost::Metered),
        fresh("b-heavy", Some(WorkloadTier::Heavy), Cost::Metered),
        fresh("c-standard", Some(WorkloadTier::Standard), Cost::Metered),
    ];
    let routed = world.choose(
        SessionRouter::new(),
        None,
        &destinations,
        heuristic(STANDARD_REPO_TASK),
    );
    assert_eq!(
        movement(&routed),
        &TierMovement::Held {
            tier: WorkloadTier::Standard,
            reason: HoldReason::NoTrigger { retry_after: None },
        }
    );
    assert_eq!(routed.chosen().id(), "c-standard");
    assert!(!routed.render().contains("tier         "));
}

/// **Line 1559, the guard.** Struggling candidates at the classified tier
/// and *no* healthy candidate above it: the preference stays rather than
/// pointing at nothing, and the explanation says so.
#[test]
fn an_escalation_with_no_healthy_target_is_held_and_says_so() {
    let mut world = World::new();
    let standard = fresh("a-standard", Some(WorkloadTier::Standard), Cost::Metered);
    let heavy = fresh("b-heavy", Some(WorkloadTier::Heavy), Cost::Metered);
    // Ruling 2026-09-02 (GH-CANDIDATE-GEN, line 1518): both must remain
    // eligible for the movement decision to see them as struggling with
    // nowhere better to go — a rejected credential would be hard-excluded
    // instead, which is a different fact than "no healthy target".
    world.struggle(&standard);
    world.struggle(&heavy);
    let routed = world.choose(
        SessionRouter::new(),
        None,
        &[standard, heavy],
        heuristic(STANDARD_REPO_TASK),
    );
    assert!(
        matches!(
            movement(&routed),
            TierMovement::Held {
                reason: HoldReason::NoTarget {
                    to: WorkloadTier::Heavy,
                    ..
                },
                ..
            }
        ),
        "{}",
        routed.render_overview()
    );
    assert!(movement(&routed).describe().contains("pointing at nothing"));
    assert_eq!(routed.rejected().len(), 0);
}

// --- 1560 -------------------------------------------------------------------

/// **Line 1560.** A heavy verdict from heuristics alone escalates one tier;
/// the identical verdict confirmed by a routing model does not.
#[test]
fn a_heuristic_heavy_verdict_escalates_and_a_model_confirmed_one_does_not() {
    let world = World::new();
    let destinations = [
        fresh("a-heavy", Some(WorkloadTier::Heavy), Cost::Metered),
        fresh("z-frontier", Some(WorkloadTier::Frontier), Cost::Metered),
    ];

    let routed = world.choose(
        SessionRouter::new(),
        None,
        &destinations,
        heuristic(HEAVY_SHELL_TASK),
    );
    assert_eq!(
        movement(&routed),
        &TierMovement::Escalated {
            from: WorkloadTier::Heavy,
            to: WorkloadTier::Frontier,
            trigger: EscalationTrigger::HeuristicHeavy,
            capped: Vec::new(),
        },
        "{}",
        routed.render_overview()
    );
    assert_eq!(
        routed.chosen().id(),
        "z-frontier",
        "offered second, so caller order did not choose it:\n{}",
        routed.render_overview()
    );

    let control = world.choose(
        SessionRouter::new(),
        None,
        &destinations,
        model_confirmed_heavy(),
    );
    assert!(
        matches!(movement(&control), TierMovement::Held { .. }),
        "a model's own heavy verdict is not the guess line 1560 escalates:\n{}",
        control.render_overview()
    );
    assert_eq!(control.chosen().id(), "a-heavy");
}

// --- 1564 -------------------------------------------------------------------

/// **Line 1564.** Told that the last exchange on the current destination
/// ended in a model-capability failure, the router promotes one tier; told
/// of a health or quota failure, it does not, and says which it saw.
#[test]
fn an_attributable_failure_promotes_one_tier_and_a_health_failure_does_not() {
    let world = World::new();
    let current = existing(
        "current-standard",
        Some(WorkloadTier::Standard),
        Cost::Metered,
        live(),
    );
    let destinations = [
        current.clone(),
        fresh("z-heavy", Some(WorkloadTier::Heavy), Cost::Metered),
    ];

    for class in [
        FailureClass::RequestIncompatibility,
        FailureClass::EmptyCompletion,
    ] {
        let routed = world.choose(
            SessionRouter::new().with_retry_after(Some(class)),
            Some(&current),
            &destinations,
            heuristic(STANDARD_REPO_TASK),
        );
        assert_eq!(
            movement(&routed),
            &TierMovement::Escalated {
                from: WorkloadTier::Standard,
                to: WorkloadTier::Heavy,
                trigger: EscalationTrigger::AttributableFailure(class),
                capped: Vec::new(),
            },
            "{class}:\n{}",
            routed.render_overview()
        );
        assert_eq!(
            term(&routed, "z-heavy", "workload tier fit"),
            0.4,
            "{class}"
        );
        assert_eq!(
            term(&routed, "current-standard", "workload tier fit"),
            0.0,
            "{class}"
        );
    }

    for class in [
        FailureClass::Throttle,
        FailureClass::Upstream5xx,
        FailureClass::CredentialFailure,
    ] {
        let routed = world.choose(
            SessionRouter::new().with_retry_after(Some(class)),
            Some(&current),
            &destinations,
            heuristic(STANDARD_REPO_TASK),
        );
        assert_eq!(
            movement(&routed),
            &TierMovement::Held {
                tier: WorkloadTier::Standard,
                reason: HoldReason::NoTrigger {
                    retry_after: Some(class),
                },
            },
            "{class}:\n{}",
            routed.render_overview()
        );
        assert!(
            movement(&routed)
                .describe()
                .contains("not promoted on (line 1564)"),
            "{class}: {}",
            movement(&routed).describe()
        );
    }
}

// --- 1565 -------------------------------------------------------------------

/// **Line 1565.** Two triggers at once — a promotion *and* a struggling tier
/// — move the preference exactly one tier, the second trigger is named as
/// capped, and the frontier candidate is not reached.
#[test]
fn two_triggers_move_one_tier_and_the_second_is_named_as_capped() {
    let mut world = World::new();
    let struggling = fresh("c-standard", Some(WorkloadTier::Standard), Cost::Metered);
    // Ruling 2026-09-02 (GH-CANDIDATE-GEN, line 1518): struggling, not
    // rejected — a rejected credential would be hard-excluded rather than
    // capped as a second trigger.
    world.struggle(&struggling);
    let destinations = [
        fresh("a-frontier", Some(WorkloadTier::Frontier), Cost::Metered),
        fresh("b-heavy", Some(WorkloadTier::Heavy), Cost::Metered),
        struggling,
    ];
    let routed = world.choose(
        SessionRouter::new().with_retry_after(Some(FailureClass::EmptyCompletion)),
        None,
        &destinations,
        heuristic(STANDARD_REPO_TASK),
    );
    assert_eq!(
        movement(&routed),
        &TierMovement::Escalated {
            from: WorkloadTier::Standard,
            to: WorkloadTier::Heavy,
            trigger: EscalationTrigger::AttributableFailure(FailureClass::EmptyCompletion),
            capped: vec![EscalationTrigger::TierStruggling],
        },
        "{}",
        routed.render_overview()
    );
    assert_eq!(
        routed.chosen().id(),
        "b-heavy",
        "one tier, not two — the frontier candidate is where a malformed task must not \
         land by itself:\n{}",
        routed.render_overview()
    );
    assert!(
        movement(&routed).describe().contains("were capped"),
        "{}",
        movement(&routed).describe()
    );
}

// --- 1562 / 1563 / 1561 -----------------------------------------------------

/// The two-destination world lines 1562, 1563 and 1561 are about: a metered
/// standard-ceiling resource in the tight band, offered first, and a free
/// leaf-ceiling resource the classified tier would refuse.
fn premium_tight_and_free_leaf() -> [Destination; 2] {
    [
        fresh("a-premium", Some(WorkloadTier::Standard), Cost::Metered)
            .with_capacity_facts(tight()),
        fresh("z-free", Some(WorkloadTier::Leaf), Cost::Free),
    ]
}

/// **Line 1562.** Routine standard-tier work, every metered candidate tight:
/// the tier is downgraded to leaf, the free leaf-ceiling resource — which the
/// classified tier's gate would have refused — becomes eligible, fits
/// exactly, and wins while `low-tier spend` prices the premium one out.
#[test]
fn routine_work_under_tight_premium_capacity_is_downgraded_to_a_free_resource() {
    let world = World::new();
    let routed = world.choose(
        SessionRouter::new(),
        None,
        &premium_tight_and_free_leaf(),
        model_rated_standard_question(None),
    );
    assert_eq!(
        movement(&routed),
        &TierMovement::Downgraded {
            from: WorkloadTier::Standard,
            to: WorkloadTier::Leaf,
            target: "z-free".to_owned(),
        },
        "{}",
        routed.render_overview()
    );
    assert!(!rejected(&routed, "z-free"), "{}", routed.render_overview());
    assert_eq!(term(&routed, "z-free", "workload tier fit"), 0.4);
    assert_eq!(term(&routed, "a-premium", "low-tier spend"), -3.0);
    assert_eq!(
        routed.chosen().id(),
        "z-free",
        "{}",
        routed.render_overview()
    );
    assert!(
        routed
            .render()
            .contains("tier         downgraded from `standard` to `leaf`")
    );
}

/// **Line 1562's two controls.** With the premium resource healthy, and with
/// non-routine work (code modification) under the same pressure, nothing is
/// downgraded and the free leaf resource stays refused by the classified
/// tier's gate.
#[test]
fn a_healthy_premium_resource_or_non_routine_work_is_not_downgraded() {
    let world = World::new();

    let mut healthy = premium_tight_and_free_leaf();
    healthy[0] = healthy[0]
        .clone()
        .with_capacity_facts(CapacityFacts::new(Some(CapacityBand::Healthy), None));
    let routed = world.choose(
        SessionRouter::new(),
        None,
        &healthy,
        model_rated_standard_question(None),
    );
    assert!(
        matches!(movement(&routed), TierMovement::Held { .. }),
        "{}",
        routed.render_overview()
    );
    assert!(rejected(&routed, "z-free"));
    assert_eq!(routed.chosen().id(), "a-premium");

    let routed = world.choose(
        SessionRouter::new(),
        None,
        &premium_tight_and_free_leaf(),
        heuristic(STANDARD_REPO_TASK),
    );
    assert!(
        matches!(movement(&routed), TierMovement::Held { .. }),
        "code modification is not routine support work:\n{}",
        routed.render_overview()
    );
    assert!(rejected(&routed, "z-free"));
}

/// **Line 1563.** The same routine work stated to run for many turns is not
/// downgraded: a failed attempt below its tier would be redone in full.
#[test]
fn a_multi_turn_task_is_not_downgraded_because_its_retry_costs_more_than_it_saves() {
    let world = World::new();
    let routed = world.choose(
        SessionRouter::new(),
        None,
        &premium_tight_and_free_leaf(),
        model_rated_standard_question(Some(DurationClass::LongRunning)),
    );
    assert_eq!(
        movement(&routed),
        &TierMovement::Held {
            tier: WorkloadTier::Standard,
            reason: HoldReason::RetryCost {
                duration: DurationClass::LongRunning,
            },
        },
        "{}",
        routed.render_overview()
    );
    assert!(rejected(&routed, "z-free"));
    assert_eq!(routed.chosen().id(), "a-premium");
    assert!(movement(&routed).describe().contains("(line 1563)"));
}

/// **Line 1561.** A live existing session at the classified tier holds the
/// downgrade on its own affinity term; the same session resumable and idle
/// for almost the whole relevance window has too little warmth left to, and
/// the downgrade proceeds.
#[test]
fn a_warm_higher_tier_session_holds_the_downgrade_until_its_warmth_decays() {
    let world = World::new();
    let [premium, free] = premium_tight_and_free_leaf();

    let warm = existing(
        "w-warm",
        Some(WorkloadTier::Standard),
        Cost::Metered,
        live(),
    )
    .with_capacity_facts(tight());
    let routed = world.choose(
        SessionRouter::new(),
        None,
        &[premium.clone(), warm, free.clone()],
        model_rated_standard_question(None),
    );
    assert!(
        matches!(
            movement(&routed),
            TierMovement::Held {
                reason: HoldReason::WarmContext { session, .. },
                ..
            } if session == "w-warm"
        ),
        "{}",
        routed.render_overview()
    );
    assert_eq!(routed.chosen().id(), "w-warm");
    assert!(rejected(&routed, "z-free"));

    let cold = existing(
        "w-warm",
        Some(WorkloadTier::Standard),
        Cost::Metered,
        WarmSession {
            state: WarmSessionState::Resumable,
            idle_seconds: 8 * 60 * 60 - 60,
        },
    )
    .with_capacity_facts(tight());
    let routed = world.choose(
        SessionRouter::new(),
        None,
        &[premium, cold, free],
        model_rated_standard_question(None),
    );
    assert!(
        matches!(movement(&routed), TierMovement::Downgraded { .. }),
        "a minute of warmth left is not a context worth a tier:\n{}",
        routed.render_overview()
    );
    assert_eq!(
        routed.chosen().id(),
        "z-free",
        "{}",
        routed.render_overview()
    );
}

// --- preservation -----------------------------------------------------------

/// A decision with no task states no tier, decides no movement, and renders
/// exactly what it rendered before any of this existed.
#[test]
fn no_task_means_no_movement_and_nothing_new_rendered() {
    let world = World::new();
    let routed = world.choose(
        SessionRouter::new(),
        None,
        &premium_tight_and_free_leaf(),
        TaskRequirements::default(),
    );
    assert!(routed.movement().is_none());
    let report = routed.render_overview();
    assert!(!report.contains("tier movement"), "{report}");
    assert!(!report.contains("tier         "), "{report}");
}

// ===========================================================================
// Half two — the shipped binary: lines 1565 and 1566.
// ===========================================================================

const CREDENTIAL_VAR: &str = "GLASSHOUSE_TIER_ESCALATION_TEST_KEY";

struct Fixture {
    _tmp: tempfile::TempDir,
    base: PathBuf,
    root: PathBuf,
}

impl Fixture {
    /// One claude-code fake harness; provider `alpha` with `mid` capped at
    /// heavy and `big` at frontier; a launchable profile for each.
    fn new() -> Self {
        let tmp = tempfile::tempdir().expect("tempdir");
        let base = tmp.path().to_path_buf();
        let root = base.join("workspace");
        std::fs::create_dir_all(root.join(".git")).expect("create project root");
        let root = std::fs::canonicalize(&root).expect("canonicalize project root");
        let bin_dir = base.join("bin");
        std::fs::create_dir_all(&bin_dir).expect("create bin dir");
        let exe = install_fake_harness(&bin_dir, "claude-code");
        let escaped = exe.display().to_string().replace('\\', "\\\\");
        let config = format!(
            "version = 1\n\n[integrations.claude-code]\nenabled = true\nexecutable = \"{escaped}\"\n\n\
             [providers.alpha]\ntemplate = \"openrouter\"\n\
             credential_env = [\"{CREDENTIAL_VAR}\"]\n\n\
             [providers.alpha.model_ceilings]\nmid = \"heavy\"\nbig = \"frontier\"\n\n\
             [profiles.a-mid]\nharness = \"claude-code\"\nmodel = \"mid\"\n\
             expected_protocol = \"anthropic-messages\"\n\
             [profiles.a-mid.backend]\nkind = \"direct-provider\"\nprovider = \"alpha\"\n\n\
             [profiles.z-big]\nharness = \"claude-code\"\nmodel = \"big\"\n\
             expected_protocol = \"anthropic-messages\"\n\
             [profiles.z-big.backend]\nkind = \"direct-provider\"\nprovider = \"alpha\"\n"
        );
        let config_dir = base.join("config");
        std::fs::create_dir_all(&config_dir).expect("create config dir");
        std::fs::write(config_dir.join("config.toml"), config).expect("write user config");
        Self {
            _tmp: tmp,
            base,
            root,
        }
    }

    fn glasshouse(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_glasshouse"))
            .arg("--scope")
            .arg(&self.root)
            .arg("--data-dir")
            .arg(self.base.join("data"))
            .arg("--config-dir")
            .arg(self.base.join("config"))
            .args(args)
            .env(CREDENTIAL_VAR, "planted-opaque-tier-escalation-value")
            .env("PATH", self.base.join("empty-path"))
            .output()
            .expect("the glasshouse binary must be runnable")
    }

    fn runtime(&self) -> Runtime {
        let cli = Cli::try_parse_from([
            "glasshouse",
            "--data-dir",
            self.base.join("data").to_str().unwrap(),
            "--config-dir",
            self.base.join("config").to_str().unwrap(),
        ])
        .unwrap();
        glasshouse::bootstrap(&cli, &self.root).unwrap()
    }

    /// How many rows the evidence ledger holds under `purpose`, read through
    /// the same ledger the binary wrote.
    fn rows_with_purpose(&self, purpose: &str) -> usize {
        let runtime = self.runtime();
        let ledger = EvidenceLedger::open(&runtime).expect("open the evidence ledger");
        ledger
            .recent(
                ObservationQuery {
                    provider: "glasshouse",
                    model: "session-router",
                    route: None,
                    harness: Some("claude-code"),
                },
                64,
            )
            .expect("read routing observations")
            .into_iter()
            .filter(|row| row.purpose.as_deref() == Some(purpose))
            .count()
    }
}

#[cfg(unix)]
fn install_fake_harness(bin_dir: &Path, harness: &str) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;

    let path = bin_dir.join(format!("fake-{harness}"));
    std::fs::write(&path, "#!/bin/sh\nexit 0\n").expect("write fake harness");
    let mut perms = std::fs::metadata(&path).unwrap().permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&path, perms).unwrap();
    path
}

#[cfg(windows)]
fn install_fake_harness(bin_dir: &Path, harness: &str) -> PathBuf {
    let path = bin_dir.join(format!("fake-{harness}.cmd"));
    std::fs::write(&path, "@echo off\r\nexit /b 0\r\n").expect("write fake harness");
    path
}

fn both_streams(output: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

/// **Lines 1565 and 1566 on the shipped binary.** `route` renders the
/// escalation as a heading; `launch` announces it on stderr before the
/// destination and leaves exactly one `tier-escalation` row, while a launch
/// whose movement is held leaves none.
#[test]
fn the_shipped_binary_announces_and_records_an_escalation() {
    let fixture = Fixture::new();

    let route = fixture.glasshouse(&["route", "--task", HEAVY_SHELL_TASK]);
    let report = both_streams(&route);
    assert!(route.status.success(), "{report}");
    assert!(
        report.contains("tier         escalated from `heavy` to `frontier`"),
        "{report}"
    );
    assert!(
        report.contains("destination  fresh:claude-code:z-big"),
        "the frontier-ceiling profile must win the escalated preference:\n{report}"
    );
    assert_eq!(
        fixture.rows_with_purpose("tier-escalation"),
        0,
        "`route` reports and records nothing"
    );

    // A launch under the heavy-capped profile alone has nowhere to escalate
    // to — the launch path offers one fresh destination and this project has
    // no session yet — so the movement is held: not announced, not recorded.
    // This runs first because the launch below leaves a resumable session on
    // the frontier-capped model, which would then be a healthy target.
    let held = fixture.glasshouse(&[
        "launch",
        "claude-code",
        "--headless",
        "--profile",
        "a-mid",
        "--task",
        HEAVY_SHELL_TASK,
    ]);
    let said = both_streams(&held);
    assert!(held.status.success(), "{said}");
    assert!(!said.contains("glasshouse: tier escalated"), "{said}");
    assert_eq!(fixture.rows_with_purpose("tier-escalation"), 0, "{said}");

    let launched = fixture.glasshouse(&[
        "launch",
        "claude-code",
        "--headless",
        "--profile",
        "z-big",
        "--task",
        HEAVY_SHELL_TASK,
    ]);
    let said = both_streams(&launched);
    assert!(launched.status.success(), "{said}");
    assert!(
        said.contains("glasshouse: tier escalated from `heavy` to `frontier`"),
        "line 1565's visibility is the announcement:\n{said}"
    );
    assert_eq!(
        fixture.rows_with_purpose("tier-escalation"),
        1,
        "line 1566: one row per movement acted on:\n{said}"
    );
}

/// **Line 1564 on the shipped binary.** With a session on the heavy-capped
/// model and the ledger's most recent row for that backend an
/// `empty_completion`, a task-boundary `route` promotes standard-tier work
/// one tier and names the failure; with the row a `throttle`, it does not
/// and says why. The row is written through the same ledger the gateway
/// writes, keyed as the gateway keys it (provider name, model label).
#[test]
fn a_task_boundary_route_promotes_after_the_ledgers_last_attributable_failure() {
    let fixture = Fixture::new();
    let launched =
        fixture.glasshouse(&["launch", "claude-code", "--headless", "--profile", "a-mid"]);
    assert!(launched.status.success(), "{}", both_streams(&launched));

    let record = |class: FailureClass| {
        let runtime = fixture.runtime();
        let ledger = EvidenceLedger::open(&runtime).expect("open the evidence ledger");
        let now = glasshouse::provider::cache::now_unix_seconds();
        ledger
            .record(
                NewObservation::new("alpha", "mid")
                    .with_harness(Some("claude-code"))
                    .with_timing(Some(now), Some(now))
                    .with_outcome(Outcome::Failed)
                    .with_failure_class(Some(class)),
                now,
            )
            .expect("record the failed exchange");
    };

    record(FailureClass::Throttle);
    let route = fixture.glasshouse(&[
        "route",
        "--moment",
        "task-boundary",
        "--task",
        STANDARD_REPO_TASK,
    ]);
    let report = both_streams(&route);
    assert!(route.status.success(), "{report}");
    assert!(
        report.contains("`throttle` failure is a provider-health or quota fact"),
        "a throttle is seen and deliberately not promoted on:\n{report}"
    );
    assert!(!report.contains("tier         escalated"), "{report}");

    record(FailureClass::EmptyCompletion);
    let route = fixture.glasshouse(&[
        "route",
        "--moment",
        "task-boundary",
        "--task",
        STANDARD_REPO_TASK,
    ]);
    let report = both_streams(&route);
    assert!(route.status.success(), "{report}");
    assert!(
        report.contains("tier         escalated from `standard` to `heavy`")
            && report.contains("ended in `empty_completion`"),
        "the latest row promotes, and the report names it:\n{report}"
    );
}
