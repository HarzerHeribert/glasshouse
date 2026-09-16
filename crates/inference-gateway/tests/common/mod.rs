//! Shared black-box fixtures for the `inference-gateway` binary, used by
//! both `tests/bin.rs` and `tests/boundary.rs`.
//!
//! Moved out of `bin.rs` unchanged in behaviour: [`FakeProvider::start`]
//! still answers every request with [`CANNED_BODY`] over `200`, exactly as
//! it did before the move. [`FakeProvider::answering`] is new — it lets a
//! caller plant a specific status, headers and body, which `bin.rs` never
//! needed and `boundary.rs`'s provider-error and cross-protocol tests do.
//!
//! `bin.rs` and `boundary.rs` each pull this module in as their own copy —
//! that is how `tests/*.rs` integration binaries work — so a fixture only
//! one of the two calls is dead code from the other's point of view. Rather
//! than split the shared file in two, this is allowed crate-wide.
#![allow(dead_code)]

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::process::{Child, Command};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// How long a test waits for the ready line, for the fixture to see a
/// request, or for the child to exit. Generous, because a loaded machine is
/// the normal case for this suite and a flake here costs a rerun.
pub const PATIENCE: Duration = Duration::from_secs(20);

/// One request as it actually arrived at the fixture.
#[derive(Debug, Clone)]
pub struct Recorded {
    pub request_line: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Recorded {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(header, _)| header.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }
}

/// A canned provider on loopback: an address to point the gateway at, and a
/// record of everything that reached it. Answers every request the same way
/// — [`Self::start`]'s fixed `200`, or whatever [`Self::answering`] planted.
pub struct FakeProvider {
    address: SocketAddr,
    seen: Arc<Mutex<Vec<Recorded>>>,
    stop: Arc<AtomicBool>,
}

/// What [`FakeProvider::start`] answers every request with — a minimal
/// Anthropic Messages response, so a same-protocol forward has something
/// well-formed to carry back.
pub const CANNED_BODY: &str = r#"{"id":"msg_fixture","type":"message","role":"assistant","model":"fixture-model","content":[{"type":"text","text":"pong"}],"stop_reason":"end_turn","usage":{"input_tokens":1,"output_tokens":1}}"#;

impl FakeProvider {
    pub fn start() -> Self {
        Self::answering(
            "HTTP/1.1 200 OK",
            "content-type: application/json\r\n",
            CANNED_BODY,
        )
    }

    /// A fixture that answers every request with exactly `status_line`,
    /// `headers` (each already `\r\n`-terminated) and `body` — no
    /// content-length or connection header is implied by the caller; both
    /// are appended here so every planted response is well-formed.
    pub fn answering(status_line: &str, headers: &str, body: &str) -> Self {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).expect("loopback is bindable");
        let address = listener
            .local_addr()
            .expect("a bound listener has an address");
        listener
            .set_nonblocking(true)
            .expect("a listener can be polled");
        let seen = Arc::new(Mutex::new(Vec::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let response = format!(
            "{status_line}\r\n{headers}content-length: {}\r\nconnection: close\r\n\r\n{body}",
            body.len()
        );
        std::thread::spawn({
            let seen = Arc::clone(&seen);
            let stop = Arc::clone(&stop);
            move || {
                while !stop.load(Ordering::SeqCst) {
                    match listener.accept() {
                        Ok((stream, _)) => serve_one(stream, &seen, &response),
                        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                            std::thread::sleep(Duration::from_millis(10));
                        }
                        Err(_) => break,
                    }
                }
            }
        });
        Self {
            address,
            seen,
            stop,
        }
    }

    pub fn base_url(&self) -> String {
        format!("http://{}", self.address)
    }

    /// The requests seen so far, waiting up to [`PATIENCE`] for at least
    /// `count` of them.
    pub fn requests(&self, count: usize) -> Vec<Recorded> {
        let deadline = Instant::now() + PATIENCE;
        loop {
            let seen = self
                .seen
                .lock()
                .expect("the fixture's record is not poisoned");
            if seen.len() >= count || Instant::now() >= deadline {
                return seen.clone();
            }
            drop(seen);
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}

impl Drop for FakeProvider {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
    }
}

/// Read one request head, record it, drain its declared body, answer with
/// the response text `answering` (or `start`) already built.
fn serve_one(mut stream: TcpStream, seen: &Arc<Mutex<Vec<Recorded>>>, response: &str) {
    stream
        .set_nonblocking(false)
        .expect("an accepted stream can block");
    let mut reader = BufReader::new(stream.try_clone().expect("a stream clones"));
    let mut request_line = String::new();
    if reader.read_line(&mut request_line).is_err() {
        return;
    }
    let mut headers = Vec::new();
    let mut content_length = 0_usize;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).is_err() {
            return;
        }
        let line = line.trim_end_matches(['\r', '\n']);
        if line.is_empty() {
            break;
        }
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        let name = name.trim().to_ascii_lowercase();
        let value = value.trim().to_owned();
        if name == "content-length" {
            content_length = value.parse().unwrap_or(0);
        }
        headers.push((name, value));
    }
    let mut body = vec![0_u8; content_length];
    if content_length > 0 && reader.read_exact(&mut body).is_err() {
        return;
    }
    seen.lock()
        .expect("the fixture's record is not poisoned")
        .push(Recorded {
            request_line: request_line.trim_end().to_owned(),
            headers,
            body,
        });
    let _ = stream.write_all(response.as_bytes());
    let _ = stream.flush();
    let _ = stream.shutdown(std::net::Shutdown::Write);
}

/// A raw HTTP POST, so what the gateway is asked is exactly what is written
/// here. Returns the whole raw response text — status line, headers and
/// body — unparsed.
pub fn post_raw(url: &str, authorization: &str, body: &str) -> String {
    post_with_headers(url, authorization, &[], body)
}

/// [`post_raw`], with extra request headers spliced in after `authorization`
/// — the one thing a raw POST cannot do without picking apart the request
/// text.
pub fn post_with_headers(
    url: &str,
    authorization: &str,
    extra_headers: &[(&str, &str)],
    body: &str,
) -> String {
    let rest = url
        .strip_prefix("http://")
        .expect("the ready line announces an http URL");
    let (authority, path) = match rest.find('/') {
        Some(index) => (&rest[..index], &rest[index..]),
        None => (rest, "/"),
    };
    let mut stream = TcpStream::connect(authority).expect("the announced address accepts");
    stream
        .set_read_timeout(Some(PATIENCE))
        .expect("a timeout can be set");
    let mut extra = String::new();
    for (name, value) in extra_headers {
        extra.push_str(&format!("{name}: {value}\r\n"));
    }
    let request = format!(
        "POST {path} HTTP/1.1\r\nhost: {authority}\r\nauthorization: {authorization}\r\n{extra}\
         content-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
        body.len()
    );
    stream
        .write_all(request.as_bytes())
        .expect("the request is written");
    stream.flush().expect("the request is flushed");
    let mut raw = String::new();
    stream
        .read_to_string(&mut raw)
        .expect("the gateway answers and closes");
    raw
}

/// A raw HTTP POST, split into its status line and body. Headers are
/// discarded — [`post_raw`] is what a test that needs one reads instead.
pub fn post(url: &str, authorization: &str, body: &str) -> (String, String) {
    let raw = post_raw(url, authorization, body);
    let (head, body) = raw.split_once("\r\n\r\n").unwrap_or((raw.as_str(), ""));
    let status = head.lines().next().unwrap_or_default().to_owned();
    (status, body.to_owned())
}

/// The binary this crate builds, with a scratch config and data directory.
pub fn gateway(config: &std::path::Path, data_dir: &std::path::Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_inference-gateway"));
    command
        .arg("--config")
        .arg(config)
        .arg("--data-dir")
        .arg(data_dir);
    command
}

/// Wait up to [`PATIENCE`] for `child` to exit, and report its status.
pub fn wait_for_exit(child: &mut Child) -> std::process::ExitStatus {
    let deadline = Instant::now() + PATIENCE;
    loop {
        match child.try_wait().expect("a spawned child can be polled") {
            Some(status) => return status,
            None if Instant::now() >= deadline => {
                let _ = child.kill();
                panic!("the gateway did not exit within {PATIENCE:?} of stdin closing");
            }
            None => std::thread::sleep(Duration::from_millis(20)),
        }
    }
}
