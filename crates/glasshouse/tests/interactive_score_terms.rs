//! Map lines 1533 and 1551 — *"include existing session affinity in
//! candidate scoring"* and *"include session-switching and bootstrap cost
//! in candidate scoring."*
//!
//! `docs/product/evidence/phase-35b.md:63-67` rules both lines structurally
//! inapplicable **to the disposable-job scorer** it was checking — a
//! disposable job has no session, so neither term has a subject there. That
//! ruling says nothing about the **interactive** router
//! (`routing::session::SessionRouter::choose`), which already carries both
//! terms in `score()` (`routing/session.rs:3083,3093`) and is proven for
//! other lines by `tests/session_router.rs`. This file is the test scoped to
//! this exact pair of lines, on the router phase-35b.md did not examine.
//!
//! Two mutations away, in `routing/session.rs::score`, are the two lines
//! this file's two tests kill: dropping the `session_affinity` push at
//! `:3083`, and dropping the `switching_and_bootstrap_cost` push at `:3093`.

use std::collections::BTreeMap;
use std::time::Instant;

use glasshouse::config::pairing::{WarmSession, WarmSessionState};
use glasshouse::harness::pairing::PairingOverrides;
use glasshouse::integrations::IntegrationId;
use glasshouse::routing::free::FreePool;
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
