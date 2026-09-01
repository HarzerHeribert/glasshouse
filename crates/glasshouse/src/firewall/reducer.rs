//! The semantic reducer seam — Phase 57B, map lines 1997-2003.
//!
//! # What this is, mirrored from
//!
//! This module is [`crate::memory::extract::model`] and
//! [`crate::memory::extract::disposable`]'s pattern, applied to a different
//! job: a trait ([`Reducer`]) a caller asks to decide over numbered
//! candidates, and the one disposable-backed implementation
//! ([`ConfiguredReducer`]) that actually calls a model, over the same
//! OpenAI-chat-completions wire protocol and through the same provider
//! plumbing. It is a second *type* because the request and reply shapes are
//! this job's own — never a second *idiom* for reaching a provider. Map line
//! 1997's "never a firewall-private provider client" is satisfied the same
//! way extraction's own client satisfies it: [`ConfiguredReducer`] is built
//! from a [`crate::provider::Provider`] the routing layer chose, and it
//! speaks the one protocol this build's disposable-job machinery already
//! speaks.
//!
//! # What may never leave this module
//!
//! Exactly [`crate::memory::extract::model`]'s own rule: no response body,
//! and no transport error's own words, ever reach a [`ReducerErrorKind`] —
//! every failure here is one of a fixed set of phrases, because a provider's
//! error body can echo the request, which is built from a tool result that
//! may itself contain user data.

use std::collections::HashSet;
use std::fmt;
use std::time::Duration;

use serde::Deserialize;
use ureq::Agent;
use ureq::config::AutoHeaderValue;

use crate::harness::WireProtocol;
use crate::secret::{REDACTED, Secret};

use super::reduce::Candidate;

// ===========================================================================
// The request and reply shapes.
// ===========================================================================

/// What the reducer is asked to decide over. Map line 1998: never the
/// conversational transcript — there is no field here able to carry one.
pub struct ReductionRequest<'a> {
    /// The stated task the hook was given (`--task`). Empty is allowed and
    /// common: not every invocation has one.
    pub task: &'a str,
    pub tool_name: &'a str,
    /// The tool's own query or command, when the adapter could name one —
    /// `None` for a tool whose input carries nothing query-shaped.
    pub tool_query: Option<&'a str>,
    /// The deterministic ladder's own retained output, numbered. The
    /// reducer may only ever refer back to these ids — see [`Verdict`].
    pub candidates: &'a [Candidate],
}

/// One candidate's structured verdict — map line 1999: ids, never text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Relevance {
    Relevant,
    Uncertain,
    Discard,
}

impl Relevance {
    fn parse(text: &str) -> Option<Self> {
        match text {
            "relevant" => Some(Self::Relevant),
            "uncertain" => Some(Self::Uncertain),
            "discard" => Some(Self::Discard),
            _ => None,
        }
    }
}

/// One candidate id's verdict and the reducer's stated reason for it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Verdict {
    pub id: usize,
    pub relevance: Relevance,
    pub reason: String,
}

/// Everything the evidence ledger needs about the call that produced a
/// [`ReducerAnswer`] or a failed [`ReducerError`] — map line 1987's second
/// half: reducer calls are real model calls, so their provider, model, route
/// and provider-reported token counts belong in the ledger's token columns,
/// never estimated.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReducerCallInfo {
    pub provider: String,
    pub model: String,
    pub route: Option<String>,
    pub input_tokens: Option<i64>,
    pub output_tokens: Option<i64>,
    pub cached_input_tokens: Option<i64>,
}

/// What a reducer answered: every verdict it gave, plus the call that
/// produced them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReducerAnswer {
    pub verdicts: Vec<Verdict>,
    pub call: ReducerCallInfo,
}

/// Every way asking a reducer can fail — map line 2001's exact vocabulary:
/// "timeout, transport, rate limit, schema, validation, or outage".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReducerErrorKind {
    TimedOut,
    Transport,
    RateLimited,
    /// The credential was refused (401/403) — a configuration problem, not
    /// an outage, but fail-open makes no distinction in what it does next.
    Refused,
    /// No resource could serve this job at all — routing found nothing.
    Unavailable,
    /// The reply was not JSON, or not the schema this module reads.
    Schema,
    /// The reply parsed, but named an id the candidates never had, or named
    /// none at all — map line 1999's validation failure.
    Validation,
    Failed(&'static str),
}

impl fmt::Display for ReducerErrorKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TimedOut => f.write_str("the reducer timed out"),
            Self::Transport => f.write_str("the reducer could not be reached"),
            Self::RateLimited => f.write_str("the reducer is rate limited"),
            Self::Refused => f.write_str("the reducer's credential was refused"),
            Self::Unavailable => f.write_str("no reducer resource is available"),
            Self::Schema => f.write_str("the reducer's reply was not in the expected schema"),
            Self::Validation => {
                f.write_str("the reducer's reply named a candidate id it was never given")
            }
            Self::Failed(phrase) => f.write_str(phrase),
        }
    }
}

/// One failed attempt to ask a reducer. `call` carries what the ledger
/// should know about the call whenever a real request actually completed
/// with a parseable reply — a real HTTP round trip and its token cost
/// happened even when the *content* was unusable — and stays `None` for a
/// failure that never reached a parsed reply (timeout, transport, rate
/// limit, an unreachable resource). Boxed so this error stays small enough
/// to return by value even though [`ReducerCallInfo`] is not.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReducerError {
    pub kind: ReducerErrorKind,
    pub call: Option<Box<ReducerCallInfo>>,
}

impl ReducerError {
    fn new(kind: ReducerErrorKind) -> Self {
        Self { kind, call: None }
    }

    fn with_call(kind: ReducerErrorKind, call: ReducerCallInfo) -> Self {
        Self {
            kind,
            call: Some(Box::new(call)),
        }
    }
}

impl fmt::Display for ReducerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.kind.fmt(f)
    }
}

/// A provider-independent seam for the semantic stage — map line 1997: a
/// trait, and [`ConfiguredReducer`] the one disposable-backed implementation
/// of it, exactly [`crate::memory::extract::ExtractionModel`]'s shape.
pub trait Reducer {
    /// Names the resource this reducer is asking, for a label or a log line
    /// — never the base URL or the credential, matching
    /// [`crate::memory::extract::ExtractionModel::describe`]'s own rule.
    fn describe(&self) -> String;

    /// Ask over `request`, and report both halves: the verdicts, and what
    /// the call cost.
    fn select(&self, request: &ReductionRequest<'_>) -> Result<ReducerAnswer, ReducerError>;
}

// ===========================================================================
// Deciding what to keep — map lines 1999 and 2000.
// ===========================================================================

/// Build the kept-id set from `verdicts` — map line 2000's bias to
/// inclusion, applied uniformly:
///
/// - `Relevant` is always kept.
/// - `Discard` is always dropped.
/// - `Uncertain`, and any candidate the reducer never mentioned at all, is
///   kept unless `aggressive` is true **and** the user explicitly configured
///   `aggressive` to drop uncertain candidates
///   (`aggressive_drops_uncertain`). A candidate never mentioned is treated
///   exactly like an explicit `Uncertain` — the reducer's silence about a
///   candidate is not a claim that it may be dropped.
pub fn decide_keep_set(
    verdicts: &[Verdict],
    candidates: &[Candidate],
    aggressive: bool,
    aggressive_drops_uncertain: bool,
) -> HashSet<usize> {
    let by_id: std::collections::HashMap<usize, Relevance> =
        verdicts.iter().map(|v| (v.id, v.relevance)).collect();
    let drop_uncertain = aggressive && aggressive_drops_uncertain;

    candidates
        .iter()
        .filter_map(|candidate| {
            let relevance = by_id
                .get(&candidate.id)
                .copied()
                .unwrap_or(Relevance::Uncertain);
            let keep = match relevance {
                Relevance::Relevant => true,
                Relevance::Discard => false,
                Relevance::Uncertain => !drop_uncertain,
            };
            keep.then_some(candidate.id)
        })
        .collect()
}

/// Validate `verdicts` against `candidates` — map line 1999: an id the
/// original never had, or an empty selection, is a validation failure the
/// caller must fail open on, never a partial application.
fn validate(verdicts: &[Verdict], candidates: &[Candidate]) -> Result<(), ()> {
    if verdicts.is_empty() {
        return Err(());
    }
    let known: HashSet<usize> = candidates.iter().map(|c| c.id).collect();
    if verdicts.iter().any(|v| !known.contains(&v.id)) {
        return Err(());
    }
    Ok(())
}

// ===========================================================================
// The privacy gate — map line 2003.
// ===========================================================================

/// Filename shapes this build refuses to send to any reducer, regardless of
/// configuration — `.env` files and the other credential-shaped files a tool
/// result's own path metadata can name. Matched against the final path
/// segment only, case-sensitively: these are conventional, exact names, not
/// a pattern language, and a project's own `.env.example` (which is not a
/// secret) is deliberately not on this list.
const SECRET_FILE_NAMES: &[&str] = &[
    ".env",
    ".env.local",
    ".env.development",
    ".env.production",
    ".env.test",
    ".npmrc",
    ".netrc",
    "credentials.json",
    "secrets.yaml",
    "secrets.yml",
    "id_rsa",
    "id_ed25519",
];

/// Filename suffixes this build refuses to send, on the same basis.
const SECRET_FILE_SUFFIXES: &[&str] = &[".pem", ".key", ".pfx", ".p12"];

/// Whether `path`'s final segment is one of this build's secret-file
/// defaults — map line 2003. No configuration widens or narrows this list:
/// it is the floor every mode and every reducer configuration sits above,
/// exactly like [`super::eligibility`]'s hard block that no `--tool` flag
/// can lift.
pub fn is_secret_shaped_path(path: &str) -> bool {
    let name = path.rsplit(['/', '\\']).next().unwrap_or(path);
    SECRET_FILE_NAMES.contains(&name)
        || SECRET_FILE_SUFFIXES
            .iter()
            .any(|suffix| name.ends_with(suffix))
}

/// Whether any of `file_paths` blocks semantic reduction outright — map line
/// 2003, run before any candidate leaves the process.
pub fn privacy_blocks_reduction(file_paths: &[String]) -> bool {
    file_paths.iter().any(|path| is_secret_shaped_path(path))
}

// ===========================================================================
// The prompt. One template, one place.
// ===========================================================================

/// The whole of what a reducer is asked, as one template — map line 1998's
/// "the prompt is small and its full text lives in one commented constant".
/// `{task}`, `{tool_name}`, `{tool_query}` and `{candidates}` are the only
/// substitutions [`build_prompt`] makes.
const REDUCER_PROMPT_TEMPLATE: &str = "\
You are the semantic stage of a context firewall inside a coding session. A \
deterministic pass already removed exact duplicate lines; below are exactly \
the lines it kept, each carrying a stable id. Decide which of them the \
coding session still needs to see.

Task: {task}
Tool: {tool_name}
Query: {tool_query}

Candidates:
{candidates}
Reply with ONLY a JSON object of this exact shape, and nothing else:
{{\"selections\":[{{\"id\":<id>,\"relevance\":\"relevant\"|\"uncertain\"|\"discard\",\"reason\":\"<short reason>\"}}]}}
Every id listed above must appear exactly once. Never invent an id that is \
not listed above. When genuinely unsure, answer \"uncertain\" rather than \
guessing — a wrong \"discard\" loses evidence a person may need; a wrong \
\"uncertain\" only costs a few extra tokens.\
";

fn build_prompt(request: &ReductionRequest<'_>) -> String {
    let mut candidates = String::new();
    for candidate in request.candidates {
        candidates.push_str("id=");
        candidates.push_str(&candidate.id.to_string());
        candidates.push_str(" text=");
        candidates.push_str(&candidate.text);
        if !candidate.text.ends_with('\n') {
            candidates.push('\n');
        }
    }
    let task = if request.task.is_empty() {
        "(none stated)"
    } else {
        request.task
    };
    REDUCER_PROMPT_TEMPLATE
        .replace("{task}", task)
        .replace("{tool_name}", request.tool_name)
        .replace("{tool_query}", request.tool_query.unwrap_or("(none)"))
        .replace("{candidates}", &candidates)
}

// ===========================================================================
// The reply schema.
// ===========================================================================

#[derive(Debug, Deserialize)]
struct RawSelections {
    selections: Vec<RawVerdict>,
}

#[derive(Debug, Deserialize)]
struct RawVerdict {
    id: usize,
    relevance: String,
    #[serde(default)]
    reason: String,
}

/// Parse a reducer's raw content string into verdicts — a schema failure for
/// anything that is not exactly this shape, including an unrecognized
/// `relevance` spelling. Never a partial parse: one bad entry fails the
/// whole reply, because a caller that kept the good half would be trusting
/// text this module could not fully make sense of.
fn parse_selection(content: &str) -> Result<Vec<Verdict>, ()> {
    let raw: RawSelections = serde_json::from_str(content).map_err(|_| ())?;
    raw.selections
        .into_iter()
        .map(|v| {
            Relevance::parse(&v.relevance).map(|relevance| Verdict {
                id: v.id,
                relevance,
                reason: v.reason,
            })
        })
        .collect::<Option<Vec<_>>>()
        .ok_or(())
}

// ===========================================================================
// The one disposable-backed implementation.
// ===========================================================================

/// Why a configured reducer could not be built at all — construction-time
/// configuration problems, distinct from [`ReducerError`], which is about a
/// call that was attempted. Mirrors
/// [`crate::memory::extract::model::ConfiguredModelError`] exactly, restated
/// for the reducer's own job name.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ConfiguredReducerError {
    #[error(
        "the context-firewall reducer speaks OpenAI chat completions, and `{provider}` serves \
         `{protocol}`; configure a provider that serves openai-chat"
    )]
    UnsupportedProtocol {
        provider: String,
        protocol: WireProtocol,
    },
    #[error("`{provider}` declares no wire protocol, so there is nothing to send a request to")]
    NoProtocol { provider: String },
    #[error("`{provider}` has no base URL configured, so there is nowhere to send a request")]
    NoBaseUrl { provider: String },
    #[error(
        "`{provider}`'s base URL carries a credential; put it in a credential variable or a \
         header and leave the URL a URL"
    )]
    CredentialInBaseUrl { provider: String },
    #[error(
        "`{provider}` names a credential variable and none of them is set; a local model needs \
         no credential and should name none"
    )]
    NoCredential { provider: String },
}

const CONNECT_TIMEOUT: Duration = Duration::from_secs(3);
const RESPONSE_TIMEOUT: Duration = Duration::from_secs(20);
const TOTAL_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_REPLY_BYTES: u64 = 1024 * 1024;
/// A selection reply is a short JSON document, one entry per candidate — a
/// few thousand candidates' worth still fits comfortably under extraction's
/// own ceiling, so the same budget is reused rather than re-derived.
const MAX_OUTPUT_TOKENS: u32 = 4096;

/// A cheap or local model, asked to select over numbered candidates —
/// [`crate::memory::extract::model::ConfiguredModel`]'s shape, over the
/// reducer's own request and reply.
pub struct ConfiguredReducer {
    provider: String,
    model: String,
    endpoint: String,
    headers: Vec<(String, String)>,
    credential: Option<Secret>,
}

impl fmt::Debug for ConfiguredReducer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ConfiguredReducer")
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

impl ConfiguredReducer {
    /// Build the reducer the user named on `provider`, or say why not — see
    /// [`crate::memory::extract::model::ConfiguredModel::new`]'s own doc for
    /// why `credential` is resolved by the caller.
    pub fn new(
        provider: &crate::provider::Provider,
        model: impl Into<String>,
        credential: Option<Secret>,
    ) -> Result<Self, ConfiguredReducerError> {
        let support =
            provider
                .protocols
                .first()
                .ok_or_else(|| ConfiguredReducerError::NoProtocol {
                    provider: provider.name.clone(),
                })?;
        if support.protocol != WireProtocol::OpenAiChat {
            return Err(ConfiguredReducerError::UnsupportedProtocol {
                provider: provider.name.clone(),
                protocol: support.protocol,
            });
        }
        let base_url = support.base_url.trim_end_matches('/');
        if base_url.is_empty() {
            return Err(ConfiguredReducerError::NoBaseUrl {
                provider: provider.name.clone(),
            });
        }
        if has_userinfo(base_url) {
            return Err(ConfiguredReducerError::CredentialInBaseUrl {
                provider: provider.name.clone(),
            });
        }
        if credential.is_none() && !provider.credential_env.is_empty() {
            return Err(ConfiguredReducerError::NoCredential {
                provider: provider.name.clone(),
            });
        }

        Ok(Self {
            provider: provider.name.clone(),
            model: model.into(),
            endpoint: format!("{base_url}/chat/completions"),
            headers: provider.headers.clone(),
            credential,
        })
    }

    fn body(&self, prompt: &str) -> serde_json::Value {
        serde_json::json!({
            "model": self.model,
            "temperature": 0,
            "stream": false,
            "max_tokens": MAX_OUTPUT_TOKENS,
            "messages": [{ "role": "user", "content": prompt }],
        })
    }
}

impl Reducer for ConfiguredReducer {
    fn describe(&self) -> String {
        format!(
            "{}/{} via {}",
            self.provider,
            self.model,
            WireProtocol::OpenAiChat
        )
    }

    fn select(&self, request: &ReductionRequest<'_>) -> Result<ReducerAnswer, ReducerError> {
        let prompt = build_prompt(request);

        let agent = Agent::new_with_config(
            Agent::config_builder()
                .http_status_as_error(false)
                .max_redirects(0)
                .accept_encoding(AutoHeaderValue::None)
                .timeout_connect(Some(CONNECT_TIMEOUT))
                .timeout_recv_response(Some(RESPONSE_TIMEOUT))
                .timeout_global(Some(TOTAL_TIMEOUT))
                .build(),
        );

        let mut builder = agent
            .post(&self.endpoint)
            .header("content-type", "application/json");
        for (name, value) in &self.headers {
            builder = builder.header(name, value);
        }
        if let Some(credential) = &self.credential {
            builder = builder.header("authorization", format!("Bearer {}", credential.expose()));
        }

        let body = self.body(&prompt).to_string();
        let response = match builder.send(body.as_bytes()) {
            Ok(response) => response,
            Err(err) => return Err(ReducerError::new(transport_error(&err))),
        };

        let status = response.status().as_u16();
        if !(200..=299).contains(&status) {
            return Err(ReducerError::new(status_error(status)));
        }

        let text = match response
            .into_body()
            .with_config()
            .limit(MAX_REPLY_BYTES)
            .read_to_string()
        {
            Ok(text) => text,
            Err(_) => {
                return Err(ReducerError::new(ReducerErrorKind::Failed(
                    "the reducer's reply could not be read",
                )));
            }
        };

        let Ok((content, usage)) = parse_reply(&text) else {
            return Err(ReducerError::new(ReducerErrorKind::Schema));
        };
        let call = ReducerCallInfo {
            provider: self.provider.clone(),
            model: self.model.clone(),
            route: Some(WireProtocol::OpenAiChat.slug().to_owned()),
            input_tokens: usage.input,
            output_tokens: usage.output,
            cached_input_tokens: usage.cached,
        };

        let Ok(verdicts) = parse_selection(&content) else {
            return Err(ReducerError::with_call(ReducerErrorKind::Schema, call));
        };
        if validate(&verdicts, request.candidates).is_err() {
            return Err(ReducerError::with_call(ReducerErrorKind::Validation, call));
        }

        Ok(ReducerAnswer { verdicts, call })
    }
}

/// Whether `url`'s authority carries userinfo — see
/// [`crate::memory::extract::model`]'s identical private helper; duplicated
/// rather than shared because that one is not `pub`, and the rule is small
/// enough that sharing it would cost more than it saves.
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

/// The three token counts an OpenAI chat-completions reply's `usage` object
/// may report — a missing count is `None`, never a zero, same rule as
/// [`crate::memory::extract::model`]'s own reader.
struct Usage {
    input: Option<i64>,
    output: Option<i64>,
    cached: Option<i64>,
}

/// The assistant message and the token counts out of an OpenAI
/// chat-completions reply.
fn parse_reply(text: &str) -> Result<(String, Usage), ()> {
    let document: serde_json::Value = serde_json::from_str(text).map_err(|_| ())?;
    let content = document
        .get("choices")
        .and_then(|choices| choices.get(0))
        .and_then(|choice| choice.get("message"))
        .and_then(|message| message.get("content"))
        .and_then(serde_json::Value::as_str)
        .ok_or(())?;
    if content.trim().is_empty() {
        return Err(());
    }
    let usage = document.get("usage");
    let input = usage.and_then(|u| reported_count(u.get("prompt_tokens")));
    let output = usage.and_then(|u| reported_count(u.get("completion_tokens")));
    let cached = usage.and_then(|u| {
        reported_count(
            u.get("prompt_tokens_details")
                .and_then(|d| d.get("cached_tokens")),
        )
    });
    Ok((
        content.to_owned(),
        Usage {
            input,
            output,
            cached,
        },
    ))
}

fn reported_count(value: Option<&serde_json::Value>) -> Option<i64> {
    match value?.as_i64()? {
        count if count >= 0 => Some(count),
        _ => None,
    }
}

fn status_error(status: u16) -> ReducerErrorKind {
    match status {
        401 | 403 => ReducerErrorKind::Refused,
        408 | 504 => ReducerErrorKind::TimedOut,
        429 => ReducerErrorKind::RateLimited,
        500..=599 => ReducerErrorKind::Failed("the reducer's provider reported a server error"),
        _ => ReducerErrorKind::Failed("the reducer answered with an unexpected status"),
    }
}

fn transport_error(err: &ureq::Error) -> ReducerErrorKind {
    match err {
        ureq::Error::Timeout(_) => ReducerErrorKind::TimedOut,
        ureq::Error::Io(io) if is_timeout_kind(io.kind()) => ReducerErrorKind::TimedOut,
        _ => ReducerErrorKind::Transport,
    }
}

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
    use crate::provider::fixture::FixtureProvider;
    use crate::provider::{ProtocolSupport, Provider};

    fn provider(base_url: &str) -> Provider {
        Provider {
            name: "fixture".to_owned(),
            protocols: vec![ProtocolSupport {
                protocol: WireProtocol::OpenAiChat,
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

    fn candidates() -> Vec<Candidate> {
        vec![
            Candidate {
                id: 0,
                text: "the needle line\n".to_owned(),
            },
            Candidate {
                id: 1,
                text: "noise\n".to_owned(),
            },
        ]
    }

    fn request<'a>(candidates: &'a [Candidate]) -> ReductionRequest<'a> {
        ReductionRequest {
            task: "",
            tool_name: "Grep",
            tool_query: Some("needle"),
            candidates,
        }
    }

    fn reply_body(content: &str) -> String {
        serde_json::json!({
            "choices": [{ "message": { "content": content } }],
            "usage": { "prompt_tokens": 42, "completion_tokens": 7 }
        })
        .to_string()
    }

    #[test]
    fn a_well_formed_reply_is_read_into_verdicts_and_a_real_call() {
        let content = r#"{"selections":[{"id":0,"relevance":"relevant","reason":"the needle"},{"id":1,"relevance":"discard","reason":"noise"}]}"#;
        let fixture = FixtureProvider::answering(
            "HTTP/1.1 200 OK",
            "content-type: application/json",
            &reply_body(content),
        );
        let reducer = ConfiguredReducer::new(&provider(&fixture.base_url()), "a-model", None)
            .expect("a well-formed loopback provider builds");
        let candidates = candidates();
        let answer = reducer.select(&request(&candidates)).expect("must answer");

        assert_eq!(answer.verdicts.len(), 2);
        assert_eq!(answer.call.provider, "fixture");
        assert_eq!(answer.call.model, "a-model");
        assert_eq!(answer.call.input_tokens, Some(42));
        assert_eq!(answer.call.output_tokens, Some(7));

        let keep = decide_keep_set(&answer.verdicts, &candidates, false, false);
        assert!(keep.contains(&0));
        assert!(!keep.contains(&1));
    }

    #[test]
    fn an_unmentioned_candidate_defaults_to_uncertain_and_survives_in_safe_mode() {
        let verdicts = vec![Verdict {
            id: 0,
            relevance: Relevance::Relevant,
            reason: "named".to_owned(),
        }];
        let candidates = candidates(); // id 1 is never mentioned
        let keep = decide_keep_set(&verdicts, &candidates, false, false);
        assert!(keep.contains(&0));
        assert!(
            keep.contains(&1),
            "an unmentioned candidate must default to uncertain, never discard"
        );
    }

    #[test]
    fn aggressive_drops_uncertain_only_when_explicitly_configured() {
        let verdicts = vec![
            Verdict {
                id: 0,
                relevance: Relevance::Uncertain,
                reason: "maybe".to_owned(),
            },
            Verdict {
                id: 1,
                relevance: Relevance::Uncertain,
                reason: "maybe".to_owned(),
            },
        ];
        let candidates = candidates();

        // aggressive mode, but the config never opted into dropping uncertain.
        let keep_default = decide_keep_set(&verdicts, &candidates, true, false);
        assert!(keep_default.contains(&0) && keep_default.contains(&1));

        // aggressive mode, explicitly configured to drop uncertain.
        let keep_dropping = decide_keep_set(&verdicts, &candidates, true, true);
        assert!(keep_dropping.is_empty());

        // the same explicit setting is inert under safe mode.
        let keep_safe = decide_keep_set(&verdicts, &candidates, false, true);
        assert!(keep_safe.contains(&0) && keep_safe.contains(&1));
    }

    #[test]
    fn a_malformed_json_reply_is_a_schema_failure() {
        let fixture = FixtureProvider::answering(
            "HTTP/1.1 200 OK",
            "content-type: application/json",
            &reply_body("not json at all"),
        );
        let reducer =
            ConfiguredReducer::new(&provider(&fixture.base_url()), "a-model", None).unwrap();
        let candidates = candidates();
        let err = reducer
            .select(&request(&candidates))
            .expect_err("not-JSON content must fail");
        assert_eq!(err.kind, ReducerErrorKind::Schema);
        assert!(
            err.call.is_some(),
            "a real reply arrived, so its cost is still known"
        );
    }

    #[test]
    fn an_unknown_candidate_id_is_a_validation_failure() {
        let content = r#"{"selections":[{"id":999,"relevance":"relevant","reason":"?"}]}"#;
        let fixture = FixtureProvider::answering(
            "HTTP/1.1 200 OK",
            "content-type: application/json",
            &reply_body(content),
        );
        let reducer =
            ConfiguredReducer::new(&provider(&fixture.base_url()), "a-model", None).unwrap();
        let candidates = candidates();
        let err = reducer
            .select(&request(&candidates))
            .expect_err("an unknown id must be refused");
        assert_eq!(err.kind, ReducerErrorKind::Validation);
        assert!(err.call.is_some());
    }

    #[test]
    fn an_empty_selection_is_a_validation_failure() {
        let content = r#"{"selections":[]}"#;
        let fixture = FixtureProvider::answering(
            "HTTP/1.1 200 OK",
            "content-type: application/json",
            &reply_body(content),
        );
        let reducer =
            ConfiguredReducer::new(&provider(&fixture.base_url()), "a-model", None).unwrap();
        let candidates = candidates();
        let err = reducer
            .select(&request(&candidates))
            .expect_err("an empty selection must be refused");
        assert_eq!(err.kind, ReducerErrorKind::Validation);
    }

    #[test]
    fn a_rate_limited_status_is_reported_as_such() {
        let fixture = FixtureProvider::answering("HTTP/1.1 429 Too Many Requests", "", "");
        let reducer =
            ConfiguredReducer::new(&provider(&fixture.base_url()), "a-model", None).unwrap();
        let candidates = candidates();
        let err = reducer.select(&request(&candidates)).unwrap_err();
        assert_eq!(err.kind, ReducerErrorKind::RateLimited);
        assert!(err.call.is_none());
    }

    #[test]
    fn a_refused_credential_is_reported_as_such() {
        let fixture = FixtureProvider::answering("HTTP/1.1 401 Unauthorized", "", "");
        let reducer =
            ConfiguredReducer::new(&provider(&fixture.base_url()), "a-model", None).unwrap();
        let candidates = candidates();
        let err = reducer.select(&request(&candidates)).unwrap_err();
        assert_eq!(err.kind, ReducerErrorKind::Refused);
    }

    #[test]
    fn a_connection_that_is_never_answered_times_out() {
        let reducer =
            ConfiguredReducer::new(&provider("http://127.0.0.1:1/v1"), "a-model", None).unwrap();
        let candidates = candidates();
        let err = reducer.select(&request(&candidates)).unwrap_err();
        assert!(
            matches!(
                err.kind,
                ReducerErrorKind::Transport | ReducerErrorKind::TimedOut
            ),
            "an unreachable port must fail as a transport problem: {:?}",
            err.kind
        );
        assert!(err.call.is_none());
    }

    #[test]
    fn a_non_openai_chat_protocol_is_refused_at_construction() {
        let mut wrong = provider("https://example.invalid");
        wrong.protocols[0].protocol = WireProtocol::AnthropicMessages;
        assert!(matches!(
            ConfiguredReducer::new(&wrong, "a-model", None),
            Err(ConfiguredReducerError::UnsupportedProtocol { .. })
        ));
    }

    #[test]
    fn neither_the_credential_nor_the_base_url_reaches_the_description_or_debug() {
        const SECRET: &str = "sk-fabricated-test-value-not-a-real-credential";
        let mut hosted = provider("https://example.invalid/v1");
        hosted.credential_env = vec!["HOSTED_API_KEY".to_owned()];
        let built = ConfiguredReducer::new(&hosted, "a-model", Some(Secret::mint_for_test(SECRET)))
            .unwrap();

        let described = built.describe();
        assert!(!described.contains(SECRET));
        assert!(!described.contains("example.invalid"));

        let debugged = format!("{built:?}");
        assert!(!debugged.contains(SECRET));
        assert!(debugged.contains(REDACTED));
    }

    #[test]
    fn secret_shaped_paths_block_reduction() {
        assert!(is_secret_shaped_path(".env"));
        assert!(is_secret_shaped_path("/home/user/project/.env"));
        assert!(is_secret_shaped_path("C:\\project\\.env.production"));
        assert!(is_secret_shaped_path("config/id_rsa"));
        assert!(is_secret_shaped_path("certs/server.pem"));
        assert!(!is_secret_shaped_path(".env.example"));
        assert!(!is_secret_shaped_path("src/main.rs"));

        assert!(privacy_blocks_reduction(&[
            "src/main.rs".to_owned(),
            ".env".to_owned()
        ]));
        assert!(!privacy_blocks_reduction(&["src/main.rs".to_owned()]));
        assert!(!privacy_blocks_reduction(&[]));
    }
}
