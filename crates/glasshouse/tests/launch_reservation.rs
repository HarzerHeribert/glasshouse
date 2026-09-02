//! **GH-DISPATCH-RESERVATION-ROW** — capability map line 1367: *reserve known
//! paced capacity at dispatch…* — this file was `dispatch_reservation.rs` until
//! 2026-09-02: Windows treats any executable whose name contains *patch*
//! (or *setup*, *install*, *update*) as an installer needing elevation and
//! refuses to start it (`os error 740`), so the test binary named after this
//! file never ran on the Windows VM leg. `scripts/tests/test_windows_test_binary_names.py`
//! keeps the next name out of that list.
//!
//! Map line 1367: *reserve known
//! paced capacity at dispatch so concurrent workers do not all consume the
//! same apparent allowance.*
//!
//! # What was missing
//!
//! `GH-ROUTED-EXTRACTION-CLIENT` gave the disposable router a real spend:
//! a dispatch now resolves a credential and makes a request against it, and
//! what that request *cost* crosses the process boundary through
//! `GatewayHealthCache`. What still did not cross was what a request is
//! **about to** cost. `glasshouse hook` and `glasshouse memory commit` are
//! separate one-second processes that overlap in supported use, and each read
//! the same remaining-request count off disk and each spent it.
//!
//! # Everything here goes through the shipped binary
//!
//! Practice §35, in the phase where the caller is what is being built: the
//! reservation is a fact about **two processes**, so a test that exercised
//! `DispatchReservationCache` in one would prove the file format and nothing
//! about the capability. Test (a) starts two real `glasshouse memory commit`
//! processes against one canned upstream and counts what arrives on the wire.
//!
//! The upstream endpoints are `routed_extraction.rs`'s, with one addition —
//! an endpoint that answers slowly — because a reservation only exists while
//! a call is in flight, and a call that returns instantly leaves nothing for
//! a second process to collide with.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use glasshouse::config::{ExtractionModelRef, ProviderConfig, UserConfig};
use glasshouse::provider::telemetry::{
    DispatchReservation, DispatchReservationCache, GatewayQuotaCache, RateLimitHeaders,
};
use glasshouse::session::{NewSession, ProjectSessions, SessionId, SessionLifecycle};
use glasshouse::{Cli, Runtime};

use clap::Parser;

/// One fabricated credential value, and the two variables it is read from.
const CREDENTIAL: &str = "sk-fabricated-test-value-not-a-real-credential";
const FREE_VAR: &str = "GLASSHOUSE_TEST_ONLY_DISPATCH_RESERVATION_FREE_KEY";
const NAMED_VAR: &str = "GLASSHOUSE_TEST_ONLY_DISPATCH_RESERVATION_NAMED_KEY";

const FREE_PROVIDER: &str = "free-runner";
const FREE_MODEL: &str = "a-free-model";
const NAMED_PROVIDER: &str = "named-runner";
const NAMED_MODEL: &str = "a-named-model";

/// The credential label the reservation is keyed by — two names, which is
/// what `CredentialId::label` renders and all a record may carry.
fn free_label() -> String {
    format!("{FREE_PROVIDER}/{FREE_VAR}")
}

/// How long the free endpoint holds a request open in the tests that need
/// two dispatches to overlap.
///
/// Long enough that a second process started beside the first certainly
/// reaches its own routing decision while the first one's request is still
/// in flight, and comfortably inside `main.rs::EXTRACTION_BOUND` (five
/// seconds) so the call is not abandoned for reasons that have nothing to do
/// with this line.
const IN_FLIGHT: Duration = Duration::from_millis(2500);

const ONE_FINDING: &str = r#"{"memories":[{"kind":"finding","authority":"constraint",
     "disposition":"accepted","support":"established","confidence":"certain",
     "rationale":"the reserved request is the one that was spent",
     "project_phase":"alpha",
     "body":"A reserved extraction request reached this project's store."}]}"#;

// ---------------------------------------------------------------------------
// A canned OpenAI chat-completions endpoint.
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
struct Seen {
    method: String,
    target: String,
    body: String,
}

struct FakeModel {
    address: SocketAddr,
    seen: Arc<Mutex<Vec<Seen>>>,
    stop: Arc<AtomicBool>,
}

impl FakeModel {
    fn answering(content: &str) -> Self {
        Self::start(content, Duration::ZERO)
    }

    /// An endpoint that records the request immediately and answers after
    /// `delay` — the overlap two concurrent dispatches need to be concurrent
    /// at all.
    fn answering_slowly(content: &str, delay: Duration) -> Self {
        Self::start(content, delay)
    }

    fn start(content: &str, delay: Duration) -> Self {
        let content = content.to_owned();
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
                        served.fetch_add(1, Ordering::SeqCst);
                        let seen = Arc::clone(&thread_seen);
                        let content = content.clone();
                        // Each request on its own thread, so a slow answer
                        // holds one dispatch open without also queueing the
                        // next one behind it.
                        std::thread::spawn(move || serve(stream, &seen, &content, delay));
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

fn serve(mut stream: TcpStream, seen: &Arc<Mutex<Vec<Seen>>>, content: &str, delay: Duration) {
    let mut reader = BufReader::new(stream.try_clone().expect("the stream clones"));
    let mut request_line = String::new();
    if reader.read_line(&mut request_line).is_err() || request_line.is_empty() {
        return;
    }
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or_default().to_owned();
    let target = parts.next().unwrap_or_default().to_owned();

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
    // Recorded before the delay: "the request arrived" is the claim these
    // tests make, and it is true the moment it arrives.
    seen.lock().unwrap().push(Seen {
        method,
        target,
        body: String::from_utf8_lossy(&body).into_owned(),
    });

    if !delay.is_zero() {
        std::thread::sleep(delay);
    }

    let document = serde_json::json!({
        "choices": [{ "message": { "role": "assistant", "content": content } }],
        "usage": { "prompt_tokens": 271, "completion_tokens": 8 }
    })
    .to_string();
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

struct Ran {
    stdout: String,
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

    fn choose_extraction_model(&self, provider: &str, model: &str) {
        let mut user = self.config();
        user.set_memory_extraction_model(Some(ExtractionModelRef::new(provider, model)));
        self.save(user);
    }

    /// A real rate-limit reading for `provider`, the way a gateway would have
    /// left one: this is what makes the pool **paced and known**, which is
    /// the precondition line 1367 names and the thing an unmeasured pool
    /// does not have.
    fn plant_pool(&self, provider: &str, limit: u32, remaining: u32) {
        GatewayQuotaCache::new(self.runtime.paths()).store(
            provider,
            &RateLimitHeaders::read(vec![
                ("x-ratelimit-limit-requests", limit.to_string().as_str()),
                (
                    "x-ratelimit-remaining-requests",
                    remaining.to_string().as_str(),
                ),
            ]),
            now_unix(),
        );
    }

    fn reservations(&self) -> DispatchReservationCache {
        DispatchReservationCache::new(self.runtime.paths())
    }

    /// Every reservation file currently on disk, whatever it says.
    fn reservation_files(&self) -> Vec<PathBuf> {
        let Ok(entries) = std::fs::read_dir(self.reservations().root()) else {
            return Vec::new();
        };
        entries
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| path.extension().and_then(|ext| ext.to_str()) == Some("json"))
            .collect()
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

    fn run(&self, args: &[&str]) -> Ran {
        let output = self
            .command(args)
            .output()
            .expect("the glasshouse binary must be runnable");
        let ran = Ran {
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        };
        assert!(
            output.status.success(),
            "`glasshouse {}` failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr)
        );
        ran
    }

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

    /// `memory commit`, started and not waited for — the only way two
    /// dispatches can be concurrent, which is the whole subject here.
    fn spawn_commit(&self, session: &SessionId) -> Child {
        self.command(&["memory", "commit", "--session", session.as_str()])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("the glasshouse binary must be runnable")
    }
}

/// The two racing dispatches' own words. Their exit status is deliberately
/// not asserted: one of them is answered by an endpoint that holds the
/// request open for `IN_FLIGHT`, and whether that call lands inside
/// `EXTRACTION_BOUND` on a loaded machine is not what these tests are about.
fn stdout_of(child: Child) -> String {
    let output = child.wait_with_output().expect("the commit must finish");
    String::from_utf8_lossy(&output.stdout).into_owned()
}

const PAYLOAD: &str = concat!(
    r#"{"session_id":"native-1","transcript_path":"/somewhere/rollout.jsonl","#,
    r#""hook_event_name":"UserPromptSubmit","cwd":"/somewhere","model":"a-model","#,
    r#""prompt":"who else is spending this allowance"}"#
);

fn now_unix() -> i64 {
    glasshouse::provider::cache::now_unix_seconds()
}

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

/// A project with a paced free resource and a metered model to fall back to.
fn two_resources(free: &FakeModel, named: &FakeModel) -> Fixture {
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
    fixture
}

// ---------------------------------------------------------------------------
// (a) Two processes, one remaining request.
// ---------------------------------------------------------------------------

/// **The line itself.** A pool with one request left, and two
/// `glasshouse memory commit` processes started together against it.
///
/// Before this batch both read the same *"one request remaining"* off the
/// quota cache, both found the free resource available, and both spent it —
/// the second one arriving at a provider that had nothing left to give. Now
/// the first one's claim is on disk before it calls, and the second sees the
/// remainder **net of it**: either it reads the row (the netting) or its own
/// exclusive claim is refused (the lock), and both roads lead to the same
/// place.
///
/// The assertion is on the wire, not on the explanation: exactly one request
/// reaches the free endpoint, and the other dispatch went to the model the
/// user named instead.
#[test]
fn two_dispatches_racing_one_remaining_request_spend_it_once() {
    let free = FakeModel::answering_slowly(ONE_FINDING, IN_FLIGHT);
    let named = FakeModel::answering(ONE_FINDING);
    let fixture = two_resources(&free, &named);
    fixture.plant_pool(FREE_PROVIDER, 10, 1);

    let session = running_session(&fixture);
    fixture.one_recorded_turn(&session);

    let first = fixture.spawn_commit(&session);
    let second = fixture.spawn_commit(&session);
    let said = format!("{}\n{}", stdout_of(first), stdout_of(second));

    assert_eq!(
        free.requests().len(),
        1,
        "one remaining request is one request spent, however many dispatches want it: {said}"
    );
    let asked = free.requests();
    assert_eq!(asked[0].method, "POST");
    assert_eq!(asked[0].target, "/v1/chat/completions");
    assert!(
        asked[0].body.contains(FREE_MODEL),
        "the request that did go must name the routed model: {}",
        asked[0].body
    );
    assert_eq!(
        named.requests().len(),
        1,
        "the dispatch that could not have the free request must fall to the model the user \
         named, not fail and not wait: {said}"
    );
    assert!(
        said.contains("reserved by another dispatch"),
        "and it must say why it fell back, naming the allowance: {said}"
    );
    assert!(
        said.contains(&free_label()),
        "the explanation names the credential by label: {said}"
    );
    assert!(!said.contains(CREDENTIAL), "and never by value: {said}");
}

// ---------------------------------------------------------------------------
// (b) A row a killed process left behind.
// ---------------------------------------------------------------------------

/// **A reservation never blocks a pool for ever.**
///
/// A `glasshouse hook` runs inside the user's turn and can be killed with the
/// harness half way through a call, leaving its row behind with nobody to
/// remove it. The row's own deadline is what stops that from taking the
/// resource out of service until the rate-limit window resets: past it, the
/// row counts for nothing and its slot is taken over.
///
/// The process id in the record is deliberately *not* what decides this —
/// pids recycle and their liveness has no portable answer — so the row here
/// carries one that is not running **and** a deadline that has passed, and
/// it is the deadline that matters.
#[test]
fn a_reservation_from_a_killed_process_expires_and_the_dispatch_proceeds() {
    let free = FakeModel::answering(ONE_FINDING);
    let named = FakeModel::answering(ONE_FINDING);
    let fixture = two_resources(&free, &named);
    fixture.plant_pool(FREE_PROVIDER, 10, 1);
    fixture
        .reservations()
        .plant(
            0,
            &DispatchReservation {
                credential_label: free_label(),
                model: FREE_MODEL.to_owned(),
                requests: 1,
                process_id: 999_999,
                reserved_at_unix: now_unix() - 600,
                expires_at_unix: now_unix() - 60,
            },
        )
        .expect("the row must be plantable");

    let session = running_session(&fixture);
    fixture.one_recorded_turn(&session);
    let ran = fixture.commit(&session);

    assert_eq!(
        free.requests().len(),
        1,
        "a row past its deadline reserves nothing: {}",
        ran.stdout
    );
    assert!(
        named.requests().is_empty(),
        "and nothing was withheld, so there was no reason to fall back: {}",
        ran.stdout
    );
    assert!(
        !ran.stdout.contains("reserved by another dispatch"),
        "nor to say the allowance was spoken for: {}",
        ran.stdout
    );
}

// ---------------------------------------------------------------------------
// (c) A row that is still live.
// ---------------------------------------------------------------------------

/// **A live row is a request this dispatch may not spend.**
///
/// The mirror of (b), with the same planted shape and one field different.
/// The free resource is otherwise perfectly usable — its endpoint is up and
/// answering, which is what makes "it was not called" a claim rather than an
/// accident — and the dispatch goes to the user's named model instead,
/// through the router's own rules and in its own words.
#[test]
fn a_live_reservation_takes_the_last_request_out_of_the_pool() {
    let free = FakeModel::answering(ONE_FINDING);
    let named = FakeModel::answering(ONE_FINDING);
    let fixture = two_resources(&free, &named);
    fixture.plant_pool(FREE_PROVIDER, 10, 1);
    fixture
        .reservations()
        .plant(
            0,
            &DispatchReservation {
                credential_label: free_label(),
                model: FREE_MODEL.to_owned(),
                requests: 1,
                process_id: 999_999,
                reserved_at_unix: now_unix(),
                expires_at_unix: now_unix() + 60,
            },
        )
        .expect("the row must be plantable");

    let session = running_session(&fixture);
    fixture.one_recorded_turn(&session);
    let ran = fixture.commit(&session);

    assert!(
        free.requests().is_empty(),
        "the pool's last request is spoken for, so nothing may be spent against it: {}",
        ran.stdout
    );
    assert_eq!(
        named.requests().len(),
        1,
        "the dispatch chooses another resource rather than waiting: {}",
        ran.stdout
    );
    assert!(
        ran.stdout.contains("reserved by another dispatch"),
        "and says which allowance was spoken for: {}",
        ran.stdout
    );
    assert!(ran.stdout.contains("stored 1"), "{}", ran.stdout);
}

// ---------------------------------------------------------------------------
// (d) A finished call gives its request back.
// ---------------------------------------------------------------------------

/// **The pool is free again when the call is, not when the process is.**
///
/// A reservation that outlived its exchange would pace a pool by the process
/// count rather than by the requests in flight. After an ordinary dispatch
/// nothing is left on disk to reserve anything — and the next dispatch spends
/// the next request, which is what proves the release is a release rather
/// than a file this test happens not to find.
#[test]
fn a_completed_call_leaves_no_reservation_behind() {
    let free = FakeModel::answering(ONE_FINDING);
    let fixture = Fixture::new();
    fixture.add_provider(FREE_PROVIDER, FREE_VAR, FREE_MODEL, &free.base_url(), true);
    fixture.choose_extraction_model(FREE_PROVIDER, FREE_MODEL);
    fixture.plant_pool(FREE_PROVIDER, 10, 2);

    let session = running_session(&fixture);
    fixture.one_recorded_turn(&session);
    let first = fixture.commit(&session);

    assert_eq!(free.requests().len(), 1, "{}", first.stdout);
    assert_eq!(
        fixture.reservation_files(),
        Vec::<PathBuf>::new(),
        "a finished call holds nothing: {}",
        first.stdout
    );
    assert_eq!(
        fixture.reservations().reserved(&free_label(), now_unix()),
        0
    );

    let second = fixture.commit(&session);
    assert_eq!(
        free.requests().len(),
        2,
        "the released request is the one the next dispatch spends: {}",
        second.stdout
    );
}

// ---------------------------------------------------------------------------
// (e) An unmeasured pool.
// ---------------------------------------------------------------------------

/// **An unknown pool reserves nothing and behaves exactly as before.**
///
/// Line 1367 is about capacity that is *known*. Nothing measured this
/// provider's pool, so there is no ceiling to claim against, and inventing
/// one would refuse dispatches on a number nobody stated. The record is not
/// written at all — the cache's whole directory is never created — which is
/// the one observation that survives the run, because a record that *was*
/// written and then released leaves the same absence of files behind.
#[test]
fn an_unmeasured_pool_reserves_nothing_and_dispatches_as_before() {
    let free = FakeModel::answering(ONE_FINDING);
    let fixture = Fixture::new();
    fixture.add_provider(FREE_PROVIDER, FREE_VAR, FREE_MODEL, &free.base_url(), true);
    fixture.choose_extraction_model(FREE_PROVIDER, FREE_MODEL);
    // Deliberately no `plant_pool`: nothing has ever read a rate-limit
    // header from this provider.

    let session = running_session(&fixture);
    fixture.one_recorded_turn(&session);
    let ran = fixture.commit(&session);

    assert_eq!(
        free.requests().len(),
        1,
        "an unmeasured pool dispatches exactly as it did before: {}",
        ran.stdout
    );
    assert!(
        !fixture.reservations().root().exists(),
        "and writes no record at all: {} exists",
        fixture.reservations().root().display()
    );
    assert!(ran.stdout.contains("stored 1"), "{}", ran.stdout);
}

// ---------------------------------------------------------------------------
// (f) What the row actually says.
// ---------------------------------------------------------------------------

/// **Two names and a model, and never a value.**
///
/// The record only exists while a call is in flight, so this reads it during
/// one: the endpoint holds the request open for `IN_FLIGHT` while the file is
/// read off disk. That makes this the one test that observes what production
/// *writes* rather than what a planted row says — (b) and (c) prove the
/// reader, and this proves the writer.
#[test]
fn the_row_a_dispatch_writes_names_the_allowance_and_never_its_value() {
    let free = FakeModel::answering_slowly(ONE_FINDING, IN_FLIGHT);
    let fixture = Fixture::new();
    fixture.add_provider(FREE_PROVIDER, FREE_VAR, FREE_MODEL, &free.base_url(), true);
    fixture.choose_extraction_model(FREE_PROVIDER, FREE_MODEL);
    fixture.plant_pool(FREE_PROVIDER, 10, 4);

    let session = running_session(&fixture);
    fixture.one_recorded_turn(&session);
    let child = fixture.spawn_commit(&session);

    let deadline = Instant::now() + Duration::from_secs(8);
    let mut written = None;
    while Instant::now() < deadline {
        if let Some(path) = fixture.reservation_files().first()
            && let Ok(text) = std::fs::read_to_string(path)
            && !text.trim().is_empty()
        {
            written = Some(text);
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let said = stdout_of(child);
    let written = written
        .expect("a dispatch against a measured pool must write its claim before it makes the call");

    assert!(
        written.contains(&free_label()),
        "the row names the allowance it holds: {written}"
    );
    assert!(
        written.contains(FREE_MODEL),
        "and what the request is for: {written}"
    );
    assert!(
        !written.contains(CREDENTIAL),
        "and never the credential's value: {written}"
    );
    assert_eq!(
        free.requests().len(),
        1,
        "the claim was taken for a call that was actually made: {said}"
    );
}
