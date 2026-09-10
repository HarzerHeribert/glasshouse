//! The Anthropic Messages wire format: turning a [`crate::contract::Conversation`]
//! into a request body, and a response back into a [`crate::contract::Message`].
//!
//! `docs/product/pane/model-contract.md` §8 fixes the one invariant this
//! module exists for: the request body is byte-identical whether
//! `ANTHROPIC_BASE_URL` names Glasshouse's gateway or nothing at all, because
//! a gateway hop that changed one byte would break the prompt cache on the
//! far side. [`request_body`] is why that invariant holds by construction --
//! it has no parameter through which a base URL could reach the body.

use std::collections::BTreeMap;
use std::env;
use std::fmt;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::contract::{Block, Conversation, Message, Role};

/// The Anthropic Messages endpoint used when `ANTHROPIC_BASE_URL` names
/// nothing.
pub const DEFAULT_BASE_URL: &str = "https://api.anthropic.com";

/// The Messages API path, appended to whichever base URL applies.
const MESSAGES_PATH: &str = "/v1/messages";

/// The model this request is for, named in the **head**.
///
/// Glasshouse's gateway routes on the request head and never on the body, so
/// a session that wants a frontier model for reasoning and a cheap one for
/// reduction has to say which is which somewhere the gateway can read before
/// it forwards a byte. The model is already in the body; this repeats it in
/// one header so the gateway need not buffer the payload to find it.
///
/// Harmless everywhere else: a provider reached directly ignores a header it
/// does not know, and Glasshouse strips it rather than forwarding it.
const MODEL_HEADER: &str = "x-glasshouse-model";

/// The `anthropic-version` header pane sends on every request. 61C's
/// `/model` slash command points at this and [`MODEL`] rather than a
/// literal, so both stay in one place.
pub const ANTHROPIC_VERSION: &str = "2023-06-01";

/// User-selected response effort. Auto leaves the existing wire body untouched.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Effort {
    #[default]
    Auto,
    Low,
    Medium,
    High,
    Xhigh,
    Max,
}
impl Effort {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "auto" => Some(Self::Auto),
            "low" => Some(Self::Low),
            "medium" => Some(Self::Medium),
            "high" => Some(Self::High),
            "xhigh" => Some(Self::Xhigh),
            "max" => Some(Self::Max),
            _ => None,
        }
    }
    pub fn name(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
            Self::Xhigh => "xhigh",
            Self::Max => "max",
        }
    }
}

/// The model pane asks for absent an explicit choice and a remembered one.
///
/// A frontier model, deliberately. The parent tier is the one a person is
/// talking to and the one whose mistakes cost a whole task; the cheap tiers
/// are `[helpers]` and `[agents]`, and they are chosen on purpose rather than
/// inherited from a timid default.
pub const MODEL: &str = "claude-opus-5";

/// The `max_tokens` pane asks for on every turn.
pub const MAX_TOKENS: u32 = 8192;

/// The base URL a turn's request goes to: `ANTHROPIC_BASE_URL` if it is set
/// to a non-empty value, [`DEFAULT_BASE_URL`] otherwise. This is the entire
/// decision behind map line 2445 -- everything else about the request is
/// fixed regardless of which URL this returns.
pub fn base_url() -> String {
    env::var("ANTHROPIC_BASE_URL")
        .ok()
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| DEFAULT_BASE_URL.to_string())
}

#[derive(Serialize)]
struct RequestBody<'a> {
    model: &'a str,
    max_tokens: u32,
    system: Vec<SystemBlock<'a>>,
    messages: Vec<WireMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<Vec<Value>>,
    /// `Some(true)` only on the streaming path.
    ///
    /// **Skipped when absent, which is what keeps the non-streaming body byte
    /// identical** — `the_gateway_hop_changes_no_byte` and the golden request
    /// test both compare whole bodies, and a `"stream":false` would be a new
    /// byte in every ordinary turn.
    #[serde(skip_serializing_if = "Option::is_none")]
    stream: Option<bool>,
}

/// Cache only the session's system prompt, before volatile conversation state.
/// A separate tools breakpoint lets changed project instructions reuse tools.
#[derive(Serialize)]
struct SystemBlock<'a> {
    #[serde(rename = "type")]
    kind: &'static str,
    text: &'a str,
    cache_control: CacheControl,
}

#[derive(Serialize)]
struct CacheControl {
    #[serde(rename = "type")]
    kind: &'static str,
}

impl<'a> RequestBody<'a> {
    fn new(
        model: &'a str,
        max_tokens: u32,
        conversation: &'a Conversation,
        tools: Vec<serde_json::Value>,
    ) -> Self {
        Self {
            model,
            max_tokens,
            system: if conversation.system.is_empty() {
                Vec::new()
            } else {
                vec![SystemBlock {
                    kind: "text",
                    text: &conversation.system,
                    cache_control: CacheControl { kind: "ephemeral" },
                }]
            },
            messages: conversation.messages.iter().map(to_wire_message).collect(),
            tools: (!tools.is_empty()).then_some(tools),
            stream: None,
        }
    }
}

#[derive(Serialize)]
struct WireMessage {
    role: &'static str,
    content: Vec<WireBlock>,
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum WireBlock {
    Text {
        text: String,
    },
    ToolUse {
        id: String,
        name: String,
        input: Value,
    },
    ToolResult {
        tool_use_id: String,
        content: String,
        #[serde(default)]
        is_error: bool,
    },
    /// A response block type this module does not send and does not act
    /// on -- 61D's sandbox does not exist, so a `tool_use` block here is
    /// data to ignore, never something to run.
    #[serde(other)]
    Other,
}

/// Serialises `conversation` into the JSON body of an Anthropic Messages
/// request. Takes no base URL, no headers, and nothing environment-derived:
/// that is what makes the byte-identity in `the_gateway_hop_changes_no_byte`
/// hold structurally rather than by care.
pub fn request_body(conversation: &Conversation) -> Vec<u8> {
    request_body_on_model(conversation, MODEL)
}

/// The same request body used for sending and estimating an explicitly selected model.
pub fn request_body_on_model(conversation: &Conversation, model: &str) -> Vec<u8> {
    request_body_configured(conversation, model, Effort::Auto)
}

/// Which tool definitions a request carries — `tool-abi.md` §3.
///
/// A visibility choice and nothing else. Every variant reaches the same
/// kernel, so this type decides what the model is *shown* and never how the
/// work runs — which is what makes an interface benchmark measure the
/// interface.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Surface {
    /// A supervisor look: no tools at all, so it cannot act.
    TextOnly,
    /// A request that may act, and the façade it acts through.
    Acting {
        interface: crate::abi::Interface,
        dialect: crate::abi::Dialect,
    },
}

impl Surface {
    /// `execute_cell` and nothing else.
    ///
    /// The surface for every narrowed context — a helper or a subagent —
    /// because a dialect row advertises a capability such a context may not
    /// bind, and a declared-but-absent tool is the one failure the
    /// narrowing exists to avoid.
    #[must_use]
    pub fn cells() -> Self {
        Self::Acting {
            interface: crate::abi::Interface::Cells,
            dialect: crate::abi::Dialect::Anthropic,
        }
    }

    /// The tool definitions this surface declares, in declaration order.
    ///
    /// The cache breakpoint sits on the **last** definition so the whole
    /// tool prefix is one cacheable block (`model-contract.md` §8). With a
    /// cells-only surface that is `execute_cell`, exactly as before.
    #[must_use]
    pub fn tool_definitions(self) -> Vec<serde_json::Value> {
        let Self::Acting { interface, dialect } = self else {
            return Vec::new();
        };
        let mut tools = Vec::new();
        if interface.declares_cell() {
            tools.push(serde_json::json!({
                "name": crate::prompt::declarations::EXECUTE_CELL_NAME,
                "description": crate::prompt::declarations::EXECUTE_CELL_DESCRIPTION,
                "input_schema": {"type":"object","properties":{"code":{"type":"string"}},"required":["code"],"additionalProperties":false},
            }));
        }
        if interface.declares_direct_tools() {
            tools.extend(
                dialect
                    .shapes()
                    .iter()
                    .map(super::abi::Shape::tool_definition),
            );
        }
        if let Some(last) = tools.last_mut()
            && let Some(object) = last.as_object_mut()
        {
            object.insert(
                "cache_control".into(),
                serde_json::json!({"type": "ephemeral"}),
            );
        }
        tools
    }
}

pub fn request_body_configured(
    conversation: &Conversation,
    model: &str,
    effort: Effort,
) -> Vec<u8> {
    request_body_for_surface(conversation, model, effort, Surface::cells())
}

/// [`request_body_configured`] for a caller that knows its own tool surface.
///
/// The task path passes the session's; helpers and subagents keep
/// [`Surface::cells`], because a narrowed context binds fewer capabilities
/// than a dialect row would advertise.
pub fn request_body_for_surface(
    conversation: &Conversation,
    model: &str,
    effort: Effort,
    surface: Surface,
) -> Vec<u8> {
    configure_effort(
        build_request_body(model, MAX_TOKENS, conversation, surface),
        model,
        effort,
    )
}

fn configure_effort(body: Vec<u8>, model: &str, effort: Effort) -> Vec<u8> {
    if effort == Effort::Auto {
        return body;
    }
    let mut value: serde_json::Value = serde_json::from_slice(&body).expect("serialized request");
    // The word, always. It is the only form that distinguishes all five
    // levels: a token budget saturates, so `high`, `xhigh` and `max` used to
    // arrive as one thing and two of the three the user picked did not exist
    // on the wire at all.
    value["output_config"] = serde_json::json!({"effort": effort.name()});
    if !model.contains("claude") {
        // A budget as well, for the Anthropic-shaped leg that reads one --
        // Glasshouse's codec keeps both and gives each target the form it
        // uses. Leave response space above the thinking allocation.
        let budget = match effort {
            Effort::Low => 4096,
            Effort::Medium => 16384,
            Effort::High => 32769,
            Effort::Xhigh => 49152,
            // `Auto` returned above; this arm is `Max` and the compiler
            // cannot see that, so it is spelled rather than a wildcard that
            // would silently absorb a sixth level.
            Effort::Max | Effort::Auto => 65536,
        };
        value["thinking"] = serde_json::json!({"type":"enabled", "budget_tokens":budget});
        value["max_tokens"] = serde_json::json!(budget + MAX_TOKENS);
    }
    serde_json::to_vec(&value).expect("serialized request")
}

fn to_wire_message(message: &Message) -> WireMessage {
    WireMessage {
        role: message.role.as_str(),
        content: message.content.iter().map(to_wire_block).collect(),
    }
}

fn to_wire_block(block: &Block) -> WireBlock {
    match block {
        Block::Text(text) => WireBlock::Text { text: text.clone() },
        Block::ToolUse { id, name, input } => WireBlock::ToolUse {
            id: id.clone(),
            name: name.clone(),
            input: input.clone(),
        },
        Block::ToolResult {
            tool_use_id,
            content,
            is_error,
        } => WireBlock::ToolResult {
            tool_use_id: tool_use_id.clone(),
            content: content.clone(),
            is_error: *is_error,
        },
    }
}

#[derive(Deserialize)]
struct ResponseBody {
    role: String,
    content: Vec<WireBlock>,
    #[serde(default)]
    stop_reason: Option<String>,
    #[serde(default)]
    usage: Option<UsageRow>,
}

/// The Messages response's own `usage` object, before the "absent or
/// malformed is `None`, never zero" rule is applied -- either field can be
/// missing without the object itself being.
#[derive(Debug, Clone, Copy, Deserialize)]
struct UsageRow {
    #[serde(default)]
    input_tokens: Option<u64>,
    #[serde(default)]
    output_tokens: Option<u64>,
    #[serde(default, deserialize_with = "optional_cache_count")]
    cache_read_input_tokens: Option<u64>,
    #[serde(default, deserialize_with = "optional_cache_count")]
    cache_creation_input_tokens: Option<u64>,
}

// Optional provider extensions must not make an otherwise valid reply fail.
// A malformed count is unknown; never coerce strings or negatives into a hit.
fn optional_cache_count<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<u64>, D::Error> {
    Ok(Value::deserialize(deserializer)?.as_u64())
}

/// The provider's own token count for the request that produced one turn.
/// `model-contract.md` §6 reads this "rather than estimated" when there is
/// nothing else to prefer -- see [`Turn::usage`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Usage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    /// Provider-reported cache reads; `None` is unknown, `Some(0)` is no hit.
    pub cache_read_input_tokens: Option<u64>,
    /// Provider-reported cache writes, separate from both reads and input tokens.
    pub cache_creation_input_tokens: Option<u64>,
}

impl Usage {
    /// Every provider-reported token class consumed by this exchange.
    /// Anthropic reports uncached input, cache reads, and cache creation as
    /// disjoint fields, so omitting either cache class would make a warm
    /// session appear to consume fewer context tokens than it actually did.
    pub fn total_tokens(&self) -> u64 {
        self.input_tokens
            .saturating_add(self.output_tokens)
            .saturating_add(self.cache_read_input_tokens.unwrap_or(0))
            .saturating_add(self.cache_creation_input_tokens.unwrap_or(0))
    }
}

/// One assistant turn: the message the runtime and rollout act on, plus the
/// provider's own usage for the request that produced it.
///
/// `usage` is `None` whenever the response carried no `usage` object, or one
/// missing either field -- never a fabricated zero, because a zero here would
/// read as "the provider reported no tokens" rather than "it reported
/// nothing".
#[derive(Debug)]
pub struct Turn {
    pub message: Message,
    pub usage: Option<Usage>,
}

fn to_usage(row: Option<UsageRow>) -> Option<Usage> {
    let row = row?;
    Some(Usage {
        input_tokens: row.input_tokens?,
        output_tokens: row.output_tokens?,
        cache_read_input_tokens: row.cache_read_input_tokens,
        cache_creation_input_tokens: row.cache_creation_input_tokens,
    })
}

/// The most of a response body an error will ever carry, in bytes.
const BODY_HEAD_LIMIT: usize = 240;

/// Everything that can go wrong sending or parsing one turn.
#[derive(Debug)]
pub enum WireError {
    /// The request could not reach a server at all: DNS, a refused
    /// connection, or any other transport-level failure below HTTP status.
    Http(Box<ureq::Error>),
    /// The server answered with a non-2xx status. `body_head` is the first
    /// `BODY_HEAD_LIMIT` bytes of its response body, escaped onto one
    /// line -- never anything from the request.
    Status { status: u16, body_head: String },
    /// The response body was not the JSON shape a Messages response has.
    Json(serde_json::Error),
    /// The response parsed, but its `role` was not `"assistant"`.
    UnexpectedRole(String),
    /// A streamed response ended without a complete reply, or carried an
    /// `error` event of its own. Distinct from [`WireError::Json`] because
    /// the bytes parsed and the *stream* was wrong.
    Stream(String),
    /// The provider exhausted its generation budget. Partial prose remains
    /// inspectable as diagnostic data, never as a completed assistant turn.
    IncompleteResponse { partial_text: String },
}

impl WireError {
    /// Whether this is the provider saying the conversation no longer fits.
    ///
    /// The invariant: **an overflow is a state to recover from, not a
    /// failure to report.** Before this existed a conversation that outgrew
    /// the window became `Status { 400 }`, the task ended, and the session
    /// was over — the one failure a long task is guaranteed to reach.
    ///
    /// Matched on the message rather than a code because there is no
    /// distinct code: every provider here answers 400, and only the body
    /// separates "too long" from "malformed". The phrases are the ones
    /// Anthropic and the OpenAI-compatible gateways actually send; an
    /// unrecognised 400 stays an ordinary error rather than being retried
    /// as an overflow, so a mis-match costs a report and never a loop.
    pub fn is_context_overflow(&self) -> bool {
        let WireError::Status { status, body_head } = self else {
            return false;
        };
        if !matches!(status, 400 | 413) {
            return false;
        }
        let body = body_head.to_ascii_lowercase();
        [
            "prompt is too long",
            "context length",
            "context_length_exceeded",
            "maximum context",
            "too many tokens",
            "exceeds the maximum",
            "input length and `max_tokens` exceed",
        ]
        .iter()
        .any(|phrase| body.contains(phrase))
    }
}

impl fmt::Display for WireError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            WireError::Http(err) => write!(f, "request failed: {err}"),
            WireError::Status { status, body_head } => {
                write!(f, "http status: {status} — {body_head}")
            }
            WireError::Json(err) => write!(f, "could not parse response: {err}"),
            WireError::UnexpectedRole(role) => write!(f, "unexpected response role {role:?}"),
            WireError::Stream(what) => write!(f, "the stream ended without a reply: {what}"),
            WireError::IncompleteResponse { partial_text } => {
                write!(
                    f,
                    "provider stopped at max_tokens; response incomplete and no code from this response was executed"
                )?;
                if !partial_text.is_empty() {
                    // Preserve the entire partial text while escaping terminal
                    // controls. It is a diagnostic, not executable input.
                    write!(f, "; partial assistant text: {partial_text:?}")?;
                }
                Ok(())
            }
        }
    }
}

impl std::error::Error for WireError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            WireError::Http(err) => Some(err),
            WireError::Status { .. } => None,
            WireError::Json(err) => Some(err),
            WireError::UnexpectedRole(_) => None,
            WireError::Stream(_) => None,
            WireError::IncompleteResponse { .. } => None,
        }
    }
}

/// Renders a provider's response body as an error's `body_head`: the first
/// `BODY_HEAD_LIMIT` bytes, cut on a char boundary, with control
/// characters escaped so the whole thing prints on one line, `…` appended
/// when the body was longer, and a fixed placeholder for an empty body.
fn body_head(body: &str) -> String {
    if body.is_empty() {
        return "(empty body)".to_string();
    }
    let mut cut = body.len().min(BODY_HEAD_LIMIT);
    while !body.is_char_boundary(cut) {
        cut -= 1;
    }
    let truncated = cut < body.len();
    let mut head: String = body[..cut]
        .chars()
        .flat_map(|c| match c {
            '\n' => "\\n".chars().collect::<Vec<_>>(),
            '\r' => "\\r".chars().collect::<Vec<_>>(),
            '\t' => "\\t".chars().collect::<Vec<_>>(),
            c if c.is_control() => format!("\\u{{{:x}}}", c as u32).chars().collect::<Vec<_>>(),
            c => vec![c],
        })
        .collect();
    if truncated {
        head.push('…');
    }
    head
}

/// The credential pane attaches to a request, and the header it goes in.
///
/// `ANTHROPIC_AUTH_TOKEN` carries a bearer token (the shape a gateway hands
/// out); `ANTHROPIC_API_KEY` carries a provider key sent as `x-api-key`.
/// Neither is read by any test -- see the packet's SECURITY section.
fn credential_header() -> Option<(&'static str, String)> {
    if let Ok(token) = env::var("ANTHROPIC_AUTH_TOKEN")
        && !token.is_empty()
    {
        return Some(("Authorization", format!("Bearer {token}")));
    }
    if let Ok(key) = env::var("ANTHROPIC_API_KEY")
        && !key.is_empty()
    {
        return Some(("x-api-key", key));
    }
    None
}

/// Sends `conversation` as one Anthropic Messages request to [`base_url`]
/// and returns the assistant's reply. Blocking, and it does not stream --
/// see the packet's OBJECTIVE for why a streaming reader is out of scope
/// here.
///
/// `http_status_as_error(false)` turns off `ureq`'s default of folding a
/// non-2xx status into `Err(ureq::Error::StatusCode)` before the body can be
/// read at all -- with it on, [`WireError::Status`]'s `body_head` would
/// always be empty. With it off, `send` only errors on an actual transport
/// failure, and status is read and handled here instead.
pub fn send_turn(conversation: &Conversation) -> Result<Turn, WireError> {
    send_turn_on_model(conversation, MODEL)
}

/// A task request with its active model, preserving provider usage accounting.
pub fn send_turn_on_model(conversation: &Conversation, model: &str) -> Result<Turn, WireError> {
    send_turn_configured(conversation, model, Effort::Auto)
}
pub fn send_turn_configured(
    conversation: &Conversation,
    model: &str,
    effort: Effort,
) -> Result<Turn, WireError> {
    send_turn_bounded(conversation, model, effort, None)
}

/// A task turn, optionally bounded.
///
/// The task path passes `None`: a person is watching it and can stop it. A
/// **helper's** loop passes [`SIDE_ERRAND_TIMEOUT`], because it runs inside a
/// native callback on another thread. Its caller can stop waiting, but the
/// owned provider request still needs this ceiling before its thread ends —
/// the same reason [`send_turn_with`] carries the ceiling.
pub fn send_turn_bounded(
    conversation: &Conversation,
    model: &str,
    effort: Effort,
    timeout: Option<std::time::Duration>,
) -> Result<Turn, WireError> {
    send_turn_bounded_with(conversation, model, effort, timeout, None)
}

/// [`send_turn_bounded`] with one caller-owned routing header. Narrowed
/// helpers use this to retain their helper identity even though they take the
/// multi-turn agent path; ordinary task and subagent traffic passes `None`.
pub fn send_turn_bounded_with(
    conversation: &Conversation,
    model: &str,
    effort: Effort,
    timeout: Option<std::time::Duration>,
    extra_header: Option<(&str, &str)>,
) -> Result<Turn, WireError> {
    send_turn_bounded_on(
        conversation,
        model,
        effort,
        timeout,
        extra_header,
        Surface::cells(),
    )
}

/// [`send_turn_bounded_with`] for a caller that knows its own tool surface.
pub fn send_turn_bounded_on(
    conversation: &Conversation,
    model: &str,
    effort: Effort,
    timeout: Option<std::time::Duration>,
    extra_header: Option<(&str, &str)>,
    surface: Surface,
) -> Result<Turn, WireError> {
    let url = format!("{}{MESSAGES_PATH}", base_url());
    let body = request_body_for_surface(conversation, model, effort, surface);

    let mut builder = ureq::post(&url).config().http_status_as_error(false);
    if let Some(timeout) = timeout {
        builder = builder.timeout_global(Some(timeout));
    }
    let mut request = builder
        .build()
        .header("content-type", "application/json")
        .header("anthropic-version", ANTHROPIC_VERSION)
        .header(MODEL_HEADER, model);
    if let Some((name, value)) = credential_header() {
        request = request.header(name, value);
    }
    if let Some((name, value)) = extra_header {
        request = request.header(name, value);
    }

    let mut response = request
        .send(body.as_slice())
        .map_err(|err| WireError::Http(Box::new(err)))?;
    let status = response.status().as_u16();
    let text = response
        .body_mut()
        .read_to_string()
        .map_err(|err| WireError::Http(Box::new(err)))?;
    if !response.status().is_success() {
        return Err(WireError::Status {
            status,
            body_head: body_head(&text),
        });
    }
    parse_response(&text)
}

/// [`send_turn`] with an explicit `model`, `max_tokens` and one optional
/// extra header -- the supervisor's look (`docs/product/pane/supervisor.md`
/// §3): a **cheaper** model than the task's own, a small `max_tokens` for its
/// one-line JSON answer, and a header the ledger can key on before the
/// gateway reads it itself.
///
/// The serializer is shared with task requests. This call supplies its own
/// model and token limit; [`send_turn`] keeps the default [`MODEL`].
/// A side errand's hard ceiling.
///
/// [`send_turn_with`] has exactly two callers -- the supervisor's look and a
/// little helper -- and neither is the task path. Both are supposed to be
/// quick questions answered on a cheap model, and a helper's call runs inside
/// a native v8 callback where `terminate_execution` cannot reach it: without
/// this, a provider that accepts and never answers outlives the cell's
/// `cell_wall_clock_s` and `/stop` both. Generous against a real answer
/// (measured: 7.5s for `gpt-5.6-luna`, 32s for a reasoning model) and finite
/// against a hang.
pub const SIDE_ERRAND_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(120);

pub fn send_turn_with(
    conversation: &Conversation,
    model: &str,
    max_tokens: u32,
    extra_header: Option<(&str, &str)>,
) -> Result<Message, WireError> {
    send_turn_with_usage(conversation, model, max_tokens, extra_header).map(|turn| turn.message)
}

/// [`send_turn_with`] without discarding the response's provider usage.
/// Helpers retain it in their durable call record; the supervisor keeps the
/// message-only compatibility wrapper above.
pub fn send_turn_with_usage(
    conversation: &Conversation,
    model: &str,
    max_tokens: u32,
    extra_header: Option<(&str, &str)>,
) -> Result<Turn, WireError> {
    let url = format!("{}{MESSAGES_PATH}", base_url());
    let body = build_request_body(model, max_tokens, conversation, Surface::TextOnly);

    let mut request = ureq::post(&url)
        .config()
        .http_status_as_error(false)
        .timeout_global(Some(SIDE_ERRAND_TIMEOUT))
        .build()
        .header("content-type", "application/json")
        .header("anthropic-version", ANTHROPIC_VERSION)
        .header(MODEL_HEADER, model);
    if let Some((name, value)) = extra_header {
        request = request.header(name, value);
    }
    if let Some((name, value)) = credential_header() {
        request = request.header(name, value);
    }

    let mut response = request
        .send(body.as_slice())
        .map_err(|err| WireError::Http(Box::new(err)))?;
    let status = response.status().as_u16();
    let text = response
        .body_mut()
        .read_to_string()
        .map_err(|err| WireError::Http(Box::new(err)))?;
    if !response.status().is_success() {
        return Err(WireError::Status {
            status,
            body_head: body_head(&text),
        });
    }
    parse_response(&text)
}

/// Shared serialization for default, selected-model, and supervisor requests.
fn build_request_body(
    model: &str,
    max_tokens: u32,
    conversation: &Conversation,
    surface: Surface,
) -> Vec<u8> {
    let body = RequestBody::new(model, max_tokens, conversation, surface.tool_definitions());
    serde_json::to_vec(&body).expect("Conversation has no non-serialisable field")
}

fn parse_response(text: &str) -> Result<Turn, WireError> {
    let parsed: ResponseBody = serde_json::from_str(text).map_err(WireError::Json)?;
    if parsed.role != "assistant" {
        return Err(WireError::UnexpectedRole(parsed.role));
    }
    if parsed.stop_reason.as_deref() == Some("max_tokens") {
        return Err(WireError::IncompleteResponse {
            partial_text: parsed
                .content
                .iter()
                .filter_map(|block| match block {
                    WireBlock::Text { text } => Some(text.as_str()),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join("\n"),
        });
    }
    let mut content = Vec::new();
    for block in parsed.content {
        match block {
            WireBlock::Text { text } => content.push(Block::Text(text)),
            WireBlock::ToolUse { id, name, input } => {
                content.push(Block::ToolUse { id, name, input })
            }
            WireBlock::ToolResult { .. } => {
                return Err(WireError::Stream(
                    "assistant response contained a user-only tool_result block".into(),
                ));
            }
            WireBlock::Other => {}
        }
    }
    Ok(Turn {
        message: Message {
            role: Role::Assistant,
            content,
            historical: None,
        },
        usage: to_usage(parsed.usage),
    })
}

/// Builds one [`Turn`] out of a Messages stream, one `data:` payload at a
/// time.
///
/// The invariant: **a streamed turn and a whole-response turn are the same
/// value.** The session, the rollout and the supervisor see no difference,
/// so streaming stays a transport concern and nothing downstream branches on
/// it. Pure over its input and holding no socket, so the parse is tested
/// without a server.
#[derive(Debug, Default)]
pub struct StreamAccumulator {
    blocks: BTreeMap<u64, PendingBlock>,
    usage: Option<UsageRow>,
    saw_stop: bool,
    stopped_at_max_tokens: bool,
}

#[derive(Debug)]
enum PendingBlock {
    Text(String),
    ToolUse {
        id: String,
        name: String,
        input: Value,
        partial: String,
        saw_delta: bool,
        stopped: bool,
    },
    Ignored,
}

/// Presentation-only progress from a stream. Recorded messages retain typed
/// blocks; callers must not append these fragments to conversation history.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StreamDelta {
    Text(String),
    /// Raw incremental JSON input bytes, for progress only. Never render as
    /// code and never append to conversation history.
    ToolInput(String),
    /// Complete decoded source after the provider closed valid tool input.
    ToolReady(String),
}

impl StreamAccumulator {
    pub fn new() -> Self {
        Self::default()
    }

    /// Feeds one SSE `data:` payload and returns presentation progress. Tool
    /// source appears only once its complete JSON input has validated.
    ///
    /// An event this does not know is ignored rather than refused — the
    /// Messages stream gains event types over time, and a `ping` or a
    /// `thinking` block is not a reason to fail a turn that is arriving
    /// correctly.
    pub fn event(&mut self, data: &str) -> Result<Option<StreamDelta>, WireError> {
        let value: serde_json::Value = serde_json::from_str(data).map_err(WireError::Json)?;
        match value.get("type").and_then(|t| t.as_str()) {
            Some("content_block_start") => {
                let index = value
                    .get("index")
                    .and_then(Value::as_u64)
                    .ok_or_else(|| WireError::Stream("content block start has no index".into()))?;
                let block = value
                    .get("content_block")
                    .ok_or_else(|| WireError::Stream("content block start has no block".into()))?;
                let pending = match block.get("type").and_then(Value::as_str) {
                    Some("text") => PendingBlock::Text(
                        block
                            .get("text")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .into(),
                    ),
                    Some("tool_use") => PendingBlock::ToolUse {
                        id: block
                            .get("id")
                            .and_then(Value::as_str)
                            .ok_or_else(|| WireError::Stream("tool_use has no id".into()))?
                            .into(),
                        name: block
                            .get("name")
                            .and_then(Value::as_str)
                            .ok_or_else(|| WireError::Stream("tool_use has no name".into()))?
                            .into(),
                        input: block
                            .get("input")
                            .cloned()
                            .unwrap_or_else(|| serde_json::json!({})),
                        partial: String::new(),
                        saw_delta: false,
                        stopped: false,
                    },
                    Some("tool_result") => {
                        return Err(WireError::Stream(
                            "assistant stream contained a user-only tool_result block".into(),
                        ));
                    }
                    _ => PendingBlock::Ignored,
                };
                if self.blocks.insert(index, pending).is_some() {
                    return Err(WireError::Stream(format!(
                        "duplicate content block index {index}"
                    )));
                }
                Ok(None)
            }
            Some("content_block_delta") => {
                // Only `text_delta`: a `signature_delta` or a
                // `thinking_delta` is part of a block this harness does not
                // put in the conversation, exactly as `WireBlock::Other`
                // drops it on the whole-response path.
                let delta = value.get("delta");
                let is_text = delta
                    .and_then(|d| d.get("type"))
                    .and_then(|t| t.as_str())
                    .is_some_and(|t| t == "text_delta");
                let index = value.get("index").and_then(Value::as_u64).unwrap_or(0);
                if is_text {
                    let Some(text) = delta.and_then(|d| d.get("text")).and_then(|t| t.as_str())
                    else {
                        return Ok(None);
                    };
                    match self
                        .blocks
                        .entry(index)
                        .or_insert_with(|| PendingBlock::Text(String::new()))
                    {
                        PendingBlock::Text(held) => held.push_str(text),
                        _ => {
                            return Err(WireError::Stream(format!(
                                "text delta targets non-text block {index}"
                            )));
                        }
                    }
                    return Ok(Some(StreamDelta::Text(text.to_string())));
                }
                if delta.and_then(|d| d.get("type")).and_then(Value::as_str)
                    == Some("input_json_delta")
                {
                    let fragment = delta
                        .and_then(|d| d.get("partial_json"))
                        .and_then(Value::as_str)
                        .ok_or_else(|| {
                            WireError::Stream("input_json_delta has no partial_json".into())
                        })?;
                    match self.blocks.get_mut(&index) {
                        Some(PendingBlock::ToolUse {
                            stopped: false,
                            partial,
                            saw_delta,
                            ..
                        }) => {
                            partial.push_str(fragment);
                            *saw_delta = true;
                        }
                        Some(PendingBlock::ToolUse { stopped: true, .. }) => {
                            return Err(WireError::Stream(format!(
                                "input delta arrived after tool_use block {index} stopped"
                            )));
                        }
                        _ => {
                            return Err(WireError::Stream(format!(
                                "input delta targets no tool_use block {index}"
                            )));
                        }
                    }
                    return Ok(Some(StreamDelta::ToolInput(fragment.to_string())));
                }
                Ok(None)
            }
            Some("content_block_stop") => {
                let index = value
                    .get("index")
                    .and_then(Value::as_u64)
                    .ok_or_else(|| WireError::Stream("content block stop has no index".into()))?;
                if let Some(PendingBlock::ToolUse {
                    input,
                    partial,
                    saw_delta,
                    stopped,
                    ..
                }) = self.blocks.get_mut(&index)
                {
                    if *stopped {
                        return Err(WireError::Stream(format!(
                            "duplicate stop for tool_use block {index}"
                        )));
                    }
                    if *saw_delta {
                        *input = serde_json::from_str(partial).map_err(|_| {
                            WireError::Stream(format!(
                                "tool_use block {index} ended with malformed input JSON"
                            ))
                        })?;
                    }
                    *stopped = true;
                    return Ok(input
                        .get("code")
                        .and_then(Value::as_str)
                        .map(|code| StreamDelta::ToolReady(code.to_string())));
                }
                Ok(None)
            }
            Some("message_start") => {
                if let Some(role) = value
                    .get("message")
                    .and_then(|m| m.get("role"))
                    .and_then(|r| r.as_str())
                    && role != "assistant"
                {
                    return Err(WireError::UnexpectedRole(role.to_string()));
                }
                self.read_usage(value.get("message").and_then(|m| m.get("usage")));
                Ok(None)
            }
            // The final `usage` lands here, and it is the one that carries
            // the output tokens; `message_start`'s is a header with zeroes.
            Some("message_delta") => {
                if value
                    .get("delta")
                    .and_then(|d| d.get("stop_reason"))
                    .and_then(Value::as_str)
                    == Some("max_tokens")
                {
                    self.stopped_at_max_tokens = true;
                }
                self.read_usage(value.get("usage"));
                Ok(None)
            }
            Some("message_stop") => {
                self.saw_stop = true;
                Ok(None)
            }
            Some("error") => Err(WireError::Stream(body_head(data))),
            _ => Ok(None),
        }
    }

    /// Merges a `usage` object, keeping whichever field actually carried a
    /// number: the stream reports input tokens at the start and output
    /// tokens at the end, and neither message has both.
    fn read_usage(&mut self, usage: Option<&serde_json::Value>) {
        let Some(usage) = usage else { return };
        let Ok(row) = serde_json::from_value::<UsageRow>(usage.clone()) else {
            return;
        };
        let held = self.usage.take().unwrap_or(UsageRow {
            input_tokens: None,
            output_tokens: None,
            cache_read_input_tokens: None,
            cache_creation_input_tokens: None,
        });
        self.usage = Some(UsageRow {
            input_tokens: row.input_tokens.or(held.input_tokens),
            output_tokens: row.output_tokens.or(held.output_tokens),
            cache_read_input_tokens: row.cache_read_input_tokens.or(held.cache_read_input_tokens),
            cache_creation_input_tokens: row
                .cache_creation_input_tokens
                .or(held.cache_creation_input_tokens),
        });
    }

    /// The turn the stream carried.
    ///
    /// **A stream that stopped early is an error, not a short reply.** A
    /// connection cut mid-reply would otherwise become a truncated answer the
    /// session appends and treats as final.
    pub fn finish(self) -> Result<Turn, WireError> {
        if !self.saw_stop {
            return Err(WireError::Stream(
                "no message_stop arrived; the connection ended mid-reply".to_string(),
            ));
        }
        if self.stopped_at_max_tokens {
            return Err(WireError::IncompleteResponse {
                partial_text: self
                    .blocks
                    .values()
                    .filter_map(|block| match block {
                        PendingBlock::Text(text) => Some(text.as_str()),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
                    .join("\n"),
            });
        }
        let mut content = Vec::new();
        for (index, block) in self.blocks {
            match block {
                PendingBlock::Text(text) if !text.is_empty() => content.push(Block::Text(text)),
                PendingBlock::ToolUse {
                    id,
                    name,
                    input,
                    stopped: true,
                    ..
                } => content.push(Block::ToolUse { id, name, input }),
                PendingBlock::ToolUse { .. } => {
                    return Err(WireError::Stream(format!(
                        "tool_use block {index} never completed"
                    )));
                }
                _ => {}
            }
        }
        Ok(Turn {
            message: Message {
                role: Role::Assistant,
                content,
                historical: None,
            },
            usage: to_usage(self.usage),
        })
    }
}

/// [`send_turn`] over a Server-Sent Events stream, calling `on_delta` with
/// each fragment of text as it arrives and returning the identical [`Turn`]
/// at the end.
///
/// The caller decides whether to stream; nothing here reads a setting. A
/// gateway that cannot stream is not detected and not fallen back to — the
/// session picks the path, so a failure is reported rather than silently
/// changing transport underneath a screen already drawing deltas.
pub fn send_turn_streaming(
    conversation: &Conversation,
    model: &str,
    on_delta: &mut dyn FnMut(StreamDelta),
) -> Result<Turn, WireError> {
    send_turn_streaming_configured(conversation, model, Effort::Auto, on_delta)
}
pub fn send_turn_streaming_configured(
    conversation: &Conversation,
    model: &str,
    effort: Effort,
    on_delta: &mut dyn FnMut(StreamDelta),
) -> Result<Turn, WireError> {
    send_turn_streaming_on(conversation, model, effort, Surface::cells(), on_delta)
}

/// [`send_turn_streaming_configured`] for a caller that knows its own tool
/// surface. `model-contract.md` §8 requires the streamed body to use the same
/// serializer and the same cache boundaries as the whole-response one, so the
/// surface reaches both through the same [`Surface::tool_definitions`].
pub fn send_turn_streaming_on(
    conversation: &Conversation,
    model: &str,
    effort: Effort,
    surface: Surface,
    on_delta: &mut dyn FnMut(StreamDelta),
) -> Result<Turn, WireError> {
    use std::io::{BufRead, BufReader};

    let url = format!("{}{MESSAGES_PATH}", base_url());
    let mut body = RequestBody::new(model, MAX_TOKENS, conversation, surface.tool_definitions());
    body.stream = Some(true);
    let body = configure_effort(
        serde_json::to_vec(&body).expect("Conversation has no non-serialisable field"),
        model,
        effort,
    );

    let mut request = ureq::post(&url)
        .config()
        .http_status_as_error(false)
        .build()
        .header("content-type", "application/json")
        .header("accept", "text/event-stream")
        .header("anthropic-version", ANTHROPIC_VERSION)
        .header(MODEL_HEADER, model);
    if let Some((name, value)) = credential_header() {
        request = request.header(name, value);
    }

    let mut response = request
        .send(body.as_slice())
        .map_err(|err| WireError::Http(Box::new(err)))?;
    let status = response.status().as_u16();
    if !response.status().is_success() {
        let text = response
            .body_mut()
            .read_to_string()
            .map_err(|err| WireError::Http(Box::new(err)))?;
        return Err(WireError::Status {
            status,
            body_head: body_head(&text),
        });
    }

    let mut accumulator = StreamAccumulator::new();
    let reader = BufReader::new(response.body_mut().as_reader());
    for line in reader.lines() {
        let line = line.map_err(|err| WireError::Http(Box::new(err.into())))?;
        // SSE: `event:` names the type, `data:` carries it, a blank line ends
        // one event. Every payload here is self-describing by its own `type`
        // field, so only `data:` is read and the framing needs no state.
        let Some(payload) = line.strip_prefix("data:") else {
            continue;
        };
        let payload = payload.trim();
        if payload.is_empty() || payload == "[DONE]" {
            continue;
        }
        if let Some(delta) = accumulator.event(payload)? {
            on_delta(delta);
        }
    }
    accumulator.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_conversation() -> Conversation {
        Conversation {
            system: "You act by writing TypeScript.".to_string(),
            messages: vec![
                Message::text(Role::User, "How many files name IntegrationId?"),
                Message::text(Role::Assistant, "```pane\nreturn 1;\n```"),
            ],
        }
    }

    #[test]
    fn request_body_carries_the_conversation() {
        let conversation = sample_conversation();
        let value: serde_json::Value =
            serde_json::from_slice(&request_body(&conversation)).unwrap();
        assert_eq!(value["model"], MODEL);
        assert_eq!(value["max_tokens"], MAX_TOKENS);
        assert_eq!(value["system"][0]["text"], conversation.system);
        assert_eq!(value["messages"][0]["role"], "user");
        assert_eq!(value["messages"][1]["role"], "assistant");
    }

    #[test]
    fn send_turn_with_names_the_model_it_is_given() {
        let conversation = sample_conversation();
        let body = build_request_body(
            "cheap-model-for-the-test",
            200,
            &conversation,
            Surface::TextOnly,
        );
        let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(value["model"], "cheap-model-for-the-test");
        assert_eq!(value["max_tokens"], 200);
        assert_ne!(
            value["model"], MODEL,
            "the look must not fall back to the task's own model"
        );
        assert!(
            value.get("tools").is_none(),
            "supervisor requests must stay text-only"
        );
        let task: serde_json::Value = serde_json::from_slice(&request_body(&conversation)).unwrap();
        assert_eq!(task["tools"][0]["name"], "execute_cell");
    }

    /// The real event sequence a Messages stream sends, captured from the
    /// gateway on 2026-09-06 — including the `thinking` block that arrives
    /// ahead of the text and must not become conversation.
    #[test]
    fn a_stream_of_deltas_becomes_the_same_turn_a_whole_response_would() {
        let mut acc = StreamAccumulator::new();
        let mut seen = String::new();
        for data in [
            r#"{"type":"message_start","message":{"role":"assistant","usage":{"input_tokens":13,"output_tokens":0}}}"#,
            r#"{"type":"ping"}"#,
            r#"{"type":"content_block_start","index":0,"content_block":{"type":"thinking","thinking":""}}"#,
            r#"{"type":"content_block_delta","index":0,"delta":{"type":"signature_delta","signature":"EpAC"}}"#,
            r#"{"type":"content_block_stop","index":0}"#,
            r#"{"type":"content_block_start","index":1,"content_block":{"type":"text","text":""}}"#,
            r#"{"type":"content_block_delta","index":1,"delta":{"type":"text_delta","text":"1, 2, "}}"#,
            r#"{"type":"content_block_delta","index":1,"delta":{"type":"text_delta","text":"3"}}"#,
            // The real Anthropic shape: `message_delta`'s usage carries the
            // output count **and no input count**, so a naive overwrite
            // loses the 13 from `message_start` and `to_usage` then reports
            // no usage at all -- the session would silently fall back to
            // estimating a figure the provider had already given it.
            r#"{"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":30}}"#,
            r#"{"type":"message_stop"}"#,
        ] {
            if let Some(StreamDelta::Text(text)) = acc.event(data).unwrap() {
                seen.push_str(&text);
            }
        }
        assert_eq!(
            seen, "1, 2, 3",
            "the deltas were not handed over as they arrived"
        );

        let turn = acc.finish().unwrap();
        assert_eq!(turn.message.role, Role::Assistant);
        assert_eq!(
            turn.message.content,
            vec![Block::Text("1, 2, 3".to_string())]
        );
        let usage = turn.usage.expect("the stream reported usage");
        assert_eq!(usage.input_tokens, 13);
        assert_eq!(
            usage.output_tokens, 30,
            "`message_start`'s zero overwrote `message_delta`'s real count"
        );
    }

    /// A connection cut mid-reply must not become a short answer the session
    /// appends and treats as final.
    #[test]
    fn a_stream_that_stops_early_is_an_error_not_a_short_reply() {
        let mut acc = StreamAccumulator::new();
        acc.event(r#"{"type":"content_block_delta","delta":{"type":"text_delta","text":"half"}}"#)
            .unwrap();
        let error = acc.finish().unwrap_err();
        assert!(
            matches!(error, WireError::Stream(_)),
            "expected a stream error, got {error:?}"
        );
    }

    #[test]
    fn an_error_event_fails_the_turn_rather_than_ending_it_empty() {
        let mut acc = StreamAccumulator::new();
        let error = acc
            .event(r#"{"type":"error","error":{"type":"overloaded_error"}}"#)
            .unwrap_err();
        assert!(matches!(error, WireError::Stream(_)), "{error:?}");
    }

    /// Streaming is opt-in at the call, and the ordinary body must not gain
    /// a byte for it — two golden tests compare whole bodies.
    #[test]
    fn the_non_streaming_body_carries_no_stream_field() {
        let conversation = sample_conversation();
        let body = String::from_utf8(request_body(&conversation)).unwrap();
        assert!(!body.contains("stream"), "{body}");
        let supervisor = String::from_utf8(build_request_body(
            "m",
            200,
            &conversation,
            Surface::TextOnly,
        ))
        .unwrap();
        assert!(!supervisor.contains("stream"), "{supervisor}");
    }

    #[test]
    fn parse_response_reads_the_assistant_text() {
        let body = r#"{"role":"assistant","content":[{"type":"text","text":"hi"}]}"#;
        let turn = parse_response(body).unwrap();
        assert_eq!(turn.message.role, Role::Assistant);
        assert_eq!(turn.message.content, vec![Block::Text("hi".to_string())]);
    }

    #[test]
    fn parse_response_preserves_a_native_call() {
        let body = r#"{"role":"assistant","content":[
            {"type":"tool_use","id":"1","name":"grep","input":{}},
            {"type":"text","text":"hi"}
        ]}"#;
        let turn = parse_response(body).unwrap();
        assert_eq!(
            turn.message.content,
            vec![
                Block::ToolUse {
                    id: "1".into(),
                    name: "grep".into(),
                    input: serde_json::json!({})
                },
                Block::Text("hi".to_string())
            ]
        );
    }

    #[test]
    fn streaming_assembles_split_native_input_and_preserves_backticks() {
        let mut acc = StreamAccumulator::new();
        for data in [
            r#"{"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"call-7","name":"execute_cell","input":{}}}"#,
            r#"{"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"{\"code\":\"const x = `a"}}"#,
            r#"{"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"b`; return x;\"}"}}"#,
            r#"{"type":"content_block_stop","index":0}"#,
            r#"{"type":"message_stop"}"#,
        ] {
            acc.event(data).unwrap();
        }
        assert_eq!(
            acc.finish().unwrap().message.content,
            vec![Block::ToolUse {
                id: "call-7".into(),
                name: "execute_cell".into(),
                input: serde_json::json!({"code":"const x = `ab`; return x;"}),
            }]
        );
    }

    #[test]
    fn incomplete_or_malformed_native_input_never_becomes_a_call() {
        let mut incomplete = StreamAccumulator::new();
        incomplete.event(r#"{"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"x","name":"execute_cell","input":{}}}"#).unwrap();
        incomplete.event(r#"{"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"{\"code\":"}}"#).unwrap();
        incomplete.event(r#"{"type":"message_stop"}"#).unwrap();
        assert!(incomplete.finish().is_err());

        let mut malformed = StreamAccumulator::new();
        malformed.event(r#"{"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"x","name":"execute_cell","input":{}}}"#).unwrap();
        malformed.event(r#"{"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"nope"}}"#).unwrap();
        assert!(
            malformed
                .event(r#"{"type":"content_block_stop","index":0}"#)
                .is_err()
        );
    }

    #[test]
    fn a_stopped_native_block_rejects_late_input_duplicate_stop_and_max_tokens() {
        fn ready() -> StreamAccumulator {
            let mut acc = StreamAccumulator::new();
            acc.event(r#"{"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"x","name":"execute_cell","input":{}}}"#).unwrap();
            acc.event(r#"{"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"{\"code\":\"return 1\"}"}}"#).unwrap();
            acc.event(r#"{"type":"content_block_stop","index":0}"#)
                .unwrap();
            acc
        }
        assert!(ready().event(r#"{"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":" "}}"#).is_err());
        assert!(
            ready()
                .event(r#"{"type":"content_block_stop","index":0}"#)
                .is_err()
        );
        let mut capped = ready();
        capped.event(r#"{"type":"message_delta","delta":{"stop_reason":"max_tokens"},"usage":{"output_tokens":10}}"#).unwrap();
        capped.event(r#"{"type":"message_stop"}"#).unwrap();
        assert!(matches!(
            capped.finish(),
            Err(WireError::IncompleteResponse { partial_text }) if partial_text.is_empty()
        ));

        let whole = r#"{"role":"assistant","stop_reason":"max_tokens","content":[{"type":"tool_use","id":"x","name":"execute_cell","input":{"code":"return 1"}}]}"#;
        assert!(matches!(
            parse_response(whole),
            Err(WireError::IncompleteResponse { partial_text }) if partial_text.is_empty()
        ));
    }

    #[test]
    fn max_token_prose_is_incomplete_and_inspectable_in_both_transports() {
        // More than BODY_HEAD_LIMIT, including Unicode and terminal controls:
        // the diagnostic keeps every character, escaping controls for display.
        let partial = format!("{}\nnext: café\u{1b}[2J", "working… ".repeat(80));
        let whole = serde_json::json!({
            "role": "assistant",
            "stop_reason": "max_tokens",
            "content": [{"type": "text", "text": partial}],
        });
        let json_error = parse_response(&whole.to_string()).unwrap_err();
        let mut streamed = StreamAccumulator::new();
        streamed.event(r#"{"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}"#).unwrap();
        streamed
            .event(
                &serde_json::json!({
                    "type": "content_block_delta", "index": 0,
                    "delta": {"type": "text_delta", "text": partial},
                })
                .to_string(),
            )
            .unwrap();
        streamed
            .event(r#"{"type":"content_block_stop","index":0}"#)
            .unwrap();
        streamed
            .event(r#"{"type":"message_delta","delta":{"stop_reason":"max_tokens"}}"#)
            .unwrap();
        streamed.event(r#"{"type":"message_stop"}"#).unwrap();
        let stream_error = streamed.finish().unwrap_err();

        for error in [&json_error, &stream_error] {
            assert!(
                matches!(error, WireError::IncompleteResponse { partial_text } if partial_text == &partial)
            );
            assert!(!error.is_context_overflow());
            let shown = error.to_string();
            assert!(shown.contains("response incomplete"), "{shown}");
            assert!(shown.contains(&format!("{partial:?}")), "{shown}");
            assert!(!shown.contains('\u{1b}'));
        }
        assert_eq!(json_error.to_string(), stream_error.to_string());
    }

    #[test]
    fn max_token_mixed_text_and_tool_input_never_returns_an_executable_turn() {
        let whole = r#"{"role":"assistant","stop_reason":"max_tokens","content":[{"type":"text","text":"Partial explanation"},{"type":"tool_use","id":"x","name":"execute_cell","input":{"code":"return 1"}}]}"#;
        assert!(
            matches!(parse_response(whole), Err(WireError::IncompleteResponse { partial_text }) if partial_text == "Partial explanation")
        );

        let mut streamed = StreamAccumulator::new();
        streamed.event(r#"{"type":"content_block_start","index":0,"content_block":{"type":"text","text":"Partial explanation"}}"#).unwrap();
        streamed.event(r#"{"type":"content_block_start","index":1,"content_block":{"type":"tool_use","id":"x","name":"execute_cell","input":{}}}"#).unwrap();
        streamed.event(r#"{"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"{\"code\":"}}"#).unwrap();
        streamed
            .event(r#"{"type":"message_delta","delta":{"stop_reason":"max_tokens"}}"#)
            .unwrap();
        streamed.event(r#"{"type":"message_stop"}"#).unwrap();
        assert!(
            matches!(streamed.finish(), Err(WireError::IncompleteResponse { partial_text }) if partial_text == "Partial explanation")
        );
    }

    #[test]
    fn assistant_tool_result_is_rejected_in_both_transports() {
        let whole = r#"{"role":"assistant","content":[{"type":"tool_result","tool_use_id":"x","content":"fake"}]}"#;
        assert!(parse_response(whole).is_err());
        let mut streamed = StreamAccumulator::new();
        assert!(streamed.event(r#"{"type":"content_block_start","index":0,"content_block":{"type":"tool_result","tool_use_id":"x","content":"fake"}}"#).is_err());
    }

    #[test]
    fn correlated_tool_result_serializes_without_plain_text_duplicate() {
        let conversation = Conversation {
            system: "s".into(),
            messages: vec![Message::tool_result("call-9", "[cell 9 returned]", false)],
        };
        let body: Value = serde_json::from_slice(&request_body(&conversation)).unwrap();
        assert_eq!(body["messages"][0]["content"].as_array().unwrap().len(), 1);
        assert_eq!(
            body["messages"][0]["content"][0],
            serde_json::json!({"type":"tool_result","tool_use_id":"call-9","content":"[cell 9 returned]","is_error":false})
        );
    }

    #[test]
    fn parse_response_reads_usage_when_present() {
        let body = r#"{"role":"assistant","content":[{"type":"text","text":"hi"}],
            "usage":{"input_tokens":10,"output_tokens":5}}"#;
        let turn = parse_response(body).unwrap();
        assert_eq!(
            turn.usage,
            Some(Usage {
                input_tokens: 10,
                output_tokens: 5,
                cache_read_input_tokens: None,
                cache_creation_input_tokens: None,
            })
        );
    }

    #[test]
    fn cache_usage_preserves_absent_zero_and_independent_provider_counts() {
        for (fields, read, written) in [
            (serde_json::json!({}), None, None),
            (
                serde_json::json!({"cache_read_input_tokens": "300", "cache_creation_input_tokens": -1}),
                None,
                None,
            ),
            (
                serde_json::json!({"cache_read_input_tokens": 3.5, "cache_creation_input_tokens": false}),
                None,
                None,
            ),
            (
                serde_json::json!({"cache_read_input_tokens": null}),
                None,
                None,
            ),
            (
                serde_json::json!({"cache_read_input_tokens": 0, "cache_creation_input_tokens": 0}),
                Some(0),
                Some(0),
            ),
            (
                serde_json::json!({"cache_read_input_tokens": 300}),
                Some(300),
                None,
            ),
            (
                serde_json::json!({"cache_creation_input_tokens": 400}),
                None,
                Some(400),
            ),
        ] {
            let mut usage = serde_json::json!({"input_tokens": 0, "output_tokens": 0});
            usage
                .as_object_mut()
                .unwrap()
                .extend(fields.as_object().unwrap().clone());
            let whole = parse_response(
                &serde_json::json!({
                    "role": "assistant", "content": [], "usage": usage
                })
                .to_string(),
            )
            .unwrap()
            .usage
            .unwrap();
            assert_eq!(whole.cache_read_input_tokens, read);
            assert_eq!(whole.cache_creation_input_tokens, written);
            let mut stream = StreamAccumulator::new();
            stream
                .event(
                    &serde_json::json!({
                        "type": "message_start", "message": {"role": "assistant", "usage": usage}
                    })
                    .to_string(),
                )
                .unwrap();
            stream
                .event(r#"{"type":"message_delta","usage":{"output_tokens":0}}"#)
                .unwrap();
            stream.event(r#"{"type":"message_stop"}"#).unwrap();
            assert_eq!(stream.finish().unwrap().usage, Some(whole));
        }
    }

    #[test]
    fn streamed_cache_usage_uses_latest_supplied_count_without_summing_or_inventing_hits() {
        let mut stream = StreamAccumulator::new();
        stream.event(r#"{"type":"message_start","message":{"role":"assistant","usage":{"input_tokens":12,"output_tokens":0,"cache_read_input_tokens":100,"cache_creation_input_tokens":200}}}"#).unwrap();
        stream.event(r#"{"type":"message_delta","usage":{"output_tokens":4,"cache_read_input_tokens":0,"cache_creation_input_tokens":300}}"#).unwrap();
        stream.event(r#"{"type":"message_stop"}"#).unwrap();
        let usage = stream.finish().unwrap().usage.unwrap();
        assert_eq!(usage.cache_read_input_tokens, Some(0));
        assert_eq!(usage.cache_creation_input_tokens, Some(300));
    }

    #[test]
    fn parse_response_without_usage_yields_none_not_zero() {
        let body = r#"{"role":"assistant","content":[{"type":"text","text":"hi"}]}"#;
        let turn = parse_response(body).unwrap();
        assert_eq!(turn.usage, None);
    }

    #[test]
    fn parse_response_with_a_partial_usage_row_yields_none() {
        let body = r#"{"role":"assistant","content":[{"type":"text","text":"hi"}],
            "usage":{"input_tokens":10}}"#;
        let turn = parse_response(body).unwrap();
        assert_eq!(turn.usage, None);
    }

    #[test]
    fn parse_response_rejects_a_non_assistant_role() {
        let body = r#"{"role":"user","content":[]}"#;
        let err = parse_response(body).unwrap_err();
        assert!(matches!(err, WireError::UnexpectedRole(role) if role == "user"));
    }
}

#[cfg(test)]
mod effort_tests {
    use super::*;
    #[test]
    fn effort_preserves_auto_bytes_and_selects_the_supported_wire_form() {
        let conversation = Conversation {
            system: "system".into(),
            messages: vec![],
        };
        assert_eq!(
            request_body_configured(&conversation, MODEL, Effort::Auto),
            request_body(&conversation)
        );
        let claude: serde_json::Value = serde_json::from_slice(&request_body_configured(
            &conversation,
            "claude-fable-5.1",
            Effort::High,
        ))
        .unwrap();
        assert_eq!(claude["output_config"]["effort"], "high");
        assert!(claude.get("thinking").is_none());
        let translated: serde_json::Value = serde_json::from_slice(&request_body_configured(
            &conversation,
            "deepseek-v4-flash",
            Effort::Medium,
        ))
        .unwrap();
        assert_eq!(translated["thinking"]["budget_tokens"], 16384);
        assert!(translated["max_tokens"].as_u64().unwrap() > 16384);
        // The word rides along on the translated leg too, because a budget
        // saturates and cannot say `xhigh` or `max`.
        assert_eq!(translated["output_config"]["effort"], "medium");
    }

    /// Five levels a person can pick must be five things on the wire.
    ///
    /// They were not: `high`, `xhigh` and `max` all became one token budget
    /// on a translated model, and on top of that `xhigh` and `max` were
    /// silently reset to `auto` whenever the model was not a Claude one --
    /// so two of the five could not be used at all.
    #[test]
    fn every_effort_level_is_a_distinct_thing_on_the_wire() {
        let conversation = Conversation {
            system: "system".into(),
            messages: vec![],
        };
        let levels = [
            Effort::Low,
            Effort::Medium,
            Effort::High,
            Effort::Xhigh,
            Effort::Max,
        ];

        let mut budgets = Vec::new();
        for effort in levels {
            let claude: serde_json::Value = serde_json::from_slice(&request_body_configured(
                &conversation,
                "claude-opus-5",
                effort,
            ))
            .unwrap();
            assert_eq!(claude["output_config"]["effort"], effort.name());

            let translated: serde_json::Value = serde_json::from_slice(
                &request_body_configured(&conversation, "deepseek-v4-flash", effort),
            )
            .unwrap();
            assert_eq!(translated["output_config"]["effort"], effort.name());
            budgets.push(translated["thinking"]["budget_tokens"].as_u64().unwrap());
        }

        let mut ascending = budgets.clone();
        ascending.sort_unstable();
        ascending.dedup();
        assert_eq!(
            ascending.len(),
            budgets.len(),
            "two levels collapsed onto one budget: {budgets:?}"
        );
        assert_eq!(ascending, budgets, "a higher level must not buy less: {budgets:?}");
    }
}
