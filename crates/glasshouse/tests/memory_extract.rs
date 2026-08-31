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
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex};

use clap::Parser;

use glasshouse::events::{EventBus, EventLog, LifecycleEvent, TurnOutcome};
use glasshouse::memory::extract::chunk::{ChunkLimits, SessionChunk};
use glasshouse::memory::extract::lifecycle;
use glasshouse::memory::extract::{
    ExtractionFailure, ExtractionModel, ExtractionOutcome, ExtractionTrigger, Extractor,
    ModelError, Prompt, Rejection,
};
use glasshouse::memory::{
    DecisionProvenance, MemoryAuthority, MemoryKind, MemoryStatus, ProjectMemory, ProjectPhase,
    SourceEvents, search::SearchScope,
};
use glasshouse::session::SessionId;
use glasshouse::session::store::Clock;
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

/// Like [`memory_json`], but with a `subject` — the field the reply schema
/// asks the model for and that `memory_json` never emits.
fn memory_json_with_subject(kind: &str, authority: &str, subject: &str, body: &str) -> String {
    format!(
        r#"{{"kind":"{kind}","authority":"{authority}","disposition":"accepted",
             "support":"established","confidence":"certain",
             "subject":"{subject}","body":"{body}"}}"#
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

/// A clock that advances by `step` each call, for [`EventBus`]'s history —
/// copied from `tests/events_log.rs`'s own helper of the same shape.
fn ticking(start: i64, step: i64) -> Clock {
    let next = AtomicI64::new(start);
    Arc::new(move || next.fetch_add(step, Ordering::SeqCst))
}

/// Append `events` for `session` through a real [`EventLog`], returning what
/// was written, oldest first, exactly as [`EventLog::recent_for_session`]
/// would read it back.
fn log_events(log: &EventLog, session: &SessionId, events: Vec<LifecycleEvent>) {
    let bus = EventBus::with_history_and_clock(events.len().max(1), ticking(1_700_000_000, 1));
    for event in events {
        let recorded = bus.publish(session, event);
        log.append(&recorded, None).unwrap();
    }
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

/// The duplicate check reads `existing_bodies`' `"{subject}: {body}"` key, so it
/// must be built the same way here. Without a subject this passed while the
/// production path was dead for every memory that had one.
///
/// Mutation: revert `store_one`'s key to `normalize(&body)`.
#[test]
fn a_memory_with_a_subject_that_the_project_already_holds_is_not_stored_again() {
    let tmp = tempdir();
    let fixture = Fixture::new(tmp.path(), "alpha");

    let model = Canned::new(reply(&[memory_json_with_subject(
        "finding",
        "constraint",
        "ConPTY reflows",
        "ConPTY renders into a screen buffer and reflows long lines",
    )]));

    let first = extract(&fixture, &model, &chunk(&["we measured ConPTY"]));
    assert_eq!(first.stored(), 1);

    let second = extract(&fixture, &model, &chunk(&["we measured ConPTY again"]));
    assert_eq!(
        second.duplicates, 1,
        "a memory with a subject was not recognised"
    );
    assert_eq!(second.stored(), 0, "the same memory was stored twice");
    assert_eq!(stored(&fixture).len(), 1);
}

/// The over-match direction of the same bug: two memories in one reply with
/// the same body but *different* subjects must not collapse into one, since
/// they are keyed on subject and body together.
#[test]
fn two_memories_with_the_same_body_but_different_subjects_are_both_stored() {
    let tmp = tempdir();
    let fixture = Fixture::new(tmp.path(), "alpha");

    let body = "the store retries on a locked database";
    let model = Canned::new(reply(&[
        memory_json_with_subject("finding", "constraint", "SQLite retries", body),
        memory_json_with_subject("finding", "constraint", "Postgres retries", body),
    ]));

    let outcome = extract(&fixture, &model, &chunk(&["monday"]));

    assert_eq!(
        outcome.stored(),
        2,
        "different-subject memories were collapsed"
    );
    assert_eq!(outcome.duplicates, 0);
    assert_eq!(stored(&fixture).len(), 2);
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

/// Migration 6 gave the rationale its own column and rebuilt the FTS5 index
/// over it, so this asserts the two halves that used to be one: the body is
/// *only* the body, and the reason is still findable by searching for it.
#[test]
fn the_rationale_is_stored_beside_the_body_and_is_still_searchable() {
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
    assert_eq!(rows[0].2, "Use blocking threads for the gateway.");
    assert!(
        !rows[0].2.contains("async runtime"),
        "the rationale must no longer be folded into the body"
    );

    // The reason is in its own column, and still in the index: that is what
    // migration 6's FTS5 rebuild is for, and dropping `rationale` from the
    // virtual table would leave this the only thing that notices.
    let memory = fixture.memory();
    let store = memory.store();
    let found = store
        .search("async runtime", SearchScope::Current, 10)
        .unwrap();
    assert_eq!(found.len(), 1);
    assert_eq!(
        found[0].provenance.rationale.as_deref(),
        Some("no async runtime is in the dependency set")
    );
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

// -------------------------------------------------------------------------
// Phase 21B — provenance, end to end: reply in, columns out
// -------------------------------------------------------------------------

/// Line: "store the originating session and event references so extracted
/// memory retains provenance" plus the nine Phase 21B storage lines, all in
/// one round trip.
///
/// Mutation: drop one field from the `DecisionProvenance` literal in
/// `schema::judge`, or delete `.with_provenance(...)` from
/// `Extractor::store_one`.
#[test]
fn a_reply_carrying_every_phase_21b_field_lands_in_every_column() {
    let tmp = tempdir();
    let fixture = Fixture::new(tmp.path(), "alpha");

    let model = Canned::new(reply(&[r#"{"kind":"decision","authority":"constraint",
             "disposition":"accepted","support":"established","confidence":"certain",
             "body":"Store checkpoints in SQLite.",
             "rationale":"the project database is already open",
             "project_phase":"alpha",
             "problem":"handing a session's context to a fresh one",
             "assumptions":"one machine holds the project",
             "scale_assumptions":"tens of sessions, not thousands",
             "security_assumptions":"the database file is owner-only",
             "compatibility_assumptions":"SQLite ships with the binary",
             "operational_assumptions":"single-instance, no daemon",
             "evidence":"the size cap is enforced by a CHECK",
             "source_excerpt":"we agreed checkpoints go in the project db"}"#
        .to_owned()]));
    let outcome = extract(
        &fixture,
        &model,
        &chunk(&["we settled the checkpoint format"]),
    );

    assert_eq!(outcome.stored(), 1);
    let memory = fixture.memory();
    let record = memory.store().get(&outcome.recorded[0]).unwrap().unwrap();

    let expected = DecisionProvenance {
        rationale: Some("the project database is already open".to_owned()),
        project_phase: Some(ProjectPhase::Alpha),
        problem: Some("handing a session's context to a fresh one".to_owned()),
        assumptions: Some("one machine holds the project".to_owned()),
        scale_assumptions: Some("tens of sessions, not thousands".to_owned()),
        security_assumptions: Some("the database file is owner-only".to_owned()),
        compatibility_assumptions: Some("SQLite ships with the binary".to_owned()),
        operational_assumptions: Some("single-instance, no daemon".to_owned()),
        evidence: Some("the size cap is enforced by a CHECK".to_owned()),
        source_excerpt: Some("we agreed checkpoints go in the project db".to_owned()),
    };
    assert_eq!(record.provenance, expected);
    assert_eq!(record.body, "Store checkpoints in SQLite.");

    // The fold is gone: none of the ten provenance values are duplicated into
    // the body text migration 6's FTS5 rebuild indexes as `body`.
    for value in [
        "the project database is already open",
        "handing a session's context to a fresh one",
        "one machine holds the project",
        "tens of sessions, not thousands",
        "the database file is owner-only",
        "SQLite ships with the binary",
        "single-instance, no daemon",
        "the size cap is enforced by a CHECK",
        "we agreed checkpoints go in the project db",
    ] {
        assert!(
            !record.body.contains(value),
            "the body still carries `{value}`, which belongs to provenance"
        );
    }
}

/// The map says "when known" of every one of the nine fields, so a reply
/// naming none of them still stores the memory rather than being refused for
/// an invented value it never had.
#[test]
fn a_reply_carrying_no_phase_21b_field_still_stores_the_memory_with_everything_none() {
    let tmp = tempdir();
    let fixture = Fixture::new(tmp.path(), "alpha");

    let model = Canned::new(reply(&[memory_json(
        "finding",
        "historical",
        "ConPTY reflows long lines",
    )]));
    let outcome = extract(&fixture, &model, &chunk(&["we watched ConPTY behave"]));

    assert_eq!(outcome.stored(), 1);
    let memory = fixture.memory();
    let record = memory.store().get(&outcome.recorded[0]).unwrap().unwrap();
    assert_eq!(record.provenance, DecisionProvenance::default());
}

// -------------------------------------------------------------------------
// Phase 21 — extraction over a session's recorded events
// -------------------------------------------------------------------------

/// Line: "feed the extractor bounded session/event chunks rather than entire
/// unbounded session histories" — the **event** half, read back through a
/// real [`EventLog`] rather than a file of activity.
///
/// Mutation: in `lifecycle::chunk_for_session`, use `events` instead of
/// `window` when computing the range (i.e. always claim the whole slice
/// handed in, regardless of what the chunk's own budget kept).
#[test]
fn extraction_over_a_sessions_recorded_events_stores_the_source_event_range() {
    let tmp = tempdir();
    let fixture = Fixture::new(tmp.path(), "alpha");
    let session = SessionId::new("session-in-the-log");

    let log = EventLog::open(&fixture.runtime).unwrap();
    log_events(
        &log,
        &session,
        (0..5)
            .map(|_| LifecycleEvent::TurnEnded {
                outcome: TurnOutcome::Completed,
            })
            .collect(),
    );

    let events = log
        .recent_for_session(&session, lifecycle::EVENT_WINDOW)
        .unwrap();
    assert_eq!(
        events.len(),
        5,
        "all five recorded events should be read back"
    );
    let expected = SourceEvents::new(events.first().unwrap().seq, events.last().unwrap().seq);

    let chunk =
        lifecycle::chunk_for_session(&session, &events, Some("a938fcc"), ChunkLimits::default());

    let model = Canned::new(reply(&[memory_json(
        "finding",
        "historical",
        "the session ended five turns cleanly",
    )]));
    let outcome = extract(&fixture, &model, &chunk);
    assert_eq!(outcome.stored(), 1);

    let memory = fixture.memory();
    let record = memory.store().get(&outcome.recorded[0]).unwrap().unwrap();
    assert_eq!(
        record.source_session_id.as_deref(),
        Some("session-in-the-log")
    );
    assert_eq!(record.source_events, expected);
}

/// The property `lifecycle.rs`'s own module documentation calls "the
/// difference between provenance and a guess": when the chunk's budget drops
/// the oldest events, the stored range must start *after* them, not at the
/// beginning of the whole slice that was handed in.
///
/// `lifecycle.rs`'s unit tests cover the arithmetic in isolation; this proves
/// it survives the trip through a real store.
///
/// Mutation: same as above — make `chunk_for_session` use the whole `events`
/// slice for its range instead of the surviving window.
#[test]
fn a_chunk_whose_budget_dropped_the_oldest_events_does_not_claim_them_as_source() {
    let tmp = tempdir();
    let fixture = Fixture::new(tmp.path(), "alpha");
    let session = SessionId::new("session-with-a-long-history");

    let log = EventLog::open(&fixture.runtime).unwrap();
    log_events(
        &log,
        &session,
        (0..20)
            .map(|_| LifecycleEvent::TurnEnded {
                outcome: TurnOutcome::Completed,
            })
            .collect(),
    );

    let events = log
        .recent_for_session(&session, lifecycle::EVENT_WINDOW)
        .unwrap();
    assert_eq!(events.len(), 20);

    let tight = ChunkLimits {
        max_entries: 4,
        max_entry_chars: 2_000,
        max_total_chars: 24_000,
    };
    let chunk = lifecycle::chunk_for_session(&session, &events, None, tight);
    assert_eq!(
        chunk.entries().len(),
        4,
        "the tight budget should have bound the chunk"
    );

    // Only the newest four survived; the range must start after the sixteen
    // that were dropped, not at the head of the whole session.
    let surviving = &events[events.len() - 4..];
    let expected = SourceEvents::new(
        surviving.first().unwrap().seq,
        surviving.last().unwrap().seq,
    );
    assert_ne!(
        expected,
        SourceEvents::new(events.first().unwrap().seq, events.last().unwrap().seq),
        "the test is only meaningful if the surviving range differs from the whole slice"
    );

    let model = Canned::new(reply(&[memory_json(
        "finding",
        "historical",
        "only the recent turns are what the model actually saw",
    )]));
    let outcome = extract(&fixture, &model, &chunk);
    assert_eq!(outcome.stored(), 1);

    let memory = fixture.memory();
    let record = memory.store().get(&outcome.recorded[0]).unwrap().unwrap();
    assert_eq!(
        record.source_events, expected,
        "the stored range must not claim events the budget dropped"
    );
}

/// There is no API to put a conversation payload into an event-derived
/// chunk — [`LifecycleEvent`] has no field a hook payload could reach, per
/// `lifecycle.rs`'s own module documentation — so the property worth pinning
/// is positive: every entry a chunk built from events carries is one of
/// `lifecycle::describe`'s own sentences, built from a [`LoggedEvent`] alone.
///
/// This is worth pinning because a future writer who adds a payload column to
/// `LifecycleEvent` (or a field a hook payload could fill) and then threads it
/// into `describe` would silently turn this bounded, safe-by-construction
/// source into one that can carry conversation text — and nothing else here
/// would catch that.
#[test]
fn an_event_chunk_carries_no_conversation_only_lifecycles_own_sentences() {
    let tmp = tempdir();
    let fixture = Fixture::new(tmp.path(), "alpha");
    let session = SessionId::new("session-shape-only");

    let log = EventLog::open(&fixture.runtime).unwrap();
    log_events(
        &log,
        &session,
        vec![
            LifecycleEvent::SessionStarted,
            LifecycleEvent::TurnStarted,
            LifecycleEvent::TurnEnded {
                outcome: TurnOutcome::Completed,
            },
        ],
    );

    let events = log
        .recent_for_session(&session, lifecycle::EVENT_WINDOW)
        .unwrap();
    let chunk = lifecycle::chunk_for_session(&session, &events, None, ChunkLimits::default());

    assert_eq!(chunk.entries().len(), events.len());
    for (entry, event) in chunk.entries().iter().zip(events.iter()) {
        assert_eq!(
            entry,
            &lifecycle::describe(event),
            "a chunk entry was not one of lifecycle::describe's own sentences"
        );
    }

    let prompt = Prompt::build(&chunk, &[]);
    assert!(prompt.as_str().contains("the session's process started"));
    assert!(prompt.as_str().contains("the harness started working"));
    assert!(prompt.as_str().contains("a turn ended, completed"));
}

// -------------------------------------------------------------------------
// Phase 21 — the duplicate key after the rationale moved out of the body
// -------------------------------------------------------------------------

/// Migration 6 moved the rationale out of the body, and the duplicate check
/// (`extract::normalize` / `extract::duplicate_key`) has only ever read the
/// body (and subject). So two replies with the same body and a **different**
/// rationale are duplicates of each other: the second is not stored, and the
/// first's rationale is left exactly as it was.
///
/// **Judgement: this is right, but only barely, and it is worth someone
/// deciding on purpose rather than by omission.** Phase 21's duplicate rule
/// is "avoid duplicating an existing active memory when nothing materially
/// changed" — the same phrase `Extractor::store_one`'s own doc comment quotes.
/// A changed rationale for the same accepted decision is not a new fact about
/// the project; it is often a model re-deriving *why* something already true
/// is true, which is not "material change" in the sense the map means. But it
/// is also information that is silently thrown away: nothing here supersedes
/// the old memory with the new rationale, or merges the two, so a better
/// rationale offered by a later extraction is lost rather than recorded. If
/// Phase 21C's revalidation work ever wants "the most recent reasoning for a
/// standing decision" to be retrievable, this is the exact case it would need
/// to handle differently — probably by updating the existing row's rationale
/// in place rather than either duplicating or discarding.
#[test]
fn two_replies_with_the_same_body_and_different_rationales_are_duplicates_of_each_other() {
    let tmp = tempdir();
    let fixture = Fixture::new(tmp.path(), "alpha");

    let body = "Use blocking threads for the gateway.";
    let first = Canned::new(reply(&[format!(
        r#"{{"kind":"decision","authority":"constraint","disposition":"accepted",
             "support":"established","confidence":"certain","body":"{body}",
             "rationale":"no async runtime is in the dependency set"}}"#
    )]));
    let outcome1 = extract(&fixture, &first, &chunk(&["monday"]));
    assert_eq!(outcome1.stored(), 1);

    let second = Canned::new(reply(&[format!(
        r#"{{"kind":"decision","authority":"constraint","disposition":"accepted",
             "support":"established","confidence":"certain","body":"{body}",
             "rationale":"blocking threads are simpler to reason about here"}}"#
    )]));
    let outcome2 = extract(&fixture, &second, &chunk(&["tuesday"]));

    assert_eq!(
        outcome2.stored(),
        0,
        "a changed rationale alone stored the same decision a second time"
    );
    assert_eq!(outcome2.duplicates, 1);

    let memory = fixture.memory();
    let store = memory.store();
    let rows = store.with_status(MemoryStatus::Active, 10).unwrap();
    assert_eq!(
        rows.len(),
        1,
        "the duplicate must not have created a second row"
    );
    assert_eq!(
        rows[0].provenance.rationale.as_deref(),
        Some("no async runtime is in the dependency set"),
        "a skipped duplicate must not overwrite the rationale the first row recorded"
    );
}
