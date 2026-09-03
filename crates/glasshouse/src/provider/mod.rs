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
//! # What the model-list probes established, on 2026-08-26
//!
//! Every template here shipped with `model_list_endpoint: Unverified` except
//! OpenRouter's and LiteLLM's, both of which cited a documentation page
//! rather than a response. Six live `GET <base>/models` requests were then
//! made against the exact base URLs these templates declare, unauthenticated,
//! and read for their entry counts:
//!
//! | provider | base URL | HTTP | entries |
//! |---|---|---|---|
//! | openrouter | `https://openrouter.ai/api/v1` | 200 | 417 |
//! | unorouter | `https://api.unorouter.com/v1` | 200 | 374 |
//! | anyrouter | `https://anyrouter.dev/api/v1` | 200 | 102 |
//! | kilo | `https://kilo.ai/api/openrouter` | 200 | 367 |
//! | nous | `https://inference-api.nousresearch.com/v1` | 200 | 372 |
//! | zai | `https://api.z.ai/api/paas/v4` | 401 | — |
//!
//! The five that answered `200` are the entries whose `model_list_endpoint`
//! is now `Verified`. **The promotion goes no further than that.** A
//! `GET /models` that answers `200` establishes that a model list is served
//! at that URL and nothing whatever about streaming, tool calls or reasoning,
//! so every one of those stays `Unverified` — the same discipline the
//! OpenRouter Responses entry below already documents for its own probe.
//!
//! Two of those counts are worth reading as snapshots rather than facts about
//! the service. UnoRouter answered `374` at 09:00 on 2026-08-26 and `369` an
//! hour later, re-probed independently. A catalogue that moves within the
//! hour is why every citation here names a date and why nothing downstream
//! may treat a count as stable.
//!
//! # z.ai stays `Unverified`, and the reason is the control
//!
//! **A `401` from z.ai establishes nothing about `/models`,** and the batch
//! that first promoted it said so itself without knowing: its stated control
//! was that "a host that served nothing there would have answered `404`".
//! That is exactly the right test, and it was cited from the OpenRouter
//! Responses probe rather than run against this host. Run against this host,
//! on 2026-08-26, it fails:
//!
//! - `/api/paas/v4/models` → `401`
//! - `/api/paas/v4/definitely-not-real-xyz` → `401`
//! - `/api/paas/v4/nonsense/deep/path` → `401`
//! - `/api/paas/v9/models`, a version prefix that does not exist → **`200`**,
//!   carrying the same authentication error in its body
//!
//! The service refuses every path under that prefix identically and will not
//! say whether a route exists until a credential is presented, so the `401`
//! discriminates nothing. `https://api.z.ai/totally/bogus` does answer `404`,
//! which is what made the original reasoning look sound — the `404` behaviour
//! is real, it simply lives outside the API prefix where the probe cannot use
//! it.
//!
//! The base URL is unchanged and still `unverified_support`; only the claim
//! that a model list is served at `<base>/models` is withdrawn. Establishing
//! it needs one authenticated request with the user's own key, which is a
//! free-models-only condition away and belongs to whoever spends it.
//!
//! **The transferable rule, which is this project's own and was applied to
//! the wrong subject here: a control has to be run against the host it is
//! being used to justify.** A control borrowed from another service is a
//! statement about that service.
//!
//! # Kilo and Nous have endpoints now
//!
//! Both were deliberately absent from [`templates`] until 2026-08-26 because
//! the user held a credential for each and no endpoint had been read for
//! either. The probes above are those endpoints, so both are templates now.
//!
//! **Kilo moved, and the template declares the new host.**
//! `https://kilocode.ai/api/openrouter/models` answers `308` with
//! `Location: https://kilo.ai/api/openrouter/models`. A template on the old
//! host would work only for a client that follows redirects, and
//! [`mod@crate::provider::discovery`] deliberately follows none — a redirect
//! means deciding whether to re-attach a credential to a host named at
//! runtime, which is not a decision to make silently.

pub mod cache;
pub mod discovery;
#[cfg(test)]
pub(crate) mod fixture;
pub mod pricing;
pub mod quota;
pub mod registry;
pub mod resources;
pub mod telemetry;

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

/// Providers that passed a protocol-compatibility routing constraint.
///
/// Construct this only with [`ProtocolCompatibleProviders::for_protocol`] or
/// [`ProtocolCompatibleProviders::for_any_protocol`]. Both constructors
/// remove a provider that either does not declare the required protocol or
/// has no base URL for it. A later model-quality scorer must accept this type,
/// not a raw provider slice, so it has no unfiltered provider set to rank.
#[derive(Debug)]
pub struct ProtocolCompatibleProviders<'a> {
    candidates: Vec<ProtocolCompatibleProvider<'a>>,
}

/// One provider that passed [`ProtocolCompatibleProviders`]' routing
/// constraint.
///
/// Its field stays private so callers cannot mark an unchecked [`Provider`]
/// compatible by constructing this value themselves.
#[derive(Debug)]
pub struct ProtocolCompatibleProvider<'a> {
    provider: &'a Provider,
}

impl<'a> ProtocolCompatibleProviders<'a> {
    /// Filter `providers` to those routeable over `required`.
    ///
    /// A protocol declaration with an empty base URL does not survive: there
    /// is nowhere to send a request, so it is not compatible for routing.
    pub fn for_protocol(providers: &'a [Provider], required: WireProtocol) -> Self {
        Self::for_any_protocol(providers, &[required])
    }

    /// Filter `providers` to those routeable over at least one protocol in
    /// `required`.
    ///
    /// This is for an ingress which can carry several protocols. A single
    /// required protocol should use [`Self::for_protocol`].
    pub fn for_any_protocol(providers: &'a [Provider], required: &[WireProtocol]) -> Self {
        Self {
            candidates: providers
                .iter()
                .filter(|provider| {
                    required.iter().any(|protocol| {
                        provider
                            .serves(*protocol)
                            .is_some_and(|support| !support.base_url.is_empty())
                    })
                })
                .map(|provider| ProtocolCompatibleProvider { provider })
                .collect(),
        }
    }

    /// How many providers survived the routing constraint.
    pub fn len(&self) -> usize {
        self.candidates.len()
    }

    /// Whether no provider survived the routing constraint.
    pub fn is_empty(&self) -> bool {
        self.candidates.is_empty()
    }

    /// The one surviving provider, if there is exactly one.
    pub fn only(&self) -> Option<&'a Provider> {
        match self.candidates.as_slice() {
            [candidate] => Some(candidate.provider),
            [] | [_, _, ..] => None,
        }
    }

    /// The surviving providers, still marked as protocol-compatible.
    ///
    /// This is the input shape a later ranking step must take. It exposes no
    /// raw `Provider` collection that could be confused with an unfiltered
    /// candidate set.
    pub fn iter(&self) -> impl Iterator<Item = &ProtocolCompatibleProvider<'a>> {
        self.candidates.iter()
    }
}

impl ProtocolCompatibleProvider<'_> {
    /// The configured provider's name, for a routing diagnostic or ranking
    /// explanation.
    pub fn name(&self) -> &str {
        &self.provider.name
    }

    /// The provider after it passed the routing constraint.
    pub fn provider(&self) -> &Provider {
        self.provider
    }
}

/// Whether a codec pair exists to translate `from` into `to`.
///
/// Answered from the gateway's pair table, [`crate::gateway::translate`],
/// which lists every ordered pair of wire protocols exactly once —
/// supported, or refused by name with its reason (capability map line
/// 1949). The table is keyed by slug because no file under `gateway/` may
/// name [`WireProtocol`]; this is the one place the enum meets the slug, and
/// `every_wire_protocol_pair_has_exactly_one_row_in_the_gateway_table` holds
/// the table complete against the enum.
///
/// Translation still never happens because two protocols merely looked
/// close: a row is supported only behind its own end-to-end test through
/// the shipped binary against a fixture upstream (line 1956). Three rows
/// carry one today — Claude Code's Anthropic Messages served by an
/// OpenAI-Chat entitlement (T1), and both directions of Anthropic Messages
/// <-> OpenAI Responses (T2).
pub fn translation_available(from: WireProtocol, to: WireProtocol) -> bool {
    crate::gateway::translate::is_supported(from.slug(), to.slug())
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

/// A provider that declares exactly one protocol and nothing beyond it: no
/// model-list endpoint, no usage telemetry, no extra headers established.
/// The shape seven of [`templates`]'s built-ins share verbatim; the
/// providers with a second protocol, a verified endpoint, or a header stay
/// spelled out as full [`Provider`] literals because this helper would not
/// save them anything true.
fn unverified_provider(
    name: &str,
    protocol: WireProtocol,
    base_url: &str,
    credential_env: Vec<String>,
) -> Provider {
    Provider {
        name: name.to_owned(),
        protocols: vec![unverified_support(protocol, base_url)],
        model_list_endpoint: Declared::Unverified,
        usage_telemetry: Declared::Unverified,
        credential_env,
        headers: vec![],
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
                // OpenRouter serves OpenAI Responses too, and this entry is
                // what makes it usable: `crate::config` lets a user override
                // a provider's base URL and credential variable but never
                // its protocols, so a protocol no template declares is a
                // protocol nothing can be configured for — and Codex 0.149.1
                // speaks Responses and nothing else.
                //
                // Established by empty-body `POST`s against the live service
                // on 2026-08-26, with a control: `/v1/responses`,
                // `/v1/chat/completions` and `/v1/messages` each answered
                // `400` (the route exists, the body was rejected) while
                // `/v1/definitely-not-a-real-endpoint` answered `404`.
                // Without that control a `400` would prove nothing. The
                // `/v1` is on the base URL because an OpenAI-shaped client
                // appends `/responses` itself — Codex 0.149.1 pointed at a
                // path-less base URL was observed sending exactly
                // `POST /responses`.
                //
                // Still `unverified_support`: the probe established that the
                // route exists, and nothing about streaming, tool calls or
                // reasoning, so those stay Unverified rather than being
                // upgraded by association.
                unverified_support(
                    WireProtocol::OpenAiResponses,
                    "https://openrouter.ai/api/v1",
                ),
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
                 GET https://openrouter.ai/api/v1/models, read 2026-08-25; that request \
                 answered 200 with 417 entries when made, 2026-08-26",
            ),
            // Phase 32B established the route; Phase QUOTA-FOLLOWUP read an
            // authenticated response from it. **The route exists, gates on
            // authentication, and answers with a body this crate now
            // parses** — `provider::telemetry::read_provider_usage`.
            //
            // The control was run against this host, not borrowed from
            // another: `/api/v1/glasshouse-nonexistent-control` answers
            // `404` with OpenRouter's own HTML error page, while
            // `/api/v1/key` and `/api/v1/credits` each answer `401` with the
            // documented `{"error":{"message":…,"code":401}}` envelope. A
            // host that served nothing at those paths would have answered
            // the `404`. That is the exact step z.ai's promotion skipped —
            // see this module's own note on it — and it is why the evidence
            // string names the control and not only the probe.
            usage_telemetry: Declared::verified(
                true,
                "GET https://openrouter.ai/api/v1/key and /api/v1/credits each answered 401                  with a JSON error envelope, unauthenticated, while the sibling path                  /api/v1/glasshouse-nonexistent-control answered 404 on the same host in the                  same minute, 2026-08-27; the routes exist and require a credential. An                  authenticated GET to /api/v1/key, run by the orchestrator (no worker holds a                  key), answered 200 with a body whose field names and types were recorded —                  data.limit, data.limit_remaining and data.limit_reset are each nullable                  integers and were null on the probed account — never a value, 2026-08-27",
            ),
            credential_env: vec!["OPENROUTER_API_KEY".to_owned()],
            headers: vec![],
        },
        Provider {
            name: "unorouter".to_owned(),
            protocols: vec![unverified_support(
                WireProtocol::OpenAiChat,
                "https://api.unorouter.com/v1",
            )],
            model_list_endpoint: Declared::verified(
                true,
                "GET https://api.unorouter.com/v1/models answered 200 with 374 entries under \
                 a top-level `data` array, probed 2026-08-26",
            ),
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
            model_list_endpoint: Declared::verified(
                true,
                "GET https://anyrouter.dev/api/v1/models answered 200 with 102 entries under \
                 a top-level `data` array, probed 2026-08-26",
            ),
            usage_telemetry: Declared::Unverified,
            credential_env: vec!["ANYROUTER_API_KEY".to_owned()],
            headers: vec![],
        },
        // Deliberately NOT promoted, unlike the five beside it. z.ai answers
        // `401` to every path under `/api/paas/v4/`, including ones that
        // plainly do not exist, so its `401` on `/models` establishes
        // nothing about `/models`. The full control run is in the module
        // documentation. Promoting this needs one authenticated request, not
        // another unauthenticated one.
        unverified_provider(
            "zai",
            WireProtocol::OpenAiChat,
            "https://api.z.ai/api/paas/v4",
            vec!["ZAI_API_KEY".to_owned()],
        ),
        // Kilo and Nous, both added 2026-08-26 from a live `GET /models` —
        // see the module documentation. Until that date both were named in
        // `DELIBERATELY_UNTEMPLATED` precisely because no endpoint had been
        // read for either.
        Provider {
            name: "kilo".to_owned(),
            // `kilo.ai`, not `kilocode.ai`. The old host answers `308` to
            // this one; a template pointing at it would only work for a
            // client that follows redirects, and a POST that follows a `308`
            // is not something to depend on.
            protocols: vec![unverified_support(
                WireProtocol::OpenAiChat,
                "https://kilo.ai/api/openrouter",
            )],
            model_list_endpoint: Declared::verified(
                true,
                "GET https://kilo.ai/api/openrouter/models answered 200 with 367 entries \
                 under a top-level `data` array, probed 2026-08-26",
            ),
            usage_telemetry: Declared::Unverified,
            credential_env: vec!["KILO_API_KEY".to_owned()],
            headers: vec![],
        },
        Provider {
            name: "nous".to_owned(),
            protocols: vec![unverified_support(
                WireProtocol::OpenAiChat,
                "https://inference-api.nousresearch.com/v1",
            )],
            model_list_endpoint: Declared::verified(
                true,
                "GET https://inference-api.nousresearch.com/v1/models answered 200 with 372 \
                 entries under a top-level `data` array, probed 2026-08-26",
            ),
            usage_telemetry: Declared::Unverified,
            credential_env: vec!["NOUS_API_KEY".to_owned()],
            headers: vec![],
        },
        // No credential environment variable was established for
        // opencode-zen — see the module documentation on guessing.
        unverified_provider(
            "opencode-zen",
            WireProtocol::OpenAiChat,
            "https://opencode.ai/zen/v1",
            vec![],
        ),
        unverified_provider(
            "ollama",
            WireProtocol::OpenAiChat,
            "http://localhost:11434/v1",
            vec![],
        ),
        unverified_provider(
            "llama-cpp",
            WireProtocol::OpenAiChat,
            "http://localhost:8080/v1",
            vec![],
        ),
        // `docs.api.nvidia.com/nim/reference/llm-apis` gives base
        // `https://integrate.api.nvidia.com` with `POST
        // /v1/chat/completions`; NVIDIA's own `build.nvidia.com` model
        // pages use `base_url = "https://integrate.api.nvidia.com/v1"`.
        // Read 2026-08-25. `openai-chat` only — no Responses endpoint
        // was established, so this template cannot back Codex, which
        // needs `openai-responses`. NVIDIA's own sample reads
        // `api_key = "$NVIDIA_API_KEY"`.
        unverified_provider(
            "nvidia",
            WireProtocol::OpenAiChat,
            "https://integrate.api.nvidia.com/v1",
            vec!["NVIDIA_API_KEY".to_owned()],
        ),
        Provider {
            name: "groq".to_owned(),
            // Base URL and wire protocol read off the live service itself,
            // not documentation — the orchestrator measured both against
            // the real host with the user's own credential, 2026-08-27
            // (`.agent-runtime/probe-quota-headers-2026-08-27.md`):
            // `GET https://api.groq.com/openai/v1/models` answered 200 with
            // a real catalogue, and `POST .../chat/completions` answered
            // 200 too. `openai-chat` only — no Responses endpoint was
            // established, so this template cannot back Codex, which needs
            // `openai-responses`, the same consequence NVIDIA's entry above
            // records.
            protocols: vec![unverified_support(
                WireProtocol::OpenAiChat,
                "https://api.groq.com/openai/v1",
            )],
            model_list_endpoint: Declared::verified(
                true,
                "GET https://api.groq.com/openai/v1/models answered 200 with a real \
                 catalogue, measured by the orchestrator against the live host with the \
                 user's own credential, 2026-08-27",
            ),
            // The inference response carries a full rate-limit header set
            // (limit/remaining/reset for both requests and tokens), but
            // that seam is the gateway's own forwarding path
            // (`crate::provider::telemetry::GatewayQuotaCache`), not a
            // dedicated usage endpoint this crate queries directly — no
            // such endpoint was established for Groq, so this stays
            // Unverified rather than conflating the two seams.
            usage_telemetry: Declared::Unverified,
            credential_env: vec!["GROQ_API_KEY".to_owned()],
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
        // Google AI Studio, the service `x-goog-api-key` authenticates.
        //
        // Base URL and credential delivery read off Google's published
        // Generative Language API reference: `POST
        // https://generativelanguage.googleapis.com/v1beta/models/{model}:generateContent`
        // with the key in an `x-goog-api-key` header (the `?key=` query form
        // is documented too and deliberately not used — a credential in a
        // URL lands in every proxy log between here and Google).
        //
        // The base URL is the **bare host**, with no version segment, for
        // the same reason OpenRouter's Anthropic Messages entry above is the
        // root: a Gemini client's own request target already begins
        // `/v1beta`, the gateway relays a served target byte for byte, and a
        // base URL carrying the version too composes `/v1beta/v1beta/…` —
        // a `404` the harness would report as a model error. The gateway's
        // Gemini codec therefore states `/v1beta` itself on a translated
        // request.
        //
        // `GEMINI_API_KEY` is the variable Google's own SDK samples and the
        // Gemini CLI both read. `model_list_endpoint` and `usage_telemetry`
        // stay `Unverified`: no request has been made against this host from
        // this project — the `generateContent` route is documented, not
        // probed, and nothing here upgrades a document into a measurement.
        // `USAGE_ENDPOINTS` therefore gains no row.
        unverified_provider(
            "gemini",
            WireProtocol::GeminiGenerateContent,
            "https://generativelanguage.googleapis.com",
            vec!["GEMINI_API_KEY".to_owned()],
        ),
        // The two generic templates: a concrete protocol is established
        // (OpenAI-compatible chat completions, or Anthropic Messages), but
        // the base URL and credential are the user's to supply — there is no
        // one service behind either name.
        unverified_provider("openai-compatible", WireProtocol::OpenAiChat, "", vec![]),
        unverified_provider(
            "anthropic-compatible",
            WireProtocol::AnthropicMessages,
            "",
            vec![],
        ),
    ]
}

/// The built-in template named `name`, or `None`.
pub fn template(name: &str) -> Option<Provider> {
    templates().into_iter().find(|p| p.name == name)
}

/// Where a provider's own usage endpoint lives, relative to its base URL —
/// capability map line 1230.
///
/// # A lookup table, not a [`Provider`] field
///
/// [`Provider`] is constructed by struct literal at every call site that
/// builds one, including `crate::secret` and `tests/pty_smoke.rs`'s fixture
/// providers — both outside this package's partition. A new required field
/// would need every one of those literals updated to compile, which is
/// exactly the ripple [`resources::harness_status_args`] already avoids for
/// harness status commands with the same shape of table. Only one template
/// has an established route, so a table earns its keep more than a field
/// nothing but this one entry would ever populate.
///
/// `/key`, not `/credits` — OpenRouter documents both, and `/key` is the one
/// whose authenticated response was actually read (see the `usage_telemetry`
/// evidence string on the `openrouter` template). Relative to this
/// protocol's own base URL, which already ends in `/v1`.
const USAGE_ENDPOINTS: &[(&str, &str)] = &[("openrouter", "/key")];

/// The path `provider_name`'s own usage endpoint lives at, if one is
/// established — `None` for every provider but the ones named in this
/// module's own usage-endpoint table.
pub fn usage_endpoint(provider_name: &str) -> Option<&'static str> {
    USAGE_ENDPOINTS
        .iter()
        .find(|(name, _)| *name == provider_name)
        .map(|(_, path)| *path)
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
///
/// **This list is empty today, and that is a statement rather than a gap.**
/// It held Kilo and Nous, both of which were given real endpoints on
/// 2026-08-26 by the probes in the module documentation — exactly the
/// transition this list exists to make visible. The mechanism stays because an absence has to stay assertable:
/// the next credential someone holds for a service with no readable endpoint
/// belongs here, not in a guessed template. See
/// `no_template_exists_for_a_service_whose_endpoint_is_unestablished`, whose
/// control case is what keeps the check itself honest while the list is
/// empty.
#[cfg(test)]
const DELIBERATELY_UNTEMPLATED: &[(&str, &str)] = &[];

#[cfg(test)]
mod tests;
