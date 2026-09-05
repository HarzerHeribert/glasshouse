//! The only authority a cell has — map line 2463's in-program half.
//!
//! **Every one of these functions is the isolate's whole outside world.** The
//! four tools go through [`crate::tools::invoke::run`] and therefore through
//! the merged sandbox; `keep`, `free` and `handles` touch nothing but the
//! handle table; `console` writes to a bounded buffer. There is no file, no
//! socket, no timer, no `require` and no dynamic import here, because a V8
//! context has none of those unless an embedder adds them and this module is
//! the only place that adds anything.
//!
//! A refusal is a throw of `PermissionDenied` inside the model's own program
//! (`sandbox-grants.md` §1.4 and §5): catchable, final, and never a prompt.

use std::rc::Rc;

use crate::runtime::handles::HandleMeta;
use crate::runtime::marshal;
use crate::runtime::preview::{ArrayValue, FileValue, StringValue, Value};
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
delete globalThis.WebAssembly;
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

pub(crate) fn state(scope: &v8::PinScope) -> Rc<RuntimeState> {
    scope
        .get_slot::<Rc<RuntimeState>>()
        .expect("the runtime installs its state before any callback can run")
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
        set_key(scope, global, name, function.into());
    }

    if let Some(function) = v8::Function::builder(keep_callback).build(scope) {
        set_key(scope, global, "keep", function.into());
    }
    if let Some(function) = v8::Function::builder(free_callback).build(scope) {
        set_key(scope, global, "free", function.into());
    }
    if let Some(function) = v8::Function::builder(handles_callback).build(scope) {
        set_key(scope, global, "handles", function.into());
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
    if let Some(function) = v8::Function::builder(capture_callback).build(scope) {
        set_key(scope, object, "s", function.into());
    }
    if let Some(function) = v8::Function::builder(fell_callback).build(scope) {
        set_key(scope, object, "e", function.into());
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
    let outcome = {
        let context = ToolContext {
            profile: &state.profile,
            glasshouse: &state.glasshouse,
            session: &state.session,
        };
        // `run`, not a cancellable variant: `tools::invoke` exports no
        // cancellation token in this tree, and inventing a stand-in would be
        // a second answer to a question that package owns.
        invoke::run(&context, tool.name(), &call_args)
    };

    match outcome {
        Ok(result) => {
            let value = typed_result(scope, tool, &call_args, &result, &state);
            retval.set(value);
        }
        Err(ToolError::Denied(denied)) => throw_denied(scope, &denied),
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
    let index = state.record_call(RecordedCall { preview, meta });
    if let Ok(object) = v8::Local::<v8::Object>::try_from(value) {
        let tag = call_tag(scope);
        let marker = v8::Integer::new(scope, index as i32);
        object.set_private(scope, tag, marker.into());
    }
    value
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
        let line = v8::Number::new(scope, found.line as f64);
        set_key(scope, object, "line", line.into());
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
    line: u64,
    text: String,
}

/// One match as `runtime-contract.md` §6's worked table shows it.
fn match_line(found: &GrepMatch) -> String {
    let text: String = found.text.chars().take(160).collect();
    format!("{}:{}  {text}", found.path, found.line)
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
                    line: digits.parse().unwrap_or(0),
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
                line: digits.parse().unwrap_or(0),
                text: line[colon + 1..].to_string(),
            };
        }
    }
    GrepMatch {
        path: fallback.to_string(),
        line: 0,
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
    capture(scope, &name, args.get(1));
}

fn free_callback(
    scope: &mut v8::PinScope,
    args: v8::FunctionCallbackArguments,
    _retval: v8::ReturnValue,
) {
    let name = args.get(0).to_rust_string_lossy(scope);
    let state = state(scope);
    state.table.borrow_mut().free(&name);
    state.note_free(&name);
    let context = scope.get_current_context();
    let global = context.global(scope);
    if let Some(key) = v8::String::new(scope, &name) {
        global.delete(scope, key.into());
    }
}

fn handles_callback(
    scope: &mut v8::PinScope,
    _args: v8::FunctionCallbackArguments,
    mut retval: v8::ReturnValue,
) {
    let state = state(scope);
    let names: Vec<String> = state
        .table
        .borrow()
        .names()
        .into_iter()
        .map(str::to_string)
        .collect();
    let array = v8::Array::new(scope, names.len() as i32);
    for (index, name) in names.iter().enumerate() {
        let value = js_string(scope, name);
        array.set_index(scope, index as u32, value);
    }
    retval.set(array.into());
}

fn capture_callback(
    scope: &mut v8::PinScope,
    args: v8::FunctionCallbackArguments,
    _retval: v8::ReturnValue,
) {
    let name = args.get(0).to_rust_string_lossy(scope);
    capture(scope, &name, args.get(1));
}

fn fell_callback(
    scope: &mut v8::PinScope,
    _args: v8::FunctionCallbackArguments,
    _retval: v8::ReturnValue,
) {
    state(scope).current.borrow_mut().fell_off_the_end = true;
}

/// The one path by which a value becomes a handle: it is put on the
/// persistent scope so the next cell can name it, and its preview is
/// recorded so this turn can show it.
fn capture(scope: &mut v8::PinScope, name: &str, value: v8::Local<v8::Value>) {
    let state = state(scope);
    let context = scope.get_current_context();
    let global = context.global(scope);
    if let Some(key) = v8::String::new(scope, name) {
        global.set(scope, key.into(), value);
    }
    let (preview, meta) = preview_of(scope, &state, value);
    state.capture(name, preview, meta);
}

/// A tool-produced object keeps the preview and provenance its call already
/// built; anything else is marshalled now.
fn preview_of(
    scope: &mut v8::PinScope,
    state: &Rc<RuntimeState>,
    value: v8::Local<v8::Value>,
) -> (Value, HandleMeta) {
    if let Ok(object) = v8::Local::<v8::Object>::try_from(value) {
        let tag = call_tag(scope);
        if let Some(marker) = object.get_private(scope, tag)
            && let Some(index) = marker.number_value(scope)
            && index.is_finite()
            && index >= 0.0
            && let Some(call) = state.recorded(index as usize)
        {
            return (call.preview, call.meta);
        }
    }
    let preview = marshal::marshal(scope, value);
    let meta = HandleMeta {
        type_label: None,
        size_estimate: marshal::size_estimate(scope, value),
        provenance: None,
    };
    (preview, meta)
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
        assert_eq!(found.line, 12);
        assert_eq!(found.text, "pub mod runtime;");
    }

    #[test]
    fn a_path_with_a_colon_in_it_does_not_move_the_split() {
        let found = parse_match("C:/tmp/a.rs:7:let x = 1;", "root");
        assert_eq!(found.path, "C:/tmp/a.rs");
        assert_eq!(found.line, 7);
        assert_eq!(found.text, "let x = 1;");
    }

    #[test]
    fn a_line_without_a_filename_takes_the_requested_path() {
        let found = parse_match("9:hit", "/tmp/one.txt");
        assert_eq!(found.path, "/tmp/one.txt");
        assert_eq!(found.line, 9);
        assert_eq!(found.text, "hit");
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
