//! Phase 37 — the session-aware router, entered the way production enters it.
//!
//! Every test here goes through [`SessionRouter::choose`]. Practice §35: a
//! scorer every test reaches by calling its inner function is not a router,
//! and the six `Consider X` lines (1595–1600) are only closed by a test that
//! holds two destinations differing **in that axis alone** and shows them
//! resolving differently. A test that asserted the axis merely *appears* in
//! the explanation would prove the renderer.
//!
//! The other reason the pairs are built this carefully is
//! `docs/product/evidence/phase-9j.md`'s last entry: a signal that is
//! constant across the candidate set cannot change a ranking, whatever its
//! magnitude. Each pair below is therefore also the executable evidence that
//! the axis it names really does vary across a candidate set this router can
//! be handed.

use std::time::{Duration, Instant};

use std::collections::BTreeMap;

use glasshouse::config::pairing::{WarmSession, WarmSessionState};
use glasshouse::harness::WireProtocol;
use glasshouse::harness::pairing::{ModelBehaviourFit, ModelCorrection, PairingOverrides};
use glasshouse::integrations::IntegrationId;
use glasshouse::provider::quota::{
    Capacity, CapacityState, NativeAmount, Pool, Reading, ReadingSource, RemainingCapacityScore,
};
use glasshouse::routing::free::{FreePool, FreeResource, WorkloadOutcome};
use glasshouse::routing::session::{
    CheckpointQuality, Destination, RouterInputs, RoutingMoment, RoutingOverride, SessionRouter,
    TaskRequirements,
};
use glasshouse::routing::{
    AssignedModel, Backend, Cost, CredentialId, HardConstraint, ToolSemantics,
};
use glasshouse::secret::SecretRef;

// ---------------------------------------------------------------------------
// Fixtures. Deliberately thin: every test below builds its *own* pair, so
// that "differing only in X" is visible in the test body rather than hidden
// in a helper that sets up the world (§35's own warning).
// ---------------------------------------------------------------------------

const ANTHROPIC: &str = "anthropic-messages";

fn backend(provider: &str, model: &str, var: &str) -> Backend {
    backend_on(provider, model, var, ANTHROPIC, ToolSemantics::Verified)
}

fn backend_on(
    provider: &str,
    model: &str,
    var: &str,
    protocol: &str,
    tools: ToolSemantics,
) -> Backend {
    Backend::new(
        provider,
        protocol,
        AssignedModel::named(model),
        CredentialId::new(
            provider,
            SecretRef::Environment {
                var: var.to_owned(),
            },
        ),
        Cost::Metered,
        tools,
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

/// A real, fully-measured remaining-capacity score at `percent`.
fn capacity(percent: i64) -> RemainingCapacityScore {
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
        .expect("both halves of the credits pool are measured")
}

// ---------------------------------------------------------------------------
// Line 1592 — the boundary gate.
// ---------------------------------------------------------------------------

/// Line 1592, as the discriminating pair: **identical inputs**, and the only
/// thing that changes is the moment. A router that ranked at every moment
/// would move the work in both halves, which is the defect the line exists to
/// forbid.
#[test]
fn routing_is_taken_at_a_task_boundary_and_not_between_turns() {
    let fixture = Fixture::new();
    let current = Destination::existing(
        "current",
        IntegrationId::ClaudeCode,
        "default",
        backend("anthropic", "claude-opus-4", "ANTHROPIC_API_KEY"),
        live(0),
    );
    // A destination that outscores `current` on quota alone, so a router that
    // re-decides at all will move to it.
    let better = Destination::existing(
        "better",
        IntegrationId::ClaudeCode,
        "default",
        backend("anthropic", "claude-opus-4", "ANTHROPIC_API_KEY"),
        live(0),
    )
    .with_capacity(Some(capacity(100)));
    let destinations = vec![current.clone(), better];

    let router = SessionRouter::new();

    let mid_turn = router
        .choose(
            RoutingMoment::MidTurn,
            Some(&current),
            &destinations,
            &fixture.inputs(),
        )
        .expect("a current destination is always answerable");
    assert_eq!(
        mid_turn.chosen().id(),
        "current",
        "a mid-turn moment re-decided the destination: line 1592 forbids switching between \
         conversational turns"
    );
    assert!(!mid_turn.re_decided());

    let boundary = router
        .choose(
            RoutingMoment::TaskBoundary,
            Some(&current),
            &destinations,
            &fixture.inputs(),
        )
        .expect("destinations were offered");
    assert_eq!(
        boundary.chosen().id(),
        "better",
        "a task boundary is exactly where line 1592 says routing may be taken, and the better \
         destination was not chosen"
    );
    assert!(boundary.re_decided());
}

// ---------------------------------------------------------------------------
// Lines 1593 and 1594 — existing against fresh.
// ---------------------------------------------------------------------------

/// Line 1593. The fresh destination is *better resourced* — full quota
/// against none read — and the warm session still wins, because affinity plus
/// the bootstrap cost it does not pay outweighs it. That is the line's own
/// word "outweighs", tested as a comparison rather than as a preference.
#[test]
fn a_warm_existing_session_outweighs_a_better_resourced_fresh_one() {
    let fixture = Fixture::new();
    let warm = Destination::existing(
        "warm",
        IntegrationId::ClaudeCode,
        "default",
        backend("anthropic", "claude-opus-4", "ANTHROPIC_API_KEY"),
        live(0),
    );
    // The fresh destination is given *both* benefits this build can quantify:
    // a full quota reading nothing has been read for the warm one, and the
    // best checkpoint there is, so its bootstrap cost is at its minimum. That
    // is deliberate — the test has to be one affinity can lose, or it would
    // be proving the bootstrap cost while claiming to prove line 1593.
    let fresh = Destination::fresh(
        "fresh",
        IntegrationId::ClaudeCode,
        "default",
        backend("anthropic", "claude-opus-4", "ANTHROPIC_API_KEY"),
        Some(CheckpointQuality::new(true, true)),
    )
    .with_capacity(Some(capacity(100)));

    let routed = SessionRouter::new()
        .choose(
            RoutingMoment::SessionStart,
            None,
            &[fresh, warm],
            &fixture.inputs(),
        )
        .expect("destinations were offered");
    assert_eq!(routed.chosen().id(), "warm");

    // And the margin is the affinity, not a rounding accident: without it the
    // fresh destination is ahead.
    let totals: Vec<f64> = routed
        .considered()
        .iter()
        .map(|(_, explanation)| explanation.total())
        .collect();
    let affinity = routed
        .explanation()
        .contributions()
        .iter()
        .find(|c| c.name() == "session affinity")
        .expect("the winner is an existing session")
        .magnitude();
    assert!(
        totals[0] - affinity < totals[1],
        "the warm session would still have won with no affinity at all ({totals:?}, affinity \
         {affinity}): this test proves the bootstrap cost, not line 1593"
    );
}

/// **Line 1594 does not close, and this test is why.**
///
/// The line is *"prefer a fresh session when existing relevant sessions are
/// cold, bloated, or semantically poor **and** a good checkpoint exists"* —
/// a disjunction of three defects. Two of the three have **no producer in
/// this build**: `WarmSession` deliberately carries no accumulated-context
/// figure (so nothing can say "bloated"), and Phase 36's semantic-quality
/// lines 1584 and 1586 are at zero (so nothing can say "semantically poor").
/// The one that does have a producer is "cold", and coldness **on its own
/// does not justify preferring a fresh session**: a session idle for thirty
/// hours still holds its entire transcript, and resuming it costs nothing,
/// while a fresh session pays a bootstrap even from the best checkpoint
/// there is. Pricing coldness as a defect would be inventing the very signal
/// that is missing.
///
/// So this asserts what is actually true today, as an executable tripwire in
/// the shape `phase-9j.md` established: the cold session still wins. **If
/// anyone gives this router a bloat or semantic-quality signal, this test
/// fails, and its failure means line 1594 has just become reachable.**
#[test]
fn a_merely_cold_session_still_beats_a_fresh_one_because_coldness_is_not_a_defect() {
    let fixture = Fixture::new();
    // Past `WARM_SESSION_RELEVANCE_WINDOW_SECONDS`, so its affinity is
    // exactly zero rather than merely small: the coldest a session gets.
    let cold = Destination::existing(
        "cold",
        IntegrationId::ClaudeCode,
        "default",
        backend("anthropic", "claude-opus-4", "ANTHROPIC_API_KEY"),
        live(30 * 60 * 60),
    );
    let fresh = Destination::fresh(
        "fresh",
        IntegrationId::ClaudeCode,
        "default",
        backend("anthropic", "claude-opus-4", "ANTHROPIC_API_KEY"),
        Some(CheckpointQuality::new(true, true)),
    );

    let routed = SessionRouter::new()
        .choose(
            RoutingMoment::TaskBoundary,
            None,
            &[cold, fresh],
            &fixture.inputs(),
        )
        .expect("destinations were offered");
    assert_eq!(
        routed.chosen().id(),
        "cold",
        "a fresh session was preferred over a merely cold one: that is line 1594's conclusion \
         reached without line 1594's evidence — nothing in this build can say a session is \
         bloated or semantically poor"
    );
}

/// Line 1594's *other* clause does have a producer, and it decides something
/// real: between two fresh destinations, the one with a good checkpoint to
/// boot from wins. This is what the checkpoint term is closing today, and it
/// is deliberately not sold as line 1594.
#[test]
fn a_good_checkpoint_decides_between_two_fresh_destinations() {
    let fixture = Fixture::new();
    let serving = backend("anthropic", "claude-opus-4", "ANTHROPIC_API_KEY");
    let from_nothing = Destination::fresh(
        "from-nothing",
        IntegrationId::ClaudeCode,
        "default",
        serving.clone(),
        None,
    );
    let from_checkpoint = Destination::fresh(
        "from-checkpoint",
        IntegrationId::ClaudeCode,
        "default",
        serving.clone(),
        Some(CheckpointQuality::new(true, true)),
    );
    let from_trimmed = Destination::fresh(
        "from-trimmed",
        IntegrationId::ClaudeCode,
        "default",
        serving,
        // Next actions present, but content was dropped to fit the bound —
        // `CheckpointQuality::is_good` requires both.
        Some(CheckpointQuality::new(true, false)),
    );

    let routed = SessionRouter::new()
        .choose(
            RoutingMoment::SessionStart,
            None,
            &[from_nothing, from_trimmed, from_checkpoint],
            &fixture.inputs(),
        )
        .expect("destinations were offered");
    assert_eq!(routed.chosen().id(), "from-checkpoint");
}

// ---------------------------------------------------------------------------
// Lines 1595 to 1600 — one discriminating pair each.
// ---------------------------------------------------------------------------

/// Line 1595. The two destinations differ **only** in harness. Claude Code
/// declares it speaks the Anthropic Messages protocol, so its fit is `native`;
/// OpenCode declares `openai-chat`, so on the same route it reaches this
/// provider only because the provider also serves the protocol OpenCode does
/// speak — `compatible`. Same model, same credential, same warmth, same
/// everything else.
///
/// This pair is also the direct answer to phase 9J's constancy finding: the
/// signal `classify` derives is constant across a candidate set that varies
/// only by route, and it is **not** constant across one that varies by
/// harness, which is what a destination can do and a backend cannot.
#[test]
fn harness_capability_fit_decides_between_two_otherwise_identical_destinations() {
    let fixture = Fixture::new();
    let serving = backend("openrouter", "claude-opus-4", "OPENROUTER_API_KEY");
    let both_protocols = vec![WireProtocol::AnthropicMessages, WireProtocol::OpenAiChat];

    let opencode = Destination::existing(
        "opencode",
        IntegrationId::OpenCode,
        "default",
        serving.clone(),
        live(0),
    )
    .with_provider_protocols(both_protocols.clone());
    let claude_code = Destination::existing(
        "claude-code",
        IntegrationId::ClaudeCode,
        "default",
        serving,
        live(0),
    )
    .with_provider_protocols(both_protocols);

    let routed = SessionRouter::new()
        .choose(
            RoutingMoment::SessionStart,
            None,
            &[opencode, claude_code],
            &fixture.inputs(),
        )
        .expect("destinations were offered");

    assert_eq!(
        routed.chosen().id(),
        "claude-code",
        "the harness that speaks this route's own wire protocol did not win: line 1595's signal \
         is not deciding anything"
    );
    let fit: Vec<f64> = routed
        .considered()
        .iter()
        .map(|(_, explanation)| {
            explanation
                .contributions()
                .iter()
                .find(|c| c.name() == "harness capability fit")
                .expect("every candidate is scored for capability fit")
                .magnitude()
        })
        .collect();
    assert!(
        fit[0] != fit[1],
        "the capability-fit contribution was identical for both harnesses ({fit:?}): a signal \
         constant across the candidate set cannot change the ranking (phase-9j)"
    );
}

/// Line 1596. Two existing sessions on the same backend, differing only in how
/// long they have been idle.
#[test]
fn session_affinity_decides_between_two_otherwise_identical_sessions() {
    let fixture = Fixture::new();
    let serving = backend("anthropic", "claude-opus-4", "ANTHROPIC_API_KEY");
    let stale = Destination::existing(
        "stale",
        IntegrationId::ClaudeCode,
        "default",
        serving.clone(),
        live(7 * 60 * 60),
    );
    let recent = Destination::existing(
        "recent",
        IntegrationId::ClaudeCode,
        "default",
        serving,
        live(60),
    );

    let routed = SessionRouter::new()
        .choose(
            RoutingMoment::SessionStart,
            None,
            &[stale, recent],
            &fixture.inputs(),
        )
        .expect("destinations were offered");
    assert_eq!(routed.chosen().id(), "recent");
}

/// Line 1597. Two existing sessions on the same provider and model, reached
/// through **different credentials**, against a current destination on one of
/// them. That is `CacheLocality`'s own `LikelyLost(CredentialChanged)` case
/// against `Preserved` — the one the map's word "likely" is actually about —
/// and it isolates prompt-cache state from provider identity, which a
/// different-provider pair would have confounded with health and switching
/// cost.
#[test]
fn prompt_cache_state_decides_between_two_credentials_of_one_provider() {
    let fixture = Fixture::new();
    let current = Destination::existing(
        "current",
        IntegrationId::ClaudeCode,
        "default",
        backend("anthropic", "claude-opus-4", "ANTHROPIC_API_KEY"),
        live(0),
    );
    let rotated = Destination::existing(
        "rotated",
        IntegrationId::ClaudeCode,
        "default",
        backend("anthropic", "claude-opus-4", "ANTHROPIC_API_KEY_2"),
        live(0),
    );
    let same_key = Destination::existing(
        "same-key",
        IntegrationId::ClaudeCode,
        "default",
        backend("anthropic", "claude-opus-4", "ANTHROPIC_API_KEY"),
        live(0),
    );

    let routed = SessionRouter::new()
        .choose(
            RoutingMoment::TaskBoundary,
            Some(&current),
            &[rotated, same_key],
            &fixture.inputs(),
        )
        .expect("destinations were offered");
    assert_eq!(
        routed.chosen().id(),
        "same-key",
        "the destination whose provider-side cache survives did not win: line 1597's signal is \
         not deciding anything"
    );
}

/// Line 1598. Two destinations differing only in what has been **read** about
/// their remaining quota. Note the loser is the one with a real low reading,
/// not the one with no reading: an unread resource contributes `0.0` and is
/// neither preferred nor withheld.
#[test]
fn known_quota_pressure_decides_between_two_otherwise_identical_destinations() {
    let fixture = Fixture::new();
    let tight = Destination::existing(
        "tight",
        IntegrationId::ClaudeCode,
        "default",
        backend("anthropic", "claude-opus-4", "KEY_A"),
        live(0),
    )
    .with_capacity(Some(capacity(5)));
    let roomy = Destination::existing(
        "roomy",
        IntegrationId::ClaudeCode,
        "default",
        backend("anthropic", "claude-opus-4", "KEY_B"),
        live(0),
    )
    .with_capacity(Some(capacity(95)));

    let routed = SessionRouter::new()
        .choose(
            RoutingMoment::SessionStart,
            None,
            &[tight, roomy],
            &fixture.inputs(),
        )
        .expect("destinations were offered");
    assert_eq!(routed.chosen().id(), "roomy");
}

/// Line 1599. Two destinations differing only in what a **real workload** has
/// observed about them — the free pool's health half, whose only mutator is
/// `FreePool::observe` and whose production caller is
/// `gateway::session::observe_exchange`. Nothing here probes; the unhealthy
/// destination is unhealthy because a request that was going to happen anyway
/// failed.
#[test]
fn observed_provider_health_decides_between_two_otherwise_identical_destinations() {
    let mut fixture = Fixture::new();
    let flaky_backend = backend("anthropic", "claude-opus-4", "KEY_A");
    let healthy_backend = backend("anthropic", "claude-opus-4", "KEY_B");

    fixture.health.observe(
        &FreeResource::new(
            flaky_backend.credential().clone(),
            flaky_backend.model().label(),
        ),
        WorkloadOutcome::CapacityFailure,
        fixture.now,
    );

    let flaky = Destination::existing(
        "flaky",
        IntegrationId::ClaudeCode,
        "default",
        flaky_backend,
        live(0),
    );
    let healthy = Destination::existing(
        "healthy",
        IntegrationId::ClaudeCode,
        "default",
        healthy_backend,
        live(0),
    );

    let routed = SessionRouter::new()
        .choose(
            RoutingMoment::SessionStart,
            None,
            &[flaky, healthy],
            &fixture.inputs(),
        )
        .expect("destinations were offered");
    assert_eq!(
        routed.chosen().id(),
        "healthy",
        "an observed failure did not cost the destination anything: line 1599's signal is not \
         deciding anything"
    );
}

/// Line 1600. Two destinations reachable on the same terms, differing only in
/// what the **move** costs: one keeps the harness the work is on, the other
/// changes it.
#[test]
fn switching_cost_decides_between_two_otherwise_identical_destinations() {
    let fixture = Fixture::new();
    let serving = backend("openrouter", "claude-opus-4", "OPENROUTER_API_KEY");
    let both_protocols = vec![WireProtocol::AnthropicMessages, WireProtocol::OpenAiChat];

    let current = Destination::existing(
        "current",
        IntegrationId::OpenCode,
        "default",
        serving.clone(),
        live(0),
    )
    .with_provider_protocols(both_protocols.clone());
    // Same harness as `current`, so no harness switch.
    let stay = Destination::existing(
        "stay",
        IntegrationId::OpenCode,
        "default",
        serving.clone(),
        live(0),
    )
    .with_provider_protocols(both_protocols.clone());
    // A different harness. Its capability fit is *better* (native rather than
    // compatible), and the switching cost still has to be able to outweigh
    // that for line 1600 to be doing anything.
    let move_harness = Destination::existing(
        "move",
        IntegrationId::ClaudeCode,
        "default",
        serving,
        live(0),
    )
    .with_provider_protocols(both_protocols);

    let routed = SessionRouter::new()
        .choose(
            RoutingMoment::TaskBoundary,
            Some(&current),
            &[move_harness, stay],
            &fixture.inputs(),
        )
        .expect("destinations were offered");
    assert_eq!(
        routed.chosen().id(),
        "stay",
        "changing the harness cost nothing: line 1600's signal is not deciding anything"
    );
}

// ---------------------------------------------------------------------------
// Line 1601 — the inspectable explanation.
// ---------------------------------------------------------------------------

/// Line 1601. The overview names every contribution that decided the winner,
/// scores every alternative, and says which hard constraint removed anything
/// it removed — the three things "why this one" needs and a winner-only
/// answer cannot give.
#[test]
fn the_overview_explains_the_winner_the_alternatives_and_the_rejections() {
    let fixture = Fixture::new();
    let ok = Destination::existing(
        "ok",
        IntegrationId::ClaudeCode,
        "default",
        backend("anthropic", "claude-opus-4", "ANTHROPIC_API_KEY"),
        live(0),
    );
    let second = Destination::fresh(
        "second",
        IntegrationId::ClaudeCode,
        "default",
        backend("anthropic", "claude-opus-4", "ANTHROPIC_API_KEY"),
        None,
    );
    // A harness that declares protocols, none of which this provider serves
    // or the gateway translates to one it does: `ProtocolFit::Incompatible`,
    // which is a hard constraint here. OpenCode on an anthropic-only backend
    // since T2b — openai-chat <-> openai-responses became a translated pairing too.
    let unreachable = Destination::existing(
        "unreachable",
        IntegrationId::OpenCode,
        "default",
        backend_on(
            "chat",
            "claude-opus-4",
            "ANTHROPIC_API_KEY",
            ANTHROPIC,
            ToolSemantics::Verified,
        ),
        live(0),
    );

    let routed = SessionRouter::new()
        .choose(
            RoutingMoment::SessionStart,
            None,
            &[ok, second, unreachable],
            &fixture.inputs(),
        )
        .expect("destinations were offered");

    let names: Vec<&str> = routed
        .explanation()
        .contributions()
        .iter()
        .map(|c| c.name())
        .collect();
    for expected in [
        "harness capability fit",
        "session affinity",
        "prompt-cache state",
        "known quota pressure",
        "provider health",
        "switching and bootstrap cost",
    ] {
        assert!(
            names.contains(&expected),
            "the explanation for the chosen destination does not name `{expected}`: {names:?}"
        );
    }

    assert_eq!(routed.rejected().len(), 1);
    assert_eq!(routed.rejected()[0].0.id(), "unreachable");
    assert_eq!(routed.rejected()[0].1, HardConstraint::Protocol);

    let overview = routed.render_overview();
    assert!(overview.contains("second"), "{overview}");
    assert!(overview.contains("unreachable"), "{overview}");
    assert!(overview.contains("hard protocol constraint"), "{overview}");
    // A routing explanation reaches a diagnostic. Credentials appear as
    // `SecretRef` names and never as values — the value is not even in the
    // type — and the variable name is what a reader needs to act on.
    assert!(
        !overview.contains("sk-"),
        "a routing overview must never carry a credential value: {overview}"
    );
}

/// A build with no evidence, no health and no quota data must still choose
/// something sensible and say that it had nothing to go on — the packet's own
/// required behaviour, and the state of every machine before any telemetry
/// exists.
#[test]
fn a_build_with_nothing_configured_still_chooses_and_says_it_had_nothing_to_go_on() {
    let fixture = Fixture::new();
    let only = Destination::fresh(
        "only",
        IntegrationId::ClaudeCode,
        "default",
        backend("anthropic", "claude-opus-4", "ANTHROPIC_API_KEY"),
        None,
    );

    let routed = SessionRouter::new()
        .choose(
            RoutingMoment::SessionStart,
            None,
            &[only],
            &fixture.inputs(),
        )
        .expect("one destination was offered");
    assert_eq!(routed.chosen().id(), "only");

    let rendered = routed.render();
    assert!(
        rendered.contains("nothing has been read about"),
        "the explanation does not say the quota was never read: {rendered}"
    );
    assert!(
        rendered.contains("the absence of one"),
        "the explanation does not say health was never observed: {rendered}"
    );
}

// ---------------------------------------------------------------------------
// Line 1602 — the override.
// ---------------------------------------------------------------------------

/// Line 1602's main case: the user names a destination the ranking put last,
/// and it wins — and the explanation says what it overruled, so the override
/// is inspectable rather than silent.
#[test]
fn a_user_override_beats_the_ranking_and_says_what_it_overruled() {
    let fixture = Fixture::new();
    let warm = Destination::existing(
        "warm",
        IntegrationId::ClaudeCode,
        "default",
        backend("anthropic", "claude-opus-4", "ANTHROPIC_API_KEY"),
        live(0),
    );
    let cold = Destination::fresh(
        "cold",
        IntegrationId::ClaudeCode,
        "default",
        backend("anthropic", "claude-opus-4", "ANTHROPIC_API_KEY"),
        None,
    );
    let destinations = vec![warm, cold];

    let automatic = SessionRouter::new()
        .choose(
            RoutingMoment::SessionStart,
            None,
            &destinations,
            &fixture.inputs(),
        )
        .expect("destinations were offered");
    assert_eq!(automatic.chosen().id(), "warm");
    assert_eq!(automatic.overrode(), None);

    let overridden = SessionRouter::with_override(RoutingOverride::to("cold"))
        .choose(
            RoutingMoment::SessionStart,
            None,
            &destinations,
            &fixture.inputs(),
        )
        .expect("destinations were offered");
    assert_eq!(overridden.chosen().id(), "cold");
    assert_eq!(overridden.overrode(), Some("warm"));
    assert!(overridden.render().contains("would have chosen `warm`"));
}

/// The other automatic choice line 1602's word "every" covers: *whether to
/// route at this moment at all*. Without the override the mid-turn moment
/// holds the work; with it, the same call ranks.
#[test]
fn a_user_override_can_lift_the_boundary_gate_itself() {
    let fixture = Fixture::new();
    let current = Destination::existing(
        "current",
        IntegrationId::ClaudeCode,
        "default",
        backend("anthropic", "claude-opus-4", "ANTHROPIC_API_KEY"),
        live(0),
    );
    let better = Destination::existing(
        "better",
        IntegrationId::ClaudeCode,
        "default",
        backend("anthropic", "claude-opus-4", "ANTHROPIC_API_KEY"),
        live(0),
    )
    .with_capacity(Some(capacity(100)));
    let destinations = vec![current.clone(), better];

    let held = SessionRouter::new()
        .choose(
            RoutingMoment::MidTurn,
            Some(&current),
            &destinations,
            &fixture.inputs(),
        )
        .expect("a current destination is always answerable");
    assert_eq!(held.chosen().id(), "current");

    let asked = SessionRouter::with_override(RoutingOverride::route_now())
        .choose(
            RoutingMoment::MidTurn,
            Some(&current),
            &destinations,
            &fixture.inputs(),
        )
        .expect("destinations were offered");
    assert_eq!(
        asked.chosen().id(),
        "better",
        "a person asking to re-route now is not line 1592's blind per-turn switching, and \
         line 1602's `every` includes this choice"
    );
    assert!(asked.re_decided());
}

/// An override may overrule a ranking and not a fact. The named destination
/// cannot serve the protocol at all; the router refuses, says so, and falls
/// back to the ranking rather than starting a session that cannot run.
#[test]
fn an_override_cannot_overrule_a_hard_constraint_and_reports_the_refusal() {
    let fixture = Fixture::new();
    let ok = Destination::existing(
        "ok",
        IntegrationId::ClaudeCode,
        "default",
        backend("anthropic", "claude-opus-4", "ANTHROPIC_API_KEY"),
        live(0),
    );
    // OpenCode on an anthropic-only backend: still incompatible after T2,
    // the one pairing the table still refuses.
    let unreachable = Destination::existing(
        "unreachable",
        IntegrationId::OpenCode,
        "default",
        backend_on(
            "chat",
            "claude-opus-4",
            "ANTHROPIC_API_KEY",
            ANTHROPIC,
            ToolSemantics::Verified,
        ),
        live(0),
    );

    let routed = SessionRouter::with_override(RoutingOverride::to("unreachable"))
        .choose(
            RoutingMoment::SessionStart,
            None,
            &[ok, unreachable],
            &fixture.inputs(),
        )
        .expect("destinations were offered");
    assert_eq!(routed.chosen().id(), "ok");
    let refusal = routed
        .override_refused()
        .expect("the override named a destination a hard constraint rejected");
    assert!(refusal.to_string().contains("hard protocol constraint"));
    assert!(routed.render().contains("not applied"));
}

// ---------------------------------------------------------------------------
// The hard-constraint gate, and the one thing it must not do.
// ---------------------------------------------------------------------------

/// A task that needs tool calls may not be sent where tool calls are
/// established not to work — and `Unverified` is **not** a `no`, which is the
/// distinction `ToolSemantics`' third state exists for.
#[test]
fn a_task_needing_tool_calls_refuses_a_backend_known_not_to_carry_them() {
    let fixture = Fixture::new();
    let inputs = RouterInputs {
        overrides: &fixture.overrides,
        health: &fixture.health,
        now: fixture.now,
        requirements: TaskRequirements {
            needs_tool_calls: true,
            ..TaskRequirements::default()
        },
    };

    let known_absent = Destination::existing(
        "known-absent",
        IntegrationId::ClaudeCode,
        "default",
        backend_on(
            "anthropic",
            "claude-opus-4",
            "KEY_A",
            ANTHROPIC,
            ToolSemantics::KnownAbsent,
        ),
        live(0),
    );
    let unverified = Destination::existing(
        "unverified",
        IntegrationId::ClaudeCode,
        "default",
        backend_on(
            "anthropic",
            "claude-opus-4",
            "KEY_B",
            ANTHROPIC,
            ToolSemantics::Unverified,
        ),
        live(4 * 60 * 60),
    );

    let routed = SessionRouter::new()
        .choose(
            RoutingMoment::SessionStart,
            None,
            &[known_absent, unverified],
            &inputs,
        )
        .expect("destinations were offered");
    assert_eq!(routed.chosen().id(), "unverified");
    assert_eq!(routed.rejected().len(), 1);
    assert_eq!(routed.rejected()[0].1, HardConstraint::ToolSemantics);
}

/// A user correction is what makes `ModelBehaviourFit::KnownAbsent` reachable
/// at all, so this is also the test that `RouterInputs::overrides` is
/// load-bearing rather than carried.
#[test]
fn a_user_correction_that_a_model_misbehaves_costs_it_the_ranking() {
    let health = FreePool::new();
    let now = Instant::now();
    let serving = backend("anthropic", "claude-opus-4", "ANTHROPIC_API_KEY");
    let other = backend("anthropic", "some-other-model", "ANTHROPIC_API_KEY");

    let correction = ModelCorrection {
        behaviour: Some(ModelBehaviourFit::KnownAbsent),
        ..ModelCorrection::default()
    };
    let mut models = BTreeMap::new();
    models.insert("claude-opus-4".to_owned(), correction);
    let overrides =
        PairingOverrides::from_parts("the test's own configuration", models, BTreeMap::new());

    let corrected = Destination::existing(
        "corrected",
        IntegrationId::ClaudeCode,
        "default",
        serving,
        live(0),
    );
    let untouched = Destination::existing(
        "untouched",
        IntegrationId::ClaudeCode,
        "default",
        other,
        live(0),
    );

    let routed = SessionRouter::new()
        .choose(
            RoutingMoment::SessionStart,
            None,
            &[corrected, untouched],
            &RouterInputs {
                overrides: &overrides,
                health: &health,
                now,
                requirements: TaskRequirements::default(),
            },
        )
        .expect("destinations were offered");
    assert_eq!(
        routed.chosen().id(),
        "untouched",
        "a user's own correction that this model does not behave the way the harness needs did \
         not reach the ranking"
    );
}

/// Nothing here reads a clock, so a router asked the same question twice with
/// the same `now` answers the same way. The value is that a decision can be
/// reproduced from a log rather than from when it was asked.
#[test]
fn the_router_is_a_pure_function_of_its_arguments() {
    let fixture = Fixture::new();
    let destinations = vec![
        Destination::existing(
            "a",
            IntegrationId::ClaudeCode,
            "default",
            backend("anthropic", "claude-opus-4", "KEY_A"),
            live(120),
        ),
        Destination::fresh(
            "b",
            IntegrationId::ClaudeCode,
            "default",
            backend("anthropic", "claude-opus-4", "KEY_B"),
            Some(CheckpointQuality::new(true, true)),
        ),
    ];
    let router = SessionRouter::new();
    let first = router
        .choose(
            RoutingMoment::SessionStart,
            None,
            &destinations,
            &fixture.inputs(),
        )
        .expect("destinations were offered");
    let again = router
        .choose(
            RoutingMoment::SessionStart,
            None,
            &destinations,
            &fixture.inputs(),
        )
        .expect("destinations were offered");
    assert_eq!(first, again);
    let _ = Duration::from_secs(0);
}
