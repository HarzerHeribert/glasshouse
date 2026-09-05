//! Acceptance for GH-PANE-61E-ISOLATE against
//! `docs/product/pane/runtime-contract.md` §1, §2 and §5.
//!
//! **Every program here is assembled as a string by this file.** No model is
//! called anywhere in this package, and nothing that runs in the isolate came
//! from one — map line 2457 is not touched by these tests.
//!
//! The tests that spawn are gated to the platforms with a sandbox applier
//! that has ever executed (`sandbox-grants.md` §3): on Windows
//! `tools::invoke` refuses rather than spawning unconfined, which is correct
//! and would make a "the tool ran" assertion fail for a reason that has
//! nothing to do with this package.

use pane::contract::SessionId;
use pane::glasshouse::Glasshouse;
use pane::runtime::isolate::Runtime;
use pane::runtime::outcome::CellOutcome;
use pane::runtime::preview::{self, Value};
use pane::sandbox::profile::Profile;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static COUNTER: AtomicU64 = AtomicU64::new(0);

/// A throwaway project root with a `.claude/`, and a directory outside it.
struct Fixture {
    root: PathBuf,
    outside: PathBuf,
}

impl Fixture {
    fn new(label: &str) -> Self {
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let stem = format!("pane-runtime-{}-{label}-{n}", std::process::id());
        let root = std::env::temp_dir().join(&stem);
        std::fs::create_dir_all(root.join(".claude")).unwrap();
        let outside = std::env::temp_dir().join(format!("{stem}-outside"));
        std::fs::create_dir_all(&outside).unwrap();
        Self { root, outside }
    }

    fn profile(&self) -> Profile {
        Profile::compile(&self.root, Some(&settings()))
    }

    fn write(&self, path: &Path, contents: &str) -> PathBuf {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(path, contents).unwrap();
        path.to_path_buf()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
        let _ = std::fs::remove_dir_all(&self.outside);
    }
}

/// `Bash(echo*)` is argv admission and grants no file access at all
/// (`sandbox-grants.md` §2) — which is why it is safe in a fixture.
fn settings() -> String {
    r#"{"permissions":{"allow":["Bash(echo*)","Bash(cat*)"]}}"#.to_string()
}

/// A stand-in for the `glasshouse` binary that records every invocation.
#[cfg(unix)]
fn fake_glasshouse(dir: &Path, log: &Path) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let script = dir.join("fake-glasshouse.sh");
    std::fs::write(
        &script,
        format!(
            "#!/bin/sh\nprintf 'ARGS %s\\n' \"$*\" >> '{log}'\ncat >> '{log}'\nprintf '\\n' >> '{log}'\nexit 0\n",
            log = log.display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    script
}

fn runtime(fixture: &Fixture, glasshouse: &Glasshouse, session: &SessionId) -> Runtime {
    Runtime::new(&fixture.profile(), glasshouse, session)
}

fn returned(outcome: &CellOutcome) -> &Value {
    match outcome {
        CellOutcome::Returned { value, .. } => value,
        other => panic!("expected a return, got {other:?}"),
    }
}

fn returned_string(outcome: &CellOutcome) -> String {
    match returned(outcome) {
        Value::String(text) => text.head().to_string(),
        other => panic!("expected a string, got {other:?}"),
    }
}

// --- §1 and §2: the persistent scope -----------------------------------

#[test]
fn a_top_level_binding_persists_into_the_next_cell_and_a_redeclaration_replaces_it() {
    let fixture = Fixture::new("scope");
    let glasshouse = Glasshouse::None;
    let session = SessionId::new("scope-session");
    let mut runtime = runtime(&fixture, &glasshouse, &session);

    let first = runtime.run_cell("const hits = [1, 2, 3];\nconst other = \"kept\";\n");
    assert!(matches!(first, CellOutcome::Yielded { .. }), "{first:?}");
    assert!(runtime.is_live("hits"));
    assert!(runtime.is_live("other"));

    // Cell 2 reads cell 1's binding by the name the model itself wrote.
    let second = runtime.run_cell("return hits.length + other.length;\n");
    assert_eq!(returned(&second), &Value::Number(7.0));

    // Cell 3 redeclares: not a SyntaxError, and the table says where.
    let third = runtime.run_cell("const hits = [9];\n");
    assert!(matches!(third, CellOutcome::Yielded { .. }), "{third:?}");
    let rendered = runtime.render_handles();
    assert!(
        rendered.contains("hits  Array  (replaced at cell 3)"),
        "{rendered}"
    );
    assert_eq!(runtime.handle_names(), vec!["other", "hits"]);

    // And the replacement is what cell 4 reads.
    let fourth = runtime.run_cell("return hits[0];\n");
    assert_eq!(returned(&fourth), &Value::Number(9.0));
}

#[test]
fn a_cell_that_falls_off_the_end_yields_and_a_top_level_return_ends_the_task() {
    let fixture = Fixture::new("endings");
    let glasshouse = Glasshouse::None;
    let session = SessionId::new("endings-session");
    let mut runtime = runtime(&fixture, &glasshouse, &session);

    let yielded = runtime.run_cell("const a = 1;\nconst b = a + 1;\n");
    match &yielded {
        CellOutcome::Yielded { turn } => {
            assert!(turn.table.contains("a  number"), "{}", turn.table);
            assert!(turn.table.contains("b  number"), "{}", turn.table);
        }
        other => panic!("expected a yield, got {other:?}"),
    }
    assert!(!yielded.ends_the_task());

    let returned_outcome = runtime.run_cell("return { total: a + b };\n");
    assert!(returned_outcome.ends_the_task());
    match returned(&returned_outcome) {
        Value::Object(object) => {
            assert_eq!(object.key_count(), 1);
            assert_eq!(object.entries()[0].0, "total");
        }
        other => panic!("expected an object, got {other:?}"),
    }

    // A bare `return` ends the task too, with undefined.
    let bare = runtime.run_cell("return;\n");
    assert!(bare.ends_the_task());
    assert_eq!(returned(&bare), &Value::Undefined);
}

#[test]
fn a_throw_is_a_result_and_keeps_the_bindings_made_before_it() {
    let fixture = Fixture::new("throw");
    let glasshouse = Glasshouse::None;
    let session = SessionId::new("throw-session");
    let mut runtime = runtime(&fixture, &glasshouse, &session);

    let outcome =
        runtime.run_cell("const before = 41;\nthrow new TypeError(\"boom\");\nconst after = 99;\n");
    let CellOutcome::Threw { error, turn } = &outcome else {
        panic!("expected a throw, got {outcome:?}");
    };
    assert_eq!(error.class, "TypeError");
    assert_eq!(error.message, "boom");
    // The model's own line 2, and the model's own column -- not the
    // wrapper's.
    assert_eq!(error.line, Some(2), "{error:?}");
    assert!(
        error
            .stack
            .iter()
            .all(|frame| frame.description.starts_with("cell ")),
        "a host frame reached the model: {error:?}"
    );

    // §5's third item: the bindings that completed are still there.
    assert!(runtime.is_live("before"), "{turn:?}");
    assert!(!runtime.is_live("after"));
    assert!(turn.table.contains("before  number"), "{}", turn.table);

    // And the next cell recovers in one line, which is the point of it.
    let recovered = runtime.run_cell("return before + 1;\n");
    assert_eq!(returned(&recovered), &Value::Number(42.0));
}

/// A cell that will not even compile is answered in the same turn slot, with
/// the model's own position.
#[test]
fn a_cell_that_does_not_compile_is_a_result_too() {
    let fixture = Fixture::new("syntax");
    let glasshouse = Glasshouse::None;
    let session = SessionId::new("syntax-session");
    let mut runtime = runtime(&fixture, &glasshouse, &session);

    let outcome = runtime.run_cell("const a = 1;\nconst = ;\n");
    let CellOutcome::Threw { error, .. } = &outcome else {
        panic!("expected a throw, got {outcome:?}");
    };
    assert_eq!(error.class, "SyntaxError");
    assert_eq!(error.line, Some(2), "{error:?}");
}

// --- §2: nothing is ever evicted ---------------------------------------

#[test]
fn a_handle_is_never_freed_by_the_runtime() {
    let fixture = Fixture::new("evict");
    let glasshouse = Glasshouse::None;
    let session = SessionId::new("evict-session");
    let mut runtime = runtime(&fixture, &glasshouse, &session);

    // Enough wide handles that the 2,048-token table cap must drop some
    // from the *rendering*.
    let mut program = String::new();
    for i in 0..40 {
        program.push_str(&format!("const h{i} = \"{}\";\n", "x".repeat(900)));
    }
    let outcome = runtime.run_cell(&program);
    let CellOutcome::Yielded { turn } = &outcome else {
        panic!("expected a yield, got {outcome:?}");
    };
    assert!(
        turn.table.contains("older handles not shown"),
        "the table was not over its cap, so this test proves nothing: {}",
        turn.table
    );
    assert!(
        preview::estimate_tokens(&turn.table) <= preview::TABLE_TOKEN_CAP,
        "the rendered table exceeded its own cap"
    );

    // Every one of them is still live, and every one is still addressable
    // from the next cell -- including the ones the rendering dropped.
    for i in 0..40 {
        assert!(runtime.is_live(&format!("h{i}")), "h{i} was freed");
    }
    let sum = runtime.run_cell("return h0.length + h39.length;\n");
    assert_eq!(returned(&sum), &Value::Number(1800.0));

    // The model's own `free` is one of the three things that can shrink it.
    let after_free = runtime.run_cell("free(\"h0\");\nreturn typeof h0;\n");
    assert_eq!(returned_string(&after_free), "undefined");
    assert!(!runtime.is_live("h0"));
    assert!(runtime.is_live("h1"));

    // And the task ending is the third.
    runtime.end_task();
    assert!(runtime.handle_names().is_empty());
}

#[test]
fn keep_names_a_value_the_model_never_bound() {
    let fixture = Fixture::new("keep");
    let glasshouse = Glasshouse::None;
    let session = SessionId::new("keep-session");
    let mut runtime = runtime(&fixture, &glasshouse, &session);

    let outcome = runtime.run_cell("keep(\"chosen\", [1, 2, 3].map(n => n * 2));\n");
    assert!(
        matches!(outcome, CellOutcome::Yielded { .. }),
        "{outcome:?}"
    );
    assert!(runtime.is_live("chosen"));
    let next = runtime.run_cell("return chosen[2];\n");
    assert_eq!(returned(&next), &Value::Number(6.0));
}

// --- §3: the console tail ----------------------------------------------

#[test]
fn console_output_is_capped_to_the_last_512_tokens() {
    let fixture = Fixture::new("console");
    let glasshouse = Glasshouse::None;
    let session = SessionId::new("console-session");
    let mut runtime = runtime(&fixture, &glasshouse, &session);

    let outcome = runtime.run_cell(
        "for (let i = 0; i < 3000; i++) { console.log(\"padding line number \" + i); }\n",
    );
    let turn = outcome.turn();
    assert!(
        preview::estimate_tokens(&turn.stdout_tail) <= preview::STDOUT_TOKEN_CAP,
        "the tail was {} tokens",
        preview::estimate_tokens(&turn.stdout_tail)
    );
    assert!(
        turn.stdout_dropped_tokens > 0,
        "3,000 lines fitted in 512 tokens, which cannot be right"
    );
    assert!(
        turn.stdout_tail.contains("padding line number 2999"),
        "the tail is not the end of the output"
    );
    assert!(
        !turn.stdout_tail.contains("padding line number 0\n"),
        "the tail still contains the beginning"
    );
}

// --- the tools ---------------------------------------------------------

#[cfg(any(target_os = "macos", target_os = "linux"))]
#[test]
fn a_refused_call_throws_permission_denied_inside_the_program_and_is_catchable() {
    let fixture = Fixture::new("denied");
    let secret = fixture.write(&fixture.outside.join("secret.txt"), "OUTSIDE-SECRET\n");
    let glasshouse = Glasshouse::None;
    let session = SessionId::new("denied-session");
    let mut runtime = runtime(&fixture, &glasshouse, &session);

    let program = format!(
        "try {{\n  await read({{ path: {path:?} }});\n  return \"no throw\";\n}} catch (e) {{\n  \
         return [e.name, e instanceof PermissionDenied, e.tool, e.rule.length > 0].join(\"|\");\n}}\n",
        path = secret.to_string_lossy()
    );
    let outcome = runtime.run_cell(&program);
    assert_eq!(
        returned_string(&outcome),
        "PermissionDenied|true|read|true",
        "{outcome:?}"
    );

    // Catchable and final: the turn continued, the task did not end on the
    // refusal itself, and no prompt was involved.
    let after = runtime.run_cell("const still = 1;\n");
    assert!(matches!(after, CellOutcome::Yielded { .. }), "{after:?}");

    // And the refused file's contents never entered the isolate.
    let rendered = runtime.render_handles();
    assert!(!rendered.contains("OUTSIDE-SECRET"), "{rendered}");
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
#[test]
fn a_tool_result_is_a_live_object_the_program_computes_over() {
    let fixture = Fixture::new("live");
    let file = fixture.write(
        &fixture.root.join("notes.txt"),
        "alpha\nbeta\ngamma\ndelta\n",
    );
    let glasshouse = Glasshouse::None;
    let session = SessionId::new("live-session");
    let mut runtime = runtime(&fixture, &glasshouse, &session);

    let program = format!(
        "const doc = await read({{ path: {path:?} }});\n",
        path = file.to_string_lossy()
    );
    let outcome = runtime.run_cell(&program);
    assert!(
        matches!(outcome, CellOutcome::Yielded { .. }),
        "{outcome:?}"
    );

    let rendered = runtime.render_handles();
    assert!(rendered.contains("doc  File"), "{rendered}");
    // §3: the preview shows the first two lines and never the contents.
    assert!(rendered.contains("alpha"), "{rendered}");
    assert!(!rendered.contains("gamma"), "{rendered}");

    // The payload is nevertheless there, in the isolate, for the program.
    let counted = runtime.run_cell("return doc.lines.filter(l => l.length === 5).length;\n");
    assert_eq!(returned(&counted), &Value::Number(3.0));
}

#[cfg(unix)]
#[test]
fn pre_and_post_tool_use_fire_once_per_call_inside_a_program() {
    let fixture = Fixture::new("hooks");
    let inside = fixture.write(&fixture.root.join("inside.txt"), "hook-content\n");
    let log = fixture.root.join("hook.log");
    let script = fake_glasshouse(&fixture.root, &log);
    let glasshouse = Glasshouse::Command { glasshouse: script };
    let session = SessionId::new("hook-session");
    let mut runtime = runtime(&fixture, &glasshouse, &session);

    let program = format!(
        "const doc = await read({{ path: {path:?} }});\n",
        path = inside.to_string_lossy()
    );
    let _ = runtime.run_cell(&program);

    let recorded = std::fs::read_to_string(&log).expect("the hook was delivered");
    assert_eq!(
        recorded
            .matches(r#""hook_event_name":"PreToolUse""#)
            .count(),
        1,
        "{recorded}"
    );
    assert_eq!(
        recorded
            .matches(r#""hook_event_name":"PostToolUse""#)
            .count(),
        1,
        "{recorded}"
    );
    for line in recorded.lines().filter(|line| line.starts_with("ARGS ")) {
        assert_eq!(
            line, "ARGS context-firewall hook --session hook-session",
            "a tool event went somewhere other than the context firewall"
        );
    }

    // Two calls in one program fire twice, and no more.
    std::fs::write(&log, "").unwrap();
    let two = format!(
        "const a = await read({{ path: {path:?} }});\nconst b = await read({{ path: {path:?} }});\n",
        path = inside.to_string_lossy()
    );
    let _ = runtime.run_cell(&two);
    let recorded = std::fs::read_to_string(&log).unwrap();
    assert_eq!(
        recorded
            .matches(r#""hook_event_name":"PreToolUse""#)
            .count(),
        2,
        "{recorded}"
    );
    assert_eq!(
        recorded
            .matches(r#""hook_event_name":"PostToolUse""#)
            .count(),
        2,
        "{recorded}"
    );
}

/// A guard whose condition is false performs no tool call at all: no child,
/// no effect, no hook. The paired positive half runs the *same* call with the
/// *same* grant and must succeed, so a profile that refused everything fails
/// here instead of passing quietly.
#[cfg(unix)]
#[test]
fn a_branch_not_taken_performs_no_tool_call() {
    let fixture = Fixture::new("branch");
    let log = fixture.root.join("hook.log");
    let script = fake_glasshouse(&fixture.root, &log);
    let glasshouse = Glasshouse::Command { glasshouse: script };
    let session = SessionId::new("branch-session");
    let mut runtime = runtime(&fixture, &glasshouse, &session);
    let marker = fixture.root.join("marker");

    let taken = |value: bool| {
        format!(
            "if ({value}) {{ await bash({{ command: \"echo made-it > marker\" }}); }}\nconst done{value} = 1;\n"
        )
    };

    let _ = runtime.run_cell(&taken(false));
    assert!(
        !marker.exists(),
        "a branch that was not taken performed the call"
    );
    let recorded = std::fs::read_to_string(&log).unwrap_or_default();
    assert!(
        !recorded.contains("PreToolUse"),
        "a branch that was not taken fired a hook: {recorded}"
    );

    let _ = runtime.run_cell(&taken(true));
    assert!(
        marker.exists(),
        "the paired positive half did not run, so the negative proves nothing"
    );
    let recorded = std::fs::read_to_string(&log).expect("the hook was delivered");
    assert_eq!(
        recorded
            .matches(r#""hook_event_name":"PreToolUse""#)
            .count(),
        1,
        "{recorded}"
    );
}

/// Every tool result a program binds carries the recorded call that produced
/// it, and `pure` is the tool's own declaration rather than an inference.
#[cfg(any(target_os = "macos", target_os = "linux"))]
#[test]
fn a_bound_tool_result_carries_its_calls_provenance() {
    let fixture = Fixture::new("provenance");
    let file = fixture.write(&fixture.root.join("one.txt"), "hello\n");
    let glasshouse = Glasshouse::None;
    let session = SessionId::new("provenance-session");
    let mut runtime = runtime(&fixture, &glasshouse, &session);

    let program = format!(
        "const doc = await read({{ path: {path:?} }});\n",
        path = file.to_string_lossy()
    );
    let outcome = runtime.run_cell(&program);
    let record = &outcome.turn().record;
    let handle = record
        .handles
        .iter()
        .find(|handle| handle.name == "doc")
        .expect("doc is a handle");
    assert_eq!(handle.type_name, "File");
    let provenance = handle.provenance.as_ref().expect("a recorded call");
    assert_eq!(provenance.tool, "read");
    assert!(provenance.pure, "`read` declares itself pure");
    assert_eq!(provenance.sha256.len(), 64, "{provenance:?}");
    assert_eq!(
        provenance.args.get("path").map(String::as_str),
        Some(&*file.to_string_lossy())
    );
}

// --- the constructor ---------------------------------------------------

/// `runtime-contract.md`'s whole ordering claim, as a property of the type:
/// there is no way to reach a cell executor without the session's compiled
/// profile. The executable proof is the `compile_fail` doctest on
/// `Runtime::new`, which `cargo test -p pane` runs; this test holds the
/// property that doctest depends on, so removing it fails here rather than
/// turning the doctest into a tautology.
#[test]
fn the_runtime_cannot_be_built_without_a_profile() {
    const SOURCE: &str = include_str!("../src/runtime/isolate.rs");
    let production = SOURCE
        .split_once("#[cfg(test)]")
        .map_or(SOURCE, |(before, _)| before);
    let constructors: Vec<&str> = production
        .lines()
        .filter(|line| line.trim_start().starts_with("pub fn ") && line.contains("-> Self"))
        .collect();
    assert!(
        !constructors.is_empty(),
        "no constructor was found at all, so this test would pass for the wrong reason"
    );
    for constructor in &constructors {
        assert!(
            constructor.contains("profile: &Profile"),
            "`{constructor}` builds a Runtime without a Profile"
        );
    }
    assert!(
        !production.contains("impl Default for Runtime"),
        "a Default would be a second constructor with no profile"
    );
}

// --- §2's heap ceiling, and §1's absent event loop ---------------------

/// Crossing the ceiling fails the **cell**, names the five largest live
/// handles so the *model* can choose, and frees nothing. §2 is explicit that
/// a handle vanishing under a program that still names it is the failure
/// that would make the whole channel untrustworthy.
#[test]
fn the_heap_ceiling_fails_the_cell_and_frees_nothing() {
    let fixture = Fixture::new("heap");
    let glasshouse = Glasshouse::None;
    let session = SessionId::new("heap-session");
    let mut runtime =
        Runtime::with_heap_limit(&fixture.profile(), &glasshouse, &session, 32 * 1024 * 1024);

    let first =
        runtime.run_cell("const big = \"b\".repeat(1_000_000);\nconst small = \"s\".repeat(10);\n");
    assert!(matches!(first, CellOutcome::Yielded { .. }), "{first:?}");

    let outcome = runtime
        .run_cell("const junk = [];\nwhile (true) { junk.push(new Array(200000).fill(7)); }\n");
    let CellOutcome::Threw { error, .. } = &outcome else {
        panic!("expected the cell to fail, got {outcome:?}");
    };
    assert_eq!(error.class, "RuntimeOutOfMemory", "{error:?}");
    assert!(
        error.message.contains("big"),
        "the error must name the largest live handles: {}",
        error.message
    );
    assert!(
        error.message.contains("nothing was freed"),
        "{}",
        error.message
    );

    // Nothing was evicted: every handle is still live and still named.
    for name in ["big", "small", "junk"] {
        assert!(runtime.is_live(name), "{name} was freed by the runtime");
    }

    // The model decides. Once it does, the isolate is usable again.
    let recovered = runtime.run_cell("free(\"junk\");\nreturn small.length;\n");
    assert_eq!(returned(&recovered), &Value::Number(10.0), "{recovered:?}");
}

/// There is no event loop, so a promise nothing can settle is answered
/// rather than waited on: a cell that hung would take the whole session with
/// it, and §1 gives a cell exactly two endings plus §5's third.
#[test]
fn an_unsettleable_await_is_answered_not_waited_on() {
    let fixture = Fixture::new("stall");
    let glasshouse = Glasshouse::None;
    let session = SessionId::new("stall-session");
    let mut runtime = runtime(&fixture, &glasshouse, &session);

    let outcome = runtime.run_cell("const before = 1;\nawait new Promise(() => {});\n");
    let CellOutcome::Threw { error, .. } = &outcome else {
        panic!("expected a throw, got {outcome:?}");
    };
    assert_eq!(error.class, "RuntimeStalled", "{error:?}");
    // And §5 still holds: the binding made before it is live.
    assert!(runtime.is_live("before"));
}

/// A `then` chain of ordinary promises still resolves: the one microtask
/// checkpoint drains the queue, including jobs the queue's own jobs enqueue.
#[test]
fn a_promise_chain_settles_without_an_event_loop() {
    let fixture = Fixture::new("chain");
    let glasshouse = Glasshouse::None;
    let session = SessionId::new("chain-session");
    let mut runtime = runtime(&fixture, &glasshouse, &session);

    let outcome = runtime.run_cell(
        "let n = 0;\nfor (let i = 0; i < 50; i++) { n = await Promise.resolve(n + 1); }\nreturn n;\n",
    );
    assert_eq!(returned(&outcome), &Value::Number(50.0), "{outcome:?}");
}

/// TypeScript that has no JavaScript to erase to is refused with a message
/// that says so, in the same turn slot a yield would have used.
#[test]
fn typescript_that_cannot_be_erased_is_a_result_with_a_reason() {
    let fixture = Fixture::new("erase");
    let glasshouse = Glasshouse::None;
    let session = SessionId::new("erase-session");
    let mut runtime = runtime(&fixture, &glasshouse, &session);

    let outcome = runtime.run_cell("enum Colour { Red, Green }\n");
    let CellOutcome::Threw { error, .. } = &outcome else {
        panic!("expected a throw, got {outcome:?}");
    };
    assert_eq!(error.class, "TypeScriptNotErasable");
    assert!(error.message.contains("enum"), "{}", error.message);

    // Types that *can* be erased run untouched, at the model's own columns.
    let typed = runtime.run_cell(
        "interface Row { n: number }\nconst rows: Row[] = [{ n: 1 }, { n: 2 }];\nreturn rows.length;\n",
    );
    assert_eq!(returned(&typed), &Value::Number(2.0), "{typed:?}");
}

/// The isolate has no ambient authority: nothing an embedder did not add is
/// there, and the four tools plus the handle functions are all that was
/// added.
#[test]
fn the_isolate_has_no_ambient_authority() {
    let fixture = Fixture::new("ambient");
    let glasshouse = Glasshouse::None;
    let session = SessionId::new("ambient-session");
    let mut runtime = runtime(&fixture, &glasshouse, &session);

    let absent = [
        "require",
        "process",
        "fetch",
        "setTimeout",
        "setInterval",
        "XMLHttpRequest",
        "WebAssembly",
        "Deno",
        "global",
        "importScripts",
    ];
    let program = format!(
        "return [{}].filter(n => typeof globalThis[n] !== \"undefined\").join(\",\");\n",
        absent
            .iter()
            .map(|name| format!("{name:?}"))
            .collect::<Vec<_>>()
            .join(", ")
    );
    let outcome = runtime.run_cell(&program);
    assert_eq!(returned_string(&outcome), "", "{outcome:?}");

    // And exactly what this package adds is there.
    let present = runtime.run_cell(
        "return [\"grep\",\"read\",\"glob\",\"bash\",\"keep\",\"free\",\"handles\"]\n  .filter(n => typeof globalThis[n] === \"function\").length;\n",
    );
    assert_eq!(returned(&present), &Value::Number(7.0), "{present:?}");
}

/// `runtime-contract.md` §5: a throw carries "the source line and column
/// inside the model's own program" and "no stack from inside the runtime, no
/// host frames". A refusal is thrown from a native callback, so V8 captures
/// no stack for it at all (measured: `stack: []`); what the model gets is the
/// line and column of its own call, read from the message. Lead's test,
/// asserting that positively. The host-frame filter itself is unobservable
/// today — no throw in this runtime can carry a frame from outside a cell —
/// and its mutation is recorded as surviving by construction.
#[cfg(any(target_os = "macos", target_os = "linux"))]
#[test]
fn a_refusal_carries_the_models_own_line_and_column_and_no_host_frame() {
    let fixture = Fixture::new("no-host-frame");
    let glasshouse = Glasshouse::None;
    let session = SessionId::new("no-host-frame");
    let mut runtime = runtime(&fixture, &glasshouse, &session);

    let outcome = runtime.run_cell("const secret = await read({ path: \"/etc/passwd\" });\n");
    let CellOutcome::Threw { error, .. } = &outcome else {
        panic!("a refused read must throw: {outcome:?}");
    };
    assert_eq!(error.class, "PermissionDenied", "{error:?}");
    assert_eq!(
        (error.line, error.column),
        (Some(1), Some(21)),
        "the refusal must point at the model's own `read(` call: {error:?}"
    );
    for frame in &error.stack {
        let inside_program =
            frame.description.starts_with("cell 1,") || frame.description.contains("(cell 1,");
        assert!(
            inside_program,
            "a host frame reached the model: {:?} in {error:?}",
            frame.description
        );
    }
}
