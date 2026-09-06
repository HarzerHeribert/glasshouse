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
        Profile::compile(&self.root, Some(r#"{"permissions":{"allow":["Bash(echo*)"]}}"#))
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
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for _ in 0..turns {
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
            turns: 4,
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
fn a_subagent_that_never_returns_stops_at_its_turn_cap() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let fixture = Fixture::new("cap");
    // A program that always yields: it binds a name and runs off the end.
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
            turns: 2,
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
    assert!(result.stdout.contains("without returning"), "{result:?}");
}
