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

/// **No tool-holding helper may be callable from a cell.**
///
/// `run_with_tools` goes through `agent::run_narrowed`, which builds a second
/// V8 isolate. `agent.rs`'s module doc states the hazard directly: the isolate
/// is borrowed while the cell runs, so re-entering the loop from a host
/// callback would re-enter V8 — `bg` runs that loop on another thread instead
/// (`bg.rs:453`). Until a tool-holding helper rides that seam, this asserts the
/// hazard is unreachable.
///
/// Stated as a rule over the roster rather than by naming today's specs, so it
/// also refuses a future spec that adds `CallSite::Cell` beside a toolset.
#[test]
fn no_tool_holding_helper_is_reachable_from_a_cell() {
    let fixture = Fixture::new("no-nested-isolate");
    let mut runtime = Runtime::new(
        &fixture.profile(),
        &Glasshouse::None,
        &SessionId::new("helpers-no-nested"),
    );

    let mut checked = 0;
    for spec in pane::helpers::HELPERS {
        if spec.tools.is_empty() {
            continue;
        }
        checked += 1;
        assert!(
            !spec.call_sites.contains(&CallSite::Cell),
            "`{}` holds tools and declares CallSite::Cell; calling it from a cell \
             would build a nested isolate on the thread already holding one",
            spec.name
        );
        let kind =
            returned_text(&runtime.run_cell(&format!("return typeof helper[{:?}];", spec.name)));
        assert_eq!(
            kind, "undefined",
            "`helper.{}` holds tools and must not be on the global",
            spec.name
        );
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
