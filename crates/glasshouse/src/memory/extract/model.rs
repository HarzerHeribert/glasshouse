//! An [`ExtractionModel`] that actually calls one — Phase 21's *"allow a
//! configurable cheap or local model to perform memory extraction"*.
//!
//! # Why this exists here rather than behind Phase 39
//!
//! [`mod@super`] was written against a seam and said, in its own module
//! header, that nothing in this codebase could call a model. That was true
//! when it was written and is no longer: `ureq` arrived with the gateway, and
//! `crate::provider::discovery` has been making real authenticated requests
//! to configured providers since. So the missing half of line 834 was never
//! an architecture, only a transport — and this is that transport, kept
//! deliberately small.
//!
//! # OpenAI chat completions, and nothing else
//!
//! [`WireProtocol::OpenAiChat`] is the only protocol here, and that is a
//! reading of the line rather than a shortfall against it. *Cheap or local*
//! is what the map asks for, and every local runner (Ollama, LM Studio,
//! llama.cpp, vLLM) and every cheap hosted router speaks OpenAI chat
//! completions. [`WireProtocol::AnthropicMessages`] would need this module to
//! **originate** an `anthropic-version` header, which nothing in this
//! codebase has ever done — the gateway only ever forwards a client's own —
//! and inventing a protocol version is the same class of guess
//! `crate::provider` refuses when it declines to invent a base URL.
//! [`WireProtocol::OpenAiResponses`] is a different request shape again.
//! Both are refused **by name at construction**, so an unusable choice is one
//! logged sentence at the wiring rather than a failure on every turn.
//!
//! # What may never leave this module
//!
//! A provider's error body can echo the request, and the request is a prompt
//! built from the user's own session. So no response body, and no `ureq`
//! error's own words, ever reaches a [`ModelError`]: every failure here is
//! one of a fixed set of phrases chosen in this file. That is the same rule
//! [`ModelError::Failed`] was given a `&'static str` to enforce, applied at
//! the first place that could have broken it.
//!
//! The credential is read in exactly one place — the header
//! [`ConfiguredModel::complete`] builds — and [`ConfiguredModel`]'s
//! [`Debug`](std::fmt::Debug) prints [`REDACTED`] in its place. The base URL
//! is **not** in [`ExtractionModel::describe`]'s answer, because a base URL
//! can have a credential embedded in it and that string is stored on every
//! outcome and printed in every log line.

use std::fmt;
use std::time::Duration;

use ureq::Agent;
use ureq::config::AutoHeaderValue;

use crate::harness::WireProtocol;
use crate::secret::{REDACTED, Secret};

use super::{ExtractionModel, ModelError, Prompt};

/// How long the call waits for the TCP connection and any TLS handshake.
///
/// Shorter than `crate::provider::discovery::CONNECT_TIMEOUT`'s five seconds
/// on purpose: that one bounds a probe a person asked for and is watching,
/// and this one bounds a support job running inside somebody's coding
/// session. A local runner on loopback connects in microseconds and a hosted
/// one in well under a second; three seconds is already generous for the case
/// this is actually built for.
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(3);

/// How long the call waits for the response head once the request is sent.
///
/// This is the one whose absence would be a hang: a server that accepts a
/// connection and then says nothing looks healthy until it expires. A
/// generation is not a catalogue read, so this is longer than
/// `crate::provider::discovery::RESPONSE_TIMEOUT` — but see
/// [`ConfiguredModel`]'s own note on why the number that usually decides this
/// is the caller's bound and not this one.
pub const RESPONSE_TIMEOUT: Duration = Duration::from_secs(20);

/// A ceiling on the whole call, body included.
///
/// The other two bound the phases a stall is *likely* in. This bounds the one
/// nobody thinks of: a server that answers its head promptly and then dribbles
/// the body forever satisfies both of the others indefinitely.
pub const TOTAL_TIMEOUT: Duration = Duration::from_secs(30);

/// The largest reply this will read.
///
/// A structured extraction reply is a few kilobytes. One mebibyte is three
/// orders of magnitude of headroom and is still a bound — which is the point,
/// because the alternative is a support job reading a hostile endpoint's
/// output into the memory of a process running inside a coding session.
const MAX_REPLY_BYTES: u64 = 1024 * 1024;

/// The output budget the request declares.
///
/// Extraction emits a bounded JSON document, not prose. Declaring a ceiling
/// is what stops a model that has started rambling from being paid for — or
/// waited on — indefinitely.
const MAX_OUTPUT_TOKENS: u32 = 2048;

/// Every timeout one extraction call is bounded by.
///
/// A type rather than three constants read directly, for the reason
/// `crate::provider::discovery::ProbeTimeouts` is one: a test that proves a
/// stalled endpoint is bounded has to be able to pick short values, and a
/// test that waited out [`RESPONSE_TIMEOUT`] is a twenty-second test somebody
/// eventually marks `#[ignore]`. Production uses [`CallTimeouts::default`]
/// and nothing else.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CallTimeouts {
    pub connect: Duration,
    pub response: Duration,
    pub total: Duration,
}

impl Default for CallTimeouts {
    fn default() -> Self {
        Self {
            connect: CONNECT_TIMEOUT,
            response: RESPONSE_TIMEOUT,
            total: TOTAL_TIMEOUT,
        }
    }
}

/// Why a configured extraction model could not be built at all.
///
/// Distinct from [`ModelError`], which is about a call that was attempted.
/// These are configurations that cannot produce a call, and the caller's job
/// on seeing one is to say so once and go on with no model — never to guess
/// at a correction.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ConfiguredModelError {
    /// The provider serves a protocol this module does not speak. Named
    /// rather than approximated — see the module header.
    #[error(
        "extraction speaks OpenAI chat completions, and `{provider}` serves `{protocol}`; \
         configure a provider that serves openai-chat"
    )]
    UnsupportedProtocol {
        provider: String,
        protocol: WireProtocol,
    },
    /// The provider declares no protocol at all.
    #[error("`{provider}` declares no wire protocol, so there is nothing to send a request to")]
    NoProtocol { provider: String },
    /// The provider has no base URL. The two generic templates leave it
    /// empty on purpose, because it is the user's to supply.
    #[error("`{provider}` has no base URL configured, so there is nowhere to send a request")]
    NoBaseUrl { provider: String },
    /// The base URL carries a credential in its userinfo — `https://key@host`.
    ///
    /// Refused rather than accepted-and-redacted, and the reason is the
    /// `crate::config` module's own "No secrets here" rule: a base URL lives
    /// in a configuration file, and a configuration file holds credential
    /// variable *names* and never a value. Accepting one would put a secret
    /// somewhere every diagnostic, every `Debug`, and every support bundle
    /// can reach — this module's own test found exactly that leak through
    /// [`ConfiguredModel`]'s [`Debug`](std::fmt::Debug), which redacts the
    /// credential field and had no reason to suspect the endpoint.
    #[error(
        "`{provider}`'s base URL carries a credential; put it in a credential \
         variable or a header and leave the URL a URL"
    )]
    CredentialInBaseUrl { provider: String },
    /// The provider names credential variables and none of them resolves.
    ///
    /// A provider that names *no* credential variable is not an error — that
    /// is exactly the local case this line exists for, and it is built
    /// without one.
    #[error(
        "`{provider}` names a credential variable and none of them is set; \
         a local model needs no credential and should name none"
    )]
    NoCredential { provider: String },
}

/// A cheap or local model, asked over OpenAI chat completions.
///
/// # The bound that usually decides this is not one of ours
///
/// In the hook path, `main.rs`'s `EXTRACTION_BOUND` abandons the whole
/// extraction after five seconds, and the hook process exits moments later —
/// so a model slower than that produces nothing, whatever
/// [`RESPONSE_TIMEOUT`] says. That is the design and not a defect: extraction
/// is a support job, and a coding session waiting on one has the relationship
/// backwards. The timeouts here are what bound the paths with no such caller
/// above them, and what stop a socket outliving the answer on the one that
/// has.
pub struct ConfiguredModel {
    provider: String,
    model: String,
    endpoint: String,
    headers: Vec<(String, String)>,
    credential: Option<Secret>,
    timeouts: CallTimeouts,
}

/// Prints [`REDACTED`] where the credential is, and whether there is one at
/// all — a fact a diagnostic legitimately needs, and which reveals nothing
/// about the value.
impl fmt::Debug for ConfiguredModel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ConfiguredModel")
            .field("provider", &self.provider)
            .field("model", &self.model)
            .field("endpoint", &self.endpoint)
            .field("headers", &self.headers)
            .field(
                "credential",
                &match self.credential {
                    Some(_) => REDACTED,
                    None => "(none)",
                },
            )
            .finish()
    }
}

impl ConfiguredModel {
    /// Build the model the user named on `provider`, or say why not.
    ///
    /// `credential` is resolved by the caller, because resolving a
    /// [`crate::secret::SecretRef`] needs a
    /// [`crate::secret::SecretStore`] and this module has no business
    /// choosing one. `None` with a provider that names no credential
    /// variable is the local case and is built; `None` with a provider that
    /// names one is [`ConfiguredModelError::NoCredential`].
    pub fn new(
        provider: &crate::provider::Provider,
        model: impl Into<String>,
        credential: Option<Secret>,
    ) -> Result<Self, ConfiguredModelError> {
        let support =
            provider
                .protocols
                .first()
                .ok_or_else(|| ConfiguredModelError::NoProtocol {
                    provider: provider.name.clone(),
                })?;
        if support.protocol != WireProtocol::OpenAiChat {
            return Err(ConfiguredModelError::UnsupportedProtocol {
                provider: provider.name.clone(),
                protocol: support.protocol,
            });
        }
        let base_url = support.base_url.trim_end_matches('/');
        if base_url.is_empty() {
            return Err(ConfiguredModelError::NoBaseUrl {
                provider: provider.name.clone(),
            });
        }
        if has_userinfo(base_url) {
            return Err(ConfiguredModelError::CredentialInBaseUrl {
                provider: provider.name.clone(),
            });
        }
        if credential.is_none() && !provider.credential_env.is_empty() {
            return Err(ConfiguredModelError::NoCredential {
                provider: provider.name.clone(),
            });
        }

        Ok(Self {
            provider: provider.name.clone(),
            model: model.into(),
            endpoint: format!("{base_url}/chat/completions"),
            headers: provider.headers.clone(),
            credential,
            timeouts: CallTimeouts::default(),
        })
    }

    /// Replace the timeouts. For tests that need a stall bounded in
    /// milliseconds rather than in tens of seconds.
    pub fn with_timeouts(mut self, timeouts: CallTimeouts) -> Self {
        self.timeouts = timeouts;
        self
    }

    /// The request body, as its own function so a test can read it without a
    /// socket.
    ///
    /// `temperature` is zero and `stream` is false: this is a structured
    /// document being asked for once, not a conversation. A streamed reply
    /// would need a second parser for no benefit a support job can use.
    fn body(&self, prompt: &Prompt) -> serde_json::Value {
        serde_json::json!({
            "model": self.model,
            "temperature": 0,
            "stream": false,
            "max_tokens": MAX_OUTPUT_TOKENS,
            "messages": [{ "role": "user", "content": prompt.as_str() }],
        })
    }
}

impl ExtractionModel for ConfiguredModel {
    /// Names the resource and the route, and neither the base URL nor the
    /// credential — see the module header for why the base URL is excluded
    /// even though it looks harmless.
    fn describe(&self) -> String {
        format!(
            "{}/{} via {}",
            self.provider,
            self.model,
            WireProtocol::OpenAiChat
        )
    }

    fn complete(&self, prompt: &Prompt) -> Result<String, ModelError> {
        // `http_status_as_error(false)` is load-bearing: with the default a
        // `401` arrives as an `Err` indistinguishable in shape from a refused
        // connection, and those are different problems with different fixes.
        // `max_redirects(0)` is the second: following a redirect would
        // re-attach the credential to a host named at runtime, which is a
        // decision this module has no business making silently.
        let agent = Agent::new_with_config(
            Agent::config_builder()
                .http_status_as_error(false)
                .max_redirects(0)
                .accept_encoding(AutoHeaderValue::None)
                .timeout_connect(Some(self.timeouts.connect))
                .timeout_recv_response(Some(self.timeouts.response))
                .timeout_global(Some(self.timeouts.total))
                .build(),
        );

        let mut builder = agent
            .post(&self.endpoint)
            .header("content-type", "application/json");
        for (name, value) in &self.headers {
            builder = builder.header(name, value);
        }
        if let Some(credential) = &self.credential {
            // The one place a resolved credential is read in this module. It
            // goes into a header value and nowhere else — not into the URL,
            // not into a log line, not into any error below.
            builder = builder.header("authorization", format!("Bearer {}", credential.expose()));
        }

        // Serialized here rather than through `ureq`'s own `send_json`,
        // which this build does not have: the crate is declared with
        // `default-features = false` so that nothing but `rustls` is linked.
        let body = self.body(prompt).to_string();
        let response = match builder.send(body.as_bytes()) {
            Ok(response) => response,
            Err(err) => return Err(transport_error(&err)),
        };

        let status = response.status().as_u16();
        if !(200..=299).contains(&status) {
            return Err(status_error(status));
        }

        let text = response
            .into_body()
            .with_config()
            .limit(MAX_REPLY_BYTES)
            .read_to_string()
            .map_err(|_| ModelError::Failed {
                phrase: "the extraction model's reply could not be read",
            })?;

        content_of(&text)
    }
}

/// Whether `url`'s authority carries userinfo — the `key@` in
/// `https://key@host/v1`.
///
/// Deliberately crude, and deliberately erring towards refusal: it looks for
/// an `@` in the authority only, which is everything between `//` and the
/// next `/`. A query string or a path may legitimately contain one and is not
/// examined; an authority may not.
fn has_userinfo(url: &str) -> bool {
    let Some(after_scheme) = url.split_once("//") else {
        return false;
    };
    let authority = after_scheme
        .1
        .split(['/', '?', '#'])
        .next()
        .unwrap_or_default();
    authority.contains('@')
}

/// The assistant message out of an OpenAI chat-completions reply.
///
/// Separate from [`ConfiguredModel::complete`] so the shapes a real endpoint
/// produces can be tested without one. Note what it does **not** do: it never
/// puts any part of `text` into the error, because `text` is a provider's
/// answer to a prompt built from the user's session.
fn content_of(text: &str) -> Result<String, ModelError> {
    let document: serde_json::Value =
        serde_json::from_str(text).map_err(|_| ModelError::Failed {
            phrase: "the extraction model's reply was not JSON",
        })?;
    let content = document
        .get("choices")
        .and_then(|choices| choices.get(0))
        .and_then(|choice| choice.get("message"))
        .and_then(|message| message.get("content"))
        .and_then(serde_json::Value::as_str)
        .ok_or(ModelError::Failed {
            phrase: "the extraction model's reply carried no message content",
        })?;
    if content.trim().is_empty() {
        return Err(ModelError::Failed {
            phrase: "the extraction model answered with an empty message",
        });
    }
    Ok(content.to_owned())
}

/// An HTTP status, as a [`ModelError`].
///
/// Every arm is a fixed phrase. A status a provider chose is a number
/// Glasshouse can render; a *body* a provider chose is text that may echo the
/// prompt, and none of it is here.
fn status_error(status: u16) -> ModelError {
    match status {
        401 | 403 => ModelError::Refused,
        408 | 504 => ModelError::TimedOut,
        429 => ModelError::Failed {
            phrase: "the extraction model is rate limited",
        },
        404 => ModelError::Failed {
            phrase: "the extraction model's endpoint answered 404; check the provider's base URL",
        },
        500..=599 => ModelError::Failed {
            phrase: "the extraction model's provider reported a server error",
        },
        _ => ModelError::Failed {
            phrase: "the extraction model answered with an unexpected status",
        },
    }
}

/// A transport failure, as a [`ModelError`].
///
/// Built from a fixed set of phrases rather than from the error's own words,
/// exactly as `crate::provider::discovery`'s own `unreachable_reason` is and
/// for the same reason.
fn transport_error(err: &ureq::Error) -> ModelError {
    match err {
        ureq::Error::Timeout(_) => ModelError::TimedOut,
        ureq::Error::Io(io) if is_timeout_kind(io.kind()) => ModelError::TimedOut,
        ureq::Error::HostNotFound => ModelError::Failed {
            phrase: "the extraction model's host name did not resolve",
        },
        ureq::Error::ConnectionFailed => ModelError::Failed {
            phrase: "the connection to the extraction model failed",
        },
        ureq::Error::BadUri(_) => ModelError::Failed {
            phrase: "the extraction model's base URL is not a usable URL",
        },
        _ => ModelError::Unavailable,
    }
}

/// Two shapes rather than one because `ureq` raises its own configured
/// timeouts as [`ureq::Error::Timeout`] but a socket-level deadline arrives
/// as an [`std::io::Error`] instead — the same split
/// `crate::provider::discovery::is_timeout` documents.
fn is_timeout_kind(kind: std::io::ErrorKind) -> bool {
    matches!(
        kind,
        std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::harness::Declared;
    use crate::provider::{ProtocolSupport, Provider};

    fn provider(name: &str, protocol: WireProtocol, base_url: &str) -> Provider {
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
            credential_env: vec![],
            headers: vec![],
        }
    }

    /// The local case the line exists for: a runner on loopback, no
    /// credential named and none needed.
    #[test]
    fn a_local_provider_naming_no_credential_builds_without_one() {
        let built = ConfiguredModel::new(
            &provider(
                "local",
                WireProtocol::OpenAiChat,
                "http://127.0.0.1:11434/v1",
            ),
            "qwen2.5-coder:7b",
            None,
        )
        .expect("a local runner needs no credential");
        assert_eq!(built.endpoint, "http://127.0.0.1:11434/v1/chat/completions");
        assert_eq!(built.describe(), "local/qwen2.5-coder:7b via openai-chat");
    }

    /// A provider that *names* a credential variable and has none resolved is
    /// refused rather than called without one: an unauthenticated request to
    /// a hosted router is a `401` at best and a surprise at worst.
    #[test]
    fn a_provider_that_names_a_credential_is_refused_without_one() {
        let mut hosted = provider(
            "hosted",
            WireProtocol::OpenAiChat,
            "https://example.invalid/v1",
        );
        hosted.credential_env = vec!["HOSTED_API_KEY".to_owned()];
        assert!(matches!(
            ConfiguredModel::new(&hosted, "a-model", None),
            Err(ConfiguredModelError::NoCredential { .. })
        ));
    }

    /// The two protocols this module does not speak are refused by name at
    /// construction — see the module header for why neither is approximated.
    #[test]
    fn a_protocol_this_module_does_not_speak_is_refused_by_name() {
        for protocol in [
            WireProtocol::AnthropicMessages,
            WireProtocol::OpenAiResponses,
        ] {
            let err = ConfiguredModel::new(
                &provider("other", protocol, "https://example.invalid"),
                "a-model",
                None,
            )
            .expect_err("only openai-chat is spoken here");
            assert!(
                err.to_string().contains(protocol.slug()),
                "the refusal must name the protocol it refused: {err}"
            );
        }
    }

    /// The generic templates leave the base URL empty because it is the
    /// user's to supply. An empty one is a configuration error, not a request
    /// to a relative URL.
    #[test]
    fn an_empty_base_url_is_refused_rather_than_requested() {
        assert!(matches!(
            ConfiguredModel::new(
                &provider("generic", WireProtocol::OpenAiChat, ""),
                "m",
                None
            ),
            Err(ConfiguredModelError::NoBaseUrl { .. })
        ));
    }

    /// A base URL that carries a credential is refused outright.
    ///
    /// **This test found a real leak and the fix is why it reads this way.**
    /// The first version accepted such a URL and asserted only that
    /// [`ExtractionModel::describe`] omitted it; the credential then appeared,
    /// in full, in [`ConfiguredModel`]'s own `Debug` — which redacts the
    /// credential field and had no reason to suspect the endpoint built from
    /// the base URL. Redacting the second exit would have left the third.
    #[test]
    fn a_base_url_carrying_a_credential_is_refused_rather_than_redacted() {
        const SECRET: &str = "sk-fabricated-test-value-not-a-real-credential";
        let err = ConfiguredModel::new(
            &provider(
                "hosted",
                WireProtocol::OpenAiChat,
                &format!("https://{SECRET}@example.invalid/v1"),
            ),
            "a-model",
            None,
        )
        .expect_err("a configuration file may not hold a credential value");
        assert!(matches!(
            err,
            ConfiguredModelError::CredentialInBaseUrl { .. }
        ));
        assert!(
            !err.to_string().contains(SECRET),
            "even the refusal must not repeat it: {err}"
        );

        // The crudeness is deliberate: an `@` outside the authority is not a
        // credential and must not be refused as one.
        assert!(has_userinfo("https://key@example.invalid/v1"));
        assert!(!has_userinfo("https://example.invalid/v1/a@b"));
        assert!(!has_userinfo("http://127.0.0.1:11434/v1"));
    }

    /// Neither the credential nor the base URL reaches the two strings that
    /// are stored on every outcome and printed in every log line.
    ///
    /// The base URL is excluded from `describe()` even now that one cannot
    /// carry a credential: defence in depth costs a provider name and a model
    /// name, which is all a reader of that line actually needs.
    #[test]
    fn neither_the_credential_nor_the_base_url_reaches_a_description() {
        const SECRET: &str = "sk-fabricated-test-value-not-a-real-credential";
        let mut hosted = provider(
            "hosted",
            WireProtocol::OpenAiChat,
            "https://example.invalid/v1",
        );
        hosted.credential_env = vec!["HOSTED_API_KEY".to_owned()];
        let built =
            ConfiguredModel::new(&hosted, "a-model", Some(Secret::mint_for_test(SECRET))).unwrap();

        let described = built.describe();
        assert!(
            !described.contains(SECRET),
            "describe() leaked: {described}"
        );
        assert!(
            !described.contains("example.invalid"),
            "describe() must not carry a base URL at all: {described}"
        );

        let debugged = format!("{built:?}");
        assert!(
            !debugged.contains(SECRET),
            "Debug leaked the credential: {debugged}"
        );
        assert!(debugged.contains(REDACTED));
    }

    /// The request names the configured model and declares its own bound.
    #[test]
    fn the_request_names_the_model_and_bounds_its_output() {
        let built = ConfiguredModel::new(
            &provider("local", WireProtocol::OpenAiChat, "http://127.0.0.1:1/v1"),
            "a-cheap-model",
            None,
        )
        .unwrap();
        let chunk = super::super::chunk::SessionChunk::build(
            "session",
            None::<String>,
            std::iter::empty(),
            super::super::chunk::ChunkLimits::default(),
        );
        let body = built.body(&Prompt::build(&chunk, &[]));
        assert_eq!(body["model"], "a-cheap-model");
        assert_eq!(body["stream"], false);
        assert_eq!(body["max_tokens"], MAX_OUTPUT_TOKENS);
        assert!(
            body["messages"][0]["content"]
                .as_str()
                .expect("the prompt is the message")
                .contains("Session session"),
            "the prompt must be what is sent"
        );
    }

    /// The reply shapes a real endpoint produces, and the ones that are not
    /// an answer.
    #[test]
    fn a_reply_without_message_content_is_a_named_failure_and_never_an_echo() {
        assert_eq!(
            content_of(r#"{"choices":[{"message":{"content":"{\"memories\":[]}"}}]}"#),
            Ok(r#"{"memories":[]}"#.to_owned())
        );

        // A reasoning model that answered with reasoning and no content, and
        // a provider that answered with an error document. Neither is an
        // answer, and neither may carry its own text into the diagnostic.
        const ECHOED: &str = "PROMPT-ECHO-a1b2c3-MUST-NEVER-REACH-A-DIAGNOSTIC";
        for reply in [
            format!(r#"{{"choices":[{{"message":{{"reasoning":"{ECHOED}"}}}}]}}"#),
            format!(r#"{{"error":{{"message":"bad request: {ECHOED}"}}}}"#),
            r#"{"choices":[{"message":{"content":"   "}}]}"#.to_owned(),
            format!("not json at all: {ECHOED}"),
        ] {
            let err = content_of(&reply).expect_err("none of these is an answer");
            assert!(
                !err.to_string().contains(ECHOED),
                "a provider's own text reached a diagnostic: {err}"
            );
        }
    }

    /// A `401` is a credential problem and a refused connection is a network
    /// problem; the coarse [`ModelError`] still tells them apart.
    #[test]
    fn a_rejected_credential_and_an_unreachable_host_are_different_failures() {
        assert_eq!(status_error(401), ModelError::Refused);
        assert_eq!(status_error(403), ModelError::Refused);
        assert_eq!(
            transport_error(&ureq::Error::HostNotFound),
            ModelError::Failed {
                phrase: "the extraction model's host name did not resolve",
            }
        );
        assert_eq!(
            transport_error(&ureq::Error::Io(std::io::Error::from(
                std::io::ErrorKind::TimedOut
            ))),
            ModelError::TimedOut
        );
    }
}
