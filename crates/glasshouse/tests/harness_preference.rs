//! Map line 1952 — entered through [`SessionRouter::choose`], the same
//! surface `route_recommendation` and `launch_session` call in the shipped
//! binary (both go through `main.rs::session_router`, which is where
//! `HarnessEfficiencySummary::from_outcomes` is wired in, over the exact
//! producer `harness_efficiency_section` prints — map line 1951).
//!
//! `session.rs`'s own `harness_efficiency_tests` module (`#[cfg(test)]`)
//! proves the term itself and its gates directly. This file proves the
//! router-level behavior the packet's REQUIRED BEHAVIOR section states: a
//! preference between two otherwise-identical fresh destinations on
//! different harnesses, preservation below the sample gate and with an
//! empty summary, and that a candidate set already scoped to one harness is
//! never re-ranked across harnesses by this term.

use std::time::Instant;

use glasshouse::evaluation::HarnessTierOutcome;
use glasshouse::harness::pairing::PairingOverrides;
use glasshouse::integrations::IntegrationId;
use glasshouse::routing::classify::{
    ClassificationSource, Complexity, Confidence, TaskClassification, WarmContextValue,
    WorkloadTier,
};
use glasshouse::routing::free::FreePool;
use glasshouse::routing::request::{AnswerProvenance, RouterAnswer};
use glasshouse::routing::session::{
    Destination, HarnessEfficiencySummary, RouterInputs, RoutingMoment, SessionRouter,
    TaskRequirements,
};
use glasshouse::routing::{AssignedModel, Backend, Cost, CredentialId, ToolSemantics};
use glasshouse::secret::SecretRef;

fn destination(id: &str, harness: IntegrationId, protocol: &str) -> Destination {
    Destination::fresh(
        id,
        harness,
        "profile",
        Backend::new(
            "provider",
            protocol,
            AssignedModel::named("an-unverified-model"),
            CredentialId::new(
                "provider",
                SecretRef::Environment {
                    var: format!("{id}_KEY"),
                },
            ),
            Cost::Free,
            ToolSemantics::Verified,
        ),
        None,
    )
}

/// A stated task classified `Standard` at `Confidence::High` — not
/// conservative, so the bucket this summary is keyed by is the
/// un-escalated `"standard"`.
fn standard_task() -> TaskRequirements {
    let classification = TaskClassification::new(
        false,
        true,
        false,
        false,
        Complexity::Moderate,
        false,
        WorkloadTier::Standard,
        false,
        WarmContextValue::PreferStrongerCold,
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

fn measured(
    harness: &str,
    task_class: &str,
    successful: i64,
    sample_size: i64,
) -> HarnessTierOutcome {
    HarnessTierOutcome {
        harness: harness.to_owned(),
        outcome: glasshouse::evaluation::TierOutcome {
            bucket: task_class.to_owned(),
            undecided: 0,
            verdict: glasshouse::evaluation::TierOutcomeVerdict::Measured {
                successful,
                failed: sample_size - successful,
                sample_size,
            },
        },
    }
}

fn inputs<'a>(
    overrides: &'a PairingOverrides,
    health: &'a FreePool,
    requirements: TaskRequirements,
) -> RouterInputs<'a> {
    RouterInputs {
        overrides,
        health,
        now: Instant::now(),
        requirements,
    }
}

/// REQUIRED BEHAVIOR 1: two otherwise-identical fresh destinations on
/// different harnesses, with the summary saying one harness succeeded more
/// often on this task's class above the sample gate — that harness is
/// chosen and its explanation names harness, class, count and rate.
///
/// Each destination speaks its own harness's native protocol (so
/// `harness capability fit` ties at the same magnitude for both) and
/// carries no tier ceiling, no capacity reading, and a free cost (so
/// `workload tier fit`, `cost preference` and every capacity-pressure term
/// also tie) — the only thing that differs between them is which harness
/// they are on, which is exactly what "otherwise identical" requires this
/// test to hold.
#[test]
fn the_harness_with_the_better_recorded_rate_is_chosen() {
    let summary = HarnessEfficiencySummary::from_outcomes(&[
        measured("claude-code", "standard", 9, 10),
        measured("codex", "standard", 2, 10),
    ]);
    let overrides = PairingOverrides::default();
    let health = FreePool::new();
    let router = SessionRouter::new().with_harness_efficiency(summary);

    let claude = destination(
        "claude-code-fresh",
        IntegrationId::ClaudeCode,
        "anthropic-messages",
    );
    let codex = destination("codex-fresh", IntegrationId::Codex, "openai-responses");

    let routed = router
        .choose(
            RoutingMoment::SessionStart,
            None,
            &[claude, codex],
            &inputs(&overrides, &health, standard_task()),
        )
        .expect("two fresh destinations are always answerable");

    assert_eq!(
        routed.chosen().id(),
        "claude-code-fresh",
        "`claude-code` recorded 9 of 10 successes on `standard` tasks against codex's 2 of 10, \
         and should have been preferred"
    );

    let term = routed
        .explanation()
        .contributions()
        .iter()
        .find(|c| c.name() == "harness efficiency")
        .expect("the winning explanation must carry the harness-efficiency term");
    assert!(term.magnitude() > 0.0, "{term:?}");
    assert!(
        term.magnitude() < 1.5,
        "must stay below warm affinity: {term:?}"
    );
    for needle in ["claude-code", "standard", "9", "10"] {
        assert!(
            term.evidence().contains(needle),
            "explanation must name the harness, the class, the sample count and the rate \
             (missing `{needle}`): {}",
            term.evidence()
        );
    }
}

/// REQUIRED BEHAVIOR 2: below the sample gate, the term is `0.0` with the
/// inert text, and the ranking is byte-identical to a build carrying no
/// summary at all — pinned by comparing every score total against a router
/// built with `HarnessEfficiencySummary::empty()`.
#[test]
fn below_the_sample_gate_the_ranking_is_unaffected() {
    let overrides = PairingOverrides::default();
    let health = FreePool::new();
    let destinations = || {
        [
            destination(
                "claude-code-fresh",
                IntegrationId::ClaudeCode,
                "anthropic-messages",
            ),
            destination("codex-fresh", IntegrationId::Codex, "openai-responses"),
        ]
    };

    let thin_summary = HarnessEfficiencySummary::from_outcomes(&[
        measured("claude-code", "standard", 2, 3),
        measured("codex", "standard", 1, 3),
    ]);
    let with_thin_evidence = SessionRouter::new()
        .with_harness_efficiency(thin_summary)
        .choose(
            RoutingMoment::SessionStart,
            None,
            &destinations(),
            &inputs(&overrides, &health, standard_task()),
        )
        .expect("destinations were offered");

    let with_no_ledger = SessionRouter::new()
        .choose(
            RoutingMoment::SessionStart,
            None,
            &destinations(),
            &inputs(&overrides, &health, standard_task()),
        )
        .expect("destinations were offered");

    assert_eq!(
        with_thin_evidence.chosen().id(),
        with_no_ledger.chosen().id()
    );
    let thin_totals: Vec<f64> = with_thin_evidence
        .considered()
        .iter()
        .map(|(_, explanation)| explanation.total())
        .collect();
    let empty_totals: Vec<f64> = with_no_ledger
        .considered()
        .iter()
        .map(|(_, explanation)| explanation.total())
        .collect();
    assert_eq!(
        thin_totals, empty_totals,
        "fewer than MIN_SAMPLE_FOR_SUMMARY observations must score exactly like no summary at all"
    );

    let term = with_thin_evidence
        .explanation()
        .contributions()
        .iter()
        .find(|c| c.name() == "harness efficiency")
        .expect("the term is present, and inert");
    assert_eq!(term.magnitude(), 0.0);
    assert!(term.evidence().contains("inert"), "{}", term.evidence());
}

/// REQUIRED BEHAVIOR 3: a candidate set already scoped to a user-assigned
/// harness — `launch_session`'s own shape when the user named one — is
/// unaffected: only one harness is offered, so there is no "other" for this
/// term to prefer, whatever a different harness recorded elsewhere.
#[test]
fn a_candidate_set_scoped_to_one_harness_is_never_re_ranked_across_harnesses() {
    let summary = HarnessEfficiencySummary::from_outcomes(&[
        measured("claude-code", "standard", 9, 10),
        measured("codex", "standard", 1, 10),
    ]);
    let overrides = PairingOverrides::default();
    let health = FreePool::new();
    let router = SessionRouter::new().with_harness_efficiency(summary);

    // Only `claude-code` is offered, exactly as `launch_session` builds the
    // set for an assigned harness.
    let only = destination(
        "claude-code-fresh",
        IntegrationId::ClaudeCode,
        "anthropic-messages",
    );

    let routed = router
        .choose(
            RoutingMoment::SessionStart,
            None,
            &[only],
            &inputs(&overrides, &health, standard_task()),
        )
        .expect("one destination is always answerable");

    assert_eq!(routed.chosen().id(), "claude-code-fresh");
    let term = routed
        .explanation()
        .contributions()
        .iter()
        .find(|c| c.name() == "harness efficiency")
        .expect("the term is present, and inert");
    assert_eq!(
        term.magnitude(),
        0.0,
        "one harness offered is nothing to prefer among, whatever the ledger says about others"
    );
}
