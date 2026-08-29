//! The Unix domain socket transport and its request handlers.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::io::AsRawFd;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use glasshouse::Runtime;
use glasshouse::checkpoint::store::{ProjectCheckpoints, StoreError as CheckpointStoreError};
use glasshouse::checkpoint::{Checkpoint, CheckpointReason, Handoff};
use glasshouse::config::{self, EffectiveConfig, UserConfig};
use glasshouse::launch::HarnessLaunch;
use glasshouse::session::api::{ApiError, SessionApi};
use glasshouse::session::{
    NewSession, ProjectSessions, SessionId, SessionLifecycle, SessionPresentation, SessionRole,
    SessionRuntime, SessionStore,
};

use super::protocol::{Request, Response};

/// The socket file's name inside the project's own state directory, when
/// nothing overrides it.
const DEFAULT_SOCKET_NAME: &str = "control.sock";

/// `sockaddr_un.sun_path` is 104 bytes on macOS/BSD and 108 on Linux,
/// including the terminating nul this crate never sees. A path within this
/// bound is safe on every Unix `glasshouse` ships for; anything longer binds
/// with `EINVAL`/`ENAMETOOLONG` well before this door gets to authorize a
/// single connection. Chosen as 90 rather than the tighter platform minimum
/// so the margin survives a slightly longer project id without needing a
/// per-platform constant.
const MAX_SOCKET_PATH_BYTES: usize = 90;

/// How often the background tick answers terminal queries and reaps exited
/// sessions between requests. Mirrors `run_headless`'s `POLL` in `main.rs`:
/// short enough that `poll_exits` marks a dead session promptly, long enough
/// not to spin the accept thread's sibling for no reason.
const TICK: Duration = Duration::from_millis(50);

/// Run the control API until the process is killed.
///
/// Binds a fresh Unix domain socket at `socket_override`, or the project's
/// own state directory when nothing is given and that path fits
/// `sockaddr_un` — see [`socket_path_for`] for what happens when it does
/// not. Refuses to keep a stale file from a crashed prior run. Every
/// accepted connection is authorized (see [`authorize`]) before its one
/// request is read.
pub fn serve(runtime: &Runtime, socket_override: Option<PathBuf>) -> anyhow::Result<()> {
    let socket_path = match socket_override {
        Some(path) => path,
        None => socket_path_for(runtime),
    };
    if let Some(parent) = socket_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    // A stale socket from a process that never got to unlink it on the way
    // out must not permanently block this project's door.
    if socket_path.exists() {
        std::fs::remove_file(&socket_path)?;
    }
    let listener = UnixListener::bind(&socket_path)?;
    // Owner-only. Box 12's first half — see `authorize` for the second.
    std::fs::set_permissions(&socket_path, std::fs::Permissions::from_mode(0o600))?;

    eprintln!(
        "glasshouse: control API listening on {}",
        socket_path.display()
    );

    let sessions = ProjectSessions::open(runtime)?;
    let live = Arc::new(Mutex::new(SessionRuntime::new()));

    // The accept loop only touches the runtime while a request is being
    // handled; a session with nothing asking about it between requests would
    // otherwise never have its exit reaped. This mirrors `run_headless`'s own
    // reason for ticking outside the wait for the next event.
    {
        let live = Arc::clone(&live);
        std::thread::spawn(move || {
            loop {
                {
                    let mut live = lock(&live);
                    live.answer_terminal_queries();
                    for _ in live.poll_exits() {}
                }
                std::thread::sleep(TICK);
            }
        });
    }

    for incoming in listener.incoming() {
        let stream = match incoming {
            Ok(stream) => stream,
            Err(err) => {
                eprintln!("glasshouse: control API accept error: {err}");
                continue;
            }
        };
        if let Err(refusal) = authorize(&stream) {
            eprintln!("glasshouse: control API refused a connection: {refusal}");
            continue;
        }
        if let Err(err) = handle_connection(stream, runtime, &sessions, &live) {
            eprintln!("glasshouse: control API connection error: {err}");
        }
    }

    Ok(())
}

/// Where the socket binds when nothing overrides it.
///
/// The project's own state directory is preferred — it keeps the door next
/// to everything else this project owns, and cleans up the same way. But a
/// state directory nested under a long data directory (a test's temp
/// directory, a CI runner's workspace, a home directory itself deep in some
/// deployments) can push `control.sock`'s full path past `sockaddr_un`'s
/// limit, and `bind(2)` refuses that outright — not a permissions problem
/// [`authorize`] could ever see, a path that never becomes a socket at all.
/// When the preferred path would not fit, this falls back to a short name
/// under the system temp directory, keyed by the project id so two projects
/// never collide and one project always gets the same path back.
fn socket_path_for(runtime: &Runtime) -> PathBuf {
    let preferred = runtime.state_dir().join(DEFAULT_SOCKET_NAME);
    if preferred.as_os_str().len() <= MAX_SOCKET_PATH_BYTES {
        return preferred;
    }

    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(runtime.project().id().as_str().as_bytes());
    let digest = hasher.finalize();
    let short = hex::encode(&digest[..8]);
    std::env::temp_dir().join(format!("glasshouse-{short}.sock"))
}

/// Restrict the control channel to processes running as the same user —
/// box 12.
///
/// The socket file's `0600` permissions are the first check, enforced by the
/// kernel before `connect(2)` ever succeeds for another user; this is the
/// second, defence in depth against a umask or a copied file that loosened
/// them. `SO_PEERCRED` (Linux) and `getpeereid` (the BSD family, including
/// macOS) both read the credentials the kernel attached to the connection
/// itself — supplied by the connecting process's own kernel-verified
/// identity, not by anything the peer said — so there is nothing for an
/// unrelated local process to spoof by sending a convincing first line.
fn authorize(stream: &UnixStream) -> Result<(), String> {
    let peer_uid =
        peer_uid(stream).map_err(|err| format!("could not read peer identity: {err}"))?;
    let self_uid = process_uid();
    if peer_uid != self_uid {
        return Err(format!(
            "peer uid {peer_uid} does not match this Glasshouse's uid {self_uid}"
        ));
    }
    Ok(())
}

fn process_uid() -> u32 {
    // SAFETY: `getuid` takes no arguments, has no failure mode, and returns
    // a plain integer — there is no invariant for the caller to uphold.
    unsafe { libc::getuid() }
}

#[cfg(target_os = "linux")]
fn peer_uid(stream: &UnixStream) -> std::io::Result<u32> {
    let mut cred: libc::ucred = unsafe { std::mem::zeroed() };
    let mut len = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
    // SAFETY: `cred` and `len` are sized and initialized for exactly the
    // `SO_PEERCRED` option this reads, and the file descriptor is a live
    // socket owned by `stream` for the duration of the call.
    let ret = unsafe {
        libc::getsockopt(
            stream.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            &mut cred as *mut libc::ucred as *mut libc::c_void,
            &mut len,
        )
    };
    if ret != 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(cred.uid)
}

#[cfg(not(target_os = "linux"))]
fn peer_uid(stream: &UnixStream) -> std::io::Result<u32> {
    let mut uid: libc::uid_t = 0;
    let mut gid: libc::gid_t = 0;
    // SAFETY: `uid` and `gid` are valid, aligned out-parameters for the
    // duration of the call, and the file descriptor is a live socket owned
    // by `stream`.
    let ret = unsafe { libc::getpeereid(stream.as_raw_fd(), &mut uid, &mut gid) };
    if ret != 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(uid)
}

/// Take the runtime's lock, ignoring poisoning — a panicking handler thread
/// must not strand every session the door already holds. Mirrors
/// `main.rs`'s own `lock` helper for `run_headless`'s runtime.
fn lock(live: &Mutex<SessionRuntime>) -> std::sync::MutexGuard<'_, SessionRuntime> {
    live.lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Read one request line, dispatch it, and write back one response line.
fn handle_connection(
    stream: UnixStream,
    runtime: &Runtime,
    sessions: &ProjectSessions,
    live: &Mutex<SessionRuntime>,
) -> anyhow::Result<()> {
    let mut writer = stream.try_clone()?;
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    reader.read_line(&mut line)?;

    let response = match serde_json::from_str::<Request>(line.trim_end()) {
        Ok(request) => dispatch(request, runtime, sessions, live),
        Err(err) => Response::err(format!("malformed request: {err}")),
    };

    let mut payload = serde_json::to_string(&response)?;
    payload.push('\n');
    writer.write_all(payload.as_bytes())?;
    Ok(())
}

fn dispatch(
    request: Request,
    runtime: &Runtime,
    sessions: &ProjectSessions,
    live: &Mutex<SessionRuntime>,
) -> Response {
    let store = sessions.store();

    match request {
        // Through `SessionApi::list`, not `store.list` directly — box 13.
        // `SessionStore::list` trusts its own connection to already be
        // scoped to one project (true for every real database file, which
        // carries its own `project_id` and a trigger refusing any other
        // one), but `SessionApi` re-checks per record anyway, precisely for
        // the row that should never exist. Going around that seam here
        // would leave this door's own listing the one caller in the binary
        // that did not have to.
        Request::ListSessions => {
            let mut guard = lock(live);
            let api = SessionApi::new(&store, &mut guard);
            match api.list() {
                Ok(records) => Response::ok(serde_json::json!(
                    records.iter().map(session_summary).collect::<Vec<_>>()
                )),
                Err(err) => Response::err(api_error(err)),
            }
        }
        Request::SessionState { session } => {
            let mut guard = lock(live);
            let api = SessionApi::new(&store, &mut guard);
            match api.state(&SessionId::new(session)) {
                Ok(state) => Response::ok(serde_json::json!({ "lifecycle": lifecycle_str(state) })),
                Err(err) => Response::err(api_error(err)),
            }
        }
        Request::SpawnSession {
            harness,
            args,
            role,
            task,
        } => spawn_session(runtime, &store, live, &harness, args, role.as_deref(), task),
        Request::SendMessage { session, text } => {
            let mut guard = lock(live);
            let mut api = SessionApi::new(&store, &mut guard);
            match api.send_text(&SessionId::new(session), &text) {
                Ok(()) => Response::ok(serde_json::json!({})),
                Err(err) => Response::err(api_error(err)),
            }
        }
        Request::Interrupt { session } => {
            let mut guard = lock(live);
            let mut api = SessionApi::new(&store, &mut guard);
            match api.interrupt(&SessionId::new(session)) {
                Ok(()) => Response::ok(serde_json::json!({})),
                Err(err) => Response::err(api_error(err)),
            }
        }
        Request::ResourceCapacity => resource_capacity(runtime),
        Request::GetCheckpoint {
            checkpoint,
            document,
        } => get_checkpoint(runtime, checkpoint.as_deref(), document),
        Request::QueryMemory {
            query,
            history,
            limit,
        } => query_memory(runtime, &query, history, limit),
        Request::TakeCheckpoint {
            session,
            objective,
            state,
            decisions,
            failed_approaches,
            files,
            test_state,
            next_actions,
        } => request_checkpoint(
            runtime,
            sessions,
            session.as_deref(),
            objective,
            state,
            decisions,
            failed_approaches,
            files,
            test_state,
            next_actions,
        ),
    }
}

/// A session record's summary, as JSON: everything box 8 asks for that this
/// door can actually answer. `backend_resource`, `model` and `protocol` are
/// this project's durable record of a session's route — see
/// `SessionRecord`'s own doc comment — but no live *health* signal is
/// exposed here: `routing::free::ResourceHealth` lives in whichever
/// process's `Gateway` last computed a route, in memory, and this door's own
/// `SpawnSession` never touches the gateway at all (see that handler's doc
/// comment). A field this door cannot honestly fill in is left out rather
/// than reported as `null` and misread as "no route assigned".
fn session_summary(record: &glasshouse::session::SessionRecord) -> serde_json::Value {
    serde_json::json!({
        "session": record.id.as_str(),
        "harness": record.harness,
        "role": record.role.as_str(),
        "lifecycle": lifecycle_str(record.lifecycle),
        "backend_resource": record.backend_resource,
        "model": record.model.as_ref().map(|m| m.label().to_owned()),
        "protocol": record.protocol.as_ref().map(|p| format!("{p:?}")),
    })
}

fn lifecycle_str(lifecycle: SessionLifecycle) -> &'static str {
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

fn api_error(err: ApiError) -> String {
    err.to_string()
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
fn spawn_session(
    runtime: &Runtime,
    store: &SessionStore<'_>,
    live: &Mutex<SessionRuntime>,
    harness: &str,
    args: Vec<String>,
    role: Option<&str>,
    task: Option<String>,
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

    let record = match store.create(
        NewSession::embedded(selection.id().slug())
            .with_presentation(SessionPresentation::Headless)
            .with_role(role),
    ) {
        Ok(record) => record,
        Err(err) => return Response::err(err),
    };

    let launch = HarnessLaunch::new(selection.executable().clone(), runtime.project()).args(args);
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
        if let Err(err) = api.send_text(&record.id, &task) {
            return Response::err(format!(
                "session `{}` was spawned but its task could not be delivered: {err}",
                record.id
            ));
        }
    }

    Response::ok(serde_json::json!({ "session": record.id.as_str() }))
}

/// Retrieve a checkpoint — box 10, the read half of [`request_checkpoint`].
///
/// Resolution mirrors `main.rs`'s own `resolve_checkpoint` (private to that
/// file, so reproduced here against the same public [`ProjectCheckpoints`]
/// surface rather than duplicated by copy): a named id or unambiguous
/// prefix, `"latest"`, or nothing, all meaning what `glasshouse checkpoint
/// show` already means by them. Scoped to this project the same way every
/// other request this door answers is — the socket was opened for one
/// project's own database file, and a checkpoint's row lives in that file
/// alone (see `database.rs`'s per-project file separation).
fn get_checkpoint(runtime: &Runtime, checkpoint: Option<&str>, document: bool) -> Response {
    let checkpoints = match ProjectCheckpoints::open(runtime) {
        Ok(checkpoints) => checkpoints,
        Err(err) => return Response::err(err),
    };
    let store = checkpoints.store();

    let resolved = match checkpoint {
        None | Some("latest") => store.latest(),
        Some(named) => match store.resolve_id(named) {
            Ok(id) => store.get(&id),
            Err(err) => return Response::err(checkpoint_store_err(err)),
        },
    };

    match resolved {
        Ok(Some(stored)) => Response::ok(serde_json::json!({
            "checkpoint": stored.id.short(),
            "session": stored.checkpoint.session.as_str(),
            "harness": stored.checkpoint.harness,
            "trimmed": stored.checkpoint.trimmed,
            "document": if document {
                stored.checkpoint.render()
            } else {
                stored.checkpoint.bootstrap_prompt()
            },
        })),
        Ok(None) => Response::err("this project has no checkpoints yet"),
        Err(err) => Response::err(checkpoint_store_err(err)),
    }
}

fn checkpoint_store_err(err: CheckpointStoreError) -> String {
    err.to_string()
}

/// Current resource capacity and quota telemetry — capability map line 1679.
///
/// Mirrors `main.rs`'s own `resources_report` for its non-probe path: reads
/// the user's configuration, folds in the persisted gateway-quota and
/// gateway-health caches [`crate::api`]'s door doc comment already promises
/// this project shares with every other process, and asks each installed
/// harness for its own status the same cheap, no-quota way `glasshouse
/// resources` does with no flags. Never makes a network request — this
/// request carries no provider name to probe, unlike the CLI's own `--probe`.
fn resource_capacity(runtime: &Runtime) -> Response {
    let user = match UserConfig::load(runtime.paths()) {
        Ok(user) => user,
        Err(err) => return Response::err(err),
    };
    let project_config = match config::load_project_config(runtime.project()) {
        Ok(project_config) => project_config,
        Err(err) => return Response::err(err),
    };
    let effective = EffectiveConfig::new(&user, project_config.as_ref());
    let now_unix = glasshouse::provider::cache::now_unix_seconds();

    let telemetry = glasshouse::provider::resources::GatheredTelemetry::new()
        .gather_gateway_quota(&glasshouse::provider::telemetry::GatewayQuotaCache::new(
            runtime.paths(),
        ))
        .gather_gateway_health(&glasshouse::provider::telemetry::GatewayHealthCache::new(
            runtime.paths(),
        ))
        .gather_harness_status(now_unix);

    Response::ok(glasshouse::provider::resources::capacity_json(
        &effective, &telemetry, now_unix,
    ))
}

/// Search this project's durable memory — box 10, and Phase 21F lines
/// 935/936: the machine door carries each result's authority, validity
/// state, and — for a memory that may constrain implementation — its
/// rationale and invalidation conditions, as structured fields rather than
/// only inside a rendered string.
///
/// `invariants_and_constraints`/`other` is `main.rs`'s own
/// `memory_search_grouped` (line 929), the exact search
/// `glasshouse memory search` runs; `report` is `render_memory_report`'s
/// exact text over the same result, so this door and that command can never
/// disagree about what a query finds. One search, not two: the CLI's report
/// text is rendered from the already-fetched grouping rather than searched
/// for a second time.
fn query_memory(runtime: &Runtime, query: &str, history: bool, limit: usize) -> Response {
    let grouped = match crate::memory_search_grouped(runtime, query, history, limit) {
        Ok(grouped) => grouped,
        Err(err) => return Response::err(err),
    };
    let report = match crate::render_memory_report(&grouped, query, history) {
        Ok(report) => report,
        Err(err) => return Response::err(err),
    };

    Response::ok(serde_json::json!({
        "invariants_and_constraints": grouped
            .invariants_and_constraints
            .iter()
            .map(memory_result_json)
            .collect::<Vec<_>>(),
        "other": grouped.other.iter().map(memory_result_json).collect::<Vec<_>>(),
        "report": report,
    }))
}

/// One memory, as JSON for the machine door — Phase 21F lines 935/936.
///
/// `may_constrain_implementation` is `MemoryAuthority::is_binding` made
/// explicit as its own field rather than left for a caller to infer from
/// which of `validity_conditions`/`invalidation_conditions` happen to be
/// non-null: those two are deliberately `null` for a memory that is not
/// binding even if the row somehow carries text in them, for the same
/// reason `main.rs`'s `constraint_lines` gates on it rather than on
/// presence.
///
/// `review` is `null` unless `status` is currently `needs_review`: 21C's
/// `review_reason` is not cleared when a review is resolved back to another
/// status (`MemoryStore::set_status` never touches it), so it is stale
/// information once the status has moved on, and only `status` says whether
/// a challenge is still open.
fn memory_result_json(record: &glasshouse::memory::MemoryRecord) -> serde_json::Value {
    use glasshouse::memory::{MemoryAuthority, MemoryStatus};

    let may_constrain_implementation = record.authority.is_some_and(MemoryAuthority::is_binding);
    let review = (record.status == MemoryStatus::NeedsReview)
        .then_some(record.review_reason)
        .flatten()
        .map(|reason| {
            serde_json::json!({
                "reason": reason.as_str(),
                "marked_at": record.review_marked_at,
            })
        });

    serde_json::json!({
        "id": record.id.as_str(),
        "kind": record.kind.as_str(),
        "authority": record.authority.map(MemoryAuthority::as_str),
        "status": record.status.as_str(),
        "current": record.is_current(),
        "subject": record.subject,
        "body": record.body,
        "may_constrain_implementation": may_constrain_implementation,
        "rationale": record.provenance.rationale,
        "validity_conditions": if may_constrain_implementation {
            record.validity_conditions.clone()
        } else {
            None
        },
        "invalidation_conditions": if may_constrain_implementation {
            record.invalidation_conditions.clone()
        } else {
            None
        },
        "review": review,
        "last_validated_at": record.last_validated_at,
        "created_at": record.created_at,
    })
}

/// Take a checkpoint — box 11.
///
/// Mirrors `main.rs`'s `CheckpointCommand::Save` arm: the same session
/// resolution (`crate::active_session`, named or the project's most recently
/// active), the same `Checkpoint::capture`, the same store. Duplicated
/// rather than called through because that arm prints to standard output as
/// part of returning an `ExitCode`, which has nothing to do with what this
/// door writes to a socket.
#[allow(clippy::too_many_arguments)]
fn request_checkpoint(
    runtime: &Runtime,
    sessions: &ProjectSessions,
    session: Option<&str>,
    objective: String,
    implementation_state: String,
    decisions: Vec<String>,
    failed_approaches: Vec<String>,
    files: Vec<String>,
    test_state: Option<String>,
    next_actions: Vec<String>,
) -> Response {
    let record = match crate::active_session(sessions, session) {
        Ok(Some(record)) => record,
        Ok(None) => {
            return Response::err(
                "this project has no recorded sessions to check point; start one first",
            );
        }
        Err(err) => return Response::err(err),
    };

    let checkpoints = match ProjectCheckpoints::open(runtime) {
        Ok(checkpoints) => checkpoints,
        Err(err) => return Response::err(err),
    };
    let store = checkpoints.store();

    let checkpoint = Checkpoint::capture(
        &record.id,
        &record.harness,
        CheckpointReason::Manual,
        store.now(),
        runtime.project().root(),
        Handoff {
            objective,
            implementation_state,
            decisions,
            failed_approaches,
            files,
            test_state,
            next_actions,
        },
    );

    match store.save(checkpoint) {
        Ok(stored) => Response::ok(serde_json::json!({
            "checkpoint": stored.id.short(),
            "session": record.id.as_str(),
            "trimmed": stored.checkpoint.trimmed,
        })),
        Err(err) => Response::err(err),
    }
}
