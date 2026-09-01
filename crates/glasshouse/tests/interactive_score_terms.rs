//! Map lines 1533, 1551, 1537 and 1538 — *"include existing session affinity
//! in candidate scoring,"* *"include session-switching and bootstrap cost in
//! candidate scoring,"* *"include provider health in candidate scoring"* and
//! *"include expected marginal cost in candidate scoring."*
//!
//! `docs/product/evidence/phase-35b.md:63-67` rules 1533/1551 structurally
//! inapplicable **to the disposable-job scorer** it was checking — a
//! disposable job has no session, so neither term has a subject there. That
//! ruling says nothing about the **interactive** router
//! (`routing::session::SessionRouter::choose`), which already carries both
//! terms in `score()` (`routing/session.rs:3083,3093`) and is proven for
//! other lines by `tests/session_router.rs`. This file is the test scoped to
//! this exact pair of lines, on the router phase-35b.md did not examine.
//!
//! Two mutations away, in `routing/session.rs::score`, are the two lines
//! this file's first two tests kill: dropping the `session_affinity` push at
//! `:3083`, and dropping the `switching_and_bootstrap_cost` push at `:3093`.
//!
//! **1537** is the same shape as 1533/1551: `provider_health` already lived
//! in `score()` (Phase 37 line 1599), proven only for that line, never for
//! 35B's 1537. No production change — this file's tests are the first to
//! kill the `provider_health` push specifically as 1537's evidence.
//!
//! **1538** is new production code in this package: `expected_marginal_cost`,
//! pushed unconditionally in `score()` and inert whenever `cost_preference`
//! (line 1558) is already active, so the two never price the same candidate.
//! See `expected_marginal_cost`'s own doc for why.

use std::collections::BTreeMap;
use std::time::Instant;

use glasshouse::config::pairing::{WarmSession, WarmSessionState};
use glasshouse::harness::pairing::PairingOverrides;
use glasshouse::integrations::IntegrationId;
use glasshouse::routing::free::{FreePool, FreeResource, WorkloadOutcome};
use glasshouse::routing::session::{
    CheckpointQuality, Destination, RouterInputs, RoutingMoment, SessionRouter, TaskRequirements,
};
use glasshouse::routing::{AssignedModel, Backend, Cost, CredentialId, ToolSemantics};
use glasshouse::secret::SecretRef;

const ANTHROPIC: &str = "anthropic-messages";

fn backend(provider: &str, model: &str, var: &str) -> Backend {
    Backend::new(
        provider,
        ANTHROPIC,
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

fn backend_with_cost(provider: &str, model: &str, var: &str, cost: Cost) -> Backend {
    Backend::new(
        provider,
        ANTHROPIC,
        AssignedModel::named(model),
        CredentialId::new(
            provider,
            SecretRef::Environment {
                var: var.to_owned(),
            },
        ),
        cost,
        ToolSemantics::Verified,
    )
}

fn live(idle_seconds: i64) -> WarmSession {
    WarmSession {
        state: WarmSessionState::Live,
        idle_seconds,
    }
}

fn no_overrides() -> PairingOverrides {
    PairingOverrides::from_parts("no configuration", BTreeMap::new(), BTreeMap::new())
}

struct Fixture {
    overrides: PairingOverrides,
    health: FreePool,
    now: Instant,
}

impl Fixture {
    fn new() -> Self {
        Self {
            overrides: no_overrides(),
            health: FreePool::new(),
            now: Instant::now(),
        }
    }

    fn inputs(&self) -> RouterInputs<'_> {
        RouterInputs {
            overrides: &self.overrides,
            health: &self.health,
            now: self.now,
            requirements: TaskRequirements::default(),
        }
    }
}

// ---------------------------------------------------------------------------
// Line 1533 — session affinity, on the interactive router.
// ---------------------------------------------------------------------------

/// **Line 1533.** Two existing sessions on the same backend, differing only
/// in how warm they are: the router prefers the warmer one, and its
/// explanation carries a `session affinity` contribution that is not zero —
/// so the term is not merely rendered, it is what decided the ranking.
#[test]
fn a_warm_candidates_explanation_carries_session_affinity_and_it_moves_the_ranking() {
    let fixture = Fixture::new();
    let serving = backend("anthropic", "claude-opus-4", "ANTHROPIC_API_KEY");
    let stale = Destination::existing(
        "stale",
        IntegrationId::ClaudeCode,
        "default",
        serving.clone(),
        live(7 * 60 * 60),
    );
    let warm = Destination::existing(
        "warm",
        IntegrationId::ClaudeCode,
        "default",
        serving,
        live(60),
    );

    let routed = SessionRouter::new()
        .choose(
            RoutingMoment::SessionStart,
            None,
            &[stale, warm],
            &fixture.inputs(),
        )
        .expect("destinations were offered");

    assert_eq!(
        routed.chosen().id(),
        "warm",
        "the warmer session did not win: session affinity is not deciding anything"
    );

    let affinities: Vec<f64> = routed
        .considered()
        .iter()
        .map(|(_, explanation)| {
            explanation
                .contributions()
                .iter()
                .find(|c| c.name() == "session affinity")
                .expect("every candidate must be scored for session affinity")
                .magnitude()
        })
        .collect();
    assert!(
        affinities[0] != affinities[1],
        "session affinity was constant across two sessions differing only in warmth \
         ({affinities:?}): the term is rendered but not doing anything"
    );
    assert!(
        affinities.iter().any(|magnitude| *magnitude != 0.0),
        "session affinity contributed nothing at all: {affinities:?}"
    );
}

// ---------------------------------------------------------------------------
// Line 1551 — session-switching and bootstrap cost, on the interactive
// router.
// ---------------------------------------------------------------------------

/// **Line 1551.** A candidate that would need a fresh session (bootstrap
/// cost) against one that continues the session already in hand (no
/// bootstrap cost): the router prefers the cheaper move, and its explanation
/// carries a `switching and bootstrap cost` contribution that differs
/// between the two and is not zero for the one that must bootstrap.
#[test]
fn a_candidate_needing_a_bootstrap_carries_switching_and_bootstrap_cost_and_it_moves_the_ranking() {
    let fixture = Fixture::new();
    let serving = backend("anthropic", "claude-opus-4", "ANTHROPIC_API_KEY");

    let current = Destination::existing(
        "current",
        IntegrationId::ClaudeCode,
        "default",
        serving.clone(),
        live(0),
    );
    // The same session continuing — no bootstrap, no switch.
    let stay = Destination::existing(
        "stay",
        IntegrationId::ClaudeCode,
        "default",
        serving.clone(),
        live(0),
    );
    // A fresh destination with no checkpoint to boot from: the most
    // expensive bootstrap this build prices.
    let bootstrap = Destination::fresh(
        "bootstrap",
        IntegrationId::ClaudeCode,
        "default",
        serving,
        None,
    );

    let routed = SessionRouter::new()
        .choose(
            RoutingMoment::TaskBoundary,
            Some(&current),
            &[bootstrap, stay],
            &fixture.inputs(),
        )
        .expect("destinations were offered");

    assert_eq!(
        routed.chosen().id(),
        "stay",
        "the candidate needing no bootstrap did not win: switching-and-bootstrap cost is not \
         deciding anything"
    );

    let costs: Vec<(&str, f64)> = routed
        .considered()
        .iter()
        .map(|(destination, explanation)| {
            let magnitude = explanation
                .contributions()
                .iter()
                .find(|c| c.name() == "switching and bootstrap cost")
                .expect("every candidate must be scored for switching and bootstrap cost")
                .magnitude();
            (destination.id(), magnitude)
        })
        .collect();

    let stay_cost = costs
        .iter()
        .find(|(id, _)| *id == "stay")
        .expect("`stay` was scored")
        .1;
    let bootstrap_cost = costs
        .iter()
        .find(|(id, _)| *id == "bootstrap")
        .expect("`bootstrap` was scored")
        .1;
    assert!(
        bootstrap_cost < stay_cost,
        "a fresh destination with no checkpoint did not score worse (a more negative \
         contribution) than continuing the current session ({costs:?}): the term is rendered \
         but not doing anything"
    );
    assert!(
        bootstrap_cost != 0.0,
        "the bootstrapping candidate's switching-and-bootstrap cost was zero: {costs:?}"
    );
}

/// The overview names both terms for whichever destination it explains —
/// the same non-vacuity `tests/session_router.rs`'s
/// `the_overview_explains_the_winner_the_alternatives_and_the_rejections`
/// checks, kept here so this file stands on its own for 1533/1551 without
/// depending on that file staying unchanged.
#[test]
fn the_interactive_routers_explanation_names_both_terms() {
    let fixture = Fixture::new();
    let serving = backend("anthropic", "claude-opus-4", "ANTHROPIC_API_KEY");
    let existing = Destination::existing(
        "existing",
        IntegrationId::ClaudeCode,
        "default",
        serving.clone(),
        live(0),
    );
    let fresh = Destination::fresh(
        "fresh",
        IntegrationId::ClaudeCode,
        "default",
        serving,
        Some(CheckpointQuality::new(true, true)),
    );

    let routed = SessionRouter::new()
        .choose(
            RoutingMoment::SessionStart,
            None,
            &[existing, fresh],
            &fixture.inputs(),
        )
        .expect("destinations were offered");

    let names: Vec<&str> = routed
        .explanation()
        .contributions()
        .iter()
        .map(|c| c.name())
        .collect();
    assert!(
        names.contains(&"session affinity"),
        "the winner's explanation does not name `session affinity`: {names:?}"
    );
    assert!(
        names.contains(&"switching and bootstrap cost"),
        "the winner's explanation does not name `switching and bootstrap cost`: {names:?}"
    );
}

// ---------------------------------------------------------------------------
// Line 1537 — provider health, on the interactive router.
// ---------------------------------------------------------------------------

/// **Line 1537.** Two otherwise-identical fresh destinations on different
/// providers, one with an observed capacity failure and one with none: the
/// router prefers the untouched provider, and its explanation carries a
/// `provider health` contribution that differs between the two and is
/// negative for the degraded one.
///
/// One observed failure only — `FAILURES_BEFORE_COOLDOWN` is 2 — so the
/// degraded candidate stays *available* and this isolates the health term's
/// own penalty rather than the separate cooldown-unavailable case
/// `provider_available` already guards elsewhere.
#[test]
fn a_degraded_providers_explanation_carries_provider_health_and_it_moves_the_ranking() {
    let overrides = no_overrides();
    let now = Instant::now();
    let degraded_credential = CredentialId::new(
        "degraded-provider",
        SecretRef::Environment {
            var: "DEGRADED_API_KEY".to_owned(),
        },
    );
    let mut health = FreePool::new();
    health.observe(
        &FreeResource::new(degraded_credential, "claude-opus-4"),
        WorkloadOutcome::CapacityFailure,
        now,
    );

    let healthy = Destination::fresh(
        "healthy",
        IntegrationId::ClaudeCode,
        "default",
        backend("healthy-provider", "claude-opus-4", "HEALTHY_API_KEY"),
        None,
    );
    let degraded = Destination::fresh(
        "degraded",
        IntegrationId::ClaudeCode,
        "default",
        backend("degraded-provider", "claude-opus-4", "DEGRADED_API_KEY"),
        None,
    );

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
            &[degraded, healthy],
            &inputs,
        )
        .expect("destinations were offered");

    assert_eq!(
        routed.chosen().id(),
        "healthy",
        "the undegraded provider did not win: provider health is not deciding anything"
    );

    let health_scores: Vec<(&str, f64)> = routed
        .considered()
        .iter()
        .map(|(destination, explanation)| {
            let magnitude = explanation
                .contributions()
                .iter()
                .find(|c| c.name() == "provider health")
                .expect("every candidate must be scored for provider health")
                .magnitude();
            (destination.id(), magnitude)
        })
        .collect();
    let healthy_score = health_scores
        .iter()
        .find(|(id, _)| *id == "healthy")
        .expect("`healthy` was scored")
        .1;
    let degraded_score = health_scores
        .iter()
        .find(|(id, _)| *id == "degraded")
        .expect("`degraded` was scored")
        .1;
    assert!(
        degraded_score < healthy_score,
        "the degraded provider did not score worse ({health_scores:?}): the term is rendered \
         but not doing anything"
    );
    assert!(
        degraded_score != 0.0,
        "the degraded provider's provider-health contribution was zero: {health_scores:?}"
    );
    assert_eq!(
        healthy_score, 0.0,
        "a provider with no observations should contribute nothing — absence of a claim, not a \
         claim of health: {health_scores:?}"
    );
}

// ---------------------------------------------------------------------------
// Line 1538 — expected marginal cost, on the interactive router.
// ---------------------------------------------------------------------------

/// **Line 1538.** Two otherwise-identical fresh destinations differing only
/// in `Cost`, at session start where no workload tier is established (so
/// `cost_preference`, line 1558, is not the term deciding this): the router
/// prefers the free one, and its explanation carries an `expected marginal
/// cost` contribution that differs between the two and is negative for the
/// metered one.
#[test]
fn a_metered_candidates_explanation_carries_expected_marginal_cost_and_it_moves_the_ranking() {
    let fixture = Fixture::new();
    let free_dest = Destination::fresh(
        "free",
        IntegrationId::ClaudeCode,
        "default",
        backend_with_cost(
            "anthropic",
            "claude-opus-4",
            "ANTHROPIC_API_KEY",
            Cost::Free,
        ),
        None,
    );
    let metered_dest = Destination::fresh(
        "metered",
        IntegrationId::ClaudeCode,
        "default",
        backend_with_cost(
            "anthropic",
            "claude-opus-4",
            "ANTHROPIC_API_KEY",
            Cost::Metered,
        ),
        None,
    );

    let routed = SessionRouter::new()
        .choose(
            RoutingMoment::SessionStart,
            None,
            &[metered_dest, free_dest],
            &fixture.inputs(),
        )
        .expect("destinations were offered");

    assert_eq!(
        routed.chosen().id(),
        "free",
        "the free destination did not win: expected marginal cost is not deciding anything"
    );

    let costs: Vec<(&str, f64)> = routed
        .considered()
        .iter()
        .map(|(destination, explanation)| {
            let magnitude = explanation
                .contributions()
                .iter()
                .find(|c| c.name() == "expected marginal cost")
                .expect("every candidate must be scored for expected marginal cost")
                .magnitude();
            (destination.id(), magnitude)
        })
        .collect();
    let free_cost = costs
        .iter()
        .find(|(id, _)| *id == "free")
        .expect("`free` was scored")
        .1;
    let metered_cost = costs
        .iter()
        .find(|(id, _)| *id == "metered")
        .expect("`metered` was scored")
        .1;
    assert!(
        metered_cost < free_cost,
        "the metered destination did not score worse ({costs:?}): the term is rendered but not \
         doing anything"
    );
    assert!(
        metered_cost != 0.0,
        "the metered candidate's expected-marginal-cost contribution was zero: {costs:?}"
    );
    assert_eq!(
        free_cost, 0.0,
        "a free candidate should contribute nothing — nothing is spent by preferring it: \
         {costs:?}"
    );
}

/// The overview names both new terms for whichever destination it explains —
/// same non-vacuity shape as `the_interactive_routers_explanation_names_both_terms`
/// above, kept separate so this file's 1537/1538 evidence does not depend on
/// that test staying unchanged.
#[test]
fn the_interactive_routers_explanation_names_provider_health_and_expected_marginal_cost() {
    let fixture = Fixture::new();
    let serving = backend("anthropic", "claude-opus-4", "ANTHROPIC_API_KEY");
    let existing = Destination::existing(
        "existing",
        IntegrationId::ClaudeCode,
        "default",
        serving.clone(),
        live(0),
    );
    let fresh = Destination::fresh(
        "fresh",
        IntegrationId::ClaudeCode,
        "default",
        serving,
        Some(CheckpointQuality::new(true, true)),
    );

    let routed = SessionRouter::new()
        .choose(
            RoutingMoment::SessionStart,
            None,
            &[existing, fresh],
            &fixture.inputs(),
        )
        .expect("destinations were offered");

    let names: Vec<&str> = routed
        .explanation()
        .contributions()
        .iter()
        .map(|c| c.name())
        .collect();
    assert!(
        names.contains(&"provider health"),
        "the winner's explanation does not name `provider health`: {names:?}"
    );
    assert!(
        names.contains(&"expected marginal cost"),
        "the winner's explanation does not name `expected marginal cost`: {names:?}"
    );
}

// ---------------------------------------------------------------------------
// Regression — every input this package can add absent.
// ---------------------------------------------------------------------------

/// **Required behavior.** With no observed health failures and no cost
/// distinction (both candidates identically metered), two otherwise-equal
/// fresh destinations still tie exactly as they did before this package: the
/// new unconditional `expected marginal cost` term does not manufacture a
/// winner where the ranking never had one, and `provider health` reads as
/// pure absence, not as a claim.
#[test]
fn with_nothing_new_observed_the_ranking_is_unchanged() {
    let fixture = Fixture::new();
    let serving = backend("anthropic", "claude-opus-4", "ANTHROPIC_API_KEY");
    let a = Destination::fresh(
        "a",
        IntegrationId::ClaudeCode,
        "default",
        serving.clone(),
        None,
    );
    let b = Destination::fresh("b", IntegrationId::ClaudeCode, "default", serving, None);

    let routed = SessionRouter::new()
        .choose(
            RoutingMoment::SessionStart,
            None,
            &[a, b],
            &fixture.inputs(),
        )
        .expect("destinations were offered");

    assert_eq!(
        routed.chosen().id(),
        "a",
        "two identical candidates should still resolve by caller order, exactly as before this \
         package"
    );

    for (_, explanation) in routed.considered() {
        let health = explanation
            .contributions()
            .iter()
            .find(|c| c.name() == "provider health")
            .expect("provider health is always scored")
            .magnitude();
        assert_eq!(
            health, 0.0,
            "no health observation exists for either candidate, so provider health must \
             contribute nothing: {health}"
        );
    }
    let costs: Vec<f64> = routed
        .considered()
        .iter()
        .map(|(_, explanation)| {
            explanation
                .contributions()
                .iter()
                .find(|c| c.name() == "expected marginal cost")
                .expect("expected marginal cost is always scored")
                .magnitude()
        })
        .collect();
    assert_eq!(
        costs[0], costs[1],
        "two identically-metered candidates must not be pulled apart by the new cost term: \
         {costs:?}"
    );
}
