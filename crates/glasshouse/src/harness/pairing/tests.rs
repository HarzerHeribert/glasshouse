use super::*;

fn query(harness: IntegrationId, model: &str) -> PairingQuery {
    PairingQuery {
        harness,
        model: AssignedModel::named(model),
        route: ServingRoute::default(),
        tool_calls: Declared::Unverified,
        provider_protocols: Vec::new(),
    }
}

fn none() -> PairingOverrides {
    PairingOverrides::default()
}

/// [`wire_protocol_from_slug`] round-trips every slug [`WireProtocol`]
/// actually produces, and refuses one none of them did rather than
/// guessing.
#[test]
fn wire_protocol_from_slug_round_trips_every_known_slug_and_refuses_an_unknown_one() {
    for protocol in [
        WireProtocol::AnthropicMessages,
        WireProtocol::OpenAiResponses,
        WireProtocol::OpenAiChat,
        WireProtocol::GeminiGenerateContent,
    ] {
        assert_eq!(wire_protocol_from_slug(protocol.slug()), Some(protocol));
    }
    assert_eq!(wire_protocol_from_slug("google-gemini"), None);
}

/// Line 557: both halves, and the second half is the one that matters.
#[test]
fn a_vendor_native_pairing_needs_the_family_and_the_developer() {
    let pairing = classify(&query(IntegrationId::ClaudeCode, "claude-fable-5"), &none());
    assert_eq!(pairing.class(), PairingClass::VendorNative);
    assert_eq!(pairing.developer().slug(), Some("anthropic"));
    assert_eq!(pairing.family(), Some("fable"));
}

/// The same family, in a harness whose vendor does not declare it. Google
/// publishes Antigravity; `sonnet` is not a family Antigravity declares
/// as its own, so this is not vendor-native however Anthropic-shaped the
/// name is.
#[test]
fn another_vendors_model_is_never_vendor_native_however_its_name_reads() {
    let pairing = classify(
        &query(IntegrationId::Antigravity, "claude-sonnet-4-6"),
        &none(),
    );
    assert_ne!(pairing.class(), PairingClass::VendorNative);
    assert_eq!(pairing.class(), PairingClass::VendorSupported);
}

/// Line 557's second half, which a family list alone cannot enforce: a
/// model that *calls itself* part of a vendor's family, developed by
/// somebody else, is not a first-party pairing. Nothing in a name gets
/// to promote a model into a vendor's own line.
#[test]
fn a_family_name_alone_does_not_make_a_pairing_vendor_native() {
    let mut models = BTreeMap::new();
    models.insert(
        "acme/gemini-clone-1".to_owned(),
        ModelCorrection {
            developer: Some(ModelDeveloper::named("acme")),
            family: Some("gemini".to_owned()),
            behaviour: None,
        },
    );
    let overrides =
        PairingOverrides::from_parts("the user configuration file", models, BTreeMap::new());

    let pairing = classify(
        &query(IntegrationId::Antigravity, "acme/gemini-clone-1"),
        &overrides,
    );
    assert_eq!(pairing.family(), Some("gemini"));
    assert_ne!(
        pairing.class(),
        PairingClass::VendorNative,
        "Antigravity declares `gemini` as its own family, but acme is not Google and this \
         is not a first-party pairing"
    );
    assert_eq!(pairing.class(), PairingClass::Unknown);
}

/// Line 558: the harness vendor's own statement, and it does not need to
/// know who wrote the weights. `gpt-oss-120b-medium` is in Antigravity's
/// own model list and in nobody's attribution.
#[test]
fn vendor_supported_stands_without_an_attributed_developer() {
    let pairing = classify(
        &query(IntegrationId::Antigravity, "gpt-oss-120b-medium"),
        &none(),
    );
    assert_eq!(pairing.class(), PairingClass::VendorSupported);
    assert!(
        pairing.developer().is_unknown(),
        "a vendor's support list says nothing about who developed the model, and must not \
         be allowed to fill the developer in: {:?}",
        pairing.developer()
    );
}

/// Line 560, and the mutation this phase exists to fail on: an
/// unattributed model must not answer `vendor-native`, and must not be
/// promoted by the wire either.
#[test]
fn an_unattributed_model_is_unknown_even_on_the_harnesss_own_wire() {
    let mut q = query(IntegrationId::ClaudeCode, "stealth-alpha");
    q.route.provider = Some("openrouter".to_owned());
    q.route.protocol = Some(WireProtocol::AnthropicMessages);
    let pairing = classify(&q, &none());

    assert_eq!(pairing.class(), PairingClass::Unknown);
    assert!(pairing.developer().is_unknown());
    assert_eq!(pairing.family(), None);
    // The wire is still described. The two answers are separate, and the
    // separation is line 559.
    assert_eq!(pairing.protocol_fit(), ProtocolFit::Native);
}

/// Line 560 again, in the form that actually shows up: a model whose name
/// carries a company's. Nothing may read `anthropic/` as an attribution.
#[test]
fn a_model_named_after_a_company_is_not_attributed_to_it() {
    let pairing = classify(
        &query(IntegrationId::ClaudeCode, "anthropic/claude-fable-5"),
        &none(),
    );
    assert!(
        pairing.developer().is_unknown(),
        "a routing prefix is a name, not an attribution: {:?}",
        pairing.developer()
    );
    assert_eq!(pairing.class(), PairingClass::Unknown);
}

/// Line 555, from the other side: the serving provider is stored, and it
/// is never the answer to who developed the model.
#[test]
fn the_serving_provider_never_becomes_the_developer() {
    let mut q = query(IntegrationId::ClaudeCode, "unlisted-model-v1");
    q.route.provider = Some("anthropic".to_owned());
    q.route.protocol = Some(WireProtocol::AnthropicMessages);
    let pairing = classify(&q, &none());

    assert_eq!(pairing.route().provider.as_deref(), Some("anthropic"));
    assert!(
        pairing.developer().is_unknown(),
        "a provider called `anthropic` is a service, not an author: {:?}",
        pairing.developer()
    );
    assert_eq!(pairing.class(), PairingClass::Unknown);
}

/// The same model, reached two ways. The class is a fact about the
/// harness and the model; the route is stored beside it and does not move
/// the class — which is line 554's independence made observable.
#[test]
fn a_reseller_in_front_of_a_native_model_does_not_change_the_class() {
    let direct = classify(&query(IntegrationId::ClaudeCode, "claude-fable-5"), &none());

    let mut resold = query(IntegrationId::ClaudeCode, "claude-fable-5");
    resold.route.provider = Some("openrouter".to_owned());
    resold.route.gateway = Some("glasshouse".to_owned());
    resold.route.protocol = Some(WireProtocol::AnthropicMessages);
    let resold = classify(&resold, &none());

    assert_eq!(direct.class(), PairingClass::VendorNative);
    assert_eq!(resold.class(), PairingClass::VendorNative);
    assert_eq!(resold.route().provider.as_deref(), Some("openrouter"));
    assert_eq!(resold.route().gateway.as_deref(), Some("glasshouse"));
    assert_eq!(resold.developer().slug(), Some("anthropic"));
}

/// Line 559: three axes, and they disagree. Every built-in provider
/// template declares tool calls `Unverified`, so a pairing on a harness's
/// own wire is `Native` on one axis and unestablished on the other two.
#[test]
fn the_three_compatibility_axes_are_answered_separately() {
    let mut q = query(IntegrationId::ClaudeCode, "claude-fable-5");
    q.route.protocol = Some(WireProtocol::AnthropicMessages);
    q.tool_calls = Declared::Unverified;
    let pairing = classify(&q, &none());

    assert_eq!(pairing.protocol_fit(), ProtocolFit::Native);
    assert_eq!(pairing.model_behaviour(), ModelBehaviourFit::Unverified);
    assert_eq!(pairing.tool_semantics(), ToolSemantics::Unverified);
}

/// And they disagree in the other direction too: a provider that is known
/// *not* to carry tool calls on a protocol the harness speaks natively.
/// A single "compatible" verdict could not say this.
#[test]
fn a_native_protocol_does_not_make_tool_calls_or_behaviour_verified() {
    let mut q = query(IntegrationId::ClaudeCode, "claude-fable-5");
    q.route.protocol = Some(WireProtocol::AnthropicMessages);
    q.tool_calls = Declared::verified(false, "the provider's own documentation says so");
    let pairing = classify(&q, &none());

    assert_eq!(pairing.protocol_fit(), ProtocolFit::Native);
    assert_eq!(pairing.tool_semantics(), ToolSemantics::KnownAbsent);
    assert_eq!(pairing.model_behaviour(), ModelBehaviourFit::Unverified);
}

/// The protocol rungs, for an attributed model with no vendor
/// relationship. Codex speaks OpenAI Responses; a provider serving only
/// OpenAI chat completions is compatible by way of nothing here, but a
/// provider that also serves Responses is.
#[test]
fn the_protocol_rungs_separate_native_from_compatible_from_incompatible() {
    let mut native = query(IntegrationId::Codex, "claude-fable-5");
    native.route.protocol = Some(WireProtocol::OpenAiResponses);
    assert_eq!(
        classify(&native, &none()).class(),
        PairingClass::ProtocolNative
    );

    let mut compatible = query(IntegrationId::Codex, "claude-fable-5");
    compatible.route.protocol = Some(WireProtocol::OpenAiChat);
    compatible.provider_protocols = vec![WireProtocol::OpenAiChat, WireProtocol::OpenAiResponses];
    let compatible = classify(&compatible, &none());
    assert_eq!(compatible.protocol_fit(), ProtocolFit::Compatible);
    assert_eq!(compatible.class(), PairingClass::ProtocolCompatible);

    // T2 made Codex on an Anthropic-only route a *translated* pairing,
    // and T2b made openai-chat <-> openai-responses one too — Codex's
    // whole repertoire now classifies Native/Compatible/Translated, so
    // the incompatible witness moves to OpenCode (openai-chat) on an
    // Anthropic-only route: openai-chat -> anthropic-messages is the one
    // refused row left.
    let mut translated = query(IntegrationId::Codex, "claude-fable-5");
    translated.route.protocol = Some(WireProtocol::AnthropicMessages);
    translated.provider_protocols = vec![WireProtocol::AnthropicMessages];
    let translated = classify(&translated, &none());
    assert_eq!(translated.protocol_fit(), ProtocolFit::Translated);
    assert_eq!(translated.class(), PairingClass::ProtocolTranslated);

    let mut incompatible = query(IntegrationId::OpenCode, "claude-fable-5");
    incompatible.route.protocol = Some(WireProtocol::AnthropicMessages);
    incompatible.provider_protocols = vec![WireProtocol::AnthropicMessages];
    let incompatible = classify(&incompatible, &none());
    assert_eq!(incompatible.protocol_fit(), ProtocolFit::Incompatible);
    assert_eq!(incompatible.class(), PairingClass::Unknown);
}

/// Exactly the pairs the gateway's translation table supports are
/// translated — five today: an Anthropic Messages harness served from
/// an OpenAI Chat upstream (T1, 2026-08-31), both directions of
/// Anthropic Messages <-> OpenAI Responses (T2, 2026-08-31, each behind
/// its own end-to-end test in `tests/gateway_translate_responses.rs`),
/// and both directions of OpenAI Chat <-> OpenAI Responses (T2b,
/// 2026-08-31, each behind its own end-to-end test in
/// `tests/gateway_translate_t2b.rs`). If this fails, a codec pair was
/// added or removed; the pair table in `gateway::translate` and
/// `docs/product/evidence/phase-56.md` are what should be re-read, and
/// this pin updated with them.
#[test]
fn exactly_the_supported_pairs_are_translated() {
    let supported = [
        (WireProtocol::AnthropicMessages, WireProtocol::OpenAiChat),
        (
            WireProtocol::AnthropicMessages,
            WireProtocol::OpenAiResponses,
        ),
        (
            WireProtocol::OpenAiResponses,
            WireProtocol::AnthropicMessages,
        ),
        (WireProtocol::OpenAiChat, WireProtocol::OpenAiResponses),
        (WireProtocol::OpenAiResponses, WireProtocol::OpenAiChat),
        // T3 (2026-09-02), each behind its own end-to-end test in
        // `tests/gateway_translate_gemini.rs`. Nothing translates OUT of
        // Gemini: no installed harness speaks it at the ingress, which
        // is what those rows are refused for.
        (
            WireProtocol::AnthropicMessages,
            WireProtocol::GeminiGenerateContent,
        ),
        (
            WireProtocol::OpenAiResponses,
            WireProtocol::GeminiGenerateContent,
        ),
        (
            WireProtocol::OpenAiChat,
            WireProtocol::GeminiGenerateContent,
        ),
    ];
    for from in [
        WireProtocol::AnthropicMessages,
        WireProtocol::OpenAiResponses,
        WireProtocol::OpenAiChat,
        WireProtocol::GeminiGenerateContent,
    ] {
        for to in [
            WireProtocol::AnthropicMessages,
            WireProtocol::OpenAiResponses,
            WireProtocol::OpenAiChat,
            WireProtocol::GeminiGenerateContent,
        ] {
            assert_eq!(
                crate::provider::translation_available(from, to),
                supported.contains(&(from, to)),
                "{from} -> {to}: the translation table disagrees with this pin"
            );
        }
    }
}

/// T1's own shipped pairing: Claude Code (which speaks
/// `anthropic-messages`) served by a route that serves only
/// `openai-chat` — OpenRouter and every OpenAI-compatible key. The
/// translation arm used to ask the table backwards
/// (`translation_available(route, spoken)`), which looked up the
/// refused reverse row (`openai-chat -> anthropic-messages`) and
/// answered `Incompatible` for the pairing the gateway translates every
/// day. This is the witness for the direction fix: the arm must ask
/// `translation_available(spoken, route)`.
#[test]
fn a_harness_speaking_anthropic_messages_on_a_chat_only_route_is_translated() {
    // A model attributed to a different vendor than Claude Code's own,
    // so the pairing falls through to the protocol rungs instead of
    // being decided by `VendorNative` first (line 560).
    let mut q = query(IntegrationId::ClaudeCode, "gpt-5.5");
    q.route.protocol = Some(WireProtocol::OpenAiChat);
    q.provider_protocols = vec![WireProtocol::OpenAiChat];
    let pairing = classify(&q, &none());
    assert_eq!(pairing.protocol_fit(), ProtocolFit::Translated);
    assert_eq!(pairing.class(), PairingClass::ProtocolTranslated);
}

/// The asymmetric witness that the fix is not a blanket "either
/// direction supported" flip. OpenCode speaks `openai-chat`; a route
/// serving only `anthropic-messages` looks up `openai-chat ->
/// anthropic-messages`, which the table refuses (`NOT_YET_REVERSE`)
/// even though the opposite direction (`anthropic-messages ->
/// openai-chat`, T1's pairing above) is supported. A classifier that
/// answered `Translated` whenever *either* direction is supported would
/// wrongly translate this pairing too.
#[test]
fn a_harness_speaking_openai_chat_on_an_anthropic_only_route_stays_incompatible() {
    let mut q = query(IntegrationId::OpenCode, "claude-fable-5");
    q.route.protocol = Some(WireProtocol::AnthropicMessages);
    q.provider_protocols = vec![WireProtocol::AnthropicMessages];
    let pairing = classify(&q, &none());
    assert_eq!(pairing.protocol_fit(), ProtocolFit::Incompatible);
}

/// The translation seam is *asked*, not assumed absent. A classifier that
/// hard-coded "nothing translates" would pass every other test in this
/// file and would silently ignore the first adapter anyone adds.
#[test]
fn the_classifier_asks_the_one_function_that_owns_translation() {
    let code = include_str!("mod.rs")
        .split("#[cfg(test)]")
        .next()
        .expect("split always yields at least one part");
    assert!(
        code.contains("crate::provider::translation_available("),
        "harness/pairing.rs no longer calls provider::translation_available: the \
         protocol-translated class has stopped being decided by the seam that owns it"
    );
}

/// A launch profile that names no model. Nothing may fill it in from the
/// harness's publisher.
#[test]
fn a_harness_default_model_is_not_the_harness_vendors_model() {
    let q = PairingQuery {
        harness: IntegrationId::ClaudeCode,
        model: AssignedModel::HarnessDefault,
        route: ServingRoute {
            provider: None,
            gateway: None,
            protocol: Some(WireProtocol::AnthropicMessages),
        },
        tool_calls: Declared::Unverified,
        provider_protocols: Vec::new(),
    };
    let pairing = classify(&q, &none());

    assert_eq!(pairing.class(), PairingClass::Unknown);
    assert!(pairing.developer().is_unknown());
    assert_eq!(
        pairing.harness_vendor().value(),
        Some(&Vendor::Anthropic),
        "the harness's publisher is still known; it is simply not an attribution"
    );
}

/// Line 561: a correction in configuration changes the answer, and
/// nothing in the classifier had to be edited to make it.
#[test]
fn a_user_correction_attributes_a_model_the_catalogue_never_read() {
    let mut q = query(IntegrationId::ClaudeCode, "z-ai/glm-4.6");
    q.route.provider = Some("openrouter".to_owned());
    q.route.protocol = Some(WireProtocol::AnthropicMessages);

    let before = classify(&q, &none());
    assert_eq!(before.class(), PairingClass::Unknown);

    let mut models = BTreeMap::new();
    models.insert(
        "z-ai/glm-4.6".to_owned(),
        ModelCorrection {
            developer: Some(ModelDeveloper::named("zhipu-ai")),
            family: Some("glm".to_owned()),
            behaviour: None,
        },
    );
    let overrides =
        PairingOverrides::from_parts("the user configuration file", models, BTreeMap::new());

    let after = classify(&q, &overrides);
    assert_eq!(after.class(), PairingClass::ProtocolNative);
    assert_eq!(after.developer().slug(), Some("zhipu-ai"));
    assert_eq!(after.family(), Some("glm"));
    assert!(matches!(
        after.attribution().source,
        AttributionSource::Correction { .. }
    ));
}

/// The other half of line 561, and of 562: a harness's official support
/// list is data, and a person can correct it when a release outruns
/// Glasshouse.
#[test]
fn a_user_correction_can_add_official_support_a_release_has_not_shipped() {
    let q = query(IntegrationId::ClaudeCode, "opus");
    assert_eq!(classify(&q, &none()).class(), PairingClass::VendorNative);

    let mut harnesses = BTreeMap::new();
    harnesses.insert(
        "claude-code".to_owned(),
        SupportCorrection {
            native_families: Some(Vec::new()),
            supported_models: Some(vec!["opus".to_owned()]),
        },
    );
    let overrides =
        PairingOverrides::from_parts("the user configuration file", BTreeMap::new(), harnesses);

    let corrected = classify(&q, &overrides);
    assert_eq!(corrected.class(), PairingClass::VendorSupported);
    assert_eq!(corrected.developer().slug(), Some("anthropic"));
}

/// A person can record what a pairing actually did, on the axis nothing
/// measures yet — and it must not move the other two.
#[test]
fn a_behaviour_correction_moves_one_axis_and_only_one() {
    let mut q = query(IntegrationId::ClaudeCode, "claude-fable-5");
    q.route.protocol = Some(WireProtocol::AnthropicMessages);

    let mut models = BTreeMap::new();
    models.insert(
        "claude-fable-5".to_owned(),
        ModelCorrection {
            developer: None,
            family: None,
            behaviour: Some(ModelBehaviourFit::KnownAbsent),
        },
    );
    let overrides =
        PairingOverrides::from_parts("this project's configuration file", models, BTreeMap::new());

    let pairing = classify(&q, &overrides);
    assert_eq!(pairing.model_behaviour(), ModelBehaviourFit::KnownAbsent);
    assert_eq!(pairing.protocol_fit(), ProtocolFit::Native);
    assert_eq!(pairing.tool_semantics(), ToolSemantics::Unverified);
    // The class is about the vendor relationship, which a behaviour note
    // does not change.
    assert_eq!(pairing.class(), PairingClass::VendorNative);
}

/// Line 562, mechanically: every harness's declared support is data an
/// adapter states with evidence, exactly like every other declaration in
/// this module.
#[test]
fn every_declared_support_list_cites_its_evidence() {
    for adapter in super::super::all() {
        let support = adapter.official_model_support();
        for (what, declared) in [
            ("native families", support.native_families.evidence()),
            ("supported models", support.supported_models.evidence()),
        ] {
            if let Some(evidence) = declared {
                assert!(
                    evidence.len() > 30,
                    "{:?}'s {what} declaration cites `{evidence}`, which is too short to \
                     be a citation anybody could re-check",
                    adapter.id()
                );
            }
        }
    }
}

/// A vendor whose own model line nothing established can never produce a
/// vendor-native pairing, whatever its adapter declares. The table is the
/// only comparison between a harness vendor and a model developer, and it
/// is empty for five of the eight — `Vendor::Glasshouse` included, since
/// publishing a harness is not developing a model.
#[test]
fn a_vendor_with_no_established_model_line_is_never_native() {
    for vendor in [
        Vendor::Cursor,
        Vendor::OpenCode,
        Vendor::Pi,
        Vendor::Hermes,
        Vendor::Glasshouse,
    ] {
        assert_eq!(
            vendor_organisation(vendor),
            None,
            "{vendor} claims a model-developing organisation nothing established"
        );
    }
    assert_eq!(vendor_organisation(Vendor::Anthropic), Some("anthropic"));
    assert_eq!(vendor_organisation(Vendor::OpenAi), Some("openai"));
    assert_eq!(vendor_organisation(Vendor::Google), Some("google"));
}

/// Cursor CLI names three models it supports and no family of its own, so
/// its best answer is vendor-supported and never vendor-native.
#[test]
fn a_harness_with_no_native_family_still_reaches_vendor_supported() {
    let pairing = classify(&query(IntegrationId::Cursor, "claude-opus-4-8"), &none());
    assert_eq!(pairing.class(), PairingClass::VendorSupported);
    assert!(
        pairing.developer().is_unknown(),
        "nothing here read who developed `claude-opus-4-8`"
    );
}

/// Every catalogue entry is exact, and nothing in it was derived from
/// another entry's stem.
#[test]
fn the_catalogue_matches_ids_exactly_and_cites_every_one() {
    for entry in catalogue() {
        assert!(
            entry.evidence.len() > 30,
            "`{}` cites `{}`, which is too short to be a citation",
            entry.id,
            entry.evidence
        );
        assert_eq!(catalogued(entry.id).map(|e| e.id), Some(entry.id));
    }
    assert!(catalogued("claude-fable-5-turbo").is_none());
    assert!(catalogued("openrouter/opus").is_none());
}

/// Line 572: the same nominal model through a different gateway is
/// different evidence. A key built from two routes that differ only in
/// `gateway` must not compare equal.
#[test]
fn an_evidence_key_separates_the_same_model_across_gateways() {
    let direct = ServingRoute {
        provider: Some("openrouter".to_owned()),
        ..ServingRoute::default()
    };
    let mut gatewayed = direct.clone();
    gatewayed.gateway = Some("glasshouse".to_owned());

    let a = EvidenceKey::new(
        IntegrationId::ClaudeCode,
        "default",
        AssignedModel::named("claude-fable-5"),
        direct,
    );
    let b = EvidenceKey::new(
        IntegrationId::ClaudeCode,
        "default",
        AssignedModel::named("claude-fable-5"),
        gatewayed,
    );
    assert_ne!(
        a, b,
        "the same model through a gateway must be a different evidence key"
    );
}

/// The same line, for a protocol translation rather than a gateway: two
/// routes that differ only in wire protocol are different evidence too.
#[test]
fn an_evidence_key_separates_the_same_model_across_protocols() {
    let anthropic = ServingRoute {
        protocol: Some(WireProtocol::AnthropicMessages),
        ..ServingRoute::default()
    };
    let openai = ServingRoute {
        protocol: Some(WireProtocol::OpenAiChat),
        ..ServingRoute::default()
    };

    let a = EvidenceKey::new(
        IntegrationId::Codex,
        "default",
        AssignedModel::named("some-model"),
        anthropic,
    );
    let b = EvidenceKey::new(
        IntegrationId::Codex,
        "default",
        AssignedModel::named("some-model"),
        openai,
    );
    assert_ne!(a, b);
}

/// And the identical route, harness, profile and model produce an equal
/// key — the positive case that guards against an over-eager distinction.
#[test]
fn an_evidence_key_is_equal_for_an_identical_route() {
    let route = ServingRoute {
        provider: Some("openrouter".to_owned()),
        gateway: None,
        protocol: Some(WireProtocol::AnthropicMessages),
    };
    let a = EvidenceKey::new(
        IntegrationId::ClaudeCode,
        "default",
        AssignedModel::named("claude-fable-5"),
        route.clone(),
    );
    let b = EvidenceKey::new(
        IntegrationId::ClaudeCode,
        "default",
        AssignedModel::named("claude-fable-5"),
        route,
    );
    assert_eq!(a, b);
}
