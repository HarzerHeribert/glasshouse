//! Phase 42, capability map line 1680: "allow the API to retrieve the
//! current routing-model selection and health."
//!
//! `mod api` is declared from `main.rs`, so — exactly as
//! `session_model.rs`'s own `control_api` module and `capacity_api.rs`
//! explain — nothing outside the binary can reach the control door any other
//! way. This drives `glasshouse api serve` for real over its Unix domain
//! socket, following `capacity_api.rs`'s own fixture shape: this file needs
//! no live harness process, only a project with no sessions.

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

    /// Write a project-level config file directly — this test proves the API
    /// reads it, not the settings UI that writes it, which belongs to a
    /// different package. Mirrors `capacity_api.rs`'s own
    /// `write_project_config`.
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
        Self::start_with_env(fixture, root, &[])
    }

    fn start_with_env(fixture: &Fixture, root: &Path, env: &[(&str, &str)]) -> Self {
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
        for (key, value) in env {
            command.env(key, value);
        }
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

/// A project with no recorded routing preference gets the honest default —
/// deterministic heuristics, `not_configured` — not an error and not a
/// fabricated pin. Box 1680, requirement 3.
#[test]
fn the_default_project_reports_its_default_selection_and_layer() {
    let fixture = Fixture::new();
    let root = fixture.project_root("alpha");
    let server = Server::start(&fixture, &root);

    let response = server.call(serde_json::json!({ "op": "routing_model" }));
    assert_eq!(response["status"], "ok", "unexpected response: {response}");

    let result = &response["result"];
    assert_eq!(result["selection"]["choice"], "deterministic", "{result}");
    assert!(result["selection"]["provider"].is_null(), "{result}");
    assert!(result["selection"]["model"].is_null(), "{result}");
    assert_eq!(result["layer"], "default", "{result}");
    assert_eq!(result["resolution"]["state"], "heuristics", "{result}");
    assert_eq!(result["resolution"]["reason"], "not_configured", "{result}");
}

/// A pinned routing model, naming a provider that is actually configured,
/// round-trips through the door with its provider and model intact, and
/// resolves rather than degrading. Box 1680, requirement 1 and 2.
#[test]
fn a_pinned_routing_model_round_trips_through_the_door() {
    let fixture = Fixture::new();
    let root = fixture.project_root("beta");
    fixture.write_project_config(
        &root,
        "version = 1\n\n\
         [providers.anyrouter]\n\
         template = \"anyrouter\"\n\n\
         [routing.model]\n\
         kind = \"pinned\"\n\
         provider = \"anyrouter\"\n\
         model = \"claude-opus\"\n",
    );
    let server = Server::start(&fixture, &root);

    let response = server.call(serde_json::json!({ "op": "routing_model" }));
    assert_eq!(response["status"], "ok", "unexpected response: {response}");

    let result = &response["result"];
    assert_eq!(result["selection"]["choice"], "pinned", "{result}");
    assert_eq!(result["selection"]["provider"], "anyrouter", "{result}");
    assert_eq!(result["selection"]["model"], "claude-opus", "{result}");
    assert_eq!(result["layer"], "project", "{result}");
    assert_eq!(result["resolution"]["state"], "pinned", "{result}");
    assert_eq!(result["resolution"]["provider"], "anyrouter", "{result}");
    assert_eq!(result["resolution"]["model"], "claude-opus", "{result}");
}

/// A pin naming a provider that is not configured degrades to heuristics
/// with the reason named in `RoutingFallback`'s own words — not an error and
/// not a silent success that pretends the pin still applies. Box 1680,
/// requirement 2.
#[test]
fn a_pin_naming_an_unconfigured_provider_degrades_to_heuristics_with_the_reason() {
    let fixture = Fixture::new();
    let root = fixture.project_root("gamma");
    fixture.write_project_config(
        &root,
        "version = 1\n\n\
         [routing.model]\n\
         kind = \"pinned\"\n\
         provider = \"ghost-provider\"\n\
         model = \"ghost-model\"\n",
    );
    let server = Server::start(&fixture, &root);

    let response = server.call(serde_json::json!({ "op": "routing_model" }));
    assert_eq!(response["status"], "ok", "unexpected response: {response}");

    let result = &response["result"];
    // The recorded choice is still reported honestly...
    assert_eq!(result["selection"]["choice"], "pinned", "{result}");
    assert_eq!(
        result["selection"]["provider"], "ghost-provider",
        "{result}"
    );
    assert_eq!(result["selection"]["model"], "ghost-model", "{result}");
    assert_eq!(result["layer"], "project", "{result}");
    // ...but the resolution says plainly that it cannot be honored.
    assert_eq!(result["resolution"]["state"], "heuristics", "{result}");
    assert_eq!(
        result["resolution"]["reason"], "provider_not_configured",
        "{result}"
    );
    assert_eq!(
        result["resolution"]["provider"], "ghost-provider",
        "{result}"
    );
    assert_eq!(result["resolution"]["model"], "ghost-model", "{result}");
}

/// A provider's credential lives behind an environment variable named in
/// `credential_env` — never a value this door reads or could echo back.
/// `RoutingModelChoice::Pinned` only ever carries a provider name and a
/// model name (see its own doc comment), so this asserts the negative
/// directly against the raw wire response rather than trusting the type by
/// inspection alone. Security invariant from the packet.
#[test]
fn no_credential_value_appears_in_the_routing_model_response() {
    let fixture = Fixture::new();
    let root = fixture.project_root("delta");
    fixture.write_project_config(
        &root,
        "version = 1\n\n\
         [providers.anyrouter]\n\
         template = \"anyrouter\"\n\
         credential_env = [\"ROUTING_API_TEST_SECRET\"]\n\n\
         [routing.model]\n\
         kind = \"pinned\"\n\
         provider = \"anyrouter\"\n\
         model = \"claude-opus\"\n",
    );
    const SECRET: &str = "sk-do-not-leak-BB6B6E9F3C9E4E39A9E9";
    let server = Server::start_with_env(&fixture, &root, &[("ROUTING_API_TEST_SECRET", SECRET)]);

    let response = server.call(serde_json::json!({ "op": "routing_model" }));
    assert_eq!(response["status"], "ok", "unexpected response: {response}");

    let raw = serde_json::to_string(&response).expect("serialize response");
    assert!(
        !raw.contains(SECRET),
        "the routing-model response must never carry a credential value: {raw}"
    );
}
