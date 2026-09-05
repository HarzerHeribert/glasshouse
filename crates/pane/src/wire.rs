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
    let body = RequestBody {
        model: MODEL,
        max_tokens: MAX_TOKENS,
        system: &conversation.system,
        messages: conversation.messages.iter().map(to_wire_message).collect(),
    };
    serde_json::to_vec(&body).expect("Conversation has no non-serialisable field")
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
}

/// Everything that can go wrong sending or parsing one turn.
#[derive(Debug)]
pub enum WireError {
    /// The HTTP call itself failed, including a non-2xx status.
    Http(Box<ureq::Error>),
    /// The response body was not the JSON shape a Messages response has.
    Json(serde_json::Error),
    /// The response parsed, but its `role` was not `"assistant"`.
    UnexpectedRole(String),
}

impl fmt::Display for WireError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            WireError::Http(err) => write!(f, "request failed: {err}"),
            WireError::Json(err) => write!(f, "could not parse response: {err}"),
            WireError::UnexpectedRole(role) => write!(f, "unexpected response role {role:?}"),
        }
    }
}

impl std::error::Error for WireError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            WireError::Http(err) => Some(err),
            WireError::Json(err) => Some(err),
            WireError::UnexpectedRole(_) => None,
        }
    }
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
pub fn send_turn(conversation: &Conversation) -> Result<Message, WireError> {
    let url = format!("{}{MESSAGES_PATH}", base_url());
    let body = request_body(conversation);

    let mut request = ureq::post(&url)
        .header("content-type", "application/json")
        .header("anthropic-version", ANTHROPIC_VERSION);
    if let Some((name, value)) = credential_header() {
        request = request.header(name, value);
    }

    let mut response = request
        .send(body.as_slice())
        .map_err(|err| WireError::Http(Box::new(err)))?;
    let text = response
        .body_mut()
        .read_to_string()
        .map_err(|err| WireError::Http(Box::new(err)))?;
    parse_response(&text)
}

fn parse_response(text: &str) -> Result<Message, WireError> {
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
    Ok(Message {
        role: Role::Assistant,
        content,
    })
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
    fn parse_response_reads_the_assistant_text() {
        let body = r#"{"role":"assistant","content":[{"type":"text","text":"hi"}]}"#;
        let message = parse_response(body).unwrap();
        assert_eq!(message.role, Role::Assistant);
        assert_eq!(message.content, vec![Block::Text("hi".to_string())]);
    }

    #[test]
    fn parse_response_ignores_a_non_text_block() {
        let body = r#"{"role":"assistant","content":[
            {"type":"tool_use","id":"1","name":"grep","input":{}},
            {"type":"text","text":"hi"}
        ]}"#;
        let message = parse_response(body).unwrap();
        assert_eq!(message.content, vec![Block::Text("hi".to_string())]);
    }

    #[test]
    fn parse_response_rejects_a_non_assistant_role() {
        let body = r#"{"role":"user","content":[]}"#;
        let err = parse_response(body).unwrap_err();
        assert!(matches!(err, WireError::UnexpectedRole(role) if role == "user"));
    }
}
