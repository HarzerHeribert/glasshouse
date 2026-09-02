//! `GH-TASK-CLASS-COST-JOIN`, capability map line 1301 — the join
//! `docs/product/evidence/phase-32g.md`'s Censused 2026-09-02 entry named
//! missing: *"`record_routing_latency` writes `task_class` on a row with no
//! tokens; `record_routing_observation` writes tokens on a row with no
//! class."* A gateway-served row now learns the launch's task class the way
//! it already learns its session id
//! (`crate::gateway::session::SessionRouting::serve_task_class`), a reader
//! (`crate::routing::burn::output_tokens_by_class`) turns recent comparable
//! rows into a median output size per class, and the router's
//! expected-marginal-cost evidence
//! (`crate::routing::session::expected_marginal_cost`) states the expected
//! output cost from it.
//!
//! # Where these tests enter
//!
//! (a) and (e) go through **`glasshouse launch`** itself — the production
//! caller `main.rs::launch_session` calls beside `serve_session` — because
//! practice §35 is explicit that a caller every test bypasses is not a
//! caller: a test that called `SessionRouting::serve_task_class` directly
//! would pass against a build where the shipped binary never called it at
//! all. The fixture (`FixtureUpstream`, `LaunchedSession`, the chat-request
//! bodies) is copied from `tests/routing_session_column.rs`, trimmed to what
//! this file needs — that file's own header explains why every pair file in
//! this crate carries its own copy rather than a shared module.
//!
//! (b), (c) and (d) go through **`glasshouse route --task`** — the same
//! shipped-binary diagnostic `tests/route_command.rs`'s pricing tests
//! already prove `expected_marginal_cost`'s mechanism against — with rows
//! planted directly in the evidence ledger under the same `--data-dir` the
//! route command reads. The fixture (`Fixture`, `install_fake_harness`,
//! `plant_pricing`) is copied from `tests/route_command.rs`.

use std::io::{Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use glasshouse::routing::AssignedModel;
use glasshouse::routing::evidence::{
    EvidenceLedger, HARNESS_TURN_PURPOSE, MIN_SAMPLE_FOR_SUMMARY, NewObservation, ObservationQuery,
    Outcome, ROUTING_LATENCY_PURPOSE,
};
use serde_json::json;

/// A task text that heuristically classifies as `code modification` with no
/// routing model configured — `"fix "` and `".rs"` are both signal words
/// `routing::classify::classify_heuristically` reads (`fix ` is a
/// code-modification keyword, `.rs` is a repository reference), so this
/// classifies the same way with or without a routing model in front of it.
const CODE_MOD_TASK: &str = "fix the bug in foo.rs";

// ===========================================================================
// (a) and (e): a real launch, through `glasshouse launch --task`.
// ===========================================================================

const CHAT_KEY: &str = "sk-planted-task-class-cost-join-000111";
const CHAT_CREDENTIAL_VAR: &str = "GLASSHOUSE_TASK_CLASS_COST_JOIN_CHAT_TEST_KEY";

fn read_request(stream: &mut TcpStream) -> Option<()> {
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
    Some(())
}

fn write_document(stream: &mut TcpStream, body: &str) {
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
        body.len()
    );
    let _ = stream.write_all(response.as_bytes());
    let _ = stream.flush();
}

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
        "usage": {"prompt_tokens": 40, "completion_tokens": 12, "total_tokens": 52}
    })
    .to_string()
}

/// A loopback TCP server answering every connection with one preset body.
struct FixtureUpstream {
    address: SocketAddr,
    stop: Arc<AtomicBool>,
    accept: Option<JoinHandle<()>>,
}

impl FixtureUpstream {
    fn answering(body: String) -> Self {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).expect("loopback is bindable");
        let address = listener.local_addr().expect("bound");
        listener.set_nonblocking(true).expect("polling mode");
        let stop = Arc::new(AtomicBool::new(false));
        let accept = std::thread::spawn({
            let stop = Arc::clone(&stop);
            let body = Arc::new(body);
            move || {
                while !stop.load(Ordering::Relaxed) {
                    match listener.accept() {
                        Ok((mut stream, _)) => {
                            let body = Arc::clone(&body);
                            std::thread::spawn(move || {
                                stream.set_nonblocking(false).expect("blocking mode");
                                let _ = stream.set_read_timeout(Some(Duration::from_secs(60)));
                                let _ = stream.set_nodelay(true);
                                if read_request(&mut stream).is_some() {
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
            stop,
            accept: Some(accept),
        }
    }

    fn base_url(&self) -> String {
        format!("http://{}/v1", self.address)
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

fn prompt_body() -> String {
    json!({
        "model": "claude-x",
        "max_tokens": 10,
        "messages": [{"role": "user", "content": "hi"}],
        "stream": false
    })
    .to_string()
}

fn send_and_read(address: SocketAddr, raw: &[u8]) -> Vec<u8> {
    let mut client = TcpStream::connect(address).expect("the gateway accepts connections");
    client
        .set_read_timeout(Some(Duration::from_secs(20)))
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
) -> Vec<glasshouse::routing::evidence::RoutingObservation> {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        let rows = ledger.recent(query, 32).expect("read the ledger");
        if rows.len() >= at_least || Instant::now() >= deadline {
            return rows;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

const LAUNCH_ENV_DUMP_VAR: &str = "GLASSHOUSE_TASK_CLASS_COST_JOIN_LAUNCH_ENV_DUMP";
const LAUNCH_STOP_VAR: &str = "GLASSHOUSE_TASK_CLASS_COST_JOIN_LAUNCH_STOP";
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

fn dumped(dump: &str, name: &str) -> String {
    dump.lines()
        .find_map(|line| line.strip_prefix(&format!("{name}=")))
        .unwrap_or_else(|| panic!("the harness's environment had no {name}:\n{dump}"))
        .trim()
        .to_owned()
}

/// A real `glasshouse launch claude-code --profile gateway-chat --headless
/// --task <CODE_MOD_TASK>` against a chat-only fixture, with the harness
/// held open so its gateway keeps serving.
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
                    "--task",
                    CODE_MOD_TASK,
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

    fn stop(mut self) {
        std::fs::write(&self.stop_file, "go").expect("write the stop file");
        let status = self.launch.wait().expect("wait for the launch");
        assert!(status.success(), "the launch exited {status}");
    }
}

fn launched_query() -> ObservationQuery<'static> {
    ObservationQuery {
        provider: "chat",
        model: AssignedModel::HarnessDefault.label(),
        route: Some("anthropic-messages->openai-chat"),
        harness: Some("claude-code"),
    }
}

/// `record_routing_latency`'s own row for this launch — `provider =
/// "glasshouse"`, `model = "session-router"`, no route.
fn routing_latency_query() -> ObservationQuery<'static> {
    ObservationQuery {
        provider: "glasshouse",
        model: "session-router",
        route: None,
        harness: Some("claude-code"),
    }
}

// ===========================================================================
// (a) A launch the router classifies: its gateway rows carry that class.
// ===========================================================================

/// Map line 1301's producer, end to end through the shipped binary: a
/// session `glasshouse launch --task` classified as `code modification`
/// writes a gateway-served routing-observation row carrying that class —
/// the same join `record_routing_latency`'s own row already made, now made
/// by the seam that never had it.
///
/// Enters at `glasshouse launch` rather than at the gateway door: the thing
/// this proves is a line in `main.rs` (practice §35), and a test that
/// called `SessionRouting::serve_task_class` itself would be green against a
/// build where nothing on the launch path ever did.
#[test]
fn a_launched_sessions_classified_task_stamps_its_gateways_served_rows() {
    let fixture = FixtureUpstream::answering(chat_completion_answer());
    let session = LaunchedSession::start(&fixture);

    session.send(&prompt_body());

    let runtime = session.runtime();
    let ledger = EvidenceLedger::open(&runtime).expect("open the launched project's ledger");
    let rows = wait_for_rows(&ledger, launched_query(), 1);
    assert_eq!(rows.len(), 1, "one row for the one exchange served");
    assert_eq!(rows[0].outcome, Some(Outcome::Succeeded));
    assert_eq!(
        rows[0].task_class,
        Some(glasshouse::routing::request::TaskClass::CodeModification),
        "`{CODE_MOD_TASK}` classifies as code modification with no routing model configured, \
         and the gateway's own served-exchange row must carry it: {:?}",
        rows[0]
    );

    session.stop();
}

// ===========================================================================
// (e) `record_routing_latency`'s own row is unchanged by this package.
// ===========================================================================

/// The same launch's routing-decision row — the one `record_routing_latency`
/// wrote before this package existed and is untouched by it: it still
/// carries the classified task class, and it still carries no tokens, since
/// nothing about this producer's own inputs changed.
#[test]
fn record_routing_latencys_own_row_is_unchanged_by_this_package() {
    let fixture = FixtureUpstream::answering(chat_completion_answer());
    let session = LaunchedSession::start(&fixture);

    session.send(&prompt_body());

    let runtime = session.runtime();
    let ledger = EvidenceLedger::open(&runtime).expect("open the launched project's ledger");
    let rows = wait_for_rows(&ledger, routing_latency_query(), 1);
    assert_eq!(
        rows.len(),
        1,
        "the routing decision itself writes exactly one row"
    );
    assert_eq!(rows[0].purpose.as_deref(), Some(ROUTING_LATENCY_PURPOSE));
    assert_eq!(
        rows[0].task_class,
        Some(glasshouse::routing::request::TaskClass::CodeModification),
        "this row has carried the classified task class since Phase 34C/`with_task_class` \
         landed, and this package must not disturb it: {:?}",
        rows[0]
    );
    assert_eq!(
        rows[0].output_tokens, None,
        "the routing decision reads no provider response and must still record no tokens: {:?}",
        rows[0]
    );
    assert_eq!(
        rows[0].input_tokens, None,
        "the routing decision reads no provider response and must still record no tokens: {:?}",
        rows[0]
    );

    session.stop();
}

// ===========================================================================
// (b), (c), (d): `glasshouse route --task`, with rows planted in the ledger.
// ===========================================================================

const PRICING_CREDENTIAL_VAR: &str = "GLASSHOUSE_TASK_CLASS_COST_JOIN_PRICING_TEST_KEY";

/// A project with a fake `claude-code` and one metered, priced destination —
/// `tests/route_command.rs`'s own `PRICING_PROFILES` fixture, trimmed to the
/// one profile this file needs.
const PRICED_PROFILE: &str = "\n\
     [providers.pricing-probe]\ntemplate = \"openrouter\"\n\
     credential_env = [\"GLASSHOUSE_TASK_CLASS_COST_JOIN_PRICING_TEST_KEY\"]\n\n\
     [profiles.priced]\nharness = \"claude-code\"\n\
     expected_protocol = \"anthropic-messages\"\n\
     model = \"task-class-cost-join-model\"\n\n\
     [profiles.priced.backend]\nkind = \"direct-provider\"\n\
     provider = \"pricing-probe\"\n";

/// Write each distinct fixture executable once per test binary — the same
/// reason `tests/route_command.rs`'s own `shared_fixture` exists (project
/// memory `gatekeeper-scans-make-pty-fixtures-flaky`): a fresh executable on
/// macOS pays a Gatekeeper scan the first time it is run, and this file
/// never runs the harness at all (`glasshouse route` decides and starts
/// nothing), so the scan cost is the only thing sharing saves here — but it
/// is still one binary's worth of `.rs` files creating their own copy of
/// this pattern rather than a shared module, `tests/routing_session_column.rs`'s
/// own header explains why.
fn shared_fake_harness() -> PathBuf {
    use std::sync::OnceLock;

    static PATH: OnceLock<PathBuf> = OnceLock::new();
    PATH.get_or_init(|| {
        let dir = Box::leak(Box::new(tempfile::tempdir().expect("shared fixture dir")));
        let path = dir.path().join(if cfg!(windows) {
            "fake-claude-code.cmd"
        } else {
            "fake-claude-code"
        });
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::write(&path, "#!/bin/sh\nexit 0\n").expect("write fake harness");
            let mut perms = std::fs::metadata(&path).unwrap().permissions();
            perms.set_mode(0o755);
            std::fs::set_permissions(&path, perms).unwrap();
        }
        #[cfg(windows)]
        {
            std::fs::write(&path, "@echo off\r\nexit /b 0\r\n").expect("write fake harness");
        }
        path
    })
    .clone()
}

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

        let harness = shared_fake_harness();
        let escaped = harness.display().to_string().replace('\\', "\\\\");
        let config_dir = base.join("config");
        std::fs::create_dir_all(&config_dir).expect("create config dir");
        std::fs::write(
            config_dir.join("config.toml"),
            format!(
                "version = 1\n\n\
                 [integrations.claude-code]\nenabled = true\nexecutable = \"{escaped}\"\n\
                 {PRICED_PROFILE}"
            ),
        )
        .expect("write user config");

        Self {
            _tmp: tmp,
            base,
            root,
        }
    }

    fn stdout(&self, args: &[&str]) -> String {
        let output = Command::new(env!("CARGO_BIN_EXE_glasshouse"))
            .arg("--scope")
            .arg(&self.root)
            .arg("--data-dir")
            .arg(self.base.join("data"))
            .arg("--config-dir")
            .arg(self.base.join("config"))
            .args(args)
            .env(PRICING_CREDENTIAL_VAR, "planted-opaque-value")
            .env("PATH", self.base.join("empty-path"))
            .output()
            .expect("the glasshouse binary must be runnable");
        String::from_utf8_lossy(&output.stdout).into_owned()
    }

    fn data_dir(&self) -> PathBuf {
        self.base.join("data")
    }

    /// A `Runtime` over this fixture's own `--data-dir`/`--config-dir`, so a
    /// ledger opened through it is the one `glasshouse route` reads.
    fn runtime(&self) -> glasshouse::Runtime {
        let cli = glasshouse::Cli {
            scope: Some(self.root.clone()),
            allow_unsafe_scope: false,
            data_dir: Some(self.data_dir()),
            config_dir: Some(self.base.join("config")),
            log_level: None,
            log_file: None,
            log_stderr: false,
            command: None,
        };
        glasshouse::bootstrap(&cli, &self.root).expect("bootstrap over the fixture's own dirs")
    }
}

fn plant_pricing(fixture: &Fixture, contents: &str) {
    std::fs::write(
        fixture
            .base
            .join("config")
            .join(glasshouse::provider::pricing::PRICING_FILE_NAME),
        contents,
    )
    .expect("write pricing.toml");
}

/// Plant `count` `HARNESS_TURN_PURPOSE` rows for `class`, with output-token
/// sizes `1000, 1100, 1200, ...` — a distinct value per row so a median
/// computed over the wrong subset would print a different number than the
/// one these tests assert on.
fn plant_output_rows_from(
    ledger: &EvidenceLedger,
    class: glasshouse::routing::request::TaskClass,
    count: usize,
    base: i64,
) {
    let now = glasshouse::provider::cache::now_unix_seconds();
    for i in 0..count {
        ledger
            .record(
                NewObservation::new("planted-provider", "planted-model")
                    .with_purpose(Some(HARNESS_TURN_PURPOSE))
                    .with_task_class(Some(class))
                    .with_tokens(None, Some(base + 100 * i as i64), None),
                now,
            )
            .expect("plant a comparable-output row");
    }
}

/// `plant_output_rows_from`, at the same base every non-adversarial test
/// here uses: `1000, 1100, 1200, ...` — a distinct value per row so a
/// median computed over the wrong subset would print a different number
/// than the one these tests assert on.
fn plant_output_rows(
    ledger: &EvidenceLedger,
    class: glasshouse::routing::request::TaskClass,
    count: usize,
) {
    plant_output_rows_from(ledger, class, count, 1000);
}

// ---------------------------------------------------------------------------
// (b) Above the floor: the median and the output-cost figure are cited.
// ---------------------------------------------------------------------------

#[test]
fn rows_above_the_floor_are_cited_with_their_median_and_output_cost() {
    let fixture = Fixture::new();
    plant_pricing(
        &fixture,
        r#"
        [[prices]]
        provider = "pricing-probe"
        model = "task-class-cost-join-model"
        input_per_million_usd = 3.0
        output_per_million_usd = 9.0
        "#,
    );
    let runtime = fixture.runtime();
    let ledger = EvidenceLedger::open(&runtime).expect("open the fixture's own ledger");
    plant_output_rows(
        &ledger,
        glasshouse::routing::request::TaskClass::CodeModification,
        MIN_SAMPLE_FOR_SUMMARY,
    );
    // A second class, tiny sizes far from `code modification`'s own — so a
    // reader that pools every class together (rather than filtering by it)
    // pulls the median far enough from 1200 that this test's own assertion
    // below would fail rather than pass for the wrong reason.
    plant_output_rows_from(
        &ledger,
        glasshouse::routing::request::TaskClass::Question,
        MIN_SAMPLE_FOR_SUMMARY,
        10,
    );
    drop(ledger);
    drop(runtime);

    let report = fixture.stdout(&["route", "--task", CODE_MOD_TASK]);
    // Five rows of 1000, 1100, 1200, 1300, 1400 — the median is 1200, and
    // 1200 tokens at $9.00/million output is exactly $0.0108.
    assert!(
        report.contains(&format!("{MIN_SAMPLE_FOR_SUMMARY} in the window"))
            && report.contains("median of 1200 output tokens")
            && report.contains("$0.0108"),
        "the real route output must cite the planted rows' median and the output cost it \
         implies:\n{report}"
    );
    // The magnitude never moves — line 1298's own precedent, restated for
    // the output half: only the evidence gained a figure.
    assert!(
        report.contains("+0.000  expected marginal cost"),
        "the output-cost evidence must never change this term's magnitude:\n{report}"
    );
}

// ---------------------------------------------------------------------------
// (c) Below the floor: unmeasured, with the floor named.
// ---------------------------------------------------------------------------

#[test]
fn rows_below_the_floor_read_as_unmeasured_with_the_floor_named() {
    let fixture = Fixture::new();
    plant_pricing(
        &fixture,
        r#"
        [[prices]]
        provider = "pricing-probe"
        model = "task-class-cost-join-model"
        input_per_million_usd = 3.0
        output_per_million_usd = 9.0
        "#,
    );
    let runtime = fixture.runtime();
    let ledger = EvidenceLedger::open(&runtime).expect("open the fixture's own ledger");
    plant_output_rows(
        &ledger,
        glasshouse::routing::request::TaskClass::CodeModification,
        MIN_SAMPLE_FOR_SUMMARY - 1,
    );
    drop(ledger);
    drop(runtime);

    let report = fixture.stdout(&["route", "--task", CODE_MOD_TASK]);
    assert!(
        report.contains(&format!(
            "expected output size unmeasured (fewer than {MIN_SAMPLE_FOR_SUMMARY} comparable \
             code modification tasks recorded)"
        )),
        "rows below the standing floor must read as unmeasured, with the floor named, never a \
         size nobody earned:\n{report}"
    );
    assert!(
        !report.contains("median of"),
        "a size below the floor must never be reported: {report}"
    );
}

// ---------------------------------------------------------------------------
// (d) No task classified at all: the words, never an invented class.
// ---------------------------------------------------------------------------

#[test]
fn with_no_task_classified_the_evidence_says_no_class_established() {
    let fixture = Fixture::new();
    plant_pricing(
        &fixture,
        r#"
        [[prices]]
        provider = "pricing-probe"
        model = "task-class-cost-join-model"
        input_per_million_usd = 3.0
        output_per_million_usd = 9.0
        "#,
    );
    // No rows planted and no `--task` passed: nothing to measure and no
    // class to measure it against.

    let report = fixture.stdout(&["route"]);
    assert!(
        report.contains("expected output size unmeasured (no task class established)"),
        "with no task classified, the evidence must say so in words, never borrow a class \
         nobody named:\n{report}"
    );
}
