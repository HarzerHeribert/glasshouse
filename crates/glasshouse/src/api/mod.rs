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
pub use client::{interrupt, mute, read_output, send_message, unmute};
#[cfg(unix)]
pub use unix::serve;

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
