//! Phase 34D, 34E, 35A and line 1849 — classification on the path that
//! **acts**, entered through the shipped binary.
//!
//! Every binary-level test here runs `glasshouse launch --headless` against a
//! fake harness that logs its argv and exits, with a routing model served by
//! a canned loopback endpoint that remembers every request body it was sent.
//! The assertions are on three things the binary cannot fake: **the wire**
//! (what the routing model was actually shown, and how many times it was
//! asked), **the harness's argv** (which session the decision landed on), and
//! **the evidence ledger** (what routing recorded). A build that classifies
//! through a seam nothing calls, or that sends the raw task instead of the
//! router request, fails on the first of those.
//!
//! The tier gate and tier fit (lines 1516 and 1531) are proven at the library
//! level with hand-built destinations, and say so: nothing in the shipped
//! binary states a destination's workload ceiling, so the gate is inert on
//! its path — see `Destination::with_tier_ceiling`'s own doc comment and the
//! evidence entry.

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use clap::Parser;
use glasshouse::config::pairing::{WarmSession, WarmSessionState};
use glasshouse::harness::pairing::PairingOverrides;
use glasshouse::integrations::IntegrationId;
use glasshouse::routing::classify::{
    Confidence, DurationClass, ExecutionShape, WorkloadTier, parse_classification,
};
use glasshouse::routing::evidence::{EvidenceLedger, ObservationQuery};
use glasshouse::routing::free::FreePool;
use glasshouse::routing::session::{
    Destination, RouterInputs, RoutingMoment, SessionRouter, TaskRequirements,
};
use glasshouse::routing::{
    AssignedModel, Backend, Cost, CredentialId, HardConstraint, ToolSemantics,
};
use glasshouse::secret::SecretRef;
use glasshouse::{Cli, Runtime};

/// The one credential variable every fixture provider reads. A name; the
/// value below is the fabricated string the fake endpoint sees in the
/// `authorization` header and nowhere else.
const CREDENTIAL_VAR: &str = "GLASSHOUSE_LAUNCH_CLASSIFIER_TEST_KEY";
const CREDENTIAL: &str = "fabricated-launch-classifier-value-not-a-real-credential";

/// The routing model, as the fixture's configuration names it.
const ROUTING_PROVIDER: &str = "router-runner";
const ROUTING_MODEL: &str = "router-model";
/// What `ConfiguredModel::describe` renders for it, and therefore what every
/// explanation attributes a model answer to.
const ROUTING_MODEL_LABEL: &str = "router-runner/router-model via openai-chat";

/// The direct provider the `metered` launch profile runs on. Quota readings
/// planted for it reach the router as a **band** on the wire.
const DIRECT_PROVIDER: &str = "route-probe";

/// A classification no heuristic can produce: `frontier` has no heuristic
/// producer at all, so its presence in a decision is a fact only the model's
/// own answer explains.
const FRONTIER_ANSWER: &str = r#"{
  "needs_repo_context": true,
  "needs_code_modification": true,
  "needs_shell_execution": true,
  "needs_browser_interaction": true,
  "complexity": "complex",
  "likely_multi_turn": true,
  "workload_tier": "frontier",
  "safe_for_disposable_model": false,
  "warm_context": "prefer_warm",
  "confidence": "high",
  "expected_duration": "long_running",
  "execution_shape": "reuse_session"
}"#;

/// Line 1467's "low-risk": nothing modified, nothing executed, a modest tier,
/// and confidence the router may act on.
const LOW_RISK_ANSWER: &str = r#"{
  "needs_repo_context": false,
  "needs_code_modification": false,
  "needs_shell_execution": false,
  "needs_browser_interaction": false,
  "complexity": "trivial",
  "likely_multi_turn": false,
  "workload_tier": "leaf",
  "safe_for_disposable_model": true,
  "warm_context": "prefer_stronger_cold",
  "confidence": "medium",
  "expected_duration": "single_turn",
  "execution_shape": "reuse_session"
}"#;

/// Line 1459's input: a model that states `standard` and admits it is
/// guessing. The decision must use `heavy`.
const LOW_CONFIDENCE_ANSWER: &str = r#"{
  "needs_repo_context": true,
  "needs_code_modification": true,
  "needs_shell_execution": false,
  "needs_browser_interaction": false,
  "complexity": "moderate",
  "likely_multi_turn": true,
  "workload_tier": "standard",
  "safe_for_disposable_model": false,
  "warm_context": "prefer_warm",
  "confidence": "low"
}"#;

// ---------------------------------------------------------------------------
// A canned OpenAI chat-completions endpoint — the same shape
// `tests/classification_call.rs` uses, kept local so this target reads on
// its own.
// ---------------------------------------------------------------------------

/// One request as it actually arrived on the wire.
#[derive(Debug, Clone)]
struct Seen {
    body: String,
}

enum Answer {
    Content(String),
    ServerError,
}

struct FakeModel {
    address: SocketAddr,
    seen: Arc<Mutex<Vec<Seen>>>,
    stop: Arc<AtomicBool>,
}

impl FakeModel {
    fn answering(content: &str) -> Self {
        let content = content.to_owned();
        Self::start(move || Answer::Content(content.clone()))
    }

    fn failing() -> Self {
        Self::start(|| Answer::ServerError)
    }

    fn start(responder: impl Fn() -> Answer + Send + Sync + 'static) -> Self {
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
                        serve(stream, &thread_seen, &responder);
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

fn serve(
    mut stream: TcpStream,
    seen: &Arc<Mutex<Vec<Seen>>>,
    responder: &(impl Fn() -> Answer + ?Sized),
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

    let response = match responder() {
        Answer::Content(content) => {
            let document = serde_json::json!({
                "choices": [{ "message": { "role": "assistant", "content": content } }],
                "usage": { "prompt_tokens": 314, "completion_tokens": 15 }
            })
            .to_string();
            format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\
                 connection: close\r\n\r\n{document}",
                document.len()
            )
        }
        Answer::ServerError => "HTTP/1.1 500 Internal Server Error\r\ncontent-length: 0\r\n\
             connection: close\r\n\r\n"
            .to_owned(),
    };
    let _ = stream.write_all(response.as_bytes());
    let _ = stream.flush();
}

// ---------------------------------------------------------------------------
// A project with a fake harness, a direct-provider profile, and — when a test
// asks for one — a pinned routing model at the fake endpoint.
// ---------------------------------------------------------------------------

struct Fixture {
    _tmp: tempfile::TempDir,
    base: PathBuf,
    root: PathBuf,
    argv_log: PathBuf,
}

impl Fixture {
    /// A project whose routing model is pinned to the fake endpoint at
    /// `model_base_url`, or — `None` — one with no routing model at all, so
    /// `RoutingModelResolution::Heuristics` answers (line 1471).
    fn new(model_base_url: Option<&str>) -> Self {
        let tmp = tempfile::tempdir().expect("tempdir");
        let base = tmp.path().to_path_buf();
        let root = base.join("workspace");
        std::fs::create_dir_all(root.join(".git")).expect("create project root");
        let root = std::fs::canonicalize(&root).expect("canonicalize project root");

        let bin_dir = base.join("bin");
        std::fs::create_dir_all(&bin_dir).expect("create bin dir");
        let argv_log = base.join("argv.log");
        let harness = install_fake_harness(&bin_dir);
        let escaped = harness.display().to_string().replace('\\', "\\\\");

        let routing = match model_base_url {
            Some(url) => format!(
                "\n[providers.{ROUTING_PROVIDER}]\ntemplate = \"openai-compatible\"\n\
                 base_url = \"{url}\"\ncredential_env = [\"{CREDENTIAL_VAR}\"]\n\
                 free_models = [\"{ROUTING_MODEL}\"]\n\n\
                 [routing]\nmodel = {{ kind = \"pinned\", provider = \"{ROUTING_PROVIDER}\", \
                 model = \"{ROUTING_MODEL}\" }}\n"
            ),
            None => String::new(),
        };
        let config_dir = base.join("config");
        std::fs::create_dir_all(&config_dir).expect("create config dir");
        std::fs::write(
            config_dir.join("config.toml"),
            format!(
                "version = 1\n\n\
                 [integrations.claude-code]\nenabled = true\nexecutable = \"{escaped}\"\n\n\
                 [providers.{DIRECT_PROVIDER}]\ntemplate = \"openrouter\"\n\
                 credential_env = [\"{CREDENTIAL_VAR}\"]\n\n\
                 [profiles.metered]\nharness = \"claude-code\"\n\
                 expected_protocol = \"anthropic-messages\"\n\n\
                 [profiles.metered.backend]\nkind = \"direct-provider\"\n\
                 provider = \"{DIRECT_PROVIDER}\"\n{routing}"
            ),
        )
        .expect("write user config");

        Self {
            _tmp: tmp,
            base,
            root,
            argv_log,
        }
    }

    fn glasshouse_with_env(&self, args: &[&str], env: &[(&str, &str)]) -> Output {
        let mut command = Command::new(env!("CARGO_BIN_EXE_glasshouse"));
        command
            .arg("--scope")
            .arg(&self.root)
            .arg("--data-dir")
            .arg(self.data_dir())
            .arg("--config-dir")
            .arg(self.base.join("config"))
            .args(args)
            .env(CREDENTIAL_VAR, CREDENTIAL)
            .env(ARGV_LOG_VAR, &self.argv_log)
            .env("PATH", self.base.join("empty-path"));
        for (name, value) in env {
            command.env(name, value);
        }
        command
            .output()
            .expect("the glasshouse binary must be runnable")
    }

    fn glasshouse(&self, args: &[&str]) -> Output {
        self.glasshouse_with_env(args, &[])
    }

    /// `glasshouse launch claude-code --headless <args>`, and both streams.
    fn launch(&self, args: &[&str]) -> (Output, String) {
        let mut full = vec!["launch", "claude-code", "--headless"];
        full.extend_from_slice(args);
        let out = self.glasshouse(&full);
        let said = both_streams(&out);
        (out, said)
    }

    fn data_dir(&self) -> PathBuf {
        self.base.join("data")
    }

    /// Every argv the harness has been started with, oldest first.
    fn harness_invocations(&self) -> Vec<String> {
        match std::fs::read_to_string(&self.argv_log) {
            Ok(log) => log.lines().map(str::to_owned).collect(),
            Err(_) => Vec::new(),
        }
    }

    /// A runtime over the same directories the binary was run with, for
    /// reading back what it recorded.
    fn runtime(&self) -> Runtime {
        let cli = Cli::try_parse_from([
            "glasshouse",
            "--data-dir",
            self.data_dir().to_str().unwrap(),
            "--config-dir",
            self.base.join("config").to_str().unwrap(),
        ])
        .unwrap();
        glasshouse::bootstrap(&cli, &self.root).unwrap()
    }

    /// How many `routing-latency` rows the evidence ledger holds — map line
    /// 1849's own record, read through the same ledger the binary wrote.
    fn routing_latency_rows(&self) -> usize {
        let runtime = self.runtime();
        let ledger = EvidenceLedger::open(&runtime).expect("open the evidence ledger");
        ledger
            .recent(
                ObservationQuery {
                    provider: "glasshouse",
                    model: "session-router",
                    route: None,
                    harness: Some("claude-code"),
                },
                64,
            )
            .expect("read routing observations")
            .into_iter()
            .filter(|row| row.purpose.as_deref() == Some("routing-latency"))
            .count()
    }
}

fn both_streams(output: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

/// The env var each spawned harness reads its argv-log destination from,
/// set per spawn by [`Fixture::glasshouse_with_env`] rather than baked into
/// the script bytes — see [`shared_fixture`]'s doc for why.
const ARGV_LOG_VAR: &str = "GLASSHOUSE_TEST_ARGV_LOG";

/// Write each distinct fixture executable once per test binary instead of
/// once per test, so macOS Gatekeeper (`syspolicyd`/XProtect) validates it
/// once per run instead of once per test — see the project memory
/// `gatekeeper-scans-make-pty-fixtures-flaky` and GH-FIXTURE-REUSE /
/// GH-ARGV-LOG-HOIST. The argv-log destination used to be interpolated into
/// the script bytes, which made every call's content distinct; it is now
/// read from `ARGV_LOG_VAR` at spawn time (set by the caller's `Command`),
/// so the script bytes are constant and every call below collapses onto the
/// one file the first caller writes.
///
/// Sharing is keyed by content, never by the caller's requested name, so a
/// name never causes two distinct fixtures to collide, and a repeated name
/// with the same bytes never causes a second write. Race-free the way
/// `provider/cache.rs::write_json_atomically` is: one process-wide mutex
/// serialises the check-and-write, and the write itself lands in a
/// same-directory temporary name before an atomic rename.
fn shared_fixture(unique_name: &str, contents: &str) -> PathBuf {
    use std::collections::HashMap;
    use std::hash::{Hash, Hasher};
    use std::sync::{Mutex, OnceLock};

    static DIR: OnceLock<tempfile::TempDir> = OnceLock::new();
    static CACHE: OnceLock<Mutex<HashMap<String, PathBuf>>> = OnceLock::new();

    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    let mut guard = cache.lock().expect("shared fixture cache poisoned");
    if let Some(path) = guard.get(contents) {
        return path.clone();
    }

    let dir = DIR.get_or_init(|| tempfile::tempdir().expect("shared fixture dir"));
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    contents.hash(&mut hasher);
    let digest = format!("{:016x}", hasher.finish());
    let named = Path::new(unique_name);
    let stem = named
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or(unique_name);
    let filename = match named.extension().and_then(|e| e.to_str()) {
        Some(ext) => format!("{stem}-{digest}.{ext}"),
        None => format!("{stem}-{digest}"),
    };
    let path = dir.path().join(&filename);
    let temporary = dir.path().join(format!("{filename}.writing"));
    std::fs::write(&temporary, contents).expect("write shared fixture");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(&temporary).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&temporary, perms).unwrap();
    }
    std::fs::rename(&temporary, &path).expect("rename shared fixture into place");
    guard.insert(contents.to_string(), path.clone());
    path
}

#[cfg(unix)]
fn install_fake_harness(_bin_dir: &Path) -> PathBuf {
    // Exit 0, deliberately: a failed session is not a warm one, and the
    // sticky tests below need a session to route back into.
    shared_fixture(
        "fake-claude-code",
        &format!("#!/bin/sh\nprintf '%s\\n' \"$*\" >> \"${ARGV_LOG_VAR}\"\nexit 0\n"),
    )
}

#[cfg(windows)]
fn install_fake_harness(_bin_dir: &Path) -> PathBuf {
    shared_fixture(
        "fake-claude-code.cmd",
        &format!("@echo off\r\necho %*>>\"%{ARGV_LOG_VAR}%\"\r\nexit /b 0\r\n"),
    )
}

#[cfg(test)]
mod shared_fixture_proof {
    use super::{ARGV_LOG_VAR, Fixture, install_fake_harness};

    /// **The once-per-binary proof, through the real caller.** Every test in
    /// this file that spawns the harness goes through [`Fixture::new`],
    /// which unconditionally calls `install_fake_harness` — so two
    /// independent per-test tempdirs asking for it, the ordinary shape this
    /// binary runs under, must collapse to one file rather than each
    /// writing its own.
    #[test]
    fn two_tempdirs_installing_the_fake_harness_get_one_shared_file() {
        let tmp_a = tempfile::tempdir().expect("tempdir a");
        let tmp_b = tempfile::tempdir().expect("tempdir b");
        let a = install_fake_harness(tmp_a.path());
        let meta_before = std::fs::metadata(&a).expect("fixture exists after first install");

        let b = install_fake_harness(tmp_b.path());
        assert_eq!(
            a, b,
            "two different tempdirs installing the fixture must share one file"
        );
        assert!(
            !a.starts_with(tmp_a.path()) && !a.starts_with(tmp_b.path()),
            "the shared file must live in the per-binary fixture dir, not either \
             test's own tempdir: {a:?}"
        );

        let meta_after = std::fs::metadata(&b).expect("fixture exists after second install");
        assert_eq!(
            meta_before.modified().unwrap(),
            meta_after.modified().unwrap(),
            "a second install of the same fixture must not rewrite the file"
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            assert_eq!(
                meta_before.ino(),
                meta_after.ino(),
                "a second install of the same fixture must return the same inode, \
                 not a second copy"
            );
        }
    }

    /// **Bytes constant.** The shared fixture's bytes read the argv-log
    /// destination from `ARGV_LOG_VAR` rather than embedding a per-test
    /// path, so the script text is the same regardless of which tempdir
    /// asked for it.
    #[cfg(unix)]
    #[test]
    fn the_shared_fixture_reads_its_log_path_from_the_env_var_not_the_script() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = install_fake_harness(tmp.path());
        let content = std::fs::read_to_string(&path).expect("read shared fixture");
        assert_eq!(
            content,
            format!("#!/bin/sh\nprintf '%s\\n' \"$*\" >> \"${ARGV_LOG_VAR}\"\nexit 0\n"),
            "the shared fixture's bytes must read the log destination from the env var, \
             not have a path baked in"
        );
    }

    /// **End-to-end, through the real caller.** The env var the fixture
    /// reads is exactly the one [`Fixture::glasshouse_with_env`] sets per
    /// spawn — proven by actually launching and reading the argv log back,
    /// not by inspecting the script text alone.
    #[test]
    fn a_real_launch_through_the_shared_fixture_writes_its_argv_to_the_requested_log() {
        let fixture = Fixture::new(None);
        let (out, said) = fixture.launch(&[]);
        assert!(out.status.success(), "launch must succeed:\n{said}");
        let invocations = fixture.harness_invocations();
        assert_eq!(
            invocations.len(),
            1,
            "the shared, env-driven fixture must still log exactly one invocation \
             into this fixture's own argv log:\n{invocations:?}"
        );
    }
}

/// Wall-clock now, for planting readings the binary reads with its own clock.
fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("the system clock is after 1970")
        .as_secs() as i64
}

/// Plant a gateway quota reading exactly where `GatewayQuotaCache::new`
/// resolves one from this run's `--data-dir`, and prove it landed.
fn plant_quota(fixture: &Fixture, provider: &str, remaining: i64, limit: i64) {
    let cache = glasshouse::provider::telemetry::GatewayQuotaCache::at(
        fixture.data_dir().join("gateway-quota"),
    );
    cache.store(
        provider,
        &glasshouse::provider::telemetry::RateLimitHeaders::read(vec![
            ("ratelimit-limit", limit.to_string().as_str()),
            ("ratelimit-remaining", remaining.to_string().as_str()),
        ]),
        now_unix(),
    );
    assert!(
        cache.load(provider).is_some(),
        "the planted reading for `{provider}` must be on disk and readable"
    );
}

/// The identifier the fake harness was started with, from one logged argv
/// line: `--session-id <uuid>` for a fresh launch, `--resume <uuid>` for a
/// resume.
fn session_arg(argv: &str, flag: &str) -> String {
    let mut tokens = argv.split_whitespace();
    while let Some(token) = tokens.next() {
        if token == flag {
            return tokens
                .next()
                .unwrap_or_else(|| panic!("`{flag}` carried no identifier in `{argv}`"))
                .to_owned();
        }
    }
    panic!("no `{flag}` in `{argv}`")
}

// ---------------------------------------------------------------------------
// Byte-for-byte: no task, no classification, no model, no row.
// ---------------------------------------------------------------------------

/// REQUIRED BEHAVIOR 1. A launch that states no task must not classify: the
/// pinned routing model is never asked, the explanation carries no
/// classification, and no routing-latency row is written. The canned endpoint
/// is what makes "never asked" checkable — a build that classified
/// unconditionally would hit it.
#[test]
fn a_launch_without_a_task_routes_exactly_as_before_and_calls_no_model() {
    let model = FakeModel::answering(FRONTIER_ANSWER);
    let fixture = Fixture::new(Some(&model.base_url()));

    let (out, said) = fixture.launch(&[]);
    assert!(out.status.success(), "{said}");
    assert!(
        model.requests().is_empty(),
        "a launch with no --task must not ask the routing model; it was asked {} time(s)",
        model.requests().len()
    );
    assert!(
        !said.contains("classified by"),
        "no task was stated, so nothing may claim a classification:\n{said}"
    );
    assert_eq!(
        fixture.routing_latency_rows(),
        0,
        "no classification ran, so no routing-latency row may exist"
    );

    // And the report path agrees: the explanation has no classification line
    // and the capability term reads exactly as `TaskRequirements::default()`
    // has always rendered it.
    let report = String::from_utf8_lossy(&fixture.glasshouse(&["route"]).stdout).into_owned();
    assert!(!report.contains("task classification"), "{report}");
    assert!(!report.contains("workload tier fit"), "{report}");
    assert!(
        report.contains("the task named no hard capability requirement"),
        "{report}"
    );
    assert!(model.requests().is_empty());
}

// ---------------------------------------------------------------------------
// Lines 1447–1451, 1454: the request on the wire.
// ---------------------------------------------------------------------------

/// REQUIRED BEHAVIOR 2. With a task and a pinned routing model, the launch
/// classifies **through the model**, and the body on the wire is the rendered
/// router request: it names the task, the band word for the candidate
/// provider, and the warm-session fact — and does **not** name the quota
/// numbers the band was computed from (line 1449).
#[test]
fn a_stated_task_classifies_through_the_routing_model_and_the_request_carries_bands_not_numbers() {
    let model = FakeModel::answering(FRONTIER_ANSWER);
    let fixture = Fixture::new(Some(&model.base_url()));
    // 947 of 1213 remaining is 78%, which is `plenty` under the default
    // thresholds (healthy starts at 70%). Neither number is a round one a
    // request body could contain for another reason.
    plant_quota(&fixture, DIRECT_PROVIDER, 947, 1213);

    let task = "refactor the session store's locking so resume never blocks";
    let (out, said) = fixture.launch(&["--profile", "metered", "--task", task]);
    assert!(out.status.success(), "{said}");

    let requests = model.requests();
    assert_eq!(
        requests.len(),
        1,
        "one launch with one task is one classification call"
    );
    let body = &requests[0].body;
    assert!(
        body.contains(task),
        "the task must reach the model:\n{body}"
    );
    assert!(
        body.contains(&format!("{DIRECT_PROVIDER:<18}plenty")),
        "line 1449: the candidate provider's capacity must reach the model as a band:\n{body}"
    );
    assert!(
        !body.contains("947") && !body.contains("1213"),
        "line 1449: the raw quota reading must never reach the model:\n{body}"
    );
    assert!(
        body.contains("warm session      none"),
        "line 1448: a first launch has no warm session, and the request must say so:\n{body}"
    );
    assert!(
        body.contains("harness           claude-code, named by the user"),
        "line 1450: the harness the person named is a stated constraint:\n{body}"
    );
    assert!(
        body.contains("code modification: yes"),
        "line 1451: the tool's own expectation that code will change is in the request:\n{body}"
    );
    assert!(
        body.contains("long-running multi-turn: yes"),
        "line 1454: and its expectation of a multi-turn task:\n{body}"
    );

    // The decision acted on the model's answer, not the heuristic's: only a
    // model can say `frontier`.
    assert!(
        said.contains(&format!(
            "classified by the routing model ({ROUTING_MODEL_LABEL})"
        )),
        "{said}"
    );
    assert!(said.contains("tier frontier"), "{said}");

    // A second launch sees the session the first one recorded, and the
    // request says so — the warm-session fact is a fact about the candidates.
    let (out, said) = fixture.launch(&["--profile", "metered", "--task", task]);
    assert!(out.status.success(), "{said}");
    let requests = model.requests();
    assert_eq!(requests.len(), 2, "{said}");
    assert!(
        requests[1]
            .body
            .contains("warm session      yes — a resumable session"),
        "line 1448: the second launch must tell the model a warm session exists:\n{}",
        requests[1].body
    );
}

/// SECURITY / ISOLATION INVARIANT. Four things are planted where a careless
/// router would read them — a file in the repository, a transcript line
/// through the hook a harness reports with, a memory body in the project's
/// memory store, and a credential in the launch process's environment — and
/// the request on the wire carries none of them. The task names the planted
/// file by name, which is exactly the prompt a helpful classifier might be
/// tempted to expand.
#[test]
fn the_router_request_never_carries_repository_contents_transcripts_or_secrets() {
    const REPO_SENTINEL: &str = "REPOSITORY-CONTENT-SENTINEL-2741";
    const TRANSCRIPT_SENTINEL: &str = "TRANSCRIPT-LINE-SENTINEL-7734";
    const MEMORY_SENTINEL: &str = "MEMORY-BODY-SENTINEL-9182";
    const ENV_VAR: &str = "GLASSHOUSE_PLANTED_ENVIRONMENT_VALUE";
    const ENV_SENTINEL: &str = "planted-environment-value-4471";

    let model = FakeModel::answering(LOW_RISK_ANSWER);
    let fixture = Fixture::new(Some(&model.base_url()));

    // 1. Repository contents.
    std::fs::write(
        fixture.root.join("planted.rs"),
        format!("// {REPO_SENTINEL}\nfn planted() {{}}\n"),
    )
    .unwrap();

    // 2. A transcript. Two plants, because a transcript takes two shapes
    //    here: the harness's own transcript file (the `transcript_path` its
    //    hook payload names — the only transcript that persists, since
    //    Glasshouse's event log stores no turn text), and the prompt line the
    //    hook payload itself carries. The hook needs a session to report
    //    against, so start one first — with no task, which asks nothing.
    let transcript_dir = fixture.root.join(".harness");
    std::fs::create_dir_all(&transcript_dir).unwrap();
    let transcript_path = transcript_dir.join("rollout.jsonl");
    std::fs::write(
        &transcript_path,
        format!(
            "{{\"type\":\"user\",\"text\":\"{TRANSCRIPT_SENTINEL} please look at this\"}}\n\
             {{\"type\":\"assistant\",\"text\":\"an answer that repeats {TRANSCRIPT_SENTINEL}\"}}\n"
        ),
    )
    .unwrap();
    let (out, said) = fixture.launch(&[]);
    assert!(out.status.success(), "{said}");
    assert!(model.requests().is_empty());
    let session_id = {
        let runtime = fixture.runtime();
        let sessions = glasshouse::session::ProjectSessions::open(&runtime).unwrap();
        let store = sessions.store();
        store
            .list()
            .unwrap()
            .into_iter()
            .next()
            .expect("the launch recorded a session")
            .id
    };
    let transcript_path_json = transcript_path.display().to_string().replace('\\', "\\\\");
    let payload = format!(
        r#"{{"session_id":"native-1","transcript_path":"{transcript_path_json}","hook_event_name":"Stop","cwd":"/somewhere","model":"a-model","prompt":"{TRANSCRIPT_SENTINEL} please look at this","last_assistant_message":"an answer that repeats {TRANSCRIPT_SENTINEL}"}}"#
    );
    let mut child = Command::new(env!("CARGO_BIN_EXE_glasshouse"))
        .arg("--scope")
        .arg(&fixture.root)
        .arg("--data-dir")
        .arg(fixture.data_dir())
        .arg("--config-dir")
        .arg(fixture.base.join("config"))
        .arg("hook")
        .arg("--session")
        .arg(session_id.as_str())
        .arg("--event")
        .arg("Stop")
        .env(CREDENTIAL_VAR, CREDENTIAL)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("the glasshouse binary must be runnable");
    child
        .stdin
        .as_mut()
        .unwrap()
        .write_all(payload.as_bytes())
        .unwrap();
    let hook = child.wait_with_output().unwrap();
    assert!(hook.status.success(), "{}", both_streams(&hook));

    // 3. A memory body.
    {
        use glasshouse::memory::{MemoryKind, NewMemory, ProjectMemory};
        let runtime = fixture.runtime();
        let memory = ProjectMemory::open(&runtime).unwrap();
        memory
            .store()
            .record(NewMemory::new(
                MemoryKind::Finding,
                format!("the planted memory {MEMORY_SENTINEL} must stay in the store"),
            ))
            .expect("the planted memory is admissible");
    }

    // 4. A credential, in the environment the launch runs with.
    let task = "fix the bug in planted.rs";
    let out = fixture.glasshouse_with_env(
        &["launch", "claude-code", "--headless", "--task", task],
        &[(ENV_VAR, ENV_SENTINEL)],
    );
    let said = both_streams(&out);
    assert!(out.status.success(), "{said}");

    let requests = model.requests();
    assert_eq!(requests.len(), 1, "{said}");
    let body = &requests[0].body;
    assert!(
        body.contains(task),
        "the task itself must reach the model:\n{body}"
    );
    for (what, sentinel) in [
        ("repository contents (line 1455)", REPO_SENTINEL),
        ("a transcript line (line 1456)", TRANSCRIPT_SENTINEL),
        ("a memory body (line 1426)", MEMORY_SENTINEL),
        ("an environment value (line 1426)", ENV_SENTINEL),
        ("the provider credential (line 1426)", CREDENTIAL),
    ] {
        assert!(
            !body.contains(sentinel),
            "{what} reached the routing model:\n{body}"
        );
    }
}

// ---------------------------------------------------------------------------
// Line 1459: low confidence is conservative, and says so.
// ---------------------------------------------------------------------------

/// REQUIRED BEHAVIOR 4. A model that states `standard` with low confidence is
/// acted on as `heavy`, and the explanation on the person's terminal says
/// both the tier used and why it is not the tier stated.
#[test]
fn low_confidence_routes_on_the_conservative_tier_and_says_so() {
    let model = FakeModel::answering(LOW_CONFIDENCE_ANSWER);
    let fixture = Fixture::new(Some(&model.base_url()));

    let (out, said) = fixture.launch(&["--task", "add a retry to the session store"]);
    assert!(out.status.success(), "{said}");
    assert_eq!(model.requests().len(), 1);
    assert!(
        said.contains("tier heavy (conservative: confidence was low, escalated from standard)"),
        "the explanation must name the conservative tier and the reason:\n{said}"
    );
    assert!(
        said.contains("long-running"),
        "a low-confidence answer is planned for as long-running work:\n{said}"
    );
    assert!(
        !said.contains("shape disposable job"),
        "a low-confidence answer is never sent to a throwaway model:\n{said}"
    );
}

/// A configured model that fails is not a failed launch: heuristics answer,
/// the explanation names the failure in this repository's own words, and the
/// harness still starts.
#[test]
fn a_failing_routing_model_falls_back_to_heuristics_and_the_launch_still_starts() {
    let model = FakeModel::failing();
    let fixture = Fixture::new(Some(&model.base_url()));

    let (out, said) = fixture.launch(&["--task", "run cargo test and fix whatever fails"]);
    assert!(out.status.success(), "{said}");
    assert_eq!(model.requests().len(), 1, "the model was asked, and failed");
    assert!(
        said.contains("deterministic heuristics answered instead"),
        "{said}"
    );
    assert!(
        said.contains("classified by deterministic heuristics ("),
        "{said}"
    );
    assert!(said.contains("shell work"), "{said}");
    assert_eq!(fixture.harness_invocations().len(), 1, "{said}");
}

// ---------------------------------------------------------------------------
// Lines 1470 and 1471: deterministic when it can be.
// ---------------------------------------------------------------------------

/// REQUIRED BEHAVIOR 5 and line 1470. `--fresh` and `--to <id>` decide on
/// their own: heuristics classify for the explanation, and the routing model
/// is never asked.
#[test]
fn an_explicit_destination_is_deterministic_and_asks_no_model() {
    let model = FakeModel::answering(FRONTIER_ANSWER);
    let fixture = Fixture::new(Some(&model.base_url()));
    let task = "run cargo test and fix whatever fails";

    let (out, said) = fixture.launch(&["--fresh", "--task", task]);
    assert!(out.status.success(), "{said}");
    assert!(
        model.requests().is_empty(),
        "`--fresh` is deterministic; the routing model must not be asked:\n{said}"
    );
    assert!(
        said.contains("the destination was stated, so no routing model was asked"),
        "{said}"
    );
    assert!(
        said.contains("tier heavy"),
        "heuristics still classify, for the explanation and the hard constraints:\n{said}"
    );

    let (out, said) = fixture.launch(&["--to", "fresh:claude-code:native", "--task", task]);
    assert!(out.status.success(), "{said}");
    assert!(
        model.requests().is_empty(),
        "`--to` is deterministic; the routing model must not be asked:\n{said}"
    );
    assert_eq!(fixture.harness_invocations().len(), 2, "{said}");
}

/// Line 1471. With no routing model configured, a stated task still routes
/// on the heuristic classification: the explanation carries the hard
/// capabilities the task implies, and the launch proceeds.
#[test]
fn with_no_routing_model_configured_a_stated_task_still_routes_on_heuristics() {
    let fixture = Fixture::new(None);

    let (out, said) = fixture.launch(&[
        "--task",
        "open the browser and take a screenshot of the homepage",
    ]);
    assert!(out.status.success(), "{said}");
    assert!(
        said.contains("classified by deterministic heuristics (no routing model is configured)"),
        "{said}"
    );
    assert!(said.contains("browser work"), "{said}");
    assert!(said.contains("browser interaction"), "{said}");
    assert_eq!(fixture.harness_invocations().len(), 1, "{said}");

    // And the report path carries the same classification into the
    // explanation, where the capability term reads it.
    let report = String::from_utf8_lossy(
        &fixture
            .glasshouse(&[
                "route",
                "--task",
                "open the browser and take a screenshot of the homepage",
            ])
            .stdout,
    )
    .into_owned();
    assert!(report.contains("task classification"), "{report}");
    assert!(report.contains("needs browser interaction"), "{report}");
    assert!(
        report.contains("workload tier fit"),
        "a stated task states a tier, and the fit term is present and says what it could \
         not see:\n{report}"
    );
    assert!(
        report.contains("nothing has established `fresh:claude-code:native`'s ceiling"),
        "no production source states a destination's ceiling, and the term must say so \
         rather than score a guess:\n{report}"
    );
}

/// `glasshouse route --task` asks the same routing model a launch would and
/// prints the same classification — the diagnostic and the decision cannot
/// disagree, because one function produces both.
#[test]
fn route_with_a_task_shows_the_classification_a_launch_would_act_on() {
    let model = FakeModel::answering(FRONTIER_ANSWER);
    let fixture = Fixture::new(Some(&model.base_url()));

    let report = String::from_utf8_lossy(
        &fixture
            .glasshouse(&["route", "--task", "what is a monad"])
            .stdout,
    )
    .into_owned();
    assert_eq!(model.requests().len(), 1, "{report}");
    assert!(
        report.contains(&format!(
            "task classification — classified by the routing model ({ROUTING_MODEL_LABEL})"
        )),
        "{report}"
    );
    assert!(report.contains("tier frontier"), "{report}");
    assert!(report.contains("shape reuse session"), "{report}");
}

// ---------------------------------------------------------------------------
// Line 1849: routing latency.
// ---------------------------------------------------------------------------

/// REQUIRED BEHAVIOR 6. The latency row exists after a launch that
/// classified, and not after one that did not.
#[test]
fn routing_latency_is_recorded_only_when_classification_ran() {
    let model = FakeModel::answering(LOW_RISK_ANSWER);
    let fixture = Fixture::new(Some(&model.base_url()));

    let (out, said) = fixture.launch(&[]);
    assert!(out.status.success(), "{said}");
    assert_eq!(fixture.routing_latency_rows(), 0);

    let (out, said) = fixture.launch(&["--task", "what is a mutex"]);
    assert!(out.status.success(), "{said}");
    assert_eq!(
        fixture.routing_latency_rows(),
        1,
        "a launch that classified must leave exactly one routing-latency row:\n{said}"
    );

    let (out, said) = fixture.launch(&[]);
    assert!(out.status.success(), "{said}");
    assert_eq!(
        fixture.routing_latency_rows(),
        1,
        "a launch that did not classify must not add a row"
    );

    // The row carries both ends of the measurement, so `duration_ms` is a
    // figure and not an absence.
    let runtime = fixture.runtime();
    let ledger = EvidenceLedger::open(&runtime).unwrap();
    let row = ledger
        .recent(
            ObservationQuery {
                provider: "glasshouse",
                model: "session-router",
                route: None,
                harness: Some("claude-code"),
            },
            1,
        )
        .unwrap()
        .into_iter()
        .next()
        .unwrap();
    assert!(
        row.duration_ms().is_some(),
        "both timing columns must be written: {row:?}"
    );
}

// ---------------------------------------------------------------------------
// Lines 1467 and 1468: the sticky bypass.
// ---------------------------------------------------------------------------

/// Line 1467. A low-risk answer for the session the person is in stands for
/// the next turn: the second launch continues the same session, says the
/// answer was reused, and the routing model is not asked again.
#[test]
fn repeated_low_risk_turns_in_the_same_sticky_session_bypass_the_routing_model() {
    let model = FakeModel::answering(LOW_RISK_ANSWER);
    let fixture = Fixture::new(Some(&model.base_url()));

    let (out, said) = fixture.launch(&["--task", "what is a mutex"]);
    assert!(out.status.success(), "{said}");
    assert_eq!(model.requests().len(), 1);
    let first = session_arg(&fixture.harness_invocations()[0], "--session-id");

    let (out, said) = fixture.launch(&["--task", "what is a monad"]);
    assert!(out.status.success(), "{said}");
    assert_eq!(
        model.requests().len(),
        1,
        "a low-risk turn in the same sticky session must not ask the routing model again:\n{said}"
    );
    assert!(
        said.contains("reused without asking the routing model"),
        "{said}"
    );
    assert!(
        said.contains(&format!("({ROUTING_MODEL_LABEL})")),
        "the reused answer still names the model that originally gave it:\n{said}"
    );
    let invocations = fixture.harness_invocations();
    assert_eq!(invocations.len(), 2, "{said}");
    assert_eq!(
        session_arg(&invocations[1], "--resume"),
        first,
        "the second launch must continue the sticky session, not start another:\n{said}"
    );
}

/// Line 1468. When what the answer was conditioned on changes — here, a
/// provider-health reading appears for the session's own credential — the
/// routing model is asked again rather than the old answer reused.
#[test]
fn a_material_change_in_resource_conditions_re_runs_classification() {
    let model = FakeModel::answering(LOW_RISK_ANSWER);
    let fixture = Fixture::new(Some(&model.base_url()));

    let (out, said) = fixture.launch(&["--task", "what is a mutex"]);
    assert!(out.status.success(), "{said}");
    let (out, said) = fixture.launch(&["--task", "what is a monad"]);
    assert!(out.status.success(), "{said}");
    assert_eq!(model.requests().len(), 1, "premise: the second turn reused");

    // A native session's credential is the harness's own sign-in, rendered
    // exactly as `CredentialId::label` renders an `OsCredential`; a health
    // reading persisted against it is a fact the fingerprint did not hold
    // before.
    let cache = glasshouse::provider::telemetry::GatewayHealthCache::at(
        fixture.data_dir().join("gateway-health"),
    );
    cache.store(
        "claude-code",
        &[glasshouse::provider::telemetry::GatewayHealthReading {
            credential_label: "claude-code/claude-code:the harness's own sign-in".to_owned(),
            model: "the harness's own default".to_owned(),
            consecutive_failures: 1,
            cooling_down_until_unix: None,
            cooldown_cause: None,
            credential_rejected: false,
        }],
        now_unix(),
    );
    assert_eq!(cache.load("claude-code").len(), 1);

    let (out, said) = fixture.launch(&["--task", "what is a semaphore"]);
    assert!(out.status.success(), "{said}");
    assert_eq!(
        model.requests().len(),
        2,
        "changed conditions must send the classifier back to the model:\n{said}"
    );
    assert!(
        said.contains(&format!(
            "classified by the routing model ({ROUTING_MODEL_LABEL})"
        )),
        "{said}"
    );
    assert!(!said.contains("reused"), "{said}");
}

/// The other half of line 1467: an answer that was **not** low-risk is
/// never reused, however sticky the session.
#[test]
fn a_classification_that_is_not_low_risk_is_asked_again_every_turn() {
    let model = FakeModel::answering(FRONTIER_ANSWER);
    let fixture = Fixture::new(Some(&model.base_url()));

    let (out, said) = fixture.launch(&["--task", "rewrite the scheduler"]);
    assert!(out.status.success(), "{said}");
    let (out, said) = fixture.launch(&["--task", "rewrite the scheduler again"]);
    assert!(out.status.success(), "{said}");
    assert_eq!(
        model.requests().len(),
        2,
        "frontier work is not low-risk; each turn is classified afresh:\n{said}"
    );
}

// ---------------------------------------------------------------------------
// Lines 1516 and 1531, at the library level — see this file's header.
// ---------------------------------------------------------------------------

fn backend() -> Backend {
    Backend::new(
        "anthropic",
        "anthropic-messages",
        AssignedModel::HarnessDefault,
        CredentialId::new(
            "anthropic",
            SecretRef::Environment {
                var: "ANTHROPIC_API_KEY".to_owned(),
            },
        ),
        Cost::Metered,
        ToolSemantics::Unverified,
    )
}

fn fresh_with_ceiling(id: &str, ceiling: Option<WorkloadTier>) -> Destination {
    Destination::fresh(id, IntegrationId::ClaudeCode, "default", backend(), None)
        .with_tier_ceiling(ceiling)
}

fn existing_with_ceiling(id: &str, ceiling: Option<WorkloadTier>) -> Destination {
    Destination::existing(
        id,
        IntegrationId::ClaudeCode,
        "default",
        backend(),
        WarmSession {
            state: WarmSessionState::Resumable,
            idle_seconds: 5,
        },
    )
    .with_tier_ceiling(ceiling)
}

struct RouterFixture {
    overrides: PairingOverrides,
    health: FreePool,
    now: Instant,
}

impl RouterFixture {
    fn new() -> Self {
        Self {
            overrides: PairingOverrides::from_parts(
                "no configuration",
                BTreeMap::new(),
                BTreeMap::new(),
            ),
            health: FreePool::new(),
            now: Instant::now(),
        }
    }

    fn requiring(&self, minimum_tier: Option<WorkloadTier>) -> RouterInputs<'_> {
        RouterInputs {
            overrides: &self.overrides,
            health: &self.health,
            now: self.now,
            requirements: TaskRequirements {
                minimum_tier,
                ..TaskRequirements::default()
            },
        }
    }
}

/// REQUIRED BEHAVIOR 3 and line 1516. A destination established to serve
/// below the required tier is under `rejected()` with a reason a person can
/// read, and never under `considered()`.
#[test]
fn a_destination_below_the_required_tier_is_excluded_with_a_readable_reason() {
    let fixture = RouterFixture::new();
    let below = existing_with_ceiling("below", Some(WorkloadTier::Standard));
    let fits = fresh_with_ceiling("fits", Some(WorkloadTier::Heavy));

    // `below` is an existing, warm session: on every soft term it would beat
    // the fresh `fits`. Only the gate can keep it from winning.
    let routed = SessionRouter::new()
        .choose(
            RoutingMoment::SessionStart,
            None,
            &[below, fits],
            &fixture.requiring(Some(WorkloadTier::Heavy)),
        )
        .expect("destinations were offered");

    assert_eq!(routed.chosen().id(), "fits");
    assert!(
        routed.considered().iter().all(|(d, _)| d.id() != "below"),
        "a destination below the tier must not be ranked at all"
    );
    assert_eq!(routed.rejected().len(), 1);
    assert_eq!(routed.rejected()[0].0.id(), "below");
    assert_eq!(
        routed.rejected()[0].1,
        HardConstraint::WorkloadTier {
            required: WorkloadTier::Heavy,
            offered: WorkloadTier::Standard,
        }
    );
    let overview = routed.render_overview();
    assert!(
        overview.contains(
            "hard workload tier constraint — the task needs at least the `heavy` tier and \
             this destination is established to offer at most `standard`"
        ),
        "{overview}"
    );
}

/// The honesty rule the gate shares with every other absent reading in this
/// router: a destination whose ceiling nobody has established is not below
/// anything. This is the case every destination the shipped binary builds
/// is in today.
#[test]
fn a_destination_with_no_established_ceiling_is_never_excluded_by_the_tier_gate() {
    let fixture = RouterFixture::new();
    let unknown = existing_with_ceiling("unknown", None);
    let fits = fresh_with_ceiling("fits", Some(WorkloadTier::Frontier));

    let routed = SessionRouter::new()
        .choose(
            RoutingMoment::SessionStart,
            None,
            &[unknown, fits],
            &fixture.requiring(Some(WorkloadTier::Frontier)),
        )
        .expect("destinations were offered");

    assert!(routed.rejected().is_empty(), "{:?}", routed.rejected());
    assert_eq!(routed.considered().len(), 2);
    let overview = routed.render_overview();
    assert!(
        overview.contains("nothing has established `unknown`'s ceiling — not a `no`"),
        "{overview}"
    );
}

/// Line 1531, as a discriminating pair (practice §35): two destinations
/// differing **only** in their established ceiling resolve differently, in
/// both orders — an exact fit outranks headroom, and headroom outranks
/// nothing established.
#[test]
fn workload_tier_fit_decides_between_two_otherwise_identical_destinations() {
    let fixture = RouterFixture::new();
    let inputs = fixture.requiring(Some(WorkloadTier::Heavy));

    let exact = fresh_with_ceiling("exact", Some(WorkloadTier::Heavy));
    let headroom = fresh_with_ceiling("headroom", Some(WorkloadTier::Frontier));
    let unknown = fresh_with_ceiling("unknown", None);

    for order in [
        vec![exact.clone(), headroom.clone()],
        vec![headroom.clone(), exact.clone()],
    ] {
        let routed = SessionRouter::new()
            .choose(RoutingMoment::SessionStart, None, &order, &inputs)
            .unwrap();
        assert_eq!(
            routed.chosen().id(),
            "exact",
            "an exact tier fit must beat headroom whichever was offered first"
        );
    }
    for order in [
        vec![headroom.clone(), unknown.clone()],
        vec![unknown.clone(), headroom.clone()],
    ] {
        let routed = SessionRouter::new()
            .choose(RoutingMoment::SessionStart, None, &order, &inputs)
            .unwrap();
        assert_eq!(
            routed.chosen().id(),
            "headroom",
            "an established ceiling above the tier must beat one nobody established"
        );
    }

    // And with no tier stated, the term is absent and the two tie on the
    // caller's order — the byte-for-byte case.
    let routed = SessionRouter::new()
        .choose(
            RoutingMoment::SessionStart,
            None,
            &[headroom, exact],
            &fixture.requiring(None),
        )
        .unwrap();
    assert_eq!(routed.chosen().id(), "headroom");
    assert!(
        !routed.render_overview().contains("workload tier fit"),
        "{}",
        routed.render_overview()
    );
}

// ---------------------------------------------------------------------------
// Lines 1457 and 1458: the two recommendation fields parse leniently.
// ---------------------------------------------------------------------------

/// ACCEPTANCE TEST 8. A reply that predates the two recommendation keys, or
/// that gives one a value this build does not know, still parses — to a
/// classification whose recommendations are derived from what it did state,
/// conservatively — and never to a failure.
#[test]
fn parse_of_an_answer_missing_new_fields_yields_the_conservative_default() {
    let classification = parse_classification(LOW_CONFIDENCE_ANSWER, "label")
        .expect("a reply without the two optional keys must parse");
    assert_eq!(classification.stated_duration(), None);
    assert_eq!(classification.stated_execution_shape(), None);
    assert_eq!(classification.confidence(), Confidence::Low);
    assert_eq!(
        classification.expected_duration(),
        DurationClass::LongRunning,
        "low confidence plans for the longer case"
    );
    assert_eq!(
        classification.expected_execution_shape(),
        ExecutionShape::ReuseSession,
        "derived from `likely_multi_turn` and `prefer_warm`, never a throwaway model"
    );

    let unknown_words = LOW_RISK_ANSWER
        .replace("\"single_turn\"", "\"forever\"")
        .replace("\"reuse_session\"", "\"teleport\"");
    let classification =
        parse_classification(&unknown_words, "label").expect("unknown words are not failures");
    assert_eq!(classification.stated_duration(), None);
    assert_eq!(classification.stated_execution_shape(), None);
    assert_eq!(
        classification.expected_duration(),
        DurationClass::SingleTurn
    );

    let stated = parse_classification(LOW_RISK_ANSWER, "label").unwrap();
    assert_eq!(stated.stated_duration(), Some(DurationClass::SingleTurn));
    assert_eq!(
        stated.stated_execution_shape(),
        Some(ExecutionShape::ReuseSession)
    );
    assert_eq!(
        stated.expected_execution_shape(),
        ExecutionShape::ReuseSession
    );

    // A stated `disposable_job` is withdrawn when the classification is not
    // conservatively safe for one.
    let disposable_but_low = LOW_CONFIDENCE_ANSWER.replace(
        "\"confidence\": \"low\"",
        "\"confidence\": \"low\",\n  \"execution_shape\": \"disposable_job\"",
    );
    let classification = parse_classification(&disposable_but_low, "label").unwrap();
    assert_eq!(
        classification.stated_execution_shape(),
        Some(ExecutionShape::DisposableJob)
    );
    assert_ne!(
        classification.expected_execution_shape(),
        ExecutionShape::DisposableJob,
        "line 1459: a low-confidence answer never routes to a throwaway model"
    );
}
