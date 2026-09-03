//! The request handlers every control-API transport shares, and the Unix
//! domain socket transport that was the first of them.
//!
//! Two halves live here, and only one of them is Unix-specific. The handlers
//! — [`dispatch`] and everything it calls — are plain functions over the
//! project's stores and this process's [`SessionRuntime`], and they compile
//! on every platform Glasshouse ships for, because the MCP door
//! (`super::mcp`, Phase 43) reaches them over stdio on every one of those
//! platforms. The socket transport — [`serve`], [`handle_connection`], and
//! the peer-credential check behind them — is `#[cfg(unix)]`, item by item,
//! for the same reason the module used to be gated as a whole: a Unix domain
//! socket is a Unix thing. The module keeps its name because the handlers
//! are the same handlers, the co-editing rounds in flight on this file are
//! easier to reconcile against a file that stayed put, and a rename is a
//! cheap follow-up once those have landed.
//!
//! [`ServerContext`] is the seam between the halves: it owns what every
//! handler needs and offers exactly one verb, `handle`. A transport holds a
//! context and nothing else, which is how the rule that no door may reach a
//! store except through `dispatch` is a property of the types rather than a
//! matter of discipline.

mod assumptions;
mod checkpoints;
mod events;
mod memory;
mod routing;
mod sessions;

use std::collections::{HashMap, HashSet};
#[cfg(unix)]
use std::io::{BufRead, BufReader, Write};
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
#[cfg(unix)]
use std::os::unix::io::AsRawFd;
#[cfg(unix)]
use std::os::unix::net::{UnixListener, UnixStream};
#[cfg(unix)]
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use glasshouse::Runtime;
use glasshouse::events::{EventBus, EventLog, EventLogSink, EventSink};
use glasshouse::guardrails::{NewAssumption, NewTransition};
use glasshouse::policy;
use glasshouse::session::api::{ApiError, SessionApi};
use glasshouse::session::{ProjectSessions, SessionId, SessionRuntime};

use assumptions::{
    list_assumptions, preflight, promote_assumption, record_assumption, update_assumption,
};
use checkpoints::{get_checkpoint, request_checkpoint};
use events::{WatchState, Watches, project_events, pump_watches, watch_worker, watching};
use memory::{Injected, current_memory, deliver_memory, get_memory, query_memory, select_memory};
use routing::{recommend_route, resource_capacity, routing_model_status};
use sessions::{
    Muted, Policied, deliver_policy, lifecycle_str, mute_refusal, mute_remaining, mute_session,
    send_through_pane, session_summary, spawn_session, unmute_session,
};

use super::protocol::{Request, RequestOrigin, Response};

/// The socket file's name inside the project's own state directory, when
/// nothing overrides it.
#[cfg(unix)]
const DEFAULT_SOCKET_NAME: &str = "control.sock";

/// `sockaddr_un.sun_path` is 104 bytes on macOS/BSD and 108 on Linux,
/// including the terminating nul this crate never sees. A path within this
/// bound is safe on every Unix `glasshouse` ships for; anything longer binds
/// with `EINVAL`/`ENAMETOOLONG` well before this door gets to authorize a
/// single connection. Chosen as 90 rather than the tighter platform minimum
/// so the margin survives a slightly longer project id without needing a
/// per-platform constant.
#[cfg(unix)]
const MAX_SOCKET_PATH_BYTES: usize = 90;

/// The hard ceiling on how much of a session's terminal output
/// [`Request::RecentOutput`] returns in one call, regardless of the
/// `max_bytes` a caller asks for — the same shape as [`MAX_MEMORY_LIMIT`] and
/// [`MAX_SNAPSHOT_BODY_CHARS`] above, and load-bearing for a reason neither
/// of those has.
///
/// Every other bound on this door limits how many *rows* a caller may pull
/// out of a store it is querying. This one limits a buffer nobody queried:
/// a session's scrollback is `session::runtime::DEFAULT_SCROLLBACK_BYTES`
/// wide, filled by whatever the harness happened to print, and a caller
/// asking for `usize::MAX` would otherwise receive the whole of it —
/// JSON-escaped, on one line, over a socket — with the size decided by how
/// long the worker had been talking rather than by anything either end
/// chose.
///
/// Sixty-four kibibytes is many screenfuls of a worker's terminal and a
/// quarter of what the scrollback holds: enough to see what a worker is
/// doing, and far short of "send me everything you have". A caller that
/// wants a specific earlier moment is asking for history, which this door
/// does not have — see [`Request::RecentOutput`]'s own doc comment for why
/// there is none to give.
const MAX_RECENT_OUTPUT_BYTES: usize = 64 * 1024;

/// How often the background tick answers terminal queries and reaps exited
/// sessions between requests. Mirrors `run_headless`'s `POLL` in `main.rs`:
/// short enough that `poll_exits` marks a dead session promptly, long enough
/// not to spin the accept thread's sibling for no reason.
const TICK: Duration = Duration::from_millis(50);

/// Everything a request handler needs, owned once per server process and
/// shared by every transport that answers a [`Request`].
///
/// # One context, two doors
///
/// [`dispatch`] needs six things — the project's [`Runtime`], its open
/// session store, the [`SessionRuntime`] this process holds pseudo-terminals
/// in, the registry of orchestrator watches, the event recorder, and the
/// memory-injection ledger. Until this type existed [`serve`] built all six
/// on its own stack and threaded them through every call, which was fine
/// while the Unix socket was the only transport. The MCP door (`super::mcp`,
/// Phase 43) is a second transport onto the same handlers, and the design
/// ruling behind it is that no tool may perform an operation this door does
/// not already perform, nor reach a store except through the same
/// `dispatch`. The cheapest way to make that structural is for there to be
/// exactly one thing a transport can hold, and for its only verb to be
/// [`ServerContext::handle`].
///
/// # The tick comes with it
///
/// The background tick — reaping exited sessions, answering terminal
/// queries, pumping orchestrator watches — is started by
/// [`ServerContext::open`], not by the transport, because a session spawned
/// through either door needs its exit reaped by *somebody*, and the process
/// that spawned it is the only one that can. A transport that forgot to tick
/// would leave every one of its sessions `running` forever; a transport that
/// cannot forget is better.
///
/// # Scope
///
/// Opened for one already-resolved [`Runtime`] and answering only against
/// it — see `super`'s module doc. There is no way to construct one for a
/// project the process was not started in, and nothing in it names a
/// project, a path, or a database that a request could override.
pub(crate) struct ServerContext {
    runtime: Runtime,
    sessions: ProjectSessions,
    live: Arc<Mutex<SessionRuntime>>,
    watches: Arc<Watches>,
    recorder: EventRecorder,
    injected: Injected,
    /// Line 1717's record, alongside the runtime whose sessions it is about
    /// — a field here rather than a parameter threaded through the socket
    /// door, so the stdio door cannot forget it.
    muted: Muted,
    policied: Policied,
}

impl ServerContext {
    /// Open the project's session store, build the runtime every handler
    /// shares, and start the tick that keeps it honest.
    ///
    /// Fallible before anything is announced on purpose: a project database
    /// that cannot be opened read-write ends this before a transport has
    /// said it is listening, which is the order [`serve`]'s ready line
    /// depends on.
    pub(crate) fn open(runtime: &Runtime) -> anyhow::Result<Self> {
        let sessions = ProjectSessions::open(runtime)?;
        // The door's own lifecycle stream, and the durable recording of it.
        // See [`EventRecorder`] for why the runtime is built around a bus
        // this function owns rather than `SessionRuntime::new`'s private one.
        let events = EventBus::new();
        let recorder = EventRecorder::attach(runtime, &events);
        let live = Arc::new(Mutex::new(SessionRuntime::with_event_bus(
            glasshouse::session::runtime::DEFAULT_SCROLLBACK_BYTES,
            events,
        )));
        let watches: Arc<Watches> = Arc::new(Mutex::new(Vec::new()));
        // Line 1135's record, alongside the runtime whose sessions it is
        // about.
        let injected: Injected = Arc::new(Mutex::new(HashMap::new()));
        // Line 1717's record, in the same place and for the same reason.
        let muted: Muted = Arc::new(Mutex::new(HashMap::new()));
        // The other half of the same bookkeeping: what this session has
        // already been told, for the text Glasshouse wrote rather than the
        // text it quoted.
        let policied: Policied = Arc::new(Mutex::new(HashSet::new()));

        let context = Self {
            runtime: runtime.clone(),
            sessions,
            live,
            watches,
            recorder,
            injected,
            muted,
            policied,
        };
        context.start_tick();
        Ok(context)
    }

    /// Answer one request against this project.
    ///
    /// This is the whole of what a transport may do with a context. Every
    /// verb, every bound, and every scope check is [`dispatch`]'s, so two
    /// doors cannot disagree about what a request means.
    pub(crate) fn handle(&self, request: Request) -> Response {
        dispatch(
            request,
            &self.runtime,
            &self.sessions,
            &self.live,
            &self.watches,
            &self.recorder,
            &self.injected,
            &self.muted,
            &self.policied,
        )
    }

    /// Start the background tick.
    ///
    /// A transport only touches the runtime while a request is being
    /// handled; a session with nothing asking about it between requests
    /// would otherwise never have its exit reaped. This mirrors
    /// `run_headless`'s own reason for ticking outside the wait for the next
    /// event.
    ///
    /// The orchestrator wake-up flow rides the same tick — see
    /// [`pump_watches`]. It belongs here rather than in a thread of its own
    /// because it needs exactly what this one already holds, and because a
    /// watch that only advanced while a request happened to arrive would
    /// wake an orchestrator only when it was already talking to the door.
    /// `Runtime` is cheap to clone and holds no connection, so the thread
    /// owns one rather than borrowing this context's.
    fn start_tick(&self) {
        let live = Arc::clone(&self.live);
        let watches = Arc::clone(&self.watches);
        let runtime = self.runtime.clone();
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
}

/// Run the control API until the process is killed.
///
/// Binds a fresh Unix domain socket at `socket_override`, or the project's
/// own state directory when nothing is given and that path fits
/// `sockaddr_un` — see [`socket_path_for`] for what happens when it does
/// not. Refuses to keep a stale file from a crashed prior run. Every
/// accepted connection is authorized (see [`authorize`]) before its one
/// request is read.
#[cfg(unix)]
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

    let context = ServerContext::open(runtime)?;

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
        if let Err(err) = handle_connection(stream, &context) {
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
#[cfg(unix)]
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
#[cfg(unix)]
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

#[cfg(unix)]
fn process_uid() -> u32 {
    // SAFETY: `getuid` takes no arguments, has no failure mode, and returns
    // a plain integer — there is no invariant for the caller to uphold.
    unsafe { libc::getuid() }
}

#[cfg(all(unix, target_os = "linux"))]
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

#[cfg(all(unix, not(target_os = "linux")))]
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
pub(super) fn lock(live: &Mutex<SessionRuntime>) -> std::sync::MutexGuard<'_, SessionRuntime> {
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
#[cfg(unix)]
fn handle_connection(stream: UnixStream, context: &ServerContext) -> anyhow::Result<()> {
    let mut writer = stream.try_clone()?;
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    reader.read_line(&mut line)?;

    let response = match serde_json::from_str::<Request>(line.trim_end()) {
        Ok(request) => context.handle(request),
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
    muted: &Muted,
    policied: &Policied,
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
            guardrail,
            presentation,
        } => spawn_session(
            runtime,
            &store,
            live,
            &harness,
            args,
            role.as_deref(),
            task,
            guardrail,
            presentation.as_deref(),
            injected,
            policied,
        ),
        // Line 1125's *"routed task"* is not only a spawn's own. This verb
        // is the follow-up half of the same seam — `SessionApi::send_text` —
        // so the context-selection step belongs on both or on neither. It is
        // also the only path on which line 1135 is a real question: a session
        // is spawned once, and can be given a task many times.
        //
        // The origin is the request's, defaulting to `Machine`, so an
        // orchestrator that says nothing is recorded exactly as it was before
        // the field existed and `glasshouse api send` is recorded as the
        // person who ran it. See `protocol::RequestOrigin`, including why
        // this is attribution rather than authentication.
        Request::SendMessage {
            session,
            text,
            origin,
        } => {
            let id = SessionId::new(session);
            // Line 1717's mute, answered **before** this door opens the
            // project's memory store below — the one control that lives here
            // rather than at the seam.
            //
            // Here because a mute is this door's own state and this door's
            // own policy: it is about *requests an orchestrator makes*, not
            // about every write into a pseudo-terminal, and the answer has to
            // be a `Response::Error` naming the remaining time. Early because
            // `select_memory` opens the memory database behind SQLite's busy
            // timeout, and paying that for a request already decided against
            // is the acquisition-on-an-unwatched-path practice §65 records
            // the cost of.
            //
            // **Line 1719 is deliberately not checked here.** It is taken at
            // `SessionApi::send_text`, the one seam every machine write in
            // this process passes through, and a second copy of it on this
            // path would be a rule with two enforcement points that can
            // drift — and, measured: the mutation
            // `1719-the-seam-admits-everything` SURVIVED while this check
            // existed, because the door answered first and nothing in the
            // suite ever reached the seam. One rule, one place.
            //
            // Only machine-originated messages are checked. A mute exists to
            // stop a person being talked over and has nothing to say about
            // the person themselves.
            if origin == RequestOrigin::Machine {
                {
                    let mut guard = lock(live);
                    let api = SessionApi::new(&store, &mut guard);
                    // Project scope first, before the mute map is touched: a
                    // session that is not this project's is refused for being
                    // foreign, never for being muted.
                    if let Err(err) = api.state(&id) {
                        return Response::err(api_error(err));
                    }
                }
                if let Some(remaining) = mute_remaining(muted, &id) {
                    return Response::err(mute_refusal(&id, remaining));
                }
            }
            // Selected before the runtime lock is taken: opening the
            // project's memory goes through `database::open`, which can wait
            // on a busy timeout, and nothing that waits may hold the lock
            // every live session's I/O runs through.
            let briefing = select_memory(runtime, &id, &text, injected);
            // The door first, always — Phase 17 line 758. The lock is
            // released before any fallback: reaching a pane spawns `cmux`,
            // and nothing that waits on a child process may hold the lock
            // every live session's I/O runs through.
            let attempt = {
                let mut guard = lock(live);
                let mut api = SessionApi::new(&store, &mut guard);
                deliver_memory(runtime, &mut api, &id, briefing, injected);
                deliver_policy(&mut api, runtime, &id, policied);
                api.send_text(&id, &text, origin.message_origin())
            };
            match attempt {
                Ok(()) => Response::ok(serde_json::json!({ "via": "door" })),
                // `NotLive` is the one refusal a pane can answer: the
                // session exists in this project (`resolve` passed) and no
                // runtime here holds it — which is exactly what a session
                // launched inside a cmux pane looks like from this process.
                Err(ApiError::NotLive { .. }) => send_through_pane(&store, &id, &text),
                Err(err) => Response::err(api_error(err)),
            }
        }
        // Deliberately not gated by a mute or by line 1719's precedence
        // window, whoever sends it — see `SessionApi::interrupt`, which
        // carries the reason: a control that could leave a runaway harness
        // unstoppable would have taken something away in the name of giving
        // the person control.
        Request::Interrupt { session, origin } => {
            let mut guard = lock(live);
            let mut api = SessionApi::new(&store, &mut guard);
            match api.interrupt(&SessionId::new(session), origin.message_origin()) {
                Ok(()) => Response::ok(serde_json::json!({})),
                Err(err) => Response::err(api_error(err)),
            }
        }
        Request::MuteSession { session, seconds } => {
            mute_session(&store, live, muted, &session, seconds)
        }
        Request::UnmuteSession { session } => unmute_session(&store, live, muted, &session),
        // Through `SessionApi::recent_output`, never into the runtime's
        // scrollback directly, for the reason `ListSessions` above goes
        // through `SessionApi::list`: that seam is where project scope is
        // checked, and a door that read a worker's terminal from underneath
        // it would be the one caller in this binary that never had to say
        // whose session it was reading. A scrollback is the most sensitive
        // thing this door returns — it is whatever the harness printed — so
        // this is the last verb that should be the exception.
        //
        // The bound is applied here rather than passed through, so a caller
        // may lower the ceiling and cannot raise it. `usize::MAX` is a
        // request for [`MAX_RECENT_OUTPUT_BYTES`], not for everything.
        //
        // `NotLive` is passed on as the refusal it is. Answering `ok` with
        // an empty string would collapse "nothing is running this session"
        // into "this session has printed nothing", and the caller has no way
        // to tell them apart afterwards.
        Request::RecentOutput { session, max_bytes } => {
            let mut guard = lock(live);
            let api = SessionApi::new(&store, &mut guard);
            match api.recent_output(
                &SessionId::new(session),
                max_bytes.min(MAX_RECENT_OUTPUT_BYTES),
            ) {
                Ok(output) => Response::ok(serde_json::json!({ "output": output })),
                Err(err) => Response::err(api_error(err)),
            }
        }
        Request::ResourceCapacity => resource_capacity(runtime),
        Request::RoutingModel => routing_model_status(runtime),
        Request::RecommendRoute {
            task,
            moment,
            alternatives,
        } => recommend_route(runtime, task.as_deref(), &moment, alternatives),
        Request::Events {
            after,
            limit,
            assumptions_after,
        } => project_events(runtime, after, limit, assumptions_after, recorder),
        // Phase 21K — the five assumption verbs. Every session named on one
        // goes through `SessionApi` first, for `ListSessions`' reason: the
        // ledger is scoped by trigger at the database, and the door checks
        // anyway, so a foreign session is refused before a row could be
        // written for it.
        Request::Preflight { session, change } => {
            preflight(runtime, sessions, live, session.as_deref(), change)
        }
        Request::RecordAssumption {
            session,
            claim,
            evidence,
            evidence_source,
            uncertainty,
            affected,
            verification,
            origin,
        } => record_assumption(
            runtime,
            &store,
            live,
            NewAssumption {
                session,
                claim,
                evidence,
                evidence_source,
                uncertainty,
                affected,
                verification,
                origin: origin.guardrail_origin(),
            },
        ),
        Request::UpdateAssumption {
            assumption,
            state,
            note,
            response,
            record_failed_approach,
            origin,
        } => update_assumption(
            runtime,
            &assumption,
            NewTransition {
                state,
                origin: origin.guardrail_origin(),
                note,
                response,
                subject: None,
            },
            record_failed_approach,
        ),
        Request::ListAssumptions { session, limit } => {
            list_assumptions(runtime, &store, live, session.as_deref(), limit)
        }
        Request::PromoteAssumption {
            assumption,
            kind,
            note,
            origin,
        } => promote_assumption(runtime, &assumption, kind, note, origin.guardrail_origin()),
        Request::WatchWorker { session, notify } => {
            watch_worker(runtime, &store, live, watches, &session, &notify)
        }
        // Constant text, and the only handler here that reads nothing at
        // all: no store is opened, no session is resolved and no
        // configuration is consulted. `implementation_policy = false`
        // silences delivery, not an answer to a caller that asked.
        Request::ImplementationPolicy { part } => {
            Response::ok(serde_json::json!({ "policy": policy::render(part) }))
        }
        Request::GetCheckpoint {
            checkpoint,
            document,
        } => get_checkpoint(runtime, checkpoint.as_deref(), document),
        Request::QueryMemory {
            query,
            history,
            limit,
            path,
        } => query_memory(runtime, &query, history, limit, path.as_deref()),
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

pub(super) fn api_error(err: ApiError) -> String {
    err.to_string()
}
