//! Phase 12, capability map line 701: "Deliver lifecycle events to the
//! orchestration layer without coupling orchestration to a specific
//! harness."
//!
//! The orchestration layer is the control API — see this door's own
//! `Request::Events` doc comment. This drives `glasshouse api serve` for
//! real over its Unix domain socket, the same harness shape
//! `capacity_api.rs` already uses.
//!
//! Seeding goes straight through `glasshouse::events::EventLog`, the same
//! producer this packet's own feasibility section names, rather than
//! through the hook CLI path a different package already covers — this file
//! is proving the *read* side of the door, the same way `capacity_api.rs`'s
//! own `write_project_config` seeds state directly rather than through the
//! settings UI that writes it in production.

#![cfg(unix)]

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use glasshouse::cli::Cli;
use glasshouse::events::{EventBus, EventLog, LifecycleEvent, Observation, TurnOutcome};
use glasshouse::session::SessionId;

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

/// Bootstrap the fixture's own runtime and append `events` directly to its
/// event log, each with the harness observation given alongside it. Returns
/// the log's head after appending. The runtime — and the database
/// connection it owns — is dropped before returning, so nothing holds the
/// file open when the server is started next.
fn seed_events(
    fixture: &Fixture,
    root: &Path,
    session: &SessionId,
    events: Vec<(LifecycleEvent, Option<Observation>)>,
) -> i64 {
    let cli = Cli {
        scope: Some(root.to_path_buf()),
        allow_unsafe_scope: false,
        data_dir: Some(fixture.base.join("data")),
        config_dir: Some(fixture.base.join("config")),
        log_level: None,
        log_file: None,
        log_stderr: false,
        command: None,
    };
    let runtime = glasshouse::bootstrap(&cli, root).expect("bootstrap the fixture runtime");
    let log = EventLog::open(&runtime).expect("open the event log");
    let bus = EventBus::new();
    for (event, observed) in events {
        let recorded = bus.publish(session, event);
        log.append(&recorded, observed.as_ref())
            .expect("append a fixture event");
    }
    log.head().expect("read the log's head")
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

/// Premise first (§17): a project with no events returns an empty list, not
/// an error, and `head` says the log is empty rather than being absent.
#[test]
fn a_project_with_no_events_returns_an_empty_list_not_an_error() {
    let fixture = Fixture::new();
    let root = fixture.project_root("alpha");
    let server = Server::start(&fixture, &root);

    let response = server.call(serde_json::json!({ "op": "events" }));
    assert_eq!(response["status"], "ok", "unexpected response: {response}");
    let events = response["result"]["events"]
        .as_array()
        .expect("an events array");
    assert!(events.is_empty(), "{response}");
    assert_eq!(response["result"]["head"], 0, "{response}");
}

/// Events recorded for a session come back with kind, session id and
/// timestamp — capability map line 701 — in Glasshouse's own vocabulary
/// rather than the harness's: `kind` is `session_started`/`turn_ended`, not
/// `SessionStart`/`Stop`, and the harness only appears as the `harness`
/// attribute.
#[test]
fn events_recorded_for_a_session_come_back_with_kind_session_and_timestamp() {
    let fixture = Fixture::new();
    let root = fixture.project_root("beta");
    let session = SessionId::new("session-1".to_owned());

    seed_events(
        &fixture,
        &root,
        &session,
        vec![
            (
                LifecycleEvent::SessionStarted,
                Some(Observation::new("claude-code", "SessionStart")),
            ),
            (
                LifecycleEvent::TurnEnded {
                    outcome: TurnOutcome::Completed,
                },
                Some(Observation::new("claude-code", "Stop")),
            ),
        ],
    );

    let server = Server::start(&fixture, &root);
    let response = server.call(serde_json::json!({ "op": "events" }));
    assert_eq!(response["status"], "ok", "unexpected response: {response}");
    let events = response["result"]["events"]
        .as_array()
        .expect("an events array");
    assert_eq!(events.len(), 2, "{events:?}");

    assert_eq!(events[0]["kind"], "session_started", "{events:?}");
    assert_eq!(events[0]["session"], "session-1", "{events:?}");
    assert!(events[0]["at"].is_i64(), "{events:?}");
    assert_eq!(events[0]["harness"], "claude-code", "{events:?}");

    assert_eq!(events[1]["kind"], "turn_ended", "{events:?}");
    assert_eq!(events[1]["outcome"], "completed", "{events:?}");
    assert_eq!(events[1]["session"], "session-1", "{events:?}");
}

/// The caller may ask for only what it has not seen: a bounded first call
/// returns the log's true `head` even though `limit` cut its own `events`
/// short, and a second call with `after` set to what the first call already
/// returned comes back with only the remainder.
#[test]
fn the_incremental_read_returns_only_what_the_caller_has_not_seen() {
    let fixture = Fixture::new();
    let root = fixture.project_root("gamma");
    let session = SessionId::new("session-1".to_owned());

    let observation = || Some(Observation::new("claude-code", "hook"));
    let head = seed_events(
        &fixture,
        &root,
        &session,
        vec![
            (LifecycleEvent::TurnStarted, observation()),
            (
                LifecycleEvent::TurnEnded {
                    outcome: TurnOutcome::Completed,
                },
                observation(),
            ),
            (LifecycleEvent::WaitingForUser, observation()),
        ],
    );
    assert_eq!(head, 3, "the fixture seeded three events");

    let server = Server::start(&fixture, &root);

    let first = server.call(serde_json::json!({ "op": "events", "limit": 1 }));
    assert_eq!(first["status"], "ok", "unexpected response: {first}");
    let first_events = first["result"]["events"]
        .as_array()
        .expect("an events array");
    assert_eq!(first_events.len(), 1, "{first_events:?}");
    assert_eq!(first_events[0]["kind"], "turn_started", "{first_events:?}");
    assert_eq!(
        first["result"]["head"], 3,
        "head reports the whole log, not just what `limit` returned: {first}"
    );
    let seen = first_events[0]["seq"].as_i64().expect("a seq number");

    let second = server.call(serde_json::json!({ "op": "events", "after": seen }));
    assert_eq!(second["status"], "ok", "unexpected response: {second}");
    let second_events = second["result"]["events"]
        .as_array()
        .expect("an events array");
    assert_eq!(second_events.len(), 2, "{second_events:?}");
    assert_eq!(second_events[0]["kind"], "turn_ended", "{second_events:?}");
    assert_eq!(
        second_events[1]["kind"], "waiting_for_user",
        "{second_events:?}"
    );
    assert!(
        second_events
            .iter()
            .all(|event| event["seq"].as_i64().expect("a seq number") > seen),
        "the second call must not repeat what the first one already returned: {second_events:?}"
    );
}

/// The negative, and it matters most: the harness's own raw word for an
/// event never crosses this door, only its name as an attribute. A response
/// that leaked it would fail this even though every other assertion in this
/// file could still pass.
#[test]
fn no_raw_harness_event_name_appears_in_any_response() {
    let fixture = Fixture::new();
    let root = fixture.project_root("delta");
    let session = SessionId::new("session-1".to_owned());

    seed_events(
        &fixture,
        &root,
        &session,
        vec![(
            LifecycleEvent::TurnStarted,
            Some(Observation::new(
                "claude-code",
                "RAW-HOOK-EVENT-SPELLING-must-not-leak",
            )),
        )],
    );

    let server = Server::start(&fixture, &root);
    let response = server.call(serde_json::json!({ "op": "events" }));
    assert_eq!(response["status"], "ok", "unexpected response: {response}");
    let rendered = serde_json::to_string(&response).expect("render the response");

    assert!(
        !rendered.contains("RAW-HOOK-EVENT-SPELLING-must-not-leak"),
        "the harness's raw event spelling must never cross this door: {rendered}"
    );
    // A positive control: the harness's *name* is expected to appear as an
    // attribute, so the negative above is not passing because the response
    // is empty or broken.
    assert!(
        rendered.contains("claude-code"),
        "the harness name should still appear as an attribute: {rendered}"
    );
}
