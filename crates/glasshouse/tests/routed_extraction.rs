//! **GH-ROUTED-EXTRACTION-CLIENT** — the disposable router's choice performs
//! the extraction.
//!
//! # What was missing, in the recon's words
//!
//! `docs/product/evidence/phase-33c.md`'s *1367 and 1369 censused* entry:
//! *"`RoutedModel` chooses and then calls no model at all … and
//! `disposable_extraction_model` returns a configured extraction model
//! before consulting the router at all. So today the free-pool policy
//! chooses only when nothing will be called, and the model that was called
//! was never routed."*
//!
//! Both halves are closed here, and both are proved through the **shipped
//! binary** against canned OpenAI chat-completions endpoints on loopback —
//! practice §35, in the phase where the caller is what is being built. A test
//! that handed an `Extractor` a fake `ExtractionModel` would pass on every
//! build in this repository's history, including the ones where nothing on
//! this path could call anything.
//!
//! Each endpoint parses the request itself rather than reusing anything in
//! this crate, so *"the request arrived, naming this model"* is a claim about
//! the wire.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use glasshouse::config::{ExtractionModelRef, ProviderConfig, UserConfig};
use glasshouse::routing::evidence::{EvidenceLedger, ObservationQuery};
use glasshouse::session::{NewSession, ProjectSessions, SessionId, SessionLifecycle};
use glasshouse::{Cli, Runtime};

use clap::Parser;

/// One fabricated credential value, and the two variables it is read from.
///
/// Distinct variables per provider so that "which credential paid" is a
/// question with a different answer per resource — otherwise the label
/// assertions below could pass on a build that recorded the wrong one.
const CREDENTIAL: &str = "sk-fabricated-test-value-not-a-real-credential";
const FREE_VAR: &str = "GLASSHOUSE_TEST_ONLY_ROUTED_EXTRACTION_FREE_KEY";
const NAMED_VAR: &str = "GLASSHOUSE_TEST_ONLY_ROUTED_EXTRACTION_NAMED_KEY";

const FREE_PROVIDER: &str = "free-runner";
const FREE_MODEL: &str = "a-free-model";
const NAMED_PROVIDER: &str = "named-runner";
const NAMED_MODEL: &str = "a-named-model";

/// The route slug every observation here is recorded under —
/// `WireProtocol::OpenAiChat`, the only protocol `ConfiguredModel` speaks.
const ROUTE: &str = "openai-chat";

/// What a cheap model answers: one finding, in the extraction contract's own
/// shape, with a body no other test in this repository stores.
const ONE_FINDING: &str = r#"{"memories":[{"kind":"finding","authority":"constraint",
     "disposition":"accepted","support":"established","confidence":"certain",
     "rationale":"the routed resource is the one that answered",
     "project_phase":"alpha",
     "body":"A routed extraction request reached this project's store."}]}"#;

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

enum Answer {
    Content(String),
    /// `429` with no `retry-after` — the shape a shared free tier refuses
    /// with, and the one `routing::free` has to invent its own cooldown for.
    RateLimited,
}

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

    fn rate_limiting() -> Self {
        Self::start(|_| Answer::RateLimited)
    }

    fn start(responder: impl Fn(usize) -> Answer + Send + Sync + 'static) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("loopback must bind");
        listener
            .set_nonblocking(true)
            .expect("the accept loop polls its stop flag");
        let address = listener.local_addr().unwrap();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let served = AtomicUsize::new(0);

        let thread_seen = Arc::clone(&seen);
        let thread_stop = Arc::clone(&stop);
        std::thread::spawn(move || {
            while !thread_stop.load(Ordering::SeqCst) {
                match listener.accept() {
                    Ok((stream, _)) => {
                        let _ = stream.set_nonblocking(false);
                        let nth = served.fetch_add(1, Ordering::SeqCst);
                        serve(stream, &thread_seen, &responder, nth);
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
    responder: &(impl Fn(usize) -> Answer + ?Sized),
    nth: usize,
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
        body,
    });

    let response = match responder(nth) {
        Answer::Content(content) => {
            let document = serde_json::json!({
                "choices": [{ "message": { "role": "assistant", "content": content } }],
                "usage": { "prompt_tokens": 271, "completion_tokens": 8 }
            })
            .to_string();
            format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\
                 connection: close\r\n\r\n{document}",
                document.len()
            )
        }
        Answer::RateLimited => {
            "HTTP/1.1 429 Too Many Requests\r\ncontent-length: 0\r\nconnection: close\r\n\r\n"
                .to_owned()
        }
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

struct Ran {
    stdout: String,
    stderr: String,
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

    /// One provider speaking OpenAI chat completions at `base_url`, with
    /// `model` marked according to `free`.
    fn add_provider(&self, name: &str, var: &str, model: &str, base_url: &str, free: bool) {
        let mut user = self.config();
        let mut provider = ProviderConfig::new("openai-compatible");
        provider.set_base_url(Some(base_url.to_owned()));
        provider.set_credential_env(vec![var.to_owned()]);
        if free {
            provider.set_free_models(vec![model.to_owned()]);
        } else {
            provider.set_metered_models(vec![model.to_owned()]);
        }
        user.providers_mut().set(name, provider);
        self.save(user);
    }

    /// A provider the user configured and then switched off. Its base URL is
    /// real and reachable, which is what makes "nothing was dialled" a claim
    /// rather than an accident of there being nowhere to dial.
    fn add_disabled_provider(&self, name: &str, var: &str, model: &str, base_url: &str) {
        let mut user = self.config();
        let mut provider = ProviderConfig::new("openai-compatible");
        provider.set_base_url(Some(base_url.to_owned()));
        provider.set_credential_env(vec![var.to_owned()]);
        provider.set_metered_models(vec![model.to_owned()]);
        provider.set_enabled(false);
        user.providers_mut().set(name, provider);
        self.save(user);
    }

    /// `[memory] extraction_model` — the consent, and the model the user
    /// names for this job.
    fn choose_extraction_model(&self, provider: &str, model: &str) {
        let mut user = self.config();
        user.set_memory_extraction_model(Some(ExtractionModelRef::new(provider, model)));
        self.save(user);
    }

    fn command(&self, args: &[&str]) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_glasshouse"));
        command
            .env(FREE_VAR, CREDENTIAL)
            .env(NAMED_VAR, CREDENTIAL)
            .arg("--scope")
            .arg(&self.root)
            .arg("--data-dir")
            .arg(self.base.join("data"))
            .arg("--config-dir")
            .arg(self.base.join("config"))
            .args(args);
        command
    }

    /// `glasshouse <args...>`, run the way a person runs it. Both streams are
    /// returned rather than only stdout: one test's whole subject is what
    /// reaches stderr.
    fn run(&self, args: &[&str]) -> Ran {
        let output = self
            .command(args)
            .output()
            .expect("the glasshouse binary must be runnable");
        let ran = Ran {
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        };
        assert!(
            output.status.success(),
            "`glasshouse {}` failed: {}",
            args.join(" "),
            ran.stderr
        );
        ran
    }

    /// Give the session something to extract from: one recorded turn.
    fn one_recorded_turn(&self, session: &SessionId) {
        let mut child = self
            .command(&[
                "hook",
                "--session",
                session.as_str(),
                "--event",
                "UserPromptSubmit",
            ])
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
        let output = child.wait_with_output().expect("the hook must finish");
        assert!(output.status.success());
    }

    fn commit(&self, session: &SessionId) -> Ran {
        self.run(&["memory", "commit", "--session", session.as_str()])
    }

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

    fn memory_count(&self) -> i64 {
        let conn = rusqlite::Connection::open(self.runtime.database_path()).unwrap();
        conn.query_row("SELECT COUNT(*) FROM memories", [], |row| row.get(0))
            .unwrap()
    }
}

const PAYLOAD: &str = concat!(
    r#"{"session_id":"native-1","transcript_path":"/somewhere/rollout.jsonl","#,
    r#""hook_event_name":"UserPromptSubmit","cwd":"/somewhere","model":"a-model","#,
    r#""prompt":"why does the router never call anything"}"#
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
// (a) The chosen resource is the one that is called.
// ---------------------------------------------------------------------------

/// **The joined link, and the disclosed precedence.**
///
/// A user with a configured free model *and* a configured extraction model
/// gets the **free** one dialled: `[memory] extraction_model` is the consent
/// that a model may be called at all, and `DisposableRouting::choose` decides
/// which — map line 530's *prefer free models for Glasshouse's own bounded
/// support work when quality is sufficient*, on the path that actually spends
/// something.
///
/// Before this batch the configured model bypassed the router entirely, so
/// this test's free endpoint would have seen nothing and the named one would
/// have seen the call. That is the precedence change, and this is the test
/// that pins it.
///
/// Three claims, and the third is the one the recon says was missing: the
/// request arrived on the wire naming the routed model; the ledger says what
/// it cost, under the extraction purpose; and the memory landed in the store.
#[test]
fn the_routed_free_model_receives_the_request_and_the_named_one_does_not() {
    let free = FakeModel::answering(ONE_FINDING);
    let named = FakeModel::answering(ONE_FINDING);
    let fixture = Fixture::new();
    fixture.add_provider(FREE_PROVIDER, FREE_VAR, FREE_MODEL, &free.base_url(), true);
    fixture.add_provider(
        NAMED_PROVIDER,
        NAMED_VAR,
        NAMED_MODEL,
        &named.base_url(),
        false,
    );
    fixture.choose_extraction_model(NAMED_PROVIDER, NAMED_MODEL);

    let session = running_session(&fixture);
    fixture.one_recorded_turn(&session);
    let ran = fixture.commit(&session);

    let asked = free.requests();
    assert_eq!(
        asked.len(),
        1,
        "one extraction is one model call, no more and no fewer: {}",
        ran.stdout
    );
    assert_eq!(asked[0].method, "POST");
    assert_eq!(asked[0].target, "/v1/chat/completions");
    assert!(
        asked[0].body.contains(FREE_MODEL),
        "the request must name the model routing chose: {}",
        asked[0].body
    );
    assert_eq!(
        asked[0].header("authorization"),
        Some(format!("Bearer {CREDENTIAL}").as_str()),
        "the credential the chosen candidate named must be what authenticates the call"
    );
    assert!(
        named.requests().is_empty(),
        "the configured extraction model is a candidate, not a bypass: it must not be \
         dialled while a free resource can serve"
    );

    let rows = fixture.observations(FREE_PROVIDER, FREE_MODEL);
    assert_eq!(
        rows.len(),
        1,
        "the exchange must be recorded against the resource that made it: {}",
        ran.stdout
    );
    assert_eq!(
        rows[0].purpose.as_deref(),
        Some("memory-extraction"),
        "map line 1832: the row must say what the call was for"
    );
    assert_eq!(
        rows[0].input_tokens,
        Some(271),
        "the row must carry what the provider reported spending"
    );
    assert!(
        fixture.observations(NAMED_PROVIDER, NAMED_MODEL).is_empty(),
        "a resource that was not called must not have a row"
    );

    assert!(ran.stdout.contains("stored 1"), "{}", ran.stdout);
    assert_eq!(fixture.memory_count(), 1);
    assert!(
        ran.stdout.contains(FREE_MODEL),
        "the report must name the resource that answered: {}",
        ran.stdout
    );
}

// ---------------------------------------------------------------------------
// (b) Pool health crosses the process boundary.
// ---------------------------------------------------------------------------

/// **What one process learned, the next one acts on.**
///
/// `glasshouse hook` and `glasshouse memory commit` are separate short-lived
/// processes that never see each other's `FreePool` (`docs/product/evidence/
/// phase-33c.md`: *"`RoutedModel::new` builds `FreePool::new()` — two empty
/// `Vec`s — and drops it"*). Two real `429`s from the free resource are two
/// separate processes' observations, and `FAILURES_BEFORE_COOLDOWN` is two —
/// so the third dispatch can only avoid the free resource if the first two
/// wrote what they learned somewhere the third read.
///
/// The fallback is the configured extraction model, chosen with
/// `UseReason::Fallback`: it is a candidate the whole time, and it wins
/// exactly when free capacity cannot serve.
#[test]
fn health_learned_in_two_processes_moves_the_third_to_the_configured_model() {
    let free = FakeModel::rate_limiting();
    let named = FakeModel::answering(ONE_FINDING);
    let fixture = Fixture::new();
    fixture.add_provider(FREE_PROVIDER, FREE_VAR, FREE_MODEL, &free.base_url(), true);
    fixture.add_provider(
        NAMED_PROVIDER,
        NAMED_VAR,
        NAMED_MODEL,
        &named.base_url(),
        false,
    );
    fixture.choose_extraction_model(NAMED_PROVIDER, NAMED_MODEL);

    let session = running_session(&fixture);
    fixture.one_recorded_turn(&session);

    let first = fixture.commit(&session);
    let second = fixture.commit(&session);
    assert_eq!(
        free.requests().len(),
        2,
        "the first two dispatches must both try the free resource: {} / {}",
        first.stdout,
        second.stdout
    );
    assert!(
        named.requests().is_empty(),
        "a rate limit is not a reason to spend the metered fallback until the pool says the \
         free resource cannot serve"
    );
    assert_eq!(
        fixture.memory_count(),
        0,
        "a rate-limited call stores nothing: {}",
        second.stdout
    );

    let third = fixture.commit(&session);
    assert_eq!(
        free.requests().len(),
        2,
        "the third dispatch must not try a resource two earlier processes found cooling \
         down: {}",
        third.stdout
    );
    assert_eq!(
        named.requests().len(),
        1,
        "the configured extraction model is what serves when free capacity cannot: {}",
        third.stdout
    );
    assert!(
        third.stdout.contains(NAMED_MODEL),
        "the report must name the resource that answered: {}",
        third.stdout
    );
    assert!(third.stdout.contains("stored 1"), "{}", third.stdout);
    assert_eq!(fixture.memory_count(), 1);

    let rows = fixture.observations(NAMED_PROVIDER, NAMED_MODEL);
    assert_eq!(rows.len(), 1, "{}", third.stdout);
    assert_eq!(rows[0].purpose.as_deref(), Some("memory-extraction"));
}

// ---------------------------------------------------------------------------
// (c) No adequate resource: today's words, and nothing dialled.
// ---------------------------------------------------------------------------

/// **A refusal is still not a call.**
///
/// The user named an extraction model on a provider they then disabled, and
/// configured nothing else. The endpoint is real and reachable — that is the
/// point — and nothing reaches it: the policy has no candidate, the command
/// says so in the words it said before this batch, and the store is untouched.
///
/// This is what stops the new client from becoming a second path around the
/// router: the client is only ever built for a resource
/// `DisposableRouting::choose` returned, so a configuration the policy
/// refuses is a configuration nothing dials.
#[test]
fn no_adequate_resource_fails_in_words_and_dials_nothing() {
    let endpoint = FakeModel::answering(ONE_FINDING);
    let fixture = Fixture::new();
    fixture.add_disabled_provider(NAMED_PROVIDER, NAMED_VAR, NAMED_MODEL, &endpoint.base_url());
    fixture.choose_extraction_model(NAMED_PROVIDER, NAMED_MODEL);

    let session = running_session(&fixture);
    fixture.one_recorded_turn(&session);
    let ran = fixture.commit(&session);

    assert!(
        endpoint.requests().is_empty(),
        "a resource the policy refused must not be dialled anyway"
    );
    assert!(
        ran.stdout.contains("no model was called"),
        "the words a run that called nothing has always printed: {}",
        ran.stdout
    );
    assert!(
        ran.stdout
            .contains("no provider is configured for Glasshouse's own support work"),
        "the refusal must say why, not merely that: {}",
        ran.stdout
    );
    assert_eq!(fixture.memory_count(), 0);
    assert!(
        fixture.observations(NAMED_PROVIDER, NAMED_MODEL).is_empty(),
        "nothing was spent, so there is nothing to record"
    );
}

// ---------------------------------------------------------------------------
// (d) The credential value goes to the request and nowhere else.
// ---------------------------------------------------------------------------

/// **One destination for the value, and the label everywhere else.**
///
/// `CredentialId::label` is a provider and a variable *name*; the value
/// belongs in exactly one place, the `authorization` header
/// `ConfiguredModel` builds. This asserts both directions: the header carries
/// it (otherwise the test could pass on a build that authenticates nothing),
/// and every column of the ledger row, the routing explanation the command
/// prints, and both of the process's own streams do not.
///
/// The row's `quota_context` is the positive half — it must be the label, so
/// that *which allowance paid* is answerable without going near the value.
#[test]
fn the_credential_value_reaches_the_request_and_neither_the_ledger_nor_the_output() {
    let free = FakeModel::answering(ONE_FINDING);
    let fixture = Fixture::new();
    fixture.add_provider(FREE_PROVIDER, FREE_VAR, FREE_MODEL, &free.base_url(), true);
    fixture.choose_extraction_model(FREE_PROVIDER, FREE_MODEL);

    let session = running_session(&fixture);
    fixture.one_recorded_turn(&session);
    let ran = fixture.commit(&session);

    let asked = free.requests();
    assert_eq!(asked.len(), 1, "{}", ran.stdout);
    assert_eq!(
        asked[0].header("authorization"),
        Some(format!("Bearer {CREDENTIAL}").as_str()),
        "the one place the value belongs"
    );
    assert!(
        !asked[0].body.contains(CREDENTIAL),
        "not even the request body: {}",
        asked[0].body
    );

    let rows = fixture.observations(FREE_PROVIDER, FREE_MODEL);
    assert_eq!(rows.len(), 1, "{}", ran.stdout);
    assert_eq!(
        rows[0].quota_context.as_deref(),
        Some(format!("{FREE_PROVIDER}/{FREE_VAR}").as_str()),
        "the row must name which allowance paid — the label, which is two names"
    );
    let row = format!("{:?}", rows[0]);
    assert!(
        !row.contains(CREDENTIAL),
        "no column of a routing observation may carry a credential value: {row}"
    );

    assert!(
        !ran.stdout.contains(CREDENTIAL),
        "the routing explanation names resources, never values: {}",
        ran.stdout
    );
    assert!(!ran.stderr.contains(CREDENTIAL), "stderr: {}", ran.stderr);
}
