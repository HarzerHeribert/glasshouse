//! Provider protocol model (Phase 9C) and built-in provider templates, the
//! declarative half of Phase 9D.
//!
//! # Per protocol, not per provider
//!
//! A [`Provider`] does not answer "does this provider support streaming" —
//! it answers that question once per [`WireProtocol`] it is known to serve,
//! in [`ProtocolSupport`]. A service can speak OpenAI chat completions at one
//! path and nothing else at all, and a provider model that let one protocol's
//! declaration stand in for another's would be the exact mistake this module
//! exists to prevent: [`Provider::serves`] returns `None` for a protocol
//! nothing established, and that `None` is a hard answer, never an
//! invitation to check a neighbouring protocol instead.
//!
//! # Declarations are evidence, not recollection
//!
//! Every capability a [`ProtocolSupport`] or [`Provider`] states beyond its
//! protocol and base URL is a [`crate::harness::Declared`] value, for exactly
//! the reason [`mod@crate::harness`] uses it: "nobody checked" and "verified
//! absent" are different claims, and a router deciding what a provider can be
//! trusted to do needs to be able to tell them apart.
//!
//! # What was actually established, on 2026-08-25
//!
//! Every built-in template in [`templates`] was read from a real installation
//! or the service's own endpoint list on 2026-08-25, exactly once, the same
//! way an adapter in [`mod@crate::harness`] is read from an installed binary.
//! Only OpenRouter's and LiteLLM's model-list endpoints (both a documented,
//! public `GET /models`) were established well enough to declare `Verified`;
//! every other capability nothing was actually established for is
//! `Unverified` — never filled in from what a service probably supports.
//!
//! Two sources were added on the same date, alongside the two above:
//!
//! - **NVIDIA.** `docs.api.nvidia.com/nim/reference/llm-apis` gives base
//!   `https://integrate.api.nvidia.com` with `POST /v1/chat/completions`, and
//!   NVIDIA's own `build.nvidia.com` model pages use
//!   `base_url = "https://integrate.api.nvidia.com/v1"`. No Responses
//!   endpoint was established, so [`templates`]' `nvidia` entry declares
//!   `openai-chat` only — which is also why it cannot back Codex, whose
//!   `wire_api` dropped `"chat"` in 0.149.1.
//! - **LiteLLM.** Its quick-start and `proxy/user_keys` documentation pages
//!   both use exactly `http://0.0.0.0:4000` as the client `base_url` — kept
//!   verbatim rather than "fixed" to `localhost`. Its proxy documentation
//!   also lists `GET /models - available models on server`, which is the
//!   second `Verified` model-list endpoint above.
//! - **OpenRouter serves Anthropic Messages too**, established two
//!   independent ways: an unauthenticated `POST
//!   https://openrouter.ai/api/v1/messages` answers `401`, while `POST
//!   https://openrouter.ai/api/v1/nonexistent-endpoint` under the same prefix
//!   answers `404` — the working control case that turns "the endpoint
//!   exists and wants a credential" into a finding rather than a guess. And
//!   the user's own working launcher (`~/projects/openrouter-clis/bin/claude-or`)
//!   drives real Claude Code against exactly `https://openrouter.ai/api`,
//!   its own comment explaining why: it strips `/v1` from the OpenAI base
//!   URL because Claude Code appends `/v1/messages` itself.
//!
//! **Kilo, Nous and RouterAI are deliberately not templates here.** The user
//! holds a credential for each, but no endpoint has been established for any
//! of the three — no real installation and no documentation page was read for
//! them the way it was for the providers in [`templates`]. A template with an
//! invented base URL would be the same failure Phase 9A already refuses for
//! an invented environment-variable name. All three are reachable today
//! through the generic `openai-compatible` template once someone reads a real
//! endpoint from the service's own documentation; do not "helpfully" guess
//! one in the meantime.

use crate::harness::{Declared, WireProtocol};
use crate::secret::SecretRef;

/// What a provider serves, for ONE protocol.
///
/// Per protocol, not per provider — see the module documentation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProtocolSupport {
    pub protocol: WireProtocol,
    /// This protocol's own base URL. Separate per protocol because a
    /// provider may serve them at different paths.
    pub base_url: String,
    pub streaming: Declared<bool>,
    pub tool_calls: Declared<bool>,
    pub reasoning: Declared<bool>,
}

/// One configured or built-in provider: what it serves, and how a credential
/// for it might be found.
///
/// Holds no credential value anywhere — [`Provider::credential_env`] is
/// environment variable *names* only, exactly like
/// [`crate::harness::HookCommand`] never holding a shell it did not
/// construct itself. See the crate's [`crate::config`] module docs for the
/// same rule applied to how a provider is configured.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Provider {
    pub name: String,
    /// Every protocol this provider is known to serve. A provider may serve
    /// more than one; it may also serve none, which is honest rather than
    /// broken.
    pub protocols: Vec<ProtocolSupport>,
    pub model_list_endpoint: Declared<bool>,
    pub usage_telemetry: Declared<bool>,
    /// The environment variable names this provider's credential may come
    /// from. **Names only — never a value.** More than one is allowed: a
    /// user may hold several keys for the same router.
    pub credential_env: Vec<String>,
    /// Extra HTTP headers this provider needs, as name/value pairs.
    /// Configuration, not credentials: a header VALUE here is written by the
    /// user into their own config file and is not resolved through
    /// `SecretStore`. A provider needing a secret in a header is out of scope
    /// for this line — refuse it rather than smuggling a credential through.
    pub headers: Vec<(String, String)>,
}

impl Provider {
    /// The support entry for `protocol`, or `None`.
    ///
    /// `None` is a hard answer, not an invitation to try a neighbouring
    /// protocol. Nothing may infer `OpenAiResponses` from `OpenAiChat`, or
    /// `AnthropicMessages` from either.
    pub fn serves(&self, protocol: WireProtocol) -> Option<&ProtocolSupport> {
        self.protocols.iter().find(|p| p.protocol == protocol)
    }

    /// One [`SecretRef`] per name in [`Provider::credential_env`], in the
    /// order declared.
    ///
    /// Still names only — a [`SecretRef`] is a reference, and resolving one
    /// into a value is [`crate::secret::SecretStore`]'s job, not this
    /// type's. The point of returning them is that a caller which needs a
    /// credential stops handling bare strings, whose meaning it would
    /// otherwise have to infer.
    ///
    /// Several names yield several references rather than a chosen one:
    /// which key of a pool to use is a routing decision, and this method
    /// refuses to make it silently. A provider with no credential variable
    /// yields none — never a reference to an invented variable name, for
    /// the same reason this module ships no template with an invented base
    /// URL.
    pub fn secret_refs(&self) -> Vec<SecretRef> {
        self.credential_env
            .iter()
            .map(|var| SecretRef::Environment { var: var.clone() })
            .collect()
    }
}

/// Whether an explicit adapter exists to translate `from` into `to`.
///
/// Always `false` today, and that is the capability rather than a gap: V1
/// prefers pass-through, and translation must never happen because two
/// protocols merely looked close. The seam exists so an adapter can be added
/// later for one concrete pair, with its own tests.
pub fn translation_available(from: WireProtocol, to: WireProtocol) -> bool {
    let _ = (from, to);
    false
}

/// `protocol` served at `base_url`, with nothing beyond that established.
fn unverified_support(protocol: WireProtocol, base_url: &str) -> ProtocolSupport {
    ProtocolSupport {
        protocol,
        base_url: base_url.to_owned(),
        streaming: Declared::Unverified,
        tool_calls: Declared::Unverified,
        reasoning: Declared::Unverified,
    }
}

/// The built-in provider templates — see the module documentation for what
/// "built-in" means here and what was deliberately left out.
pub fn templates() -> Vec<Provider> {
    vec![
        Provider {
            name: "openrouter".to_owned(),
            protocols: vec![
                unverified_support(WireProtocol::OpenAiChat, "https://openrouter.ai/api/v1"),
                // See the module documentation's "OpenRouter serves Anthropic
                // Messages too" entry for both sources. The root, with no
                // `/v1` — Claude Code appends `/v1/messages` itself.
                // Streaming, tool_calls and reasoning stay Unverified: only
                // the endpoint's existence was established.
                unverified_support(WireProtocol::AnthropicMessages, "https://openrouter.ai/api"),
            ],
            model_list_endpoint: Declared::verified(
                true,
                "OpenRouter's API reference documents a public, unauthenticated \
                 GET https://openrouter.ai/api/v1/models, read 2026-08-25",
            ),
            usage_telemetry: Declared::Unverified,
            credential_env: vec!["OPENROUTER_API_KEY".to_owned()],
            headers: vec![],
        },
        Provider {
            name: "unorouter".to_owned(),
            protocols: vec![unverified_support(
                WireProtocol::OpenAiChat,
                "https://api.unorouter.com/v1",
            )],
            model_list_endpoint: Declared::Unverified,
            usage_telemetry: Declared::Unverified,
            credential_env: vec!["UNOROUTER_API_KEY".to_owned()],
            headers: vec![],
        },
        Provider {
            name: "anyrouter".to_owned(),
            protocols: vec![unverified_support(
                WireProtocol::OpenAiChat,
                "https://anyrouter.dev/api/v1",
            )],
            model_list_endpoint: Declared::Unverified,
            usage_telemetry: Declared::Unverified,
            credential_env: vec!["ANYROUTER_API_KEY".to_owned()],
            headers: vec![],
        },
        Provider {
            name: "zai".to_owned(),
            protocols: vec![unverified_support(
                WireProtocol::OpenAiChat,
                "https://api.z.ai/api/paas/v4",
            )],
            model_list_endpoint: Declared::Unverified,
            usage_telemetry: Declared::Unverified,
            credential_env: vec!["ZAI_API_KEY".to_owned()],
            headers: vec![],
        },
        Provider {
            name: "opencode-zen".to_owned(),
            protocols: vec![unverified_support(
                WireProtocol::OpenAiChat,
                "https://opencode.ai/zen/v1",
            )],
            model_list_endpoint: Declared::Unverified,
            usage_telemetry: Declared::Unverified,
            // No credential environment variable was established for this
            // one — see the module documentation on guessing.
            credential_env: vec![],
            headers: vec![],
        },
        Provider {
            name: "ollama".to_owned(),
            protocols: vec![unverified_support(
                WireProtocol::OpenAiChat,
                "http://localhost:11434/v1",
            )],
            model_list_endpoint: Declared::Unverified,
            usage_telemetry: Declared::Unverified,
            credential_env: vec![],
            headers: vec![],
        },
        Provider {
            name: "llama-cpp".to_owned(),
            protocols: vec![unverified_support(
                WireProtocol::OpenAiChat,
                "http://localhost:8080/v1",
            )],
            model_list_endpoint: Declared::Unverified,
            usage_telemetry: Declared::Unverified,
            credential_env: vec![],
            headers: vec![],
        },
        Provider {
            name: "nvidia".to_owned(),
            // `docs.api.nvidia.com/nim/reference/llm-apis` gives base
            // `https://integrate.api.nvidia.com` with `POST
            // /v1/chat/completions`; NVIDIA's own `build.nvidia.com` model
            // pages use `base_url = "https://integrate.api.nvidia.com/v1"`.
            // Read 2026-08-25. `openai-chat` only — no Responses endpoint
            // was established, so this template cannot back Codex, which
            // needs `openai-responses`.
            protocols: vec![unverified_support(
                WireProtocol::OpenAiChat,
                "https://integrate.api.nvidia.com/v1",
            )],
            model_list_endpoint: Declared::Unverified,
            usage_telemetry: Declared::Unverified,
            // NVIDIA's own sample reads `api_key = "$NVIDIA_API_KEY"`.
            credential_env: vec!["NVIDIA_API_KEY".to_owned()],
            headers: vec![],
        },
        Provider {
            name: "litellm".to_owned(),
            // LiteLLM's quick-start and `proxy/user_keys` pages both use
            // exactly `http://0.0.0.0:4000` as the client `base_url`. Written
            // as read — not "fixed" to `localhost`. Read 2026-08-25.
            protocols: vec![unverified_support(
                WireProtocol::OpenAiChat,
                "http://0.0.0.0:4000",
            )],
            model_list_endpoint: Declared::verified(
                true,
                "LiteLLM's proxy documentation lists `GET /models - available models on \
                 server`, read 2026-08-25",
            ),
            usage_telemetry: Declared::Unverified,
            // Deliberately empty. LiteLLM documents no dedicated credential
            // variable, and its own examples reuse the generic
            // `OPENAI_API_KEY` — declaring that here would make Glasshouse
            // read a user's real OpenAI key for what is usually a local
            // proxy. A LiteLLM key is a per-deployment virtual key, so the
            // user names its variable through
            // `ProviderConfig::set_credential_env`, exactly as the two
            // generic templates below already expect.
            credential_env: vec![],
            headers: vec![],
        },
        // The two generic templates: a concrete protocol is established
        // (OpenAI-compatible chat completions, or Anthropic Messages), but
        // the base URL and credential are the user's to supply — there is no
        // one service behind either name.
        Provider {
            name: "openai-compatible".to_owned(),
            protocols: vec![unverified_support(WireProtocol::OpenAiChat, "")],
            model_list_endpoint: Declared::Unverified,
            usage_telemetry: Declared::Unverified,
            credential_env: vec![],
            headers: vec![],
        },
        Provider {
            name: "anthropic-compatible".to_owned(),
            protocols: vec![unverified_support(WireProtocol::AnthropicMessages, "")],
            model_list_endpoint: Declared::Unverified,
            usage_telemetry: Declared::Unverified,
            credential_env: vec![],
            headers: vec![],
        },
    ]
}

/// The built-in template named `name`, or `None`.
pub fn template(name: &str) -> Option<Provider> {
    templates().into_iter().find(|p| p.name == name)
}

/// The two templates whose base URL is the user's to supply, not a service's
/// own documented endpoint. Named once here so
/// [`mod@crate::config`]'s validation and this module's own tests cannot
/// drift apart on which templates those are.
pub const GENERIC_TEMPLATE_NAMES: &[&str] = &["openai-compatible", "anthropic-compatible"];

/// Services a user may hold a credential for but which have no template here
/// because no endpoint has been established for them — see the module
/// documentation. Named so a test can assert none of them ever appears in
/// [`templates`], with the reason attached to the assertion itself.
#[cfg(test)]
const DELIBERATELY_UNTEMPLATED: &[(&str, &str)] = &[
    (
        "kilo",
        "the user holds a credential for Kilo, but no endpoint has been read from a real \
         installation or Kilo's own documentation",
    ),
    (
        "nous",
        "the user holds a credential for Nous, but no endpoint has been read from a real \
         installation or Nous's own documentation",
    ),
    (
        "routerai",
        "the user holds a credential for RouterAI, but no endpoint has been read from a real \
         installation or RouterAI's own documentation",
    ),
];

#[cfg(test)]
mod tests {
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

    #[test]
    fn openai_chat_support_never_implies_openai_responses() {
        // Line 408. OpenRouter's real, established shape: openai-chat only.
        let openrouter = template("openrouter").expect("openrouter is a built-in template");
        assert!(openrouter.serves(WireProtocol::OpenAiChat).is_some());
        assert!(
            openrouter.serves(WireProtocol::OpenAiResponses).is_none(),
            "a provider serving only openai-chat must not answer for openai-responses"
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

    #[test]
    fn no_translation_is_available_between_any_two_protocols() {
        // Line 410: translation is possible to add later, never implicit.
        const ALL: [WireProtocol; 3] = [
            WireProtocol::AnthropicMessages,
            WireProtocol::OpenAiResponses,
            WireProtocol::OpenAiChat,
        ];
        for &from in &ALL {
            for &to in &ALL {
                if from == to {
                    continue;
                }
                assert!(
                    !translation_available(from, to),
                    "{from} -> {to} must not be available"
                );
            }
        }
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

    #[test]
    fn no_template_exists_for_a_service_whose_endpoint_is_unestablished() {
        let names: Vec<String> = templates().into_iter().map(|p| p.name).collect();
        for (name, reason) in DELIBERATELY_UNTEMPLATED {
            assert!(
                !names.iter().any(|n| n == name),
                "`{name}` must not appear in templates(): {reason}"
            );
        }
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
                "litellm.model_list_endpoint".to_owned(),
            ],
            "only openrouter's and litellm's model-list endpoints were authorised as Verified; \
             every other capability must be Unverified: {verified:?}"
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
}
