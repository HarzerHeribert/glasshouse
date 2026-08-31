//! The OpenAI Responses codec: that wire's requests, responses and stream
//! events, into and out of [`super::canonical`].
//!
//! The same three categories as the two T1 codecs — carried, refused by
//! name, or ignored by name — and the same rule that an unknown key is a
//! refusal. Four decisions particular to this wire are worth reading before
//! the code:
//!
//! **`call_id` is the identity, and item ids are bookkeeping.** A Responses
//! function call carries two ids: `call_id`, which the caller's
//! `function_call_output` must repeat, and an item `id` (`fc_…`) that names
//! the output item itself. Only `call_id` is the tool-call identity, so it is
//! the string that crosses this codec verbatim in both directions — it *is*
//! Anthropic's `tool_use.id` — while item ids are ignored on decode and
//! minted on encode, exactly because nothing downstream may depend on them.
//!
//! **Server-side state is refused, not simulated.** `previous_response_id`,
//! `store: true`, background mode, stored prompts and item references all
//! ask the provider to hold conversation state between requests. A
//! translated upstream has no such store, and pretending otherwise would
//! fail on the *second* request, after the first had already misled the
//! client. Each is a named refusal — and the encoder always sends
//! `store: false`, because the Responses API stores responses by default and
//! the harness on the other side of a translated pair never asked for that.
//!
//! **An erroring tool result travels the same way as on OpenAI Chat.**
//! `function_call_output` has no error flag, so the flag rides as
//! [`TOOL_ERROR_MARKER`] on the output's first line — the identical
//! convention, deliberately, so the round trip through either OpenAI wire
//! restores `is_error` exactly.
//!
//! **A reasoning item that says nothing is skipped; one that says anything
//! is refused.** Responses upstreams emit `reasoning` output items even at
//! default settings, usually with an empty summary. An empty item carries no
//! information, so it is ignored by name; a summary, content, or encrypted
//! payload is model reasoning the canonical form cannot carry, and dropping
//! *that* silently is exactly what this directory never does.
//!
//! One canonical field has no home on this wire at all: `stop`. The
//! Responses API has no stop-sequence parameter, so this codec refuses a
//! request carrying one via [`Codec::refuse_unencodable`], before anything
//! is opened upstream, rather than letting the infallible encoder drop it.

use serde_json::{Map, Value, json};

use super::canonical::{
    Block, BlockStart, Delta, ImageSource, Message, Request, Response, Role, StopReason,
    StreamEvent, ToolChoice, ToolDefinition, Unsupported, Usage, json_kind, parse_tool_input,
};
use super::fields::{Fields, element};
use super::stream::{self, SseEvent};
use super::{Codec, StreamDecoder, StreamEncoder, TOOL_ERROR_MARKER};

pub(super) const PROTOCOL: &str = "openai-responses";

/// The one target this codec translates, version segment stripped. Codex
/// 0.149.1 was observed sending exactly `POST /responses` against a
/// path-less base URL — see `profile::ingress_targets`.
pub(super) const ENDPOINT: &str = "/responses";

/// The request fields and shapes this codec refuses, with the reason each
/// refusal carries.
pub(super) const REFUSED_FIELDS: &[(&str, &str)] = &[
    (
        "previous_response_id",
        "server-side conversation state is not available through a translated pair; resend the \
         whole conversation in `input`",
    ),
    (
        "store",
        "storing the response server-side cannot be provided by a translated upstream; send \
         `store: false`",
    ),
    (
        "background",
        "background mode is server-side state a translated upstream cannot provide",
    ),
    (
        "built-in tool",
        "a hosted tool type (web search, file search, computer use, code interpreter, image \
         generation, MCP, local shell) has no equivalent on the translated upstream; only \
         function tools are translated",
    ),
    (
        "reasoning",
        "reasoning configuration has no equivalent in this codec; turn it off for this pairing",
    ),
    (
        "reasoning item",
        "a reasoning item cannot be carried across a translated pair",
    ),
    (
        "item_reference",
        "an item reference points at server-side stored state, which a translated pair does not \
         have",
    ),
    (
        "include",
        "the extra response fields `include` asks for do not exist on the translated upstream",
    ),
    (
        "input_file",
        "a file input has no equivalent on the translated upstream; send the content as text or \
         an inline image",
    ),
    (
        "audio",
        "audio content has no equivalent on the translated upstream",
    ),
    (
        "refusal",
        "a refusal message part has no equivalent in this codec",
    ),
    (
        "strict",
        "strict schema adherence is not enforced by the translated upstream, and pretending it \
         was would silently change what the caller asked for",
    ),
    (
        "text.format",
        "a structured output format has no equivalent on the translated upstream",
    ),
    (
        "verbosity",
        "a verbosity setting has no equivalent on the translated upstream",
    ),
    (
        "truncation",
        "automatic truncation is server-side context management a translated upstream cannot \
         provide; use `truncation: \"disabled\"`",
    ),
    (
        "metadata",
        "stored response metadata has no home on a translated upstream",
    ),
    (
        "top_logprobs",
        "log probabilities have no equivalent in this codec",
    ),
    (
        "max_tool_calls",
        "capping tool calls per response has no equivalent in this codec",
    ),
    (
        "service_tier",
        "the translated upstream has no equivalent service tier",
    ),
    (
        "prompt",
        "a stored prompt template lives server-side and is not available through a translated \
         pair",
    ),
    (
        "prompt_cache_key",
        "prompt-cache routing has no equivalent on the translated upstream",
    ),
    (
        "stop_sequences",
        "the OpenAI Responses API has no stop sequences; remove them for this pairing",
    ),
];

/// Fields ignored by name: informational, never asked for by the caller,
/// and named here so that ignoring them is a recorded decision.
pub(super) const IGNORED_FIELDS: &[&str] = &[
    "object",
    "created_at",
    "input[].id",
    "input[].status",
    "output[].id",
    "output[].status",
    "output[].reasoning (empty summary)",
    "output_text.annotations",
    "output_text.logprobs",
    "input_image.detail",
    "usage.total_tokens",
    "usage.input_tokens_details.audio_tokens",
    "usage.output_tokens_details",
    "response document echo fields (instructions, tools, tool_choice, sampling parameters)",
    "stream event bookkeeping (sequence_number, item_id, output_index, content_index, \
     obfuscation)",
    "response.created/completed snapshots beyond id, model, status, incomplete_details and usage",
];

fn reason(field: &str) -> &'static str {
    REFUSED_FIELDS
        .iter()
        .find(|(name, _)| *name == field)
        .map(|(_, reason)| *reason)
        .expect("every refusal named in this file is listed in REFUSED_FIELDS")
}

pub(super) struct OpenAiResponses;

impl Codec for OpenAiResponses {
    fn protocol(&self) -> &'static str {
        PROTOCOL
    }

    fn endpoint(&self) -> &'static str {
        ENDPOINT
    }

    fn refuse_unencodable(&self, request: &Request) -> Result<(), Unsupported> {
        if request.stop.is_empty() {
            Ok(())
        } else {
            // Named in the one spelling that can reach this codec: the only
            // supported pair *into* openai-responses decodes Anthropic
            // Messages, whose field is `stop_sequences`.
            Err(Unsupported::new("stop_sequences", reason("stop_sequences")))
        }
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
        Box::new(EventDecoder::default())
    }

    fn stream_encoder(&self) -> Box<dyn StreamEncoder + Send> {
        Box::new(EventEncoder::default())
    }

    fn error_kind(&self, status: u16) -> &'static str {
        match status {
            400 | 413 => "invalid_request_error",
            401 => "authentication_error",
            403 => "permission_error",
            404 => "not_found_error",
            429 => "rate_limit_error",
            _ => "server_error",
        }
    }

    fn encode_error(&self, kind: &str, message: &str) -> Vec<u8> {
        json!({"error": {"message": message, "type": kind, "param": null, "code": null}})
            .to_string()
            .into_bytes()
    }

    fn encode_stream_error(&self, kind: &str, message: &str) -> Vec<u8> {
        stream::encode(
            Some("error"),
            &json!({"type": "error", "code": kind, "message": message, "param": null}).to_string(),
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

    let mut system_parts: Vec<String> = Vec::new();
    if let Some(instructions) = top.take_string("instructions")? {
        system_parts.push(instructions);
    }

    let mut messages: Vec<Message> = Vec::new();
    // Function-call outputs answer the assistant turn before them and open
    // the user turn after them; on the Anthropic side they are the first
    // blocks of that user turn, so they wait here until the next item says
    // which turn they belong to — the same shape as the OpenAI Chat codec's
    // tool messages.
    let mut pending_results: Vec<Block> = Vec::new();
    match top.take("input") {
        None | Some(Value::Null) => {}
        Some(Value::String(text)) => messages.push(Message {
            role: Role::User,
            blocks: vec![Block::Text(text)],
        }),
        Some(Value::Array(items)) => {
            for (index, item) in items.into_iter().enumerate() {
                decode_input_item(
                    item,
                    &element("input", index),
                    &mut system_parts,
                    &mut messages,
                    &mut pending_results,
                )?;
            }
        }
        Some(other) => {
            return Err(Unsupported::new(
                "input",
                format!(
                    "the input must be a string or an array of items, not {}",
                    json_kind(&other)
                ),
            ));
        }
    }
    flush_results(&mut pending_results, &mut messages);
    let system = if system_parts.is_empty() {
        None
    } else {
        Some(system_parts.join("\n\n"))
    };

    let mut tools = Vec::new();
    for (index, item) in top
        .take_array("tools")?
        .unwrap_or_default()
        .into_iter()
        .enumerate()
    {
        let mut tool = Fields::of(item, element("tools", index))?;
        let kind = tool.require_string("type")?;
        if kind != "function" {
            return Err(Unsupported::new(tool.at("type"), reason("built-in tool")));
        }
        if let Some(strict) = tool.take_bool("strict")?
            && strict
        {
            return Err(Unsupported::new(tool.at("strict"), reason("strict")));
        }
        let name = tool.require_string("name")?;
        let description = tool.take_string("description")?;
        let input_schema = match tool.take("parameters") {
            Some(schema @ Value::Object(_)) => schema,
            None | Some(Value::Null) => json!({"type": "object", "properties": {}}),
            Some(other) => {
                return Err(Unsupported::new(
                    tool.at("parameters"),
                    format!(
                        "a function's parameters must be a JSON object, not {}",
                        json_kind(&other)
                    ),
                ));
            }
        };
        tool.finish()?;
        tools.push(ToolDefinition {
            name,
            description,
            input_schema,
        });
    }

    let tool_choice = match top.take("tool_choice") {
        None | Some(Value::Null) => None,
        Some(Value::String(choice)) => Some(match choice.as_str() {
            "none" => ToolChoice::None,
            "auto" => ToolChoice::Auto,
            "required" => ToolChoice::Any,
            other => {
                return Err(Unsupported::new(
                    "tool_choice",
                    format!("the tool choice `{other}` is not one this codec knows"),
                ));
            }
        }),
        Some(object @ Value::Object(_)) => {
            let mut choice = Fields::of(object, "tool_choice")?;
            let kind = choice.require_string("type")?;
            if kind != "function" {
                return Err(Unsupported::new(choice.at("type"), reason("built-in tool")));
            }
            // Flat, unlike OpenAI Chat's nested `function` object.
            let name = choice.require_string("name")?;
            choice.finish()?;
            Some(ToolChoice::Tool(name))
        }
        Some(other) => {
            return Err(Unsupported::new(
                "tool_choice",
                format!(
                    "a tool choice must be a string or an object, not {}",
                    json_kind(&other)
                ),
            ));
        }
    };

    let parallel_tool_calls = top.take_bool("parallel_tool_calls")?;
    let max_tokens = top.take_u64("max_output_tokens")?;
    let temperature = top.take_f64("temperature")?;
    let top_p = top.take_f64("top_p")?;
    let stream = top.take_bool("stream")?.unwrap_or(false);

    // Two spellings of the same end-user identifier: `user` (the older one)
    // and `safety_identifier` (its successor). Either is carried; both at
    // once must agree, or one of them would be dropped silently.
    let user = match (
        top.take_string("user")?,
        top.take_string("safety_identifier")?,
    ) {
        (Some(user), Some(safety)) if user != safety => {
            return Err(Unsupported::new(
                "safety_identifier",
                "user and safety_identifier disagree",
            ));
        }
        (user, safety) => user.or(safety),
    };

    // Value-gated refusals: a value that asks for exactly what the pair does
    // anyway is carried, and a value the pair cannot honour is refused.
    if top.take_bool("store")?.unwrap_or(false) {
        return Err(Unsupported::new("store", reason("store")));
    }
    if top.take_bool("background")?.unwrap_or(false) {
        return Err(Unsupported::new("background", reason("background")));
    }
    match top.take_string("truncation")?.as_deref() {
        None | Some("disabled") => {}
        Some("auto") => return Err(Unsupported::new("truncation", reason("truncation"))),
        Some(other) => {
            return Err(Unsupported::new(
                "truncation",
                format!("the truncation mode `{other}` is not one this codec knows"),
            ));
        }
    }
    if let Some(include) = top.take_array("include")?
        && !include.is_empty()
    {
        return Err(Unsupported::new("include", reason("include")));
    }
    match top.take("metadata") {
        None | Some(Value::Null) => {}
        Some(Value::Object(map)) if map.is_empty() => {}
        Some(Value::Object(_)) => return Err(Unsupported::new("metadata", reason("metadata"))),
        Some(other) => {
            return Err(Unsupported::new(
                "metadata",
                format!("metadata must be an object, not {}", json_kind(&other)),
            ));
        }
    }
    if let Some(mut text) = top.take_object("text")? {
        if let Some(mut format) = text.take_object("format")? {
            let kind = format.require_string("type")?;
            if kind != "text" {
                return Err(Unsupported::new(format.at("type"), reason("text.format")));
            }
            format.finish()?;
        }
        text.refuse_if_present("verbosity", reason("verbosity"))?;
        text.finish()?;
    }
    for field in [
        "previous_response_id",
        "prompt",
        "prompt_cache_key",
        "reasoning",
        "service_tier",
        "top_logprobs",
        "max_tool_calls",
    ] {
        top.refuse_if_present(field, reason(field))?;
    }
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
        // The Responses API has no stop-sequence parameter, so a request on
        // this wire can never carry one in.
        stop: Vec::new(),
        stream,
        user,
    })
}

fn decode_input_item(
    value: Value,
    path: &str,
    system_parts: &mut Vec<String>,
    messages: &mut Vec<Message>,
    pending_results: &mut Vec<Block>,
) -> Result<(), Unsupported> {
    let mut item = Fields::of(value, path)?;
    // The API accepts both the typed item and the bare `{role, content}`
    // shorthand; a missing type means a message.
    let kind = item
        .take_string("type")?
        .unwrap_or_else(|| "message".to_owned());
    match kind.as_str() {
        "message" => {
            item.ignore("id");
            item.ignore("status");
            let role = item.require_string("role")?;
            match role.as_str() {
                "system" | "developer" => {
                    flush_results(pending_results, messages);
                    system_parts.push(text_content(&mut item)?);
                }
                "user" => {
                    let mut blocks = std::mem::take(pending_results);
                    blocks.extend(user_content(&mut item)?);
                    messages.push(Message {
                        role: Role::User,
                        blocks,
                    });
                }
                "assistant" => {
                    flush_results(pending_results, messages);
                    let blocks = assistant_content(&mut item)?;
                    messages.push(Message {
                        role: Role::Assistant,
                        blocks,
                    });
                }
                other => {
                    return Err(Unsupported::new(
                        item.at("role"),
                        format!("the message role `{other}` is not one this codec carries"),
                    ));
                }
            }
            item.finish()?;
        }
        "function_call" => {
            // A call after results is a new round: the results close the
            // previous one as their own user turn first.
            flush_results(pending_results, messages);
            item.ignore("id");
            item.ignore("status");
            let id = item.require_string("call_id")?;
            let name = item.require_string("name")?;
            let arguments = item.take_string("arguments")?.unwrap_or_default();
            let arguments_path = item.at("arguments");
            let input = parse_tool_input(&arguments)
                .map_err(|why| Unsupported::new(arguments_path, why))?;
            item.finish()?;
            let block = Block::ToolUse { id, name, input };
            // A function call is part of the model turn it follows: Codex
            // replays the assistant's message item and its calls as separate
            // input items, and they are one canonical assistant turn.
            match messages.last_mut() {
                Some(message) if message.role == Role::Assistant => message.blocks.push(block),
                _ => messages.push(Message {
                    role: Role::Assistant,
                    blocks: vec![block],
                }),
            }
        }
        "function_call_output" => {
            item.ignore("id");
            item.ignore("status");
            let tool_use_id = item.require_string("call_id")?;
            let output = match item.take("output") {
                None | Some(Value::Null) => String::new(),
                Some(Value::String(text)) => text,
                Some(other) => {
                    return Err(Unsupported::new(
                        item.at("output"),
                        format!(
                            "a function call output must be a string, not {}",
                            json_kind(&other)
                        ),
                    ));
                }
            };
            item.finish()?;
            let (content, is_error) = match output.strip_prefix(TOOL_ERROR_MARKER) {
                Some(rest) => (rest.strip_prefix('\n').unwrap_or(rest).to_owned(), true),
                None => (output, false),
            };
            pending_results.push(Block::ToolResult {
                tool_use_id,
                content,
                is_error,
            });
        }
        "reasoning" => {
            return Err(Unsupported::new(item.at("type"), reason("reasoning item")));
        }
        "item_reference" => {
            return Err(Unsupported::new(item.at("type"), reason("item_reference")));
        }
        "web_search_call"
        | "file_search_call"
        | "computer_call"
        | "computer_call_output"
        | "code_interpreter_call"
        | "image_generation_call"
        | "local_shell_call"
        | "local_shell_call_output"
        | "mcp_call"
        | "mcp_list_tools"
        | "mcp_approval_request"
        | "mcp_approval_response" => {
            return Err(Unsupported::new(item.at("type"), reason("built-in tool")));
        }
        other => {
            return Err(Unsupported::new(
                item.at("type"),
                format!("the input item type `{other}` is not one this codec carries"),
            ));
        }
    }
    Ok(())
}

/// Function-call outputs with no user turn after them still make a user turn.
fn flush_results(pending: &mut Vec<Block>, messages: &mut Vec<Message>) {
    if !pending.is_empty() {
        messages.push(Message {
            role: Role::User,
            blocks: std::mem::take(pending),
        });
    }
}

/// A system or developer message's text: a string, or `input_text` parts
/// joined with newlines.
fn text_content(message: &mut Fields) -> Result<String, Unsupported> {
    match message.take("content") {
        Some(Value::String(text)) => Ok(text),
        Some(Value::Array(parts)) => {
            let path = message.at("content");
            let mut texts = Vec::with_capacity(parts.len());
            for (index, part) in parts.into_iter().enumerate() {
                let mut part = Fields::of(part, element(&path, index))?;
                let kind = part.require_string("type")?;
                if kind != "input_text" {
                    return Err(Unsupported::new(
                        part.at("type"),
                        format!("this content must be input_text, not `{kind}`"),
                    ));
                }
                texts.push(part.require_string("text")?);
                part.finish()?;
            }
            Ok(texts.join("\n"))
        }
        Some(other) => Err(Unsupported::new(
            message.at("content"),
            format!(
                "content must be a string or text parts, not {}",
                json_kind(&other)
            ),
        )),
        None => Err(Unsupported::new(
            message.at("content"),
            "content is required",
        )),
    }
}

fn user_content(message: &mut Fields) -> Result<Vec<Block>, Unsupported> {
    match message.take("content") {
        Some(Value::String(text)) => Ok(vec![Block::Text(text)]),
        Some(Value::Array(parts)) => {
            let path = message.at("content");
            let mut blocks = Vec::with_capacity(parts.len());
            for (index, part) in parts.into_iter().enumerate() {
                let mut part = Fields::of(part, element(&path, index))?;
                match part.require_string("type")?.as_str() {
                    "input_text" => blocks.push(Block::Text(part.require_string("text")?)),
                    "input_image" => {
                        part.ignore("detail");
                        part.refuse_if_present("file_id", reason("input_file"))?;
                        let url = part.require_string("image_url")?;
                        blocks.push(Block::Image(image_source(&url).ok_or_else(|| {
                            Unsupported::new(
                                part.at("image_url"),
                                "an inline image must be a base64 data URL with a media type",
                            )
                        })?));
                    }
                    "input_file" => {
                        return Err(Unsupported::new(part.at("type"), reason("input_file")));
                    }
                    "input_audio" => {
                        return Err(Unsupported::new(part.at("type"), reason("audio")));
                    }
                    other => {
                        return Err(Unsupported::new(
                            part.at("type"),
                            format!(
                                "a user content part must be input_text or input_image, not \
                                 `{other}`"
                            ),
                        ));
                    }
                }
                part.finish()?;
            }
            Ok(blocks)
        }
        Some(other) => Err(Unsupported::new(
            message.at("content"),
            format!(
                "user content must be a string or parts, not {}",
                json_kind(&other)
            ),
        )),
        None => Err(Unsupported::new(
            message.at("content"),
            "a user message needs content",
        )),
    }
}

fn assistant_content(message: &mut Fields) -> Result<Vec<Block>, Unsupported> {
    match message.take("content") {
        None | Some(Value::Null) => Ok(Vec::new()),
        Some(Value::String(text)) => Ok(if text.is_empty() {
            Vec::new()
        } else {
            vec![Block::Text(text)]
        }),
        Some(Value::Array(parts)) => {
            let path = message.at("content");
            let mut blocks = Vec::with_capacity(parts.len());
            for (index, part) in parts.into_iter().enumerate() {
                let mut part = Fields::of(part, element(&path, index))?;
                match part.require_string("type")?.as_str() {
                    "output_text" => {
                        part.ignore("annotations");
                        part.ignore("logprobs");
                        blocks.push(Block::Text(part.require_string("text")?));
                    }
                    "refusal" => {
                        return Err(Unsupported::new(part.at("type"), reason("refusal")));
                    }
                    other => {
                        return Err(Unsupported::new(
                            part.at("type"),
                            format!("an assistant content part must be output_text, not `{other}`"),
                        ));
                    }
                }
                part.finish()?;
            }
            Ok(blocks)
        }
        Some(other) => Err(Unsupported::new(
            message.at("content"),
            format!(
                "assistant content must be a string, null or parts, not {}",
                json_kind(&other)
            ),
        )),
    }
}

/// An `input_image` URL as a source: a `data:` URL is inline base64,
/// anything else is fetched by URL. The same reading as the OpenAI Chat
/// codec's, which cannot be shared because that file is settled.
fn image_source(url: &str) -> Option<ImageSource> {
    let Some(rest) = url.strip_prefix("data:") else {
        return Some(ImageSource::Url(url.to_owned()));
    };
    let (media_type, data) = rest.split_once(";base64,")?;
    if media_type.is_empty() {
        return None;
    }
    Some(ImageSource::Base64 {
        media_type: media_type.to_owned(),
        data: data.to_owned(),
    })
}

fn image_url(source: &ImageSource) -> String {
    match source {
        ImageSource::Base64 { media_type, data } => format!("data:{media_type};base64,{data}"),
        ImageSource::Url(url) => url.clone(),
    }
}

pub(super) fn encode_request(request: &Request) -> Vec<u8> {
    // `refuse_unencodable` keeps a request with stop sequences out of here;
    // the Responses API has no parameter for them, and dropping one silently
    // is the one thing no codec in this directory does.
    debug_assert!(
        request.stop.is_empty(),
        "guarded by Codec::refuse_unencodable"
    );
    let mut input = Vec::new();
    for message in &request.messages {
        match message.role {
            Role::User => {
                let mut parts = Vec::new();
                for block in &message.blocks {
                    match block {
                        Block::ToolResult {
                            tool_use_id,
                            content,
                            is_error,
                        } => {
                            // Results first, as function_call_output items;
                            // the user's own blocks below follow them.
                            let output = if *is_error {
                                format!("{TOOL_ERROR_MARKER}\n{content}")
                            } else {
                                content.clone()
                            };
                            input.push(json!({
                                "type": "function_call_output",
                                "call_id": tool_use_id,
                                "output": output,
                            }));
                        }
                        Block::Text(text) => {
                            parts.push(json!({"type": "input_text", "text": text}));
                        }
                        Block::Image(source) => parts.push(json!({
                            "type": "input_image",
                            "image_url": image_url(source),
                        })),
                        // A tool use in a user turn is not a shape either
                        // wire produces; carried as text so nothing is lost
                        // silently, and never reached from a decoder.
                        Block::ToolUse { id, name, input } => parts.push(json!({
                            "type": "input_text",
                            "text": format!("[tool_use {id} {name}] {input}"),
                        })),
                    }
                }
                if !parts.is_empty() {
                    input.push(json!({
                        "type": "message",
                        "role": "user",
                        "content": Value::Array(parts),
                    }));
                }
            }
            Role::Assistant => {
                let mut parts = Vec::new();
                let mut calls = Vec::new();
                for block in &message.blocks {
                    match block {
                        Block::Text(text) => {
                            parts.push(json!({"type": "output_text", "text": text}));
                        }
                        Block::ToolUse { id, name, input } => calls.push(json!({
                            "type": "function_call",
                            "call_id": id,
                            "name": name,
                            "arguments": input.to_string(),
                        })),
                        Block::Image(source) => parts.push(json!({
                            "type": "output_text",
                            "text": match source {
                                ImageSource::Url(url) => url.clone(),
                                ImageSource::Base64 { .. } => "[image]".to_owned(),
                            },
                        })),
                        Block::ToolResult { content, .. } => {
                            parts.push(json!({"type": "output_text", "text": content}));
                        }
                    }
                }
                if !parts.is_empty() {
                    input.push(json!({
                        "type": "message",
                        "role": "assistant",
                        "content": Value::Array(parts),
                    }));
                }
                input.extend(calls);
            }
        }
    }

    let mut document = Map::new();
    document.insert("model".to_owned(), json!(request.model));
    if let Some(system) = &request.system {
        document.insert("instructions".to_owned(), json!(system));
    }
    document.insert("input".to_owned(), Value::Array(input));
    if !request.tools.is_empty() {
        document.insert(
            "tools".to_owned(),
            Value::Array(
                request
                    .tools
                    .iter()
                    .map(|tool| {
                        let mut entry = Map::new();
                        entry.insert("type".to_owned(), json!("function"));
                        entry.insert("name".to_owned(), json!(tool.name));
                        if let Some(description) = &tool.description {
                            entry.insert("description".to_owned(), json!(description));
                        }
                        entry.insert("parameters".to_owned(), tool.input_schema.clone());
                        // Never omitted: the Responses API defaults `strict`
                        // to true, and a schema that crossed from the other
                        // wire was not written to satisfy strict mode.
                        entry.insert("strict".to_owned(), json!(false));
                        Value::Object(entry)
                    })
                    .collect(),
            ),
        );
    }
    if let Some(choice) = &request.tool_choice {
        document.insert(
            "tool_choice".to_owned(),
            match choice {
                ToolChoice::Auto => json!("auto"),
                ToolChoice::Any => json!("required"),
                ToolChoice::None => json!("none"),
                ToolChoice::Tool(name) => json!({"type": "function", "name": name}),
            },
        );
    }
    if let Some(parallel) = request.parallel_tool_calls {
        document.insert("parallel_tool_calls".to_owned(), json!(parallel));
    }
    if let Some(max_tokens) = request.max_tokens {
        document.insert("max_output_tokens".to_owned(), json!(max_tokens));
    }
    if let Some(temperature) = request.temperature {
        document.insert("temperature".to_owned(), json!(temperature));
    }
    if let Some(top_p) = request.top_p {
        document.insert("top_p".to_owned(), json!(top_p));
    }
    if request.stream {
        document.insert("stream".to_owned(), json!(true));
    }
    // Never left to the provider's default, which is to store: the harness
    // on the other side of a translated pair never asked the provider to
    // keep its conversation server-side.
    document.insert("store".to_owned(), json!(false));
    if let Some(user) = &request.user {
        document.insert("user".to_owned(), json!(user));
    }
    Value::Object(document).to_string().into_bytes()
}

// --- responses ----------------------------------------------------------------

/// The response-document fields this codec ignores by name: each one echoes
/// the request or carries provider bookkeeping the harness never asked for.
const RESPONSE_ECHO_FIELDS: &[&str] = &[
    "created_at",
    "background",
    "billing",
    "instructions",
    "max_output_tokens",
    "max_tool_calls",
    "metadata",
    "parallel_tool_calls",
    "previous_response_id",
    "prompt_cache_key",
    "reasoning",
    "safety_identifier",
    "service_tier",
    "store",
    "temperature",
    "text",
    "tool_choice",
    "tools",
    "top_logprobs",
    "top_p",
    "truncation",
    "user",
];

pub(super) fn decode_response(body: &[u8]) -> Result<Response, Unsupported> {
    let value: Value = serde_json::from_slice(body)
        .map_err(|_| Unsupported::new("body", "the response body is not a JSON document"))?;
    let mut top = Fields::of(value, "")?;
    let id = top.require_string("id")?;
    if let Some(object) = top.take_string("object")?
        && object != "response"
    {
        return Err(Unsupported::new(
            "object",
            format!("a response document must be a response, not `{object}`"),
        ));
    }
    // The error first, so a failed response is refused with the provider's
    // own message rather than with whatever field happened to be read next.
    match top.take("error") {
        None | Some(Value::Null) => {}
        Some(error) => {
            let message = error
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned();
            return Err(Unsupported::new(
                "error",
                format!("the provider reported an error: {message}"),
            ));
        }
    }
    let status = top.require_string("status")?;
    let incomplete_reason = match top.take_object("incomplete_details")? {
        None => None,
        Some(mut details) => {
            let reason = details.take_string("reason")?;
            details.finish()?;
            reason
        }
    };
    let model = top.take_string("model")?.unwrap_or_default();
    let mut blocks = Vec::new();
    for (index, item) in top
        .take_array("output")?
        .unwrap_or_default()
        .into_iter()
        .enumerate()
    {
        decode_output_item(item, &element("output", index), &mut blocks)?;
    }
    let usage = match top.take_object("usage")? {
        Some(usage) => decode_usage(usage)?,
        None => Usage::default(),
    };
    for ignored in RESPONSE_ECHO_FIELDS {
        top.ignore(ignored);
    }
    top.finish()?;
    let stop_reason = stop_reason_of(&status, incomplete_reason.as_deref(), &blocks)?;
    Ok(Response {
        id,
        model,
        blocks,
        stop_reason,
        // The Responses wire cannot say which stop sequence matched — it has
        // no stop sequences at all.
        stop_sequence: None,
        usage,
    })
}

/// The stop reason a Responses document states, from its status and its
/// blocks. The wire has no first-class tool-use stop: a response that ends
/// on function calls is simply `completed`, and the calls are the signal.
fn stop_reason_of(
    status: &str,
    incomplete_reason: Option<&str>,
    blocks: &[Block],
) -> Result<StopReason, Unsupported> {
    match status {
        "completed" => Ok(
            if blocks
                .iter()
                .any(|block| matches!(block, Block::ToolUse { .. }))
            {
                StopReason::ToolUse
            } else {
                StopReason::EndTurn
            },
        ),
        "incomplete" => match incomplete_reason {
            Some("max_output_tokens") => Ok(StopReason::MaxTokens),
            Some("content_filter") => Ok(StopReason::Refusal),
            Some(other) => Err(Unsupported::new(
                "incomplete_details.reason",
                format!("the incomplete reason `{other}` is not one this codec knows"),
            )),
            None => Err(Unsupported::new(
                "incomplete_details.reason",
                "an incomplete response must say why it stopped",
            )),
        },
        other => Err(Unsupported::new(
            "status",
            format!("a complete response must be completed or incomplete, not `{other}`"),
        )),
    }
}

fn decode_output_item(
    value: Value,
    path: &str,
    blocks: &mut Vec<Block>,
) -> Result<(), Unsupported> {
    let mut item = Fields::of(value, path)?;
    let kind = item.require_string("type")?;
    match kind.as_str() {
        "message" => {
            item.ignore("id");
            item.ignore("status");
            let role = item.require_string("role")?;
            if role != "assistant" {
                return Err(Unsupported::new(
                    item.at("role"),
                    format!("an output message is from the assistant, not `{role}`"),
                ));
            }
            let content_path = item.at("content");
            for (index, part) in item
                .take_array("content")?
                .unwrap_or_default()
                .into_iter()
                .enumerate()
            {
                let mut part = Fields::of(part, element(&content_path, index))?;
                match part.require_string("type")?.as_str() {
                    "output_text" => {
                        part.ignore("annotations");
                        part.ignore("logprobs");
                        blocks.push(Block::Text(part.require_string("text")?));
                    }
                    "refusal" => {
                        return Err(Unsupported::new(part.at("type"), reason("refusal")));
                    }
                    other => {
                        return Err(Unsupported::new(
                            part.at("type"),
                            format!("an output content part must be output_text, not `{other}`"),
                        ));
                    }
                }
                part.finish()?;
            }
            item.finish()?;
        }
        "function_call" => {
            item.ignore("id");
            item.ignore("status");
            let id = item.require_string("call_id")?;
            let name = item.require_string("name")?;
            let arguments = item.take_string("arguments")?.unwrap_or_default();
            let arguments_path = item.at("arguments");
            let input = parse_tool_input(&arguments)
                .map_err(|why| Unsupported::new(arguments_path, why))?;
            item.finish()?;
            blocks.push(Block::ToolUse { id, name, input });
        }
        "reasoning" => {
            // See the module doc: an empty reasoning item carries no
            // information and is skipped by name; one carrying anything is
            // model reasoning the canonical form cannot hold.
            let refusal = Unsupported::new(item.at("type"), reason("reasoning item"));
            item.ignore("id");
            item.ignore("status");
            let summary_empty = match item.take("summary") {
                None | Some(Value::Null) => true,
                Some(Value::Array(summary)) => summary.is_empty(),
                Some(_) => false,
            };
            let content_empty = match item.take("content") {
                None | Some(Value::Null) => true,
                Some(Value::Array(content)) => content.is_empty(),
                Some(_) => false,
            };
            let no_encrypted = matches!(item.take("encrypted_content"), None | Some(Value::Null));
            item.finish()?;
            if !(summary_empty && content_empty && no_encrypted) {
                return Err(refusal);
            }
        }
        "web_search_call"
        | "file_search_call"
        | "computer_call"
        | "code_interpreter_call"
        | "image_generation_call"
        | "local_shell_call"
        | "mcp_call"
        | "mcp_list_tools"
        | "mcp_approval_request" => {
            return Err(Unsupported::new(item.at("type"), reason("built-in tool")));
        }
        other => {
            return Err(Unsupported::new(
                item.at("type"),
                format!("the output item type `{other}` is not one this codec carries"),
            ));
        }
    }
    Ok(())
}

fn decode_usage(mut usage: Fields) -> Result<Usage, Unsupported> {
    let input_total = usage.take_u64("input_tokens")?.unwrap_or(0);
    let output = usage.take_u64("output_tokens")?.unwrap_or(0);
    let cached = match usage.take_object("input_tokens_details")? {
        Some(mut details) => {
            let cached = details.take_u64("cached_tokens")?;
            details.ignore("audio_tokens");
            details.finish()?;
            cached
        }
        None => None,
    };
    usage.ignore("output_tokens_details");
    usage.ignore("total_tokens");
    usage.finish()?;
    // `input_tokens` includes the cached ones; the form's `input` does not.
    Ok(Usage {
        input: input_total.saturating_sub(cached.unwrap_or(0)),
        output,
        cached,
    })
}

fn usage_json(usage: &Usage) -> Value {
    let input = usage.input + usage.cached.unwrap_or(0);
    let mut entry = Map::new();
    entry.insert("input_tokens".to_owned(), json!(input));
    if let Some(cached) = usage.cached {
        entry.insert(
            "input_tokens_details".to_owned(),
            json!({"cached_tokens": cached}),
        );
    }
    entry.insert("output_tokens".to_owned(), json!(usage.output));
    entry.insert("total_tokens".to_owned(), json!(input + usage.output));
    Value::Object(entry)
}

/// The status and `incomplete_details` a canonical stop reason becomes.
///
/// `StopSequence` is `completed`: the wire has no stop sequences, so it
/// cannot say which one matched — and no request this codec decodes can set
/// one, so through a translated pair the case never arises.
fn status_of(stop_reason: StopReason) -> (&'static str, Value) {
    match stop_reason {
        StopReason::EndTurn | StopReason::ToolUse | StopReason::StopSequence => {
            ("completed", Value::Null)
        }
        StopReason::MaxTokens => ("incomplete", json!({"reason": "max_output_tokens"})),
        StopReason::Refusal => ("incomplete", json!({"reason": "content_filter"})),
    }
}

/// The complete output items a response's blocks become: one message item
/// carrying every text part, then one `function_call` item per tool use,
/// with `call_id` verbatim and item ids minted — see the module doc.
fn output_items(blocks: &[Block]) -> Vec<Value> {
    let mut texts = Vec::new();
    let mut calls = Vec::new();
    for block in blocks {
        match block {
            Block::Text(text) => {
                texts.push(json!({"type": "output_text", "text": text, "annotations": []}));
            }
            Block::ToolUse { id, name, input } => calls.push(json!({
                "type": "function_call",
                "id": format!("fc_{}", calls.len()),
                "call_id": id,
                "name": name,
                "arguments": input.to_string(),
                "status": "completed",
            })),
            // A response never carries these — `decode_response` on either
            // codec cannot produce them.
            Block::Image(_) | Block::ToolResult { .. } => {}
        }
    }
    let mut output = Vec::new();
    if !texts.is_empty() {
        output.push(json!({
            "type": "message",
            "id": "msg_0",
            "status": "completed",
            "role": "assistant",
            "content": texts,
        }));
    }
    output.extend(calls);
    output
}

pub(super) fn encode_response(response: &Response) -> Vec<u8> {
    let (status, incomplete_details) = status_of(response.stop_reason);
    json!({
        "id": response.id,
        "object": "response",
        "created_at": 0,
        "status": status,
        "incomplete_details": incomplete_details,
        "error": null,
        "model": response.model,
        "output": output_items(&response.blocks),
        "usage": usage_json(&response.usage),
    })
    .to_string()
    .into_bytes()
}

// --- streams ------------------------------------------------------------------

/// Which kind of canonical block is open.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OpenKind {
    Text,
    Tool,
}

/// Turns Responses stream events into the canonical event order.
///
/// Blocks are strictly sequential on this wire — a content part opens,
/// receives deltas and closes before the next item — so the decoder keeps
/// one open block and its canonical index. `response.completed` carries the
/// final usage and closes the message; nothing before it says the provider
/// has finished speaking.
#[derive(Default)]
struct EventDecoder {
    started: bool,
    open: Option<(usize, OpenKind)>,
    next_index: usize,
    saw_tool_call: bool,
    done: bool,
}

impl EventDecoder {
    fn close_open(&mut self, events: &mut Vec<StreamEvent>) {
        if let Some((index, _)) = self.open.take() {
            events.push(StreamEvent::BlockStop { index });
        }
    }

    fn open_block(&mut self, block: BlockStart, events: &mut Vec<StreamEvent>) -> usize {
        self.close_open(events);
        let index = self.next_index;
        self.next_index += 1;
        events.push(StreamEvent::BlockStart { index, block });
        index
    }
}

/// Read the identity out of a lifecycle event's response snapshot,
/// leniently: the snapshot is the whole response document echoed at every
/// lifecycle event, and refusing its unread echo fields would refuse every
/// real stream. The deliberate ignore is one [`IGNORED_FIELDS`] row; the
/// usage object itself is still read strictly.
fn snapshot_identity(snapshot: Option<&Value>) -> Result<(String, String, Usage), Unsupported> {
    let id = snapshot
        .and_then(|response| response.get("id"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    let model = snapshot
        .and_then(|response| response.get("model"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    Ok((id, model, snapshot_usage(snapshot)?))
}

fn snapshot_usage(snapshot: Option<&Value>) -> Result<Usage, Unsupported> {
    match snapshot.and_then(|response| response.get("usage")) {
        None | Some(Value::Null) => Ok(Usage::default()),
        Some(usage) => decode_usage(Fields::of(usage.clone(), "response.usage")?),
    }
}

impl StreamDecoder for EventDecoder {
    fn feed(&mut self, event: &SseEvent) -> Result<Vec<StreamEvent>, Unsupported> {
        if self.done {
            return Ok(Vec::new());
        }
        // The real API ends after `response.completed`; some compatible
        // providers add OpenAI Chat's `[DONE]` sentinel after it.
        if event.data.trim() == "[DONE]" {
            return Ok(Vec::new());
        }
        let value: Value = serde_json::from_str(&event.data)
            .map_err(|_| Unsupported::new("event", "a stream event was not a JSON document"))?;
        let mut top = Fields::of(value, "")?;
        let kind = top.require_string("type")?;
        top.ignore("sequence_number");
        let mut events = Vec::new();
        match kind.as_str() {
            "response.created" | "response.in_progress" | "response.queued" => {
                let snapshot = top.take("response");
                if kind == "response.created" && !self.started {
                    self.started = true;
                    let (id, model, usage) = snapshot_identity(snapshot.as_ref())?;
                    events.push(StreamEvent::MessageStart { id, model, usage });
                }
            }
            "response.output_item.added" => {
                top.ignore("output_index");
                let mut item = top.take_object("item")?.ok_or_else(|| {
                    Unsupported::new("item", "response.output_item.added has no item")
                })?;
                let item_kind = item.require_string("type")?;
                match item_kind.as_str() {
                    // The message item's content parts drive the blocks.
                    "message" => {
                        item.ignore("id");
                        item.ignore("status");
                        item.ignore("role");
                        item.ignore("content");
                    }
                    "function_call" => {
                        let id = item.require_string("call_id")?;
                        let name = item.take_string("name")?.unwrap_or_default();
                        let arguments = item.take_string("arguments")?.unwrap_or_default();
                        item.ignore("id");
                        item.ignore("status");
                        self.saw_tool_call = true;
                        let index = self.open_block(BlockStart::ToolUse { id, name }, &mut events);
                        self.open = Some((index, OpenKind::Tool));
                        if !arguments.is_empty() {
                            events.push(StreamEvent::BlockDelta {
                                index,
                                delta: Delta::InputJson(arguments),
                            });
                        }
                    }
                    // Whether a reasoning item stays ignorable is decided at
                    // its `done` event, once its summary has a value.
                    "reasoning" => {
                        item.ignore("id");
                        item.ignore("status");
                        item.ignore("summary");
                        item.ignore("content");
                        item.ignore("encrypted_content");
                    }
                    "web_search_call"
                    | "file_search_call"
                    | "computer_call"
                    | "code_interpreter_call"
                    | "image_generation_call"
                    | "local_shell_call"
                    | "mcp_call"
                    | "mcp_list_tools"
                    | "mcp_approval_request" => {
                        return Err(Unsupported::new(item.at("type"), reason("built-in tool")));
                    }
                    other => {
                        return Err(Unsupported::new(
                            item.at("type"),
                            format!(
                                "the streamed item type `{other}` is not one this codec \
                                     carries"
                            ),
                        ));
                    }
                }
                item.finish()?;
            }
            "response.content_part.added" => {
                top.ignore("item_id");
                top.ignore("output_index");
                top.ignore("content_index");
                let mut part = top.take_object("part")?.ok_or_else(|| {
                    Unsupported::new("part", "response.content_part.added has no part")
                })?;
                match part.require_string("type")?.as_str() {
                    "output_text" => {
                        part.ignore("text");
                        part.ignore("annotations");
                        part.ignore("logprobs");
                        let index = self.open_block(BlockStart::Text, &mut events);
                        self.open = Some((index, OpenKind::Text));
                    }
                    "refusal" => {
                        return Err(Unsupported::new(part.at("type"), reason("refusal")));
                    }
                    "reasoning_text" => {
                        return Err(Unsupported::new(part.at("type"), reason("reasoning item")));
                    }
                    other => {
                        return Err(Unsupported::new(
                            part.at("type"),
                            format!(
                                "the streamed part type `{other}` is not one this codec \
                                     carries"
                            ),
                        ));
                    }
                }
                part.finish()?;
            }
            "response.output_text.delta" => {
                top.ignore("item_id");
                top.ignore("output_index");
                top.ignore("content_index");
                top.ignore("logprobs");
                top.ignore("obfuscation");
                let delta = top.require_string("delta")?;
                let Some((index, OpenKind::Text)) = self.open else {
                    return Err(Unsupported::new(
                        "delta",
                        "a text delta arrived with no text block open",
                    ));
                };
                events.push(StreamEvent::BlockDelta {
                    index,
                    delta: Delta::Text(delta),
                });
            }
            // The full-text echo of the deltas that already streamed.
            "response.output_text.done" => {
                top.ignore("item_id");
                top.ignore("output_index");
                top.ignore("content_index");
                top.ignore("logprobs");
                top.ignore("text");
            }
            "response.content_part.done" => {
                top.ignore("item_id");
                top.ignore("output_index");
                top.ignore("content_index");
                top.ignore("part");
                let Some((index, OpenKind::Text)) = self.open.take() else {
                    return Err(Unsupported::new(
                        "part",
                        "a content part closed with no text block open",
                    ));
                };
                events.push(StreamEvent::BlockStop { index });
            }
            "response.function_call_arguments.delta" => {
                top.ignore("item_id");
                top.ignore("output_index");
                top.ignore("obfuscation");
                let delta = top.require_string("delta")?;
                let Some((index, OpenKind::Tool)) = self.open else {
                    return Err(Unsupported::new(
                        "delta",
                        "a tool-arguments delta arrived with no tool block open",
                    ));
                };
                events.push(StreamEvent::BlockDelta {
                    index,
                    delta: Delta::InputJson(delta),
                });
            }
            // The full-arguments echo of the deltas that already streamed.
            "response.function_call_arguments.done" => {
                top.ignore("item_id");
                top.ignore("output_index");
                top.ignore("name");
                top.ignore("arguments");
            }
            "response.output_item.done" => {
                top.ignore("output_index");
                let mut item = top.take_object("item")?.ok_or_else(|| {
                    Unsupported::new("item", "response.output_item.done has no item")
                })?;
                let item_kind = item.require_string("type")?;
                match item_kind.as_str() {
                    // The complete-item echo; its parts already closed
                    // themselves, and a still-open block (a stream that
                    // skipped `content_part.done`) is closed here.
                    "message" => {
                        item.ignore("id");
                        item.ignore("status");
                        item.ignore("role");
                        item.ignore("content");
                        self.close_open(&mut events);
                    }
                    "function_call" => {
                        item.ignore("id");
                        item.ignore("status");
                        item.ignore("call_id");
                        item.ignore("name");
                        item.ignore("arguments");
                        self.close_open(&mut events);
                    }
                    "reasoning" => {
                        let refusal = Unsupported::new(item.at("type"), reason("reasoning item"));
                        item.ignore("id");
                        item.ignore("status");
                        let summary_empty = match item.take("summary") {
                            None | Some(Value::Null) => true,
                            Some(Value::Array(summary)) => summary.is_empty(),
                            Some(_) => false,
                        };
                        let content_empty = match item.take("content") {
                            None | Some(Value::Null) => true,
                            Some(Value::Array(content)) => content.is_empty(),
                            Some(_) => false,
                        };
                        let no_encrypted =
                            matches!(item.take("encrypted_content"), None | Some(Value::Null));
                        if !(summary_empty && content_empty && no_encrypted) {
                            return Err(refusal);
                        }
                    }
                    other => {
                        return Err(Unsupported::new(
                            item.at("type"),
                            format!(
                                "the streamed item type `{other}` is not one this codec \
                                     carries"
                            ),
                        ));
                    }
                }
                item.finish()?;
            }
            "response.completed" | "response.incomplete" => {
                self.close_open(&mut events);
                let snapshot = top.take("response");
                let usage = snapshot_usage(snapshot.as_ref())?;
                let stop_reason = if kind == "response.completed" {
                    if self.saw_tool_call {
                        StopReason::ToolUse
                    } else {
                        StopReason::EndTurn
                    }
                } else {
                    let incomplete_reason = snapshot
                        .as_ref()
                        .and_then(|response| response.get("incomplete_details"))
                        .and_then(|details| details.get("reason"))
                        .and_then(Value::as_str)
                        .map(str::to_owned);
                    // The blocks already streamed; only the status decides.
                    stop_reason_of("incomplete", incomplete_reason.as_deref(), &[])?
                };
                events.push(StreamEvent::MessageDelta {
                    stop_reason,
                    stop_sequence: None,
                    usage,
                });
                events.push(StreamEvent::MessageStop);
                self.done = true;
            }
            "response.failed" => {
                let message = top
                    .take("response")
                    .as_ref()
                    .and_then(|response| response.get("error"))
                    .and_then(|error| error.get("message"))
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned();
                return Err(Unsupported::new(
                    "response.failed",
                    format!("the provider reported the response failed: {message}"),
                ));
            }
            "error" => {
                top.ignore("code");
                top.ignore("param");
                let message = top.take_string("message")?.unwrap_or_default();
                return Err(Unsupported::new(
                    "error",
                    format!("the provider's stream reported an error: {message}"),
                ));
            }
            "response.reasoning_summary_part.added"
            | "response.reasoning_summary_part.done"
            | "response.reasoning_summary_text.delta"
            | "response.reasoning_summary_text.done"
            | "response.reasoning_text.delta"
            | "response.reasoning_text.done" => {
                return Err(Unsupported::new("type", reason("reasoning item")));
            }
            "response.refusal.delta" | "response.refusal.done" => {
                return Err(Unsupported::new("type", reason("refusal")));
            }
            other => {
                return Err(Unsupported::new(
                    "type",
                    format!("the stream event `{other}` is not one this codec knows"),
                ));
            }
        }
        top.finish()?;
        Ok(events)
    }

    fn finish(&mut self) -> Result<Vec<StreamEvent>, Unsupported> {
        if self.done {
            return Ok(Vec::new());
        }
        Err(Unsupported::new(
            "response.completed",
            "the provider's stream ended before response.completed",
        ))
    }

    fn is_done(&self) -> bool {
        self.done
    }
}

/// One block the encoder currently has open, with what has streamed so far —
/// the `done` events on this wire echo the whole item.
struct OpenItem {
    index: usize,
    item_id: String,
    kind: OpenEncode,
}

enum OpenEncode {
    Text {
        text: String,
    },
    Tool {
        call_id: String,
        name: String,
        arguments: String,
    },
}

/// Turns the canonical event order into Responses stream events.
///
/// Item ids are minted (`msg_<index>`, `fc_<index>`): the canonical form has
/// no item ids because Anthropic's wire has none, and nothing may depend on
/// them. The `call_id` on a function-call item is the canonical tool-use id,
/// verbatim, and the message delta's stop reason and usage are held until
/// `MessageStop`, because this wire says both only in its final snapshot.
#[derive(Default)]
struct EventEncoder {
    id: String,
    model: String,
    open: Option<OpenItem>,
    /// Completed output items, echoed whole in the final snapshot.
    output: Vec<Value>,
    stop_reason: Option<StopReason>,
    /// What the message start stated — an Anthropic upstream states the
    /// input count only there, and its final delta restates only the output.
    start_usage: Usage,
    usage: Usage,
}

impl EventEncoder {
    fn event(name: &str, mut data: Map<String, Value>) -> Vec<u8> {
        data.insert("type".to_owned(), json!(name));
        stream::encode(Some(name), &Value::Object(data).to_string())
    }

    fn snapshot(&self, status: &str, incomplete_details: Value, usage: Value) -> Value {
        json!({
            "id": self.id,
            "object": "response",
            "created_at": 0,
            "status": status,
            "incomplete_details": incomplete_details,
            "error": null,
            "model": self.model,
            "output": self.output,
            "usage": usage,
        })
    }
}

impl StreamEncoder for EventEncoder {
    fn encode(&mut self, event: &StreamEvent) -> Vec<u8> {
        match event {
            StreamEvent::MessageStart { id, model, usage } => {
                self.id = id.clone();
                self.model = model.clone();
                self.start_usage = *usage;
                let mut data = Map::new();
                data.insert(
                    "response".to_owned(),
                    self.snapshot("in_progress", Value::Null, usage_json(usage)),
                );
                Self::event("response.created", data)
            }
            StreamEvent::BlockStart { index, block } => match block {
                BlockStart::Text => {
                    let item_id = format!("msg_{index}");
                    let mut added = Map::new();
                    added.insert("output_index".to_owned(), json!(index));
                    added.insert(
                        "item".to_owned(),
                        json!({
                            "id": item_id,
                            "type": "message",
                            "status": "in_progress",
                            "role": "assistant",
                            "content": [],
                        }),
                    );
                    let mut part = Map::new();
                    part.insert("item_id".to_owned(), json!(item_id));
                    part.insert("output_index".to_owned(), json!(index));
                    part.insert("content_index".to_owned(), json!(0));
                    part.insert(
                        "part".to_owned(),
                        json!({"type": "output_text", "text": "", "annotations": []}),
                    );
                    let mut out = Self::event("response.output_item.added", added);
                    out.extend(Self::event("response.content_part.added", part));
                    self.open = Some(OpenItem {
                        index: *index,
                        item_id,
                        kind: OpenEncode::Text {
                            text: String::new(),
                        },
                    });
                    out
                }
                BlockStart::ToolUse { id, name } => {
                    let item_id = format!("fc_{index}");
                    let mut added = Map::new();
                    added.insert("output_index".to_owned(), json!(index));
                    added.insert(
                        "item".to_owned(),
                        json!({
                            "id": item_id,
                            "type": "function_call",
                            "status": "in_progress",
                            "call_id": id,
                            "name": name,
                            "arguments": "",
                        }),
                    );
                    self.open = Some(OpenItem {
                        index: *index,
                        item_id,
                        kind: OpenEncode::Tool {
                            call_id: id.clone(),
                            name: name.clone(),
                            arguments: String::new(),
                        },
                    });
                    Self::event("response.output_item.added", added)
                }
            },
            StreamEvent::BlockDelta { delta, .. } => {
                // Canonical sequences open a block before its deltas — both
                // producers in this crate do — so an orphan delta has no item
                // to ride on and produces nothing.
                let Some(open) = self.open.as_mut() else {
                    return Vec::new();
                };
                let mut data = Map::new();
                data.insert("item_id".to_owned(), json!(open.item_id));
                data.insert("output_index".to_owned(), json!(open.index));
                match (&mut open.kind, delta) {
                    (OpenEncode::Text { text }, Delta::Text(more)) => {
                        text.push_str(more);
                        data.insert("content_index".to_owned(), json!(0));
                        data.insert("delta".to_owned(), json!(more));
                        Self::event("response.output_text.delta", data)
                    }
                    (OpenEncode::Tool { arguments, .. }, Delta::InputJson(more)) => {
                        arguments.push_str(more);
                        data.insert("delta".to_owned(), json!(more));
                        Self::event("response.function_call_arguments.delta", data)
                    }
                    _ => Vec::new(),
                }
            }
            StreamEvent::BlockStop { .. } => {
                let Some(open) = self.open.take() else {
                    return Vec::new();
                };
                match open.kind {
                    OpenEncode::Text { text } => {
                        let part = json!({"type": "output_text", "text": text, "annotations": []});
                        let item = json!({
                            "id": open.item_id,
                            "type": "message",
                            "status": "completed",
                            "role": "assistant",
                            "content": [part.clone()],
                        });
                        let mut text_done = Map::new();
                        text_done.insert("item_id".to_owned(), json!(open.item_id));
                        text_done.insert("output_index".to_owned(), json!(open.index));
                        text_done.insert("content_index".to_owned(), json!(0));
                        text_done.insert("text".to_owned(), json!(text));
                        let mut part_done = Map::new();
                        part_done.insert("item_id".to_owned(), json!(open.item_id));
                        part_done.insert("output_index".to_owned(), json!(open.index));
                        part_done.insert("content_index".to_owned(), json!(0));
                        part_done.insert("part".to_owned(), part);
                        let mut item_done = Map::new();
                        item_done.insert("output_index".to_owned(), json!(open.index));
                        item_done.insert("item".to_owned(), item.clone());
                        let mut out = Self::event("response.output_text.done", text_done);
                        out.extend(Self::event("response.content_part.done", part_done));
                        out.extend(Self::event("response.output_item.done", item_done));
                        self.output.push(item);
                        out
                    }
                    OpenEncode::Tool {
                        call_id,
                        name,
                        arguments,
                    } => {
                        let item = json!({
                            "id": open.item_id,
                            "type": "function_call",
                            "status": "completed",
                            "call_id": call_id,
                            "name": name,
                            "arguments": arguments,
                        });
                        let mut arguments_done = Map::new();
                        arguments_done.insert("item_id".to_owned(), json!(open.item_id));
                        arguments_done.insert("output_index".to_owned(), json!(open.index));
                        arguments_done.insert("arguments".to_owned(), json!(arguments));
                        let mut item_done = Map::new();
                        item_done.insert("output_index".to_owned(), json!(open.index));
                        item_done.insert("item".to_owned(), item.clone());
                        let mut out =
                            Self::event("response.function_call_arguments.done", arguments_done);
                        out.extend(Self::event("response.output_item.done", item_done));
                        self.output.push(item);
                        out
                    }
                }
            }
            StreamEvent::MessageDelta {
                stop_reason, usage, ..
            } => {
                // This wire states the stop reason and usage only in the
                // final snapshot; held until MessageStop — merged with the
                // start's reading exactly as `canonical::accumulate` does,
                // because an input of zero in the final delta means "not
                // restated", not "none".
                self.stop_reason = Some(*stop_reason);
                self.usage = Usage {
                    input: if usage.input == 0 {
                        self.start_usage.input
                    } else {
                        usage.input
                    },
                    output: usage.output,
                    cached: usage.cached.or(self.start_usage.cached),
                };
                Vec::new()
            }
            StreamEvent::MessageStop => {
                let stop_reason = self.stop_reason.unwrap_or(StopReason::EndTurn);
                let (status, incomplete_details) = status_of(stop_reason);
                let name = if status == "completed" {
                    "response.completed"
                } else {
                    "response.incomplete"
                };
                let mut data = Map::new();
                data.insert(
                    "response".to_owned(),
                    self.snapshot(status, incomplete_details, usage_json(&self.usage)),
                );
                Self::event(name, data)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use super::super::anthropic::tests::full_request;
    use super::super::canonical::{accumulate, tests::tool_call_response};
    use super::super::stream::SseReader;
    use std::io::BufReader;

    /// The supported subset on this wire: [`full_request`] minus the stop
    /// sequences the Responses API has no parameter for.
    fn responses_request() -> Request {
        let mut request = full_request();
        request.stop = Vec::new();
        request
    }

    #[test]
    fn a_request_round_trips_through_the_openai_responses_wire() {
        let request = responses_request();
        let wire = encode_request(&request);
        let decoded = decode_request(&wire).expect("the codec reads what it wrote");
        assert_eq!(decoded, request);
    }

    #[test]
    fn the_tool_round_crosses_the_responses_wire_with_ids_and_the_error_flag_intact() {
        let request = responses_request();
        let wire = encode_request(&request);
        let document: Value = serde_json::from_slice(&wire).unwrap();

        assert_eq!(document["instructions"], "You are careful.");
        assert_eq!(document["max_output_tokens"], 4096);
        assert_eq!(
            document["store"], false,
            "never left to the provider's default"
        );
        assert_eq!(document["user"], "user_123");
        assert_eq!(
            document["tool_choice"],
            json!({"type": "function", "name": "Bash"})
        );
        assert_eq!(document["parallel_tool_calls"], false);
        assert_eq!(document["stream"], true);
        assert_eq!(
            document.get("stop"),
            None,
            "no stop parameter exists on this wire"
        );

        let input = document["input"].as_array().expect("input items");
        assert_eq!(input[0]["type"], "message");
        assert_eq!(input[0]["role"], "user");
        assert_eq!(input[1]["role"], "assistant");
        assert_eq!(
            input[1]["content"],
            json!([{"type": "output_text", "text": "On it."}])
        );
        assert_eq!(input[2]["type"], "function_call");
        assert_eq!(input[2]["call_id"], "toolu_01A");
        assert_eq!(input[2]["name"], "Bash");
        assert_eq!(input[2]["arguments"], r#"{"command":"ls"}"#);
        assert_eq!(input[3]["call_id"], "toolu_01B");
        assert_eq!(input[4]["type"], "function_call_output");
        assert_eq!(input[4]["call_id"], "toolu_01A");
        assert_eq!(
            input[4]["output"],
            format!("{TOOL_ERROR_MARKER}\nls: cannot access"),
            "an erroring result is carried, labelled, in the only channel the wire has"
        );
        assert_eq!(input[5]["call_id"], "toolu_01B");
        assert_eq!(input[5]["output"], "# notes\nhello");
        assert_eq!(input[6]["type"], "message");
        assert_eq!(input[6]["role"], "user");
        let parts = input[6]["content"].as_array().unwrap();
        assert_eq!(
            parts[0],
            json!({"type": "input_text", "text": "Now summarise."})
        );
        assert_eq!(
            parts[1],
            json!({"type": "input_image", "image_url": "data:image/png;base64,iVBORw0KGgo="})
        );
        assert_eq!(
            parts[2],
            json!({"type": "input_image", "image_url": "https://example.test/a.png"})
        );

        let tools = document["tools"].as_array().expect("tools");
        assert_eq!(tools[0]["type"], "function");
        assert_eq!(
            tools[0]["name"], "Bash",
            "flat, unlike OpenAI Chat's nesting"
        );
        assert_eq!(
            tools[0]["strict"], false,
            "strict is never left to this wire's default, which is true"
        );
        assert_eq!(tools[0]["parameters"]["required"][0], "command");
    }

    #[test]
    fn the_request_codex_sends_decodes_with_the_tool_run_merged_into_turns() {
        let wire = br#"{
            "model": "gpt-5",
            "instructions": "sys",
            "input": [
                {"type": "message", "role": "user", "content": [{"type": "input_text", "text": "go"}, {"type": "input_image", "image_url": "data:image/png;base64,AAAA", "detail": "auto"}]},
                {"type": "message", "role": "assistant", "content": [{"type": "output_text", "text": "Sure.", "annotations": []}]},
                {"type": "function_call", "id": "fc_1", "call_id": "call_1", "name": "f", "arguments": "", "status": "completed"},
                {"type": "function_call_output", "call_id": "call_1", "output": "ok"},
                {"role": "user", "content": "thanks"}
            ],
            "tools": [{"type": "function", "name": "f", "description": "d", "strict": false}],
            "tool_choice": "required",
            "max_output_tokens": 50,
            "store": false,
            "truncation": "disabled",
            "safety_identifier": "u1",
            "parallel_tool_calls": true,
            "stream": false
        }"#;
        let request = decode_request(wire).expect("a Codex-shaped request decodes");
        assert_eq!(request.system.as_deref(), Some("sys"));
        assert_eq!(request.messages.len(), 3);
        assert_eq!(
            request.messages[1].blocks,
            vec![
                Block::Text("Sure.".to_owned()),
                Block::ToolUse {
                    id: "call_1".to_owned(),
                    name: "f".to_owned(),
                    input: json!({}),
                },
            ],
            "the assistant message item and its function_call are one turn"
        );
        assert_eq!(
            request.messages[2].blocks,
            vec![
                Block::ToolResult {
                    tool_use_id: "call_1".to_owned(),
                    content: "ok".to_owned(),
                    is_error: false,
                },
                Block::Text("thanks".to_owned()),
            ]
        );
        assert_eq!(
            request.messages[0].blocks[1],
            Block::Image(ImageSource::Base64 {
                media_type: "image/png".to_owned(),
                data: "AAAA".to_owned()
            })
        );
        assert_eq!(request.tools[0].input_schema["type"], "object");
        assert_eq!(request.tool_choice, Some(ToolChoice::Any));
        assert_eq!(request.parallel_tool_calls, Some(true));
        assert_eq!(request.max_tokens, Some(50));
        assert_eq!(request.user.as_deref(), Some("u1"));
        assert!(request.stop.is_empty());
    }

    #[test]
    fn every_refused_request_field_is_refused_by_its_name() {
        let base = |extra: &str| {
            format!(r#"{{"model": "m", "input": [{{"role": "user", "content": "x"}}]{extra}}}"#)
        };
        let cases: Vec<(String, &str)> = vec![
            (base(r#", "previous_response_id": "resp_1""#), "previous_response_id"),
            (base(r#", "store": true"#), "store"),
            (base(r#", "background": true"#), "background"),
            (base(r#", "include": ["reasoning.encrypted_content"]"#), "include"),
            (base(r#", "reasoning": {"effort": "medium"}"#), "reasoning"),
            (base(r#", "truncation": "auto""#), "truncation"),
            (base(r#", "metadata": {"a": "b"}"#), "metadata"),
            (base(r#", "top_logprobs": 3"#), "top_logprobs"),
            (base(r#", "max_tool_calls": 2"#), "max_tool_calls"),
            (base(r#", "service_tier": "flex""#), "service_tier"),
            (base(r#", "prompt": {"id": "p1"}"#), "prompt"),
            (base(r#", "prompt_cache_key": "k""#), "prompt_cache_key"),
            (base(r#", "text": {"format": {"type": "json_object"}}"#), "text.format.type"),
            (base(r#", "text": {"verbosity": "low"}"#), "text.verbosity"),
            (base(r#", "user": "a", "safety_identifier": "b""#), "safety_identifier"),
            (base(r#", "unknown_future_field": 1"#), "unknown_future_field"),
            (
                base(r#", "tools": [{"type": "web_search"}]"#),
                "tools[0].type",
            ),
            (
                base(r#", "tools": [{"type": "function", "name": "f", "strict": true}]"#),
                "tools[0].strict",
            ),
            (
                base(r#", "tool_choice": {"type": "mcp", "server_label": "s"}"#),
                "tool_choice.type",
            ),
            (
                r#"{"model": "m", "input": [{"type": "reasoning", "id": "rs_1", "summary": [], "encrypted_content": "xx"}]}"#.to_owned(),
                "input[0].type",
            ),
            (
                r#"{"model": "m", "input": [{"type": "item_reference", "id": "msg_1"}]}"#.to_owned(),
                "input[0].type",
            ),
            (
                r#"{"model": "m", "input": [{"type": "web_search_call", "id": "ws_1"}]}"#.to_owned(),
                "input[0].type",
            ),
            (
                r#"{"model": "m", "input": [{"role": "user", "content": [{"type": "input_file", "file_id": "f1"}]}]}"#.to_owned(),
                "input[0].content[0].type",
            ),
            (
                r#"{"model": "m", "input": [{"role": "assistant", "content": [{"type": "refusal", "refusal": "no"}]}]}"#.to_owned(),
                "input[0].content[0].type",
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
    fn the_stop_sequences_this_wire_cannot_carry_are_refused_before_encoding() {
        let refusal = OpenAiResponses
            .refuse_unencodable(&full_request())
            .expect_err("full_request carries stop sequences");
        assert_eq!(refusal.field, "stop_sequences");
        assert!(refusal.reason.contains("no stop sequences"));
        assert!(
            OpenAiResponses
                .refuse_unencodable(&responses_request())
                .is_ok()
        );
    }

    #[test]
    fn a_response_round_trips_through_the_openai_responses_wire() {
        let response = tool_call_response();
        let wire = encode_response(&response);
        let document: Value = serde_json::from_slice(&wire).unwrap();
        assert_eq!(document["status"], "completed");
        assert_eq!(document["output"][0]["type"], "message");
        assert_eq!(
            document["output"][1]["call_id"], "call_abc123",
            "the call_id is the canonical tool-use id, verbatim"
        );
        assert_eq!(document["output"][2]["call_id"], "call_def456");
        assert_eq!(document["usage"]["input_tokens"], 220);
        assert_eq!(
            document["usage"]["input_tokens_details"]["cached_tokens"],
            100
        );
        assert_eq!(
            decode_response(&wire).expect("the codec reads what it wrote"),
            response
        );

        // ... and a text-only max-tokens response, which crosses as
        // incomplete with its reason.
        let cut_short = Response {
            id: "resp_2".to_owned(),
            model: "m".to_owned(),
            blocks: vec![Block::Text("partial".to_owned())],
            stop_reason: StopReason::MaxTokens,
            stop_sequence: None,
            usage: Usage {
                input: 3,
                output: 2,
                cached: None,
            },
        };
        let wire = encode_response(&cut_short);
        let document: Value = serde_json::from_slice(&wire).unwrap();
        assert_eq!(document["status"], "incomplete");
        assert_eq!(
            document["incomplete_details"]["reason"],
            "max_output_tokens"
        );
        assert_eq!(decode_response(&wire).unwrap(), cut_short);
    }

    #[test]
    fn an_incomplete_response_maps_its_reason_and_a_failed_one_carries_the_providers_message() {
        let incomplete = |reason: &str| {
            format!(
                r#"{{"id": "r", "status": "incomplete", "incomplete_details": {{"reason": "{reason}"}}, "model": "m", "output": []}}"#
            )
        };
        assert_eq!(
            decode_response(incomplete("max_output_tokens").as_bytes())
                .unwrap()
                .stop_reason,
            StopReason::MaxTokens
        );
        assert_eq!(
            decode_response(incomplete("content_filter").as_bytes())
                .unwrap()
                .stop_reason,
            StopReason::Refusal
        );

        let failed = br#"{"id": "r", "status": "failed", "error": {"code": "server_error", "message": "the model ran aground"}, "model": "m", "output": []}"#;
        let refusal = decode_response(failed).unwrap_err();
        assert_eq!(refusal.field, "error");
        assert!(refusal.reason.contains("the model ran aground"));

        let in_progress = br#"{"id": "r", "status": "in_progress", "model": "m", "output": []}"#;
        assert_eq!(decode_response(in_progress).unwrap_err().field, "status");
    }

    #[test]
    fn an_empty_reasoning_item_is_skipped_and_one_carrying_anything_is_refused() {
        let with_empty = br#"{"id": "r", "status": "completed", "model": "m", "output": [
            {"type": "reasoning", "id": "rs_1", "summary": []},
            {"type": "message", "id": "msg_1", "status": "completed", "role": "assistant", "content": [{"type": "output_text", "text": "hi", "annotations": []}]}
        ]}"#;
        let response = decode_response(with_empty).expect("an empty reasoning item is skipped");
        assert_eq!(response.blocks, vec![Block::Text("hi".to_owned())]);

        let with_summary = br#"{"id": "r", "status": "completed", "model": "m", "output": [
            {"type": "reasoning", "id": "rs_1", "summary": [{"type": "summary_text", "text": "thinking..."}]}
        ]}"#;
        assert_eq!(
            decode_response(with_summary).unwrap_err().field,
            "output[0].type"
        );

        let with_builtin = br#"{"id": "r", "status": "completed", "model": "m", "output": [
            {"type": "web_search_call", "id": "ws_1", "status": "completed"}
        ]}"#;
        let refusal = decode_response(with_builtin).unwrap_err();
        assert_eq!(refusal.field, "output[0].type");
        assert!(refusal.reason.contains("hosted tool"));
    }

    #[test]
    fn a_stream_round_trips_through_the_openai_responses_wire() {
        let response = tool_call_response();
        let events = response.as_events();
        let mut encoder = EventEncoder::default();
        let mut wire = Vec::new();
        for event in &events {
            wire.extend(encoder.encode(event));
        }
        let text = String::from_utf8_lossy(&wire);
        assert!(text.contains("event: response.created\n"));
        assert!(text.contains("event: response.completed\n"));
        assert!(text.contains(r#""call_id":"call_abc123""#));

        let mut reader = SseReader::new(BufReader::new(&wire[..]));
        let mut decoder = EventDecoder::default();
        let mut decoded = Vec::new();
        while let Some(event) = reader.next_event().unwrap() {
            decoded.extend(decoder.feed(&event).expect("the codec reads what it wrote"));
        }
        assert!(decoder.is_done());
        assert_eq!(decoded, events);
    }

    /// The event shape a real Responses upstream sends for a text part and a
    /// function call: lifecycle snapshots, `sequence_number` on every event,
    /// `obfuscation` padding on deltas, the full-text and full-arguments
    /// `done` echoes, and usage only in the final snapshot.
    #[test]
    fn real_provider_events_become_anthropics_event_order_with_ids_preserved() {
        let chunks = [
            r#"{"type":"response.created","sequence_number":0,"response":{"id":"resp_1","object":"response","created_at":1,"status":"in_progress","model":"gpt-5","output":[],"usage":null,"store":false,"tools":[]}}"#,
            r#"{"type":"response.in_progress","sequence_number":1,"response":{"id":"resp_1","object":"response","created_at":1,"status":"in_progress","model":"gpt-5","output":[],"usage":null}}"#,
            r#"{"type":"response.output_item.added","sequence_number":2,"output_index":0,"item":{"id":"msg_1","type":"message","status":"in_progress","role":"assistant","content":[]}}"#,
            r#"{"type":"response.content_part.added","sequence_number":3,"item_id":"msg_1","output_index":0,"content_index":0,"part":{"type":"output_text","text":"","annotations":[]}}"#,
            r#"{"type":"response.output_text.delta","sequence_number":4,"item_id":"msg_1","output_index":0,"content_index":0,"delta":"Sure.","logprobs":[],"obfuscation":"xK9"}"#,
            r#"{"type":"response.output_text.done","sequence_number":5,"item_id":"msg_1","output_index":0,"content_index":0,"text":"Sure.","logprobs":[]}"#,
            r#"{"type":"response.content_part.done","sequence_number":6,"item_id":"msg_1","output_index":0,"content_index":0,"part":{"type":"output_text","text":"Sure.","annotations":[]}}"#,
            r#"{"type":"response.output_item.done","sequence_number":7,"output_index":0,"item":{"id":"msg_1","type":"message","status":"completed","role":"assistant","content":[{"type":"output_text","text":"Sure.","annotations":[]}]}}"#,
            r#"{"type":"response.output_item.added","sequence_number":8,"output_index":1,"item":{"id":"fc_1","type":"function_call","status":"in_progress","call_id":"call_A","name":"Bash","arguments":""}}"#,
            r#"{"type":"response.function_call_arguments.delta","sequence_number":9,"item_id":"fc_1","output_index":1,"delta":"{\"command\"","obfuscation":"a"}"#,
            r#"{"type":"response.function_call_arguments.delta","sequence_number":10,"item_id":"fc_1","output_index":1,"delta":": \"ls\"}"}"#,
            r#"{"type":"response.function_call_arguments.done","sequence_number":11,"item_id":"fc_1","output_index":1,"name":"Bash","arguments":"{\"command\": \"ls\"}"}"#,
            r#"{"type":"response.output_item.done","sequence_number":12,"output_index":1,"item":{"id":"fc_1","type":"function_call","status":"completed","call_id":"call_A","name":"Bash","arguments":"{\"command\": \"ls\"}"}}"#,
            r#"{"type":"response.completed","sequence_number":13,"response":{"id":"resp_1","object":"response","created_at":1,"status":"completed","model":"gpt-5","output":[],"usage":{"input_tokens":50,"input_tokens_details":{"cached_tokens":10},"output_tokens":9,"output_tokens_details":{"reasoning_tokens":0},"total_tokens":59}}}"#,
        ];
        let mut decoder = EventDecoder::default();
        let mut events = Vec::new();
        for data in chunks {
            events.extend(
                decoder
                    .feed(&SseEvent {
                        event: None,
                        data: data.to_owned(),
                    })
                    .expect("a real provider's events decode"),
            );
        }
        assert!(decoder.is_done());
        let response = accumulate(&events).expect("a complete message");
        assert_eq!(
            response.blocks,
            vec![
                Block::Text("Sure.".to_owned()),
                Block::ToolUse {
                    id: "call_A".to_owned(),
                    name: "Bash".to_owned(),
                    input: json!({"command": "ls"}),
                },
            ]
        );
        assert_eq!(response.stop_reason, StopReason::ToolUse);
        assert_eq!(
            response.usage,
            Usage {
                input: 40,
                output: 9,
                cached: Some(10)
            }
        );
        // The order is Anthropic's: every block stops before the next
        // starts, and the message delta comes last but one.
        let stops: Vec<usize> = events
            .iter()
            .filter_map(|event| match event {
                StreamEvent::BlockStop { index } => Some(*index),
                _ => None,
            })
            .collect();
        assert_eq!(stops, vec![0, 1]);
        assert!(matches!(
            events[events.len() - 2],
            StreamEvent::MessageDelta {
                stop_reason: StopReason::ToolUse,
                ..
            }
        ));
    }

    #[test]
    fn a_reasoning_summary_a_refusal_part_and_a_stream_error_are_named_refusals() {
        let mut decoder = EventDecoder::default();
        let summary = SseEvent {
            event: None,
            data: r#"{"type":"response.reasoning_summary_text.delta","sequence_number":1,"item_id":"rs_1","output_index":0,"summary_index":0,"delta":"hmm"}"#.to_owned(),
        };
        let refusal = decoder.feed(&summary).unwrap_err();
        assert!(refusal.reason.contains("reasoning item"));

        let mut decoder = EventDecoder::default();
        let refusal_part = SseEvent {
            event: None,
            data: r#"{"type":"response.content_part.added","sequence_number":1,"item_id":"msg_1","output_index":0,"content_index":0,"part":{"type":"refusal","refusal":"no"}}"#.to_owned(),
        };
        assert_eq!(decoder.feed(&refusal_part).unwrap_err().field, "part.type");

        let mut decoder = EventDecoder::default();
        let error = SseEvent {
            event: None,
            data: r#"{"type":"error","code":"server_error","message":"Overloaded","param":null,"sequence_number":2}"#.to_owned(),
        };
        let refusal = decoder.feed(&error).unwrap_err();
        assert_eq!(refusal.field, "error");
        assert!(refusal.reason.contains("Overloaded"));

        let mut decoder = EventDecoder::default();
        let failed = SseEvent {
            event: None,
            data: r#"{"type":"response.failed","sequence_number":3,"response":{"id":"r","status":"failed","error":{"code":"server_error","message":"upstream fell over"}}}"#.to_owned(),
        };
        let refusal = decoder.feed(&failed).unwrap_err();
        assert!(refusal.reason.contains("upstream fell over"));
    }

    #[test]
    fn a_stream_that_ends_before_response_completed_is_refused_at_finish() {
        let mut decoder = EventDecoder::default();
        assert_eq!(decoder.finish().unwrap_err().field, "response.completed");

        // ... including one that started and streamed a block first.
        let mut decoder = EventDecoder::default();
        for data in [
            r#"{"type":"response.created","response":{"id":"r","model":"m","output":[],"usage":null}}"#,
            r#"{"type":"response.output_item.added","output_index":0,"item":{"id":"msg_1","type":"message","status":"in_progress","role":"assistant","content":[]}}"#,
        ] {
            decoder
                .feed(&SseEvent {
                    event: None,
                    data: data.to_owned(),
                })
                .unwrap();
        }
        assert!(!decoder.is_done());
        assert_eq!(decoder.finish().unwrap_err().field, "response.completed");

        // An orphan delta is refused rather than misfiled.
        let mut decoder = EventDecoder::default();
        let orphan = SseEvent {
            event: None,
            data: r#"{"type":"response.output_text.delta","item_id":"msg_1","output_index":0,"content_index":0,"delta":"x"}"#.to_owned(),
        };
        assert_eq!(decoder.feed(&orphan).unwrap_err().field, "delta");
    }
}
