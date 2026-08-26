//! Memory extraction, exercised end to end through a fake model.
//!
//! An integration test on purpose: every path here goes through
//! `glasshouse::bootstrap` and a real project database, so the migration, the
//! project binding, the admission guard and the isolation triggers are all in
//! play rather than mocked away.
//!
//! # What this file is for
//!
//! The acceptance condition of Phase 21, in both directions:
//!
//! > **The extractor must never be fed, and must never emit, credential
//! > material.**
//!
//! `memories.subject` and `memories.body` are free text, and the pinned-schema
//! test `the_project_database_schema_has_nowhere_to_put_a_credential` says in
//! its own doc comment that no schema can close that gap — the control belongs
//! to the producer. These tests are that control's evidence, and every one of
//! them has a recorded mutation.
//!
//! It also covers the three properties that are about the pipeline rather than
//! the contract: that a chunk is bounded, that nothing is stored twice, and
//! that **no failure here can reach the coding session**.

use std::path::Path;
use std::sync::Mutex;

use clap::Parser;

use glasshouse::memory::extract::chunk::{ChunkLimits, SessionChunk};
use glasshouse::memory::extract::schema::RATIONALE_MARKER;
use glasshouse::memory::extract::{
    ExtractionFailure, ExtractionModel, ExtractionOutcome, ExtractionTrigger, Extractor,
    ModelError, Prompt, Rejection,
};
use glasshouse::memory::{
    MemoryAuthority, MemoryKind, MemoryStatus, ProjectMemory, search::SearchScope,
};
use glasshouse::{Cli, Runtime};

/// A value shaped like a real key, built here rather than pasted, so nothing
/// in this repository is a credential anyone could try.
const PLANTED: &str = "hunter2xyzabcdefghijklmn";

// -------------------------------------------------------------------------
// Fixtures
// -------------------------------------------------------------------------

/// A bootstrapped project inside `base`, sharing `base`'s data and config
/// roots. Two fixtures over one `base` are two real projects on one machine.
struct Fixture {
    _root: std::path::PathBuf,
    runtime: Runtime,
}

impl Fixture {
    fn new(base: &Path, name: &str) -> Self {
        let root = base.join("workspace").join(name);
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
        let runtime = glasshouse::bootstrap(&cli, &root).unwrap();
        Fixture {
            _root: root,
            runtime,
        }
    }

    fn memory(&self) -> ProjectMemory {
        ProjectMemory::open(&self.runtime).unwrap()
    }
}

fn tempdir() -> tempfile::TempDir {
    tempfile::tempdir().unwrap()
}

/// Answers with a fixed reply and records every prompt it was handed.
///
/// The recording is the point for the credential tests: asserting that a
/// credential is absent from the *store* would pass even if the model had been
/// shown it, and being shown it is the half that leaves the machine.
struct Canned {
    reply: String,
    seen: Mutex<Vec<String>>,
}

impl Canned {
    fn new(reply: impl Into<String>) -> Self {
        Self {
            reply: reply.into(),
            seen: Mutex::new(Vec::new()),
        }
    }

    fn last_prompt(&self) -> String {
        self.seen
            .lock()
            .unwrap()
            .last()
            .cloned()
            .expect("the model was never asked")
    }
}

impl ExtractionModel for Canned {
    fn describe(&self) -> String {
        "fake/canned".to_owned()
    }

    fn complete(&self, prompt: &Prompt) -> Result<String, ModelError> {
        self.seen.lock().unwrap().push(prompt.as_str().to_owned());
        Ok(self.reply.clone())
    }
}

/// Refuses, the way an unconfigured or unreachable model would.
struct Broken(ModelError);

impl ExtractionModel for Broken {
    fn describe(&self) -> String {
        "fake/broken".to_owned()
    }
    fn complete(&self, _prompt: &Prompt) -> Result<String, ModelError> {
        Err(self.0.clone())
    }
}

/// Panics, the way a provider implementation with a bug would.
struct Exploding;

impl ExtractionModel for Exploding {
    fn describe(&self) -> String {
        "fake/exploding".to_owned()
    }
    fn complete(&self, _prompt: &Prompt) -> Result<String, ModelError> {
        panic!("a provider implementation with a bug in it");
    }
}

/// One well-formed memory, as JSON, with the fields a caller wants to vary.
fn memory_json(kind: &str, authority: &str, body: &str) -> String {
    format!(
        r#"{{"kind":"{kind}","authority":"{authority}","disposition":"accepted",
             "support":"established","confidence":"certain","body":"{body}"}}"#
    )
}

fn reply(memories: &[String]) -> String {
    format!("{{\"memories\": [{}]}}", memories.join(","))
}

fn chunk(activity: &[&str]) -> SessionChunk {
    SessionChunk::build(
        "session-alpha",
        Some("a938fcc"),
        activity.iter().map(|s| (*s).to_owned()),
        ChunkLimits::default(),
    )
}

/// Run one extraction against `fixture`, returning the outcome.
fn extract(
    fixture: &Fixture,
    model: &dyn ExtractionModel,
    chunk: &SessionChunk,
) -> ExtractionOutcome {
    let memory = fixture.memory();
    let store = memory.store();
    Extractor::new(&store, model).run(chunk, ExtractionTrigger::Manual)
}

/// Every current memory in `fixture`, as `(kind, authority, body)`.
fn stored(fixture: &Fixture) -> Vec<(MemoryKind, Option<MemoryAuthority>, String)> {
    let memory = fixture.memory();
    let store = memory.store();
    store
        .with_status(MemoryStatus::Active, 1_000)
        .unwrap()
        .into_iter()
        .map(|record| (record.kind, record.authority, record.body))
        .collect()
}

// -------------------------------------------------------------------------
// THE ACCEPTANCE CONDITION — never fed a credential
// -------------------------------------------------------------------------

/// Mutation: remove the `credentials::scrub` call from `SessionChunk::build`.
///
/// This is the half that matters most, because it is the half that leaves the
/// machine. A credential that never reaches a prompt cannot be logged by a
/// provider, cached by a gateway, or trained on.
#[test]
fn the_model_is_never_shown_a_credential_from_session_activity() {
    let tmp = tempdir();
    let fixture = Fixture::new(tmp.path(), "alpha");
    let model = Canned::new(reply(&[]));

    let chunk = chunk(&[
        "we decided the gateway holds the credential",
        &format!("export ANTHROPIC_AUTH_TOKEN={PLANTED}"),
        "and then the harness came up",
    ]);
    extract(&fixture, &model, &chunk);

    let prompt = model.last_prompt();
    assert!(
        !prompt.contains(PLANTED),
        "the prompt handed to the model carried the credential"
    );
    assert!(
        !prompt.contains("ANTHROPIC_AUTH_TOKEN"),
        "the prompt named the variable, which says which credential it was"
    );
    // The session is still worth extracting from — this is scrubbing, not
    // discarding. Losing the hour to punish one line would lose far more
    // than it protects.
    assert!(prompt.contains("the gateway holds the credential"));
    assert!(prompt.contains("the harness came up"));
    assert_eq!(
        chunk.redactions(),
        1,
        "the removal must be counted, not silent"
    );
}

/// A memory recorded before extraction existed never passed a screen, and the
/// prompt quotes existing memories back to the model for the duplicate rule.
///
/// Mutation: drop the `credentials::scrub` call from `Prompt::build`'s
/// existing-memories loop.
#[test]
fn the_model_is_never_shown_a_credential_from_an_already_stored_memory() {
    let tmp = tempdir();
    let fixture = Fixture::new(tmp.path(), "alpha");

    // Planted through the store directly, which is exactly how a row written
    // by something other than the extractor arrives.
    {
        let memory = fixture.memory();
        memory
            .store()
            .record(glasshouse::memory::NewMemory::new(
                MemoryKind::Finding,
                format!("legacy row: API_KEY={PLANTED}"),
            ))
            .unwrap();
    }

    let model = Canned::new(reply(&[]));
    extract(&fixture, &model, &chunk(&["something happened"]));

    assert!(
        !model.last_prompt().contains(PLANTED),
        "a stored memory leaked a credential into the prompt"
    );
}

// -------------------------------------------------------------------------
// THE ACCEPTANCE CONDITION — never emits a credential
// -------------------------------------------------------------------------

/// Mutation: remove the `credentials::screen` call from `schema::judge`.
///
/// Refused **whole**, not redacted. A redacted secret in a durable row still
/// carries its neighbourhood — which host, which account, which variable —
/// and `secret::redact`'s own documentation records the time a captured line
/// had its credential redacted and the prompt body around it verbatim.
#[test]
fn a_memory_carrying_a_credential_is_never_stored() {
    let tmp = tempdir();
    let fixture = Fixture::new(tmp.path(), "alpha");

    let model = Canned::new(reply(&[memory_json(
        "finding",
        "constraint",
        &format!("the gateway needs API_KEY={PLANTED} to start"),
    )]));
    let outcome = extract(&fixture, &model, &chunk(&["we configured the gateway"]));

    assert!(
        stored(&fixture).is_empty(),
        "a credential reached a memory row"
    );
    assert_eq!(outcome.stored(), 0);
    assert_eq!(outcome.rejected.len(), 1);
    assert!(matches!(
        outcome.rejected[0],
        Rejection::Contract(glasshouse::memory::extract::schema::Refusal::Credential(_))
    ));
}

/// The refusal is reported, and the report is not a second copy of the leak.
///
/// An extraction outcome is a value a caller logs or prints. If the refusal
/// quoted the memory it refused, the credential would move from the database
/// into the log — the same leak in a different place.
#[test]
fn a_refusal_never_repeats_the_credential_it_refused() {
    let tmp = tempdir();
    let fixture = Fixture::new(tmp.path(), "alpha");

    let model = Canned::new(reply(&[memory_json(
        "finding",
        "constraint",
        &format!("set API_KEY={PLANTED}"),
    )]));
    let outcome = extract(&fixture, &model, &chunk(&["we configured the gateway"]));

    let rendered = outcome
        .rejected
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        !rendered.contains(PLANTED),
        "the rejection leaked: {rendered}"
    );
    assert!(
        !rendered.contains("API_KEY"),
        "the rejection named the variable"
    );

    let debugged = format!("{outcome:?}");
    assert!(!debugged.contains(PLANTED), "the outcome's Debug leaked");
}

/// One poisoned memory costs one memory. A reply where the middle element
/// carries a credential still stores the two around it.
#[test]
fn a_credential_in_one_memory_does_not_discard_the_others() {
    let tmp = tempdir();
    let fixture = Fixture::new(tmp.path(), "alpha");

    let model = Canned::new(reply(&[
        memory_json("finding", "constraint", "ConPTY reflows long lines"),
        memory_json("finding", "constraint", &format!("API_KEY={PLANTED}")),
        memory_json("todo", "preference", "close the interrupt box"),
    ]));
    let outcome = extract(&fixture, &model, &chunk(&["a busy afternoon"]));

    assert_eq!(outcome.stored(), 2);
    assert_eq!(outcome.rejected.len(), 1);

    let bodies: Vec<String> = stored(&fixture)
        .into_iter()
        .map(|(_, _, body)| body)
        .collect();
    assert!(bodies.iter().any(|b| b.contains("ConPTY reflows")));
    assert!(bodies.iter().any(|b| b.contains("interrupt box")));
    assert!(
        bodies.iter().all(|b| !b.contains(PLANTED)),
        "the credential reached a row"
    );
}

/// The screen runs over the whole element before any field is read, so a
/// credential in a field this contract does not even look at is still caught.
#[test]
fn a_credential_in_a_field_the_contract_ignores_is_still_refused() {
    let tmp = tempdir();
    let fixture = Fixture::new(tmp.path(), "alpha");

    let model = Canned::new(format!(
        r#"{{"memories":[{{"kind":"finding","authority":"constraint",
              "disposition":"accepted","support":"established",
              "confidence":"certain","body":"a clean body",
              "model_scratchpad":"I saw API_KEY={PLANTED} in the log"}}]}}"#
    ));
    let outcome = extract(&fixture, &model, &chunk(&["a busy afternoon"]));

    assert_eq!(
        outcome.stored(),
        0,
        "an unread field carried a credential in"
    );
    assert!(stored(&fixture).is_empty());
}

// -------------------------------------------------------------------------
// Phase 21 — bounded chunks
// -------------------------------------------------------------------------

/// Mutation: delete the `max_total_chars` branch in `SessionChunk::build`.
///
/// The per-entry cap is not a bound on the chunk: a thousand entries each
/// just under it is an unbounded history assembled out of bounded parts,
/// which is exactly what the map's *"rather than entire unbounded session
/// histories"* forbids.
#[test]
fn a_whole_session_history_cannot_reach_the_model() {
    let tmp = tempdir();
    let fixture = Fixture::new(tmp.path(), "alpha");
    let model = Canned::new(reply(&[]));

    let limits = ChunkLimits {
        max_entries: 1_000,
        max_entry_chars: 200,
        max_total_chars: 500,
    };
    let huge = SessionChunk::build(
        "session-alpha",
        Some("a938fcc"),
        (0..5_000).map(|i| format!("turn {i}: {}", "detail ".repeat(20))),
        limits,
    );

    assert!(huge.chars() <= 500, "the chunk itself is not bounded");
    assert!(
        huge.dropped() > 4_000,
        "4900 entries went somewhere unrecorded"
    );

    let memory = fixture.memory();
    let store = memory.store();
    Extractor::new(&store, &model).run(&huge, ExtractionTrigger::TaskCompleted);

    // The prompt carries the contract and the schema, which are fixed, plus
    // the activity, which is not. The bound that matters is on the activity.
    let prompt = model.last_prompt();
    let activity = prompt
        .split_once("## Session")
        .expect("the prompt should name its session")
        .1;
    assert!(
        activity.chars().count() < 2_000,
        "the activity block was {} characters",
        activity.chars().count()
    );
    assert!(
        prompt.contains("earlier entries omitted"),
        "a partial slice must say that it is one"
    );
}

// -------------------------------------------------------------------------
// Phase 21 — failure is non-fatal
// -------------------------------------------------------------------------

/// Mutation: change `Extractor::run` to `expect()` the model's result.
///
/// `run` has no error channel at all, which is the structural form of the
/// map's *"keep memory-extraction failure non-fatal to the coding session"*:
/// there is nothing for a caller on a lifecycle-event path to propagate with
/// `?`.
#[test]
fn an_unavailable_model_is_an_outcome_rather_than_an_error() {
    let tmp = tempdir();
    let fixture = Fixture::new(tmp.path(), "alpha");

    let outcome = extract(
        &fixture,
        &Broken(ModelError::Unavailable),
        &chunk(&["a busy afternoon"]),
    );

    assert_eq!(
        outcome.failure,
        Some(ExtractionFailure::Model(ModelError::Unavailable))
    );
    assert_eq!(outcome.stored(), 0);
    assert!(stored(&fixture).is_empty());
    // The project database is still usable afterwards, which is the property
    // "non-fatal" actually means.
    assert_eq!(
        fixture
            .memory()
            .store()
            .count(MemoryStatus::Active)
            .unwrap(),
        0
    );
}

#[test]
fn every_model_failure_shape_produces_an_outcome() {
    let tmp = tempdir();
    let fixture = Fixture::new(tmp.path(), "alpha");

    for err in [
        ModelError::Unavailable,
        ModelError::Refused,
        ModelError::TimedOut,
        ModelError::Failed {
            phrase: "the gateway refused the connection",
        },
    ] {
        let outcome = extract(&fixture, &Broken(err.clone()), &chunk(&["afternoon"]));
        assert_eq!(outcome.failure, Some(ExtractionFailure::Model(err)));
    }
}

/// Mutation: delete the `catch_unwind` in `Extractor::run`.
///
/// A disposable support job taking a coding session down is the same defect
/// as an error propagating, wearing a worse hat. The panic hook is swapped
/// out for the duration so the deliberate panic does not print a scary
/// backtrace into an otherwise clean test run.
#[test]
fn a_model_that_panics_does_not_take_the_session_with_it() {
    let tmp = tempdir();
    let fixture = Fixture::new(tmp.path(), "alpha");

    let hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    let outcome = extract(&fixture, &Exploding, &chunk(&["a busy afternoon"]));
    std::panic::set_hook(hook);

    assert_eq!(outcome.failure, Some(ExtractionFailure::ModelPanicked));
    assert_eq!(outcome.stored(), 0);
    assert!(stored(&fixture).is_empty());
}

#[test]
fn a_reply_that_is_not_a_document_is_reported_rather_than_guessed_at() {
    let tmp = tempdir();
    let fixture = Fixture::new(tmp.path(), "alpha");

    let outcome = extract(
        &fixture,
        &Canned::new("I could not find anything worth remembering."),
        &chunk(&["a busy afternoon"]),
    );
    assert!(matches!(outcome.failure, Some(ExtractionFailure::Reply(_))));
    assert!(stored(&fixture).is_empty());
}

/// Truncated JSON is the failure a real model produces when it runs out of
/// output budget, so it gets its own case.
#[test]
fn a_truncated_reply_is_a_failure_and_not_a_panic() {
    let tmp = tempdir();
    let fixture = Fixture::new(tmp.path(), "alpha");

    let outcome = extract(
        &fixture,
        &Canned::new(r#"{"memories": [{"kind": "finding", "body": "half a th"#),
        &chunk(&["a busy afternoon"]),
    );
    assert!(outcome.failure.is_some());
    assert!(stored(&fixture).is_empty());
}

#[test]
fn an_empty_chunk_is_reported_without_asking_a_model_at_all() {
    let tmp = tempdir();
    let fixture = Fixture::new(tmp.path(), "alpha");
    let model = Canned::new(reply(&[]));

    let empty = SessionChunk::build("s", None::<String>, Vec::new(), ChunkLimits::default());
    let outcome = extract(&fixture, &model, &empty);

    assert_eq!(outcome.failure, Some(ExtractionFailure::NothingToExtract));
    assert!(
        model.seen.lock().unwrap().is_empty(),
        "a model was billed for an empty session"
    );
}

// -------------------------------------------------------------------------
// Phase 21 — provenance, and the record of who did the extracting
// -------------------------------------------------------------------------

#[test]
fn every_extracted_memory_carries_the_session_and_commit_it_came_from() {
    let tmp = tempdir();
    let fixture = Fixture::new(tmp.path(), "alpha");

    let model = Canned::new(reply(&[memory_json(
        "finding",
        "constraint",
        "ConPTY reflows long lines",
    )]));
    extract(&fixture, &model, &chunk(&["we measured ConPTY"]));

    let memory = fixture.memory();
    let records = memory
        .store()
        .search("ConPTY", SearchScope::Current, 10)
        .unwrap();
    assert_eq!(records.len(), 1);
    assert_eq!(
        records[0].source_session_id.as_deref(),
        Some("session-alpha")
    );
    assert_eq!(records[0].source_commit.as_deref(), Some("a938fcc"));
}

/// Phase 39 requires Glasshouse to *"record which resource performed
/// important memory extraction or classification for debugging"*. This is
/// where that lands until Phase 39 exists to fill it in.
#[test]
fn the_outcome_records_which_resource_did_the_extracting_and_why() {
    let tmp = tempdir();
    let fixture = Fixture::new(tmp.path(), "alpha");
    let model = Canned::new(reply(&[]));

    let memory = fixture.memory();
    let store = memory.store();
    let outcome = Extractor::new(&store, &model)
        .run(&chunk(&["afternoon"]), ExtractionTrigger::BeforeCompaction);

    assert_eq!(outcome.model, "fake/canned");
    assert_eq!(outcome.trigger, ExtractionTrigger::BeforeCompaction);
    assert_eq!(outcome.session_id, "session-alpha");
    assert_eq!(outcome.commit.as_deref(), Some("a938fcc"));
}

// -------------------------------------------------------------------------
// Phase 21 — no duplicates when nothing materially changed
// -------------------------------------------------------------------------

/// Mutation: delete the `seen.contains(&key)` branch in `store_one`.
#[test]
fn a_memory_the_project_already_holds_is_not_stored_again() {
    let tmp = tempdir();
    let fixture = Fixture::new(tmp.path(), "alpha");

    let body = "ConPTY renders into a screen buffer and reflows long lines";
    let model = Canned::new(reply(&[memory_json("finding", "constraint", body)]));

    let first = extract(&fixture, &model, &chunk(&["we measured ConPTY"]));
    assert_eq!(first.stored(), 1);
    assert_eq!(first.duplicates, 0);

    let second = extract(&fixture, &model, &chunk(&["we measured ConPTY again"]));
    assert_eq!(second.stored(), 0, "the same memory was stored twice");
    assert_eq!(second.duplicates, 1);

    assert_eq!(stored(&fixture).len(), 1);
}

/// Case, whitespace and a trailing full stop are presentation. *"Nothing
/// materially changed"* is the map's phrase, and none of those is material.
///
/// The second body differs from the first in **all three** ways on purpose.
/// The first version of this test varied only whitespace and the full stop, so
/// a mutation removing `normalize`'s `to_lowercase` survived it — the test was
/// asserting a property it only half exercised.
#[test]
fn a_reformatted_duplicate_is_still_a_duplicate() {
    let tmp = tempdir();
    let fixture = Fixture::new(tmp.path(), "alpha");

    let first = Canned::new(reply(&[memory_json(
        "finding",
        "constraint",
        "ConPTY reflows long lines",
    )]));
    extract(&fixture, &first, &chunk(&["monday"]));

    let second = Canned::new(reply(&[memory_json(
        "finding",
        "constraint",
        "conpty  REFLOWS   long lines.",
    )]));
    let outcome = extract(&fixture, &second, &chunk(&["tuesday"]));

    assert_eq!(outcome.duplicates, 1);
    assert_eq!(stored(&fixture).len(), 1);
}

/// The other direction, and the one that makes the rule useful rather than
/// merely safe: something that *did* materially change is stored.
#[test]
fn a_materially_different_memory_is_stored_beside_the_old_one() {
    let tmp = tempdir();
    let fixture = Fixture::new(tmp.path(), "alpha");

    let first = Canned::new(reply(&[memory_json(
        "finding",
        "constraint",
        "ConPTY reflows long lines",
    )]));
    extract(&fixture, &first, &chunk(&["monday"]));

    let second = Canned::new(reply(&[memory_json(
        "finding",
        "constraint",
        "a Unix pty is a byte pipe and does not reflow",
    )]));
    let outcome = extract(&fixture, &second, &chunk(&["tuesday"]));

    assert_eq!(outcome.stored(), 1);
    assert_eq!(outcome.duplicates, 0);
    assert_eq!(stored(&fixture).len(), 2);
}

/// A model that repeats itself inside one reply is deduplicated too. The
/// duplicate check reads the store *and* what this run has already added, so
/// it does not need a second pass to notice.
#[test]
fn a_reply_that_repeats_itself_stores_one_memory() {
    let tmp = tempdir();
    let fixture = Fixture::new(tmp.path(), "alpha");

    let body = "ConPTY reflows long lines";
    let model = Canned::new(reply(&[
        memory_json("finding", "constraint", body),
        memory_json("finding", "constraint", body),
        memory_json("finding", "constraint", body),
    ]));
    let outcome = extract(&fixture, &model, &chunk(&["monday"]));

    assert_eq!(outcome.stored(), 1);
    assert_eq!(outcome.duplicates, 2);
}

// -------------------------------------------------------------------------
// Phase 21A — conservative classification, end to end
// -------------------------------------------------------------------------

/// Mutation: replace `authority::conservative(...).stored` with the declared
/// authority in `store_one`.
#[test]
fn a_model_cannot_write_an_invariant_into_this_project() {
    let tmp = tempdir();
    let fixture = Fixture::new(tmp.path(), "alpha");

    let model = Canned::new(reply(&[r#"{"kind":"decision","authority":"invariant",
             "disposition":"accepted","support":"established","confidence":"certain",
             "rationale":"the whole design depends on it",
             "body":"Every interactive session is backed by a real harness."}"#
        .to_owned()]));
    let outcome = extract(&fixture, &model, &chunk(&["we settled the architecture"]));

    assert_eq!(outcome.stored(), 1);
    let rows = stored(&fixture);
    assert_eq!(rows[0].1, Some(MemoryAuthority::Constraint));

    // The demotion is reported, not silent — otherwise a reviewer could not
    // find the memories worth promoting by hand.
    assert_eq!(outcome.lowered.len(), 1);
    assert_eq!(outcome.lowered[0].1.declared, MemoryAuthority::Invariant);
    assert_eq!(outcome.lowered[0].1.stored, MemoryAuthority::Constraint);
    assert!(!outcome.lowered[0].1.reasons.is_empty());
}

/// Phase 21A: *distinguish an accepted decision from an idea that was merely
/// discussed enthusiastically.*
#[test]
fn an_enthusiastic_proposal_is_stored_as_an_idea_and_is_not_binding() {
    let tmp = tempdir();
    let fixture = Fixture::new(tmp.path(), "alpha");

    let model = Canned::new(reply(&[r#"{"kind":"feature","authority":"decision",
             "disposition":"proposed","support":"established","confidence":"certain",
             "body":"A web dashboard would make routing decisions visible."}"#
        .to_owned()]));
    extract(
        &fixture,
        &model,
        &chunk(&["an excited late-night discussion"]),
    );

    let rows = stored(&fixture);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].1, Some(MemoryAuthority::Idea));
    assert!(
        !rows[0].1.unwrap().is_binding(),
        "an idea discussed once became a binding instruction"
    );
}

#[test]
fn the_rationale_survives_into_the_stored_body() {
    let tmp = tempdir();
    let fixture = Fixture::new(tmp.path(), "alpha");

    let model = Canned::new(reply(&[r#"{"kind":"decision","authority":"constraint",
             "disposition":"accepted","support":"established","confidence":"certain",
             "rationale":"no async runtime is in the dependency set",
             "body":"Use blocking threads for the gateway."}"#
        .to_owned()]));
    extract(&fixture, &model, &chunk(&["we settled the gateway"]));

    let rows = stored(&fixture);
    assert_eq!(rows.len(), 1);
    assert!(
        rows[0]
            .2
            .starts_with("Use blocking threads for the gateway.")
    );
    assert!(rows[0].2.contains(RATIONALE_MARKER.trim_start()));
    assert!(rows[0].2.contains("no async runtime"));

    // And it is searchable, which is the point of folding it in rather than
    // dropping it: a search for the reason finds the decision.
    let memory = fixture.memory();
    let found = memory
        .store()
        .search("async runtime", SearchScope::Current, 10)
        .unwrap();
    assert_eq!(found.len(), 1);
}

// -------------------------------------------------------------------------
// The storage layer's own guard still applies through extraction
// -------------------------------------------------------------------------

/// Phase 20's admission guard is not bypassed by arriving through the
/// extractor: a step-by-step plan filed as a todo is still refused, and the
/// refusal is reported as a rejection rather than crashing the run.
#[test]
fn the_admission_guard_still_refuses_a_plan_that_arrives_through_extraction() {
    let tmp = tempdir();
    let fixture = Fixture::new(tmp.path(), "alpha");

    let plan = "1. open the file\\n2. change the constant\\n3. run the tests\\n4. commit";
    let model = Canned::new(reply(&[
        memory_json("todo", "preference", plan),
        memory_json("finding", "constraint", "ConPTY reflows long lines"),
    ]));
    let outcome = extract(&fixture, &model, &chunk(&["we planned the change"]));

    assert_eq!(outcome.stored(), 1, "the plan was stored as durable memory");
    assert_eq!(outcome.rejected.len(), 1);
    assert!(matches!(outcome.rejected[0], Rejection::Store(_)));
    assert!(
        outcome.rejected[0]
            .to_string()
            .contains("step-by-step plan")
    );
}

// -------------------------------------------------------------------------
// Project isolation
// -------------------------------------------------------------------------

/// Two fixtures over one base are two real projects. Extraction is bound to
/// the project whose store it was handed, and there is no argument that could
/// point it at another.
#[test]
fn extraction_reaches_only_the_project_it_was_opened_for() {
    let tmp = tempdir();
    let alpha = Fixture::new(tmp.path(), "alpha");
    let beta = Fixture::new(tmp.path(), "beta");

    let model = Canned::new(reply(&[memory_json(
        "finding",
        "constraint",
        "alpha learned something",
    )]));
    let outcome = extract(&alpha, &model, &chunk(&["alpha's afternoon"]));
    assert_eq!(outcome.stored(), 1);

    assert_eq!(stored(&alpha).len(), 1);
    assert!(
        stored(&beta).is_empty(),
        "a memory crossed into another project"
    );

    let beta_memory = beta.memory();
    assert!(
        beta_memory
            .store()
            .search("alpha learned", SearchScope::Historical, 10)
            .unwrap()
            .is_empty()
    );
}

/// The duplicate check reads memories, so it is a read path, so it is a place
/// project scope could leak. The same body in two projects is stored twice —
/// once each — because neither project can see the other's.
#[test]
fn the_duplicate_check_does_not_see_another_projects_memories() {
    let tmp = tempdir();
    let alpha = Fixture::new(tmp.path(), "alpha");
    let beta = Fixture::new(tmp.path(), "beta");

    let body = "ConPTY reflows long lines";
    let model = Canned::new(reply(&[memory_json("finding", "constraint", body)]));

    let first = extract(&alpha, &model, &chunk(&["monday"]));
    let second = extract(&beta, &model, &chunk(&["monday"]));

    assert_eq!(first.stored(), 1);
    assert_eq!(
        second.stored(),
        1,
        "beta's extraction was deduplicated against alpha's memory"
    );
    assert_eq!(second.duplicates, 0);
}
