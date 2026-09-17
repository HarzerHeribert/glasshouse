//! **GH-ROUTED-EXTRACTION-CLIENT**, superseded by
//! **GH-GLASSHOUSE-CONFIGURED-MODELS** (design-decisions.md, 2026-09-16):
//! `[memory] extraction_model` performs the extraction — never
//! `DisposableRouting`'s choice, which is consulted nowhere on this path any
//! more.
//!
//! GH-GLASSHOUSE-CONFIGURED-MODELS removed two tests that proved the
//! superseded behaviour and could not be adapted, since they asserted the
//! opposite of the current contract:
//! `the_routed_free_model_receives_the_request_and_the_named_one_does_not`
//! (a free candidate beside the configured model must win — there is no
//! longer a candidate list to rank) and
//! `health_learned_in_two_processes_moves_the_third_to_the_configured_model`
//! (two real `429`s teach the next process to skip the free resource — there
//! is no longer a pool to learn from). `no_adequate_resource_fails_in_words_and_dials_nothing`
//! and `the_credential_value_reaches_the_request_and_neither_the_ledger_nor_the_output`
//! stay: both already exercised one configured candidate, which is now the
//! only kind there is.
//!
//! Proved through the **shipped binary** against canned OpenAI
//! chat-completions endpoints on loopback — practice §35, in the phase where
//! the caller is what is being built. A test that handed an `Extractor` a
//! fake `ExtractionModel` would pass on every build in this repository's
//! history, including the ones where nothing on this path could call
//! anything.
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
use glasshouse::routing::evidence::EvidenceLedger;
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
    seen.lock().unwrap().push(Seen { headers, body });

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

    /// [`glasshouse::memory::extract::ModelCall::observation`]'s own doc
    /// comment: an extraction row deliberately leaves `outcome` `NULL` rather
    /// than fill it with a nearby guess, so `observations_in_window`'s
    /// `outcome IS NOT NULL` filter would silently drop every row this
    /// method plants for. `consumption_in_window` is the outcome-agnostic
    /// reader, the same fix `tests/context_firewall.rs`'s own `recent`
    /// needed for the identical reason.
    fn observations(
        &self,
        provider: &str,
        model: &str,
    ) -> Vec<glasshouse::routing::evidence::RoutingObservation> {
        EvidenceLedger::open(&self.runtime)
            .unwrap()
            .consumption_in_window(i64::MAX, i64::MAX)
            .unwrap()
            .into_iter()
            .filter(|row| {
                row.provider == provider
                    && row.model == model
                    && row.route.as_deref() == Some(ROUTE)
                    && row.harness.is_none()
            })
            .collect()
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
// (a) The configured model is the one that is called, however many free
// models are also configured.
// ---------------------------------------------------------------------------

/// **Acceptance, configured.** A free provider is configured beside the
/// named extraction model — reachable, and exactly the resource
/// `DisposableRouting::choose` used to prefer (map line 530) before this
/// package. It is never dialled: `[memory] extraction_model` names the one
/// model that is ever called, and nothing ranks it against anything else
/// (design-decisions.md, 2026-09-16).
#[test]
fn the_configured_model_receives_the_request_and_the_free_one_does_not() {
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

    let asked = named.requests();
    assert_eq!(
        asked.len(),
        1,
        "one extraction is one model call, no more and no fewer: {}",
        ran.stdout
    );
    assert!(
        asked[0].body.contains(NAMED_MODEL),
        "the request must name the configured model: {}",
        asked[0].body
    );
    assert_eq!(
        asked[0].header("authorization"),
        Some(format!("Bearer {CREDENTIAL}").as_str()),
        "the credential the configured provider names must be what authenticates the call"
    );
    assert!(
        free.requests().is_empty(),
        "a free resource nobody named must never be dialled, however available it is"
    );

    let rows = fixture.observations(NAMED_PROVIDER, NAMED_MODEL);
    assert_eq!(rows.len(), 1, "{}", ran.stdout);
    assert_eq!(
        rows[0].purpose.as_deref(),
        Some("memory-extraction"),
        "map line 1832: the row must say what the call was for"
    );
    assert!(
        fixture.observations(FREE_PROVIDER, FREE_MODEL).is_empty(),
        "a resource that was not called must not have a row"
    );

    assert!(ran.stdout.contains("stored 1"), "{}", ran.stdout);
    assert_eq!(fixture.memory_count(), 1);
}

// ---------------------------------------------------------------------------
// (c) No adequate resource: today's words, and nothing dialled.
// ---------------------------------------------------------------------------

/// **A refusal is still not a call.**
///
/// The user named an extraction model on a provider they then disabled, and
/// configured nothing else. The endpoint is real and reachable — that is the
/// point — and nothing reaches it: `[memory] extraction_model` names a
/// provider Glasshouse cannot use, the command says so in words naming the
/// provider, and the store is untouched.
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
        "a resource that cannot be used must not be dialled anyway"
    );
    assert!(
        ran.stdout.contains("no model was called"),
        "the words a run that called nothing has always printed: {}",
        ran.stdout
    );
    assert!(
        ran.stdout.contains(&format!(
            "the configured memory-extraction model names `{NAMED_PROVIDER}`, which this \
             project cannot use"
        )),
        "the refusal must name the key and the provider, not merely that it failed: {}",
        ran.stdout
    );
    assert_eq!(fixture.memory_count(), 0);
    assert!(
        fixture.observations(NAMED_PROVIDER, NAMED_MODEL).is_empty(),
        "nothing was spent, so there is nothing to record"
    );
}

// ---------------------------------------------------------------------------
// (e) GH-GLASSHOUSE-CONFIGURED-MODELS: unset key, no call.
// ---------------------------------------------------------------------------

/// **Acceptance, unset.** A free provider is configured — reachable, and
/// able to serve — but `[memory] extraction_model` never names it:
/// extraction must not dial it, and the notice names the key.
#[test]
fn no_extraction_model_configured_dials_nothing_and_the_notice_names_the_key() {
    let free = FakeModel::answering(ONE_FINDING);
    let fixture = Fixture::new();
    fixture.add_provider(FREE_PROVIDER, FREE_VAR, FREE_MODEL, &free.base_url(), true);

    let session = running_session(&fixture);
    fixture.one_recorded_turn(&session);
    let ran = fixture.commit(&session);

    assert!(
        free.requests().is_empty(),
        "an unconfigured extraction model must dial nothing, however reachable a free \
         resource is"
    );
    assert!(ran.stdout.contains("no model was called"), "{}", ran.stdout);
    assert!(
        ran.stdout.contains("[memory] extraction_model"),
        "the notice must name the key that is unset: {}",
        ran.stdout
    );
    assert_eq!(fixture.memory_count(), 0);
    assert!(fixture.observations(FREE_PROVIDER, FREE_MODEL).is_empty());
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
