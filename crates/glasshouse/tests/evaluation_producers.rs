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
use glasshouse::routing::evidence::{EXTRACTION_PURPOSE, EvidenceLedger, NewObservation};
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

    /// Where the gateway's own caches live: `RuntimePaths::resolve` derives
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

    fn db(&self) -> Connection {
        Connection::open(self.runtime.database_path()).unwrap()
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

/// Read the request whole — its head, then exactly the body its
/// `Content-Length` declares — before the caller answers it.
///
/// This used to be a single `read` into a 4 KiB buffer, on the reasoning
/// that a stub which never parses the request need not read it. That is a
/// race with any client that writes its head and its body separately, and
/// the gateway is one: `ureq` sends the head, then streams the relayed body
/// from the client socket (`gateway::ingress`'s
/// `SendBody::from_owned_reader`). When the stub's single read lands
/// between those two writes it takes the head alone, and the body is still
/// in the socket's receive queue when the stub answers and drops the
/// stream.
///
/// Closing a socket that still holds unread data is an *abortive* close:
/// the stack sends RST instead of FIN. Winsock then discards whatever it
/// had already buffered for the peer, so the gateway's read of the response
/// this stub had just written failed with a connection reset, `agent.run`
/// returned `Err`, and the gateway answered its own `502 Bad Gateway`
/// (`ingress::serve`'s `Outcome::Unreachable`) rather than relaying the
/// scripted status. Unix hands the buffered bytes back first and only
/// reports the reset once they are drained, which is why the same stub was
/// reliable on macOS and Linux and flaked on the Windows ARM64 CI VM.
///
/// Nothing here is conditional on the platform, and no assertion moves:
/// reading a request before answering it is what any HTTP server does, and
/// it is already what `evaluation_producers.rs`'s `serve_json` does in this
/// same suite. On Unix it only reads bytes that were arriving anyway.
fn read_whole_request(stream: &mut TcpStream) {
    let mut reader = BufReader::new(stream);
    let mut declared = 0usize;
    let mut line = String::new();
    loop {
        line.clear();
        match reader.read_line(&mut line) {
            Ok(0) | Err(_) => return,
            Ok(_) => {}
        }
        let line = line.trim_end_matches(['\r', '\n']);
        if line.is_empty() {
            break;
        }
        if let Some((name, value)) = line.split_once(':')
            && name.trim().eq_ignore_ascii_case("content-length")
        {
            declared = value.trim().parse().unwrap_or(0);
        }
    }
    let mut body = vec![0u8; declared];
    let _ = reader.read_exact(&mut body);
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
            read_whole_request(stream);
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
        &[profile.backend_demand()],
        || Ok(upstream),
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
        &gateway
            .upstream()
            .expect("a started gateway has its upstream"),
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
