//! The Unix domain socket transport and its request handlers.

use std::collections::{HashMap, HashSet};
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
use glasshouse::events::{
    EventBus, EventLog, EventLogSink, EventSink, GatewayFailure, LifecycleEvent, LoggedEvent,
    MessageOrigin, TurnOutcome,
};
use glasshouse::launch::HarnessLaunch;
use glasshouse::memory::inject::{self, Injection};
use glasshouse::memory::{MemoryId, ProjectMemory};
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

/// The hard ceiling on how many events [`Request::Events`] returns in one
/// call, regardless of the `limit` a caller asks for — box 701's "bounded
/// output" requirement. A caller that has fallen behind by more than this
/// many events gets a `head` past what it can see in this response and
/// polls again rather than pulling the whole table in one line of JSON.
const MAX_EVENTS_LIMIT: usize = 1000;

/// The hard ceiling on how many memories [`Request::QueryMemory`] returns in
/// one call, regardless of the `limit` a caller asks for — capability map
/// line 1115's *"concise results rather than dumping the complete memory
/// database into agent context."*
///
/// Generous against the default of twenty and still far short of a project's
/// whole store, because the point of the bound is that the response size
/// stops depending on how much the project has accumulated. A caller that
/// wants more searches again with a narrower query, which is what a ranked
/// search is for.
const MAX_MEMORY_LIMIT: usize = 100;

/// The hard ceiling on entries in any one [`Request::CurrentMemory`] section,
/// regardless of the `limit` asked for — line 1115 again, per section rather
/// than per response, because `memory::snapshot::snapshot` budgets each
/// section independently.
const MAX_SNAPSHOT_SECTION_LIMIT: usize = 50;

/// The hard ceiling on characters in any one [`Request::CurrentMemory`]
/// entry's body, regardless of the `body_chars` asked for.
///
/// A snapshot is a summary an agent reads to orient itself; a memory it needs
/// in full is one identifier away through [`Request::GetMemory`], which is
/// where an untruncated body belongs. Without this pair of ceilings a caller
/// passing `usize::MAX` to both would receive exactly the complete dump this
/// line forbids.
const MAX_SNAPSHOT_BODY_CHARS: usize = 2000;

/// Which of this project's memories each live session has already been sent
/// — capability map line 1135's *"already-aware hot session"*.
///
/// # Why this is in memory and not in the database
///
/// A hot session is a session this process holds a pseudo-terminal for. It
/// exists exactly as long as [`serve`]'s own [`SessionRuntime`] does, and so
/// does the fact that it has already read a memory: a session that has been
/// restarted has read nothing, and a `glasshouse` process that is not this
/// one holds no such session at all (see `super`'s module doc comment). A
/// durable table would therefore record a claim that outlived the thing it
/// was about, and would have to be reconciled against a runtime it cannot
/// see.
///
/// Keyed by the session id's string rather than by [`SessionId`] so this map
/// needs nothing from the session type it does not already have.
type Injected = Arc<Mutex<HashMap<String, HashSet<MemoryId>>>>;

/// The most memory identifiers remembered per session before this door stops
/// growing the set.
///
/// One delivery carries at most [`inject::MAX_INJECTED_MEMORIES`], so a
/// session has to be given more than fifty separate tasks, each selecting
/// entirely different memories, to reach this. Past it, the de-duplication
/// degrades toward re-injecting rather than toward unbounded growth in a
/// process that is meant to run for days.
const MAX_REMEMBERED_INJECTIONS: usize = 256;

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

    let sessions = ProjectSessions::open(runtime)?;
    // The door's own lifecycle stream, and the durable recording of it. See
    // [`EventRecorder`] for why the runtime is built around a bus this
    // function owns rather than `SessionRuntime::new`'s private one.
    let events = EventBus::new();
    let recorder = EventRecorder::attach(runtime, &events);
    let live = Arc::new(Mutex::new(SessionRuntime::with_event_bus(
        glasshouse::session::runtime::DEFAULT_SCROLLBACK_BYTES,
        events,
    )));
    let watches: Arc<Watches> = Arc::new(Mutex::new(Vec::new()));
    // Line 1135's record, alongside the runtime whose sessions it is about.
    let injected: Injected = Arc::new(Mutex::new(HashMap::new()));

    // Announced here rather than straight after `bind`, because everything
    // above it can still fail — a project database that cannot be opened
    // read-write ends this function — and a door that has said it is
    // listening and then exits is worse than one that has not spoken yet.
    // Every caller in this repository treats this line as the ready signal,
    // which is only true if it comes after the last thing that can refuse to
    // start. The socket is bound by now, so a client that connects between
    // this line and the accept loop waits in the backlog rather than being
    // refused.
    eprintln!(
        "glasshouse: control API listening on {}",
        socket_path.display()
    );

    // The accept loop only touches the runtime while a request is being
    // handled; a session with nothing asking about it between requests would
    // otherwise never have its exit reaped. This mirrors `run_headless`'s own
    // reason for ticking outside the wait for the next event.
    //
    // The orchestrator wake-up flow rides the same tick — see
    // [`pump_watches`]. It belongs here rather than in a thread of its own
    // because it needs exactly what this one already holds, and because a
    // watch that only advanced while a request happened to arrive would wake
    // an orchestrator only when it was already talking to the door.
    // `Runtime` is cheap to clone and holds no connection, so the thread
    // owns one rather than borrowing this function's.
    {
        let live = Arc::clone(&live);
        let watches = Arc::clone(&watches);
        let runtime = runtime.clone();
        std::thread::spawn(move || {
            let mut state: Option<WatchState> = None;
            let mut complained = false;
            loop {
                {
                    let mut guard = lock(&live);
                    guard.answer_terminal_queries();
                    for _ in guard.poll_exits() {}
                }
                // Nothing is opened, and no lock is taken beyond the registry
                // peek, until an orchestrator has actually registered
                // interest. A door nobody is watching through does exactly
                // what it did before this phase.
                if watching(&watches) {
                    if state.is_none() {
                        state = WatchState::open(&runtime);
                        if state.is_none() && !std::mem::replace(&mut complained, true) {
                            tracing::warn!(
                                "could not open this project's database to \
                                 deliver worker completions"
                            );
                        }
                    }
                    if let Some(state) = &state {
                        pump_watches(state, &live, &watches);
                    }
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
        if let Err(err) = handle_connection(
            stream, runtime, &sessions, &live, &watches, &recorder, &injected,
        ) {
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

/// How long [`project_events`] waits for the recorder to catch up before it
/// answers anyway.
///
/// Matches `shell::run`'s own flush bound, for the same reason it gives: a
/// bookkeeping step must never be the thing that makes the interface
/// unresponsive. Reached only when the writer is genuinely behind, because
/// the flush answers as soon as the queue drains.
const RECORDER_FLUSH: Duration = Duration::from_millis(500);

/// This door's durable recording of what happens to the sessions it owns.
///
/// # The hole this fills
///
/// `shell::run` builds an [`EventBus`], attaches an [`EventLogSink`] to it,
/// and hands the bus to its [`SessionRuntime`]. This door built its runtime
/// with `SessionRuntime::new()` — a bus with no sink and no subscriber — so
/// every lifecycle event of every orchestrated worker was published into
/// nothing. Not only the interventions of map line 748: `session_started`
/// and `process_exited` too. A worker's whole life left no durable trace
/// unless a `glasshouse hook` process happened to write a row from outside.
///
/// # Why the log is opened on the writer thread, and not before there is
/// something to write
///
/// `EventLog::open` goes through `database::open`, which takes a
/// `BEGIN IMMEDIATE` **write** transaction and runs the migration ladder,
/// under a five-second busy timeout. It is not a cheap handle to acquire and
/// it can genuinely wait — on the very `glasshouse hook` processes that run
/// inside a user's own session, which [`WatchState`]'s doc explains must
/// never be made to queue behind this door's bookkeeping.
///
/// So neither the accept thread nor a pseudo-terminal's thread ever performs
/// that open. The sink's writer thread does, on the first event it is handed,
/// which has three consequences worth stating separately:
///
/// - **A door that records nothing opens nothing.** `serve` attaches this
///   unconditionally, but a process that never starts a session publishes no
///   event, so the connection is never created. That is [`WatchState`]'s
///   pattern and it is here for §65's reason: a resource acquired on a path
///   nobody exercises is invisible to every test and still charged for at
///   runtime, on the platform where SQLite's locks are mandatory rather than
///   advisory.
/// - **The five-second wait, if it ever happens, is paid by a thread nobody
///   is waiting on.** No request is delayed, no pty is stalled, and
///   [`project_events`]'s flush is separately bounded, so even a caller that
///   asks for history while the open is in flight gets an answer.
/// - **A failure to open is not a failure to serve.** It is warned about once
///   and the door keeps working — the same direction `shell::attach_event_log`
///   trades in, for the same reason: a project whose database cannot be
///   opened should lose event history and keep its sessions.
///
/// # On holding the handle afterwards
///
/// Once open it is kept, because the alternative is re-running that
/// transaction and that ladder per event. It costs one connection, which is
/// not a new class of thing for this process: `serve` already opens
/// [`ProjectSessions`] unconditionally and holds it for the door's whole
/// life. In SQLite's rollback-journal mode an idle connection holds no lock
/// on any platform; what costs is the open, and this design performs at most
/// one of those.
struct EventRecorder {
    sink: Arc<EventLogSink>,
}

impl EventRecorder {
    /// Send everything `events` records to the project's log as well.
    fn attach(runtime: &Runtime, events: &EventBus) -> Self {
        let runtime = runtime.clone();
        // Opened by the closure below, on the writer thread, at the first
        // event — never here. `attempted` is what keeps a project whose
        // database genuinely cannot be opened from retrying the whole
        // migration ladder once per lifecycle event for the life of the door.
        let mut log: Option<EventLog> = None;
        let mut attempted = false;
        let sink = EventLogSink::with_writer(
            glasshouse::events::log::DEFAULT_SINK_QUEUE,
            move |recorded, observed| {
                if !std::mem::replace(&mut attempted, true) {
                    match EventLog::open(&runtime) {
                        Ok(opened) => log = Some(opened),
                        Err(err) => tracing::warn!(
                            error = %format!("{err:#}"),
                            "could not open the project event log; the sessions this \
                             door owns will not be recorded"
                        ),
                    }
                }
                let Some(log) = log.as_ref() else {
                    return;
                };
                if let Err(err) = log.append(&recorded, observed.as_ref()) {
                    tracing::warn!(error = %err, "could not append to the project event log");
                }
            },
        );
        events.attach_sink(Arc::clone(&sink) as Arc<dyn EventSink>);
        Self { sink }
    }

    /// Wait, at most [`RECORDER_FLUSH`], for what has been published so far
    /// to reach the database.
    ///
    /// Never fallible from the caller's side: a writer that did not finish in
    /// time is a reason the next answer is more complete, not a reason to
    /// refuse this one.
    fn flush(&self) {
        if !self.sink.flush(RECORDER_FLUSH) {
            tracing::debug!(
                dropped = self.sink.dropped(),
                "the event log had not caught up when the door was asked for history"
            );
        }
    }
}

/// Read one request line, dispatch it, and write back one response line.
#[allow(clippy::too_many_arguments)]
fn handle_connection(
    stream: UnixStream,
    runtime: &Runtime,
    sessions: &ProjectSessions,
    live: &Mutex<SessionRuntime>,
    watches: &Watches,
    recorder: &EventRecorder,
    injected: &Injected,
) -> anyhow::Result<()> {
    let mut writer = stream.try_clone()?;
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    reader.read_line(&mut line)?;

    let response = match serde_json::from_str::<Request>(line.trim_end()) {
        Ok(request) => dispatch(
            request, runtime, sessions, live, watches, recorder, injected,
        ),
        Err(err) => Response::err(format!("malformed request: {err}")),
    };

    let mut payload = serde_json::to_string(&response)?;
    payload.push('\n');
    writer.write_all(payload.as_bytes())?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn dispatch(
    request: Request,
    runtime: &Runtime,
    sessions: &ProjectSessions,
    live: &Mutex<SessionRuntime>,
    watches: &Watches,
    recorder: &EventRecorder,
    injected: &Injected,
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
        } => spawn_session(
            runtime,
            &store,
            live,
            &harness,
            args,
            role.as_deref(),
            task,
            injected,
        ),
        // Line 1125's *"routed task"* is not only a spawn's own. This verb
        // is the follow-up half of the same seam — `SessionApi::send_text`,
        // as `MessageOrigin::Machine`, never as the user (see that type's
        // doc comment) — so the context-selection step belongs on both or on
        // neither. It is also the only path on which line 1135 is a real
        // question: a session is spawned once, and can be given a task many
        // times.
        Request::SendMessage { session, text } => {
            let id = SessionId::new(session);
            // Selected before the runtime lock is taken: opening the
            // project's memory goes through `database::open`, which can wait
            // on a busy timeout, and nothing that waits may hold the lock
            // every live session's I/O runs through.
            let briefing = select_memory(runtime, &id, &text, injected);
            let mut guard = lock(live);
            let mut api = SessionApi::new(&store, &mut guard);
            deliver_memory(&mut api, &id, briefing, injected);
            match api.send_text(&id, &text) {
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
        Request::RoutingModel => routing_model_status(runtime),
        Request::Events { after, limit } => project_events(runtime, after, limit, recorder),
        Request::WatchWorker { session, notify } => {
            watch_worker(runtime, &store, live, watches, &session, &notify)
        }
        Request::GetCheckpoint {
            checkpoint,
            document,
        } => get_checkpoint(runtime, checkpoint.as_deref(), document),
        Request::QueryMemory {
            query,
            history,
            limit,
        } => query_memory(runtime, &query, history, limit),
        Request::GetMemory { memory } => get_memory(runtime, &memory),
        Request::CurrentMemory { limit, body_chars } => current_memory(runtime, limit, body_chars),
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
#[allow(clippy::too_many_arguments)]
fn spawn_session(
    runtime: &Runtime,
    store: &SessionStore<'_>,
    live: &Mutex<SessionRuntime>,
    harness: &str,
    args: Vec<String>,
    role: Option<&str>,
    task: Option<String>,
    injected: &Injected,
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
        deliver_memory(&mut api, &record.id, briefing, injected);
        if let Err(err) = api.send_text(&record.id, &task) {
            return Response::err(format!(
                "session `{}` was spawned but its task could not be delivered: {err}",
                record.id
            ));
        }
    }

    Response::ok(serde_json::json!({ "session": record.id.as_str() }))
}

/// Take the injection ledger's lock, ignoring poisoning, for the reason
/// [`lock`] gives: a panicking handler must not permanently disable a
/// bookkeeping record that only ever makes deliveries quieter.
fn lock_injected(
    injected: &Injected,
) -> std::sync::MutexGuard<'_, HashMap<String, HashSet<MemoryId>>> {
    injected
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Choose the project memory a session about to be given `task` should have —
/// capability map lines 1125-1127 and 1131-1134.
///
/// The whole selection lives in [`glasshouse::memory::inject::briefing`],
/// which is cross-platform and knows nothing about this door; what belongs
/// here is only the two things this door owns: which project's memory is
/// being read (the runtime this socket was opened for — there is no request
/// field naming a project, see `super`'s module doc comment), and what this
/// session has already been sent.
///
/// # Never a reason to fail a delivery
///
/// Every failure path returns `None` and logs. A session that starts and
/// receives its task without memory is strictly better than one that does
/// not start, and a memory store that cannot be opened is not a reason to
/// refuse to talk to a worker. The error is logged rather than answered
/// with, and it never reaches the injected text: `database::DatabaseError`
/// names the project file's absolute path in every variant, and nothing this
/// module puts on a session's terminal is built from an error at all.
fn select_memory(
    runtime: &Runtime,
    session: &SessionId,
    task: &str,
    injected: &Injected,
) -> Option<Injection> {
    let already = lock_injected(injected)
        .get(session.as_str())
        .cloned()
        .unwrap_or_default();

    let project = match ProjectMemory::open(runtime) {
        Ok(project) => project,
        Err(err) => {
            tracing::warn!(
                session = %session,
                error = %format!("{err:#}"),
                "could not open this project's memory to select context for a routed task"
            );
            return None;
        }
    };
    match inject::briefing(&project.store(), task, &already) {
        Ok(briefing) => briefing,
        Err(err) => {
            tracing::warn!(
                session = %session,
                error = %err,
                "could not select project memory for a routed task"
            );
            None
        }
    }
}

/// Deliver a selected briefing to `session`, and record what it carried.
///
/// # Line 1128: a message, not a write into the harness's own history
///
/// This goes through [`SessionApi::send_text`] — the same seam
/// `Request::SendMessage` uses, as [`MessageOrigin::Machine`] — and touches
/// no harness session file, transcript or resume state. Glasshouse's memory
/// arrives the way anything else Glasshouse says arrives, which is what
/// keeps it distinguishable from the harness's own record of the
/// conversation.
///
/// # Injection failure is never a delivery failure
///
/// A refused or failed injection is logged and swallowed. The ledger is
/// updated only on a send that actually succeeded, so a memory that did not
/// arrive is not recorded as one the session already has.
fn deliver_memory(
    api: &mut SessionApi<'_>,
    session: &SessionId,
    briefing: Option<Injection>,
    injected: &Injected,
) {
    let Some(briefing) = briefing else { return };
    if let Err(err) = api.send_text(session, briefing.text()) {
        tracing::warn!(
            session = %session,
            error = %api_error(err),
            "could not deliver this project's memory to a session; its task is being sent without it"
        );
        return;
    }

    let mut ledger = lock_injected(injected);
    let seen = ledger.entry(session.as_str().to_owned()).or_default();
    if seen.len() < MAX_REMEMBERED_INJECTIONS {
        seen.extend(briefing.memories().iter().cloned());
    }
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

/// Current routing-model selection and its health — capability map line 1680.
///
/// `selection` is the recorded [`config::RoutingModelChoice`] together with
/// the layer it came from, reported the way every other layered value in
/// this project is reported (see [`describe_layer`]). `resolution` is what
/// will actually classify a request right now:
/// `EffectiveConfig::routing_model_resolution` already checks a pinned
/// choice against the providers configured this instant and degrades to
/// heuristics with a named [`config::RoutingFallback`] when one has gone
/// missing — this handler reports that computed state, keyed by the type's
/// own variant names, rather than re-deriving or re-wording it into prose of
/// its own. There is no live latency or health probe anywhere in this
/// project (see that function's own doc comment); a project that has
/// configured nothing gets [`config::RoutingFallback::NotConfigured`], the
/// honest default, never a fabricated pin.
fn routing_model_status(runtime: &Runtime) -> Response {
    let user = match UserConfig::load(runtime.paths()) {
        Ok(user) => user,
        Err(err) => return Response::err(err),
    };
    let project_config = match config::load_project_config(runtime.project()) {
        Ok(project_config) => project_config,
        Err(err) => return Response::err(err),
    };
    let effective = EffectiveConfig::new(&user, project_config.as_ref());

    let selection = effective.routing_model();
    let resolution = effective.routing_model_resolution();

    Response::ok(serde_json::json!({
        "selection": routing_choice_json(&selection.value),
        "layer": describe_layer(resolution.layer),
        "resolution": routing_resolution_json(&resolution.value),
    }))
}

/// A recorded [`config::RoutingModelChoice`] as JSON. `provider`/`model` are
/// `null` for every choice but [`config::RoutingModelChoice::Pinned`] —
/// never an empty string, so an absent value cannot be mistaken for one that
/// was measured and happened to be empty (§71).
fn routing_choice_json(choice: &config::RoutingModelChoice) -> serde_json::Value {
    match choice {
        config::RoutingModelChoice::Deterministic => serde_json::json!({
            "choice": "deterministic",
            "provider": null,
            "model": null,
        }),
        config::RoutingModelChoice::Automatic => serde_json::json!({
            "choice": "automatic",
            "provider": null,
            "model": null,
        }),
        config::RoutingModelChoice::Pinned { provider, model } => serde_json::json!({
            "choice": "pinned",
            "provider": provider,
            "model": model,
        }),
    }
}

/// A computed [`config::RoutingModelResolution`] as JSON — what will
/// actually classify a request right now, distinct from the recorded
/// [`routing_choice_json`].
fn routing_resolution_json(resolution: &config::RoutingModelResolution) -> serde_json::Value {
    match resolution {
        config::RoutingModelResolution::Automatic => serde_json::json!({ "state": "automatic" }),
        config::RoutingModelResolution::Pinned { provider, model } => serde_json::json!({
            "state": "pinned",
            "provider": provider,
            "model": model,
        }),
        config::RoutingModelResolution::Heuristics(reason) => routing_fallback_json(reason),
    }
}

/// Why deterministic heuristics are answering instead of a model, keyed by
/// [`config::RoutingFallback`]'s own variant names rather than its
/// [`std::fmt::Display`] prose — a client matching on `reason` must be able
/// to tell the cases apart mechanically, not by parsing a sentence meant for
/// a person.
fn routing_fallback_json(reason: &config::RoutingFallback) -> serde_json::Value {
    match reason {
        config::RoutingFallback::NotConfigured => serde_json::json!({
            "state": "heuristics",
            "reason": "not_configured",
        }),
        config::RoutingFallback::DeterministicChosen => serde_json::json!({
            "state": "heuristics",
            "reason": "deterministic_chosen",
        }),
        config::RoutingFallback::ProviderNotConfigured { provider, model } => serde_json::json!({
            "state": "heuristics",
            "reason": "provider_not_configured",
            "provider": provider,
            "model": model,
        }),
    }
}

/// Matches `provider::resources::describe_layer`'s own wire spelling for
/// [`config::Layer`] (`"project"` / `"user"` / `"default"`), duplicated
/// rather than imported because that one is private to its own module.
fn describe_layer(layer: config::Layer) -> &'static str {
    match layer {
        config::Layer::Project => "project",
        config::Layer::User => "user",
        config::Layer::Default => "default",
    }
}

/// This project's lifecycle events, harness-independent — capability map
/// line 701.
///
/// Incremental: `after` is the log position the caller has already consumed,
/// and `head` — the log's current position, returned even when `events` is
/// empty — is what it hands back next time, so a caller that sees nothing
/// new still has a cursor rather than only after the first event ever
/// exists. `limit` is capped at [`MAX_EVENTS_LIMIT`] regardless of what is
/// asked for.
///
/// # Why this reads [`EventLog::since`] and not `observed_since`
///
/// Because the caller is **in another process**. `observed_since` filters to
/// harness-reported rows for one stated reason — a reader subscribed to this
/// process's own [`EventBus`] would otherwise see every in-process event
/// twice — and that reason is a fact about `shell::run`, which holds both a
/// subscription and a log tail. Nothing on the far end of this socket holds
/// either. Applying the filter here does not de-duplicate anything; it
/// deletes the entire class of events this process is the only witness to,
/// which is every spawn, intervention and exit of every orchestrated worker
/// — see [`EventRecorder`] for the other half of the same defect.
///
/// The narrower query is still right where its premise holds, and it is
/// still used there: [`pump_watches`] wants exactly the harness reports,
/// because `TurnEnded` is minted only in a hook process and a completion
/// carries the reporting harness's name.
///
/// # Why it flushes first
///
/// Recording is asynchronous by construction (see [`EventRecorder`]), so an
/// orchestrator that sends a message and immediately asks what happened
/// would otherwise race its own write. The wait is bounded and its failure
/// is ignored: a slow writer makes this answer *older*, never absent, and
/// the caller's cursor brings it back next call.
///
/// This makes the door's **own** writes visible before it answers. It cannot
/// do the same for a harness report, which is written by a separate
/// `glasshouse hook` process on its own schedule — no reader anywhere can
/// know that one is pending.
fn project_events(
    runtime: &Runtime,
    after: i64,
    limit: usize,
    recorder: &EventRecorder,
) -> Response {
    recorder.flush();

    let log = match EventLog::open(runtime) {
        Ok(log) => log,
        Err(err) => return Response::err(err.to_string()),
    };

    let bounded_limit = limit.min(MAX_EVENTS_LIMIT);
    let events = match log.since(after, bounded_limit) {
        Ok(events) => events,
        Err(err) => return Response::err(err.to_string()),
    };
    let head = match log.head() {
        Ok(head) => head,
        Err(err) => return Response::err(err.to_string()),
    };

    Response::ok(serde_json::json!({
        "events": events.iter().map(event_json).collect::<Vec<_>>(),
        "head": head,
    }))
}

/// One logged event, translated for the door.
///
/// `kind` is [`LifecycleEvent::kind`]'s own word, never the harness's —
/// that is what capability map line 701 asks for. The harness that reported
/// it, when one did, appears only as the `harness` attribute; the harness's
/// own raw spelling of the event ([`glasshouse::events::Observation::event`])
/// and every hook payload field stay behind this door, matching the
/// guarantee `tests/session_hook.rs` already holds for the project database
/// itself — this handler exposes translated events, not raw adapter
/// observations. Every payload field a kind does not carry is `null`, never
/// `0` or `""` (§71).
fn event_json(logged: &LoggedEvent) -> serde_json::Value {
    let mut fields = serde_json::Map::new();
    fields.insert("seq".to_owned(), serde_json::json!(logged.seq));
    fields.insert(
        "session".to_owned(),
        serde_json::json!(logged.session.as_str()),
    );
    fields.insert("at".to_owned(), serde_json::json!(logged.at));
    fields.insert("kind".to_owned(), serde_json::json!(logged.event.kind()));
    fields.insert(
        "harness".to_owned(),
        serde_json::json!(logged.observed.as_ref().map(|o| o.harness.as_str())),
    );
    for key in [
        "outcome",
        "origin",
        "bytes",
        "exit_code",
        "exit_signal",
        "resource",
        "reason",
        "provider",
        "model",
        "cause",
    ] {
        fields.insert(key.to_owned(), serde_json::Value::Null);
    }

    match &logged.event {
        LifecycleEvent::TurnEnded { outcome } => {
            fields.insert(
                "outcome".to_owned(),
                serde_json::json!(turn_outcome_str(*outcome)),
            );
        }
        LifecycleEvent::TextDelivered { origin, bytes } => {
            fields.insert(
                "origin".to_owned(),
                serde_json::json!(message_origin_str(*origin)),
            );
            fields.insert("bytes".to_owned(), serde_json::json!(bytes));
        }
        LifecycleEvent::InterruptDelivered { origin } => {
            fields.insert(
                "origin".to_owned(),
                serde_json::json!(message_origin_str(*origin)),
            );
        }
        LifecycleEvent::ProcessExited { exit } => {
            fields.insert("exit_code".to_owned(), serde_json::json!(exit.code()));
            fields.insert("exit_signal".to_owned(), serde_json::json!(exit.signal()));
        }
        LifecycleEvent::GatewayUnhealthy { resource, reason } => {
            fields.insert("resource".to_owned(), serde_json::json!(resource));
            fields.insert(
                "reason".to_owned(),
                serde_json::json!(gateway_failure_str(*reason)),
            );
        }
        LifecycleEvent::GatewayBackendChanged {
            provider,
            model,
            cause,
        } => {
            fields.insert("provider".to_owned(), serde_json::json!(provider));
            fields.insert("model".to_owned(), serde_json::json!(model));
            fields.insert("cause".to_owned(), serde_json::json!(cause));
        }
        LifecycleEvent::SessionStarted
        | LifecycleEvent::SessionResumed
        | LifecycleEvent::TurnStarted
        | LifecycleEvent::WaitingForUser
        | LifecycleEvent::OutputEnded => {}
    }

    serde_json::Value::Object(fields)
}

/// A memory-side failure this door may put on the wire, as a message.
///
/// `MemoryStoreError` is written for exactly this audience — it names the
/// memory, both project identifiers, and the operation that failed, and
/// nothing else. Everything else that can surface from opening the project's
/// memory is a `database::DatabaseError`, and **every one of its variants
/// names the database file's absolute path** (`crates/glasshouse/src/
/// database.rs`'s error enum). That path lies outside the project root and
/// outside what this door is scoped to, so it does not leave here — a caller
/// on the far end of a socket cannot repair the file anyway, and the class of
/// failure is the whole of what it can act on.
///
/// Down-cast rather than matched on a string, and `pub(crate)`-invisible
/// `DatabaseError` is deliberately not named here: this stays correct if a
/// new error type joins the anyhow chain, because anything that is not a
/// `MemoryStoreError` is treated as the unsafe case.
fn memory_error_message(err: &anyhow::Error) -> String {
    match err.downcast_ref::<glasshouse::memory::MemoryStoreError>() {
        Some(store_error) => store_error.to_string(),
        None => "the project's memory database could not be opened".to_owned(),
    }
}

/// One selected memory in full — capability map line 1112's `memory.get`.
///
/// # The read boundary is the whole point (line 1114)
///
/// `memory` is an identifier or an unambiguous prefix of one, and it is the
/// only caller-supplied value on this path. It cannot name a project: there
/// is no project field on this door (see `super`'s module doc comment), the
/// store is opened from the [`Runtime`] this process was started for, and
/// `MemoryStore::get` compares the row's own `project_id` against the active
/// one before handing anything back.
///
/// A row belonging to another project comes back as
/// `MemoryStoreError::ForeignProject` — **an error, never an empty answer**.
/// That distinction is the requirement, not an implementation detail: an
/// agent reading `null` would conclude the memory does not exist, when what
/// actually happened is that this project's file holds a row it must not
/// read. `MemoryStore::resolve_id` is deliberately left unscoped for the same
/// reason — scoping the prefix lookup by project would turn a foreign row
/// back into a silent "not found", which is precisely the answer the store's
/// own doc comment refuses to give.
fn get_memory(runtime: &Runtime, memory: &str) -> Response {
    use glasshouse::memory::{MemoryStoreError, ProjectMemory};

    let project = match ProjectMemory::open(runtime) {
        Ok(project) => project,
        Err(err) => return Response::err(memory_error_message(&err)),
    };
    let store = project.store();

    let id = match store.resolve_id(memory) {
        Ok(id) => id,
        Err(err) => return Response::err(err),
    };
    match store.get(&id) {
        Ok(Some(record)) => Response::ok(memory_full_json(&record)),
        // `resolve_id` found the row and `get` did not, which means it was
        // deleted between the two statements. `NotFound` rather than a null
        // result, for [`get_memory`]'s own reason.
        Ok(None) => Response::err(MemoryStoreError::NotFound { id }),
        Err(err) => Response::err(err),
    }
}

/// A concise snapshot of what this project currently knows — capability map
/// line 1113's `memory.current`.
///
/// Answered from `memory::snapshot::snapshot`, the same producer the TUI's
/// project overview reads (`shell::build_project_overview_memory`), so the
/// two cannot disagree about what "current" means. There is no second
/// snapshot implementation behind this door and there must not be one.
///
/// # Bounded on both axes, server-side (line 1115)
///
/// A caller's `limit` and `body_chars` are each `min`'d against a constant
/// here before they reach [`glasshouse::memory::snapshot::SnapshotBudget`],
/// so they may only ever *lower*
/// the ceiling. Passing `usize::MAX` to both — the executable form of
/// "dumping the complete memory database into agent context" — yields the
/// same bounded response as passing the ceiling itself.
///
/// # Sections, not a flattened dump
///
/// The response keeps `snapshot`'s own structure: one entry per
/// `MemoryKind`, present even when empty, each reporting how many entries it
/// left out. A section that hit its cap says so, and a body that was cut says
/// so, so a caller can tell "this project has nothing of that kind" from
/// "there is more of it than you asked for" without a second call.
fn current_memory(runtime: &Runtime, limit: usize, body_chars: usize) -> Response {
    use glasshouse::memory::ProjectMemory;
    use glasshouse::memory::snapshot::{SnapshotBudget, snapshot};

    let project = match ProjectMemory::open(runtime) {
        Ok(project) => project,
        Err(err) => return Response::err(memory_error_message(&err)),
    };
    let store = project.store();

    let budget = SnapshotBudget::new(
        limit.min(MAX_SNAPSHOT_SECTION_LIMIT),
        body_chars.min(MAX_SNAPSHOT_BODY_CHARS),
    );
    match snapshot(&store, &budget) {
        Ok(snapshot) => Response::ok(serde_json::json!({
            "sections": snapshot
                .sections
                .iter()
                .map(snapshot_section_json)
                .collect::<Vec<_>>(),
            // The budget actually applied, not the one asked for, so a caller
            // that named a larger number learns what it got rather than
            // inferring it from a section that happens to be short.
            "budget": {
                "per_section_limit": budget.per_section_limit,
                "max_body_chars": budget.max_body_chars,
            },
        })),
        Err(err) => Response::err(err),
    }
}

/// One [`glasshouse::memory::snapshot::SnapshotSection`], as JSON.
fn snapshot_section_json(
    section: &glasshouse::memory::snapshot::SnapshotSection,
) -> serde_json::Value {
    use glasshouse::memory::MemoryKind;

    serde_json::json!({
        "kind": MemoryKind::as_str(section.kind),
        "entries": section.entries.iter().map(snapshot_entry_json).collect::<Vec<_>>(),
        "omitted": section.omitted,
    })
}

/// One snapshot entry, as JSON — line 1116's provenance, at the resolution a
/// snapshot has it.
///
/// A `SnapshotEntry` carries the two fields that *locate* a memory —
/// `source_session_id` and `source_commit` — and not the ten
/// `DecisionProvenance` fields that explain it, because the snapshot's job is
/// to be concise (line 1115). They are one [`Request::GetMemory`] away, which
/// is the division of labour those two lines describe between them: enough
/// provenance here to know where to look, all of it there.
fn snapshot_entry_json(entry: &glasshouse::memory::snapshot::SnapshotEntry) -> serde_json::Value {
    use glasshouse::memory::MemoryAuthority;

    serde_json::json!({
        "id": entry.id.as_str(),
        "subject": entry.subject,
        "body": entry.body,
        "body_truncated": entry.body_truncated,
        "authority": entry.authority.map(MemoryAuthority::as_str),
        "provenance": {
            "source_session_id": entry.source_session_id,
            "source_commit": entry.source_commit,
        },
    })
}

// ---------------------------------------------------------------------------
// The orchestrator wake-up flow — capability map Phase 15, lines 733-739.
// ---------------------------------------------------------------------------

/// How many log rows one pump reads. Bounded for the same reason
/// [`MAX_EVENTS_LIMIT`] is: a watch that has fallen a long way behind must
/// catch up over several ticks rather than pull the whole table into one
/// pass, and the cursor makes the next tick resume exactly where this one
/// stopped.
const WATCH_PUMP_LIMIT: usize = 256;

/// How many event kinds a completion summary names before it elides.
const SUMMARY_KINDS: usize = 6;

/// The one thing an orchestrator learns without asking — line 736's
/// "machine-originated message" — and therefore the one place this door
/// speaks first.
///
/// # This is a statement about the past, never about the present
///
/// Every field is read from one row of the durable event log and the row is
/// named by its own `seq`. That is deliberate and it is the whole design
/// ruling behind lines 740 and 748: an orchestrator woken by this knows that
/// *at log position `seq`* a harness reported a turn ending, and knows
/// nothing whatever about the session **now**. Anyone — a person, another
/// orchestrator, the harness itself — may have moved the session in the
/// meantime, and the only way to find out is to ask again
/// ([`Request::SessionState`], line 738).
///
/// A notification that carried a live state would be a lie with a timestamp
/// on it, and it would make the orchestrator's picture authoritative over
/// the user's, which is exactly backwards.
struct Completion {
    worker: SessionId,
    harness: Option<String>,
    outcome: TurnOutcome,
    seq: i64,
    at: i64,
    summary: String,
}

impl Completion {
    /// One line, and one line only.
    ///
    /// [`SessionApi::send_text`] appends the carriage return that submits
    /// this to the harness's line editor, so an embedded newline here would
    /// submit half a notification and leave the rest as the start of the
    /// next message. Every field below is either an integer, a session
    /// identifier, a harness slug, or [`Completion::summary`] — see
    /// [`summarize`] for why none of them can contain one.
    fn line(&self) -> String {
        let payload = serde_json::json!({
            "worker": self.worker.as_str(),
            "harness": self.harness,
            "outcome": turn_outcome_str(self.outcome),
            "seq": self.seq,
            "at": self.at,
            "summary": self.summary,
        });
        format!("glasshouse worker-completion {payload}")
    }
}

/// One registered interest — line 733.
///
/// `cursor` is the log position this watch has already consumed. It is the
/// entire dedup mechanism (line 739): a row is read exactly once, because
/// the cursor advances past **every** row the pump saw, matched or not, and
/// the log assigns `seq` monotonically from the database rather than from
/// any one process's counter.
struct Watch {
    worker: SessionId,
    notify: SessionId,
    cursor: i64,
}

/// Every registered interest this door is currently holding.
///
/// # Lock order: `watches` before `live`, never the reverse
///
/// Two paths take both — [`watch_worker`], which validates a registration
/// against the runtime, and [`pump_watches`], which delivers through it.
/// Taking them in opposite orders is a deadlock that would strand the whole
/// door, and it is a deadlock that compiles and passes every test that does
/// not happen to interleave a registration with a delivery. So the order is
/// fixed here and stated at both sites.
type Watches = Mutex<Vec<Watch>>;

/// Register interest in a worker's completions — line 733.
///
/// Both identifiers go through [`SessionApi`], so both are refused unless
/// they belong to this project: a watch is a standing instruction to type
/// into a session, and the one thing this door may never do is type into
/// another project's.
///
/// Three refusals, each because the alternative is worse than an error:
///
/// - **`notify` is not live in this process.** The runtime this door owns is
///   the only one it can write to (see this module's own doc comment), so an
///   orchestrator that was not spawned through this door has no terminal
///   here. Registering anyway would produce a watch that silently never
///   fires — the exact failure `scripts/worker-watch.sh` produced when a
///   finished worker was lost because nothing was really watching it.
/// - **`session` is the same as `notify`.** A session watching itself types
///   its own completions into itself, which is a loop with a keyboard.
/// - **an unknown or foreign session**, from `SessionApi` itself.
fn watch_worker(
    runtime: &Runtime,
    store: &SessionStore<'_>,
    live: &Mutex<SessionRuntime>,
    watches: &Watches,
    session: &str,
    notify: &str,
) -> Response {
    let worker = SessionId::new(session.to_owned());
    let notify = SessionId::new(notify.to_owned());

    if worker == notify {
        return Response::err(format!(
            "session `{worker}` cannot be watched on its own behalf: a completion \
             would be typed back into the session that produced it"
        ));
    }

    // Lock order: `watches` first, then `live`. See [`Watches`].
    let mut registry = watches
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);

    {
        let mut guard = lock(live);
        let api = SessionApi::new(store, &mut guard);
        // `state` rather than `list`: it resolves through the same
        // project-scope check every other method starts with, and answers
        // for the one session asked about.
        if let Err(err) = api.state(&worker) {
            return Response::err(api_error(err));
        }
        if let Err(err) = api.state(&notify) {
            return Response::err(api_error(err));
        }
        if guard.get(&notify).is_none() {
            return Response::err(format!(
                "session `{notify}` is not live in this Glasshouse, so a completion \
                 notification would have nowhere to be delivered; an orchestrator \
                 must be a session this door holds"
            ));
        }
    }

    let from = match EventLog::open(runtime) {
        Ok(log) => match log.head() {
            Ok(head) => head,
            Err(err) => return Response::err(err.to_string()),
        },
        Err(err) => return Response::err(err.to_string()),
    };

    // Idempotent per pair. A second registration replaces the first rather
    // than adding to it: two watches over one pair would deliver one
    // completion twice, which is precisely line 739's failure.
    if let Some(existing) = registry
        .iter_mut()
        .find(|watch| watch.worker == worker && watch.notify == notify)
    {
        existing.cursor = from;
    } else {
        registry.push(Watch {
            worker: worker.clone(),
            notify: notify.clone(),
            cursor: from,
        });
    }

    Response::ok(serde_json::json!({
        "worker": worker.as_str(),
        "notify": notify.as_str(),
        "from": from,
    }))
}

/// Whether any orchestrator has registered interest yet.
///
/// Peeked before anything is opened, so a door nobody is watching through
/// costs one uncontended mutex acquisition per tick and nothing else.
fn watching(watches: &Watches) -> bool {
    !watches
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .is_empty()
}

/// The two database handles the wake-up pump reads through, opened **once**.
///
/// # Why this is not opened per tick
///
/// Both of these go through `database::open`, which does considerably more
/// than hand back a connection: it takes a `BEGIN IMMEDIATE` **write**
/// transaction and runs the migration ladder before returning. That is
/// exactly right once per process and badly wrong twenty times a second.
///
/// A pump that reopened on every 50ms tick would take SQLite's write lock
/// forty times a second for the whole life of the door — contending with the
/// very `glasshouse hook` processes that write the rows it is reading. Those
/// run *inside the user's own session*, and `report_hook`'s own doc comment
/// explains why a hook must never be made slow: Claude Code treats a hook's
/// failure as a veto, and the busy wait it would be pushed into is five
/// seconds. The door's bookkeeping is never more important than the session
/// it is keeping books about.
///
/// Opened lazily, on the first tick where a watch exists, so a door nobody
/// watches through holds no extra connection at all.
struct WatchState {
    log: EventLog,
    sessions: ProjectSessions,
}

impl WatchState {
    fn open(runtime: &Runtime) -> Option<Self> {
        let log = EventLog::open(runtime).ok()?;
        let sessions = ProjectSessions::open(runtime).ok()?;
        Some(Self { log, sessions })
    }
}

/// Deliver any completion each watch has not yet seen — lines 734-737, 739.
///
/// Called from the door's own background tick, which is what makes this a
/// production installation rather than a mechanism waiting for one: nothing
/// outside `glasshouse api serve` has to remember to call it.
///
/// # Why this reads the log rather than the bus
///
/// A turn ending is reported by the harness's own lifecycle hook, in a
/// **separate short-lived process** (`glasshouse hook <session> Stop`), which
/// translates it through `session::lifecycle::event_for` — the single
/// construction site of `TurnEnded` — and appends it to the project's event
/// log. That row is the only place this process can see it: the hook's
/// process is gone by the time anyone could have subscribed to anything.
///
/// So this is line 734's *"from native lifecycle hooks"* in the literal
/// sense. It is not a screen-scraper and it cannot become one: nothing here
/// reads a session's output, and `TurnEnded` cannot be minted from silence.
///
/// # Why the cursor advances past rows that did not match
///
/// `observed_since` returns every observed row, not only this worker's. A
/// cursor that advanced only on a match would re-read the same unmatched
/// rows on every tick forever, and would eventually re-read a matched row
/// too once the batch limit cut it off. Advancing past everything seen is
/// what makes "read exactly once" a property of the loop rather than of the
/// filter.
fn pump_watches(state: &WatchState, live: &Mutex<SessionRuntime>, watches: &Watches) {
    // Lock order: `watches` first, then `live`. See [`Watches`].
    let mut registry = watches
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if registry.is_empty() {
        return;
    }

    let log = &state.log;
    let store = state.sessions.store();

    let mut dropped = Vec::new();
    for (index, watch) in registry.iter_mut().enumerate() {
        let rows = match log.observed_since(watch.cursor, WATCH_PUMP_LIMIT) {
            Ok(rows) => rows,
            Err(err) => {
                tracing::warn!(error = %err, "could not read the event log for a worker watch");
                continue;
            }
        };
        if rows.is_empty() {
            continue;
        }

        let mut completions = Vec::new();
        for row in &rows {
            watch.cursor = watch.cursor.max(row.seq);
            let LifecycleEvent::TurnEnded { outcome } = row.event else {
                continue;
            };
            if row.session != watch.worker {
                continue;
            }
            completions.push(Completion {
                worker: row.session.clone(),
                harness: row.observed.as_ref().map(|o| o.harness.clone()),
                outcome,
                seq: row.seq,
                at: row.at,
                summary: summarize(log, &row.session, row.seq, row.at),
            });
        }
        if completions.is_empty() {
            continue;
        }

        let mut guard = lock(live);
        let mut api = SessionApi::new(&store, &mut guard);
        for completion in completions {
            // `SessionApi::send_text`, so the delivery is recorded as
            // `MessageOrigin::Machine` — line 736's "machine-originated
            // message" — through the same seam an orchestrator's own
            // `send_message` uses. There is no second write path into a
            // session, deliberately.
            match api.send_text(&watch.notify, &completion.line()) {
                Ok(()) => tracing::info!(
                    worker = %completion.worker,
                    notify = %watch.notify,
                    seq = completion.seq,
                    "delivered a worker completion to an orchestrator"
                ),
                // The orchestrator's own session ended. A watch that can
                // never be delivered again is dropped rather than retried
                // every tick for the life of the process.
                Err(ApiError::NotLive { .. }) | Err(ApiError::NotFound { .. }) => {
                    tracing::warn!(
                        notify = %watch.notify,
                        "dropping a worker watch: the session to notify is gone"
                    );
                    dropped.push(index);
                    break;
                }
                Err(err) => tracing::warn!(
                    notify = %watch.notify,
                    error = %err,
                    "could not deliver a worker completion"
                ),
            }
        }
    }

    for index in dropped.into_iter().rev() {
        registry.remove(index);
    }
}

/// What Glasshouse actually observed about the turn that just ended — line
/// 737's "concise result summary", closed at exactly the width the evidence
/// supports and no wider.
///
/// # Every character of this comes from a fixed vocabulary
///
/// The rendered kinds are [`LifecycleEvent::kind`]'s own words — a
/// `&'static str` from the eleven the enum defines — joined with an arrow,
/// plus one integer. **No value read out of a hook payload, a session's
/// scrollback, or a harness's own event spelling can reach this string**,
/// because none of those is in the type it is built from. That is not care
/// on the author's part; it is what `LoggedEvent` makes available.
///
/// This matters more than concision. A summary quoting a worker's output
/// would breach the same boundary `tests/session_hook.rs` holds for the
/// project database — the hook path deliberately drains its payload into
/// `io::sink()` unread — and it would do it on the one path whose whole
/// purpose is to carry information *out* of a worker and into another agent.
///
/// # What it can honestly say, and what it cannot
///
/// It says: the shape of the turn, in Glasshouse's own vocabulary, and how
/// long it took. `turn_started → waiting_for_user → turn_ended in 41s` tells
/// an orchestrator that the worker stopped to ask something and then
/// finished, which is real and actionable.
///
/// It does **not** say what the worker did, produced, or concluded.
/// Glasshouse does not observe that anywhere — the only place it exists is
/// the conversation, which this door does not read. An orchestrator that
/// needs the result asks the worker (line 738) or reads a checkpoint.
fn summarize(log: &EventLog, session: &SessionId, seq: i64, at: i64) -> String {
    let history = match log.recent_for_session(session, SUMMARY_KINDS * 4) {
        Ok(history) => history,
        Err(err) => {
            tracing::warn!(session = %session, error = %err, "could not summarize a turn");
            return "no observed history for this turn".to_owned();
        }
    };

    // The turn is what happened after the previous `turn_ended` — the
    // harness's own boundary, not a guess at one.
    let start = history
        .iter()
        .rposition(|row| row.seq < seq && matches!(row.event, LifecycleEvent::TurnEnded { .. }))
        .map(|index| index + 1)
        .unwrap_or(0);
    let turn: Vec<&LoggedEvent> = history[start..]
        .iter()
        .filter(|row| row.seq <= seq)
        .collect();

    let elapsed = turn.first().map(|first| at - first.at).unwrap_or(0).max(0);

    let mut kinds: Vec<&'static str> = turn.iter().map(|row| row.event.kind()).collect();
    let elided = kinds.len().saturating_sub(SUMMARY_KINDS);
    if elided > 0 {
        // Keep the end of the turn, which is the part that says how it went.
        kinds.drain(..elided);
    }
    let shape = kinds.join(" → ");
    let shape = if elided > 0 {
        format!("… ({elided} earlier) → {shape}")
    } else {
        shape
    };

    format!("{shape} in {elapsed}s")
}

/// Matches `events::log`'s own private `outcome_sql` spelling, duplicated
/// rather than imported because that one is private to its own module (same
/// reasoning as [`describe_layer`]).
fn turn_outcome_str(outcome: TurnOutcome) -> &'static str {
    match outcome {
        TurnOutcome::Completed => "completed",
        TurnOutcome::Failed => "failed",
    }
}

/// Matches `events::log`'s own private `origin_sql` spelling.
fn message_origin_str(origin: MessageOrigin) -> &'static str {
    match origin {
        MessageOrigin::UserKeystroke => "user_keystroke",
        MessageOrigin::Machine => "machine",
    }
}

/// Matches `events::log`'s own private `gateway_reason_sql` spelling.
fn gateway_failure_str(reason: GatewayFailure) -> &'static str {
    match reason {
        GatewayFailure::Unreachable => "unreachable",
        GatewayFailure::TimedOut => "timed_out",
        GatewayFailure::Rejected => "rejected",
    }
}

/// Search this project's durable memory — box 10, capability map line 1111's
/// project-scoped `memory.search`, and Phase 21F lines 935/936: the machine
/// door carries each result's authority, validity state, and — for a memory
/// that may constrain implementation — its rationale and invalidation
/// conditions, as structured fields rather than only inside a rendered
/// string.
///
/// # Project scope, and why there is no project argument
///
/// Line 1114. The scope is structural: this door is opened for one resolved
/// [`Runtime`] and no request field names a project (see `super`'s module doc
/// comment), and `MemoryStore::search` filters on `memories.project_id` in
/// its own `WHERE` clause underneath that rather than trusting it. The two
/// are independent, which is the point — see `memory::store`'s own
/// "Project isolation" section for why the read boundary is not redundant
/// with the trigger.
///
/// `invariants_and_constraints`/`other` is `main.rs`'s own
/// `memory_search_grouped` (line 929), the exact search
/// `glasshouse memory search` runs; `report` is `render_memory_report`'s
/// exact text over the same result, so this door and that command can never
/// disagree about what a query finds. One search, not two: the CLI's report
/// text is rendered from the already-fetched grouping rather than searched
/// for a second time.
fn query_memory(runtime: &Runtime, query: &str, history: bool, limit: usize) -> Response {
    // Line 1115. The cap is applied here rather than left to the caller's
    // `limit`, and it is a `min` rather than a rejection: a caller asking for
    // more than the door will give gets the door's answer, not an error, the
    // same shape [`project_events`] uses for `MAX_EVENTS_LIMIT`.
    let limit = limit.min(MAX_MEMORY_LIMIT);
    let grouped = match crate::memory_search_grouped(runtime, query, history, limit) {
        Ok(grouped) => grouped,
        // Through [`memory_error_message`], not `Response::err(err)` directly:
        // this anyhow chain carries a `database::DatabaseError` when the
        // project's database cannot be opened, and every variant of that type
        // names the file's absolute path. See that function.
        Err(err) => return Response::err(memory_error_message(&err)),
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
        "provenance": provenance_json(record),
        "review": review,
        "last_validated_at": record.last_validated_at,
        "created_at": record.created_at,
    })
}

/// Everything that lets a caller trace one memory back to where it came from
/// — capability map line 1116, *"include provenance with machine-retrieved
/// memory so an agent can verify important claims against source or code."*
///
/// Deliberately the vocabulary `tests/memory_provenance.rs` already proves
/// round-trips, field for field and spelling for spelling — the two
/// *locating* fields `source_session_id` and `source_commit`, the event
/// slice, and all ten of Phase 21B's `DecisionProvenance` fields — rather
/// than a second provenance shape invented for this door. An agent that
/// wants to check a claim against code has `source_commit`; against the
/// conversation that produced it, `source_session_id` and `source_events`;
/// against the reasoning, `rationale`, `evidence` and `source_excerpt`.
///
/// Every field is `null` when absent and never `""` or `0` (§71): a decision
/// nobody recorded a security assumption for is a different fact from one
/// that recorded there was none, and `MemoryRecord`'s own doc comments say
/// so field by field.
///
/// `rationale` also appears at the top level of [`memory_result_json`], where
/// Phase 21F line 936 put it; it is repeated rather than moved so that this
/// change adds a field to the door's answer and removes none.
///
/// # Secrets
///
/// Nothing here is a credential by construction. `memory::store`'s module
/// documentation states there is no column for a token, a key, or a provider
/// secret; the screening is on the producer side, where
/// `memory::extract::schema::judge` inspects each emitted element **whole**
/// before any field is read. `source_excerpt` is the sharpest of these
/// because it is verbatim session text — and it is exactly as screened as
/// `body`, which this door has carried since Phase 21F. This is a `json!`
/// over named fields, never a `Debug` format of a struct, so the
/// `provider/discovery.rs::ProbeRequest` shape cannot reappear here.
fn provenance_json(record: &glasshouse::memory::MemoryRecord) -> serde_json::Value {
    use glasshouse::memory::ProjectPhase;

    let provenance = &record.provenance;
    serde_json::json!({
        "source_session_id": record.source_session_id,
        "source_commit": record.source_commit,
        "source_events": record.source_events.map(|events| {
            serde_json::json!({ "first": events.first, "last": events.last })
        }),
        "project_phase": provenance.project_phase.map(ProjectPhase::as_str),
        "problem": provenance.problem,
        "rationale": provenance.rationale,
        "assumptions": provenance.assumptions,
        "scale_assumptions": provenance.scale_assumptions,
        "security_assumptions": provenance.security_assumptions,
        "compatibility_assumptions": provenance.compatibility_assumptions,
        "operational_assumptions": provenance.operational_assumptions,
        "evidence": provenance.evidence,
        "source_excerpt": provenance.source_excerpt,
    })
}

/// One memory with nothing elided — [`Request::GetMemory`]'s answer, and
/// capability map line 1112's *"in full"*.
///
/// [`memory_result_json`] plus what only a single-memory lookup has room to
/// say: the supersession relationship and the reason for it (line 925), when
/// the row was last written, whether it is still open work, and — stated
/// rather than left to be assumed — that this body was **not** cut, which is
/// the one thing distinguishing it from the same memory seen through
/// [`Request::CurrentMemory`].
///
/// `validity_conditions` and `invalidation_conditions` stay gated on
/// `may_constrain_implementation`, exactly as in the search answer: "in full"
/// means nothing is elided, not that a non-binding memory starts being
/// presented as one that constrains implementation. That gate is Phase 21F
/// line 936's, and one verb relaxing it would make the door disagree with
/// itself about the same row.
fn memory_full_json(record: &glasshouse::memory::MemoryRecord) -> serde_json::Value {
    let serde_json::Value::Object(mut fields) = memory_result_json(record) else {
        // Unreachable: `memory_result_json` is a `json!({...})` object
        // literal. Answering with the search shape rather than unwrapping
        // keeps an impossible case off this door's panic path all the same —
        // a panic here would take the whole server down, not one request.
        return memory_result_json(record);
    };

    fields.insert(
        "superseded_by".to_owned(),
        serde_json::json!(record.superseded_by.as_ref().map(|id| id.as_str())),
    );
    fields.insert(
        "superseded_reason".to_owned(),
        serde_json::json!(record.superseded_reason),
    );
    fields.insert(
        "open_todo".to_owned(),
        serde_json::json!(record.is_open_todo()),
    );
    fields.insert("body_truncated".to_owned(), serde_json::json!(false));
    fields.insert(
        "updated_at".to_owned(),
        serde_json::json!(record.updated_at),
    );

    serde_json::Value::Object(fields)
}

/// The project's current binding memory, rendered for a checkpoint's
/// `Handoff::memory` — line 1641.
///
/// Identical to `main.rs`'s function of the same name, and duplicated for the
/// reason [`request_checkpoint`] itself is: opening the project's memory
/// database or reading its binding records must never fail a checkpoint, so
/// either failure degrades to an empty list rather than propagating.
fn binding_memory_lines(runtime: &Runtime) -> Vec<String> {
    use glasshouse::memory::ProjectMemory;

    let Ok(memory) = ProjectMemory::open(runtime) else {
        return Vec::new();
    };
    let Ok(records) = memory.store().binding(20) else {
        return Vec::new();
    };
    records
        .into_iter()
        .map(|record| match record.subject {
            // Phase 20 allows an absent subject; rendering an empty one would
            // print a heading nobody wrote.
            Some(subject) => format!("{subject}: {}", record.body),
            None => record.body,
        })
        .collect()
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
            memory: binding_memory_lines(runtime),
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
