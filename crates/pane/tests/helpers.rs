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
use pane::runtime::isolate::Runtime;
use pane::runtime::outcome::CellOutcome;
use pane::sandbox::profile::Profile;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

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
                "usage": {"input_tokens": 10, "output_tokens": 5}
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

fn configured(model: &str, calls_per_cell: u32) -> HelpersConfig {
    HelpersConfig {
        model: model.to_string().into(),
        enabled: true,
        calls_per_cell,
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
    assert!(
        !record.asked.contains("SECRET-PAYLOAD-MARKER"),
        "`asked` must describe the payload, never carry it: {record:?}"
    );
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
/// in front of the model, with no second place to edit.
#[test]
fn every_helper_in_the_roster_is_declared_to_the_model() {
    let runtime = pane::prompt::render_runtime();
    assert!(runtime.contains("declare const helper: {"), "{runtime}");
    for spec in pane::helpers::HELPERS {
        assert!(
            runtime.contains(&format!("  {}(text: string): Promise<string>;", spec.name)),
            "`{}` is in the roster and undeclared",
            spec.name
        );
        assert!(
            runtime.contains(spec.summary),
            "`{}`'s summary is not what the model is shown",
            spec.name
        );
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

/// Every helper the model is TOLD about must actually be installed.
///
/// The declaration is generated from `HELPERS` while the bindings are
/// installed by hand, so appending a second `HelperSpec` would declare a
/// method that does not exist and the model would call it and get a
/// `TypeError` instead of the catchable `ToolError` the declaration promises.
/// This is the test that fails the moment those two drift.
#[test]
fn every_declared_helper_is_actually_installed() {
    let fixture = Fixture::new("declared-installed");
    let mut runtime = Runtime::new(
        &fixture.profile(),
        &Glasshouse::None,
        &SessionId::new("helpers-declared"),
    );

    for spec in pane::helpers::HELPERS {
        let probe = format!("return typeof helper[{:?}];", spec.name);
        let kind = returned_text(&runtime.run_cell(&probe));
        assert_eq!(
            kind, "function",
            "`helper.{}` is declared to the model but is {kind} on the global",
            spec.name
        );
    }
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
