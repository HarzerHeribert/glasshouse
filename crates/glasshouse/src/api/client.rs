//! The client side of the control door — capability map lines 745, 746 and
//! 747: `send_message`, `interrupt` and `read_output` let a person act on a
//! running worker directly, without the orchestrator as an intermediary.
//!
//! Every write here carries `"origin": "user"` (see [`ORIGIN`]) because a
//! process a person started from their own terminal is the one caller on
//! this door that knows a person is behind it — attribution, not
//! authentication, since the socket is already restricted to this user.
//!
//! It never retries: one connect, one line written, one line read. It has
//! no `--socket`: the door resolves from the same [`Runtime`] every other
//! subcommand resolves, so aiming this client at another project stays
//! impossible.
//!
//! [`socket_path_for`] duplicates `unix::socket_path_for`, which is private
//! to its own module; the two are proven to agree by
//! `tests/worker_access.rs::the_client_finds_the_door_the_server_actually_bound`,
//! which starts a real server and drives a real client against it.
//!
//! History: design-decisions.md, "Trims: api/client.rs", module doc.

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

/// Send one machine-originated line of text to a live session in this
/// project, without printing anything to stdout — capability map line 2414.
///
/// [`send_message`] cannot be reused as is for this: it always states
/// `"origin": "user"` (this module's doc comment says why that is not a
/// parameter) and always prints a confirmation line, and this function's one
/// caller — the edit-intent hook — has neither fact to state and cannot
/// afford the print: the hook's stdout is `PreToolUse`'s own response
/// channel, and a second line on it would corrupt that protocol. Omitting
/// `origin` here states nothing, which the wire format already treats as
/// `RequestOrigin::Machine` (`protocol.rs`) — exactly what this caller is.
///
/// Returns [`super::CallError`] rather than a rendered sentence: the caller
/// logs a different sentence for "nothing is listening", "the socket
/// refused the connection" and "the door does not hold this session live",
/// and must never guess between them by parsing another function's prose.
pub(crate) fn send_machine_message(
    runtime: &Runtime,
    session: &str,
    text: &str,
) -> Result<(), super::CallError> {
    call_inner(
        runtime,
        &serde_json::json!({
            "op": "send_message",
            "session": session,
            "text": text,
        }),
    )?;
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

/// Stop this project's control API delivering orchestrator messages to one
/// session, for `seconds` — map line 1717.
///
/// A person's own messages are unaffected, and so is an interrupt: muting a
/// worker is how you get it to yourself, not how you lose the ability to stop
/// it. The door caps the duration and its answer says what was actually
/// granted, which is why this prints the door's number rather than the one
/// that was asked for.
///
/// The mute lives in the `glasshouse api serve` process and nowhere else, so
/// restarting that process clears every mute it was holding. Said here, on
/// the command a person runs, rather than only in the protocol.
pub fn mute(runtime: &Runtime, session: &str, seconds: u64) -> anyhow::Result<()> {
    let result = call(
        runtime,
        &serde_json::json!({
            "op": "mute_session",
            "session": session,
            "seconds": seconds,
        }),
    )?;
    let granted = result
        .get("muted_for_seconds")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(seconds);
    println!(
        "glasshouse: session `{session}` will refuse orchestrator messages for {granted}s; \
         your own messages and interrupts still reach it"
    );
    if result.get("capped").and_then(serde_json::Value::as_bool) == Some(true) {
        eprintln!(
            "glasshouse: {seconds}s is longer than this door will mute a session for, so it \
             granted {granted}s"
        );
    }
    eprintln!(
        "glasshouse: a mute lives in the `glasshouse api serve` process; restarting it lifts \
         every mute"
    );
    Ok(())
}

/// Lift a mute before it expires — map line 1717.
///
/// Safe to run against a session nobody muted: the door answers with what it
/// found, and this says which it was rather than reporting the harmless case
/// as a failure.
pub fn unmute(runtime: &Runtime, session: &str) -> anyhow::Result<()> {
    let result = call(
        runtime,
        &serde_json::json!({
            "op": "unmute_session",
            "session": session,
        }),
    )?;
    match result.get("was_muted").and_then(serde_json::Value::as_bool) {
        Some(false) => println!("glasshouse: session `{session}` was not muted"),
        _ => println!("glasshouse: session `{session}` accepts orchestrator messages again"),
    }
    Ok(())
}

/// Show the recent terminal output of a live session in this project — map
/// line 745.
///
/// The door distinguishes four cases a client must not flatten: output
/// (written verbatim to stdout, nothing else there), no output yet (`ok`
/// with empty `output`, printed to stderr as success), not live (a
/// refusal, since `SessionApi::recent_output` treats an empty string as an
/// undetectable lie), and no such session (a refusal, passed through
/// unchanged — see [`call`]).
///
/// `max_bytes` stays optional: the door owns both the default and the
/// ceiling, and a client cannot enforce a ceiling it merely copied.
///
/// History: design-decisions.md, "Trims: api/client.rs", `fn read_output`.
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
    call_inner(runtime, request).map_err(anyhow::Error::from)
}

/// [`call`]'s body, with the errors it used to build inline kept as
/// [`super::CallError`] variants instead of being flattened into
/// `anyhow::Error` immediately — see [`send_machine_message`] for the
/// caller that needs the distinction and [`call`]'s own doc comment for why
/// its four existing callers do not need it and see nothing different.
fn call_inner(
    runtime: &Runtime,
    request: &serde_json::Value,
) -> Result<serde_json::Value, super::CallError> {
    let socket = socket_path_for(runtime);
    let stream = UnixStream::connect(&socket).map_err(|err| match err.kind() {
        std::io::ErrorKind::NotFound | std::io::ErrorKind::ConnectionRefused => {
            super::CallError::NotListening(
                "this project's control API is not listening; start it with `glasshouse api \
                 serve` in this project, and note that it can only reach sessions that same \
                 door started"
                    .to_owned(),
            )
        }
        std::io::ErrorKind::PermissionDenied => super::CallError::ConnectionRefused(
            "this project's control socket refused the connection; it is restricted to the \
             user that started `glasshouse api serve`"
                .to_owned(),
        ),
        _ => super::CallError::Other(anyhow!("could not reach this project's control API: {err}")),
    })?;
    // Bounded on both halves. A door that accepted the connection and then
    // stopped reading would otherwise wedge the write, which looks exactly
    // like a wedged read from the outside and is just as unhelpful.
    stream
        .set_write_timeout(Some(CALL_TIMEOUT))
        .map_err(|err| super::CallError::Other(err.into()))?;
    stream
        .set_read_timeout(Some(CALL_TIMEOUT))
        .map_err(|err| super::CallError::Other(err.into()))?;

    let mut writer = stream
        .try_clone()
        .map_err(|err| super::CallError::Other(err.into()))?;
    let mut payload = serde_json::to_string(request)
        .map_err(|err| super::CallError::Other(anyhow!("could not encode this request: {err}")))?;
    payload.push('\n');
    writer
        .write_all(payload.as_bytes())
        .map_err(|err| super::CallError::Other(timed_out(&err, "sending the request")))?;
    writer
        .flush()
        .map_err(|err| super::CallError::Other(timed_out(&err, "sending the request")))?;

    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    let read = reader
        .read_line(&mut line)
        .map_err(|err| super::CallError::Other(timed_out(&err, "waiting for the answer")))?;
    if read == 0 {
        return Err(super::CallError::Other(anyhow!(
            "this project's control API closed the connection without answering; check the \
             `glasshouse api serve` process"
        )));
    }

    let response: serde_json::Value = serde_json::from_str(line.trim_end()).map_err(|err| {
        super::CallError::Other(anyhow!(
            "this project's control API sent an answer this Glasshouse cannot read: {err}"
        ))
    })?;
    match response.get("status").and_then(serde_json::Value::as_str) {
        Some("ok") => Ok(response
            .get("result")
            .cloned()
            .unwrap_or(serde_json::Value::Null)),
        // The door's sentence, verbatim. See [`call`]'s doc comment.
        Some("error") => Err(super::CallError::DoorRefused(
            response
                .get("message")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("this project's control API refused the request and did not say why")
                .to_owned(),
        )),
        _ => Err(super::CallError::Other(anyhow!(
            "this project's control API sent an answer with no status this Glasshouse knows"
        ))),
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
