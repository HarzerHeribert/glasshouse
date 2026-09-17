//! Phase 64 end to end: a subagent runs a real turn loop against a provider
//! and its answer comes back as an event, not as a blocking return.
//!
//! The provider here is a local socket answering a canned Messages reply, so
//! nothing in this file reaches a network or a model.

use pane::agent::AgentOptions;
use pane::bg;
use pane::contract::SessionId;
use pane::events::Kind;
use pane::glasshouse::Glasshouse;
use pane::sandbox::profile::Profile;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

static COUNTER: AtomicU64 = AtomicU64::new(0);
static ENV_LOCK: Mutex<()> = Mutex::new(());

struct Fixture {
    root: PathBuf,
    session: SessionId,
}

impl Fixture {
    fn new(label: &str) -> Self {
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let root =
            std::env::temp_dir().join(format!("pane-subagent-{}-{label}-{n}", std::process::id()));
        std::fs::create_dir_all(root.join(".claude")).unwrap();
        Self {
            root,
            session: SessionId::new(format!("subagent-{label}-{n}")),
        }
    }

    fn profile(&self) -> Profile {
        Profile::compile(
            &self.root,
            Some(r#"{"permissions":{"allow":["Bash(echo*)"]}}"#),
        )
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        bg::shutdown(&self.session);
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

/// A provider that answers every request with the same assistant message.
fn start_provider(reply: &'static str, turns: usize) -> String {
    start_provider_sequence(vec![reply; turns])
}

fn start_provider_sequence(replies: Vec<&'static str>) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for reply in replies {
            let Ok((mut stream, _)) = listener.accept() else {
                return;
            };
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut length = 0usize;
            loop {
                let mut line = String::new();
                if reader.read_line(&mut line).unwrap_or(0) == 0 {
                    return;
                }
                if line == "\r\n" || line == "\n" {
                    break;
                }
                if let Some(rest) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                    length = rest.trim().parse().unwrap_or(0);
                }
            }
            let mut body = vec![0u8; length];
            let _ = reader.read_exact(&mut body);
            let payload = serde_json::json!({
                "role": "assistant",
                "content": [{"type": "text", "text": reply}],
                "usage": {"input_tokens": 11, "output_tokens": 7}
            })
            .to_string();
            let head = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                payload.len()
            );
            let _ = stream.write_all(head.as_bytes());
            let _ = stream.write_all(payload.as_bytes());
            let _ = stream.flush();
        }
    });
    format!("http://127.0.0.1:{port}")
}

fn start_native_provider_sequence(replies: Vec<serde_json::Value>) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for payload in replies {
            let Ok((mut stream, _)) = listener.accept() else {
                return;
            };
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut length = 0usize;
            loop {
                let mut line = String::new();
                if reader.read_line(&mut line).unwrap_or(0) == 0 {
                    return;
                }
                if line == "\r\n" || line == "\n" {
                    break;
                }
                if let Some(rest) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                    length = rest.trim().parse().unwrap_or(0);
                }
            }
            let mut body = vec![0; length];
            let _ = reader.read_exact(&mut body);
            let payload = payload.to_string();
            let head = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                payload.len()
            );
            let _ = stream.write_all(head.as_bytes());
            let _ = stream.write_all(payload.as_bytes());
        }
    });
    format!("http://127.0.0.1:{port}")
}

#[test]
fn subagent_uses_native_cell_handoff_across_turns() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let fixture = Fixture::new("native");
    let reply = |id: &str, code: &str| {
        serde_json::json!({
            "role":"assistant", "content":[{"type":"tool_use","id":id,"name":"execute_cell","input":{"code":code}}],
            "usage":{"input_tokens":11,"output_tokens":7}
        })
    };
    let base = start_native_provider_sequence(vec![
        reply("first", "const answer = 42; console.log(answer);"),
        reply("second", "return `native ${answer}`;"),
    ]);
    unsafe {
        std::env::set_var("ANTHROPIC_BASE_URL", &base);
    }
    let _handle = bg::agent(
        &fixture.profile(),
        &Glasshouse::None,
        &fixture.session,
        "compute",
        &AgentOptions {
            turns: Some(4),
            deadline: None,
            model: "test-model".into(),
            effort: pane::wire::Effort::default(),
        },
    );
    let events = wait_for_event(&fixture.session, Duration::from_secs(20));
    unsafe {
        std::env::remove_var("ANTHROPIC_BASE_URL");
    }
    let done = events
        .iter()
        .find(|event| matches!(event.kind, Kind::AgentDone { .. }))
        .unwrap();
    let result = bg::payload(&fixture.session, done.payload.as_str()).unwrap();
    assert_eq!(result.status, "returned");
    assert_eq!(result.stdout, "native 42");
}

#[test]
fn subagent_continues_after_structured_notebook_output() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let fixture = Fixture::new("structured-output");
    let reply = |id: &str, code: &str| {
        serde_json::json!({
            "role":"assistant", "content":[{"type":"tool_use","id":id,"name":"execute_cell","input":{"code":code}}],
            "usage":{"input_tokens":11,"output_tokens":7}
        })
    };
    let base = start_native_provider_sequence(vec![
        reply("inspect", "return {matchesCount: 0, sampleMatches: []};"),
        reply(
            "answer",
            "return \"recommendations follow from the inspection\";",
        ),
    ]);
    unsafe {
        std::env::set_var("ANTHROPIC_BASE_URL", &base);
    }
    let _handle = bg::agent(
        &fixture.profile(),
        &Glasshouse::None,
        &fixture.session,
        "recommend changes",
        &AgentOptions {
            turns: Some(4),
            deadline: None,
            model: "test-model".into(),
            effort: pane::wire::Effort::default(),
        },
    );
    let events = wait_for_event(&fixture.session, Duration::from_secs(20));
    unsafe {
        std::env::remove_var("ANTHROPIC_BASE_URL");
    }
    let done = events
        .iter()
        .find(|event| matches!(event.kind, Kind::AgentDone { .. }))
        .expect("structured output must not finish the subagent");
    let result = bg::payload(&fixture.session, done.payload.as_str()).unwrap();
    assert_eq!(result.status, "returned");
    assert_eq!(result.stdout, "recommendations follow from the inspection");
}

fn wait_for_event(session: &SessionId, within: Duration) -> Vec<pane::events::Event> {
    let deadline = Instant::now() + within;
    loop {
        let drained = bg::drain(session);
        if !drained.is_empty() {
            return drained;
        }
        if Instant::now() >= deadline {
            return Vec::new();
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// The whole of Phase 64's first line: a handle at once, the work out of band,
/// and the answer arriving as an event rather than as a blocking return.
#[test]
fn a_subagent_answers_in_a_later_event_and_never_blocks_the_caller() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let fixture = Fixture::new("answers");
    let base_url = start_provider("```pane\nreturn \"the answer is 42\";\n```", 2);
    // SAFETY: `_guard` holds `ENV_LOCK` for this whole test.
    unsafe {
        std::env::set_var("ANTHROPIC_BASE_URL", &base_url);
    }

    let started = Instant::now();
    let handle = bg::agent(
        &fixture.profile(),
        &Glasshouse::None,
        &fixture.session,
        "what is six times seven",
        &AgentOptions {
            turns: Some(4),
            deadline: None,
            model: "test-model".to_string(),
            effort: pane::wire::Effort::default(),
        },
    );
    let elapsed = started.elapsed();
    assert!(
        elapsed < Duration::from_millis(500),
        "agent.run blocked for {elapsed:?}; it must return before the subagent has done anything"
    );
    assert!(!handle.is_empty());

    // **The override is held until the event has arrived**, and that is not
    // tidiness. `wire::base_url` is read at request time on the subagent's own
    // thread, so unsetting it when `bg::agent` returns leaves a started
    // subagent pointing at the real provider — which is what happened, with a
    // 401 from api.anthropic.com to prove it.
    let events = wait_for_event(&fixture.session, Duration::from_secs(20));
    unsafe {
        std::env::remove_var("ANTHROPIC_BASE_URL");
    }
    let done = events
        .iter()
        .find(|event| matches!(event.kind, Kind::AgentDone { .. }))
        .expect("an agent.done event should have been delivered");
    assert_eq!(
        done.kind.as_str(),
        "agent.done",
        "a subagent must not arrive as a bg.done"
    );
    assert_eq!(done.source, format!("agent/{handle}"));

    let result = bg::payload(&fixture.session, done.payload.as_str())
        .expect("the completion's payload handle resolves");
    assert_eq!(result.stdout, "the answer is 42");
    assert_eq!(result.status, "returned");
}

/// A subagent that never returns is stopped by its own turn cap, and says so
/// rather than reporting an answer it does not have.
#[test]
fn a_subagent_that_never_returns_stops_at_its_turn_hint_and_keeps_its_work() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let fixture = Fixture::new("cap");
    // A program that always yields: it binds a name and runs off the end.
    // The prose beside it is the subagent's own last words, which are what
    // the parent must receive when the hint runs out.
    let base_url = start_provider(
        "Still narrowing it down; the parser is in config.rs.\n```pane\nconst n = 1;\n```",
        8,
    );
    // SAFETY: `_guard` holds `ENV_LOCK` for this whole test.
    unsafe {
        std::env::set_var("ANTHROPIC_BASE_URL", &base_url);
    }
    bg::agent(
        &fixture.profile(),
        &Glasshouse::None,
        &fixture.session,
        "loop forever",
        &AgentOptions {
            turns: Some(2),
            deadline: None,
            model: "test-model".to_string(),
            effort: pane::wire::Effort::default(),
        },
    );
    let events = wait_for_event(&fixture.session, Duration::from_secs(20));
    unsafe {
        std::env::remove_var("ANTHROPIC_BASE_URL");
    }

    let done = events
        .iter()
        .find(|event| matches!(event.kind, Kind::AgentDone { .. }))
        .expect("a capped subagent still completes");
    let result = bg::payload(&fixture.session, done.payload.as_str()).expect("resolves");
    assert_eq!(result.status, "turns", "{result:?}");
    // **The work survives the stop.** Until 2026-09-17 this answered "the
    // subagent used every turn it was given without returning" and dropped
    // everything the subagent had produced.
    assert!(
        result.stdout.contains("the parser is in config.rs"),
        "a subagent that stops early returns its own last words: {result:?}"
    );
    assert!(
        !result.stdout.contains("without returning"),
        "no placeholder may stand in for the work: {result:?}"
    );
}

/// No turn cap and no turn default: a subagent given no hint works past the
/// eight turns it used to be handed and the twenty-four it could never
/// exceed. The user, 2026-09-17: *"Limits are dumb for abstract tasks … what
/// if it needed 9 or 25. all for nothing?"*
#[test]
fn a_subagent_with_no_turn_hint_works_until_it_answers() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let fixture = Fixture::new("uncapped");
    let mut replies = vec!["```pane\nconst n = 1;\n```"; 25];
    replies.push("```pane\nreturn 'the twenty-sixth turn answered';\n```");
    let base_url = start_provider_sequence(replies);
    // SAFETY: `_guard` holds `ENV_LOCK` for this whole test.
    unsafe {
        std::env::set_var("ANTHROPIC_BASE_URL", &base_url);
    }
    bg::agent(
        &fixture.profile(),
        &Glasshouse::None,
        &fixture.session,
        "take as long as it takes",
        &AgentOptions {
            turns: None,
            deadline: None,
            model: "test-model".to_string(),
            effort: pane::wire::Effort::default(),
        },
    );
    let events = wait_for_event(&fixture.session, Duration::from_secs(60));
    unsafe {
        std::env::remove_var("ANTHROPIC_BASE_URL");
    }

    let done = events
        .iter()
        .find(|event| matches!(event.kind, Kind::AgentDone { .. }))
        .expect("an uncapped subagent completes when it answers");
    let result = bg::payload(&fixture.session, done.payload.as_str()).expect("resolves");
    assert_eq!(result.status, "returned", "{result:?}");
    assert!(
        result.stdout.contains("twenty-sixth turn answered"),
        "{result:?}"
    );
}

/// The wall clock the person configured is what stops a subagent that never
/// answers — and it too keeps the work. `[agents] deadline_minutes` is absent
/// by default, so nothing here fires unless someone asked for it.
#[test]
fn a_configured_deadline_stops_a_subagent_and_keeps_its_work() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let fixture = Fixture::new("deadline");
    let base_url = start_provider(
        "Reading the config parser now.\n```pane\nconst n = 1;\n```",
        200,
    );
    // SAFETY: `_guard` holds `ENV_LOCK` for this whole test.
    unsafe {
        std::env::set_var("ANTHROPIC_BASE_URL", &base_url);
    }
    bg::agent(
        &fixture.profile(),
        &Glasshouse::None,
        &fixture.session,
        "loop forever",
        &AgentOptions {
            turns: None,
            deadline: Some(Duration::from_millis(400)),
            model: "test-model".to_string(),
            effort: pane::wire::Effort::default(),
        },
    );
    let events = wait_for_event(&fixture.session, Duration::from_secs(30));
    unsafe {
        std::env::remove_var("ANTHROPIC_BASE_URL");
    }

    let done = events
        .iter()
        .find(|event| matches!(event.kind, Kind::AgentDone { .. }))
        .expect("a subagent out of time still completes");
    let result = bg::payload(&fixture.session, done.payload.as_str()).expect("resolves");
    assert_eq!(
        result.status, "deadline",
        "time running out is its own stop, not a bare cancellation: {result:?}"
    );
    assert!(
        result.stdout.contains("Reading the config parser"),
        "{result:?}"
    );
    assert!(result.stderr.contains("ran out of time"), "{result:?}");
}

/// Measured 2026-09-17 (session `tlitep-13fv`): three subagents came back
/// `{status: "cancelled", stdout: "", stderr: ""}`, so the parent could not
/// tell an exhausted turn budget from a refusal and started the same doomed
/// subagent twice more. A subagent that stops early now says what it did.
#[test]
fn a_subagent_that_stopped_early_reports_its_turns_and_trajectory() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let fixture = Fixture::new("early");
    let base_url = start_provider("```pane\nconst n = 1;\n```", 8);
    // SAFETY: `_guard` holds `ENV_LOCK` for this whole test.
    unsafe {
        std::env::set_var("ANTHROPIC_BASE_URL", &base_url);
    }
    bg::agent(
        &fixture.profile(),
        &Glasshouse::None,
        &fixture.session,
        "loop forever",
        &AgentOptions {
            turns: Some(2),
            deadline: None,
            model: "test-model".to_string(),
            effort: pane::wire::Effort::default(),
        },
    );
    let events = wait_for_event(&fixture.session, Duration::from_secs(20));
    unsafe {
        std::env::remove_var("ANTHROPIC_BASE_URL");
    }

    let done = events
        .iter()
        .find(|event| matches!(event.kind, Kind::AgentDone { .. }))
        .expect("a capped subagent still completes");
    let result = bg::payload(&fixture.session, done.payload.as_str()).expect("resolves");
    assert_eq!(result.status, "turns", "{result:?}");
    assert!(
        result.stderr.contains("turn hint"),
        "the parent must be able to tell why it stopped: {result:?}"
    );
    assert!(
        result.stderr.contains("2 turn(s)"),
        "the parent must be able to see how far it got: {result:?}"
    );
}

#[test]
fn a_subagent_can_amend_its_parse_failed_cell() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let fixture = Fixture::new("repair");
    let base = start_provider_sequence(vec![
        "```pane\nreturn 'repaired;\n```",
        "```pane-edit\n{\"cell\":1,\"replace\":\"'repaired;\",\"with\":\"'repaired';\"}\n```",
    ]);
    // SAFETY: the environment lock is held until the child has finished.
    unsafe {
        std::env::set_var("ANTHROPIC_BASE_URL", base);
    }
    let result = pane::agent::run(
        &fixture.profile(),
        &Glasshouse::None,
        &fixture.session,
        "answer",
        &AgentOptions {
            turns: Some(2),
            deadline: None,
            model: "test-model".into(),
            effort: pane::wire::Effort::default(),
        },
        &pane::tools::invoke::CancellationToken::new(),
    );
    unsafe {
        std::env::remove_var("ANTHROPIC_BASE_URL");
    }
    assert_eq!(result.status, "returned");
    assert_eq!(result.answer, "repaired");
    assert_eq!(result.turns, 2);
}

#[test]
fn subagent_plain_prose_is_its_result_without_a_marker_round_trip() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let fixture = Fixture::new("explicit-completion");
    let base = start_provider_sequence(vec![
        "I will calculate the answer next.",
        "The answer is 42.\n<!-- pane:done -->",
    ]);
    // SAFETY: serialized with the other environment-dependent tests.
    unsafe {
        std::env::set_var("ANTHROPIC_BASE_URL", base);
    }
    let result = pane::agent::run(
        &fixture.profile(),
        &Glasshouse::None,
        &fixture.session,
        "Answer the question",
        &AgentOptions {
            turns: Some(2),
            deadline: None,
            model: "test-model".into(),
            effort: pane::wire::Effort::default(),
        },
        &pane::tools::invoke::CancellationToken::new(),
    );
    unsafe {
        std::env::remove_var("ANTHROPIC_BASE_URL");
    }
    assert_eq!(result.status, "returned");
    assert_eq!(result.answer, "I will calculate the answer next.");
    assert_eq!(result.turns, 1);
}
