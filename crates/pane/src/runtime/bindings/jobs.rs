//! A `bash` call whose command outlives the cell's bound, and `bg.wait`,
//! which collects it.
//!
//! **The invariant: collecting a handed-on call gives the program exactly
//! what the call would have returned had it waited**, and records it as that
//! call -- so a test run collected a cell later still counts as the task's
//! verification, with its real exit code. Until then the call is recorded
//! with a `yielded` argument and is not one: a command still running has
//! not passed or failed anything.

use super::*;

/// How long `bg.wait` waits when the program names no `timeout`. Long,
/// because waiting is what the program asked for; bounded, so a job that
/// never ends costs a `{running: true}` rather than a cell that never does.
const DEFAULT_WAIT: std::time::Duration = std::time::Duration::from_secs(600);

/// Whether a `bash` call's argument object asked to wait to the end
/// (`wait: true`). Read before the arguments are, because it is not an
/// argument of the tool: it is how long the cell waits for it.
pub(super) fn wait_to_end(scope: &mut v8::PinScope, value: v8::Local<v8::Value>) -> bool {
    let Ok(object) = v8::Local::<v8::Object>::try_from(value) else {
        return false;
    };
    let Some(key) = v8::String::new(scope, "wait") else {
        return false;
    };
    object.get(scope, key.into()).is_some_and(|given| {
        given.is_true() || (given.is_string() && given.to_rust_string_lossy(scope) == "true")
    })
}

/// Takes a command the call handed back ([`invoke::Yielding`]) onto the
/// board as a job, and makes the call's own result say so. `None` when the
/// command finished within the bound, which leaves the call untouched.
pub(super) fn hand_on(
    state: &RuntimeState,
    traced: &mut invoke::Traced,
    yielding: Option<&invoke::Yielding>,
    bound: Option<std::time::Duration>,
) -> Option<String> {
    let running = yielding?.handed.borrow_mut().take()?;
    let command = traced.checked.get("command").cloned().unwrap_or_default();
    let job = bg::adopt(&state.session, &command, running);
    if let Ok(result) = &mut traced.outcome {
        state.yielded.borrow_mut().insert(
            job.clone(),
            crate::runtime::state::YieldedCall {
                checked: traced.checked.clone(),
                grant: result.grant.clone(),
                confinement: result.confinement,
            },
        );
        result.stderr = format!(
            "[pane] still running after {} s; it goes on in the background as bg/{job}. \
             `await bg.wait(result.job)` returns what this call would have; otherwise its \
             exit arrives as a bg.done event.",
            bound.map_or(0, |bound| bound.as_secs())
        );
    }
    traced.checked.insert("yielded".to_string(), job.clone());
    Some(job)
}

/// Marks a handed-on call's result: `running: true` and the `job` that
/// `bg.wait` takes.
pub(super) fn mark_running(scope: &mut v8::PinScope, value: v8::Local<v8::Value>, job: &str) {
    let Ok(object) = v8::Local::<v8::Object>::try_from(value) else {
        return;
    };
    let running = v8::Boolean::new(scope, true);
    set_fixed_key(scope, object, "running", running.into());
    let handle = job_object(scope, job);
    set_fixed_key(scope, object, "job", handle);
}

/// `bg.wait(job, {timeout})`: waits for a command job to exit and answers
/// with its result, or `{running: true, id}` when `timeout` (ms) passed
/// first. The cell's compute clock stops while it waits, as it does while a
/// call waits on its own child, and a person stopping the task stops it.
pub(super) fn bg_wait_callback(
    scope: &mut v8::PinScope,
    args: v8::FunctionCallbackArguments,
    mut retval: v8::ReturnValue,
) {
    let given = args.get(0);
    let id = match v8::Local::<v8::Object>::try_from(given) {
        Ok(object) => v8::String::new(scope, "id")
            .and_then(|key| object.get(scope, key.into()))
            .filter(|value| !value.is_null_or_undefined())
            .map(|value| value.to_rust_string_lossy(scope))
            .unwrap_or_default(),
        Err(_) if given.is_string() => given.to_rust_string_lossy(scope),
        Err(_) => String::new(),
    };
    if id.is_empty() {
        throw_tool_error(
            scope,
            "bg.wait needs a job: the `job` a running bash() returned, what bg.run returned, or its id",
        );
        return;
    }
    let bound = read_millis(scope, args.get(1), "timeout")
        .map_or(DEFAULT_WAIT, std::time::Duration::from_millis);
    let state = state(scope);
    let token = state.token.borrow().clone();
    let watchdog = state.watchdog_fired.borrow().clone();
    let isolate = scope.thread_safe_handle();
    let stopped = || {
        token.is_cancelled()
            || watchdog
                .as_ref()
                .is_none_or(|fired| fired.load(std::sync::atomic::Ordering::SeqCst))
            || isolate.is_execution_terminating()
    };
    let waited = {
        let _waiting = state.host_clock.pause();
        bg::wait(&state.session, &id, bound, &stopped)
    };
    match waited {
        bg::JobWait::Unknown => {
            throw_tool_error(scope, &format!("bg.wait: no command job named `{id}`"));
        }
        bg::JobWait::Running => {
            let object = v8::Object::new(scope);
            let running = v8::Boolean::new(scope, true);
            set_fixed_key(scope, object, "running", running.into());
            let job = js_string(scope, &id);
            set_fixed_key(scope, object, "id", job);
            retval.set(object.into());
        }
        bg::JobWait::Done(result) => {
            let yielded = state.yielded.borrow_mut().remove(&id);
            let value = match yielded {
                Some(call) => collected(scope, &state, &id, call, result),
                None => plain(scope, &result),
            };
            retval.set(value);
        }
    }
}

/// A handed-on call's result, built and recorded as the call itself would
/// have been: the `bash` shape, a handle, and a trajectory entry carrying
/// the real exit code.
fn collected<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    state: &Rc<RuntimeState>,
    id: &str,
    call: crate::runtime::state::YieldedCall,
    job: bg::JobResult,
) -> v8::Local<'s, v8::Value> {
    let exit_code = job.status.parse::<i32>().ok();
    let result = ToolResult {
        modified: None,
        tool: "bash".to_string(),
        stdout: job.stdout,
        stderr: job.stderr,
        exit_code,
        grant: call.grant,
        confinement: call.confinement,
    };
    let mut checked = call.checked;
    checked.insert("collected".to_string(), id.to_string());
    let command = checked.get("command").cloned().unwrap_or_default();
    trace(scope).record(CallRecord {
        tool: "bash".to_string(),
        args: checked.clone(),
        evidence: None,
        lifted_from: None,
        exit_code,
        repeat_of: None,
        error: None,
        ended: Ended::Ok,
    });
    let Some(bash) = registry::lookup("bash") else {
        return plain(
            scope,
            &bg::JobResult {
                stdout: result.stdout,
                stderr: result.stderr,
                status: job.status,
            },
        );
    };
    let args = Args::new().with("command", command);
    let (value, _) = typed_result(scope, bash, &args, &result, state, None, false);
    value
}

/// A `bg.run` job's result: its status and both streams, as its `bg.done`
/// payload holds them.
fn plain<'s>(scope: &mut v8::PinScope<'s, '_>, job: &bg::JobResult) -> v8::Local<'s, v8::Value> {
    let object = v8::Object::new(scope);
    let status = js_string(scope, &job.status);
    set_fixed_key(scope, object, "status", status);
    let stdout = js_string(scope, &job.stdout);
    set_fixed_key(scope, object, "stdout", stdout);
    let stderr = js_string(scope, &job.stderr);
    set_fixed_key(scope, object, "stderr", stderr);
    let exit_code = match job.status.parse::<i32>() {
        Ok(code) => v8::Number::new(scope, f64::from(code)).into(),
        Err(_) => v8::null(scope).into(),
    };
    set_fixed_key(scope, object, "exit_code", exit_code);
    object.into()
}
