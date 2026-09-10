//! The `inference-gateway` binary, driven the way Pane drives it.
//!
//! Every test here spawns the **built binary** and talks to it over the two
//! channels the contract names — one line of stdout, and stdin as the
//! shutdown signal. Nothing calls a library function to stand in for the
//! process, because the three facts under test are all facts about a
//! process: that the ready line is the first and only thing on stdout, that
//! a request carrying the announced token reaches the provider, and that
//! closing stdin ends it with status `0`.
//!
//! The provider is a loopback fixture in this test process that parses HTTP
//! itself. It re-uses no parser from the crate on purpose: a fixture built
//! on the production reader would agree with it about a request it had
//! mis-framed, and "the request arrived" would stop being a claim about the
//! wire.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// How long a test waits for the ready line, for the fixture to see a
/// request, or for the child to exit. Generous, because a loaded machine is
/// the normal case for this suite and a flake here costs a rerun.
const PATIENCE: Duration = Duration::from_secs(20);

/// One request as it actually arrived at the fixture.
#[derive(Debug, Clone)]
struct Recorded {
    request_line: String,
    headers: Vec<(String, String)>,
}

impl Recorded {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(header, _)| header.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }
}

/// A canned provider on loopback: an address to point the gateway at, and a
/// record of everything that reached it.
struct FakeProvider {
    address: SocketAddr,
    seen: Arc<Mutex<Vec<Recorded>>>,
    stop: Arc<AtomicBool>,
}

/// What the fixture answers every request with — a minimal Anthropic
/// Messages response, so a same-protocol forward has something well-formed
/// to carry back.
const CANNED_BODY: &str = r#"{"id":"msg_fixture","type":"message","role":"assistant","model":"fixture-model","content":[{"type":"text","text":"pong"}],"stop_reason":"end_turn","usage":{"input_tokens":1,"output_tokens":1}}"#;

impl FakeProvider {
    fn start() -> Self {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).expect("loopback is bindable");
        let address = listener
            .local_addr()
            .expect("a bound listener has an address");
        listener
            .set_nonblocking(true)
            .expect("a listener can be polled");
        let seen = Arc::new(Mutex::new(Vec::new()));
        let stop = Arc::new(AtomicBool::new(false));
        std::thread::spawn({
            let seen = Arc::clone(&seen);
            let stop = Arc::clone(&stop);
            move || {
                while !stop.load(Ordering::SeqCst) {
                    match listener.accept() {
                        Ok((stream, _)) => serve_one(stream, &seen),
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

    fn base_url(&self) -> String {
        format!("http://{}", self.address)
    }

    /// The requests seen so far, waiting up to [`PATIENCE`] for at least
    /// `count` of them.
    fn requests(&self, count: usize) -> Vec<Recorded> {
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

/// Read one request head, record it, drain its declared body, answer.
fn serve_one(mut stream: TcpStream, seen: &Arc<Mutex<Vec<Recorded>>>) {
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
        });
    let response = format!(
        "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: \
         close\r\n\r\n{CANNED_BODY}",
        CANNED_BODY.len()
    );
    let _ = stream.write_all(response.as_bytes());
    let _ = stream.flush();
    let _ = stream.shutdown(std::net::Shutdown::Write);
}

/// A raw HTTP POST, so what the gateway is asked is exactly what is written
/// here. Returns the status line and the body.
fn post(url: &str, authorization: &str, body: &str) -> (String, String) {
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
    let request = format!(
        "POST {path} HTTP/1.1\r\nhost: {authority}\r\nauthorization: {authorization}\r\n\
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
    let (head, body) = raw.split_once("\r\n\r\n").unwrap_or((raw.as_str(), ""));
    let status = head.lines().next().unwrap_or_default().to_owned();
    (status, body.to_owned())
}

/// The binary this crate builds, with a scratch config and data directory.
fn gateway(config: &std::path::Path, data_dir: &std::path::Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_inference-gateway"));
    command
        .arg("--config")
        .arg(config)
        .arg("--data-dir")
        .arg(data_dir);
    command
}

/// Wait up to [`PATIENCE`] for `child` to exit, and report its status.
fn wait_for_exit(child: &mut Child) -> std::process::ExitStatus {
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

/// The whole interprocess contract, end to end: one ready line, a request
/// through the announced address with the announced token reaching the
/// provider, and stdin closing ending it with `0`.
#[test]
fn serve_announces_one_line_forwards_with_it_and_exits_when_stdin_closes() {
    let provider = FakeProvider::start();
    let scratch = tempfile::tempdir().expect("a scratch directory");
    let config_path = scratch.path().join("gateway.toml");
    std::fs::write(
        &config_path,
        format!(
            r#"
[providers.fixture]
base_url = "{}"
protocol = "anthropic-messages"
credential_env = ["GATEWAY_BIN_TEST_KEY"]

[accounts.local]
kind = "api-key"
provider = "fixture"
credential = {{ env = "GATEWAY_BIN_TEST_KEY" }}
"#,
            provider.base_url()
        ),
    )
    .expect("the configuration is written");

    let mut child = gateway(&config_path, scratch.path())
        .args(["serve", "--listen", "127.0.0.1:0"])
        .env("GATEWAY_BIN_TEST_KEY", "fixture-provider-key")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("the built binary runs");

    let mut stdout = BufReader::new(child.stdout.take().expect("stdout was piped"));
    let mut ready = String::new();
    stdout.read_line(&mut ready).expect("a ready line arrives");
    let ready: serde_json::Value =
        serde_json::from_str(ready.trim()).expect("the ready line is one JSON object");
    let listening = ready["listening"]
        .as_str()
        .expect("`listening` is a string");
    let token = ready["token"].as_str().expect("`token` is a string");
    assert_eq!(
        ready.as_object().map(|object| object.len()),
        Some(2),
        "the ready line carries exactly `listening` and `token`: {ready}"
    );
    assert!(
        listening.starts_with("http://127.0.0.1:"),
        "the gateway announces a loopback URL: {listening}"
    );
    assert!(!token.is_empty(), "the gateway announces a token");

    let (status, body) = post(
        &format!("{listening}/v1/messages"),
        &format!("Bearer {token}"),
        r#"{"model":"fixture-model","max_tokens":1,"messages":[{"role":"user","content":"ping"}]}"#,
    );
    assert!(status.contains("200"), "the forward succeeded: {status}");
    assert!(
        body.contains("msg_fixture"),
        "the provider's own answer came back: {body}"
    );

    let seen = provider.requests(1);
    assert_eq!(seen.len(), 1, "the fixture saw exactly one request");
    assert!(
        seen[0].request_line.contains("/v1/messages"),
        "the request target was forwarded verbatim: {}",
        seen[0].request_line
    );
    assert_eq!(
        seen[0].header("authorization"),
        Some("Bearer fixture-provider-key"),
        "the gateway swapped its own token for the provider's credential"
    );

    // Closing stdin is the shutdown channel, and it is the whole of it.
    drop(child.stdin.take().expect("stdin was piped"));
    let status = wait_for_exit(&mut child);
    assert!(
        status.success(),
        "the gateway exits 0 when stdin reaches EOF, got {status:?}"
    );

    // ... and nothing followed the ready line on stdout.
    let mut trailing = String::new();
    stdout
        .read_to_string(&mut trailing)
        .expect("stdout can be drained");
    assert!(
        trailing.trim().is_empty(),
        "stdout carried more than the ready line: {trailing:?}"
    );
}

/// `entitlements --json` over a two-account catalogue: the documented keys,
/// sorted by account, and a subscription row that says which flow connects
/// it and that nothing has.
#[test]
fn entitlements_json_reports_the_documented_shape() {
    let scratch = tempfile::tempdir().expect("a scratch directory");
    let config_path = scratch.path().join("gateway.toml");
    std::fs::write(
        &config_path,
        r#"
[providers.fixture]
base_url = "http://127.0.0.1:1"
credential_env = ["GATEWAY_BIN_TEST_KEY"]

[accounts.zeta]
kind = "claude"
subscription_broker = "cliproxyapi"

[accounts.alpha]
kind = "api-key"
provider = "fixture"
credential = { env = "GATEWAY_BIN_TEST_KEY" }
"#,
    )
    .expect("the configuration is written");

    let output = gateway(&config_path, scratch.path())
        .args(["entitlements", "--json"])
        .output()
        .expect("the built binary runs");
    assert!(
        output.status.success(),
        "entitlements exits 0: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let document: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("one JSON document on stdout");
    assert_eq!(document["version"], 1);
    let accounts = document["accounts"]
        .as_array()
        .expect("`accounts` is an array");
    assert_eq!(accounts.len(), 2);
    assert_eq!(
        accounts[0]["account"], "alpha",
        "accounts are sorted by name"
    );
    assert_eq!(accounts[1]["account"], "zeta");

    for account in accounts {
        let object = account.as_object().expect("each account is an object");
        for key in [
            "account",
            "provider",
            "models",
            "scope",
            "selectable",
            "unavailable_reason",
            "authenticated",
            "connect_with",
        ] {
            assert!(object.contains_key(key), "`{key}` is present: {account}");
        }
        assert_eq!(account["selectable"], true);
        assert_eq!(account["unavailable_reason"], serde_json::Value::Null);
    }

    // The provider-backed row names its provider and claims no model,
    // because nothing has read a catalogue for it.
    assert_eq!(accounts[0]["provider"], "fixture");
    assert_eq!(accounts[0]["scope"], "unknown");
    assert_eq!(accounts[0]["models"], serde_json::json!([]));
    assert_eq!(accounts[0]["connect_with"], serde_json::Value::Null);
    assert_eq!(accounts[0]["authenticated"], serde_json::Value::Null);

    // The subscription row names the flow that would connect it, and says
    // out loud that nothing has. Its `provider` is the broker's own slug
    // because this account states no `vendor`: a stated vendor names it
    // instead, and that fallback order is the host's, copied.
    assert_eq!(accounts[1]["provider"], "cliproxyapi");
    assert_eq!(accounts[1]["connect_with"], "anthropic");
    assert_eq!(accounts[1]["authenticated"], false);
}

/// A `--config` naming a file that does not exist is an empty catalogue and
/// a note, not a crash.
#[test]
fn a_missing_config_is_an_empty_catalogue() {
    let scratch = tempfile::tempdir().expect("a scratch directory");
    let output = gateway(&scratch.path().join("no-such-file.toml"), scratch.path())
        .args(["entitlements", "--json"])
        .output()
        .expect("the built binary runs");

    assert!(
        output.status.success(),
        "a missing configuration is not a failure"
    );
    let document: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("one JSON document on stdout");
    assert_eq!(document["version"], 1);
    assert_eq!(document["accounts"], serde_json::json!([]));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("does not exist"),
        "the note says which file was missing: {stderr}"
    );
}

/// `--listen` is refused rather than quietly ignored when it names an
/// address the library cannot bind, and the refusal reaches stderr with
/// nothing on stdout — so a caller reading one line of stdout is never left
/// waiting on a process that has already given up.
#[test]
fn a_fixed_listen_port_is_refused_before_anything_is_bound() {
    let scratch = tempfile::tempdir().expect("a scratch directory");
    let output = gateway(&scratch.path().join("no-such-file.toml"), scratch.path())
        .args(["serve", "--listen", "127.0.0.1:8080"])
        .stdin(Stdio::null())
        .output()
        .expect("the built binary runs");

    assert!(
        !output.status.success(),
        "an address that cannot be honoured is a failure, not a warning"
    );
    assert!(
        output.stdout.is_empty(),
        "nothing reaches stdout when there is no ready line: {:?}",
        String::from_utf8_lossy(&output.stdout)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("127.0.0.1:8080") && stderr.contains("127.0.0.1:0"),
        "the refusal names what was asked for and what is accepted: {stderr}"
    );
}
