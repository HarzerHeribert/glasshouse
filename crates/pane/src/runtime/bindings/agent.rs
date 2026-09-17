//! The handle `agent.run` answers with, and the one look a parent can take
//! at a running subagent.
//!
//! Split out of `bindings.rs` for the Phase 59 size ratchet, 2026-09-17;
//! nothing here is new but [`agent_progress`], which is the user's ruling of
//! that day: *"A parent model should check on a subagent from some time. But
//! even Claude Code does not do that."*

use super::*;

pub(super) fn agent_object<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    handle: &str,
) -> v8::Local<'s, v8::Value> {
    let object = v8::Object::new(scope);
    let id = js_string(scope, handle);
    set_fixed_key(scope, object, "id", id);
    let source = js_string(scope, &format!("agent/{handle}"));
    set_fixed_key(scope, object, "source", source);
    // The handle travels in the function's own data slot, the way a tool's
    // registry name does: one `fn` item serves every job object, and a
    // callback built per handle in a loop would be coerced to a fn pointer.
    let data = js_string(scope, handle);
    if let Some(function) = v8::Function::builder(agent_progress_callback)
        .data(data)
        .build(scope)
    {
        set_fixed_key(scope, object, "progress", function.into());
    }
    object.into()
}

/// `job.progress()` — what a running subagent has done so far.
///
/// **A look, never a wait.** It reads the board and returns; it makes no
/// provider request and cannot block on the subagent's thread, which is what
/// makes it safe to offer to a model that must not spin. `null` for a handle
/// with no progress to report — a job that is not a subagent, or one whose
/// session has already been shut down.
pub(super) fn agent_progress_callback(
    scope: &mut v8::PinScope,
    args: v8::FunctionCallbackArguments,
    mut retval: v8::ReturnValue,
) {
    let handle = args.data().to_rust_string_lossy(scope);
    let session = state(scope).session.clone();
    let Some(progress) = crate::bg::progress(&session, &handle) else {
        retval.set(v8::null(scope).into());
        return;
    };
    let object = v8::Object::new(scope);
    let turns = v8::Number::new(scope, progress.turns as f64);
    set_key(scope, object, "turns", turns.into());
    let elapsed = v8::Number::new(scope, progress.elapsed_ms as f64);
    set_key(scope, object, "elapsed_ms", elapsed.into());
    let running = v8::Boolean::new(scope, progress.running);
    set_key(scope, object, "running", running.into());
    let calls = v8::Array::new(scope, progress.calls.len() as i32);
    for (index, call) in progress.calls.iter().enumerate() {
        let name = js_string(scope, call);
        calls.set_index(scope, index as u32, name);
    }
    set_key(scope, object, "calls", calls.into());
    retval.set(object.into());
}
