//! The project control door is the only messaging transport. Discovery uses
//! the public binary verb, never Glasshouse's private state-path rules.
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::contract::SessionId;
use crate::glasshouse::Glasshouse;

pub const MESSAGE_BYTES: usize = 65_536;
pub const PAGE_SIZE: usize = 32;

/// Body-bearing data deliberately has no Debug implementation.
#[derive(Clone, Deserialize)]
pub struct Message {
    pub seq: i64,
    pub from: Option<String>,
    pub text: String,
    pub at: i64,
}

#[derive(Deserialize)]
struct Page {
    messages: Vec<Message>,
}

/// One cursor per Pane session, surviving ordinary tasks and door restarts.
/// An unavailable door never resets it. Only accepted rows advance it.
pub struct Inbox {
    socket: Option<PathBuf>,
    after: i64,
}

impl Inbox {
    pub fn discover(glasshouse: &Glasshouse, root: &Path) -> Self {
        Self {
            socket: socket_path(glasshouse, root),
            after: 0,
        }
    }

    pub fn poll(&mut self, session: &SessionId) -> Vec<Message> {
        self.poll_limit(session, PAGE_SIZE)
    }

    fn poll_limit(&mut self, session: &SessionId, limit: usize) -> Vec<Message> {
        let Some(socket) = &self.socket else {
            return Vec::new();
        };
        let request = serde_json::json!({
            "op": "inbox", "session": session.as_str(),
            "after": self.after, "limit": limit,
        });
        let Ok(result) = exchange(socket, &request) else {
            return Vec::new();
        };
        let Ok(page) = serde_json::from_value::<Page>(result) else {
            return Vec::new();
        };
        // Validate the whole page before advancing. Never use the full head:
        // on a limited page it includes rows we have not read yet.
        let mut last = self.after;
        for message in &page.messages {
            if message.seq <= last || page.messages.len() > limit {
                return Vec::new();
            }
            last = message.seq;
        }
        self.after = last;
        page.messages
    }

    /// Drain bounded pages into the existing window. The cursor and all
    /// spillover belong to the session, including between separate tasks.
    pub fn drain_into(
        &mut self,
        session: &SessionId,
        window: &mut super::window::Window,
        payloads: &mut std::collections::HashMap<String, Message>,
    ) {
        let started = std::time::Instant::now();
        let mut remaining = 1_000;
        while remaining > 0 && started.elapsed() < std::time::Duration::from_secs(2) {
            let limit = remaining.min(PAGE_SIZE);
            let messages = self.poll_limit(session, limit);
            let count = messages.len();
            for message in messages {
                let id = format!("message/{}", message.seq);
                let at = super::now();
                let event = super::Event::pending(
                    super::Kind::Message {
                        message_id: message.seq.to_string(),
                    },
                    format!("session/{}", message.from.as_deref().unwrap_or("unknown")),
                    at,
                    super::PayloadRef::new(&id),
                    super::Priority::Batch,
                    "message received",
                );
                payloads.insert(id, message);
                window.accept(event, at);
            }
            remaining -= count;
            if count < limit {
                break;
            }
        }
    }
}

pub fn validate(session: &str, message: &str) -> Result<(), &'static str> {
    if session.is_empty() || session.len() > 256 || session.chars().any(char::is_control) {
        return Err("send requires a session id of 1–256 bytes without control characters");
    }
    if message.is_empty() || message.len() > MESSAGE_BYTES {
        return Err("send requires a message of 1–65536 UTF-8 bytes");
    }
    Ok(())
}

pub fn send(
    glasshouse: &Glasshouse,
    root: &Path,
    from: &SessionId,
    session: &str,
    message: &str,
) -> Result<(), &'static str> {
    send_unless_cancelled(glasshouse, root, from, session, message, || false)
}

pub(crate) fn send_unless_cancelled(
    glasshouse: &Glasshouse,
    root: &Path,
    from: &SessionId,
    session: &str,
    message: &str,
    cancelled: impl Fn() -> bool,
) -> Result<(), &'static str> {
    validate(session, message)?;
    let socket =
        socket_path(glasshouse, root).ok_or("MessagingUnavailable: no project control socket")?;
    if cancelled() {
        return Err("Cancelled: message was not sent");
    }
    exchange(
        &socket,
        &serde_json::json!({
            "op": "send_message", "session": session, "text": message,
            "from": from.as_str(), "origin": "machine",
        }),
    )?;
    Ok(())
}

#[cfg(unix)]
fn socket_path(glasshouse: &Glasshouse, root: &Path) -> Option<PathBuf> {
    use std::io::Read;
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};
    let Glasshouse::Command { glasshouse } = glasshouse else {
        return None;
    };
    let mut command = Command::new(glasshouse);
    command
        .args(["api", "socket-path"])
        .current_dir(root)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    for (name, _) in std::env::vars_os() {
        if crate::tools::invoke::is_credential_variable(&name.to_string_lossy()) {
            command.env_remove(name);
        }
    }
    let mut child = command.spawn().ok()?;
    let started = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if started.elapsed() < Duration::from_secs(2) => {
                std::thread::sleep(Duration::from_millis(10));
            }
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    };
    if !status.success() {
        return None;
    }
    let mut bytes = Vec::new();
    child
        .stdout
        .take()?
        .take(4097)
        .read_to_end(&mut bytes)
        .ok()?;
    if bytes.len() > 4096 {
        return None;
    }
    let text = String::from_utf8(bytes).ok()?;
    let path = text.strip_suffix('\n').unwrap_or(&text);
    if path.contains(['\n', '\r', '\0']) || !Path::new(path).is_absolute() {
        return None;
    }
    Some(PathBuf::from(path))
}

#[cfg(not(unix))]
fn socket_path(_: &Glasshouse, _: &Path) -> Option<PathBuf> {
    None
}

/// One request, one response, no retry. Remote errors are never reflected:
/// a peer can include message content in an error just as in a result.
#[cfg(unix)]
fn exchange(socket: &Path, request: &serde_json::Value) -> Result<serde_json::Value, &'static str> {
    use std::io::{BufRead, BufReader, Read, Write};
    use std::os::unix::net::UnixStream;
    use std::time::Duration;
    const RESPONSE_BYTES: u64 = 16 * 1024 * 1024;
    let mut stream = UnixStream::connect(socket)
        .map_err(|_| "MessagingUnavailable: control socket refused connection")?;
    stream
        .set_read_timeout(Some(Duration::from_millis(500)))
        .map_err(|_| "MessagingUnavailable: socket timeout unavailable")?;
    stream
        .set_write_timeout(Some(Duration::from_millis(500)))
        .map_err(|_| "MessagingUnavailable: socket timeout unavailable")?;
    let mut bytes =
        serde_json::to_vec(request).map_err(|_| "MessagingUnavailable: invalid request")?;
    bytes.push(b'\n');
    stream
        .write_all(&bytes)
        .map_err(|_| "DeliveryUnknown: request interrupted; not retried")?;
    let mut response = Vec::new();
    BufReader::new(stream.take(RESPONSE_BYTES + 1))
        .read_until(b'\n', &mut response)
        .map_err(|_| "DeliveryUnknown: response unavailable; not retried")?;
    if response.len() as u64 > RESPONSE_BYTES || response.last() != Some(&b'\n') {
        return Err("DeliveryUnknown: incomplete response; not retried");
    }
    let value: serde_json::Value = serde_json::from_slice(&response)
        .map_err(|_| "DeliveryUnknown: invalid response; not retried")?;
    match value.get("status").and_then(serde_json::Value::as_str) {
        Some("ok") => value
            .get("result")
            .cloned()
            .ok_or("DeliveryUnknown: missing result; not retried"),
        Some("error") => Err("MessageRejected: project control door refused the message"),
        _ => Err("DeliveryUnknown: invalid status; not retried"),
    }
}

#[cfg(not(unix))]
fn exchange(_: &Path, _: &serde_json::Value) -> Result<serde_json::Value, &'static str> {
    Err("MessagingUnavailable: project control sockets are unsupported on this platform")
}
