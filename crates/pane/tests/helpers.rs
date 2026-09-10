//! Acceptance for the pulled half of `docs/product/pane/little-helpers.md`:
//! `await helper.reduce(text)` from inside a cell.
//!
//! **No test here reaches a real provider.** The two that need a wire call
//! point `ANTHROPIC_BASE_URL` at a listener this file owns; the fail-closed
//! test points it at one that accepts nothing, so "no wire call was
//! attempted" is a count this file measures rather than a claim it makes.

use pane::config::HelpersConfig;
use pane::contract::SessionId;
use pane::glasshouse::Glasshouse;
use pane::helpers::{CallSite, HelperSpec, REDUCER};
use pane::prompt::declarations::callable_from_a_cell;
use pane::runtime::bindings::HostGlobals;
use pane::runtime::isolate::Runtime;
use pane::runtime::outcome::CellOutcome;
use pane::sandbox::profile::Profile;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// `ANTHROPIC_BASE_URL` is process-global, so the tests that set it are
/// serialised against each other exactly as `turns.rs` serialises its own.
static ENV_LOCK: Mutex<()> = Mutex::new(());
static COUNTER: AtomicU64 = AtomicU64::new(0);

struct Fixture {
    root: PathBuf,
}

impl Fixture {
    fn new(label: &str) -> Self {
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let root =
            std::env::temp_dir().join(format!("pane-helpers-{}-{label}-{n}", std::process::id()));
        std::fs::create_dir_all(root.join(".claude")).unwrap();
        Self { root }
    }

    fn profile(&self) -> Profile {
        Profile::compile(&self.root, Some(r#"{"permissions":{"allow":[]}}"#))
    }

    /// A profile with a grant, for the `CallSite::PostResult` tests. Gated
    /// like its only callers: the Windows cell denies warnings and a helper
    /// whose callers are all `unix` tests is dead there.
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    fn profile_with(&self, settings: &str) -> Profile {
        Profile::compile(&self.root, Some(settings))
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

/// A listener that answers every request with one assistant message whose
/// only content is `text`, and counts what it was asked.
struct Provider {
    url: String,
    requests: Arc<AtomicUsize>,
}

fn provider(text: &str) -> Provider {
    provider_with_usage(
        text,
        serde_json::json!({"input_tokens": 10, "output_tokens": 5}),
    )
}

fn provider_with_usage(text: &str, usage: serde_json::Value) -> Provider {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let requests = Arc::new(AtomicUsize::new(0));
    let seen = requests.clone();
    let reply = text.to_string();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { return };
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
                if let Some(value) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                    length = value.trim().parse().unwrap();
                }
            }
            let mut body = vec![0; length];
            if reader.read_exact(&mut body).is_err() {
                return;
            }
            seen.fetch_add(1, Ordering::SeqCst);
            let payload = serde_json::json!({
                "role": "assistant",
                "content": [{"type": "text", "text": reply}],
                "usage": usage
            })
            .to_string();
            let _ = write!(
                stream,
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                payload.len(),
                payload
            );
        }
    });
    Provider {
        url: format!("http://{address}"),
        requests,
    }
}

fn scripted_provider(payloads: Vec<serde_json::Value>) -> Provider {
    scripted_provider_with_delays(
        payloads
            .into_iter()
            .map(|payload| (payload, Duration::ZERO))
            .collect(),
    )
}

fn scripted_provider_with_delays(payloads: Vec<(serde_json::Value, Duration)>) -> Provider {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let requests = Arc::new(AtomicUsize::new(0));
    let seen = requests.clone();
    std::thread::spawn(move || {
        for (payload, delay) in payloads {
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
                if let Some(value) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                    length = value.trim().parse().unwrap();
                }
            }
            let mut body = vec![0; length];
            if reader.read_exact(&mut body).is_err() {
                return;
            }
            seen.fetch_add(1, Ordering::SeqCst);
            std::thread::sleep(delay);
            let payload = payload.to_string();
            let _ = write!(
                stream,
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                payload.len(),
                payload
            );
        }
    });
    Provider {
        url: format!("http://{address}"),
        requests,
    }
}

fn configured(model: &str, calls_per_cell: u32) -> HelpersConfig {
    HelpersConfig {
        model: model.to_string().into(),
        enabled: true,
        calls_per_cell,
        ..HelpersConfig::default()
    }
}

fn threw(outcome: &CellOutcome) -> (String, String) {
    match outcome {
        CellOutcome::Threw { error, .. } => (error.class.clone(), error.message.clone()),
        other => panic!("expected a throw, got {other:?}"),
    }
}

fn returned_text(outcome: &CellOutcome) -> String {
    match outcome {
        CellOutcome::Returned { value, .. } => match value {
            pane::runtime::preview::Value::String(text) => text.head().to_string(),
            other => panic!("expected a string, got {other:?}"),
        },
        other => panic!("expected a return, got {other:?}"),
    }
}

/// The fail-closed default: `[helpers] model` unset means no helper runs, and
/// the refusal is a `ToolError` the model's own program can catch.
#[test]
fn an_unconfigured_helper_refuses_and_makes_no_wire_call() {
    let _environment = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let fixture = Fixture::new("unconfigured");
    let provider = provider("never asked");
    // SAFETY: the environment lock serialises every test in this file that
    // touches these variables, and no other thread here reads them.
    unsafe {
        std::env::set_var("ANTHROPIC_BASE_URL", &provider.url);
    }

    let mut runtime = Runtime::new(
        &fixture.profile(),
        &Glasshouse::None,
        &SessionId::new("helpers-unconfigured"),
    );
    let outcome = runtime.run_cell(
        "try { await helper.reduce(\"a log line\"); return \"no refusal\"; }\n\
         catch (e) { return e.name + \": \" + e.message; }\n",
    );

    unsafe {
        std::env::remove_var("ANTHROPIC_BASE_URL");
    }

    let caught = returned_text(&outcome);
    assert!(
        caught.starts_with("ToolError: "),
        "the refusal must be a catchable ToolError, got {caught:?}"
    );
    assert!(
        caught.contains("[helpers] model"),
        "the refusal must name the configuration that is missing, got {caught:?}"
    );
    assert_eq!(
        provider.requests.load(Ordering::SeqCst),
        0,
        "an unconfigured helper must not reach the wire at all"
    );
    assert!(
        runtime.helper_records().is_empty(),
        "a call that never ran is not a helper record"
    );
}

/// The per-cell ceiling: `calls_per_cell` calls go through and the next one
/// is a refusal the program can catch, so a loop cannot spend without bound.
#[test]
fn the_cell_call_ceiling_refuses_the_call_after_it() {
    let _environment = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let fixture = Fixture::new("ceiling");
    let provider = provider("error[E0433]: failed to resolve");
    unsafe {
        std::env::set_var("ANTHROPIC_BASE_URL", &provider.url);
    }

    let mut runtime = Runtime::new(
        &fixture.profile(),
        &Glasshouse::None,
        &SessionId::new("helpers-ceiling"),
    )
    .with_helpers(configured("test-helper-model", 2));
    let outcome = runtime.run_cell(
        "let done = 0;\n\
         for (let i = 0; i < 3; i++) {\n\
         \x20 try { await helper.reduce(\"a log line\"); done++; }\n\
         \x20 catch (e) { return e.name + \" after \" + done + \": \" + e.message; }\n\
         }\n\
         return \"never refused\";\n",
    );

    unsafe {
        std::env::remove_var("ANTHROPIC_BASE_URL");
    }

    let caught = returned_text(&outcome);
    assert!(
        caught.starts_with("ToolError after 2: "),
        "the third call must be the catchable refusal, got {caught:?}"
    );
    assert!(
        caught.contains("2 helper call"),
        "the refusal must say what the ceiling was, got {caught:?}"
    );
    assert_eq!(
        provider.requests.load(Ordering::SeqCst),
        2,
        "the refused call must not reach the wire"
    );
    assert_eq!(
        runtime.helper_records().len(),
        2,
        "only the calls that ran are records"
    );
}

/// What the lane and the `/cell` inspector read: one record per call, naming
/// the helper and a bounded `asked` that is **not** the payload.
#[test]
fn a_helper_call_leaves_a_record_that_names_it_and_not_its_payload() {
    let _environment = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let fixture = Fixture::new("record");
    let provider = provider("error[E0433]: failed to resolve `foo`");
    unsafe {
        std::env::set_var("ANTHROPIC_BASE_URL", &provider.url);
    }

    let mut runtime = Runtime::new(
        &fixture.profile(),
        &Glasshouse::None,
        &SessionId::new("helpers-record"),
    )
    .with_helpers(configured("test-helper-model", 8));
    // Four distinct lines, one of them a phrase no summary may echo.
    let outcome = runtime.run_cell(
        "const log = [\"warning: unused\", \"SECRET-PAYLOAD-MARKER\", \"error: boom\", \"done\"]\n\
         \x20 .join(\"\\n\");\n\
         return await helper.reduce(log);\n",
    );

    unsafe {
        std::env::remove_var("ANTHROPIC_BASE_URL");
    }

    assert_eq!(
        returned_text(&outcome),
        "error[E0433]: failed to resolve `foo`"
    );
    let records = runtime.helper_records();
    assert_eq!(records.len(), 1, "{records:?}");
    let record = &records[0];
    assert_eq!(record.helper, "reduce");
    assert_eq!(record.verb, "reducing");
    assert_eq!(record.turns, 1);
    assert!(record.outcome.ok, "{record:?}");
    assert_eq!(record.asked, "4 lines", "{record:?}");
    assert_eq!(record.usage.model, "test-helper-model");
    assert_eq!(record.usage.requests, 1);
    assert_eq!(record.usage.reported_requests, 1);
    assert_eq!(record.usage.known_tokens(), 15);
    assert!(
        !record.usage.complete(),
        "omitted cache fields are unknown rather than measured zero: {record:?}"
    );
    assert!(
        !record.asked.contains("SECRET-PAYLOAD-MARKER"),
        "`asked` must describe the payload, never carry it: {record:?}"
    );
}

#[test]
fn a_one_shot_helper_records_every_reported_token_class() {
    let _environment = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let fixture = Fixture::new("usage-complete");
    let provider = provider_with_usage(
        "one failure",
        serde_json::json!({
            "input_tokens": 10,
            "output_tokens": 5,
            "cache_read_input_tokens": 70,
            "cache_creation_input_tokens": 20
        }),
    );
    unsafe { std::env::set_var("ANTHROPIC_BASE_URL", &provider.url) };
    let mut runtime = Runtime::new(
        &fixture.profile(),
        &Glasshouse::None,
        &SessionId::new("helpers-usage-complete"),
    )
    .with_helpers(configured("test-helper-model", 8));

    let outcome = runtime.run_cell("return await helper.reduce(\"a log line\");\n");
    unsafe { std::env::remove_var("ANTHROPIC_BASE_URL") };

    assert_eq!(returned_text(&outcome), "one failure");
    let records = runtime.helper_records();
    let usage = &records[0].usage;
    assert_eq!(usage.model, "test-helper-model");
    assert_eq!(usage.requests, 1);
    assert_eq!(usage.reported_requests, 1);
    assert_eq!(usage.input_tokens, 10);
    assert_eq!(usage.output_tokens, 5);
    assert_eq!(usage.cache_read_input_tokens, 70);
    assert_eq!(usage.cache_creation_input_tokens, 20);
    assert_eq!(usage.known_tokens(), 105);
    assert!(usage.complete());
}

#[test]
fn a_helper_with_no_usage_object_records_unknown_coverage_not_zero_usage() {
    let _environment = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let fixture = Fixture::new("usage-missing");
    let provider = provider_with_usage("one failure", serde_json::Value::Null);
    unsafe { std::env::set_var("ANTHROPIC_BASE_URL", &provider.url) };
    let mut runtime = Runtime::new(
        &fixture.profile(),
        &Glasshouse::None,
        &SessionId::new("helpers-usage-missing"),
    )
    .with_helpers(configured("test-helper-model", 8));

    let outcome = runtime.run_cell("return await helper.reduce(\"a log line\");\n");
    unsafe { std::env::remove_var("ANTHROPIC_BASE_URL") };

    assert_eq!(returned_text(&outcome), "one failure");
    let records = runtime.helper_records();
    let usage = &records[0].usage;
    assert!(usage.coverage_known);
    assert_eq!(usage.requests, 1);
    assert_eq!(usage.reported_requests, 0);
    assert_eq!(usage.known_tokens(), 0, "unknown usage invents no tokens");
    assert!(!usage.complete());
}

#[test]
fn a_multiturn_helper_sums_each_response_once_with_cache_coverage() {
    const TWO_TURN: HelperSpec = HelperSpec {
        name: "two_turn_test",
        summary: "test helper",
        verb: "testing",
        preamble: "Use a cell, then return.",
        tools: &[],
        max_tokens: 128,
        max_turns: 2,
        input: pane::helpers::InputKind::Text,
        output: pane::helpers::OutputKind::Reduction,
        call_sites: &[CallSite::Cell],
    };
    let _environment = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let fixture = Fixture::new("usage-multiturn");
    let provider = scripted_provider(vec![
        serde_json::json!({
            "role": "assistant",
            "content": [{
                "type": "tool_use", "id": "cell-1", "name": "execute_cell",
                "input": {"code": "const observed = 1; console.log(observed);"}
            }],
            "usage": {"input_tokens": 10, "output_tokens": 5,
                "cache_read_input_tokens": 70, "cache_creation_input_tokens": 20}
        }),
        serde_json::json!({
            "role": "assistant",
            "content": [{
                "type": "tool_use", "id": "cell-2", "name": "execute_cell",
                "input": {"code": "return \"found\";"}
            }],
            "usage": {"input_tokens": 11, "output_tokens": 6,
                "cache_read_input_tokens": 71, "cache_creation_input_tokens": 21}
        }),
    ]);
    unsafe { std::env::set_var("ANTHROPIC_BASE_URL", &provider.url) };

    let call = pane::helpers::run(
        &TWO_TURN,
        "test-helper-model",
        "inspect this",
        &fixture.profile(),
        &Glasshouse::None,
        &SessionId::new("helpers-usage-multiturn"),
        &pane::tools::invoke::CancellationToken::new(),
    );
    unsafe { std::env::remove_var("ANTHROPIC_BASE_URL") };

    assert!(call.outcome.ok, "{call:?}");
    assert_eq!(call.outcome.text, "found");
    assert_eq!(call.turns, 2);
    assert_eq!(call.usage.model, "test-helper-model");
    assert_eq!(call.usage.requests, 2);
    assert_eq!(call.usage.reported_requests, 2);
    assert_eq!(call.usage.input_tokens, 21);
    assert_eq!(call.usage.output_tokens, 11);
    assert_eq!(call.usage.cache_read_input_tokens, 141);
    assert_eq!(call.usage.cache_creation_input_tokens, 41);
    assert_eq!(call.usage.known_tokens(), 214);
    assert!(call.usage.complete());
}

#[test]
fn cancellation_keeps_completed_usage_and_marks_the_inflight_request_unknown() {
    const TWO_TURN: HelperSpec = HelperSpec {
        name: "two_turn_cancel_test",
        summary: "test helper",
        verb: "testing",
        preamble: "Use a cell, then return.",
        tools: &[],
        max_tokens: 128,
        max_turns: 2,
        input: pane::helpers::InputKind::Text,
        output: pane::helpers::OutputKind::Reduction,
        call_sites: &[CallSite::Cell],
    };
    let _environment = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let fixture = Fixture::new("usage-cancelled");
    let provider = scripted_provider_with_delays(vec![
        (
            serde_json::json!({
                "role": "assistant",
                "content": [{
                    "type": "tool_use", "id": "cell-1", "name": "execute_cell",
                    "input": {"code": "const observed = 1; console.log(observed);"}
                }],
                "usage": {"input_tokens": 10, "output_tokens": 5,
                    "cache_read_input_tokens": 70, "cache_creation_input_tokens": 20}
            }),
            Duration::ZERO,
        ),
        (
            serde_json::json!({
                "role": "assistant",
                "content": [{"type": "text", "text": "late"}],
                "usage": {"input_tokens": 999, "output_tokens": 999,
                    "cache_read_input_tokens": 999, "cache_creation_input_tokens": 999}
            }),
            Duration::from_millis(400),
        ),
    ]);
    unsafe { std::env::set_var("ANTHROPIC_BASE_URL", &provider.url) };

    let token = pane::tools::invoke::CancellationToken::new();
    let cancel = token.clone();
    let requests = provider.requests.clone();
    let canceller = std::thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(2);
        while requests.load(Ordering::SeqCst) < 2 && Instant::now() < deadline {
            std::thread::yield_now();
        }
        assert_eq!(requests.load(Ordering::SeqCst), 2);
        cancel.cancel();
    });
    let started = Instant::now();
    let call = pane::helpers::run(
        &TWO_TURN,
        "test-helper-model",
        "inspect this",
        &fixture.profile(),
        &Glasshouse::None,
        &SessionId::new("helpers-usage-cancelled"),
        &token,
    );
    canceller.join().unwrap();
    unsafe { std::env::remove_var("ANTHROPIC_BASE_URL") };

    assert!(call.outcome.cancelled, "{call:?}");
    assert!(started.elapsed() < Duration::from_millis(300));
    assert_eq!(call.usage.requests, 2, "one completed and one in flight");
    assert_eq!(call.usage.reported_requests, 1);
    assert_eq!(call.usage.known_tokens(), 105);
    assert_eq!(call.usage.cache_read_reported_requests, 1);
    assert_eq!(call.usage.cache_creation_reported_requests, 1);
    assert!(!call.usage.complete());
}

#[test]
fn preflight_carries_its_helper_usage_into_the_returned_record() {
    let _environment = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let fixture = Fixture::new("usage-preflight");
    let provider = provider_with_usage(
        "src/lib.rs:1 — entry point\nNot checked: other files",
        serde_json::json!({
            "input_tokens": 10,
            "output_tokens": 5,
            "cache_read_input_tokens": 70,
            "cache_creation_input_tokens": 20
        }),
    );
    unsafe { std::env::set_var("ANTHROPIC_BASE_URL", &provider.url) };

    let record = pane::helpers::preflight(
        "Find the repository entry point",
        "test-helper-model",
        &fixture.profile(),
        &Glasshouse::None,
        &SessionId::new("helpers-usage-preflight"),
        &pane::tools::invoke::CancellationToken::new(),
        |_| {},
    )
    .expect("the roster has a preflight helper");
    unsafe { std::env::remove_var("ANTHROPIC_BASE_URL") };

    assert!(record.outcome.ok, "{record:?}");
    assert_eq!(record.usage.model, "test-helper-model");
    assert_eq!(record.usage.requests, 1);
    assert_eq!(record.usage.reported_requests, 1);
    assert_eq!(record.usage.known_tokens(), 105);
    assert!(record.usage.complete());
}

#[test]
fn a_historical_helper_record_deserializes_with_unknown_usage_coverage() {
    let record: pane::helpers::HelperRecord = serde_json::from_value(serde_json::json!({
        "helper": "reduce",
        "verb": "reducing",
        "asked": "old rollout",
        "outcome": {"text": "done", "ok": true, "cancelled": false, "elapsed_ms": 1},
        "turns": 1,
        "looked": []
    }))
    .unwrap();

    assert!(!record.usage.coverage_known);
    assert_eq!(record.usage.known_tokens(), 0);
    assert!(!record.usage.complete());
}

/// A helper that could not answer is a throw, never a reduction that looks
/// healthy — `supervisor.rs` shipped for weeks rendering the opposite.
#[test]
fn a_failed_helper_call_throws_and_is_recorded_as_failed() {
    let _environment = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let fixture = Fixture::new("failed");
    // Nothing is listening on this port, so the request cannot complete.
    unsafe {
        std::env::set_var("ANTHROPIC_BASE_URL", "http://127.0.0.1:9");
    }

    let mut runtime = Runtime::new(
        &fixture.profile(),
        &Glasshouse::None,
        &SessionId::new("helpers-failed"),
    )
    .with_helpers(configured("test-helper-model", 8));
    let outcome = runtime.run_cell("return await helper.reduce(\"a log line\");\n");

    unsafe {
        std::env::remove_var("ANTHROPIC_BASE_URL");
    }

    let (class, message) = threw(&outcome);
    assert_eq!(class, "ToolError", "{message}");
    let records = runtime.helper_records();
    assert_eq!(records.len(), 1, "{records:?}");
    assert!(
        !records[0].outcome.ok,
        "a failed call must never be recorded as a healthy one: {records:?}"
    );
}

/// The roster declares itself: appending a `HelperSpec` is what puts a helper
/// in front of the model, with no second place to edit — and `call_sites` is
/// what decides which helpers a cell is told about at all.
#[test]
fn every_helper_in_the_roster_is_declared_to_the_model() {
    let runtime = pane::prompt::render_runtime();
    assert!(runtime.contains("declare const helper: {"), "{runtime}");
    for spec in pane::helpers::HELPERS {
        let declared =
            runtime.contains(&format!("  {}(text: string): Promise<string>;", spec.name));
        if callable_from_a_cell(spec) {
            assert!(declared, "`{}` is in the roster and undeclared", spec.name);
            assert!(
                runtime.contains(spec.summary),
                "`{}`'s summary is not what the model is shown",
                spec.name
            );
        } else {
            assert!(
                !declared,
                "`{}` may not be called from a cell, so a cell must not be told it exists",
                spec.name
            );
        }
    }
    assert!(
        pane::prompt::declarations::declares_global("helper"),
        "the isolate binds `helper` and the enumeration test reads this"
    );
}

/// `[helpers] enabled = false` must refuse **even with a model configured**.
///
/// The gate this pins fails OPEN when deleted: a user who turned helpers off
/// but left `model` set would have them run and spend, silently. That is the
/// direction that costs money, so it gets its own test rather than riding on
/// the unconfigured case.
#[test]
fn helpers_disabled_with_a_model_configured_still_refuse_and_never_reach_the_wire() {
    let _environment = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let fixture = Fixture::new("disabled");
    let provider = provider("never asked");
    // SAFETY: the environment lock serialises every test in this file that
    // touches these variables, and no other thread here reads them.
    unsafe {
        std::env::set_var("ANTHROPIC_BASE_URL", &provider.url);
    }

    let mut runtime = Runtime::new(
        &fixture.profile(),
        &Glasshouse::None,
        &SessionId::new("helpers-disabled"),
    )
    .with_helpers(HelpersConfig {
        model: "a-real-model".to_string().into(),
        enabled: false,
        calls_per_cell: 8,
        ..HelpersConfig::default()
    });
    let outcome = runtime.run_cell(
        "try { await helper.reduce(\"a log line\"); return \"no refusal\"; }\n\
         catch (e) { return e.name + \": \" + e.message; }\n",
    );

    unsafe {
        std::env::remove_var("ANTHROPIC_BASE_URL");
    }

    let caught = returned_text(&outcome);
    assert!(
        caught.starts_with("ToolError: "),
        "a disabled helper must refuse catchably, got {caught:?}"
    );
    assert!(
        caught.contains("enabled"),
        "the refusal must name `[helpers] enabled`, got {caught:?}"
    );
    assert_eq!(
        provider.requests.load(Ordering::SeqCst),
        0,
        "a disabled helper must not reach the wire even with a model set"
    );
}

/// The `helper` global carries **exactly** the roster entries a cell may
/// call: every one of them, and nothing else.
///
/// The declaration is generated from `HELPERS` and the bindings are installed
/// from `HELPERS`, so this is the test that fails the moment those two drift.
/// A declared helper that is not installed would hand the model a `TypeError`
/// where the declaration promises a catchable `ToolError`; an installed
/// helper that is not declared would be reachable from a cell the spec says
/// may not reach it. Both directions are the same equality, so it is asserted
/// as a set rather than as a loop that only checks one of them.
#[test]
fn every_declared_helper_is_actually_installed() {
    let fixture = Fixture::new("declared-installed");
    let mut runtime = Runtime::new(
        &fixture.profile(),
        &Glasshouse::None,
        &SessionId::new("helpers-declared"),
    );

    let mut expected: Vec<&str> = pane::helpers::HELPERS
        .iter()
        .filter(|spec| callable_from_a_cell(spec))
        .map(|spec| spec.name)
        .collect();
    expected.sort_unstable();
    assert!(
        !expected.is_empty(),
        "the roster must offer a cell at least one helper, or this proves nothing"
    );

    let installed = returned_text(
        &runtime.run_cell("return Object.getOwnPropertyNames(helper).sort().join(\",\");\n"),
    );
    assert_eq!(
        installed,
        expected.join(","),
        "the `helper` global must carry exactly the helpers a cell may call"
    );

    for name in expected {
        let kind = returned_text(&runtime.run_cell(&format!("return typeof helper[{name:?}];")));
        assert_eq!(
            kind, "function",
            "`helper.{name}` is declared to the model but is {kind} on the global"
        );
    }
}

/// A helper whose `call_sites` exclude `Cell` reaches neither the global nor
/// the declaration — `little-helpers.md`'s "where it may be invoked from".
///
/// `HELPERS` is a `const`, so no test can append a rogue entry to the shipped
/// roster and watch it be filtered out, and every entry today names `Cell`.
/// The seam is the predicate, driven with a legal spec naming every call site
/// *except* `Cell` — and because a predicate nothing consults filters nothing,
/// the second half reads `install` and pins that its roster loop is gated on
/// that same function, the way `session.rs` pins its startup validation.
#[test]
fn a_helper_that_may_not_be_called_from_a_cell_is_not_installed() {
    let preflight_only = HelperSpec {
        call_sites: &[
            CallSite::Preflight,
            CallSite::PostResult,
            CallSite::CompletionGate,
        ],
        ..REDUCER
    };
    assert!(
        pane::helpers::check_spec(&preflight_only).is_ok(),
        "the rogue must be a legal spec, or its exclusion proves nothing"
    );
    assert!(
        !callable_from_a_cell(&preflight_only),
        "a spec whose call sites exclude `Cell` must be neither installed nor declared"
    );
    assert!(
        callable_from_a_cell(&HelperSpec {
            call_sites: &[CallSite::Preflight, CallSite::Cell],
            ..REDUCER
        }),
        "a spec naming `Cell` among its call sites must pass the same filter"
    );

    const BINDINGS: &str = include_str!("../src/runtime/bindings.rs");
    let loop_body = BINDINGS
        .split_once("for spec in crate::helpers::HELPERS {")
        .expect("`install` must bind the `helper` global from the roster itself")
        .1
        .split_once("\n    }")
        .expect("that loop must still close at one level inside `install`")
        .0;
    assert!(
        loop_body.contains("callable_from_a_cell"),
        "`install` binds every roster entry without consulting `call_sites`, so a \
         preflight-only helper would be on the global: {loop_body}"
    );
}

/// `helper` may not be shadowed by a cell's own binding.
///
/// The protected set is a list, and a test that iterates that list cannot
/// notice a name missing from it — which is exactly how `write`, `context` and
/// `edit` went unprotected until `9fdd763`. So `helper` is asserted by name.
#[test]
fn a_cell_may_not_shadow_the_helper_global() {
    let fixture = Fixture::new("shadow-helper");
    let mut runtime = Runtime::new(
        &fixture.profile(),
        &Glasshouse::None,
        &SessionId::new("helpers-shadow"),
    );
    let outcome = runtime.run_cell("const marker = 2;\nconst helper = 1;\n");
    let (class, message) = threw(&outcome);
    assert_eq!(
        class, "ShadowsHostFunction",
        "declaring `helper` must be refused at compile time, got {message:?}"
    );
    assert!(
        !runtime.is_live("marker"),
        "the cell must be refused before it runs, but `marker` survived"
    );
}

/// **A tool-holding helper is callable, and is still a capability boundary.**
///
/// Replaces the guard that kept SCOUT and CHECKER off the cell path while two
/// hazards were open: a nested V8 isolate on the borrowed thread (now closed —
/// `run_with_tools` runs the loop on its own thread, the way `bg::serve_once`
/// does), and a runtime that bound every tool regardless of the spec (now
/// closed — `HostGlobals::Helper` carries the spec's own list).
///
/// Stated over the roster rather than by naming today's specs, so a spec added
/// later is held to the same two properties: it is reachable, and reaching it
/// grants nothing it did not name.
#[test]
fn a_tool_holding_helper_is_callable_and_grants_only_what_it_named() {
    let fixture = Fixture::new("tool-holding-callable");
    let mut runtime = Runtime::new(
        &fixture.profile(),
        &Glasshouse::None,
        &SessionId::new("helpers-tool-holding"),
    );

    let mut checked = 0;
    for spec in pane::helpers::HELPERS {
        if spec.tools.is_empty() {
            continue;
        }
        checked += 1;

        if spec.call_sites.contains(&CallSite::Cell) {
            let kind = returned_text(
                &runtime.run_cell(&format!("return typeof helper[{:?}];", spec.name)),
            );
            assert_eq!(
                kind, "function",
                "`{}` declares CallSite::Cell but is not on the global",
                spec.name
            );
        }

        // The boundary itself: whatever it holds, it cannot reach outside the
        // isolate. Driven through the real narrowed constructor, so this fails
        // if a spec's toolset ever admits a mutating tool.
        for forbidden in pane::helpers::FORBIDDEN_TOOLS {
            assert!(
                !spec.tools.contains(&forbidden),
                "`{}` names the mutating tool `{forbidden}`",
                spec.name
            );
        }
    }
    assert!(
        checked >= 2,
        "the roster should carry the tool-holding specs this guards; checked {checked}"
    );
}

/// **A helper's runtime holds nothing that can cause an effect.**
///
/// `little-helpers.md` makes the toolset the safety boundary, and that is
/// true of the registered tools and false of the host globals installed
/// beside them: `bg.run` executes a command, `send` messages another session,
/// `mcp.call` reaches a server. Not one of the three is a tool, so narrowing
/// `spec.tools` never touched them and a Scout could have shelled out.
///
/// The runtime is built the way `helpers::run_with_tools` builds one, through
/// `agent::run_narrowed`. The second half is what stops the first passing for
/// the wrong reason: an ordinary cell still holds all three, so a name that
/// does not exist would fail here rather than read as a narrowing.
#[test]
fn a_helpers_runtime_holds_no_global_that_can_cause_an_effect() {
    // Every name that reaches outside the isolate: the three host globals AND
    // the three mutating tools. The tools are the half that matters most --
    // `bash` executes, and a helper runs `.as_subagent()`, which skips the
    // approval gate entirely.
    const PROGRAM: &str = "return [\"bg\", \"send\", \"mcp\", \"bash\", \"write\", \"edit\"]\n\
         \x20 .map(n => n + \"=\" + typeof globalThis[n]).join(\",\");\n";
    let fixture = Fixture::new("narrowed-globals");

    // A Scout-shaped toolset: read-only tools only, exactly as its spec names.
    let mut helper = Runtime::for_helper(
        &fixture.profile(),
        &Glasshouse::None,
        &SessionId::new("helpers-narrowed"),
        &["read", "grep"],
    )
    .as_subagent()
    .with_instruction_context();
    assert_eq!(
        returned_text(&helper.run_cell(PROGRAM)),
        "bg=undefined,send=undefined,mcp=undefined,bash=undefined,write=undefined,edit=undefined",
        "a helper's runtime must bind nothing that reaches outside the isolate"
    );
    assert_eq!(
        returned_text(
            &helper.run_cell(
                "return [\"read\", \"grep\"].map(n => typeof globalThis[n]).join(\",\");\n"
            )
        ),
        "function,function",
        "and it must still bind the tools its spec did name"
    );

    let mut cell = Runtime::new(
        &fixture.profile(),
        &Glasshouse::None,
        &SessionId::new("helpers-ordinary-cell"),
    );
    assert_eq!(
        returned_text(&cell.run_cell(PROGRAM)),
        "bg=object,send=function,mcp=object,bash=function,write=function,edit=function",
        "an ordinary cell keeps every host global and tool it had"
    );
}

/// The declaration matches what is installed.
///
/// Telling a helper about a global its context does not bind buys a
/// `TypeError` on a name the system block promised, where the point of the
/// narrowing is that the capability is simply absent.
#[test]
fn a_helper_is_never_declared_a_global_its_runtime_does_not_hold() {
    let every = pane::prompt::render_runtime();
    let helper = pane::prompt::render_runtime_for(HostGlobals::Helper(&["read", "grep"]));
    for head in [
        "declare const bg: {",
        "declare function send(",
        "declare const mcp: {",
    ] {
        assert!(every.contains(head), "`{head}` is what a cell is shown");
        assert!(
            !helper.contains(head),
            "a helper is told about `{head}`, which its runtime does not bind"
        );
    }
    assert!(
        helper.contains("declare function keep("),
        "the narrowed block must still declare what a helper does hold"
    );
}

/// The production caller is what asks for the narrowing.
///
/// A helper's loop cannot be run from here without a wire call, so the seam is
/// pinned by reading `agent::run_narrowed`, exactly as this file pins
/// `install`'s roster loop: the branch that decides, the runtime it builds,
/// and the declaration it renders from the same value.
#[test]
fn the_narrowed_loop_is_what_asks_for_a_narrowed_runtime() {
    const AGENT: &str = include_str!("../src/agent.rs");
    let production = AGENT
        .split_once("#[cfg(test)]")
        .map_or(AGENT, |(before, _)| before);
    // The decisive expressions, not their surrounding shape: the narrowing is
    // derived from the SPEC'S OWN toolset, and the same value reaches both the
    // runtime and the system block — so a helper can never be told about a
    // capability it does not hold, nor hold one it was not told about.
    for named in [
        "HostGlobals::Helper(narrowed.tools)",
        "Runtime::for_helper(profile, glasshouse, session, tools)",
        "prompt::render_system_for(&instructions, &tools, &facts, globals)",
    ] {
        assert!(
            production.contains(named),
            "`run_narrowed` no longer carries `{named}`, so a helper's loop may hold \
             globals its spec never named"
        );
    }
}

// --- CallSite::PostResult -----------------------------------------------
//
// The pushed half nothing asks for: an oversized command result is reduced
// by the host, without the model spending a turn to request it. Every test
// below spawns a `bash`, and each is gated to macOS and Linux because those
// are the hosts where `bash` and its brace expansion are certainly present.
// Windows has had an applier since 2026-09-09 and no longer refuses, so the
// gate is now about the runner's own tools rather than about confinement;
// widening it is a named successor in `sandbox-grants.md` §7.

/// The whole grant these tests need. `printf` and brace expansion are both
/// bash builtins, so `bash` is the only binary that is ever exec'd.
#[cfg(any(target_os = "macos", target_os = "linux"))]
const PRINTF_ONLY: &str = r#"{"permissions":{"allow":["Bash(printf*)"]}}"#;

/// A command line whose output is comfortably over
/// `preview::STDOUT_TOKEN_CAP`, so the automatic reduction's own trigger is
/// what fires rather than a number this file chose.
#[cfg(any(target_os = "macos", target_os = "linux"))]
fn oversized_command(marker: &str) -> String {
    format!(r#"printf '{marker} %s\n' {{1..4000}}"#)
}

/// `const r = await bash(…); return r.stdout.length + "|" + <the reduction>`.
#[cfg(any(target_os = "macos", target_os = "linux"))]
fn report_program(command: &str) -> String {
    format!(
        "const r = await bash({{ command: {command:?} }});\n\
         return r.stdout.length + \"|\" + (r.reduced === undefined ? \"none\" : r.reduced);\n"
    )
}

/// Splits `"<length>|<reduction-or-none>"` back into the two facts.
#[cfg(any(target_os = "macos", target_os = "linux"))]
fn reported(outcome: &CellOutcome) -> (usize, String) {
    let text = returned_text(outcome);
    let (length, reduction) = text
        .split_once('|')
        .unwrap_or_else(|| panic!("expected `<length>|<reduction>`, got {text:?}"));
    (length.parse().expect(length), reduction.to_string())
}

/// **A result the model could have printed whole is left alone.**
///
/// The reduction fires without being asked for, so the case that matters
/// most is the one where it must not fire at all: an ordinary command result
/// carries no `reduced`, leaves no record, and reaches no wire.
#[cfg(any(target_os = "macos", target_os = "linux"))]
#[test]
fn a_small_command_result_is_untouched_and_costs_no_helper_call() {
    let _environment = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let fixture = Fixture::new("post-result-small");
    let provider = provider("never asked");
    unsafe {
        std::env::set_var("ANTHROPIC_BASE_URL", &provider.url);
    }

    let mut runtime = Runtime::new(
        &fixture.profile_with(PRINTF_ONLY),
        &Glasshouse::None,
        &SessionId::new("post-result-small"),
    )
    .with_helpers(configured("test-helper-model", 8));
    let outcome = runtime.run_cell(&report_program(r"printf 'error: boom\n'"));

    unsafe {
        std::env::remove_var("ANTHROPIC_BASE_URL");
    }

    let (length, reduction) = reported(&outcome);
    assert_eq!(length, "error: boom\n".len(), "{outcome:?}");
    assert_eq!(
        reduction, "none",
        "a result under the cap is not worth a request"
    );
    assert_eq!(
        provider.requests.load(Ordering::SeqCst),
        0,
        "a small result must not reach the wire at all"
    );
    assert!(
        runtime.helper_records().is_empty(),
        "a call that never ran is not a helper record: {:?}",
        runtime.helper_records()
    );
}

/// **An oversized result is reduced, and the full output is still there.**
///
/// Three facts in one, because they are one path: the handle table offers
/// `reduced` as a key the next turn can read, the program still holds every
/// byte `stdout` carried, and one `HelperRecord` says what the lane and
/// `/cell` show. A summary that replaced the output would make the helper
/// the only witness to it; a reduction nothing names would never be read.
#[cfg(any(target_os = "macos", target_os = "linux"))]
#[test]
fn an_oversized_command_result_is_reduced_and_the_full_output_remains() {
    let _environment = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let fixture = Fixture::new("post-result-big");
    let provider = provider("3 distinct failures");
    unsafe {
        std::env::set_var("ANTHROPIC_BASE_URL", &provider.url);
    }

    let mut runtime = Runtime::new(
        &fixture.profile_with(PRINTF_ONLY),
        &Glasshouse::None,
        &SessionId::new("post-result-big"),
    )
    .with_helpers(configured("test-helper-model", 8));
    let command = oversized_command("error: boom");
    let first = runtime.run_cell(&format!(
        "const r = await bash({{ command: {command:?} }});\n"
    ));

    // The turn the model actually gets: the reduction is a key it can read,
    // never text injected into its context — `little-helpers.md`'s "output
    // never injects itself".
    let table = match &first {
        CellOutcome::Yielded { turn } => turn.table.clone(),
        other => panic!("expected a yield, got {other:?}"),
    };
    assert!(
        table.contains("\"reduced\": string"),
        "the next turn must be told the reduction is there to read: {table}"
    );

    let records = runtime.helper_records();
    assert_eq!(records.len(), 1, "{records:?}");
    let record = &records[0];
    assert_eq!(record.helper, "reduce", "{record:?}");
    assert_eq!(record.verb, "reducing", "{record:?}");
    assert!(record.outcome.ok, "{record:?}");
    assert_eq!(record.asked, "4,000 lines", "{record:?}");
    assert_eq!(record.usage.requests, 1);
    assert_eq!(record.usage.reported_requests, 1);
    assert_eq!(record.usage.known_tokens(), 15);

    let (length, reduction) = reported(&runtime.run_cell(
        "return r.stdout.length + \"|\" + (r.reduced === undefined ? \"none\" : r.reduced);\n",
    ));

    unsafe {
        std::env::remove_var("ANTHROPIC_BASE_URL");
    }

    assert!(
        length / 4 > pane::runtime::preview::STDOUT_TOKEN_CAP,
        "the full output must still be there, and over the cap: {length} chars"
    );
    assert_eq!(
        reduction, "3 distinct failures",
        "the reduction must reach the program as `reduced`"
    );
    assert_eq!(
        provider.requests.load(Ordering::SeqCst),
        1,
        "one oversized result is one reduction"
    );
}

/// **Do not reduce the same value twice.**
///
/// A cell is code, so the same command inside a loop is the ordinary case.
/// The second identical result is served the reduction the first one paid
/// for: one request, one record, and both results carry it.
#[cfg(any(target_os = "macos", target_os = "linux"))]
#[test]
fn the_same_output_is_never_reduced_twice() {
    let _environment = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let fixture = Fixture::new("post-result-twice");
    let provider = provider("3 distinct failures");
    unsafe {
        std::env::set_var("ANTHROPIC_BASE_URL", &provider.url);
    }

    let mut runtime = Runtime::new(
        &fixture.profile_with(PRINTF_ONLY),
        &Glasshouse::None,
        &SessionId::new("post-result-twice"),
    )
    .with_helpers(configured("test-helper-model", 8));
    let command = oversized_command("error: boom");
    let outcome = runtime.run_cell(&format!(
        "const first = await bash({{ command: {command:?} }});\n\
         const second = await bash({{ command: {command:?} }});\n\
         return first.reduced + \"|\" + second.reduced;\n"
    ));

    unsafe {
        std::env::remove_var("ANTHROPIC_BASE_URL");
    }

    assert_eq!(
        returned_text(&outcome),
        "3 distinct failures|3 distinct failures",
        "the second identical result must still carry the reduction"
    );
    assert_eq!(
        provider.requests.load(Ordering::SeqCst),
        1,
        "the second identical result must not be reduced again"
    );
    assert_eq!(
        runtime.helper_records().len(),
        1,
        "a served reduction is not a second call"
    );
}

/// **With helpers unconfigured, an oversized result behaves exactly as
/// today.**
///
/// This is the gate that fails OPEN when it is deleted: a user who never
/// configured a helper would have one run and spend on every large command
/// result, silently.
#[cfg(any(target_os = "macos", target_os = "linux"))]
#[test]
fn an_oversized_result_is_untouched_when_helpers_are_unconfigured() {
    let _environment = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let fixture = Fixture::new("post-result-off");
    let provider = provider("never asked");
    unsafe {
        std::env::set_var("ANTHROPIC_BASE_URL", &provider.url);
    }

    // No `with_helpers`: the default carries no model, which is helpers off.
    let mut runtime = Runtime::new(
        &fixture.profile_with(PRINTF_ONLY),
        &Glasshouse::None,
        &SessionId::new("post-result-off"),
    );
    let outcome = runtime.run_cell(&report_program(&oversized_command("error: boom")));

    unsafe {
        std::env::remove_var("ANTHROPIC_BASE_URL");
    }

    let (length, reduction) = reported(&outcome);
    assert!(
        length / 4 > pane::runtime::preview::STDOUT_TOKEN_CAP,
        "the fixture must be over the cap, got {length} chars"
    );
    assert_eq!(
        reduction, "none",
        "an unconfigured helper must leave the result exactly as it was"
    );
    assert_eq!(
        provider.requests.load(Ordering::SeqCst),
        0,
        "an unconfigured helper must not reach the wire at all"
    );
    assert!(
        runtime.helper_records().is_empty(),
        "a call that never ran is not a helper record"
    );
}

/// **The per-cell ceiling still applies, and reaching it degrades rather
/// than throwing.**
///
/// A reduction the model did not ask for must never be the thing that fails
/// its program: the second oversized result simply arrives without
/// `reduced`, which is today's behaviour.
#[cfg(any(target_os = "macos", target_os = "linux"))]
#[test]
fn the_cell_ceiling_bounds_reductions_nobody_asked_for() {
    let _environment = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let fixture = Fixture::new("post-result-ceiling");
    let provider = provider("3 distinct failures");
    unsafe {
        std::env::set_var("ANTHROPIC_BASE_URL", &provider.url);
    }

    let mut runtime = Runtime::new(
        &fixture.profile_with(PRINTF_ONLY),
        &Glasshouse::None,
        &SessionId::new("post-result-ceiling"),
    )
    .with_helpers(configured("test-helper-model", 1));
    // Two *different* oversized outputs, so the second is a fresh value and
    // the ceiling is the only thing that can stop it.
    let first = oversized_command("error: boom");
    let second = oversized_command("error: other");
    let outcome = runtime.run_cell(&format!(
        "const a = await bash({{ command: {first:?} }});\n\
         const b = await bash({{ command: {second:?} }});\n\
         return (a.reduced === undefined ? \"none\" : a.reduced)\n\
         \x20 + \"|\" + (b.reduced === undefined ? \"none\" : b.reduced);\n"
    ));

    unsafe {
        std::env::remove_var("ANTHROPIC_BASE_URL");
    }

    assert_eq!(
        returned_text(&outcome),
        "3 distinct failures|none",
        "the call past the ceiling must be skipped, not thrown"
    );
    assert_eq!(
        provider.requests.load(Ordering::SeqCst),
        1,
        "the refused reduction must not reach the wire"
    );
    assert_eq!(
        runtime.helper_records().len(),
        1,
        "only the reduction that ran is a record"
    );
}

/// **A reduction nobody asked for never starves a call the model made.**
///
/// Both halves spend one per-cell budget, so an automatic reduction firing on
/// several oversized results could leave the model refused for a `helper.*`
/// call it did make. Slots are reserved for the pulled half.
///
/// Gated like its five siblings above, and for the same reason: it spends
/// `PRINTF_ONLY` and `oversized_command`, which are a bash grant and a brace
/// expansion. Those are gated to the two Unix hosts, so leaving this test
/// ungated does not make it run on Windows — it makes the file fail to
/// compile there, since `-D warnings` is the least of it once the names have
/// gone. It was added after the other five and simply lost the attribute.
#[cfg(any(target_os = "macos", target_os = "linux"))]
#[test]
fn a_pushed_reduction_leaves_slots_for_the_models_own_calls() {
    let _environment = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let fixture = Fixture::new("pushed-vs-pulled");
    let provider = provider("reduced");
    // SAFETY: the environment lock serialises every test in this file.
    unsafe {
        std::env::set_var("ANTHROPIC_BASE_URL", &provider.url);
    }

    // The default ceiling, so the reservation has room to bite.
    let mut runtime = Runtime::new(
        &fixture.profile_with(PRINTF_ONLY),
        &Glasshouse::None,
        &SessionId::new("pushed-vs-pulled"),
    )
    .with_helpers(configured("test-helper-model", 8));

    // NINE distinct oversized results against a ceiling of eight: without the
    // reservation the pushed half consumes every slot and the model's own call
    // is refused for a call it did make. Seven would not discriminate — the
    // model's call would still fit — and a mutation proved that version green.
    let mut program = String::new();
    for i in 0..9 {
        let command = oversized_command(&format!("error: boom {i}"));
        program.push_str(&format!("await bash({{ command: {command:?} }});\n"));
    }
    // Then a call the MODEL makes. It must still be served.
    program.push_str("return await helper.reduce(\"a log the model asked about\");\n");

    let outcome = runtime.run_cell(&program);

    unsafe {
        std::env::remove_var("ANTHROPIC_BASE_URL");
    }

    assert_eq!(
        returned_text(&outcome),
        "reduced",
        "the model's own helper call must survive the pushed reductions"
    );
}
