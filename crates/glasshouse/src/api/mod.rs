//! The external control door for one project's Glasshouse.
//!
//! `glasshouse api serve` owns a single process's `SessionRuntime` and
//! answers requests against it, plus the project's memory and checkpoint
//! stores, over a Unix domain socket — the only way to reach a running
//! session's runtime from outside the process that started it. A session
//! started outside this door is visible here (the store is shared) but not
//! controllable here (send/interrupt answer `ApiError::NotLive`).
//! `unix` answers the door; `client` knocks on it — see `client`'s own doc
//! comment for why it deliberately takes no socket path. Every handler in
//! `unix` reaches sessions through `session::api::SessionApi`, which
//! refuses a foreign session by construction, and memory and checkpoints
//! through `memory::ProjectMemory` and `checkpoint::store::ProjectCheckpoints`,
//! all opened against one already-resolved `Runtime` — there is no request
//! field naming a project: the door itself is the scope. See
//! `unix::authorize` for the authentication mechanism and its limits.
//! Proven by running the shipped binary (`tests/session_model.rs`'s API
//! cluster), not by in-process unit tests.
// History: design-decisions.md, "Trims: api, events, harness and config module docs, second packet", crates/glasshouse/src/api/mod.rs module doc.

/// Not gated, since Phase 43. `protocol` used to be `#[cfg(unix)]` because
/// its only consumer was `unix::serve`, and a platform without Unix domain
/// sockets made the whole module dead code under `-D warnings`. The MCP door
/// below reaches every request over stdio on every platform, so the wire
/// shape is live everywhere now — and so are the handlers in `unix`, whose
/// socket-specific items carry their own `#[cfg(unix)]` item by item (see
/// that module's doc for why it kept its name).
mod protocol;

/// See [`protocol`]: the handlers compile everywhere, the socket does not.
mod unix;

/// The MCP door — Phase 43. A second transport over the same handlers, on
/// stdio, and therefore on every platform: nothing in it is gated.
mod mcp;

pub use mcp::serve as serve_mcp;

/// Gated to match the transport it speaks over, exactly as `unix`'s socket
/// items are. It is the *client* half of this door — the half that connects,
/// writes one request and reads one answer — and it is separate from `unix`
/// because that module is the server and the two share nothing but the wire
/// shape in `protocol`.
#[cfg(unix)]
mod client;

#[cfg(unix)]
pub(crate) use client::send_machine_message;
#[cfg(unix)]
pub use client::{interrupt, mute, read_output, send_message, unmute};
#[cfg(unix)]
pub use unix::serve;

/// Why a machine-originated write through this project's control door did
/// not reach the session it named — capability map line 2414.
///
/// Kept distinct rather than flattened to a sentence immediately: this
/// packet's one caller, the edit-intent hook's
/// `notify_orchestrator_of_conflict`, must log a different sentence for
/// "nothing is listening", "the socket refused the connection" and "the
/// door does not hold this session live" — and must never guess between
/// them. [`send_message`], `interrupt`, `mute`, `unmute` and `read_output`
/// still get one flattened sentence, unchanged: see this type's `From`
/// impl below.
///
/// Defined here rather than inside `client` (which is `#[cfg(unix)]`)
/// because the non-Unix fallback for [`send_machine_message`] below needs
/// to name it too.
pub(crate) enum CallError {
    /// The socket does not exist, or nothing accepted the connection:
    /// `glasshouse api serve` is not running for this project.
    NotListening(String),
    /// The socket exists but rejected this connection outright — it is
    /// restricted to the user that started the door.
    ConnectionRefused(String),
    /// The door was reached and answered `status: "error"` — in practice,
    /// for this caller, because it does not hold the named session live
    /// (never started through it, or reachable only through a pane it
    /// could not reach).
    DoorRefused(String),
    /// Any other transport-level failure: a bad response, a timed-out
    /// write or read, or a status this Glasshouse does not recognise.
    Other(anyhow::Error),
}

impl std::fmt::Display for CallError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CallError::NotListening(msg)
            | CallError::ConnectionRefused(msg)
            | CallError::DoorRefused(msg) => f.write_str(msg),
            CallError::Other(err) => write!(f, "{err}"),
        }
    }
}

impl From<CallError> for anyhow::Error {
    fn from(err: CallError) -> Self {
        match err {
            CallError::Other(err) => err,
            other => anyhow::anyhow!("{other}"),
        }
    }
}

/// The control API needs a Unix domain socket, and Windows has no drop-in
/// equivalent (a named pipe is a different API with different authentication
/// primitives). Refusing loudly here is the honest answer until that
/// transport exists, rather than a build that silently does nothing on
/// Windows.
///
/// One sentence for all four verbs rather than four near-copies: a client
/// that could not connect on this platform and a server that could not bind
/// on it are the *same* missing transport, and a user who reads two different
/// explanations of one absence has been told there are two problems.
#[cfg(not(unix))]
fn no_unix_socket() -> anyhow::Error {
    anyhow::anyhow!(
        "glasshouse: the control API needs a Unix domain socket, which this platform does not \
         have; Windows needs a named-pipe transport that does not exist yet (Phase 42's socket \
         door is Unix-only)"
    )
}

/// See [`no_unix_socket`].
#[cfg(not(unix))]
pub fn serve(
    _runtime: &glasshouse::Runtime,
    _socket_override: Option<std::path::PathBuf>,
) -> anyhow::Result<()> {
    Err(no_unix_socket())
}

/// See [`no_unix_socket`]. Refused for the same reason `serve` is, and said
/// the same way: there is no door to knock on here because there is no door
/// to open here.
#[cfg(not(unix))]
pub fn send_message(
    _runtime: &glasshouse::Runtime,
    _session: &str,
    _text: &str,
) -> anyhow::Result<()> {
    Err(no_unix_socket())
}

/// See [`no_unix_socket`]. The edit-intent hook's undeliverable branch (see
/// [`CallError::NotListening`]) is exactly the sentence this platform needs:
/// there is no door here for the hook to have failed to reach.
#[cfg(not(unix))]
pub(crate) fn send_machine_message(
    _runtime: &glasshouse::Runtime,
    _session: &str,
    _text: &str,
) -> Result<(), CallError> {
    Err(CallError::NotListening(no_unix_socket().to_string()))
}

/// See [`no_unix_socket`].
#[cfg(not(unix))]
pub fn interrupt(_runtime: &glasshouse::Runtime, _session: &str) -> anyhow::Result<()> {
    Err(no_unix_socket())
}

/// See [`no_unix_socket`]. There is no door to read through here for the same
/// reason there is none to write through.
#[cfg(not(unix))]
pub fn read_output(
    _runtime: &glasshouse::Runtime,
    _session: &str,
    _max_bytes: Option<usize>,
) -> anyhow::Result<()> {
    Err(no_unix_socket())
}

/// See [`no_unix_socket`]. Line 1717's two verbs are client calls like the
/// three above, so they are absent here for exactly the same reason and say
/// so in exactly the same sentence — the CLI shape is identical on every
/// platform, and only the answer differs.
#[cfg(not(unix))]
pub fn mute(_runtime: &glasshouse::Runtime, _session: &str, _seconds: u64) -> anyhow::Result<()> {
    Err(no_unix_socket())
}

/// See [`no_unix_socket`].
#[cfg(not(unix))]
pub fn unmute(_runtime: &glasshouse::Runtime, _session: &str) -> anyhow::Result<()> {
    Err(no_unix_socket())
}

/// The Windows half of Phase 42's door, proved on the platform that has it.
///
/// `main.rs`'s `ApiCommand::Mute`/`Unmute` arms call `api::mute`/`api::unmute`
/// unconditionally — that is the CLI shape being identical everywhere — so on
/// Windows the person running them must get a sentence rather than a build
/// failure. This asserts the sentence is `no_unix_socket`'s own, and that it
/// names no socket path: there is no socket here, and an error that invented a
/// path would be describing a machine rather than a missing transport.
#[cfg(all(test, not(unix)))]
mod platform_refusal {
    use clap::Parser;

    fn runtime(tmp: &std::path::Path) -> glasshouse::Runtime {
        let root = tmp.join("project");
        std::fs::create_dir_all(root.join(".git")).unwrap();
        let cli = glasshouse::Cli::try_parse_from([
            "glasshouse",
            "--data-dir",
            tmp.join("data").to_str().unwrap(),
            "--config-dir",
            tmp.join("config").to_str().unwrap(),
        ])
        .unwrap();
        glasshouse::bootstrap(&cli, &root).unwrap()
    }

    #[test]
    fn mute_and_unmute_refuse_by_name_where_there_is_no_unix_socket() {
        let tmp = tempfile::tempdir().unwrap();
        let runtime = runtime(tmp.path());

        for message in [
            super::mute(&runtime, "session-1", 600)
                .expect_err("mute must refuse without a Unix socket")
                .to_string(),
            super::unmute(&runtime, "session-1")
                .expect_err("unmute must refuse without a Unix socket")
                .to_string(),
        ] {
            assert!(
                message.contains("the control API needs a Unix domain socket"),
                "the refusal must be `no_unix_socket`'s own sentence, got: {message}"
            );
            assert!(
                message.contains("named-pipe transport that does not exist yet"),
                "the refusal must name what is missing, got: {message}"
            );
            assert!(
                !message.contains(".sock") && !message.contains('\\'),
                "the refusal must not print a socket path, got: {message}"
            );
        }
    }
}
