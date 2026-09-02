//! `GH-CANDIDATE-PROOFS` — proof-only regression tests for map lines
//! 1513 (protocol/tool half), 1520 (entitlement half) and 1521 (end to
//! end), reached through the library's public API rather than
//! `main.rs`'s private generators. See `.agent-runtime/report-recon-35a.md`
//! Cause 3 for the production caller chain each test pins.

use std::collections::BTreeMap;
use std::time::Instant;

use glasshouse::config::{EffectiveConfig, ProfileConfig, UserConfig};
use glasshouse::harness::pairing::PairingOverrides;
use glasshouse::integrations::IntegrationId;
use glasshouse::routing::free::FreePool;
use glasshouse::routing::session::{
    Destination, RouterInputs, RoutingMoment, SessionRouter, TaskRequirements,
};
use glasshouse::routing::{
    AssignedModel, Backend, Cost, CredentialId, Entitlement, EntitlementRules, HardConstraint,
    ToolSemantics,
};
use glasshouse::secret::SecretRef;

fn no_overrides() -> PairingOverrides {
    PairingOverrides::from_parts("no configuration", BTreeMap::new(), BTreeMap::new())
}

fn credential(provider: &str) -> CredentialId {
    CredentialId::new(
        provider,
        SecretRef::Environment {
            var: format!("{}_KEY", provider.to_uppercase()),
        },
    )
}

fn backend(provider: &str, protocol: &str, tools: ToolSemantics) -> Backend {
    Backend::new(
        provider,
        protocol,
        AssignedModel::named("some-model"),
        credential(provider),
        Cost::Metered,
        tools,
    )
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

    fn inputs(&self, needs_tool_calls: bool) -> RouterInputs<'_> {
        RouterInputs {
            overrides: &self.overrides,
            health: &self.health,
            now: self.now,
            requirements: TaskRequirements {
                needs_tool_calls,
                ..TaskRequirements::default()
            },
        }
    }
}

// ---------------------------------------------------------------------
// 1513 — the protocol/tool half of "fresh gateway-backed candidates only
// as installed-harness profiles whose protocol and tool semantics match".
//
// The gate (`session::hard_constraint`) is not gateway-specific code: it
// applies uniformly to every `Destination`, regardless of which backend
// produced it. These tests exercise that uniform gate directly through
// `SessionRouter`, which is exactly how a gateway-backed destination
// built by `main.rs::destination_backend` would be checked.
// ---------------------------------------------------------------------

/// `IntegrationId::OpenCode` declares only `WireProtocol::OpenAiChat`
/// (`harness/opencode.rs`), and the gateway translation table has no
/// `openai-chat -> anthropic-messages` pair (`gateway/translate/mod.rs`,
/// `PairStatus::Refused(NOT_YET_REVERSE)`), so a destination whose backend
/// serves `anthropic-messages` is protocol-incompatible for it. The
/// census's mutation (drop the protocol check for a gateway-backed
/// destination) would let this candidate reach scoring instead.
#[test]
fn a_protocol_incompatible_destination_is_excluded_before_scoring_1513() {
    let fixture = Fixture::new();
    let incompatible = Destination::fresh(
        "incompatible",
        IntegrationId::OpenCode,
        "gateway",
        backend(
            "some-gateway",
            "anthropic-messages",
            ToolSemantics::Verified,
        ),
        None,
    );

    let inputs = fixture.inputs(false);
    let rejected = SessionRouter::new().refused(std::slice::from_ref(&incompatible), &inputs);

    assert_eq!(
        rejected.len(),
        1,
        "the protocol-incompatible destination must be hard-refused, not scored"
    );
    assert_eq!(rejected[0].0.id(), "incompatible");
    assert_eq!(
        rejected[0].1,
        HardConstraint::Protocol,
        "the refusal must name the protocol constraint specifically"
    );

    // The same gate lets a compatible destination through, proving the
    // exclusion above is about the protocol and not about something else
    // this test accidentally also changed.
    let compatible = Destination::fresh(
        "compatible",
        IntegrationId::OpenCode,
        "gateway",
        backend("some-gateway", "openai-chat", ToolSemantics::Verified),
        None,
    );
    let routed = SessionRouter::new()
        .choose(
            RoutingMoment::SessionStart,
            None,
            std::slice::from_ref(&compatible),
            &inputs,
        )
        .expect("a protocol-compatible destination must be chosen when it is the only one");
    assert_eq!(routed.chosen().id(), "compatible");
}

/// The tool-semantics half of the same gate
/// (`session.rs:4838-4842`): a task that needs tool calls cannot be sent to
/// a destination established **not** to carry them, uniformly across
/// backend types. The census's mutation drops this check for a
/// gateway-backed destination specifically.
#[test]
fn a_tool_incompatible_destination_is_excluded_when_the_task_needs_tool_calls_1513() {
    let fixture = Fixture::new();
    let no_tools = Destination::fresh(
        "no-tools",
        IntegrationId::OpenCode,
        "gateway",
        backend("some-gateway", "openai-chat", ToolSemantics::KnownAbsent),
        None,
    );

    let inputs = fixture.inputs(true);
    let rejected = SessionRouter::new().refused(std::slice::from_ref(&no_tools), &inputs);

    assert_eq!(rejected.len(), 1);
    assert_eq!(
        rejected[0].1,
        HardConstraint::ToolSemantics,
        "a task needing tool calls must refuse a destination established not to carry them"
    );
}

// ---------------------------------------------------------------------
// 1520 — the entitlement half of "exclude candidates explicitly disabled
// or forbidden by user policy". The generation-time half (a disabled
// profile never reaches the offered set) is pinned in `main.rs`'s own
// test module; this is the post-generation hard exclusion
// (`Entitlement::constraint`, `session.rs:4818`, called from
// `hard_constraint`).
// ---------------------------------------------------------------------

/// A destination backed by an entitlement whose rules deny this harness is
/// excluded outright — refused, never merely scored lower. The census's
/// mutation (bypass `Entitlement::constraint`'s deny check) would let a
/// denied harness reach scoring instead of being refused.
#[test]
fn a_destination_backed_by_a_harness_denying_entitlement_is_excluded_not_scored_1520() {
    let fixture = Fixture::new();
    let denied_rules = EntitlementRules::UNRESTRICTED.deny_harnesses([IntegrationId::OpenCode]);
    let denied = Destination::fresh(
        "denied",
        IntegrationId::OpenCode,
        "gateway",
        backend("some-gateway", "openai-chat", ToolSemantics::Verified),
        None,
    )
    .with_entitlement(Some(Entitlement::new("policy-test", denied_rules)));

    let inputs = fixture.inputs(false);
    let rejected = SessionRouter::new().refused(std::slice::from_ref(&denied), &inputs);

    assert_eq!(
        rejected.len(),
        1,
        "a candidate whose entitlement forbids this harness must be excluded, not merely \
         disfavoured by scoring"
    );
    assert!(
        matches!(rejected[0].1, HardConstraint::Entitlement { .. }),
        "the exclusion must be attributed to the entitlement rule: {:?}",
        rejected[0].1
    );

    // The same rules admit a harness they do not deny, proving the
    // exclusion above is about the policy and not about entitlements
    // refusing everything unconditionally.
    let admitted_rules = EntitlementRules::UNRESTRICTED.deny_harnesses([IntegrationId::Codex]);
    let admitted = Destination::fresh(
        "admitted",
        IntegrationId::OpenCode,
        "gateway",
        backend("some-gateway", "openai-chat", ToolSemantics::Verified),
        None,
    )
    .with_entitlement(Some(Entitlement::new("policy-test", admitted_rules)));
    let routed = SessionRouter::new()
        .choose(
            RoutingMoment::SessionStart,
            None,
            std::slice::from_ref(&admitted),
            &inputs,
        )
        .expect("a candidate whose entitlement does not deny this harness must be chosen");
    assert_eq!(routed.chosen().id(), "admitted");
}

// ---------------------------------------------------------------------
// 1521 — "keep at least one deterministic fallback candidate when a
// usable native session exists". The guarantee lives in
// `EffectiveConfig::profile_enabled`'s unconditional short-circuit for
// the implied Native profile (`config/mod.rs:5045-5048`), confirmed here
// end to end: even a user who writes a `[profiles.native]` entry with
// `enabled = false` — which does reach the profile table, since nothing
// stops that key from being configured — must still see it enabled, and
// a destination built from it must still survive `SessionRouter::choose`
// against a task with no tier requirement (per the packet: "usable" is
// doing real work in the line's own wording, so the proof stays inside
// what the line actually promises).
// ---------------------------------------------------------------------
#[test]
fn the_native_profile_survives_as_a_deterministic_fallback_end_to_end_1521() {
    let harness = IntegrationId::ClaudeCode;

    let mut user = UserConfig::default();
    let mut attempted_disable = ProfileConfig::new(harness);
    attempted_disable.set_enabled(false);
    user.profiles_mut()
        .set(glasshouse::profile::NATIVE_PROFILE_NAME, attempted_disable);

    let effective = EffectiveConfig::new(&user, None);
    assert!(
        effective
            .profile_enabled(glasshouse::profile::NATIVE_PROFILE_NAME)
            .value,
        "the implied Native profile must stay enabled even against a user config entry that \
         tries to disable it"
    );

    // A destination built the way `main.rs::destination_backend`'s
    // `BackendResource::Native` arm builds one: the harness's own
    // sign-in, always protocol/tool-compatible with its own harness.
    let native = Destination::fresh(
        "native",
        harness,
        glasshouse::profile::NATIVE_PROFILE_NAME,
        backend(
            harness.slug(),
            "anthropic-messages",
            ToolSemantics::Verified,
        ),
        None,
    );

    let fixture = Fixture::new();
    // No tier requirement — the packet's own instruction: the line does
    // not claim a profile whose ceiling is below a classified minimum
    // survives, so the proof stays inside what it actually promises.
    let inputs = fixture.inputs(false);
    let routed = SessionRouter::new()
        .choose(
            RoutingMoment::SessionStart,
            None,
            std::slice::from_ref(&native),
            &inputs,
        )
        .expect("the Native destination must remain a candidate through choose");
    assert_eq!(
        routed.chosen().id(),
        "native",
        "the deterministic fallback must be the one chosen when it is the only candidate"
    );
}
