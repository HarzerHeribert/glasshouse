//! Capability map line 2039: `glasshouse routing-cost`'s `EFFORT SHADOW`
//! section — the shadow measurement `docs/product/design-decisions.md`'s
//! *Carrying effort across a translated pairing* asks for before any clamp
//! is offered (*"Then the measurement, then the clamp"*).
//!
//! (a)-(e) plant rows through `EvidenceLedger::record` and
//! `EvaluationObservations::record` in-process, the same shape
//! `tests/savings_readout.rs` and `tests/routing_session_column.rs`(b)/(c)
//! already use: the *producer* of a translated exchange's row is proven in
//! `tests/gateway_translate_evidence.rs` and `tests/routing_session_column.rs`,
//! and the producer of a `TurnOutcomeObserved` row is proven in
//! `tests/evaluation_producers.rs` — this file tests the reader,
//! `EvidenceLedger::effort_shadow`, and the renderer,
//! `main.rs::render_effort_shadow_section`.
//!
//! (f) is the one exception (practice §35: a caller every test bypasses is
//! not a caller): it goes through a real `glasshouse launch` and a real
//! `glasshouse hook --event Stop`, copying the launch fixture from
//! `tests/routing_session_column.rs` rather than sharing it — every
//! integration test binary in this crate carries its own copy.

use std::io::{Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use clap::Parser;
use glasshouse::Runtime;
use glasshouse::evaluation::{
    EvaluationKind, EvaluationObservations, NewObservation as EvaluationNewObservation,
};
use glasshouse::routing::evidence::{
    EffortLevel, EvidenceLedger, HARNESS_TURN_PURPOSE, MIN_SAMPLE_FOR_SUMMARY, NewObservation,
    Outcome, TurnShape,
};
use serde_json::json;

// ===========================================================================
// Fixture — same shape as `tests/savings_readout.rs`'s own, duplicated
// rather than shared.
// ===========================================================================

struct Fixture {
    base: PathBuf,
    root: PathBuf,
    runtime: Runtime,
}

impl Fixture {
    fn new(base: &Path) -> Self {
        let root = base.join("workspace");
        std::fs::create_dir_all(root.join(".git")).unwrap();
        let root = std::fs::canonicalize(&root).unwrap();
        std::fs::create_dir_all(base.join("config")).unwrap();

        let cli = glasshouse::Cli::try_parse_from([
            "glasshouse",
            "--data-dir",
            base.join("data").to_str().unwrap(),
            "--config-dir",
            base.join("config").to_str().unwrap(),
        ])
        .unwrap();
        let runtime = glasshouse::bootstrap(&cli, &root).unwrap();
        Fixture {
            base: base.to_path_buf(),
            root,
            runtime,
        }
    }

    fn run(&self, args: &[&str], stdin_bytes: &[u8]) -> Output {
        let mut child = Command::new(env!("CARGO_BIN_EXE_glasshouse"))
            .arg("--scope")
            .arg(&self.root)
            .arg("--data-dir")
            .arg(self.base.join("data"))
            .arg("--config-dir")
            .arg(self.base.join("config"))
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn glasshouse");
        child
            .stdin
            .take()
            .unwrap()
            .write_all(stdin_bytes)
            .expect("write stdin");
        child.wait_with_output().expect("wait for glasshouse")
    }

    fn routing_cost(&self) -> String {
        let output = self.run(&["routing-cost"], b"");
        assert!(
            output.status.success(),
            "routing-cost must succeed: stderr={}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).into_owned()
    }

    /// Plant one translated-exchange row through the real ledger API — every
    /// column a producer might set, so an absent one becomes `NULL` exactly
    /// as `NewObservation`'s own doc comments require.
    #[allow(clippy::too_many_arguments)]
    fn plant_routing_row(
        &self,
        session_id: Option<&str>,
        effort_level: Option<EffortLevel>,
        turn_shape: Option<TurnShape>,
        output_tokens: i64,
        outcome: Outcome,
        observed_at_unix: i64,
    ) {
        let observation = NewObservation::new("fixture", "fixture-model")
            .with_harness(Some("claude-code"))
            .with_purpose(Some(HARNESS_TURN_PURPOSE))
            .with_route(Some("anthropic-messages->openai-chat"))
            .with_quota_context(Some("cred-a"))
            .with_session_id(session_id)
            .with_effort_level(effort_level)
            .with_turn_shape(turn_shape)
            .with_tokens(Some(50), Some(output_tokens), Some(0))
            .with_outcome(outcome);
        let ledger = EvidenceLedger::open(&self.runtime).unwrap();
        ledger.record(observation, observed_at_unix).unwrap();
    }

    /// Plant one `TurnOutcomeObserved` row — the same shape
    /// `evaluation::record_turn_outcome` writes.
    fn plant_verdict(&self, session_id: &str, subject: &str, observed_at_unix: i64) {
        let ledger = EvaluationObservations::open(&self.runtime).unwrap();
        ledger
            .record(
                EvaluationNewObservation::new(EvaluationKind::TurnOutcomeObserved)
                    .with_subject(subject)
                    .with_session_id(session_id),
                observed_at_unix,
            )
            .unwrap();
    }
}

fn now() -> i64 {
    glasshouse::provider::cache::now_unix_seconds()
}

/// The `EFFORT SHADOW` section: everything from its own header to the end of
/// the report, since `render_routing_cost` prints it last.
fn effort_shadow_section(report: &str) -> String {
    let marker = "\nEFFORT SHADOW\n";
    let start = report
        .find(marker)
        .unwrap_or_else(|| panic!("no EFFORT SHADOW section in:\n{report}"));
    report[start..].to_owned()
}

// ===========================================================================
// (a) Two turn-shape groups and a third effort-level group, with the right
//     median (once the sample floor is met) and the right verdict counts.
// ===========================================================================

#[test]
fn groups_by_turn_shape_and_effort_show_the_right_median_and_verdict_counts() {
    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path());
    let at = now() - 60;

    // tool-resume / high: five exchanges, five sessions, so the median is a
    // real digit (300) rather than words for the sample floor. Verdicts:
    // three completed, one failed, one session with no verdict row at all.
    let high_tokens = [100_i64, 200, 300, 400, 500];
    let high_sessions = ["ses-a1", "ses-a2", "ses-a3", "ses-a4", "ses-a5"];
    for (session, tokens) in high_sessions.iter().zip(high_tokens.iter()) {
        fixture.plant_routing_row(
            Some(session),
            Some(EffortLevel::High),
            Some(TurnShape::ToolResume),
            *tokens,
            Outcome::Succeeded,
            at,
        );
    }
    fixture.plant_verdict("ses-a1", "completed", at + 5);
    fixture.plant_verdict("ses-a2", "completed", at + 5);
    fixture.plant_verdict("ses-a3", "completed", at + 5);
    fixture.plant_verdict("ses-a4", "failed", at + 5);
    // ses-a5 gets no verdict row at all.

    // tool-resume / low: one exchange, below the sample floor.
    fixture.plant_routing_row(
        Some("ses-b"),
        Some(EffortLevel::Low),
        Some(TurnShape::ToolResume),
        80,
        Outcome::Succeeded,
        at,
    );
    fixture.plant_verdict("ses-b", "completed", at + 5);

    // prompt / high: one exchange, a different turn shape than the group
    // above that also carries `high` — proving the grouping key is the pair,
    // not either field alone.
    fixture.plant_routing_row(
        Some("ses-c"),
        Some(EffortLevel::High),
        Some(TurnShape::Prompt),
        999,
        Outcome::Succeeded,
        at,
    );
    fixture.plant_verdict("ses-c", "failed", at + 5);

    let report = fixture.routing_cost();
    let section = effort_shadow_section(&report);

    assert!(
        section.contains("\n  tool-resume / high\n"),
        "missing the tool-resume/high group:\n{section}"
    );
    assert!(
        section.contains("5 exchanges, median output tokens 300"),
        "expected the real median of [100,200,300,400,500]:\n{section}"
    );
    assert!(
        section.contains("verdicts: 3 completed, 1 failed, 1 unverdicted"),
        "expected the tool-resume/high verdict counts:\n{section}"
    );

    assert!(
        section.contains("\n  tool-resume / low\n"),
        "missing the tool-resume/low group:\n{section}"
    );
    assert!(
        section.contains(&format!(
            "1 exchanges, median output tokens below the sample floor (1 of {MIN_SAMPLE_FOR_SUMMARY} \
             exchanges needed)"
        )),
        "expected the below-floor words for a single-sample group:\n{section}"
    );
    assert!(
        section.contains("verdicts: 1 completed, 0 failed, 0 unverdicted"),
        "expected the tool-resume/low verdict counts:\n{section}"
    );

    assert!(
        section.contains("\n  prompt / high\n"),
        "missing the prompt/high group, distinct from tool-resume/high:\n{section}"
    );
    assert!(
        section.contains("verdicts: 0 completed, 1 failed, 0 unverdicted"),
        "expected the prompt/high verdict counts:\n{section}"
    );
}

// ===========================================================================
// (b) A row with no recorded turn shape is unread, never a prompt turn.
// ===========================================================================

#[test]
fn a_row_with_no_turn_shape_is_counted_as_unread_and_joins_no_group() {
    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path());
    let at = now() - 10;

    fixture.plant_routing_row(
        Some("ses-null-shape"),
        None,
        None,
        123,
        Outcome::Succeeded,
        at,
    );

    let report = fixture.routing_cost();
    let section = effort_shadow_section(&report);

    assert!(
        section.contains("\n  no translated exchanges recorded in this window\n"),
        "the one planted row carries no turn shape, so no group may appear:\n{section}"
    );
    assert!(
        !section.contains("prompt /") && !section.contains("tool-resume /"),
        "a row with no turn shape must never be folded into a group:\n{section}"
    );
    assert!(
        section.contains(
            "unread: 1 (rows with no recorded turn shape — relayed, or written before the \
             column existed)"
        ),
        "expected the unread count and its words:\n{section}"
    );
}

// ===========================================================================
// (c) A served 2xx row whose session's next verdict failed is counted as
//     failed — never read from `routing_observations.outcome`.
// ===========================================================================

#[test]
fn a_served_row_whose_session_later_failed_is_counted_as_failed() {
    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path());
    let at = now() - 10;

    fixture.plant_routing_row(
        Some("ses-served-then-failed"),
        Some(EffortLevel::Minimal),
        Some(TurnShape::ToolResume),
        50,
        // The exchange's own transport outcome is a served 2xx — succeeded —
        // and must never be read as the verdict.
        Outcome::Succeeded,
        at,
    );
    fixture.plant_verdict("ses-served-then-failed", "failed", at + 5);

    let report = fixture.routing_cost();
    let section = effort_shadow_section(&report);

    assert!(
        section.contains("\n  tool-resume / minimal\n"),
        "missing the tool-resume/minimal group:\n{section}"
    );
    assert!(
        section.contains("verdicts: 0 completed, 1 failed, 0 unverdicted"),
        "a served (2xx) exchange whose session's next verdict failed must count as failed, \
         never as completed from its own transport outcome:\n{section}"
    );
}

// ===========================================================================
// (d) A verdict row before the exchange, and none after, is unverdicted.
// ===========================================================================

#[test]
fn a_verdict_recorded_before_the_exchange_does_not_count_and_the_exchange_is_unverdicted() {
    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path());
    let at = now() - 10;

    // The verdict is written first, well before the exchange it must not be
    // allowed to answer for.
    fixture.plant_verdict("ses-stale-verdict", "completed", at - 100);
    fixture.plant_routing_row(
        Some("ses-stale-verdict"),
        Some(EffortLevel::High),
        Some(TurnShape::ToolResume),
        77,
        Outcome::Succeeded,
        at,
    );

    let report = fixture.routing_cost();
    let section = effort_shadow_section(&report);

    assert!(
        section.contains("\n  tool-resume / high\n"),
        "missing the tool-resume/high group:\n{section}"
    );
    assert!(
        section.contains("verdicts: 0 completed, 0 failed, 1 unverdicted"),
        "a verdict recorded before the exchange must not be read as its answer:\n{section}"
    );
}

// ===========================================================================
// (e) An empty window prints the words, never a fabricated group.
// ===========================================================================

#[test]
fn an_empty_window_prints_the_words_and_a_real_unread_count() {
    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path());

    let report = fixture.routing_cost();
    let section = effort_shadow_section(&report);

    assert!(
        section.contains("\n  no translated exchanges recorded in this window\n"),
        "{section}"
    );
    assert!(
        section.contains(
            "unread: 0 (rows with no recorded turn shape — relayed, or written before the \
             column existed)"
        ),
        "an unread count of zero is a real, counted zero, not a fabrication:\n{section}"
    );
    assert!(
        section
            .contains("a clamp is not offered; this section is the evidence for or against one."),
        "the closing sentence must be present even on an empty window:\n{section}"
    );
}

// ===========================================================================
// (f) End to end: a launch through a translated fixture, a real
//     `glasshouse hook --event Stop`, then `routing-cost`.
// ===========================================================================

const CHAT_KEY: &str = "sk-planted-effort-shadow-000111";
const CHAT_CREDENTIAL_VAR: &str = "GLASSHOUSE_EFFORT_SHADOW_CHAT_TEST_KEY";
const CLIENT_TIMEOUT: Duration = Duration::from_secs(20);

static ENV_LOCK: Mutex<()> = Mutex::new(());

fn env_lock() -> std::sync::MutexGuard<'static, ()> {
    ENV_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// A turn whose last user message is nothing but tool results — migration
/// 24's `turn_shape` calls it *tool-resume*. Copied from
/// `tests/routing_session_column.rs`.
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
        "stream": false
    })
    .to_string()
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
        "usage": {
            "prompt_tokens": 40,
            "completion_tokens": 12,
            "total_tokens": 52
        }
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

fn read_request(stream: &mut TcpStream) -> Option<Vec<u8>> {
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
    Some(body)
}

fn write_document(stream: &mut TcpStream, body: &str) {
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
        body.len()
    );
    let _ = stream.write_all(response.as_bytes());
    let _ = stream.flush();
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

const LAUNCH_ENV_DUMP_VAR: &str = "GLASSHOUSE_EFFORT_SHADOW_LAUNCH_ENV_DUMP";
const LAUNCH_STOP_VAR: &str = "GLASSHOUSE_EFFORT_SHADOW_LAUNCH_STOP";
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

/// A real `glasshouse launch claude-code --profile gateway-chat --headless`
/// against a chat-only fixture, held open so its gateway keeps serving.
/// Copied from `tests/routing_session_column.rs::LaunchedSession`.
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
        let _guard = env_lock();
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
        drop(_guard);

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

    fn session_id(&self) -> glasshouse::session::SessionId {
        let runtime = self.runtime();
        let sessions =
            glasshouse::session::ProjectSessions::open(&runtime).expect("open the session store");
        let records = sessions.store().list().expect("list sessions");
        assert_eq!(
            records.len(),
            1,
            "the launch records exactly one session: {records:?}"
        );
        records[0].id.clone()
    }

    /// A real `glasshouse hook --session <id> --event Stop`, run as its own
    /// process exactly as a harness runs it — `main.rs::report_hook_with`
    /// never reads its stdin, so an empty payload is enough.
    fn hook_stop(&self, session: &str) {
        let output = Command::new(env!("CARGO_BIN_EXE_glasshouse"))
            .arg("--scope")
            .arg(&self.root)
            .arg("--data-dir")
            .arg(self.base.join("data"))
            .arg("--config-dir")
            .arg(self.base.join("config"))
            .args(["hook", "--session", session, "--event", "Stop"])
            .output()
            .expect("run the hook");
        assert!(
            output.status.success(),
            "a hook always exits zero: stderr={}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

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

/// A session `glasshouse launch` created, sent one `thinking` tool-resume
/// request over a translated pairing, then told `glasshouse hook --event
/// Stop` that its turn ended: `routing-cost` must show one tool-resume row
/// at the mapped effort level with a completed verdict.
#[test]
fn a_launched_and_hooked_session_shows_one_tool_resume_row_with_a_completed_verdict() {
    let fixture = FixtureUpstream::answering(chat_completion_answer());
    let session = LaunchedSession::start(&fixture);

    // 16,000 maps to `medium` — `tests/routing_session_column.rs`'s own
    // waypoint, neither end of the ladder.
    session.send(&tool_resume_body(16_000));

    let session_id = session.session_id();
    session.hook_stop(session_id.as_str());

    let report = session.routing_cost();
    let section = effort_shadow_section(&report);

    assert!(
        section.contains("\n  tool-resume / medium\n"),
        "expected the launched exchange's own group:\n{section}"
    );
    assert!(
        section.contains(&format!(
            "1 exchanges, median output tokens below the sample floor (1 of {MIN_SAMPLE_FOR_SUMMARY} \
             exchanges needed)"
        )),
        "one exchange is below the sample floor:\n{section}"
    );
    assert!(
        section.contains("verdicts: 1 completed, 0 failed, 0 unverdicted"),
        "the hook's Stop event must record a completed verdict for this session:\n{section}"
    );

    session.stop();
}
