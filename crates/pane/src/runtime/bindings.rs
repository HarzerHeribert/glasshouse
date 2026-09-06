//! The only authority a cell has — map line 2463's in-program half.
//!
//! **Every one of these functions is the isolate's whole outside world.** The
//! four tools go through [`crate::tools::invoke::run`] and therefore through
//! the merged sandbox; `keep`, `free` and `handles` touch nothing but the
//! handle table; `console` writes to a bounded buffer. There is no file, no
//! socket, no timer, no `require` and no dynamic import here, because a V8
//! context has none of those unless an embedder adds them and this module is
//! the only place that adds anything. `yieldNow` is the eighth function and
//! the one that runs no code: it stops the cell (`runtime-contract.md` §9.3).
//!
//! A refusal is a throw of `PermissionDenied` inside the model's own program
//! (`sandbox-grants.md` §1.4 and §5): catchable, final, and never a prompt.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use crate::bg::{self, RunOptions, WatchOptions};
use crate::events::batch::Batch;
use crate::events::{BatchStore, Event, EventId};
use crate::runtime::cell::{HOST_FUNCTIONS, RESERVED_PREFIX};
use crate::runtime::handles::HandleMeta;
use crate::runtime::isolate::DEFAULT_RESPONSE_BYTE_CAP;
use crate::runtime::marshal;
use crate::runtime::outcome::{CallRecord, Ended, PlanItem, PlanStatus};
use crate::runtime::preview::{
    ArrayValue, FileValue, PREVIEW_TOKEN_CAP, StringValue, Value, thousands,
};
use crate::runtime::state::{RecordedCall, RuntimeState, provenance};
use crate::sandbox::profile::PermissionDenied;
use crate::tools::invoke::{self, Args, ToolContext, ToolError, ToolResult};
use crate::tools::registry::{self, Tool};

/// Declared once so the classes a refusal is thrown as exist before any cell
/// runs, and so the one JIT surface a code-over-objects runtime has no use
/// for is gone.
pub(crate) const BOOTSTRAP: &str = r#"
globalThis.PermissionDenied = class PermissionDenied extends Error {
  constructor(message, tool, path, rule) {
    super(message);
    this.name = "PermissionDenied";
    this.tool = tool;
    this.path = path;
    this.rule = rule;
  }
};
globalThis.ToolError = class ToolError extends Error {
  constructor(message) { super(message); this.name = "ToolError"; }
};
globalThis.Cancelled = class Cancelled extends Error {
  constructor(message, tool) { super(message); this.name = "Cancelled"; this.tool = tool; }
};
delete globalThis.WebAssembly;
delete globalThis.SharedArrayBuffer;
delete globalThis.Atomics;
"#;

/// The private symbol a tool-produced object is tagged with, so the binding
/// that ends up holding it can be given that call's provenance and its
/// already-built preview instead of being marshalled a second time.
///
/// A *private* symbol: invisible to `Object.keys`,
/// `Object.getOwnPropertySymbols` and `JSON.stringify`, so tagging a result
/// cannot change what a model's own program sees of it.
fn call_tag<'s>(scope: &mut v8::PinScope<'s, '_>) -> v8::Local<'s, v8::Private> {
    let name = v8::String::new(scope, "pane.call").expect("a short literal is a valid string");
    v8::Private::for_api(scope, Some(name))
}

/// The private symbol the value `e()` answers with is tagged with, carrying
/// the number of the cell that minted it.
///
/// **This is what makes `runtime-contract.md` §1's two endings decidable
/// rather than declarable.** A cell yields when its promise fulfils with a
/// value carrying this tag for *this* cell, and returns otherwise, so a
/// program that calls `e()` itself and then returns something of its own
/// still returns: the marker is a value only the host can mint, and a marker
/// kept from an earlier cell does not answer for a later one. `v8::Private`
/// is API-only — there is no JavaScript operation that sets one — which is
/// the same property the preview tag above relies on.
///
/// **Only the host can mint it, and the host mints it on demand** for the
/// cell's own `__pane_cell.e()`, so `return __pane_cell.e()` yields by
/// construction — §1's one counterexample to "a top-level `return` ends the
/// task", reachable only by naming an internal the model is never shown, and
/// gaining no authority when it is.
fn end_tag<'s>(scope: &mut v8::PinScope<'s, '_>) -> v8::Local<'s, v8::Private> {
    let name = v8::String::new(scope, "pane.end").expect("a short literal is a valid string");
    v8::Private::for_api(scope, Some(name))
}

/// Whether `value` is the marker [`fell_callback`] minted for cell `cell`.
pub(crate) fn is_end_marker(
    scope: &mut v8::PinScope,
    value: v8::Local<v8::Value>,
    cell: u64,
) -> bool {
    let Ok(object) = v8::Local::<v8::Object>::try_from(value) else {
        return false;
    };
    let tag = end_tag(scope);
    object
        .get_private(scope, tag)
        .and_then(|marker| marker.number_value(scope))
        .is_some_and(|minted| minted == cell as f64)
}

pub(crate) fn state(scope: &v8::PinScope) -> Rc<RuntimeState> {
    scope
        .get_slot::<Rc<RuntimeState>>()
        .expect("the runtime installs its state before any callback can run")
        .clone()
}

/// What the host keeps about a cell's ending beyond [`RuntimeState`]:
/// §9.4's trajectory, §9.3's yield request, and §9.2's response cap. One per
/// runtime, in the isolate's second slot; the trajectory and the request are
/// cleared by the isolate at the start of every cell and consumed when the
/// cell ends, and the cap is read when a cell returns a string.
pub(crate) struct CellTrace {
    calls: RefCell<Vec<CallRecord>>,
    yield_requested: Cell<bool>,
    yield_reason: RefCell<Option<String>>,
    pub(crate) response_byte_cap: Cell<usize>,
}

impl CellTrace {
    pub(crate) fn new() -> Rc<Self> {
        Rc::new(Self {
            calls: RefCell::new(Vec::new()),
            yield_requested: Cell::new(false),
            yield_reason: RefCell::new(None),
            response_byte_cap: Cell::new(DEFAULT_RESPONSE_BYTE_CAP),
        })
    }

    pub(crate) fn begin_cell(&self) {
        self.calls.borrow_mut().clear();
        self.yield_requested.set(false);
        self.yield_reason.borrow_mut().take();
    }

    /// Every call that ran this cell, in order, taken once.
    pub(crate) fn take_calls(&self) -> Vec<CallRecord> {
        std::mem::take(&mut *self.calls.borrow_mut())
    }

    /// `Some(reason)` once per cell when `yieldNow` was called, with the
    /// reason it gave; `None` when it was not.
    pub(crate) fn take_yield(&self) -> Option<Option<String>> {
        if !self.yield_requested.replace(false) {
            return None;
        }
        Some(self.yield_reason.borrow_mut().take())
    }

    fn record(&self, call: CallRecord) {
        self.calls.borrow_mut().push(call);
    }

    fn request_yield(&self, reason: Option<String>) {
        self.yield_requested.set(true);
        *self.yield_reason.borrow_mut() = reason;
    }
}

fn trace(scope: &v8::PinScope) -> Rc<CellTrace> {
    scope
        .get_slot::<Rc<CellTrace>>()
        .expect("the runtime installs its trace before any callback can run")
        .clone()
}

fn js_string<'s>(scope: &mut v8::PinScope<'s, '_>, text: &str) -> v8::Local<'s, v8::Value> {
    v8::String::new(scope, text).map_or_else(|| v8::undefined(scope).into(), |string| string.into())
}

fn set_key(
    scope: &mut v8::PinScope,
    object: v8::Local<v8::Object>,
    key: &str,
    value: v8::Local<v8::Value>,
) {
    if let Some(key) = v8::String::new(scope, key) {
        object.set(scope, key.into(), value);
    }
}

/// The attributes every host function is installed with: not writable and
/// not deletable, so `free("grep")`, `grep = 1`, `delete globalThis.grep` and
/// `Object.defineProperty(globalThis, "grep", …)` all fail rather than
/// costing the task a tool it cannot get back. Not configurable is what
/// closes the `defineProperty` door: redefining a non-configurable property
/// is a `TypeError` by the language, not by a check this crate remembers to
/// make.
fn host_attributes() -> v8::PropertyAttribute {
    v8::PropertyAttribute::READ_ONLY | v8::PropertyAttribute::DONT_DELETE
}

/// [`set_key`] for something a program may not replace.
fn set_fixed_key(
    scope: &mut v8::PinScope,
    object: v8::Local<v8::Object>,
    key: &str,
    value: v8::Local<v8::Value>,
) {
    if let Some(key) = v8::String::new(scope, key) {
        object.define_own_property(scope, key.into(), value, host_attributes());
    }
}

/// Installs every host function on the context's global object.
pub(crate) fn install(scope: &mut v8::PinScope) {
    let context = scope.get_current_context();
    let global = context.global(scope);

    for tool in registry::ALL {
        let name = tool.name();
        let data = js_string(scope, name);
        let Some(function) = v8::Function::builder(tool_callback).data(data).build(scope) else {
            continue;
        };
        set_fixed_key(scope, global, name, function.into());
    }

    if let Some(function) = v8::Function::builder(keep_callback).build(scope) {
        set_fixed_key(scope, global, "keep", function.into());
    }
    if let Some(function) = v8::Function::builder(free_callback).build(scope) {
        set_fixed_key(scope, global, "free", function.into());
    }
    if let Some(function) = v8::Function::builder(handles_callback).build(scope) {
        set_fixed_key(scope, global, "handles", function.into());
    }
    if let Some(function) = v8::Function::builder(yield_now_callback).build(scope) {
        set_fixed_key(scope, global, "yieldNow", function.into());
    }

    // `events-contract.md` §5's three background-job entry points, on one
    // fixed object for the same reason every host function above is fixed: a
    // program that replaced `bg` would lose the only way it has to stop what
    // it started, and nothing could put it back.
    let background = v8::Object::new(scope);
    if let Some(function) = v8::Function::builder(bg_run_callback).build(scope) {
        set_fixed_key(scope, background, "run", function.into());
    }
    if let Some(function) = v8::Function::builder(bg_watch_callback).build(scope) {
        set_fixed_key(scope, background, "watch", function.into());
    }
    if let Some(function) = v8::Function::builder(bg_cancel_callback).build(scope) {
        set_fixed_key(scope, background, "cancel", function.into());
    }
    set_fixed_key(scope, global, "bg", background.into());

    // Subagents. Fixed for the same reason as `bg`: a program that replaced
    // `agent` could not stop what it started.
    let agent = v8::Object::new(scope);
    if let Some(function) = v8::Function::builder(agent_run_callback).build(scope) {
        set_fixed_key(scope, agent, "run", function.into());
    }
    set_fixed_key(scope, global, "agent", agent.into());

    // The model's own plan. Fixed like every other host object: a program
    // that replaced `todo` would leave the screen showing a checklist nothing
    // could update.
    let todo = v8::Object::new(scope);
    if let Some(function) = v8::Function::builder(todo_write_callback).build(scope) {
        set_fixed_key(scope, todo, "write", function.into());
    }
    if let Some(function) = v8::Function::builder(todo_read_callback).build(scope) {
        set_fixed_key(scope, todo, "read", function.into());
    }
    set_fixed_key(scope, global, "todo", todo.into());

    let console = v8::Object::new(scope);
    for method in ["log", "info", "warn", "error", "debug", "trace"] {
        if let Some(function) = v8::Function::builder(console_callback).build(scope) {
            set_key(scope, console, method, function.into());
        }
    }
    set_key(scope, global, "console", console.into());
    // The declared API is usable before the first event arrives. No handle
    // preview or event store is allocated until an actual batch is delivered.
    install_batch(scope);
}

/// The per-cell host object the generated wrapper takes as its parameter:
/// `s` captures a completed top-level binding, `e` records that the body ran
/// off its end. It is a parameter rather than a global so it is unreachable
/// the moment the cell's function returns.
pub(crate) fn host_object<'s>(scope: &mut v8::PinScope<'s, '_>) -> v8::Local<'s, v8::Object> {
    let object = v8::Object::new(scope);
    // Fixed for the same reason the globals are: a program that replaced `e`
    // would decide its own cell's ending, and one that replaced `s` would
    // decide what the table says it bound.
    if let Some(function) = v8::Function::builder(capture_callback).build(scope) {
        set_fixed_key(scope, object, "s", function.into());
    }
    if let Some(function) = v8::Function::builder(fell_callback).build(scope) {
        set_fixed_key(scope, object, "e", function.into());
    }
    object
}

// --- the four tools ----------------------------------------------------

fn tool_callback(
    scope: &mut v8::PinScope,
    args: v8::FunctionCallbackArguments,
    mut retval: v8::ReturnValue,
) {
    let name = args.data().to_rust_string_lossy(scope);
    let Some(tool) = registry::lookup(&name) else {
        // Unreachable through `install`, which binds only registered names,
        // and a refusal rather than a panic if it ever is reached.
        throw_denied(
            scope,
            &PermissionDenied {
                tool: name.clone(),
                path: String::new(),
                rule: format!("no tool named `{name}` is registered"),
            },
        );
        return;
    };

    let call_args = read_arguments(scope, args.get(0));
    let state = state(scope);
    // Cloned rather than borrowed across the call: the call is the longest
    // thing this crate does, and a `RefCell` borrow held across it would
    // outlive every reason to hold it. Every clone names the same flag.
    let token = state.token.borrow().clone();
    let traced = {
        let context = ToolContext {
            profile: &state.profile,
            glasshouse: &state.glasshouse,
            session: &state.session,
        };
        invoke::run_traced(&context, &token, tool.name(), &call_args)
    };

    // §9.4: recorded here because this is where every call funnels, and
    // recorded with the arguments `invoke` checked rather than the ones the
    // program wrote. Each class below is the class the matching throw
    // constructs, so the line says what the program could have caught.
    let ended = match &traced.outcome {
        Ok(_) => Ended::Ok,
        Err(ToolError::Denied(denied)) => Ended::Denied {
            rule: denied.rule.clone(),
        },
        Err(ToolError::Cancelled { .. }) => Ended::Threw {
            class: "Cancelled".to_string(),
        },
        Err(ToolError::Spawn { .. }) => Ended::Threw {
            class: "ToolError".to_string(),
        },
    };
    trace(scope).record(CallRecord {
        tool: tool.name().to_string(),
        args: traced.checked,
        ended,
    });

    match traced.outcome {
        Ok(result) => {
            let value = typed_result(scope, tool, &call_args, &result, &state);
            retval.set(value);
        }
        Err(ToolError::Denied(denied)) => throw_denied(scope, &denied),
        Err(ToolError::Cancelled { tool }) => throw_cancelled(scope, &tool),
        Err(other) => throw_tool_error(scope, &other.to_string()),
    }
}

/// Reads the call's single object argument into [`Args`].
///
/// Every own property is passed through, including one the tool does not
/// declare: [`invoke::run`] refuses an undeclared argument, and dropping it
/// here would make a call that named the wrong argument look like it had
/// honoured it.
fn read_arguments(scope: &mut v8::PinScope, value: v8::Local<v8::Value>) -> Args {
    let mut args = Args::new();
    if !value.is_object() {
        return args;
    }
    let Ok(object) = v8::Local::<v8::Object>::try_from(value) else {
        return args;
    };
    let Some(names) = object.get_own_property_names(scope, v8::GetPropertyNamesArgs::default())
    else {
        return args;
    };
    for index in 0..names.length() {
        let Some(key) = names.get_index(scope, index) else {
            continue;
        };
        let Some(given) = object.get(scope, key) else {
            continue;
        };
        if given.is_undefined() || given.is_null() {
            continue;
        }
        args = args.with(
            key.to_rust_string_lossy(scope),
            given.to_rust_string_lossy(scope),
        );
    }
    args
}

/// Builds the tool's declared result type, records the call, and tags the
/// object so the binding that holds it inherits the call's provenance.
fn typed_result<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    tool: &Tool,
    args: &Args,
    result: &ToolResult,
    state: &Rc<RuntimeState>,
) -> v8::Local<'s, v8::Value> {
    // Before the builders, because every one of them reads `stdout` and none
    // of them reads the exit code: a `read` of a missing file produced a
    // `File` handle of 0 bytes carrying the SHA-256 of the empty string, and
    // a model quoted it as the task's answer.
    if let Some(message) = call_failure(tool.name(), result) {
        throw_tool_error(scope, &message);
        // The exception is what the callback answers with; V8 discards a
        // return value once one is pending, and nothing below has run, so no
        // call is recorded and no handle is minted.
        return v8::undefined(scope).into();
    }

    let (value, preview, label) = match tool.name() {
        "read" => {
            let (value, preview) = build_file(scope, args, result);
            (value, preview, "File")
        }
        "grep" => {
            let (value, preview) = build_grep(scope, args, result);
            (value, preview, "Grep.Match[]")
        }
        "glob" => {
            let (value, preview) = build_glob(scope, result);
            (value, preview, "string[]")
        }
        _ => {
            let (value, preview) = build_bash(scope, result);
            (value, preview, "Bash.Result")
        }
    };

    let meta = HandleMeta {
        type_label: Some(label.to_string()),
        size_estimate: (result.stdout.len() + result.stderr.len()) as u64,
        provenance: Some(provenance(
            tool.name(),
            args,
            &result.stdout,
            tool.purity().may_rematerialise(),
        )),
    };
    let id = state.record_call(RecordedCall { preview, meta });
    if let Ok(object) = v8::Local::<v8::Object>::try_from(value) {
        let tag = call_tag(scope);
        // A `Number`, not an `Integer`: the id counts every call of the whole
        // task, and an `i32` would wrap where a task made two billion of them.
        let marker = v8::Number::new(scope, id as f64);
        object.set_private(scope, tag, marker.into());
    }
    value
}

/// Why a tool call cannot become a result — `runtime-contract.md` §9.1's *a
/// failed call cannot itself become an answer*.
///
/// **`grep` and `glob` keep exit 1**, which is "no matches" and is an empty
/// array rather than a failure; exit 2 and above is a bad pattern or an
/// unreadable root and throws like everything else. **`bash` is never refused
/// here**: its exit code is part of its declared result and the program reads
/// it. A child killed by a signal has no exit status (`None`) and is left to
/// the builders, which is the behaviour that was there before.
fn call_failure(tool: &str, result: &ToolResult) -> Option<String> {
    let code = result.exit_code?;
    let tolerated = match tool {
        "bash" => return None,
        "grep" | "glob" => code <= 1,
        _ => code == 0,
    };
    if tolerated {
        return None;
    }
    // One line, bounded like a preview: the model needs the reason, not the
    // child's whole diagnostic.
    let detail = result
        .stderr
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map_or_else(
            || "the child wrote nothing to stderr".to_string(),
            |line| line.chars().take(200).collect::<String>(),
        );
    Some(format!("`{tool}` failed with exit {code}: {detail}"))
}

fn build_file<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    args: &Args,
    result: &ToolResult,
) -> (v8::Local<'s, v8::Value>, Value) {
    let path = args.get("path").unwrap_or_default().to_string();
    let lines: Vec<&str> = result.stdout.lines().collect();

    let object = v8::Object::new(scope);
    let path_value = js_string(scope, &path);
    set_key(scope, object, "path", path_value);
    let bytes = v8::Number::new(scope, result.stdout.len() as f64);
    set_key(scope, object, "bytes", bytes.into());
    let count = v8::Number::new(scope, lines.len() as f64);
    set_key(scope, object, "lineCount", count.into());
    let text = js_string(scope, &result.stdout);
    set_key(scope, object, "text", text);
    let array = v8::Array::new(scope, lines.len() as i32);
    for (index, line) in lines.iter().enumerate() {
        let line = js_string(scope, line);
        array.set_index(scope, index as u32, line);
    }
    set_key(scope, object, "lines", array.into());
    // Content and admitted metadata arrive together from the tool layer.
    let mtime = result.modified.clone().unwrap_or_else(|| "unknown".into());
    let mtime_value = js_string(scope, &mtime);
    set_key(scope, object, "mtime", mtime_value);

    let preview = Value::File(FileValue {
        path,
        byte_len: result.stdout.len() as u64,
        line_count: lines.len() as u64,
        mtime,
        lines: lines.iter().take(2).map(|line| line.to_string()).collect(),
    });
    (object.into(), preview)
}

fn build_grep<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    args: &Args,
    result: &ToolResult,
) -> (v8::Local<'s, v8::Value>, Value) {
    let fallback = args.get("path").unwrap_or_default();
    let matches: Vec<GrepMatch> = result
        .stdout
        .lines()
        .filter(|line| !line.is_empty())
        .map(|line| parse_match(line, fallback))
        .collect();

    let array = v8::Array::new(scope, matches.len() as i32);
    for (index, found) in matches.iter().enumerate() {
        let object = v8::Object::new(scope);
        let path = js_string(scope, &found.path);
        set_key(scope, object, "path", path);
        // `null`, not `0`: `grep -r` prints lines that are not located
        // matches — `Binary file … matches` is the routine one — and giving
        // them a line number makes them indistinguishable from a hit at the
        // top of a file, so a program cannot filter them out.
        let line: v8::Local<v8::Value> = match found.line {
            Some(line) => v8::Number::new(scope, line as f64).into(),
            None => v8::null(scope).into(),
        };
        set_key(scope, object, "line", line);
        let text = js_string(scope, &found.text);
        set_key(scope, object, "text", text);
        array.set_index(scope, index as u32, object.into());
    }

    // The preview shows the match, not the object's shape: §3's depth-1
    // object rendering would say `Object n_keys=3`, and §6's worked table
    // shows `path:line "text"`.
    let render = |found: &GrepMatch| {
        Value::String(StringValue::sampled(
            found.path.chars().count() + found.text.chars().count(),
            match_line(found),
        ))
    };
    let head: Vec<Value> = matches.iter().take(3).map(render).collect();
    let last = if matches.len() > 3 {
        matches.last().map(render)
    } else {
        None
    };
    (
        array.into(),
        Value::Array(ArrayValue::sampled(matches.len(), head, last)),
    )
}

struct GrepMatch {
    path: String,
    /// The line the match is on, and `None` for a line `grep` printed that
    /// is not a located match at all.
    line: Option<u64>,
    text: String,
}

/// One match as `runtime-contract.md` §6's worked table shows it.
fn match_line(found: &GrepMatch) -> String {
    let text: String = found.text.chars().take(160).collect();
    match found.line {
        Some(line) => format!("{}:{}  {text}", found.path, line),
        None => text,
    }
}

/// Splits one `grep -r -n` line into `path`, `line` and `text`.
///
/// The split is on the first colon whose following segment is entirely
/// digits, so a path that itself contains a colon does not move it. A line
/// grep printed without a filename — which BSD `grep` does for a single file
/// argument — takes the requested path.
fn parse_match(line: &str, fallback: &str) -> GrepMatch {
    let mut start = 0usize;
    while let Some(offset) = line[start..].find(':') {
        let colon = start + offset;
        let rest = &line[colon + 1..];
        if let Some(next) = rest.find(':') {
            let digits = &rest[..next];
            if !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit()) {
                return GrepMatch {
                    path: line[..colon].to_string(),
                    line: digits.parse().ok(),
                    text: rest[next + 1..].to_string(),
                };
            }
        }
        start = colon + 1;
        if start >= line.len() {
            break;
        }
    }
    // `<line>:<text>`, which BSD `grep` prints when its one argument was a
    // file rather than a directory: the path is the one that was asked for.
    if let Some(colon) = line.find(':') {
        let digits = &line[..colon];
        if !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit()) {
            return GrepMatch {
                path: fallback.to_string(),
                line: digits.parse().ok(),
                text: line[colon + 1..].to_string(),
            };
        }
    }
    // Nothing the colon heuristic can place: `Binary file … matches`, a
    // permission notice, anything `grep` says that is not a hit. It is kept
    // rather than dropped — silently losing a line grep printed is worse —
    // and it is marked as unlocated so a program can tell the two apart.
    GrepMatch {
        path: fallback.to_string(),
        line: None,
        text: line.to_string(),
    }
}

fn build_glob<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    result: &ToolResult,
) -> (v8::Local<'s, v8::Value>, Value) {
    let paths: Vec<&str> = result
        .stdout
        .lines()
        .filter(|line| !line.is_empty())
        .collect();
    let array = v8::Array::new(scope, paths.len() as i32);
    for (index, path) in paths.iter().enumerate() {
        let value = js_string(scope, path);
        array.set_index(scope, index as u32, value);
    }
    let head: Vec<Value> = paths.iter().take(3).map(Value::string).collect();
    let last = if paths.len() > 3 {
        paths.last().map(Value::string)
    } else {
        None
    };
    (
        array.into(),
        Value::Array(ArrayValue::sampled(paths.len(), head, last)),
    )
}

fn build_bash<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    result: &ToolResult,
) -> (v8::Local<'s, v8::Value>, Value) {
    let object = v8::Object::new(scope);
    let stdout = js_string(scope, &result.stdout);
    set_key(scope, object, "stdout", stdout);
    let stderr = js_string(scope, &result.stderr);
    set_key(scope, object, "stderr", stderr);
    match result.exit_code {
        Some(code) => {
            let code = v8::Integer::new(scope, code);
            set_key(scope, object, "exit_code", code.into());
        }
        None => {
            let null = v8::null(scope);
            set_key(scope, object, "exit_code", null.into());
        }
    }
    let preview = Value::object(vec![
        ("stdout".to_string(), Value::string(&result.stdout)),
        ("stderr".to_string(), Value::string(&result.stderr)),
        (
            "exit_code".to_string(),
            result
                .exit_code
                .map_or(Value::Null, |code| Value::Number(f64::from(code))),
        ),
    ]);
    (object.into(), preview)
}

// --- the handle functions ----------------------------------------------

fn keep_callback(
    scope: &mut v8::PinScope,
    args: v8::FunctionCallbackArguments,
    _retval: v8::ReturnValue,
) {
    let name = args.get(0).to_rust_string_lossy(scope);
    if !is_identifier(&name) {
        throw_tool_error(
            scope,
            &format!("keep(\"{name}\") is not a name a program could have bound"),
        );
        return;
    }
    capture(scope, &name, args.get(1), false);
}

fn free_callback(
    scope: &mut v8::PinScope,
    args: v8::FunctionCallbackArguments,
    _retval: v8::ReturnValue,
) {
    let name = args.get(0).to_rust_string_lossy(scope);
    if refuse_host_name(scope, &name, "freed") {
        return;
    }
    let state = state(scope);
    state.table.borrow_mut().free(&name);
    state.note_free(&name);
    let context = scope.get_current_context();
    let global = context.global(scope);
    if let Some(key) = v8::String::new(scope, &name) {
        global.delete(scope, key.into());
    }
}

/// Every name the model can address right now — `runtime-contract.md` §3's
/// drop note promises this is "the full list", and the list a program is
/// shown when it asks mid-cell has to include what that cell has just bound.
///
/// The table alone is one cell stale: captures are drained into it when the
/// cell ends. So the current cell's captures are appended, newest last by
/// construction, and a name `free` released this cell is in neither.
fn handles_callback(
    scope: &mut v8::PinScope,
    _args: v8::FunctionCallbackArguments,
    mut retval: v8::ReturnValue,
) {
    let state = state(scope);
    let mut names: Vec<String> = state
        .table
        .borrow()
        .names()
        .into_iter()
        .map(str::to_string)
        .collect();
    for capture in &state.current.borrow().captures {
        if !names.contains(&capture.name) {
            names.push(capture.name.clone());
        }
    }
    let array = v8::Array::new(scope, names.len() as i32);
    for (index, name) in names.iter().enumerate() {
        let value = js_string(scope, name);
        array.set_index(scope, index as u32, value);
    }
    retval.set(array.into());
}

/// `s(name, value)` from a declaration line, and `s(name, value, true)` from
/// the generated epilogue.
///
/// The third argument is what tells the two apart, and it is load-bearing for
/// `free`: a name the model freed and then bound again in the same cell must
/// come back (that is a binding the program still names), while the
/// epilogue's blind re-read of every `late` name must **not** resurrect one
/// the model freed after declaring it. Only [`cell::compile`] writes either
/// call, and a program that reaches this callback by hand reaches it through
/// `keep`, which passes no third argument.
fn capture_callback(
    scope: &mut v8::PinScope,
    args: v8::FunctionCallbackArguments,
    _retval: v8::ReturnValue,
) {
    let name = args.get(0).to_rust_string_lossy(scope);
    let late = args.get(2).is_true();
    capture(scope, &name, args.get(1), late);
}

/// The most characters of a `yieldNow` reason the result block carries —
/// `runtime-contract.md` §3's preview cap, by the crate's own four-characters-
/// a-token estimate.
const REASON_CHARS: usize = PREVIEW_TOKEN_CAP * 4;

/// `yieldNow(reason?)` — `runtime-contract.md` §9.3: the cell ends in the
/// yield slot at once, from wherever it was called.
///
/// **It runs no code and touches no state but the cell's own flag.** The
/// mechanism is `terminate_execution`, the one way V8 offers to stop a
/// running program from inside a callback that no `try`/`catch` in the
/// program can intercept; the isolate reads the flag the instant the cell
/// stops and answers with a yield rather than the `RuntimeTerminated` a
/// termination nobody asked for would be. The heap ceiling and the wall
/// clock are read first and win, so a cell that hit either while also asking
/// to yield is told the truth about why it stopped.
fn yield_now_callback(
    scope: &mut v8::PinScope,
    args: v8::FunctionCallbackArguments,
    _retval: v8::ReturnValue,
) {
    let given = args.get(0);
    let reason = if given.is_undefined() || given.is_null() {
        None
    } else {
        Some(bound_reason(&given.to_rust_string_lossy(scope)))
    };
    trace(scope).request_yield(reason);
    scope.terminate_execution();
    // `terminate_execution` requests an interrupt, and V8 services one at a
    // stack check -- a function entry or a loop back-edge -- never on the
    // return from an API callback. Measured: straight-line statements after
    // `yieldNow()` ran to the end of the cell, which then fell off normally.
    // Entering this loop is the stack check: the pending termination is
    // serviced before its first iteration completes and unwinds through here
    // and out of the program, so nothing after the call ever runs. It cannot
    // spin -- the termination is already requested -- and were it somehow
    // not honoured, the watchdog would end the cell as a timeout rather than
    // let it hang.
    if let Some(source) = v8::String::new(scope, "for (;;) {}")
        && let Some(script) = v8::Script::compile(scope, source, None)
    {
        script.run(scope);
    }
}

/// One line, at most [`REASON_CHARS`] characters, cut on a character
/// boundary and saying so — the reason is a line of the result block and
/// must neither forge a second line nor cost the turn more than a preview.
fn bound_reason(reason: &str) -> String {
    let one_line = reason.lines().collect::<Vec<_>>().join(" ");
    let total = one_line.chars().count();
    if total <= REASON_CHARS {
        return one_line;
    }
    let head: String = one_line.chars().take(REASON_CHARS).collect();
    format!(
        "{head}…(reason cut at {} of {} characters)",
        thousands(REASON_CHARS as u64),
        thousands(total as u64)
    )
}

/// The generated epilogue's `return __pane_cell.e()`: it answers with a
/// fresh object carrying [`end_tag`] for the cell that is running, and the
/// isolate reads the cell's ending off the value its promise fulfils with.
///
/// Nothing is recorded on the host side, which is the point — the flag this
/// replaced could be set by one line of the model's own program, and the
/// cell then yielded where §1 says it returns.
fn fell_callback(
    scope: &mut v8::PinScope,
    _args: v8::FunctionCallbackArguments,
    mut retval: v8::ReturnValue,
) {
    let cell = state(scope).cell.get();
    let marker = v8::Object::new(scope);
    let tag = end_tag(scope);
    let minted = v8::Number::new(scope, cell as f64);
    marker.set_private(scope, tag, minted.into());
    retval.set(marker.into());
}

/// The one path by which a value becomes a handle: it is put on the
/// persistent scope so the next cell can name it, and its preview is
/// recorded so this turn can show it.
///
/// **Every write to the persistent scope goes through here** — `keep`, the
/// compiled declaration-line captures and the epilogue's re-reads alike —
/// which is why the host-function guard lives here rather than only in
/// `cell::compile`. The compile-time refusal is the good early message for a
/// binding a program declared; this is the one that holds for a name it
/// assembled at run time.
///
/// **The write is checked, and a refused write is a throw.** Until it was,
/// a frozen `globalThis` put the name in the handle table, rendered it to the
/// model this turn, and left it `undefined` the next — the one failure
/// `runtime-contract.md` §2 says would make the whole channel untrustworthy,
/// reached by one line of defensive tidiness in the model's own program.
///
/// `late` marks the generated epilogue's re-read of a name, which is the one
/// caller that must not un-free anything: see [`capture_callback`].
fn capture(scope: &mut v8::PinScope, name: &str, value: v8::Local<v8::Value>, late: bool) {
    if refuse_host_name(scope, name, "bound") {
        return;
    }
    let state = state(scope);
    let context = scope.get_current_context();
    let global = context.global(scope);
    let Some(key) = v8::String::new(scope, name).map(v8::Local::<v8::Value>::from) else {
        throw_unbindable(scope, name);
        return;
    };
    global.set(scope, key, value);
    // Checked by reading the binding back, not by `set`'s own answer.
    // `v8::Object::set` performs a **sloppy-mode** store: on a frozen
    // `globalThis` it silently does nothing and still answers `Some(true)`
    // (measured — afterwards `Object.isExtensible(globalThis)` is false, the
    // name is not an own property, and `typeof globalThis.x` is
    // `"undefined"`). §2's question is only ever "will this name be there
    // next cell", and reading it back is that question exactly — a
    // non-writable property that kept its old value and a setter that stored
    // somewhere else both answer it correctly, and neither is visible in a
    // boolean.
    let bound = global
        .get(scope, key)
        .is_some_and(|read| read.same_value(value));
    if !bound {
        throw_unbindable(scope, name);
        return;
    }
    if !late {
        // §2 gives a handle three ways to die and `free` is one of them —
        // but a name the same cell binds *again* after freeing it is a
        // binding the program still names, and `Runtime::forget_freed` would
        // otherwise delete it off the persistent scope after the cell. This
        // is the cheapest recovery `RuntimeOutOfMemory`'s own message invites
        // ("call free(\"name\") on what you no longer need"), so it has to
        // leave the model holding the summary it kept.
        state
            .current
            .borrow_mut()
            .freed
            .retain(|freed| freed != name);
    }
    let (preview, meta) = preview_of(scope, &state, value);
    state.capture(name, preview, meta);
}

/// A tool-produced object keeps the preview and provenance its call already
/// built; anything else is marshalled now.
pub(crate) fn preview_of(
    scope: &mut v8::PinScope,
    state: &Rc<RuntimeState>,
    value: v8::Local<v8::Value>,
) -> (Value, HandleMeta) {
    if let Some(call) = recorded_call(scope, state, value) {
        return (call.preview, call.meta);
    }
    let preview = marshal::marshal(scope, value);
    let meta = HandleMeta {
        type_label: None,
        size_estimate: marshal::size_estimate(scope, value),
        provenance: None,
    };
    (preview, meta)
}

/// The call a tool-produced object was tagged with, when `value` is one;
/// `None` for anything the program built itself. This is the whole of
/// "handle-aware": a reader that asks here before it reads a property can
/// answer with the call's preview and never touch the payload (§4).
pub(crate) fn recorded_call(
    scope: &mut v8::PinScope,
    state: &Rc<RuntimeState>,
    value: v8::Local<v8::Value>,
) -> Option<RecordedCall> {
    let object = v8::Local::<v8::Object>::try_from(value).ok()?;
    let tag = call_tag(scope);
    let id = object.get_private(scope, tag)?.number_value(scope)?;
    if !id.is_finite() || id < 1.0 {
        return None;
    }
    state.recorded(id as u64)
}

fn is_identifier(name: &str) -> bool {
    let mut chars = name.chars();
    chars
        .next()
        .is_some_and(|first| first.is_alphabetic() || first == '_' || first == '$')
        && chars.all(|c| c.is_alphanumeric() || c == '_' || c == '$')
}

// --- the batch, and the background jobs that fill it --------------------

fn batch_store(scope: &v8::PinScope) -> Option<Rc<BatchStore>> {
    scope.get_slot::<Rc<BatchStore>>().cloned()
}

/// One event of the live batch, copied out of the `RefCell` before anything
/// re-enters V8.
///
/// Owned rather than borrowed on purpose: building a JS object calls back
/// into the isolate, and a `RefCell` borrow of the batch held across that
/// call would be a borrow live while a callback that also reads the batch
/// could run.
struct EventRow {
    id: EventId,
    kind: String,
    source: String,
    at: String,
    summary: String,
    age: u32,
    payload: String,
}

fn row(event: &Event, age: u32) -> EventRow {
    EventRow {
        id: event.id,
        kind: event.kind.as_str(),
        source: event.source.clone(),
        at: event.at.to_string(),
        summary: event.summary.clone(),
        age,
        payload: event.payload.as_str().to_string(),
    }
}

/// Every event of the live batch that `pick` selects, with its age.
fn selected(scope: &v8::PinScope, pick: impl FnOnce(&Batch) -> Vec<&Event>) -> Vec<EventRow> {
    let Some(store) = batch_store(scope) else {
        return Vec::new();
    };
    store
        .with(|batch| {
            let ages: std::collections::HashMap<EventId, u32> = batch
                .events()
                .into_iter()
                .map(|(event, age)| (event.id, age))
                .collect();
            pick(&*batch)
                .into_iter()
                .map(|event| row(event, ages.get(&event.id).copied().unwrap_or(0)))
                .collect()
        })
        .unwrap_or_default()
}

fn events_array<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    rows: &[EventRow],
) -> v8::Local<'s, v8::Value> {
    let array = v8::Array::new(scope, rows.len() as i32);
    for (index, row) in rows.iter().enumerate() {
        let object = v8::Object::new(scope);
        let id = v8::Number::new(scope, row.id as f64);
        set_fixed_key(scope, object, "id", id.into());
        for (key, text) in [
            ("kind", &row.kind),
            ("source", &row.source),
            ("at", &row.at),
            ("summary", &row.summary),
        ] {
            let value = js_string(scope, text);
            set_fixed_key(scope, object, key, value);
        }
        let age = v8::Number::new(scope, f64::from(row.age));
        set_fixed_key(scope, object, "age", age.into());
        // §1: a payload is materialised on first access, and §3 never
        // previews one. A method rather than a property is what makes that
        // true of this object too -- reading the event costs nothing until
        // the program asks.
        let data = js_string(scope, &row.payload);
        if let Some(function) = v8::Function::builder(payload_callback)
            .data(data)
            .build(scope)
        {
            set_fixed_key(scope, object, "payload", function.into());
        }
        array.set_index(scope, index as u32, object.into());
    }
    array.into()
}

/// `batch.where({kind, source})` -- both optional, `hook.*` matched by
/// prefix, exactly as `Batch::where_` decides.
fn batch_where_callback(
    scope: &mut v8::PinScope,
    args: v8::FunctionCallbackArguments,
    mut retval: v8::ReturnValue,
) {
    let (kind, source) = read_filter(scope, args.get(0));
    let rows = selected(scope, |batch| {
        batch.where_(kind.as_deref(), source.as_deref())
    });
    let array = events_array(scope, &rows);
    retval.set(array);
}

/// `batch.rest()` -- this batch's events not yet acked.
fn batch_rest_callback(
    scope: &mut v8::PinScope,
    _args: v8::FunctionCallbackArguments,
    mut retval: v8::ReturnValue,
) {
    let rows = selected(scope, Batch::rest);
    let array = events_array(scope, &rows);
    retval.set(array);
}

/// `batch.ack(ids)` -- an id this batch does not hold comes back in
/// `unknown` rather than being silently dropped, which is `Batch::ack`'s own
/// decision and not a second one here.
fn batch_ack_callback(
    scope: &mut v8::PinScope,
    args: v8::FunctionCallbackArguments,
    mut retval: v8::ReturnValue,
) {
    let ids = read_ids(scope, args.get(0));
    let acked = batch_store(scope)
        .and_then(|store| store.with(|batch| batch.ack(&ids)))
        .unwrap_or_else(|| crate::events::batch::Acked {
            acked: Vec::new(),
            unknown: ids,
        });
    let object = v8::Object::new(scope);
    for (key, list) in [("acked", &acked.acked), ("unknown", &acked.unknown)] {
        let array = v8::Array::new(scope, list.len() as i32);
        for (index, id) in list.iter().enumerate() {
            let value = v8::Number::new(scope, *id as f64);
            array.set_index(scope, index as u32, value.into());
        }
        set_fixed_key(scope, object, key, array.into());
    }
    retval.set(object.into());
}

/// An event's payload, materialised now: the status verbatim, and `stdout`
/// and `stderr` as methods, so §5's "a job that printed 40 MB costs a status
/// line" is true of the object and not only of the preview.
fn payload_callback(
    scope: &mut v8::PinScope,
    args: v8::FunctionCallbackArguments,
    mut retval: v8::ReturnValue,
) {
    let id = args.data().to_rust_string_lossy(scope);
    let session = state(scope).session.clone();
    let Some(result) = bg::payload(&session, &id) else {
        let null = v8::null(scope);
        retval.set(null.into());
        return;
    };
    let object = v8::Object::new(scope);
    let status = js_string(scope, &result.status);
    set_fixed_key(scope, object, "status", status);
    // Built one at a time rather than over an array of callbacks: rusty_v8
    // takes a zero-sized *fn item*, and an array coerces both arms to a fn
    // pointer, which fails a `const` size check inside the binding rather
    // than at this line.
    let data = js_string(scope, &id);
    if let Some(function) = v8::Function::builder(stdout_callback)
        .data(data)
        .build(scope)
    {
        set_fixed_key(scope, object, "stdout", function.into());
    }
    let data = js_string(scope, &id);
    if let Some(function) = v8::Function::builder(stderr_callback)
        .data(data)
        .build(scope)
    {
        set_fixed_key(scope, object, "stderr", function.into());
    }
    retval.set(object.into());
}

fn stdout_callback(
    scope: &mut v8::PinScope,
    args: v8::FunctionCallbackArguments,
    mut retval: v8::ReturnValue,
) {
    let text = stream(scope, &args, |result| result.stdout);
    let value = js_string(scope, &text);
    retval.set(value);
}

fn stderr_callback(
    scope: &mut v8::PinScope,
    args: v8::FunctionCallbackArguments,
    mut retval: v8::ReturnValue,
) {
    let text = stream(scope, &args, |result| result.stderr);
    let value = js_string(scope, &text);
    retval.set(value);
}

fn stream(
    scope: &mut v8::PinScope,
    args: &v8::FunctionCallbackArguments,
    pick: impl FnOnce(bg::JobResult) -> String,
) -> String {
    let id = args.data().to_rust_string_lossy(scope);
    let session = state(scope).session.clone();
    bg::payload(&session, &id).map(pick).unwrap_or_default()
}

// --- agent.run ---------------------------------------------------------

/// Phase 64's `agent.run(task, {turns, model})`.
///
/// **Both refusals happen before a handle exists**, which is `bg.run`'s own
/// rule and matters for the same reason: a program that catches the exception
/// is holding nothing, and there is no started subagent to stop.
///
/// The first refusal is depth — a subagent may not start a subagent, and the
/// runtime knows which it is. The second is budget: a subagent charges the
/// parent's task budget, so one the parent cannot pay for is refused rather
/// than started and killed halfway, which would spend the tokens and produce
/// nothing.
fn agent_run_callback(
    scope: &mut v8::PinScope,
    args: v8::FunctionCallbackArguments,
    mut retval: v8::ReturnValue,
) {
    let task = args.get(0).to_rust_string_lossy(scope);
    if task.trim().is_empty() {
        throw_tool_error(scope, "agent.run needs a task to work on");
        return;
    }
    let turns = read_millis(scope, args.get(1), "turns").unwrap_or(crate::agent::DEFAULT_TURNS);
    let asked_model = read_option(scope, args.get(1), "model");

    let state = state(scope);
    if state.subagent.get() {
        throw_denied(
            scope,
            &PermissionDenied {
                tool: "agent".to_string(),
                path: String::new(),
                rule: "a subagent may not start a subagent; answer the question you were given"
                    .to_string(),
            },
        );
        return;
    }

    // A turn's worth of budget is the floor, not the whole cost: what a
    // subagent actually spends is charged as its `agent.done` arrives. This
    // refuses the case the parent plainly cannot afford rather than
    // predicting one it might.
    let remaining = state.budget_remaining.get();
    if remaining > 0 && remaining < MINIMUM_AGENT_BUDGET {
        throw_denied(
            scope,
            &PermissionDenied {
                tool: "agent".to_string(),
                path: String::new(),
                rule: format!(
                    "the task has {remaining} token(s) left and a subagent needs at least \
                     {MINIMUM_AGENT_BUDGET}; finish or return"
                ),
            },
        );
        return;
    }

    let options = crate::agent::AgentOptions {
        turns: turns.clamp(1, crate::agent::MAX_TURNS),
        model: asked_model.unwrap_or_else(|| state.model.borrow().clone()),
        effort: crate::wire::Effort::default(),
    };
    let handle = crate::bg::agent(
        &state.profile,
        &state.glasshouse,
        &state.session,
        &task,
        &options,
    );
    trace(scope).record(CallRecord {
        tool: "agent.run".into(),
        args: [("source".into(), format!("agent/{handle}"))]
            .into_iter()
            .collect(),
        ended: Ended::Ok,
    });
    let object = agent_object(scope, &handle);
    retval.set(object);
}

/// What a task must have left before a subagent may start. One ordinary
/// turn's ceiling, which is the smallest amount that could produce an answer
/// rather than a truncation.
const MINIMUM_AGENT_BUDGET: u64 = crate::wire::MAX_TOKENS as u64;

fn agent_object<'s>(scope: &mut v8::PinScope<'s, '_>, handle: &str) -> v8::Local<'s, v8::Value> {
    let object = v8::Object::new(scope);
    let id = js_string(scope, handle);
    set_fixed_key(scope, object, "id", id);
    let source = js_string(scope, &format!("agent/{handle}"));
    set_fixed_key(scope, object, "source", source);
    object.into()
}

// --- todo.write, todo.read ---------------------------------------------

/// `todo.write(items)` — the model's own plan, replaced whole.
///
/// The invariant: **what this stores is renderable.** Every item has text and
/// one of three statuses, checked here, because the plan is shown to the
/// person and counted in the turn line; an item that is neither would be a
/// row nothing can draw. A malformed write throws and changes nothing, so a
/// program that catches it still holds the plan it had.
fn todo_write_callback(
    scope: &mut v8::PinScope,
    args: v8::FunctionCallbackArguments,
    _retval: v8::ReturnValue,
) {
    let Ok(array) = v8::Local::<v8::Array>::try_from(args.get(0)) else {
        throw_tool_error(scope, "todo.write takes an array of {text, status}");
        return;
    };
    let mut items = Vec::with_capacity(array.length() as usize);
    for index in 0..array.length() {
        let Some(entry) = array.get_index(scope, index) else {
            continue;
        };
        let Ok(object) = v8::Local::<v8::Object>::try_from(entry) else {
            throw_tool_error(
                scope,
                &format!("todo.write item {index} is not an object with text and status"),
            );
            return;
        };
        let text = read_object_key(scope, object, "text").unwrap_or_default();
        if text.trim().is_empty() {
            throw_tool_error(scope, &format!("todo.write item {index} has no text"));
            return;
        }
        let status_text = read_object_key(scope, object, "status")
            .unwrap_or_else(|| PlanStatus::Pending.as_str().to_string());
        let Some(status) = PlanStatus::parse(&status_text) else {
            throw_tool_error(
                scope,
                &format!(
                    "todo.write item {index}: status {status_text:?} is not one of pending, \
                     active, done"
                ),
            );
            return;
        };
        items.push(PlanItem { text, status });
    }
    state(scope).set_plan(items);
}

/// `todo.read()` — the plan as it stands, as plain objects.
fn todo_read_callback(
    scope: &mut v8::PinScope,
    _args: v8::FunctionCallbackArguments,
    mut retval: v8::ReturnValue,
) {
    let plan = state(scope).plan();
    let array = v8::Array::new(scope, plan.len() as i32);
    for (index, item) in plan.iter().enumerate() {
        let object = v8::Object::new(scope);
        let text = js_string(scope, &item.text);
        set_key(scope, object, "text", text);
        let status = js_string(scope, item.status.as_str());
        set_key(scope, object, "status", status);
        array.set_index(scope, index as u32, object.into());
    }
    retval.set(array.into());
}

/// One string property of an object, or `None` when it is absent or is not a
/// string. Distinct from [`read_option`], which reads a property off an
/// options argument that may itself be absent.
fn read_object_key(
    scope: &mut v8::PinScope,
    object: v8::Local<v8::Object>,
    key: &str,
) -> Option<String> {
    let key = v8::String::new(scope, key)?;
    let value = object.get(scope, key.into())?;
    if value.is_null_or_undefined() {
        return None;
    }
    Some(value.to_rust_string_lossy(scope))
}

// --- bg.run, bg.watch, bg.cancel ---------------------------------------

/// §5's `bg.run`. The refusal is `Profile::admits_command`'s own and it
/// happens inside [`bg::run`] **before** a handle exists, so a program that
/// catches this exception is holding nothing.
fn bg_run_callback(
    scope: &mut v8::PinScope,
    args: v8::FunctionCallbackArguments,
    mut retval: v8::ReturnValue,
) {
    let command = args.get(0).to_rust_string_lossy(scope);
    let options = RunOptions {
        cwd: read_option(scope, args.get(1), "cwd"),
        env: read_option(scope, args.get(1), "env"),
        timeout_ms: read_millis(scope, args.get(1), "timeout"),
    };
    let state = state(scope);
    match bg::run(
        &state.profile,
        &state.glasshouse,
        &state.session,
        &command,
        &options,
    ) {
        Ok(handle) => {
            let object = job_object(scope, &handle);
            retval.set(object);
        }
        Err(denied) => throw_denied(scope, &denied),
    }
}

/// §5's `bg.watch`. `every` defaults to a second, which is the smallest
/// cadence a shell command can be run at without the polling itself being
/// the load.
///
/// **The default is not the bound.** A program that names its own `every` is
/// answered by `bg::watch`'s floor, which refuses a cadence under it rather
/// than clamping silently — the enforcement is there and not here so that no
/// caller of the module can get under it.
fn bg_watch_callback(
    scope: &mut v8::PinScope,
    args: v8::FunctionCallbackArguments,
    mut retval: v8::ReturnValue,
) {
    let command = args.get(0).to_rust_string_lossy(scope);
    let options = WatchOptions {
        every_ms: read_millis(scope, args.get(1), "every").unwrap_or(DEFAULT_WATCH_EVERY_MS),
        until: read_option(scope, args.get(1), "until"),
        timeout_ms: read_millis(scope, args.get(1), "timeout"),
    };
    let state = state(scope);
    match bg::watch(
        &state.profile,
        &state.glasshouse,
        &state.session,
        &command,
        &options,
    ) {
        Ok(handle) => {
            let object = job_object(scope, &handle);
            retval.set(object);
        }
        Err(denied) => throw_denied(scope, &denied),
    }
}

/// How often `bg.watch` runs its command when the model named no cadence.
const DEFAULT_WATCH_EVERY_MS: u64 = 1_000;

/// §5's `bg.cancel(handle)`: idempotent, and it takes either the object
/// `bg.run` answered with or the bare id off it, because a model that kept
/// only `job.id` should not have to reconstruct the object.
fn bg_cancel_callback(
    scope: &mut v8::PinScope,
    args: v8::FunctionCallbackArguments,
    _retval: v8::ReturnValue,
) {
    let given = args.get(0);
    let id = match v8::Local::<v8::Object>::try_from(given) {
        Ok(object) => v8::String::new(scope, "id")
            .and_then(|key| object.get(scope, key.into()))
            .map(|value| value.to_rust_string_lossy(scope))
            .unwrap_or_default(),
        Err(_) => given.to_rust_string_lossy(scope),
    };
    if id.is_empty() {
        return;
    }
    let session = state(scope).session.clone();
    bg::cancel(&session, &id);
}

fn job_object<'s>(scope: &mut v8::PinScope<'s, '_>, handle: &str) -> v8::Local<'s, v8::Value> {
    let object = v8::Object::new(scope);
    let id = js_string(scope, handle);
    set_fixed_key(scope, object, "id", id);
    let source = js_string(scope, &format!("bg/{handle}"));
    set_fixed_key(scope, object, "source", source);
    object.into()
}

/// One string property of an options object, or `None` when the object, the
/// property or its value is absent.
fn read_option(scope: &mut v8::PinScope, value: v8::Local<v8::Value>, key: &str) -> Option<String> {
    let object = v8::Local::<v8::Object>::try_from(value).ok()?;
    let key = v8::String::new(scope, key)?;
    let given = object.get(scope, key.into())?;
    if given.is_undefined() || given.is_null() {
        return None;
    }
    Some(given.to_rust_string_lossy(scope))
}

/// One millisecond count off an options object. A value that is not a finite
/// non-negative number is `None` rather than zero: a zero deadline would
/// cancel the job it was meant to bound.
fn read_millis(scope: &mut v8::PinScope, value: v8::Local<v8::Value>, key: &str) -> Option<u64> {
    let object = v8::Local::<v8::Object>::try_from(value).ok()?;
    let key = v8::String::new(scope, key)?;
    let given = object.get(scope, key.into())?;
    let number = given.number_value(scope)?;
    (number.is_finite() && number >= 1.0).then_some(number as u64)
}

/// `{kind, source}`, both optional.
fn read_filter(
    scope: &mut v8::PinScope,
    value: v8::Local<v8::Value>,
) -> (Option<String>, Option<String>) {
    (
        read_option(scope, value, "kind"),
        read_option(scope, value, "source"),
    )
}

/// An array of event ids. A value that is not a finite id is skipped here
/// rather than becoming `0`, which is not an id any window ever assigns.
fn read_ids(scope: &mut v8::PinScope, value: v8::Local<v8::Value>) -> Vec<EventId> {
    let Ok(array) = v8::Local::<v8::Array>::try_from(value) else {
        return Vec::new();
    };
    let mut ids = Vec::with_capacity(array.length() as usize);
    for index in 0..array.length() {
        if let Some(item) = array.get_index(scope, index)
            && let Some(number) = item.number_value(scope)
            && number.is_finite()
            && number >= 1.0
        {
            ids.push(number as EventId);
        }
    }
    ids
}

/// §4's delivery, in the isolate: the `batch` name, and the three methods
/// §3 gives it.
///
/// Bound with an ordinary write rather than [`set_fixed_key`], because §2's
/// replacement rule applies to this handle exactly as it does to one the
/// model bound itself -- each delivery replaces the last.
pub(crate) fn install_batch(scope: &mut v8::PinScope) {
    let context = scope.get_current_context();
    let global = context.global(scope);
    let object = v8::Object::new(scope);
    let n = batch_store(scope)
        .and_then(|store| store.with(|batch| batch.n))
        .unwrap_or(0);
    let count = v8::Number::new(scope, n as f64);
    set_fixed_key(scope, object, "n", count.into());
    if let Some(function) = v8::Function::builder(batch_where_callback).build(scope) {
        // `where` is a reserved word in JavaScript nowhere -- it is not a
        // keyword at all -- and a property name could carry it even if it
        // were, so the model writes `batch.where(...)` and the Rust name
        // stays `where_`.
        set_fixed_key(scope, object, "where", function.into());
    }
    if let Some(function) = v8::Function::builder(batch_ack_callback).build(scope) {
        set_fixed_key(scope, object, "ack", function.into());
    }
    if let Some(function) = v8::Function::builder(batch_rest_callback).build(scope) {
        set_fixed_key(scope, object, "rest", function.into());
    }
    set_key(scope, global, "batch", object.into());
}

// --- console -----------------------------------------------------------

const CONSOLE_ARGUMENT_CHARS: usize = 4096;
const CONSOLE_DEPTH: usize = 3;
const CONSOLE_KEYS: usize = 12;

fn console_string(scope: &mut v8::PinScope, text: v8::Local<v8::String>) -> String {
    let mut buffer = [0u8; CONSOLE_ARGUMENT_CHARS];
    let mut consumed = 0;
    let written = text.write_utf8_v2(
        scope,
        &mut buffer,
        v8::WriteFlags::kReplaceInvalidUtf8,
        Some(&mut consumed),
    );
    let mut result = String::from_utf8_lossy(&buffer[..written]).into_owned();
    if consumed < text.length() {
        result.push('…');
    }
    result
}

/// A bounded inspection of one console argument. Objects are read through
/// own property descriptors, so an accessor is named rather than invoked;
/// proxies are not inspected because even asking for their keys runs a trap.
fn inspect_console(
    scope: &mut v8::PinScope,
    value: v8::Local<v8::Value>,
    depth: usize,
    seen: &mut Vec<i32>,
) -> String {
    if value.is_string() {
        let text = console_string(
            scope,
            v8::Local::<v8::String>::try_from(value).expect("string checked"),
        );
        return if depth == 0 {
            text
        } else {
            serde_json::to_string(&text).unwrap_or_else(|_| "\"<unprintable>\"".into())
        };
    }
    if value.is_null() {
        return "null".into();
    }
    if value.is_undefined() {
        return "undefined".into();
    }
    if value.is_function() {
        return "[Function]".into();
    }
    if value.is_proxy() {
        return "[Proxy]".into();
    }
    if !value.is_object() {
        return value
            .to_string(scope)
            .map(|text| text.to_rust_string_lossy(scope))
            .unwrap_or_else(|| "<unprintable>".into());
    }
    let object: v8::Local<v8::Object> = match value.try_into() {
        Ok(object) => object,
        Err(_) => return "<unprintable>".into(),
    };
    let identity = object.get_identity_hash().get();
    if seen.contains(&identity) {
        return "[Circular]".into();
    }
    let is_array = value.is_array();
    if depth >= CONSOLE_DEPTH {
        return if is_array { "[…]" } else { "{…}" }.into();
    }
    seen.push(identity);
    let rendered = inspect_console_object(scope, object, is_array, depth, seen);
    seen.pop();
    rendered
}

fn inspect_console_object(
    scope: &mut v8::PinScope,
    object: v8::Local<v8::Object>,
    is_array: bool,
    depth: usize,
    seen: &mut Vec<i32>,
) -> String {
    if is_array {
        let Ok(array) = v8::Local::<v8::Array>::try_from(object) else {
            return "<unprintable>".into();
        };
        let total = array.length() as usize;
        let mut parts = Vec::new();
        for index in 0..total.min(CONSOLE_KEYS) {
            let Some(key) = v8::String::new(scope, &index.to_string()) else {
                continue;
            };
            let shown = array
                .get_own_property_descriptor(scope, key.into())
                .and_then(|descriptor| v8::Local::<v8::Object>::try_from(descriptor).ok())
                .and_then(|descriptor| {
                    v8::String::new(scope, "value")
                        .and_then(|name| descriptor.get(scope, name.into()))
                })
                .map(|value| inspect_console(scope, value, depth + 1, seen))
                .unwrap_or_else(|| "<empty>".into());
            parts.push(shown);
        }
        if total > CONSOLE_KEYS {
            parts.push(format!("… {} more", total - CONSOLE_KEYS));
        }
        return format!("[{}]", parts.join(", "));
    }
    let Some(names) = object.get_own_property_names(scope, v8::GetPropertyNamesArgs::default())
    else {
        return "<unprintable>".into();
    };
    let total = names.length() as usize;
    let mut parts = Vec::new();
    for index in 0..total.min(CONSOLE_KEYS) {
        let Some(key_value) = names.get_index(scope, index as u32) else {
            continue;
        };
        let Ok(key) = v8::Local::<v8::Name>::try_from(key_value) else {
            continue;
        };
        let key_text = key_value
            .to_string(scope)
            .map(|key| console_string(scope, key))
            .unwrap_or_default();
        let Some(descriptor_value) = object.get_own_property_descriptor(scope, key) else {
            continue;
        };
        let Ok(descriptor) = v8::Local::<v8::Object>::try_from(descriptor_value) else {
            continue;
        };
        let getter = v8::String::new(scope, "get")
            .and_then(|name| descriptor.get(scope, name.into()))
            .is_some_and(|value| !value.is_undefined());
        let shown = if getter {
            "[Getter]".into()
        } else {
            v8::String::new(scope, "value")
                .and_then(|name| descriptor.get(scope, name.into()))
                .map(|value| inspect_console(scope, value, depth + 1, seen))
                .unwrap_or_else(|| "undefined".into())
        };
        let key = serde_json::to_string(&key_text).unwrap_or_else(|_| "\"?\"".into());
        parts.push(format!("{key}: {shown}"));
    }
    if total > CONSOLE_KEYS {
        parts.push(format!("… {} more", total - CONSOLE_KEYS));
    }
    format!("{{{}}}", parts.join(", "))
}

fn console_callback(
    scope: &mut v8::PinScope,
    args: v8::FunctionCallbackArguments,
    _retval: v8::ReturnValue,
) {
    let mut parts: Vec<String> = Vec::new();
    for index in 0..args.length().min(CONSOLE_KEYS as i32) {
        let value = args.get(index);
        parts.push(
            inspect_console(scope, value, 0, &mut Vec::new())
                .chars()
                .take(CONSOLE_ARGUMENT_CHARS)
                .collect(),
        );
    }
    if args.length() > CONSOLE_KEYS as i32 {
        parts.push("… more arguments".into());
    }
    state(scope)
        .current
        .borrow_mut()
        .console
        .write_line(&parts.join(" "));
}

// --- refusals ----------------------------------------------------------

fn throw_denied(scope: &mut v8::PinScope, denied: &PermissionDenied) {
    let message = js_string(scope, &denied.to_string());
    let tool = js_string(scope, &denied.tool);
    let path = js_string(scope, &denied.path);
    let rule = js_string(scope, &denied.rule);
    match construct(scope, "PermissionDenied", &[message, tool, path, rule]) {
        Some(error) => {
            scope.throw_exception(error);
        }
        None => throw_plain(scope, &denied.to_string()),
    }
}

/// Refuses a write or a free that names one of the eight host functions or
/// the runtime's own `__pane_` prefix, and answers whether it did.
///
/// A `ToolError` throw rather than a silent no-op: a model that lost `grep`
/// silently would have no way to learn it, and `runtime-contract.md` §5 makes
/// a throw the shape a refusal already has.
fn refuse_host_name(scope: &mut v8::PinScope, name: &str, verb: &str) -> bool {
    if HOST_FUNCTIONS.contains(&name) {
        throw_tool_error(
            scope,
            &format!(
                "`{name}` is a host function and may not be {verb}; it would replace it on the \
                 persistent scope for the rest of the task and nothing could put it back"
            ),
        );
        return true;
    }
    if name.starts_with(RESERVED_PREFIX) {
        throw_tool_error(
            scope,
            &format!("`{name}` starts with `{RESERVED_PREFIX}`, which the runtime reserves"),
        );
        return true;
    }
    false
}

/// A write to the persistent scope the scope itself refused.
///
/// A `TypeError`, because that is what assigning to a frozen or non-writable
/// property throws in strict-mode JavaScript, and because the model's
/// question is about its own `globalThis`, not about a tool.
fn throw_unbindable(scope: &mut v8::PinScope, name: &str) {
    let message = format!(
        "`{name}` could not be bound on the persistent scope, so it would not be there next \
         cell; is `globalThis` frozen, or is `{name}` a non-writable property of it?"
    );
    let Some(text) = v8::String::new(scope, &message) else {
        return;
    };
    let error = v8::Exception::type_error(scope, text);
    scope.throw_exception(error);
}

fn throw_cancelled(scope: &mut v8::PinScope, tool: &str) {
    let message = js_string(
        scope,
        &ToolError::Cancelled {
            tool: tool.to_string(),
        }
        .to_string(),
    );
    let named = js_string(scope, tool);
    match construct(scope, "Cancelled", &[message, named]) {
        Some(error) => {
            scope.throw_exception(error);
        }
        None => throw_plain(scope, "the call was cancelled"),
    }
}

fn throw_tool_error(scope: &mut v8::PinScope, message: &str) {
    let text = js_string(scope, message);
    match construct(scope, "ToolError", &[text]) {
        Some(error) => {
            scope.throw_exception(error);
        }
        None => throw_plain(scope, message),
    }
}

/// The last resort when the bootstrap's own classes are not reachable: a
/// plain `Error`, so a refusal is never silently swallowed.
fn throw_plain(scope: &mut v8::PinScope, message: &str) {
    let Some(text) = v8::String::new(scope, message) else {
        return;
    };
    let error = v8::Exception::error(scope, text);
    scope.throw_exception(error);
}

fn construct<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    class: &str,
    args: &[v8::Local<'s, v8::Value>],
) -> Option<v8::Local<'s, v8::Value>> {
    let context = scope.get_current_context();
    let global = context.global(scope);
    let key = v8::String::new(scope, class)?;
    let found = global.get(scope, key.into())?;
    let constructor = v8::Local::<v8::Function>::try_from(found).ok()?;
    constructor.new_instance(scope, args).map(Into::into)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_grep_line_splits_on_the_first_colon_that_precedes_a_line_number() {
        let found = parse_match("crates/pane/src/lib.rs:12:pub mod runtime;", "root");
        assert_eq!(found.path, "crates/pane/src/lib.rs");
        assert_eq!(found.line, Some(12));
        assert_eq!(found.text, "pub mod runtime;");
    }

    #[test]
    fn a_path_with_a_colon_in_it_does_not_move_the_split() {
        let found = parse_match("C:/tmp/a.rs:7:let x = 1;", "root");
        assert_eq!(found.path, "C:/tmp/a.rs");
        assert_eq!(found.line, Some(7));
        assert_eq!(found.text, "let x = 1;");
    }

    #[test]
    fn a_line_without_a_filename_takes_the_requested_path() {
        let found = parse_match("9:hit", "/tmp/one.txt");
        assert_eq!(found.path, "/tmp/one.txt");
        assert_eq!(found.line, Some(9));
        assert_eq!(found.text, "hit");
    }

    /// `grep -r` prints this routinely, and it is not a match: giving it
    /// line 0 made `new Set(hits.map(m => m.path))` — §6's own worked cell —
    /// count the searched directory as a file.
    #[test]
    fn a_line_that_is_not_a_located_match_has_no_line_number() {
        let found = parse_match("Binary file /tmp/root/bin.dat matches", "/tmp/root");
        assert_eq!(found.line, None);
        assert_eq!(found.path, "/tmp/root");
        assert_eq!(found.text, "Binary file /tmp/root/bin.dat matches");
        // And it renders as what grep said, not as a located hit.
        assert_eq!(match_line(&found), "Binary file /tmp/root/bin.dat matches");
    }

    #[test]
    fn a_yield_reason_is_one_line_and_bounded_on_a_character_boundary() {
        assert_eq!(bound_reason("two\nlines"), "two lines");
        let long: String = "é".repeat(REASON_CHARS + 5);
        let bounded = bound_reason(&long);
        assert!(bounded.starts_with(&"é".repeat(REASON_CHARS)), "{bounded}");
        assert!(
            bounded.contains("reason cut at 1,024 of 1,029 characters"),
            "{bounded}"
        );
        assert!(
            !bounded.contains(&"é".repeat(REASON_CHARS + 1)),
            "{bounded}"
        );
    }

    #[test]
    fn only_a_name_a_program_could_have_bound_is_a_handle_name() {
        assert!(is_identifier("hits"));
        assert!(is_identifier("_a$1"));
        assert!(!is_identifier(""));
        assert!(!is_identifier("1a"));
        assert!(!is_identifier("a b"));
        assert!(!is_identifier("a.b"));
    }
}
