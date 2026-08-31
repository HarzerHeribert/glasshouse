//! Eight V1-completion criteria (map lines 1930-1937) over Glasshouse's
//! routing, quota and guardrail phases — each already closed by production
//! code the map records elsewhere. One test per criterion, entering through
//! the shipped binary or the nearest deterministic production seam, per
//! `.agent-runtime/packet-prove-it-v1-routing.md`.

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::time::{Duration, Instant};

use clap::Parser;

use glasshouse::config::pairing::{WarmSession, WarmSessionState};
use glasshouse::gateway::{Route, Upstream, UpstreamBackend};
use glasshouse::harness::pairing::PairingOverrides;
use glasshouse::integrations::IntegrationId;
use glasshouse::profile::{BackendResource, LaunchProfile};
use glasshouse::provider::quota::{self, CapacityBand, ReserveDecisionInputs};
use glasshouse::provider::registry::ResourceKind;
use glasshouse::provider::telemetry::GatewayHealthCache;
use glasshouse::routing::classify::WorkloadTier;
use glasshouse::routing::evidence::{EvidenceLedger, ObservationQuery, Outcome};
use glasshouse::routing::session::{
    Destination, RouterInputs, RoutingMoment, SessionRouter, TaskRequirements,
};
use glasshouse::routing::{AssignedModel, Backend, Cost, CredentialId, ToolSemantics};
use glasshouse::secret::{EnvironmentSecretStore, Secret, SecretRef, SecretStore};
use glasshouse::{Cli, Runtime};

// ---------------------------------------------------------------------------
// Line 1930 — the router chooses between a relevant warm session and fresh.
// ---------------------------------------------------------------------------

const ANTHROPIC: &str = "anthropic-messages";

fn backend(provider: &str, model: &str, var: &str) -> Backend {
    Backend::new(
        provider,
        ANTHROPIC,
        AssignedModel::named(model),
        CredentialId::new(
            provider,
            SecretRef::Environment {
                var: var.to_owned(),
            },
        ),
        Cost::Metered,
        ToolSemantics::Verified,
    )
}

fn live(idle_seconds: i64) -> WarmSession {
    WarmSession {
        state: WarmSessionState::Live,
        idle_seconds,
    }
}

fn no_overrides() -> PairingOverrides {
    PairingOverrides::from_parts(
        "no configuration",
        std::collections::BTreeMap::new(),
        std::collections::BTreeMap::new(),
    )
}

/// Line 1930. With one relevant warm session and nothing else, the router
/// continues it; with no destinations at all but a fresh one, it starts
/// fresh — and both decisions name the rule that decided them in the
/// explanation, over `SessionRouter::choose`, the function `main.rs`'s
/// `launch_session` calls on every real launch (`phase-37.md`).
#[test]
fn line_1930_the_router_chooses_a_relevant_warm_session_or_starts_fresh_and_names_the_rule() {
    let overrides = no_overrides();
    let health = glasshouse::routing::free::FreePool::new();
    let now = Instant::now();
    let inputs = RouterInputs {
        overrides: &overrides,
        health: &health,
        now,
        requirements: TaskRequirements::default(),
    };

    // One relevant warm session, offered alongside a fresh alternative: the
    // router continues the warm one — a real comparison, not a single-choice
    // default, so a router that stopped preferring warmth would fail here.
    let warm = Destination::existing(
        "warm",
        IntegrationId::ClaudeCode,
        "default",
        backend("anthropic", "claude-opus-4", "V1_ROUTER_KEY"),
        live(0),
    );
    let fresh_alternative = Destination::fresh(
        "fresh-alternative",
        IntegrationId::ClaudeCode,
        "default",
        backend("anthropic", "claude-opus-4", "V1_ROUTER_KEY"),
        None,
    );
    let routed = SessionRouter::new()
        .choose(
            RoutingMoment::SessionStart,
            None,
            &[warm, fresh_alternative],
            &inputs,
        )
        .expect("destinations were offered");
    assert_eq!(
        routed.chosen().id(),
        "warm",
        "one relevant warm session, offered against a fresh alternative, must be the one \
         chosen: {}",
        routed.render()
    );
    assert!(
        routed
            .explanation()
            .contributions()
            .iter()
            .any(|c| c.name() == "session affinity"),
        "the decision to continue a warm session must name session affinity as its rule: {}",
        routed.render()
    );

    // With none — no warm session anywhere, only a fresh destination — the
    // router starts fresh, and names the same rule as inert rather than
    // silent.
    let fresh = Destination::fresh(
        "fresh",
        IntegrationId::ClaudeCode,
        "default",
        backend("anthropic", "claude-opus-4", "V1_ROUTER_KEY"),
        None,
    );
    let routed_fresh = SessionRouter::new()
        .choose(RoutingMoment::SessionStart, None, &[fresh], &inputs)
        .expect("one destination was offered");
    assert_eq!(routed_fresh.chosen().id(), "fresh");
    assert!(
        routed_fresh
            .explanation()
            .contributions()
            .iter()
            .any(|c| c.name() == "session affinity"),
        "starting fresh must still name the affinity rule, inert rather than absent: {}",
        routed_fresh.render()
    );
}

// ---------------------------------------------------------------------------
// Line 1931 — a fixture provider's quota headers render in native units.
// ---------------------------------------------------------------------------

const TELEMETRY_OBSERVED: i64 = 1_787_800_000;

/// A project directory and a private configuration directory the shipped
/// binary can be pointed at — `tests/provider_discovery.rs`'s own
/// `BinaryFixture` shape.
struct BinaryFixture {
    project: tempfile::TempDir,
    config: tempfile::TempDir,
}

impl BinaryFixture {
    fn new() -> Self {
        let project = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir_all(project.path().join(".git")).expect("create project root");
        let config = tempfile::tempdir().expect("tempdir");
        Self { project, config }
    }

    fn run(&self, args: &[&str]) -> String {
        let output = Command::new(env!("CARGO_BIN_EXE_glasshouse"))
            .current_dir(self.project.path())
            .args([
                "--data-dir",
                self.config.path().to_str().unwrap(),
                "--config-dir",
                self.config.path().to_str().unwrap(),
            ])
            .args(args)
            .output()
            .expect("the glasshouse binary runs");
        assert!(
            output.status.success(),
            "`glasshouse {}` failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).expect("stdout is UTF-8")
    }
}

/// Line 1931. A fixture provider's quota headers, planted exactly where the
/// gateway itself would have written them (`GatewayQuotaCache`, phase-32b's
/// own production door), reach `glasshouse resources`'s report in the
/// provider's own units — requests, with a reset — never a bare percentage.
#[test]
fn line_1931_a_fixture_providers_quota_headers_render_in_native_units_not_a_bare_percentage() {
    let fixture = BinaryFixture::new();
    let quota_cache_dir = fixture.config.path().join("gateway-quota");
    let cache = glasshouse::provider::telemetry::GatewayQuotaCache::at(&quota_cache_dir);
    cache.store(
        "anyrouter",
        &glasshouse::provider::telemetry::RateLimitHeaders::read(vec![
            ("ratelimit-limit", "300"),
            ("ratelimit-remaining", "297"),
        ]),
        TELEMETRY_OBSERVED,
    );
    assert!(
        cache.load("anyrouter").is_some(),
        "the planted reading must be on disk for this test to mean anything"
    );

    let stdout = fixture.run(&["resources", "--no-harness"]);
    let row = stdout
        .split("\n\n")
        .find(|block| block.starts_with("anyrouter"))
        .unwrap_or_else(|| panic!("no anyrouter block in:\n{stdout}"));
    assert!(
        row.contains("297 requests") && row.contains("300 requests"),
        "the report must name the native unit right beside the number the header measured, \
         not a bare figure with the unit dropped: {row}"
    );
}

// ---------------------------------------------------------------------------
// Line 1932 — opaque subscription capacity is unknown, never fabricated.
// ---------------------------------------------------------------------------

/// Line 1932. A native subscription's capacity — the resource kind every
/// harness's own account is — is represented as opaque/unknown at every
/// pool the provider does not publish, and the model can never be read as a
/// fabricated exact figure: `Capacity::is_readable()` answers `false` for
/// it, guarding Phase 32B against ever filling one in
/// (`docs/product/evidence/phase-32a.md`).
#[test]
fn line_1932_an_opaque_subscriptions_capacity_is_unknown_and_never_fabricated() {
    let subscription = ResourceKind::NativeSubscription {
        harness: IntegrationId::ClaudeCode,
    }
    .capacity();

    let remaining = subscription.tokens().combined().remaining();
    assert!(
        !remaining.is_readable(),
        "an opaque subscription's remaining tokens must not be a value Phase 32B could ever \
         fill in: {remaining:?}"
    );
    assert!(
        !remaining.is_measured(),
        "and it must not already carry a measured value: {remaining:?}"
    );
    assert_eq!(remaining.as_str(), "opaque to the provider");
    assert!(
        remaining.reading().is_none(),
        "no numeric reading may be read off an opaque pool: {remaining:?}"
    );
    assert!(
        subscription.normalized().is_none(),
        "with no pool measured, the model must compute no normalized percentage at all: {:?}",
        subscription.normalized()
    );
}

// ---------------------------------------------------------------------------
// Line 1933 — a configured routing model assigns a tier; a failing one
// falls back to the deterministic heuristic and says so.
// ---------------------------------------------------------------------------

/// A canned OpenAI chat-completions endpoint that always answers the same
/// content, or always fails — `tests/classification_call.rs`'s own shape,
/// reduced to the one thing this criterion needs.
struct FakeModel {
    address: SocketAddr,
    fail: bool,
    stop: Arc<std::sync::atomic::AtomicBool>,
}

impl FakeModel {
    fn start(fail: bool) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("loopback must bind");
        listener
            .set_nonblocking(true)
            .expect("the accept loop polls its stop flag");
        let address = listener.local_addr().unwrap();
        let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let thread_stop = Arc::clone(&stop);
        std::thread::spawn(move || {
            while !thread_stop.load(std::sync::atomic::Ordering::SeqCst) {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        let _ = stream.set_nonblocking(false);
                        let mut buf = [0u8; 4096];
                        let _ = stream.read(&mut buf);
                        let response = if fail {
                            "HTTP/1.1 500 Internal Server Error\r\ncontent-length: 0\r\n\
                             connection: close\r\n\r\n"
                                .to_owned()
                        } else {
                            let content = r#"{"needs_repo_context": false, "needs_code_modification": false, "needs_shell_execution": false, "needs_browser_interaction": false, "complexity": "complex", "likely_multi_turn": true, "workload_tier": "frontier", "safe_for_disposable_model": false, "warm_context": "prefer_warm", "confidence": "high"}"#;
                            let document = serde_json::json!({
                                "choices": [{ "message": { "role": "assistant", "content": content } }],
                                "usage": { "prompt_tokens": 10, "completion_tokens": 5 }
                            })
                            .to_string();
                            format!(
                                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\n\
                                 content-length: {}\r\nconnection: close\r\n\r\n{document}",
                                document.len()
                            )
                        };
                        let _ = stream.write_all(response.as_bytes());
                        let _ = stream.flush();
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
            fail,
            stop,
        }
    }

    fn base_url(&self) -> String {
        format!("http://{}/v1", self.address)
    }
}

impl Drop for FakeModel {
    fn drop(&mut self) {
        self.stop.store(true, std::sync::atomic::Ordering::SeqCst);
        let _ = self.fail;
    }
}

struct ClassifyFixture {
    _tmp: tempfile::TempDir,
    base: PathBuf,
    root: PathBuf,
}

impl ClassifyFixture {
    const CREDENTIAL_VAR: &'static str = "GLASSHOUSE_V1_CRITERIA_ROUTING_MODEL_KEY";

    fn new(base_url: &str) -> Self {
        let tmp = tempfile::tempdir().expect("tempdir");
        let base = tmp.path().to_path_buf();
        let root = base.join("workspace");
        std::fs::create_dir_all(root.join(".git")).expect("create project root");
        let root = std::fs::canonicalize(&root).expect("canonicalize project root");

        let config_dir = base.join("config");
        std::fs::create_dir_all(&config_dir).expect("create config dir");
        std::fs::write(
            config_dir.join("config.toml"),
            format!(
                "version = 1\n\n\
                 [providers.v1-router]\ntemplate = \"openai-compatible\"\n\
                 base_url = \"{base_url}\"\ncredential_env = [\"{}\"]\n\
                 free_models = [\"v1-router-model\"]\n\n\
                 [routing]\nmodel = {{ kind = \"pinned\", provider = \"v1-router\", model = \"v1-router-model\" }}\n",
                Self::CREDENTIAL_VAR
            ),
        )
        .expect("write user config");

        Self {
            _tmp: tmp,
            base,
            root,
        }
    }

    fn classify(&self, text: &str) -> (bool, String, String) {
        let output = Command::new(env!("CARGO_BIN_EXE_glasshouse"))
            .env(Self::CREDENTIAL_VAR, "sk-fabricated-test-value-not-real")
            .arg("--scope")
            .arg(&self.root)
            .arg("--data-dir")
            .arg(self.base.join("data"))
            .arg("--config-dir")
            .arg(self.base.join("config"))
            .arg("classify")
            .arg(text)
            .output()
            .expect("the glasshouse binary must be runnable");
        (
            output.status.success(),
            String::from_utf8_lossy(&output.stdout).into_owned(),
            String::from_utf8_lossy(&output.stderr).into_owned(),
        )
    }
}

/// Line 1933. With a configured cheap routing model reachable through a
/// fixture, a task is assigned the tier the model answers with; with the
/// same configuration unreachable, `glasshouse classify` falls back to the
/// deterministic heuristic and says out loud that it degraded
/// (`docs/product/evidence/phase-34a.md`, `phase-34c.md`).
#[test]
fn line_1933_a_configured_routing_model_assigns_a_tier_and_a_failing_one_falls_back_and_says_so() {
    let model = FakeModel::start(false);
    let fixture = ClassifyFixture::new(&model.base_url());
    let (ok, stdout, stderr) = fixture.classify("what should this project do next");
    assert!(ok, "stderr: {stderr}");
    assert!(
        stdout.contains("workload tier           frontier"),
        "`frontier` has no heuristic producer, so only the model's own answer explains it:\n\
         {stdout}"
    );
    assert!(
        stdout.contains("source                  model"),
        "the report must attribute the classification to the model that answered:\n{stdout}"
    );

    drop(model);
    let failing = FakeModel::start(true);
    let fixture = ClassifyFixture::new(&failing.base_url());
    let (ok, stdout, stderr) = fixture.classify("what should this project do next");
    assert!(
        ok,
        "a failed routing model must not fail the command: {stderr}"
    );
    assert!(
        stdout.contains("source                  deterministic heuristics"),
        "an unreachable model must fall back to the heuristic, never invent an answer:\n{stdout}"
    );
    assert!(
        stderr.contains("deterministic heuristics answered instead"),
        "the fallback must be said out loud, not implied: {stderr}"
    );
}

// ---------------------------------------------------------------------------
// Line 1934 — a reserve protecting a premium resource routes low-tier work
// away from it, and the explanation names the reserve.
// ---------------------------------------------------------------------------

/// Line 1934. A resource in the protected reserve band, with a cheaper
/// adequate alternative available, denies a low-tier task the spend and
/// names the reserve in the reason — `evaluate_reserve_spend`
/// (`provider/quota.rs`), reached in production from
/// `routing/disposable.rs::choose` (`docs/product/evidence/phase-32f.md`).
#[test]
fn line_1934_a_reserve_protecting_a_premium_resource_routes_low_tier_work_away_and_names_it() {
    let denied = quota::evaluate_reserve_spend(ReserveDecisionInputs {
        band: CapacityBand::Reserve,
        tier: WorkloadTier::Leaf,
        cheaper_adequate_resource_exists: true,
        user_override: false,
        seconds_until_reset: None,
        task_nearly_complete: false,
    });
    assert!(
        !denied.is_allowed(),
        "low-tier work with a cheaper adequate alternative must not spend the reserve: {}",
        denied.reason()
    );
    assert!(
        denied.reason().contains("reserve") && denied.reason().contains("1288"),
        "the explanation must name the reserve as the reason: {}",
        denied.reason()
    );

    // The dual: the same resource, above the reserve band, is never denied
    // — the reserve is what is being protected, not tightness in general.
    let allowed = quota::evaluate_reserve_spend(ReserveDecisionInputs {
        band: CapacityBand::Healthy,
        tier: WorkloadTier::Leaf,
        cheaper_adequate_resource_exists: true,
        user_override: false,
        seconds_until_reset: None,
        task_nearly_complete: false,
    });
    assert!(
        allowed.is_allowed(),
        "a healthy resource must not be denied by a policy meant to protect the reserve band \
         alone: {}",
        allowed.reason()
    );
}

// ---------------------------------------------------------------------------
// Line 1935 — a substantial task records assumptions with evidence state
// and creates a checkpoint before implementation.
// ---------------------------------------------------------------------------

use std::io::BufRead;
use std::process::{Child, ChildStdin, Stdio};
use std::sync::mpsc::{self, Receiver};

use serde_json::{Value, json};

const MCP_TIMEOUT: Duration = Duration::from_secs(30);

struct GuardrailFixture {
    _tmp: tempfile::TempDir,
    base: PathBuf,
}

impl GuardrailFixture {
    fn new() -> Self {
        let tmp = tempfile::tempdir().expect("tempdir");
        let base = tmp.path().to_path_buf();
        let config_dir = base.join("config");
        std::fs::create_dir_all(&config_dir).expect("create config dir");
        std::fs::write(config_dir.join("config.toml"), "version = 1\n").expect("write config");
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
            "--scope",
            root.to_str().unwrap(),
            "--data-dir",
            self.base.join("data").to_str().unwrap(),
            "--config-dir",
            self.base.join("config").to_str().unwrap(),
        ])
        .expect("parse the fixture command line");
        glasshouse::bootstrap(&cli, root).expect("bootstrap the fixture runtime")
    }

    fn seed_session(&self, root: &Path) -> String {
        let runtime = self.runtime(root);
        let sessions = glasshouse::session::ProjectSessions::open(&runtime).expect("open store");
        let record = sessions
            .store()
            .create(glasshouse::session::NewSession::embedded("claude-code"))
            .expect("seed a session record");
        record.id.as_str().to_owned()
    }

    fn command(&self, root: &Path) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_glasshouse"));
        command
            .arg("--scope")
            .arg(root)
            .arg("--data-dir")
            .arg(self.base.join("data"))
            .arg("--config-dir")
            .arg(self.base.join("config"));
        command
    }
}

struct McpServer {
    child: Child,
    stdin: Option<ChildStdin>,
    lines: Receiver<String>,
    next_id: u64,
}

impl McpServer {
    fn start(fixture: &GuardrailFixture, root: &Path) -> Self {
        let mut child = fixture
            .command(root)
            .arg("mcp")
            .arg("serve")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .expect("spawn `glasshouse mcp serve`");
        let stdout = child.stdout.take().expect("captured stdout");
        let stdin = child.stdin.take().expect("captured stdin");
        let (sender, lines) = mpsc::channel();
        std::thread::spawn(move || {
            for line in std::io::BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                if sender.send(line).is_err() {
                    break;
                }
            }
        });
        let mut server = Self {
            child,
            stdin: Some(stdin),
            lines,
            next_id: 0,
        };
        server.initialize();
        server
    }

    fn send_raw(&mut self, frame: &str) {
        let stdin = self.stdin.as_mut().expect("stdin is still open");
        stdin
            .write_all(format!("{frame}\n").as_bytes())
            .expect("write a frame");
        stdin.flush().expect("flush stdin");
    }

    fn next_reply(&self) -> Value {
        let line = self
            .lines
            .recv_timeout(MCP_TIMEOUT)
            .expect("the server must answer within the timeout");
        serde_json::from_str(&line).unwrap_or_else(|err| panic!("not JSON: {err}: {line}"))
    }

    fn request(&mut self, method: &str, params: Value) -> Value {
        self.next_id += 1;
        let id = self.next_id;
        let frame = json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params });
        self.send_raw(&frame.to_string());
        let reply = self.next_reply();
        assert_eq!(
            reply["id"],
            json!(id),
            "a reply to a request never made: {reply}"
        );
        reply
    }

    fn initialize(&mut self) {
        let reply = self.request(
            "initialize",
            json!({
                "protocolVersion": "2025-06-18",
                "capabilities": {},
                "clientInfo": { "name": "v1_criteria_routing test", "version": "0" },
            }),
        );
        assert!(reply["error"].is_null(), "initialize was refused: {reply}");
        let frame =
            json!({ "jsonrpc": "2.0", "method": "notifications/initialized", "params": {} });
        self.send_raw(&frame.to_string());
    }

    fn ok(&mut self, name: &str, arguments: Value) -> Value {
        let reply = self.request(
            "tools/call",
            json!({ "name": name, "arguments": arguments }),
        );
        let result = &reply["result"];
        assert!(
            result.is_object(),
            "`{name}` answered a protocol error: {reply}"
        );
        assert_eq!(result["isError"], false, "`{name}` refused: {reply}");
        let text = result["content"][0]["text"].as_str().expect("text content");
        serde_json::from_str(text)
            .unwrap_or_else(|err| panic!("`{name}` answered non-JSON: {err}: {text}"))
    }
}

impl Drop for McpServer {
    fn drop(&mut self) {
        self.stdin.take();
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Line 1935. A substantial change (here: a migration) triggers a preflight
/// that takes a checkpoint before anything is implemented, and the assumption
/// it records afterward carries an explicit evidence state — six fields, never
/// the agent's reasoning (`docs/product/evidence/phase-21k.md`).
#[test]
fn line_1935_a_substantial_task_records_assumptions_with_evidence_state_and_checkpoints_first() {
    let fixture = GuardrailFixture::new();
    let root = fixture.project_root("alpha");
    let mut server = McpServer::start(&fixture, &root);

    let session = fixture.seed_session(&root);
    let preflight = server.ok(
        "glasshouse_preflight",
        json!({
            "session": session,
            "change": {
                "description": "add a migration",
                "footprint": 3,
                "subsystems": ["database"],
                "reversible": true,
                "blast_radius": "module",
                "migration": true,
            }
        }),
    );
    assert_eq!(preflight["risk"], "substantial", "{preflight}");

    // A checkpoint exists before any implementation ran — nothing in this
    // test has touched a file.
    let checkpoint_id = preflight["checkpoint"]["checkpoint"]
        .as_str()
        .expect("line 1036: a substantial preflight must take a checkpoint");
    let fetched = server.ok(
        "glasshouse_get_checkpoint",
        json!({ "checkpoint": checkpoint_id }),
    );
    assert_eq!(fetched["session"], session, "{fetched}");

    // The assumption itself: six fields, an explicit evidence-source state,
    // and never a reasoning field.
    let recorded = server.ok(
        "glasshouse_record_assumption",
        json!({
            "session": session,
            "claim": "the migration is additive and needs no backfill",
            "evidence": "grep found no NOT NULL column with no default",
            "evidence_source": "repository",
            "uncertainty": "medium",
            "affected": "database.rs and every reader of the new table",
            "verification": "run the migration's own round-trip test",
        }),
    );
    assert_eq!(recorded["state"], "proposed", "{recorded}");
    assert_eq!(recorded["evidence_source"], "repository", "{recorded}");
    assert!(recorded.get("reasoning").is_none(), "{recorded}");
}

// ---------------------------------------------------------------------------
// Line 1936 — the route report shows workload tier, session affinity,
// resource capacity and the primary reason.
// ---------------------------------------------------------------------------

struct RouteFixture {
    _tmp: tempfile::TempDir,
    base: PathBuf,
    root: PathBuf,
}

impl RouteFixture {
    const CREDENTIAL_VAR: &'static str = "GLASSHOUSE_V1_CRITERIA_ROUTE_KEY";

    fn new() -> Self {
        let tmp = tempfile::tempdir().expect("tempdir");
        let base = tmp.path().to_path_buf();
        let root = base.join("workspace");
        std::fs::create_dir_all(root.join(".git")).expect("create project root");
        let root = std::fs::canonicalize(&root).expect("canonicalize project root");

        let bin_dir = base.join("bin");
        std::fs::create_dir_all(&bin_dir).expect("create bin dir");
        let harness = install_fake_harness(&bin_dir);
        let escaped = harness.display().to_string().replace('\\', "\\\\");

        let config_dir = base.join("config");
        std::fs::create_dir_all(&config_dir).expect("create config dir");
        std::fs::write(
            config_dir.join("config.toml"),
            format!(
                "version = 1\n\n\
                 [integrations.claude-code]\nenabled = true\nexecutable = \"{escaped}\"\n\n\
                 [providers.v1-route]\ntemplate = \"openrouter\"\n\
                 credential_env = [\"{}\"]\n\n\
                 [profiles.direct]\nharness = \"claude-code\"\n\
                 expected_protocol = \"openai-chat\"\n\n\
                 [profiles.direct.backend]\nkind = \"direct-provider\"\n\
                 provider = \"v1-route\"\n",
                Self::CREDENTIAL_VAR
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
            .env(Self::CREDENTIAL_VAR, "planted-opaque-route-value")
            .env("PATH", self.base.join("empty-path"))
            .output()
            .expect("the glasshouse binary must be runnable");
        String::from_utf8_lossy(&output.stdout).into_owned()
    }
}

#[cfg(unix)]
fn install_fake_harness(bin_dir: &Path) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let path = bin_dir.join("fake-claude-code");
    std::fs::write(&path, "#!/bin/sh\nexit 0\n").expect("write fake harness");
    let mut perms = std::fs::metadata(&path).unwrap().permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&path, perms).unwrap();
    path
}

#[cfg(windows)]
fn install_fake_harness(bin_dir: &Path) -> PathBuf {
    let path = bin_dir.join("fake-claude-code.cmd");
    std::fs::write(&path, "@echo off\r\nexit /b 0\r\n").expect("write fake harness");
    path
}

/// Line 1936. One `glasshouse route` decision's report shows all four
/// things a reader needs to answer "why this one": the workload tier a
/// stated task classified to, session affinity, resource capacity (known
/// quota pressure), and a primary reason readable in the "why" section
/// (`routing/session.rs::workload_tier_fit`, `::session_affinity`,
/// `::quota_pressure`, `Routed::render_overview`).
#[test]
fn line_1936_the_route_report_shows_workload_tier_affinity_capacity_and_a_primary_reason() {
    let fixture = RouteFixture::new();
    // A task the heuristic classifies above its default tier, so
    // `workload_tier_fit` becomes a live contribution rather than absent —
    // `TaskRequirements::minimum_tier` is `Some` whenever a task was
    // classified at all (`routing/request.rs::RouterAnswer::requirements`).
    let report = fixture.stdout(&[
        "route",
        "--task",
        "review this repository's routing logic for correctness bugs",
    ]);

    assert!(
        report.contains("workload tier fit"),
        "line 1936's tier half: a classified task must show the workload-tier term:\n{report}"
    );
    assert!(
        report.contains("session affinity"),
        "line 1936's affinity half:\n{report}"
    );
    assert!(
        report.contains("known quota pressure"),
        "line 1936's resource-capacity half:\n{report}"
    );

    // The primary reason: the "why" section names every contribution with
    // its magnitude and a sentence — the one with the largest magnitude is
    // what a reader identifies as the decisive reason, and it must carry
    // real explanatory text rather than a bare number.
    let why = report
        .split("why\n")
        .nth(1)
        .expect("the report must have a \"why\" section naming the primary reason");
    let mut best: Option<(f64, &str)> = None;
    for line in why.lines() {
        let line = line.trim();
        if line.is_empty() || line == "alternatives" || line == "rejected" {
            break;
        }
        let Some(magnitude_str) = line.split_whitespace().next() else {
            continue;
        };
        let Ok(magnitude) = magnitude_str.parse::<f64>() else {
            continue;
        };
        if best.is_none_or(|(current, _)| magnitude.abs() > current.abs()) {
            best = Some((magnitude, line));
        }
    }
    let (_, primary) = best.expect("at least one contribution must be present");
    assert!(
        primary.contains(" — "),
        "the primary reason must carry explanatory text, not a bare score: {primary}"
    );
}

// ---------------------------------------------------------------------------
// Line 1937 — a gateway-backed route records a classified success and a
// failure with first-byte timing, and a later route explanation cites it.
// ---------------------------------------------------------------------------

fn test_credential(var: &str) -> Secret {
    // SAFETY: `var` is unique to this test and removed immediately after
    // resolving it, before it is inspected, so no other test observes it.
    unsafe {
        std::env::set_var(var, "sk-planted-not-a-real-key-v1-criteria");
    }
    let resolved = EnvironmentSecretStore::new()
        .resolve(&SecretRef::Environment {
            var: var.to_owned(),
        })
        .expect("the variable was just set");
    unsafe {
        std::env::remove_var(var);
    }
    resolved
}

fn gateway_response(status_line: &str, headers: &[&str], body: &[u8]) -> Vec<u8> {
    let mut head = format!("HTTP/1.1 {status_line}\r\nConnection: close\r\n");
    for header in headers {
        head.push_str(header);
        head.push_str("\r\n");
    }
    head.push_str("\r\n");
    let mut bytes = head.into_bytes();
    bytes.extend_from_slice(body);
    bytes
}

/// A local HTTP server answering its connections in order, one scripted
/// response each — `tests/gateway_failure_taxonomy.rs`'s own `stub_server`.
fn stub_server(responses: Vec<Vec<u8>>) -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").expect("loopback is bindable");
    let address = listener
        .local_addr()
        .expect("a bound listener has an address");
    listener
        .set_nonblocking(true)
        .expect("a listener can be put in polling mode");

    std::thread::Builder::new()
        .name("v1-criteria-routing-stub".to_owned())
        .spawn(move || {
            for scripted in responses {
                let deadline = Instant::now() + Duration::from_secs(20);
                let mut stream = loop {
                    match listener.accept() {
                        Ok((stream, _peer)) => break Some(stream),
                        Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {
                            if Instant::now() >= deadline {
                                break None;
                            }
                            std::thread::sleep(Duration::from_millis(5));
                        }
                        Err(_) => break None,
                    }
                };
                let Some(stream) = stream.as_mut() else {
                    return;
                };
                let _ = stream.set_nonblocking(false);
                let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
                let mut buf = [0u8; 4096];
                let _ = stream.read(&mut buf);
                let _ = stream.write_all(&scripted);
                let _ = stream.flush();
            }
        })
        .expect("can spawn the stub server thread");

    address
}

fn messages_request(model: &str, token: &str) -> Vec<u8> {
    let body = format!(r#"{{"model":"{model}"}}"#);
    format!(
        "POST /v1/messages HTTP/1.1\r\n\
         Host: 127.0.0.1\r\n\
         Authorization: Bearer {token}\r\n\
         Content-Type: application/json\r\n\
         Anthropic-Version: 2023-06-01\r\n\
         Content-Length: {}\r\n\
         \r\n\
         {body}",
        body.len()
    )
    .into_bytes()
}

fn send_and_read(address: SocketAddr, raw: &[u8]) -> Vec<u8> {
    let mut client = TcpStream::connect(address).expect("the gateway accepts");
    client
        .set_read_timeout(Some(Duration::from_secs(20)))
        .expect("a non-zero read timeout is valid");
    client.write_all(raw).expect("the gateway reads");
    client.flush().expect("the gateway reads");
    let mut out = Vec::new();
    client
        .read_to_end(&mut out)
        .expect("the gateway answers and closes");
    out
}

fn wait_for_rows(
    ledger: &EvidenceLedger,
    query: ObservationQuery<'_>,
    expected: usize,
) -> Vec<glasshouse::routing::evidence::RoutingObservation> {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let mut rows = ledger.recent(query, 64).unwrap();
        if rows.len() >= expected || Instant::now() >= deadline {
            rows.sort_by_key(|row| row.seq);
            return rows;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// Line 1937. A real gateway, started through the production entry point
/// against a fixture upstream, forwards one request that succeeds and one
/// that fails: `routing_observations` carries both, classified, with
/// first-byte timing on each — and a later `glasshouse route`, pointed at
/// the same data directory, cites the health that exchange left behind in
/// its own "provider health" term (`docs/product/evidence/phase-33a.md`,
/// `phase-33c.md`, `phase-51.md`).
#[test]
fn line_1937_a_gateway_backed_route_records_a_success_and_a_failure_and_route_cites_it() {
    const PROVIDER: &str = "v1-criteria-fixture";
    const CREDENTIAL_VAR: &str = "GLASSHOUSE_V1_CRITERIA_GATEWAY_KEY";
    const MODEL: &str = "v1-fixture-model";

    let tmp = tempfile::tempdir().expect("tempdir");
    let base = tmp.path().to_path_buf();
    let root = base.join("workspace").join("proj");
    std::fs::create_dir_all(root.join(".git")).expect("create project root");
    let root = std::fs::canonicalize(&root).expect("canonicalize project root");
    let config_dir = base.join("config");
    std::fs::create_dir_all(&config_dir).expect("create config dir");

    let runtime = {
        let cli = Cli::try_parse_from([
            "glasshouse",
            "--data-dir",
            base.join("data").to_str().unwrap(),
            "--config-dir",
            config_dir.to_str().unwrap(),
        ])
        .unwrap();
        glasshouse::bootstrap(&cli, &root).unwrap()
    };
    let ledger = Arc::new(EvidenceLedger::open(&runtime).unwrap());
    let health_cache = GatewayHealthCache::new(runtime.paths());

    // One success, one failure, from a fixture upstream on loopback.
    let address = stub_server(vec![
        gateway_response("200 OK", &["Content-Length: 2"], b"{}"),
        gateway_response("500 Internal Server Error", &["Content-Length: 0"], b""),
    ]);
    let upstream_backend = UpstreamBackend::new(
        PROVIDER.to_owned(),
        vec![Route::new(
            ANTHROPIC.to_owned(),
            &["/messages"],
            &format!("http://{address}"),
        )],
        test_credential(CREDENTIAL_VAR),
        CredentialId::new(
            PROVIDER,
            SecretRef::Environment {
                var: CREDENTIAL_VAR.to_owned(),
            },
        ),
        Cost::Metered,
    )
    .expect("a loopback http URL is absolute and this credential is header-safe");
    let upstream = Upstream::with_failover(vec![upstream_backend]).expect("one backend, not none");

    let mut profile = LaunchProfile::native(IntegrationId::ClaudeCode);
    profile.backend = BackendResource::GlasshouseGateway;
    let gateway = glasshouse::gateway::start_if_required_with_telemetry(
        &[profile],
        || Ok(upstream),
        None,
        Some(ledger.clone()),
        Some(health_cache),
    )
    .expect("loopback is bindable")
    .expect("a gateway-backed profile requires a gateway");
    gateway.routing().bind(
        "claude-code",
        ANTHROPIC,
        AssignedModel::named(MODEL),
        gateway.upstream(),
    );

    let ok = send_and_read(
        gateway.address(),
        &messages_request(MODEL, gateway.token().expose()),
    );
    assert!(
        String::from_utf8_lossy(&ok).starts_with("HTTP/1.1 200"),
        "the fixture's first scripted response must succeed"
    );
    let failed = send_and_read(
        gateway.address(),
        &messages_request(MODEL, gateway.token().expose()),
    );
    assert!(
        String::from_utf8_lossy(&failed).starts_with("HTTP/1.1 500"),
        "the fixture's second scripted response must fail"
    );

    let rows = wait_for_rows(
        &ledger,
        ObservationQuery {
            provider: PROVIDER,
            model: MODEL,
            route: Some(ANTHROPIC),
            harness: Some("claude-code"),
        },
        2,
    );
    assert_eq!(rows.len(), 2, "one row per exchange, classified and timed");
    assert_eq!(rows[0].outcome, Some(Outcome::Succeeded), "{rows:?}");
    assert!(
        rows[0].first_byte_at_unix.is_some(),
        "the successful exchange must carry a first-byte timestamp: {:?}",
        rows[0]
    );
    assert_eq!(rows[1].outcome, Some(Outcome::Failed), "{rows:?}");
    assert!(
        rows[1].first_byte_at_unix.is_some(),
        "the failed exchange must still carry a first-byte timestamp — it reached the \
         provider and got an answer: {:?}",
        rows[1]
    );

    drop(gateway);

    // A later `glasshouse route`, pointed at the same data directory, with
    // a direct-provider profile naming the same resource this gateway just
    // observed.
    let bin_dir = base.join("bin");
    std::fs::create_dir_all(&bin_dir).expect("create bin dir");
    let harness = install_fake_harness(&bin_dir);
    let escaped = harness.display().to_string().replace('\\', "\\\\");
    std::fs::write(
        config_dir.join("config.toml"),
        format!(
            "version = 1\n\n\
             [integrations.claude-code]\nenabled = true\nexecutable = \"{escaped}\"\n\n\
             [providers.{PROVIDER}]\ntemplate = \"anthropic-compatible\"\n\
             base_url = \"http://127.0.0.1:9/\"\ncredential_env = [\"{CREDENTIAL_VAR}\"]\n\n\
             [profiles.direct]\nharness = \"claude-code\"\nmodel = \"{MODEL}\"\n\
             expected_protocol = \"{ANTHROPIC}\"\n\n\
             [profiles.direct.backend]\nkind = \"direct-provider\"\nprovider = \"{PROVIDER}\"\n"
        ),
    )
    .expect("write user config");

    let output = Command::new(env!("CARGO_BIN_EXE_glasshouse"))
        .arg("--scope")
        .arg(&root)
        .arg("--data-dir")
        .arg(base.join("data"))
        .arg("--config-dir")
        .arg(&config_dir)
        .arg("route")
        .env(CREDENTIAL_VAR, "planted-opaque-value")
        .output()
        .expect("the glasshouse binary must be runnable");
    let report = String::from_utf8_lossy(&output.stdout).into_owned();
    assert!(
        output.status.success(),
        "route must succeed:\n{report}\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        report.contains("provider health"),
        "the report must carry the provider-health term at all:\n{report}"
    );
    assert!(
        report.contains("1 consecutive observed failures"),
        "the report must cite the failure this gateway exchange just recorded, not print the \
         no-observation default:\n{report}"
    );
}
