//! `GH-ROUTING-CAPABILITY` — the capability registry (map line 1382), joined
//! to a task's hard capability requirements through the destination router.
//!
//! Practice §35/§36: a scorer a test reaches only by calling its inner
//! function directly is not a router. Every test that stands for the
//! production-consumer requirement (tests 1, 2 and 4) goes through
//! [`SessionRouter::choose`] the way `main.rs` does, and inspects what it
//! actually returned rather than calling `capability::capability_fit` or the
//! registry directly. Test 3 is the one exception: box lines 1383–1389 are
//! about the registry's own descriptive capability, not about a routing
//! consumer, so it exercises `capability::ResourceCapabilities` on its own.

use std::collections::BTreeMap;
use std::time::Instant;

use glasshouse::config::pairing::{WarmSession, WarmSessionState};
use glasshouse::harness::pairing::PairingOverrides;
use glasshouse::harness::{Capabilities as HarnessCapabilities, Declared};
use glasshouse::integrations::IntegrationId;
use glasshouse::routing::capability::{CapabilityAxis, ResourceCapabilities, ResourceFacts};
use glasshouse::routing::classify::HardCapability;
use glasshouse::routing::free::FreePool;
use glasshouse::routing::session::{
    Destination, RouterInputs, RoutingMoment, SessionRouter, TaskRequirements,
};
use glasshouse::routing::{AssignedModel, Backend, Cost, CredentialId, ToolSemantics};
use glasshouse::secret::SecretRef;

/// `WireProtocol::OpenAiResponses`'s slug — the one protocol
/// `IntegrationId::Codex` actually declares (`harness::codex::PROTOCOLS`), so
/// a destination on it clears the protocol hard constraint.
const CODEX_PROTOCOL: &str = "openai-responses";

fn backend(provider: &str, model: &str, var: &str) -> Backend {
    Backend::new(
        provider,
        CODEX_PROTOCOL,
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

fn verified(value: bool) -> Declared<bool> {
    Declared::verified(value, "test evidence")
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

    fn inputs(&self, hard_capabilities: Vec<HardCapability>) -> RouterInputs<'_> {
        RouterInputs {
            overrides: &self.overrides,
            health: &self.health,
            now: self.now,
            requirements: TaskRequirements {
                hard_capabilities,
                ..TaskRequirements::default()
            },
        }
    }
}

/// A destination on `IntegrationId::Codex`, whose real adapter declares
/// `browser_use` and `shell_access` as `Unverified` — the honest baseline a
/// test can override with `resource_facts` without depending on what a real
/// harness adapter happens to declare today.
fn codex_destination(id: &str, facts: ResourceFacts) -> Destination {
    Destination::existing(
        id,
        IntegrationId::Codex,
        "default",
        backend("openai", "gpt-5-codex", "OPENAI_API_KEY"),
        live(0),
    )
    .with_resource_facts(facts)
}

/// Acceptance test 1. A task requiring `BrowserInteraction`, routed against
/// two destinations differing **only** in browser capability, ranks the
/// capable one higher and names the axis in the explanation.
#[test]
fn a_task_needing_browser_interaction_ranks_the_browser_capable_destination_higher() {
    let fixture = Fixture::new();
    let capable = codex_destination(
        "capable",
        ResourceFacts {
            browser_use: verified(true),
            ..ResourceFacts::UNVERIFIED
        },
    );
    let not_established = codex_destination("not-established", ResourceFacts::UNVERIFIED);

    let inputs = fixture.inputs(vec![HardCapability::BrowserInteraction]);
    let routed = SessionRouter::new()
        .choose(
            RoutingMoment::SessionStart,
            None,
            &[not_established, capable],
            &inputs,
        )
        .expect("destinations were offered");

    assert_eq!(
        routed.chosen().id(),
        "capable",
        "the destination established to support browser interaction did not win"
    );

    let evidence = routed
        .explanation()
        .contributions()
        .iter()
        .find(|c| c.name() == "capability fit")
        .expect("the winner is scored for capability fit")
        .evidence()
        .to_owned();
    assert!(
        evidence.contains("browser-use"),
        "the explanation did not name the axis that decided this ranking: {evidence}"
    );
}

/// Acceptance test 2, and ruling 3's own test. A resource whose axis is
/// `Declared::Unverified` must score strictly better than one established
/// absent, and must not read as a `no`.
#[test]
fn an_unverified_axis_scores_strictly_better_than_an_established_absent_one() {
    let fixture = Fixture::new();
    let unverified = codex_destination("unverified", ResourceFacts::UNVERIFIED);
    let established_absent = codex_destination(
        "established-absent",
        ResourceFacts {
            browser_use: verified(false),
            ..ResourceFacts::UNVERIFIED
        },
    );

    let inputs = fixture.inputs(vec![HardCapability::BrowserInteraction]);
    let routed = SessionRouter::new()
        .choose(
            RoutingMoment::SessionStart,
            None,
            &[established_absent, unverified],
            &inputs,
        )
        .expect("destinations were offered");

    assert_eq!(
        routed.chosen().id(),
        "unverified",
        "an established-absent resource must not outrank one that is merely unverified"
    );

    let capability_fit = |id: &str| {
        routed
            .considered()
            .iter()
            .find(|(destination, _)| destination.id() == id)
            .expect("both candidates were eligible")
            .1
            .contributions()
            .iter()
            .find(|c| c.name() == "capability fit")
            .expect("every candidate is scored for capability fit")
            .clone()
    };
    let unverified_term = capability_fit("unverified");
    let absent_term = capability_fit("established-absent");
    assert!(
        unverified_term.magnitude() > absent_term.magnitude(),
        "unverified ({}) did not score strictly better than established-absent ({})",
        unverified_term.magnitude(),
        absent_term.magnitude()
    );
    assert!(
        unverified_term.evidence().contains("not a `no`"),
        "the unverified evidence string does not read as \"not a no\": {}",
        unverified_term.evidence()
    );
}

/// Acceptance test 3. All seven axes 1383–1389 are representable and each
/// appears in a rendered registry description — the direct evidence for
/// those seven boxes. This is the one test in this file that reads the
/// registry directly rather than through a routing decision, because these
/// boxes are about the registry's own descriptive capability.
#[test]
fn all_seven_axes_are_representable_and_named_in_a_rendered_description() {
    let resource =
        ResourceCapabilities::describe(&HarnessCapabilities::UNVERIFIED, ResourceFacts::UNVERIFIED);
    let rendered = resource.render();
    for axis in CapabilityAxis::ALL {
        assert!(
            rendered.contains(axis.name()),
            "axis `{}` is missing from the rendered registry description:\n{rendered}",
            axis.name()
        );
    }
}

/// Acceptance test 4 — 1390's executable form. Correcting a resource's
/// capability description changes what `SessionRouter::choose` computes, and
/// this file never edits `session.rs` to make that happen: the only thing
/// that varies between the two calls below is a `ResourceFacts` value.
///
/// Uses `browser-use`, not `shell/tool-use`: `IntegrationId::Codex`'s real
/// adapter already declares `shell_access` established present, so a
/// baseline with no facts override would fall back to the same value the
/// "corrected" description asserts and prove nothing. `browser_use` is the
/// axis Codex's adapter leaves `Unverified`, which is what makes the
/// baseline here actually undescribed.
#[test]
fn changing_a_capability_description_changes_routing_without_touching_session_rs() {
    let fixture = Fixture::new();
    let inputs = fixture.inputs(vec![HardCapability::BrowserInteraction]);

    let baseline = codex_destination("resource", ResourceFacts::UNVERIFIED);
    let corrected = codex_destination(
        "resource",
        ResourceFacts {
            browser_use: verified(true),
            ..ResourceFacts::UNVERIFIED
        },
    );

    let routed_before = SessionRouter::new()
        .choose(
            RoutingMoment::SessionStart,
            None,
            std::slice::from_ref(&baseline),
            &inputs,
        )
        .expect("a destination was offered");
    let routed_after = SessionRouter::new()
        .choose(
            RoutingMoment::SessionStart,
            None,
            std::slice::from_ref(&corrected),
            &inputs,
        )
        .expect("a destination was offered");

    let total_before = routed_before.explanation().total();
    let total_after = routed_after.explanation().total();
    assert!(
        total_after > total_before,
        "correcting the resource's browser-use description did not change the routing \
         score ({total_before} vs {total_after}) — 1390 requires that a description change \
         alone can move a routing decision"
    );
}
