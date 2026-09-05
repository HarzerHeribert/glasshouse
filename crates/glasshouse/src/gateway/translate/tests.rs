use super::*;

/// Line 1331/1332's rule, fed canonical events directly rather than
/// through a decoder: a whitespace-only text delta, a text block start
/// with no delta of its own, a tool-input-JSON fragment, then real text,
/// then a tool-use block start. Only the last two qualify, each stamps
/// once, and nothing already stamped is stamped again.
#[test]
fn first_events_note_stamps_only_a_real_token_and_a_tool_use_and_never_twice() {
    use std::cell::Cell;

    // Both readings the production closure supplies, moving together:
    // the seconds tick from 100 and the milliseconds are 50x each, so an
    // assertion can tell which reading a stamp came from.
    let clock = Cell::new(100i64);
    let now = || {
        let at = clock.get();
        clock.set(at + 1);
        (at, Some(at * 50))
    };

    let mut first = FirstEvents::default();
    assert_eq!(first.first_token_at, None);
    assert_eq!(first.first_tool_call_at, None);

    // Whitespace-only text: not a real token.
    first.note(
        &StreamEvent::BlockDelta {
            index: 0,
            delta: Delta::Text("   \n".to_owned()),
        },
        &now,
    );
    assert_eq!(first.first_token_at, None, "whitespace must not count");

    // A text block opening with no delta of its own.
    first.note(
        &StreamEvent::BlockStart {
            index: 0,
            block: BlockStart::Text,
        },
        &now,
    );
    assert_eq!(first.first_token_at, None);
    assert_eq!(first.first_tool_call_at, None);

    // A tool-input JSON fragment is not a text delta.
    first.note(
        &StreamEvent::BlockDelta {
            index: 1,
            delta: Delta::InputJson("{\"command\"".to_owned()),
        },
        &now,
    );
    assert_eq!(first.first_token_at, None);
    assert_eq!(first.first_tool_call_at, None);

    // Real text: the first qualifying event.
    first.note(
        &StreamEvent::BlockDelta {
            index: 0,
            delta: Delta::Text("hello".to_owned()),
        },
        &now,
    );
    assert_eq!(first.first_token_at, Some(100));
    assert_eq!(
        first.first_token_ms,
        Some(5000),
        "migration 25's offset is stamped from the same reading as the second"
    );
    assert_eq!(first.first_tool_call_at, None);
    assert_eq!(first.first_tool_call_ms, None);

    // A tool-use block start: the first qualifying event for its field.
    first.note(
        &StreamEvent::BlockStart {
            index: 1,
            block: BlockStart::ToolUse {
                id: "call_1".to_owned(),
                name: "Bash".to_owned(),
            },
        },
        &now,
    );
    assert_eq!(first.first_token_at, Some(100));
    assert_eq!(first.first_tool_call_at, Some(101));
    assert_eq!(first.first_token_ms, Some(5000));
    assert_eq!(first.first_tool_call_ms, Some(5050));

    // Neither restamps on a later qualifying event of its own kind.
    first.note(
        &StreamEvent::BlockDelta {
            index: 0,
            delta: Delta::Text("more".to_owned()),
        },
        &now,
    );
    first.note(
        &StreamEvent::BlockStart {
            index: 2,
            block: BlockStart::ToolUse {
                id: "call_2".to_owned(),
                name: "Read".to_owned(),
            },
        },
        &now,
    );
    assert_eq!(
        first.first_token_at,
        Some(100),
        "first token must not restamp"
    );
    assert_eq!(
        first.first_tool_call_at,
        Some(101),
        "first tool call must not restamp"
    );
    assert_eq!(
        first.first_token_ms,
        Some(5000),
        "migration 25's offset must not restamp either"
    );
    assert_eq!(first.first_tool_call_ms, Some(5050));
}

/// [`FirstEvents::of_document`]'s own rule: both instants equal the given
/// `first_byte_at` when the event sequence carries a qualifying event,
/// `None` when it does not or when `first_byte_at` itself is `None` (a
/// document that never reached a provider has no instant to derive from).
#[test]
fn first_events_of_document_uses_first_byte_at_as_the_only_clock_reading() {
    let events = vec![
        StreamEvent::BlockStart {
            index: 0,
            block: BlockStart::Text,
        },
        StreamEvent::BlockDelta {
            index: 0,
            delta: Delta::Text("hi there".to_owned()),
        },
        StreamEvent::BlockStart {
            index: 1,
            block: BlockStart::ToolUse {
                id: "call_1".to_owned(),
                name: "Bash".to_owned(),
            },
        },
    ];
    let first = FirstEvents::of_document(&events, Some(1_700_000_000), Some(42));
    assert_eq!(first.first_token_at, Some(1_700_000_000));
    assert_eq!(first.first_tool_call_at, Some(1_700_000_000));
    assert_eq!(
        (first.first_token_ms, first.first_tool_call_ms),
        (Some(42), Some(42)),
        "a document exposes no finer boundary than its own arrival, in \
         milliseconds exactly as in seconds"
    );

    let text_only = vec![StreamEvent::BlockDelta {
        index: 0,
        delta: Delta::Text("hi".to_owned()),
    }];
    let first = FirstEvents::of_document(&text_only, Some(1_700_000_000), Some(42));
    assert_eq!(first.first_token_at, Some(1_700_000_000));
    assert_eq!(first.first_tool_call_at, None);
    assert_eq!(first.first_token_ms, Some(42));
    assert_eq!(first.first_tool_call_ms, None);

    let nothing_qualifying = vec![StreamEvent::BlockDelta {
        index: 0,
        delta: Delta::Text("   ".to_owned()),
    }];
    let first = FirstEvents::of_document(&nothing_qualifying, Some(1_700_000_000), Some(42));
    assert_eq!(first.first_token_at, None);
    assert_eq!(first.first_tool_call_at, None);

    // No `first_byte_at` at all: nothing to derive an instant from.
    let first = FirstEvents::of_document(&events, None, None);
    assert_eq!(first, FirstEvents::default());
}

#[test]
fn every_ordered_pair_appears_exactly_once() {
    for from in PROTOCOLS {
        for to in PROTOCOLS {
            let rows = TABLE
                .iter()
                .filter(|pair| pair.from == from && pair.to == to)
                .count();
            assert_eq!(rows, 1, "{from} -> {to} appears {rows} times");
        }
    }
    assert_eq!(TABLE.len(), PROTOCOLS.len() * PROTOCOLS.len());
    // ... and every slug in the table is a protocol.
    for pair in &TABLE {
        assert!(PROTOCOLS.contains(&pair.from), "{}", pair.from);
        assert!(PROTOCOLS.contains(&pair.to), "{}", pair.to);
    }
}

#[test]
fn exactly_the_supported_pairs_are_supported_and_every_other_row_carries_a_reason() {
    let supported: Vec<String> = TABLE
        .iter()
        .filter(|pair| pair.is_supported())
        .map(Pair::slug)
        .collect();
    assert_eq!(
        supported,
        vec![
            "anthropic-messages->openai-chat".to_owned(),
            "anthropic-messages->openai-responses".to_owned(),
            "openai-chat->openai-responses".to_owned(),
            "openai-responses->anthropic-messages".to_owned(),
            "openai-responses->openai-chat".to_owned(),
            "anthropic-messages->gemini-generate-content".to_owned(),
            "openai-responses->gemini-generate-content".to_owned(),
            "openai-chat->gemini-generate-content".to_owned(),
        ]
    );
    for pair in TABLE.iter().filter(|pair| !pair.is_supported()) {
        let reason = pair.refusal().expect("a refused pair has a reason");
        assert!(!reason.is_empty(), "{}", pair.slug());
    }
    // A supported pair has a codec on both sides — the table cannot
    // promise what the codecs cannot do.
    for pair in TABLE.iter().filter(|pair| pair.is_supported()) {
        assert!(codec_for(pair.from).is_some(), "{}", pair.from);
        assert!(codec_for(pair.to).is_some(), "{}", pair.to);
    }
    assert!(is_supported("anthropic-messages", "openai-chat"));
    assert!(is_supported("anthropic-messages", "openai-responses"));
    assert!(is_supported("openai-responses", "anthropic-messages"));
    assert!(is_supported("openai-responses", "openai-chat"));
    assert!(is_supported("openai-chat", "openai-responses"));
    assert!(is_supported(
        "anthropic-messages",
        "gemini-generate-content"
    ));
    assert!(is_supported("openai-responses", "gemini-generate-content"));
    assert!(is_supported("openai-chat", "gemini-generate-content"));
    assert!(!is_supported("openai-chat", "anthropic-messages"));
    assert!(!is_supported("anthropic-messages", "anthropic-messages"));
    assert!(!is_supported("anthropic-messages", "gemini"));
    // Every row OUT of Gemini is refused for the reason that is true:
    // nothing installed speaks it at the ingress (T3b).
    for to in PROTOCOLS {
        let pair = lookup("gemini-generate-content", to).expect("a row exists");
        assert!(!pair.is_supported(), "{}", pair.slug());
        let refusal = pair.refusal().expect("a refused pair has a reason");
        if to == "gemini-generate-content" {
            assert_eq!(refusal, SAME_PROTOCOL);
        } else {
            assert!(refusal.contains("T3b"), "{}: {refusal}", pair.slug());
        }
    }
}

#[test]
fn a_target_is_placed_from_its_path_alone_and_a_served_protocol_never_enters_a_codec() {
    let served_chat = ["openai-chat"];
    assert!(matches!(
        place("/v1/messages?beta=true", &served_chat),
        Placement::Translate(pair) if pair.slug() == "anthropic-messages->openai-chat"
    ));
    assert!(matches!(
        place("/messages", &served_chat),
        Placement::Translate(_)
    ));
    // The endpoint's sub-targets are refused by name, not translated.
    assert!(matches!(
        place("/v1/messages/count_tokens", &served_chat),
        Placement::TargetRefused {
            from: "anthropic-messages"
        }
    ));
    // A served protocol is never placed, even though a codec claims it.
    assert!(matches!(
        place("/v1/messages", &["anthropic-messages", "openai-chat"]),
        Placement::Unplaceable
    ));
    assert!(matches!(
        place("/v1/chat/completions", &["openai-chat"]),
        Placement::Unplaceable
    ));
    // Not a codec's target at all: the plain 404 stays.
    assert!(matches!(
        place("/api/hello", &served_chat),
        Placement::Unplaceable
    ));
    assert!(matches!(
        place("/models?client_version=1", &served_chat),
        Placement::Unplaceable
    ));
    assert!(matches!(
        place("/v1/messagesomethingelse", &served_chat),
        Placement::Unplaceable
    ));
    // The two T2 pairs place: Claude Code at a Responses-only provider,
    // and a Codex-shaped client at an Anthropic-only one.
    assert!(matches!(
        place("/v1/messages", &["openai-responses"]),
        Placement::Translate(pair) if pair.slug() == "anthropic-messages->openai-responses"
    ));
    assert!(matches!(
        place("/responses", &["anthropic-messages"]),
        Placement::Translate(pair) if pair.slug() == "openai-responses->anthropic-messages"
    ));
    // ... and a served Responses target never enters the codec.
    assert!(matches!(
        place("/v1/responses", &["openai-responses", "anthropic-messages"]),
        Placement::Unplaceable
    ));
    // The two T2b pairs place too: an OpenCode-shaped client at a
    // Responses-only provider, and a Codex-shaped client at a Chat-only
    // one.
    assert!(matches!(
        place("/v1/chat/completions", &["openai-responses"]),
        Placement::Translate(pair) if pair.slug() == "openai-chat->openai-responses"
    ));
    assert!(matches!(
        place("/responses", &["openai-chat"]),
        Placement::Translate(pair) if pair.slug() == "openai-responses->openai-chat"
    ));
    // OpenAI Chat at an Anthropic-only provider: the reverse pair, refused
    // by name until its own end-to-end test exists.
    match place("/v1/chat/completions", &["anthropic-messages"]) {
        Placement::PairRefused { refused, .. } => {
            assert!(refused[0].refusal().unwrap().contains("1956"));
        }
        other => panic!("expected a refused pair, got {other:?}"),
    }
}

#[test]
fn a_refusal_names_the_pair_the_field_and_the_reason() {
    let pair = lookup("anthropic-messages", "openai-chat").unwrap();
    let refusal = TranslationRefusal::new(
        pair,
        Unsupported::new("messages[0].content[0].citations", "no home for it"),
    );
    let text = refusal.to_string();
    assert!(text.contains("anthropic-messages->openai-chat"));
    assert!(text.contains("`messages[0].content[0].citations`"));
    assert!(text.contains("no home for it"));
}

#[test]
fn field_rows_exist_for_every_codec_and_for_nothing_else() {
    let rows = field_rows("anthropic-messages").unwrap();
    // Carried (2014), not refused: `cache_control` left REFUSED_FIELDS
    // when this codec started accepting it.
    assert!(
        !rows
            .refused
            .iter()
            .any(|(field, _)| *field == "cache_control"),
        "cache_control is carried now, not refused"
    );
    assert_eq!(
        rows.cache, None,
        "Anthropic is never asked to encode a cache marker it did not itself decode"
    );
    // Carried (GH-EFFORT-CARRY), not refused: `thinking` left
    // REFUSED_FIELDS when this codec started accepting it.
    assert!(
        !rows.refused.iter().any(|(field, _)| *field == "thinking"),
        "thinking is carried now, not refused"
    );
    assert_eq!(
        rows.effort, None,
        "Anthropic is never asked to encode an effort marker it did not itself decode"
    );
    assert!(rows.ignored.contains(&"usage.service_tier"));
    let rows = field_rows("openai-chat").unwrap();
    assert!(rows.refused.iter().any(|(field, _)| *field == "n"));
    assert!(matches!(
        rows.cache,
        Some(CacheDisposition::Carried {
            field: "prompt_cache_key",
            ..
        })
    ));
    assert!(matches!(
        rows.effort,
        Some(EffortDisposition::Carried {
            field: "reasoning_effort",
            ..
        })
    ));
    let rows = field_rows("openai-responses").unwrap();
    assert!(
        rows.refused
            .iter()
            .any(|(field, _)| *field == "previous_response_id")
    );
    assert!(rows.ignored.contains(&"output[].id"));
    assert!(matches!(
        rows.cache,
        Some(CacheDisposition::Carried {
            field: "prompt_cache_key",
            ..
        })
    ));
    assert!(matches!(
        rows.effort,
        Some(EffortDisposition::Carried {
            field: "reasoning.effort",
            ..
        })
    ));
    let rows = field_rows("gemini-generate-content").unwrap();
    assert!(
        rows.refused
            .iter()
            .any(|(field, _)| *field == "safetySettings")
    );
    assert!(
        rows.ignored.contains(&"user"),
        "the one request field this gateway drops is named in the table it drops it from"
    );
    assert!(
        matches!(rows.cache, Some(CacheDisposition::Stripped(_))),
        "Gemini has no per-request cache marker to carry the harness's onto"
    );
    assert!(
        matches!(
            rows.effort,
            Some(EffortDisposition::Carried {
                field: "generationConfig.thinkingConfig.thinkingBudget",
                ..
            })
        ),
        "Gemini has a thinking-budget field to carry the harness's effort onto: {:?}",
        rows.effort
    );
    // ... and `gemini` alone is not a protocol slug.
    assert!(field_rows("gemini").is_none());
}

fn request_for(model: &str, stream: bool) -> Request {
    Request {
        model: model.to_owned(),
        max_tokens: None,
        system: None,
        messages: Vec::new(),
        tools: Vec::new(),
        tool_choice: None,
        parallel_tool_calls: None,
        temperature: None,
        top_p: None,
        stop: Vec::new(),
        stream,
        user: None,
        cache_requested: false,
        effort: None,
    }
}

#[test]
fn a_translated_request_is_posted_to_the_targets_native_clients_send() {
    // The convention every provider base URL is composed for — see
    // `outbound_target`'s own doc. The Anthropic path carries the
    // version segment because Claude Code sends it and the Anthropic
    // base URLs omit it; the OpenAI paths omit it because their clients
    // do and their base URLs carry it.
    let plain = request_for("m", false);
    assert_eq!(
        outbound_target(codec_for("anthropic-messages").unwrap(), &plain),
        "/v1/messages"
    );
    assert_eq!(
        outbound_target(codec_for("openai-chat").unwrap(), &plain),
        "/chat/completions"
    );
    assert_eq!(
        outbound_target(codec_for("openai-responses").unwrap(), &plain),
        "/responses"
    );
    // Gemini's is the one that is built rather than looked up: the model
    // is a path segment and a streamed request is a different method.
    // Its version segment comes from the codec and NOT from
    // `VERSION_SEGMENT`, because the provider's base URL is the bare
    // host — a relayed Gemini target carries `/v1beta` itself.
    let gemini = codec_for("gemini-generate-content").unwrap();
    assert_eq!(
        outbound_target(gemini, &request_for("gemini-2.5-pro", false)),
        "/v1beta/models/gemini-2.5-pro:generateContent"
    );
    assert_eq!(
        outbound_target(gemini, &request_for("gemini-2.5-pro", true)),
        "/v1beta/models/gemini-2.5-pro:streamGenerateContent?alt=sse"
    );
}

/// The fourth protocol places from its own path shape, and — because no
/// harness speaks it — is refused by name at the ingress rather than
/// translated, with nothing about the request read.
#[test]
fn a_gemini_target_places_to_a_refusal_and_a_gemini_provider_is_a_destination() {
    // Claude Code at a Gemini-only provider: the pair T3 supports.
    assert!(matches!(
        place("/v1/messages", &["gemini-generate-content"]),
        Placement::Translate(pair)
            if pair.slug() == "anthropic-messages->gemini-generate-content"
    ));
    assert!(matches!(
        place("/responses", &["gemini-generate-content"]),
        Placement::Translate(pair)
            if pair.slug() == "openai-responses->gemini-generate-content"
    ));
    assert!(matches!(
        place("/v1/chat/completions", &["gemini-generate-content"]),
        Placement::Translate(pair)
            if pair.slug() == "openai-chat->gemini-generate-content"
    ));
    // A Gemini-shaped request at an Anthropic-only provider: refused by
    // name, and the reason is the one that is true.
    match place(
        "/v1beta/models/gemini-2.5-pro:generateContent",
        &["anthropic-messages"],
    ) {
        Placement::PairRefused { from, refused } => {
            assert_eq!(from, "gemini-generate-content");
            assert!(refused[0].refusal().unwrap().contains("T3b"));
        }
        other => panic!("expected a refused pair, got {other:?}"),
    }
    // Its siblings under the same protocol are refused for the endpoint
    // rule, not for the pair.
    assert!(matches!(
        place(
            "/v1beta/models/gemini-2.5-pro:countTokens",
            &["openai-chat"]
        ),
        Placement::TargetRefused {
            from: "gemini-generate-content"
        }
    ));
    // A served Gemini target never enters a codec — the second lock on
    // the byte-for-byte rule, for the new protocol too.
    assert!(matches!(
        place(
            "/v1beta/models/gemini-2.5-pro:generateContent",
            &["gemini-generate-content"]
        ),
        Placement::Unplaceable
    ));
    // ... and a model listing is still nobody's endpoint.
    assert!(matches!(
        place("/v1beta/models", &["anthropic-messages"]),
        Placement::Unplaceable
    ));
}

#[test]
fn strict_fields_refuse_what_nobody_read_by_its_full_path() {
    let value: serde_json::Value =
        serde_json::json!({"a": 1, "nested": {"b": "x", "surprise": true}});
    let mut top = fields::Fields::of(value, "").unwrap();
    assert_eq!(top.take_u64("a").unwrap(), Some(1));
    let mut nested = top.take_object("nested").unwrap().unwrap();
    assert_eq!(nested.require_string("b").unwrap(), "x");
    let refusal = nested.finish().unwrap_err();
    assert_eq!(refusal.field, "nested.surprise");
    assert!(top.finish().is_ok());

    let wrong = fields::Fields::of(serde_json::json!({"n": "not a number"}), "")
        .unwrap()
        .take_u64("n")
        .unwrap_err();
    assert_eq!(wrong.field, "n");
    assert!(wrong.reason.contains("a string"));
}

/// A decoder that replays a scripted sequence of already-canonical
/// batches, ignoring the raw SSE bytes it is fed. The wire shape that
/// produces this exact canonical sequence from a real provider is
/// `anthropic::EventDecoder` (swarm finding break/gateway-translate#1):
/// its `require_index` only range-checks the provider's index and never
/// compares it to which blocks have started. Scripting the decoder here
/// reproduces that output directly against the real `stream_events`
/// path without duplicating the Anthropic wire format.
struct Scripted {
    batches: std::collections::VecDeque<Result<Vec<StreamEvent>, Unsupported>>,
    finished: bool,
}

impl StreamDecoder for Scripted {
    fn feed(&mut self, _event: &SseEvent) -> Result<Vec<StreamEvent>, Unsupported> {
        self.batches.pop_front().unwrap_or(Ok(Vec::new()))
    }
    fn finish(&mut self) -> Result<Vec<StreamEvent>, Unsupported> {
        self.finished = true;
        Ok(vec![StreamEvent::MessageStop])
    }
    fn is_done(&self) -> bool {
        self.finished
    }
}

fn test_finish(
    outcome: Outcome,
    status: u16,
    framing: Framing,
    tokens: Option<Tokens>,
    first: FirstEvents,
) -> (Exchange, RateLimitHeaders) {
    (
        Exchange {
            outcome,
            status,
            provider: String::new(),
            protocol: None,
            host: String::new(),
            first_byte_at: None,
            first_token_at: first.first_token_at,
            first_tool_call_at: first.first_tool_call_at,
            first_byte_ms: None,
            first_token_ms: first.first_token_ms,
            first_tool_call_ms: first.first_tool_call_ms,
            completed_ms: None,
            framing: Some(framing),
            tokens,
            effort: None,
            turn_shape: None,
            tool_rounds: Some(first.tool_uses),
            repairs: None,
        },
        RateLimitHeaders::default(),
    )
}

/// The acceptance shape from break/gateway-translate#1: `call_A`'s block
/// starts, `call_B`'s block starts before `call_A`'s stops, and a delta
/// addressed to `call_A` (index 0) arrives while `call_B` (index 1) is
/// the open block. Before `canonical::Order` this delta rode on whatever
/// block `openai_responses::EventEncoder` had open — `call_B` — so the
/// harness would have been told `call_B` received `call_A`'s arguments.
/// It must instead be refused by name, before the encoder ever sees it.
#[test]
fn stream_events_refuses_a_delta_that_would_misfile_under_another_calls_id() {
    use std::collections::VecDeque;
    use std::io::Cursor;
    use std::net::TcpListener;

    let mut decoder = Scripted {
        batches: VecDeque::from([
            Ok(vec![StreamEvent::MessageStart {
                id: "msg_fix".to_owned(),
                model: "claude-x".to_owned(),
                usage: canonical::Usage::default(),
            }]),
            Ok(vec![
                StreamEvent::BlockStart {
                    index: 0,
                    block: canonical::BlockStart::ToolUse {
                        id: "call_A".to_owned(),
                        name: "Bash".to_owned(),
                    },
                },
                StreamEvent::BlockStart {
                    index: 1,
                    block: canonical::BlockStart::ToolUse {
                        id: "call_B".to_owned(),
                        name: "Read".to_owned(),
                    },
                },
            ]),
            Ok(vec![StreamEvent::BlockDelta {
                index: 0,
                delta: canonical::Delta::InputJson("{\"command\": \"ls\"}".to_owned()),
            }]),
        ]),
        finished: false,
    };
    let from = codec_for("openai-responses").expect("openai-responses is a registered codec");

    // Three raw placeholder SSE events: `Scripted` ignores their content
    // and returns the canned batches above instead, one per `feed` call.
    let raw = b"event: x\ndata: {}\n\nevent: x\ndata: {}\n\nevent: x\ndata: {}\n\n".to_vec();
    let mut events = SseReader::new(BufReader::new(Cursor::new(raw)));

    let listener = TcpListener::bind("127.0.0.1:0").expect("loopback is bindable");
    let addr = listener.local_addr().expect("bound");
    let mut client = TcpStream::connect(addr).expect("loopback connects");
    let (mut server, _) = listener.accept().expect("loopback accepts");

    let finish: Finish<'_> = &test_finish;
    let served_by = ServedBy::for_test("test-provider", "test-provider/TEST_API_KEY");
    stream_events(
        &mut server,
        &mut events,
        &mut decoder,
        from,
        finish,
        200,
        Instant::now(),
        &served_by,
    );
    drop(server);

    let mut received = Vec::new();
    client
        .read_to_end(&mut received)
        .expect("the client reads whatever the gateway wrote before closing");
    let text = String::from_utf8_lossy(&received);

    assert!(
        text.contains("a delta arrived for a block that is not the open one"),
        "expected the wrong-tool-call-id refusal, got: {text}"
    );
    assert!(
        !text.contains("\"command\": \"ls\""),
        "call_A's argument fragment must never reach the client attached to \
         call_B's item: {text}"
    );
}
