//! GH-MEMORY-RERANKER — capability map lines 1089-1092 and 1094: a
//! reranking seat in the disposable router.
//!
//! Drives the shipped binary through the control-API door exactly as
//! `tests/context_injection.rs` does — a real `glasshouse api serve`,
//! `spawn_session` with a task, and a harness that logs what it read —
//! because that is the production seam whose `query` is the task text
//! itself (`api::unix::select_memory`); `glasshouse launch --task` feeds
//! only routing, not the briefing (`main.rs::brief_launch_session`'s query
//! comes from `--from-checkpoint` alone). Extended with
//! `tests/routed_extraction.rs`'s own canned OpenAI chat-completions
//! endpoint on loopback for the rerank model.
//!
//! Every test seeds memory in-process before spawning, so the lexical
//! search's own candidate order is known before a fixture is ever asked to
//! reorder it.

#![cfg(unix)]

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use clap::Parser;

use glasshouse::config::{ExtractionModelRef, ProviderConfig, UserConfig};
use glasshouse::memory::inject::MEMORY_MARKER;
use glasshouse::memory::{MemoryAuthority, MemoryKind, NewMemory, ProjectMemory};
use glasshouse::{Cli, Runtime};

const TIMEOUT: Duration = Duration::from_secs(30);

const CREDENTIAL: &str = "sk-fabricated-test-value-not-a-real-credential";
const CREDENTIAL_VAR: &str = "GLASSHOUSE_TEST_ONLY_RERANKER_KEY";
const PROVIDER: &str = "rerank-provider";
const MODEL: &str = "rerank-model";

// ---------------------------------------------------------------------------
// The fixture — `tests/context_injection.rs`'s own shape.
// ---------------------------------------------------------------------------

struct Fixture {
    _tmp: tempfile::TempDir,
    base: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let tmp = tempfile::tempdir().expect("tempdir");
        let base = tmp.path().to_path_buf();

        let bin_dir = base.join("bin");
        std::fs::create_dir_all(&bin_dir).expect("create bin dir");
        let harness = install_session_tagging_harness(&bin_dir);

        let config_dir = base.join("config");
        std::fs::create_dir_all(&config_dir).expect("create config dir");
        let escaped = harness.display().to_string().replace('\\', "\\\\");
        // `implementation_policy = false`: this file is about the memory
        // block and the rerank seat alone — `tests/context_injection.rs`'s
        // own reasoning.
        std::fs::write(
            config_dir.join("config.toml"),
            format!(
                "version = 1\nimplementation_policy = false\n\n[integrations.claude-code]\nenabled = \
                 true\nexecutable = \"{escaped}\"\n"
            ),
        )
        .expect("write user config");

        Self { _tmp: tmp, base }
    }

    fn project_root(&self, name: &str) -> PathBuf {
        let root = self.base.join("workspace").join(name);
        std::fs::create_dir_all(root.join(".git")).expect("create project root");
        std::fs::canonicalize(&root).expect("canonicalize project root")
    }

    fn runtime(&self, root: &Path) -> Runtime {
        let cli = Cli::try_parse_from([
            "glasshouse",
            "--data-dir",
            self.base.join("data").to_str().unwrap(),
            "--config-dir",
            self.base.join("config").to_str().unwrap(),
        ])
        .unwrap();
        glasshouse::bootstrap(&cli, root).unwrap()
    }

    fn config(&self, root: &Path) -> UserConfig {
        UserConfig::load(self.runtime(root).paths()).unwrap()
    }

    fn save(&self, root: &Path, user: UserConfig) {
        user.save(self.runtime(root).paths()).unwrap();
    }

    /// A provider speaking OpenAI chat completions at `base_url`, its model
    /// marked free.
    fn add_provider(&self, root: &Path, base_url: &str) {
        let mut user = self.config(root);
        let mut provider = ProviderConfig::new("openai-compatible");
        provider.set_base_url(Some(base_url.to_owned()));
        provider.set_credential_env(vec![CREDENTIAL_VAR.to_owned()]);
        provider.set_free_models(vec![MODEL.to_owned()]);
        user.providers_mut().set(PROVIDER, provider);
        self.save(root, user);
    }

    /// `[memory] rerank_model` — the seat's consent.
    fn choose_rerank_model(&self, root: &Path) {
        let mut user = self.config(root);
        user.memory_mut()
            .set_rerank_model(Some(ExtractionModelRef::new(PROVIDER, MODEL)));
        self.save(root, user);
    }

    fn enable_diagnostics(&self, root: &Path) {
        let mut user = self.config(root);
        user.memory_mut().set_retrieval_diagnostics(Some(true));
        self.save(root, user);
    }

    fn diagnostics_path(&self, root: &Path) -> PathBuf {
        self.runtime(root)
            .state_dir()
            .join("memory-retrieval.jsonl")
    }

    /// Every line of `memory-retrieval.jsonl`, parsed — empty when the file
    /// does not exist, exactly as "nothing written" reads to a caller that
    /// only cares whether there is anything to see.
    fn diagnostics_lines(&self, root: &Path) -> Vec<serde_json::Value> {
        std::fs::read_to_string(self.diagnostics_path(root))
            .map(|text| {
                text.lines()
                    .map(|line| serde_json::from_str(line).expect("a well-formed diagnostics line"))
                    .collect()
            })
            .unwrap_or_default()
    }

    fn received(&self, root: &Path, session: &str) -> Option<String> {
        std::fs::read_to_string(root.join(format!("received-{session}.log"))).ok()
    }

    fn argv(&self, root: &Path, session: &str) -> Option<String> {
        std::fs::read_to_string(root.join(format!("argv-{session}.log"))).ok()
    }
}

/// A harness that names its log files after the session it was started for
/// — copied from `tests/context_injection.rs`'s own helper of the same name.
fn install_session_tagging_harness(bin_dir: &Path) -> PathBuf {
    let path = bin_dir.join("session-tagging-harness");
    std::fs::write(
        &path,
        "#!/bin/sh\n\
         tag=unknown\n\
         prev=\"\"\n\
         for a in \"$@\"; do\n\
         if [ \"$prev\" = \"--settings\" ]; then tag=$(basename \"$(dirname \"$a\")\"); fi\n\
         prev=\"$a\"\n\
         done\n\
         echo \"$@\" > \"$PWD/argv-$tag.log\"\n\
         echo READY\n\
         while IFS= read -r line; do\n\
         printf '%s\\n' \"$line\" >> \"$PWD/received-$tag.log\"\n\
         echo \"got:$line\"\n\
         done\n",
    )
    .expect("write the session-tagging harness");
    let mut perms = std::fs::metadata(&path).unwrap().permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&path, perms).unwrap();
    path
}

fn seed_memory(runtime: &Runtime, kind: MemoryKind, subject: &str, body: &str) -> String {
    ProjectMemory::open(runtime)
        .unwrap()
        .store()
        .record(NewMemory::new(kind, body).with_subject(Some(subject.to_owned())))
        .unwrap()
        .id
        .as_str()
        .to_owned()
}

fn seed_constraint(runtime: &Runtime, subject: &str, body: &str) -> String {
    ProjectMemory::open(runtime)
        .unwrap()
        .store()
        .record(
            NewMemory::new(MemoryKind::Constraint, body)
                .with_subject(Some(subject.to_owned()))
                .with_authority(Some(MemoryAuthority::Constraint)),
        )
        .unwrap()
        .id
        .as_str()
        .to_owned()
}

// ---------------------------------------------------------------------------
// The control-API server — `tests/context_injection.rs`'s own shape.
// ---------------------------------------------------------------------------

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
            .env(CREDENTIAL_VAR, CREDENTIAL)
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

    fn spawn_with_task(&self, task: &str) -> String {
        let response = self.call(serde_json::json!({
            "op": "spawn_session",
            "harness": "claude-code",
            "role": "worker",
            "task": task,
        }));
        assert_eq!(response["status"], "ok", "{response}");
        response["result"]["session"]
            .as_str()
            .expect("a session id")
            .to_owned()
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn wait_for<F: FnMut() -> bool>(what: &str, mut done: F) {
    let deadline = Instant::now() + TIMEOUT;
    loop {
        if done() {
            return;
        }
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// Wait until the harness has read `count` deliveries, and return them.
fn deliveries(fixture: &Fixture, root: &Path, session: &str, count: usize) -> Vec<String> {
    let deadline = Instant::now() + TIMEOUT;
    loop {
        let received = fixture.received(root, session);
        if received
            .as_deref()
            .is_some_and(|text| text.lines().count() >= count)
        {
            return received
                .expect("a received log")
                .lines()
                .map(str::to_owned)
                .collect();
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for {count} deliveries to reach the harness; it read: {:#?}",
            received
                .as_deref()
                .map(|text| text.lines().collect::<Vec<_>>())
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn the_injected_block(lines: &[String]) -> &str {
    let blocks: Vec<&String> = lines
        .iter()
        .filter(|line| line.contains(MEMORY_MARKER))
        .collect();
    assert_eq!(
        blocks.len(),
        1,
        "exactly one delivered line must carry the labelled memory block: {lines:?}"
    );
    blocks[0]
}

// ---------------------------------------------------------------------------
// A canned OpenAI chat-completions endpoint — `tests/routed_extraction.rs`'s
// own `FakeModel`, extended with a responder that never answers.
// ---------------------------------------------------------------------------

struct Seen {
    body: String,
}

enum Answer {
    Content(String),
    /// Accepts the connection, reads the request, and then writes nothing —
    /// the shape that exercises the rerank seat's `response` timeout bound
    /// rather than its `connect` one.
    Waiting,
}

struct FakeModel {
    address: SocketAddr,
    seen: Arc<Mutex<Vec<Seen>>>,
    stop: Arc<AtomicBool>,
}

impl FakeModel {
    fn answering(content: &str) -> Self {
        let content = content.to_owned();
        Self::start(move |_| Answer::Content(content.clone()))
    }

    fn waiting() -> Self {
        Self::start(|_| Answer::Waiting)
    }

    fn start(responder: impl Fn(usize) -> Answer + Send + Sync + 'static) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("loopback must bind");
        listener
            .set_nonblocking(true)
            .expect("the accept loop polls its stop flag");
        let address = listener.local_addr().unwrap();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let served = AtomicUsize::new(0);

        let thread_seen = Arc::clone(&seen);
        let thread_stop = Arc::clone(&stop);
        std::thread::spawn(move || {
            while !thread_stop.load(Ordering::SeqCst) {
                match listener.accept() {
                    Ok((stream, _)) => {
                        let _ = stream.set_nonblocking(false);
                        let nth = served.fetch_add(1, Ordering::SeqCst);
                        serve(stream, &thread_seen, &responder, nth);
                    }
                    Err(ref err) if err.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(5));
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
        self.seen.lock().unwrap().drain(..).collect()
    }
}

impl Drop for FakeModel {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
    }
}

fn serve(
    mut stream: TcpStream,
    seen: &Arc<Mutex<Vec<Seen>>>,
    responder: &(impl Fn(usize) -> Answer + ?Sized),
    nth: usize,
) {
    let mut reader = BufReader::new(stream.try_clone().expect("the stream clones"));
    let mut request_line = String::new();
    if reader.read_line(&mut request_line).is_err() || request_line.is_empty() {
        return;
    }

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
        if let Some((name, value)) = line.split_once(':')
            && name.trim().eq_ignore_ascii_case("content-length")
        {
            length = value.trim().parse().unwrap_or(0);
        }
    }

    let mut body = vec![0u8; length];
    if reader.read_exact(&mut body).is_err() {
        return;
    }
    let body = String::from_utf8_lossy(&body).into_owned();
    seen.lock().unwrap().push(Seen { body });

    match responder(nth) {
        Answer::Content(content) => {
            let document = serde_json::json!({
                "choices": [{ "message": { "role": "assistant", "content": content } }],
                "usage": { "prompt_tokens": 42, "completion_tokens": 4 }
            })
            .to_string();
            let response = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\
                 connection: close\r\n\r\n{document}",
                document.len()
            );
            let _ = stream.write_all(response.as_bytes());
            let _ = stream.flush();
        }
        Answer::Waiting => {
            // Read the request, answer nothing, and hold the connection
            // until the client's own response timeout ends it.
            std::thread::sleep(Duration::from_secs(30));
        }
    }
}

// ---------------------------------------------------------------------------
// (a) No `rerank_model` configured: the block is lexical, and nothing is
// dialled even though a reachable endpoint exists.
// ---------------------------------------------------------------------------

#[test]
fn a_no_rerank_model_configured_leaves_the_block_lexical_and_dials_nothing() {
    let fixture = Fixture::new();
    let root = fixture.project_root("alpha");
    let fake = FakeModel::answering(r#"["should-never-be-read"]"#);
    // The provider exists and is reachable, but `[memory] rerank_model`
    // never names it — line 1090's own proof shape.
    fixture.add_provider(&root, &fake.base_url());

    // Two ordinary candidates: `rerank` never calls anything for fewer than
    // two regardless of consent (`RerankOutcome::TooFew`), and this test is
    // about consent specifically, not about the window floor.
    let runtime = fixture.runtime(&root);
    seed_memory(
        &runtime,
        MemoryKind::Finding,
        "marmot burrow",
        "marmot burrow depth in this project.",
    );
    seed_memory(
        &runtime,
        MemoryKind::Finding,
        "marmot burrow second",
        "marmot burrow second finding in this project.",
    );

    let server = Server::start(&fixture, &root);
    let session = server.spawn_with_task("marmot burrow");
    wait_for("the worker's harness to start", || {
        fixture.argv(&root, &session).is_some()
    });
    let lines = deliveries(&fixture, &root, &session, 2);
    let block = the_injected_block(&lines);

    assert!(
        block.contains("marmot burrow depth"),
        "the lexical match must still be injected: {block}"
    );
    assert!(
        fake.requests().is_empty(),
        "no rerank_model means no request reaches any fixture"
    );
}

// ---------------------------------------------------------------------------
// (b) A reversed reply reorders the ordinary bucket; a constraint stays
// first.
// ---------------------------------------------------------------------------

#[test]
fn a_reversed_reply_reorders_ordinary_memories_with_a_constraint_still_first() {
    let fixture = Fixture::new();
    let root = fixture.project_root("alpha");
    let runtime = fixture.runtime(&root);
    seed_constraint(
        &runtime,
        "marmot export",
        "The marmot export must never write partial files.",
    );
    let first_id = seed_memory(
        &runtime,
        MemoryKind::Finding,
        "marmot alpha",
        "marmot burrow alpha finding for this project.",
    );
    let second_id = seed_memory(
        &runtime,
        MemoryKind::Finding,
        "marmot beta",
        "marmot burrow beta finding for this project.",
    );

    let fake = FakeModel::answering(&format!(r#"["{second_id}", "{first_id}"]"#));
    fixture.add_provider(&root, &fake.base_url());
    fixture.choose_rerank_model(&root);

    let server = Server::start(&fixture, &root);
    let session = server.spawn_with_task("marmot burrow");
    wait_for("the worker's harness to start", || {
        fixture.argv(&root, &session).is_some()
    });
    let lines = deliveries(&fixture, &root, &session, 2);
    let block = the_injected_block(&lines).to_owned();

    let requests = fake.requests();
    assert_eq!(requests.len(), 1, "one rerank call, no more");
    assert!(
        requests[0].body.contains(&first_id) && requests[0].body.contains(&second_id),
        "both ordinary candidates must have been offered: {}",
        requests[0].body
    );

    let constraint_pos = block
        .find("must never write partial files")
        .expect("the constraint must be injected");
    let second_pos = block
        .find("burrow beta finding")
        .expect("the reply-led memory must be injected");
    let first_pos = block
        .find("burrow alpha finding")
        .expect("the omitted-but-sent memory must still be injected");
    assert!(
        constraint_pos < second_pos,
        "the constraint must precede every ordinary memory: {block}"
    );
    assert!(
        second_pos < first_pos,
        "the reply's order must be honoured among ordinary memories: {block}"
    );
}

// ---------------------------------------------------------------------------
// (c) An unknown id bypasses the whole reply, and diagnostics record why.
// ---------------------------------------------------------------------------

#[test]
fn an_unknown_id_bypasses_to_lexical_order_and_diagnostics_record_the_reason() {
    let fixture = Fixture::new();
    let root = fixture.project_root("alpha");
    let runtime = fixture.runtime(&root);
    // Two ordinary candidates: `rerank` never calls anything for fewer than
    // two (`RerankOutcome::TooFew`), and this test is about a call that was
    // made and bypassed, not one that was never owed.
    seed_memory(
        &runtime,
        MemoryKind::Finding,
        "marmot gamma",
        "marmot burrow gamma finding for this project.",
    );
    seed_memory(
        &runtime,
        MemoryKind::Finding,
        "marmot gamma second",
        "marmot burrow gamma second finding for this project.",
    );

    let fake = FakeModel::answering(r#"["not-a-real-candidate-id"]"#);
    fixture.add_provider(&root, &fake.base_url());
    fixture.choose_rerank_model(&root);
    fixture.enable_diagnostics(&root);

    let server = Server::start(&fixture, &root);
    let session = server.spawn_with_task("marmot burrow");
    wait_for("the worker's harness to start", || {
        fixture.argv(&root, &session).is_some()
    });
    let lines = deliveries(&fixture, &root, &session, 2);
    let block = the_injected_block(&lines);

    assert!(
        block.contains("burrow gamma finding"),
        "an untrustworthy reply must still leave the lexical match injected: {block}"
    );

    let diag = fixture.diagnostics_lines(&root);
    assert_eq!(
        diag.len(),
        1,
        "one briefing, one diagnostics line: {diag:?}"
    );
    assert_eq!(diag[0]["rerank"]["outcome"], "bypassed");
    assert!(
        diag[0]["rerank"]["reason"]
            .as_str()
            .unwrap()
            .contains("not-a-real-candidate-id"),
        "the reason must name the offending id: {}",
        diag[0]
    );
}

// ---------------------------------------------------------------------------
// (d) A fixture that never answers bypasses within the seat's own bound, and
// the reason is recorded.
// ---------------------------------------------------------------------------

#[test]
fn a_fixture_that_never_answers_bypasses_within_the_seats_timeout() {
    let fixture = Fixture::new();
    let root = fixture.project_root("alpha");
    let runtime = fixture.runtime(&root);
    seed_memory(
        &runtime,
        MemoryKind::Finding,
        "marmot delta",
        "marmot burrow delta finding for this project.",
    );
    seed_memory(
        &runtime,
        MemoryKind::Finding,
        "marmot delta second",
        "marmot burrow delta second finding for this project.",
    );

    let fake = FakeModel::waiting();
    fixture.add_provider(&root, &fake.base_url());
    fixture.choose_rerank_model(&root);
    fixture.enable_diagnostics(&root);

    let server = Server::start(&fixture, &root);
    let started = Instant::now();
    let session = server.spawn_with_task("marmot burrow");
    wait_for("the worker's harness to start", || {
        fixture.argv(&root, &session).is_some()
    });
    let lines = deliveries(&fixture, &root, &session, 2);
    assert!(
        started.elapsed() < Duration::from_secs(15),
        "the rerank seat's own bound must be well under extraction's 30s default: {:?}",
        started.elapsed()
    );
    let block = the_injected_block(&lines);

    assert!(
        block.contains("burrow delta finding"),
        "a model that never answers must still leave the lexical match injected: {block}"
    );

    let diag = fixture.diagnostics_lines(&root);
    assert_eq!(diag.len(), 1, "{diag:?}");
    assert_eq!(diag[0]["rerank"]["outcome"], "bypassed");
    assert!(
        diag[0]["rerank"]["reason"]
            .as_str()
            .unwrap()
            .contains("bound"),
        "the reason must say the call did not answer within its bound: {}",
        diag[0]
    );
}

// ---------------------------------------------------------------------------
// (e) A conflicted (never-current) memory is never injected, even ranked
// first — the currency filter runs after the reorder.
// ---------------------------------------------------------------------------

#[test]
fn a_conflicted_memory_is_never_injected_even_when_ranked_first() {
    let fixture = Fixture::new();
    let root = fixture.project_root("alpha");
    let runtime = fixture.runtime(&root);
    // Phase 22's contradiction shape, in the *ordinary* bucket: neither
    // record's authority is invariant or constraint, so both group into
    // `other` and reach the reranker — unlike `context_injection.rs`'s own
    // conflict test, whose pair is authority-constraint and never leaves
    // `invariants_and_constraints`.
    let adopted_id = seed_memory(
        &runtime,
        MemoryKind::Decision,
        "marmot epsilon export",
        "The marmot epsilon export now runs hourly.",
    );
    seed_memory(
        &runtime,
        MemoryKind::FailedAttempt,
        "marmot epsilon export",
        "The marmot epsilon export was abandoned after data loss.",
    );
    seed_memory(
        &runtime,
        MemoryKind::Finding,
        "marmot epsilon dashboard",
        "The marmot epsilon dashboard is read-only.",
    );

    let fake = FakeModel::answering(&format!(r#"["{adopted_id}"]"#));
    fixture.add_provider(&root, &fake.base_url());
    fixture.choose_rerank_model(&root);

    let server = Server::start(&fixture, &root);
    let session = server.spawn_with_task("marmot epsilon");
    wait_for("the worker's harness to start", || {
        fixture.argv(&root, &session).is_some()
    });
    let lines = deliveries(&fixture, &root, &session, 2);
    let block = the_injected_block(&lines);

    assert!(
        block.contains("dashboard is read-only"),
        "the memory in no conflict must be injected, so this test is not vacuous: {block}"
    );
    assert!(
        !block.contains("now runs hourly"),
        "the reply ranked this memory first, and it must still never be injected: {block}"
    );
    assert!(
        !block.contains("abandoned after data loss"),
        "nor the memory it conflicts with: {block}"
    );
}

// ---------------------------------------------------------------------------
// (f) More than `RERANK_CANDIDATES` ordinary candidates: the fixture
// receives exactly the window.
// ---------------------------------------------------------------------------

#[test]
fn more_than_the_window_sends_exactly_rerank_candidates_ids() {
    use glasshouse::memory::rerank::RERANK_CANDIDATES;

    let fixture = Fixture::new();
    let root = fixture.project_root("alpha");
    let runtime = fixture.runtime(&root);
    for n in 0..(RERANK_CANDIDATES + 4) {
        seed_memory(
            &runtime,
            MemoryKind::Finding,
            &format!("marmot zeta {n}"),
            &format!("marmot zeta finding number {n} for this project."),
        );
    }

    let fake = FakeModel::answering(r#"[]"#);
    fixture.add_provider(&root, &fake.base_url());
    fixture.choose_rerank_model(&root);

    let server = Server::start(&fixture, &root);
    let session = server.spawn_with_task("marmot zeta");
    wait_for("the worker's harness to start", || {
        fixture.argv(&root, &session).is_some()
    });
    deliveries(&fixture, &root, &session, 2);

    let requests = fake.requests();
    assert_eq!(requests.len(), 1);
    // `\n` anchors on an actual candidate line — the contract's own
    // instructional prose also contains the literal text "id: ".
    let sent_ids = requests[0].body.matches("\\nid: ").count();
    assert_eq!(
        sent_ids, RERANK_CANDIDATES,
        "exactly RERANK_CANDIDATES ids must reach the model: {}",
        requests[0].body
    );
}

// ---------------------------------------------------------------------------
// (g) Diagnostics: off writes nothing; on writes one well-formed line per
// briefing.
// ---------------------------------------------------------------------------

#[test]
fn diagnostics_off_writes_nothing_and_on_writes_one_line_per_briefing() {
    let fixture = Fixture::new();
    let root = fixture.project_root("alpha");
    seed_memory(
        &fixture.runtime(&root),
        MemoryKind::Finding,
        "marmot eta",
        "marmot burrow eta finding for this project.",
    );
    let fake = FakeModel::answering(r#"[]"#);
    fixture.add_provider(&root, &fake.base_url());
    fixture.choose_rerank_model(&root);

    let server = Server::start(&fixture, &root);
    let first_session = server.spawn_with_task("marmot burrow eta");
    wait_for("the first worker's harness to start", || {
        fixture.argv(&root, &first_session).is_some()
    });
    deliveries(&fixture, &root, &first_session, 2);
    assert!(
        !fixture.diagnostics_path(&root).exists(),
        "diagnostics off must leave no file behind"
    );

    fixture.enable_diagnostics(&root);
    let second_session = server.spawn_with_task("marmot burrow eta");
    wait_for("the second worker's harness to start", || {
        fixture.argv(&root, &second_session).is_some()
    });
    deliveries(&fixture, &root, &second_session, 2);

    let diag = fixture.diagnostics_lines(&root);
    assert_eq!(diag.len(), 1, "one briefing, one line: {diag:?}");
    assert_eq!(diag[0]["query"], "marmot burrow eta");
    assert!(diag[0]["candidates"].is_array());
    assert!(diag[0]["selected"].is_array());
}

// ---------------------------------------------------------------------------
// (h) `memory search --explain` prints the same record and writes nothing.
// ---------------------------------------------------------------------------

#[test]
fn memory_search_explain_prints_the_record_and_writes_no_file() {
    let fixture = Fixture::new();
    let root = fixture.project_root("alpha");
    let runtime = fixture.runtime(&root);
    let first_id = seed_memory(
        &runtime,
        MemoryKind::Finding,
        "marmot theta",
        "marmot burrow theta finding for this project.",
    );
    let second_id = seed_memory(
        &runtime,
        MemoryKind::Finding,
        "marmot iota",
        "marmot burrow iota finding for this project.",
    );

    let fake = FakeModel::answering(&format!(r#"["{second_id}", "{first_id}"]"#));
    fixture.add_provider(&root, &fake.base_url());
    fixture.choose_rerank_model(&root);

    let out = Command::new(env!("CARGO_BIN_EXE_glasshouse"))
        .arg("--scope")
        .arg(&root)
        .arg("--data-dir")
        .arg(fixture.base.join("data"))
        .arg("--config-dir")
        .arg(fixture.base.join("config"))
        .arg("memory")
        .arg("search")
        .arg("--explain")
        .arg("marmot burrow")
        .env(CREDENTIAL_VAR, CREDENTIAL)
        .output()
        .expect("the glasshouse binary must be runnable");
    assert!(
        out.status.success(),
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        !fixture.diagnostics_path(&root).exists(),
        "--explain must write no file"
    );

    let record: serde_json::Value =
        serde_json::from_str(String::from_utf8_lossy(&out.stdout).trim())
            .expect("--explain must print one JSON object");
    assert_eq!(record["rerank"]["outcome"], "reordered");
    assert_eq!(
        record["selected"],
        serde_json::json!([second_id, first_id]),
        "the printed record must reflect the same reorder a real briefing would apply: {record}"
    );
}
