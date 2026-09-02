//! **GH-EXPECTED-LATENCY-SCORE** — capability map line 1539, *include
//! expected latency in candidate scoring*.
//!
//! # What was missing
//!
//! `docs/product/evidence/phase-35b.md`'s *Why seventeen stay open* list:
//! line 1539 has no source at all. Line 1421 (`docs/product/evidence/
//! phase-34c.md`) already scores the classifier role's own latency, but
//! every other disposable job — extraction, and every other support-work
//! call — records no timing at all: `ModelCall::observation` never called
//! `with_timing`.
//!
//! # Two levels, `tests/routing_economics.rs`'s own shape
//!
//! (a) proves the **producer**: it runs the shipped binary against a canned
//! OpenAI chat-completions endpoint on loopback, exactly as
//! `tests/routed_extraction.rs` does, and reads the recorded row back. A test
//! that handed an `ExtractionModel` a fake `complete_observed` would pass on
//! a build where nothing on this path ever read a clock.
//!
//! (b), (c), (d) and the arithmetic unit prove the **reader and the score
//! term** at the policy level, entering through
//! `glasshouse::routing::disposable::DisposableRouting::choose` — the exact
//! function every dispatch function in `main.rs` calls — with facts a caller
//! attaches to a candidate by hand, and real rows planted in a ledger for the
//! reader half. Practice §35: a filter no test feeds through the production
//! entry point is, to the suite, not applied.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Instant;

use clap::Parser;
use glasshouse::config::{ExtractionModelRef, ProviderConfig, UserConfig};
use glasshouse::routing::disposable::{DisposableCandidate, DisposableRouting, JobKind};
use glasshouse::routing::evidence::{
    CLASSIFICATION_EVIDENCE_WINDOW_SECONDS, EXTRACTION_PURPOSE, EvidenceLedger, LatencyRecord,
    MIN_SAMPLE_FOR_SUMMARY, NewObservation,
};
use glasshouse::routing::free::{FreePool, FreePreferences};
use glasshouse::routing::{Cost, CredentialId};
use glasshouse::secret::SecretRef;
use glasshouse::session::{NewSession, ProjectSessions, SessionId, SessionLifecycle};
use glasshouse::{Cli, Runtime};

// ===========================================================================
// (a) The producer, through the shipped binary — `routed_extraction.rs`'s
// own fixture shape.
// ===========================================================================

const CREDENTIAL: &str = "sk-fabricated-test-value-not-a-real-credential";
const FREE_VAR: &str = "GLASSHOUSE_TEST_ONLY_EXPECTED_LATENCY_FREE_KEY";
const FREE_PROVIDER: &str = "free-runner";
const FREE_MODEL: &str = "a-free-model";
const ROUTE: &str = "openai-chat";

const ONE_FINDING: &str = r#"{"memories":[{"kind":"finding","authority":"constraint",
     "disposition":"accepted","support":"established","confidence":"certain",
     "rationale":"the routed resource is the one that answered",
     "project_phase":"alpha",
     "body":"A routed extraction request reached this project's store."}]}"#;

/// One request as it actually arrived on the wire — only what this file's
/// single binary test needs, unlike `routed_extraction.rs`'s own copy.
struct FakeModel {
    address: SocketAddr,
    stop: Arc<AtomicBool>,
}

impl FakeModel {
    fn answering(content: &str) -> Self {
        let content = content.to_owned();
        let listener = TcpListener::bind("127.0.0.1:0").expect("loopback must bind");
        listener
            .set_nonblocking(true)
            .expect("the accept loop polls its stop flag");
        let address = listener.local_addr().unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let served = AtomicUsize::new(0);

        let thread_stop = Arc::clone(&stop);
        std::thread::spawn(move || {
            while !thread_stop.load(Ordering::SeqCst) {
                match listener.accept() {
                    Ok((stream, _)) => {
                        let _ = stream.set_nonblocking(false);
                        let nth = served.fetch_add(1, Ordering::SeqCst);
                        serve(stream, &content, nth);
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

fn serve(mut stream: TcpStream, content: &str, _nth: usize) {
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

    // A slow-but-real reply: the row this test reads back must show
    // `completed_at_unix >= dispatched_at_unix`, and a call that returns
    // instantly can round to the same second either way. One second of real
    // latency on the wire makes the ordering a claim about the clock, not an
    // artifact of two reads landing in the same second.
    std::thread::sleep(std::time::Duration::from_millis(1_100));

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

    fn add_free_provider(&self, name: &str, var: &str, model: &str, base_url: &str) {
        let mut user = self.config();
        let mut provider = ProviderConfig::new("openai-compatible");
        provider.set_base_url(Some(base_url.to_owned()));
        provider.set_credential_env(vec![var.to_owned()]);
        provider.set_free_models(vec![model.to_owned()]);
        user.providers_mut().set(name, provider);
        self.save(user);
    }

    fn choose_extraction_model(&self, provider: &str, model: &str) {
        let mut user = self.config();
        user.set_memory_extraction_model(Some(ExtractionModelRef::new(provider, model)));
        self.save(user);
    }

    fn command(&self, args: &[&str]) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_glasshouse"));
        command
            .env(FREE_VAR, CREDENTIAL)
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
        let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
        let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
        assert!(
            output.status.success(),
            "`glasshouse {}` failed: {stderr}",
            args.join(" ")
        );
        Ran { stdout }
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

    /// Every row this project's ledger holds under `EXTRACTION_PURPOSE` for
    /// `(provider, model)`, read straight from the connection rather than
    /// through `EvidenceLedger::recent` — this test wants `dispatched_at`
    /// and `completed_at` themselves, which that reader does not expose.
    fn extraction_timing(&self, provider: &str, model: &str) -> Vec<(Option<i64>, Option<i64>)> {
        let conn = rusqlite::Connection::open(self.runtime.database_path()).unwrap();
        let mut statement = conn
            .prepare(
                "SELECT dispatched_at, completed_at FROM routing_observations \
                 WHERE provider = ?1 AND model = ?2 AND purpose = ?3",
            )
            .unwrap();
        statement
            .query_map(
                rusqlite::params![provider, model, EXTRACTION_PURPOSE],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap()
            .map(|row| row.unwrap())
            .collect()
    }
}

const PAYLOAD: &str = concat!(
    r#"{"session_id":"native-1","transcript_path":"/somewhere/rollout.jsonl","#,
    r#""hook_event_name":"UserPromptSubmit","cwd":"/somewhere","model":"a-model","#,
    r#""prompt":"does the extraction call now record its own timing"}"#
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

/// (a): after one `memory commit` against a fixture upstream, the extraction
/// row carries `dispatched_at` and `completed_at`, and `completed >=
/// dispatched` — the property `RoutingObservation::duration_ms` relies on to
/// ever produce a duration at all.
#[test]
fn a_memory_commit_records_dispatched_and_completed_timing_on_its_row() {
    let free = FakeModel::answering(ONE_FINDING);
    let fixture = Fixture::new();
    fixture.add_free_provider(FREE_PROVIDER, FREE_VAR, FREE_MODEL, &free.base_url());
    fixture.choose_extraction_model(FREE_PROVIDER, FREE_MODEL);

    let session = running_session(&fixture);
    fixture.one_recorded_turn(&session);
    let ran = fixture.commit(&session);
    assert!(ran.stdout.contains("stored 1"), "{}", ran.stdout);

    let rows = fixture.extraction_timing(FREE_PROVIDER, FREE_MODEL);
    assert_eq!(rows.len(), 1, "one extraction is one row");
    let (dispatched, completed) = rows[0];
    assert!(
        dispatched.is_some() && completed.is_some(),
        "a call that actually reached the provider must carry both timestamps: {rows:?}"
    );
    assert!(
        completed.unwrap() >= dispatched.unwrap(),
        "completion cannot precede dispatch: {rows:?}"
    );
}

// ===========================================================================
// (b), (c), (d) and the arithmetic unit — the reader and the score term, at
// the policy level, through `DisposableRouting::choose` — `tests/
// routing_economics.rs`'s own entry point for the sibling capability.
// ===========================================================================

fn credential(provider: &str) -> CredentialId {
    CredentialId::new(
        provider,
        SecretRef::Environment {
            var: format!("{}_API_KEY", provider.to_uppercase()),
        },
    )
}

fn metered(provider: &str, model: &str) -> DisposableCandidate {
    DisposableCandidate::new(provider, model, credential(provider), Cost::Metered)
}

fn free(provider: &str, model: &str) -> DisposableCandidate {
    DisposableCandidate::new(provider, model, credential(provider), Cost::Free)
}

fn support_routing() -> DisposableRouting {
    DisposableRouting::for_support_work(true, FreePreferences::new())
}

fn rendered(choice: &glasshouse::routing::disposable::DisposableChoice) -> String {
    choice.explanation().render()
}

struct EvidenceFixture {
    _tmp: tempfile::TempDir,
    runtime: Runtime,
}

impl EvidenceFixture {
    fn new() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path().to_path_buf();
        let root = base.join("workspace");
        std::fs::create_dir_all(root.join(".git")).unwrap();
        let root = std::fs::canonicalize(&root).unwrap();
        let runtime = bootstrap(&base, &root);
        Self { _tmp: tmp, runtime }
    }

    /// Plant one extraction row as `ModelCall::observation` plus its
    /// `main.rs::record_extraction_observation` purpose stamp would have
    /// written it — `dispatched`/`completed` in whole Unix seconds, so the
    /// resulting duration is an exact multiple of 1000ms, as
    /// `ClassificationRecord`'s own doc comment states for its sibling.
    fn plant_extraction(&self, provider: &str, model: &str, dispatched: i64, completed: i64) {
        EvidenceLedger::open(&self.runtime)
            .unwrap()
            .record(
                NewObservation::new(provider, model)
                    .with_route(Some(ROUTE))
                    .with_purpose(Some(EXTRACTION_PURPOSE))
                    .with_timing(Some(dispatched), Some(completed)),
                completed,
            )
            .unwrap();
    }

    fn latency(&self, provider: &str, model: &str, now: i64) -> LatencyRecord {
        EvidenceLedger::open(&self.runtime)
            .unwrap()
            .support_work_latency(provider, model, now, CLASSIFICATION_EVIDENCE_WINDOW_SECONDS)
            .unwrap()
    }
}

/// (b): two metered candidates, both above the sample floor with different
/// planted medians — the faster one ranks higher among the metered fallback,
/// and the winner's explanation cites its own median. Run twice with the
/// medians swapped between providers, so across the test both planted
/// medians are the one a winning explanation names — proving the ranking
/// follows the magnitude, never the list position.
#[test]
fn a_faster_metered_candidate_ranks_higher_among_the_metered_fallback() {
    let now = glasshouse::provider::cache::now_unix_seconds();
    let window_start = now - 1_000;

    // One evidence store, two candidates: alpha's rows are 3s exchanges,
    // beta's are instantaneous (0ms) exchanges.
    let evidence = EvidenceFixture::new();
    for i in 0..MIN_SAMPLE_FOR_SUMMARY {
        let at = window_start + i as i64;
        evidence.plant_extraction("alpha", "alpha-model", at, at + 3);
        evidence.plant_extraction("beta", "beta-model", at, at);
    }
    let alpha_latency = evidence.latency("alpha", "alpha-model", now);
    let beta_latency = evidence.latency("beta", "beta-model", now);
    assert_eq!(alpha_latency.median_duration_ms, Some(3_000));
    assert_eq!(beta_latency.median_duration_ms, Some(0));

    let slower = metered("alpha", "alpha-model").with_latency(Some(alpha_latency));
    let faster = metered("beta", "beta-model").with_latency(Some(beta_latency));

    let choice = support_routing()
        .choose(
            JobKind::MemoryExtraction,
            &[slower, faster],
            &FreePool::new(),
            Instant::now(),
            None,
        )
        .expect("both metered candidates are admitted at the default Plenty band");
    assert_eq!(
        choice.provider(),
        "beta",
        "the faster candidate must rank higher among the metered fallback: {}",
        rendered(&choice)
    );
    assert!(
        rendered(&choice).contains("median 0ms over 5 timed support-work calls"),
        "the winner's explanation must cite its own median:\n{}",
        rendered(&choice)
    );
}

/// The reverse of the scenario above, with the medians swapped between the
/// two providers — across both tests, both planted medians (3000ms and 0ms)
/// have appeared in a winning explanation, proving the ranking tracks the
/// number rather than which provider happens to carry it.
#[test]
fn the_faster_candidate_still_wins_with_the_medians_swapped_between_providers() {
    let now = glasshouse::provider::cache::now_unix_seconds();
    let window_start = now - 1_000;

    let evidence = EvidenceFixture::new();
    for i in 0..MIN_SAMPLE_FOR_SUMMARY {
        let at = window_start + i as i64;
        evidence.plant_extraction("alpha", "alpha-model", at, at);
        evidence.plant_extraction("beta", "beta-model", at, at + 3);
    }
    let alpha_latency = evidence.latency("alpha", "alpha-model", now);
    let beta_latency = evidence.latency("beta", "beta-model", now);
    assert_eq!(alpha_latency.median_duration_ms, Some(0));
    assert_eq!(beta_latency.median_duration_ms, Some(3_000));

    let faster = metered("alpha", "alpha-model").with_latency(Some(alpha_latency));
    let slower = metered("beta", "beta-model").with_latency(Some(beta_latency));

    let choice = support_routing()
        .choose(
            JobKind::MemoryExtraction,
            &[faster, slower],
            &FreePool::new(),
            Instant::now(),
            None,
        )
        .expect("both metered candidates are admitted at the default Plenty band");
    assert_eq!(
        choice.provider(),
        "alpha",
        "the faster candidate must rank higher regardless of which provider carries it: {}",
        rendered(&choice)
    );
    assert!(
        rendered(&choice).contains("median 0ms over 5 timed support-work calls"),
        "the winner's explanation must cite its own median:\n{}",
        rendered(&choice)
    );
}

/// (c): the reader's own floor, and the score term's honesty below it. Fewer
/// than [`MIN_SAMPLE_FOR_SUMMARY`] planted rows must still yield no median
/// from `support_work_latency`, and the score term must render the
/// *unmeasured* note rather than a magnitude computed from too few rows.
#[test]
fn below_the_sample_floor_the_reader_withholds_a_median_and_the_term_says_unmeasured() {
    let now = glasshouse::provider::cache::now_unix_seconds();
    let window_start = now - 1_000;

    let evidence = EvidenceFixture::new();
    for i in 0..(MIN_SAMPLE_FOR_SUMMARY - 1) {
        let at = window_start + i as i64;
        evidence.plant_extraction("alpha", "alpha-model", at, at + 3);
    }
    let record = evidence.latency("alpha", "alpha-model", now);
    assert_eq!(record.timed, MIN_SAMPLE_FOR_SUMMARY - 1);
    assert_eq!(
        record.median_duration_ms, None,
        "below the sample floor there is no median, only a count"
    );

    let candidate = metered("alpha", "alpha-model").with_latency(Some(record));
    let choice = support_routing()
        .choose(
            JobKind::MemoryExtraction,
            &[candidate],
            &FreePool::new(),
            Instant::now(),
            None,
        )
        .expect("the sole metered candidate is admitted at the default Plenty band");
    let explanation = rendered(&choice);
    assert!(
        explanation.contains(&format!(
            "no latency figure yet ({} of {MIN_SAMPLE_FOR_SUMMARY} timed support-work calls) \
             — this preference is inert",
            MIN_SAMPLE_FOR_SUMMARY - 1
        )),
        "below the floor the term must say it is unmeasured, not compute a magnitude:\n\
         {explanation}"
    );
}

/// (d): a free candidate's position is unchanged by any planted latency — the
/// term ranks the metered fallback and informs the explanation, and must
/// never decide which free candidate the disposable job actually uses.
#[test]
fn a_free_candidates_position_is_unchanged_by_planted_latency() {
    // The second-listed candidate carries the far better (lower) median, so
    // a regression that let the term drive free selection would also have
    // its own preference working in its favor.
    let first_choice = free("alpha", "alpha-model").with_latency(Some(LatencyRecord {
        timed: MIN_SAMPLE_FOR_SUMMARY,
        median_duration_ms: Some(5_000),
    }));
    let latency_would_prefer = free("beta", "beta-model").with_latency(Some(LatencyRecord {
        timed: MIN_SAMPLE_FOR_SUMMARY,
        median_duration_ms: Some(0),
    }));

    let choice = support_routing()
        .choose(
            JobKind::MemoryExtraction,
            &[first_choice, latency_would_prefer],
            &FreePool::new(),
            Instant::now(),
            None,
        )
        .expect("both free candidates are available");
    assert_eq!(
        choice.provider(),
        "alpha",
        "the expected-latency term must never reorder the free selection: {}",
        rendered(&choice)
    );
}

/// (d), second witness: the expected-latency term must not join the sum
/// `choose_for_automatic_classification` uses to order the free candidates
/// the user has not ranked (`classification_preferences`) — the OTHER place
/// a term could reorder a free selection, distinct from the loop
/// `a_free_candidates_position_is_unchanged_by_planted_latency` covers.
/// Neither candidate carries a classification record, so
/// `classification_preferences` sums to `0.0` for both and a stable sort
/// must leave the caller's own order untouched; only a term that leaked in
/// would break the tie.
#[test]
fn planted_latency_does_not_join_the_automatic_classification_free_order() {
    use glasshouse::routing::disposable::AutomaticClassificationDecision;

    let first_choice = free("alpha", "alpha-model").with_latency(Some(LatencyRecord {
        timed: MIN_SAMPLE_FOR_SUMMARY,
        median_duration_ms: Some(5_000),
    }));
    let latency_would_prefer = free("beta", "beta-model").with_latency(Some(LatencyRecord {
        timed: MIN_SAMPLE_FOR_SUMMARY,
        median_duration_ms: Some(0),
    }));

    let decision = support_routing()
        .choose_for_automatic_classification(
            &[first_choice, latency_would_prefer],
            &FreePool::new(),
            Instant::now(),
            glasshouse::provider::cache::now_unix_seconds(),
            None,
            None,
        )
        .expect("both free candidates are admissible under the default classification policy");
    let choice = match decision {
        AutomaticClassificationDecision::Fresh(choice, _) => choice,
        AutomaticClassificationDecision::Retained(choice) => {
            panic!("no retained pick was supplied, yet one was reused: {choice:?}")
        }
    };
    assert_eq!(
        choice.provider(),
        "alpha",
        "the expected-latency term must never join classification_preferences' sum and \
         decide the free order: {}",
        rendered(&choice)
    );
}

/// Unit: the term's arithmetic on fixed inputs — the same formula
/// classification latency's own term uses (map line 1421), `WEIGHT / (1 +
/// median_ms / 1000)`, checked against a value computed independently of the
/// production constant.
#[test]
fn the_expected_latency_terms_magnitude_matches_the_formula_on_fixed_inputs() {
    let candidate = metered("alpha", "alpha-model").with_latency(Some(LatencyRecord {
        timed: MIN_SAMPLE_FOR_SUMMARY,
        median_duration_ms: Some(1_500),
    }));
    let choice = support_routing()
        .choose(
            JobKind::MemoryExtraction,
            &[candidate],
            &FreePool::new(),
            Instant::now(),
            None,
        )
        .expect("the sole metered candidate is admitted at the default Plenty band");

    let magnitude = choice
        .explanation()
        .contributions()
        .iter()
        .find(|contribution| contribution.name() == "expected latency")
        .expect("the term is always rendered, measured or not")
        .magnitude();
    // The same weight (0.25) capability map lines 1420/1421/1422/1438 use,
    // applied to a 1500ms median: 0.25 / (1 + 1.5) = 0.1.
    let expected = 0.25_f64 / 2.5_f64;
    assert!(
        (magnitude - expected).abs() < 1e-9,
        "expected {expected}, got {magnitude}"
    );
}
