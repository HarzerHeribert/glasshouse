//! `GH-OBSERVATION-SESSION-COLUMN`, capability map line 2019's per-session
//! half: a gateway-written routing-observation row names the Glasshouse
//! session it served, and — on a translated exchange — the effort the
//! request carried and the shape of its turn. `crate::database` migration 24
//! and `docs/product/design-decisions.md`'s *A session identity on the
//! routing evidence rows — Cluster G's first column*.
//!
//! # Where these tests enter
//!
//! (a), (d) and (f) go through **`glasshouse launch`** itself, because the
//! producer this package adds is a line in `main.rs`'s launch path — the
//! call that tells the gateway which session it serves — and practice §35 is
//! explicit that a caller every test bypasses is not a caller. A test that
//! called `SessionRouting::serve_session` itself would pass against a build
//! where the shipped binary never called it at all.
//!
//! (b) and (c) enter one level down, at
//! `gateway::start_if_required_with_degrade_sink` — the same door
//! `tests/gateway_translate_evidence.rs` uses — because what they are about
//! is the *seam* rather than the launch: a prompt-shaped request's stored
//! word, and a relayed exchange writing `NULL` for two facts it never read.
//! The launch door is already proven by (a).
//!
//! (e) opens a database directly: it is about a schema upgrade, and there is
//! no gateway in it.
//!
//! # Fixtures: copied, not shared
//!
//! The chat fixture, the provider/upstream builders, the ledger fixture and
//! the launch harness are copied from `tests/gateway_translate_cache.rs` and
//! `tests/gateway_translate_evidence.rs` rather than lifted into a shared
//! module — every pair file in this crate already carries its own copy, and
//! those two files' own headers explain why.

use std::io::{Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use glasshouse::gateway::{Gateway, Route, Upstream, UpstreamBackend};
use glasshouse::integrations::IntegrationId;
use glasshouse::profile::{BackendResource, LaunchProfile};
use glasshouse::provider::Provider;
use glasshouse::routing::evidence::{
    EffortLevel, EvidenceLedger, HARNESS_TURN_PURPOSE, NewObservation, ObservationQuery, Outcome,
    RoutingObservation, TurnShape,
};
use glasshouse::routing::{AssignedModel, Cost, CredentialId};
use glasshouse::secret::{EnvironmentSecretStore, SecretRef, SecretStore};
use glasshouse::session::{ProjectSessions, SessionId};
use serde_json::{Value, json};

/// Planted provider credentials, unique to this test binary. Never real
/// keys, and asserted on in (f) so that `!contains` has something to bite.
const CHAT_KEY: &str = "sk-planted-session-column-000111";
const CHAT_CREDENTIAL_VAR: &str = "GLASSHOUSE_ROUTING_SESSION_COLUMN_CHAT_TEST_KEY";
const RELAY_KEY: &str = "sk-planted-session-column-000222";
const RELAY_CREDENTIAL_VAR: &str = "GLASSHOUSE_ROUTING_SESSION_COLUMN_RELAY_TEST_KEY";

/// The harness's `metadata.user_id`, which (f) proves never reaches a row.
const PLANTED_USER_ID: &str = "PLANTED-HARNESS-USER-ID-DO-NOT-STORE";

const CLIENT_TIMEOUT: Duration = Duration::from_secs(20);

/// Every set/resolve/remove of a `CREDENTIAL_VAR` happens under this lock —
/// the environment is process state and this binary's tests run in parallel.
static ENV_LOCK: Mutex<()> = Mutex::new(());

fn env_lock() -> MutexGuard<'static, ()> {
    ENV_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

// --- recorded requests --------------------------------------------------------

#[derive(Debug, Clone)]
struct RecordedRequest {
    body: Vec<u8>,
}

impl RecordedRequest {
    fn json(&self) -> Value {
        serde_json::from_slice(&self.body).expect("the fixture received a JSON body")
    }
}

/// Independent of anything under test: reads a request head byte by byte and
/// the body by its declared length.
fn read_request(stream: &mut TcpStream) -> Option<RecordedRequest> {
    let mut head = Vec::new();
    let mut byte = [0u8; 1];
    while !head.ends_with(b"\r\n\r\n") {
        match stream.read(&mut byte) {
            Ok(0) | Err(_) => return None,
            Ok(_) => head.push(byte[0]),
        }
        if head.len() > 64 * 1024 {
            return None;
        }
    }
    let text = String::from_utf8(head).ok()?;
    let mut content_length = 0usize;
    for line in text.split("\r\n").skip(1) {
        if let Some(value) = line.to_ascii_lowercase().strip_prefix("content-length:") {
            content_length = value.trim().parse().ok()?;
        }
    }
    let mut body = vec![0u8; content_length];
    stream.read_exact(&mut body).ok()?;
    Some(RecordedRequest { body })
}

fn write_document(stream: &mut TcpStream, body: &str) {
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
        body.len()
    );
    let _ = stream.write_all(response.as_bytes());
    let _ = stream.flush();
}

/// An OpenAI Chat answer stating a `cached_tokens` reading, so the row this
/// test reads back has a real cache ratio in it rather than a zero.
fn chat_completion_answer() -> String {
    json!({
        "id": "chatcmpl-fixture",
        "object": "chat.completion",
        "created": 1,
        "model": "fixture-model",
        "choices": [{
            "index": 0,
            "message": {"role": "assistant", "content": "Checking."},
            "finish_reason": "stop",
            "logprobs": null
        }],
        "usage": {
            "prompt_tokens": 40,
            "completion_tokens": 12,
            "total_tokens": 52,
            "prompt_tokens_details": {"cached_tokens": 8}
        }
    })
    .to_string()
}

/// An Anthropic Messages answer carrying a real `usage` object — for (c),
/// where the point is that the relay records nothing from a body it never
/// reads.
const RELAYED_ANSWER: &str = r#"{"type":"message","id":"msg_relayed","role":"assistant","model":"fixture-model","content":[{"type":"text","text":"hi there"}],"stop_reason":"end_turn","stop_sequence":null,"usage":{"input_tokens":999,"output_tokens":888}}"#;

/// A loopback TCP server answering every connection with one preset body and
/// recording what it received.
struct FixtureUpstream {
    address: SocketAddr,
    requests: Arc<Mutex<Vec<RecordedRequest>>>,
    stop: Arc<AtomicBool>,
    accept: Option<JoinHandle<()>>,
}

impl FixtureUpstream {
    fn answering(body: String) -> Self {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).expect("loopback is bindable");
        let address = listener.local_addr().expect("bound");
        listener.set_nonblocking(true).expect("polling mode");
        let requests = Arc::new(Mutex::new(Vec::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let accept = std::thread::spawn({
            let requests = Arc::clone(&requests);
            let stop = Arc::clone(&stop);
            let body = Arc::new(body);
            move || {
                while !stop.load(Ordering::Relaxed) {
                    match listener.accept() {
                        Ok((mut stream, _)) => {
                            let requests = Arc::clone(&requests);
                            let body = Arc::clone(&body);
                            std::thread::spawn(move || {
                                stream.set_nonblocking(false).expect("blocking mode");
                                let _ = stream.set_read_timeout(Some(Duration::from_secs(60)));
                                let _ = stream.set_nodelay(true);
                                if let Some(request) = read_request(&mut stream) {
                                    requests.lock().unwrap().push(request);
                                    write_document(&mut stream, &body);
                                }
                            });
                        }
                        Err(_) => std::thread::sleep(Duration::from_millis(10)),
                    }
                }
            }
        });
        Self {
            address,
            requests,
            stop,
            accept: Some(accept),
        }
    }

    fn base_url(&self) -> String {
        format!("http://{}/v1", self.address)
    }

    fn root_url(&self) -> String {
        format!("http://{}", self.address)
    }

    fn requests(&self) -> Vec<RecordedRequest> {
        self.requests.lock().unwrap().clone()
    }
}

impl Drop for FixtureUpstream {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(accept) = self.accept.take() {
            let _ = accept.join();
        }
    }
}

// --- upstreams ----------------------------------------------------------------

fn chat_only_provider(fixture: &FixtureUpstream) -> Provider {
    let mut provider = glasshouse::provider::templates()
        .into_iter()
        .find(|provider| provider.name == "openai-compatible")
        .expect("the openai-compatible template exists");
    provider.name = "chat".to_owned();
    provider.protocols[0].base_url = fixture.base_url();
    provider.credential_env = vec![CHAT_CREDENTIAL_VAR.to_owned()];
    provider
}

/// `profile::gateway_upstream`, the production builder, run with the planted
/// credential set only for the duration of the call.
fn chat_upstream(provider: &Provider) -> Upstream {
    let _guard = env_lock();
    // SAFETY: `CHAT_CREDENTIAL_VAR` is unique to this test binary, set and
    // removed around the one resolve that reads it, under `ENV_LOCK` for the
    // whole window.
    unsafe {
        std::env::set_var(CHAT_CREDENTIAL_VAR, CHAT_KEY);
    }
    let upstream = glasshouse::profile::gateway_upstream(
        std::slice::from_ref(provider),
        &EnvironmentSecretStore::new(),
        &|_| false,
    );
    unsafe {
        std::env::remove_var(CHAT_CREDENTIAL_VAR);
    }
    upstream.expect("one chat-only provider with a resolvable credential builds an upstream")
}

/// A hand-built backend serving the harness's own protocol, so the gateway
/// relays instead of translating — `gateway_translate_evidence.rs`'s own
/// `upstream_serving`.
fn native_upstream(fixture: &FixtureUpstream) -> Upstream {
    let _guard = env_lock();
    // SAFETY: as above, for this file's other unique variable.
    unsafe {
        std::env::set_var(RELAY_CREDENTIAL_VAR, RELAY_KEY);
    }
    let credential = EnvironmentSecretStore::new()
        .resolve(&SecretRef::Environment {
            var: RELAY_CREDENTIAL_VAR.to_owned(),
        })
        .expect("just set");
    unsafe {
        std::env::remove_var(RELAY_CREDENTIAL_VAR);
    }
    let backend = UpstreamBackend::new(
        "fixture".to_owned(),
        vec![Route::new(
            "anthropic-messages".to_owned(),
            &["/messages"],
            &fixture.root_url(),
        )],
        credential,
        CredentialId::new(
            "fixture",
            SecretRef::Environment {
                var: RELAY_CREDENTIAL_VAR.to_owned(),
            },
        ),
        Cost::Metered,
    )
    .expect("a loopback URL is absolute and the planted key is header-safe");
    Upstream::with_failover(vec![backend]).expect("one backend")
}

// --- the ledger, and the gateway built the way the binary builds it -----------

struct LedgerFixture {
    _tmp: tempfile::TempDir,
    runtime: glasshouse::Runtime,
    ledger: Arc<EvidenceLedger>,
}

fn ledger_fixture() -> LedgerFixture {
    let tmp = tempfile::tempdir().expect("tempdir");
    let base = std::fs::canonicalize(tmp.path()).expect("canonicalize");
    let root = base.join("workspace");
    std::fs::create_dir_all(root.join(".git")).expect("project root");
    let cli = glasshouse::Cli {
        scope: Some(root.clone()),
        allow_unsafe_scope: false,
        data_dir: Some(base.join("data")),
        config_dir: Some(base.join("config")),
        log_level: None,
        log_file: None,
        log_stderr: false,
        command: None,
    };
    let runtime = glasshouse::bootstrap(&cli, &root).expect("bootstrap");
    let ledger = Arc::new(EvidenceLedger::open(&runtime).expect("open the ledger"));
    LedgerFixture {
        _tmp: tmp,
        runtime,
        ledger,
    }
}

fn start_gateway(upstream: Upstream, ledger: Arc<EvidenceLedger>) -> Gateway {
    let mut profile = LaunchProfile::native(IntegrationId::ClaudeCode);
    profile.backend = BackendResource::GlasshouseGateway;
    glasshouse::gateway::start_if_required_with_degrade_sink(
        &[profile],
        || Ok(upstream),
        None,
        Some(ledger),
        None,
        None,
        None,
    )
    .expect("loopback is bindable")
    .expect("a gateway-backed profile requires a gateway")
}

// --- Claude Code request bodies ------------------------------------------------

/// A turn whose last user message is nothing but tool results — the shape
/// migration 24's `turn_shape` calls *tool-resume*, and the first such
/// fixture request in this crate.
fn tool_resume_body(budget_tokens: u64) -> String {
    json!({
        "model": "claude-x",
        "max_tokens": 10,
        "messages": [
            {"role": "user", "content": [{"type": "text", "text": "run it"}]},
            {"role": "assistant", "content": [
                {"type": "tool_use", "id": "call_A", "name": "Bash", "input": {"command": "ls"}}
            ]},
            {"role": "user", "content": [
                {"type": "tool_result", "tool_use_id": "call_A", "content": "a.txt"}
            ]}
        ],
        "thinking": {"type": "enabled", "budget_tokens": budget_tokens},
        "metadata": {"user_id": PLANTED_USER_ID},
        "stream": false
    })
    .to_string()
}

/// A plain typed turn — *prompt*, whatever else it carries.
fn prompt_body() -> String {
    json!({
        "model": "claude-x",
        "max_tokens": 10,
        "messages": [{"role": "user", "content": "hi"}],
        "stream": false
    })
    .to_string()
}

fn messages_request(token: &str, body: &str) -> Vec<u8> {
    format!(
        "POST /v1/messages?beta=true HTTP/1.1\r\n\
         Host: 127.0.0.1\r\n\
         Authorization: Bearer {token}\r\n\
         Content-Type: application/json\r\n\
         Anthropic-Version: 2023-06-01\r\n\
         User-Agent: claude-cli/2.1.245\r\n\
         Content-Length: {}\r\n\
         \r\n\
         {body}",
        body.len()
    )
    .into_bytes()
}

fn send_and_read(address: SocketAddr, raw: &[u8]) -> Vec<u8> {
    let mut client = TcpStream::connect(address).expect("the gateway accepts connections");
    client
        .set_read_timeout(Some(CLIENT_TIMEOUT))
        .expect("a non-zero timeout is valid");
    client
        .write_all(raw)
        .expect("the gateway reads the request");
    client.flush().expect("flush");
    let mut received = Vec::new();
    client
        .read_to_end(&mut received)
        .or_else(|err| match err.kind() {
            std::io::ErrorKind::ConnectionReset => Ok(received.len()),
            _ => Err(err),
        })
        .expect("the gateway answers and then closes");
    received
}

fn status_line(response: &[u8]) -> String {
    let end = response
        .windows(2)
        .position(|window| window == b"\r\n")
        .expect("a response head has a status line");
    String::from_utf8_lossy(&response[..end]).into_owned()
}

fn wait_for_rows(
    ledger: &EvidenceLedger,
    query: ObservationQuery<'_>,
    at_least: usize,
) -> Vec<RoutingObservation> {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        let rows = ledger.recent(query, 32).expect("read the ledger");
        if rows.len() >= at_least || Instant::now() >= deadline {
            return rows;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

// --- driving `glasshouse launch` ------------------------------------------------

const LAUNCH_ENV_DUMP_VAR: &str = "GLASSHOUSE_ROUTING_SESSION_COLUMN_LAUNCH_ENV_DUMP";
const LAUNCH_STOP_VAR: &str = "GLASSHOUSE_ROUTING_SESSION_COLUMN_LAUNCH_STOP";
const LAUNCH_HARNESS_TICKS: u32 = 900;

struct Launch {
    child: std::process::Child,
}

impl std::ops::Deref for Launch {
    type Target = std::process::Child;

    fn deref(&self) -> &std::process::Child {
        &self.child
    }
}

impl std::ops::DerefMut for Launch {
    fn deref_mut(&mut self) -> &mut std::process::Child {
        &mut self.child
    }
}

impl Drop for Launch {
    fn drop(&mut self) {
        if matches!(self.child.try_wait(), Ok(Some(_))) {
            return;
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[cfg(unix)]
fn install_waiting_harness(bin_dir: &Path) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;

    let path = bin_dir.join("fake-claude-waiting");
    std::fs::write(
        &path,
        format!(
            "#!/bin/sh\n\
             env > \"${LAUNCH_ENV_DUMP_VAR}.partial\"\n\
             mv \"${LAUNCH_ENV_DUMP_VAR}.partial\" \"${LAUNCH_ENV_DUMP_VAR}\"\n\
             ticks=0\n\
             while [ ! -f \"${LAUNCH_STOP_VAR}\" ] && [ \"$ticks\" -lt {LAUNCH_HARNESS_TICKS} ]; do\n\
             ticks=$((ticks + 1)); sleep 0.1\n\
             done\n\
             exit 0\n"
        ),
    )
    .expect("write fake harness");
    let mut perms = std::fs::metadata(&path).unwrap().permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&path, perms).unwrap();
    path
}

#[cfg(windows)]
fn install_waiting_harness(bin_dir: &Path) -> PathBuf {
    let path = bin_dir.join("fake-claude-waiting.cmd");
    std::fs::write(
        &path,
        format!(
            "@echo off\r\n\
             set > \"%{LAUNCH_ENV_DUMP_VAR}%.partial\"\r\n\
             move /y \"%{LAUNCH_ENV_DUMP_VAR}%.partial\" \"%{LAUNCH_ENV_DUMP_VAR}%\" >nul\r\n\
             set /a ticks=0\r\n\
             :wait\r\n\
             if exist \"%{LAUNCH_STOP_VAR}%\" exit /b 0\r\n\
             if %ticks% GEQ {LAUNCH_HARNESS_TICKS} exit /b 0\r\n\
             set /a ticks+=1\r\n\
             ping -n 2 127.0.0.1 >nul\r\n\
             goto wait\r\n"
        ),
    )
    .expect("write fake harness");
    path
}

fn wait_for_launch_file(path: &Path, child: &mut Launch, what: &str) {
    let deadline = Instant::now() + Duration::from_secs(30);
    while !path.exists() {
        if let Some(status) = child.try_wait().expect("poll the launch") {
            panic!("the binary exited ({status}) before {what}");
        }
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// One `NAME=value` line's value from a dumped environment.
fn dumped(dump: &str, name: &str) -> String {
    dump.lines()
        .find_map(|line| line.strip_prefix(&format!("{name}=")))
        .unwrap_or_else(|| panic!("the harness's environment had no {name}:\n{dump}"))
        .trim()
        .to_owned()
}

/// A real `glasshouse launch claude-code --profile gateway-chat --headless`
/// against a chat-only fixture, with the harness held open so its gateway
/// keeps serving.
struct LaunchedSession {
    _tmp: tempfile::TempDir,
    base: PathBuf,
    root: PathBuf,
    address: SocketAddr,
    token: String,
    stop_file: PathBuf,
    launch: Launch,
}

impl LaunchedSession {
    fn start(fixture: &FixtureUpstream) -> Self {
        let tmp = tempfile::tempdir().expect("tempdir");
        let base = std::fs::canonicalize(tmp.path()).expect("canonicalize");
        let root = base.join("workspace");
        std::fs::create_dir_all(root.join(".git")).expect("project root");
        std::fs::create_dir_all(base.join("config")).expect("config dir");
        let bin_dir = base.join("bin");
        std::fs::create_dir_all(&bin_dir).expect("bin dir");
        let harness = install_waiting_harness(&bin_dir);
        let escaped = harness.display().to_string().replace('\\', "\\\\");
        std::fs::write(
            base.join("config").join("config.toml"),
            format!(
                "version = 1\n\n\
                 [integrations.claude-code]\nenabled = true\nexecutable = \"{escaped}\"\n\n\
                 [providers.chat]\ntemplate = \"openai-compatible\"\n\
                 base_url = \"{}\"\n\
                 credential_env = [\"{CHAT_CREDENTIAL_VAR}\"]\n\n\
                 [profiles.gateway-chat]\nharness = \"claude-code\"\n\n\
                 [profiles.gateway-chat.backend]\nkind = \"glasshouse-gateway\"\n",
                fixture.base_url()
            ),
        )
        .expect("write user config");

        let env_dump = base.join("harness-env.txt");
        let stop_file = base.join("stop");
        let mut launch = Launch {
            child: Command::new(env!("CARGO_BIN_EXE_glasshouse"))
                .arg("--scope")
                .arg(&root)
                .arg("--data-dir")
                .arg(base.join("data"))
                .arg("--config-dir")
                .arg(base.join("config"))
                .args([
                    "launch",
                    "claude-code",
                    "--profile",
                    "gateway-chat",
                    "--headless",
                ])
                .env(LAUNCH_ENV_DUMP_VAR, &env_dump)
                .env(LAUNCH_STOP_VAR, &stop_file)
                .env(CHAT_CREDENTIAL_VAR, CHAT_KEY)
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .expect("the glasshouse binary must be runnable"),
        };

        wait_for_launch_file(
            &env_dump,
            &mut launch,
            "the harness to record its environment",
        );
        let dump = std::fs::read_to_string(&env_dump).expect("read the harness environment");
        let base_url = dumped(&dump, "ANTHROPIC_BASE_URL");
        let token = dumped(&dump, "ANTHROPIC_AUTH_TOKEN");
        let address: SocketAddr = base_url
            .strip_prefix("http://")
            .expect("the gateway is plain loopback HTTP")
            .parse()
            .expect("the gateway's base URL is host:port");

        Self {
            _tmp: tmp,
            base,
            root,
            address,
            token,
            stop_file,
            launch,
        }
    }

    fn send(&self, body: &str) {
        let response = send_and_read(self.address, &messages_request(&self.token, body));
        assert!(
            status_line(&response).starts_with("HTTP/1.1 200"),
            "{}",
            status_line(&response)
        );
    }

    /// A `Runtime` over the *launched* binary's own data and config
    /// directories, so the session record and the ledger read back are the
    /// ones the launch wrote.
    fn runtime(&self) -> glasshouse::Runtime {
        let cli = glasshouse::Cli {
            scope: Some(self.root.clone()),
            allow_unsafe_scope: false,
            data_dir: Some(self.base.join("data")),
            config_dir: Some(self.base.join("config")),
            log_level: None,
            log_file: None,
            log_stderr: false,
            command: None,
        };
        glasshouse::bootstrap(&cli, &self.root).expect("bootstrap over the launched data dir")
    }

    /// The one session this launch recorded.
    fn session_id(&self) -> SessionId {
        let runtime = self.runtime();
        let sessions = ProjectSessions::open(&runtime).expect("open the session store");
        let records = sessions.store().list().expect("list sessions");
        assert_eq!(
            records.len(),
            1,
            "the launch records exactly one session: {records:?}"
        );
        records[0].id.clone()
    }

    /// `glasshouse routing-cost`, run as its own process against the same
    /// directories.
    fn routing_cost(&self) -> String {
        let output = Command::new(env!("CARGO_BIN_EXE_glasshouse"))
            .arg("--scope")
            .arg(&self.root)
            .arg("--data-dir")
            .arg(self.base.join("data"))
            .arg("--config-dir")
            .arg(self.base.join("config"))
            .arg("routing-cost")
            .output()
            .expect("run routing-cost");
        assert!(
            output.status.success(),
            "routing-cost must succeed: stderr={}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).into_owned()
    }

    fn stop(mut self) {
        std::fs::write(&self.stop_file, "go").expect("write the stop file");
        let status = self.launch.wait().expect("wait for the launch");
        assert!(status.success(), "the launch exited {status}");
    }
}

/// The query every launched-session test reads its rows back with.
fn launched_query() -> ObservationQuery<'static> {
    ObservationQuery {
        provider: "chat",
        model: AssignedModel::HarnessDefault.label(),
        route: Some("anthropic-messages->openai-chat"),
        harness: Some("claude-code"),
    }
}

// ===========================================================================
// (a) The launch door: the session's own id, the effort, and a tool-resume
//     shape, on the row a real launched session's gateway wrote.
// ===========================================================================

/// Map line 2019's producer, end to end through the shipped binary: a session
/// `glasshouse launch` created, on a translated pairing whose fixture states
/// `cached_tokens`, writes a routing-observation row carrying **that
/// session's own id** — the value `sessions.id` holds, not the harness's
/// `metadata.user_id` and not a native session id — together with the four-
/// word effort the request's `thinking` budget maps to and the turn shape
/// derived from its last user message.
///
/// This enters at `glasshouse launch` rather than at the gateway door
/// because the thing it proves is a line in `main.rs` (practice §35): a test
/// that called `SessionRouting::serve_session` itself would be green against
/// a build where nothing on the launch path ever did.
#[test]
fn a_launched_sessions_gateway_stamps_its_id_the_effort_and_a_tool_resume_shape() {
    let fixture = FixtureUpstream::answering(chat_completion_answer());
    let session = LaunchedSession::start(&fixture);

    // 16,000 is the "complex tasks" waypoint `canonical::EFFORT_LOW_MAX` sits
    // below and `EFFORT_MEDIUM_MAX` above, so it maps to `medium` — a word
    // that is neither end of the ladder, so a mapping stuck at either end
    // would be visible here.
    session.send(&tool_resume_body(16_000));

    let runtime = session.runtime();
    let ledger = EvidenceLedger::open(&runtime).expect("open the launched project's ledger");
    let rows = wait_for_rows(&ledger, launched_query(), 1);
    assert_eq!(rows.len(), 1, "one row for the one exchange served");

    let expected = session.session_id();
    assert_eq!(rows[0].outcome, Some(Outcome::Succeeded));
    assert_eq!(
        rows[0].session_id.as_deref(),
        Some(expected.as_str()),
        "the row must name the session this launch created"
    );
    assert_ne!(
        rows[0].session_id.as_deref(),
        Some(PLANTED_USER_ID),
        "the harness's own metadata.user_id is not a Glasshouse session id"
    );
    assert_eq!(
        rows[0].effort_level,
        Some(EffortLevel::Medium),
        "a 16,000-token thinking budget is the medium rung of the ladder"
    );
    assert_eq!(
        rows[0].turn_shape,
        Some(TurnShape::ToolResume),
        "the last user message carried nothing but a tool result"
    );
    // The measuring half of 2019, already production before this package and
    // undisturbed by it — so the ratio the per-session readout computes has
    // a real numerator.
    assert_eq!(rows[0].cached_input_tokens, Some(8));

    session.stop();
}

// ===========================================================================
// (b) A prompt-shaped request stores the other word.
// ===========================================================================

/// The same seam, the other turn shape: a request whose last user message is
/// text records `prompt`, and a request that asked for no thinking at all
/// records no effort rather than the bottom rung of the ladder.
#[test]
fn a_prompt_shaped_request_records_the_prompt_shape_and_no_invented_effort() {
    let fixture = FixtureUpstream::answering(chat_completion_answer());
    let ledger = ledger_fixture();
    let gateway = start_gateway(
        chat_upstream(&chat_only_provider(&fixture)),
        Arc::clone(&ledger.ledger),
    );
    gateway.routing().bind(
        "claude-code",
        "openai-chat",
        AssignedModel::HarnessDefault,
        gateway.upstream(),
    );
    let served = SessionId::new("ses_prompt_shaped");
    gateway.routing().serve_session(served.as_str());

    let response = send_and_read(
        gateway.address(),
        &messages_request(gateway.token().expose(), &prompt_body()),
    );
    assert!(
        status_line(&response).starts_with("HTTP/1.1 200"),
        "{}",
        status_line(&response)
    );

    let rows = wait_for_rows(&ledger.ledger, launched_query(), 1);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].session_id.as_deref(), Some(served.as_str()));
    assert_eq!(rows[0].turn_shape, Some(TurnShape::Prompt));
    assert_eq!(
        rows[0].effort_level, None,
        "no thinking was asked for, so no rung of the ladder is invented for it"
    );
}

// ===========================================================================
// (c) A relayed exchange: the session, and nothing read from a body.
// ===========================================================================

/// The restraint half. A request against an upstream serving the harness's
/// own protocol is **relayed** — the gateway never decodes it — so the row
/// still names the session the launch told it about, and records `NULL` for
/// both request-derived columns. Unread, not absent: the relayed body here
/// carries a real `usage` object precisely so that the `NULL`s mean
/// restraint rather than an empty response.
#[test]
fn a_relayed_exchange_records_the_session_and_neither_request_fact() {
    let fixture = FixtureUpstream::answering(RELAYED_ANSWER.to_owned());
    let ledger = ledger_fixture();
    let gateway = start_gateway(native_upstream(&fixture), Arc::clone(&ledger.ledger));
    assert_eq!(gateway.served_protocols(), vec!["anthropic-messages"]);
    gateway.routing().bind(
        "claude-code",
        "anthropic-messages",
        AssignedModel::named("fixture-model"),
        gateway.upstream(),
    );
    let served = SessionId::new("ses_relayed");
    gateway.routing().serve_session(served.as_str());

    // A body that WOULD have produced a tool-resume shape and a medium
    // effort if anything on this path read it.
    let response = send_and_read(
        gateway.address(),
        &messages_request(gateway.token().expose(), &tool_resume_body(16_000)),
    );
    assert!(
        status_line(&response).starts_with("HTTP/1.1 200"),
        "{}",
        status_line(&response)
    );

    let rows = wait_for_rows(
        &ledger.ledger,
        ObservationQuery {
            provider: "fixture",
            model: "fixture-model",
            route: Some("anthropic-messages"),
            harness: Some("claude-code"),
        },
        1,
    );
    assert_eq!(rows.len(), 1);
    assert_eq!(
        rows[0].session_id.as_deref(),
        Some(served.as_str()),
        "the session is told to the gateway by the launch, not read off the wire, so a \
         relayed exchange records it exactly as a translated one does"
    );
    assert_eq!(
        rows[0].effort_level, None,
        "the relay never decoded this request, and its thinking budget must not appear"
    );
    assert_eq!(rows[0].turn_shape, None);
    assert_eq!(
        rows[0].input_tokens, None,
        "the pre-existing restraint on the token columns is unchanged"
    );
}

// ===========================================================================
// (d) The readout: a per-session ratio, and words where no session was named.
// ===========================================================================

/// Map line 2019's readout half, through `glasshouse routing-cost` itself:
/// the `SAVINGS` section's translation facet gains a per-session grouping
/// beside the per-credential one it already prints, naming the launched
/// session's own id with its ratio and denominator — and saying *no session
/// recorded* in words, never `0`, for a row that names none.
///
/// Five exchanges, because a ratio is a rate and sits behind the ledger's
/// standing sample floor (`MIN_SAMPLE_FOR_SUMMARY`); the sixth planted row
/// below the floor is what proves the floor prints words rather than a
/// percentage nobody earned.
#[test]
fn routing_cost_prints_a_per_session_ratio_and_words_for_a_row_with_no_session() {
    let fixture = FixtureUpstream::answering(chat_completion_answer());
    let session = LaunchedSession::start(&fixture);
    for _ in 0..glasshouse::routing::evidence::MIN_SAMPLE_FOR_SUMMARY {
        session.send(&prompt_body());
    }

    let runtime = session.runtime();
    let ledger = EvidenceLedger::open(&runtime).expect("open the launched project's ledger");
    let rows = wait_for_rows(
        &ledger,
        launched_query(),
        glasshouse::routing::evidence::MIN_SAMPLE_FOR_SUMMARY,
    );
    assert_eq!(
        rows.len(),
        glasshouse::routing::evidence::MIN_SAMPLE_FOR_SUMMARY
    );

    // A translated row naming no session — what a build older than migration
    // 24 wrote, and what a gateway nobody told writes.
    ledger
        .record(
            NewObservation::new("planted", "planted-model")
                .with_harness(Some("claude-code"))
                .with_purpose(Some(HARNESS_TURN_PURPOSE))
                .with_route(Some("anthropic-messages->openai-chat"))
                .with_quota_context(Some("cred-planted"))
                .with_tokens(Some(90), Some(5), Some(10)),
            glasshouse::provider::cache::now_unix_seconds(),
        )
        .expect("plant a session-less translated row");

    let report = session.routing_cost();
    let expected = session.session_id();

    let facet = section(&report, "translation by session");
    assert!(
        facet.contains(expected.as_str()),
        "the per-session facet must name the launched session:\n{report}"
    );
    // Five exchanges, each stating `prompt_tokens: 40` of which
    // `cached_tokens: 8` — the decoder records the 8 apart from the 32 that
    // were not served from cache, so the group is 40 cached of 200.
    assert!(
        facet.contains("5 exchanges, prompt-cache reads 40 of 200 translated input tokens (20.0%)"),
        "expected the launched session's own counts and ratio in:\n{facet}"
    );
    assert!(
        facet.contains("(no session recorded)"),
        "a row naming no session must say so in words:\n{facet}"
    );
    assert!(
        facet.contains(
            "1 exchanges, prompt-cache reads 10 of 100 translated input tokens \
                        (not counted: 1 of 5 exchanges needed)"
        ),
        "a group below the standing sample floor must print words for its ratio, never a \
         percentage nobody earned:\n{facet}"
    );

    session.stop();
}

/// The rendered block for one `SAVINGS` facet's label, from the blank line
/// before `  {label}` to the next blank line — `tests/savings_readout.rs`'s
/// own convention.
fn section(report: &str, label: &str) -> String {
    let marker = format!("\n  {label}\n");
    let start = report
        .find(&marker)
        .unwrap_or_else(|| panic!("no section for {label:?} in:\n{report}"));
    let rest = &report[start + 1..];
    let end = rest.find("\n\n").unwrap_or(rest.len());
    rest[..end].to_owned()
}

// ===========================================================================
// (e) The migration: three NULLs for an older row, and no error for a word
//     this build does not know.
// ===========================================================================

/// A database at version 23 with a row a version-23 build wrote migrates to
/// 24 on an ordinary bootstrap and reads that row back with `NULL` in all
/// three new columns — never a zero and never an invented id — while a row
/// carrying words this build does not recognise reads back as `None` for
/// them **without an error**, the way `task_class` does and `failure_class`
/// deliberately does not.
///
/// The in-crate half of this proof is `database::tests::
/// migration_24_adds_the_session_columns_and_undoes_cleanly`, which also
/// checks the undo and the whole-schema equality. This one exists because it
/// goes through `glasshouse::bootstrap` from outside the crate, on a
/// database an older build could really have left behind.
#[test]
fn a_version_23_database_migrates_and_reads_back_three_nulls() {
    use rusqlite::Connection;

    let tmp = tempfile::tempdir().expect("tempdir");
    let base = std::fs::canonicalize(tmp.path()).expect("canonicalize");
    let root = base.join("workspace");
    std::fs::create_dir_all(root.join(".git")).expect("project root");
    let cli = glasshouse::Cli {
        scope: Some(root.clone()),
        allow_unsafe_scope: false,
        data_dir: Some(base.join("data")),
        config_dir: Some(base.join("config")),
        log_level: None,
        log_file: None,
        log_stderr: false,
        command: None,
    };
    let runtime = glasshouse::bootstrap(&cli, &root).expect("bootstrap");
    let db_path = runtime.database_path();
    let project_id: String = {
        let conn = Connection::open(&db_path).expect("open");
        conn.query_row(
            "SELECT value FROM project_metadata WHERE key = 'project_id'",
            [],
            |row| row.get(0),
        )
        .expect("the project binding")
    };
    drop(runtime);

    // Back to 23, and a row written the way a version-23 build wrote them.
    {
        let conn = Connection::open(&db_path).expect("open");
        conn.execute_batch(
            "ALTER TABLE routing_observations DROP COLUMN completed_ms;
             ALTER TABLE routing_observations DROP COLUMN first_tool_call_ms;
             ALTER TABLE routing_observations DROP COLUMN first_token_ms;
             ALTER TABLE routing_observations DROP COLUMN first_byte_ms;
             ALTER TABLE routing_observations DROP COLUMN turn_shape;
             ALTER TABLE routing_observations DROP COLUMN effort_level;
             ALTER TABLE routing_observations DROP COLUMN session_id;
             -- Migration 27's table: a rollback that leaves it in place
             -- meets `table file_claims already exists` on the re-run.
             DROP TABLE IF EXISTS file_claims;
             DELETE FROM schema_migrations WHERE version >= 24;",
        )
        .expect("roll back to 23");
        conn.execute(
            "INSERT INTO routing_observations (project_id, observed_at, provider, model, outcome)
             VALUES (?1, 1, 'older-build', 'm', 'succeeded')",
            [&project_id],
        )
        .expect("a version-23 row");
    }

    // Forward, through the same bootstrap a launch runs.
    let migrated = glasshouse::bootstrap(&cli, &root).expect("the upgrade bootstrap");
    {
        let ledger = EvidenceLedger::open(&migrated).expect("open the ledger");
        let older = ledger
            .recent(
                ObservationQuery {
                    provider: "older-build",
                    model: "m",
                    route: None,
                    harness: None,
                },
                1,
            )
            .expect("read the older row");
        assert_eq!(older.len(), 1);
        assert_eq!(
            older[0].session_id, None,
            "a row from before the column existed names no session, and no id is invented"
        );
        assert_eq!(older[0].effort_level, None);
        assert_eq!(older[0].turn_shape, None);
    }

    // A future build's words: read back as nothing recorded, and the row
    // still reads.
    {
        let conn = Connection::open(&db_path).expect("open");
        conn.execute(
            "INSERT INTO routing_observations
                 (project_id, observed_at, provider, model, outcome,
                  session_id, effort_level, turn_shape)
             VALUES (?1, 2, 'future-build', 'm', 'succeeded',
                     'ses_future', 'transcendent', 'interpretive dance')",
            [&project_id],
        )
        .expect("a future-build row");
    }
    {
        let ledger = EvidenceLedger::open(&migrated).expect("open the ledger");
        let future = ledger
            .recent(
                ObservationQuery {
                    provider: "future-build",
                    model: "m",
                    route: None,
                    harness: None,
                },
                1,
            )
            .expect("the row reads, it does not error");
        assert_eq!(future.len(), 1);
        assert_eq!(future[0].effort_level, None);
        assert_eq!(future[0].turn_shape, None);
        assert_eq!(future[0].outcome, Some(Outcome::Succeeded));
    }
}

// ===========================================================================
// (f) Nothing else reached the row.
// ===========================================================================

/// The security invariant, checked against the database file's own bytes
/// rather than against the typed row: neither the harness's
/// `metadata.user_id` nor the provider credential appears anywhere in the
/// project database after an exchange carrying both has been served and
/// recorded.
///
/// A byte scan and not a column scan on purpose — a column scan proves only
/// that the columns this test knows to look at are clean, and the whole
/// point of `an_exchange_has_nowhere_to_put_a_body` next door is that the
/// hazard is a *new* place appearing to put one.
#[test]
fn no_harness_identifier_and_no_credential_reaches_any_row() {
    let fixture = FixtureUpstream::answering(chat_completion_answer());
    let ledger = ledger_fixture();
    let gateway = start_gateway(
        chat_upstream(&chat_only_provider(&fixture)),
        Arc::clone(&ledger.ledger),
    );
    gateway.routing().bind(
        "claude-code",
        "openai-chat",
        AssignedModel::HarnessDefault,
        gateway.upstream(),
    );
    let served = SessionId::new("ses_isolation");
    gateway.routing().serve_session(served.as_str());

    let response = send_and_read(
        gateway.address(),
        &messages_request(gateway.token().expose(), &tool_resume_body(16_000)),
    );
    assert!(
        status_line(&response).starts_with("HTTP/1.1 200"),
        "{}",
        status_line(&response)
    );
    let rows = wait_for_rows(&ledger.ledger, launched_query(), 1);
    assert_eq!(rows.len(), 1, "the row this test is about must exist");
    assert_eq!(rows[0].session_id.as_deref(), Some(served.as_str()));

    // The fixture really did receive both, so `!contains` below has
    // something to bite: the credential on the wire, and the user id in the
    // translated body.
    let received = fixture.requests();
    assert_eq!(received.len(), 1);
    assert_eq!(
        received[0].json()["prompt_cache_key"],
        PLANTED_USER_ID,
        "the request that produced this row really did carry the harness's user id"
    );

    // Every handle closed before the file is read, so nothing is still in a
    // write-ahead log this scan would miss.
    drop(gateway);
    let bytes = std::fs::read(ledger.runtime.database_path()).expect("read the project database");
    let text = String::from_utf8_lossy(&bytes);
    assert!(
        !text.contains(PLANTED_USER_ID),
        "the harness's metadata.user_id must never reach this database"
    );
    assert!(
        !text.contains(CHAT_KEY),
        "the provider credential must never reach this database"
    );
    assert!(
        text.contains(served.as_str()),
        "the scan must be able to see the session id that IS stored, or it proves nothing"
    );
}
