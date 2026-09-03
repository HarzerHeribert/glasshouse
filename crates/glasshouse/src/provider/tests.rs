use super::*;

fn support(protocol: WireProtocol, base_url: &str) -> ProtocolSupport {
    unverified_support(protocol, base_url)
}

// --- protocol support is per protocol, never inferred ---------------

#[test]
fn a_provider_may_serve_more_than_one_protocol() {
    let provider = Provider {
        name: "test-multi".to_owned(),
        protocols: vec![
            support(WireProtocol::OpenAiChat, "https://a.example/v1"),
            support(
                WireProtocol::AnthropicMessages,
                "https://a.example/anthropic",
            ),
        ],
        model_list_endpoint: Declared::Unverified,
        usage_telemetry: Declared::Unverified,
        credential_env: vec!["A_EXAMPLE_KEY".to_owned()],
        headers: vec![],
    };
    assert!(provider.serves(WireProtocol::OpenAiChat).is_some());
    assert!(provider.serves(WireProtocol::AnthropicMessages).is_some());
    assert!(provider.serves(WireProtocol::OpenAiResponses).is_none());
}

#[test]
fn each_protocol_carries_its_own_base_url() {
    let provider = Provider {
        name: "test-split".to_owned(),
        protocols: vec![
            support(WireProtocol::OpenAiChat, "https://a.example/v1/chat"),
            support(
                WireProtocol::AnthropicMessages,
                "https://a.example/v1/anthropic",
            ),
        ],
        model_list_endpoint: Declared::Unverified,
        usage_telemetry: Declared::Unverified,
        credential_env: vec![],
        headers: vec![],
    };
    let chat = provider.serves(WireProtocol::OpenAiChat).unwrap();
    let anthropic = provider.serves(WireProtocol::AnthropicMessages).unwrap();
    assert_eq!(chat.base_url, "https://a.example/v1/chat");
    assert_eq!(anthropic.base_url, "https://a.example/v1/anthropic");
    assert_ne!(chat.base_url, anthropic.base_url);
}

/// Line 408. Serving OpenAI Chat says nothing about serving OpenAI
/// Responses; they are two protocols that happen to share a vendor.
///
/// This test used to make the point with the `openrouter` template,
/// which declared Chat and not Responses. It cannot any more:
/// OpenRouter's Responses route was **separately established** — an
/// empty-body `POST` to `https://openrouter.ai/api/v1/responses`
/// answered `400` where an unknown path answered `404` — so that
/// template now declares both, and it declares them because each was
/// probed rather than because one implied the other. Using it here would
/// have quietly turned this rule into a description of one provider's
/// catalogue.
///
/// So the rule is asserted against a provider built to serve Chat and
/// nothing else, and against a real template that is still exactly that
/// — which is what makes this a rule rather than a snapshot.
#[test]
fn openai_chat_support_never_implies_openai_responses() {
    let chat_only = Provider {
        name: "chat-only".to_owned(),
        protocols: vec![support(WireProtocol::OpenAiChat, "https://a.example/v1")],
        model_list_endpoint: Declared::Unverified,
        usage_telemetry: Declared::Unverified,
        credential_env: vec![],
        headers: vec![],
    };
    assert!(chat_only.serves(WireProtocol::OpenAiChat).is_some());
    assert!(
        chat_only.serves(WireProtocol::OpenAiResponses).is_none(),
        "a provider serving only openai-chat must not answer for openai-responses"
    );

    let unorouter = template("unorouter").expect("unorouter is a built-in template");
    assert!(unorouter.serves(WireProtocol::OpenAiChat).is_some());
    assert!(
        unorouter.serves(WireProtocol::OpenAiResponses).is_none(),
        "a template whose Responses route nobody has probed must not declare one"
    );
}

#[test]
fn neither_openai_protocol_ever_satisfies_anthropic_messages() {
    // Line 409.
    let openai_chat_only = Provider {
        name: "chat-only".to_owned(),
        protocols: vec![support(WireProtocol::OpenAiChat, "https://a.example/v1")],
        model_list_endpoint: Declared::Unverified,
        usage_telemetry: Declared::Unverified,
        credential_env: vec![],
        headers: vec![],
    };
    let openai_responses_only = Provider {
        name: "responses-only".to_owned(),
        protocols: vec![support(
            WireProtocol::OpenAiResponses,
            "https://a.example/v1",
        )],
        model_list_endpoint: Declared::Unverified,
        usage_telemetry: Declared::Unverified,
        credential_env: vec![],
        headers: vec![],
    };
    assert!(
        openai_chat_only
            .serves(WireProtocol::AnthropicMessages)
            .is_none()
    );
    assert!(
        openai_responses_only
            .serves(WireProtocol::AnthropicMessages)
            .is_none()
    );
}

/// Line 410 and Phase 56: translation is never implicit. Every ordered
/// pair of `WireProtocol` — including each with itself — has exactly one
/// row in the gateway's table, exactly the pairs with end-to-end tests
/// are supported, and a protocol is never "translated" to itself.
/// The table may not name the enum, so this is the test that holds it
/// complete against it.
#[test]
fn every_wire_protocol_pair_has_exactly_one_row_in_the_gateway_table() {
    const ALL: [WireProtocol; 4] = [
        WireProtocol::AnthropicMessages,
        WireProtocol::OpenAiResponses,
        WireProtocol::OpenAiChat,
        WireProtocol::GeminiGenerateContent,
    ];
    let table = crate::gateway::translate::pairs();
    assert_eq!(table.len(), ALL.len() * ALL.len());
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
        // T3, each behind its own end-to-end test in
        // `tests/gateway_translate_gemini.rs`. Nothing translates OUT of
        // Gemini: no installed harness speaks it at the ingress.
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
    for &from in &ALL {
        for &to in &ALL {
            let rows = table
                .iter()
                .filter(|pair| pair.from == from.slug() && pair.to == to.slug())
                .count();
            assert_eq!(rows, 1, "{from} -> {to} has {rows} rows");
            assert_eq!(
                translation_available(from, to),
                supported.contains(&(from, to)),
                "{from} -> {to}"
            );
        }
    }
}

/// This is deliberately a test-only stand-in for the model-quality
/// scorer Phase 34 will add. Its input is the filtered wrapper, not a
/// raw provider slice: attempting to call it with `&providers` does not
/// type-check. That makes compatibility filtering structurally precede
/// ranking without inventing a production scorer today.
fn rank_fixture_quality(candidates: &ProtocolCompatibleProviders<'_>) -> Vec<String> {
    let mut ranked: Vec<(u8, String)> = candidates
        .iter()
        .map(|candidate| {
            let quality = match candidate.name() {
                "incompatible-but-best" => 100,
                "responses-without-url" => 99,
                "compatible" => 1,
                other => panic!("unexpected candidate reached ranking: {other}"),
            };
            (quality, candidate.name().to_owned())
        })
        .collect();
    ranked.sort_by_key(|(quality, _)| std::cmp::Reverse(*quality));
    ranked.into_iter().map(|(_, name)| name).collect()
}

#[test]
fn protocol_compatibility_filters_the_candidate_set_before_fixture_quality_ranking() {
    let providers = vec![
        Provider {
            name: "incompatible-but-best".to_owned(),
            protocols: vec![support(WireProtocol::OpenAiChat, "https://chat.example/v1")],
            model_list_endpoint: Declared::Unverified,
            usage_telemetry: Declared::Unverified,
            credential_env: vec![],
            headers: vec![],
        },
        Provider {
            name: "responses-without-url".to_owned(),
            protocols: vec![support(WireProtocol::OpenAiResponses, "")],
            model_list_endpoint: Declared::Unverified,
            usage_telemetry: Declared::Unverified,
            credential_env: vec![],
            headers: vec![],
        },
        Provider {
            name: "compatible".to_owned(),
            protocols: vec![support(
                WireProtocol::OpenAiResponses,
                "https://responses.example/v1",
            )],
            model_list_endpoint: Declared::Unverified,
            usage_telemetry: Declared::Unverified,
            credential_env: vec![],
            headers: vec![],
        },
    ];

    let candidates =
        ProtocolCompatibleProviders::for_protocol(&providers, WireProtocol::OpenAiResponses);
    let names: std::collections::BTreeSet<&str> = candidates
        .iter()
        .map(ProtocolCompatibleProvider::name)
        .collect();
    assert_eq!(names, std::collections::BTreeSet::from(["compatible"]));

    // The two excluded providers would win a quality-only score. They
    // cannot reach it: ranking accepts the filtered wrapper above, and
    // sees only the one compatible provider.
    assert_eq!(rank_fixture_quality(&candidates), vec!["compatible"]);
}

// --- built-in templates ----------------------------------------------

#[test]
fn every_built_in_template_declares_a_protocol_and_a_base_url() {
    for provider in templates() {
        assert!(
            !provider.protocols.is_empty(),
            "{} declares no protocol at all",
            provider.name
        );
        let is_generic = GENERIC_TEMPLATE_NAMES.contains(&provider.name.as_str());
        for protocol in &provider.protocols {
            if is_generic {
                assert!(
                    protocol.base_url.is_empty(),
                    "{} is a generic template; its base URL must be empty (user-supplied), \
                     found {:?}",
                    provider.name,
                    protocol.base_url
                );
            } else {
                assert!(
                    !protocol.base_url.is_empty(),
                    "{} is not a generic template and must declare a real base URL",
                    provider.name
                );
            }
        }
    }
}

/// OpenRouter is the one configured provider that can back every
/// ingress the Phase 9G gateway serves, and this pins each protocol to
/// the base URL a request for it is actually appended to.
///
/// The three URLs are not interchangeable and the difference is not
/// cosmetic: an OpenAI-shaped client appends `/responses` or
/// `/chat/completions` to its base URL, so those two carry the `/v1`,
/// while Claude Code appends `/v1/messages` itself, so Anthropic
/// Messages must be the root or the `/v1` is doubled. A gateway holding
/// one base URL for all three would have to get two of them wrong.
///
/// Lose this and the Responses entry can silently acquire the Anthropic
/// root, and every Codex request through the gateway goes to
/// `https://openrouter.ai/api/responses` — a path the service answers
/// `404` for, which the harness would report as a model error.
#[test]
fn openrouter_declares_every_gateway_ingress_protocol_at_its_own_base_url() {
    let openrouter = template("openrouter").expect("openrouter is a built-in template");
    for (protocol, base_url) in [
        (
            WireProtocol::OpenAiResponses,
            "https://openrouter.ai/api/v1",
        ),
        (WireProtocol::OpenAiChat, "https://openrouter.ai/api/v1"),
        (WireProtocol::AnthropicMessages, "https://openrouter.ai/api"),
    ] {
        let support = openrouter
            .serves(protocol)
            .unwrap_or_else(|| panic!("openrouter must serve {protocol}"));
        assert_eq!(
            support.base_url, base_url,
            "{protocol} is declared at the wrong base URL"
        );
    }

    // ... and the entry is honest about what the probe established: a
    // route exists. Nothing was learned about streaming, tool calls or
    // reasoning, so nothing may claim to have been.
    let responses = openrouter
        .serves(WireProtocol::OpenAiResponses)
        .expect("checked above");
    assert_eq!(responses.streaming, Declared::Unverified);
    assert_eq!(responses.tool_calls, Declared::Unverified);
    assert_eq!(responses.reasoning, Declared::Unverified);
}

/// The list is empty today — see [`DELIBERATELY_UNTEMPLATED`]'s own doc
/// for why that is a statement rather than a gap.
///
/// An empty list would make the loop below vacuous, so the control after
/// it is not decoration: it is the same device as the `404` control in
/// the Responses probe, and it is what proves the check can still fail
/// while there is nothing in the list for it to catch.
#[test]
fn no_template_exists_for_a_service_whose_endpoint_is_unestablished() {
    let names: Vec<String> = templates().into_iter().map(|p| p.name).collect();
    for (name, reason) in DELIBERATELY_UNTEMPLATED {
        assert!(
            !names.iter().any(|n| n == name),
            "`{name}` must not appear in templates(): {reason}"
        );
    }
    assert!(
        !names
            .iter()
            .any(|n| n == "a-service-whose-endpoint-nobody-has-read"),
        "the control case must not match a real template, or this check proves nothing"
    );
    assert!(
        names.iter().any(|n| n == "openrouter"),
        "and the list of names must be non-empty, or the control above would pass \
         against nothing at all"
    );
}

#[test]
fn an_unestablished_capability_is_unverified_rather_than_assumed() {
    let mut verified: Vec<String> = Vec::new();
    for provider in templates() {
        if provider.model_list_endpoint.is_verified() {
            verified.push(format!("{}.model_list_endpoint", provider.name));
        }
        if provider.usage_telemetry.is_verified() {
            verified.push(format!("{}.usage_telemetry", provider.name));
        }
        for protocol in &provider.protocols {
            if protocol.streaming.is_verified() {
                verified.push(format!("{}.{}.streaming", provider.name, protocol.protocol));
            }
            if protocol.tool_calls.is_verified() {
                verified.push(format!(
                    "{}.{}.tool_calls",
                    provider.name, protocol.protocol
                ));
            }
            if protocol.reasoning.is_verified() {
                verified.push(format!("{}.{}.reasoning", provider.name, protocol.protocol));
            }
        }
    }
    assert_eq!(
        verified,
        vec![
            "openrouter.model_list_endpoint".to_owned(),
            // Phase 32B, and the only non-`model_list_endpoint` entry
            // here: the route exists and gates on authentication, proved
            // with a `404` control on the same host. What it answers with
            // is still unread, and no schema is declared anywhere.
            "openrouter.usage_telemetry".to_owned(),
            "unorouter.model_list_endpoint".to_owned(),
            "anyrouter.model_list_endpoint".to_owned(),
            "kilo.model_list_endpoint".to_owned(),
            "nous.model_list_endpoint".to_owned(),
            // PACKET-QUOTA-LIVE: measured live by the orchestrator,
            // 2026-08-27 — see the `groq` template's own comment.
            "groq.model_list_endpoint".to_owned(),
            "litellm.model_list_endpoint".to_owned(),
        ],
        "only a capability someone actually probed may be Verified, and every other one \
         must be Unverified — a `GET /models` that answered says nothing about \
         streaming, tool calls or reasoning, and z.ai's 401 says nothing even about the \
         model list, because it answers 401 to every path under that prefix. The one \
         usage_telemetry entry is OpenRouter's, whose /api/v1/key and /api/v1/credits \
         answered 401 while an invented sibling path answered 404 on the same host: \
         {verified:?}"
    );
}

#[test]
fn a_provider_may_declare_several_credential_variable_names() {
    let provider = Provider {
        name: "multi-key".to_owned(),
        protocols: vec![support(WireProtocol::OpenAiChat, "https://a.example/v1")],
        model_list_endpoint: Declared::Unverified,
        usage_telemetry: Declared::Unverified,
        credential_env: vec![
            "A_EXAMPLE_KEY".to_owned(),
            "A_EXAMPLE_KEY_BACKUP".to_owned(),
        ],
        headers: vec![],
    };
    assert_eq!(provider.credential_env.len(), 2);
    assert!(
        provider
            .credential_env
            .contains(&"A_EXAMPLE_KEY".to_owned())
    );
    assert!(
        provider
            .credential_env
            .contains(&"A_EXAMPLE_KEY_BACKUP".to_owned())
    );
}

#[test]
fn template_looks_up_by_name_and_is_none_for_unknown_names() {
    assert!(template("openrouter").is_some());
    assert!(template("not-a-real-provider").is_none());
}

// --- line 415: NVIDIA ------------------------------------------------

#[test]
fn nvidia_serves_chat_only_at_the_documented_base_url() {
    let nvidia = template("nvidia").expect("nvidia is a built-in template");
    let chat = nvidia
        .serves(WireProtocol::OpenAiChat)
        .expect("nvidia serves openai-chat");
    assert_eq!(chat.base_url, "https://integrate.api.nvidia.com/v1");
    assert!(
        nvidia.serves(WireProtocol::OpenAiResponses).is_none(),
        "no Responses endpoint was established for NVIDIA — a provider serving only \
         openai-chat must not answer for openai-responses"
    );
    assert_eq!(nvidia.credential_env, vec!["NVIDIA_API_KEY".to_owned()]);
    assert!(nvidia.headers.is_empty());
}

// --- Groq (PACKET-QUOTA-LIVE) -----------------------------------------

#[test]
fn groq_serves_chat_only_at_the_live_base_url() {
    let groq = template("groq").expect("groq is a built-in template");
    let chat = groq
        .serves(WireProtocol::OpenAiChat)
        .expect("groq serves openai-chat");
    assert_eq!(chat.base_url, "https://api.groq.com/openai/v1");
    assert!(
        groq.serves(WireProtocol::OpenAiResponses).is_none(),
        "no Responses endpoint was established for Groq — a provider serving only \
         openai-chat must not answer for openai-responses"
    );
    assert!(
        groq.model_list_endpoint.is_verified(),
        "GET /models was measured live, 200, a real catalogue — this must be Verified"
    );
    assert_eq!(groq.credential_env, vec!["GROQ_API_KEY".to_owned()]);
    assert!(groq.headers.is_empty());
}

/// A provider serving only `openai-chat` cannot back Codex, whose
/// `wire_api` dropped `"chat"` in 0.149.1 — the module documentation's
/// own rule, and NVIDIA's template records the identical consequence.
/// Groq is exactly that shape, so the honest proof is that the routing
/// constraint itself refuses it, not a configuration that would compose
/// today and only fail once Codex started rejecting it.
#[test]
fn groq_alone_cannot_satisfy_a_codex_routed_session() {
    let providers = vec![template("groq").expect("groq is a built-in template")];
    let candidates =
        ProtocolCompatibleProviders::for_protocol(&providers, WireProtocol::OpenAiResponses);
    assert!(
        candidates.is_empty(),
        "Groq declares no openai-responses support, so it must not survive a \
         Codex-shaped (openai-responses) routing filter"
    );
}

// --- line 416: LiteLLM -------------------------------------------------

#[test]
fn litellm_serves_chat_with_a_verified_model_list_and_no_credential_variable() {
    let litellm = template("litellm").expect("litellm is a built-in template");
    let chat = litellm
        .serves(WireProtocol::OpenAiChat)
        .expect("litellm serves openai-chat");
    assert_eq!(chat.base_url, "http://0.0.0.0:4000");
    assert!(
        litellm.model_list_endpoint.is_verified(),
        "LiteLLM's documented GET /models must be Verified, not Unverified"
    );
    assert!(
        litellm.credential_env.is_empty(),
        "LiteLLM declares no dedicated credential variable — the user must name their \
         own via ProviderConfig::set_credential_env, never OPENAI_API_KEY by default"
    );
    assert!(litellm.headers.is_empty());
}

// --- line 353: OpenRouter also serves Anthropic Messages ---------------

#[test]
fn openrouter_also_serves_anthropic_messages_at_the_api_root_with_no_v1() {
    let openrouter = template("openrouter").expect("openrouter is a built-in template");
    let anthropic = openrouter
        .serves(WireProtocol::AnthropicMessages)
        .expect("openrouter must also serve anthropic-messages");
    assert_eq!(
        anthropic.base_url, "https://openrouter.ai/api",
        "Claude Code appends /v1/messages itself, so the configured base URL must be \
         the root with no /v1 suffix"
    );
    // The original protocol is untouched by adding a second one.
    let chat = openrouter
        .serves(WireProtocol::OpenAiChat)
        .expect("openrouter must still serve openai-chat");
    assert_eq!(chat.base_url, "https://openrouter.ai/api/v1");
}

#[test]
fn every_built_in_template_ships_no_header_unless_one_was_established() {
    // Nothing established a required header for any built-in template —
    // inventing one is the same failure as inventing a base URL.
    for provider in templates() {
        assert!(
            provider.headers.is_empty(),
            "{} declares a header nobody established",
            provider.name
        );
    }
}

// --- GH-DEDUP-PROVIDER finding #3: templates() output is pinned --------

/// Pins `templates()`'s `Debug` output, captured **before** the
/// `unverified_provider` refactor that collapses seven of its fourteen
/// `Provider` literals into one call each. `Provider` and every type it
/// contains derive `Debug` deterministically (field order, no `HashMap`),
/// so a byte-for-byte match here is the same claim as "every provider's
/// every field is unchanged" — the proof the packet's finding #3 requires
/// before the literal-collapsing refactor is admissible. The fixture was
/// generated by running `templates()` against the pre-refactor source and
/// writing its `Debug` output verbatim; it is data, not something to hand
/// edit.
///
/// A **new template** is the one change that may edit it, and it edits
/// it by insertion: the `gemini` entry added by Phase 56's T3 package
/// was spliced in at its position in [`templates`] and nothing else in
/// the file was touched, so the other thirteen are still pinned byte for
/// byte against the pre-refactor capture. Regenerating the whole file
/// would silently retire that claim.
#[test]
fn templates_output_is_unchanged_by_the_literal_dedup_refactor() {
    let pinned = include_str!("testdata/templates_pin.txt");
    assert_eq!(
        format!("{:?}", templates()),
        pinned,
        "templates() no longer produces byte-identical output to the \
         pre-refactor fixture — finding #3's refactor must not change \
         what any provider template contains"
    );
}
