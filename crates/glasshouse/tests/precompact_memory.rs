//! Map line 1174 — *"record enough pre-compaction durable memory that
//! important project decisions do not depend solely on a lossy native
//! compact summary."*
//!
//! `tests/memory_extract_triggers.rs` already proves that a `PreCompact`
//! hook, through the shipped binary, asks the configured model and writes a
//! memory (`a_harness_about_to_compact_runs_extraction_and_records_no_lifecycle_event`).
//! What that file does not assert is the one thing 1174 itself is about: the
//! memory left behind names `BeforeCompaction` as its own trigger, distinct
//! from every other extraction path, so a reader can tell *why* it exists.
//! This file adds that assertion and the mutation that guards it — the same
//! shape `docs/product/evidence/phase-31.md`'s 1171 entry used for its own
//! neighbour in the same hook arm (`main.rs:5940-5975`).
//!
//! Everything here goes through `glasshouse hook`, spawned exactly as a
//! harness spawns it (practice §35): a seam that called `run_extraction`
//! directly would not be proving the production caller at `main.rs:5971`.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use clap::Parser;

use glasshouse::config::{ExtractionModelRef, ProviderConfig, UserConfig};
use glasshouse::memory::ProjectMemory;
use glasshouse::memory::search::SearchScope;
use glasshouse::session::{NewSession, ProjectSessions, SessionId, SessionLifecycle};
use glasshouse::{Cli, Runtime};

use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

const CREDENTIAL_VAR: &str = "GLASSHOUSE_TEST_ONLY_PRECOMPACT_MODEL_KEY";
const CREDENTIAL: &str = "sk-fabricated-test-value-not-a-real-credential";
const MODEL: &str = "a-cheap-local-model";
const PROVIDER: &str = "precompact-test-runner";

/// One finding, in the extraction contract's own shape — the body text is
/// unique to this file so a search that finds it found this test's memory
/// and not another file's.
const ONE_FINDING: &str = r#"{"memories":[{"kind":"finding","authority":"constraint",
     "disposition":"accepted","support":"established","confidence":"certain",
     "rationale":"a compaction was about to happen",
     "project_phase":"alpha",
     "body":"Durable memory written before a PreCompact hook fired, line 1174."}]}"#;

// ---------------------------------------------------------------------------
// A canned OpenAI chat-completions endpoint — the same shape
// `memory_extract_triggers.rs` uses, kept self-contained rather than shared
// across integration-test binaries.
// ---------------------------------------------------------------------------

struct FakeModel {
    address: SocketAddr,
    requests: Arc<Mutex<usize>>,
    stop: Arc<AtomicBool>,
}

impl FakeModel {
    fn answering(content: &str) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("loopback must bind");
        listener
            .set_nonblocking(true)
            .expect("the accept loop polls its stop flag");
        let address = listener.local_addr().unwrap();
        let requests = Arc::new(Mutex::new(0usize));
        let stop = Arc::new(AtomicBool::new(false));

        let thread_requests = Arc::clone(&requests);
        let thread_stop = Arc::clone(&stop);
        let content = content.to_owned();
        std::thread::spawn(move || {
            while !thread_stop.load(Ordering::SeqCst) {
                match listener.accept() {
                    Ok((stream, _)) => {
                        *thread_requests.lock().unwrap() += 1;
                        serve(stream, &content);
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
            requests,
            stop,
        }
    }

    fn base_url(&self) -> String {
        format!("http://{}/v1", self.address)
    }

    fn request_count(&self) -> usize {
        *self.requests.lock().unwrap()
    }
}

impl Drop for FakeModel {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
    }
}

/// Drain one request byte-oriented and answer with `content` as the model's
/// message — enough to satisfy `ConfiguredModel`, nothing more.
///
/// # The accepted stream is put back into blocking mode, and that is the
/// whole of a defect this file used to have
///
/// [`FakeModel::answering`] marks the *listener* non-blocking so the accept
/// loop can poll its stop flag. On Linux `accept(2)` hands back a socket
/// without `O_NONBLOCK`; on macOS and the BSDs the accepted socket
/// **inherits** it. So on this project's development platform every stream
/// arriving here was non-blocking, and the first `read_line` below returned
/// `WouldBlock` whenever the client's request bytes had not landed yet — a
/// pure race with the client, won when the machine is quiet and lost when it
/// is busy.
///
/// The old code read that `Err` as "this client is finished with me" and
/// returned, dropping the stream unanswered. `ureq` then saw the peer hang up
/// mid-exchange, `transport_error`'s catch-all mapped it to
/// `ModelError::Unavailable`, and extraction reported *"no extraction model
/// is available"* about a model that was listening the whole time. Measured
/// 2026-08-31 on this machine: **14 of 20 serial runs failed** under load,
/// 0 of 8 when quiet — which is exactly the shape that reads as a product
/// defect and is not one.
///
/// A test server that answers only when the machine is idle proves nothing
/// about the code under test, so the flag is cleared here and the two socket
/// deadlines below keep the bound this thread still needs.
fn serve(mut stream: TcpStream, content: &str) {
    use std::io::{BufRead, BufReader, Read};

    // See this function's own doc comment: on BSD-derived platforms this
    // stream inherited the listener's `O_NONBLOCK` and every read below would
    // race the client for it.
    stream
        .set_nonblocking(false)
        .expect("an accepted stream must be readable with a blocking read");
    // Blocking is not the same as unbounded. A client that connects and says
    // nothing must not park this thread for the rest of the run, so both
    // directions get a deadline far longer than a loopback exchange needs and
    // far shorter than a test's patience.
    let bound = std::time::Duration::from_secs(30);
    stream
        .set_read_timeout(Some(bound))
        .expect("a read deadline");
    stream
        .set_write_timeout(Some(bound))
        .expect("a write deadline");

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

    let document = serde_json::json!({
        "choices": [{ "message": { "role": "assistant", "content": content } }]
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

    /// A provider naming the fake model, chosen for extraction — the exact
    /// configuration a person writes to point Glasshouse at a cheap runner.
    fn choose_model(&self, base_url: &str) {
        let mut user = UserConfig::load(self.runtime.paths()).unwrap();
        let mut provider = ProviderConfig::new("openai-compatible");
        provider.set_base_url(Some(base_url.to_owned()));
        provider.set_credential_env(vec![CREDENTIAL_VAR.to_owned()]);
        user.providers_mut().set(PROVIDER, provider);
        user.set_memory_extraction_model(Some(ExtractionModelRef::new(PROVIDER, MODEL)));
        user.save(self.runtime.paths()).unwrap();
    }

    /// Run the shipped binary's hook command, and keep everything the hook
    /// said while doing it.
    ///
    /// The stderr is not a debugging convenience here, it is the thing under
    /// test: see
    /// `a_precompact_hook_that_records_nothing_says_so_with_no_logging_configured`.
    /// **No `--log-level` or `--log-stderr` is passed, deliberately** — a
    /// harness spawning this process passes neither, `LogConfig::resolve`
    /// answers `LogSink::Disabled`, and a test that turned logging on would
    /// be proving the observability of a configuration nobody runs.
    fn hook(&self, session: &SessionId, event: &str) -> HookRun {
        let started = std::time::Instant::now();
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
            .env("RUST_LOG", "glasshouse=debug")
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
        let elapsed = started.elapsed();
        let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
        if !stderr.trim().is_empty() {
            eprintln!("--- hook stderr ---\n{stderr}--- end ---");
        }
        HookRun {
            status: output.status,
            stderr,
            elapsed,
        }
    }

    fn memories(&self) -> Vec<glasshouse::memory::MemoryRecord> {
        // NOTE (integration, 2026-08-31): the PreCompact hook exits before its
        // extraction has necessarily reached the store, so a read taken the
        // instant the hook returns races it — three serial runs went
        // FAIL/ok/FAIL. `memories_eventually` below is the bounded wait the
        // capability's own proof needs; this raw read stays for callers that
        // want the instantaneous view.

        ProjectMemory::open(&self.runtime)
            .unwrap()
            .store()
            .search("PreCompact hook fired", SearchScope::Current, 10)
            .unwrap()
    }

    /// The store's memories once at least `at_least` have landed, or after a
    /// 10 s deadline — whichever comes first. The hook's extraction is
    /// asynchronous with respect to the hook's exit.
    fn memories_eventually(&self, at_least: usize) -> Vec<glasshouse::memory::MemoryRecord> {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            let stored = self.memories();
            if stored.len() >= at_least || std::time::Instant::now() >= deadline {
                return stored;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
    }
}

/// One run of `glasshouse hook`, and everything it said.
struct HookRun {
    status: std::process::ExitStatus,
    /// Everything the hook wrote to standard error. Empty is a meaningful
    /// value here and not a missing one — it is what a silently lost memory
    /// looked like before `hook_extraction` existed.
    stderr: String,
    elapsed: std::time::Duration,
}

const PAYLOAD: &str = r#"{"session_id":"native-1","hook_event_name":"PreCompact"}"#;

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

fn running_session(fixture: &Fixture, harness: &str) -> SessionId {
    let sessions = ProjectSessions::open(&fixture.runtime).unwrap();
    let store = sessions.store();
    let record = store.create(NewSession::embedded(harness)).unwrap();
    store
        .set_lifecycle(&record.id, SessionLifecycle::Running)
        .unwrap();
    record.id
}

/// A turn has to have happened before a compaction can extract anything:
/// `Extractor::run` short-circuits an empty chunk before asking the model
/// (see `memory_extract_triggers.rs`'s own note on this), so every test here
/// starts a turn first.
fn with_history(fixture: &Fixture, id: &SessionId) {
    assert!(fixture.hook(id, "UserPromptSubmit").status.success());
}

// ---------------------------------------------------------------------------
// Line 1174.
// ---------------------------------------------------------------------------

/// **Line 1174, through the shipped binary.** A `PreCompact` hook with
/// extraction enabled and a configured model leaves a memory behind whose
/// `extraction_trigger` is `before_compaction` — not `task_completed`,
/// `manual`, or absent — which is what lets a later reader tell this memory
/// apart from one written by any of the other three triggers.
#[test]
fn a_precompact_hook_leaves_a_memory_stamped_before_compaction() {
    let model = FakeModel::answering(ONE_FINDING);
    let fixture = Fixture::new();
    fixture.choose_model(&model.base_url());
    let id = running_session(&fixture, "codex");
    with_history(&fixture, &id);

    let run = fixture.hook(&id, "PreCompact");
    assert!(
        run.status.success(),
        "a hook must exit zero whatever extraction did"
    );
    assert_eq!(
        {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
            loop {
                if model.request_count() >= 1 || std::time::Instant::now() >= deadline {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            model.request_count()
        },
        1,
        "the harness's own approaching compaction must run extraction exactly once"
    );

    let stored = fixture.memories_eventually(1);

    // Line 1174 has exactly two acceptable answers and this asserts both of
    // them, because the third answer is the defect.
    //
    // Either the memory reached the store -- the ordinary outcome, and the
    // rest of this test is about what is stamped on it -- **or** it did not
    // and the person was told so. A hook that recorded nothing and said
    // nothing is the case that makes a reader believe their decisions were
    // captured when they were not, which is precisely the dependence on a
    // lossy compact summary the line refuses.
    //
    // There is no `hook_elapsed >= EXTRACTION_BOUND` escape here any more.
    // That escape existed because this fixture used to drop connections
    // unanswered under load (see `serve`), and it let a failing run pass by
    // pointing at a bound the run had not reached. A loss is now allowed, but
    // only out loud.
    //
    // Both branches fail together if the `hook_extraction` call is deleted
    // from `main.rs`'s `PreCompact` arm: the hook returns in milliseconds
    // with no memory **and** no notice.
    if stored.is_empty() {
        assert!(
            run.stderr
                .contains("memory extraction for `before_compaction`")
                && run.stderr.contains("glasshouse: warning:"),
            "no memory reached the store and the hook said nothing about it \
             (hook returned in {:?}); stderr was:\n{}",
            run.elapsed,
            run.stderr
        );
        eprintln!(
            "this run lost its memory and announced it, which is the second of line 1174's two \
             acceptable answers; hook took {:?}",
            run.elapsed
        );
        return;
    }
    assert_eq!(
        stored[0].extraction_trigger.as_deref(),
        Some("before_compaction"),
        "a memory written ahead of a native compaction must name that as its trigger, not \
         `task_completed`, `manual`, or nothing at all"
    );
    assert_eq!(stored[0].source_session_id.as_deref(), Some(id.as_str()));
}

/// The discriminating half: the same session, the same history, but a `Stop`
/// instead of a `PreCompact` — the memory this produces is stamped
/// `task_completed`, proving the trigger column tracks which event asked for
/// extraction rather than being a constant every extraction writes.
#[test]
fn a_completed_turn_stamps_a_different_trigger_than_a_compaction_does() {
    let model = FakeModel::answering(ONE_FINDING);
    let fixture = Fixture::new();
    fixture.choose_model(&model.base_url());
    let id = running_session(&fixture, "codex");
    with_history(&fixture, &id);

    assert!(fixture.hook(&id, "Stop").status.success());

    let stored = fixture.memories_eventually(1);
    assert_eq!(stored.len(), 1);
    assert_eq!(
        stored[0].extraction_trigger.as_deref(),
        Some("task_completed"),
        "a turn ending is not a compaction and must not be recorded as one"
    );
}

/// **The defect this file was written to find, as a deterministic test.**
///
/// A `PreCompact` hook with extraction enabled and a configured model that
/// cannot be reached: the hook exits zero, no memory is recorded — and until
/// 2026-08-31 that was the *entire* observable behaviour. `run_extraction`
/// logged `"memory extraction produced nothing"` at `INFO`, but a harness
/// spawns `glasshouse hook` with no `--log-*` flag and no `GLASSHOUSE_LOG`,
/// `logging::LogConfig::resolve` therefore answers `LogSink::Disabled`, and
/// the line was written to a subscriber that had never been installed. Exit
/// 0, empty stderr, no memory, and a person about to compact in the belief
/// that their decisions had been captured.
///
/// That is line 1174 read backwards, so this asserts the notice
/// `main.rs::hook_extraction` now writes.
///
/// # Why an unreachable model rather than a loaded machine
///
/// The original reproduction needed host load and failed about two runs in
/// three, which is not a regression test. A closed port fails the model call
/// the same way every time — `ureq` cannot connect, `transport_error` answers
/// `ModelError::Failed`, and `Extractor::run` returns before it ever reaches
/// the store — so the loss is certain and the assertion is about what the
/// hook *says*, which is the part that was missing.
#[test]
fn a_precompact_hook_that_records_nothing_says_so_with_no_logging_configured() {
    let fixture = Fixture::new();
    fixture.choose_model(&closed_base_url());
    let id = running_session(&fixture, "codex");
    with_history(&fixture, &id);

    let run = fixture.hook(&id, "PreCompact");

    // Phase 21's *"keep memory-extraction failure non-fatal to the coding
    // session"* is unchanged by any of this: the notice is on stderr and the
    // exit code still says the turn may proceed.
    assert!(
        run.status.success(),
        "a hook must exit zero whatever extraction did"
    );
    assert!(
        fixture.memories().is_empty(),
        "the premise of this test is that nothing was recorded"
    );

    // The premise the defect turned on: nothing configured logging, so the
    // tracing lines about this failure do not exist. If this ever fails, the
    // test below has stopped proving what it claims.
    assert!(
        !run.stderr.contains("INFO glasshouse") && !run.stderr.contains("DEBUG glasshouse"),
        "logging must be off for this test to mean anything; stderr was:\n{}",
        run.stderr
    );

    assert!(
        run.stderr
            .contains("memory extraction for `before_compaction` recorded nothing"),
        "a compaction that recorded nothing must say so where the person can read it; \
         stderr was:\n{}",
        run.stderr
    );
    assert!(
        run.stderr.contains("the coding session is unaffected"),
        "the notice must also say the session is fine, or it reads as a failure of the turn; \
         stderr was:\n{}",
        run.stderr
    );

    // A closed port is a *configured* model that refused the connection, not
    // an unconfigured one; the two must not read the same on stderr.
    assert!(
        run.stderr
            .contains("the extraction model could not be reached"),
        "a configured model on a closed port must be named unreachable; stderr was:\n{}",
        run.stderr
    );
    assert!(
        !run.stderr.contains("no extraction model is available"),
        "a closed port is a configured model refusing the connection, not the \
         no-model-configured case; stderr was:\n{}",
        run.stderr
    );
}

/// A loopback address with nothing listening on it.
///
/// Bound and dropped, so the port was free a moment ago and the kernel has
/// not handed it to anybody since — the ordinary way to name a closed port
/// without hard-coding one.
fn closed_base_url() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").expect("loopback must bind");
    let address = listener.local_addr().unwrap();
    drop(listener);
    format!("http://{address}/v1")
}
