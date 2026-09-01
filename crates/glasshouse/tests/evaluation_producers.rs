//! Phase 51 — the producers the evaluation and evidence ledgers were still
//! missing, and what a person reads off them.
//!
//! - **1832** *"Measure memory-extraction cost separately from interactive
//!   coding cost."*
//! - **1833** *"Measure routing-model cost and request consumption separately
//!   from interactive coding cost."*
//! - **1834** *"Measure how often workload-tier classification predicts
//!   successful execution without escalation."*
//! - **1851** *"Measure how often failure-domain evidence prevents a failover
//!   onto the same unhealthy upstream."*
//! - **1854** *"Measure how often sparse, stale, or incorrectly segmented
//!   evidence causes a poor routing decision."* — the **stale** half; *sparse*
//!   landed with `tests/routing_outcome.rs` and *incorrectly segmented* still
//!   has no producer anywhere and is not asserted here.
//!
//! # Everything that has a production entry point is entered through it
//!
//! Practice §35: a caller every test bypasses is not a caller. So the
//! stamping test spawns `glasshouse hook` the way a harness does, against a
//! model that really answers on a real socket; the tier and staleness tests
//! run `glasshouse launch`; the failover test starts a real gateway through
//! `gateway::start_if_required_with_degrade_sink`, the same door `main.rs`
//! calls, and makes a real HTTP request to it.
//!
//! The one thing entered below its production caller is the **rendering** of
//! consumption by purpose, where the ledger rows are planted directly. That
//! is deliberate and it is not the §35 shape: the producers for those rows
//! are proved by the first test and by `tests/classification_call.rs`, and
//! what is left to show is arithmetic over a window that a launch cannot
//! place rows in.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use clap::Parser;
use rusqlite::Connection;

use glasshouse::config::{ExtractionModelRef, ProviderConfig, UserConfig};
use glasshouse::evaluation::{EvaluationKind, EvaluationObservations};
use glasshouse::provider::telemetry::{GatewayHealthCache, GatewayHealthReading};
use glasshouse::routing::evidence::{
    CLASSIFICATION_PURPOSE, EXTRACTION_PURPOSE, EvidenceLedger, NewObservation,
    ROUTING_LATENCY_PURPOSE,
};
use glasshouse::session::{NewSession, ProjectSessions, SessionId, SessionLifecycle};
use glasshouse::{Cli, Runtime};

// ===========================================================================
// A canned OpenAI chat-completions endpoint, adapted from `usage_reader.rs`.
// ===========================================================================

/// What a cheap model answers, in the extraction contract's own shape.
const ONE_FINDING: &str = r#"{"memories":[{"kind":"finding","authority":"constraint",
     "disposition":"accepted","support":"established","confidence":"certain",
     "rationale":"the hook process is the only thing that sees a turn end",
     "project_phase":"alpha",
     "body":"An extraction call is stamped with the purpose it was made for."}]}"#;

struct FakeModel {
    address: SocketAddr,
    stop: Arc<AtomicBool>,
}

impl FakeModel {
    fn answering_with_usage(input: i64, output: i64) -> Self {
        let document = serde_json::json!({
            "choices": [{ "message": { "role": "assistant", "content": ONE_FINDING } }],
            "usage": { "prompt_tokens": input, "completion_tokens": output },
        })
        .to_string();
        let listener = TcpListener::bind("127.0.0.1:0").expect("loopback must bind");
        listener
            .set_nonblocking(true)
            .expect("the accept loop polls its stop flag");
        let address = listener.local_addr().unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let thread_stop = Arc::clone(&stop);
        std::thread::spawn(move || {
            while !thread_stop.load(Ordering::SeqCst) {
                match listener.accept() {
                    Ok((stream, _)) => {
                        let _ = stream.set_nonblocking(false);
                        serve_json(stream, &document);
                    }
                    Err(ref err) if err.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(std::time::Duration::from_millis(5));
                    }
                    Err(_) => break,
                }
            }
        });
        Self { address, stop }
    }

    fn base_url(&self) -> String {
        format!("http://{}/v1", self.address)
    }
}

impl Drop for FakeModel {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
    }
}

/// Read one request head byte-oriented, read exactly `content-length` bytes,
/// and answer `document` — nothing in this crate is reused, so *"the call
/// happened"* stays a claim about the wire.
fn serve_json(mut stream: TcpStream, document: &str) {
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
    let response = format!(
        "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\
         connection: close\r\n\r\n{document}",
        document.len()
    );
    let _ = stream.write_all(response.as_bytes());
    let _ = stream.flush();
}

fn now_unix() -> i64 {
    glasshouse::provider::cache::now_unix_seconds()
}

fn both_streams(output: &std::process::Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
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

// ===========================================================================
// A project with a fake harness and two direct-provider profiles.
// ===========================================================================

const CREDENTIAL_VAR: &str = "GLASSHOUSE_TEST_ONLY_EVALUATION_PRODUCERS_KEY";
const CREDENTIAL: &str = "sk-fabricated-test-value-not-a-real-credential";
const PROVIDER: &str = "probe";
const MODEL: &str = "probe/a-model";

struct Fixture {
    _tmp: tempfile::TempDir,
    base: PathBuf,
    runtime: Runtime,
}

impl Fixture {
    fn new() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path().to_path_buf();
        let root = base.join("workspace");
        std::fs::create_dir_all(root.join(".git")).unwrap();
        let root = std::fs::canonicalize(&root).unwrap();

        let bin_dir = base.join("bin");
        std::fs::create_dir_all(&bin_dir).unwrap();
        let harness = install_fake_harness(&bin_dir);
        let escaped = harness.display().to_string().replace('\\', "\\\\");

        let config_dir = base.join("config");
        std::fs::create_dir_all(&config_dir).unwrap();
        std::fs::write(
            config_dir.join("config.toml"),
            format!(
                "version = 1\n\n\
                 [integrations.claude-code]\nenabled = true\nexecutable = \"{escaped}\"\n\n\
                 [providers.{PROVIDER}]\ntemplate = \"anthropic-compatible\"\n\
                 base_url = \"http://127.0.0.1:9/\"\n\
                 credential_env = [\"{CREDENTIAL_VAR}\"]\n\n\
                 [profiles.metered]\nharness = \"claude-code\"\nmodel = \"{MODEL}\"\n\n\
                 [profiles.metered.backend]\nkind = \"direct-provider\"\nprovider = \"{PROVIDER}\"\n"
            ),
        )
        .unwrap();

        let cli = Cli::try_parse_from([
            "glasshouse",
            "--data-dir",
            base.join("data").to_str().unwrap(),
            "--config-dir",
            config_dir.to_str().unwrap(),
        ])
        .unwrap();
        let runtime = glasshouse::bootstrap(&cli, &root).unwrap();
        Fixture {
            _tmp: tmp,
            base,
            runtime,
        }
    }

    fn data_dir(&self) -> PathBuf {
        self.base.join("data")
    }

    fn glasshouse(&self, args: &[&str]) -> std::process::Output {
        Command::new(env!("CARGO_BIN_EXE_glasshouse"))
            .current_dir(self.runtime.project().root())
            .env(CREDENTIAL_VAR, CREDENTIAL)
            .arg("--data-dir")
            .arg(self.data_dir())
            .arg("--config-dir")
            .arg(self.base.join("config"))
            .args(args)
            .output()
            .expect("the glasshouse binary must run")
    }

    /// Point Glasshouse at a model for memory extraction, exactly as a person
    /// writing configuration would.
    fn choose_extraction_model(&self, base_url: &str) {
        let mut user = UserConfig::load(self.runtime.paths()).unwrap();
        let mut provider = ProviderConfig::new("openai-compatible");
        provider.set_base_url(Some(base_url.to_owned()));
        provider.set_credential_env(vec![CREDENTIAL_VAR.to_owned()]);
        user.providers_mut().set("extractor", provider);
        user.set_memory_extraction_model(Some(ExtractionModelRef::new(
            "extractor",
            "a-cheap-local-model",
        )));
        user.save(self.runtime.paths()).unwrap();
    }

    /// Run `glasshouse hook`, exactly as a harness runs it: a separate
    /// process, the event on argv, a payload on standard input.
    fn hook(&self, session: &str, event: &str) {
        let mut child = Command::new(env!("CARGO_BIN_EXE_glasshouse"))
            .current_dir(self.runtime.project().root())
            .env(CREDENTIAL_VAR, CREDENTIAL)
            .arg("--data-dir")
            .arg(self.data_dir())
            .arg("--config-dir")
            .arg(self.base.join("config"))
            .arg("hook")
            .arg("--session")
            .arg(session)
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
            .write_all(HOOK_PAYLOAD.as_bytes())
            .expect("write the hook payload");
        let output = child.wait_with_output().expect("the hook must exit");
        assert!(
            output.status.success(),
            "a hook always exits zero:\n{}",
            both_streams(&output)
        );
    }

    /// Launch, and return the id of the one session it created.
    fn launch(&self, args: &[&str]) -> String {
        let before = self.session_ids();
        let mut argv = vec!["launch", "claude-code", "--headless"];
        argv.extend_from_slice(args);
        let launched = self.glasshouse(&argv);
        assert!(
            launched.status.success(),
            "the launch must succeed:\n{}",
            both_streams(&launched)
        );
        let mut created: Vec<String> = self
            .session_ids()
            .into_iter()
            .filter(|id| !before.contains(id))
            .collect();
        assert_eq!(
            created.len(),
            1,
            "one launch, one session; before: {before:?}"
        );
        created.remove(0)
    }

    fn db(&self) -> Connection {
        Connection::open(self.runtime.database_path()).unwrap()
    }

    fn session_ids(&self) -> Vec<String> {
        let conn = self.db();
        let mut statement = conn.prepare("SELECT id FROM sessions").unwrap();
        statement
            .query_map([], |row| row.get(0))
            .unwrap()
            .map(Result::unwrap)
            .collect()
    }

    fn ledger(&self) -> EvaluationObservations {
        EvaluationObservations::open(&self.runtime).unwrap()
    }

    /// `(subject, detail)` for the one row of `kind` naming this session.
    fn row_for(&self, kind: EvaluationKind, session: &str) -> (Option<String>, Option<String>) {
        let mut rows: Vec<_> = self
            .ledger()
            .recent_of_kind(kind, 50)
            .unwrap()
            .into_iter()
            .filter(|row| row.session_id.as_deref() == Some(session))
            .collect();
        assert_eq!(
            rows.len(),
            1,
            "exactly one `{}` row must name session `{session}`",
            kind.as_str()
        );
        let row = rows.remove(0);
        (row.subject, row.detail)
    }

    /// Every `(purpose, provider, input_tokens)` the evidence ledger holds,
    /// read straight out of the column rather than through an aggregate, so
    /// *"the stamp is on the row"* is a claim about the row.
    fn purposes(&self) -> Vec<(Option<String>, String, Option<i64>)> {
        let conn = self.db();
        let mut statement = conn
            .prepare(
                "SELECT purpose, provider, input_tokens FROM routing_observations ORDER BY seq",
            )
            .unwrap();
        statement
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
            .unwrap()
            .map(Result::unwrap)
            .collect()
    }

    fn running_session(&self) -> SessionId {
        let sessions = ProjectSessions::open(&self.runtime).unwrap();
        let store = sessions.store();
        let record = store.create(NewSession::embedded("claude-code")).unwrap();
        store
            .set_lifecycle(&record.id, SessionLifecycle::Running)
            .unwrap();
        record.id
    }
}

const HOOK_PAYLOAD: &str = concat!(
    r#"{"session_id":"native-1","transcript_path":"/somewhere/rollout.jsonl","#,
    r#""hook_event_name":"Stop","cwd":"/somewhere","model":"a-model","#,
    r#""prompt":"a prompt that must never be stored","#,
    r#""last_assistant_message":"a reply that must never be stored"}"#
);

// ===========================================================================
// 1832 — the stamp, on rows written from now on and on nothing else.
// ===========================================================================

/// **Acceptance 1.** An extraction call the shipped binary makes leaves a row
/// stamped `memory-extraction`, and a row already on disk with no purpose
/// keeps its `NULL`.
///
/// The mutation target is the stamp itself: drop `.with_purpose(...)` from
/// `main.rs::record_extraction_observation` and the row still appears with
/// its tokens, every other test still passes, and only this one fails —
/// which is the whole of what line 1832 asks for beyond what already
/// existed.
#[test]
fn extraction_rows_are_stamped_and_old_rows_are_not_relabelled() {
    let fixture = Fixture::new();

    // A row from before the stamp existed: no purpose, no harness, and real
    // token counts — exactly the shape every extraction row already on disk
    // has. `NewObservation::with_purpose` is deliberately not called.
    {
        let ledger = EvidenceLedger::open(&fixture.runtime).unwrap();
        ledger
            .record(
                NewObservation::new("older-build", "an-older-model").with_tokens(
                    Some(11),
                    Some(7),
                    None,
                ),
                now_unix() - 60,
            )
            .unwrap();
    }
    assert_eq!(
        fixture.purposes(),
        vec![(None, "older-build".to_owned(), Some(11))],
        "premise: the project starts with exactly one unstamped row"
    );

    let model = FakeModel::answering_with_usage(120, 34);
    fixture.choose_extraction_model(&model.base_url());
    let session = fixture.running_session();
    fixture.hook(session.as_str(), "Stop");

    let rows = fixture.purposes();
    assert_eq!(
        rows.len(),
        2,
        "the extraction call must have left exactly one new row: {rows:?}"
    );
    assert_eq!(
        rows[0],
        (None, "older-build".to_owned(), Some(11)),
        "the row written before the stamp existed must still carry `NULL` — a back-filled \
         purpose would make `this build recorded nothing here` indistinguishable from \
         `this build recorded a purpose`"
    );
    assert_eq!(
        rows[1].0.as_deref(),
        Some(EXTRACTION_PURPOSE),
        "the row this extraction wrote must say what the call was for: {rows:?}"
    );
    assert_eq!(
        rows[1].2,
        Some(120),
        "the stamp must not have displaced the counts the model reported"
    );
}

// ===========================================================================
// 1832 / 1833 — the rendering, tokens and calls, each with its denominator.
// ===========================================================================

/// **Acceptance 2.** `glasshouse resources` separates what extraction spent,
/// what the routing model consumed, what the coding agent relayed, and what
/// no producer stamped — every one of them with **both** denominators, tokens
/// and calls.
///
/// The rows are planted directly: their producers are proved by acceptance 1
/// above and by `tests/classification_call.rs`, and what is left is
/// arithmetic over a window a launch cannot place rows in.
#[test]
fn resources_separates_extraction_and_routing_consumption_by_tokens_and_calls() {
    let fixture = Fixture::new();
    let now = now_unix();
    {
        let ledger = EvidenceLedger::open(&fixture.runtime).unwrap();
        // Two classification calls, 150 tokens.
        for i in 0..2 {
            ledger
                .record(
                    NewObservation::new("router", "a-router-model")
                        .with_purpose(Some(CLASSIFICATION_PURPOSE))
                        .with_tokens(Some(50), Some(25), None),
                    now - 100 + i,
                )
                .unwrap();
        }
        // Three extraction calls, 900 tokens.
        for i in 0..3 {
            ledger
                .record(
                    NewObservation::new("extractor", "a-cheap-local-model")
                        .with_purpose(Some(EXTRACTION_PURPOSE))
                        .with_tokens(Some(200), Some(100), None),
                    now - 90 + i,
                )
                .unwrap();
        }
        // Four decision-latency rows, which carry no tokens at all.
        for i in 0..4 {
            ledger
                .record(
                    NewObservation::new("glasshouse", "session-router")
                        .with_purpose(Some(ROUTING_LATENCY_PURPOSE))
                        .with_harness(Some("claude-code")),
                    now - 80 + i,
                )
                .unwrap();
        }
        // Five relayed exchanges: a harness, and no counts, because the
        // gateway relays a body it never parses.
        for i in 0..5 {
            ledger
                .record(
                    NewObservation::new("anyrouter", "a-coding-model")
                        .with_harness(Some("claude-code")),
                    now - 70 + i,
                )
                .unwrap();
        }
        // One row from before any purpose was stamped.
        ledger
            .record(
                NewObservation::new("older-build", "an-older-model").with_tokens(
                    Some(10),
                    Some(5),
                    None,
                ),
                now - 60,
            )
            .unwrap();
    }

    let ran = fixture.glasshouse(&["resources", "--no-harness"]);
    assert!(ran.status.success(), "{}", both_streams(&ran));
    let stdout = String::from_utf8_lossy(&ran.stdout).into_owned();
    let block = stdout
        .split_once("ROUTING ECONOMICS")
        .unwrap_or_else(|| panic!("no ROUTING ECONOMICS block in:\n{stdout}"))
        .1;

    for expected in [
        "routing spend   150 tokens over 2 classification calls",
        "extraction      900 tokens over 3 extraction calls",
        "routing model   tokens not counted over 4 decision rows",
        "coding agent    tokens not counted over 5 relayed exchanges",
        "unstamped       15 tokens over 1 calls",
        // Line 1465's own aggregate is unchanged and still sums to exactly
        // the four lines above it.
        "task spend      915 tokens over 13 other calls",
    ] {
        assert!(
            block.contains(expected),
            "missing `{expected}` in:\n{block}"
        );
    }
    assert!(
        block.contains("never re-labelled"),
        "the unstamped line must say that those rows were not moved into a bucket somebody \
         invented for them:\n{block}"
    );
}

// ===========================================================================
// 1834 — the tier the decision used, and whether it was escalated.
// ===========================================================================

/// **Acceptance 3.** A launch that states a task records the tier its routing
/// decision used and whether line 1459's conservative rule moved it; a launch
/// that states none records `unclassified`, **its own bucket and never
/// nothing**; and `glasshouse route` prints the buckets with denominators.
///
/// Three launches, three shapes, and all three through the shipped binary —
/// `record_routed_session`'s tier argument is computed from the classification
/// the decision actually acted on, so a test that built a `RoutingTier` itself
/// would assert about a value no launch produced (practice §35).
#[test]
fn a_classified_launch_records_its_tier_and_escalation_and_an_unclassified_one_says_so() {
    let fixture = Fixture::new();

    // A task naming shell execution: the heuristic classifier matches a
    // signal, so confidence is `Medium` and the conservative rule does not
    // fire — `heavy`, stated `heavy`.
    let confident = fixture.launch(&[
        "--profile",
        "metered",
        "--task",
        "run cargo test and fix whatever fails",
    ]);
    assert_eq!(
        fixture.row_for(EvaluationKind::RoutingTierObserved, &confident),
        (Some("heavy".to_owned()), Some("heavy".to_owned())),
        "a task matching a signal is classified with confidence, and its tier is the one the \
         classifier stated"
    );

    // A task matching no signal at all: `Confidence::Low`, so the tier the
    // decision used is one step above the `leaf` the classifier stated.
    let escalated = fixture.launch(&["--fresh", "--profile", "metered", "--task", "hello"]);
    assert_eq!(
        fixture.row_for(EvaluationKind::RoutingTierObserved, &escalated),
        (
            Some("standard-escalated".to_owned()),
            Some("leaf".to_owned())
        ),
        "an uncertain classification is escalated, the row says so, and the `detail` keeps the \
         tier the classifier itself stated — without which nobody could tell what was escalated \
         from"
    );

    // No `--task`: nothing classified this launch, and that is a bucket
    // rather than an absence.
    let unclassified = fixture.launch(&["--fresh", "--profile", "metered"]);
    assert_eq!(
        fixture.row_for(EvaluationKind::RoutingTierObserved, &unclassified),
        (Some("unclassified".to_owned()), None),
        "a launch that states no task still made a routing decision; recording nothing would \
         make `this project never states its tasks` read as `this project never launches`"
    );

    // The reader. Both denominators, no percentage.
    let ran = fixture.glasshouse(&["route", "--moment", "session-start"]);
    assert!(ran.status.success(), "{}", both_streams(&ran));
    let stdout = String::from_utf8_lossy(&ran.stdout).into_owned();
    let block = stdout
        .split_once("Past routes in this project")
        .unwrap_or_else(|| panic!("no past-routes section in:\n{stdout}"))
        .1;
    assert!(
        block.contains(
            "by workload tier the decision used, and whether the conservative rule \
                        escalated it"
        ),
        "{block}"
    );
    for expected in [
        "heavy",
        "standard-escalated",
        "unclassified",
        "1 session routed",
    ] {
        assert!(
            block.contains(expected),
            "missing `{expected}` in:\n{block}"
        );
    }
    assert!(
        !block.contains('%'),
        "a bare percentage cannot be told from a lucky afternoon:\n{block}"
    );
}

// ===========================================================================
// 1854 — the stale half, and a reading nothing can date.
// ===========================================================================

/// The credential label the write side renders for this fixture's provider —
/// `CredentialId::label()` for a `SecretRef::Environment`, which is what
/// `gateway::session::SessionRouting::health_readings_for` persists and what
/// `main.rs::observed_health_of` matches on, forward only.
fn credential_label() -> String {
    format!("{PROVIDER}/{CREDENTIAL_VAR}")
}

fn health_reading() -> GatewayHealthReading {
    GatewayHealthReading {
        credential_label: credential_label(),
        model: MODEL.to_owned(),
        consecutive_failures: 1,
        cooling_down_until_unix: None,
        cooldown_cause: None,
        credential_rejected: false,
    }
}

/// **Acceptance 5.** A health reading older than the horizon is recorded as
/// `observed-stale`; a fresh one as `observed-fresh`; and a cache file
/// nothing can date is `absent`, **never fresh**.
#[test]
fn a_stale_health_reading_is_recorded_as_stale_and_a_pre_change_reading_as_absent() {
    use glasshouse::evaluation::HEALTH_EVIDENCE_HORIZON_SECONDS;

    // --- fresh -------------------------------------------------------------
    let fixture = Fixture::new();
    let cache = GatewayHealthCache::at(fixture.data_dir().join("gateway-health"));
    cache.store(PROVIDER, &[health_reading()], now_unix());
    assert_eq!(
        cache.load(PROVIDER).len(),
        1,
        "premise: the planted reading is on disk and readable through the reader production uses"
    );
    let session = fixture.launch(&["--profile", "metered"]);
    assert_eq!(
        fixture
            .row_for(EvaluationKind::RoutingEvidenceObserved, &session)
            .0
            .as_deref(),
        Some("observed-fresh"),
        "a reading written a moment ago is what the router was actually holding"
    );

    // --- stale -------------------------------------------------------------
    let fixture = Fixture::new();
    let cache = GatewayHealthCache::at(fixture.data_dir().join("gateway-health"));
    let long_ago = now_unix() - HEALTH_EVIDENCE_HORIZON_SECONDS - 60;
    cache.store(PROVIDER, &[health_reading()], long_ago);
    let session = fixture.launch(&["--profile", "metered"]);
    assert_eq!(
        fixture
            .row_for(EvaluationKind::RoutingEvidenceObserved, &session)
            .0
            .as_deref(),
        Some("observed-stale"),
        "past the horizon the reading no longer describes the resource the router is choosing, \
         and the row must say so rather than call it evidence held"
    );

    // --- a file nothing can date ------------------------------------------
    //
    // The shape a cache written before this build's reader existed would
    // have: every field but the file's own timestamp. It must read as
    // `absent`, which is what "a reading whose age is unknown is never
    // fresh" means in practice.
    let fixture = Fixture::new();
    let health_dir = fixture.data_dir().join("gateway-health");
    std::fs::create_dir_all(&health_dir).unwrap();
    std::fs::write(
        health_dir.join(format!("{PROVIDER}.json")),
        serde_json::json!({
            "version": 1,
            "provider": PROVIDER,
            "entries": [{
                "credential_label": credential_label(),
                "model": MODEL,
                "consecutive_failures": 1,
                "cooling_down_until_unix": null,
                "credential_rejected": false,
            }],
        })
        .to_string(),
    )
    .unwrap();
    let session = fixture.launch(&["--profile", "metered"]);
    assert_eq!(
        fixture
            .row_for(EvaluationKind::RoutingEvidenceObserved, &session)
            .0
            .as_deref(),
        Some("absent"),
        "an undatable reading must never be read as a fresh one — that is the single \
         substitution that turns a missing fact into a favourable one"
    );
}

// ===========================================================================
// 1851 — what the failure-domain term did to a real failover's ranking.
// ===========================================================================

/// The provider whose backend fails, and the one that is somewhere else
/// entirely. `SHARED` is a second credential on `FAILING`'s **own** provider
/// — line 1372's case exactly: a different queue onto the same upstream.
const FAILING: &str = "fixture-failing-provider";
const ELSEWHERE: &str = "fixture-other-provider";
const FAILOVER_MODEL: &str = "stub-model";

/// A credential resolved through the real environment store — `Secret` has no
/// public constructor outside `crate::secret`, so this is
/// `gateway_retry_after.rs`'s own helper, unchanged.
fn planted_credential(var: &str) -> glasshouse::secret::Secret {
    use glasshouse::secret::{EnvironmentSecretStore, SecretRef, SecretStore};

    // SAFETY: `var` is unique to the one call site that sets it and is removed
    // again before the resolved value is even inspected, so no other test in
    // this binary can observe it set.
    unsafe {
        std::env::set_var(var, "sk-planted-not-a-real-key-failover");
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

fn upstream_backend(
    provider: &str,
    var: &str,
    address: SocketAddr,
) -> glasshouse::gateway::UpstreamBackend {
    use glasshouse::gateway::{Route, UpstreamBackend};
    use glasshouse::routing::{Cost, CredentialId};
    use glasshouse::secret::SecretRef;

    UpstreamBackend::new(
        provider.to_owned(),
        vec![Route::new(
            "anthropic-messages".to_owned(),
            &["/messages"],
            &format!("http://{address}"),
        )],
        planted_credential(var),
        CredentialId::new(
            provider,
            SecretRef::Environment {
                var: var.to_owned(),
            },
        ),
        Cost::Metered,
    )
    .expect("a loopback http URL is absolute and this credential is header-safe")
}

/// A server that answers one connection with a `500` and exits — a genuine
/// provider failure, and deliberately **not** a `429` or a `401`, which
/// `observe_exchange` answers with credential rotation before any failover
/// ranking happens at all.
fn stub_500_server() -> SocketAddr {
    use std::time::{Duration, Instant};

    let listener = TcpListener::bind("127.0.0.1:0").expect("loopback is bindable");
    let address = listener
        .local_addr()
        .expect("a bound listener has an address");
    listener
        .set_nonblocking(true)
        .expect("a listener can be put in polling mode");
    std::thread::Builder::new()
        .name("evaluation-producers-stub-500".to_owned())
        .spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(10);
            let mut stream = loop {
                match listener.accept() {
                    Ok((stream, _)) => break Some(stream),
                    Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {
                        if Instant::now() >= deadline {
                            break None;
                        }
                        std::thread::sleep(Duration::from_millis(10));
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
            let _ = stream
                .write_all(b"HTTP/1.1 500 Internal Server Error\r\nContent-Length: 0\r\n\r\n");
            let _ = stream.flush();
        })
        .expect("can spawn the stub server thread");
    address
}

fn messages_request(token: &str) -> Vec<u8> {
    let body = format!(r#"{{"model":"{FAILOVER_MODEL}"}}"#);
    format!(
        "POST /v1/messages HTTP/1.1\r\nHost: 127.0.0.1\r\nAuthorization: Bearer {token}\r\n\
         Content-Type: application/json\r\nAnthropic-Version: 2023-06-01\r\n\
         Content-Length: {}\r\n\r\n{body}",
        body.len()
    )
    .into_bytes()
}

fn send_and_read(address: SocketAddr, raw: &[u8]) -> String {
    use std::time::Duration;

    let mut client = TcpStream::connect(address).expect("the gateway accepts connections");
    client
        .set_read_timeout(Some(Duration::from_secs(20)))
        .unwrap();
    client
        .write_all(raw)
        .expect("the gateway reads the request");
    client.flush().expect("the gateway reads the request");
    let mut out = Vec::new();
    client
        .read_to_end(&mut out)
        .expect("the gateway answers and then closes");
    String::from_utf8_lossy(&out).into_owned()
}

/// Every prevention the sink was told about, as `(prevented, displaced)` —
/// exactly the two things `FailureDomainEffect` can answer.
type Preventions = Vec<(bool, Option<String>)>;

/// Drive one real exchange through a real gateway whose serving backend
/// answers `500`, with `candidates` behind it in the caller's own order, and
/// return every prevention the sink was told about.
///
/// The gateway is started through `start_if_required_with_degrade_sink` —
/// **the same door `main.rs` calls at both of its launch sites** — so the
/// sink argument being read at all is proved here rather than assumed.
fn preventions_after_a_failover(
    candidates: Vec<glasshouse::gateway::UpstreamBackend>,
) -> Preventions {
    use std::time::{Duration, Instant};

    use glasshouse::gateway::Upstream;
    use glasshouse::integrations::IntegrationId;
    use glasshouse::profile::{BackendResource, LaunchProfile};
    use glasshouse::routing::AssignedModel;

    let seen: Arc<Mutex<Preventions>> = Arc::new(Mutex::new(Vec::new()));
    let sink_seen = Arc::clone(&seen);
    let sink: glasshouse::gateway::session::FailoverPreventionSink = Arc::new(
        move |effect: &glasshouse::routing::interactive::FailureDomainEffect| {
            sink_seen
                .lock()
                .unwrap()
                .push((effect.prevented(), effect.displaced().map(str::to_owned)));
        },
    );

    let upstream = Upstream::with_failover(candidates).expect("a non-empty backend list");
    let mut profile = LaunchProfile::native(IntegrationId::ClaudeCode);
    profile.backend = BackendResource::GlasshouseGateway;
    let gateway = glasshouse::gateway::start_if_required_with_degrade_sink(
        &[profile],
        || Ok(upstream),
        None,
        None,
        None,
        None,
        Some(sink),
    )
    .expect("loopback is bindable")
    .expect("a gateway-backed profile requires a gateway");

    gateway.routing().bind(
        "claude-code",
        "anthropic-messages",
        AssignedModel::named(FAILOVER_MODEL),
        gateway.upstream(),
    );

    let response = send_and_read(
        gateway.address(),
        &messages_request(gateway.token().expose()),
    );
    assert!(
        response.starts_with("HTTP/1.1 500"),
        "the gateway must relay the provider's own 500: {response}"
    );

    // The connection thread's routing bookkeeping runs after `ingress::serve`
    // has closed the response socket, so the client finishing is not proof
    // the sink has been called yet — `gateway_retry_after.rs`'s own finding.
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let rows = seen.lock().unwrap().clone();
        if !rows.is_empty() || Instant::now() >= deadline {
            return rows;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// **Acceptance 4.** A failover whose winner the failure-domain term changed
/// is counted as `prevented` and **names the candidate it displaced**; one
/// where the term changed nothing is counted as `not-prevented` rather than
/// omitted, because a numerator without a denominator is not a rate.
///
/// The two cases differ in exactly one input — whether the first candidate
/// behind the failing backend is on the failing backend's own provider — and
/// nothing else. With no evidence ledger every candidate scores `0.0`, so
/// `best`'s first-seen tie-break picks the first candidate; the `-1.0`
/// failure-domain term is therefore the only thing that can move the winner,
/// which is what makes this a measurement of the term rather than of the
/// ranking.
#[test]
fn a_failover_the_domain_term_prevented_is_counted_and_one_it_did_not_is_not() {
    let failing = stub_500_server();
    let elsewhere = stub_500_server();

    // The first candidate behind the failing backend shares its provider, so
    // without the term it would win the tie and the session would move to
    // another queue onto the same upstream.
    let prevented = preventions_after_a_failover(vec![
        upstream_backend(FAILING, "GLASSHOUSE_TEST_ONLY_FAILOVER_A1", failing),
        upstream_backend(FAILING, "GLASSHOUSE_TEST_ONLY_FAILOVER_A2", failing),
        upstream_backend(ELSEWHERE, "GLASSHOUSE_TEST_ONLY_FAILOVER_A3", elsewhere),
    ]);
    assert_eq!(
        prevented.len(),
        1,
        "one failover, one prevention row: {prevented:?}"
    );
    assert!(
        prevented[0].0,
        "the term displaced the shared-upstream candidate that would otherwise have won the \
         tie, which is exactly line 1851's `failover onto the same unhealthy upstream`"
    );
    assert_eq!(
        prevented[0].1.as_deref(),
        Some(format!("{FAILING}/{FAILOVER_MODEL}").as_str()),
        "the row must name what was displaced; `prevented` on its own says nothing a reader \
         could check: {prevented:?}"
    );

    // The same shape with no shared-upstream candidate at all: every
    // candidate scores identically in both rankings, so the term changed
    // nothing — and that is recorded rather than dropped.
    let untouched = preventions_after_a_failover(vec![
        upstream_backend(
            FAILING,
            "GLASSHOUSE_TEST_ONLY_FAILOVER_B1",
            stub_500_server(),
        ),
        upstream_backend(
            ELSEWHERE,
            "GLASSHOUSE_TEST_ONLY_FAILOVER_B2",
            stub_500_server(),
        ),
    ]);
    assert_eq!(
        untouched.len(),
        1,
        "one failover, one prevention row, whichever way it went: {untouched:?}"
    );
    assert!(
        !untouched[0].0,
        "with nothing sharing the failed provider the term cannot move a winner: {untouched:?}"
    );
    assert_eq!(
        untouched[0].1, None,
        "nothing was displaced, so nothing is named"
    );
}

/// **Acceptance 4, the reader half.** The count the sink above produces is
/// printed to a person with its denominator, and a window holding no failover
/// says so rather than printing a rate over nothing.
///
/// The rows are planted: their producer is the test above, which drives a real
/// gateway through the real door, and what is left to show is that
/// `glasshouse route` divides the right pair — which needs a project, and a
/// gateway failover cannot be made to happen inside one.
#[test]
fn the_failover_prevention_ratio_is_printed_with_its_denominator_and_never_over_nothing() {
    use glasshouse::evaluation::FailoverPrevention;

    let fixture = Fixture::new();

    // Premise: with nothing recorded, the section must not print a rate.
    let empty = fixture.glasshouse(&["route", "--moment", "session-start"]);
    assert!(empty.status.success(), "{}", both_streams(&empty));
    let empty = String::from_utf8_lossy(&empty.stdout).into_owned();
    assert!(
        empty.contains("no gateway failover was ranked in this window"),
        "a window with no failover must say so, never print `0 of 0`:\n{empty}"
    );

    // One prevented, two not — three failovers ranked, one steered.
    {
        let ledger = fixture.ledger();
        let now = now_unix();
        ledger
            .record(
                glasshouse::evaluation::NewObservation::new(EvaluationKind::FailoverPrevented)
                    .with_subject(FailoverPrevention::Prevented.as_str())
                    .with_detail("alpha/a-model"),
                now - 30,
            )
            .unwrap();
        for i in 0..2 {
            ledger
                .record(
                    glasshouse::evaluation::NewObservation::new(EvaluationKind::FailoverPrevented)
                        .with_subject(FailoverPrevention::NotPrevented.as_str()),
                    now - 20 + i,
                )
                .unwrap();
        }
    }

    let ran = fixture.glasshouse(&["route", "--moment", "session-start"]);
    assert!(ran.status.success(), "{}", both_streams(&ran));
    let stdout = String::from_utf8_lossy(&ran.stdout).into_owned();
    assert!(
        stdout.contains("1 of 3 gateway failovers"),
        "the ratio must carry the denominator it came from — `1 of 3`, never `33%`:\n{stdout}"
    );
}
