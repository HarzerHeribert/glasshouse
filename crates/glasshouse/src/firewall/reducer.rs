//! The semantic reducer seam — Phase 57B, map lines 1997-2003.
//!
//! This module mirrors [`crate::memory::extract::model`] and
//! [`crate::memory::extract::disposable`]'s pattern, applied to a different
//! job: a trait ([`Reducer`]) a caller asks to decide over numbered
//! candidates, and the one disposable-backed implementation
//! ([`ConfiguredReducer`]) that actually calls a model, over the same
//! OpenAI-chat-completions wire protocol. It is a second *type* because the
//! request and reply shapes are this job's own — never a second *idiom* for
//! reaching a provider; [`ConfiguredReducer`] is built from a
//! [`crate::provider::Provider`] the routing layer chose, satisfying map
//! line 1997's "never a firewall-private provider client".
//!
//! No response body, and no transport error's own words, ever reach a
//! [`ReducerErrorKind`] — every failure is one of a fixed set of phrases,
//! because a provider's error body can echo a tool result that may itself
//! contain user data.
//!
//! History: design-decisions.md, "Trims: the remaining module docs, second
//! packet", firewall/reducer.rs module doc.

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
    /// Phase 58, map line 2029: [`LocalToolReducer`]'s own executable could
    /// not be started at all.
    LocalAbsent,
    /// Phase 58, map line 2029: [`LocalToolReducer`] did not answer inside
    /// its configured timeout.
    LocalTimeout,
    /// Phase 58, map line 2029: [`LocalToolReducer`]'s tool exited non-zero,
    /// or its reply was not the local contract's shape.
    LocalFailed,
    /// Phase 58, map line 2029: [`LocalToolReducer`]'s tool reported a
    /// `tool_version` that does not prefix-match the configured pin.
    LocalVersion,
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
            Self::LocalAbsent => f.write_str("the local reducer's tool could not be started"),
            Self::LocalTimeout => {
                f.write_str("the local reducer's tool did not answer inside its timeout")
            }
            Self::LocalFailed => f.write_str(
                "the local reducer's tool exited non-zero or answered outside its contract",
            ),
            Self::LocalVersion => {
                f.write_str("the local reducer's tool version does not match the configured pin")
            }
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

// ===========================================================================
// The local out-of-process reducer — Phase 58, map lines 2028-2030. See
// design-decisions.md's *The local reducer seat* for the boundary this
// implements: one subprocess per [`Reducer::select`] call, argv only, the
// contract's JSON on stdin and stdout, and never a compressed-text reply —
// a local tool answers in verdicts by id, exactly like [`ConfiguredReducer`].
// ===========================================================================

/// Seconds Claude Code allows the context-firewall hook before abandoning
/// it. Duplicated from `crate::harness::claude_code`'s own private
/// `CONTEXT_FIREWALL_HOOK_TIMEOUT_SECONDS` rather than exported across
/// modules for it — the same call `has_userinfo`'s doc comment in this same
/// file already makes: the value is small, unlikely to move on its own, and
/// not worth a `pub(crate)` seam between a harness-registration module and
/// this one.
const CONTEXT_FIREWALL_HOOK_TIMEOUT_SECONDS: u32 = 10;

/// [`LocalReducerConfig::timeout_ms`]'s default when unset — the design's
/// own 4000.
///
/// [`LocalReducerConfig::timeout_ms`]: crate::config::firewall::LocalReducerConfig::timeout_ms
pub const DEFAULT_LOCAL_REDUCER_TIMEOUT_MS: u64 = 4000;

/// Why a [`LocalToolReducer`] could not be built at all — construction-time
/// configuration problems, distinct from [`ReducerError`], which is about a
/// call that was attempted. Mirrors [`ConfiguredReducerError`]'s own shape.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum LocalReducerConfigError {
    #[error("the local reducer `{name}` has no `command`; there is nothing to spawn")]
    EmptyCommand { name: String },
    #[error(
        "the local reducer `{name}`'s timeout_ms ({timeout_ms}) leaves less than two seconds \
         inside the context-firewall hook's own {hook_timeout_seconds}-second timeout; lower \
         timeout_ms"
    )]
    TimeoutTooLarge {
        name: String,
        timeout_ms: u64,
        hook_timeout_seconds: u32,
    },
}

/// The request this module's own contract sends on a local tool's stdin —
/// design-decisions.md's *"What crosses, and what does not"*: the tool
/// name, the tool's own query, and the candidates being reduced. Never the
/// task, the transcript, memory, or a credential — there is no field here
/// able to carry any of them.
#[derive(serde::Serialize)]
struct LocalRequestWire<'a> {
    version: u32,
    tool: &'a str,
    query: Option<&'a str>,
    candidates: Vec<LocalCandidateWire<'a>>,
}

#[derive(serde::Serialize)]
struct LocalCandidateWire<'a> {
    id: usize,
    text: &'a str,
}

/// The reply this module's own contract reads from a local tool's stdout —
/// `version` is required, exactly like the request's own, so a reply
/// missing it is already "not the contract" at the parse step.
#[derive(Debug, Deserialize)]
struct LocalReplyWire {
    version: u32,
    tool_version: String,
    verdicts: Vec<LocalVerdictWire>,
}

#[derive(Debug, Deserialize)]
struct LocalVerdictWire {
    id: usize,
    relevance: String,
    #[serde(default)]
    reason: String,
}

/// A local out-of-process tool, asked to select over numbered candidates —
/// Phase 58, map lines 2028-2030. One subprocess per [`Reducer::select`]
/// call: argv only (never a shell), the contract's request JSON on stdin,
/// its reply JSON read from stdout, stderr captured for the debug log and
/// never forwarded, the environment scrubbed of every entitlement
/// credential variable, and a per-session scratch directory as its cwd
/// rather than the project root.
pub struct LocalToolReducer {
    name: String,
    command: Vec<String>,
    version_pin: Option<String>,
    timeout: Duration,
    scratch_dir: std::path::PathBuf,
    credential_vars: Vec<String>,
}

impl fmt::Debug for LocalToolReducer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LocalToolReducer")
            .field("name", &self.name)
            .field("command", &self.command)
            .field("version_pin", &self.version_pin)
            .field("timeout", &self.timeout)
            .finish()
    }
}

impl LocalToolReducer {
    /// Build the named local reducer from its configured shape, or say why
    /// not. `credential_vars` is the launch's own credential-variable
    /// filter (`EffectiveConfig::foreign_entitlement_credential_vars`,
    /// called with `None` so every entitlement's variable is scrubbed — a
    /// subprocess Glasshouse did not write has no business carrying any
    /// account's key) — resolved by the caller, exactly
    /// [`ConfiguredReducer::new`]'s own credential is.
    pub fn new(
        name: impl Into<String>,
        config: &crate::config::firewall::LocalReducerConfig,
        scratch_dir: impl Into<std::path::PathBuf>,
        credential_vars: Vec<String>,
    ) -> Result<Self, LocalReducerConfigError> {
        let name = name.into();
        if config.command.is_empty() {
            return Err(LocalReducerConfigError::EmptyCommand { name });
        }
        let timeout_ms = config
            .timeout_ms
            .unwrap_or(DEFAULT_LOCAL_REDUCER_TIMEOUT_MS);
        let floor_ms = u64::from(CONTEXT_FIREWALL_HOOK_TIMEOUT_SECONDS.saturating_sub(2)) * 1000;
        if timeout_ms > floor_ms {
            return Err(LocalReducerConfigError::TimeoutTooLarge {
                name,
                timeout_ms,
                hook_timeout_seconds: CONTEXT_FIREWALL_HOOK_TIMEOUT_SECONDS,
            });
        }
        Ok(Self {
            name,
            command: config.command.clone(),
            version_pin: config.version.clone(),
            timeout: Duration::from_millis(timeout_ms),
            scratch_dir: scratch_dir.into(),
            credential_vars,
        })
    }
}

impl Reducer for LocalToolReducer {
    fn describe(&self) -> String {
        format!("local:{}", self.name)
    }

    fn select(&self, request: &ReductionRequest<'_>) -> Result<ReducerAnswer, ReducerError> {
        if std::fs::create_dir_all(&self.scratch_dir).is_err() {
            return Err(ReducerError::new(ReducerErrorKind::LocalFailed));
        }

        let wire_request = LocalRequestWire {
            version: 1,
            tool: request.tool_name,
            query: request.tool_query,
            candidates: request
                .candidates
                .iter()
                .map(|candidate| LocalCandidateWire {
                    id: candidate.id,
                    text: &candidate.text,
                })
                .collect(),
        };
        let body = serde_json::to_vec(&wire_request)
            .expect("LocalRequestWire is a plain owned/borrowed shape and always serializes");

        let mut command = std::process::Command::new(&self.command[0]);
        command
            .args(&self.command[1..])
            .current_dir(&self.scratch_dir)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());
        for var in &self.credential_vars {
            command.env_remove(var);
        }

        let mut child = match command.spawn() {
            Ok(child) => child,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                return Err(ReducerError::new(ReducerErrorKind::LocalAbsent));
            }
            Err(_) => return Err(ReducerError::new(ReducerErrorKind::LocalFailed)),
        };

        // Stdin is written and stdout/stderr are drained on their own
        // threads, concurrently with the child running, so a tool that
        // interleaves reading and writing (or simply produces more output
        // than one OS pipe buffer holds) can never deadlock against this
        // process waiting on a full pipe.
        let mut stdin = child.stdin.take().expect("stdin was requested piped");
        let mut stdout = child.stdout.take().expect("stdout was requested piped");
        let mut stderr = child.stderr.take().expect("stderr was requested piped");

        let stdin_thread = std::thread::spawn(move || {
            use std::io::Write as _;
            let _ = stdin.write_all(&body);
            // `stdin` drops here, closing the pipe so a well-behaved tool
            // reading to EOF is not left waiting for more input.
        });
        let stdout_thread = std::thread::spawn(move || {
            use std::io::Read as _;
            let mut buf = Vec::new();
            let _ = stdout.read_to_end(&mut buf);
            buf
        });
        let stderr_thread = std::thread::spawn(move || {
            use std::io::Read as _;
            let mut buf = String::new();
            let _ = stderr.read_to_string(&mut buf);
            buf
        });

        let deadline = std::time::Instant::now() + self.timeout;
        let status = loop {
            match child.try_wait() {
                Ok(Some(status)) => break Some(status),
                Ok(None) => {
                    if std::time::Instant::now() >= deadline {
                        break None;
                    }
                    std::thread::sleep(Duration::from_millis(20));
                }
                Err(_) => break None,
            }
        };

        let Some(status) = status else {
            let _ = child.kill();
            let _ = child.wait();
            let _ = stdin_thread.join();
            let _ = stdout_thread.join();
            let _ = stderr_thread.join();
            return Err(ReducerError::new(ReducerErrorKind::LocalTimeout));
        };

        let stdout_bytes = stdout_thread.join().unwrap_or_default();
        let stderr_text = stderr_thread.join().unwrap_or_default();
        let _ = stdin_thread.join();
        if !stderr_text.is_empty() {
            tracing::debug!(
                reducer = %self.name,
                stderr = %stderr_text,
                "local reducer: stderr captured, never forwarded"
            );
        }

        if !status.success() {
            return Err(ReducerError::new(ReducerErrorKind::LocalFailed));
        }

        let Ok(reply) = serde_json::from_slice::<LocalReplyWire>(&stdout_bytes) else {
            return Err(ReducerError::new(ReducerErrorKind::LocalFailed));
        };
        if reply.version != 1 {
            return Err(ReducerError::new(ReducerErrorKind::LocalFailed));
        }

        if let Some(pin) = &self.version_pin
            && !reply.tool_version.starts_with(pin.as_str())
        {
            let call = ReducerCallInfo {
                provider: format!("local:{}", self.name),
                model: reply.tool_version,
                route: None,
                input_tokens: None,
                output_tokens: None,
                cached_input_tokens: None,
            };
            return Err(ReducerError::with_call(
                ReducerErrorKind::LocalVersion,
                call,
            ));
        }

        let Some(verdicts): Option<Vec<Verdict>> = reply
            .verdicts
            .into_iter()
            .map(|v| {
                Relevance::parse(&v.relevance).map(|relevance| Verdict {
                    id: v.id,
                    relevance,
                    reason: v.reason,
                })
            })
            .collect()
        else {
            return Err(ReducerError::new(ReducerErrorKind::LocalFailed));
        };
        if validate(&verdicts, request.candidates).is_err() {
            return Err(ReducerError::new(ReducerErrorKind::LocalFailed));
        }

        let call = ReducerCallInfo {
            provider: format!("local:{}", self.name),
            model: reply.tool_version,
            route: None,
            input_tokens: None,
            output_tokens: None,
            cached_input_tokens: None,
        };
        Ok(ReducerAnswer { verdicts, call })
    }
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

    // -----------------------------------------------------------------
    // `LocalToolReducer` — Phase 58, map lines 2028-2030.
    // -----------------------------------------------------------------

    fn local_config(command: Vec<&str>) -> crate::config::firewall::LocalReducerConfig {
        crate::config::firewall::LocalReducerConfig {
            command: command.into_iter().map(str::to_owned).collect(),
            version: None,
            timeout_ms: None,
        }
    }

    /// Map lines 2028/OBJECTIVE 1: `timeout_ms` leaving less than two
    /// seconds inside the hook's own ten-second timeout is refused at
    /// construction, naming the reducer and the timeout in its message.
    #[test]
    fn a_timeout_that_would_leave_less_than_two_seconds_is_refused_at_construction() {
        let dir = tempfile::tempdir().unwrap();
        let mut config = local_config(vec!["/bin/true"]);
        config.timeout_ms = Some(8001);
        let err = LocalToolReducer::new("too-slow", &config, dir.path(), Vec::new())
            .expect_err("8001ms leaves less than two seconds inside a ten-second hook timeout");
        assert!(matches!(
            err,
            LocalReducerConfigError::TimeoutTooLarge { .. }
        ));
        assert!(err.to_string().contains("too-slow"));
        assert!(err.to_string().contains("8001"));
    }

    /// The floor itself: exactly two seconds of headroom (8000ms of an
    /// assumed ten-second hook timeout) is accepted, never refused off by
    /// one.
    #[test]
    fn a_timeout_at_exactly_the_floor_is_accepted() {
        let dir = tempfile::tempdir().unwrap();
        let mut config = local_config(vec!["/bin/true"]);
        config.timeout_ms = Some(8000);
        assert!(LocalToolReducer::new("at-the-floor", &config, dir.path(), Vec::new()).is_ok());
    }

    /// The default (unset `timeout_ms`) is always within the floor,
    /// whatever the floor is computed from.
    #[test]
    fn an_unset_timeout_uses_the_default_and_is_always_accepted() {
        let dir = tempfile::tempdir().unwrap();
        let config = local_config(vec!["/bin/true"]);
        assert!(config.timeout_ms.is_none());
        assert!(LocalToolReducer::new("default-timeout", &config, dir.path(), Vec::new()).is_ok());
    }

    /// A table with no `command` at all has nothing to spawn — refused at
    /// construction, never at spawn time as a confusing "absent" bypass.
    #[test]
    fn an_empty_command_is_refused_at_construction() {
        let dir = tempfile::tempdir().unwrap();
        let config = local_config(vec![]);
        let err = LocalToolReducer::new("nothing", &config, dir.path(), Vec::new())
            .expect_err("an empty command has nothing to spawn");
        assert!(matches!(err, LocalReducerConfigError::EmptyCommand { .. }));
        assert!(err.to_string().contains("nothing"));
    }
}
