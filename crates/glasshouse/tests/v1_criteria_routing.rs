//! Three V1-completion criteria (map lines 1931, 1932, 1935) over
//! Glasshouse's quota and guardrail phases — each already closed by
//! production code the map records elsewhere. One test per criterion,
//! entering through the shipped binary or the nearest deterministic
//! production seam, per `.agent-runtime/packet-prove-it-v1-routing.md`.
//!
//! Lines 1930, 1933, 1934, 1936 and 1937 — this file's original eight —
//! were routing criteria (`SessionRouter::choose`, `glasshouse
//! classify`/`route`, `routing::disposable`'s reserve-spend caller) and went
//! with the router (design-decisions.md, 2026-09-16, "Glasshouse never
//! decides which model is used"): `evaluate_reserve_spend` in particular has
//! no production caller left at all once `routing::disposable::choose` is
//! gone, so proving it reachable no longer proves anything about this
//! build.

use std::path::{Path, PathBuf};
use std::process::Command;

use clap::Parser;

use glasshouse::integrations::IntegrationId;
use glasshouse::{Cli, Runtime};

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
    let quota_cache_dir = fixture.config.path().join("gateway").join("gateway-quota");
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
    let subscription =
        glasshouse::provider::registry::native_subscription(IntegrationId::ClaudeCode).capacity();

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
// Line 1935 — a substantial task records assumptions with evidence state
// and creates a checkpoint before implementation.
// ---------------------------------------------------------------------------

use std::io::{BufRead, Write};
use std::process::{Child, ChildStdin, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::time::Duration;

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
