//! Acceptance tests for the extraction contract (Phase 21), exercised through
//! its public API the way a caller reaches it.
//!
//! An integration test on purpose, the same way `memory_store.rs` is: every
//! path here goes through `glasshouse::bootstrap`, so the migration, the
//! project binding and the triggers are all in play rather than mocked away.

use std::path::Path;
use std::sync::Mutex;

use glasshouse::memory::extract::chunk::{ChunkLimits, SessionChunk};
use glasshouse::memory::extract::schema::{
    self, Confidence, Disposition, ExtractedMemory, MAX_BODY_CHARS, MAX_RATIONALE_CHARS,
    MAX_SUBJECT_CHARS, PROMPT_CONTRACT, RATIONALE_MARKER, RESPONSE_SCHEMA, Refusal, Support,
    Verdict,
};
use glasshouse::memory::extract::{
    ExtractionFailure, ExtractionModel, ExtractionTrigger, Extractor, ModelError, Prompt, Rejection,
};
use glasshouse::memory::{MemoryAuthority, MemoryKind, MemoryStatus, ProjectMemory};
use glasshouse::{Cli, Runtime};

use clap::Parser;

/// A bootstrapped project inside `base`. Copied from `memory_store.rs`'s
/// `Fixture` shape, for the same reason it exists there: the migration, the
/// project binding and the triggers should all be in play.
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

/// Answers with a fixed reply, and records the prompt it was given.
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

fn chunk_of(activity: &[&str]) -> SessionChunk {
    SessionChunk::build(
        "session-9",
        Some("a938fcc"),
        activity.iter().map(|s| (*s).to_owned()),
        ChunkLimits::default(),
    )
}

/// A well-formed memory as a JSON object literal, with every field
/// overridable so each test only states what it cares about.
struct MemoryJson {
    kind: &'static str,
    authority: &'static str,
    disposition: &'static str,
    support: &'static str,
    confidence: &'static str,
    subject: Option<&'static str>,
    body: String,
    rationale: Option<&'static str>,
}

impl MemoryJson {
    fn new(kind: &'static str, body: impl Into<String>) -> Self {
        Self {
            kind,
            authority: "historical",
            disposition: "accepted",
            support: "established",
            confidence: "certain",
            subject: None,
            body: body.into(),
            rationale: None,
        }
    }

    fn authority(mut self, authority: &'static str) -> Self {
        self.authority = authority;
        self
    }

    fn disposition(mut self, disposition: &'static str) -> Self {
        self.disposition = disposition;
        self
    }

    fn support(mut self, support: &'static str) -> Self {
        self.support = support;
        self
    }

    fn subject(mut self, subject: &'static str) -> Self {
        self.subject = Some(subject);
        self
    }

    fn rationale(mut self, rationale: &'static str) -> Self {
        self.rationale = Some(rationale);
        self
    }

    fn to_json(&self) -> String {
        let mut fields = vec![
            format!("\"kind\":{}", quote(self.kind)),
            format!("\"authority\":{}", quote(self.authority)),
            format!("\"disposition\":{}", quote(self.disposition)),
            format!("\"support\":{}", quote(self.support)),
            format!("\"confidence\":{}", quote(self.confidence)),
            format!("\"body\":{}", quote(&self.body)),
        ];
        if let Some(subject) = self.subject {
            fields.push(format!("\"subject\":{}", quote(subject)));
        }
        if let Some(rationale) = self.rationale {
            fields.push(format!("\"rationale\":{}", quote(rationale)));
        }
        format!("{{{}}}", fields.join(","))
    }
}

fn quote(value: &str) -> String {
    serde_json::to_string(value).unwrap()
}

fn envelope(memories: &[MemoryJson]) -> String {
    let items: Vec<String> = memories.iter().map(MemoryJson::to_json).collect();
    format!("{{\"memories\":[{}]}}", items.join(","))
}

// -------------------------------------------------------------------------
// A. Round-trip, through the whole pipeline.
// -------------------------------------------------------------------------

/// Line: "classify every emitted memory into one supported memory kind."
///
/// One well-formed memory of each of the six kinds ends up as six rows in the
/// store, each with the right kind, read back through `MemoryStore` rather
/// than trusted from the outcome alone.
#[test]
fn one_memory_of_each_kind_ends_up_as_six_rows_of_the_right_kind() {
    let tmp = tempdir();
    let fixture = Fixture::new(tmp.path(), "alpha");
    let memory = fixture.memory();
    let store = memory.store();

    let memories: Vec<MemoryJson> = MemoryKind::ALL
        .iter()
        .map(|kind| {
            MemoryJson::new(
                kind.as_str(),
                format!("a durable {kind} learned this session"),
            )
            .authority(if *kind == MemoryKind::FailedAttempt {
                "historical"
            } else {
                "preference"
            })
            .disposition(if *kind == MemoryKind::FailedAttempt {
                "abandoned"
            } else {
                "accepted"
            })
        })
        .collect();

    let model = Canned::new(envelope(&memories));
    let chunk = chunk_of(&["we did some work"]);
    let outcome = Extractor::new(&store, &model).run(&chunk, ExtractionTrigger::Manual);

    assert_eq!(outcome.stored(), 6, "every kind should have been stored");
    assert!(outcome.rejected.is_empty());

    let mut kinds_seen: Vec<MemoryKind> = outcome
        .recorded
        .iter()
        .map(|id| store.get(id).unwrap().unwrap().kind)
        .collect();
    kinds_seen.sort_by_key(|k| k.as_str().to_owned());

    let mut expected: Vec<MemoryKind> = MemoryKind::ALL.to_vec();
    expected.sort_by_key(|k| k.as_str().to_owned());

    assert_eq!(kinds_seen, expected);
}

/// Every stored memory carries the chunk's session id and commit as its
/// provenance.
#[test]
fn every_stored_memory_carries_the_chunks_session_and_commit() {
    let tmp = tempdir();
    let fixture = Fixture::new(tmp.path(), "alpha");
    let memory = fixture.memory();
    let store = memory.store();

    let model = Canned::new(envelope(&[MemoryJson::new(
        "finding",
        "ConPTY renders into a screen buffer",
    )]));
    let chunk = chunk_of(&["we observed ConPTY behaviour"]);
    let outcome = Extractor::new(&store, &model).run(&chunk, ExtractionTrigger::Manual);

    assert_eq!(outcome.stored(), 1);
    let record = store.get(&outcome.recorded[0]).unwrap().unwrap();
    assert_eq!(record.source_session_id.as_deref(), Some("session-9"));
    assert_eq!(record.source_commit.as_deref(), Some("a938fcc"));
}

/// Line: "preserve concise rationale when a decision's rationale is
/// important."
///
/// Subject, body and rationale survive intact, and the rationale arrives
/// folded into the stored body at `RATIONALE_MARKER`.
#[test]
fn subject_body_and_rationale_survive_with_the_rationale_folded_into_the_body() {
    let tmp = tempdir();
    let fixture = Fixture::new(tmp.path(), "alpha");
    let memory = fixture.memory();
    let store = memory.store();

    let model = Canned::new(envelope(&[MemoryJson::new(
        "decision",
        "Use blocking threads for the pty reader",
    )
    .authority("constraint")
    .subject("pty reader threading")
    .rationale("no async runtime is in the dependency set")]));
    let chunk = chunk_of(&["we chose blocking threads"]);
    let outcome = Extractor::new(&store, &model).run(&chunk, ExtractionTrigger::Manual);

    assert_eq!(outcome.stored(), 1);
    let record = store.get(&outcome.recorded[0]).unwrap().unwrap();
    assert_eq!(record.subject.as_deref(), Some("pty reader threading"));
    assert_eq!(
        record.body,
        format!(
            "Use blocking threads for the pty reader{RATIONALE_MARKER}no async runtime is in \
             the dependency set"
        )
    );
}

/// Line: Phase 39's "record which resource performed extraction".
#[test]
fn the_outcome_names_the_model_and_the_trigger_it_ran_under() {
    let tmp = tempdir();
    let fixture = Fixture::new(tmp.path(), "alpha");
    let memory = fixture.memory();
    let store = memory.store();

    let model = Canned::new(envelope(&[]));
    let chunk = chunk_of(&["nothing decided yet"]);
    let outcome = Extractor::new(&store, &model).run(&chunk, ExtractionTrigger::BeforeCompaction);

    assert_eq!(outcome.model, "fake/canned");
    assert_eq!(outcome.trigger, ExtractionTrigger::BeforeCompaction);
}

/// A reply with an empty `memories` array stores nothing and reports no
/// failure — finding nothing is a valid answer, not an error.
#[test]
fn an_empty_memories_array_stores_nothing_and_is_not_a_failure() {
    let tmp = tempdir();
    let fixture = Fixture::new(tmp.path(), "alpha");
    let memory = fixture.memory();
    let store = memory.store();

    let model = Canned::new(envelope(&[]));
    let chunk = chunk_of(&["we looked around and found nothing worth keeping"]);
    let outcome = Extractor::new(&store, &model).run(&chunk, ExtractionTrigger::Manual);

    assert_eq!(outcome.stored(), 0);
    assert_eq!(outcome.failure, None);
    assert!(!outcome.had_problems());
}

// -------------------------------------------------------------------------
// B. Rejection, one memory at a time.
// -------------------------------------------------------------------------

fn judge_json(json: &str) -> Result<Verdict, Refusal> {
    schema::judge(&serde_json::from_str(json).unwrap())
}

/// A memory with no `kind` is refused for exactly that reason.
#[test]
fn a_memory_with_no_kind_is_refused_for_a_missing_field() {
    let element = "{\"authority\":\"decision\",\"disposition\":\"accepted\",\
                    \"support\":\"established\",\"confidence\":\"certain\",\"body\":\"x\"}";
    assert_eq!(
        judge_json(element),
        Err(Refusal::MissingField { field: "kind" })
    );
}

/// A `kind` the store does not support is an unknown value, and `"idea"` — a
/// valid *authority* — is not a valid *kind*, which is the whole reason the
/// two are separate columns.
///
/// An empty string is *not* tested here alongside them: `required_enum`
/// treats a blank value as absent before it ever reaches `from_stored`, so
/// `kind: ""` is `Refusal::MissingField`, not `Refusal::UnknownValue` — see
/// the next test. The packet that asked for this one listed `""` beside
/// `"architecture"` and `"idea"` as if all three took the same path; they do
/// not, and this is called out in the report.
#[test]
fn an_unsupported_kind_is_an_unknown_value_including_idea_which_is_only_an_authority() {
    for bad_kind in ["architecture", "idea"] {
        let json = MemoryJson::new(bad_kind, "x")
            .authority("decision")
            .to_json();
        let result = schema::judge(&serde_json::from_str(&json).unwrap());
        assert!(
            matches!(result, Err(Refusal::UnknownValue { field: "kind", .. })),
            "kind `{bad_kind}` gave {result:?}"
        );
    }
}

/// An empty `kind` is treated as absent, not as an unsupported value: it is
/// blank after trimming, and `required_enum` filters a blank value out
/// before calling `MemoryKind::from_stored` at all.
#[test]
fn an_empty_kind_is_missing_rather_than_unknown() {
    let json = MemoryJson::new("", "x").authority("decision").to_json();
    let result = schema::judge(&serde_json::from_str(&json).unwrap());
    assert_eq!(result, Err(Refusal::MissingField { field: "kind" }));
}

/// Each of `authority`, `disposition`, `support` and `confidence` is required,
/// and each rejects a nonsense value.
#[test]
fn authority_disposition_support_and_confidence_are_each_required_and_validated() {
    let base = |overrides: &[(&str, Option<&str>)]| -> String {
        let mut fields: Vec<(&str, Option<&str>)> = vec![
            ("kind", Some("finding")),
            ("authority", Some("decision")),
            ("disposition", Some("accepted")),
            ("support", Some("established")),
            ("confidence", Some("certain")),
        ];
        for (name, value) in overrides {
            if let Some(slot) = fields.iter_mut().find(|(n, _)| n == name) {
                slot.1 = *value;
            }
        }
        let mut parts: Vec<String> = fields
            .into_iter()
            .filter_map(|(name, value)| value.map(|v| format!("\"{name}\":{}", quote(v))))
            .collect();
        parts.push(format!("\"body\":{}", quote("x")));
        format!("{{{}}}", parts.join(","))
    };

    for field in ["authority", "disposition", "support", "confidence"] {
        let missing = base(&[(field, None)]);
        let result = schema::judge(&serde_json::from_str(&missing).unwrap());
        assert_eq!(
            result,
            Err(Refusal::MissingField { field }),
            "missing `{field}` gave {result:?}"
        );

        let nonsense = base(&[(field, Some("not-a-real-value"))]);
        let result = schema::judge(&serde_json::from_str(&nonsense).unwrap());
        assert!(
            matches!(result, Err(Refusal::UnknownValue { field: f, .. }) if f == field),
            "nonsense `{field}` gave {result:?}"
        );
    }
}

/// A `decision` declared `abandoned` is refused: an abandoned approach is a
/// `failed_attempt` and nothing else.
#[test]
fn a_decision_declared_abandoned_is_a_conflated_disposition() {
    let json = MemoryJson::new("decision", "Use a second thread")
        .authority("decision")
        .disposition("abandoned")
        .rationale("it did not work")
        .to_json();
    let result = schema::judge(&serde_json::from_str(&json).unwrap());
    assert_eq!(
        result,
        Err(Refusal::ConflatedDisposition {
            kind: MemoryKind::Decision,
            disposition: Disposition::Abandoned,
        })
    );
}

/// A `failed_attempt` claiming to have been `accepted`, or merely `proposed`,
/// is the same conflated-disposition refusal — a `failed_attempt` is never
/// anything but abandoned.
#[test]
fn a_failed_attempt_cannot_claim_accepted_or_proposed() {
    for disposition in ["accepted", "proposed"] {
        let json = MemoryJson::new("failed_attempt", "Use a second thread")
            .authority("historical")
            .disposition(disposition)
            .to_json();
        let result = schema::judge(&serde_json::from_str(&json).unwrap());
        assert_eq!(
            result,
            Err(Refusal::ConflatedDisposition {
                kind: MemoryKind::FailedAttempt,
                disposition: Disposition::from_contract(disposition).unwrap(),
            }),
            "disposition `{disposition}` gave {result:?}"
        );
    }
}

/// A `failed_attempt` declared `abandoned` is accepted: the rule is a
/// distinction, not a ban on recording failures.
#[test]
fn a_failed_attempt_declared_abandoned_is_accepted() {
    let json = MemoryJson::new("failed_attempt", "Use a second thread")
        .authority("historical")
        .disposition("abandoned")
        .to_json();
    let Ok(Verdict::Keep(memory)) = schema::judge(&serde_json::from_str(&json).unwrap()) else {
        panic!("an abandoned failed_attempt must be kept");
    };
    assert_eq!(memory.kind, MemoryKind::FailedAttempt);
    assert_eq!(memory.disposition, Disposition::Abandoned);
}

/// A `decision` declared `invariant`, `constraint` or `decision` with no
/// rationale is refused; the same declared `preference`, `hypothesis`,
/// `idea` or `historical` is accepted without one.
#[test]
fn a_binding_decision_needs_rationale_and_a_non_binding_one_does_not() {
    for binding in ["invariant", "constraint", "decision"] {
        let json = MemoryJson::new("decision", "Use blocking threads")
            .authority(binding)
            .to_json();
        let result = schema::judge(&serde_json::from_str(&json).unwrap());
        assert_eq!(
            result,
            Err(Refusal::MissingRationale {
                declared: MemoryAuthority::from_stored(binding).unwrap(),
            }),
            "authority `{binding}` with no rationale gave {result:?}"
        );
    }

    for non_binding in ["preference", "hypothesis", "idea", "historical"] {
        let json = MemoryJson::new("decision", "Use blocking threads")
            .authority(non_binding)
            .to_json();
        let result = schema::judge(&serde_json::from_str(&json).unwrap());
        assert!(
            matches!(result, Ok(Verdict::Keep(_))),
            "authority `{non_binding}` with no rationale gave {result:?}"
        );
    }
}

/// A body over `MAX_BODY_CHARS`, a subject over `MAX_SUBJECT_CHARS`, and a
/// rationale over `MAX_RATIONALE_CHARS` are each refused naming the right
/// field. The body case is refused, not truncated: nothing is stored for it.
#[test]
fn an_over_long_field_is_refused_naming_the_right_field_and_nothing_is_stored() {
    let long_body = "x".repeat(MAX_BODY_CHARS + 1);
    let json = MemoryJson::new("finding", long_body)
        .authority("historical")
        .to_json();
    assert!(matches!(
        schema::judge(&serde_json::from_str(&json).unwrap()),
        Err(Refusal::TooLong { field: "body", .. })
    ));

    let long_subject: String = "y".repeat(MAX_SUBJECT_CHARS + 1);
    let json = format!(
        "{{\"kind\":\"finding\",\"authority\":\"historical\",\"disposition\":\"accepted\",\
          \"support\":\"established\",\"confidence\":\"certain\",\"subject\":{},\"body\":\"x\"}}",
        quote(&long_subject)
    );
    assert!(matches!(
        schema::judge(&serde_json::from_str(&json).unwrap()),
        Err(Refusal::TooLong {
            field: "subject",
            ..
        })
    ));

    let long_rationale: String = "z".repeat(MAX_RATIONALE_CHARS + 1);
    let json = format!(
        "{{\"kind\":\"decision\",\"authority\":\"decision\",\"disposition\":\"accepted\",\
          \"support\":\"established\",\"confidence\":\"certain\",\"body\":\"x\",\
          \"rationale\":{}}}",
        quote(&long_rationale)
    );
    assert!(matches!(
        schema::judge(&serde_json::from_str(&json).unwrap()),
        Err(Refusal::TooLong {
            field: "rationale",
            ..
        })
    ));

    // The over-long body must never have been stored: run it through the
    // whole extractor and check no row appears.
    let tmp = tempdir();
    let fixture = Fixture::new(tmp.path(), "alpha");
    let memory = fixture.memory();
    let store = memory.store();
    let model = Canned::new(envelope(&[MemoryJson::new(
        "finding",
        "x".repeat(MAX_BODY_CHARS + 1),
    )
    .authority("historical")]));
    let chunk = chunk_of(&["something long happened"]);
    let outcome = Extractor::new(&store, &model).run(&chunk, ExtractionTrigger::Manual);
    assert_eq!(outcome.stored(), 0);
    assert_eq!(store.count(MemoryStatus::Active).unwrap(), 0);
}

/// `support: speculative` is dropped, not refused: it lands in
/// `outcome.speculative`, contributes nothing to `outcome.rejected`, and
/// stores nothing.
#[test]
fn a_speculative_memory_is_dropped_rather_than_rejected_or_stored() {
    assert_eq!(
        judge_json(
            "{\"kind\":\"finding\",\"authority\":\"hypothesis\",\"disposition\":\"proposed\",\
              \"support\":\"speculative\",\"confidence\":\"unsure\",\"body\":\"maybe true\"}"
        ),
        Ok(Verdict::Speculative)
    );

    let tmp = tempdir();
    let fixture = Fixture::new(tmp.path(), "alpha");
    let memory = fixture.memory();
    let store = memory.store();
    let model = Canned::new(envelope(&[MemoryJson::new(
        "finding",
        "ConPTY probably reflows",
    )
    .authority("hypothesis")
    .disposition("proposed")
    .support("speculative")]));
    let chunk = chunk_of(&["someone guessed"]);
    let outcome = Extractor::new(&store, &model).run(&chunk, ExtractionTrigger::Manual);

    assert_eq!(outcome.stored(), 0);
    assert_eq!(outcome.speculative, 1);
    assert!(outcome.rejected.is_empty());
    assert_eq!(store.count(MemoryStatus::Active).unwrap(), 0);
}

// -------------------------------------------------------------------------
// C. One bad memory costs one memory.
// -------------------------------------------------------------------------

/// A reply of five memories where the second and fourth are unacceptable
/// stores exactly three, and `outcome.rejected.len() == 2` — asserted on the
/// stored bodies, not only the count.
#[test]
fn one_bad_memory_in_a_batch_costs_one_memory_not_the_whole_batch() {
    let tmp = tempdir();
    let fixture = Fixture::new(tmp.path(), "alpha");
    let memory = fixture.memory();
    let store = memory.store();

    let memories = vec![
        MemoryJson::new("finding", "the first finding survives").authority("historical"),
        MemoryJson::new("bogus-kind", "this one is broken").authority("historical"),
        MemoryJson::new("todo", "the third memory survives").authority("historical"),
        MemoryJson::new("decision", "an abandoned decision, which is conflated")
            .authority("decision")
            .disposition("abandoned")
            .rationale("does not matter"),
        MemoryJson::new("feature", "the fifth memory survives").authority("historical"),
    ];

    let model = Canned::new(envelope(&memories));
    let chunk = chunk_of(&["a mixed batch of memories"]);
    let outcome = Extractor::new(&store, &model).run(&chunk, ExtractionTrigger::Manual);

    assert_eq!(outcome.stored(), 3);
    assert_eq!(outcome.rejected.len(), 2);

    let stored_bodies: Vec<String> = outcome
        .recorded
        .iter()
        .map(|id| store.get(id).unwrap().unwrap().body)
        .collect();
    assert!(
        stored_bodies
            .iter()
            .any(|b| b.contains("the first finding survives"))
    );
    assert!(
        stored_bodies
            .iter()
            .any(|b| b.contains("the third memory survives"))
    );
    assert!(
        stored_bodies
            .iter()
            .any(|b| b.contains("the fifth memory survives"))
    );
    assert!(
        !stored_bodies
            .iter()
            .any(|b| b.contains("this one is broken"))
    );
    assert!(!stored_bodies.iter().any(|b| b.contains("conflated")));
}

// -------------------------------------------------------------------------
// D. Replies that are not clean JSON.
// -------------------------------------------------------------------------

/// A reply wrapped in prose, and one inside a ```` ```json ```` fence, both
/// work.
#[test]
fn a_reply_wrapped_in_prose_or_a_fence_both_parse() {
    let tmp = tempdir();
    let fixture = Fixture::new(tmp.path(), "alpha");
    let memory = fixture.memory();
    let store = memory.store();

    let prose_reply = format!(
        "Here is what I found:\n{}\nThat's everything.",
        envelope(&[MemoryJson::new("finding", "prose-wrapped memory").authority("historical")])
    );
    let model = Canned::new(prose_reply);
    let chunk = chunk_of(&["some activity"]);
    let outcome = Extractor::new(&store, &model).run(&chunk, ExtractionTrigger::Manual);
    assert_eq!(outcome.stored(), 1);

    let fixture2 = Fixture::new(tmp.path(), "beta");
    let memory2 = fixture2.memory();
    let store2 = memory2.store();
    let fenced_reply = format!(
        "```json\n{}\n```",
        envelope(&[MemoryJson::new("finding", "fenced memory").authority("historical")])
    );
    let model2 = Canned::new(fenced_reply);
    let chunk2 = chunk_of(&["some activity"]);
    let outcome2 = Extractor::new(&store2, &model2).run(&chunk2, ExtractionTrigger::Manual);
    assert_eq!(outcome2.stored(), 1);
}

/// A reply with no JSON object at all is a failure, and nothing is stored.
#[test]
fn a_reply_with_no_json_object_is_a_reply_failure_and_stores_nothing() {
    let tmp = tempdir();
    let fixture = Fixture::new(tmp.path(), "alpha");
    let memory = fixture.memory();
    let store = memory.store();

    let model = Canned::new("I could not find anything worth remembering.".to_owned());
    let chunk = chunk_of(&["some activity"]);
    let outcome = Extractor::new(&store, &model).run(&chunk, ExtractionTrigger::Manual);

    assert!(matches!(outcome.failure, Some(ExtractionFailure::Reply(_))));
    assert_eq!(outcome.stored(), 0);
    assert_eq!(store.count(MemoryStatus::Active).unwrap(), 0);
}

/// Truncated JSON is a failure, not a panic.
#[test]
fn truncated_json_is_a_failure_not_a_panic() {
    let tmp = tempdir();
    let fixture = Fixture::new(tmp.path(), "alpha");
    let memory = fixture.memory();
    let store = memory.store();

    let model = Canned::new("{\"memories\": [{\"kind\":".to_owned());
    let chunk = chunk_of(&["some activity"]);
    let outcome = Extractor::new(&store, &model).run(&chunk, ExtractionTrigger::Manual);

    assert!(matches!(outcome.failure, Some(ExtractionFailure::Reply(_))));
    assert_eq!(outcome.stored(), 0);
}

/// A reply that is a JSON array rather than an object, and a reply where
/// `memories` is an object instead of an array. Recorded here with what
/// each actually does, per the packet's ask — and the array case does
/// something the packet did not anticipate; see the doc comment below.
///
/// `{"memories": {}}` has an outermost object, but deserializing `memories`
/// as a `Vec<Value>` from a JSON object fails, so this is
/// `ExtractionFailure::Reply(Refusal::Malformed)`.
///
/// A **bare** top-level array with no `{` anywhere (`["just", "strings"]`)
/// has no outermost object either, so `extract_json_object` finds nothing
/// and this is also `Refusal::Malformed` — the same path as no-object-at-all.
///
/// But `extract_json_object` finds the *first* `{`, wherever it sits, not
/// only one at the top level. A top-level array that happens to *contain* an
/// object — `[{"kind": "finding"}]`, the shape a model producing one memory
/// with the brackets doubled would actually emit — has that inner
/// `{"kind": "finding"}` picked out and parsed as if it were the whole
/// envelope. It has no `memories` key, so `Envelope`'s `#[serde(default)]`
/// gives an empty vector: **no failure, and nothing stored**, indistinguishable
/// from a model that genuinely found nothing. This is the answer the report
/// calls out under "WHAT ANSWER D.19 GAVE".
#[test]
fn a_top_level_array_with_no_object_in_it_is_a_reply_failure() {
    let tmp = tempdir();
    let fixture = Fixture::new(tmp.path(), "alpha");
    let memory = fixture.memory();
    let store = memory.store();

    let model = Canned::new("[\"just\", \"strings\"]".to_owned());
    let chunk = chunk_of(&["some activity"]);
    let outcome = Extractor::new(&store, &model).run(&chunk, ExtractionTrigger::Manual);
    assert!(
        matches!(
            outcome.failure,
            Some(ExtractionFailure::Reply(Refusal::Malformed { .. }))
        ),
        "a bare top-level array gave {:?}",
        outcome.failure
    );
    assert_eq!(outcome.stored(), 0);
}

/// An array that *contains* an object — the shape one mistaken bracket
/// produces — must be a visible failure and not a silent zero.
///
/// **This test asserted the opposite when it was written, and it was
/// documenting a real defect rather than a design.** `extract_json_object`
/// takes the first `{` wherever it sits, so the inner object was read as the
/// whole envelope, had no `memories` key, defaulted to empty, and reported
/// "found nothing" with `failure == None` — indistinguishable from a model
/// that looked and found nothing. `Envelope::memories` is now a required key,
/// which leaves exactly one way to say nothing was found and makes every
/// other shape visible. Found by this suite's own author while probing
/// envelope shapes the packet asked about; fixed by the lead.
#[test]
fn a_top_level_array_containing_an_object_is_a_failure_rather_than_a_silent_zero() {
    let tmp = tempdir();
    let fixture = Fixture::new(tmp.path(), "alpha");
    let memory = fixture.memory();
    let store = memory.store();

    let model = Canned::new("[{\"kind\": \"finding\"}]".to_owned());
    let chunk = chunk_of(&["some activity"]);
    let outcome = Extractor::new(&store, &model).run(&chunk, ExtractionTrigger::Manual);

    assert!(
        matches!(
            outcome.failure,
            Some(ExtractionFailure::Reply(Refusal::Malformed { .. }))
        ),
        "a mis-bracketed reply must not look like an empty one; got {:?}",
        outcome.failure
    );
    assert_eq!(outcome.stored(), 0);
}

/// And the one shape that genuinely means "nothing worth remembering" still
/// reports success, because otherwise the fix above would have made honesty
/// impossible to express.
#[test]
fn an_empty_memories_array_is_success_with_nothing_stored() {
    let tmp = tempdir();
    let fixture = Fixture::new(tmp.path(), "alpha");
    let memory = fixture.memory();
    let store = memory.store();

    let model = Canned::new("{\"memories\": []}".to_owned());
    let chunk = chunk_of(&["some activity"]);
    let outcome = Extractor::new(&store, &model).run(&chunk, ExtractionTrigger::Manual);

    assert_eq!(outcome.failure, None, "finding nothing is a valid answer");
    assert_eq!(outcome.stored(), 0);
    assert!(outcome.rejected.is_empty());
}

/// `{"memories": {}}` — an object where an array belongs — is a reply
/// failure: `memories` cannot deserialize into `Vec<Value>` from a JSON
/// object.
#[test]
fn a_memories_object_instead_of_an_array_is_a_reply_failure() {
    let tmp = tempdir();
    let fixture = Fixture::new(tmp.path(), "alpha");
    let memory = fixture.memory();
    let store = memory.store();

    let model = Canned::new("{\"memories\": {}}".to_owned());
    let chunk = chunk_of(&["some activity"]);
    let outcome = Extractor::new(&store, &model).run(&chunk, ExtractionTrigger::Manual);
    assert!(
        matches!(
            outcome.failure,
            Some(ExtractionFailure::Reply(Refusal::Malformed { .. }))
        ),
        "`{{\"memories\": {{}}}}` gave {:?}",
        outcome.failure
    );
    assert_eq!(outcome.stored(), 0);
}

// -------------------------------------------------------------------------
// E. The prompt is a contract too.
// -------------------------------------------------------------------------

/// The prompt the fake model was handed contains the schema, the session's
/// activity, and the credential rule — asserted on the recorded prompt, not
/// on `PROMPT_CONTRACT` directly.
#[test]
fn the_recorded_prompt_carries_the_contract_the_schema_and_the_activity() {
    let tmp = tempdir();
    let fixture = Fixture::new(tmp.path(), "alpha");
    let memory = fixture.memory();
    let store = memory.store();

    let model = Canned::new(envelope(&[]));
    let chunk = chunk_of(&["we chose blocking threads for the pty reader"]);
    let _ = Extractor::new(&store, &model).run(&chunk, ExtractionTrigger::Manual);

    let seen = model.seen.lock().unwrap();
    assert_eq!(seen.len(), 1, "exactly one prompt should have been sent");
    let prompt = &seen[0];

    assert!(prompt.contains("NEVER include a credential"));
    assert!(prompt.contains("\"memories\""));
    assert!(prompt.contains("we chose blocking threads for the pty reader"));
}

/// `RESPONSE_SCHEMA` names every `MemoryKind` and every `MemoryAuthority`
/// spelling, reached through the public API rather than the module's own
/// unit test, which can see private items a consumer cannot.
#[test]
fn the_public_response_schema_names_every_supported_kind_and_authority() {
    for kind in MemoryKind::ALL {
        assert!(
            RESPONSE_SCHEMA.contains(kind.as_str()),
            "RESPONSE_SCHEMA never names the kind `{kind}`"
        );
    }
    for authority in MemoryAuthority::ALL {
        assert!(
            RESPONSE_SCHEMA.contains(authority.as_str()),
            "RESPONSE_SCHEMA never names the authority `{authority}`"
        );
    }
}

// -------------------------------------------------------------------------
// Sanity: the public surface used above stays public, and `Rejection`'s and
// `ExtractedMemory`'s own fields are reachable — a compile-time property,
// but only if these actually appear somewhere.
// -------------------------------------------------------------------------

/// `Rejection::Contract` carries the exact `Refusal`, so a caller reading
/// `outcome.rejected` can tell a missing field from a conflated disposition
/// without re-parsing anything.
#[test]
fn a_rejection_carries_the_exact_refusal_it_was_refused_for() {
    let tmp = tempdir();
    let fixture = Fixture::new(tmp.path(), "alpha");
    let memory = fixture.memory();
    let store = memory.store();

    let model = Canned::new(envelope(&[
        MemoryJson::new("bogus-kind", "x").authority("historical")
    ]));
    let chunk = chunk_of(&["some activity"]);
    let outcome = Extractor::new(&store, &model).run(&chunk, ExtractionTrigger::Manual);

    assert_eq!(outcome.rejected.len(), 1);
    assert!(matches!(
        &outcome.rejected[0],
        Rejection::Contract(Refusal::UnknownValue { field: "kind", .. })
    ));
}

/// `ExtractedMemory::stored_body` (reached indirectly, through `judge`, since
/// the extractor never hands one back directly) folds a rationale in with
/// `RATIONALE_MARKER`, matching what actually lands in the store.
#[test]
fn a_kept_memorys_stored_body_folds_in_the_rationale() {
    let json = MemoryJson::new("decision", "Use blocking threads")
        .authority("decision")
        .rationale("no async runtime is in the dependency set")
        .to_json();
    let Ok(Verdict::Keep(memory)) = schema::judge(&serde_json::from_str(&json).unwrap()) else {
        panic!("expected a kept memory");
    };
    let ExtractedMemory {
        disposition,
        confidence,
        ..
    } = &memory;
    assert_eq!(*disposition, Disposition::Accepted);
    assert_eq!(*confidence, Confidence::Certain);
    assert_eq!(
        memory.stored_body(),
        format!("Use blocking threads{RATIONALE_MARKER}no async runtime is in the dependency set")
    );
}

/// The prompt contract's own text is what the assembled prompt has to carry —
/// a change to one without the other would make the test above pass for the
/// wrong reason.
#[test]
fn the_prompt_contract_states_the_credential_rule() {
    assert!(PROMPT_CONTRACT.contains("NEVER include a credential"));
}

/// `Support::Speculative` and `ModelError` stay part of the public surface a
/// caller matches on.
#[test]
fn a_model_unavailable_error_is_a_model_failure_not_a_reply_failure() {
    struct Unavailable;
    impl ExtractionModel for Unavailable {
        fn describe(&self) -> String {
            "test/unavailable".to_owned()
        }
        fn complete(&self, _prompt: &Prompt) -> Result<String, ModelError> {
            Err(ModelError::Unavailable)
        }
    }

    let tmp = tempdir();
    let fixture = Fixture::new(tmp.path(), "alpha");
    let memory = fixture.memory();
    let store = memory.store();

    let model = Unavailable;
    let chunk = chunk_of(&["some activity"]);
    let outcome = Extractor::new(&store, &model).run(&chunk, ExtractionTrigger::Manual);

    assert_eq!(
        outcome.failure,
        Some(ExtractionFailure::Model(ModelError::Unavailable))
    );
    assert_eq!(outcome.stored(), 0);
}

/// An empty chunk never reaches the model at all — `ExtractionFailure::
/// NothingToExtract` is reported without a call being made.
#[test]
fn an_empty_chunk_never_calls_the_model() {
    let tmp = tempdir();
    let fixture = Fixture::new(tmp.path(), "alpha");
    let memory = fixture.memory();
    let store = memory.store();

    let model = Canned::new(envelope(&[]));
    let chunk = SessionChunk::build(
        "session-9",
        None::<String>,
        std::iter::empty::<String>(),
        ChunkLimits::default(),
    );
    let outcome = Extractor::new(&store, &model).run(&chunk, ExtractionTrigger::Manual);

    assert_eq!(outcome.failure, Some(ExtractionFailure::NothingToExtract));
    assert!(
        model.seen.lock().unwrap().is_empty(),
        "the model must not have been called"
    );
}

/// `Support::ALL` and `Disposition::ALL` etc. having exactly the sizes the
/// contract documents, reached from outside the module.
#[test]
fn the_contract_enums_have_the_sizes_the_contract_documents() {
    assert_eq!(Support::ALL.len(), 2);
    assert_eq!(Disposition::ALL.len(), 3);
    assert_eq!(Confidence::ALL.len(), 3);
}
