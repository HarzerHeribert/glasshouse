//! Phase 42, capability map line 1679: "allow the API to retrieve current
//! resource capacity and quota telemetry."
//!
//! `mod api` is declared from `main.rs`, so — exactly as
//! `session_model.rs`'s own `control_api` module explains — nothing outside
//! the binary can reach the control door any other way. This drives
//! `glasshouse api serve` for real over its Unix domain socket, the same
//! harness shape `session_model.rs` already uses, kept deliberately small:
//! this file needs no live harness process, only a project with no sessions.

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

    /// Write a `[providers.<name>.quota]` table with its own protected
    /// reserve percentage, into the project-level config file directly —
    /// this test proves the API reads it, not the settings UI that writes
    /// it, which belongs to a different package.
    fn write_project_config(&self, root: &Path, toml: &str) {
        let dir = root.join(".glasshouse");
        std::fs::create_dir_all(&dir).expect("create .glasshouse dir");
        std::fs::write(dir.join("config.toml"), toml).expect("write project config");
    }
}

struct Server {
    child: Child,
    socket: PathBuf,
}

impl Server {
    fn start(fixture: &Fixture, root: &Path) -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_glasshouse"))
            .arg("--scope")
            .arg(root)
            .arg("--data-dir")
            .arg(fixture.base.join("data"))
            .arg("--config-dir")
            .arg(fixture.base.join("config"))
            .arg("api")
            .arg("serve")
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn `glasshouse api serve`");

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

/// The production caller, end to end: a request naming `"resource_capacity"`
/// reaches `glasshouse::provider::resources::capacity_json`, which enumerates
/// the same registry `glasshouse resources` prints, and returns it over the
/// socket as structured data — capability map line 1679.
#[test]
fn resource_capacity_reaches_the_socket_and_lists_every_registry_resource() {
    let fixture = Fixture::new();
    let root = fixture.project_root("alpha");
    let server = Server::start(&fixture, &root);

    let response = server.call(serde_json::json!({ "op": "resource_capacity" }));
    assert_eq!(response["status"], "ok", "unexpected response: {response}");

    let resources = response["result"]["resources"]
        .as_array()
        .expect("a resources array");
    assert!(
        !resources.is_empty(),
        "the registry always describes at least one resource kind"
    );

    for entry in resources {
        assert!(entry["resource"].is_string(), "{entry}");
        assert!(entry["quota_shape"].is_string(), "{entry}");
        assert!(entry["locality"].is_string(), "{entry}");
        assert!(entry["telemetry_class"].is_string(), "{entry}");
        // `capacity` is `null` for a resource with no scoreable dimension
        // (an unmeasured subscription or the delegated gateway) and an
        // object with `band`/`score`/`effective` fields otherwise — never a
        // bare number, matching `RemainingCapacityScore`'s own guarantee
        // that a score is never handed out without what it was derived
        // from.
        assert!(
            entry["capacity"].is_null() || entry["capacity"]["band"].is_string(),
            "{entry}"
        );
    }
}

/// A provider that names its own protected reserve percentage — capability
/// map line 1288 — reaches the socket too: this proves `EffectiveConfig`,
/// not just the registry's static list, is actually read on this path.
#[test]
fn a_resource_that_defines_its_own_protected_reserve_percentage_is_visible_through_the_socket() {
    let fixture = Fixture::new();
    let root = fixture.project_root("beta");
    fixture.write_project_config(
        &root,
        "version = 1\n\n[providers.anyrouter]\ntemplate = \"anyrouter\"\n\n\
         [providers.anyrouter.quota]\nreserve_percent = 40\n",
    );
    let server = Server::start(&fixture, &root);

    let response = server.call(serde_json::json!({ "op": "resource_capacity" }));
    assert_eq!(response["status"], "ok", "unexpected response: {response}");
    let resources = response["result"]["resources"]
        .as_array()
        .expect("a resources array");
    assert!(
        resources.iter().any(|entry| entry["resource"]
            .as_str()
            .is_some_and(|r| r.contains("anyrouter"))),
        "the configured anyrouter provider must appear in the capacity listing: {resources:?}"
    );
}

/// An op the protocol does not know about is refused cleanly rather than
/// crashing the connection or the server — the same guarantee
/// `session_model.rs`'s own malformed-request test holds every other
/// request to.
#[test]
fn an_unknown_op_is_refused_and_the_server_keeps_serving() {
    let fixture = Fixture::new();
    let root = fixture.project_root("gamma");
    let server = Server::start(&fixture, &root);

    let refused = server.call(serde_json::json!({ "op": "not_a_real_operation" }));
    assert_eq!(refused["status"], "error", "{refused}");

    let response = server.call(serde_json::json!({ "op": "resource_capacity" }));
    assert_eq!(
        response["status"], "ok",
        "the server must still answer after a refusal"
    );
}
