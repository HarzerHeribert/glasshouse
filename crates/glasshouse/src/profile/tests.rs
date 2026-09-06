use super::*;
use crate::harness::{Declared, adapter_for};
use crate::provider::ProtocolSupport;

fn profile_for(harness: IntegrationId) -> LaunchProfile {
    LaunchProfile::native(harness)
}

/// A [`SecretStore`] holding values in memory.
///
/// Deliberately not [`crate::secret::EnvironmentSecretStore`]: a test
/// that set a real environment variable would make the credential
/// visible to every other test in this process and to anything that
/// inspected it, which is exactly the exposure this phase exists to
/// prevent. It also keeps these tests free of `std::env`, which
/// `harness::resolving_a_launch_profile_touches_no_files` forbids in
/// this module's production code.
struct FakeSecrets(Vec<(String, String)>);

impl FakeSecrets {
    fn empty() -> Self {
        Self(Vec::new())
    }

    fn holding(var: &str, value: &str) -> Self {
        Self(vec![(var.to_owned(), value.to_owned())])
    }
}

impl crate::secret::SecretStore for FakeSecrets {
    fn resolve(&self, reference: &SecretRef) -> Option<crate::secret::Secret> {
        // Forced by `SecretRef` gaining `OsCredential`: this fake holds
        // variable names, so a reference naming the OS store is one it
        // has nothing to answer with. No production line in this module
        // changed.
        let SecretRef::Environment { var } = reference else {
            return None;
        };
        self.0
            .iter()
            .find(|(name, _)| name == var)
            .map(|(_, value)| crate::secret::Secret::mint_for_test(value))
    }

    fn is_present(&self, reference: &SecretRef) -> bool {
        let SecretRef::Environment { var } = reference else {
            return false;
        };
        self.0.iter().any(|(name, _)| name == var)
    }

    fn describe(&self) -> &'static str {
        "in-memory test store"
    }
}

/// The context every pre-9F test used implicitly: one adapter, no
/// provider, no credential.
fn native_cx<'a>(
    adapter: &'a dyn HarnessAdapter,
    acknowledged_bypass: bool,
    secrets: &'a dyn SecretStore,
) -> Resolution<'a> {
    Resolution {
        adapter,
        acknowledged_bypass,
        provider: None,
        secrets,
    }
}

fn provider_serving(name: &str, protocol: WireProtocol, base_url: &str) -> Provider {
    Provider {
        name: name.to_owned(),
        protocols: vec![ProtocolSupport {
            protocol,
            base_url: base_url.to_owned(),
            streaming: Declared::Unverified,
            tool_calls: Declared::Unverified,
            reasoning: Declared::Unverified,
        }],
        model_list_endpoint: Declared::Unverified,
        usage_telemetry: Declared::Unverified,
        credential_env: Vec::new(),
        headers: Vec::new(),
    }
}

fn direct_profile(harness: IntegrationId, provider: &str) -> LaunchProfile {
    let mut profile = LaunchProfile::native(harness);
    profile.name = "gateway".to_owned();
    profile.backend = BackendResource::DirectProvider {
        provider: provider.to_owned(),
    };
    profile
}

fn env_value<'a>(overlay: &'a LaunchOverlay, key: &str) -> Option<&'a std::ffi::OsStr> {
    overlay
        .env()
        .iter()
        .find(|(name, _)| name == std::ffi::OsStr::new(key))
        .map(|(_, value)| value.as_os_str())
}

fn rendered_args(overlay: &LaunchOverlay) -> Vec<String> {
    overlay
        .args()
        .iter()
        .map(|a| a.to_string_lossy().into_owned())
        .collect()
}

// --- 1. every harness has a Native profile that adds nothing ---------

#[test]
fn a_native_profile_exists_for_every_harness_and_adds_nothing() {
    for &id in IntegrationId::ALL {
        let Some(adapter) = adapter_for(id) else {
            continue;
        };
        let profile = LaunchProfile::native(id);
        assert_eq!(profile.harness, id);
        assert_eq!(profile.backend, BackendResource::Native);
        assert_eq!(profile.class(), ProfileClass::NativeSubscription);

        let overlay = resolve(&profile, &native_cx(adapter, false, &FakeSecrets::empty()))
            .unwrap_or_else(|err| panic!("{}'s native profile must resolve: {err}", id.slug()));

        // Whether or not the harness declares automatic review, the
        // *native* profile only ever contributes what the harness's own
        // automatic-review mode would add — for the harnesses that have
        // none, that is nothing at all.
        let expects_args = adapter
            .approval_args(ApprovalKind::AutomaticReview)
            .is_some();
        assert_eq!(
            !overlay.args().is_empty(),
            expects_args,
            "{}'s native profile args did not match its declared automatic review",
            id.slug()
        );
        assert!(
            overlay.env().is_empty(),
            "{} native profile added env",
            id.slug()
        );
    }
}

// --- 2. explicit automatic review, refused where none exists ---------

#[test]
fn an_explicit_automatic_review_request_is_refused_on_a_harness_without_one() {
    // OpenCode declares no automatic review (only a blanket `--auto`).
    let adapter = adapter_for(IntegrationId::OpenCode).expect("a harness");
    let mut profile = profile_for(IntegrationId::OpenCode);
    profile.approval = ApprovalSelection::AutomaticReview;

    let err = resolve(&profile, &native_cx(adapter, false, &FakeSecrets::empty()))
        .expect_err("must be refused");
    match &err {
        Refusal::NoAutomaticReview {
            profile: name,
            harness,
        } => {
            assert_eq!(name, NATIVE_PROFILE_NAME);
            assert_eq!(*harness, IntegrationId::OpenCode);
        }
        other => panic!("expected NoAutomaticReview, got {other:?}"),
    }
    let message = err.to_string();
    assert!(message.contains("OpenCode"), "{message}");
    assert!(message.contains("automatic-review"), "{message}");
}

// --- 3. a defaulted profile adds no approval argument on such a harness

#[test]
fn a_defaulted_profile_on_such_a_harness_adds_no_approval_argument() {
    let adapter = adapter_for(IntegrationId::OpenCode).expect("a harness");
    let profile = profile_for(IntegrationId::OpenCode); // ApprovalSelection::Default

    let overlay = resolve(&profile, &native_cx(adapter, false, &FakeSecrets::empty())).unwrap();
    assert!(
        overlay.args().is_empty(),
        "a defaulted profile must add no approval argument at all: {:?}",
        overlay.args()
    );

    // Explicitly: not the bypass argument either, acknowledged or not.
    let bypass_args = adapter
        .approval_args(ApprovalKind::Bypass)
        .expect("OpenCode declares a bypass mode");
    for arg in &bypass_args {
        assert!(
            !overlay
                .args()
                .iter()
                .any(|a| a == std::ffi::OsStr::new(arg)),
            "a defaulted profile must never carry the bypass argument `{arg}`"
        );
    }

    let overlay_acknowledged =
        resolve(&profile, &native_cx(adapter, true, &FakeSecrets::empty())).unwrap();
    assert!(overlay_acknowledged.args().is_empty());
}

// --- 4. bypass refused until acknowledged, per harness ----------------

#[test]
fn a_bypass_is_refused_until_it_is_acknowledged_for_that_harness() {
    let adapter = adapter_for(IntegrationId::Hermes).expect("a harness");
    let mut profile = profile_for(IntegrationId::Hermes);
    profile.approval = ApprovalSelection::Bypass;

    let err = resolve(&profile, &native_cx(adapter, false, &FakeSecrets::empty()))
        .expect_err("unacknowledged bypass is refused");
    let description = match &err {
        Refusal::BypassNotAcknowledged {
            profile: name,
            harness,
            description,
        } => {
            assert_eq!(name, NATIVE_PROFILE_NAME);
            assert_eq!(*harness, IntegrationId::Hermes);
            *description
        }
        other => panic!("expected BypassNotAcknowledged, got {other:?}"),
    };
    assert!(!description.is_empty());
    assert!(err.to_string().contains(description));

    let overlay = resolve(&profile, &native_cx(adapter, true, &FakeSecrets::empty()))
        .expect("acknowledged bypass resolves");
    let expected_args = adapter.approval_args(ApprovalKind::Bypass).unwrap();
    let rendered: Vec<String> = overlay
        .args()
        .iter()
        .map(|a| a.to_string_lossy().into_owned())
        .collect();
    assert_eq!(rendered, expected_args);

    // Acknowledging Hermes must not acknowledge a different harness.
    let other_adapter = adapter_for(IntegrationId::Antigravity).expect("a harness");
    let mut other_profile = profile_for(IntegrationId::Antigravity);
    other_profile.approval = ApprovalSelection::Bypass;
    let err = resolve(
        &other_profile,
        &native_cx(other_adapter, false, &FakeSecrets::empty()),
    )
    .expect_err("Hermes's acknowledgement must not carry over to Antigravity");
    assert!(matches!(err, Refusal::BypassNotAcknowledged { .. }));
}

// --- 5. the gateway backend, and what it resolves into ---------------

/// The gateway is a *process a caller started*, so a call site with none
/// to offer cannot resolve a profile that needs one. It refuses by
/// saying exactly that, and starts nothing.
///
/// This is also what keeps [`resolve`]'s one-argument form honest: it
/// forwards `None`, so every existing caller behaves as it always did.
#[test]
fn a_gateway_backed_profile_is_refused_when_no_gateway_is_running() {
    let adapter = adapter_for(IntegrationId::ClaudeCode).expect("a harness");
    let mut profile = profile_for(IntegrationId::ClaudeCode);
    profile.backend = BackendResource::GlasshouseGateway;

    let err = resolve(&profile, &native_cx(adapter, false, &FakeSecrets::empty()))
        .expect_err("no gateway was supplied");
    match &err {
        Refusal::GatewayNotRunning { harness, .. } => {
            assert_eq!(*harness, IntegrationId::ClaudeCode);
        }
        other => panic!("expected GatewayNotRunning, got {other:?}"),
    }
    let message = err.to_string();
    assert!(message.contains("Claude Code"), "{message}");
    assert!(message.contains("gateway"), "{message}");
}

/// Phase 9H lines 505, 506 and 507: resolving a gateway-backed profile —
/// which is what `main.rs`'s `launch_session` does, through this exact
/// function — is what gives the session its backend assignment.
///
/// **This test exists because a mutation survived without it.** Deleting
/// `apply_gateway`'s call to `Gateway::routing().bind` broke nothing: the
/// gateway's own conformance tests bind the assignment themselves, so
/// every one of them passed against a build in which the production
/// launch path recorded no assignment at all. A capability whose only
/// caller can be deleted silently does not have a caller.
#[test]
fn resolving_a_gateway_backed_profile_assigns_the_session_a_provider_and_a_model() {
    let adapter = adapter_for(IntegrationId::ClaudeCode).expect("a harness");
    let gateway = gateway_serving(&[WireProtocol::AnthropicMessages]);
    let mut profile = profile_for(IntegrationId::ClaudeCode);
    profile.backend = BackendResource::GlasshouseGateway;
    profile.model = Some("a-named-model".to_owned());

    assert!(
        gateway.routing().assignment().is_none(),
        "a gateway that no profile has resolved through has assigned nothing"
    );

    let overlay = resolve_with_gateway(
        &profile,
        &native_cx(adapter, false, &FakeSecrets::empty()),
        Some(&gateway),
        &GatewayPairing::default(),
    )
    .expect("the gateway serves the protocol this harness speaks");

    let assignment = gateway
        .routing()
        .assignment()
        .expect("resolving a gateway-backed profile assigns its backend");
    assert_eq!(assignment.harness(), IntegrationId::ClaudeCode.slug());
    assert_eq!(assignment.provider(), "fixture");
    assert_eq!(
        assignment.protocol(),
        WireProtocol::AnthropicMessages.slug()
    );
    assert_eq!(
        assignment.backend().model(),
        &AssignedModel::named("a-named-model")
    );

    // And the choice is announced rather than silent — the argument
    // `gateway_upstream`'s own documentation rests on.
    let announced = overlay
        .mechanisms
        .iter()
        .find(|note| note.category == "gateway backend")
        .expect("the assignment is reported in the launch's mechanism notes");
    assert!(announced.detail.contains("a-named-model"), "{announced:?}");
    assert!(announced.detail.contains("fixture"), "{announced:?}");
    assert!(
        !announced.detail.contains(PLANTED_CREDENTIAL),
        "a mechanism note must name a credential and never carry one: {announced:?}"
    );

    // Phase 32: the gateway records its own resource kind too, and it is
    // named as delegated rather than as a flat "metered" — the gateway
    // is a router, and this session's own upstream is `ollama`-shaped
    // fixtures elsewhere in this suite, which is exactly the case a
    // blanket `MeteredBalance` would get wrong.
    let kind = overlay
        .mechanisms
        .iter()
        .find(|note| note.category == "resource kind")
        .expect("a gateway-backed launch records its resource kind");
    assert!(kind.detail.contains("glasshouse gateway"), "{kind:?}");
    assert!(kind.detail.contains("delegated"), "{kind:?}");
}

/// Phase 32, line 1185: a direct-provider launch records whether it
/// resolved to local inference or a remote one, and the two say
/// different things about quota — this is the registry's classification
/// actually reaching the launch path, not merely existing as a type.
/// Phase 32 line 1184 — a native subscription is represented separately
/// from an API-key or gateway resource, **on a real launch** and not only
/// as a type.
///
/// Phase 32A's audit found this arm recording nothing while the other two
/// recorded their kind, so `ResourceKind::NativeSubscription` was
/// constructed nowhere outside tests. A distinction the shipped binary
/// never draws is not one it makes, and this is the test that keeps the
/// arm honest — deleting the push in `resolve` fails here and nowhere else.
#[test]
fn resolving_a_native_profile_records_it_as_a_subscription_resource() {
    let adapter = adapter_for(IntegrationId::ClaudeCode).expect("a harness");
    let secrets = FakeSecrets::empty();
    let profile = profile_for(IntegrationId::ClaudeCode);

    let overlay =
        resolve(&profile, &native_cx(adapter, false, &secrets)).expect("a native profile resolves");
    let kind = overlay
        .mechanisms
        .iter()
        .find(|note| note.category == "resource kind")
        .expect("a native launch records its resource kind");

    // Its quota is a rolling window, which is precisely what must not be
    // flattened into the metered balance a direct provider reports.
    assert!(kind.detail.contains("rolling"), "{kind:?}");
    assert!(
        !kind.detail.contains("metered balance"),
        "a subscription must not claim a metered balance: {kind:?}"
    );
}

#[test]
fn resolving_a_direct_provider_profile_records_whether_it_is_local_or_remote() {
    let adapter = adapter_for(IntegrationId::ClaudeCode).expect("a harness");
    let secrets = FakeSecrets::empty();

    let local = provider_serving(
        "ollama",
        WireProtocol::AnthropicMessages,
        "http://localhost:11434/v1",
    );
    let profile = direct_profile(IntegrationId::ClaudeCode, &local.name);
    let overlay = resolve(&profile, &direct_cx(adapter, &local, &secrets))
        .expect("a provider with no credential variable still resolves");
    let kind = overlay
        .mechanisms
        .iter()
        .find(|note| note.category == "resource kind")
        .expect("a direct-provider launch records its resource kind");
    assert!(kind.detail.contains("local"), "{kind:?}");
    assert!(kind.detail.contains("unmetered"), "{kind:?}");

    let remote = provider_serving(
        "openrouter",
        WireProtocol::AnthropicMessages,
        "https://openrouter.ai/api",
    );
    let profile = direct_profile(IntegrationId::ClaudeCode, &remote.name);
    let overlay = resolve(&profile, &direct_cx(adapter, &remote, &secrets))
        .expect("a provider with no credential variable still resolves");
    let kind = overlay
        .mechanisms
        .iter()
        .find(|note| note.category == "resource kind")
        .expect("a direct-provider launch records its resource kind");
    assert!(kind.detail.contains("remote"), "{kind:?}");
    assert!(kind.detail.contains("metered balance"), "{kind:?}");
}

/// A profile that names no model assigns none, and says so rather than
/// leaving a reader unable to tell "no model" from "we forgot".
#[test]
fn a_gateway_backed_profile_with_no_model_assigns_the_harnesss_own_default() {
    let adapter = adapter_for(IntegrationId::ClaudeCode).expect("a harness");
    let gateway = gateway_serving(&[WireProtocol::AnthropicMessages]);
    let mut profile = profile_for(IntegrationId::ClaudeCode);
    profile.backend = BackendResource::GlasshouseGateway;
    profile.model = None;

    resolve_with_gateway(
        &profile,
        &native_cx(adapter, false, &FakeSecrets::empty()),
        Some(&gateway),
        &GatewayPairing::default(),
    )
    .expect("a profile need not name a model");

    let assignment = gateway.routing().assignment().expect("assigned");
    assert_eq!(assignment.backend().model(), &AssignedModel::HarnessDefault);
    assert!(
        assignment.label().contains("the harness's own default"),
        "{}",
        assignment.label()
    );
}

/// Phase 9H line 518, on the path a real launch takes: a profile that
/// records a pin turns automatic failover off before the session's first
/// request, and says so in the launch's own mechanism notes.
///
/// The pin lives on the profile because that is where a user can state it
/// today — see [`LaunchProfile::pin_gateway_backend`]. A pin nobody can
/// set is not a capability.
#[test]
fn a_profile_that_records_a_pin_turns_automatic_failover_off_at_session_start() {
    let adapter = adapter_for(IntegrationId::ClaudeCode).expect("a harness");
    let gateway = gateway_serving(&[WireProtocol::AnthropicMessages]);
    let mut profile = profile_for(IntegrationId::ClaudeCode);
    profile.backend = BackendResource::GlasshouseGateway;
    profile.pin_gateway_backend = true;

    let overlay = resolve_with_gateway(
        &profile,
        &native_cx(adapter, false, &FakeSecrets::empty()),
        Some(&gateway),
        &GatewayPairing::default(),
    )
    .expect("the gateway serves the protocol this harness speaks");

    assert_eq!(
        gateway.routing().pin().provider(),
        Some("fixture"),
        "a profile that records a pin pins the session it starts"
    );
    let note = overlay
        .mechanisms
        .iter()
        .find(|note| note.category == "gateway pin")
        .expect("a pin is a mechanism worth reporting");
    assert!(note.detail.contains("fixture"), "{note:?}");
    assert!(note.detail.contains("failover is off"), "{note:?}");
}

/// And a profile that records no pin does not pin, so the default is the
/// behaviour every profile written before the field existed already had.
#[test]
fn a_profile_without_a_pin_leaves_automatic_failover_on() {
    let adapter = adapter_for(IntegrationId::ClaudeCode).expect("a harness");
    let gateway = gateway_serving(&[WireProtocol::AnthropicMessages]);
    let mut profile = profile_for(IntegrationId::ClaudeCode);
    profile.backend = BackendResource::GlasshouseGateway;

    let overlay = resolve_with_gateway(
        &profile,
        &native_cx(adapter, false, &FakeSecrets::empty()),
        Some(&gateway),
        &GatewayPairing::default(),
    )
    .expect("resolvable");

    assert_eq!(gateway.routing().pin().provider(), None);
    assert!(
        !overlay
            .mechanisms
            .iter()
            .any(|note| note.category == "gateway pin")
    );
}

/// A running gateway serving `protocols`, for the tests below. Its
/// upstream never has to answer: resolution reads the gateway's address
/// and token and opens no connection at all.
///
/// Which protocols it serves is the parameter because that is now the
/// thing under test: a gateway serves what its one configured provider
/// declared a base URL for, and `apply_gateway` refuses against exactly
/// that.
fn gateway_serving(protocols: &[WireProtocol]) -> crate::gateway::Gateway {
    let profiles = [{
        let mut profile = profile_for(IntegrationId::ClaudeCode);
        profile.backend = BackendResource::GlasshouseGateway;
        profile
    }];
    crate::gateway::start_if_required(&profiles, || {
        Ok(Upstream::new(
            "fixture".to_owned(),
            protocols
                .iter()
                .map(|protocol| {
                    Route::new(
                        protocol.slug().to_owned(),
                        ingress_targets(*protocol),
                        "https://provider.example/api",
                    )
                })
                .collect(),
            crate::secret::Secret::mint_for_test(PLANTED_CREDENTIAL),
            crate::routing::CredentialId::new(
                "fixture",
                crate::secret::SecretRef::Environment {
                    var: "FIXTURE_API_KEY".to_owned(),
                },
            ),
        )?)
    })
    .expect("loopback is bindable")
    .expect("a gateway-backed profile asks for a gateway")
}

/// A running gateway serving the protocol Claude Code speaks — the shape
/// every test written before the ingress served more than one assumes.
fn running_gateway() -> crate::gateway::Gateway {
    gateway_serving(&[WireProtocol::AnthropicMessages])
}

/// Phase 9G's OpenAI Responses ingress, at the resolution layer: a
/// gateway whose provider serves Responses **resolves a Codex profile**,
/// which is the line's whole point.
///
/// Codex 0.149.1 removed `wire_api = "chat"` — confirmed against the
/// installed binary, which answers
/// ``Error loading config.toml: `wire_api = "chat"` is no longer
/// supported.`` — so Responses is the only protocol that can ever back a
/// Codex profile, and this ingress is therefore the only gateway path to
/// one. The same binary pointed at a path-less base URL was observed
/// sending `POST /responses`, which is why
/// [`ingress_targets`] declares the bare form.
///
/// Lose this and the Responses ingress can exist in the gateway while
/// remaining unreachable from the only harness that speaks it.
#[test]
fn a_gateway_serving_responses_resolves_a_codex_profile() {
    let adapter = adapter_for(IntegrationId::Codex).expect("a harness");
    let gateway = gateway_serving(&[WireProtocol::OpenAiResponses]);
    let mut profile = profile_for(IntegrationId::Codex);
    profile.backend = BackendResource::GlasshouseGateway;

    let overlay = resolve_with_gateway(
        &profile,
        &native_cx(adapter, false, &FakeSecrets::empty()),
        Some(&gateway),
        &GatewayPairing::default(),
    )
    .expect("a Codex profile resolves against a gateway that serves Responses");

    let rendered = format!("{:?}", overlay.args());
    assert!(
        rendered.contains(&format!("http://{}", gateway.address())),
        "the child was not pointed at this gateway: {rendered}"
    );
    assert!(
        rendered.contains("responses"),
        "the child was not configured for the Responses wire API: {rendered}"
    );

    // The gateway's own token reaches the child, and the provider
    // credential the gateway holds does not — the same rule the Claude
    // Code path already carries, asserted again on the path that did not
    // exist when it was written.
    let env: Vec<(String, String)> = overlay
        .env()
        .iter()
        .map(|(key, value)| {
            (
                key.to_string_lossy().into_owned(),
                value.to_string_lossy().into_owned(),
            )
        })
        .collect();
    assert!(
        env.iter()
            .any(|(_, value)| value == gateway.token().expose()),
        "the gateway's token did not reach the child: {env:?}"
    );
    for (key, value) in &env {
        assert!(
            !value.contains(PLANTED_CREDENTIAL),
            "the provider credential reached the child in {key}"
        );
    }
}

/// The pane adapter declares the Anthropic Messages protocol and the two
/// environment names its wire reads, so a gateway-backed pane profile
/// resolves exactly as a Claude Code one does: the child is pointed at this
/// gateway through `ANTHROPIC_BASE_URL`, the gateway's own token reaches it
/// as `ANTHROPIC_AUTH_TOKEN`, and the provider credential the gateway holds
/// does not.
#[test]
fn a_gateway_serving_messages_resolves_a_pane_profile() {
    let adapter = adapter_for(IntegrationId::Pane).expect("a harness");
    let gateway = running_gateway();
    let mut profile = profile_for(IntegrationId::Pane);
    profile.backend = BackendResource::GlasshouseGateway;

    let overlay = resolve_with_gateway(
        &profile,
        &native_cx(adapter, false, &FakeSecrets::empty()),
        Some(&gateway),
        &GatewayPairing::default(),
    )
    .expect("a pane profile resolves against a gateway that serves Messages");

    let env: Vec<(String, String)> = overlay
        .env()
        .iter()
        .map(|(key, value)| {
            (
                key.to_string_lossy().into_owned(),
                value.to_string_lossy().into_owned(),
            )
        })
        .collect();
    let base_url = env
        .iter()
        .find(|(key, _)| key == "ANTHROPIC_BASE_URL")
        .map(|(_, value)| value.as_str())
        .unwrap_or_else(|| panic!("the child was not pointed at the gateway: {env:?}"));
    assert!(
        base_url.starts_with(&format!("http://{}", gateway.address())),
        "the child was pointed elsewhere: {base_url}"
    );
    let token = env
        .iter()
        .find(|(key, _)| key == "ANTHROPIC_AUTH_TOKEN")
        .map(|(_, value)| value.as_str());
    assert_eq!(
        token,
        Some(gateway.token().expose()),
        "the gateway's token did not reach the child as the bearer: {env:?}"
    );
    for (key, value) in &env {
        assert!(
            !value.contains(PLANTED_CREDENTIAL),
            "the provider credential reached the child in {key}"
        );
    }
}

/// A gateway serving every protocol its ingress knows how to carry
/// resolves both harnesses that declare one — and hands each the
/// protocol it actually speaks, never the first one the list happens to
/// name.
///
/// Lose this and `apply_gateway` can go back to picking
/// `GATEWAY_INGRESS_PROTOCOLS[0]`, which now silently means "Anthropic
/// Messages for everyone".
#[test]
fn each_harness_is_given_the_protocol_it_speaks_not_the_first_one_served() {
    let gateway = gateway_serving(GATEWAY_INGRESS_PROTOCOLS);
    assert_eq!(
        gateway.served_protocols(),
        vec![
            "anthropic-messages",
            "openai-responses",
            "openai-chat",
            "gemini-generate-content",
        ],
        "this test proves nothing unless the gateway really serves all four"
    );

    for (harness, expected) in [
        (IntegrationId::ClaudeCode, "anthropic-messages"),
        (IntegrationId::Codex, "openai-responses"),
    ] {
        let adapter = adapter_for(harness).expect("a harness");
        let mut profile = profile_for(harness);
        profile.backend = BackendResource::GlasshouseGateway;

        let overlay = resolve_with_gateway(
            &profile,
            &native_cx(adapter, false, &FakeSecrets::empty()),
            Some(&gateway),
            &GatewayPairing::default(),
        )
        .unwrap_or_else(|err| panic!("{harness:?} did not resolve: {err}"));

        let note = overlay
            .mechanisms()
            .iter()
            .find(|note| note.category == "glasshouse gateway")
            .unwrap_or_else(|| panic!("{harness:?} recorded no gateway mechanism"));
        assert!(
            note.detail.contains(expected),
            "{harness:?} was given the wrong protocol: {}",
            note.detail
        );
    }
}

/// The target table and the protocol list are two halves of one fact,
/// and nothing else checks that they agree.
///
/// [`ingress_targets`] is a `match` on [`WireProtocol`], so the compiler
/// already refuses to let a protocol go unlisted. What it cannot check
/// is that each entry is **non-empty** and **distinct** — a protocol
/// whose targets were an empty slice would be declared served and would
/// place no request at all, and two protocols sharing a prefix would
/// make routing depend on declaration order.
#[test]
fn the_ingress_target_table_covers_every_protocol_the_gateway_serves() {
    let mut seen: Vec<&str> = Vec::new();
    for protocol in GATEWAY_INGRESS_PROTOCOLS {
        let targets = ingress_targets(*protocol);
        assert!(
            !targets.is_empty(),
            "{protocol} declares no request target, so nothing could ever be routed to it"
        );
        for target in targets {
            assert!(
                target.starts_with('/'),
                "{protocol}'s target {target:?} is not a path"
            );
            assert!(
                !seen.contains(target),
                "{target:?} is declared by two protocols, so routing would depend on the \
                 order they happen to be listed in"
            );
            seen.push(target);
        }
    }
    assert_eq!(GATEWAY_INGRESS_PROTOCOLS.len(), 4);
}

/// Phase 9G's line 1 for Claude Code, end to end at the resolution
/// layer: a gateway-backed profile **resolves**, and the child is
/// pointed at the local gateway with the gateway's own token.
///
/// The two environment variables are asserted by name and by value
/// because both are the capability. `ANTHROPIC_BASE_URL` pointing
/// anywhere but this gateway would send the user's prompts somewhere
/// nobody chose; `ANTHROPIC_AUTH_TOKEN` holding anything but the
/// gateway's token would either fail authentication or — much worse —
/// be the provider key this whole phase exists to keep out of the child.
#[test]
fn a_gateway_backed_claude_code_profile_resolves_into_the_local_gateway() {
    let adapter = adapter_for(IntegrationId::ClaudeCode).expect("a harness");
    let gateway = running_gateway();
    let mut profile = profile_for(IntegrationId::ClaudeCode);
    profile.backend = BackendResource::GlasshouseGateway;

    let overlay = resolve_with_gateway(
        &profile,
        &native_cx(adapter, false, &FakeSecrets::empty()),
        Some(&gateway),
        &GatewayPairing::default(),
    )
    .expect("a gateway-backed Claude Code profile resolves once a gateway is running");

    let env: Vec<(String, String)> = overlay
        .env()
        .iter()
        .map(|(key, value)| {
            (
                key.to_string_lossy().into_owned(),
                value.to_string_lossy().into_owned(),
            )
        })
        .collect();

    assert!(
        env.contains(&(
            "ANTHROPIC_BASE_URL".to_owned(),
            format!("http://{}", gateway.address())
        )),
        "the child was not pointed at this gateway: {env:?}"
    );
    assert!(
        env.contains(&(
            "ANTHROPIC_AUTH_TOKEN".to_owned(),
            gateway.token().expose().to_owned()
        )),
        "the child was not given this gateway's own token"
    );
    // ... and nothing resembling a provider credential went with it.
    assert!(
        !env.iter()
            .any(|(_, value)| value.contains(PLANTED_CREDENTIAL)),
        "a provider credential reached the child of a gateway-backed profile"
    );

    let mechanisms = overlay
        .mechanisms()
        .iter()
        .map(|note| format!("{}: {}", note.category, note.detail))
        .collect::<Vec<_>>()
        .join(" | ");
    assert!(mechanisms.contains("glasshouse gateway"), "{mechanisms}");
    assert!(
        !mechanisms.contains(gateway.token().expose()),
        "a mechanism note carried the gateway token"
    );
}

/// A harness the *running* gateway cannot carry, even through the pair
/// table, is refused rather than pointed at it anyway.
///
/// This test used to fixture Codex against an Anthropic-only gateway —
/// that combination is `GH-GATEWAY-TRANSLATE-T2`'s own supported row
/// (`openai-responses -> anthropic-messages`) now, so it moved to
/// [`a_gateway_serving_only_anthropic_messages_accepts_a_codex_launch_through_translation`]
/// below and this test switched to OpenCode, whose one protocol
/// (`openai-chat`) has no *reverse* pair to Anthropic Messages yet
/// (`openai-chat -> anthropic-messages` stays `PairStatus::Refused`) —
/// genuinely nothing this gateway can carry, table included.
///
/// Lose this and `apply_gateway` starts accepting a `Refused` row as if
/// it were `Supported`, and an OpenCode session comes up pointed at a
/// gateway with no route for it.
#[test]
fn a_harness_that_cannot_speak_the_ingress_protocol_is_refused() {
    let adapter = adapter_for(IntegrationId::OpenCode).expect("a harness");
    let gateway = running_gateway();
    let mut profile = profile_for(IntegrationId::OpenCode);
    profile.backend = BackendResource::GlasshouseGateway;
    profile.model = Some("oc-model".to_owned());

    let err = resolve_with_gateway(
        &profile,
        &native_cx(adapter, false, &FakeSecrets::empty()),
        Some(&gateway),
        &GatewayPairing::default(),
    )
    .expect_err("openai-chat -> anthropic-messages is a Refused row, not Supported");
    match err {
        Refusal::GatewayTranslationRefused { pair, reason, .. } => {
            assert_eq!(pair, "openai-chat->anthropic-messages");
            assert!(!reason.is_empty());
        }
        other => panic!("{other:?}"),
    }
}

/// The other side of the same seam: a harness whose own protocol the
/// pair table translates to a protocol this gateway serves is accepted,
/// speaks its own protocol at the ingress exactly as a native launch
/// would, and binds the session to the *served* backend — Phase 56 lines
/// 1948/1950/1956's launch link, GH-GATEWAY-TRANSLATE-LAUNCH.
///
/// Codex declares only `openai-responses`; this gateway serves only
/// `anthropic-messages`; the table's `openai-responses -> anthropic-messages`
/// row is `Supported` (`GH-GATEWAY-TRANSLATE-T2`'s own end-to-end test).
#[test]
fn a_gateway_serving_only_anthropic_messages_accepts_a_codex_launch_through_translation() {
    let adapter = adapter_for(IntegrationId::Codex).expect("a harness");
    let gateway = running_gateway();
    let mut profile = profile_for(IntegrationId::Codex);
    profile.backend = BackendResource::GlasshouseGateway;

    let overlay = resolve_with_gateway(
        &profile,
        &native_cx(adapter, false, &FakeSecrets::empty()),
        Some(&gateway),
        &GatewayPairing::default(),
    )
    .expect("a translated pair must be accepted, not refused as unserved");

    // The child still speaks its own protocol at the ingress — same
    // wire API a native Responses launch would configure.
    let rendered = format!("{:?}", overlay.args());
    assert!(
        rendered.contains("responses"),
        "the child was not configured for its own Responses wire API: {rendered}"
    );

    // The session is bound to the *served* backend, not to the protocol
    // the child speaks: `as_routing_backend("anthropic-messages", ..)`
    // is what had to succeed for this assignment to exist at all.
    let assignment = gateway
        .routing()
        .assignment()
        .expect("apply_gateway must bind a translated session, not skip it");
    assert_eq!(assignment.provider(), "fixture");
    assert_eq!(assignment.backend().protocol(), "anthropic-messages");
}

/// A profile that explicitly expects a protocol the ingress does not
/// serve is refused too. An explicit ask is a constraint, never a hint —
/// the same rule the direct-provider path applies.
#[test]
fn a_gateway_profile_expecting_another_protocol_is_refused() {
    let adapter = adapter_for(IntegrationId::ClaudeCode).expect("a harness");
    let gateway = running_gateway();
    let mut profile = profile_for(IntegrationId::ClaudeCode);
    profile.backend = BackendResource::GlasshouseGateway;
    profile.expected_protocol = Some(WireProtocol::OpenAiChat);

    let err = resolve_with_gateway(
        &profile,
        &native_cx(adapter, false, &FakeSecrets::empty()),
        Some(&gateway),
        &GatewayPairing::default(),
    )
    .expect_err("Claude Code cannot be pointed at openai-chat at all");
    // Refused by the generic protocol check before the gateway arm is
    // reached — which is the right layer for it, and is asserted so that
    // moving the check does not silently change the message.
    assert!(matches!(err, Refusal::ProtocolMismatch { .. }), "{err:?}");
}

/// Which provider a gateway forwards to is a routing decision, and this
/// phase makes exactly one of them: the single configured provider that
/// serves the ingress protocol. Zero and several are both refusals that
/// name what was found.
#[test]
fn the_gateway_upstream_is_the_one_provider_that_serves_the_ingress() {
    let secrets = FakeSecrets::holding(CREDENTIAL_VAR, PLANTED_CREDENTIAL);

    let mut anthropic = provider_serving(
        "openrouter",
        WireProtocol::AnthropicMessages,
        "https://openrouter.ai/api",
    );
    anthropic.credential_env = vec![CREDENTIAL_VAR.to_owned()];

    let upstream = gateway_upstream(std::slice::from_ref(&anthropic), &secrets, &|_| false)
        .expect("exactly one provider serves the ingress");
    let rendered = format!("{upstream:?}");
    assert!(rendered.contains("openrouter"), "{rendered}");
    assert!(
        !rendered.contains(PLANTED_CREDENTIAL),
        "the upstream's own rendering carried the credential it holds"
    );

    // A provider serving only OpenAI Chat is a candidate now: the
    // ingress serves that protocol too, and this is the line that
    // changed when it started to. Before, it was the example of
    // "serves nothing the ingress offers".
    let mut chat_only = provider_serving("chat", WireProtocol::OpenAiChat, "https://a.example/v1");
    chat_only.credential_env = vec![CREDENTIAL_VAR.to_owned()];
    let chat_upstream = gateway_upstream(std::slice::from_ref(&chat_only), &secrets, &|_| false)
        .expect("a provider serving one ingress protocol backs the gateway");
    assert_eq!(chat_upstream.served_protocols(), vec!["openai-chat"]);

    // Nothing serving the ingress at all: a provider that serves none of
    // the three, and no provider whatsoever.
    let none = provider_serving(
        "unrelated",
        WireProtocol::OpenAiChat,
        // Serving the protocol without declaring where is not serving it.
        "",
    );
    assert!(matches!(
        gateway_upstream(std::slice::from_ref(&none), &secrets, &|_| false),
        Err(GatewayUpstreamRefusal::NoProviderServesTheIngress { .. })
    ));
    assert!(matches!(
        gateway_upstream(&[], &secrets, &|_| false),
        Err(GatewayUpstreamRefusal::NoProviderServesTheIngress { .. })
    ));

    // A provider that serves it but declares no base URL is not a
    // candidate: launching against `""` must never happen, which is the
    // same rule `apply_direct_provider` already applies.
    let mut no_url = provider_serving("no-url", WireProtocol::AnthropicMessages, "");
    no_url.credential_env = vec![CREDENTIAL_VAR.to_owned()];
    assert!(matches!(
        gateway_upstream(&[no_url], &secrets, &|_| false),
        Err(GatewayUpstreamRefusal::NoProviderServesTheIngress { .. })
    ));

    // Several providers is Phase 9H's assignment plus its failover
    // candidates, in configuration order — no longer the refusal Phase 9G
    // answered with. See `gateway_upstream`'s own documentation for why
    // choosing here is legitimate now and was not then.
    let mut second = provider_serving(
        "another-router",
        WireProtocol::AnthropicMessages,
        "https://another.example/api",
    );
    second.credential_env = vec![CREDENTIAL_VAR.to_owned()];
    let several = gateway_upstream(&[anthropic.clone(), second], &secrets, &|_| false)
        .expect("two protocol-compatible providers is an assignment, not a collision");
    assert_eq!(
        several.backends()[0].credential_id().provider(),
        "openrouter",
        "the first configured provider is the one assigned"
    );
    assert_eq!(
        several
            .backends()
            .iter()
            .map(|backend| backend.credential_id().provider().to_owned())
            .collect::<Vec<_>>(),
        vec!["openrouter", "another-router"],
        "the rest are where a real provider failure may move the session"
    );

    // And a provider whose credential variable holds nothing is refused
    // rather than launched without one — the gateway would otherwise
    // forward requests with an empty bearer token and the user would see
    // the provider's own 401.
    match gateway_upstream(&[anthropic], &FakeSecrets::empty(), &|_| false) {
        Err(GatewayUpstreamRefusal::CredentialUnavailable { variables, .. }) => {
            assert_eq!(variables, vec![CREDENTIAL_VAR.to_owned()]);
        }
        other => panic!("expected CredentialUnavailable, got {other:?}"),
    }
}

/// Phase 9I line 532: a provider the caller's `free` closure names is a
/// `Cost::Free` backend; one it does not is still the fail-closed
/// `Cost::Metered` default. Two providers in one call, so the closure is
/// proven to answer per provider rather than for the whole launch.
#[test]
fn a_provider_the_caller_marks_free_backs_the_gateway_at_no_cost() {
    let secrets = FakeSecrets::holding(CREDENTIAL_VAR, PLANTED_CREDENTIAL);

    let mut free_provider = provider_serving(
        "openrouter",
        WireProtocol::AnthropicMessages,
        "https://openrouter.ai/api",
    );
    free_provider.credential_env = vec![CREDENTIAL_VAR.to_owned()];
    let mut metered_provider = provider_serving(
        "another-router",
        WireProtocol::AnthropicMessages,
        "https://another.example/api",
    );
    metered_provider.credential_env = vec![CREDENTIAL_VAR.to_owned()];

    let upstream = gateway_upstream(&[free_provider, metered_provider], &secrets, &|name| {
        name == "openrouter"
    })
    .expect("two protocol-compatible providers is an assignment, not a collision");

    let rendered = format!("{upstream:?}");
    assert!(
        rendered.contains("cost: \"free\""),
        "the provider the closure marked free must back the gateway at no cost: {rendered}"
    );
    assert!(
        rendered.contains("cost: \"metered\""),
        "the provider the closure did not mark must stay the fail-closed default: {rendered}"
    );
}

#[test]
fn an_empty_gateway_candidate_set_names_the_requirement_and_declarations() {
    let declares_chat_without_a_destination =
        provider_serving("chat-only", WireProtocol::OpenAiChat, "");
    let declares_responses_without_a_destination =
        provider_serving("responses-without-url", WireProtocol::OpenAiResponses, "");

    let refusal = gateway_upstream(
        &[
            declares_chat_without_a_destination,
            declares_responses_without_a_destination,
        ],
        &FakeSecrets::empty(),
        &|_| false,
    )
    .expect_err("no provider can route any protocol the gateway requires");
    let rendered = refusal.to_string();

    let GatewayUpstreamRefusal::NoProviderServesTheIngress { protocols, served } = refusal else {
        panic!("expected a no-compatible-provider refusal");
    };
    assert!(protocols.contains(&WireProtocol::AnthropicMessages));
    assert!(
        served.contains("`chat-only` declares `openai-chat` (no base URL)"),
        "{served}"
    );
    assert!(
        served.contains("`responses-without-url` declares `openai-responses` (no base URL)"),
        "{served}"
    );
    assert!(rendered.contains("anthropic-messages"), "{rendered}");
    assert!(rendered.contains(&served), "{rendered}");
}

/// Every rendering of a gateway upstream refusal, checked against a
/// planted value. These are printed on a launch path, which is exactly
/// where a credential would be seen.
#[test]
fn no_gateway_upstream_refusal_carries_a_credential() {
    let secrets = FakeSecrets::holding(CREDENTIAL_VAR, PLANTED_CREDENTIAL);
    let mut anthropic = provider_serving(
        "openrouter",
        WireProtocol::AnthropicMessages,
        "https://openrouter.ai/api",
    );
    anthropic.credential_env = vec![CREDENTIAL_VAR.to_owned()];
    let mut unusable = provider_serving(
        "unusable",
        WireProtocol::AnthropicMessages,
        "not-an-absolute-url",
    );
    unusable.credential_env = vec![CREDENTIAL_VAR.to_owned()];

    let refusals = vec![
        gateway_upstream(&[], &secrets, &|_| false).unwrap_err(),
        gateway_upstream(&[anthropic], &FakeSecrets::empty(), &|_| false).unwrap_err(),
        gateway_upstream(&[unusable], &secrets, &|_| false).unwrap_err(),
    ];

    let mut seen = std::collections::BTreeSet::new();
    for refusal in &refusals {
        let display = refusal.to_string();
        let debug = format!("{refusal:?}");
        assert!(!display.contains(PLANTED_CREDENTIAL), "{display}");
        assert!(!debug.contains(PLANTED_CREDENTIAL), "{debug}");
        seen.insert(match refusal {
            GatewayUpstreamRefusal::NoProviderServesTheIngress { .. } => "none",
            GatewayUpstreamRefusal::CredentialUnavailable { .. } => "credential",
            GatewayUpstreamRefusal::Unusable(_) => "unusable",
        });
    }
    assert_eq!(seen.len(), 3, "every variant must be exercised: {seen:?}");
}

/// A direct-provider profile whose provider the caller could not look up
/// is refused too — and it names the provider, so the user knows which
/// configuration entry is missing.
#[test]
fn a_direct_provider_profile_with_no_configured_provider_is_refused() {
    let adapter = adapter_for(IntegrationId::ClaudeCode).expect("a harness");
    let profile = direct_profile(IntegrationId::ClaudeCode, "not-configured");

    let err = resolve(&profile, &native_cx(adapter, false, &FakeSecrets::empty()))
        .expect_err("an unconfigured provider is refused");
    match &err {
        Refusal::ProviderNotConfigured {
            harness, provider, ..
        } => {
            assert_eq!(*harness, IntegrationId::ClaudeCode);
            assert_eq!(provider, "not-configured");
        }
        other => panic!("expected ProviderNotConfigured, got {other:?}"),
    }
    assert!(err.to_string().contains("not-configured"));
}

// --- 6. an overlay reaches only the child process ---------------------

#[test]
fn an_overlay_reaches_only_the_child_process() {
    use crate::Project;
    use crate::platform::exec;

    let tmp = tempfile::tempdir().unwrap();
    let script = tmp.path().join("fake-harness");
    std::fs::write(&script, "#!/bin/sh\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(&script).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&script, perms).unwrap();
    }
    let executable = exec::resolve_explicit(&script).expect("resolve");

    let root = tmp.path().join("proj");
    std::fs::create_dir_all(root.join(".git")).unwrap();
    let project = Project::discover(&root, None, false).unwrap();

    // Constructed directly rather than through `resolve`, to exercise
    // `apply` in isolation with no provider and no secret store in the
    // way.
    //
    // Until Phase 9F this was the *only* way to reach `apply` with an
    // environment at all, because only `Native` resolved and it
    // contributes none. That is no longer true — a direct-provider
    // profile now populates `env` through `resolve`, which is what
    // `a_claude_code_profile_carries_the_providers_base_url_and_credential`
    // asserts. The two together are the chain, and this half is
    // deliberately kept unit-sized: it is about `apply` carrying an
    // environment operation onto a child, not about where one came from.
    let overlay = LaunchOverlay {
        args: vec![OsString::from("--overlay-flag")],
        env: vec![(
            OsString::from("GLASSHOUSE_TEST_OVERLAY_KEY"),
            OsString::from("unmistakable-secret-shaped-value"),
        )],
        configs: Vec::new(),
        mechanisms: vec![MechanismNote {
            category: "approval mode",
            detail: "automatic review (--overlay-flag)".to_owned(),
        }],
    };

    // Before consuming the overlay: its own safe rendering never carries
    // the value either.
    for note in overlay.mechanisms() {
        assert!(!note.detail.contains("unmistakable-secret-shaped-value"));
    }

    let launch = HarnessLaunch::new(executable, &project);
    let launch = overlay.apply(launch);

    // The overlay reached the launch: the env key (never the value) and
    // the arg count both show up in the launch's own redacted `Debug`.
    let rendered = format!("{launch:?}");
    assert!(
        rendered.contains("GLASSHOUSE_TEST_OVERLAY_KEY"),
        "{rendered}"
    );
    assert!(rendered.contains("\"set\""), "{rendered}");
    assert!(
        !rendered.contains("unmistakable-secret-shaped-value"),
        "the env value leaked into the launch's Debug: {rendered}"
    );
    assert!(rendered.contains("arg_count: 1"), "{rendered}");
}

// --- 7. the user's own arguments stay last -----------------------------

#[test]
fn the_user_s_own_arguments_stay_last() {
    let adapter = adapter_for(IntegrationId::ClaudeCode).expect("a harness");
    let profile = profile_for(IntegrationId::ClaudeCode); // Default -> automatic review
    let overlay = resolve(&profile, &native_cx(adapter, false, &FakeSecrets::empty())).unwrap();
    assert!(
        !overlay.args().is_empty(),
        "Claude Code declares automatic review"
    );

    let adapter_args = vec![OsString::from("--session-id"), OsString::from("abc")];
    let user_args = [OsString::from("--resume"), OsString::from("xyz")];

    // The same order production code composes: adapter args, then the
    // overlay's own args, then the user's own `--` arguments.
    let mut composed = adapter_args.clone();
    composed.extend(overlay.args().iter().cloned());
    composed.extend(user_args.iter().cloned());

    assert_eq!(&composed[..adapter_args.len()], &adapter_args[..]);
    assert_eq!(
        &composed[adapter_args.len()..composed.len() - user_args.len()],
        overlay.args(),
        "the overlay's own arguments must sit strictly between the adapter's and the user's"
    );
    assert_eq!(
        &composed[composed.len() - user_args.len()..],
        &user_args[..],
        "the user's own arguments must be last, so they always win"
    );
}

// --- 11. no environment value is ever rendered -------------------------

#[test]
fn no_environment_value_is_ever_rendered() {
    // A model on a profile is validated but never turned into an
    // argument or an environment value in Phase 9A (see `resolve`'s
    // comment) — so even an unmistakably secret-shaped model name must
    // never surface anywhere resolution can render.
    let adapter = adapter_for(IntegrationId::ClaudeCode).expect("a harness");
    let mut profile = profile_for(IntegrationId::ClaudeCode);
    profile.model = Some("sk-SUPER-SECRET-MODEL-VALUE-should-never-render".to_owned());

    let overlay = resolve(&profile, &native_cx(adapter, false, &FakeSecrets::empty()))
        .expect("Claude Code declares a model override");

    let args_rendered: Vec<String> = overlay
        .args()
        .iter()
        .map(|a| a.to_string_lossy().into_owned())
        .collect();
    assert!(
        args_rendered.iter().all(|a| !a.contains("SUPER-SECRET")),
        "{args_rendered:?}"
    );
    assert!(overlay.env().is_empty());
    for note in overlay.mechanisms() {
        assert!(!note.detail.contains("SUPER-SECRET"), "{}", note.detail);
    }
    let debug_rendered = format!("{overlay:?}");
    assert!(!debug_rendered.contains("SUPER-SECRET"), "{debug_rendered}");

    // And a refusal on an unrelated rule must not echo the model value
    // back either.
    let opencode = adapter_for(IntegrationId::OpenCode).expect("a harness");
    let mut refused_profile = profile_for(IntegrationId::OpenCode);
    // Anthropic messages, not OpenAI chat: since Phase 9A's generated-
    // configuration work OpenCode *does* declare `openai-chat`, so
    // asking for that one would now resolve rather than refuse.
    refused_profile.expected_protocol = Some(WireProtocol::AnthropicMessages);
    let err = resolve(
        &refused_profile,
        &native_cx(opencode, false, &FakeSecrets::empty()),
    )
    .expect_err("unsupported protocol");
    assert!(!err.to_string().contains("SECRET"));
}

// --- Phase 9F: direct provider launch profiles -----------------------

/// The second file-name check — the one at the moment a name becomes a
/// path — refuses, opens nothing, and leaves the overlay pointing at
/// nothing.
///
/// Unreachable through [`resolve`], which refuses such a name first, so
/// the document is built here by hand. That is deliberate and is the
/// same shape Phase 10's M7 used: this check guards against the adapter
/// that has not been written yet, and a test that could only reach it
/// through today's adapters would be asserting a property today's code
/// cannot violate.
#[test]
fn installing_a_name_that_could_leave_the_site_refuses_and_writes_nothing() {
    let mut overlay = LaunchOverlay::empty();
    overlay.configs.push(PendingConfig {
        file_name: "../escaped.json",
        contents: "{}\n".to_owned(),
        placement: crate::harness::ConfigPathPlacement::Environment("GLASSHOUSE_TEST_CONFIG"),
    });

    let tmp = tempfile::tempdir().expect("tempdir");
    let site = tmp.path().join("session");
    let err = overlay
        .install(GeneratedConfigSite::new(&site))
        .expect_err("a name that could leave the site must be refused");
    assert_eq!(err.kind(), std::io::ErrorKind::InvalidInput);

    assert!(!site.exists(), "nothing may be created for a refused name");
    assert!(
        !tmp.path().join("escaped.json").exists(),
        "and certainly not outside the site"
    );
    assert!(
        overlay.env().is_empty() && overlay.args().is_empty(),
        "a child must not be pointed at a document that was never written"
    );
}

/// Phase 9A lines 362 and 370, at the one place a credential could newly
/// escape: a generated configuration document.
///
/// The temptation this exists to kill is the obvious shortcut — writing
/// the key into the file, because the harness would accept it there.
/// OpenCode substitutes `{env:NAME}` inside a configuration document
/// before parsing it, so the document names the variable and the value
/// travels the way every other harness's credential already does.
///
/// Asserted on the **document's own bytes**, composed by the adapter, so
/// no arrangement of writing or of diagnostics can make this pass while
/// the key is in the file.
#[test]
fn a_generated_configuration_names_the_credential_variable_and_never_its_value() {
    let opencode = adapter_for(IntegrationId::OpenCode).expect("a harness");
    let secrets = FakeSecrets::holding(CREDENTIAL_VAR, PLANTED_CREDENTIAL);
    let mut provider = provider_serving(
        "probe-router",
        WireProtocol::OpenAiChat,
        "https://probe.example/v1",
    );
    provider.credential_env = vec![CREDENTIAL_VAR.to_owned()];
    provider
        .headers
        .push(("X-Probe".to_owned(), "probe-header-value".to_owned()));

    let mut profile = direct_profile(IntegrationId::OpenCode, "probe-router");
    profile.model = Some("probe-model-x".to_owned());

    let overlay = resolve(&profile, &direct_cx(opencode, &provider, &secrets))
        .expect("OpenCode takes an OpenAI-chat provider");

    let document = overlay.configs()[0].contents();
    assert!(
        !document.contains(PLANTED_CREDENTIAL),
        "the credential must never be written into a generated configuration:\n{document}"
    );
    assert!(
        document.contains(&format!("{{env:{CREDENTIAL_VAR}}}")),
        "the document must name the variable the harness reads the value from:\n{document}"
    );
    // Everything else the harness needs really is in there, so this is
    // not passing because the document is empty.
    assert!(document.contains("https://probe.example/v1"));
    assert!(document.contains("probe-model-x"));
    assert!(document.contains("probe-header-value"));

    // And the value is in the child's environment, under the provider's
    // own declared variable — the mechanism that was already closed.
    assert_eq!(
        env_value(&overlay, CREDENTIAL_VAR),
        Some(std::ffi::OsStr::new(PLANTED_CREDENTIAL))
    );

    // No diagnostic carries the value, the document, or the base URL.
    for note in overlay.mechanisms() {
        assert!(!note.detail.contains(PLANTED_CREDENTIAL), "{}", note.detail);
        assert!(!note.detail.contains("probe.example"), "{}", note.detail);
    }
    let debug = format!("{overlay:?}");
    assert!(!debug.contains(PLANTED_CREDENTIAL), "{debug}");
    assert!(
        !format!("{:?}", overlay.configs()).contains(PLANTED_CREDENTIAL),
        "a pending document must not render its own contents"
    );
}

/// The mechanism note for a generated configuration says what a person
/// needs in order to find the file and know it is Glasshouse's — and
/// nothing else.
#[test]
fn the_generated_configuration_diagnostic_shows_names_only() {
    let opencode = adapter_for(IntegrationId::OpenCode).expect("a harness");
    let secrets = FakeSecrets::holding(CREDENTIAL_VAR, PLANTED_CREDENTIAL);
    let mut provider = provider_serving(
        "probe-router",
        WireProtocol::OpenAiChat,
        "https://probe.example/v1",
    );
    provider.credential_env = vec![CREDENTIAL_VAR.to_owned()];
    let mut profile = direct_profile(IntegrationId::OpenCode, "probe-router");
    profile.model = Some("probe-model-x".to_owned());

    let overlay = resolve(&profile, &direct_cx(opencode, &provider, &secrets)).expect("ok");
    let note = overlay
        .mechanisms()
        .iter()
        .find(|note| note.category == "generated configuration")
        .expect("a generated configuration is a mechanism, and is reported as one");
    assert!(
        note.detail.contains("opencode-provider.json"),
        "{}",
        note.detail
    );
    assert!(note.detail.contains("OPENCODE_CONFIG"), "{}", note.detail);
    assert!(
        note.detail.contains("removed with it"),
        "the diagnostic should say the document is ephemeral: {}",
        note.detail
    );

    // And nothing else. A diagnostic that rendered the document itself
    // would be carrying a whole harness configuration — today that is a
    // base URL and a set of headers, and tomorrow it is whatever an
    // adapter decides a provider entry needs. Line 370's rule is names
    // only, and this is where a generated document would break it.
    for forbidden in ["probe.example", "$schema", "npm", "baseURL", "apiKey"] {
        assert!(
            !note.detail.contains(forbidden),
            "the diagnostic carried `{forbidden}` from the document itself: {}",
            note.detail
        );
    }
}

/// The credential every test below plants in its store. Distinctive on
/// purpose: a `!contains` assertion is only worth as much as the
/// improbability of the needle appearing by accident.
const PLANTED_CREDENTIAL: &str = "sk-glasshouse-planted-credential-must-never-render";
const CREDENTIAL_VAR: &str = "GLASSHOUSE_TEST_PROVIDER_KEY";

fn anthropic_provider() -> Provider {
    let mut provider = provider_serving(
        "my-gateway",
        WireProtocol::AnthropicMessages,
        "https://gateway.example/anthropic",
    );
    provider.credential_env = vec![CREDENTIAL_VAR.to_owned()];
    provider
}

fn responses_provider() -> Provider {
    let mut provider = provider_serving(
        "my-responses",
        WireProtocol::OpenAiResponses,
        "https://gateway.example/v1",
    );
    provider.credential_env = vec![CREDENTIAL_VAR.to_owned()];
    provider
}

fn direct_cx<'a>(
    adapter: &'a dyn HarnessAdapter,
    provider: &'a Provider,
    secrets: &'a dyn SecretStore,
) -> Resolution<'a> {
    Resolution {
        adapter,
        acknowledged_bypass: false,
        provider: Some(provider),
        secrets,
    }
}

/// Line 1/2/3/5: Claude Code is pointed at a compatible gateway with the
/// provider's own base URL and the credential the store held, and neither
/// touches anything but this one child's environment.
#[test]
fn a_claude_code_profile_carries_the_providers_base_url_and_credential() {
    let adapter = adapter_for(IntegrationId::ClaudeCode).expect("a harness");
    let provider = anthropic_provider();
    let secrets = FakeSecrets::holding(CREDENTIAL_VAR, PLANTED_CREDENTIAL);
    let profile = direct_profile(IntegrationId::ClaudeCode, &provider.name);

    let overlay = resolve(&profile, &direct_cx(adapter, &provider, &secrets))
        .expect("an anthropic-compatible provider backs Claude Code");

    // Verbatim: Claude Code appends `/v1/messages` itself, so nothing
    // here may append or strip a path segment.
    assert_eq!(
        env_value(&overlay, "ANTHROPIC_BASE_URL"),
        Some(std::ffi::OsStr::new("https://gateway.example/anthropic"))
    );
    // Never `assert_eq!` on secret material: a failure prints both sides.
    let token = env_value(&overlay, "ANTHROPIC_AUTH_TOKEN").expect("the credential is placed");
    assert!(
        token == std::ffi::OsStr::new(PLANTED_CREDENTIAL),
        "ANTHROPIC_AUTH_TOKEN did not carry the value the store held"
    );
    // No arguments at all — the mechanism is purely the child's
    // environment, so nothing was written anywhere.
    assert!(
        !rendered_args(&overlay)
            .iter()
            .any(|arg| arg.starts_with("--settings")),
        "Claude Code's direct-provider mechanism must write no settings document"
    );
}

/// Line 4: a model is passed through when the profile names one, and
/// `ANTHROPIC_MODEL` is *absent* — not empty — when it does not.
#[test]
fn a_claude_code_profile_carries_a_model_only_when_one_is_named() {
    let adapter = adapter_for(IntegrationId::ClaudeCode).expect("a harness");
    let provider = anthropic_provider();
    let secrets = FakeSecrets::holding(CREDENTIAL_VAR, PLANTED_CREDENTIAL);

    let without = direct_profile(IntegrationId::ClaudeCode, &provider.name);
    let overlay = resolve(&without, &direct_cx(adapter, &provider, &secrets)).unwrap();
    assert_eq!(
        env_value(&overlay, "ANTHROPIC_MODEL"),
        None,
        "a profile naming no model must leave ANTHROPIC_MODEL unset, not empty"
    );

    let mut with = without.clone();
    with.model = Some("provider/some-model-id".to_owned());
    let overlay = resolve(&with, &direct_cx(adapter, &provider, &secrets)).unwrap();
    assert_eq!(
        env_value(&overlay, "ANTHROPIC_MODEL"),
        Some(std::ffi::OsStr::new("provider/some-model-id"))
    );
}

/// Lines 6/7/8/10: Codex gets a whole custom provider out of `-c`
/// overrides, in a fixed order, and **no file is written at all**.
#[test]
fn a_codex_profile_composes_its_provider_entirely_from_c_overrides() {
    let adapter = adapter_for(IntegrationId::Codex).expect("a harness");
    let provider = responses_provider();
    let secrets = FakeSecrets::holding(CREDENTIAL_VAR, PLANTED_CREDENTIAL);
    let mut profile = direct_profile(IntegrationId::Codex, &provider.name);
    profile.model = Some("some-responses-model".to_owned());

    let overlay = resolve(&profile, &direct_cx(adapter, &provider, &secrets))
        .expect("an openai-responses provider backs Codex");

    let args = rendered_args(&overlay);
    let expected = [
        "-c",
        "model_provider=my-responses",
        "-c",
        "model_providers.my-responses.name=my-responses",
        "-c",
        "model_providers.my-responses.base_url=https://gateway.example/v1",
        "-c",
        "model_providers.my-responses.wire_api=responses",
        "-c",
        &format!("model_providers.my-responses.env_key={CREDENTIAL_VAR}"),
        "-c",
        "model=some-responses-model",
    ]
    .map(str::to_owned);
    assert_eq!(
        &args[..expected.len()],
        &expected[..],
        "the six -c overrides must be composed in a fixed order"
    );

    // `env_key` names a variable of the child process, and the overlay
    // sets exactly that variable — a name agreeing with a destination is
    // the whole mechanism.
    let placed = env_value(&overlay, CREDENTIAL_VAR).expect("the credential is placed");
    assert!(
        placed == std::ffi::OsStr::new(PLANTED_CREDENTIAL),
        "{CREDENTIAL_VAR} did not carry the value the store held"
    );
    // Nothing that looks like a path to a generated configuration.
    assert!(
        !args.iter().any(|arg| arg.contains("config.toml")),
        "Codex's mechanism must name no configuration file: {args:?}"
    );
}

/// Codex 0.149.1 removed `wire_api = "chat"`, so a provider serving only
/// `openai-chat` cannot back Codex. Refused — never a configuration Codex
/// would reject after the process had already started.
#[test]
fn a_codex_profile_backed_by_an_openai_chat_provider_is_refused() {
    let adapter = adapter_for(IntegrationId::Codex).expect("a harness");
    let mut provider = provider_serving(
        "chat-only",
        WireProtocol::OpenAiChat,
        "https://gateway.example/v1",
    );
    provider.credential_env = vec![CREDENTIAL_VAR.to_owned()];
    let secrets = FakeSecrets::holding(CREDENTIAL_VAR, PLANTED_CREDENTIAL);
    let profile = direct_profile(IntegrationId::Codex, &provider.name);

    let err = resolve(&profile, &direct_cx(adapter, &provider, &secrets))
        .expect_err("openai-chat cannot back Codex 0.149.1");
    match &err {
        Refusal::ProviderProtocolUnsupported {
            harness, provider, ..
        } => {
            assert_eq!(*harness, IntegrationId::Codex);
            assert_eq!(provider, "chat-only");
        }
        other => panic!("expected ProviderProtocolUnsupported, got {other:?}"),
    }
    let message = err.to_string();
    assert!(
        message.contains(WireProtocol::OpenAiChat.slug()),
        "the message must name what the provider serves: {message}"
    );
    assert!(
        message.contains(WireProtocol::OpenAiResponses.slug()),
        "the message must name what Codex needs: {message}"
    );
    assert!(message.contains("Codex"), "{message}");
}

/// The real, shipped NVIDIA template — not a synthetic stand-in — is the
/// honest consequence of declaring `openai-chat` only: it cannot back
/// Codex, exactly like the synthetic case just above.
#[test]
fn a_codex_profile_backed_by_the_real_nvidia_template_is_refused_on_protocol_grounds() {
    let adapter = adapter_for(IntegrationId::Codex).expect("a harness");
    let provider = crate::provider::template("nvidia").expect("nvidia is a built-in template");
    let secrets = FakeSecrets::empty();
    let profile = direct_profile(IntegrationId::Codex, &provider.name);

    let err = resolve(&profile, &direct_cx(adapter, &provider, &secrets))
        .expect_err("NVIDIA declares openai-chat only, which cannot back Codex 0.149.1");
    assert!(matches!(err, Refusal::ProviderProtocolUnsupported { .. }));
}

/// Line 423's consumer: configured headers reach Claude Code as one
/// `ANTHROPIC_CUSTOM_HEADERS` variable, `Name: value` per line.
#[test]
fn claude_code_receives_configured_headers_as_a_custom_headers_variable() {
    let adapter = adapter_for(IntegrationId::ClaudeCode).expect("a harness");
    let mut provider = anthropic_provider();
    provider.headers = vec![
        ("X-Glasshouse-One".to_owned(), "value-one".to_owned()),
        ("X-Glasshouse-Two".to_owned(), "value-two".to_owned()),
    ];
    let secrets = FakeSecrets::holding(CREDENTIAL_VAR, PLANTED_CREDENTIAL);
    let profile = direct_profile(IntegrationId::ClaudeCode, &provider.name);

    let overlay = resolve(&profile, &direct_cx(adapter, &provider, &secrets)).unwrap();
    let headers = env_value(&overlay, "ANTHROPIC_CUSTOM_HEADERS")
        .expect("configured headers must reach the child")
        .to_string_lossy()
        .into_owned();
    assert_eq!(
        headers,
        "X-Glasshouse-One: value-one\nX-Glasshouse-Two: value-two"
    );
}

/// The same headers reach Codex as one `-c
/// model_providers.<id>.http_headers=…` inline-TOML-table override.
#[test]
fn codex_receives_configured_headers_as_an_http_headers_override() {
    let adapter = adapter_for(IntegrationId::Codex).expect("a harness");
    let mut provider = responses_provider();
    provider.headers = vec![("X-Glasshouse-One".to_owned(), "value-one".to_owned())];
    let secrets = FakeSecrets::holding(CREDENTIAL_VAR, PLANTED_CREDENTIAL);
    let profile = direct_profile(IntegrationId::Codex, &provider.name);

    let overlay = resolve(&profile, &direct_cx(adapter, &provider, &secrets)).unwrap();
    let args = rendered_args(&overlay);
    assert!(
        args.iter().any(|arg| arg
            == "model_providers.my-responses.http_headers={ \"X-Glasshouse-One\" = \"value-one\" }"),
        "the header override never reached the argument list: {args:?}"
    );
}

/// No headers configured, no header mechanism at all — on either
/// harness. An always-present but empty header line would be a subtler
/// version of the same invention this whole line refuses elsewhere.
#[test]
fn no_headers_configured_means_no_header_mechanism_on_either_harness() {
    let claude = adapter_for(IntegrationId::ClaudeCode).expect("a harness");
    let provider = anthropic_provider();
    let secrets = FakeSecrets::holding(CREDENTIAL_VAR, PLANTED_CREDENTIAL);
    let profile = direct_profile(IntegrationId::ClaudeCode, &provider.name);
    let overlay = resolve(&profile, &direct_cx(claude, &provider, &secrets)).unwrap();
    assert!(env_value(&overlay, "ANTHROPIC_CUSTOM_HEADERS").is_none());

    let codex = adapter_for(IntegrationId::Codex).expect("a harness");
    let responses = responses_provider();
    let profile = direct_profile(IntegrationId::Codex, &responses.name);
    let overlay = resolve(&profile, &direct_cx(codex, &responses, &secrets)).unwrap();
    assert!(
        !rendered_args(&overlay)
            .iter()
            .any(|arg| arg.contains("http_headers")),
    );
}

/// A provider name is interpolated into a dotted TOML path, so it is
/// refused — before any argument is composed — rather than sanitised.
#[test]
fn an_unsafe_provider_name_is_refused_before_any_argument_is_composed() {
    let adapter = adapter_for(IntegrationId::Codex).expect("a harness");
    let secrets = FakeSecrets::holding(CREDENTIAL_VAR, PLANTED_CREDENTIAL);

    for (name, offending) in [
        ("bad.name", '.'),
        ("bad;name", ';'),
        ("bad\"name", '"'),
        ("bad$name", '$'),
        ("bad name", ' '),
    ] {
        let mut provider =
            provider_serving(name, WireProtocol::OpenAiResponses, "https://a.example/v1");
        provider.credential_env = vec![CREDENTIAL_VAR.to_owned()];
        let profile = direct_profile(IntegrationId::Codex, name);

        let err = resolve(&profile, &direct_cx(adapter, &provider, &secrets))
            .expect_err("an unsafe provider name must be refused");
        match &err {
            Refusal::UnsafeProviderName {
                provider: refused,
                offending: found,
                ..
            } => {
                assert_eq!(refused, name);
                assert_eq!(*found, offending, "the offending character must be named");
            }
            other => panic!("expected UnsafeProviderName for `{name}`, got {other:?}"),
        }
        let message = err.to_string();
        assert!(
            message.contains(offending),
            "the message must name `{offending}`: {message}"
        );
    }
}

/// Line 11, and the reason this task is red-risk. A declared credential
/// with no value is a refusal, **not** a launch that lets the harness
/// reach for the user's own paid account.
#[test]
fn a_credential_that_cannot_be_resolved_is_refused_and_produces_no_overlay() {
    let adapter = adapter_for(IntegrationId::ClaudeCode).expect("a harness");
    let provider = anthropic_provider();
    let secrets = FakeSecrets::empty();
    let profile = direct_profile(IntegrationId::ClaudeCode, &provider.name);

    let result = resolve(&profile, &direct_cx(adapter, &provider, &secrets));
    assert!(
        result.is_err(),
        "an unresolvable credential must produce no overlay at all"
    );
    let err = result.unwrap_err();
    match &err {
        Refusal::CredentialUnavailable {
            harness, variables, ..
        } => {
            assert_eq!(*harness, IntegrationId::ClaudeCode);
            assert_eq!(variables, &vec![CREDENTIAL_VAR.to_owned()]);
        }
        other => panic!("expected CredentialUnavailable, got {other:?}"),
    }
    let message = err.to_string();
    assert!(
        message.contains(CREDENTIAL_VAR),
        "the message must name the variable: {message}"
    );
}

/// Several declared variables are a pool: the first that resolves wins,
/// and only an empty pool refuses.
#[test]
fn the_first_credential_variable_that_resolves_is_the_one_used() {
    let adapter = adapter_for(IntegrationId::Codex).expect("a harness");
    let mut provider = responses_provider();
    provider.credential_env = vec![
        "GLASSHOUSE_TEST_KEY_PRIMARY".to_owned(),
        "GLASSHOUSE_TEST_KEY_BACKUP".to_owned(),
    ];
    let profile = direct_profile(IntegrationId::Codex, &provider.name);

    // Only the second one has a value: it is used rather than refused.
    let secrets = FakeSecrets::holding("GLASSHOUSE_TEST_KEY_BACKUP", PLANTED_CREDENTIAL);
    let overlay = resolve(&profile, &direct_cx(adapter, &provider, &secrets)).unwrap();
    assert!(
        rendered_args(&overlay)
            .iter()
            .any(|arg| arg == "model_providers.my-responses.env_key=GLASSHOUSE_TEST_KEY_BACKUP")
    );
    assert!(env_value(&overlay, "GLASSHOUSE_TEST_KEY_BACKUP").is_some());
    assert!(env_value(&overlay, "GLASSHOUSE_TEST_KEY_PRIMARY").is_none());

    // Both set: the first declared wins, deterministically.
    let secrets = FakeSecrets(vec![
        (
            "GLASSHOUSE_TEST_KEY_PRIMARY".to_owned(),
            "primary".to_owned(),
        ),
        ("GLASSHOUSE_TEST_KEY_BACKUP".to_owned(), "backup".to_owned()),
    ]);
    let overlay = resolve(&profile, &direct_cx(adapter, &provider, &secrets)).unwrap();
    assert!(env_value(&overlay, "GLASSHOUSE_TEST_KEY_PRIMARY").is_some());
    assert!(env_value(&overlay, "GLASSHOUSE_TEST_KEY_BACKUP").is_none());
}

/// The two generic templates ship an empty base URL on purpose. Launching
/// a harness against `""` must never happen.
#[test]
fn a_provider_with_no_base_url_for_the_chosen_protocol_is_refused() {
    let adapter = adapter_for(IntegrationId::ClaudeCode).expect("a harness");
    let provider = provider_serving("anthropic-compatible", WireProtocol::AnthropicMessages, "");
    let secrets = FakeSecrets::holding(CREDENTIAL_VAR, PLANTED_CREDENTIAL);
    let profile = direct_profile(IntegrationId::ClaudeCode, &provider.name);

    let err = resolve(&profile, &direct_cx(adapter, &provider, &secrets))
        .expect_err("an empty base URL is refused");
    match &err {
        Refusal::ProviderBaseUrlMissing {
            provider, protocol, ..
        } => {
            assert_eq!(provider, "anthropic-compatible");
            assert_eq!(*protocol, WireProtocol::AnthropicMessages);
        }
        other => panic!("expected ProviderBaseUrlMissing, got {other:?}"),
    }

    // And the real shipped template is exactly that shape, so this is not
    // a hypothetical: `anthropic-compatible` cannot launch until the user
    // supplies a URL.
    let template = crate::provider::template("anthropic-compatible").unwrap();
    assert!(
        template
            .serves(WireProtocol::AnthropicMessages)
            .unwrap()
            .base_url
            .is_empty()
    );
}

/// A harness that declares no direct-provider mechanism is refused,
/// naming the harness — never launched natively instead.
#[test]
fn a_harness_with_no_direct_provider_mechanism_is_refused() {
    // The other five adapters inherit the `None` default *and* declare
    // `protocols: Unverified`, so they are refused one step earlier —
    // at the protocol intersection, which is still a refusal naming the
    // harness and still starts nothing.
    let adapter = adapter_for(IntegrationId::OpenCode).expect("a harness");
    let provider = anthropic_provider();
    let secrets = FakeSecrets::holding(CREDENTIAL_VAR, PLANTED_CREDENTIAL);
    let profile = direct_profile(IntegrationId::OpenCode, &provider.name);

    let err = resolve(&profile, &direct_cx(adapter, &provider, &secrets))
        .expect_err("OpenCode declares no direct-provider mechanism");
    assert!(
        err.to_string().contains("OpenCode"),
        "the refusal must name the harness: {err}"
    );

    // And the `NoDirectProviderMechanism` rule itself, on a harness that
    // *does* declare a matching protocol but no mechanism — the state
    // every future adapter starts in.
    let double = NoDirectProviderMechanism;
    let mut profile = direct_profile(IntegrationId::Pi, &provider.name);
    profile.name = "gateway".to_owned();
    let err = resolve(&profile, &direct_cx(&double, &provider, &secrets))
        .expect_err("a declared protocol without a mechanism is still refused");
    match &err {
        Refusal::NoDirectProviderMechanism {
            harness, protocol, ..
        } => {
            assert_eq!(*harness, IntegrationId::Pi);
            assert_eq!(*protocol, WireProtocol::AnthropicMessages);
        }
        other => panic!("expected NoDirectProviderMechanism, got {other:?}"),
    }
    assert!(err.to_string().contains("Pi"), "{err}");
}

/// A harness that *can* be pointed at a backend but declares nowhere to
/// put the credential — the one shape that would silently launch a
/// gateway-backed session the gateway itself would then refuse.
#[derive(Debug)]
struct TokenUnplaceable;

impl HarnessAdapter for TokenUnplaceable {
    fn id(&self) -> IntegrationId {
        IntegrationId::Pi
    }
    fn executable_candidates(&self) -> &'static [&'static str] {
        &["pretend"]
    }
    fn start(&self) -> crate::harness::Invocation {
        crate::harness::Invocation::bare()
    }
    fn resume(&self, _native_session: &str) -> Option<crate::harness::Invocation> {
        None
    }
    fn describe(&self) -> crate::harness::HarnessDescription {
        NoDirectProviderMechanism.describe()
    }
    fn direct_provider_launch(
        &self,
        _request: &DirectProviderRequest<'_>,
    ) -> Option<crate::harness::DirectProviderPlan> {
        Some(crate::harness::DirectProviderPlan {
            args: Vec::new(),
            env: Vec::new(),
            credential: None,
            config: None,
            mechanism: "a test double that forgets the credential".to_owned(),
        })
    }
}

/// A harness declaring a protocol it can serve, and no way at all to be
/// pointed at a provider — the default every adapter inherits.
#[derive(Debug)]
struct NoDirectProviderMechanism;

impl HarnessAdapter for NoDirectProviderMechanism {
    fn id(&self) -> IntegrationId {
        IntegrationId::Pi
    }
    fn executable_candidates(&self) -> &'static [&'static str] {
        &["pretend"]
    }
    fn start(&self) -> crate::harness::Invocation {
        crate::harness::Invocation::bare()
    }
    fn resume(&self, _native_session: &str) -> Option<crate::harness::Invocation> {
        None
    }
    fn describe(&self) -> crate::harness::HarnessDescription {
        crate::harness::HarnessDescription {
            vendor: crate::harness::Declared::Unverified,
            hooks: crate::harness::Declared::Unverified,
            session_ids: crate::harness::Declared::Unverified,
            capabilities: crate::harness::Capabilities::UNVERIFIED,
            backends: crate::harness::Backends {
                protocols: Declared::verified(
                    &[WireProtocol::AnthropicMessages],
                    "a test double, declaring exactly one protocol",
                ),
                model_override: Declared::Unverified,
                selection: Declared::Unverified,
            },
            approvals: crate::harness::ApprovalModes::UNVERIFIED,
            communication_style: crate::harness::Declared::Unverified,
        }
    }
}

/// An adapter that asks for a generated configuration under a name that
/// would leave the directory Glasshouse owns.
///
/// It exists because no real adapter can do this — every one of them
/// gets its path from [`GeneratedConfigSite::file`], which refuses such
/// a name — so the production check in `accept_generated_config` would
/// otherwise be asserting a property today's code cannot violate. This
/// is the same deliberately synthetic shape Phase 10's M7 used, and for
/// the same reason: the check is a guard against the adapter that has
/// not been written yet.
#[derive(Debug)]
struct EscapingGeneratedConfig;

impl HarnessAdapter for EscapingGeneratedConfig {
    fn id(&self) -> IntegrationId {
        IntegrationId::ClaudeCode
    }
    fn executable_candidates(&self) -> &'static [&'static str] {
        &["pretend"]
    }
    fn start(&self) -> crate::harness::Invocation {
        crate::harness::Invocation::bare()
    }
    fn resume(&self, _native_session: &str) -> Option<crate::harness::Invocation> {
        None
    }
    fn describe(&self) -> crate::harness::HarnessDescription {
        crate::harness::HarnessDescription {
            vendor: crate::harness::Declared::Unverified,
            hooks: crate::harness::Declared::Unverified,
            session_ids: crate::harness::Declared::Unverified,
            capabilities: crate::harness::Capabilities::UNVERIFIED,
            backends: crate::harness::Backends {
                protocols: Declared::verified(
                    &[WireProtocol::OpenAiChat],
                    "a test double, declaring exactly one protocol",
                ),
                model_override: Declared::verified(
                    &[crate::harness::ModelOverride::CommandLine("--model")],
                    "a test double",
                ),
                selection: Declared::Unverified,
            },
            approvals: crate::harness::ApprovalModes::UNVERIFIED,
            communication_style: crate::harness::Declared::Unverified,
        }
    }
    fn direct_provider_launch(
        &self,
        _request: &DirectProviderRequest<'_>,
    ) -> Option<crate::harness::DirectProviderPlan> {
        Some(crate::harness::DirectProviderPlan {
            args: Vec::new(),
            env: Vec::new(),
            credential: None,
            config: Some(crate::harness::GeneratedConfig {
                file_name: "../../the-users-own-config.json",
                contents: "{}\n".to_owned(),
                path_placement: crate::harness::ConfigPathPlacement::Environment(
                    "GLASSHOUSE_TEST_CONFIG",
                ),
            }),
            mechanism: "a test double that names a path instead of a file".to_owned(),
        })
    }
}

/// An explicitly expected protocol is a constraint, never a hint: the
/// provider must serve *that* one.
#[test]
fn an_expected_protocol_the_provider_does_not_serve_is_refused() {
    let adapter = adapter_for(IntegrationId::ClaudeCode).expect("a harness");
    let provider = anthropic_provider();
    let secrets = FakeSecrets::holding(CREDENTIAL_VAR, PLANTED_CREDENTIAL);
    let mut profile = direct_profile(IntegrationId::ClaudeCode, &provider.name);

    // Claude Code can serve it, so the harness-side check passes; the
    // provider cannot, so this is the provider-side refusal.
    profile.expected_protocol = Some(WireProtocol::AnthropicMessages);
    resolve(&profile, &direct_cx(adapter, &provider, &secrets))
        .expect("both sides serve anthropic-messages");

    let chat_only = {
        let mut p = provider_serving(
            "my-gateway",
            WireProtocol::OpenAiChat,
            "https://gateway.example/v1",
        );
        p.credential_env = vec![CREDENTIAL_VAR.to_owned()];
        p
    };
    let err = resolve(&profile, &direct_cx(adapter, &chat_only, &secrets))
        .expect_err("the provider does not serve the expected protocol");
    assert!(matches!(err, Refusal::ProviderProtocolUnsupported { .. }));
}

/// A credential variable name is interpolated into a `-c` value too, so
/// it is checked the same way — and the check names the problem without
/// naming any value.
#[test]
fn an_unusable_credential_variable_name_is_refused() {
    let adapter = adapter_for(IntegrationId::Codex).expect("a harness");
    let mut provider = responses_provider();
    provider.credential_env = vec!["BAD-VAR NAME".to_owned()];
    let secrets = FakeSecrets::holding("BAD-VAR NAME", PLANTED_CREDENTIAL);
    let profile = direct_profile(IntegrationId::Codex, &provider.name);

    let err = resolve(&profile, &direct_cx(adapter, &provider, &secrets))
        .expect_err("an unusable variable name is refused");
    match &err {
        Refusal::UnsafeCredentialVariable { variable, .. } => {
            assert_eq!(variable, "BAD-VAR NAME");
        }
        other => panic!("expected UnsafeCredentialVariable, got {other:?}"),
    }
    assert!(!err.to_string().contains(PLANTED_CREDENTIAL), "{err}");
}

/// **The credential never leaks.** Not from a successful overlay's
/// `Debug`, not from its mechanism notes, and not from any refusal a real
/// resolution can produce while a store holds a value.
#[test]
fn a_resolved_credential_never_reaches_a_rendering() {
    let claude = adapter_for(IntegrationId::ClaudeCode).expect("a harness");
    let codex = adapter_for(IntegrationId::Codex).expect("a harness");
    // Declares no protocols at all (`Declared::Unverified`) — the one
    // harness left that can still produce `GatewayProtocolUnserved`
    // now that the pair table covers every ordered pair of the three
    // known protocols; used only in that scenario below.
    let cursor = adapter_for(IntegrationId::Cursor).expect("a harness");
    let secrets = FakeSecrets::holding(CREDENTIAL_VAR, PLANTED_CREDENTIAL);

    // 1. A successful resolution, rendered every way this type allows.
    for (adapter, provider) in [
        (claude, anthropic_provider()),
        (codex, responses_provider()),
    ] {
        let mut profile = direct_profile(adapter.id(), &provider.name);
        profile.model = Some("a-model".to_owned());
        let overlay = resolve(&profile, &direct_cx(adapter, &provider, &secrets)).unwrap();

        let debug = format!("{overlay:?}");
        assert!(
            !debug.contains(PLANTED_CREDENTIAL),
            "the credential reached LaunchOverlay's Debug"
        );
        for note in overlay.mechanisms() {
            assert!(
                !note.detail.contains(PLANTED_CREDENTIAL),
                "the credential reached a mechanism note"
            );
        }
        for arg in rendered_args(&overlay) {
            assert!(
                !arg.contains(PLANTED_CREDENTIAL),
                "the credential reached an argument"
            );
        }
        // It *is* in exactly one place, and that place is the child's
        // environment — proven by key, never by printing the value.
        assert!(
            overlay
                .env()
                .iter()
                .any(|(_, value)| value == std::ffi::OsStr::new(PLANTED_CREDENTIAL)),
            "the credential must reach the child environment"
        );

        // And onward through `apply`, whose own `Debug` is redacted too.
        let debug = format!("{:?}", overlay.mechanisms());
        assert!(!debug.contains(PLANTED_CREDENTIAL));
    }

    // 2. Every refusal a resolution can produce, while the store holds a
    //    value, rendered both ways.
    let empty = FakeSecrets::empty();
    let mut unsafe_name = provider_serving(
        "bad.name",
        WireProtocol::AnthropicMessages,
        "https://a.example",
    );
    unsafe_name.credential_env = vec![CREDENTIAL_VAR.to_owned()];
    let mut no_url = provider_serving("no-url", WireProtocol::AnthropicMessages, "");
    no_url.credential_env = vec![CREDENTIAL_VAR.to_owned()];
    let mut bad_var = provider_serving(
        "bad-var",
        WireProtocol::AnthropicMessages,
        "https://a.example",
    );
    bad_var.credential_env = vec!["9NOPE".to_owned()];
    let mut oc_provider = provider_serving(
        "oc-provider",
        WireProtocol::OpenAiChat,
        "https://a.example/v1",
    );
    oc_provider.credential_env = vec![CREDENTIAL_VAR.to_owned()];
    // A base URL carrying a sequence the harness would substitute inside
    // the document Glasshouse generates for it.
    let mut oc_inject = provider_serving(
        "oc-inject",
        WireProtocol::OpenAiChat,
        "https://a.example/{env:HOME}/v1",
    );
    oc_inject.credential_env = vec![CREDENTIAL_VAR.to_owned()];

    let scenarios: Vec<(&str, Refusal)> = vec![
        ("harness executable unavailable", {
            let p = direct_profile(IntegrationId::ClaudeCode, "my-gateway");
            resolve_checked(
                &p,
                &direct_cx(claude, &anthropic_provider(), &secrets),
                None,
                &crate::harness::ExecutablePresence::NotFound,
            )
            .unwrap_err()
        }),
        ("gateway not running", {
            let mut p = profile_for(IntegrationId::ClaudeCode);
            p.backend = BackendResource::GlasshouseGateway;
            resolve(&p, &native_cx(claude, false, &secrets)).unwrap_err()
        }),
        ("gateway protocol unserved", {
            // Codex moved out of this scenario: `openai-responses ->
            // anthropic-messages` is a `Supported` row now, so that
            // combination resolves instead of refusing. `cursor`
            // declares no protocols at all, so no row in the table is
            // ever even consulted for it.
            let gateway = running_gateway();
            let mut p = profile_for(IntegrationId::Cursor);
            p.backend = BackendResource::GlasshouseGateway;
            resolve_with_gateway(
                &p,
                &native_cx(cursor, false, &secrets),
                Some(&gateway),
                &GatewayPairing::default(),
            )
            .unwrap_err()
        }),
        ("gateway translation refused", {
            let opencode = adapter_for(IntegrationId::OpenCode).expect("a harness");
            let gateway = running_gateway();
            let mut p = profile_for(IntegrationId::OpenCode);
            p.backend = BackendResource::GlasshouseGateway;
            p.model = Some("oc-model".to_owned());
            resolve_with_gateway(
                &p,
                &native_cx(opencode, false, &secrets),
                Some(&gateway),
                &GatewayPairing::default(),
            )
            .unwrap_err()
        }),
        ("gateway token unplaceable", {
            let gateway = running_gateway();
            let double = TokenUnplaceable;
            let mut p = profile_for(IntegrationId::Pi);
            p.backend = BackendResource::GlasshouseGateway;
            resolve_with_gateway(
                &p,
                &native_cx(&double, false, &secrets),
                Some(&gateway),
                &GatewayPairing::default(),
            )
            .unwrap_err()
        }),
        ("unconfigured provider", {
            let p = direct_profile(IntegrationId::ClaudeCode, "nope");
            resolve(&p, &native_cx(claude, false, &secrets)).unwrap_err()
        }),
        ("unsafe provider name", {
            let p = direct_profile(IntegrationId::ClaudeCode, "bad.name");
            resolve(&p, &direct_cx(claude, &unsafe_name, &secrets)).unwrap_err()
        }),
        ("unsafe credential variable", {
            let p = direct_profile(IntegrationId::ClaudeCode, "bad-var");
            resolve(&p, &direct_cx(claude, &bad_var, &secrets)).unwrap_err()
        }),
        ("protocol unsupported", {
            let p = direct_profile(IntegrationId::Codex, "my-gateway");
            resolve(&p, &direct_cx(codex, &anthropic_provider(), &secrets)).unwrap_err()
        }),
        ("base url missing", {
            let p = direct_profile(IntegrationId::ClaudeCode, "no-url");
            resolve(&p, &direct_cx(claude, &no_url, &secrets)).unwrap_err()
        }),
        ("no mechanism", {
            let p = direct_profile(IntegrationId::Pi, "my-gateway");
            resolve(
                &p,
                &direct_cx(&NoDirectProviderMechanism, &anthropic_provider(), &secrets),
            )
            .unwrap_err()
        }),
        ("credential unavailable", {
            let p = direct_profile(IntegrationId::ClaudeCode, "my-gateway");
            resolve(&p, &direct_cx(claude, &anthropic_provider(), &empty)).unwrap_err()
        }),
        ("no automatic review", {
            let opencode = adapter_for(IntegrationId::OpenCode).expect("a harness");
            let mut p = profile_for(IntegrationId::OpenCode);
            p.approval = ApprovalSelection::AutomaticReview;
            resolve(&p, &native_cx(opencode, false, &secrets)).unwrap_err()
        }),
        ("bypass not acknowledged", {
            let mut p = profile_for(IntegrationId::ClaudeCode);
            p.approval = ApprovalSelection::Bypass;
            resolve(&p, &native_cx(claude, false, &secrets)).unwrap_err()
        }),
        ("no bypass", {
            let pi = adapter_for(IntegrationId::Pi).expect("a harness");
            let mut p = profile_for(IntegrationId::Pi);
            p.approval = ApprovalSelection::Bypass;
            resolve(&p, &native_cx(pi, true, &secrets)).unwrap_err()
        }),
        ("no model override", {
            // The double declares no model-override mechanism either.
            let mut p = profile_for(IntegrationId::Pi);
            p.model = Some("m".to_owned());
            resolve(&p, &native_cx(&NoDirectProviderMechanism, false, &secrets)).unwrap_err()
        }),
        ("protocol mismatch", {
            let mut p = profile_for(IntegrationId::ClaudeCode);
            p.expected_protocol = Some(WireProtocol::OpenAiResponses);
            resolve(&p, &native_cx(claude, false, &secrets)).unwrap_err()
        }),
        ("automatic review needs a native backend", {
            let mut p = direct_profile(IntegrationId::ClaudeCode, "my-gateway");
            p.approval = ApprovalSelection::AutomaticReview;
            resolve(&p, &direct_cx(claude, &anthropic_provider(), &secrets)).unwrap_err()
        }),
        ("direct provider needs a model", {
            let opencode = adapter_for(IntegrationId::OpenCode).expect("a harness");
            let p = direct_profile(IntegrationId::OpenCode, "oc-provider");
            resolve(&p, &direct_cx(opencode, &oc_provider, &secrets)).unwrap_err()
        }),
        ("unsafe generated config name", {
            let mut p = direct_profile(IntegrationId::ClaudeCode, "oc-provider");
            p.model = Some("a-model".to_owned());
            resolve(
                &p,
                &direct_cx(&EscapingGeneratedConfig, &oc_provider, &secrets),
            )
            .unwrap_err()
        }),
        ("unsafe generated config value", {
            let opencode = adapter_for(IntegrationId::OpenCode).expect("a harness");
            let mut p = direct_profile(IntegrationId::OpenCode, "oc-inject");
            p.model = Some("a-model".to_owned());
            resolve(&p, &direct_cx(opencode, &oc_inject, &secrets)).unwrap_err()
        }),
    ];

    for (label, refusal) in &scenarios {
        let display = refusal.to_string();
        let debug = format!("{refusal:?}");
        assert!(
            !display.contains(PLANTED_CREDENTIAL),
            "`{label}`'s Display carried the credential"
        );
        assert!(
            !debug.contains(PLANTED_CREDENTIAL),
            "`{label}`'s Debug carried the credential"
        );
    }

    // Exhaustive by construction: adding a `Refusal` variant without
    // covering it here stops compiling, rather than quietly leaving a
    // rendering nobody checked.
    let mut seen = std::collections::BTreeSet::new();
    for (_, refusal) in &scenarios {
        seen.insert(match refusal {
            Refusal::GatewayNotRunning { .. } => "GatewayNotRunning",
            Refusal::GatewayProtocolUnserved { .. } => "GatewayProtocolUnserved",
            Refusal::GatewayTranslationRefused { .. } => "GatewayTranslationRefused",
            Refusal::GatewayTokenUnplaceable { .. } => "GatewayTokenUnplaceable",
            Refusal::ProviderNotConfigured { .. } => "ProviderNotConfigured",
            Refusal::ProviderProtocolUnsupported { .. } => "ProviderProtocolUnsupported",
            Refusal::ProviderBaseUrlMissing { .. } => "ProviderBaseUrlMissing",
            Refusal::UnsafeProviderName { .. } => "UnsafeProviderName",
            Refusal::UnsafeCredentialVariable { .. } => "UnsafeCredentialVariable",
            Refusal::NoDirectProviderMechanism { .. } => "NoDirectProviderMechanism",
            Refusal::CredentialUnavailable { .. } => "CredentialUnavailable",
            Refusal::NoAutomaticReview { .. } => "NoAutomaticReview",
            Refusal::BypassNotAcknowledged { .. } => "BypassNotAcknowledged",
            Refusal::NoBypass { .. } => "NoBypass",
            Refusal::NoModelOverride { .. } => "NoModelOverride",
            Refusal::ProtocolMismatch { .. } => "ProtocolMismatch",
            Refusal::AutomaticReviewNeedsNativeBackend { .. } => {
                "AutomaticReviewNeedsNativeBackend"
            }
            Refusal::HarnessExecutableUnavailable { .. } => "HarnessExecutableUnavailable",
            Refusal::DirectProviderNeedsModel { .. } => "DirectProviderNeedsModel",
            Refusal::UnsafeGeneratedConfigName { .. } => "UnsafeGeneratedConfigName",
            Refusal::UnsafeGeneratedConfigValue { .. } => "UnsafeGeneratedConfigValue",
        });
    }
    assert_eq!(
        seen.len(),
        21,
        "every Refusal variant must be exercised here: {seen:?}"
    );
}

/// Amendment 1. A gateway-backed Claude Code session must not come up
/// carrying `--permission-mode auto`: the mode's classifier is a
/// server-side model call the backend may not serve, and auto mode fails
/// closed with tools blocked. The contrast is the behaviour — the same
/// profile on `Native` still selects it.
#[test]
fn a_defaulted_profile_selects_automatic_review_only_on_a_native_backend() {
    let adapter = adapter_for(IntegrationId::ClaudeCode).expect("a harness");
    let provider = anthropic_provider();
    let secrets = FakeSecrets::holding(CREDENTIAL_VAR, PLANTED_CREDENTIAL);

    let direct = direct_profile(IntegrationId::ClaudeCode, &provider.name);
    assert_eq!(direct.approval, ApprovalSelection::Default);
    let overlay = resolve(&direct, &direct_cx(adapter, &provider, &secrets)).unwrap();
    let args = rendered_args(&overlay);
    assert!(
        !args.iter().any(|arg| arg == "--permission-mode"),
        "a gateway-backed session must carry no --permission-mode: {args:?}"
    );
    assert!(
        !args.iter().any(|arg| arg == "auto"),
        "nor its value: {args:?}"
    );
    assert!(
        overlay
            .mechanisms()
            .iter()
            .any(|note| note.detail.contains("automatic review withheld")),
        "the decision must be recorded: {:?}",
        overlay.mechanisms()
    );

    // The other half: `Native` behaviour does not change by one byte.
    let native = profile_for(IntegrationId::ClaudeCode);
    assert_eq!(native.approval, ApprovalSelection::Default);
    let overlay = resolve(&native, &native_cx(adapter, false, &secrets)).unwrap();
    let args = rendered_args(&overlay);
    assert_eq!(
        args,
        vec!["--permission-mode".to_owned(), "auto".to_owned()],
        "a native-backed session must still select automatic review"
    );
}

/// Amendment 1, the explicit half: a default that falls back is not a
/// request that is refused.
#[test]
fn an_explicit_automatic_review_request_on_a_gateway_backed_profile_is_refused() {
    let adapter = adapter_for(IntegrationId::ClaudeCode).expect("a harness");
    let provider = anthropic_provider();
    let secrets = FakeSecrets::holding(CREDENTIAL_VAR, PLANTED_CREDENTIAL);
    let mut profile = direct_profile(IntegrationId::ClaudeCode, &provider.name);
    profile.approval = ApprovalSelection::AutomaticReview;

    let err = resolve(&profile, &direct_cx(adapter, &provider, &secrets))
        .expect_err("automatic review is refused on a gateway-backed profile");
    let backend = match &err {
        Refusal::AutomaticReviewNeedsNativeBackend {
            profile: name,
            harness,
            backend,
        } => {
            assert_eq!(name, "gateway");
            assert_eq!(*harness, IntegrationId::ClaudeCode);
            *backend
        }
        other => panic!("expected AutomaticReviewNeedsNativeBackend, got {other:?}"),
    };
    let message = err.to_string();
    assert!(message.contains("Claude Code"), "{message}");
    assert!(
        message.contains(backend),
        "the message must name the backend: {message}"
    );
    assert_eq!(backend, "a direct provider");

    // A bypass is unchanged: still refused until acknowledged, still
    // resolved once it is.
    let mut bypass = direct_profile(IntegrationId::ClaudeCode, &provider.name);
    bypass.approval = ApprovalSelection::Bypass;
    let err = resolve(&bypass, &direct_cx(adapter, &provider, &secrets))
        .expect_err("an unacknowledged bypass is still refused");
    assert!(matches!(err, Refusal::BypassNotAcknowledged { .. }));

    let acknowledged = Resolution {
        adapter,
        acknowledged_bypass: true,
        provider: Some(&provider),
        secrets: &secrets,
    };
    let overlay = resolve(&bypass, &acknowledged).expect("an acknowledged bypass resolves");
    assert!(
        rendered_args(&overlay)
            .iter()
            .any(|arg| arg == "--dangerously-skip-permissions")
    );
}

// --- resolution rule coverage not named above --------------------------

#[test]
fn a_harness_without_a_model_override_refuses_a_model_request() {
    // Antigravity declares only a command-line model override; pick a
    // harness that declares none at all — none of the seven do today, so
    // this uses a double to exercise the rule.
    #[derive(Debug)]
    struct NoModelOverride;
    impl HarnessAdapter for NoModelOverride {
        fn id(&self) -> IntegrationId {
            IntegrationId::Pi
        }
        fn executable_candidates(&self) -> &'static [&'static str] {
            &["pretend"]
        }
        fn start(&self) -> crate::harness::Invocation {
            crate::harness::Invocation::bare()
        }
        fn resume(&self, _native_session: &str) -> Option<crate::harness::Invocation> {
            None
        }
        fn describe(&self) -> crate::harness::HarnessDescription {
            crate::harness::HarnessDescription {
                vendor: crate::harness::Declared::Unverified,
                hooks: crate::harness::Declared::Unverified,
                session_ids: crate::harness::Declared::Unverified,
                capabilities: crate::harness::Capabilities::UNVERIFIED,
                backends: crate::harness::Backends::UNVERIFIED,
                approvals: crate::harness::ApprovalModes::UNVERIFIED,
                communication_style: crate::harness::Declared::Unverified,
            }
        }
    }

    let adapter = NoModelOverride;
    let mut profile = profile_for(IntegrationId::Pi);
    profile.model = Some("some-model".to_owned());

    let err = resolve(&profile, &native_cx(&adapter, false, &FakeSecrets::empty()))
        .expect_err("no model override declared");
    assert!(matches!(err, Refusal::NoModelOverride { .. }));
}

#[test]
fn a_bypass_selection_on_a_harness_with_no_bypass_mode_is_refused() {
    // Pi's whole `ApprovalModes` is unverified, so it declares neither
    // automatic review nor a bypass. Asking for bypass must not panic or
    // silently produce an empty overlay — it must refuse.
    let adapter = adapter_for(IntegrationId::Pi).expect("a harness");
    let mut profile = profile_for(IntegrationId::Pi);
    profile.approval = ApprovalSelection::Bypass;

    let err = resolve(&profile, &native_cx(adapter, true, &FakeSecrets::empty()))
        .expect_err("Pi declares no bypass mode");
    assert!(matches!(err, Refusal::NoBypass { .. }));
}

#[test]
fn a_protocol_a_harness_cannot_serve_is_refused() {
    let adapter = adapter_for(IntegrationId::ClaudeCode).expect("a harness");
    let mut profile = profile_for(IntegrationId::ClaudeCode);
    profile.expected_protocol = Some(WireProtocol::OpenAiResponses);

    let err = resolve(&profile, &native_cx(adapter, false, &FakeSecrets::empty()))
        .expect_err("Claude Code only speaks Anthropic Messages");
    match &err {
        Refusal::ProtocolMismatch {
            harness, protocol, ..
        } => {
            assert_eq!(*harness, IntegrationId::ClaudeCode);
            assert_eq!(*protocol, WireProtocol::OpenAiResponses);
        }
        other => panic!("expected ProtocolMismatch, got {other:?}"),
    }
}

#[test]
fn a_protocol_a_harness_can_serve_is_accepted() {
    let adapter = adapter_for(IntegrationId::ClaudeCode).expect("a harness");
    let mut profile = profile_for(IntegrationId::ClaudeCode);
    profile.expected_protocol = Some(WireProtocol::AnthropicMessages);

    resolve(&profile, &native_cx(adapter, false, &FakeSecrets::empty()))
        .expect("Claude Code speaks Anthropic Messages natively");
}

#[test]
fn an_unverified_protocol_declaration_cannot_serve_anything() {
    // Cursor's protocols are `Unverified`. "Nobody checked" must not be
    // treated as "yes" for a protocol match.
    let adapter = adapter_for(IntegrationId::Cursor).expect("a harness");
    let mut profile = profile_for(IntegrationId::Cursor);
    profile.expected_protocol = Some(WireProtocol::AnthropicMessages);

    let err = resolve(&profile, &native_cx(adapter, false, &FakeSecrets::empty()))
        .expect_err("unverified protocols cannot serve anything");
    assert!(matches!(err, Refusal::ProtocolMismatch { .. }));
}

#[test]
fn backend_slugs_never_carry_a_credential_shaped_field_name() {
    assert_eq!(BackendResource::Native.slug(), "native");
    assert_eq!(
        BackendResource::DirectProvider {
            provider: "openrouter".to_owned()
        }
        .slug(),
        "direct-provider:openrouter"
    );
    assert_eq!(
        BackendResource::GlasshouseGateway.slug(),
        "glasshouse-gateway"
    );
}

#[test]
fn profile_class_matches_the_backend_kind() {
    let mut profile = LaunchProfile::native(IntegrationId::ClaudeCode);
    assert_eq!(profile.class(), ProfileClass::NativeSubscription);
    profile.backend = BackendResource::DirectProvider {
        provider: "openrouter".to_owned(),
    };
    assert_eq!(profile.class(), ProfileClass::DirectProvider);
    profile.backend = BackendResource::GlasshouseGateway;
    assert_eq!(profile.class(), ProfileClass::GlasshouseGateway);
}

// --- Phase 9F line 466: the executable precondition -------------------

use crate::harness::ExecutablePresence;

/// Acceptance test 1: a direct-provider profile naming a harness whose
/// executable is not installed is refused, names the harness and the
/// candidates tried, and starts nothing (there is no overlay to apply).
#[test]
fn a_direct_provider_profile_is_refused_when_the_executable_is_not_found() {
    let adapter = adapter_for(IntegrationId::ClaudeCode).expect("a harness");
    let provider = anthropic_provider();
    let secrets = FakeSecrets::holding(CREDENTIAL_VAR, PLANTED_CREDENTIAL);
    let profile = direct_profile(IntegrationId::ClaudeCode, &provider.name);

    let err = resolve_checked(
        &profile,
        &direct_cx(adapter, &provider, &secrets),
        None,
        &ExecutablePresence::NotFound,
    )
    .expect_err("an absent executable must refuse a direct-provider profile");

    match &err {
        Refusal::HarnessExecutableUnavailable {
            harness, detail, ..
        } => {
            assert_eq!(*harness, IntegrationId::ClaudeCode);
            assert!(detail.contains("candidates tried"), "{detail}");
            for candidate in IntegrationId::ClaudeCode.executable_candidates() {
                assert!(detail.contains(candidate), "{detail}");
            }
        }
        other => panic!("expected HarnessExecutableUnavailable, got {other:?}"),
    }
    let message = err.to_string();
    assert!(message.contains("Claude Code"), "{message}");
    assert!(message.contains("candidates tried"), "{message}");
}

/// Acceptance test 2: the same profile, with the executable present, is
/// not refused for that reason — and resolves exactly as plain
/// `resolve` would.
#[test]
fn the_same_profile_is_not_refused_when_the_executable_is_usable() {
    let adapter = adapter_for(IntegrationId::ClaudeCode).expect("a harness");
    let provider = anthropic_provider();
    let secrets = FakeSecrets::holding(CREDENTIAL_VAR, PLANTED_CREDENTIAL);
    let profile = direct_profile(IntegrationId::ClaudeCode, &provider.name);
    let cx = direct_cx(adapter, &provider, &secrets);

    let checked = resolve_checked(&profile, &cx, None, &ExecutablePresence::Usable)
        .expect("a usable executable must not be refused");
    let unchecked =
        resolve(&profile, &cx).expect("the same profile resolves without the check too");
    assert_eq!(
        env_value(&checked, "ANTHROPIC_BASE_URL"),
        env_value(&unchecked, "ANTHROPIC_BASE_URL")
    );
    assert_eq!(rendered_args(&checked), rendered_args(&unchecked));
}

/// A found-but-unusable executable (a Windows-interop-only `PATH` hit,
/// for example) is refused too, and the refusal carries the resolver's
/// own reason rather than "candidates tried".
#[test]
fn an_unusable_executable_is_refused_with_its_own_reason() {
    let adapter = adapter_for(IntegrationId::ClaudeCode).expect("a harness");
    let provider = anthropic_provider();
    let secrets = FakeSecrets::holding(CREDENTIAL_VAR, PLANTED_CREDENTIAL);
    let profile = direct_profile(IntegrationId::ClaudeCode, &provider.name);

    let err = resolve_checked(
        &profile,
        &direct_cx(adapter, &provider, &secrets),
        None,
        &ExecutablePresence::Unusable {
            reason: "found only in the Windows side of PATH".to_owned(),
        },
    )
    .expect_err("an unusable executable must be refused");
    match &err {
        Refusal::HarnessExecutableUnavailable { detail, .. } => {
            assert_eq!(detail, "found only in the Windows side of PATH");
        }
        other => panic!("expected HarnessExecutableUnavailable, got {other:?}"),
    }
}

/// Acceptance test 3: a `Native` profile is unaffected by line 466's
/// check, byte for byte — an absent executable changes nothing about
/// it, because the check is never even consulted for one.
#[test]
fn a_native_profile_is_unaffected_by_the_executable_check() {
    let adapter = adapter_for(IntegrationId::ClaudeCode).expect("a harness");
    let secrets = FakeSecrets::empty();
    let profile = profile_for(IntegrationId::ClaudeCode);
    let cx = native_cx(adapter, false, &secrets);

    let via_checked = resolve_checked(&profile, &cx, None, &ExecutablePresence::NotFound)
        .expect("a Native profile must resolve even when the check would refuse");
    let via_plain = resolve(&profile, &cx).expect("plain resolve must agree");

    assert_eq!(rendered_args(&via_checked), rendered_args(&via_plain));
    assert!(via_checked.env().is_empty() && via_plain.env().is_empty());
    assert_eq!(via_checked.mechanisms().len(), via_plain.mechanisms().len());
}

/// Acceptance test 6 (line 466's half): the check never reroutes to a
/// different backend — a refusal is the only effect it can have. Proven
/// by construction: `resolve_checked` either returns exactly what
/// `resolve_with_gateway` would, or refuses; there is no third path that
/// substitutes a different backend.
#[test]
fn the_executable_check_never_changes_which_backend_would_be_selected() {
    let adapter = adapter_for(IntegrationId::ClaudeCode).expect("a harness");
    let provider = anthropic_provider();
    let secrets = FakeSecrets::holding(CREDENTIAL_VAR, PLANTED_CREDENTIAL);
    let profile = direct_profile(IntegrationId::ClaudeCode, &provider.name);
    let cx = direct_cx(adapter, &provider, &secrets);

    for presence in [
        ExecutablePresence::Usable,
        ExecutablePresence::NotFound,
        ExecutablePresence::Unusable {
            reason: "x".to_owned(),
        },
    ] {
        let is_usable = presence.is_usable();
        match resolve_checked(&profile, &cx, None, &presence) {
            Ok(overlay) => {
                assert!(
                    is_usable,
                    "an unusable presence must never produce an overlay"
                );
                // Identical to what plain resolution against the same
                // provider produces — no different backend was chosen.
                let plain = resolve(&profile, &cx).unwrap();
                assert_eq!(rendered_args(&overlay), rendered_args(&plain));
            }
            Err(Refusal::HarnessExecutableUnavailable { .. }) => {
                assert!(!is_usable);
            }
            Err(other) => panic!("only the executable refusal may appear here: {other}"),
        }
    }
}

/// Acceptance test 7 (line 466's half): no credential leaks through the
/// new refusal's `Display` or `Debug`.
#[test]
fn the_executable_refusal_never_carries_a_credential() {
    let adapter = adapter_for(IntegrationId::ClaudeCode).expect("a harness");
    let provider = anthropic_provider();
    let secrets = FakeSecrets::holding(CREDENTIAL_VAR, PLANTED_CREDENTIAL);
    let profile = direct_profile(IntegrationId::ClaudeCode, &provider.name);

    let err = resolve_checked(
        &profile,
        &direct_cx(adapter, &provider, &secrets),
        None,
        &ExecutablePresence::NotFound,
    )
    .unwrap_err();
    assert!(!err.to_string().contains(PLANTED_CREDENTIAL));
    assert!(!format!("{err:?}").contains(PLANTED_CREDENTIAL));
}

/// A gateway-backed profile is covered by line 466 too, not only a
/// direct-provider one.
#[test]
fn a_gateway_backed_profile_is_also_refused_when_the_executable_is_not_found() {
    let claude = adapter_for(IntegrationId::ClaudeCode).expect("a harness");
    let secrets = FakeSecrets::empty();
    let gateway = running_gateway();
    let mut profile = profile_for(IntegrationId::ClaudeCode);
    profile.backend = BackendResource::GlasshouseGateway;

    let err = resolve_checked(
        &profile,
        &native_cx(claude, false, &secrets),
        Some(&gateway),
        &ExecutablePresence::NotFound,
    )
    .expect_err("a gateway-backed profile must be refused too");
    assert!(matches!(err, Refusal::HarnessExecutableUnavailable { .. }));
}

// --- Phase 9F line 465: the capability check ---------------------------

/// Acceptance test 5: a provider for which no cheap check is available —
/// here, a gateway-backed profile, which has no fixed upstream
/// combination — reports that no check was made, and nothing about
/// resolving the profile itself changes because of it.
#[test]
fn a_gateway_backed_profile_has_no_capability_check_available() {
    let claude = adapter_for(IntegrationId::ClaudeCode).expect("a harness");
    let secrets = FakeSecrets::empty();
    let mut profile = profile_for(IntegrationId::ClaudeCode);
    profile.backend = BackendResource::GlasshouseGateway;
    let cx = native_cx(claude, false, &secrets);

    match capability_probe(&profile, &cx) {
        CapabilityProbe::Unavailable { reason } => assert!(!reason.is_empty()),
        CapabilityProbe::Available(_) => panic!("a gateway-backed profile has no check yet"),
    }

    // The launch itself proceeds regardless — a gateway-backed profile
    // resolves (once a gateway is running) whether or not a capability
    // check was ever considered.
    let gateway = running_gateway();
    resolve_with_gateway(&profile, &cx, Some(&gateway), &GatewayPairing::default())
        .expect("the absent capability check must not block the launch");
}

/// A `Native` profile has no capability check available either — there
/// is no protocol, base URL or credential this crate holds for it.
#[test]
fn a_native_profile_has_no_capability_check_available() {
    let adapter = adapter_for(IntegrationId::ClaudeCode).expect("a harness");
    let secrets = FakeSecrets::empty();
    let profile = profile_for(IntegrationId::ClaudeCode);
    let cx = native_cx(adapter, false, &secrets);

    match capability_probe(&profile, &cx) {
        CapabilityProbe::Unavailable { reason } => assert!(!reason.is_empty()),
        CapabilityProbe::Available(_) => panic!("a native profile has no check available"),
    }
}

/// A resolvable direct-provider profile always has a check available,
/// even when the provider has no established model-list endpoint: the
/// base URL itself is still a valid target.
#[test]
fn a_resolvable_direct_provider_profile_always_has_a_check_available() {
    let adapter = adapter_for(IntegrationId::ClaudeCode).expect("a harness");
    let provider = anthropic_provider();
    assert!(
        !provider.model_list_endpoint.is_known_present(),
        "this test wants the base-URL-only path"
    );
    let secrets = FakeSecrets::holding(CREDENTIAL_VAR, PLANTED_CREDENTIAL);
    let profile = direct_profile(IntegrationId::ClaudeCode, &provider.name);
    let cx = direct_cx(adapter, &provider, &secrets);

    let request = match capability_probe(&profile, &cx) {
        CapabilityProbe::Available(request) => request,
        CapabilityProbe::Unavailable { reason } => {
            panic!("a resolvable provider must have a check available: {reason}")
        }
    };
    assert_eq!(request.provider(), provider.name);
    assert_eq!(request.protocol(), WireProtocol::AnthropicMessages);
    assert_eq!(request.url(), "https://gateway.example/anthropic");
}

/// A direct-provider profile this crate cannot resolve (here: an
/// unconfigured provider) has no capability check available either —
/// the same "unavailable, not a failure" answer, for a different reason.
#[test]
fn an_unresolvable_direct_provider_profile_has_no_check_available() {
    let adapter = adapter_for(IntegrationId::ClaudeCode).expect("a harness");
    let secrets = FakeSecrets::empty();
    let profile = direct_profile(IntegrationId::ClaudeCode, "not-configured");
    // `cx.provider` is `None`: the caller could not find "not-configured"
    // in configuration, exactly as `resolve` would see it too.
    let cx = native_cx(adapter, false, &secrets);

    match capability_probe(&profile, &cx) {
        CapabilityProbe::Unavailable { reason } => assert!(!reason.is_empty()),
        CapabilityProbe::Available(_) => {
            panic!("an unconfigured provider has nothing to probe")
        }
    }
}

/// Acceptance test 4 (formatting half): a `401` renders as
/// reachable-but-rejected, distinctly from a host that never answered —
/// the two must never read the same.
#[test]
fn describe_probe_outcome_distinguishes_rejected_from_unreachable() {
    use crate::provider::discovery::ProbeOutcome;

    let rejected = describe_probe_outcome(&ProbeOutcome::Rejected { status: 401 });
    let unreachable = describe_probe_outcome(&ProbeOutcome::Unreachable {
        reason: "the connection was refused".to_owned(),
    });
    assert_ne!(rejected, unreachable);
    assert!(rejected.contains("401"));
    assert!(rejected.contains("reachable"), "{rejected}");
    assert!(unreachable.contains("never answered"), "{unreachable}");

    let reached = describe_probe_outcome(&ProbeOutcome::Reached { status: 200 });
    assert_ne!(reached, rejected);
}

/// [`PREFLIGHT_TIMEOUTS`] is strictly tighter than the interactive
/// default on every axis — the claim that a pre-flight check cannot cost
/// a launch what an on-demand probe may cost a keystroke.
///
/// # Why this is a declaration test and not a stopwatch
///
/// The behaviour worth guarding is "a launch is bounded by a budget
/// chosen for launches". The obvious test — start `glasshouse launch`
/// against a dead host and assert it finished inside N seconds — measures
/// wall clock on the machine running it, and this project's own gate is
/// already flaky under concurrent load for exactly that reason. A timing
/// assertion there would report the runner's CPU contention as a product
/// defect.
///
/// So this asserts the thing that is actually decidable: the constant the
/// launch path uses is not the one an interactive probe uses, and is
/// smaller on all three axes. Widening it back to the default fails here,
/// on every platform, in microseconds. Found by mutation: the budget was
/// unwatched by anything until this test existed, and a launch quietly
/// restored to a twenty-second ceiling would have passed the whole suite.
#[test]
fn a_pre_flight_check_is_bounded_more_tightly_than_an_interactive_probe() {
    use crate::provider::discovery::ProbeTimeouts;

    let interactive = ProbeTimeouts::default();
    assert!(
        PREFLIGHT_TIMEOUTS.connect < interactive.connect,
        "a launch may not wait as long for a connection as a keystroke may: {:?} vs {:?}",
        PREFLIGHT_TIMEOUTS.connect,
        interactive.connect
    );
    assert!(
        PREFLIGHT_TIMEOUTS.response < interactive.response,
        "a launch may not wait as long for a response as a keystroke may: {:?} vs {:?}",
        PREFLIGHT_TIMEOUTS.response,
        interactive.response
    );
    assert!(
        PREFLIGHT_TIMEOUTS.total < interactive.total,
        "the whole-call ceiling is the one a stalled launch actually pays: {:?} vs {:?}",
        PREFLIGHT_TIMEOUTS.total,
        interactive.total
    );
    // And the ceiling is a number a person would accept in front of a
    // session they asked for, rather than merely smaller than twenty
    // seconds.
    assert!(
        PREFLIGHT_TIMEOUTS.total <= std::time::Duration::from_secs(5),
        "a pre-flight check that can hold a launch for {:?} is not cheap",
        PREFLIGHT_TIMEOUTS.total
    );
}

/// End to end, over a real loopback socket: [`capability_probe`] builds
/// the request, and [`crate::provider::discovery::connectivity`] — the
/// same function a real caller would run off-thread — actually sends it.
/// Three real servers, three real distinctions: reached, reachable but
/// rejected, and never answered at all.
#[test]
fn a_capability_probe_composes_with_a_real_connectivity_check() {
    use crate::provider::discovery::{ProbeOutcome, ProbeTimeouts, connectivity};
    use crate::provider::fixture::FixtureProvider;

    let adapter = adapter_for(IntegrationId::ClaudeCode).expect("a harness");
    let secrets = FakeSecrets::holding(CREDENTIAL_VAR, PLANTED_CREDENTIAL);
    let quick = ProbeTimeouts {
        connect: std::time::Duration::from_millis(500),
        response: std::time::Duration::from_millis(400),
        total: std::time::Duration::from_millis(900),
    };

    // A provider that answers.
    let ok_fixture = FixtureProvider::answering("HTTP/1.1 200 OK", "", "{}");
    let ok_provider = provider_serving(
        "answers-ok",
        WireProtocol::AnthropicMessages,
        &ok_fixture.base_url(),
    );
    let ok_profile = direct_profile(IntegrationId::ClaudeCode, &ok_provider.name);
    let request = match capability_probe(&ok_profile, &direct_cx(adapter, &ok_provider, &secrets)) {
        CapabilityProbe::Available(request) => request,
        CapabilityProbe::Unavailable { reason } => panic!("expected a request: {reason}"),
    };
    let outcome = connectivity(&request, quick);
    assert_eq!(outcome, ProbeOutcome::Reached { status: 200 });
    assert!(describe_probe_outcome(&outcome).contains("reached"));

    // A provider that answers, but rejects the credential.
    let rejecting_fixture = FixtureProvider::answering("HTTP/1.1 401 Unauthorized", "", "{}");
    let rejecting_provider = provider_serving(
        "answers-401",
        WireProtocol::AnthropicMessages,
        &rejecting_fixture.base_url(),
    );
    let rejecting_profile = direct_profile(IntegrationId::ClaudeCode, &rejecting_provider.name);
    let request = match capability_probe(
        &rejecting_profile,
        &direct_cx(adapter, &rejecting_provider, &secrets),
    ) {
        CapabilityProbe::Available(request) => request,
        CapabilityProbe::Unavailable { reason } => panic!("expected a request: {reason}"),
    };
    let outcome = connectivity(&request, quick);
    assert_eq!(outcome, ProbeOutcome::Rejected { status: 401 });
    let described = describe_probe_outcome(&outcome);
    assert!(described.contains("rejected"), "{described}");

    // A provider that is not there at all — nothing listening.
    let port = {
        let listener = std::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .expect("loopback is bindable");
        listener
            .local_addr()
            .expect("a bound listener has an address")
            .port()
    };
    let absent_provider = provider_serving(
        "unreachable",
        WireProtocol::AnthropicMessages,
        &format!("http://127.0.0.1:{port}"),
    );
    let absent_profile = direct_profile(IntegrationId::ClaudeCode, &absent_provider.name);
    let request = match capability_probe(
        &absent_profile,
        &direct_cx(adapter, &absent_provider, &secrets),
    ) {
        CapabilityProbe::Available(request) => request,
        CapabilityProbe::Unavailable { reason } => panic!("expected a request: {reason}"),
    };
    let outcome = connectivity(&request, quick);
    assert!(!outcome.answered(), "nothing was listening: {outcome:?}");

    // **Which** not-answered outcome a closed port produces is the
    // platform's choice, not Glasshouse's, and asserting one of them cost
    // this repository a red Windows run. A Unix stack answers a connection
    // to a closed loopback port with an immediate refusal, so the probe
    // reports `Unreachable`; Windows drops the SYN instead, so the probe
    // waits out its own bound and reports `TimedOut`. Both are honest and
    // both are correct — the product's distinction between them is worth
    // keeping, so the test asserts the property it actually cares about
    // rather than the platform's spelling of it.
    assert!(
        matches!(
            outcome,
            ProbeOutcome::TimedOut { .. } | ProbeOutcome::Unreachable { .. }
        ),
        "a closed port must be timed-out or unreachable, never a response: {outcome:?}"
    );
    let described = describe_probe_outcome(&outcome);

    // Reached, rejected and not-answered never collapse into the same
    // sentence, whichever not-answered outcome this platform produced.
    let reached_desc = describe_probe_outcome(&ProbeOutcome::Reached { status: 200 });
    let rejected_desc = describe_probe_outcome(&ProbeOutcome::Rejected { status: 401 });
    assert_ne!(reached_desc, described);
    assert_ne!(rejected_desc, described);

    // And both spellings are checked on **every** platform, not just the
    // one whose stack happens to produce them: the assertion above can
    // only ever see one of the two, which is precisely how the Windows
    // spelling reached CI unexamined. Practice §18 applied to a runtime
    // difference rather than a `cfg`.
    for not_answered in [
        ProbeOutcome::TimedOut { waited_ms: 509 },
        ProbeOutcome::Unreachable {
            reason: "connection refused".to_owned(),
        },
    ] {
        let desc = describe_probe_outcome(&not_answered);
        assert!(!not_answered.answered(), "{not_answered:?}");
        assert_ne!(reached_desc, desc);
        assert_ne!(rejected_desc, desc);
    }
}

/// Acceptance test 6 (line 465's half): nothing about a capability
/// probe's *outcome* can reach `resolve` at all — `capability_probe`
/// and `describe_probe_outcome` are read-only functions of a
/// [`ProbeRequest`]/[`ProbeOutcome`][crate::provider::discovery::ProbeOutcome]
/// that `resolve` never takes as input, so a failed check has no
/// mechanism by which it could reroute a launch to a different backend.
#[test]
fn a_capability_probe_cannot_influence_which_backend_resolve_selects() {
    let adapter = adapter_for(IntegrationId::ClaudeCode).expect("a harness");
    let provider = anthropic_provider();
    let secrets = FakeSecrets::holding(CREDENTIAL_VAR, PLANTED_CREDENTIAL);
    let profile = direct_profile(IntegrationId::ClaudeCode, &provider.name);
    let cx = direct_cx(adapter, &provider, &secrets);

    let before = resolve(&profile, &cx).unwrap();
    let _ = capability_probe(&profile, &cx);
    let after = resolve(&profile, &cx).unwrap();
    assert_eq!(rendered_args(&before), rendered_args(&after));
    assert_eq!(
        env_value(&before, "ANTHROPIC_BASE_URL"),
        env_value(&after, "ANTHROPIC_BASE_URL")
    );
}

/// Acceptance test 7 (line 465's half): the credential a capability
/// probe resolves reaches only the `ProbeRequest`'s private field —
/// never this module's own rendering of it.
#[test]
fn a_capability_probes_credential_never_reaches_this_modules_own_renderings() {
    let adapter = adapter_for(IntegrationId::ClaudeCode).expect("a harness");
    let provider = anthropic_provider();
    let secrets = FakeSecrets::holding(CREDENTIAL_VAR, PLANTED_CREDENTIAL);
    let profile = direct_profile(IntegrationId::ClaudeCode, &provider.name);
    let cx = direct_cx(adapter, &provider, &secrets);

    let request = match capability_probe(&profile, &cx) {
        CapabilityProbe::Available(request) => request,
        CapabilityProbe::Unavailable { reason } => panic!("expected a request: {reason}"),
    };
    let debug = format!("{request:?}");
    assert!(!debug.contains(PLANTED_CREDENTIAL), "{debug}");
    assert!(debug.contains(crate::secret::REDACTED), "{debug}");
    assert!(!request.url().contains(PLANTED_CREDENTIAL));
}
