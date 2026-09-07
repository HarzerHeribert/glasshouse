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
//!
//! # Capability map line 2479 — the inbox, beside the events it publishes
//!
//! The three tests at the end of this file are the door's other cursor:
//! `Request::SendMessage` to a session whose harness reads its input as a
//! batch stores the message instead of typing it, and `Request::Inbox` hands
//! it out. They live here rather than in a file of their own because the
//! thing line 2479 must not do is put a message body into the lifecycle
//! stream, and that is an assertion about `Request::Events`' answer — the
//! two verbs have to be driven through one door to say anything about each
//! other.

#![cfg(unix)]

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use glasshouse::cli::Cli;
use glasshouse::events::{EventBus, EventLog, LifecycleEvent, Observation, TurnOutcome};
use glasshouse::session::{NewSession, ProjectSessions, SessionId};

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

/// Record one session under `harness` in the fixture's own project and hand
/// back its identifier.
///
/// Through `ProjectSessions`, the store the door itself opens, rather than
/// through `Request::SpawnSession`: a pane session has no executable
/// installed in this fixture, and spawning one would be proving that a
/// harness starts rather than that a message reaches its inbox. The runtime
/// — and the database connection it owns — is dropped before returning, so
/// nothing holds the file open when the server starts.
fn seed_session(fixture: &Fixture, root: &Path, harness: &str) -> String {
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
    let sessions = ProjectSessions::open(&runtime).expect("open the session store");
    let record = sessions
        .store()
        .create(NewSession::embedded(harness))
        .expect("record the session");
    record.id.as_str().to_owned()
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

// ---------------------------------------------------------------------------
// Capability map line 2479 — the inbox
// ---------------------------------------------------------------------------

/// **Line 2479, the whole of this half in one test.** A message to a session
/// whose harness reads its input as a batch is *stored* rather than typed,
/// carries the sender the caller named, and shows up in the lifecycle stream
/// as a byte count with the text nowhere in the answer.
///
/// The negative is the load-bearing half and it is asserted over the whole
/// rendered `Request::Events` response, not over the fields this test happens
/// to know about: a future payload field that started carrying the body would
/// fail here even though every positive assertion still passed. That is the
/// shape `no_raw_harness_event_name_appears_in_any_response` above already
/// uses, for the same reason.
#[test]
fn a_message_to_a_pane_session_lands_in_its_inbox_with_the_sender_and_never_in_the_lifecycle_stream()
 {
    let fixture = Fixture::new();
    let root = fixture.project_root("inbox-alpha");
    let session = seed_session(&fixture, &root, "pane");
    let server = Server::start(&fixture, &root);

    const TEXT: &str = "INBOX-BODY-must-not-reach-the-lifecycle-stream";

    let sent = server.call(serde_json::json!({
        "op": "send_message",
        "session": session,
        "text": TEXT,
        "from": "orchestrator-1",
    }));
    assert_eq!(sent["status"], "ok", "unexpected response: {sent}");
    assert_eq!(
        sent["result"]["via"], "inbox",
        "the door must say the message was stored rather than typed: {sent}"
    );
    assert!(
        sent["result"]["seq"].as_i64().is_some_and(|seq| seq > 0),
        "a stored message must answer with its cursor position: {sent}"
    );

    let inbox = server.call(serde_json::json!({
        "op": "inbox",
        "session": session,
        "after": 0,
    }));
    assert_eq!(inbox["status"], "ok", "unexpected response: {inbox}");
    let messages = inbox["result"]["messages"]
        .as_array()
        .expect("a messages array");
    assert_eq!(messages.len(), 1, "{inbox}");
    assert_eq!(messages[0]["from"], "orchestrator-1", "{inbox}");
    assert_eq!(messages[0]["text"], TEXT, "{inbox}");
    assert!(messages[0]["at"].is_i64(), "{inbox}");
    assert_eq!(
        inbox["result"]["head"], messages[0]["seq"],
        "the cursor must be the last message's own position: {inbox}"
    );

    let events = server.call(serde_json::json!({ "op": "events" }));
    assert_eq!(events["status"], "ok", "unexpected response: {events}");
    let recorded = events["result"]["events"]
        .as_array()
        .expect("an events array");
    let delivered: Vec<_> = recorded
        .iter()
        .filter(|event| event["kind"] == "text_delivered")
        .collect();
    assert_eq!(
        delivered.len(),
        1,
        "storing a message must record exactly the delivery a typed one \
         records: {events}"
    );
    assert_eq!(delivered[0]["session"], session.as_str(), "{events}");
    assert_eq!(
        delivered[0]["bytes"],
        TEXT.len(),
        "the lifecycle stream carries the byte count: {events}"
    );

    let rendered = serde_json::to_string(&events).expect("render the response");
    assert!(
        !rendered.contains(TEXT),
        "no field of the lifecycle stream may carry the message body: {rendered}"
    );
    // A positive control, so the negative above is not passing on an empty
    // or broken response.
    assert!(
        rendered.contains("text_delivered"),
        "the delivery itself should still be visible: {rendered}"
    );
}

/// The other side of the branch, and the one this package must not have
/// changed: a session under any other harness takes today's path, refusal
/// and all, and nothing is stored for it.
///
/// A session recorded but held by no runtime is exactly what
/// `SessionApi::send_text` answers `NotLive` for, and with no
/// `presentation_ref` there is no pane to fall back to — so the refusal here
/// is the one this door has always given, arriving by the same route.
#[test]
fn a_message_to_a_non_pane_session_is_refused_as_not_live_exactly_as_before_and_its_inbox_stays_empty()
 {
    let fixture = Fixture::new();
    let root = fixture.project_root("inbox-beta");
    let session = seed_session(&fixture, &root, "claude-code");
    let server = Server::start(&fixture, &root);

    let sent = server.call(serde_json::json!({
        "op": "send_message",
        "session": session,
        "text": "a line for a terminal",
        "from": "orchestrator-1",
    }));
    assert_eq!(
        sent["status"], "error",
        "a session no runtime holds must still be refused: {sent}"
    );
    let message = sent["message"].as_str().unwrap_or_default();
    assert!(
        message.contains("is not live in this Glasshouse"),
        "and refused as not live, in the door's own words — the same sentence \
         `SessionApi::send_text` has always given: {message}"
    );

    let inbox = server.call(serde_json::json!({
        "op": "inbox",
        "session": session,
    }));
    assert_eq!(inbox["status"], "ok", "unexpected response: {inbox}");
    let messages = inbox["result"]["messages"]
        .as_array()
        .expect("a messages array");
    assert!(
        messages.is_empty(),
        "nothing may be stored for a harness that is written to by typing: {inbox}"
    );
    assert_eq!(
        inbox["result"]["head"], 0,
        "an empty inbox still answers with a cursor: {inbox}"
    );
}

/// The cursor: `after` excludes what was seen, `limit` is honoured, and
/// `head` comes back on a page that is short and on a page that is empty.
///
/// The third call is the one that matters most — a reader that has caught up
/// must still be told where it is, or it has no cursor to hand back and
/// starts from the beginning of the inbox next turn.
#[test]
fn an_inbox_cursor_hands_each_message_out_once_and_honours_the_page_limit() {
    let fixture = Fixture::new();
    let root = fixture.project_root("inbox-gamma");
    let session = seed_session(&fixture, &root, "pane");
    let server = Server::start(&fixture, &root);

    for text in ["one", "two", "three"] {
        let sent = server.call(serde_json::json!({
            "op": "send_message",
            "session": session,
            "text": text,
        }));
        assert_eq!(sent["status"], "ok", "unexpected response: {sent}");
    }

    let first = server.call(serde_json::json!({
        "op": "inbox",
        "session": session,
        "limit": 2,
    }));
    assert_eq!(first["status"], "ok", "unexpected response: {first}");
    let page = first["result"]["messages"]
        .as_array()
        .expect("a messages array");
    assert_eq!(page.len(), 2, "`limit` must bound the page: {first}");
    assert_eq!(page[0]["text"], "one", "{first}");
    assert_eq!(page[1]["text"], "two", "{first}");
    // A sender that stated nothing comes back as `null`, never as an empty
    // string: "nobody said" and "somebody said nothing" are different facts.
    assert!(page[0]["from"].is_null(), "{first}");
    let head = first["result"]["head"].as_i64().expect("a head");
    assert!(
        head > page[1]["seq"].as_i64().expect("a seq"),
        "head must report the whole inbox, not just what `limit` returned: {first}"
    );

    let second = server.call(serde_json::json!({
        "op": "inbox",
        "session": session,
        "after": page[1]["seq"],
    }));
    assert_eq!(second["status"], "ok", "unexpected response: {second}");
    let rest = second["result"]["messages"]
        .as_array()
        .expect("a messages array");
    assert_eq!(rest.len(), 1, "{second}");
    assert_eq!(rest[0]["text"], "three", "{second}");
    assert_eq!(second["result"]["head"], head, "{second}");

    let caught_up = server.call(serde_json::json!({
        "op": "inbox",
        "session": session,
        "after": head,
    }));
    assert_eq!(
        caught_up["status"], "ok",
        "unexpected response: {caught_up}"
    );
    assert!(
        caught_up["result"]["messages"]
            .as_array()
            .expect("a messages array")
            .is_empty(),
        "a reader that has caught up must be handed nothing again: {caught_up}"
    );
    assert_eq!(
        caught_up["result"]["head"], head,
        "and must still be told where it is: {caught_up}"
    );
}

/// A session this project does not have is refused by the same project-scope
/// error every other verb on this door gives it — never answered with an
/// empty inbox, which would be indistinguishable from a session that exists
/// and has had nothing sent to it (§54's family).
#[test]
fn an_inbox_for_a_session_this_project_does_not_have_is_refused_not_answered_empty() {
    let fixture = Fixture::new();
    let root = fixture.project_root("inbox-delta");
    let server = Server::start(&fixture, &root);

    let response = server.call(serde_json::json!({
        "op": "inbox",
        "session": "not-a-session-of-this-project",
    }));
    assert_eq!(
        response["status"], "error",
        "an unknown session must be refused: {response}"
    );
    assert!(
        response["message"]
            .as_str()
            .unwrap_or_default()
            .contains("not-a-session-of-this-project"),
        "and the refusal must name what was asked for: {response}"
    );
}
