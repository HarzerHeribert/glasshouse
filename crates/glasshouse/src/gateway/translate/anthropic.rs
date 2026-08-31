//! The Anthropic Messages codec: that wire's requests, responses and stream
//! events, into and out of [`super::canonical`].
//!
//! Every field this codec meets is one of three things: carried into the
//! canonical form, **refused by name** with a reason a user can act on, or —
//! for a handful of informational response fields — ignored *by name*, listed
//! in [`IGNORED_FIELDS`] so the table can show them. There is no fourth
//! category. An unknown key is a refusal, not a pass-through, because a
//! harness's tooling rides on fields this codec has never seen only if
//! somebody looked at them first.
//!
//! The shapes here are Anthropic's own — the request Claude Code 2.1.245
//! sends to `/v1/messages`, the response it reads, the event order its SDK
//! accumulates — and a `tool_use.id` crosses this codec unchanged in both
//! directions.

use serde_json::{Map, Value, json};

use super::canonical::{
    Block, BlockStart, Delta, ImageSource, Message, Request, Response, Role, StopReason,
    StreamEvent, ToolChoice, ToolDefinition, Unsupported, Usage, json_kind,
};
use super::fields::{Fields, element};
use super::stream::{self, SseEvent};
use super::{Codec, StreamDecoder, StreamEncoder};

pub(super) const PROTOCOL: &str = "anthropic-messages";

/// The one target this codec translates, version segment stripped.
pub(super) const ENDPOINT: &str = "/messages";

/// The request fields and block shapes this codec refuses, with the reason
/// each refusal carries. The pair table's per-field rows for this side.
pub(super) const REFUSED_FIELDS: &[(&str, &str)] = &[
    (
        "cache_control",
        "OpenAI Chat has no prompt-cache hint to carry it to, and dropping it would silently \
         change what the harness asked for; launch the Claude Code child with \
         DISABLE_PROMPT_CACHING=1 so it sends none",
    ),
    (
        "thinking",
        "extended thinking has no OpenAI Chat equivalent; turn it off for this pairing",
    ),
    (
        "thinking block",
        "a thinking or redacted_thinking block cannot be replayed to an OpenAI Chat model",
    ),
    ("citations", "citations have no OpenAI Chat equivalent"),
    ("top_k", "OpenAI Chat has no top_k sampling parameter"),
    ("service_tier", "OpenAI Chat has no equivalent service tier"),
    (
        "document block",
        "a document block has no OpenAI Chat equivalent",
    ),
    (
        "built-in tool",
        "a server-side tool type (bash, text editor, computer use, web search) has no OpenAI \
         Chat equivalent; only custom tools with an input schema are translated",
    ),
    (
        "tool_result image",
        "an image inside a tool result cannot be carried: an OpenAI Chat tool message is text",
    ),
    (
        "pause_turn",
        "a paused turn is a server-tool state OpenAI Chat cannot express",
    ),
];

/// Response fields ignored by name: informational, never asked for by the
/// harness, and named here so that ignoring them is a recorded decision.
pub(super) const IGNORED_FIELDS: &[&str] = &[
    "usage.service_tier",
    "usage.server_tool_use",
    "usage.cache_creation",
    "container",
    "context_management",
];

fn reason(field: &str) -> &'static str {
    REFUSED_FIELDS
        .iter()
        .find(|(name, _)| *name == field)
        .map(|(_, reason)| *reason)
        .expect("every refusal named in this file is listed in REFUSED_FIELDS")
}

pub(super) struct Anthropic;

impl Codec for Anthropic {
    fn protocol(&self) -> &'static str {
        PROTOCOL
    }

    fn endpoint(&self) -> &'static str {
        ENDPOINT
    }

    fn decode_request(&self, body: &[u8]) -> Result<Request, Unsupported> {
        decode_request(body)
    }

    fn encode_request(&self, request: &Request) -> Vec<u8> {
        encode_request(request)
    }

    fn decode_response(&self, body: &[u8]) -> Result<Response, Unsupported> {
        decode_response(body)
    }

    fn encode_response(&self, response: &Response) -> Vec<u8> {
        encode_response(response)
    }

    fn stream_decoder(&self) -> Box<dyn StreamDecoder + Send> {
        Box::new(EventDecoder { done: false })
    }

    fn stream_encoder(&self) -> Box<dyn StreamEncoder + Send> {
        Box::new(EventEncoder)
    }

    fn error_kind(&self, status: u16) -> &'static str {
        match status {
            400 => "invalid_request_error",
            401 => "authentication_error",
            403 => "permission_error",
            404 => "not_found_error",
            413 => "request_too_large",
            429 => "rate_limit_error",
            503 | 529 => "overloaded_error",
            _ => "api_error",
        }
    }

    fn encode_error(&self, kind: &str, message: &str) -> Vec<u8> {
        json!({"type": "error", "error": {"type": kind, "message": message}})
            .to_string()
            .into_bytes()
    }

    fn encode_stream_error(&self, kind: &str, message: &str) -> Vec<u8> {
        stream::encode(
            Some("error"),
            &json!({"type": "error", "error": {"type": kind, "message": message}}).to_string(),
        )
    }

    fn decode_error(&self, body: &[u8]) -> Option<String> {
        let value: Value = serde_json::from_slice(body).ok()?;
        value
            .get("error")?
            .get("message")?
            .as_str()
            .map(str::to_owned)
    }

    fn refused_fields(&self) -> &'static [(&'static str, &'static str)] {
        REFUSED_FIELDS
    }

    fn ignored_fields(&self) -> &'static [&'static str] {
        IGNORED_FIELDS
    }
}

// --- requests -----------------------------------------------------------------

pub(super) fn decode_request(body: &[u8]) -> Result<Request, Unsupported> {
    let value: Value = serde_json::from_slice(body)
        .map_err(|_| Unsupported::new("body", "the request body is not a JSON document"))?;
    let mut top = Fields::of(value, "")?;

    let model = top.require_string("model")?;
    let max_tokens = top.take_u64("max_tokens")?;

    let system = match top.take("system") {
        None | Some(Value::Null) => None,
        Some(Value::String(text)) => Some(text),
        Some(Value::Array(items)) => {
            let mut texts = Vec::with_capacity(items.len());
            for (index, item) in items.into_iter().enumerate() {
                let path = element("system", index);
                let mut block = Fields::of(item, path.clone())?;
                let kind = block.require_string("type")?;
                if kind != "text" {
                    return Err(Unsupported::new(
                        block.at("type"),
                        format!("a system block must be text, not `{kind}`"),
                    ));
                }
                texts.push(text_block(block)?);
            }
            Some(texts.join("\n\n"))
        }
        Some(other) => {
            return Err(Unsupported::new(
                "system",
                format!(
                    "the system prompt must be a string or an array of text blocks, not {}",
                    json_kind(&other)
                ),
            ));
        }
    };

    let mut messages = Vec::new();
    for (index, item) in top
        .take_array("messages")?
        .unwrap_or_default()
        .into_iter()
        .enumerate()
    {
        messages.push(decode_message(item, &element("messages", index))?);
    }

    let mut tools = Vec::new();
    for (index, item) in top
        .take_array("tools")?
        .unwrap_or_default()
        .into_iter()
        .enumerate()
    {
        tools.push(decode_tool(item, &element("tools", index))?);
    }

    let mut parallel_tool_calls = None;
    let tool_choice = match top.take_object("tool_choice")? {
        None => None,
        Some(mut choice) => {
            let kind = choice.require_string("type")?;
            let decoded = match kind.as_str() {
                "auto" => ToolChoice::Auto,
                "any" => ToolChoice::Any,
                "none" => ToolChoice::None,
                "tool" => ToolChoice::Tool(choice.require_string("name")?),
                other => {
                    return Err(Unsupported::new(
                        choice.at("type"),
                        format!("the tool choice `{other}` is not one OpenAI Chat can express"),
                    ));
                }
            };
            if let Some(disabled) = choice.take_bool("disable_parallel_tool_use")? {
                parallel_tool_calls = Some(!disabled);
            }
            choice.finish()?;
            Some(decoded)
        }
    };

    let temperature = top.take_f64("temperature")?;
    let top_p = top.take_f64("top_p")?;
    top.refuse_if_present("top_k", reason("top_k"))?;
    let stop = top
        .take_array("stop_sequences")?
        .unwrap_or_default()
        .into_iter()
        .enumerate()
        .map(|(index, item)| match item {
            Value::String(text) => Ok(text),
            other => Err(Unsupported::new(
                element("stop_sequences", index),
                format!(
                    "a stop sequence must be a string, not {}",
                    json_kind(&other)
                ),
            )),
        })
        .collect::<Result<Vec<_>, _>>()?;
    let stream = top.take_bool("stream")?.unwrap_or(false);

    let user = match top.take_object("metadata")? {
        None => None,
        Some(mut metadata) => {
            let user = metadata.take_string("user_id")?;
            metadata.finish()?;
            user
        }
    };

    top.refuse_if_present("thinking", reason("thinking"))?;
    top.refuse_if_present("service_tier", reason("service_tier"))?;
    top.refuse_if_present("cache_control", reason("cache_control"))?;
    top.finish()?;

    Ok(Request {
        model,
        max_tokens,
        system,
        messages,
        tools,
        tool_choice,
        parallel_tool_calls,
        temperature,
        top_p,
        stop,
        stream,
        user,
    })
}

fn decode_message(value: Value, path: &str) -> Result<Message, Unsupported> {
    let mut message = Fields::of(value, path)?;
    let role = match message.require_string("role")?.as_str() {
        "user" => Role::User,
        "assistant" => Role::Assistant,
        other => {
            return Err(Unsupported::new(
                message.at("role"),
                format!("a message role must be user or assistant, not `{other}`"),
            ));
        }
    };
    let blocks = match message.take("content") {
        Some(Value::String(text)) => vec![Block::Text(text)],
        Some(Value::Array(items)) => {
            let content = message.at("content");
            items
                .into_iter()
                .enumerate()
                .map(|(index, item)| decode_block(item, &element(&content, index)))
                .collect::<Result<Vec<_>, _>>()?
        }
        Some(other) => {
            return Err(Unsupported::new(
                message.at("content"),
                format!(
                    "message content must be a string or an array of blocks, not {}",
                    json_kind(&other)
                ),
            ));
        }
        None => {
            return Err(Unsupported::new(
                message.at("content"),
                "a message must have content",
            ));
        }
    };
    message.finish()?;
    Ok(Message { role, blocks })
}

/// A text block's text, refusing the two decorations OpenAI Chat cannot carry.
fn text_block(mut block: Fields) -> Result<String, Unsupported> {
    block.refuse_if_present("cache_control", reason("cache_control"))?;
    block.refuse_if_present("citations", reason("citations"))?;
    let text = block.require_string("text")?;
    block.finish()?;
    Ok(text)
}

fn decode_block(value: Value, path: &str) -> Result<Block, Unsupported> {
    let mut block = Fields::of(value, path)?;
    let kind = block.require_string("type")?;
    block.refuse_if_present("cache_control", reason("cache_control"))?;
    match kind.as_str() {
        "text" => Ok(Block::Text(text_block(block)?)),
        "image" => {
            let mut source = block
                .take_object("source")?
                .ok_or_else(|| Unsupported::new(block.at("source"), "an image needs a source"))?;
            let decoded = match source.require_string("type")?.as_str() {
                "base64" => ImageSource::Base64 {
                    media_type: source.require_string("media_type")?,
                    data: source.require_string("data")?,
                },
                "url" => ImageSource::Url(source.require_string("url")?),
                other => {
                    return Err(Unsupported::new(
                        source.at("type"),
                        format!("an image source must be base64 or url, not `{other}`"),
                    ));
                }
            };
            source.finish()?;
            block.finish()?;
            Ok(Block::Image(decoded))
        }
        "tool_use" => {
            let id = block.require_string("id")?;
            let name = block.require_string("name")?;
            let input = match block.take("input") {
                Some(input @ Value::Object(_)) => input,
                Some(other) => {
                    return Err(Unsupported::new(
                        block.at("input"),
                        format!(
                            "a tool input must be a JSON object, not {}",
                            json_kind(&other)
                        ),
                    ));
                }
                None => Value::Object(Map::new()),
            };
            block.finish()?;
            Ok(Block::ToolUse { id, name, input })
        }
        "tool_result" => {
            let tool_use_id = block.require_string("tool_use_id")?;
            let is_error = block.take_bool("is_error")?.unwrap_or(false);
            let content = match block.take("content") {
                None | Some(Value::Null) => String::new(),
                Some(Value::String(text)) => text,
                Some(Value::Array(items)) => {
                    let content = block.at("content");
                    let mut texts = Vec::with_capacity(items.len());
                    for (index, item) in items.into_iter().enumerate() {
                        let path = element(&content, index);
                        let mut inner = Fields::of(item, path)?;
                        match inner.require_string("type")?.as_str() {
                            "text" => texts.push(text_block(inner)?),
                            "image" => {
                                return Err(Unsupported::new(
                                    inner.path().to_owned(),
                                    reason("tool_result image"),
                                ));
                            }
                            other => {
                                return Err(Unsupported::new(
                                    inner.at("type"),
                                    format!("a tool result block must be text, not `{other}`"),
                                ));
                            }
                        }
                    }
                    texts.join("\n")
                }
                Some(other) => {
                    return Err(Unsupported::new(
                        block.at("content"),
                        format!(
                            "tool result content must be a string or an array of blocks, not {}",
                            json_kind(&other)
                        ),
                    ));
                }
            };
            block.finish()?;
            Ok(Block::ToolResult {
                tool_use_id,
                content,
                is_error,
            })
        }
        "thinking" | "redacted_thinking" => {
            Err(Unsupported::new(block.at("type"), reason("thinking block")))
        }
        "document" => Err(Unsupported::new(block.at("type"), reason("document block"))),
        other => Err(Unsupported::new(
            block.at("type"),
            format!("the block type `{other}` is not one this codec carries"),
        )),
    }
}

fn decode_tool(value: Value, path: &str) -> Result<ToolDefinition, Unsupported> {
    let mut tool = Fields::of(value, path)?;
    tool.refuse_if_present("cache_control", reason("cache_control"))?;
    if let Some(kind) = tool.take_string("type")?
        && kind != "custom"
    {
        return Err(Unsupported::new(tool.at("type"), reason("built-in tool")));
    }
    let name = tool.require_string("name")?;
    let description = tool.take_string("description")?;
    let input_schema = match tool.take("input_schema") {
        Some(schema @ Value::Object(_)) => schema,
        Some(other) => {
            return Err(Unsupported::new(
                tool.at("input_schema"),
                format!(
                    "a tool's input schema must be a JSON object, not {}",
                    json_kind(&other)
                ),
            ));
        }
        None => {
            return Err(Unsupported::new(
                tool.at("input_schema"),
                "a custom tool needs an input schema",
            ));
        }
    };
    tool.finish()?;
    Ok(ToolDefinition {
        name,
        description,
        input_schema,
    })
}

pub(super) fn encode_request(request: &Request) -> Vec<u8> {
    let mut document = Map::new();
    document.insert("model".to_owned(), json!(request.model));
    if let Some(max_tokens) = request.max_tokens {
        document.insert("max_tokens".to_owned(), json!(max_tokens));
    }
    if let Some(system) = &request.system {
        document.insert("system".to_owned(), json!(system));
    }
    document.insert(
        "messages".to_owned(),
        Value::Array(
            request
                .messages
                .iter()
                .map(|message| {
                    json!({
                        "role": match message.role {
                            Role::User => "user",
                            Role::Assistant => "assistant",
                        },
                        "content": message.blocks.iter().map(encode_block).collect::<Vec<_>>(),
                    })
                })
                .collect(),
        ),
    );
    if !request.tools.is_empty() {
        document.insert(
            "tools".to_owned(),
            Value::Array(
                request
                    .tools
                    .iter()
                    .map(|tool| {
                        let mut entry = Map::new();
                        entry.insert("name".to_owned(), json!(tool.name));
                        if let Some(description) = &tool.description {
                            entry.insert("description".to_owned(), json!(description));
                        }
                        entry.insert("input_schema".to_owned(), tool.input_schema.clone());
                        Value::Object(entry)
                    })
                    .collect(),
            ),
        );
    }
    // A parallel-tool-calls setting travels on the tool choice, which is the
    // only place this wire has for it; with no tool choice given, `auto` is
    // what the wire would have assumed anyway.
    let choice = match (&request.tool_choice, request.parallel_tool_calls) {
        (None, None) => None,
        (choice, parallel) => Some((choice.clone().unwrap_or(ToolChoice::Auto), parallel)),
    };
    if let Some((choice, parallel)) = choice {
        let mut entry = Map::new();
        match choice {
            ToolChoice::Auto => {
                entry.insert("type".to_owned(), json!("auto"));
            }
            ToolChoice::Any => {
                entry.insert("type".to_owned(), json!("any"));
            }
            ToolChoice::None => {
                entry.insert("type".to_owned(), json!("none"));
            }
            ToolChoice::Tool(name) => {
                entry.insert("type".to_owned(), json!("tool"));
                entry.insert("name".to_owned(), json!(name));
            }
        }
        if let Some(parallel) = parallel {
            entry.insert("disable_parallel_tool_use".to_owned(), json!(!parallel));
        }
        document.insert("tool_choice".to_owned(), Value::Object(entry));
    }
    if let Some(temperature) = request.temperature {
        document.insert("temperature".to_owned(), json!(temperature));
    }
    if let Some(top_p) = request.top_p {
        document.insert("top_p".to_owned(), json!(top_p));
    }
    if !request.stop.is_empty() {
        document.insert("stop_sequences".to_owned(), json!(request.stop));
    }
    if request.stream {
        document.insert("stream".to_owned(), json!(true));
    }
    if let Some(user) = &request.user {
        document.insert("metadata".to_owned(), json!({"user_id": user}));
    }
    Value::Object(document).to_string().into_bytes()
}

fn encode_block(block: &Block) -> Value {
    match block {
        Block::Text(text) => json!({"type": "text", "text": text}),
        Block::Image(ImageSource::Base64 { media_type, data }) => json!({
            "type": "image",
            "source": {"type": "base64", "media_type": media_type, "data": data},
        }),
        Block::Image(ImageSource::Url(url)) => json!({
            "type": "image",
            "source": {"type": "url", "url": url},
        }),
        Block::ToolUse { id, name, input } => {
            json!({"type": "tool_use", "id": id, "name": name, "input": input})
        }
        Block::ToolResult {
            tool_use_id,
            content,
            is_error,
        } => {
            let mut entry = Map::new();
            entry.insert("type".to_owned(), json!("tool_result"));
            entry.insert("tool_use_id".to_owned(), json!(tool_use_id));
            entry.insert("content".to_owned(), json!(content));
            if *is_error {
                entry.insert("is_error".to_owned(), json!(true));
            }
            Value::Object(entry)
        }
    }
}

// --- responses ----------------------------------------------------------------

pub(super) fn decode_response(body: &[u8]) -> Result<Response, Unsupported> {
    let value: Value = serde_json::from_slice(body)
        .map_err(|_| Unsupported::new("body", "the response body is not a JSON document"))?;
    let mut top = Fields::of(value, "")?;
    let id = top.require_string("id")?;
    let kind = top.require_string("type")?;
    if kind != "message" {
        return Err(Unsupported::new(
            "type",
            format!("a response must be a message, not `{kind}`"),
        ));
    }
    let role = top.require_string("role")?;
    if role != "assistant" {
        return Err(Unsupported::new(
            "role",
            format!("a response is from the assistant, not `{role}`"),
        ));
    }
    let model = top.require_string("model")?;
    let blocks = top
        .take_array("content")?
        .unwrap_or_default()
        .into_iter()
        .enumerate()
        .map(|(index, item)| decode_response_block(item, &element("content", index)))
        .collect::<Result<Vec<_>, _>>()?;
    let stop_reason = match top.take_string("stop_reason")? {
        Some(reason) => decode_stop_reason(&reason, "stop_reason")?,
        None => {
            return Err(Unsupported::new(
                "stop_reason",
                "a complete response must say why it stopped",
            ));
        }
    };
    let stop_sequence = top.take_string("stop_sequence")?;
    let usage = match top.take_object("usage")? {
        Some(usage) => decode_usage(usage)?,
        None => Usage::default(),
    };
    top.ignore("container");
    top.ignore("context_management");
    top.finish()?;
    Ok(Response {
        id,
        model,
        blocks,
        stop_reason,
        stop_sequence,
        usage,
    })
}

fn decode_response_block(value: Value, path: &str) -> Result<Block, Unsupported> {
    let block = decode_block(value, path)?;
    match block {
        Block::Text(_) | Block::ToolUse { .. } => Ok(block),
        Block::Image(_) => Err(Unsupported::new(
            path.to_owned(),
            "a response cannot carry an image block",
        )),
        Block::ToolResult { .. } => Err(Unsupported::new(
            path.to_owned(),
            "a response cannot carry a tool result",
        )),
    }
}

fn decode_stop_reason(reason: &str, path: &str) -> Result<StopReason, Unsupported> {
    Ok(match reason {
        "end_turn" => StopReason::EndTurn,
        "max_tokens" => StopReason::MaxTokens,
        "stop_sequence" => StopReason::StopSequence,
        "tool_use" => StopReason::ToolUse,
        "refusal" => StopReason::Refusal,
        "pause_turn" => {
            return Err(Unsupported::new(
                path.to_owned(),
                self::reason("pause_turn"),
            ));
        }
        other => {
            return Err(Unsupported::new(
                path.to_owned(),
                format!("the stop reason `{other}` is not one this codec knows"),
            ));
        }
    })
}

fn decode_usage(mut usage: Fields) -> Result<Usage, Unsupported> {
    let input = usage.take_u64("input_tokens")?.unwrap_or(0);
    let output = usage.take_u64("output_tokens")?.unwrap_or(0);
    let cached = usage.take_u64("cache_read_input_tokens")?;
    // Tokens written to the cache were still input tokens; the form counts
    // them as such rather than losing them.
    let written = usage.take_u64("cache_creation_input_tokens")?.unwrap_or(0);
    usage.ignore("service_tier");
    usage.ignore("server_tool_use");
    usage.ignore("cache_creation");
    usage.finish()?;
    Ok(Usage {
        input: input + written,
        output,
        cached,
    })
}

fn usage_json(usage: &Usage) -> Value {
    let mut entry = Map::new();
    entry.insert("input_tokens".to_owned(), json!(usage.input));
    entry.insert("output_tokens".to_owned(), json!(usage.output));
    if let Some(cached) = usage.cached {
        entry.insert("cache_read_input_tokens".to_owned(), json!(cached));
    }
    Value::Object(entry)
}

fn stop_reason_json(reason: StopReason) -> Value {
    json!(match reason {
        StopReason::EndTurn => "end_turn",
        StopReason::MaxTokens => "max_tokens",
        StopReason::StopSequence => "stop_sequence",
        StopReason::ToolUse => "tool_use",
        StopReason::Refusal => "refusal",
    })
}

pub(super) fn encode_response(response: &Response) -> Vec<u8> {
    json!({
        "id": response.id,
        "type": "message",
        "role": "assistant",
        "model": response.model,
        "content": response.blocks.iter().map(encode_block).collect::<Vec<_>>(),
        "stop_reason": stop_reason_json(response.stop_reason),
        "stop_sequence": response.stop_sequence,
        "usage": usage_json(&response.usage),
    })
    .to_string()
    .into_bytes()
}

// --- streams ------------------------------------------------------------------

struct EventEncoder;

impl StreamEncoder for EventEncoder {
    fn encode(&mut self, event: &StreamEvent) -> Vec<u8> {
        let (name, data) = match event {
            StreamEvent::MessageStart { id, model, usage } => (
                "message_start",
                json!({
                    "type": "message_start",
                    "message": {
                        "id": id,
                        "type": "message",
                        "role": "assistant",
                        "model": model,
                        "content": [],
                        "stop_reason": null,
                        "stop_sequence": null,
                        "usage": usage_json(usage),
                    },
                }),
            ),
            StreamEvent::BlockStart { index, block } => (
                "content_block_start",
                json!({
                    "type": "content_block_start",
                    "index": index,
                    "content_block": match block {
                        BlockStart::Text => json!({"type": "text", "text": ""}),
                        BlockStart::ToolUse { id, name } => {
                            json!({"type": "tool_use", "id": id, "name": name, "input": {}})
                        }
                    },
                }),
            ),
            StreamEvent::BlockDelta { index, delta } => (
                "content_block_delta",
                json!({
                    "type": "content_block_delta",
                    "index": index,
                    "delta": match delta {
                        Delta::Text(text) => json!({"type": "text_delta", "text": text}),
                        Delta::InputJson(partial) => {
                            json!({"type": "input_json_delta", "partial_json": partial})
                        }
                    },
                }),
            ),
            StreamEvent::BlockStop { index } => (
                "content_block_stop",
                json!({"type": "content_block_stop", "index": index}),
            ),
            StreamEvent::MessageDelta {
                stop_reason,
                stop_sequence,
                usage,
            } => (
                "message_delta",
                json!({
                    "type": "message_delta",
                    "delta": {
                        "stop_reason": stop_reason_json(*stop_reason),
                        "stop_sequence": stop_sequence,
                    },
                    "usage": usage_json(usage),
                }),
            ),
            StreamEvent::MessageStop => ("message_stop", json!({"type": "message_stop"})),
        };
        stream::encode(Some(name), &data.to_string())
    }
}

struct EventDecoder {
    done: bool,
}

impl StreamDecoder for EventDecoder {
    fn feed(&mut self, event: &SseEvent) -> Result<Vec<StreamEvent>, Unsupported> {
        if self.done {
            return Ok(Vec::new());
        }
        let value: Value = serde_json::from_str(&event.data)
            .map_err(|_| Unsupported::new("event", "a stream event was not a JSON document"))?;
        let mut top = Fields::of(value, "")?;
        let kind = top.require_string("type")?;
        let decoded = match kind.as_str() {
            "ping" => return Ok(Vec::new()),
            "message_start" => {
                let mut message = top
                    .take_object("message")?
                    .ok_or_else(|| Unsupported::new("message", "message_start has no message"))?;
                let id = message.require_string("id")?;
                let model = message.require_string("model")?;
                let usage = match message.take_object("usage")? {
                    Some(usage) => decode_usage(usage)?,
                    None => Usage::default(),
                };
                message.ignore("type");
                message.ignore("role");
                message.ignore("content");
                message.ignore("stop_reason");
                message.ignore("stop_sequence");
                message.ignore("container");
                message.ignore("context_management");
                message.finish()?;
                StreamEvent::MessageStart { id, model, usage }
            }
            "content_block_start" => {
                let index = require_index(&mut top)?;
                let mut block = top.take_object("content_block")?.ok_or_else(|| {
                    Unsupported::new("content_block", "content_block_start has no block")
                })?;
                let block_kind = block.require_string("type")?;
                let started = match block_kind.as_str() {
                    "text" => {
                        block.ignore("text");
                        block.ignore("citations");
                        BlockStart::Text
                    }
                    "tool_use" => {
                        let id = block.require_string("id")?;
                        let name = block.require_string("name")?;
                        block.ignore("input");
                        BlockStart::ToolUse { id, name }
                    }
                    "thinking" | "redacted_thinking" => {
                        return Err(Unsupported::new(block.at("type"), reason("thinking block")));
                    }
                    other => {
                        return Err(Unsupported::new(
                            block.at("type"),
                            format!(
                                "the streamed block type `{other}` is not one this codec carries"
                            ),
                        ));
                    }
                };
                block.finish()?;
                StreamEvent::BlockStart {
                    index,
                    block: started,
                }
            }
            "content_block_delta" => {
                let index = require_index(&mut top)?;
                let mut delta = top
                    .take_object("delta")?
                    .ok_or_else(|| Unsupported::new("delta", "content_block_delta has no delta"))?;
                let delta_kind = delta.require_string("type")?;
                let decoded = match delta_kind.as_str() {
                    "text_delta" => Delta::Text(delta.require_string("text")?),
                    "input_json_delta" => Delta::InputJson(delta.require_string("partial_json")?),
                    "thinking_delta" | "signature_delta" => {
                        return Err(Unsupported::new(delta.at("type"), reason("thinking block")));
                    }
                    "citations_delta" => {
                        return Err(Unsupported::new(delta.at("type"), reason("citations")));
                    }
                    other => {
                        return Err(Unsupported::new(
                            delta.at("type"),
                            format!("the delta type `{other}` is not one this codec carries"),
                        ));
                    }
                };
                delta.finish()?;
                StreamEvent::BlockDelta {
                    index,
                    delta: decoded,
                }
            }
            "content_block_stop" => StreamEvent::BlockStop {
                index: require_index(&mut top)?,
            },
            "message_delta" => {
                let mut delta = top
                    .take_object("delta")?
                    .ok_or_else(|| Unsupported::new("delta", "message_delta has no delta"))?;
                let stop_reason = match delta.take_string("stop_reason")? {
                    Some(reason) => decode_stop_reason(&reason, "delta.stop_reason")?,
                    None => {
                        return Err(Unsupported::new(
                            "delta.stop_reason",
                            "message_delta must say why the message stopped",
                        ));
                    }
                };
                let stop_sequence = delta.take_string("stop_sequence")?;
                delta.ignore("container");
                delta.finish()?;
                let usage = match top.take_object("usage")? {
                    Some(usage) => decode_usage(usage)?,
                    None => Usage::default(),
                };
                top.ignore("context_management");
                StreamEvent::MessageDelta {
                    stop_reason,
                    stop_sequence,
                    usage,
                }
            }
            "message_stop" => {
                self.done = true;
                StreamEvent::MessageStop
            }
            "error" => {
                let message = top
                    .take_object("error")?
                    .and_then(|mut error| error.take_string("message").ok().flatten())
                    .unwrap_or_default();
                return Err(Unsupported::new(
                    "error",
                    format!("the provider's stream reported an error: {message}"),
                ));
            }
            other => {
                return Err(Unsupported::new(
                    "type",
                    format!("the stream event `{other}` is not one this codec knows"),
                ));
            }
        };
        top.finish()?;
        Ok(vec![decoded])
    }

    fn finish(&mut self) -> Result<Vec<StreamEvent>, Unsupported> {
        if self.done {
            return Ok(Vec::new());
        }
        Err(Unsupported::new(
            "message_stop",
            "the provider's stream ended before message_stop",
        ))
    }

    fn is_done(&self) -> bool {
        self.done
    }
}

fn require_index(top: &mut Fields) -> Result<usize, Unsupported> {
    let index = top
        .take_u64("index")?
        .ok_or_else(|| Unsupported::new("index", "a block event needs an index"))?;
    usize::try_from(index).map_err(|_| Unsupported::new("index", "the block index is out of range"))
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;

    use super::super::stream::SseReader;
    use std::io::BufReader;

    /// The supported subset, all of it: a system prompt, a text turn, a
    /// tool-using assistant turn with two parallel calls, a user turn with
    /// one erroring result and one good one plus a follow-up text and two
    /// images, three tools, a forced tool choice with parallelism disabled,
    /// every generation parameter, and a user id.
    pub(crate) fn full_request() -> Request {
        Request {
            model: "claude-x".to_owned(),
            max_tokens: Some(4096),
            system: Some("You are careful.".to_owned()),
            messages: vec![
                Message {
                    role: Role::User,
                    blocks: vec![Block::Text("List the files, then read one.".to_owned())],
                },
                Message {
                    role: Role::Assistant,
                    blocks: vec![
                        Block::Text("On it.".to_owned()),
                        Block::ToolUse {
                            id: "toolu_01A".to_owned(),
                            name: "Bash".to_owned(),
                            input: json!({"command": "ls"}),
                        },
                        Block::ToolUse {
                            id: "toolu_01B".to_owned(),
                            name: "Read".to_owned(),
                            input: json!({"file_path": "/tmp/notes.md"}),
                        },
                    ],
                },
                Message {
                    role: Role::User,
                    blocks: vec![
                        Block::ToolResult {
                            tool_use_id: "toolu_01A".to_owned(),
                            content: "ls: cannot access".to_owned(),
                            is_error: true,
                        },
                        Block::ToolResult {
                            tool_use_id: "toolu_01B".to_owned(),
                            content: "# notes\nhello".to_owned(),
                            is_error: false,
                        },
                        Block::Text("Now summarise.".to_owned()),
                        Block::Image(ImageSource::Base64 {
                            media_type: "image/png".to_owned(),
                            data: "iVBORw0KGgo=".to_owned(),
                        }),
                        Block::Image(ImageSource::Url("https://example.test/a.png".to_owned())),
                    ],
                },
            ],
            tools: vec![
                ToolDefinition {
                    name: "Bash".to_owned(),
                    description: Some("Run a command".to_owned()),
                    input_schema: json!({"type": "object", "properties": {"command": {"type": "string"}}, "required": ["command"]}),
                },
                ToolDefinition {
                    name: "Read".to_owned(),
                    description: None,
                    input_schema: json!({"type": "object", "properties": {"file_path": {"type": "string"}}}),
                },
                ToolDefinition {
                    name: "Edit".to_owned(),
                    description: Some("Edit a file".to_owned()),
                    input_schema: json!({"type": "object"}),
                },
            ],
            tool_choice: Some(ToolChoice::Tool("Bash".to_owned())),
            parallel_tool_calls: Some(false),
            temperature: Some(0.5),
            top_p: Some(0.9),
            stop: vec!["END".to_owned(), "STOP".to_owned()],
            stream: true,
            user: Some("user_123".to_owned()),
        }
    }

    #[test]
    fn a_request_round_trips_through_the_anthropic_wire() {
        let request = full_request();
        let wire = encode_request(&request);
        let decoded = decode_request(&wire).expect("the codec reads what it wrote");
        assert_eq!(decoded, request);
    }

    #[test]
    fn the_request_claude_code_sends_decodes_including_a_block_system_and_string_content() {
        // The shape from the wire, not from the encoder: an array system, a
        // string user content, `metadata`, `stream`.
        let wire = br#"{
            "model": "claude-x",
            "max_tokens": 8192,
            "system": [{"type": "text", "text": "Be brief."}, {"type": "text", "text": "Be right."}],
            "messages": [{"role": "user", "content": "hi"}],
            "tools": [{"name": "Bash", "description": "run", "input_schema": {"type": "object"}}],
            "tool_choice": {"type": "auto"},
            "metadata": {"user_id": "u1"},
            "stream": true
        }"#;
        let request = decode_request(wire).expect("a Claude Code request decodes");
        assert_eq!(request.system.as_deref(), Some("Be brief.\n\nBe right."));
        assert_eq!(
            request.messages,
            vec![Message {
                role: Role::User,
                blocks: vec![Block::Text("hi".to_owned())]
            }]
        );
        assert_eq!(request.tool_choice, Some(ToolChoice::Auto));
        assert_eq!(request.parallel_tool_calls, None);
        assert_eq!(request.user.as_deref(), Some("u1"));
        assert!(request.stream);
    }

    #[test]
    fn every_refused_request_field_is_refused_by_its_name() {
        let base = |extra: &str| {
            format!(
                r#"{{"model": "m", "max_tokens": 1, "messages": [{{"role": "user", "content": "x"}}]{extra}}}"#
            )
        };
        let cases: Vec<(String, &str)> = vec![
            (
                base(r#", "thinking": {"type": "enabled", "budget_tokens": 1024}"#),
                "thinking",
            ),
            (base(r#", "top_k": 5"#), "top_k"),
            (base(r#", "service_tier": "auto""#), "service_tier"),
            (base(r#", "unknown_future_field": 1"#), "unknown_future_field"),
            (
                base(
                    r#", "system": [{"type": "text", "text": "s", "cache_control": {"type": "ephemeral"}}]"#,
                ),
                "system[0].cache_control",
            ),
            (
                r#"{"model": "m", "max_tokens": 1, "messages": [{"role": "user", "content": [{"type": "text", "text": "x", "cache_control": {"type": "ephemeral"}}]}]}"#.to_owned(),
                "messages[0].content[0].cache_control",
            ),
            (
                r#"{"model": "m", "max_tokens": 1, "messages": [{"role": "user", "content": [{"type": "text", "text": "x", "citations": []}]}]}"#.to_owned(),
                "messages[0].content[0].citations",
            ),
            (
                r#"{"model": "m", "max_tokens": 1, "messages": [{"role": "assistant", "content": [{"type": "thinking", "thinking": "hmm", "signature": "s"}]}]}"#.to_owned(),
                "messages[0].content[0].type",
            ),
            (
                r#"{"model": "m", "max_tokens": 1, "messages": [{"role": "user", "content": [{"type": "document", "source": {}}]}]}"#.to_owned(),
                "messages[0].content[0].type",
            ),
            (
                r#"{"model": "m", "max_tokens": 1, "messages": [], "tools": [{"type": "bash_20250124", "name": "bash"}]}"#.to_owned(),
                "tools[0].type",
            ),
            (
                r#"{"model": "m", "max_tokens": 1, "messages": [], "tools": [{"name": "t", "input_schema": {}, "cache_control": {"type": "ephemeral"}}]}"#.to_owned(),
                "tools[0].cache_control",
            ),
            (
                r#"{"model": "m", "max_tokens": 1, "messages": [], "tool_choice": {"type": "mystery"}}"#.to_owned(),
                "tool_choice.type",
            ),
            (
                r#"{"model": "m", "max_tokens": 1, "messages": [{"role": "user", "content": [{"type": "tool_result", "tool_use_id": "t", "content": [{"type": "image", "source": {"type": "url", "url": "u"}}]}]}]}"#.to_owned(),
                "messages[0].content[0].content[0]",
            ),
        ];
        for (wire, field) in cases {
            let refusal = decode_request(wire.as_bytes())
                .expect_err(&format!("{field} must be refused, in: {wire}"));
            assert_eq!(refusal.field, field, "{wire}");
            assert!(!refusal.reason.is_empty());
        }
    }

    #[test]
    fn the_cache_control_refusal_tells_the_user_the_switch() {
        assert!(reason("cache_control").contains("DISABLE_PROMPT_CACHING=1"));
    }

    #[test]
    fn a_response_round_trips_through_the_anthropic_wire() {
        let response = super::super::canonical::tests::tool_call_response();
        let wire = encode_response(&response);
        assert_eq!(
            decode_response(&wire).expect("the codec reads what it wrote"),
            response
        );

        // ... and one with a stop sequence and no cache reading.
        let stopped = Response {
            id: "msg_2".to_owned(),
            model: "m".to_owned(),
            blocks: vec![Block::Text("until END".to_owned())],
            stop_reason: StopReason::StopSequence,
            stop_sequence: Some("END".to_owned()),
            usage: Usage {
                input: 3,
                output: 2,
                cached: None,
            },
        };
        assert_eq!(
            decode_response(&encode_response(&stopped)).unwrap(),
            stopped
        );
    }

    #[test]
    fn a_stream_round_trips_event_for_event_through_the_anthropic_wire() {
        let response = super::super::canonical::tests::tool_call_response();
        let events = response.as_events();
        let mut encoder = EventEncoder;
        let mut wire = Vec::new();
        for event in &events {
            wire.extend(encoder.encode(event));
        }
        // A ping in the middle, as the real API sends, is ignored.
        wire.extend(stream::encode(Some("ping"), r#"{"type": "ping"}"#));

        let mut reader = SseReader::new(BufReader::new(&wire[..]));
        let mut decoder = EventDecoder { done: false };
        let mut decoded = Vec::new();
        while let Some(event) = reader.next_event().unwrap() {
            decoded.extend(decoder.feed(&event).expect("the codec reads what it wrote"));
        }
        assert_eq!(decoded, events);
        assert!(decoder.is_done());
    }

    #[test]
    fn a_thinking_delta_or_a_stream_error_is_a_named_refusal() {
        let mut decoder = EventDecoder { done: false };
        let thinking = SseEvent {
            event: Some("content_block_delta".to_owned()),
            data: r#"{"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"..."}}"#.to_owned(),
        };
        assert_eq!(decoder.feed(&thinking).unwrap_err().field, "delta.type");
        let error = SseEvent {
            event: Some("error".to_owned()),
            data: r#"{"type":"error","error":{"type":"overloaded_error","message":"Overloaded"}}"#
                .to_owned(),
        };
        let refusal = decoder.feed(&error).unwrap_err();
        assert_eq!(refusal.field, "error");
        assert!(refusal.reason.contains("Overloaded"));
    }

    #[test]
    fn a_stream_that_ends_before_message_stop_is_refused_at_finish() {
        let mut decoder = EventDecoder { done: false };
        assert_eq!(decoder.finish().unwrap_err().field, "message_stop");
    }
}
