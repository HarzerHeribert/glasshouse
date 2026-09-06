//! The Anthropic Messages wire format: turning a [`contract::Conversation`]
//! into a request body, and a response back into a [`contract::Message`].
//!
//! `docs/product/pane/model-contract.md` §8 fixes the one invariant this
//! module exists for: the request body is byte-identical whether
//! `ANTHROPIC_BASE_URL` names Glasshouse's gateway or nothing at all, because
//! a gateway hop that changed one byte would break the prompt cache on the
//! far side. [`request_body`] is why that invariant holds by construction --
//! it has no parameter through which a base URL could reach the body.

use std::env;
use std::fmt;

use serde::{Deserialize, Serialize};

use crate::contract::{Block, Conversation, Message, Role};

/// The Anthropic Messages endpoint used when `ANTHROPIC_BASE_URL` names
/// nothing.
pub const DEFAULT_BASE_URL: &str = "https://api.anthropic.com";

/// The Messages API path, appended to whichever base URL applies.
const MESSAGES_PATH: &str = "/v1/messages";

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

/// The model pane asks for absent an explicit choice.
pub const MODEL: &str = "claude-sonnet-5";

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
    system: &'a str,
    messages: Vec<WireMessage>,
    /// `Some(true)` only on the streaming path.
    ///
    /// **Skipped when absent, which is what keeps the non-streaming body byte
    /// identical** — `the_gateway_hop_changes_no_byte` and the golden request
    /// test both compare whole bodies, and a `"stream":false` would be a new
    /// byte in every ordinary turn.
    #[serde(skip_serializing_if = "Option::is_none")]
    stream: Option<bool>,
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

pub fn request_body_configured(
    conversation: &Conversation,
    model: &str,
    effort: Effort,
) -> Vec<u8> {
    configure_effort(
        build_request_body(model, MAX_TOKENS, conversation),
        model,
        effort,
    )
}

fn configure_effort(body: Vec<u8>, model: &str, effort: Effort) -> Vec<u8> {
    if effort == Effort::Auto {
        return body;
    }
    let mut value: serde_json::Value = serde_json::from_slice(&body).expect("serialized request");
    if model.contains("claude") {
        value["output_config"] = serde_json::json!({"effort": effort.name()});
    } else {
        // Glasshouse's Anthropic-to-provider codec maps these budgets to low,
        // medium and high; leave response space above the thinking allocation.
        let budget = match effort {
            Effort::Low => 4096,
            Effort::Medium => 16384,
            _ => 32769,
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
    }
}

#[derive(Deserialize)]
struct ResponseBody {
    role: String,
    content: Vec<WireBlock>,
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
}

/// The provider's own token count for the request that produced one turn.
/// `model-contract.md` §6 reads this "rather than estimated" when there is
/// nothing else to prefer -- see [`Turn::usage`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Usage {
    pub input_tokens: u64,
    pub output_tokens: u64,
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
    /// [`BODY_HEAD_LIMIT`] bytes of its response body, escaped onto one
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
        }
    }
}

/// Renders a provider's response body as an error's `body_head`: the first
/// [`BODY_HEAD_LIMIT`] bytes, cut on a char boundary, with control
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
    let url = format!("{}{MESSAGES_PATH}", base_url());
    let body = request_body_configured(conversation, model, effort);

    let mut request = ureq::post(&url)
        .config()
        .http_status_as_error(false)
        .build()
        .header("content-type", "application/json")
        .header("anthropic-version", ANTHROPIC_VERSION);
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

/// [`send_turn`] with an explicit `model`, `max_tokens` and one optional
/// extra header -- the supervisor's look (`docs/product/pane/supervisor.md`
/// §3): a **cheaper** model than the task's own, a small `max_tokens` for its
/// one-line JSON answer, and a header the ledger can key on before the
/// gateway reads it itself.
///
/// The serializer is shared with task requests. This call supplies its own
/// model and token limit; [`send_turn`] keeps the default [`MODEL`].
pub fn send_turn_with(
    conversation: &Conversation,
    model: &str,
    max_tokens: u32,
    extra_header: Option<(&str, &str)>,
) -> Result<Message, WireError> {
    let url = format!("{}{MESSAGES_PATH}", base_url());
    let body = build_request_body(model, max_tokens, conversation);

    let mut request = ureq::post(&url)
        .config()
        .http_status_as_error(false)
        .build()
        .header("content-type", "application/json")
        .header("anthropic-version", ANTHROPIC_VERSION);
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
    parse_response(&text).map(|turn| turn.message)
}

/// Shared serialization for default, selected-model, and supervisor requests.
fn build_request_body(model: &str, max_tokens: u32, conversation: &Conversation) -> Vec<u8> {
    let body = RequestBody {
        model,
        max_tokens,
        system: &conversation.system,
        messages: conversation.messages.iter().map(to_wire_message).collect(),
        stream: None,
    };
    serde_json::to_vec(&body).expect("Conversation has no non-serialisable field")
}

fn parse_response(text: &str) -> Result<Turn, WireError> {
    let parsed: ResponseBody = serde_json::from_str(text).map_err(WireError::Json)?;
    if parsed.role != "assistant" {
        return Err(WireError::UnexpectedRole(parsed.role));
    }
    let content = parsed
        .content
        .into_iter()
        .filter_map(|block| match block {
            WireBlock::Text { text } => Some(Block::Text(text)),
            WireBlock::Other => None,
        })
        .collect();
    Ok(Turn {
        message: Message {
            role: Role::Assistant,
            content,
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
    text: String,
    usage: Option<UsageRow>,
    saw_stop: bool,
}

impl StreamAccumulator {
    pub fn new() -> Self {
        Self::default()
    }

    /// Feeds one SSE `data:` payload; returns the text it added, so a caller
    /// can render the reply as it arrives.
    ///
    /// An event this does not know is ignored rather than refused — the
    /// Messages stream gains event types over time, and a `ping` or a
    /// `thinking` block is not a reason to fail a turn that is arriving
    /// correctly.
    pub fn event(&mut self, data: &str) -> Result<Option<String>, WireError> {
        let value: serde_json::Value = serde_json::from_str(data).map_err(WireError::Json)?;
        match value.get("type").and_then(|t| t.as_str()) {
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
                if !is_text {
                    return Ok(None);
                }
                let Some(text) = delta.and_then(|d| d.get("text")).and_then(|t| t.as_str()) else {
                    return Ok(None);
                };
                self.text.push_str(text);
                Ok(Some(text.to_string()))
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
        });
        self.usage = Some(UsageRow {
            input_tokens: row.input_tokens.filter(|n| *n > 0).or(held.input_tokens),
            output_tokens: row.output_tokens.filter(|n| *n > 0).or(held.output_tokens),
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
        let content = if self.text.is_empty() {
            Vec::new()
        } else {
            vec![Block::Text(self.text)]
        };
        Ok(Turn {
            message: Message {
                role: Role::Assistant,
                content,
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
    on_delta: &mut dyn FnMut(&str),
) -> Result<Turn, WireError> {
    send_turn_streaming_configured(conversation, model, Effort::Auto, on_delta)
}
pub fn send_turn_streaming_configured(
    conversation: &Conversation,
    model: &str,
    effort: Effort,
    on_delta: &mut dyn FnMut(&str),
) -> Result<Turn, WireError> {
    use std::io::{BufRead, BufReader};

    let url = format!("{}{MESSAGES_PATH}", base_url());
    let body = RequestBody {
        model,
        max_tokens: MAX_TOKENS,
        system: &conversation.system,
        messages: conversation.messages.iter().map(to_wire_message).collect(),
        stream: Some(true),
    };
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
        .header("anthropic-version", ANTHROPIC_VERSION);
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
        if let Some(text) = accumulator.event(payload)? {
            on_delta(&text);
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
        assert_eq!(value["system"], conversation.system);
        assert_eq!(value["messages"][0]["role"], "user");
        assert_eq!(value["messages"][1]["role"], "assistant");
    }

    #[test]
    fn send_turn_with_names_the_model_it_is_given() {
        let conversation = sample_conversation();
        let body = build_request_body("cheap-model-for-the-test", 200, &conversation);
        let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(value["model"], "cheap-model-for-the-test");
        assert_eq!(value["max_tokens"], 200);
        assert_ne!(
            value["model"], MODEL,
            "the look must not fall back to the task's own model"
        );
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
            if let Some(text) = acc.event(data).unwrap() {
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
        let supervisor = String::from_utf8(build_request_body("m", 200, &conversation)).unwrap();
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
    fn parse_response_ignores_a_non_text_block() {
        let body = r#"{"role":"assistant","content":[
            {"type":"tool_use","id":"1","name":"grep","input":{}},
            {"type":"text","text":"hi"}
        ]}"#;
        let turn = parse_response(body).unwrap();
        assert_eq!(turn.message.content, vec![Block::Text("hi".to_string())]);
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
                output_tokens: 5
            })
        );
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
    }
}
