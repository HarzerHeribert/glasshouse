//! **`glasshouse classify` dispatches a real model call** — the caller
//! `routing::classify::classify`'s `Some(..)` arm was written for and, until
//! this batch, never had.
//!
//! # Why every test here runs the shipped binary
//!
//! `classify(text, Some(..))` has been callable since Phase 35 and its only
//! production caller passed `None`. A test that constructs a
//! `TaskClassification` by hand and checks `classify` returns it would have
//! passed on every build in this repository's history, including the ones
//! where nothing could call a model at all — practice §35's *"a caller you
//! can delete without a test noticing is, to the test suite, not a caller"*,
//! in the phase where the caller is what is being built.
//!
//! So every test below runs `glasshouse classify` as a process, against a
//! canned OpenAI chat-completions endpoint on loopback, with the same
//! configuration files a person would write. No seam, no fake
//! `ExtractionModel`, no provider and no credential.
//!
//! The endpoint parses the request itself rather than reusing anything in this
//! crate, so *"the request arrived, naming this model"* is a claim about the
//! wire.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use glasshouse::config::{
    ExtractionModelRef, FreeResourceRef, ProviderConfig, RoutingModelChoice, UserConfig,
};
use glasshouse::routing::evidence::{EvidenceLedger, ObservationQuery};
use glasshouse::session::{NewSession, ProjectSessions, SessionId, SessionLifecycle};
use glasshouse::{Cli, Runtime};

use clap::Parser;

/// The variable every fixture provider's credential is read from.
///
/// A credential is not optional here even for a loopback runner:
/// `disposable_candidates` only builds a candidate for a provider whose
/// credential actually resolves, so an `Automatic` test with no credential
/// would be testing an empty candidate list rather than a routing decision.
const CREDENTIAL_VAR: &str = "GLASSHOUSE_TEST_ONLY_ROUTING_MODEL_KEY";
const CREDENTIAL: &str = "sk-fabricated-test-value-not-a-real-credential";

/// The route slug every observation this file asserts on is recorded under —
/// `WireProtocol::OpenAiChat`, the only protocol `ConfiguredModel` speaks.
const ROUTE: &str = "openai-chat";

/// A classification no run of `classify_heuristically` can produce for the
/// request below.
///
/// `workload_tier` is `frontier`: the heuristic assigns exactly three tiers
/// (`heavy`, `standard`, `leaf`) and `routing/classify.rs` says in its own
/// doc comment that `Frontier` has no producer. So "the report says frontier"
/// is a fact only a model answer can explain — which is what makes this test
/// fail on a build whose caller passes `None`.
const MODEL_ANSWER: &str = r#"{
  "needs_repo_context": true,
  "needs_code_modification": true,
  "needs_shell_execution": true,
  "needs_browser_interaction": true,
  "complexity": "complex",
  "likely_multi_turn": true,
  "workload_tier": "frontier",
  "safe_for_disposable_model": false,
  "warm_context": "prefer_warm",
  "confidence": "high"
}"#;

/// A request whose heuristic classification is as far from [`MODEL_ANSWER`]
/// as this file can make it: a pure question with no repository reference is
/// the one case `classify_heuristically` treats as needing nothing at all.
const QUESTION: &str = "what is a monad";

/// What a cheap model would answer for memory extraction — the *other*
/// producer that writes to `routing_observations`, in its own schema.
const EXTRACTION_ANSWER: &str = r#"{"memories":[{"kind":"finding","authority":"constraint",
     "disposition":"accepted","support":"established","confidence":"certain",
     "rationale":"the hook process is the only thing that sees a turn end",
     "project_phase":"alpha",
     "body":"A configured extraction model answered over loopback."}]}"#;

// ---------------------------------------------------------------------------
// A canned OpenAI chat-completions endpoint.
// ---------------------------------------------------------------------------

/// One request as it actually arrived on the wire.
#[derive(Debug, Clone)]
struct Seen {
    method: String,
    target: String,
    headers: Vec<(String, String)>,
    body: String,
}

impl Seen {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(header, _)| header == name)
            .map(|(_, value)| value.as_str())
    }
}

/// What the endpoint does with one request.
enum Answer {
    /// Answer `200` with this as the assistant message.
    Content(String),
    /// Answer `500`, the shape of a provider that is reachable and unwell.
    ServerError,
}

/// A model that decides its answer from the request body, and remembers every
/// request it was sent.
///
/// The answer is a function of the body rather than a fixed string because
/// one test needs the *same* endpoint to serve two different jobs —
/// classification and memory extraction — and to answer each in its own
/// schema. That is also how it proves which model was asked: the body names
/// the model, and the endpoint answers only for the one it recognises.
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

    fn failing() -> Self {
        Self::start(|_| Answer::ServerError)
    }

    fn start(responder: impl Fn(&str) -> Answer + Send + Sync + 'static) -> Self {
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

/// Read one request head byte-oriented, find `content-length` without help,
/// read exactly that many bytes, and answer.
fn serve(
    mut stream: TcpStream,
    seen: &Arc<Mutex<Vec<Seen>>>,
    responder: &(impl Fn(&str) -> Answer + ?Sized),
) {
    let mut reader = BufReader::new(stream.try_clone().expect("the stream clones"));
    let mut request_line = String::new();
    if reader.read_line(&mut request_line).is_err() || request_line.is_empty() {
        return;
    }
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or_default().to_owned();
    let target = parts.next().unwrap_or_default().to_owned();

    let mut headers = Vec::new();
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
        if let Some((name, value)) = line.split_once(':') {
            let name = name.trim().to_ascii_lowercase();
            let value = value.trim().to_owned();
            if name == "content-length" {
                length = value.parse().unwrap_or(0);
            }
            headers.push((name, value));
        }
    }

    let mut body = vec![0u8; length];
    if reader.read_exact(&mut body).is_err() {
        return;
    }
    let body = String::from_utf8_lossy(&body).into_owned();
    seen.lock().unwrap().push(Seen {
        method,
        target,
        headers,
        body: body.clone(),
    });

    let response = match responder(&body) {
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
// A project, and the binary run against it.
// ---------------------------------------------------------------------------

struct Fixture {
    _tmp: tempfile::TempDir,
    base: PathBuf,
    root: PathBuf,
    runtime: Runtime,
}

/// What one `glasshouse classify` run printed.
struct Classified {
    stdout: String,
    stderr: String,
    status: std::process::ExitStatus,
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

    fn config(&self) -> UserConfig {
        UserConfig::load(self.runtime.paths()).unwrap()
    }

    fn save(&self, user: UserConfig) {
        user.save(self.runtime.paths()).unwrap();
    }

    /// Add one provider that speaks OpenAI chat completions at `base_url`,
    /// naming `model` as a free model it may be routed to.
    fn add_provider(&self, name: &str, model: &str, base_url: &str) {
        let mut user = self.config();
        let mut provider = ProviderConfig::new("openai-compatible");
        provider.set_base_url(Some(base_url.to_owned()));
        provider.set_credential_env(vec![CREDENTIAL_VAR.to_owned()]);
        provider.set_free_models(vec![model.to_owned()]);
        user.providers_mut().set(name, provider);
        self.save(user);
    }

    /// `routing.model = pinned`, the configuration a person writes to name
    /// exactly which model classifies.
    fn pin_routing_model(&self, provider: &str, model: &str) {
        let mut user = self.config();
        user.routing_mut()
            .set_model(Some(RoutingModelChoice::Pinned {
                provider: provider.to_owned(),
                model: model.to_owned(),
            }));
        self.save(user);
    }

    /// `routing.model = automatic`, the configuration that hands the choice
    /// to `DisposableRouting`.
    fn automatic_routing_model(&self) {
        let mut user = self.config();
        user.routing_mut()
            .set_model(Some(RoutingModelChoice::Automatic));
        self.save(user);
    }

    /// The user's own free-resource order — the input `DisposableRouting`
    /// arranges its free candidates by, and the one this file uses to make
    /// the routed answer differ from the first candidate.
    fn prefer_free_resource(&self, provider: &str, model: &str) {
        let mut user = self.config();
        user.routing_mut()
            .set_free_resource_order(Some(vec![FreeResourceRef::new(provider, model)]));
        self.save(user);
    }

    fn choose_extraction_model(&self, provider: &str, model: &str) {
        let mut user = self.config();
        user.set_memory_extraction_model(Some(ExtractionModelRef::new(provider, model)));
        self.save(user);
    }

    /// Run `glasshouse classify <text>`, exactly as a person runs it.
    fn classify(&self, text: &str) -> Classified {
        let output = Command::new(env!("CARGO_BIN_EXE_glasshouse"))
            .env(CREDENTIAL_VAR, CREDENTIAL)
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
        Classified {
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
            status: output.status,
        }
    }

    /// Run `glasshouse hook`, exactly as a harness runs it — the *other*
    /// producer of `routing_observations` rows.
    fn hook(&self, session: &SessionId) -> std::process::ExitStatus {
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
            .arg("Stop")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .expect("the glasshouse binary must be runnable");
        child
            .stdin
            .as_mut()
            .expect("stdin was piped")
            .write_all(HOOK_PAYLOAD.as_bytes())
            .expect("the handler must read its payload rather than closing the pipe");
        child
            .wait_with_output()
            .expect("the hook must finish")
            .status
    }

    /// Every observation recorded for one `(provider, model)` identity.
    fn observations(
        &self,
        provider: &str,
        model: &str,
    ) -> Vec<glasshouse::routing::evidence::RoutingObservation> {
        EvidenceLedger::open(&self.runtime)
            .unwrap()
            .recent(
                ObservationQuery {
                    provider,
                    model,
                    route: Some(ROUTE),
                    harness: None,
                },
                16,
            )
            .unwrap()
    }
}

const HOOK_PAYLOAD: &str = concat!(
    r#"{"session_id":"native-1","transcript_path":"/somewhere/rollout.jsonl","#,
    r#""hook_event_name":"Stop","cwd":"/somewhere","model":"a-model","#,
    r#""prompt":"a question about the build","#,
    r#""last_assistant_message":"an answer about the build"}"#
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

// ---------------------------------------------------------------------------
// 1. A model reply becomes the classification.
// ---------------------------------------------------------------------------

/// **The joined link.** `glasshouse classify` asks the model the user
/// configured, over the wire, and prints *that model's* answer.
///
/// Fails on a build whose only production caller passes `None`: every
/// assertion below is about a value `classify_heuristically` cannot produce
/// for this request — `frontier` has no heuristic producer at all, and a pure
/// question with no repository reference is the one case the heuristic
/// classifies as needing nothing.
#[test]
fn a_configured_routing_model_answers_and_the_report_says_a_model_did() {
    let model = FakeModel::answering(MODEL_ANSWER);
    let fixture = Fixture::new();
    fixture.add_provider("alpha-runner", "alpha-model", &model.base_url());
    fixture.pin_routing_model("alpha-runner", "alpha-model");

    let run = fixture.classify(QUESTION);
    assert!(run.status.success(), "stderr: {}", run.stderr);

    let requests = model.requests();
    assert_eq!(
        requests.len(),
        1,
        "one classification is one model call, no more and no fewer"
    );
    let asked = &requests[0];
    assert_eq!(asked.method, "POST");
    assert_eq!(asked.target, "/v1/chat/completions");
    assert_eq!(
        asked.header("authorization"),
        Some(format!("Bearer {CREDENTIAL}").as_str()),
        "the credential the user's provider names must be what authenticates the call"
    );
    assert!(
        asked.body.contains("alpha-model"),
        "the request must name the model the user pinned: {}",
        asked.body
    );
    assert!(
        asked.body.contains(QUESTION),
        "the request the user typed is what the model is asked to classify: {}",
        asked.body
    );

    assert!(
        run.stdout
            .contains("source                  model (alpha-runner/alpha-model via openai-chat)"),
        "the report must attribute the classification to the model that answered:\n{}",
        run.stdout
    );
    assert!(
        run.stdout.contains("workload tier           frontier"),
        "`frontier` has no heuristic producer, so only the model's own answer explains it:\n{}",
        run.stdout
    );
    assert!(
        run.stdout.contains("complexity              complex"),
        "{}",
        run.stdout
    );
    assert!(
        run.stdout.contains(
            "hard capabilities       repository access, shell execution, browser interaction"
        ),
        "the derived capabilities must follow the model's signals:\n{}",
        run.stdout
    );
}

// ---------------------------------------------------------------------------
// 2. `Automatic` goes through `DisposableRouting::choose`.
// ---------------------------------------------------------------------------

/// **The constraint this package turns on.** On `Automatic`, the model comes
/// from `DisposableRouting::choose` — the only production call site of the
/// protected-reserve gate — and not from anything reached around it.
///
/// Two providers are configured, both free, both answering at the same
/// endpoint. `zeta-runner` sorts *second* in `provider_names` (a `BTreeSet`),
/// so it is second in the candidate list — and it is first in the user's own
/// `free_resource_order`, which is an input only `choose` reads.
///
/// So the two answers differ: anything that consults the routing policy calls
/// `zeta-model`, and anything that builds a model from the candidates
/// directly calls `alpha-model`. The assertion is against the **wire**, which
/// is the one place a model can be named without a seam agreeing to it.
#[test]
fn automatic_routing_asks_the_resource_the_routing_policy_chose() {
    let model = FakeModel::answering(MODEL_ANSWER);
    let fixture = Fixture::new();
    fixture.add_provider("alpha-runner", "alpha-model", &model.base_url());
    fixture.add_provider("zeta-runner", "zeta-model", &model.base_url());
    fixture.automatic_routing_model();
    fixture.prefer_free_resource("zeta-runner", "zeta-model");

    let run = fixture.classify(QUESTION);
    assert!(run.status.success(), "stderr: {}", run.stderr);

    let requests = model.requests();
    assert_eq!(requests.len(), 1, "one classification is one model call");
    assert!(
        requests[0].body.contains("zeta-model"),
        "the call must go to the resource `DisposableRouting::choose` selected: {}",
        requests[0].body
    );
    assert!(
        !requests[0].body.contains("alpha-model"),
        "a model reached around the routing policy is a model whose cost nothing decided: {}",
        requests[0].body
    );
    assert!(
        run.stdout
            .contains("source                  model (zeta-runner/zeta-model via openai-chat)"),
        "the report must name the routed resource:\n{}",
        run.stdout
    );
}

/// Map lines 1441/1442, on the shipped binary — the production-path proof
/// `routing_disposable_tier`'s in-process tests cannot give (§35): each
/// `glasshouse classify` invocation is its own process, so stickiness must
/// survive a disk round-trip through `RoutingStickyCache`, not just two
/// calls against one in-memory `DisposableRouting` value.
///
/// The user's free-resource order is changed between the two calls — a
/// fresh `choose` would resolve it to `alpha-model` the second time, exactly
/// as `automatic_routing_asks_the_resource_the_routing_policy_chose` proves
/// order changes the answer. The second call must still name `zeta-model`:
/// proof it reused the retained pick rather than re-ranking.
#[test]
fn two_successive_classify_processes_reuse_the_same_routed_resource() {
    let model = FakeModel::answering(MODEL_ANSWER);
    let fixture = Fixture::new();
    fixture.add_provider("alpha-runner", "alpha-model", &model.base_url());
    fixture.add_provider("zeta-runner", "zeta-model", &model.base_url());
    fixture.automatic_routing_model();
    fixture.prefer_free_resource("zeta-runner", "zeta-model");

    let first = fixture.classify(QUESTION);
    assert!(first.status.success(), "stderr: {}", first.stderr);

    fixture.prefer_free_resource("alpha-runner", "alpha-model");

    let second = fixture.classify(QUESTION);
    assert!(second.status.success(), "stderr: {}", second.stderr);

    let requests = model.requests();
    assert_eq!(
        requests.len(),
        2,
        "two classify processes, one model call each"
    );
    assert!(
        requests[0].body.contains("zeta-model"),
        "the first call must go to the resource named first in the order: {}",
        requests[0].body
    );
    assert!(
        requests[1].body.contains("zeta-model") && !requests[1].body.contains("alpha-model"),
        "the second call, inside the sticky window, must reuse the retained pick rather than \
         re-rank against the changed order: {}",
        requests[1].body
    );
}

// ---------------------------------------------------------------------------
// 3. A model failure falls back to the heuristic.
// ---------------------------------------------------------------------------

/// An unreachable model produces a classification, says the heuristic
/// produced it, and exits zero.
#[test]
fn a_failing_model_falls_back_to_the_heuristic_and_says_so() {
    let model = FakeModel::failing();
    let fixture = Fixture::new();
    fixture.add_provider("alpha-runner", "alpha-model", &model.base_url());
    fixture.pin_routing_model("alpha-runner", "alpha-model");

    let run = fixture.classify(QUESTION);

    assert!(
        run.status.success(),
        "a routing model that cannot answer must not turn a working command into a failing \
         one: {}",
        run.stderr
    );
    assert!(
        !model.requests().is_empty(),
        "the endpoint must actually have been called for this to be a failure test"
    );
    assert!(
        run.stdout
            .contains("source                  deterministic heuristics"),
        "a failed call must produce the heuristic's answer, never a fabricated one:\n{}",
        run.stdout
    );
    assert!(
        !run.stdout.contains("frontier"),
        "nothing from the model's schema may survive a failed call:\n{}",
        run.stdout
    );
    assert!(
        run.stderr
            .contains("deterministic heuristics answered instead"),
        "the degrade must be said out loud, not implied by the source line:\n{}",
        run.stderr
    );
}

/// A reply that is not JSON at all is a failure, not a guess.
#[test]
fn a_reply_that_is_not_json_falls_back_rather_than_fabricating() {
    let model = FakeModel::answering("I'm sorry, I can't help with that request.");
    let fixture = Fixture::new();
    fixture.add_provider("alpha-runner", "alpha-model", &model.base_url());
    fixture.pin_routing_model("alpha-runner", "alpha-model");

    let run = fixture.classify(QUESTION);

    assert!(run.status.success(), "stderr: {}", run.stderr);
    assert!(
        run.stdout
            .contains("source                  deterministic heuristics"),
        "{}",
        run.stdout
    );
    assert!(
        run.stderr.contains("was not JSON"),
        "the reason must name the shape of the failure:\n{}",
        run.stderr
    );
}

/// A reply that is JSON and is missing one required field is a failure too.
///
/// This is the discriminating half of "never fabricate": a parser with a
/// default for `workload_tier` would answer here, and the answer would wear
/// `ClassificationSource::Model` while carrying a tier no model chose.
#[test]
fn a_reply_missing_a_field_falls_back_rather_than_defaulting() {
    let incomplete = MODEL_ANSWER.replace(r#""workload_tier": "frontier","#, "");
    let model = FakeModel::answering(&incomplete);
    let fixture = Fixture::new();
    fixture.add_provider("alpha-runner", "alpha-model", &model.base_url());
    fixture.pin_routing_model("alpha-runner", "alpha-model");

    let run = fixture.classify(QUESTION);

    assert!(run.status.success(), "stderr: {}", run.stderr);
    assert!(
        run.stdout
            .contains("source                  deterministic heuristics"),
        "a classification missing a field the model never chose is a fabrication:\n{}",
        run.stdout
    );
    assert!(
        run.stderr.contains("workload_tier"),
        "the reason must name the field that was missing:\n{}",
        run.stderr
    );
}

// ---------------------------------------------------------------------------
// 4. `Heuristics` is unchanged.
// ---------------------------------------------------------------------------

/// **The discriminating half of the whole package.** A build with no routing
/// model configured behaves exactly as it did before `glasshouse classify`
/// could call anything: it prints the heuristic's report, and it opens no
/// socket to print it.
///
/// The socket stands up and is never connected to, which is what makes
/// "asked nothing" a measurement rather than an absence of evidence.
#[test]
fn no_routing_model_configured_prints_exactly_what_it_always_did() {
    let model = FakeModel::answering(MODEL_ANSWER);
    let fixture = Fixture::new();
    // A provider *is* configured and answering — only the routing model is
    // not chosen. Without this the test would prove nothing more than that an
    // empty configuration reaches no endpoint.
    fixture.add_provider("alpha-runner", "alpha-model", &model.base_url());

    let run = fixture.classify(QUESTION);

    assert!(run.status.success(), "stderr: {}", run.stderr);
    assert!(
        model.requests().is_empty(),
        "a user who configured no routing model must not have one called for them"
    );
    assert_eq!(
        run.stdout,
        glasshouse::routing::classify::report(QUESTION, None),
        "the unconfigured path must be byte-for-byte the heuristic report"
    );
    assert!(
        run.stderr.is_empty(),
        "nothing degraded, so nothing is said: {}",
        run.stderr
    );
}

// ---------------------------------------------------------------------------
// 5. The call is recorded, and extraction's rows are not back-filled.
// ---------------------------------------------------------------------------

/// **What it cost, and whose row is whose.** A classification call lands in
/// `routing_observations` with `purpose = "classification"`, and the
/// extraction call that ran against the same endpoint in the same project
/// keeps its `purpose` `NULL`.
///
/// Both halves matter. The first is the axis Phase 34E's own lines need to
/// tell routing spend from task spend. The second is the refusal to
/// back-fill: `ModelCall::observation` leaves `purpose` unwritten and
/// documents why, and extraction's existing rows are already on disk without
/// one — a builder added for a new producer must not silently relabel them.
#[test]
fn a_classification_call_is_recorded_under_its_purpose_and_extraction_is_not() {
    let model = FakeModel::start(|body| {
        if body.contains("routing-model") {
            Answer::Content(MODEL_ANSWER.to_owned())
        } else {
            Answer::Content(EXTRACTION_ANSWER.to_owned())
        }
    });
    let fixture = Fixture::new();
    fixture.add_provider("alpha-runner", "routing-model", &model.base_url());
    fixture.add_provider("omega-runner", "extraction-model", &model.base_url());
    fixture.pin_routing_model("alpha-runner", "routing-model");
    fixture.choose_extraction_model("omega-runner", "extraction-model");

    assert!(fixture.classify(QUESTION).status.success());
    let session = running_session(&fixture);
    assert!(
        fixture.hook(&session).success(),
        "a hook must exit zero whatever extraction did"
    );

    let classification = fixture.observations("alpha-runner", "routing-model");
    assert_eq!(
        classification.len(),
        1,
        "the classification call must have left exactly one row"
    );
    assert_eq!(
        classification[0].purpose.as_deref(),
        Some("classification"),
        "the row must say what the call was for"
    );
    assert_eq!(
        classification[0].input_tokens,
        Some(314),
        "the row must carry what the provider reported spending"
    );

    let extraction = fixture.observations("omega-runner", "extraction-model");
    assert_eq!(
        extraction.len(),
        1,
        "the extraction call must have left exactly one row"
    );
    assert_eq!(
        extraction[0].purpose, None,
        "extraction's producer writes no purpose, and adding one for classification must not \
         change that"
    );
}

// ---------------------------------------------------------------------------
// The newtype's invariant: what reaches the wire is scrubbed.
// ---------------------------------------------------------------------------

/// **A credential pasted into a request never reaches the model.**
///
/// `Prompt` is a newtype with no public field and no `From<String>` so that
/// the only text able to reach a model is text its own constructors
/// assembled — and the whole point of that is the scrub each one performs.
/// `Prompt::for_request` is a second constructor added to that type, so it
/// carries the same obligation, and this is the test that holds it to it.
///
/// It was written because the mutation that removes the scrub **survived**
/// every other test in this file: each of them asserts about the
/// classification that came back, and none of them looks at what went out.
/// A security invariant nothing watches is one refactor away from being gone.
///
/// The key below is fabricated and matches a shape `secret::redact` names in
/// its own tests. The assertion is against the request **body**, not the
/// whole request: the `authorization` header legitimately carries the
/// fixture's credential, which is what makes "no credential in the body" a
/// claim about the prompt rather than about the connection.
/// Phase 34D. `glasshouse classify` sends the same structured router request
/// a launch does — with every session fact honestly absent, because this
/// command decides nothing — and the report prints the two recommendation
/// fields the reply schema gained, marking them derived when the model
/// stated none.
#[test]
fn the_classify_command_sends_the_structured_router_request() {
    let model = FakeModel::answering(MODEL_ANSWER);
    let fixture = Fixture::new();
    fixture.add_provider("alpha-runner", "alpha-model", &model.base_url());
    fixture.pin_routing_model("alpha-runner", "alpha-model");

    let run = fixture.classify(QUESTION);
    assert!(run.status.success(), "stderr: {}", run.stderr);

    let requests = model.requests();
    assert_eq!(requests.len(), 1);
    let body = &requests[0].body;
    assert!(body.contains("## The routing request"), "{body}");
    assert!(body.contains(QUESTION), "{body}");
    assert!(
        body.contains("warm session      none"),
        "a diagnostic with no session in hand must say so rather than invent one:\n{body}"
    );
    assert!(
        body.contains("harness           not named; selected by the tool"),
        "{body}"
    );
    assert!(body.contains("no candidate provider named"), "{body}");
    assert!(body.contains("destination       none stated"), "{body}");

    assert!(
        run.stdout
            .contains("expected duration       long-running (derived; the classifier stated none)"),
        "{}",
        run.stdout
    );
    assert!(
        run.stdout.contains(
            "execution shape         reuse session (derived; the classifier stated none)"
        ),
        "{}",
        run.stdout
    );
}

#[test]
fn a_credential_in_the_request_never_reaches_the_model() {
    const PASTED_KEY: &str = "sk-abcd1234efgh5678ijkl";
    let request = format!("why does {PASTED_KEY} stop working after an hour");

    let model = FakeModel::answering(MODEL_ANSWER);
    let fixture = Fixture::new();
    fixture.add_provider("alpha-runner", "alpha-model", &model.base_url());
    fixture.pin_routing_model("alpha-runner", "alpha-model");

    let run = fixture.classify(&request);
    assert!(run.status.success(), "stderr: {}", run.stderr);

    let requests = model.requests();
    assert_eq!(requests.len(), 1, "the model must actually have been asked");
    assert!(
        !requests[0].body.contains(PASTED_KEY),
        "a credential the user pasted into their request must not leave the process:\n{}",
        requests[0].body
    );
    assert!(
        requests[0].body.contains("stop working after an hour"),
        "the rest of the request must still be there — scrubbed, not dropped:\n{}",
        requests[0].body
    );
    assert!(
        run.stdout.contains(PASTED_KEY),
        "the report echoes the request the user actually typed; only what reaches the model is \
         altered:\n{}",
        run.stdout
    );
}
