//! `GH-MEMORY-RATING` — "Phase 51, the memory half of RC-B: an explicit
//! rating when given, a labelled proxy otherwise", user ruling 2026-09-02.
//!
//! - **1821** *"Measure how often retrieved memory is actually useful to the
//!   receiving agent."*
//! - **1823** *"Measure how often an old decision causes an agent to add
//!   unnecessary implementation complexity."*
//! - **1824** *"Measure how often revalidation correctly identifies a
//!   decision whose original assumptions no longer hold."*
//! - **1825** *"Measure how often agents challenge a remembered decision and
//!   whether the challenge was justified."*
//! - **1831** *"Measure how often memory prevents repetition of a recorded
//!   failed approach."*
//!
//! `glasshouse memory rate` and `glasshouse memory retrievals`' new readout
//! go **through the shipped binary** — practice §35: a caller every test
//! bypasses is not a caller, and the whole point of this package's reader
//! methods is that `glasshouse memory retrievals` (their one production
//! caller) actually prints what they compute.
//!
//! `GH-RETRIEVAL-ATTRIBUTION` adds three more producers: a session id on a
//! retrieval (`main.rs::memory_search_grouped`'s two callers, still `None`
//! in production today — see that function's own doc comment),
//! `api::unix::deliver_memory` attaching a session id to a successful
//! launch-time injection, and `EvaluationKind::MemoryRevalidated` giving
//! 1824 a denominator.
//!
//! # The proxy's whole world is now real production — `GH-TURN-OUTCOME-ROW`
//!
//! `deliver_memory`'s row is proven through the shipped binary below — a
//! session really spawned through the machine door, briefed with a real
//! memory, whose `MemoryRetrieved` row really carries that session's id.
//! The proxy used to need a `RoutingOutcomeObserved` row for the same
//! session, and that row can never arise for a door-spawned session — see
//! `evaluation/mod.rs`'s own doc comment on the reader block below for the
//! full account — so one test used to plant it directly. `GH-TURN-OUTCOME-ROW`
//! moves the join onto `TurnOutcomeObserved`, written by `record_turn_outcome`
//! for *any* session the hook's `TurnEnded` arm reaches, routed or not. The
//! test below now ends the door-spawned session's turn through the real
//! `glasshouse hook`, exactly as a harness would, and reads the proxy back
//! through the real CLI with nothing planted.

use std::path::{Path, PathBuf};
use std::process::Command;

use clap::Parser;

use glasshouse::evaluation::{EvaluationKind, EvaluationObservations, EvaluationOutcome};
// Used only by the `#[cfg(unix)]` tests below; on Windows the import would be
// unused and `-D warnings` refuses it (the wave-100 Windows VM leg, 2026-09-02).
#[cfg(unix)]
use glasshouse::evaluation::NewObservation;
use glasshouse::memory::{MemoryKind, NewMemory, ProjectMemory};
use glasshouse::{Cli, Runtime, bootstrap};

fn tempdir() -> tempfile::TempDir {
    tempfile::tempdir().unwrap()
}

/// The same shape `tests/evaluation_observations.rs`'s `Fixture` uses: a
/// bootstrapped project inside `base`, sharing `base`'s data and config
/// roots, so two fixtures over one `base` are two real projects.
struct Fixture {
    base: PathBuf,
    runtime: Runtime,
}

impl Fixture {
    fn new(base: &Path, name: &str) -> Self {
        let root: PathBuf = base.join("workspace").join(name);
        std::fs::create_dir_all(root.join(".git")).unwrap();
        let root = std::fs::canonicalize(&root).unwrap();

        let cli = Cli::try_parse_from([
            "glasshouse",
            "--data-dir",
            base.join("data").to_str().unwrap(),
            "--config-dir",
            base.join("config").to_str().unwrap(),
        ])
        .unwrap();
        let runtime = bootstrap(&cli, &root).unwrap();
        Fixture {
            base: base.to_path_buf(),
            runtime,
        }
    }

    fn memory(&self) -> ProjectMemory {
        ProjectMemory::open(&self.runtime).unwrap()
    }

    fn ledger(&self) -> EvaluationObservations {
        EvaluationObservations::open(&self.runtime).unwrap()
    }

    /// Run the shipped binary in this project, exactly as a user would.
    fn run(&self, args: &[&str]) -> std::process::Output {
        Command::new(env!("CARGO_BIN_EXE_glasshouse"))
            .current_dir(self.runtime.project().root())
            .arg("--data-dir")
            .arg(self.base.join("data"))
            .arg("--config-dir")
            .arg(self.base.join("config"))
            .args(args)
            .output()
            .expect("the glasshouse binary must run")
    }

    /// `glasshouse memory retrievals`, asserted to have succeeded, stdout
    /// returned.
    fn retrievals(&self) -> String {
        let output = self.run(&["memory", "retrievals", "--hours", "24"]);
        assert!(
            output.status.success(),
            "`glasshouse memory retrievals` failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).into_owned()
    }
}

/// The single line under a header whose figures a test wants, so an
/// assertion reads the labelled sentence rather than re-deriving the number
/// from raw counts.
fn line_containing<'a>(report: &'a str, needle: &str) -> &'a str {
    report
        .lines()
        .find(|line| line.contains(needle))
        .unwrap_or_else(|| panic!("no line contains `{needle}` in:\n{report}"))
}

// -------------------------------------------------------------------------
// 1821 — explicit ratings
// -------------------------------------------------------------------------

/// Rate a retrieved memory `useful` through the real command, and see it in
/// the real readout as explicit — the round trip both `glasshouse memory
/// rate` and the new section of `glasshouse memory retrievals` exist for.
#[test]
fn a_rated_memory_appears_as_explicit_in_the_retrievals_readout() {
    let tmp = tempdir();
    let fixture = Fixture::new(tmp.path(), "alpha");

    let memory_id = {
        let memory = fixture.memory();
        let record = memory
            .store()
            .record(NewMemory::new(
                MemoryKind::Decision,
                "onyx caching is keyed by content hash",
            ))
            .unwrap();
        record.id.as_str().to_owned()
    };

    let rate_output = fixture.run(&["memory", "rate", &memory_id, "useful"]);
    assert!(
        rate_output.status.success(),
        "`glasshouse memory rate` failed: {}",
        String::from_utf8_lossy(&rate_output.stderr)
    );
    assert!(
        String::from_utf8_lossy(&rate_output.stdout).contains("useful"),
        "the command should say what it recorded"
    );

    // The rating is its own row, never an edit of a retrieval.
    let rows = fixture
        .ledger()
        .recent_of_kind(EvaluationKind::MemoryRated, 10)
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].outcome, EvaluationOutcome::Useful);
    assert_eq!(rows[0].memory_id.as_deref(), Some(memory_id.as_str()));

    let report = fixture.retrievals();
    let line = line_containing(&report, "explicit useful");
    assert!(
        line.contains("explicit useful 1 / not-useful 0 of 1 rated"),
        "got: {line}"
    );
}

// -------------------------------------------------------------------------
// 1823, 1825, 1831 — explicit ratings over a real denominator
// -------------------------------------------------------------------------

/// **Map line 1831's own denominator**: retrievals of `memories.kind =
/// 'failed_attempt'` — the memory's own class, real and independent of the
/// proxy's missing `session_id` producer.
#[test]
fn prevented_repetition_counts_over_retrieved_failed_approach_memories() {
    let tmp = tempdir();
    let fixture = Fixture::new(tmp.path(), "alpha");

    let memory_id = {
        let memory = fixture.memory();
        let record = memory
            .store()
            .record(NewMemory::new(
                MemoryKind::FailedAttempt,
                "onyx tried a global lock and it deadlocked under load",
            ))
            .unwrap();
        record.id.as_str().to_owned()
    };

    let search_output = fixture.run(&["memory", "search", "onyx"]);
    assert!(search_output.status.success());

    let rate_output = fixture.run(&["memory", "rate", &memory_id, "prevented-repetition"]);
    assert!(
        rate_output.status.success(),
        "{}",
        String::from_utf8_lossy(&rate_output.stderr)
    );

    let report = fixture.retrievals();
    let line = line_containing(&report, "explicit prevented-repetition");
    assert!(
        line.contains("explicit prevented-repetition 1 of 1 retrieved-failed-approach-memories"),
        "got: {line}"
    );
}

/// **Map line 1823's own denominator**: retrievals of `memories.kind =
/// 'decision'`. Two decision memories are retrieved through a real search;
/// one is rated `caused-complexity`, the other left unrated.
#[test]
fn caused_complexity_counts_over_retrieved_decision_memories() {
    let tmp = tempdir();
    let fixture = Fixture::new(tmp.path(), "alpha");

    let rated_id = {
        let memory = fixture.memory();
        let store = memory.store();
        let rated = store
            .record(NewMemory::new(
                MemoryKind::Decision,
                "onyx queries are routed through the write-time index",
            ))
            .unwrap();
        store
            .record(NewMemory::new(
                MemoryKind::Decision,
                "onyx queries also cache the last hundred results",
            ))
            .unwrap();
        rated.id.as_str().to_owned()
    };

    let output = fixture.run(&["memory", "search", "onyx"]);
    assert!(output.status.success());

    let rate_output = fixture.run(&["memory", "rate", &rated_id, "caused-complexity"]);
    assert!(
        rate_output.status.success(),
        "{}",
        String::from_utf8_lossy(&rate_output.stderr)
    );

    let report = fixture.retrievals();
    let line = line_containing(&report, "explicit caused-complexity");
    assert!(
        line.contains("explicit caused-complexity 1 of 2 retrieved-decision-memories"),
        "got: {line}"
    );
    let unknown_line = line_containing(&report, "unknown 1 of 2 retrieved-decision-memories");
    assert!(
        !unknown_line.is_empty(),
        "one of the two retrieved decision memories was never rated"
    );
}

/// **Map line 1825's own denominator**: `memories.review_marked_at`, written
/// by `glasshouse memory challenge` (`MemoryStore::mark_for_review`). A
/// challenged memory is rated `challenge-justified` through the real
/// command.
#[test]
fn challenge_accuracy_counts_over_memories_marked_for_review() {
    let tmp = tempdir();
    let fixture = Fixture::new(tmp.path(), "alpha");

    let memory_id = {
        let memory = fixture.memory();
        let record = memory
            .store()
            .record(NewMemory::new(
                MemoryKind::Decision,
                "onyx replicas are pinned by zone",
            ))
            .unwrap();
        record.id.as_str().to_owned()
    };

    let challenge_output = fixture.run(&["memory", "challenge", &memory_id, "architecture_drift"]);
    assert!(
        challenge_output.status.success(),
        "{}",
        String::from_utf8_lossy(&challenge_output.stderr)
    );

    let rate_output = fixture.run(&["memory", "rate", &memory_id, "challenge-justified"]);
    assert!(
        rate_output.status.success(),
        "{}",
        String::from_utf8_lossy(&rate_output.stderr)
    );

    let report = fixture.retrievals();
    let line = line_containing(&report, "explicit challenge-justified");
    assert!(
        line.contains("explicit challenge-justified 1 / challenge-unjustified 0 of 1 challenges"),
        "got: {line}"
    );
    line_containing(&report, "unknown 0 of 1 challenges");
}

// -------------------------------------------------------------------------
// 1824 — revalidation's own denominator, `GH-RETRIEVAL-ATTRIBUTION`
// -------------------------------------------------------------------------

/// `glasshouse memory revalidate <id> reaffirmed` records its own
/// `EvaluationKind::MemoryRevalidated` row, giving 1824 a real denominator
/// where none existed before this package.
#[test]
fn a_revalidation_gives_1824_a_denominator() {
    let tmp = tempdir();
    let fixture = Fixture::new(tmp.path(), "alpha");

    let memory_id = {
        let memory = fixture.memory();
        let record = memory
            .store()
            .record(NewMemory::new(
                MemoryKind::Decision,
                "onyx caching is keyed by content hash",
            ))
            .unwrap();
        record.id.as_str().to_owned()
    };

    let output = fixture.run(&["memory", "revalidate", &memory_id, "reaffirmed"]);
    assert!(
        output.status.success(),
        "`glasshouse memory revalidate` failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let rows = fixture
        .ledger()
        .recent_of_kind(EvaluationKind::MemoryRevalidated, 10)
        .unwrap();
    assert_eq!(rows.len(), 1, "{rows:#?}");
    assert_eq!(rows[0].memory_id.as_deref(), Some(memory_id.as_str()));
    assert_eq!(rows[0].subject.as_deref(), Some("reaffirmed"));

    let report = fixture.retrievals();
    let line = line_containing(&report, "explicit revalidation-correct");
    assert!(
        line.contains("explicit revalidation-correct 0 / revalidation-wrong 0 of 1 revalidations"),
        "got: {line}"
    );
    line_containing(&report, "unknown 1 of 1 revalidations");
}

// -------------------------------------------------------------------------
// Proxy — through the real briefing door, `GH-RETRIEVAL-ATTRIBUTION`
//
// A minimal `glasshouse api serve` + a long-running no-op harness, so
// `api::unix::deliver_memory` actually runs — see the module header for why
// half of this join is production now and half is still planted.
// -------------------------------------------------------------------------

#[cfg(unix)]
mod door {
    use std::io::{BufRead, BufReader, Write};
    use std::os::unix::fs::PermissionsExt;
    use std::os::unix::net::UnixStream;
    use std::path::{Path, PathBuf};
    use std::process::{Child, Command, Stdio};
    use std::time::{Duration, Instant};

    use clap::Parser;

    use glasshouse::{Cli, Runtime};

    const TIMEOUT: Duration = Duration::from_secs(30);

    /// A project wired with a harness that stays alive reading its stdin
    /// forever — `deliver_memory`'s `SessionApi::send_text` needs a live
    /// process on the far end of the pty, and nothing here cares what it
    /// does with what it reads.
    pub struct Fixture {
        _tmp: tempfile::TempDir,
        pub base: PathBuf,
    }

    impl Fixture {
        pub fn new() -> Self {
            let tmp = tempfile::tempdir().expect("tempdir");
            let base = tmp.path().to_path_buf();

            let bin_dir = base.join("bin");
            std::fs::create_dir_all(&bin_dir).expect("create bin dir");
            let harness = bin_dir.join("silent-harness");
            std::fs::write(&harness, "#!/bin/sh\nwhile IFS= read -r line; do :; done\n")
                .expect("write the silent harness");
            let mut perms = std::fs::metadata(&harness).unwrap().permissions();
            perms.set_mode(0o755);
            std::fs::set_permissions(&harness, perms).unwrap();

            let config_dir = base.join("config");
            std::fs::create_dir_all(&config_dir).expect("create config dir");
            let escaped = harness.display().to_string().replace('\\', "\\\\");
            std::fs::write(
                config_dir.join("config.toml"),
                format!(
                    "version = 1\nimplementation_policy = false\n\n[integrations.claude-code]\n\
                     enabled = true\nexecutable = \"{escaped}\"\n"
                ),
            )
            .expect("write user config");

            Self { _tmp: tmp, base }
        }

        pub fn project_root(&self, name: &str) -> PathBuf {
            let root = self.base.join("workspace").join(name);
            std::fs::create_dir_all(root.join(".git")).expect("create project root");
            std::fs::canonicalize(&root).expect("canonicalize project root")
        }

        pub fn runtime(&self, root: &Path) -> Runtime {
            let cli = Cli::try_parse_from([
                "glasshouse",
                "--data-dir",
                self.base.join("data").to_str().unwrap(),
                "--config-dir",
                self.base.join("config").to_str().unwrap(),
            ])
            .unwrap();
            glasshouse::bootstrap(&cli, root).unwrap()
        }
    }

    /// A running `glasshouse api serve`, killed on drop — the same shape
    /// `tests/context_injection.rs`'s own `Server` uses for this door.
    pub struct Server {
        child: Child,
        socket: PathBuf,
    }

    impl Server {
        pub fn start(fixture: &Fixture, root: &Path) -> Self {
            let mut child = Command::new(env!("CARGO_BIN_EXE_glasshouse"))
                .arg("--scope")
                .arg(root)
                .arg("--data-dir")
                .arg(fixture.base.join("data"))
                .arg("--config-dir")
                .arg(fixture.base.join("config"))
                .arg("api")
                .arg("serve")
                .stderr(Stdio::piped())
                .spawn()
                .expect("spawn `glasshouse api serve`");

            let stderr = child.stderr.take().expect("captured stderr");
            let mut reader = BufReader::new(stderr);
            let deadline = Instant::now() + TIMEOUT;
            let socket = loop {
                let mut line = String::new();
                let read = reader.read_line(&mut line).expect("read server stderr");
                assert!(read > 0, "the server exited before announcing its socket");
                if let Some(path) = line
                    .trim_end()
                    .strip_prefix("glasshouse: control API listening on ")
                {
                    break PathBuf::from(path);
                }
                assert!(
                    Instant::now() < deadline,
                    "timed out waiting for the server to announce its socket"
                );
            };

            Self { child, socket }
        }

        pub fn call(&self, request: serde_json::Value) -> serde_json::Value {
            let deadline = Instant::now() + TIMEOUT;
            let mut stream = loop {
                match UnixStream::connect(&self.socket) {
                    Ok(stream) => break stream,
                    Err(err) => {
                        assert!(
                            Instant::now() < deadline,
                            "timed out connecting to the control socket: {err}"
                        );
                        std::thread::sleep(Duration::from_millis(20));
                    }
                }
            };
            let mut payload = serde_json::to_string(&request).expect("encode request");
            payload.push('\n');
            stream.write_all(payload.as_bytes()).expect("write request");

            let mut reader = BufReader::new(stream);
            let mut line = String::new();
            reader.read_line(&mut line).expect("read response");
            serde_json::from_str(line.trim_end()).expect("parse response")
        }

        /// `Request::SpawnSession` with a task — the same door that runs
        /// `select_memory`/`deliver_memory` before the task is ever sent.
        pub fn spawn_with_task(&self, task: &str) -> String {
            let response = self.call(serde_json::json!({
                "op": "spawn_session",
                "harness": "claude-code",
                "role": "worker",
                "task": task,
            }));
            assert_eq!(response["status"], "ok", "{response}");
            response["result"]["session"]
                .as_str()
                .expect("a session id")
                .to_owned()
        }

        /// `Request::SendMessage` to an already-live session — the
        /// hot-session half of `deliver_memory`'s dedup.
        pub fn send_message(&self, session: &str, text: &str) {
            let response = self.call(serde_json::json!({
                "op": "send_message",
                "session": session,
                "text": text,
            }));
            assert_eq!(response["status"], "ok", "{response}");
        }
    }

    impl Drop for Server {
        fn drop(&mut self) {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}

/// `glasshouse memory retrievals` against a project reached by its own
/// `base`/`root` rather than through [`Fixture`] — the shape [`door::Fixture`]
/// needs, since it drives the door directly instead of one-shot CLI calls.
#[cfg(unix)]
fn retrievals_report(base: &Path, root: &Path) -> String {
    let output = Command::new(env!("CARGO_BIN_EXE_glasshouse"))
        .current_dir(root)
        .arg("--data-dir")
        .arg(base.join("data"))
        .arg("--config-dir")
        .arg(base.join("config"))
        .arg("memory")
        .arg("retrievals")
        .arg("--hours")
        .arg("24")
        .output()
        .expect("the glasshouse binary must run");
    assert!(
        output.status.success(),
        "`glasshouse memory retrievals` failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).into_owned()
}

/// Run `glasshouse hook`, exactly as a harness runs it — the same shape
/// `tests/routing_outcome.rs::Fixture::hook` uses — against a project
/// reached by `base`/`root` rather than through [`Fixture`], for the same
/// reason [`retrievals_report`] is.
#[cfg(unix)]
fn run_hook(base: &Path, root: &Path, session: &str, event: &str) {
    use std::io::Write as _;
    use std::process::Stdio;

    let mut child = Command::new(env!("CARGO_BIN_EXE_glasshouse"))
        .current_dir(root)
        .arg("--data-dir")
        .arg(base.join("data"))
        .arg("--config-dir")
        .arg(base.join("config"))
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
        .write_all(b"{\"prompt\":\"unread\"}")
        .expect("write the hook payload");
    let output = child.wait_with_output().expect("the hook must exit");
    assert!(
        output.status.success(),
        "a hook always exits zero:\n{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

/// A memory injected into a session through the real launch-time briefing
/// door carries that session's id into `MemoryRetrieved` —
/// `api::unix::deliver_memory`'s new row, `GH-RETRIEVAL-ATTRIBUTION`'s
/// producer for gap 2. With no turn end ever reported for the session, the
/// proxy reads `unknown`, never `proxy` — nothing is inferred from silence.
///
/// # Mutation `record-every-injection-twice` (§16)
///
/// A second `send_message` with the same task to the same now-live session
/// must not inject the unchanged memory again — `select_memory`'s own
/// `already`-set dedup, proven here against the row count rather than
/// against delivered text the way `tests/context_injection.rs` does.
#[cfg(unix)]
#[test]
fn a_retrieval_delivered_by_the_briefing_door_with_no_turn_end_counts_as_unknown() {
    let fixture = door::Fixture::new();
    let root = fixture.project_root("alpha");
    let runtime = fixture.runtime(&root);
    let memory_id = ProjectMemory::open(&runtime)
        .unwrap()
        .store()
        .record(NewMemory::new(
            MemoryKind::Decision,
            "onyx sharding is by content hash",
        ))
        .unwrap()
        .id
        .as_str()
        .to_owned();

    let server = door::Server::start(&fixture, &root);
    let session = server.spawn_with_task("onyx");

    let ledger = EvaluationObservations::open(&runtime).unwrap();
    let retrieved = ledger
        .recent_of_kind(EvaluationKind::MemoryRetrieved, 10)
        .unwrap();
    assert_eq!(retrieved.len(), 1, "{retrieved:#?}");
    assert_eq!(retrieved[0].memory_id.as_deref(), Some(memory_id.as_str()));
    assert_eq!(
        retrieved[0].session_id.as_deref(),
        Some(session.as_str()),
        "deliver_memory must attach the session it briefed"
    );

    // The dedup mutation target: the same unchanged memory must not be
    // injected, and so must not be recorded, a second time into a session
    // that already has it.
    server.send_message(&session, "onyx");
    let retrieved_again = ledger
        .recent_of_kind(EvaluationKind::MemoryRetrieved, 10)
        .unwrap();
    assert_eq!(
        retrieved_again.len(),
        1,
        "a dedup-suppressed repeat must not record a second retrieval: {retrieved_again:#?}"
    );

    // No hook `TurnEnded` was ever sent for this session — a real production
    // absence, not a plant.
    let report = retrievals_report(&fixture.base, &root);
    let proxy_line = line_containing(&report, "proxy useful");
    assert!(
        proxy_line.contains("proxy useful 0 of 0 retrieved-into-completed-turns"),
        "a session with no turn end must not qualify for the proxy; got: {proxy_line}"
    );
    line_containing(&report, "unknown 1 of 1 retrieved");
}

/// The other half of the proxy join: a session whose harness reported a
/// `Completed` turn, with no rating, counts as `proxy`. Nothing here is
/// planted — see the module header for why the door-spawned session's own
/// `glasshouse hook` call is what makes this real: `record_turn_outcome`
/// writes a row for it even though it was never routed, which is exactly
/// what `record_routing_outcome` (still, correctly) refuses to do.
#[cfg(unix)]
#[test]
fn a_retrieval_delivered_by_the_briefing_door_into_a_completed_session_counts_as_proxy() {
    let fixture = door::Fixture::new();
    let root = fixture.project_root("alpha");
    let runtime = fixture.runtime(&root);
    ProjectMemory::open(&runtime)
        .unwrap()
        .store()
        .record(NewMemory::new(
            MemoryKind::Decision,
            "onyx replicas are pinned by zone",
        ))
        .unwrap();

    let server = door::Server::start(&fixture, &root);
    let session = server.spawn_with_task("onyx");

    let ledger = EvaluationObservations::open(&runtime).unwrap();
    let retrieved = ledger
        .recent_of_kind(EvaluationKind::MemoryRetrieved, 10)
        .unwrap();
    assert_eq!(retrieved.len(), 1, "{retrieved:#?}");
    assert_eq!(retrieved[0].session_id.as_deref(), Some(session.as_str()));

    // The harness reports its turn ended, through the real hook — the
    // door-spawned session was never routed, so `record_routing_outcome`
    // still writes nothing for it, but `record_turn_outcome` does.
    run_hook(&fixture.base, &root, &session, "Stop");

    assert!(
        ledger
            .recent_of_kind(EvaluationKind::RoutingOutcomeObserved, 10)
            .unwrap()
            .is_empty(),
        "a door-spawned session is never routed, so this row must stay empty"
    );
    let turn_outcomes = ledger
        .recent_of_kind(EvaluationKind::TurnOutcomeObserved, 10)
        .unwrap();
    assert_eq!(turn_outcomes.len(), 1, "{turn_outcomes:#?}");
    assert_eq!(
        turn_outcomes[0].session_id.as_deref(),
        Some(session.as_str())
    );
    assert_eq!(turn_outcomes[0].subject.as_deref(), Some("completed"));

    let report = retrievals_report(&fixture.base, &root);
    let line = line_containing(&report, "proxy useful");
    assert!(
        line.contains("proxy useful 1 of 1 retrieved-into-completed-turns"),
        "got: {line}"
    );
    // No rating was ever issued, so explicit must still read zero.
    let explicit_line = line_containing(&report, "explicit useful");
    assert!(
        explicit_line.contains("explicit useful 0 / not-useful 0 of 0 rated"),
        "got: {explicit_line}"
    );
}

/// A session that was both routed by `glasshouse launch` **and** briefed —
/// this build's other producer path — is not double-counted: one retrieval,
/// one turn ended, one proxy hit. This is the case that would have shipped
/// wrong if `usefulness()`/`prevented_repetition()` had joined on session id
/// alone without `EvaluationKind::TurnOutcomeObserved` being the single row
/// written per session-turn (`record_turn_outcome` writes exactly one row per
/// call, and the hook is called once per `TurnEnded`).
#[cfg(unix)]
#[test]
fn a_routed_and_briefed_session_counts_the_proxy_once() {
    let fixture = door::Fixture::new();
    let root = fixture.project_root("alpha");
    let runtime = fixture.runtime(&root);
    ProjectMemory::open(&runtime)
        .unwrap()
        .store()
        .record(NewMemory::new(
            MemoryKind::Decision,
            "onyx compaction runs nightly",
        ))
        .unwrap();

    let server = door::Server::start(&fixture, &root);
    let session = server.spawn_with_task("onyx");

    let ledger = EvaluationObservations::open(&runtime).unwrap();
    assert_eq!(
        ledger
            .recent_of_kind(EvaluationKind::MemoryRetrieved, 10)
            .unwrap()
            .len(),
        1
    );

    // This session was never routed (the door doesn't route), so a routing
    // decision is recorded against it here, by hand, purely to prove the
    // reader does not double the proxy hit when both rows exist for one
    // session — `RoutingOutcomeObserved` from the routed half and
    // `TurnOutcomeObserved` from the hook both being present for the same
    // session is exactly the shape a genuine `glasshouse launch` + briefing
    // combination would produce.
    ledger
        .record(
            NewObservation::new(EvaluationKind::RoutingOutcomeObserved)
                .with_subject("completed")
                .with_session_id(session.clone()),
            glasshouse::evaluation::now_unix(),
        )
        .unwrap();

    run_hook(&fixture.base, &root, &session, "Stop");

    let report = retrievals_report(&fixture.base, &root);
    let line = line_containing(&report, "proxy useful");
    assert!(
        line.contains("proxy useful 1 of 1 retrieved-into-completed-turns"),
        "one retrieval, one completed turn, one proxy hit — not two; got: {line}"
    );
}

// -------------------------------------------------------------------------
// Refusals
// -------------------------------------------------------------------------

/// A verdict word this build does not know is refused by name, and nothing
/// is recorded.
#[test]
fn an_unknown_verdict_word_is_refused_by_name() {
    let tmp = tempdir();
    let fixture = Fixture::new(tmp.path(), "alpha");

    let memory_id = {
        let memory = fixture.memory();
        let record = memory
            .store()
            .record(NewMemory::new(
                MemoryKind::Decision,
                "onyx indexing is lazy",
            ))
            .unwrap();
        record.id.as_str().to_owned()
    };

    let output = fixture.run(&["memory", "rate", &memory_id, "extremely-helpful"]);
    assert!(
        !output.status.success(),
        "an unrecognized verdict word must be refused"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("extremely-helpful"),
        "the refusal should name the word it refused; got: {stderr}"
    );

    assert!(
        fixture
            .ledger()
            .recent_of_kind(EvaluationKind::MemoryRated, 10)
            .unwrap()
            .is_empty(),
        "a refused rating must record nothing"
    );

    // `unknown` itself is not a rating verdict a person may type — it is
    // the sentinel this ledger writes for "not yet known".
    let output = fixture.run(&["memory", "rate", &memory_id, "unknown"]);
    assert!(
        !output.status.success(),
        "`unknown` must be refused as a verdict too"
    );
}

/// A memory id from another project is refused by name, never rated — the
/// same project isolation `memory challenge` and `memory resolve` already
/// get from `MemoryStore::resolve_id`.
#[test]
fn a_memory_from_another_project_is_refused() {
    let tmp = tempdir();
    let alpha = Fixture::new(tmp.path(), "alpha");
    let beta = Fixture::new(tmp.path(), "beta");

    let beta_memory_id = {
        let memory = beta.memory();
        let record = memory
            .store()
            .record(NewMemory::new(
                MemoryKind::Decision,
                "beta's own decision, invisible to alpha",
            ))
            .unwrap();
        record.id.as_str().to_owned()
    };

    let output = alpha.run(&["memory", "rate", &beta_memory_id, "useful"]);
    assert!(
        !output.status.success(),
        "rating another project's memory must be refused"
    );

    assert!(
        alpha
            .ledger()
            .recent_of_kind(EvaluationKind::MemoryRated, 10)
            .unwrap()
            .is_empty(),
        "alpha's ledger must record nothing for a refused rating"
    );
    assert!(
        beta.ledger()
            .recent_of_kind(EvaluationKind::MemoryRated, 10)
            .unwrap()
            .is_empty(),
        "the command ran against alpha's project, not beta's, so beta must \
         see nothing either"
    );
}
