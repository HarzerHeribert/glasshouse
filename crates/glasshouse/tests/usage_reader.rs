//! The first thing in this build that counts tokens, driven through the
//! **built binary** against a model that really answers.
//!
//! # Why this spawns a process and stands up a socket
//!
//! `routing_observations` has carried `input_tokens`, `output_tokens` and
//! `cached_input_tokens` since migration 11, and until this package nothing
//! had ever written one — `crates/glasshouse/src/routing/evidence.rs`'s own
//! module header said so in production source, and
//! `crate::provider::resources` printed the consequence to the user:
//! *"Glasshouse does not count spend against this."*
//!
//! The producer is on the **disposable** path: Glasshouse builds the request
//! itself and already deserializes the whole reply document, so `usage` is a
//! sibling key of something already parsed. The relay path in
//! `crate::gateway::ingress` is untouched and stays byte-opaque by design.
//!
//! A unit test against a fake `ExtractionModel` would prove the wrong thing
//! here, for `memory_extract_triggers.rs`'s own reason: what has to be shown
//! is that `glasshouse hook`, spawned the way a harness spawns it, calls the
//! model the user configured and leaves a row in this project's ledger with
//! the counts that model reported. Practice §35 is the sharper form — a
//! caller every test bypasses is not a caller — and the production caller
//! here is `main.rs::run_extraction`, which only the binary reaches. So every
//! assertion below is made against the real process, the real config files, a
//! real socket, and the ledger's own public read path.
//!
//! The canned endpoint parses the request itself and writes its response by
//! hand rather than reusing anything in this crate, so "the counts arrived"
//! is a claim about the wire. It is adapted from the fixture in
//! `memory_extract_triggers.rs`, deliberately duplicated rather than shared:
//! that file's server answers a fixed document, and the whole subject here is
//! varying the `usage` half of it.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use glasshouse::config::{ExtractionModelRef, ProviderConfig, UserConfig};
use glasshouse::memory::extract::{ModelCall, TokenUsage};
use glasshouse::routing::evidence::{EvidenceLedger, ObservationQuery, RoutingObservation};
use glasshouse::session::{NewSession, ProjectSessions, SessionId, SessionLifecycle};
use glasshouse::{Cli, Runtime};

use clap::Parser;

const CREDENTIAL_VAR: &str = "GLASSHOUSE_TEST_ONLY_USAGE_READER_KEY";
const CREDENTIAL: &str = "sk-fabricated-test-value-not-a-real-credential";
const MODEL: &str = "a-cheap-local-model";
const PROVIDER: &str = "usage-test-runner";

/// The wire protocol slug this producer records as the route — the same
/// spelling `crate::gateway::session` uses for its own observations.
const ROUTE: &str = "openai-chat";

/// What a cheap model would answer, in the extraction contract's own shape.
const ONE_FINDING: &str = r#"{"memories":[{"kind":"finding","authority":"constraint",
     "disposition":"accepted","support":"established","confidence":"certain",
     "rationale":"the hook process is the only thing that sees a turn end",
     "project_phase":"alpha",
     "body":"A configured extraction model reported what its call cost."}]}"#;

// ---------------------------------------------------------------------------
// A canned OpenAI chat-completions endpoint whose `usage` the test chooses.
// ---------------------------------------------------------------------------

/// A model that answers [`ONE_FINDING`] with whatever `usage` value the test
/// asks for.
struct FakeModel {
    address: SocketAddr,
    stop: Arc<AtomicBool>,
}

impl FakeModel {
    /// `usage` is spliced into the reply document verbatim, or omitted
    /// entirely when [`None`] — so a test can send a shape no builder in this
    /// crate would produce, which is the point: providers disagree, and the
    /// reader has to survive whatever arrives.
    fn answering(usage: Option<serde_json::Value>) -> Self {
        let mut document = serde_json::json!({
            "choices": [{ "message": { "role": "assistant", "content": ONE_FINDING } }]
        });
        if let Some(usage) = usage {
            document["usage"] = usage;
        }
        Self::start(document.to_string())
    }

    fn start(document: String) -> Self {
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
                        serve(stream, &document);
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

/// Read one request head byte-oriented, find `content-length` without help,
/// read exactly that many bytes, and answer with `document`.
fn serve(mut stream: TcpStream, document: &str) {
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

    /// The configuration a person writes to point Glasshouse at a cheap or
    /// local model.
    fn choose_model(&self, base_url: &str) {
        let mut user = UserConfig::load(self.runtime.paths()).unwrap();
        let mut provider = ProviderConfig::new("openai-compatible");
        provider.set_base_url(Some(base_url.to_owned()));
        provider.set_credential_env(vec![CREDENTIAL_VAR.to_owned()]);
        user.providers_mut().set(PROVIDER, provider);
        user.set_memory_extraction_model(Some(ExtractionModelRef::new(PROVIDER, MODEL)));
        user.save(self.runtime.paths()).unwrap();
    }

    /// Run `glasshouse hook`, exactly as a harness runs it.
    fn hook(&self, session: &SessionId, event: &str) -> std::process::ExitStatus {
        let mut child = Command::new(env!("CARGO_BIN_EXE_glasshouse"))
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
        child
            .wait_with_output()
            .expect("the hook must finish")
            .status
    }

    /// Every observation this project's ledger holds for the configured
    /// model, through the ledger's own public read path.
    ///
    /// `route` and `harness` match exactly, including [`None`], so this asks
    /// for the identity the producer actually writes rather than for
    /// anything that happens to be in the table.
    fn observations(&self) -> Vec<RoutingObservation> {
        EvidenceLedger::open(&self.runtime)
            .expect("the project database is bound")
            .recent(
                ObservationQuery {
                    provider: PROVIDER,
                    model: MODEL,
                    route: Some(ROUTE),
                    harness: None,
                },
                16,
            )
            .expect("reading observations back")
    }

    /// The one observation this project's ledger holds, or a failure naming
    /// how many there really were.
    fn one_observation(&self) -> RoutingObservation {
        let mut rows = self.observations();
        assert_eq!(
            rows.len(),
            1,
            "one call to the model must record exactly one observation"
        );
        rows.remove(0)
    }
}

const PAYLOAD: &str = concat!(
    r#"{"session_id":"native-1","transcript_path":"/somewhere/rollout.jsonl","#,
    r#""hook_event_name":"Stop","cwd":"/somewhere","model":"a-model","#,
    r#""prompt":"a prompt that must never be stored","#,
    r#""last_assistant_message":"a reply that must never be stored"}"#
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

fn running_session(fixture: &Fixture) -> SessionId {
    let sessions = ProjectSessions::open(&fixture.runtime).unwrap();
    let store = sessions.store();
    let record = store.create(NewSession::embedded("claude-code")).unwrap();
    store
        .set_lifecycle(&record.id, SessionLifecycle::Running)
        .unwrap();
    record.id
}

/// Drive one whole extraction against a model reporting `usage`, and return
/// the row it left behind.
fn extraction_reporting(usage: Option<serde_json::Value>) -> RoutingObservation {
    let model = FakeModel::answering(usage);
    let fixture = Fixture::new();
    fixture.choose_model(&model.base_url());
    let id = running_session(&fixture);

    assert!(
        fixture.hook(&id, "Stop").success(),
        "a hook must exit zero whatever extraction did"
    );
    fixture.one_observation()
}

// ---------------------------------------------------------------------------
// 1. A reply carrying `usage` produces the counts, in the shipped binary.
// ---------------------------------------------------------------------------

/// **The producer, end to end.** A harness reports `Stop`; the binary asks the
/// model the user configured; the model answers with an OpenAI
/// chat-completions `usage` object; and the counts it reported are in this
/// project's `routing_observations` when the process is gone.
///
/// This fails on `main`, where nothing anywhere reads a provider's reported
/// token usage — `prompt_tokens|completion_tokens|total_tokens|cached_tokens`
/// has zero readers in `crates/glasshouse/src` — so every column asserted
/// below is `NULL` on every row that build could write.
#[test]
fn a_reply_carrying_usage_records_the_counts_the_provider_reported() {
    let observed = extraction_reporting(Some(serde_json::json!({
        "prompt_tokens": 1234,
        "completion_tokens": 56,
        "total_tokens": 1290,
        "prompt_tokens_details": { "cached_tokens": 1024 },
    })));

    assert_eq!(
        observed.input_tokens,
        Some(1234),
        "`usage.prompt_tokens` is the input count"
    );
    assert_eq!(
        observed.output_tokens,
        Some(56),
        "`usage.completion_tokens` is the output count"
    );
    assert_eq!(
        observed.cached_input_tokens,
        Some(1024),
        "`usage.prompt_tokens_details.cached_tokens` is the cached-input count"
    );

    // The identity the counts are attributed to, which is what makes them
    // readable later: the user's own names for the provider and the model,
    // and the wire protocol as the route.
    assert_eq!(observed.provider, PROVIDER);
    assert_eq!(observed.model, MODEL);
    assert_eq!(observed.route.as_deref(), Some(ROUTE));

    // Not claimed, and each for its own reason. A cost needs per-model
    // pricing this build does not have, and migration 11 `CHECK`s a cost
    // against a confidence label precisely so an unpriced count cannot
    // masquerade as a priced one.
    assert_eq!(
        observed.cost, None,
        "tokens are reported; a price would have to be invented"
    );
}

// ---------------------------------------------------------------------------
// 2. A reply with no `usage` records `None`, and never a zero.
// ---------------------------------------------------------------------------

/// **The honesty rule, and the most important test here.**
///
/// `usage` is optional and plenty of OpenAI-compatible endpoints omit it. A
/// zero recorded for a count nobody reported is worse than no row at all: it
/// is indistinguishable downstream from a call that genuinely used no input
/// tokens, and `routing_observations` makes these columns nullable for
/// exactly that reason. The capability map's own standing refusal is the
/// rule — *a fabricated value here does not degrade the policy, it inverts
/// it* — and this is the test that enforces it.
///
/// The row itself must still exist: the call happened, and *which resource
/// answered* is a real fact even when what it cost is not.
#[test]
fn a_reply_with_no_usage_records_nothing_rather_than_zero() {
    let observed = extraction_reporting(None);

    assert_eq!(
        observed.input_tokens, None,
        "a provider that reported no input count must not be recorded as having used none"
    );
    assert_eq!(
        observed.output_tokens, None,
        "a provider that reported no output count must not be recorded as having used none"
    );
    assert_eq!(
        observed.cached_input_tokens, None,
        "a provider that reported no cached count must not be recorded as having used none"
    );

    assert_eq!(
        observed.provider, PROVIDER,
        "the call still happened, and which resource answered is still a fact"
    );
}

// ---------------------------------------------------------------------------
// 3. A partial `usage` records the half that arrived.
// ---------------------------------------------------------------------------

/// Providers disagree field by field, not only document by document. One
/// count present and another absent must record the one and leave the other
/// unknown — neither dropping the whole object because it was incomplete, nor
/// filling the gap.
#[test]
fn a_partial_usage_records_what_arrived_and_leaves_the_rest_unknown() {
    let observed = extraction_reporting(Some(serde_json::json!({
        "prompt_tokens": 77,
    })));

    assert_eq!(
        observed.input_tokens,
        Some(77),
        "the count that arrived must be recorded"
    );
    assert_eq!(
        observed.output_tokens, None,
        "the count that did not arrive must stay unknown"
    );
    assert_eq!(
        observed.cached_input_tokens, None,
        "a `usage` with no `prompt_tokens_details` reported no cached count"
    );
}

/// A count that is not a non-negative integer is not a count.
///
/// `routing_observations` `CHECK`s all three columns `>= 0`, so a negative
/// passed through would turn a telemetry write into a failed one — and a
/// provider reporting a negative token count, a string, or a fraction has
/// told us nothing recordable either way. All of them are the same answer as
/// silence.
#[test]
fn a_count_that_is_not_a_non_negative_integer_is_recorded_as_unreported() {
    let observed = extraction_reporting(Some(serde_json::json!({
        "prompt_tokens": -5,
        "completion_tokens": "many",
        "prompt_tokens_details": { "cached_tokens": 12.5 },
    })));

    assert_eq!(
        observed.input_tokens, None,
        "a negative count is not a count"
    );
    assert_eq!(observed.output_tokens, None, "a string is not a count");
    assert_eq!(
        observed.cached_input_tokens, None,
        "a fractional count is not a count"
    );
}

// ---------------------------------------------------------------------------
// 4. Nothing that reached no provider records anything.
// ---------------------------------------------------------------------------

/// The discriminating half, and the guard practice §65 asks for.
///
/// A user who has configured no extraction model gets `RoutedModel`, which
/// chooses a resource and calls nothing. There is no call, so there is no
/// usage, so there must be no row — and, because
/// `ExtractionOutcome::observation` is what gates it, no
/// `EvidenceLedger::open` either. A ledger that recorded a routing *decision*
/// as though it were a measured *call* would put a fabricated turn in a table
/// whose whole purpose is to be trusted.
///
/// Without this test, "extraction records an observation" would be satisfied
/// by "extraction records an observation whether or not anything happened".
#[test]
fn a_run_that_called_no_model_records_no_observation() {
    let model = FakeModel::answering(Some(serde_json::json!({ "prompt_tokens": 9 })));
    let fixture = Fixture::new();
    // Deliberately not `choose_model`: the socket above stands so that this
    // can assert nothing reached it.
    let id = running_session(&fixture);

    assert!(fixture.hook(&id, "Stop").success());

    assert!(
        fixture.observations().is_empty(),
        "a routing decision that called nothing is not a measured call"
    );
    drop(model);
}

// ---------------------------------------------------------------------------
// 5. The mapping into the ledger, without a socket.
// ---------------------------------------------------------------------------

/// A call with nothing reported maps to an observation whose every token
/// column is absent — the same claim as the end-to-end test above, made
/// against the mapping itself so that a failure separates "the reader lost
/// the counts" from "the mapping lost them".
#[test]
fn a_call_reporting_nothing_maps_to_an_observation_with_no_counts() {
    let observation = ModelCall {
        provider: "p".to_owned(),
        model: "m".to_owned(),
        route: Some(ROUTE.to_owned()),
        credential_label: None,
        usage: TokenUsage::UNREPORTED,
        dispatched_at_unix: None,
        completed_at_unix: None,
    }
    .observation();

    assert_eq!(observation.input_tokens, None);
    assert_eq!(observation.output_tokens, None);
    assert_eq!(observation.cached_input_tokens, None);
    assert_eq!(observation.provider, "p");
    assert_eq!(observation.model, "m");
    assert_eq!(observation.route.as_deref(), Some(ROUTE));

    // The columns this producer will not fill with the nearest available
    // number. Asserted rather than assumed, because each of them is a column
    // somebody could plausibly reach for later without noticing it would be a
    // guess.
    assert_eq!(observation.cost, None);
    assert_eq!(observation.dispatched_at_unix, None);
    assert_eq!(observation.completed_at_unix, None);
    assert_eq!(observation.outcome, None);
    assert_eq!(observation.purpose, None);
    assert_eq!(observation.tool_rounds, None);
    // `quota_context` carries the credential *label* when the call was routed
    // to a named credential, and nothing at all when it was not — never a
    // provider name standing in for an account.
    assert_eq!(observation.quota_context, None);
}

/// The counts survive the mapping unchanged, in the order the fields are
/// declared.
///
/// Three same-typed `Option<i64>` arguments in a row is exactly the shape a
/// transposition hides in, so the three values here are distinct and the
/// assertion names which is which.
#[test]
fn the_three_counts_reach_the_three_columns_they_belong_in() {
    let observation = ModelCall {
        provider: "p".to_owned(),
        model: "m".to_owned(),
        route: None,
        credential_label: None,
        usage: TokenUsage {
            input_tokens: Some(11),
            output_tokens: Some(22),
            cached_input_tokens: Some(33),
        },
        dispatched_at_unix: None,
        completed_at_unix: None,
    }
    .observation();

    assert_eq!(observation.input_tokens, Some(11), "input");
    assert_eq!(observation.output_tokens, Some(22), "output");
    assert_eq!(observation.cached_input_tokens, Some(33), "cached input");
    assert_eq!(
        observation.route, None,
        "a call with no route recorded must not acquire one"
    );
}
