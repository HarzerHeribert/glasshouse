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

use crate::runtime::cell::{HOST_FUNCTIONS, RESERVED_PREFIX};
use crate::runtime::handles::HandleMeta;
use crate::runtime::isolate::DEFAULT_RESPONSE_BYTE_CAP;
use crate::runtime::marshal;
use crate::runtime::outcome::{CallRecord, Ended};
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

    let console = v8::Object::new(scope);
    for method in ["log", "info", "warn", "error", "debug", "trace"] {
        if let Some(function) = v8::Function::builder(console_callback).build(scope) {
            set_key(scope, console, method, function.into());
        }
    }
    set_key(scope, global, "console", console.into());
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
    // `mtime` is empty rather than guessed: the registry's `read` runs `cat`
    // and a `cat` result carries no modification time, and stat-ing the path
    // here would be a filesystem access from outside the sandbox that
    // confined the read.
    let mtime = js_string(scope, "");
    set_key(scope, object, "mtime", mtime);

    let preview = Value::File(FileValue {
        path,
        byte_len: result.stdout.len() as u64,
        line_count: lines.len() as u64,
        mtime: "unknown".to_string(),
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

// --- console -----------------------------------------------------------

fn console_callback(
    scope: &mut v8::PinScope,
    args: v8::FunctionCallbackArguments,
    _retval: v8::ReturnValue,
) {
    let mut parts: Vec<String> = Vec::with_capacity(args.length() as usize);
    for index in 0..args.length() {
        let value = args.get(index);
        parts.push(match value.to_string(scope) {
            // Bounded per argument: `console.log` of a megabyte is a
            // megabyte of copying for output that is capped anyway.
            Some(string) => {
                let text = string.to_rust_string_lossy(scope);
                text.chars().take(4096).collect()
            }
            None => "<unprintable>".to_string(),
        });
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
