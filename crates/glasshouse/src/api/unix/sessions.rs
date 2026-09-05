use std::collections::{HashMap, HashSet};
use std::ffi::OsString;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use glasshouse::Runtime;
use glasshouse::config::{self, EffectiveConfig, UserConfig};
use glasshouse::events::MessageOrigin;
use glasshouse::guardrails::{self, GuardrailOverride, Origin};
use glasshouse::integrations::cmux;
use glasshouse::launch::HarnessLaunch;
use glasshouse::policy;
use glasshouse::session::api::{ApiError, SessionApi};
use glasshouse::session::{
    HarnessSelection, NewSession, SessionId, SessionLifecycle, SessionPresentation, SessionRole,
    SessionRuntime, SessionStore,
};

use super::assumptions::guardrail_error_message;
use super::memory::{Injected, deliver_memory, select_memory};
use super::{api_error, lock};
use crate::api::protocol::Response;

/// A session record's summary, as JSON: everything box 8 asks for that this
/// door can actually answer. `backend_resource`, `model` and `protocol` are
/// this project's durable record of a session's route — see
/// `SessionRecord`'s own doc comment — but no live *health* signal is
/// exposed here: `routing::free::ResourceHealth` lives in whichever
/// process's `Gateway` last computed a route, in memory, and this door's own
/// `SpawnSession` never touches the gateway at all (see that handler's doc
/// comment). A field this door cannot honestly fill in is left out rather
/// than reported as `null` and misread as "no route assigned".
pub(super) fn session_summary(record: &glasshouse::session::SessionRecord) -> serde_json::Value {
    serde_json::json!({
        "session": record.id.as_str(),
        "harness": record.harness,
        "role": record.role.as_str(),
        "lifecycle": lifecycle_str(record.lifecycle),
        "presentation": record.presentation.as_str(),
        "presentation_ref": record.presentation_ref,
        "backend_resource": record.backend_resource,
        "model": record.model.as_ref().map(|m| m.label().to_owned()),
        "protocol": record.protocol.as_ref().map(|p| format!("{p:?}")),
    })
}

pub(super) fn lifecycle_str(lifecycle: SessionLifecycle) -> &'static str {
    match lifecycle {
        SessionLifecycle::Starting => "starting",
        SessionLifecycle::Running => "running",
        SessionLifecycle::Idle => "idle",
        SessionLifecycle::WaitingForUser => "waiting_for_user",
        SessionLifecycle::Stopped => "stopped",
        SessionLifecycle::Failed => "failed",
        SessionLifecycle::Closed => "closed",
    }
}

/// Parse the wire spelling of a role — box 1: a session is tagged with the
/// orchestrator role by naming it here, never by a separate proprietary
/// mechanism. Absent means [`SessionRole::Worker`]: every session this door
/// spawns is spawned by something other than a person (this module's own
/// doc comment), so an unstated role is a worker rather than
/// indistinguishable from a session a person started by hand.
fn parse_role(role: Option<&str>) -> Result<SessionRole, String> {
    match role {
        None => Ok(SessionRole::Worker),
        Some(text) => [
            SessionRole::Normal,
            SessionRole::Orchestrator,
            SessionRole::Worker,
        ]
        .into_iter()
        .find(|candidate| candidate.as_str() == text)
        .ok_or_else(|| {
            format!(
                "`{text}` is not a role Glasshouse knows; the roles are: normal, orchestrator, worker"
            )
        }),
    }
}

/// Start a new session under an installed harness — box 3.
///
/// Deliberately narrower than `main.rs`'s own `launch_session`: it resolves
/// which executable answers to `harness` through `session::select`, the same
/// resolver `launch_session` uses, but skips launch-profile and
/// response-profile resolution entirely. Both live behind `config::` and
/// `profile::` machinery this phase's packet does not hold, and — more to
/// the point — both are about how a session *presents itself to a person*,
/// which an API-spawned session run by something other than a person has no
/// occasion to need. What every session gets regardless — a store record and
/// a running process this door can message and interrupt — this gives in
/// full. Argued in the evidence ledger as the deliberate simplification it
/// is, not an oversight.
#[allow(clippy::too_many_arguments)]
pub(super) fn spawn_session(
    runtime: &Runtime,
    store: &SessionStore<'_>,
    live: &Mutex<SessionRuntime>,
    harness: &str,
    args: Vec<String>,
    role: Option<&str>,
    task: Option<String>,
    guardrail: Option<GuardrailOverride>,
    presentation: Option<&str>,
    injected: &Injected,
    policied: &Policied,
) -> Response {
    let role = match parse_role(role) {
        Ok(role) => role,
        Err(err) => return Response::err(err),
    };
    let user = match UserConfig::load(runtime.paths()) {
        Ok(user) => user,
        Err(err) => return Response::err(err),
    };
    let project_config = match config::load_project_config(runtime.project()) {
        Ok(project_config) => project_config,
        Err(err) => return Response::err(err),
    };
    let effective = EffectiveConfig::new(&user, project_config.as_ref());
    let selection = match glasshouse::session::select(Some(harness), effective) {
        Ok(selection) => selection,
        Err(err) => return Response::err(err),
    };

    // Phase 17 lines 757 and 761 — presented in cmux when asked *and* when
    // cmux answers. When it does not, the spawn is the headless one this
    // verb always did, and the answer says why: line 755's rule, applied
    // through the door.
    let mut presentation_note = None;
    match presentation.map(cmux::Backend::parse).transpose() {
        Err(err) => return Response::err(err),
        Ok(Some(cmux::Backend::Cmux)) => match cmux::detect() {
            cmux::Availability::Available(control) => {
                return spawn_in_cmux(runtime, store, &control, &selection, args, task);
            }
            cmux::Availability::Absent(reason) => {
                presentation_note = Some(format!(
                    "cmux is not available ({reason}); the session runs headless in this Glasshouse"
                ));
            }
        },
        Ok(None) => {}
    }

    let record = match store.create(
        NewSession::embedded(selection.id().slug())
            .with_presentation(SessionPresentation::Headless)
            .with_role(role),
    ) {
        Ok(record) => record,
        Err(err) => return Response::err(err),
    };

    // Phase 21K line 1008: the per-task override, recorded on the new
    // session's ledger the moment its record exists and before its process
    // does, so no preflight it could ever make answers without it. The
    // origin is `agent`: a spawn is an orchestrator's request, and the door
    // records who asked rather than who the orchestrator was acting for —
    // `protocol::RequestOrigin`'s own attribution boundary. Refused loudly
    // rather than spawned without it: a worker started under a waiver its
    // gate does not know about is the one outcome worse than no worker.
    if let Some(kind) = guardrail
        && let Err(err) =
            guardrails::record_override(runtime, record.id.as_str(), kind, Origin::Agent)
    {
        return Response::err(format!(
            "session `{}` was recorded but its guardrail override could not be: {}",
            record.id,
            guardrail_error_message(&err)
        ));
    }

    // Lifecycle hooks — capability map line 734, *"detect worker turn
    // completion from native lifecycle hooks when available."*
    //
    // Until this existed, a session spawned through this door was the one
    // kind of Glasshouse session that reported nothing: `main.rs`'s
    // `launch_session` installs a hook document and this path did not, so a
    // worker an orchestrator started could finish a turn and leave no trace
    // of having finished. The wake-up flow's whole producer is the `Stop`
    // this makes the harness send.
    //
    // Best effort, exactly as `main.rs`'s own installation is: a harness with
    // no verified hook mechanism, or a document that cannot be written, is a
    // session Glasshouse knows less about — and that is a far smaller loss
    // than refusing to spawn a worker somebody asked for.
    //
    // `Application::none` because this door deliberately resolves no
    // response profile (see this function's own doc comment above); the
    // reason travels with it rather than being an empty cell.
    let mut launch_args = args;
    launch_args.extend(
        install_worker_hooks(runtime, &selection, &record.id, &effective)
            .into_iter()
            .map(|arg| arg.to_string_lossy().into_owned()),
    );

    let launch =
        HarnessLaunch::new(selection.executable().clone(), runtime.project()).args(launch_args);

    // Line 1125's context-selection step, between the spawn and the task.
    // Run here — before the runtime lock, and only when there is a task to
    // select context *for* — because opening the project's memory goes
    // through `database::open`, which takes a write transaction under a busy
    // timeout, and nothing that can wait may hold the lock every live
    // session's I/O runs through. A spawn with no task selects nothing and
    // reaches the harness exactly as it did before this phase.
    let briefing = task
        .as_deref()
        .and_then(|task| select_memory(runtime, &record.id, task, injected));

    let mut guard = lock(live);
    if let Err(err) = guard.start(record.id.clone(), SessionPresentation::Headless, &launch) {
        return Response::err(err);
    }

    // Box 6: a natural-language task assigned at spawn, distinct from
    // `Request::SendMessage`'s follow-up to a session that already exists.
    // Delivered through the same seam `SendMessage` uses, immediately after
    // the process is live, so a caller that asks for both a spawn and a
    // task gets one atomic call rather than a spawn that can race a
    // separate `send_message`.
    if let Some(task) = task {
        let mut api = SessionApi::new(store, &mut guard);
        // Before the task, never merged into it: what the caller asked for is
        // delivered as its own message, byte for byte, and the memory arrives
        // as a separately labelled one. See `deliver_memory` for why a
        // failure here is not a failed spawn.
        deliver_memory(runtime, &mut api, &record.id, briefing, injected);
        // And after it, Glasshouse's own implementation policy — lines
        // 955-990. After, deliberately: the memory block is what this project
        // learned and the policy is what Glasshouse says about how to work,
        // and a reader that meets the trusted instruction second meets it
        // last, next to the task it applies to.
        deliver_policy(&mut api, runtime, &record.id, policied);
        // `MessageOrigin::Machine`, unconditionally and with no request
        // field to override it: a task delivered at spawn is something the
        // caller asked Glasshouse to start a session *with*, not a line a
        // person typed into a session that was already running. There is no
        // `glasshouse api spawn`, so nothing human reaches this branch.
        if let Err(err) = api.send_text(&record.id, &task, MessageOrigin::Machine) {
            return Response::err(format!(
                "session `{}` was spawned but its task could not be delivered: {err}",
                record.id
            ));
        }
    }

    let mut result = serde_json::json!({ "session": record.id.as_str() });
    if let Some(note) = presentation_note {
        result["presentation"] = "headless".into();
        result["presentation_note"] = note.into();
    }
    Response::ok(result)
}

/// Open a cmux workspace running an ordinary `glasshouse launch` of the
/// selected harness, wait briefly for it to record itself, and deliver the
/// task through cmux — Phase 17 lines 757 and 761 through the door.
///
/// This process starts nothing else and holds nothing: the session belongs
/// to the launch inside the pane, which records it (`External`, with the
/// workspace) and installs its own hooks. That is also why the `role` this
/// door parsed is not carried — `launch` takes none, so the pane's session
/// is recorded as `normal`; the answer's `presentation` says which path was
/// taken so a caller can tell.
///
/// `focus: false`: an orchestrator spawning a worker must not have its own
/// view taken away.
fn spawn_in_cmux(
    runtime: &Runtime,
    store: &SessionStore<'_>,
    control: &impl cmux::CmuxControl,
    selection: &HarnessSelection,
    args: Vec<String>,
    task: Option<String>,
) -> Response {
    let executable = match std::env::current_exe() {
        Ok(path) => path,
        Err(err) => {
            return Response::err(format!(
                "cannot locate this glasshouse executable for the pane to run: {err}"
            ));
        }
    };
    let paths = runtime.paths();
    let root = runtime.project().display_root().to_path_buf();
    let global: Vec<OsString> = vec![
        "--scope".into(),
        root.as_os_str().to_owned(),
        "--data-dir".into(),
        paths.data_dir().as_os_str().to_owned(),
        "--config-dir".into(),
        paths.config_dir().as_os_str().to_owned(),
    ];
    let mut launch: Vec<OsString> = vec![
        "launch".into(),
        selection.id().slug().into(),
        "--presentation-ref".into(),
        "caller".into(),
    ];
    if !args.is_empty() {
        launch.push("--".into());
        launch.extend(args.iter().map(OsString::from));
    }
    let workspace = cmux::NewWorkspace {
        name: format!("glasshouse {}", selection.id().slug()),
        cwd: root,
        command: cmux::pane_command(&executable, &global, &launch),
        focus: false,
    };

    let before = match cmux::recorded_panes(store) {
        Ok(before) => before,
        Err(err) => return Response::err(err),
    };
    let pane = match control.create_workspace(&workspace) {
        Ok(pane) => pane,
        Err(err) => {
            return Response::err(format!(
                "cmux could not open a workspace for the session: {err}"
            ));
        }
    };
    let session = match cmux::await_session_at(store, &pane, &before, cmux::RECORD_WAIT) {
        Ok(session) => session,
        Err(err) => return Response::err(err),
    };
    let task_delivery = match (task, &session) {
        (None, _) => serde_json::Value::Null,
        (Some(task), Some(_)) => match control.send_line(&pane, &task) {
            Ok(()) => "cmux".into(),
            Err(err) => {
                return Response::err(format!(
                    "the session was spawned in cmux {pane} but its task could not be \
                     delivered: {err}"
                ));
            }
        },
        (Some(_), None) => {
            "not delivered: the session has not recorded itself in the pane yet".into()
        }
    };
    Response::ok(serde_json::json!({
        "session": session.as_ref().map(SessionId::as_str),
        "presentation": "external",
        "presentation_ref": pane.as_str(),
        "task_delivery": task_delivery,
    }))
}

/// Line 758's fallback: a session this door cannot reach, but which has a
/// pane, is reached through cmux — and the answer says so. A session with
/// no pane gets the same `NotLive` refusal it always did, and a pane cmux
/// cannot reach right now is reported with the reason, never retried
/// silently.
pub(super) fn send_through_pane(store: &SessionStore<'_>, id: &SessionId, text: &str) -> Response {
    let not_live = api_error(ApiError::NotLive { id: id.clone() });
    let record = match store.get(id) {
        Ok(Some(record)) => record,
        Ok(None) => return Response::err(not_live),
        Err(err) => return Response::err(err),
    };
    let Some(reference) = record.presentation_ref.as_deref() else {
        return Response::err(not_live);
    };
    match cmux::detect() {
        cmux::Availability::Absent(reason) => Response::err(format!(
            "{not_live}; it is presented in cmux {reference}, and cmux is not available from \
             here to reach it ({reason})"
        )),
        cmux::Availability::Available(control) => {
            match cmux::send_line(reference, text, &control) {
                Ok(pane) => Response::ok(serde_json::json!({
                    "via": "cmux",
                    "presentation_ref": pane.as_str(),
                })),
                Err(err) => Response::err(format!(
                    "{not_live}; delivering through its cmux pane failed: {err}"
                )),
            }
        }
    }
}

fn lock_muted(muted: &Muted) -> std::sync::MutexGuard<'_, HashMap<String, Instant>> {
    muted
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// How much longer `id` is muted for, or `None` — capability map line 1717.
///
/// Expiry happens **here**, on the read, and the expired entry is removed as
/// it is found. There is no sweeper: a mute nobody asks about has nobody to
/// affect, and a map keyed by live sessions in a process that owns them
/// cannot grow past the sessions that process started.
///
/// Says nothing about project scope. Every caller resolves the session
/// through [`SessionApi`] first, so a foreign id never reaches this.
pub(super) fn mute_remaining(muted: &Muted, id: &SessionId) -> Option<Duration> {
    let now = Instant::now();
    let mut map = lock_muted(muted);
    let until = *map.get(id.as_str())?;
    match until.checked_duration_since(now) {
        Some(remaining) if !remaining.is_zero() => Some(remaining),
        _ => {
            map.remove(id.as_str());
            None
        }
    }
}

/// What a machine caller is told when the session it addressed is muted.
///
/// Names the remaining time, because the caller's next question is when to
/// try again, and names the two things a mute does not touch — a person's own
/// message, and an interrupt — because a caller that read this as "this
/// session is unreachable" would stop being able to stop a runaway worker.
///
/// Carries no part of the refused text: what an orchestrator was about to say
/// is not a fact about the mute, and this sentence travels to a caller and
/// into whatever logs it.
pub(super) fn mute_refusal(id: &SessionId, remaining: Duration) -> String {
    format!(
        "session `{id}` is muted for another {}s: a person asked Glasshouse to stop \
         delivering orchestrator messages to it. Their own messages still arrive, and an \
         interrupt is never muted. `glasshouse api unmute --session {id}` lifts it early.",
        // Rounded up, for `SessionApi::send_text`'s reason: a refusal naming
        // `0s` while still refusing reads as a bug.
        remaining.as_secs() + u64::from(remaining.subsec_nanos() > 0)
    )
}

/// `Request::MuteSession` — capability map line 1717.
///
/// Scope-checked through [`SessionApi`] before any state is touched, like
/// every other verb on this door, and **not** gated on the session being
/// live: muting a session whose process has stopped is a statement about
/// deliveries, and a session that is about to be restarted is exactly one
/// somebody might want left alone first.
pub(super) fn mute_session(
    store: &SessionStore<'_>,
    live: &Mutex<SessionRuntime>,
    muted: &Muted,
    session: &str,
    seconds: u64,
) -> Response {
    let id = SessionId::new(session);
    {
        let mut guard = lock(live);
        let api = SessionApi::new(store, &mut guard);
        if let Err(err) = api.state(&id) {
            return Response::err(api_error(err));
        }
    }
    // Refused rather than accepted as a mute that is over before it is
    // recorded. A zero here is the shape this project keeps paying for — an
    // empty result indistinguishable from a successful one (practice §68) —
    // and the caller is still there to be told.
    if seconds == 0 {
        return Response::err(format!(
            "a mute needs a duration: `{session}` was asked to be muted for 0 seconds, which \
             would be no mute at all. Ask for the time you want, or use `unmute` to lift one."
        ));
    }
    let granted = seconds.min(MAX_MUTE_SECONDS);
    lock_muted(muted).insert(
        id.as_str().to_owned(),
        Instant::now() + Duration::from_secs(granted),
    );
    Response::ok(serde_json::json!({
        "session": id.as_str(),
        "muted_for_seconds": granted,
        // Said out loud, never silently: a caller that asked for a week and
        // got twelve hours has to be able to see that from the answer.
        "capped": granted < seconds,
    }))
}

/// `Request::UnmuteSession` — capability map line 1717.
///
/// Idempotent, and says which it was. Lifting a mute nobody set is the state
/// the caller asked for; reporting it as an error would make "make sure this
/// session is reachable" a call you cannot safely make twice.
pub(super) fn unmute_session(
    store: &SessionStore<'_>,
    live: &Mutex<SessionRuntime>,
    muted: &Muted,
    session: &str,
) -> Response {
    let id = SessionId::new(session);
    {
        let mut guard = lock(live);
        let api = SessionApi::new(store, &mut guard);
        if let Err(err) = api.state(&id) {
            return Response::err(api_error(err));
        }
    }
    let was_muted = mute_remaining(muted, &id).is_some();
    lock_muted(muted).remove(id.as_str());
    Response::ok(serde_json::json!({
        "session": id.as_str(),
        "was_muted": was_muted,
    }))
}

/// Deliver Glasshouse's own implementation policy to `session`, once —
/// capability map lines 955-990.
///
/// A separate function, not a second `Injection`: [`deliver_memory`] carries
/// text Glasshouse *quoted*, this carries text Glasshouse *wrote* — every
/// byte a literal in `glasshouse::policy`, with no untrusted body to keep
/// from forging a label, so it gets its own marker pair, distinct from
/// `MEMORY_MARKER`.
/// Once per session: `policied` is the record, checked before the first
/// line goes out so a session is never given half of a second copy, and
/// written only after every line has actually been sent, for the reason
/// [`deliver_memory`] writes its own ledger late.
/// Several lines because thirty rules do not fit in one — a delivery longer
/// than a terminal's canonical line limit is discarded *and* wedges that
/// session's input permanently, which is why `policy::deliveries` bounds
/// every element and this function sends them one at a time.
///
/// Failure is never a delivery failure, as [`deliver_memory`]: a send that
/// fails is logged and swallowed, and the task still goes.
// History: design-decisions.md, "Trims: api, events, harness and config module docs, second packet", crates/glasshouse/src/api/unix/sessions.rs `deliver_policy`.
pub(super) fn deliver_policy(
    api: &mut SessionApi<'_>,
    runtime: &Runtime,
    session: &SessionId,
    policied: &Policied,
) {
    if !policy_delivery_enabled(runtime) {
        return;
    }
    if lock_policied(policied).contains(session.as_str()) {
        return;
    }

    for line in policy::deliveries() {
        if let Err(err) = api.send_text(session, &line, MessageOrigin::Machine) {
            tracing::warn!(
                session = %session,
                error = %api_error(err),
                "could not deliver Glasshouse's implementation policy to a session; its task is \
                 being sent without it"
            );
            return;
        }
    }

    lock_policied(policied).insert(session.as_str().to_owned());
}

/// Whether this project wants the policy delivered — `implementation_policy`
/// in the project's own configuration, then the user's, then the default,
/// which is `true`.
///
/// Read per delivery rather than cached, so a team that turns it off does not
/// have to restart a long-running door for the change to take. Every failure
/// path answers `true`: a configuration file that cannot be read is not a
/// decision to stay silent, and the same read failing is already reported by
/// every other command that loads it.
fn policy_delivery_enabled(runtime: &Runtime) -> bool {
    let Ok(user) = UserConfig::load(runtime.paths()) else {
        return true;
    };
    let Ok(project) = config::load_project_config(runtime.project()) else {
        return true;
    };
    EffectiveConfig::new(&user, project.as_ref())
        .implementation_policy_enabled()
        .value
}

/// Take the policy ledger's lock, ignoring poisoning, for the reason
/// [`lock_injected`] gives.
fn lock_policied(policied: &Policied) -> std::sync::MutexGuard<'_, HashSet<String>> {
    policied
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Write the harness's lifecycle-hook document for a session this door is
/// about to start, and return the arguments that make the harness read it.
///
/// The same public seam `main.rs`'s own `install_session_document` uses —
/// [`glasshouse::harness::HookCommand`] and
/// `session::HarnessSelection::install_session_document` — rather than a
/// second mechanism. There is one hook vocabulary in this crate and one
/// place that installs it; a door with its own would be a second source of
/// truth for what a Glasshouse session reports.
///
/// Every path the hook needs is pinned from the runtime, because a hook runs
/// as a fresh process with whatever working directory the harness gives it —
/// see `HookCommand::report`'s own doc comment. Nothing here is derived from
/// the environment.
fn install_worker_hooks(
    runtime: &Runtime,
    selection: &glasshouse::session::HarnessSelection,
    id: &SessionId,
    effective: &EffectiveConfig<'_>,
) -> Vec<std::ffi::OsString> {
    let program = match std::env::current_exe() {
        Ok(program) => program,
        Err(err) => {
            tracing::warn!(error = %err, "could not find the Glasshouse executable for hooks");
            return Vec::new();
        }
    };
    let report = glasshouse::harness::HookCommand::new(
        program,
        id.as_str(),
        runtime.session_dir(id.as_str()),
        runtime.project().root(),
        runtime.paths().data_dir(),
        runtime.paths().config_dir(),
    );
    let consent = effective.project_hooks(selection.id()).value;
    let response = glasshouse::harness::response::Application::none(
        "the control API resolves no response profile for a session it spawns",
    );
    match selection.install_session_document(&report, consent, &response) {
        Ok(document) => document.args,
        Err(err) => {
            tracing::warn!(
                session = %id,
                error = %format!("{err:#}"),
                "could not write a spawned worker's harness document"
            );
            Vec::new()
        }
    }
}

/// The sessions a person has temporarily muted, and when each mute runs out
/// — capability map line 1717.
///
/// # Why this is in memory, like [`Injected`] and for a sharper reason
///
/// A mute is a statement about deliveries *this process* is going to make.
/// The only thing that can send a machine message into a session is the
/// process holding that session's pseudo-terminal, and that is this one; a
/// door that has just started is not running any session that was muted
/// before it started. So a mute lost with the process is a mute whose
/// subject is gone too, and there is no window in which forgetting it lets a
/// message through that a persisted mute would have stopped.
///
/// That is the whole argument for keeping it out of SQLite, and it is worth
/// stating because the alternative — a `muted_until` column — would have
/// been a migration, a reconciliation against a runtime the database cannot
/// see, and a row that outlives what it is about.
///
/// Keyed by the session id's string, like [`Injected`], for the same reason.
pub(super) type Muted = Arc<Mutex<HashMap<String, Instant>>>;

/// The longest a session may be muted in one call — capability map line
/// 1717's *"temporarily"*, made a number.
///
/// Twelve hours is longer than any working session and far short of
/// indefinite: a person who wants a worker left alone for the afternoon can
/// say so, and a mute nobody remembers setting still ends inside a day.
/// Applied as a cap rather than a refusal, like every other bound on this
/// door — a caller may ask for less and cannot ask for more — and the
/// response says what was actually granted, so an asked-for week is visibly
/// not what happened.
const MAX_MUTE_SECONDS: u64 = 12 * 60 * 60;

/// Which live sessions have already been given Glasshouse's implementation
/// policy — `glasshouse::policy`, capability map lines 955-990.
///
/// In memory and keyed by session-id string for exactly the reasons
/// [`Injected`] is: the claim "this session has read the policy" is about a
/// pseudo-terminal this process holds, and it stops being true the moment
/// that process or that session does.
///
/// A set rather than a map because the policy has no parts a session can have
/// some of: it is delivered whole, once, and either has been or has not.
/// Unbounded only in the number of sessions this process has ever briefed,
/// which is the same bound [`Injected`]'s outer map already carries.
pub(super) type Policied = Arc<Mutex<HashSet<String>>>;
