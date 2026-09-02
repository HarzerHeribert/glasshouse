//! The Google Generative Language codec: `generateContent` and
//! `streamGenerateContent` requests, responses and stream chunks, into and
//! out of [`super::canonical`].
//!
//! The fourth wire, and the first that differs from the other three in
//! **shape** rather than only in spelling. Five decisions are worth reading
//! before the code, because each is a place where a mechanical mapping would
//! have been wrong.
//!
//! # 1. The model is in the path, not in the body
//!
//! Anthropic Messages, OpenAI Chat and OpenAI Responses all post to one
//! fixed path and name the model in the document. Gemini posts to
//! `…/v1beta/models/<model>:generateContent`, so the request target *is*
//! part of the translation. [`Gemini::outbound_endpoint`] is where that
//! happens, and [`Gemini::refuse_unencodable`] refuses a model name that
//! could not address a path — a name carrying `/`, `?`, `#` or whitespace
//! would otherwise be smuggled into the request line.
//!
//! The outbound path carries **`/v1beta` itself**, and the `gemini`
//! provider template's base URL is the bare host, for the reason Anthropic
//! Messages' entry in [`super::outbound_target`] records: a request the
//! provider serves natively is **relayed byte for byte**, target included,
//! and a Gemini client's own target already starts `/v1beta`. A base URL
//! carrying the version and a relayed target carrying it too composes
//! `…/v1beta/v1beta/models/…`, which the service answers `404` for and
//! which the harness would report as a model error. One of the two has to
//! own the segment; the relay cannot, so this does.
//!
//! A streamed request goes to `:streamGenerateContent?alt=sse`. Without
//! `alt=sse` Google answers a streamed **JSON array**, not server-sent
//! events, and [`super::stream::SseReader`] would see one enormous line.
//!
//! # 2. A function call has no id, so a tool result is matched by NAME
//!
//! This is the one that decides whether a harness's tooling survives.
//! Gemini's `functionCall` carries `{name, args}` and no id at all, and its
//! `functionResponse` carries `{name, response}` — the **name** is the
//! matching key on this wire, where on the other three the id is.
//!
//! So the two directions are not symmetric, and neither of them invents a
//! mapping table:
//!
//! - **Encoding** a canonical request (the direction every supported pair
//!   uses): a [`Block::ToolResult`]'s `tool_use_id` is resolved to the
//!   `name` of the [`Block::ToolUse`] carrying that id **in the same
//!   request**. Every harness this gateway serves resends its whole
//!   conversation, so the call a result answers is right there. An id with
//!   no such block is refused by name rather than guessed at — a
//!   `functionResponse` under the wrong name runs the wrong tool's result
//!   into the model.
//! - **Decoding** a Gemini response: the harness needs *some* id to send
//!   back, and Gemini issued none, so this codec mints
//!   `gemini-call-<index>-<name>` — unique within one answer, and carrying
//!   the name it was minted from so a person reading a transcript can see
//!   what it means. It is never parsed back: the resolution above goes
//!   through the tool-use block, not through the id's spelling.
//!
//! # 3. `STOP` is not `end_turn` when the candidate is a function call
//!
//! Gemini reports `finishReason: "STOP"` for an answer that is entirely
//! function calls. A harness told `end_turn` after a tool call **stops
//! instead of running the tool**, which is the whole of capability map line
//! 1950 failing quietly. So the canonical stop reason is derived from the
//! content as well as the reason: a candidate containing any `functionCall`
//! part stops with [`StopReason::ToolUse`].
//!
//! # 4. The end-user identifier is dropped BY NAME, and it is the only one
//!
//! Gemini's request has no field for an end-user identifier. Claude Code
//! sends `metadata.user_id` on **every** request, so refusing it would
//! refuse the pair outright rather than refuse a field — and this codec's
//! whole purpose is a pair that works. It is therefore listed in
//! [`IGNORED_FIELDS`] and dropped there, exactly as `openai_chat` already
//! lists `stream_options.include_usage` and `image_url.detail`: named in the
//! table the `field_rows` view renders, never silent. It is an
//! abuse-monitoring hint that does not change the answer, which is why it is
//! the only request field this codec drops.
//!
//! # 5. The stream ends without a terminator, so the finish reason is one
//!
//! An SSE `streamGenerateContent` has no `data: [DONE]`; the socket simply
//! closes. A stream that ended early would otherwise be indistinguishable
//! from one that finished, and the harness would be handed a truncated
//! message wearing `end_turn` — the trap `openai_chat`'s `[DONE]` rule
//! exists to close. So this decoder treats **`finishReason` as the
//! terminator**: a stream that ends without one is refused by name.
//!
//! ## Which harness-side events are synthesised, and at which chunk
//!
//! Gemini's chunks are whole `GenerateContentResponse` documents, not the
//! typed start/delta/stop events the canonical vocabulary wants, so every
//! block boundary here is synthesised. Nothing is held back for it:
//!
//! | at | emitted |
//! |---|---|
//! | the **first** chunk | [`StreamEvent::MessageStart`] with `responseId` and `modelVersion` as they arrived |
//! | a chunk carrying a `text` part | [`StreamEvent::BlockStart`] (`Text`) if no text block is open, then a [`StreamEvent::BlockDelta`] with that fragment |
//! | a chunk carrying a `functionCall` part | a [`StreamEvent::BlockStop`] for whatever was open, then `BlockStart` (`ToolUse`) and one `BlockDelta` carrying the whole `args` — Gemini sends a call's arguments in one piece, so there is nothing to fragment |
//! | the chunk carrying `finishReason` / `usageMetadata` | nothing yet; both are held for the message's own delta, because a later chunk may still carry parts |
//! | the end of the stream | `BlockStop` for the open block, then [`StreamEvent::MessageDelta`] with the stop reason and usage, then [`StreamEvent::MessageStop`] |
//!
//! The first harness-side event therefore leaves on the first chunk, and a
//! text fragment leaves on the chunk that carried it. The one thing held is
//! the message's final delta, which cannot be written before the message
//! has finished by construction.

use serde_json::{Map, Value, json};

use super::canonical::{
    Block, BlockStart, Delta, ImageSource, Message, Request, Response, Role, StopReason,
    StreamEvent, ToolChoice, ToolDefinition, Unsupported, Usage, json_kind, parse_tool_input,
};
use super::fields::{Fields, element};
use super::stream::{self, SseEvent};
use super::{CacheDisposition, Claim, Codec, StreamDecoder, StreamEncoder};

pub(super) const PROTOCOL: &str = "gemini-generate-content";

/// The one target this codec translates, as a person reads it.
///
/// Unlike the other three codecs this is a **shape**, not a path: the model
/// is a path segment, so there is no single literal to match or to post to.
/// [`Gemini::claim`] decides what belongs to this protocol and
/// [`Gemini::outbound_endpoint`] builds the path a translated request is
/// posted to; this constant is what a refusal names.
pub(super) const ENDPOINT: &str = "/models/{model}:generateContent";

/// The API version a translated request is posted to.
///
/// `v1beta` rather than `v1`: function declarations, `toolConfig` and
/// `systemInstruction` — the three things a harness's tooling rides on — are
/// documented there, and it is the version every current Gemini client
/// sends. Stated by this codec rather than left to the provider's base URL,
/// because a relayed request carries the client's own version segment and
/// the two would compose into `/v1beta/v1beta/…`.
const OUTBOUND_VERSION: &str = "/v1beta";

/// The method suffix of a non-streamed request.
const GENERATE: &str = "generateContent";
/// The method suffix of a streamed request.
const STREAM_GENERATE: &str = "streamGenerateContent";

/// Google's own version segments, longest first. `VERSION_SEGMENT` — the
/// `/v1` the rest of the gateway strips before classifying a target — does
/// not cover `v1beta`, which is the segment every current Gemini client
/// sends, so this codec strips its own.
const VERSION_SEGMENTS: [&str; 3] = ["/v1beta", "/v1alpha", "/v1"];

/// The request fields and shapes this codec refuses, with the reason each
/// refusal carries.
pub(super) const REFUSED_FIELDS: &[(&str, &str)] = &[
    (
        "safetySettings",
        "per-request safety thresholds have no equivalent on the protocols this gateway \
         translates from, and silently dropping them would loosen a limit the caller set",
    ),
    (
        "cachedContent",
        "Gemini's explicit context cache is addressed by a resource name no other protocol \
         issues",
    ),
    (
        "generationConfig.responseMimeType",
        "a structured output format has no equivalent in the canonical form",
    ),
    (
        "generationConfig.responseSchema",
        "a structured output schema has no equivalent in the canonical form",
    ),
    (
        "generationConfig.responseJsonSchema",
        "a structured output schema has no equivalent in the canonical form",
    ),
    (
        "generationConfig.thinkingConfig",
        "a thinking budget has no equivalent in the canonical form",
    ),
    (
        "generationConfig.candidateCount",
        "more than one candidate per request has no equivalent in the canonical form",
    ),
    (
        "generationConfig.topK",
        "top-k sampling has no equivalent in the canonical form",
    ),
    (
        "generationConfig.seed",
        "deterministic sampling has no equivalent in the canonical form",
    ),
    (
        "generationConfig.presencePenalty",
        "presence penalties have no equivalent in the canonical form",
    ),
    (
        "generationConfig.frequencyPenalty",
        "frequency penalties have no equivalent in the canonical form",
    ),
    (
        "generationConfig.responseLogprobs",
        "log probabilities have no equivalent in the canonical form",
    ),
    (
        "generationConfig.logprobs",
        "log probabilities have no equivalent in the canonical form",
    ),
    (
        "generationConfig.responseModalities",
        "a non-text response modality has no equivalent in the canonical form",
    ),
    (
        "generationConfig.speechConfig",
        "audio output has no equivalent in the canonical form",
    ),
    (
        "fileData",
        "a Files-API URI is not a URL anything downstream could fetch, and the canonical form \
         carries only inline bytes or a public URL",
    ),
    (
        "thought",
        "a thought part is the model's own reasoning and has no equivalent in the canonical form",
    ),
    (
        "executableCode",
        "Gemini's code-execution tool is a server-side tool no other protocol declares",
    ),
    (
        "codeExecution",
        "Gemini's code-execution tool is a server-side tool no other protocol declares",
    ),
    (
        "googleSearch",
        "Gemini's search grounding is a server-side tool no other protocol declares",
    ),
    (
        "googleSearchRetrieval",
        "Gemini's search grounding is a server-side tool no other protocol declares",
    ),
    (
        "parallel_tool_calls",
        "Gemini has no parameter that disables parallel function calling, and a request that \
         asked for it would be answered as though it had not",
    ),
    (
        "model",
        "a Gemini request addresses its model in the path, so the model's name must be one a \
         path segment can carry: no `/`, `?`, `#` or whitespace",
    ),
    (
        "tool_use_id",
        "Gemini matches a function response to its call by NAME, and this result's id names no \
         tool-use block in the same request, so the call it answers cannot be identified",
    ),
    (
        "image",
        "Gemini carries an image as inline bytes; a URL source has no equivalent, and fetching \
         it here would make the gateway a downloader",
    ),
];

/// Fields dropped by name — see the module doc's fourth decision for `user`,
/// which is the only *request* field in this list and the only one whose
/// absence the caller could notice.
pub(super) const IGNORED_FIELDS: &[&str] = &[
    "user",
    "modelVersion",
    "candidates[].index",
    "candidates[].safetyRatings",
    "candidates[].citationMetadata",
    "candidates[].groundingMetadata",
    "candidates[].avgLogprobs",
    "candidates[].tokenCount",
    "candidates[].finishMessage",
    "promptFeedback",
    "usageMetadata.totalTokenCount",
    "usageMetadata.promptTokensDetails",
    "usageMetadata.candidatesTokensDetails",
    "usageMetadata.cacheTokensDetails",
    "usageMetadata.toolUsePromptTokenCount",
    "usageMetadata.toolUsePromptTokensDetails",
];

fn reason(field: &str) -> &'static str {
    REFUSED_FIELDS
        .iter()
        .find(|(name, _)| *name == field)
        .map(|(_, reason)| *reason)
        .expect("every refusal named in this file is listed in REFUSED_FIELDS")
}

pub(super) struct Gemini;

impl Codec for Gemini {
    fn protocol(&self) -> &'static str {
        PROTOCOL
    }

    fn endpoint(&self) -> &'static str {
        ENDPOINT
    }

    /// `/models/<model>:<method>`, with Google's own version segment
    /// stripped rather than the gateway's `/v1`.
    ///
    /// A path with no `:` is not claimed at all — that includes `/models`
    /// itself, which is a model *listing* on this wire and on OpenAI's, and
    /// which the gateway has always answered with its plain `404`.
    fn claim(&self, path: &str) -> Claim {
        let path = VERSION_SEGMENTS
            .iter()
            .find_map(|segment| match path.strip_prefix(segment) {
                Some(rest) if rest.starts_with('/') => Some(rest),
                _ => None,
            })
            .unwrap_or(path);
        let Some(rest) = path.strip_prefix("/models/") else {
            return Claim::None;
        };
        let Some((model, method)) = rest.split_once(':') else {
            return Claim::None;
        };
        if model.is_empty() || model.contains('/') || method.is_empty() {
            return Claim::None;
        }
        if method == GENERATE || method == STREAM_GENERATE {
            Claim::Endpoint
        } else {
            // `:countTokens`, `:embedContent`, `:batchEmbedContents` — this
            // protocol's own surface, and none of it is inference this
            // gateway translates.
            Claim::Other
        }
    }

    /// `/v1beta/models/<model>:generateContent`, or the streaming method
    /// with `alt=sse` when the harness asked for a stream.
    ///
    /// The version segment is here rather than on the provider's base URL —
    /// see the module doc's first decision, and the relay test that found
    /// it. `alt=sse` is not decoration either: without it Google answers a
    /// streamed JSON **array** rather than server-sent events, and the
    /// reader on the other side of this call frames events.
    fn outbound_endpoint(&self, request: &Request) -> String {
        let model = model_segment(&request.model);
        if request.stream {
            format!("{OUTBOUND_VERSION}/models/{model}:{STREAM_GENERATE}?alt=sse")
        } else {
            format!("{OUTBOUND_VERSION}/models/{model}:{GENERATE}")
        }
    }

    fn refuse_unencodable(&self, request: &Request) -> Result<(), Unsupported> {
        let model = model_segment(&request.model);
        if model.is_empty()
            || model
                .chars()
                .any(|c| c.is_whitespace() || matches!(c, '/' | '?' | '#'))
        {
            return Err(Unsupported::new("model", reason("model")));
        }
        if request.parallel_tool_calls == Some(false) {
            return Err(Unsupported::new(
                "parallel_tool_calls",
                reason("parallel_tool_calls"),
            ));
        }
        // Every tool result must name a call in this same request — see the
        // module doc's second decision.
        let names = tool_call_names(request);
        for message in &request.messages {
            for block in &message.blocks {
                match block {
                    Block::ToolResult { tool_use_id, .. } => {
                        if !names.iter().any(|(id, _)| id == tool_use_id) {
                            return Err(Unsupported::new("tool_use_id", reason("tool_use_id")));
                        }
                    }
                    Block::Image(ImageSource::Url(_)) => {
                        return Err(Unsupported::new("image", reason("image")));
                    }
                    Block::Text(_) | Block::Image(_) | Block::ToolUse { .. } => {}
                }
            }
        }
        Ok(())
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
        Box::new(ChunkDecoder::default())
    }

    fn stream_encoder(&self) -> Box<dyn StreamEncoder + Send> {
        Box::new(ChunkEncoder::default())
    }

    /// Google's `status` vocabulary, which is what a Gemini client reads out
    /// of an error document — never the HTTP number twice.
    fn error_kind(&self, status: u16) -> &'static str {
        match status {
            400 => "INVALID_ARGUMENT",
            401 => "UNAUTHENTICATED",
            403 => "PERMISSION_DENIED",
            404 => "NOT_FOUND",
            413 => "INVALID_ARGUMENT",
            429 => "RESOURCE_EXHAUSTED",
            _ => "INTERNAL",
        }
    }

    fn encode_error(&self, kind: &str, message: &str) -> Vec<u8> {
        // `code` is null rather than a number: Google's `code` is *its* own
        // numeric status, and a refusal written here came from the gateway,
        // not from Google. Inventing one would let a reader attribute this
        // document to the provider.
        json!({"error": {"code": Value::Null, "message": message, "status": kind}})
            .to_string()
            .into_bytes()
    }

    fn encode_stream_error(&self, kind: &str, message: &str) -> Vec<u8> {
        stream::encode(
            None,
            &json!({"error": {"code": Value::Null, "message": message, "status": kind}})
                .to_string(),
        )
    }

    fn decode_error(&self, body: &[u8]) -> Option<String> {
        let value: Value = serde_json::from_slice(body).ok()?;
        // Google answers a batch of errors as a one-element array at the top
        // level; a single error is an object. Both carry `error.message`.
        let object = match &value {
            Value::Array(items) => items.first()?,
            other => other,
        };
        object
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

    fn cache_disposition(&self) -> Option<CacheDisposition> {
        Some(CacheDisposition::Stripped(
            "Gemini's caching is either implicit (an automatic prefix cache the request does \
             not address) or the explicit `cachedContent` resource this codec already refuses \
             on decode — neither is a per-request marker a translated request can set, so a \
             harness's cache_control is never encoded onto this wire",
        ))
    }
}

/// The model as a path segment: Gemini's own full resource name is
/// `models/<id>`, and a harness pointed at a Gemini provider may send either
/// spelling. The prefix is stripped rather than doubled — `/models/models/x`
/// is a `404` nobody could diagnose.
fn model_segment(model: &str) -> &str {
    model.strip_prefix("models/").unwrap_or(model)
}

/// Every `(tool-use id, tool name)` in a request, in order.
fn tool_call_names(request: &Request) -> Vec<(&str, &str)> {
    request
        .messages
        .iter()
        .flat_map(|message| &message.blocks)
        .filter_map(|block| match block {
            Block::ToolUse { id, name, .. } => Some((id.as_str(), name.as_str())),
            _ => None,
        })
        .collect()
}

// --- requests -----------------------------------------------------------------

pub(super) fn decode_request(body: &[u8]) -> Result<Request, Unsupported> {
    let value: Value = serde_json::from_slice(body)
        .map_err(|_| Unsupported::new("body", "the request body is not a JSON document"))?;
    let mut top = Fields::of(value, "")?;

    // The model is in the path on this wire, so a body that names one is
    // read if it does and left empty if it does not. `serve` re-addresses a
    // translated request from `Request::model`, and every supported pair
    // decodes some *other* protocol, so this only ever matters to a round
    // trip through this codec's own two halves.
    let model = top.take_string("model")?.unwrap_or_default();

    top.refuse_if_present("safetySettings", reason("safetySettings"))?;
    top.refuse_if_present("cachedContent", reason("cachedContent"))?;

    let system = match top.take_object("systemInstruction")? {
        Some(instruction) => Some(content_text(instruction)?),
        None => None,
    };

    let mut messages = Vec::new();
    for (index, item) in top
        .take_array("contents")?
        .unwrap_or_default()
        .into_iter()
        .enumerate()
    {
        let path = element("contents", index);
        let mut content = Fields::of(item, path)?;
        let role = content.require_string("role")?;
        let role = match role.as_str() {
            "user" => Role::User,
            "model" => Role::Assistant,
            other => {
                return Err(Unsupported::new(
                    content.at("role"),
                    format!("a Gemini content role is `user` or `model`, not `{other}`"),
                ));
            }
        };
        let blocks = decode_parts(&mut content, role)?;
        content.finish()?;
        messages.push(Message { role, blocks });
    }

    let mut tools = Vec::new();
    for (index, item) in top
        .take_array("tools")?
        .unwrap_or_default()
        .into_iter()
        .enumerate()
    {
        let mut entry = Fields::of(item, element("tools", index))?;
        entry.refuse_if_present("codeExecution", reason("codeExecution"))?;
        entry.refuse_if_present("googleSearch", reason("googleSearch"))?;
        entry.refuse_if_present("googleSearchRetrieval", reason("googleSearchRetrieval"))?;
        let declarations = entry
            .take_array("functionDeclarations")?
            .unwrap_or_default();
        let path = entry.at("functionDeclarations");
        for (position, declaration) in declarations.into_iter().enumerate() {
            let mut declaration = Fields::of(declaration, element(&path, position))?;
            let name = declaration.require_string("name")?;
            let description = declaration.take_string("description")?;
            let input_schema = match declaration.take("parameters") {
                Some(schema @ Value::Object(_)) => schema,
                None | Some(Value::Null) => json!({"type": "object", "properties": {}}),
                Some(other) => {
                    return Err(Unsupported::new(
                        declaration.at("parameters"),
                        format!(
                            "a function declaration's parameters must be a JSON object, not {}",
                            json_kind(&other)
                        ),
                    ));
                }
            };
            declaration.finish()?;
            tools.push(ToolDefinition {
                name,
                description,
                input_schema,
            });
        }
        entry.finish()?;
    }

    let tool_choice = match top.take_object("toolConfig")? {
        None => None,
        Some(mut config) => {
            let choice = match config.take_object("functionCallingConfig")? {
                None => None,
                Some(mut calling) => {
                    let mode = calling.take_string("mode")?;
                    let allowed = calling.take_array("allowedFunctionNames")?;
                    calling.finish()?;
                    let allowed_one = match allowed.as_deref() {
                        Some([Value::String(name)]) => Some(name.clone()),
                        Some([]) | None => None,
                        Some(_) => {
                            return Err(Unsupported::new(
                                "toolConfig.functionCallingConfig.allowedFunctionNames",
                                "the canonical form can force one named tool or none, not a \
                                 subset of several",
                            ));
                        }
                    };
                    match (mode.as_deref(), allowed_one) {
                        (Some("AUTO") | None, None) => Some(ToolChoice::Auto),
                        (Some("ANY"), None) => Some(ToolChoice::Any),
                        (Some("ANY"), Some(name)) => Some(ToolChoice::Tool(name)),
                        (Some("NONE"), None) => Some(ToolChoice::None),
                        (Some(other), _) => {
                            return Err(Unsupported::new(
                                "toolConfig.functionCallingConfig.mode",
                                format!(
                                    "the function-calling mode `{other}` is not one this \
                                         codec knows"
                                ),
                            ));
                        }
                        (None, Some(name)) => Some(ToolChoice::Tool(name)),
                    }
                }
            };
            config.finish()?;
            choice
        }
    };

    let (max_tokens, temperature, top_p, stop) = match top.take_object("generationConfig")? {
        None => (None, None, None, Vec::new()),
        Some(mut generation) => {
            for field in [
                "responseMimeType",
                "responseSchema",
                "responseJsonSchema",
                "thinkingConfig",
                "topK",
                "seed",
                "presencePenalty",
                "frequencyPenalty",
                "responseLogprobs",
                "logprobs",
                "responseModalities",
                "speechConfig",
            ] {
                let named = format!("generationConfig.{field}");
                generation.refuse_if_present(field, reason(&named))?;
            }
            if let Some(count) = generation.take_u64("candidateCount")?
                && count != 1
            {
                return Err(Unsupported::new(
                    generation.at("candidateCount"),
                    reason("generationConfig.candidateCount"),
                ));
            }
            let max_tokens = generation.take_u64("maxOutputTokens")?;
            let temperature = generation.take_f64("temperature")?;
            let top_p = generation.take_f64("topP")?;
            let path = generation.at("stopSequences");
            let stop = generation
                .take_array("stopSequences")?
                .unwrap_or_default()
                .into_iter()
                .enumerate()
                .map(|(index, item)| match item {
                    Value::String(text) => Ok(text),
                    other => Err(Unsupported::new(
                        element(&path, index),
                        format!(
                            "a stop sequence must be a string, not {}",
                            json_kind(&other)
                        ),
                    )),
                })
                .collect::<Result<Vec<_>, _>>()?;
            generation.finish()?;
            (max_tokens, temperature, top_p, stop)
        }
    };

    top.finish()?;

    Ok(Request {
        model,
        max_tokens,
        system,
        messages,
        tools,
        tool_choice,
        // Gemini has no parameter for it, so nothing was asked — which is
        // different from asking for parallel calls, and both are `None`
        // here for the same reason the other codecs use `None`.
        parallel_tool_calls: None,
        temperature,
        top_p,
        stop,
        // A Gemini client asks for a stream by posting to
        // `:streamGenerateContent`, not by a body field. A decoder that
        // only sees the body cannot know, and `place` has already used the
        // target: `super::serve` re-reads the target, so this is the
        // honest answer from the body alone.
        stream: false,
        user: None,
        // Gemini's own wire has no cache-hint field to decode off a
        // request, and no installed harness speaks it at the ingress
        // anyway (`NO_GEMINI_HARNESS`), so this is never asked for from
        // this side.
        cache_requested: false,
    })
}

/// A `{parts: [...]}` object that must be text — `systemInstruction`.
fn content_text(mut content: Fields) -> Result<String, Unsupported> {
    content.ignore("role");
    let path = content.at("parts");
    let mut texts = Vec::new();
    for (index, part) in content
        .take_array("parts")?
        .unwrap_or_default()
        .into_iter()
        .enumerate()
    {
        let mut part = Fields::of(part, element(&path, index))?;
        match part.take("text") {
            Some(Value::String(text)) => texts.push(text),
            None | Some(Value::Null) => {
                return Err(Unsupported::new(
                    part.at("text"),
                    "a system instruction carries text and nothing else",
                ));
            }
            Some(other) => {
                return Err(Unsupported::new(
                    part.at("text"),
                    format!("a text part must be a string, not {}", json_kind(&other)),
                ));
            }
        }
        part.finish()?;
    }
    content.finish()?;
    Ok(texts.join("\n\n"))
}

/// The content blocks of one `contents[]` entry.
fn decode_parts(content: &mut Fields, role: Role) -> Result<Vec<Block>, Unsupported> {
    let path = content.at("parts");
    let mut blocks = Vec::new();
    for (index, part) in content
        .take_array("parts")?
        .unwrap_or_default()
        .into_iter()
        .enumerate()
    {
        let mut part = Fields::of(part, element(&path, index))?;
        let before = blocks.len();
        part.refuse_if_present("fileData", reason("fileData"))?;
        part.refuse_if_present("thought", reason("thought"))?;
        part.refuse_if_present("executableCode", reason("executableCode"))?;
        part.refuse_if_present("codeExecutionResult", reason("executableCode"))?;
        if let Some(text) = part.take_string("text")? {
            blocks.push(Block::Text(text));
        }
        if let Some(mut call) = part.take_object("functionCall")? {
            let name = call.require_string("name")?;
            let input = match call.take("args") {
                Some(args @ Value::Object(_)) => args,
                None | Some(Value::Null) => json!({}),
                Some(other) => {
                    return Err(Unsupported::new(
                        call.at("args"),
                        format!(
                            "a function call's arguments must be an object, not {}",
                            json_kind(&other)
                        ),
                    ));
                }
            };
            // Gemini issues no id; the one minted here is what the harness
            // will send back, and it names the call it belongs to. See the
            // module doc's second decision.
            let id = minted_id(blocks.len(), &name);
            call.ignore("id");
            call.finish()?;
            blocks.push(Block::ToolUse { id, name, input });
        }
        if let Some(mut response) = part.take_object("functionResponse")? {
            let name = response.require_string("name")?;
            let payload = response.take("response").unwrap_or(Value::Null);
            response.ignore("id");
            response.finish()?;
            let (content, is_error) = decode_function_response(&payload);
            blocks.push(Block::ToolResult {
                // The id a decoded result carries is the one this codec
                // would have minted for the call of that name — the pair of
                // halves round-trips, and nothing else reads it.
                tool_use_id: minted_id(blocks.len(), &name),
                content,
                is_error,
            });
        }
        if let Some(mut inline) = part.take_object("inlineData")? {
            let media_type = inline.require_string("mimeType")?;
            let data = inline.require_string("data")?;
            inline.finish()?;
            blocks.push(Block::Image(ImageSource::Base64 { media_type, data }));
        }
        let path = part.path().to_owned();
        // Whatever nobody read, refused by name — and a part that read as
        // NOTHING refused by its own path. An empty or wholly unrecognised
        // part would otherwise be the one thing this codec drops in silence,
        // which is the rule the whole module is built on.
        part.finish()?;
        if blocks.len() == before {
            return Err(Unsupported::new(
                path,
                "a Gemini part must carry text, functionCall, functionResponse or inlineData; \
                 this one carried none of them",
            ));
        }
    }
    // A model turn with no parts at all is a shape the wire allows and the
    // canonical form carries as an empty assistant message, exactly as
    // `openai_chat` carries an assistant message with null content.
    let _ = role;
    Ok(blocks)
}

/// The id this codec mints for a function call — see the module doc.
fn minted_id(index: usize, name: &str) -> String {
    format!("gemini-call-{index}-{name}")
}

/// A `functionResponse.response` as the canonical form's text plus its error
/// flag.
///
/// Gemini's `response` is an arbitrary object. This codec writes
/// `{"output": <text>}` for a success and `{"error": <text>}` for a failure,
/// which is Google's own documented convention, and reads both back. Any
/// other object is carried as its JSON text rather than refused: a tool
/// result is content the model reads, not a field the codec has to
/// understand.
fn decode_function_response(payload: &Value) -> (String, bool) {
    match payload {
        Value::Object(map) => match (map.get("output"), map.get("error")) {
            (_, Some(error)) => (text_of(error), true),
            (Some(output), None) => (text_of(output), false),
            (None, None) => (payload.to_string(), false),
        },
        other => (text_of(other), false),
    }
}

fn text_of(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        other => other.to_string(),
    }
}

pub(super) fn encode_request(request: &Request) -> Vec<u8> {
    let names = tool_call_names(request);
    let mut contents = Vec::new();
    for message in &request.messages {
        let role = match message.role {
            Role::User => "user",
            Role::Assistant => "model",
        };
        let mut parts = Vec::new();
        for block in &message.blocks {
            match block {
                Block::Text(text) => parts.push(json!({"text": text})),
                Block::Image(ImageSource::Base64 { media_type, data }) => {
                    parts.push(json!({"inlineData": {"mimeType": media_type, "data": data}}));
                }
                // Refused before this point by `refuse_unencodable`; carried
                // as its URL rather than dropped, so that a future caller
                // reaching here without the refusal still loses nothing.
                Block::Image(ImageSource::Url(url)) => parts.push(json!({"text": url})),
                Block::ToolUse { name, input, .. } => {
                    parts.push(json!({"functionCall": {"name": name, "args": input}}));
                }
                Block::ToolResult {
                    tool_use_id,
                    content,
                    is_error,
                } => {
                    // Resolved through the call's own block — see the module
                    // doc. `refuse_unencodable` has already refused an id
                    // with no such block, so the fallback below is
                    // unreachable from `serve`; it names the id rather than
                    // inventing a tool name for a caller that skipped it.
                    let name = names
                        .iter()
                        .find(|(id, _)| *id == tool_use_id)
                        .map(|(_, name)| *name)
                        .unwrap_or(tool_use_id.as_str());
                    let payload = if *is_error {
                        json!({"error": content})
                    } else {
                        json!({"output": content})
                    };
                    parts.push(json!({
                        "functionResponse": {"name": name, "response": payload},
                    }));
                }
            }
        }
        contents.push(json!({"role": role, "parts": Value::Array(parts)}));
    }

    let mut document = Map::new();
    document.insert("contents".to_owned(), Value::Array(contents));
    if let Some(system) = &request.system {
        document.insert(
            "systemInstruction".to_owned(),
            json!({"parts": [{"text": system}]}),
        );
    }
    if !request.tools.is_empty() {
        let declarations: Vec<Value> = request
            .tools
            .iter()
            .map(|tool| {
                let mut declaration = Map::new();
                declaration.insert("name".to_owned(), json!(tool.name));
                if let Some(description) = &tool.description {
                    declaration.insert("description".to_owned(), json!(description));
                }
                // Carried as given. Gemini's schema dialect is a subset of
                // JSON Schema, so a tool declaring a keyword it rejects is
                // answered `400` by Google and that error reaches the
                // harness in its own shape — rewriting the schema here
                // would change what the harness declared.
                declaration.insert("parameters".to_owned(), tool.input_schema.clone());
                Value::Object(declaration)
            })
            .collect();
        document.insert(
            "tools".to_owned(),
            json!([{"functionDeclarations": Value::Array(declarations)}]),
        );
    }
    if let Some(choice) = &request.tool_choice {
        document.insert(
            "toolConfig".to_owned(),
            match choice {
                ToolChoice::Auto => json!({"functionCallingConfig": {"mode": "AUTO"}}),
                ToolChoice::Any => json!({"functionCallingConfig": {"mode": "ANY"}}),
                ToolChoice::None => json!({"functionCallingConfig": {"mode": "NONE"}}),
                // ANY plus one allowed name is how this wire says "call
                // exactly this tool"; there is no `mode` that means it.
                ToolChoice::Tool(name) => json!({
                    "functionCallingConfig": {"mode": "ANY", "allowedFunctionNames": [name]},
                }),
            },
        );
    }
    let mut generation = Map::new();
    if let Some(max_tokens) = request.max_tokens {
        generation.insert("maxOutputTokens".to_owned(), json!(max_tokens));
    }
    if let Some(temperature) = request.temperature {
        generation.insert("temperature".to_owned(), json!(temperature));
    }
    if let Some(top_p) = request.top_p {
        generation.insert("topP".to_owned(), json!(top_p));
    }
    if !request.stop.is_empty() {
        generation.insert("stopSequences".to_owned(), json!(request.stop));
    }
    if !generation.is_empty() {
        document.insert("generationConfig".to_owned(), Value::Object(generation));
    }
    Value::Object(document).to_string().into_bytes()
}

// --- responses ----------------------------------------------------------------

pub(super) fn decode_response(body: &[u8]) -> Result<Response, Unsupported> {
    let value: Value = serde_json::from_slice(body)
        .map_err(|_| Unsupported::new("body", "the response body is not a JSON document"))?;
    let mut top = Fields::of(value, "")?;
    // `responseId` on current versions and nothing on older ones. Empty
    // rather than minted: an id nobody issued is indistinguishable from one
    // somebody did, and the harness only echoes it.
    let id = top.take_string("responseId")?.unwrap_or_default();
    let model = top.take_string("modelVersion")?.unwrap_or_default();
    top.ignore("promptFeedback");
    let usage = match top.take_object("usageMetadata")? {
        Some(usage) => decode_usage(usage)?,
        None => Usage::default(),
    };
    let mut candidates = top.take_array("candidates")?.unwrap_or_default();
    if candidates.len() != 1 {
        return Err(Unsupported::new(
            "candidates",
            format!(
                "exactly one candidate is translated; the provider returned {}",
                candidates.len()
            ),
        ));
    }
    let mut candidate = Fields::of(candidates.remove(0), "candidates[0]")?;
    for ignored in [
        "index",
        "safetyRatings",
        "citationMetadata",
        "groundingMetadata",
        "avgLogprobs",
        "logprobsResult",
        "tokenCount",
        "finishMessage",
        "urlContextMetadata",
    ] {
        candidate.ignore(ignored);
    }
    let finish = candidate.require_string("finishReason")?;
    let blocks = match candidate.take_object("content")? {
        Some(mut content) => {
            let blocks = decode_parts(&mut content, Role::Assistant)?;
            content.ignore("role");
            content.finish()?;
            blocks
        }
        None => Vec::new(),
    };
    candidate.finish()?;
    top.finish()?;
    let stop_reason = decode_finish_reason(&finish, "candidates[0].finishReason", &blocks)?;
    Ok(Response {
        id,
        model,
        blocks,
        // Gemini names no matched stop sequence, and the canonical form's
        // `None` is "the provider did not say" rather than "none matched".
        stop_sequence: None,
        stop_reason,
        usage,
    })
}

/// Gemini's `finishReason`, plus the content, as a canonical stop reason.
///
/// The content is an argument because of the module doc's third decision: a
/// candidate made entirely of function calls still finishes `STOP`, and a
/// harness told `end_turn` there stops instead of running the tool.
fn decode_finish_reason(
    finish: &str,
    path: &str,
    blocks: &[Block],
) -> Result<StopReason, Unsupported> {
    let calls_a_tool = blocks
        .iter()
        .any(|block| matches!(block, Block::ToolUse { .. }));
    Ok(match finish {
        "STOP" if calls_a_tool => StopReason::ToolUse,
        "STOP" => StopReason::EndTurn,
        "MAX_TOKENS" => StopReason::MaxTokens,
        "SAFETY" | "RECITATION" | "LANGUAGE" | "BLOCKLIST" | "PROHIBITED_CONTENT" | "SPII"
        | "IMAGE_SAFETY" => StopReason::Refusal,
        // Each of these says the answer went wrong in a way the canonical
        // vocabulary has no word for. `end_turn` would tell the harness a
        // broken answer finished normally, so they are refused by name and
        // the harness is told the provider's answer could not be carried.
        other => {
            return Err(Unsupported::new(
                path.to_owned(),
                format!(
                    "the finish reason `{other}` is not one this codec can carry: the \
                         canonical form has no stop reason that would not misdescribe it"
                ),
            ));
        }
    })
}

fn finish_reason_json(reason: StopReason) -> Value {
    json!(match reason {
        // Gemini says `STOP` for both, and for a candidate of function
        // calls too — the decoder above is where that ambiguity is resolved
        // on the way back.
        StopReason::EndTurn | StopReason::StopSequence | StopReason::ToolUse => "STOP",
        StopReason::MaxTokens => "MAX_TOKENS",
        StopReason::Refusal => "SAFETY",
    })
}

fn decode_usage(mut usage: Fields) -> Result<Usage, Unsupported> {
    let prompt = usage.take_u64("promptTokenCount")?.unwrap_or(0);
    let candidates = usage.take_u64("candidatesTokenCount")?.unwrap_or(0);
    // Reasoning tokens are reported apart from `candidatesTokenCount` and
    // are output the provider generated and charged for; the canonical
    // `output` is what was produced, so they are summed rather than
    // ignored. `cachedContentTokenCount` is a SUBSET of the prompt count,
    // exactly as OpenAI's `cached_tokens` is a subset of `prompt_tokens`.
    let thoughts = usage.take_u64("thoughtsTokenCount")?.unwrap_or(0);
    let cached = usage.take_u64("cachedContentTokenCount")?;
    for ignored in [
        "totalTokenCount",
        "promptTokensDetails",
        "candidatesTokensDetails",
        "cacheTokensDetails",
        "toolUsePromptTokenCount",
        "toolUsePromptTokensDetails",
        "thoughtsTokensDetails",
    ] {
        usage.ignore(ignored);
    }
    usage.finish()?;
    Ok(Usage {
        input: prompt.saturating_sub(cached.unwrap_or(0)),
        output: candidates.saturating_add(thoughts),
        cached,
    })
}

fn usage_json(usage: &Usage) -> Value {
    let prompt = usage.input + usage.cached.unwrap_or(0);
    let mut entry = Map::new();
    entry.insert("promptTokenCount".to_owned(), json!(prompt));
    entry.insert("candidatesTokenCount".to_owned(), json!(usage.output));
    entry.insert("totalTokenCount".to_owned(), json!(prompt + usage.output));
    if let Some(cached) = usage.cached {
        entry.insert("cachedContentTokenCount".to_owned(), json!(cached));
    }
    Value::Object(entry)
}

/// The `parts` of one assistant answer.
fn response_parts(blocks: &[Block]) -> Vec<Value> {
    blocks
        .iter()
        .filter_map(|block| match block {
            Block::Text(text) => Some(json!({"text": text})),
            Block::ToolUse { name, input, .. } => {
                Some(json!({"functionCall": {"name": name, "args": input}}))
            }
            // `decode_response` on no codec produces these in an answer.
            Block::Image(_) | Block::ToolResult { .. } => None,
        })
        .collect()
}

pub(super) fn encode_response(response: &Response) -> Vec<u8> {
    let mut document = Map::new();
    document.insert(
        "candidates".to_owned(),
        json!([{
            "content": {"role": "model", "parts": Value::Array(response_parts(&response.blocks))},
            "finishReason": finish_reason_json(response.stop_reason),
            "index": 0,
        }]),
    );
    document.insert("usageMetadata".to_owned(), usage_json(&response.usage));
    document.insert("modelVersion".to_owned(), json!(response.model));
    if !response.id.is_empty() {
        document.insert("responseId".to_owned(), json!(response.id));
    }
    Value::Object(document).to_string().into_bytes()
}

// --- streams ------------------------------------------------------------------

/// Which canonical block is open.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Open {
    Text,
    Tool,
}

/// Turns `streamGenerateContent` chunks into the canonical event order.
///
/// See the module doc's fifth decision and its table for what is emitted at
/// which chunk, and for why `finishReason` is the terminator.
#[derive(Default)]
struct ChunkDecoder {
    started: bool,
    open: Option<(Open, usize)>,
    next_index: usize,
    minted: usize,
    finish: Option<StopReason>,
    /// Whether any chunk's candidate carried a function call — the third
    /// decision, applied across the whole stream rather than one chunk.
    calls_a_tool: bool,
    /// The raw `finishReason` text, held so the stop reason can be decided
    /// once the whole stream's content is known.
    finish_text: Option<String>,
    usage: Usage,
    done: bool,
}

impl ChunkDecoder {
    fn close_open(&mut self, events: &mut Vec<StreamEvent>) {
        if let Some((_, index)) = self.open.take() {
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

impl StreamDecoder for ChunkDecoder {
    fn feed(&mut self, event: &SseEvent) -> Result<Vec<StreamEvent>, Unsupported> {
        if self.done {
            return Ok(Vec::new());
        }
        let data = event.data.trim();
        if data.is_empty() {
            return Ok(Vec::new());
        }
        let value: Value = serde_json::from_str(data)
            .map_err(|_| Unsupported::new("chunk", "a stream chunk was not a JSON document"))?;
        let mut top = Fields::of(value, "")?;
        let mut events = Vec::new();
        let id = top.take_string("responseId")?.unwrap_or_default();
        let model = top.take_string("modelVersion")?.unwrap_or_default();
        top.ignore("promptFeedback");
        if let Some(usage) = top.take_object("usageMetadata")? {
            self.usage = decode_usage(usage)?;
        }
        if !self.started {
            self.started = true;
            events.push(StreamEvent::MessageStart {
                id,
                model,
                usage: Usage {
                    input: self.usage.input,
                    output: 0,
                    cached: self.usage.cached,
                },
            });
        }
        for (index, candidate) in top
            .take_array("candidates")?
            .unwrap_or_default()
            .into_iter()
            .enumerate()
        {
            let path = element("candidates", index);
            let mut candidate = Fields::of(candidate, path)?;
            if candidate.take_u64("index")?.unwrap_or(0) != 0 {
                return Err(Unsupported::new(
                    candidate.at("index"),
                    reason("generationConfig.candidateCount"),
                ));
            }
            for ignored in [
                "safetyRatings",
                "citationMetadata",
                "groundingMetadata",
                "avgLogprobs",
                "logprobsResult",
                "tokenCount",
                "finishMessage",
                "urlContextMetadata",
            ] {
                candidate.ignore(ignored);
            }
            if let Some(finish) = candidate.take_string("finishReason")? {
                self.finish_text = Some(finish);
            }
            let Some(mut content) = candidate.take_object("content")? else {
                candidate.finish()?;
                continue;
            };
            candidate.finish()?;
            let blocks = decode_parts(&mut content, Role::Assistant)?;
            content.ignore("role");
            content.finish()?;
            for block in blocks {
                match block {
                    Block::Text(text) => {
                        if text.is_empty() {
                            continue;
                        }
                        let index = match self.open {
                            Some((Open::Text, index)) => index,
                            _ => {
                                let index = self.open_block(BlockStart::Text, &mut events);
                                self.open = Some((Open::Text, index));
                                index
                            }
                        };
                        events.push(StreamEvent::BlockDelta {
                            index,
                            delta: Delta::Text(text),
                        });
                    }
                    Block::ToolUse { name, input, .. } => {
                        self.calls_a_tool = true;
                        // The block index this codec mints ids from is the
                        // stream's own count, not the chunk's, so two calls
                        // in two chunks never share an id.
                        let id = minted_id(self.minted, &name);
                        self.minted += 1;
                        let index = self.open_block(
                            BlockStart::ToolUse {
                                id,
                                name: name.clone(),
                            },
                            &mut events,
                        );
                        self.open = Some((Open::Tool, index));
                        // Gemini sends a call's arguments whole; there is
                        // nothing to fragment and nothing to wait for.
                        events.push(StreamEvent::BlockDelta {
                            index,
                            delta: Delta::InputJson(input.to_string()),
                        });
                    }
                    // A streamed answer carries neither, and `decode_parts`
                    // only produces them from a request's own shapes.
                    Block::Image(_) | Block::ToolResult { .. } => {}
                }
            }
        }
        top.finish()?;
        Ok(events)
    }

    fn finish(&mut self) -> Result<Vec<StreamEvent>, Unsupported> {
        if self.done {
            return Ok(Vec::new());
        }
        if !self.started {
            return Err(Unsupported::new(
                "chunk",
                "the provider's stream ended before it sent a single chunk",
            ));
        }
        // `streamGenerateContent` has no `[DONE]`, so `finishReason` is the
        // terminator — see the module doc's fifth decision. Without one the
        // stream was cut, and completing it here would hand the harness a
        // partial answer wearing a stop reason nobody sent.
        let Some(finish) = self.finish_text.clone() else {
            return Err(Unsupported::new(
                "candidates[0].finishReason",
                "the provider's stream ended without a finish reason, so the message it \
                 delivered is truncated and not finished",
            ));
        };
        let calls = if self.calls_a_tool {
            vec![Block::ToolUse {
                id: String::new(),
                name: String::new(),
                input: Value::Null,
            }]
        } else {
            Vec::new()
        };
        self.finish = Some(decode_finish_reason(
            &finish,
            "candidates[0].finishReason",
            &calls,
        )?);
        self.done = true;
        let mut events = Vec::new();
        self.close_open(&mut events);
        events.push(StreamEvent::MessageDelta {
            stop_reason: self
                .finish
                .expect("set immediately above, or this line is not reached"),
            stop_sequence: None,
            usage: self.usage,
        });
        events.push(StreamEvent::MessageStop);
        Ok(events)
    }

    fn is_done(&self) -> bool {
        self.done
    }
}

/// Turns the canonical event order into `streamGenerateContent` chunks.
///
/// Reachable only when a harness speaks Gemini at the ingress, which no
/// installed harness does — every `gemini-generate-content -> …` row is
/// refused (T3b). It is written and tested anyway, because a codec whose
/// halves do not agree is a codec that cannot be trusted in the direction
/// that *is* used.
///
/// One property differs from the other encoders: Gemini's wire has **no
/// partial-arguments event**, so a tool call's `InputJson` fragments are
/// held until its block stops and the whole `functionCall` is written then.
/// That is a property of the wire, not a buffering choice — there is no
/// shape in which a fragment could be sent.
#[derive(Default)]
struct ChunkEncoder {
    id: String,
    model: String,
    open_tool: Option<(String, String)>,
    pending_args: String,
}

impl ChunkEncoder {
    fn chunk(&self, parts: Vec<Value>, finish: Option<Value>, usage: Option<Value>) -> Vec<u8> {
        let mut candidate = Map::new();
        candidate.insert(
            "content".to_owned(),
            json!({"role": "model", "parts": Value::Array(parts)}),
        );
        candidate.insert("index".to_owned(), json!(0));
        if let Some(finish) = finish {
            candidate.insert("finishReason".to_owned(), finish);
        }
        let mut document = Map::new();
        document.insert(
            "candidates".to_owned(),
            Value::Array(vec![Value::Object(candidate)]),
        );
        document.insert("modelVersion".to_owned(), json!(self.model));
        if !self.id.is_empty() {
            document.insert("responseId".to_owned(), json!(self.id));
        }
        if let Some(usage) = usage {
            document.insert("usageMetadata".to_owned(), usage);
        }
        stream::encode(None, &Value::Object(document).to_string())
    }
}

impl StreamEncoder for ChunkEncoder {
    fn encode(&mut self, event: &StreamEvent) -> Vec<u8> {
        match event {
            StreamEvent::MessageStart { id, model, .. } => {
                self.id = id.clone();
                self.model = model.clone();
                // Gemini's wire has no start event: the first chunk a client
                // sees is the first one carrying content.
                Vec::new()
            }
            StreamEvent::BlockStart {
                block: BlockStart::ToolUse { id, name },
                ..
            } => {
                self.open_tool = Some((id.clone(), name.clone()));
                self.pending_args.clear();
                Vec::new()
            }
            StreamEvent::BlockStart {
                block: BlockStart::Text,
                ..
            } => Vec::new(),
            StreamEvent::BlockDelta {
                delta: Delta::Text(text),
                ..
            } => self.chunk(vec![json!({"text": text})], None, None),
            StreamEvent::BlockDelta {
                delta: Delta::InputJson(partial),
                ..
            } => {
                self.pending_args.push_str(partial);
                Vec::new()
            }
            StreamEvent::BlockStop { .. } => match self.open_tool.take() {
                None => Vec::new(),
                Some((_, name)) => {
                    let args = parse_tool_input(&self.pending_args).unwrap_or(json!({}));
                    self.pending_args.clear();
                    self.chunk(
                        vec![json!({"functionCall": {"name": name, "args": args}})],
                        None,
                        None,
                    )
                }
            },
            StreamEvent::MessageDelta {
                stop_reason, usage, ..
            } => self.chunk(
                Vec::new(),
                Some(finish_reason_json(*stop_reason)),
                Some(usage_json(usage)),
            ),
            // The socket closing is this wire's end of stream.
            StreamEvent::MessageStop => Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::gateway::translate::canonical;

    fn decode(body: &str) -> Request {
        decode_request(body.as_bytes()).expect("a well-formed Gemini request")
    }

    fn encoded(request: &Request) -> Value {
        serde_json::from_slice(&encode_request(request)).expect("the codec writes JSON")
    }

    /// A canonical request after one tool round, the shape every supported
    /// pair hands this codec.
    fn tool_round() -> Request {
        Request {
            model: "gemini-2.5-pro".to_owned(),
            max_tokens: Some(4096),
            system: Some("You are terse.".to_owned()),
            messages: vec![
                Message {
                    role: Role::User,
                    blocks: vec![Block::Text("List the files.".to_owned())],
                },
                Message {
                    role: Role::Assistant,
                    blocks: vec![
                        Block::Text("Sure.".to_owned()),
                        Block::ToolUse {
                            id: "call_prior_1".to_owned(),
                            name: "Bash".to_owned(),
                            input: json!({"command": "ls /nope"}),
                        },
                    ],
                },
                Message {
                    role: Role::User,
                    blocks: vec![
                        Block::ToolResult {
                            tool_use_id: "call_prior_1".to_owned(),
                            content: "ls: cannot access '/nope'".to_owned(),
                            is_error: true,
                        },
                        Block::Text("Try again.".to_owned()),
                    ],
                },
            ],
            tools: vec![ToolDefinition {
                name: "Bash".to_owned(),
                description: Some("Run a shell command".to_owned()),
                input_schema: json!({"type": "object", "properties": {"command": {"type": "string"}}}),
            }],
            tool_choice: Some(ToolChoice::Auto),
            parallel_tool_calls: None,
            temperature: None,
            top_p: None,
            stop: Vec::new(),
            stream: false,
            user: Some("user_abc".to_owned()),
            cache_requested: false,
        }
    }

    /// The module doc's second decision: a tool result becomes a
    /// `functionResponse` under the NAME of the call it answers, resolved
    /// through the tool-use block carrying its id.
    #[test]
    fn a_tool_result_is_matched_to_its_call_by_the_name_the_id_resolves_to() {
        let request = tool_round();
        assert_eq!(Gemini.refuse_unencodable(&request), Ok(()));
        let sent = encoded(&request);
        let contents = sent["contents"].as_array().expect("contents");
        assert_eq!(
            contents[0],
            json!({"role": "user", "parts": [{"text": "List the files."}]})
        );
        assert_eq!(contents[1]["role"], "model");
        assert_eq!(
            contents[1]["parts"],
            json!([
                {"text": "Sure."},
                {"functionCall": {"name": "Bash", "args": {"command": "ls /nope"}}},
            ])
        );
        assert_eq!(contents[2]["role"], "user");
        assert_eq!(
            contents[2]["parts"][0],
            json!({"functionResponse": {
                "name": "Bash",
                "response": {"error": "ls: cannot access '/nope'"},
            }}),
            "the result names the call's function, and an erroring result says so"
        );
        assert_eq!(contents[2]["parts"][1], json!({"text": "Try again."}));
        assert_eq!(
            sent["systemInstruction"],
            json!({"parts": [{"text": "You are terse."}]})
        );
        assert_eq!(sent["generationConfig"], json!({"maxOutputTokens": 4096}));
        assert_eq!(
            sent["toolConfig"],
            json!({"functionCallingConfig": {"mode": "AUTO"}})
        );
        assert_eq!(
            sent["tools"],
            json!([{"functionDeclarations": [{
                "name": "Bash",
                "description": "Run a shell command",
                "parameters": {"type": "object", "properties": {"command": {"type": "string"}}},
            }]}])
        );
        // The one dropped request field, dropped by name and listed.
        assert_eq!(sent.get("user"), None);
        assert!(IGNORED_FIELDS.contains(&"user"));
    }

    /// A result whose id names no call in the same request is refused by
    /// name, before anything is opened upstream — never written under some
    /// other tool's name.
    #[test]
    fn a_tool_result_whose_id_names_no_call_is_refused_rather_than_guessed() {
        let mut request = tool_round();
        request.messages[1].blocks.truncate(1);
        let refusal = Gemini
            .refuse_unencodable(&request)
            .expect_err("the call this result answers is gone");
        assert_eq!(refusal.field, "tool_use_id");
        assert!(refusal.reason.contains("by NAME"), "{}", refusal.reason);
    }

    /// The model is a path segment on this wire, so a name that could not
    /// be one is refused rather than smuggled into the request line — and
    /// Gemini's own `models/<id>` spelling is not doubled.
    #[test]
    fn the_model_addresses_the_path_and_a_name_a_path_cannot_carry_is_refused() {
        let mut request = tool_round();
        assert_eq!(
            Gemini.outbound_endpoint(&request),
            "/v1beta/models/gemini-2.5-pro:generateContent",
            "the version segment is the codec's, not the base URL's, or a relayed target \
             composes it twice"
        );
        request.stream = true;
        assert_eq!(
            Gemini.outbound_endpoint(&request),
            "/v1beta/models/gemini-2.5-pro:streamGenerateContent?alt=sse",
            "without alt=sse Google answers a JSON array rather than server-sent events"
        );
        request.model = "models/gemini-2.5-flash".to_owned();
        assert_eq!(
            Gemini.outbound_endpoint(&request),
            "/v1beta/models/gemini-2.5-flash:streamGenerateContent?alt=sse"
        );
        for bad in ["a/b", "a?b", "a#b", "a b", ""] {
            let mut request = tool_round();
            request.model = bad.to_owned();
            let refusal = Gemini
                .refuse_unencodable(&request)
                .expect_err("a model a path segment cannot carry");
            assert_eq!(refusal.field, "model", "for {bad:?}");
        }
    }

    /// The module doc's third decision, which is the one that keeps a
    /// harness's tooling working: `STOP` beside a function call is
    /// `tool_use`, not `end_turn`.
    #[test]
    fn a_candidate_of_function_calls_stops_for_tool_use_even_though_gemini_says_stop() {
        let body = json!({
            "candidates": [{
                "content": {"role": "model", "parts": [
                    {"text": "Checking."},
                    {"functionCall": {"name": "Bash", "args": {"command": "ls"}}},
                ]},
                "finishReason": "STOP",
                "index": 0,
            }],
            "usageMetadata": {
                "promptTokenCount": 40,
                "candidatesTokenCount": 10,
                "thoughtsTokenCount": 2,
                "cachedContentTokenCount": 8,
                "totalTokenCount": 52,
            },
            "modelVersion": "gemini-2.5-pro-001",
            "responseId": "resp-fixture",
        })
        .to_string();
        let response = decode_response(body.as_bytes()).expect("a well-formed answer");
        assert_eq!(response.stop_reason, StopReason::ToolUse);
        assert_eq!(response.id, "resp-fixture");
        assert_eq!(response.model, "gemini-2.5-pro-001");
        assert_eq!(
            response.blocks[1],
            Block::ToolUse {
                id: "gemini-call-1-Bash".to_owned(),
                name: "Bash".to_owned(),
                input: json!({"command": "ls"}),
            }
        );
        assert_eq!(
            response.usage,
            Usage {
                input: 32,
                output: 12,
                cached: Some(8),
            },
            "the prompt count includes the cached tokens and the output count includes the \
             reasoning ones"
        );

        // ... and the same reason with no call is an ordinary end of turn.
        let plain = json!({
            "candidates": [{
                "content": {"role": "model", "parts": [{"text": "Done."}]},
                "finishReason": "STOP",
            }],
        })
        .to_string();
        assert_eq!(
            decode_response(plain.as_bytes()).unwrap().stop_reason,
            StopReason::EndTurn
        );
    }

    /// A finish reason the canonical vocabulary has no honest word for is
    /// refused by name rather than reported as a normal end of turn.
    #[test]
    fn a_finish_reason_with_no_honest_canonical_word_is_refused_by_name() {
        for finish in ["MALFORMED_FUNCTION_CALL", "OTHER", "UNEXPECTED_TOOL_CALL"] {
            let body = json!({
                "candidates": [{
                    "content": {"role": "model", "parts": [{"text": ""}]},
                    "finishReason": finish,
                }],
            })
            .to_string();
            let refusal = decode_response(body.as_bytes())
                .expect_err("no canonical stop reason describes this");
            assert_eq!(refusal.field, "candidates[0].finishReason");
            assert!(refusal.reason.contains(finish), "{}", refusal.reason);
        }
        for (finish, expected) in [
            ("MAX_TOKENS", StopReason::MaxTokens),
            ("SAFETY", StopReason::Refusal),
            ("PROHIBITED_CONTENT", StopReason::Refusal),
        ] {
            let body = json!({
                "candidates": [{
                    "content": {"role": "model", "parts": [{"text": "x"}]},
                    "finishReason": finish,
                }],
            })
            .to_string();
            assert_eq!(
                decode_response(body.as_bytes()).unwrap().stop_reason,
                expected
            );
        }
    }

    /// Nothing is dropped silently: a field this codec does not carry is
    /// refused by its own path.
    #[test]
    fn an_unknown_or_unsupported_field_is_refused_by_its_full_path() {
        for (body, field) in [
            (
                json!({"contents": [], "safetySettings": [{"category": "HARM_CATEGORY_HARASSMENT"}]}),
                "safetySettings",
            ),
            (
                json!({"contents": [], "cachedContent": "x"}),
                "cachedContent",
            ),
            (
                json!({"contents": [], "generationConfig": {"topK": 4}}),
                "generationConfig.topK",
            ),
            (
                json!({"contents": [], "generationConfig": {"thinkingConfig": {"thinkingBudget": 1}}}),
                "generationConfig.thinkingConfig",
            ),
            (
                json!({"contents": [], "someFutureField": 1}),
                "someFutureField",
            ),
            (
                json!({"contents": [{"role": "user", "parts": [{"videoMetadata": {"fps": 1}}]}]}),
                "contents[0].parts[0].videoMetadata",
            ),
            // A part that reads as nothing at all: the one silent drop this
            // codec could have had, refused by the part's own path.
            (
                json!({"contents": [{"role": "user", "parts": [{}]}]}),
                "contents[0].parts[0]",
            ),
            (
                json!({"contents": [{"role": "user", "parts": [{"fileData": {"fileUri": "x"}}]}]}),
                "contents[0].parts[0].fileData",
            ),
            (
                json!({"contents": [{"role": "system", "parts": [{"text": "x"}]}]}),
                "contents[0].role",
            ),
        ] {
            let refusal = decode_request(body.to_string().as_bytes())
                .expect_err("this field has no home in the canonical form");
            assert_eq!(refusal.field, field);
        }
    }

    /// The two halves agree: a request written by this codec decodes back to
    /// what produced it, tool round included.
    #[test]
    fn a_request_written_by_this_codec_decodes_back_to_the_same_form() {
        let request = tool_round();
        let bytes = encode_request(&request);
        let back = decode_request(&bytes).expect("its own output");
        assert_eq!(back.system, request.system);
        assert_eq!(back.tools, request.tools);
        assert_eq!(back.tool_choice, request.tool_choice);
        assert_eq!(back.max_tokens, request.max_tokens);
        assert_eq!(back.messages[0], request.messages[0]);
        assert_eq!(back.messages[1], request.messages[1].clone_with_minted_id());
        // The tool result comes back matched to the call by name, under the
        // id this codec mints for a call of that name at that position.
        let Block::ToolResult {
            content, is_error, ..
        } = &back.messages[2].blocks[0]
        else {
            panic!("a tool result, got {:?}", back.messages[2].blocks[0]);
        };
        assert_eq!(content, "ls: cannot access '/nope'");
        assert!(is_error, "the error flag survives the round trip");
    }

    /// A forced tool choice round-trips through the one shape this wire has
    /// for it.
    #[test]
    fn a_forced_tool_choice_is_any_plus_one_allowed_name() {
        let mut request = tool_round();
        request.tool_choice = Some(ToolChoice::Tool("Bash".to_owned()));
        let sent = encoded(&request);
        assert_eq!(
            sent["toolConfig"],
            json!({"functionCallingConfig": {"mode": "ANY", "allowedFunctionNames": ["Bash"]}})
        );
        let back = decode(&String::from_utf8(encode_request(&request)).unwrap());
        assert_eq!(back.tool_choice, Some(ToolChoice::Tool("Bash".to_owned())));

        request.tool_choice = Some(ToolChoice::Any);
        assert_eq!(
            encoded(&request)["toolConfig"],
            json!({"functionCallingConfig": {"mode": "ANY"}})
        );
        request.tool_choice = Some(ToolChoice::None);
        assert_eq!(
            encoded(&request)["toolConfig"],
            json!({"functionCallingConfig": {"mode": "NONE"}})
        );
    }

    /// Gemini has no parameter that disables parallel function calling, so a
    /// request that asked for it is refused rather than answered as though
    /// it had not asked.
    #[test]
    fn a_request_that_disabled_parallel_tool_calls_is_refused_by_name() {
        let mut request = tool_round();
        request.parallel_tool_calls = Some(false);
        let refusal = Gemini
            .refuse_unencodable(&request)
            .expect_err("this wire cannot honour it");
        assert_eq!(refusal.field, "parallel_tool_calls");
        request.parallel_tool_calls = Some(true);
        assert_eq!(
            Gemini.refuse_unencodable(&request),
            Ok(()),
            "parallel calling is this wire's own default, so asking for it asks for nothing"
        );
    }

    fn sse(data: &Value) -> SseEvent {
        SseEvent {
            event: None,
            data: data.to_string(),
        }
    }

    /// The stream table in the module doc, as behaviour: the first chunk
    /// starts the message, a text chunk delivers its fragment, a function
    /// call arrives whole, and the message's own delta waits for the end.
    #[test]
    fn a_stream_becomes_the_canonical_order_one_chunk_at_a_time() {
        let mut decoder = ChunkDecoder::default();
        let first = decoder
            .feed(&sse(&json!({
                "candidates": [{"content": {"role": "model", "parts": [{"text": "Check"}]}}],
                "modelVersion": "gemini-2.5-pro",
                "responseId": "resp-1",
            })))
            .expect("a well-formed chunk");
        assert_eq!(
            first,
            vec![
                StreamEvent::MessageStart {
                    id: "resp-1".to_owned(),
                    model: "gemini-2.5-pro".to_owned(),
                    usage: Usage::default(),
                },
                StreamEvent::BlockStart {
                    index: 0,
                    block: BlockStart::Text,
                },
                StreamEvent::BlockDelta {
                    index: 0,
                    delta: Delta::Text("Check".to_owned()),
                },
            ]
        );
        let second = decoder
            .feed(&sse(&json!({
                "candidates": [{"content": {"role": "model", "parts": [{"text": "ing."}]}}],
            })))
            .expect("a well-formed chunk");
        assert_eq!(
            second,
            vec![StreamEvent::BlockDelta {
                index: 0,
                delta: Delta::Text("ing.".to_owned()),
            }],
            "a fragment leaves on the chunk that carried it, with no new block"
        );
        let third = decoder
            .feed(&sse(&json!({
                "candidates": [{
                    "content": {"role": "model", "parts": [
                        {"functionCall": {"name": "Bash", "args": {"command": "ls"}}},
                    ]},
                    "finishReason": "STOP",
                }],
                "usageMetadata": {"promptTokenCount": 40, "candidatesTokenCount": 12},
            })))
            .expect("a well-formed chunk");
        assert_eq!(
            third,
            vec![
                StreamEvent::BlockStop { index: 0 },
                StreamEvent::BlockStart {
                    index: 1,
                    block: BlockStart::ToolUse {
                        id: "gemini-call-0-Bash".to_owned(),
                        name: "Bash".to_owned(),
                    },
                },
                StreamEvent::BlockDelta {
                    index: 1,
                    delta: Delta::InputJson("{\"command\":\"ls\"}".to_owned()),
                },
            ],
            "the finish reason is held for the message's own delta"
        );
        assert!(!decoder.is_done());
        let end = decoder.finish().expect("the stream finished");
        assert_eq!(
            end,
            vec![
                StreamEvent::BlockStop { index: 1 },
                StreamEvent::MessageDelta {
                    stop_reason: StopReason::ToolUse,
                    stop_sequence: None,
                    usage: Usage {
                        input: 40,
                        output: 12,
                        cached: None,
                    },
                },
                StreamEvent::MessageStop,
            ]
        );
        assert!(decoder.is_done());

        // ... and the whole sequence is one the canonical order accepts and
        // accumulates into the answer it delivered.
        let mut order = canonical::Order::default();
        let all: Vec<StreamEvent> = first
            .into_iter()
            .chain(second)
            .chain(third)
            .chain(end)
            .collect();
        for event in &all {
            order.check(event).expect("a well-ordered stream");
        }
        let response = canonical::accumulate(&all).expect("one message");
        assert_eq!(response.blocks[0], Block::Text("Checking.".to_owned()));
        assert_eq!(response.stop_reason, StopReason::ToolUse);
    }

    /// The module doc's fifth decision: with no `[DONE]` on this wire, a
    /// stream that ends without a finish reason is a truncated message and
    /// is refused rather than completed.
    #[test]
    fn a_stream_that_ends_without_a_finish_reason_is_refused_as_truncated() {
        let mut decoder = ChunkDecoder::default();
        decoder
            .feed(&sse(&json!({
                "candidates": [{"content": {"role": "model", "parts": [{"text": "half"}]}}],
            })))
            .expect("a well-formed chunk");
        let refusal = decoder.finish().expect_err("the stream was cut");
        assert_eq!(refusal.field, "candidates[0].finishReason");
        assert!(refusal.reason.contains("truncated"), "{}", refusal.reason);

        let mut empty = ChunkDecoder::default();
        assert_eq!(
            empty.finish().expect_err("nothing arrived at all").field,
            "chunk"
        );
    }

    /// The encoder's one wire-imposed property: a tool call's fragments are
    /// held until the block stops, because Gemini has no partial-arguments
    /// event.
    #[test]
    fn the_encoder_writes_a_function_call_whole_because_this_wire_has_no_fragment() {
        let mut encoder = ChunkEncoder::default();
        assert!(
            encoder
                .encode(&StreamEvent::MessageStart {
                    id: "resp-1".to_owned(),
                    model: "gemini-2.5-pro".to_owned(),
                    usage: Usage::default(),
                })
                .is_empty()
        );
        assert!(
            encoder
                .encode(&StreamEvent::BlockStart {
                    index: 0,
                    block: BlockStart::ToolUse {
                        id: "call_A".to_owned(),
                        name: "Bash".to_owned(),
                    },
                })
                .is_empty()
        );
        assert!(
            encoder
                .encode(&StreamEvent::BlockDelta {
                    index: 0,
                    delta: Delta::InputJson("{\"command\": ".to_owned()),
                })
                .is_empty(),
            "a fragment has no shape on this wire and is held"
        );
        assert!(
            encoder
                .encode(&StreamEvent::BlockDelta {
                    index: 0,
                    delta: Delta::InputJson("\"ls\"}".to_owned()),
                })
                .is_empty()
        );
        let written = encoder.encode(&StreamEvent::BlockStop { index: 0 });
        let text = String::from_utf8(written).expect("UTF-8");
        let data = text
            .trim()
            .strip_prefix("data: ")
            .expect("one framed event");
        let chunk: Value = serde_json::from_str(data).expect("JSON");
        assert_eq!(
            chunk["candidates"][0]["content"]["parts"][0],
            json!({"functionCall": {"name": "Bash", "args": {"command": "ls"}}}),
            "the whole call is written once its arguments are complete"
        );
    }

    /// The claim rule: this codec owns `…/models/<model>:generateContent`
    /// under any of Google's version segments, refuses its siblings by name,
    /// and does not claim a model listing — which the gateway has always
    /// answered with its plain `404`.
    #[test]
    fn the_claim_covers_generate_content_under_googles_own_version_segments() {
        for path in [
            "/v1beta/models/gemini-2.5-pro:generateContent",
            "/v1/models/gemini-2.5-pro:generateContent",
            "/v1alpha/models/gemini-2.5-pro:streamGenerateContent",
            "/models/gemini-2.5-pro:streamGenerateContent",
        ] {
            assert!(
                matches!(Gemini.claim(path), Claim::Endpoint),
                "{path} must be this codec's endpoint"
            );
        }
        for path in [
            "/v1beta/models/gemini-2.5-pro:countTokens",
            "/models/gemini-2.5-pro:embedContent",
        ] {
            assert!(
                matches!(Gemini.claim(path), Claim::Other),
                "{path} is this protocol's own surface and not translated"
            );
        }
        for path in [
            "/models",
            "/v1beta/models",
            "/models/gemini-2.5-pro",
            "/chat/completions",
            "/v1/messages",
            "/models/a/b:generateContent",
        ] {
            assert!(
                matches!(Gemini.claim(path), Claim::None),
                "{path} does not belong to this codec"
            );
        }
    }

    /// A Gemini error document, in both the shapes Google writes it in.
    #[test]
    fn a_provider_error_message_is_read_out_of_either_shape_google_writes() {
        let object =
            json!({"error": {"code": 429, "message": "quota", "status": "RESOURCE_EXHAUSTED"}});
        assert_eq!(
            Gemini.decode_error(object.to_string().as_bytes()),
            Some("quota".to_owned())
        );
        let array =
            json!([{"error": {"code": 400, "message": "bad", "status": "INVALID_ARGUMENT"}}]);
        assert_eq!(
            Gemini.decode_error(array.to_string().as_bytes()),
            Some("bad".to_owned())
        );
        let written: Value =
            serde_json::from_slice(&Gemini.encode_error("INVALID_ARGUMENT", "no")).unwrap();
        assert_eq!(written["error"]["status"], "INVALID_ARGUMENT");
        assert_eq!(
            written["error"]["code"],
            Value::Null,
            "a refusal written here is the gateway's, not Google's, and carries no Google code"
        );
    }

    /// Every refusal this file raises names a reason from its own table, and
    /// every table entry is reachable through `field_rows`.
    #[test]
    fn every_refused_field_carries_a_reason_and_no_entry_is_a_duplicate() {
        let mut names: Vec<&str> = REFUSED_FIELDS.iter().map(|(name, _)| *name).collect();
        names.sort_unstable();
        let mut unique = names.clone();
        unique.dedup();
        assert_eq!(names, unique, "a field is refused for exactly one reason");
        for (field, why) in REFUSED_FIELDS {
            assert!(!why.is_empty(), "{field} has no reason");
        }
    }

    impl Message {
        /// The same message with each tool-use id replaced by the one this
        /// codec mints for a call of that name at that position — what a
        /// round trip through the wire produces, since Gemini issues no id.
        fn clone_with_minted_id(&self) -> Message {
            Message {
                role: self.role,
                blocks: self
                    .blocks
                    .iter()
                    .enumerate()
                    .map(|(index, block)| match block {
                        Block::ToolUse { name, input, .. } => Block::ToolUse {
                            id: minted_id(index, name),
                            name: name.clone(),
                            input: input.clone(),
                        },
                        other => other.clone(),
                    })
                    .collect(),
            }
        }
    }
}
