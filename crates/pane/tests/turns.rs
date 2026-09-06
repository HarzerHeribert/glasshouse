//! Acceptance tests for GH-PANE-61C-LOOP: running a task in turns against
//! the Anthropic Messages protocol (map line 2444) and routing through
//! Glasshouse's gateway without changing a byte of the request (map line
//! 2445). No test here makes a network call, per the packet's REQUIRED
//! BEHAVIOR #4.

use std::fs::OpenOptions;
use std::io::{BufRead, BufReader, Read, Write as _};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;

use pane::contract::{Block, Conversation, Message, Role, SessionId};
use pane::rollout::{self, Rollout};
use pane::runtime::outcome::{CellOutcomeKind, CellRecord};
use pane::wire::{self, WireError};

/// `ANTHROPIC_BASE_URL` and `ANTHROPIC_API_KEY` are process-global, and
/// `cargo test` runs this file's tests on several threads at once -- every
/// test below that touches either one holds this for its duration so their
/// sets and removes cannot interleave.
static ENV_LOCK: Mutex<()> = Mutex::new(());

/// A fresh directory unique to the calling test. Parallel test threads can
/// land in the same millisecond, so a monotonic counter breaks the tie that
/// a pid + timestamp pair alone would not.
fn tempdir() -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis();

    let mut dir = std::env::temp_dir();
    dir.push(format!(
        "pane-turns-test-{}-{millis}-{}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn sample_conversation() -> Conversation {
    Conversation {
        system: "You act by writing TypeScript.".to_string(),
        messages: vec![
            Message::text(Role::User, "How many files name IntegrationId?"),
            Message::text(Role::Assistant, "```pane\nreturn 1;\n```"),
        ],
    }
}

/// This is 2445's whole contract: the request body pane builds does not
/// depend on which base URL it will be sent to, so a gateway hop cannot
/// change a byte of it. `ANTHROPIC_BASE_URL` is exercised directly rather
/// than through a bound listener, per the packet's "assert on the
/// serialised body without sending it" option -- `wire::request_body`
/// takes no base-url parameter at all, so this also proves that by
/// construction rather than by care.
#[test]
fn the_gateway_hop_changes_no_byte() {
    let _guard = ENV_LOCK.lock().unwrap();
    let conversation = sample_conversation();

    // SAFETY: `_guard` holds `ENV_LOCK` for the duration of this test, so no
    // other test's `ANTHROPIC_BASE_URL`/`ANTHROPIC_API_KEY` set or remove can
    // interleave with these.
    unsafe {
        std::env::remove_var("ANTHROPIC_BASE_URL");
    }
    assert_eq!(wire::base_url(), wire::DEFAULT_BASE_URL);
    let body_without_gateway = wire::request_body(&conversation);

    unsafe {
        std::env::set_var("ANTHROPIC_BASE_URL", "http://127.0.0.1:9");
    }
    assert_eq!(wire::base_url(), "http://127.0.0.1:9");
    let body_with_gateway = wire::request_body(&conversation);
    unsafe {
        std::env::remove_var("ANTHROPIC_BASE_URL");
    }

    assert_eq!(body_without_gateway, body_with_gateway);
}

#[test]
fn a_turn_is_appended_and_the_file_is_never_rewritten() {
    let path = tempdir().join("session.jsonl");
    let mut rollout = Rollout::create(&path, SessionId::new("s1"), "system prompt").unwrap();

    rollout.record_turn(Role::User, "first").unwrap();
    let first_line_after_one = std::fs::read_to_string(&path)
        .unwrap()
        .lines()
        .next()
        .unwrap()
        .to_string();

    rollout.record_turn(Role::Assistant, "second").unwrap();
    let first_line_after_two = std::fs::read_to_string(&path)
        .unwrap()
        .lines()
        .next()
        .unwrap()
        .to_string();

    assert_eq!(first_line_after_one, first_line_after_two);
}

#[test]
fn resume_rebuilds_the_conversation_from_the_file_alone() {
    let path = tempdir().join("session.jsonl");
    let mut rollout = Rollout::create(&path, SessionId::new("s1"), "system prompt").unwrap();
    rollout.record_turn(Role::User, "hello").unwrap();
    rollout.record_turn(Role::Assistant, "hi there").unwrap();
    drop(rollout);

    let conversation = rollout::resume(&path).unwrap();

    assert_eq!(conversation.system, "system prompt");
    assert_eq!(
        conversation.messages,
        vec![
            Message::text(Role::User, "hello"),
            Message::text(Role::Assistant, "hi there"),
        ]
    );
}

#[test]
fn resume_skips_a_truncated_final_line_and_keeps_the_rest() {
    let path = tempdir().join("session.jsonl");
    {
        let mut rollout = Rollout::create(&path, SessionId::new("s1"), "system prompt").unwrap();
        rollout.record_turn(Role::User, "hello").unwrap();
    }
    let mut file = OpenOptions::new().append(true).open(&path).unwrap();
    // No trailing newline: this is what a process dying mid-append leaves.
    write!(file, r#"{{"kind":"turn","session_id":"s1","turn":1,"rol"#).unwrap();
    drop(file);

    let conversation = rollout::resume(&path).unwrap();

    assert_eq!(conversation.system, "system prompt");
    assert_eq!(
        conversation.messages,
        vec![Message::text(Role::User, "hello")]
    );
}

#[test]
fn resume_skips_a_rollout_kind_it_does_not_know() {
    let path = tempdir().join("session.jsonl");
    {
        let mut rollout = Rollout::create(&path, SessionId::new("s1"), "system prompt").unwrap();
        rollout.record_turn(Role::User, "hello").unwrap();
    }
    let mut file = OpenOptions::new().append(true).open(&path).unwrap();
    writeln!(
        file,
        r#"{{"kind":"cell","session_id":"s1","index":0,"code":"1+1"}}"#
    )
    .unwrap();
    drop(file);

    let conversation = rollout::resume(&path).unwrap();

    assert_eq!(
        conversation.messages,
        vec![Message::text(Role::User, "hello")]
    );
}

#[test]
fn the_system_prompt_is_recorded_once_not_per_turn() {
    let path = tempdir().join("session.jsonl");
    let mut rollout = Rollout::create(&path, SessionId::new("s1"), "system prompt").unwrap();
    rollout.record_turn(Role::User, "one").unwrap();
    rollout.record_turn(Role::Assistant, "two").unwrap();
    rollout.record_turn(Role::User, "three").unwrap();

    let system_lines = std::fs::read_to_string(&path)
        .unwrap()
        .lines()
        .filter(|line| line.contains(r#""kind":"system""#))
        .count();

    assert_eq!(system_lines, 1);
}

/// Re-opening an existing rollout must not destroy it — the property that
/// makes resume possible at all, and the one its sibling above cannot see.
///
/// `a_turn_is_appended_and_the_file_is_never_rewritten` keeps one `Rollout`
/// alive across both writes, so it holds even if the file were opened with
/// `truncate(true)`: the truncation happens once, before the first write.
/// Measured: replacing `.append(true)` with `.write(true).truncate(true)`
/// SURVIVED the whole suite. A session re-opened after a restart would then
/// come back empty, and `resume` would honestly report a conversation that
/// had been silently deleted.
#[test]
fn reopening_a_rollout_appends_to_it_rather_than_truncating_it() {
    let path = tempdir().join("session.jsonl");

    let mut first = Rollout::create(&path, SessionId::new("s1"), "system prompt").unwrap();
    first.record_turn(Role::User, "before the restart").unwrap();
    drop(first);

    let bytes_after_first = std::fs::read(&path).unwrap();

    let mut second = Rollout::create(&path, SessionId::new("s1"), "system prompt").unwrap();
    second
        .record_turn(Role::Assistant, "after the restart")
        .unwrap();
    drop(second);

    let bytes_after_second = std::fs::read(&path).unwrap();
    assert!(
        bytes_after_second.starts_with(&bytes_after_first),
        "re-opening rewrote the rollout instead of appending to it"
    );

    let conversation = rollout::resume(&path).unwrap();
    let texts: Vec<&str> = conversation
        .messages
        .iter()
        .flat_map(|message| message.content.iter().map(|block| block.text()))
        .collect();
    assert_eq!(texts, vec!["before the restart", "after the restart"]);
}

/// Binds an ephemeral local port, answers the single connection it receives
/// with a fixed status line and body, then exits -- one turn only, since
/// `send_turn` never retries.
fn start_status_provider(status_line: &'static str, body: &'static [u8]) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    thread::spawn(move || {
        let Ok((stream, _)) = listener.accept() else {
            return;
        };
        respond(stream, status_line, body);
    });
    format!("http://127.0.0.1:{port}")
}

/// Reads a minimal HTTP/1.1 request (headers, then its declared
/// `Content-Length` body) and discards it, then writes back `status_line`
/// and `body` as the whole response.
fn respond(mut stream: TcpStream, status_line: &str, body: &[u8]) {
    let mut reader = BufReader::new(stream.try_clone().unwrap());
    let mut content_length = 0usize;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).unwrap_or(0) == 0 {
            break;
        }
        if line == "\r\n" || line == "\n" {
            break;
        }
        if let Some(rest) = line.to_ascii_lowercase().strip_prefix("content-length:") {
            content_length = rest.trim().parse().unwrap_or(0);
        }
    }
    let mut request_body = vec![0u8; content_length];
    reader.read_exact(&mut request_body).ok();

    let mut response = format!(
        "HTTP/1.1 {status_line}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    )
    .into_bytes();
    response.extend_from_slice(body);
    stream.write_all(&response).unwrap();
    stream.flush().unwrap();
}

/// Binds an ephemeral local port and drops the listener immediately, so a
/// connection to it is refused fast, locally, and without ever reaching a
/// real host.
fn refused_base_url() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    format!("http://127.0.0.1:{port}")
}

/// Sets `ANTHROPIC_BASE_URL` to `base_url`, runs `send_turn`, then restores
/// the environment. Caller holds `ENV_LOCK`.
fn send_turn_against(base_url: &str) -> Result<wire::Turn, WireError> {
    // SAFETY: the caller holds `ENV_LOCK` for the duration of this call, so
    // no other test's env mutation can interleave with this one.
    unsafe {
        std::env::set_var("ANTHROPIC_BASE_URL", base_url);
    }
    let result = wire::send_turn(&sample_conversation());
    unsafe {
        std::env::remove_var("ANTHROPIC_BASE_URL");
    }
    result
}

/// §6: a response with no `usage` object leaves `Turn::usage` `None`, never a
/// fabricated zero -- a zero would read as "the provider reported no
/// tokens", which is not what an absent object means.
#[test]
fn a_response_without_usage_yields_none_not_zero() {
    let _guard = ENV_LOCK.lock().unwrap();
    let body = br#"{"role":"assistant","content":[{"type":"text","text":"hi"}]}"#;
    let base_url = start_status_provider("200 OK", body);

    let turn = send_turn_against(&base_url).unwrap();
    assert_eq!(turn.usage, None);
}

#[test]
fn a_failed_status_carries_the_head_of_the_providers_body() {
    let _guard = ENV_LOCK.lock().unwrap();
    let body = br#"{"type":"error","error":{"type":"not_found_error","message":"model: claude-sonnet-5"}}"#;
    let base_url = start_status_provider("404 Not Found", body);

    let err = send_turn_against(&base_url).unwrap_err();

    assert!(matches!(err, WireError::Status { status: 404, .. }));
    let display = err.to_string();
    assert!(display.contains("http status: 404"), "{display}");
    assert!(display.contains("not_found_error"), "{display}");
    assert!(display.contains("model: claude-sonnet-5"), "{display}");
}

#[test]
fn a_long_error_body_is_cut_on_a_char_boundary_and_says_so() {
    let _guard = ENV_LOCK.lock().unwrap();
    // 239 ASCII bytes (offsets 0..238), then a 3-byte '€' spanning offsets
    // 239..242 -- byte 240, the cut point, falls in the middle of that
    // character, so the cut must back off to the boundary at 239 rather
    // than split it.
    let mut body = "a".repeat(239);
    body.push('€');
    body.push_str(&"a".repeat(1000 - body.len() - "€".len()));
    let body: &'static str = Box::leak(body.into_boxed_str());
    let base_url = start_status_provider("500 Internal Server Error", body.as_bytes());

    let err = send_turn_against(&base_url).unwrap_err();

    let display = err.to_string();
    let (_, head) = display.split_once('—').unwrap();
    let head = head.trim();
    assert_eq!(head.chars().filter(|&c| c == 'a').count(), 239, "{display}");
    assert!(!head.contains('€'), "{display}");
    assert!(head.ends_with('…'), "{display}");
}

#[test]
fn an_error_body_is_escaped_onto_one_line_and_an_empty_one_says_empty() {
    let _guard = ENV_LOCK.lock().unwrap();
    let base_url = start_status_provider("400 Bad Request", b"line1\nline2\0end");

    let err = send_turn_against(&base_url).unwrap_err();

    let display = err.to_string();
    assert_eq!(display.lines().count(), 1, "{display}");
    assert!(display.contains("line1\\nline2"), "{display}");
    assert!(display.contains("\\u{0}"), "{display}");

    let base_url = start_status_provider("500 Internal Server Error", b"");
    let err = send_turn_against(&base_url).unwrap_err();
    assert!(
        err.to_string().contains("http status: 500 — (empty body)"),
        "{}",
        err
    );
}

#[test]
fn a_transport_failure_still_reports_as_before() {
    let _guard = ENV_LOCK.lock().unwrap();
    let base_url = refused_base_url();

    let err = send_turn_against(&base_url).unwrap_err();

    assert!(matches!(err, WireError::Http(_)));
    assert!(err.to_string().starts_with("request failed:"), "{err}");
}

#[test]
fn no_request_credential_ever_appears_in_an_error() {
    let _guard = ENV_LOCK.lock().unwrap();
    let sentinel = "sk-ant-test-sentinel-do-not-leak";
    // SAFETY: `_guard` holds `ENV_LOCK` for this call and `send_turn_against`
    // scopes `ANTHROPIC_BASE_URL` the same way; no other test reads or
    // writes `ANTHROPIC_API_KEY`.
    unsafe {
        std::env::set_var("ANTHROPIC_API_KEY", sentinel);
    }
    let base_url = start_status_provider("404 Not Found", b"not found");

    let err = send_turn_against(&base_url).unwrap_err();

    unsafe {
        std::env::remove_var("ANTHROPIC_API_KEY");
    }
    assert!(!err.to_string().contains(sentinel), "{err}");
}

/// `runtime-contract.md` §9.2: the terminal response is one assistant
/// `turn` line after the cell line -- the same line an assistant message has
/// always written -- so `resume` rebuilds it from the file alone with no new
/// reader and no new kind, and the cell line stays skipped.
#[test]
fn a_terminal_response_is_one_assistant_turn_line_resume_rebuilds() {
    let path = tempdir().join("session.jsonl");
    let mut rollout = Rollout::create(&path, SessionId::new("s1"), "system prompt").unwrap();
    rollout.record_turn(Role::User, "count them").unwrap();
    rollout
        .record_turn(Role::Assistant, "```pane\nreturn \"three files\";\n```")
        .unwrap();
    rollout
        .record_cell(&CellRecord {
            cell: 1,
            source: "return \"three files\";\n".to_string(),
            outcome: CellOutcomeKind::Returned,
            handles: Vec::new(),
            calls: Vec::new(),
        })
        .unwrap();
    rollout.record_turn(Role::Assistant, "three files").unwrap();
    drop(rollout);

    let raw = std::fs::read_to_string(&path).unwrap();
    let lines: Vec<serde_json::Value> = raw
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    let n = lines.len();
    assert_eq!(lines[n - 2]["kind"], "cell");
    assert_eq!(lines[n - 2]["outcome"], "returned");
    assert_eq!(lines[n - 1]["kind"], "turn");
    assert_eq!(lines[n - 1]["role"], "assistant");
    assert_eq!(lines[n - 1]["text"], "three files");
    assert_eq!(
        lines
            .iter()
            .filter(|line| line["kind"] == "turn" && line["text"] == "three files")
            .count(),
        1,
        "one line, one kind: {raw}"
    );

    let conversation = rollout::resume(&path).unwrap();
    assert_eq!(
        conversation.messages,
        vec![
            Message::text(Role::User, "count them"),
            Message::text(Role::Assistant, "```pane\nreturn \"three files\";\n```"),
            Message::text(Role::Assistant, "three files"),
        ]
    );
}

// --- streaming ---------------------------------------------------------

/// The real event sequence, over a real socket, chunked the way a gateway
/// chunks it: the deltas must reach the caller *before* the reply is
/// complete, which is the whole point of the path.
#[test]
fn a_streamed_turn_hands_over_deltas_as_they_arrive_and_ends_as_one_turn() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let body: &[u8] = b"event: message_start\n\
data: {\"type\":\"message_start\",\"message\":{\"role\":\"assistant\",\"usage\":{\"input_tokens\":11,\"output_tokens\":0}}}\n\
\n\
event: content_block_delta\n\
data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"one \"}}\n\
\n\
event: content_block_delta\n\
data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"two\"}}\n\
\n\
event: message_delta\n\
data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"},\"usage\":{\"input_tokens\":11,\"output_tokens\":7}}\n\
\n\
event: message_stop\n\
data: {\"type\":\"message_stop\"}\n\
\n";
    let base_url = start_status_provider("200 OK", body);
    // SAFETY: `_guard` holds `ENV_LOCK` for this whole test.
    unsafe {
        std::env::set_var("ANTHROPIC_BASE_URL", &base_url);
    }
    let mut deltas: Vec<String> = Vec::new();
    let result = wire::send_turn_streaming(&sample_conversation(), wire::MODEL, &mut |text| {
        deltas.push(text.to_string())
    });
    unsafe {
        std::env::remove_var("ANTHROPIC_BASE_URL");
    }

    let turn = result.expect("the stream should have produced a turn");
    assert_eq!(
        deltas,
        vec!["one ".to_string(), "two".to_string()],
        "the caller was not handed each fragment as it arrived"
    );
    assert_eq!(
        turn.message.content,
        vec![Block::Text("one two".to_string())]
    );
    let usage = turn.usage.expect("usage");
    assert_eq!((usage.input_tokens, usage.output_tokens), (11, 7));
}

/// A stream that ends without `message_stop` is a cut connection, and a cut
/// connection must not read as a short answer.
#[test]
fn a_truncated_stream_is_an_error_and_not_a_partial_answer() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let body: &[u8] = b"event: content_block_delta\n\
data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"half an ans\"}}\n\
\n";
    let base_url = start_status_provider("200 OK", body);
    // SAFETY: `_guard` holds `ENV_LOCK` for this whole test.
    unsafe {
        std::env::set_var("ANTHROPIC_BASE_URL", &base_url);
    }
    let result = wire::send_turn_streaming(&sample_conversation(), wire::MODEL, &mut |_| {});
    unsafe {
        std::env::remove_var("ANTHROPIC_BASE_URL");
    }
    assert!(
        matches!(result, Err(WireError::Stream(_))),
        "a truncated stream became {result:?}"
    );
}

/// The same path against a real gateway. `#[ignore]` because it needs a key
/// and a network: run it with
/// `ANTHROPIC_BASE_URL=... ANTHROPIC_API_KEY=... cargo test -p pane --test
/// turns -- --ignored streams_from_a_real_gateway --nocapture`.
///
/// It exists because a fake socket proves the parse and the framing but not
/// that a live gateway chunks the way this reads it.
#[test]
#[ignore = "needs a real gateway and a credential"]
fn streams_from_a_real_gateway() {
    let conversation = Conversation {
        system: "Answer with the digits only.".to_string(),
        messages: vec![Message::text(Role::User, "Count from 1 to 20.")],
    };
    let mut chunks = 0usize;
    let turn = wire::send_turn_streaming(&conversation, wire::MODEL, &mut |text| {
        chunks += 1;
        print!("{text}");
        use std::io::Write as _;
        std::io::stdout().flush().ok();
    })
    .expect("the gateway should stream");
    println!("\n[{chunks} delta(s), usage {:?}]", turn.usage);
    // **One delta is a pass.** Measured 2026-09-06 against the Experiential
    // gateway: it buffers the whole reply and emits a single `text_delta`
    // even at 561 output tokens, so how finely a reply is chunked is the
    // far end's property and not this transport's. Asserting more than one
    // would fail on a correct gateway and teach nothing about pane.
    assert!(chunks > 0, "no delta arrived before the reply completed");
    assert!(!turn.message.content.is_empty(), "the turn carried no text");
}
