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
//! # The proxy's world is planted directly, and that is disclosed, not hidden
//!
//! No production caller in this build attaches `session_id` to a
//! [`glasshouse::evaluation::EvaluationKind::MemoryRetrieved`] row — see
//! `evaluation/mod.rs`'s own doc comment on the reader block this package
//! adds. `glasshouse hook`'s `TurnEnded` → `RoutingOutcomeObserved` path is
//! already proved through the shipped binary by `tests/routing_outcome.rs`
//! and is not re-proved here. So the proxy tests below plant the two rows
//! the proxy join needs (`MemoryRetrieved` with a `session_id`,
//! `RoutingOutcomeObserved`) directly through
//! [`glasshouse::evaluation::EvaluationObservations::record_all`], and read
//! the result back through the real CLI — proving the *reader's* join logic,
//! which is what this package builds, while being honest that the producer
//! gap named above is not this test's claim to have closed.

use std::path::{Path, PathBuf};
use std::process::Command;

use clap::Parser;

use glasshouse::evaluation::{
    EvaluationKind, EvaluationObservations, EvaluationOutcome, NewObservation,
};
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
// Proxy — planted directly (see module header), read through the real CLI
// -------------------------------------------------------------------------

/// A retrieval attributed to a session whose harness reported a `Completed`
/// turn, with no rating, is counted as `proxy` — never merged into
/// `explicit`.
#[test]
fn a_retrieval_into_a_completed_session_with_no_rating_counts_as_proxy() {
    let tmp = tempdir();
    let fixture = Fixture::new(tmp.path(), "alpha");

    let memory_id = {
        let memory = fixture.memory();
        let record = memory
            .store()
            .record(NewMemory::new(
                MemoryKind::Decision,
                "onyx sharding is by content hash",
            ))
            .unwrap();
        record.id.as_str().to_owned()
    };

    let now = glasshouse::evaluation::now_unix();
    let ledger = fixture.ledger();
    let retrieval = NewObservation::new(EvaluationKind::MemoryRetrieved)
        .with_subject("current")
        .with_memory_id(memory_id.clone())
        .with_session_id("proxy-session");
    let outcome = NewObservation::new(EvaluationKind::RoutingOutcomeObserved)
        .with_subject("completed")
        .with_session_id("proxy-session");
    ledger.record_all(&[retrieval, outcome], now).unwrap();

    let report = fixture.retrievals();
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

/// A retrieval attributed to a session whose harness never reported a turn
/// end at all is `unknown` — nothing is inferred from silence.
#[test]
fn a_retrieval_into_a_session_with_no_turn_end_counts_as_unknown() {
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

    let now = glasshouse::evaluation::now_unix();
    let ledger = fixture.ledger();
    let retrieval = NewObservation::new(EvaluationKind::MemoryRetrieved)
        .with_subject("current")
        .with_memory_id(memory_id)
        .with_session_id("silent-session");
    ledger.record(retrieval, now).unwrap();
    // Deliberately no RoutingOutcomeObserved row for "silent-session": its
    // harness never reported a turn end.

    let report = fixture.retrievals();
    let proxy_line = line_containing(&report, "proxy useful");
    assert!(
        proxy_line.contains("proxy useful 0 of 0 retrieved-into-completed-turns"),
        "a session with no turn end must not qualify for the proxy; got: {proxy_line}"
    );
    // `line_containing` panics if no line has this exact figure, which is
    // the assertion: the retrieval is counted `unknown`, never dropped.
    line_containing(&report, "unknown 1 of 1 retrieved");
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
