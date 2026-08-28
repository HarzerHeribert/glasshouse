//! Phase 21F lines 935/936, against the shipped binary and its real socket.
//!
//! `crate::api`'s own module doc comment (`src/api/mod.rs`) states this
//! door's proof requirement directly: "this module is proven only by running
//! the shipped binary... never by an in-process unit test, which is the
//! right proof for an external door anyway." `capacity_api.rs` already
//! follows that for `resource_capacity`; this file is the same shape for
//! `query_memory`.
//!
//! Not named in the packet's `EXPECTED FILES` — reported in `packet_errors`.
//! An in-process unit test on `api::unix::query_memory` would have
//! contradicted the architecture note above, so this file exists instead of
//! one.

#![cfg(unix)]

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use clap::Parser;

use glasshouse::memory::{
    DecisionProvenance, MemoryAuthority, MemoryKind, NewMemory, ProjectMemory, ReviewReason,
};
use glasshouse::{Cli, Runtime};

const TIMEOUT: Duration = Duration::from_secs(15);

/// A project with its own data and config roots — seeded in-process (see
/// `runtime`) before the real binary is spawned against the same files, the
/// combined pattern `tests/session_model.rs` already uses.
struct Fixture {
    _tmp: tempfile::TempDir,
    base: PathBuf,
    root: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let tmp = tempfile::tempdir().expect("tempdir");
        let base = tmp.path().to_path_buf();
        let root = base.join("workspace");
        std::fs::create_dir_all(root.join(".git")).expect("create project root");
        let root = std::fs::canonicalize(&root).expect("canonicalize project root");
        std::fs::create_dir_all(base.join("config")).expect("create config dir");
        Self {
            _tmp: tmp,
            base,
            root,
        }
    }

    fn runtime(&self) -> Runtime {
        let cli = Cli::try_parse_from([
            "glasshouse",
            "--data-dir",
            self.base.join("data").to_str().unwrap(),
            "--config-dir",
            self.base.join("config").to_str().unwrap(),
        ])
        .unwrap();
        glasshouse::bootstrap(&cli, &self.root).unwrap()
    }
}

struct Server {
    child: Child,
    socket: PathBuf,
}

impl Server {
    fn start(fixture: &Fixture) -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_glasshouse"))
            .arg("--scope")
            .arg(&fixture.root)
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

/// Acceptance test 4: the machine door carries a constraint's authority,
/// validity state, rationale and invalidation conditions as structured
/// fields, not only inside a rendered string.
#[test]
fn query_memory_carries_authority_validity_rationale_and_invalidation_conditions() {
    let fixture = Fixture::new();
    let runtime = fixture.runtime();
    let project = ProjectMemory::open(&runtime).unwrap();
    let store = project.store();
    store
        .record(
            NewMemory::new(
                MemoryKind::Constraint,
                "The heron export must never write partial files.",
            )
            .with_authority(Some(MemoryAuthority::Constraint))
            .with_provenance(DecisionProvenance {
                rationale: Some("a partial file was read by a downstream job once".to_owned()),
                ..DecisionProvenance::default()
            })
            .with_validity_conditions(Some("the export stays single-writer"))
            .with_invalidation_conditions(Some("the export gains concurrent writers")),
        )
        .unwrap();

    let server = Server::start(&fixture);
    let response = server.call(serde_json::json!({
        "op": "query_memory",
        "query": "heron",
        "history": false,
        "limit": 10,
    }));
    assert_eq!(response["status"], "ok", "{response}");

    let rules = response["result"]["invariants_and_constraints"]
        .as_array()
        .expect("an invariants_and_constraints array");
    assert_eq!(rules.len(), 1, "{response}");
    let entry = &rules[0];
    assert_eq!(entry["authority"], "constraint", "{entry}");
    assert_eq!(entry["status"], "active", "{entry}");
    assert_eq!(entry["current"], true, "{entry}");
    assert_eq!(entry["may_constrain_implementation"], true, "{entry}");
    assert_eq!(
        entry["rationale"], "a partial file was read by a downstream job once",
        "{entry}"
    );
    assert_eq!(
        entry["invalidation_conditions"], "the export gains concurrent writers",
        "{entry}"
    );
    assert_eq!(
        entry["validity_conditions"], "the export stays single-writer",
        "{entry}"
    );
}

/// A memory that is not binding does not carry validity/invalidation
/// conditions into the machine door even if the row happens to hold them —
/// 936's condition is explicit on authority, not on field presence.
#[test]
fn query_memory_does_not_carry_conditions_for_a_non_binding_memory() {
    let fixture = Fixture::new();
    let runtime = fixture.runtime();
    let project = ProjectMemory::open(&runtime).unwrap();
    let store = project.store();
    store
        .record(
            NewMemory::new(MemoryKind::Finding, "The ibis job could maybe run hourly.")
                .with_authority(Some(MemoryAuthority::Idea))
                .with_validity_conditions(Some("nobody has decided this yet")),
        )
        .unwrap();

    let server = Server::start(&fixture);
    let response = server.call(serde_json::json!({
        "op": "query_memory",
        "query": "ibis",
        "history": false,
        "limit": 10,
    }));
    assert_eq!(response["status"], "ok", "{response}");

    let other = response["result"]["other"]
        .as_array()
        .expect("an other array");
    assert_eq!(other.len(), 1, "{response}");
    let entry = &other[0];
    assert_eq!(entry["may_constrain_implementation"], false, "{entry}");
    assert!(entry["validity_conditions"].is_null(), "{entry}");
    assert!(entry["invalidation_conditions"].is_null(), "{entry}");
}

/// A memory that has been challenged is not presented as settled: its
/// status and reason reach the machine door, and it never appears under a
/// default (current) search.
#[test]
fn query_memory_reflects_a_challenged_memory_as_not_settled() {
    let fixture = Fixture::new();
    let runtime = fixture.runtime();
    let project = ProjectMemory::open(&runtime).unwrap();
    let store = project.store();
    let recorded = store
        .record(
            NewMemory::new(MemoryKind::Decision, "The osprey job runs hourly.")
                .with_authority(Some(MemoryAuthority::Decision)),
        )
        .unwrap();
    store
        .mark_for_review(&recorded.id, ReviewReason::ProductionIncident)
        .unwrap();

    let server = Server::start(&fixture);

    let historical = server.call(serde_json::json!({
        "op": "query_memory",
        "query": "osprey",
        "history": true,
        "limit": 10,
    }));
    assert_eq!(historical["status"], "ok", "{historical}");
    let other = historical["result"]["other"]
        .as_array()
        .expect("an other array");
    let entry = other
        .iter()
        .find(|entry| entry["id"].as_str() == Some(recorded.id.as_str()))
        .expect("the challenged memory must still be findable as history");
    assert_eq!(entry["status"], "needs_review", "{entry}");
    assert_eq!(entry["current"], false, "{entry}");
    assert_eq!(entry["review"]["reason"], "production_incident", "{entry}");

    let current_only = server.call(serde_json::json!({
        "op": "query_memory",
        "query": "osprey",
        "history": false,
        "limit": 10,
    }));
    assert_eq!(current_only["status"], "ok", "{current_only}");
    let rules = current_only["result"]["invariants_and_constraints"]
        .as_array()
        .unwrap();
    let other2 = current_only["result"]["other"].as_array().unwrap();
    assert!(
        rules
            .iter()
            .chain(other2.iter())
            .all(|entry| entry["id"].as_str() != Some(recorded.id.as_str())),
        "a challenged memory must not be returned as current: {current_only}"
    );
}
