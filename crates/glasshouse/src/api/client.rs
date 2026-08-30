//! The half of the control door that knocks — capability map lines 745, 746
//! and 747.
//!
//! `api serve` answers `send_message` and `interrupt` against a
//! `SessionRuntime` it owns, and has done since Phase 42. Nothing in this
//! repository ever *called* it: `UnixStream::connect` appeared nowhere in
//! `crates/glasshouse/src`, and `cli::ApiCommand` had exactly one variant,
//! `Serve`. Glasshouse could answer this door and could not knock on it, so
//! the transport that carries a person's keystrokes into a running worker
//! existed with no person on either end of it. This module is the missing
//! end.
//!
//! # Why this closes 746 rather than merely relaying
//!
//! *"Allow direct user input to an orchestrated worker without requiring the
//! orchestrator as an intermediary."* An orchestrated worker's pseudo-terminal
//! is private to the process that spawned it — `super`'s own doc comment
//! explains why nothing else can reach one — so `api serve` is unavoidably
//! the process that performs the write. What line 746 forbids is not *a*
//! process in the middle; it is **the orchestrator** in the middle. Those are
//! different things and the difference is observable:
//!
//! - `glasshouse api send` is a process a person starts from their own
//!   terminal. No agent is asked, no agent's turn is consumed, and no agent
//!   need even be running — the door serves a project, not a conversation.
//! - The door does not decide anything about the text. It is
//!   `unix::dispatch`'s two shortest arms: resolve the session,
//!   write the bytes, answer. There is no model, no prompt, and no policy on
//!   this path.
//!
//! An orchestrator relaying the same words would have to be running, would
//! spend a turn, and could reword them. None of those is true here.
//!
//! # The third verb, and why it completes line 745
//!
//! *"Allow the user to enter any orchestrated worker while it is running."*
//! Send and interrupt could put a person's words and a person's `Ctrl-C`
//! into a worker and could not show them a single character of what came
//! back, so this module shipped with the honest note that a user could type
//! into a worker blind. `glasshouse api read` is the half that was missing:
//!
//!     glasshouse api read --session <ID> [--max-bytes N]
//!
//! It is answered by `Request::RecentOutput`, which is
//! `session::api::SessionApi::recent_output` — a read of a live session's
//! scrollback tail, inside the process that owns the pty, project-scoped
//! through the same seam send and interrupt resolve through. That function
//! existed for this module's whole life with **no production caller at
//! all**; the note this section replaces is what recorded it, and this is
//! the caller.
//!
//! **What this is not.** A transparent full-terminal attach — a person's own
//! terminal handed to the worker's, keystroke for keystroke — is a different
//! thing again, and `session::attach`'s own doc comment explains why it is a
//! larger decision than a verb. What these three commands are is a person in
//! a running worker without an agent between them: words in, an interrupt,
//! and the terminal read back.
//!
//! # It says who it is, and that is the point
//!
//! Every write this module makes carries `"origin": "user"`, because a
//! process a person started from their own terminal is the one caller on this
//! door that knows a person is behind it. Until it did, the event log could
//! not tell a person's intervention from an orchestrator's message: both went
//! through `session::api::SessionApi`, which hard-wired
//! `events::MessageOrigin::Machine`, and produced rows equal field for field.
//! That was harmless while nothing human reached the door and stopped being
//! harmless the moment these three commands shipped.
//!
//! **It is attribution, not authentication.** A different program could
//! connect to the same socket and claim to be a person; nothing here or on
//! the far side tries to stop it, and nothing should be built that does. The
//! socket is already restricted to this user, so a caller that lied would be
//! lying to that user about that user — and the honest callers, which are the
//! ones that exist, stop being indistinguishable. See
//! `protocol::RequestOrigin`.
//!
//! **It never retries.** One connect, one line written, one line read. A send
//! refused by the terminal's canonical line limit
//! (`session::RuntimeError::LineTooLong`) is a refusal that *prevented* a
//! wedge; a client that retried it would be attempting to cause the wedge the
//! refusal exists to avoid.
//!
//! **It has no `--socket`.** `api serve` takes one because a server may be
//! told where to bind; a client that took one could be aimed at *another
//! project's* door, and every project-scope check on the far side is a check
//! about the session named in the request, not about which door received it.
//! Aiming is the whole attack, so the aim is not a parameter: this resolves
//! the socket from the same already-resolved [`Runtime`] every other
//! subcommand resolves, and the only way to address a different project is
//! `--scope`, which changes which project you are rather than letting one
//! project reach into another.
//!
//! # The duplicated socket path, and why it is not left to drift
//!
//! [`socket_path_for`] is a copy of `unix::socket_path_for`, which is private
//! to its own module and was not made visible here because the server is not
//! this half's to change. The copy is proven
//! against the original the only way that is worth anything —
//! `tests/worker_access.rs::the_client_finds_the_door_the_server_actually_bound`
//! starts the real `glasshouse api serve`, reads the path it announces, and
//! drives a real send through this client against both branches of the
//! computation. If the two ever disagree, every client test in that file
//! fails to connect.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::anyhow;
use glasshouse::Runtime;

/// Mirrors `unix::DEFAULT_SOCKET_NAME`.
const DEFAULT_SOCKET_NAME: &str = "control.sock";

/// Mirrors `unix::MAX_SOCKET_PATH_BYTES`, and must keep mirroring it: a
/// client that disagreed with the server about where the fallback starts
/// would look for a socket in the state directory that the server had put in
/// the temp directory, and report an absent door.
const MAX_SOCKET_PATH_BYTES: usize = 90;

/// What every write this module makes says about who is making it.
///
/// `protocol::RequestOrigin::User`'s wire spelling, written as the literal it
/// is because this module only ever sends one value of it — see *"It says who
/// it is, and that is the point"* in this module's doc comment. It is
/// deliberately not a parameter: a flag saying who you are would be a flag for
/// lying about it,
/// on a door where the origin is attribution and not authentication, and
/// there is nobody to tell `glasshouse api send` that it is a command line.
const ORIGIN: &str = "user";

/// How long one call waits for the door to answer before giving up.
///
/// Generous on purpose. The door's accept loop is serial, and a single
/// `send_message` can open the project's memory store behind SQLite's
/// five-second busy timeout before it writes anything, so a bound tight
/// enough to feel responsive would report a working door as a broken one.
/// What this is really for is the case with no answer at all — a door wedged
/// on another connection — where an unbounded wait is a CLI that hangs with
/// nothing on screen.
const CALL_TIMEOUT: Duration = Duration::from_secs(60);

/// Send one line of text to a live session in this project — map line 746.
///
/// `text` is data. It is JSON-encoded into one request field and never
/// interpreted, expanded, or handed to a shell anywhere on this path.
///
/// `"origin": "user"` is stated here and nowhere upstream, for the reason
/// this module's doc comment gives: nobody needs to *tell* this function it
/// is a person's command line, because being one is the whole of what it is.
pub fn send_message(runtime: &Runtime, session: &str, text: &str) -> anyhow::Result<()> {
    call(
        runtime,
        &serde_json::json!({
            "op": "send_message",
            "session": session,
            "text": text,
            "origin": ORIGIN,
        }),
    )?;
    println!("glasshouse: delivered to session `{session}`");
    Ok(())
}

/// Interrupt a live session in this project — map line 747.
///
/// The person's, like [`send_message`]: a `Ctrl-C` somebody asked for is an
/// intervention, and an orchestrator deciding to stop a worker is not the
/// same event even though the byte on the wire is identical.
pub fn interrupt(runtime: &Runtime, session: &str) -> anyhow::Result<()> {
    call(
        runtime,
        &serde_json::json!({
            "op": "interrupt",
            "session": session,
            "origin": ORIGIN,
        }),
    )?;
    println!("glasshouse: interrupted session `{session}`");
    Ok(())
}

/// Show the recent terminal output of a live session in this project — map
/// line 745.
///
/// # Four answers, kept apart
///
/// The door distinguishes four things about a read, and a client that
/// flattened any two of them would hand the user a fact that is not true:
///
/// - **A live session with output** — written to standard output, verbatim
///   and with nothing added, and nothing else is written there. What a
///   worker's terminal holds is what a pipe receives.
/// - **A live session that has printed nothing yet** — `ok` with an empty
///   `output`. Said on standard error, because it is Glasshouse talking
///   rather than the worker, and it succeeds: a worker that has said nothing
///   is not a failure to read it.
/// - **A session no process is running** — the door's `not live` refusal,
///   which fails. This is the distinction the whole verb turns on:
///   `SessionApi::recent_output` refuses rather than answering `""` because
///   *"returning an empty string would be a lie the caller has no way to
///   detect"*, and a client that printed nothing for both would have told
///   that lie on the door's behalf.
/// - **No such session in this project** — the door's scoped sentence,
///   which fails. Passed through unchanged, as every error on this path is;
///   see [`call`].
///
/// `max_bytes` is optional rather than defaulted here on purpose. The door
/// owns both the default and the ceiling, so a client carrying its own copy
/// of either could drift from the door it is talking to — and the ceiling in
/// particular is not a client's to state, because a client cannot enforce
/// it.
pub fn read_output(
    runtime: &Runtime,
    session: &str,
    max_bytes: Option<usize>,
) -> anyhow::Result<()> {
    let mut request = serde_json::json!({
        "op": "recent_output",
        "session": session,
    });
    if let Some(max_bytes) = max_bytes {
        request["max_bytes"] = serde_json::json!(max_bytes);
    }

    let result = call(runtime, &request)?;
    let output = result
        .get("output")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| {
            anyhow!("this project's control API answered a read without any output in it")
        })?;

    if output.is_empty() {
        eprintln!("glasshouse: session `{session}` is running and has printed nothing yet");
        return Ok(());
    }

    // Verbatim, and flushed explicitly: this is the one command whose
    // standard output is another program's bytes rather than Glasshouse's
    // own sentence, so nothing is appended to it — not even a newline the
    // worker did not print.
    print!("{output}");
    std::io::stdout().flush()?;
    Ok(())
}

/// Where this project's door listens when nothing overrides it.
///
/// A copy of `unix::socket_path_for` — see this module's doc comment for why
/// it is a copy and what keeps it honest. Both branches matter: the preferred
/// path is inside the project's own state directory, and a state directory
/// nested deeply enough to push `control.sock` past `sockaddr_un`'s limit
/// makes the server fall back to a short name in the temp directory keyed by
/// the project id. A client that knew only the first branch would report "not
/// listening" against a door that was listening.
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

/// One request, one response, one connection — the whole client.
///
/// # On what reaches the user
///
/// An error the door produced is passed through as the door's own sentence,
/// unchanged and undecorated. That is deliberate: the door already owns the
/// judgement about what its errors may say — commit `8b489b7` suppressed a
/// leak of the database's absolute path on this exact surface — and a client
/// that reworded them would either lose the distinction between *"no session
/// `x` in this project"* and *"session `x` belongs to project `y`"*, or
/// re-derive a suppression rule that is not its to own.
///
/// An error *this side* produced never names the socket. A path is a fact
/// about the user's filesystem, it is never the action they need, and the
/// action they need — start the door — is the same whichever path it would
/// have bound.
fn call(runtime: &Runtime, request: &serde_json::Value) -> anyhow::Result<serde_json::Value> {
    let socket = socket_path_for(runtime);
    let stream = UnixStream::connect(&socket).map_err(|err| match err.kind() {
        std::io::ErrorKind::NotFound | std::io::ErrorKind::ConnectionRefused => anyhow!(
            "this project's control API is not listening; start it with `glasshouse api serve` \
             in this project, and note that it can only reach sessions that same door started"
        ),
        std::io::ErrorKind::PermissionDenied => anyhow!(
            "this project's control socket refused the connection; it is restricted to the user \
             that started `glasshouse api serve`"
        ),
        _ => anyhow!("could not reach this project's control API: {err}"),
    })?;
    // Bounded on both halves. A door that accepted the connection and then
    // stopped reading would otherwise wedge the write, which looks exactly
    // like a wedged read from the outside and is just as unhelpful.
    stream.set_write_timeout(Some(CALL_TIMEOUT))?;
    stream.set_read_timeout(Some(CALL_TIMEOUT))?;

    let mut writer = stream.try_clone()?;
    let mut payload = serde_json::to_string(request)?;
    payload.push('\n');
    writer
        .write_all(payload.as_bytes())
        .map_err(|err| timed_out(&err, "sending the request"))?;
    writer
        .flush()
        .map_err(|err| timed_out(&err, "sending the request"))?;

    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    let read = reader
        .read_line(&mut line)
        .map_err(|err| timed_out(&err, "waiting for the answer"))?;
    if read == 0 {
        return Err(anyhow!(
            "this project's control API closed the connection without answering; check the \
             `glasshouse api serve` process"
        ));
    }

    let response: serde_json::Value = serde_json::from_str(line.trim_end()).map_err(|err| {
        anyhow!("this project's control API sent an answer this Glasshouse cannot read: {err}")
    })?;
    match response.get("status").and_then(serde_json::Value::as_str) {
        Some("ok") => Ok(response
            .get("result")
            .cloned()
            .unwrap_or(serde_json::Value::Null)),
        // The door's sentence, verbatim. See this function's doc comment.
        Some("error") => Err(anyhow!(
            "{}",
            response
                .get("message")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("this project's control API refused the request and did not say why")
        )),
        _ => Err(anyhow!(
            "this project's control API sent an answer with no status this Glasshouse knows"
        )),
    }
}

/// Say that a bound was reached, rather than reporting a timeout as an
/// ordinary I/O failure.
///
/// The honest part is the second clause. Once the request is on the wire this
/// side cannot tell a door that never received it from one that delivered the
/// text and was too slow to say so, and a client that guessed would be
/// guessing about whether a person's words reached a worker.
fn timed_out(err: &std::io::Error, doing: &str) -> anyhow::Error {
    match err.kind() {
        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut => anyhow!(
            "this project's control API did not respond within {} seconds while {doing}; it may \
             or may not have acted on this request, so check the session before repeating it",
            CALL_TIMEOUT.as_secs()
        ),
        _ => anyhow!("could not reach this project's control API while {doing}: {err}"),
    }
}
