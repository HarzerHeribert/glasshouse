//! The two Phase 21 extraction triggers, driven through the **built binary**,
//! against a model that really answers.
//!
//! # Why this spawns a process and stands up a socket
//!
//! `docs/product/evidence/phase-21.md` set the criterion these tests exist to
//! meet, after the same lines were argued about and left open: *"The test is
//! not whether a model is called. It is whether the capability completes and
//! produces its result in the shipped binary."* Until this batch the honest
//! answer for *run after task completion* was "it tries, every time, and
//! reports it has no model" — the trigger was wired and dead-ended, because
//! nothing could supply the model half.
//!
//! So a unit test with a fake `ExtractionModel` proves the wrong thing here.
//! What has to be shown is that `glasshouse hook`, spawned the way a harness
//! spawns it, reads the user's own configuration, calls the model that
//! configuration names, and leaves a memory in this project's store. Every
//! test below therefore uses the real binary, the real config files, a real
//! socket, and no seam of any kind.
//!
//! The model is a canned HTTP server on loopback rather than a provider: a
//! test that called one would spend a credential, fail when the network did,
//! and could not assert what the model was actually sent. It parses the
//! request itself rather than reusing anything in this crate, so "the request
//! arrived" is a claim about the wire.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use glasshouse::config::{ExtractionModelRef, ProviderConfig, UserConfig};
use glasshouse::memory::ProjectMemory;
use glasshouse::memory::search::SearchScope;
use glasshouse::session::{NewSession, ProjectSessions, SessionId, SessionLifecycle};
use glasshouse::{Cli, Runtime};

use clap::Parser;

/// The variable the fixture provider's credential is read from. Named once so
/// the assertion that it reached the wire cannot drift from what was set.
const CREDENTIAL_VAR: &str = "GLASSHOUSE_TEST_ONLY_EXTRACTION_MODEL_KEY";
const CREDENTIAL: &str = "sk-fabricated-test-value-not-a-real-credential";
const MODEL: &str = "a-cheap-local-model";
const PROVIDER: &str = "extraction-test-runner";

/// What a cheap model would answer: one finding, in the extraction contract's
/// own shape. Deliberately a body no other test in this repository stores, so
/// a search that finds it found this.
const ONE_FINDING: &str = r#"{"memories":[{"kind":"finding","authority":"constraint",
     "disposition":"accepted","support":"established","confidence":"certain",
     "rationale":"the hook process is the only thing that sees a turn end",
     "project_phase":"alpha",
     "body":"A configured extraction model answered over loopback."}]}"#;

// ---------------------------------------------------------------------------
// A canned OpenAI chat-completions endpoint.
// ---------------------------------------------------------------------------

/// One request as it actually arrived on the wire.
#[derive(Debug, Clone)]
struct Seen {
    method: String,
    target: String,
    /// Header names lower-cased; values exactly as received.
    headers: Vec<(String, String)>,
    body: String,
}

impl Seen {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(header, _)| header == name)
            .map(|(_, value)| value.as_str())
    }
}

/// A model that answers `content` to every request, and remembers what it was
/// asked.
struct FakeModel {
    address: SocketAddr,
    seen: Arc<Mutex<Vec<Seen>>>,
    stop: Arc<AtomicBool>,
}

impl FakeModel {
    fn answering(content: &str) -> Self {
        Self::start(Some(content.to_owned()))
    }

    /// A model that accepts a connection and answers `500`. Used to prove a
    /// failing model still costs the session nothing.
    fn failing() -> Self {
        Self::start(None)
    }

    fn start(content: Option<String>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("loopback must bind");
        listener
            .set_nonblocking(true)
            .expect("the accept loop polls its stop flag");
        let address = listener.local_addr().unwrap();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let stop = Arc::new(AtomicBool::new(false));

        let thread_seen = Arc::clone(&seen);
        let thread_stop = Arc::clone(&stop);
        std::thread::spawn(move || {
            while !thread_stop.load(Ordering::SeqCst) {
                match listener.accept() {
                    Ok((stream, _)) => {
                        let _ = stream.set_nonblocking(false);
                        serve(stream, &thread_seen, content.as_deref());
                    }
                    Err(ref err) if err.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(std::time::Duration::from_millis(5));
                    }
                    Err(_) => break,
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
        format!("http://{}/v1", self.address)
    }

    fn requests(&self) -> Vec<Seen> {
        self.seen.lock().unwrap().clone()
    }
}

impl Drop for FakeModel {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
    }
}

/// Read one request head byte-oriented, find `content-length` without help,
/// read exactly that many bytes, and answer.
fn serve(mut stream: TcpStream, seen: &Arc<Mutex<Vec<Seen>>>, content: Option<&str>) {
    let mut reader = BufReader::new(stream.try_clone().expect("the stream clones"));
    let mut request_line = String::new();
    if reader.read_line(&mut request_line).is_err() || request_line.is_empty() {
        return;
    }
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or_default().to_owned();
    let target = parts.next().unwrap_or_default().to_owned();

    let mut headers = Vec::new();
    let mut length = 0usize;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).is_err() {
            return;
        }
        let line = line.trim_end_matches(['\r', '\n']);
        if line.is_empty() {
            break;
        }
        if let Some((name, value)) = line.split_once(':') {
            let name = name.trim().to_ascii_lowercase();
            let value = value.trim().to_owned();
            if name == "content-length" {
                length = value.parse().unwrap_or(0);
            }
            headers.push((name, value));
        }
    }

    let mut body = vec![0u8; length];
    if reader.read_exact(&mut body).is_err() {
        return;
    }
    seen.lock().unwrap().push(Seen {
        method,
        target,
        headers,
        body: String::from_utf8_lossy(&body).into_owned(),
    });

    let response = match content {
        Some(content) => {
            let document = serde_json::json!({
                "choices": [{ "message": { "role": "assistant", "content": content } }]
            })
            .to_string();
            format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\
                 connection: close\r\n\r\n{document}",
                document.len()
            )
        }
        None => "HTTP/1.1 500 Internal Server Error\r\ncontent-length: 0\r\n\
                 connection: close\r\n\r\n"
            .to_owned(),
    };
    let _ = stream.write_all(response.as_bytes());
    let _ = stream.flush();
}

// ---------------------------------------------------------------------------
// A project, and the binary run against it.
// ---------------------------------------------------------------------------

struct Fixture {
    _tmp: tempfile::TempDir,
    base: PathBuf,
    root: PathBuf,
    runtime: Runtime,
}

impl Fixture {
    fn new() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path().to_path_buf();
        let root = base.join("workspace");
        std::fs::create_dir_all(root.join(".git")).unwrap();
        let root = std::fs::canonicalize(&root).unwrap();
        let runtime = bootstrap(&base, &root);
        Self {
            _tmp: tmp,
            base,
            root,
            runtime,
        }
    }

    /// Write the configuration a user writes to choose a cheap or local model:
    /// a provider pointing at the runner, and the model by name.
    ///
    /// `credential_env` empty is the **local** case — a runner on loopback
    /// needs no key, and Glasshouse must not require one.
    fn choose_model(&self, base_url: &str, with_credential: bool) {
        let mut user = UserConfig::load(self.runtime.paths()).unwrap();
        let mut provider = ProviderConfig::new("openai-compatible");
        provider.set_base_url(Some(base_url.to_owned()));
        if with_credential {
            provider.set_credential_env(vec![CREDENTIAL_VAR.to_owned()]);
        }
        user.providers_mut().set(PROVIDER, provider);
        user.set_memory_extraction_model(Some(ExtractionModelRef::new(PROVIDER, MODEL)));
        user.save(self.runtime.paths()).unwrap();
    }

    /// Run `glasshouse hook`, exactly as a harness runs it, and return its
    /// exit status.
    fn hook(&self, session: &SessionId, event: &str) -> std::process::ExitStatus {
        self.hook_logging(session, event, None)
    }

    /// [`Fixture::hook`] with debug logging sent to a file, so a test can read
    /// what the handler actually wrote.
    fn hook_logging(
        &self,
        session: &SessionId,
        event: &str,
        log_file: Option<&Path>,
    ) -> std::process::ExitStatus {
        let mut command = Command::new(env!("CARGO_BIN_EXE_glasshouse"));
        if let Some(log_file) = log_file {
            command.arg("--log-level").arg("debug");
            command.arg("--log-file").arg(log_file);
        }
        let mut child = command
            .env(CREDENTIAL_VAR, CREDENTIAL)
            .arg("--scope")
            .arg(&self.root)
            .arg("--data-dir")
            .arg(self.base.join("data"))
            .arg("--config-dir")
            .arg(self.base.join("config"))
            .arg("hook")
            .arg("--session")
            .arg(session.as_str())
            .arg("--event")
            .arg(event)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("the glasshouse binary must be runnable");
        child
            .stdin
            .as_mut()
            .expect("stdin was piped")
            .write_all(PAYLOAD.as_bytes())
            .expect("the handler must read its payload rather than closing the pipe");
        let output = child.wait_with_output().expect("the hook must finish");
        assert!(
            output.stdout.is_empty(),
            "a hook must print nothing on standard output: {:?}",
            String::from_utf8_lossy(&output.stdout)
        );
        output.status
    }

    fn memories(&self) -> Vec<glasshouse::memory::MemoryRecord> {
        ProjectMemory::open(&self.runtime)
            .unwrap()
            .store()
            .search("loopback", SearchScope::Current, 10)
            .unwrap()
    }
}

/// The two fields that are the conversation itself, carrying values a scan can
/// find. A hook must never read them, and extraction must never store them.
const PROMPT_MARKER: &str = "PAYLOAD-PROMPT-a1b2c3-MUST-NEVER-BE-STORED";
const REPLY_MARKER: &str = "PAYLOAD-REPLY-d4e5f6-MUST-NEVER-BE-STORED";

const PAYLOAD: &str = concat!(
    r#"{"session_id":"native-1","transcript_path":"/somewhere/rollout.jsonl","#,
    r#""hook_event_name":"Stop","cwd":"/somewhere","model":"a-model","#,
    r#""prompt":"PAYLOAD-PROMPT-a1b2c3-MUST-NEVER-BE-STORED","#,
    r#""last_assistant_message":"PAYLOAD-REPLY-d4e5f6-MUST-NEVER-BE-STORED"}"#
);

fn bootstrap(base: &Path, root: &Path) -> Runtime {
    let cli = Cli::try_parse_from([
        "glasshouse",
        "--data-dir",
        base.join("data").to_str().unwrap(),
        "--config-dir",
        base.join("config").to_str().unwrap(),
    ])
    .unwrap();
    glasshouse::bootstrap(&cli, root).unwrap()
}

fn running_session(fixture: &Fixture, harness: &str) -> SessionId {
    let sessions = ProjectSessions::open(&fixture.runtime).unwrap();
    let store = sessions.store();
    let record = store.create(NewSession::embedded(harness)).unwrap();
    store
        .set_lifecycle(&record.id, SessionLifecycle::Running)
        .unwrap();
    record.id
}

// ---------------------------------------------------------------------------
// Line 842 — extraction after task completion, completing.
// ---------------------------------------------------------------------------

/// **Line 842, against the criterion `phase-21.md` set for it.**
///
/// A harness reports `Stop`; the shipped binary asks the model the user
/// configured, over the wire, and the memory it answered with is in this
/// project's store when the process is gone. Nothing here is stubbed: this is
/// the exact configuration a person writes to point Glasshouse at a local
/// runner, and the exact process a harness spawns.
#[test]
fn a_completed_task_asks_the_configured_model_and_stores_what_it_answered() {
    let model = FakeModel::answering(ONE_FINDING);
    let fixture = Fixture::new();
    fixture.choose_model(&model.base_url(), true);
    let id = running_session(&fixture, "claude-code");

    assert!(
        fixture.hook(&id, "Stop").success(),
        "a hook must exit zero whatever extraction did"
    );

    let requests = model.requests();
    assert_eq!(
        requests.len(),
        1,
        "a completed task must ask the configured model exactly once"
    );
    let asked = &requests[0];
    assert_eq!(asked.method, "POST");
    assert_eq!(asked.target, "/v1/chat/completions");
    assert_eq!(
        asked.header("authorization"),
        Some(format!("Bearer {CREDENTIAL}").as_str()),
        "the credential the user's provider names must be what authenticates the call"
    );
    assert!(
        asked.body.contains(MODEL),
        "the request must name the model the user chose: {}",
        asked.body
    );

    let stored = fixture.memories();
    assert_eq!(
        stored.len(),
        1,
        "the capability completes only if the memory reaches the project's store"
    );
    assert_eq!(stored[0].source_session_id.as_deref(), Some(id.as_str()));
    assert!(
        stored[0]
            .source_events
            .is_some_and(|events| events.first >= 1 && events.last >= events.first),
        "an extracted memory names the slice of the log it came from"
    );
}

/// The discriminating half: a user who has configured **nothing** gets exactly
/// today's behaviour.
///
/// Without this, "extraction calls a configured model" would be satisfied by
/// "extraction calls whatever it can find". The provider table here is empty,
/// so there is no model to find — and the socket the test stands up proves it
/// by never being connected to.
#[test]
fn a_user_who_configured_no_model_makes_no_request_at_all() {
    let model = FakeModel::answering(ONE_FINDING);
    let fixture = Fixture::new();
    let id = running_session(&fixture, "claude-code");

    assert!(fixture.hook(&id, "Stop").success());

    assert!(
        model.requests().is_empty(),
        "no model may be called without the user having chosen one"
    );
    assert!(fixture.memories().is_empty());
}

/// A provider named for **cost**, not for extraction, is still not a choice to
/// call a model.
///
/// This is the sharper form of the test above and the one that would catch the
/// tempting shortcut: reading the free-model list as consent. A user's
/// `free_models` entry says a model is free; it does not ask a hook running
/// inside their coding session to start making outbound requests.
#[test]
fn a_configured_free_model_is_not_by_itself_a_choice_to_call_one() {
    let model = FakeModel::answering(ONE_FINDING);
    let fixture = Fixture::new();

    let mut user = UserConfig::load(fixture.runtime.paths()).unwrap();
    let mut provider = ProviderConfig::new("openai-compatible");
    provider.set_base_url(Some(model.base_url()));
    provider.set_credential_env(vec![CREDENTIAL_VAR.to_owned()]);
    provider.set_free_models(vec![MODEL.to_owned()]);
    user.providers_mut().set(PROVIDER, provider);
    // Note what is *not* set: `memory_extraction_model`.
    user.save(fixture.runtime.paths()).unwrap();

    let id = running_session(&fixture, "claude-code");
    assert!(fixture.hook(&id, "Stop").success());

    assert!(
        model.requests().is_empty(),
        "a free-model list is a statement about cost, not consent to be called"
    );
    assert!(fixture.memories().is_empty());
}

// ---------------------------------------------------------------------------
// Line 834 — the model is cheap or local, and configurable.
// ---------------------------------------------------------------------------

/// **Line 834's local half.** A runner on loopback with no credential at all
/// is the case the line names, and Glasshouse must not demand a key for it.
///
/// The assertion that matters is the negative one: no `authorization` header
/// was sent. A provider that names no credential variable is not missing one.
#[test]
fn a_local_model_needing_no_credential_is_called_without_one() {
    let model = FakeModel::answering(ONE_FINDING);
    let fixture = Fixture::new();
    fixture.choose_model(&model.base_url(), false);
    let id = running_session(&fixture, "claude-code");

    assert!(fixture.hook(&id, "Stop").success());

    let requests = model.requests();
    assert_eq!(requests.len(), 1, "a local runner is still a chosen model");
    assert_eq!(
        requests[0].header("authorization"),
        None,
        "a provider naming no credential variable must not be sent an invented one"
    );
    assert_eq!(fixture.memories().len(), 1);
}

/// The prompt that reaches a configured model is built from **this project's
/// event log**, and a hook's payload is not in it.
///
/// `phase-21.md` records why that holds by construction rather than by a
/// screen — the handler drains the payload unread, and `lifecycle_events` has
/// no column a conversation could reach. This is that property asserted where
/// it now actually matters: the first batch in which the prompt leaves the
/// process.
#[test]
fn the_prompt_that_leaves_the_process_cannot_contain_the_hooks_payload() {
    let model = FakeModel::answering(ONE_FINDING);
    let fixture = Fixture::new();
    fixture.choose_model(&model.base_url(), true);
    let id = running_session(&fixture, "claude-code");

    assert!(fixture.hook(&id, "Stop").success());

    let requests = model.requests();
    assert_eq!(requests.len(), 1);
    for marker in [PROMPT_MARKER, REPLY_MARKER] {
        assert!(
            !requests[0].body.contains(marker),
            "the hook's payload reached a model: {}",
            requests[0].body
        );
    }
}

/// Line 820, at the one place it now has real teeth: a model that answers
/// `500` is a support job failing, and the session must be untouched by it.
///
/// Before this batch the production path could only fail one way — no model at
/// all. This is the first failure that comes back over a socket.
#[test]
fn a_model_that_fails_over_the_wire_costs_the_coding_session_nothing() {
    let model = FakeModel::failing();
    let fixture = Fixture::new();
    fixture.choose_model(&model.base_url(), true);
    let id = running_session(&fixture, "claude-code");

    assert!(
        fixture.hook(&id, "Stop").success(),
        "a failing extraction model must never make the hook exit non-zero"
    );

    assert_eq!(model.requests().len(), 1, "the model was asked and failed");
    assert!(fixture.memories().is_empty());

    // The session's own bookkeeping happened anyway.
    let sessions = ProjectSessions::open(&fixture.runtime).unwrap();
    let record = sessions.store().get(&id).unwrap().unwrap();
    assert_eq!(
        record.lifecycle,
        SessionLifecycle::Idle,
        "a failed extraction must not stop the turn being recorded as ended"
    );
}

// ---------------------------------------------------------------------------
// Line 843 — extraction before native compaction.
// ---------------------------------------------------------------------------

/// **Line 843.** Codex reports `PreCompact`, Glasshouse asks for it, and the
/// shipped binary now runs extraction when it arrives.
///
/// The session has done something first, and that is not staging — it is the
/// requirement. `Extractor::run` short-circuits an empty chunk **before**
/// asking the model, so a compaction in a session with no history correctly
/// asks nothing at all. That is what makes the negative tests below need the
/// same prelude: without it they would pass against a build where the
/// compaction trigger fired on every event.
///
/// Note the two facts asserted together, because the second is what makes the
/// first possible without a migration: the model was asked, **and** the
/// compaction added no lifecycle event. A compaction is not a session state,
/// and `database::LIFECYCLE_EVENT_KINDS` has no value for one.
#[test]
fn a_harness_about_to_compact_runs_extraction_and_records_no_lifecycle_event() {
    use glasshouse::events::EventLog;

    let model = FakeModel::answering(ONE_FINDING);
    let fixture = Fixture::new();
    fixture.choose_model(&model.base_url(), true);
    let id = running_session(&fixture, "codex");

    // A turn starts. This is a lifecycle event, it is recorded, and it is not
    // a completed task — so it asks no model of its own.
    assert!(fixture.hook(&id, "UserPromptSubmit").success());
    let log = EventLog::open(&fixture.runtime).unwrap();
    let before = log.for_session(&id).unwrap().len();
    assert_eq!(before, 1);
    assert!(model.requests().is_empty(), "a turn starting asks no model");

    // Mid-turn, the harness says it is about to compact.
    assert!(fixture.hook(&id, "PreCompact").success());

    assert_eq!(
        model.requests().len(),
        1,
        "a harness about to compact must run extraction"
    );
    assert_eq!(fixture.memories().len(), 1);

    assert_eq!(
        EventLog::open(&fixture.runtime)
            .unwrap()
            .for_session(&id)
            .unwrap()
            .len(),
        before,
        "a compaction is not a lifecycle event and must not be recorded as one"
    );

    let sessions = ProjectSessions::open(&fixture.runtime).unwrap();
    assert_eq!(
        sessions.store().get(&id).unwrap().unwrap().lifecycle,
        SessionLifecycle::Running,
        "a session that compacts was running before and is running after"
    );
}

/// The discriminating half of the compaction trigger.
///
/// `PostCompact` is a real event Glasshouse asks Codex for, and `PreToolUse`
/// is a real event it does not. Neither runs extraction — without this,
/// "before or around compaction" would be satisfied by "on any event this
/// build does not recognise", which is every event a future harness gains.
#[test]
fn an_unrecognised_event_that_is_not_a_compaction_asks_no_model() {
    for event in ["PostCompact", "PreToolUse", "SubagentStop", "SessionEnd"] {
        let model = FakeModel::answering(ONE_FINDING);
        let fixture = Fixture::new();
        fixture.choose_model(&model.base_url(), true);
        let id = running_session(&fixture, "codex");

        // The same prelude the positive test uses, so this asserts about the
        // event and not about an empty log — see
        // `a_harness_about_to_compact_runs_extraction_and_records_no_lifecycle_event`.
        assert!(fixture.hook(&id, "UserPromptSubmit").success());
        assert!(fixture.hook(&id, event).success());

        assert!(
            model.requests().is_empty(),
            "`{event}` is not a compaction and must not run extraction"
        );
        assert!(fixture.memories().is_empty());
    }
}

/// Turning automatic memory extraction off turns it off — including at a
/// compaction.
///
/// The two triggers read the same switch on purpose: a user who disabled
/// automatic extraction disabled it, not "disabled it except when the harness
/// compacts". A second, separate switch would be a way to be surprised.
#[test]
fn disabling_memory_extraction_silences_both_triggers() {
    for event in ["Stop", "PreCompact"] {
        let model = FakeModel::answering(ONE_FINDING);
        let fixture = Fixture::new();
        fixture.choose_model(&model.base_url(), true);

        let mut user = UserConfig::load(fixture.runtime.paths()).unwrap();
        user.set_memory_extraction(Some(false));
        user.save(fixture.runtime.paths()).unwrap();

        let id = running_session(&fixture, "codex");
        // Non-vacuity, as above: with the switch on, both of these events do
        // ask a model once the session has a history.
        assert!(fixture.hook(&id, "UserPromptSubmit").success());
        assert!(fixture.hook(&id, event).success());

        assert!(
            model.requests().is_empty(),
            "`{event}` ran extraction with the trigger switched off"
        );
        assert!(fixture.memories().is_empty());
    }
}

/// Which trigger ran is recorded, in words, on the outcome and in the log.
///
/// This is the packet's *"an evaluation run must never later read as evidence
/// that a model performed extraction"* rule turned the right way up: now that
/// a model really can perform extraction, the log has to say **which trigger**
/// asked it and **which resource** answered, or a memory's origin becomes
/// unrecoverable the moment there are two triggers. Both strings are asserted
/// against the real process's real log file.
#[test]
fn the_log_names_the_trigger_and_the_resource_that_answered() {
    let model = FakeModel::answering(ONE_FINDING);
    let fixture = Fixture::new();
    fixture.choose_model(&model.base_url(), true);
    let id = running_session(&fixture, "codex");

    let after_turn = fixture.base.join("after-turn.log");
    assert!(
        fixture
            .hook_logging(&id, "Stop", Some(&after_turn))
            .success()
    );
    let written = std::fs::read_to_string(&after_turn).expect("the hook must have written a log");
    assert!(
        written.contains("trigger=task_completed"),
        "the post-turn trigger must name itself: {written}"
    );
    assert!(
        written.contains(&format!("{PROVIDER}/{MODEL}")),
        "the resource that answered must be named: {written}"
    );
    assert!(
        !written.contains(CREDENTIAL),
        "the credential reached the log: {written}"
    );

    let before_compaction = fixture.base.join("before-compaction.log");
    assert!(
        fixture
            .hook_logging(&id, "PreCompact", Some(&before_compaction))
            .success()
    );
    let written =
        std::fs::read_to_string(&before_compaction).expect("the hook must have written a log");
    assert!(
        written.contains("trigger=before_compaction"),
        "the compaction trigger must name itself, and differently: {written}"
    );
}
