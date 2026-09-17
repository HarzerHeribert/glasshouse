//! Phase 42, capability map line 1680, used to be: "allow the API to
//! retrieve the current routing-model selection and health." The
//! `routing_model` control-API operation this file drove is gone with the
//! ranking it reported on (design-decisions.md, 2026-09-16, "Glasshouse
//! never decides which model is used") — `Request` no longer has a
//! `RoutingModel` variant, and `api/unix/mod.rs` no longer has
//! `routing_model_status` to answer one. `the_recommend_route_method_is_no_longer_known`
//! is what remains: a caller that still sends a routing-era `op` (here,
//! the older `recommend_route`; `routing_model` gets the identical answer
//! for the identical reason) gets the door's ordinary unknown-request error,
//! never a special case.
//!
//! `mod api` is declared from `main.rs`, so — exactly as
//! `session_model.rs`'s own `control_api` module and `capacity_api.rs`
//! explain — nothing outside the binary can reach the control door any other
//! way. This drives `glasshouse api serve` for real over its Unix domain
//! socket, following `capacity_api.rs`'s own fixture shape.

#![cfg(unix)]

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

const TIMEOUT: Duration = Duration::from_secs(15);

struct Fixture {
    _tmp: tempfile::TempDir,
    base: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let tmp = tempfile::tempdir().expect("tempdir");
        let base = tmp.path().to_path_buf();
        std::fs::create_dir_all(base.join("config")).expect("create config dir");
        Self { _tmp: tmp, base }
    }

    fn project_root(&self, name: &str) -> PathBuf {
        let root = self.base.join("workspace").join(name);
        std::fs::create_dir_all(root.join(".git")).expect("create project root");
        std::fs::canonicalize(&root).expect("canonicalize project root")
    }
}

struct Server {
    child: Child,
    socket: PathBuf,
}

impl Server {
    fn start(fixture: &Fixture, root: &Path) -> Self {
        let mut command = Command::new(env!("CARGO_BIN_EXE_glasshouse"));
        command
            .arg("--scope")
            .arg(root)
            .arg("--data-dir")
            .arg(fixture.base.join("data"))
            .arg("--config-dir")
            .arg(fixture.base.join("config"))
            .arg("api")
            .arg("serve")
            .stderr(Stdio::piped());
        let mut child = command.spawn().expect("spawn `glasshouse api serve`");

        let stderr = child.stderr.take().expect("captured stderr");
        let mut reader = BufReader::new(stderr);
        let deadline = Instant::now() + TIMEOUT;
        let socket = loop {
            let mut line = String::new();
            let read = reader.read_line(&mut line).expect("read server stderr");
            assert!(read > 0, "the server exited before announcing its socket");
            if let Some(path) = line
                .trim_end()
                .strip_prefix("glasshouse: control API listening on ")
            {
                break PathBuf::from(path);
            }
            assert!(
                Instant::now() < deadline,
                "timed out waiting for the server to announce its socket"
            );
        };

        Self { child, socket }
    }

    fn call(&self, request: serde_json::Value) -> serde_json::Value {
        let deadline = Instant::now() + TIMEOUT;
        let mut stream = loop {
            match UnixStream::connect(&self.socket) {
                Ok(stream) => break stream,
                Err(err) => {
                    assert!(
                        Instant::now() < deadline,
                        "timed out connecting to the control socket: {err}"
                    );
                    std::thread::sleep(Duration::from_millis(20));
                }
            }
        };
        let mut payload = serde_json::to_string(&request).expect("encode request");
        payload.push('\n');
        stream.write_all(payload.as_bytes()).expect("write request");

        let mut reader = BufReader::new(stream);
        let mut line = String::new();
        reader.read_line(&mut line).expect("read response");
        serde_json::from_str(line.trim_end()).expect("parse response")
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Line 1681's verb is gone with the ranking it reported on
/// (design-decisions.md, 2026-09-16, "Glasshouse never decides which model
/// is used"). A caller that still sends `recommend_route` gets the same
/// answer the door gives any `op` it does not recognise — `Request`'s
/// `#[serde(tag = "op")]` fails to deserialize an unknown variant, and
/// `handle_connection` turns that parse error into `status: error` rather
/// than dispatching it.
#[test]
fn the_recommend_route_method_is_no_longer_known() {
    let fixture = Fixture::new();
    let root = fixture.project_root("epsilon");
    let server = Server::start(&fixture, &root);

    let response = server.call(serde_json::json!({ "op": "recommend_route" }));
    assert_eq!(
        response["status"], "error",
        "unexpected response: {response}"
    );
    let message = response["message"].as_str().unwrap_or_default();
    assert!(
        message.contains("malformed request"),
        "expected the door's ordinary unknown-request error, got: {message}"
    );
}
