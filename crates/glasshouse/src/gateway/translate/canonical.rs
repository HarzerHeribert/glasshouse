//! The one form every codec meets in — Phase 56's canonical request, response
//! and stream-event vocabulary.
//!
//! # Why one form and not a translator per pair
//!
//! `docs/product/design-decisions.md`, Phase 56, *"the user's answer on
//! pairs: all of them"*: translation is one canonical form plus one codec per
//! wire protocol, so three protocols cost three codecs rather than six
//! translators, and a pair is a decoder and an encoder meeting here. Fidelity
//! is a property of a codec and is tested per codec, by round trip through
//! this form; per pair only the end-to-end test is owed.
//!
//! # What the form deliberately cannot say
//!
//! Every field here is one that **both** protocols with a codec can carry.
//! A wire field with no home in this form is not dropped by the decoder that
//! meets it — it is refused, by name, as an [`Unsupported`], and the refusal
//! reaches the harness as a `4xx` whose body names the field. That is the
//! whole of capability map line 1950's *"refuse the pairing by name when it
//! cannot be kept"* at the level of one request: the form is the supported
//! subset, and anything outside it is a named refusal rather than a silent
//! degradation.
//!
//! # Tool calls are the point
//!
//! A harness's native tooling rides on three things surviving a round trip
//! unchanged: the tool definitions it declares, the tool-use blocks the
//! model answers with, and the tool-result blocks it sends back — with the
//! **ids preserved**, because the id is how a result is matched to the call
//! that asked for it. [`Block::ToolUse`]'s `id` is the same string on both
//! wires: Anthropic's `tool_use.id` and OpenAI's `tool_calls[].id` are never
//! rewritten, minted, or mapped through a table. A wrong id here runs the
//! wrong tool, which is why the mutation on this mapping is the first one the
//! package owes.

use serde_json::Value;

/// A request, as either protocol's client would have made it.
#[derive(Debug, Clone, PartialEq)]
pub struct Request {
    pub model: String,
    /// Required by Anthropic Messages, optional on OpenAI Chat; carried as
    /// given.
    pub max_tokens: Option<u64>,
    /// The system prompt as one text. Anthropic's array of system blocks is
    /// joined with a blank line; OpenAI's `system` message is taken as is.
    pub system: Option<String>,
    pub messages: Vec<Message>,
    pub tools: Vec<ToolDefinition>,
    pub tool_choice: Option<ToolChoice>,
    /// `Some(false)` when the harness asked that tool calls not be issued in
    /// parallel — Anthropic's `disable_parallel_tool_use`, OpenAI's
    /// `parallel_tool_calls`.
    pub parallel_tool_calls: Option<bool>,
    pub temperature: Option<f64>,
    pub top_p: Option<f64>,
    /// Stop sequences, in the order given. Empty when none.
    pub stop: Vec<String>,
    pub stream: bool,
    /// An end-user identifier the harness attached — Anthropic's
    /// `metadata.user_id`, OpenAI's `user`.
    pub user: Option<String>,
    /// Whether the harness marked any point of this request for prompt
    /// caching — Anthropic's `cache_control`, on the system prompt, a
    /// content block or a tool definition.
    ///
    /// Collapsed to one flag rather than threaded through [`Block`] and
    /// [`ToolDefinition`]: no target this gateway translates to has a
    /// per-block cache primitive to carry a position into. OpenAI's
    /// `prompt_cache_key` and Gemini's cached-content resource are both
    /// request/session-scoped, so the position Claude Code marked carries
    /// no information a target here can use — only the fact that caching
    /// was asked for at all, which is what a decoder must not silently drop
    /// (see the module doc's *"refused by name, never dropped"*).
    pub cache_requested: bool,
}

impl Request {
    /// The stable per-session prompt-cache key a target that accepts one
    /// should carry (2018) — Claude Code's own `metadata.user_id`, already
    /// decoded into [`Request::user`] and already sent to every provider
    /// under that name, so nothing new crosses the wire. Never anything
    /// else: a cache key must be a value already visible to the evidence
    /// ledger, not the gateway's own token or a credential.
    pub fn prompt_cache_key(&self) -> Option<&str> {
        self.user.as_deref()
    }

    /// Serialize deterministically (2016): tool definitions sorted by name,
    /// once, at the seam every translated request passes through, so all
    /// three encoders emit the same order for the same tool set however the
    /// harness listed them. JSON-Schema key order needs no equivalent step
    /// here — this crate never enables `serde_json`'s `preserve_order`
    /// feature, so `Value::Object` is a sorted map and already serializes
    /// with keys in order; see the tripwire test in this module.
    pub fn normalized(mut self) -> Self {
        self.tools.sort_by(|a, b| a.name.cmp(&b.name));
        self
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Message {
    pub role: Role,
    pub blocks: Vec<Block>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    User,
    Assistant,
}

/// One typed content block.
#[derive(Debug, Clone, PartialEq)]
pub enum Block {
    Text(String),
    Image(ImageSource),
    /// The model asked for a tool to run. `id` is preserved verbatim across
    /// both wires — see the module doc.
    ToolUse {
        id: String,
        name: String,
        input: Value,
    },
    /// The harness ran a tool and this is what it returned. `tool_use_id`
    /// names the [`Block::ToolUse`] it answers, again verbatim.
    ToolResult {
        tool_use_id: String,
        content: String,
        is_error: bool,
    },
}

/// Where an image's bytes come from. A URL carries no media type on either
/// wire — Anthropic's `url` source and OpenAI's `image_url` both leave it to
/// the fetch — so only the inline form names one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImageSource {
    /// Inline base64 data, without a `data:` prefix, and its IANA media
    /// type, e.g. `image/png`.
    Base64 {
        media_type: String,
        data: String,
    },
    Url(String),
}

#[derive(Debug, Clone, PartialEq)]
pub struct ToolDefinition {
    pub name: String,
    pub description: Option<String>,
    /// A JSON Schema object. Carried as given; nothing here validates it.
    pub input_schema: Value,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolChoice {
    Auto,
    /// The model must call some tool — Anthropic `any`, OpenAI `required`.
    Any,
    /// The model must call this tool.
    Tool(String),
    None,
}

/// A complete (non-streamed) response.
#[derive(Debug, Clone, PartialEq)]
pub struct Response {
    pub id: String,
    pub model: String,
    /// Only [`Block::Text`] and [`Block::ToolUse`] can appear here.
    pub blocks: Vec<Block>,
    pub stop_reason: StopReason,
    pub stop_sequence: Option<String>,
    pub usage: Usage,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopReason {
    EndTurn,
    MaxTokens,
    StopSequence,
    ToolUse,
    /// The provider declined to answer — OpenAI's `content_filter`,
    /// Anthropic's `refusal`.
    Refusal,
}

/// Token counts as the provider stated them.
///
/// `input` excludes tokens served from a prompt cache; `cached` is those, when
/// the provider distinguished them. `None` is "the provider did not say",
/// which stays different from zero all the way to the evidence ledger.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Usage {
    pub input: u64,
    pub output: u64,
    pub cached: Option<u64>,
}

/// One event of a streamed response, in the one vocabulary both stream
/// codecs speak. The order is Anthropic's, because it is the stricter of the
/// two: a message starts, blocks start, receive deltas and stop, the message
/// gets its final delta with the stop reason and usage, and the message
/// stops.
#[derive(Debug, Clone, PartialEq)]
pub enum StreamEvent {
    MessageStart {
        id: String,
        model: String,
        /// What is known at the start: the input count when the provider
        /// states it up front, zero otherwise.
        usage: Usage,
    },
    BlockStart {
        index: usize,
        block: BlockStart,
    },
    BlockDelta {
        index: usize,
        delta: Delta,
    },
    BlockStop {
        index: usize,
    },
    MessageDelta {
        stop_reason: StopReason,
        stop_sequence: Option<String>,
        /// The final counts. An `input` of zero here means "not restated",
        /// and [`accumulate`] keeps the start's reading.
        usage: Usage,
    },
    MessageStop,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BlockStart {
    Text,
    ToolUse { id: String, name: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Delta {
    Text(String),
    /// A fragment of the tool input's JSON, to be concatenated in order.
    InputJson(String),
}

/// A wire field or shape this form has no home for, named.
///
/// `field` is a path in the wire document's own spelling — `system[0].cache_control`,
/// `choices` — and `reason` is one sentence a user can act on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unsupported {
    pub field: String,
    pub reason: String,
}

impl Unsupported {
    pub fn new(field: impl Into<String>, reason: impl Into<String>) -> Self {
        Self {
            field: field.into(),
            reason: reason.into(),
        }
    }
}

impl std::fmt::Display for Unsupported {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "`{}`: {}", self.field, self.reason)
    }
}

impl Response {
    /// The event sequence a streamed delivery of this response produces.
    ///
    /// What a translated exchange writes when the harness asked for a stream
    /// and the provider answered with a whole document anyway — and the
    /// other half of the stream round trip [`accumulate`] closes.
    pub fn as_events(&self) -> Vec<StreamEvent> {
        let mut events = vec![StreamEvent::MessageStart {
            id: self.id.clone(),
            model: self.model.clone(),
            usage: Usage {
                input: self.usage.input,
                output: 0,
                cached: self.usage.cached,
            },
        }];
        for (index, block) in self.blocks.iter().enumerate() {
            match block {
                Block::Text(text) => {
                    events.push(StreamEvent::BlockStart {
                        index,
                        block: BlockStart::Text,
                    });
                    if !text.is_empty() {
                        events.push(StreamEvent::BlockDelta {
                            index,
                            delta: Delta::Text(text.clone()),
                        });
                    }
                }
                Block::ToolUse { id, name, input } => {
                    events.push(StreamEvent::BlockStart {
                        index,
                        block: BlockStart::ToolUse {
                            id: id.clone(),
                            name: name.clone(),
                        },
                    });
                    events.push(StreamEvent::BlockDelta {
                        index,
                        delta: Delta::InputJson(input.to_string()),
                    });
                }
                // A response never carries these — `decode_response` on either
                // codec cannot produce them — so a stream of one has nothing
                // to say about them either.
                Block::Image(_) | Block::ToolResult { .. } => {}
            }
            events.push(StreamEvent::BlockStop { index });
        }
        events.push(StreamEvent::MessageDelta {
            stop_reason: self.stop_reason,
            stop_sequence: self.stop_sequence.clone(),
            usage: self.usage,
        });
        events.push(StreamEvent::MessageStop);
        events
    }
}

/// Fold a complete event sequence back into the response it delivered.
///
/// Refuses a sequence that is not one message — no start, a delta for a
/// block that never started, a tool input that is not a JSON object once its
/// fragments are joined — naming what was wrong.
pub fn accumulate(events: &[StreamEvent]) -> Result<Response, Unsupported> {
    struct Open {
        block: Block,
        json: String,
    }
    let mut id = None;
    let mut model = String::new();
    let mut usage = Usage::default();
    let mut blocks: Vec<Open> = Vec::new();
    let mut stop_reason = None;
    let mut stop_sequence = None;

    for event in events {
        match event {
            StreamEvent::MessageStart {
                id: started,
                model: started_model,
                usage: started_usage,
            } => {
                id = Some(started.clone());
                model = started_model.clone();
                usage = *started_usage;
            }
            StreamEvent::BlockStart { index, block } => {
                if *index != blocks.len() {
                    return Err(Unsupported::new(
                        format!("content_block_start[{index}]"),
                        format!("blocks must start in order; {} were open", blocks.len()),
                    ));
                }
                blocks.push(Open {
                    block: match block {
                        BlockStart::Text => Block::Text(String::new()),
                        BlockStart::ToolUse { id, name } => Block::ToolUse {
                            id: id.clone(),
                            name: name.clone(),
                            input: Value::Null,
                        },
                    },
                    json: String::new(),
                });
            }
            StreamEvent::BlockDelta { index, delta } => {
                let Some(open) = blocks.get_mut(*index) else {
                    return Err(Unsupported::new(
                        format!("content_block_delta[{index}]"),
                        "a delta arrived for a block that never started",
                    ));
                };
                match (&mut open.block, delta) {
                    (Block::Text(text), Delta::Text(more)) => text.push_str(more),
                    (Block::ToolUse { .. }, Delta::InputJson(more)) => open.json.push_str(more),
                    (Block::Text(_), Delta::InputJson(_)) => {
                        return Err(Unsupported::new(
                            format!("content_block_delta[{index}]"),
                            "a tool-input delta arrived for a text block",
                        ));
                    }
                    (Block::ToolUse { .. }, Delta::Text(_)) => {
                        return Err(Unsupported::new(
                            format!("content_block_delta[{index}]"),
                            "a text delta arrived for a tool-use block",
                        ));
                    }
                    (Block::Image(_) | Block::ToolResult { .. }, _) => {
                        unreachable!("a block opened here is always a text or tool-use block")
                    }
                }
            }
            StreamEvent::BlockStop { index } => {
                let Some(open) = blocks.get_mut(*index) else {
                    return Err(Unsupported::new(
                        format!("content_block_stop[{index}]"),
                        "a stop arrived for a block that never started",
                    ));
                };
                if let Block::ToolUse { input, .. } = &mut open.block {
                    *input = parse_tool_input(&open.json).map_err(|reason| {
                        Unsupported::new(format!("content_block_stop[{index}]"), reason)
                    })?;
                }
            }
            StreamEvent::MessageDelta {
                stop_reason: reason,
                stop_sequence: sequence,
                usage: final_usage,
            } => {
                stop_reason = Some(*reason);
                stop_sequence = sequence.clone();
                usage = Usage {
                    input: if final_usage.input == 0 {
                        usage.input
                    } else {
                        final_usage.input
                    },
                    output: final_usage.output,
                    cached: final_usage.cached.or(usage.cached),
                };
            }
            StreamEvent::MessageStop => {}
        }
    }

    let Some(id) = id else {
        return Err(Unsupported::new(
            "message_start",
            "the stream ended without a message ever starting",
        ));
    };
    let Some(stop_reason) = stop_reason else {
        return Err(Unsupported::new(
            "message_delta",
            "the stream ended without a stop reason",
        ));
    };
    Ok(Response {
        id,
        model,
        blocks: blocks.into_iter().map(|open| open.block).collect(),
        stop_reason,
        stop_sequence,
        usage,
    })
}

/// The ordering rules [`accumulate`] enforces over a whole sequence, enforced
/// one event at a time on a stream that is never accumulated.
///
/// A stream codec's encoder may only be handed a sequence in which a block
/// starts before its deltas and a delta names the block that is open. Nothing
/// downstream re-checks it: an encoder writes bytes and has no error channel,
/// so an out-of-order delta silently rides on whichever block happens to be
/// open — under the **wrong tool-call id**. This is where that is refused.
#[derive(Debug, Default)]
pub struct Order {
    started: usize,
    open: Option<usize>,
}

impl Order {
    /// `Ok(())` when `event` may follow everything checked so far.
    pub fn check(&mut self, event: &StreamEvent) -> Result<(), Unsupported> {
        match event {
            StreamEvent::BlockStart { index, .. } => {
                if *index != self.started {
                    return Err(Unsupported::new(
                        format!("content_block_start[{index}]"),
                        format!("blocks must start in order; {} had started", self.started),
                    ));
                }
                self.started += 1;
                self.open = Some(*index);
            }
            StreamEvent::BlockDelta { index, .. } => {
                if self.open != Some(*index) {
                    return Err(Unsupported::new(
                        format!("content_block_delta[{index}]"),
                        "a delta arrived for a block that is not the open one, and a delta \
                         carried onto another block would carry another tool call's id",
                    ));
                }
            }
            StreamEvent::BlockStop { index } => {
                if self.open != Some(*index) {
                    return Err(Unsupported::new(
                        format!("content_block_stop[{index}]"),
                        "a stop arrived for a block that is not the open one",
                    ));
                }
                self.open = None;
            }
            StreamEvent::MessageStart { .. }
            | StreamEvent::MessageDelta { .. }
            | StreamEvent::MessageStop => {}
        }
        Ok(())
    }
}

/// A tool input from its streamed JSON fragments: an empty text is an empty
/// object, which is what both wires mean by a tool call with no arguments.
pub fn parse_tool_input(json: &str) -> Result<Value, String> {
    if json.trim().is_empty() {
        return Ok(Value::Object(serde_json::Map::new()));
    }
    match serde_json::from_str::<Value>(json) {
        Ok(value @ Value::Object(_)) => Ok(value),
        Ok(other) => Err(format!(
            "a tool input must be a JSON object, not {}",
            json_kind(&other)
        )),
        Err(_) => Err("the tool input's arguments are not valid JSON".to_owned()),
    }
}

/// The kind of a JSON value, for a refusal that names what arrived.
pub fn json_kind(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "a boolean",
        Value::Number(_) => "a number",
        Value::String(_) => "a string",
        Value::Array(_) => "an array",
        Value::Object(_) => "an object",
    }
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;

    use serde_json::json;

    pub(crate) fn tool_call_response() -> Response {
        Response {
            id: "msg_01".to_owned(),
            model: "gpt-x".to_owned(),
            blocks: vec![
                Block::Text("Let me look.".to_owned()),
                Block::ToolUse {
                    id: "call_abc123".to_owned(),
                    name: "Bash".to_owned(),
                    input: json!({"command": "ls -la", "timeout": 5}),
                },
                Block::ToolUse {
                    id: "call_def456".to_owned(),
                    name: "Read".to_owned(),
                    input: json!({"file_path": "/tmp/x"}),
                },
            ],
            stop_reason: StopReason::ToolUse,
            stop_sequence: None,
            usage: Usage {
                input: 120,
                output: 33,
                cached: Some(100),
            },
        }
    }

    #[test]
    fn json_object_keys_serialize_sorted_because_this_crate_never_enables_preserve_order() {
        // The tripwire for 2016: if `serde_json`'s `preserve_order` feature
        // is ever turned on (a feature unification from an unrelated
        // dependency, not a choice made here), a JSON-Schema's declared key
        // order would leak into encoded bytes and break prefix stability
        // across turns — this pins the sorted-map behaviour the encoders
        // rely on instead.
        let mut map = serde_json::Map::new();
        map.insert("zebra".to_owned(), json!(1));
        map.insert("apple".to_owned(), json!(2));
        map.insert("mango".to_owned(), json!(3));
        let value = Value::Object(map);
        assert_eq!(value.to_string(), r#"{"apple":2,"mango":3,"zebra":1}"#);
    }

    #[test]
    fn normalized_sorts_tools_by_name_regardless_of_the_harnesss_order() {
        let tool = |name: &str| ToolDefinition {
            name: name.to_owned(),
            description: None,
            input_schema: json!({}),
        };
        let request = Request {
            model: "m".to_owned(),
            max_tokens: None,
            system: None,
            messages: Vec::new(),
            tools: vec![tool("zeta"), tool("alpha"), tool("mu")],
            tool_choice: None,
            parallel_tool_calls: None,
            temperature: None,
            top_p: None,
            stop: Vec::new(),
            stream: false,
            user: None,
            cache_requested: false,
        }
        .normalized();
        let names: Vec<&str> = request.tools.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(names, vec!["alpha", "mu", "zeta"]);
    }

    #[test]
    fn a_response_streams_out_and_accumulates_back_to_itself() {
        let response = tool_call_response();
        let events = response.as_events();
        assert_eq!(
            events.first(),
            Some(&StreamEvent::MessageStart {
                id: "msg_01".to_owned(),
                model: "gpt-x".to_owned(),
                usage: Usage {
                    input: 120,
                    output: 0,
                    cached: Some(100)
                },
            })
        );
        assert_eq!(events.last(), Some(&StreamEvent::MessageStop));
        assert_eq!(accumulate(&events).expect("a well-formed stream"), response);
    }

    #[test]
    fn a_tool_input_streamed_in_fragments_is_joined_before_it_is_parsed() {
        let events = vec![
            StreamEvent::MessageStart {
                id: "m".to_owned(),
                model: "x".to_owned(),
                usage: Usage::default(),
            },
            StreamEvent::BlockStart {
                index: 0,
                block: BlockStart::ToolUse {
                    id: "call_1".to_owned(),
                    name: "Edit".to_owned(),
                },
            },
            StreamEvent::BlockDelta {
                index: 0,
                delta: Delta::InputJson("{\"path\": \"a".to_owned()),
            },
            StreamEvent::BlockDelta {
                index: 0,
                delta: Delta::InputJson("/b\", \"n\": 2}".to_owned()),
            },
            StreamEvent::BlockStop { index: 0 },
            StreamEvent::MessageDelta {
                stop_reason: StopReason::ToolUse,
                stop_sequence: None,
                usage: Usage {
                    input: 0,
                    output: 7,
                    cached: None,
                },
            },
            StreamEvent::MessageStop,
        ];
        let response = accumulate(&events).expect("fragments join into one object");
        assert_eq!(
            response.blocks,
            vec![Block::ToolUse {
                id: "call_1".to_owned(),
                name: "Edit".to_owned(),
                input: json!({"path": "a/b", "n": 2}),
            }]
        );
        assert_eq!(response.stop_reason, StopReason::ToolUse);
    }

    #[test]
    fn a_stream_with_no_arguments_at_all_is_an_empty_object_and_not_a_refusal() {
        assert_eq!(parse_tool_input(""), Ok(json!({})));
        assert_eq!(parse_tool_input("  "), Ok(json!({})));
        assert!(parse_tool_input("[1]").is_err());
        assert!(parse_tool_input("{not json").is_err());
    }

    #[test]
    fn a_malformed_stream_is_refused_by_the_event_that_was_wrong() {
        let orphan_delta = [StreamEvent::BlockDelta {
            index: 3,
            delta: Delta::Text("x".to_owned()),
        }];
        let refusal = accumulate(&orphan_delta).expect_err("no block started");
        assert_eq!(refusal.field, "content_block_delta[3]");

        let no_stop = [StreamEvent::MessageStart {
            id: "m".to_owned(),
            model: "x".to_owned(),
            usage: Usage::default(),
        }];
        assert_eq!(
            accumulate(&no_stop).expect_err("no stop reason").field,
            "message_delta"
        );
    }

    fn tool_start(id: &str) -> BlockStart {
        BlockStart::ToolUse {
            id: id.to_owned(),
            name: "Bash".to_owned(),
        }
    }

    #[test]
    fn order_accepts_a_well_ordered_stream() {
        let mut order = Order::default();
        assert_eq!(
            order.check(&StreamEvent::MessageStart {
                id: "m".to_owned(),
                model: "x".to_owned(),
                usage: Usage::default(),
            }),
            Ok(())
        );
        assert_eq!(
            order.check(&StreamEvent::BlockStart {
                index: 0,
                block: tool_start("call_A"),
            }),
            Ok(())
        );
        assert_eq!(
            order.check(&StreamEvent::BlockDelta {
                index: 0,
                delta: Delta::InputJson("{}".to_owned()),
            }),
            Ok(())
        );
        assert_eq!(order.check(&StreamEvent::BlockStop { index: 0 }), Ok(()));
        assert_eq!(
            order.check(&StreamEvent::BlockStart {
                index: 1,
                block: tool_start("call_B"),
            }),
            Ok(())
        );
        assert_eq!(order.check(&StreamEvent::BlockStop { index: 1 }), Ok(()));
        assert_eq!(
            order.check(&StreamEvent::MessageDelta {
                stop_reason: StopReason::ToolUse,
                stop_sequence: None,
                usage: Usage::default(),
            }),
            Ok(())
        );
        assert_eq!(order.check(&StreamEvent::MessageStop), Ok(()));
    }

    #[test]
    fn order_refuses_a_block_that_starts_out_of_sequence() {
        let mut order = Order::default();
        // Block 1 starts before block 0 has ever started.
        let refusal = order
            .check(&StreamEvent::BlockStart {
                index: 1,
                block: tool_start("call_A"),
            })
            .expect_err("index 1 cannot start before index 0");
        assert_eq!(refusal.field, "content_block_start[1]");
        assert!(refusal.reason.contains("blocks must start in order"));
    }

    #[test]
    fn order_refuses_a_delta_for_a_block_that_is_not_open() {
        let mut order = Order::default();
        // The exact hazard shape: call_A starts, call_B starts before call_A
        // stops, then a delta arrives addressed to call_A (index 0) while
        // call_B (index 1) is the open block.
        order
            .check(&StreamEvent::BlockStart {
                index: 0,
                block: tool_start("call_A"),
            })
            .expect("call_A starts");
        order
            .check(&StreamEvent::BlockStart {
                index: 1,
                block: tool_start("call_B"),
            })
            .expect("call_B starts before call_A stopped");
        let refusal = order
            .check(&StreamEvent::BlockDelta {
                index: 0,
                delta: Delta::InputJson("{\"command\"".to_owned()),
            })
            .expect_err("call_A's delta must not ride on call_B's open block");
        assert_eq!(refusal.field, "content_block_delta[0]");
        assert!(refusal.reason.contains("another tool call's id"));
    }

    #[test]
    fn order_refuses_a_stop_for_a_block_that_is_not_open() {
        let mut order = Order::default();
        order
            .check(&StreamEvent::BlockStart {
                index: 0,
                block: tool_start("call_A"),
            })
            .expect("call_A starts");
        order
            .check(&StreamEvent::BlockStart {
                index: 1,
                block: tool_start("call_B"),
            })
            .expect("call_B starts before call_A stopped");
        let refusal = order
            .check(&StreamEvent::BlockStop { index: 0 })
            .expect_err("index 0 is not the open block");
        assert_eq!(refusal.field, "content_block_stop[0]");
        assert!(refusal.reason.contains("not the open one"));
    }

    #[test]
    fn order_lets_message_events_pass_through_regardless_of_block_state() {
        let mut order = Order::default();
        assert_eq!(
            order.check(&StreamEvent::MessageStart {
                id: "m".to_owned(),
                model: "x".to_owned(),
                usage: Usage::default(),
            }),
            Ok(())
        );
        assert_eq!(
            order.check(&StreamEvent::MessageDelta {
                stop_reason: StopReason::EndTurn,
                stop_sequence: None,
                usage: Usage::default(),
            }),
            Ok(()),
            "message events carry no block index and are never refused by Order"
        );
        assert_eq!(order.check(&StreamEvent::MessageStop), Ok(()));
    }
}
