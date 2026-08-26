//! Turning session activity into durable project memory (Phase 21).
//!
//! # What exists here and what deliberately does not
//!
//! This is the **producer** for the store Phases 20, 22 and 23 built. It
//! bounds and scrubs session activity, asks a model for structured memories,
//! validates what comes back against a contract it cannot argue with, and
//! records what survives.
//!
//! It does **not** call a provider. Phase 21 says *"allow a configurable
//! cheap or local model to perform memory extraction"*, and the mechanism
//! that would provide one — Phase 39's disposable-job interface — does not
//! exist yet. So [`ExtractionModel`] is the seam, tested against fakes, and
//! this batch's report states exactly what Phase 39 must supply. Building
//! half a provider call path here would be a second answer to a question
//! another phase owns.
//!
//! Nothing **triggers** extraction either. Phase 21's three trigger lines
//! (after task completion, around native compaction, manually for
//! debugging) all need a caller in a file this batch does not own;
//! [`ExtractionTrigger`] is the type they will pass, and the report carries
//! the exact wiring each one needs.
//!
//! # The acceptance condition, and where it lives
//!
//! **The extractor must never be fed, and must never emit, credential
//! material.** `memories.body` is free text and the schema cannot stop a
//! secret being put in it — the pinned-schema test
//! `the_project_database_schema_has_nowhere_to_put_a_credential` says so in
//! its own doc comment and hands the control to this module.
//!
//! It is enforced at exactly three places, each with its own regression test
//! and its own mutation:
//!
//! 1. [`chunk::SessionChunk::build`] scrubs every entry, so no chunk in the
//!    program holds un-scrubbed activity;
//! 2. [`Prompt::build`] scrubs the block of already-stored memories it
//!    quotes back, because a memory recorded before this module existed was
//!    never screened;
//! 3. [`schema::judge`] screens each emitted element **before reading any of
//!    its fields** and refuses it whole rather than redacting it.
//!
//! The first two are why nothing reaches a model. The third is why nothing
//! reaches a row. See [`credentials`] for why the two directions are
//! deliberately asymmetric — scrubbed in, refused out.
//!
//! # Failure is not the session's problem
//!
//! [`Extractor::run`] returns [`ExtractionOutcome`] and **no `Result`**.
//! There is no error channel for a caller to propagate, which is the
//! structural form of Phase 21's *"keep memory-extraction failure non-fatal
//! to the coding session"*: an unavailable model, an unparseable reply, a
//! store that refuses a row, and a model implementation that panics all
//! produce an outcome describing what happened.

pub mod authority;
pub mod chunk;
pub mod credentials;
pub mod disposable;
pub mod lifecycle;
pub mod schema;

use std::fmt;

use authority::Classification;
use chunk::SessionChunk;
use schema::{ExtractedMemory, PROMPT_CONTRACT, RESPONSE_SCHEMA, Refusal, Verdict};

use super::store::{MemoryId, MemoryKind, MemoryStore, MemoryStoreError, NewMemory};

/// How many already-stored memories are quoted back to the model.
///
/// Bounded for the same reason the chunk is: this is a prompt, not a dump of
/// the project's knowledge. Phase 21's *"avoid duplicating an existing
/// active memory"* is enforced after the reply by [`Extractor`] regardless,
/// so this list is an efficiency, not the control.
pub const EXISTING_MEMORIES_QUOTED: usize = 20;

/// How much of an already-stored memory is quoted back.
pub const EXISTING_MEMORY_CHARS: usize = 160;

/// Why extraction ran.
///
/// Recorded on every outcome so a memory produced by a debugging run is
/// distinguishable from one produced automatically. The three variants are
/// Phase 21's three trigger lines; **nothing constructs them in production
/// yet**, and this batch's report says what each needs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExtractionTrigger {
    /// A turn ended with [`crate::events::TurnOutcome::Completed`].
    TaskCompleted,
    /// The harness is about to compact, or has just compacted, its own
    /// context. Phase 21 wants durable memory written before a lossy native
    /// summary replaces the detail.
    BeforeCompaction,
    /// A person asked, to debug or evaluate extraction itself.
    Manual,
}

impl ExtractionTrigger {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::TaskCompleted => "task_completed",
            Self::BeforeCompaction => "before_compaction",
            Self::Manual => "manual",
        }
    }
}

impl fmt::Display for ExtractionTrigger {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.pad(self.as_str())
    }
}

/// Why a model could not be asked.
///
/// Deliberately coarse and deliberately without a payload that could carry
/// provider text: an extraction failure is logged, and a provider error body
/// can contain a request echo. Phase 39 will have richer diagnostics at its
/// own layer, where they belong.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ModelError {
    /// No model is configured, or the configured one cannot be reached.
    #[error("no extraction model is available")]
    Unavailable,
    /// The model declined to answer.
    #[error("the extraction model declined the request")]
    Refused,
    /// The model did not answer within its bound.
    #[error("the extraction model did not answer within its bound")]
    TimedOut,
    /// Something else went wrong at the transport. The description is a
    /// fixed phrase chosen by the implementation, never a provider body.
    #[error("the extraction model failed: {phrase}")]
    Failed { phrase: &'static str },
}

/// The prompt an extraction model is given.
///
/// A newtype with one constructor, so the only text that can reach a model
/// is text this module assembled — and therefore text that went through the
/// scrubber. There is no `From<String>` and no public field.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Prompt(String);

impl Prompt {
    /// Assemble the contract, the schema, the project's existing memories and
    /// the session activity into one prompt.
    ///
    /// `existing` is scrubbed here rather than assumed clean: those strings
    /// come out of the database, and a row written by something other than
    /// this module — a future `memory add`, an imported backup — never passed
    /// a screen. The chunk is already scrubbed by construction.
    pub fn build(chunk: &SessionChunk, existing: &[String]) -> Self {
        use std::fmt::Write as _;

        let mut out = String::with_capacity(PROMPT_CONTRACT.len() + RESPONSE_SCHEMA.len() + 4096);
        out.push_str(PROMPT_CONTRACT);
        out.push_str(RESPONSE_SCHEMA);

        if existing.is_empty() {
            out.push_str("\n\n## Memories this project already holds\n\nNone.\n");
        } else {
            out.push_str("\n\n## Memories this project already holds\n\n");
            for memory in existing.iter().take(EXISTING_MEMORIES_QUOTED) {
                let scrubbed = credentials::scrub(memory);
                let clipped: String = scrubbed
                    .text()
                    .chars()
                    .take(EXISTING_MEMORY_CHARS)
                    .collect();
                let _ = writeln!(out, "- {}", clipped.replace('\n', " "));
            }
        }

        let _ = write!(out, "\n## Session {}", chunk.session_id());
        if let Some(commit) = chunk.commit() {
            let _ = write!(out, " at commit {commit}");
        }
        out.push_str("\n\n");

        for (index, entry) in chunk.entries().iter().enumerate() {
            let _ = writeln!(out, "[{}] {entry}", index + 1);
        }

        if chunk.dropped() > 0 || chunk.truncated() > 0 {
            let _ = writeln!(
                out,
                "\n({} earlier entries omitted and {} shortened to fit; this is one \
                 bounded slice of the session, not all of it.)",
                chunk.dropped(),
                chunk.truncated()
            );
        }

        Self(out)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Something that can answer an extraction prompt.
///
/// # This is the seam, and Phase 39 is what fills it
///
/// Phase 39 defines *"a simple provider interface for non-interactive
/// disposable LLM jobs"*. Until it exists, Glasshouse has no way to call a
/// model at all, so this trait is what extraction is written against and
/// what the tests supply fakes for. The report accompanying this batch lists
/// exactly what a Phase 39 implementation must provide.
///
/// `Send + Sync` because extraction will run off the thread draining a
/// pseudo-terminal — the event bus's rule about never making a harness wait
/// applies to anything a lifecycle event triggers.
pub trait ExtractionModel: Send + Sync {
    /// Which resource this is, for the record.
    ///
    /// Phase 39 requires Glasshouse to *"record which resource performed
    /// important memory extraction or classification for debugging"*, and
    /// [`ExtractionOutcome::model`] is where it lands. Must name the model
    /// and route, and must never contain a credential or a base URL with one
    /// embedded.
    ///
    /// **Must be cheap.** [`Extractor::run`] calls it once per run including
    /// on runs that ask no model at all — an empty chunk short-circuits
    /// before [`Self::complete`] but after this, deliberately, so that a
    /// `NothingToExtract` outcome still records which resource *would* have
    /// been used. An implementation that probed a provider here would turn a
    /// no-op into a network call.
    fn describe(&self) -> String;

    /// Answer the prompt, or say why not.
    fn complete(&self, prompt: &Prompt) -> Result<String, ModelError>;
}

/// Why one emitted memory was not stored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Rejection {
    /// The model broke the contract. See [`Refusal`].
    Contract(Refusal),
    /// The store refused the row — the Phase 20 admission guard, or SQLite.
    ///
    /// A rendered message rather than the error, because
    /// [`MemoryStoreError`] is not `Clone` and an outcome is a value a
    /// caller keeps. Safe to render: the memory's text was screened before
    /// this point.
    Store(String),
}

impl fmt::Display for Rejection {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Contract(refusal) => write!(f, "{refusal}"),
            Self::Store(message) => write!(f, "{message}"),
        }
    }
}

/// Why a whole extraction produced nothing.
///
/// Distinct from [`Rejection`], which is about one memory. None of these is
/// an error a caller has to handle — see [`Extractor::run`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExtractionFailure {
    /// There was nothing in the chunk to extract from.
    NothingToExtract,
    /// The model could not be asked, or would not answer.
    Model(ModelError),
    /// The reply was not a document this contract can read.
    Reply(Refusal),
    /// The model implementation panicked. Caught rather than propagated —
    /// see [`Extractor::run`].
    ModelPanicked,
    /// The project's existing memories could not be read, so duplicate
    /// detection was impossible. Extraction stops rather than recording
    /// memories it cannot check for duplication.
    StoreUnreadable(String),
}

impl fmt::Display for ExtractionFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NothingToExtract => f.write_str("no session activity to extract from"),
            Self::Model(err) => write!(f, "{err}"),
            Self::Reply(refusal) => write!(f, "{refusal}"),
            Self::ModelPanicked => f.write_str("the extraction model panicked"),
            Self::StoreUnreadable(message) => {
                write!(
                    f,
                    "could not read this project's existing memories: {message}"
                )
            }
        }
    }
}

/// Everything one extraction did, whether or not it worked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtractionOutcome {
    pub trigger: ExtractionTrigger,
    /// Which resource performed extraction — see
    /// [`ExtractionModel::describe`].
    pub model: String,
    /// The session every memory here is attributed to.
    pub session_id: String,
    /// The commit every memory here is attributed to, when known.
    pub commit: Option<String>,
    /// What was stored.
    pub recorded: Vec<MemoryId>,
    /// Memories stored under a weaker authority than the model asked for,
    /// with the reason. Reported rather than silent: this is the
    /// conservative-classification rule being visible.
    pub lowered: Vec<(MemoryId, Classification)>,
    /// How many memories the model marked speculative, and were dropped.
    pub speculative: usize,
    /// How many memories already existed unchanged and were not stored
    /// again.
    pub duplicates: usize,
    /// Per-memory rejections, each with its reason.
    pub rejected: Vec<Rejection>,
    /// Entries the chunk's budget dropped.
    pub activity_dropped: usize,
    /// Entries the chunk's budget shortened.
    pub activity_truncated: usize,
    /// Credentials the scrubber removed on the way in.
    pub redactions: usize,
    /// Set when the whole extraction failed. Never an error a caller must
    /// handle.
    pub failure: Option<ExtractionFailure>,
}

impl ExtractionOutcome {
    fn empty(trigger: ExtractionTrigger, model: String, chunk: &SessionChunk) -> Self {
        Self {
            trigger,
            model,
            session_id: chunk.session_id().to_owned(),
            commit: chunk.commit().map(str::to_owned),
            recorded: Vec::new(),
            lowered: Vec::new(),
            speculative: 0,
            duplicates: 0,
            rejected: Vec::new(),
            activity_dropped: chunk.dropped(),
            activity_truncated: chunk.truncated(),
            redactions: chunk.redactions(),
            failure: None,
        }
    }

    /// How many memories were stored.
    pub fn stored(&self) -> usize {
        self.recorded.len()
    }

    /// Whether anything at all went wrong, at either granularity.
    ///
    /// Useful for a debugging surface. Not useful for deciding whether the
    /// coding session is in trouble: it never is.
    pub fn had_problems(&self) -> bool {
        self.failure.is_some() || !self.rejected.is_empty()
    }
}

/// Extracts durable memory from bounded session activity.
///
/// Borrows both the store and the model, so an extractor is a short-lived
/// value built around one run rather than something long-lived holding a
/// connection open.
pub struct Extractor<'a> {
    store: &'a MemoryStore<'a>,
    model: &'a dyn ExtractionModel,
}

impl fmt::Debug for Extractor<'_> {
    /// Names the model and nothing about the store's contents.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Extractor")
            .field("model", &self.model.describe())
            .finish_non_exhaustive()
    }
}

impl<'a> Extractor<'a> {
    pub fn new(store: &'a MemoryStore<'a>, model: &'a dyn ExtractionModel) -> Self {
        Self { store, model }
    }

    /// Extract from one bounded chunk.
    ///
    /// # There is no error channel, and that is the design
    ///
    /// Phase 21 requires extraction failure to be *"non-fatal to the coding
    /// session"*. A `Result` here would put the decision in every caller's
    /// hands, and one caller using `?` on a lifecycle-event path would make
    /// a failed extraction end a session. So this returns an outcome, always,
    /// and every failure is a field on it.
    ///
    /// A model implementation that **panics** is caught rather than allowed
    /// to unwind into the caller, because a disposable support job taking a
    /// coding session down is the same defect wearing a worse hat.
    /// `AssertUnwindSafe` is sound here for a specific reason: nothing has
    /// been written to the store when the model is called, so a panic
    /// unwinding out of it cannot leave a partially-recorded extraction
    /// behind — the outcome is discarded whole and reported as
    /// [`ExtractionFailure::ModelPanicked`]. Note the caveat: the default
    /// panic hook still prints to stderr, so a Glasshouse that runs
    /// extraction while a TUI owns the terminal must install a hook of its
    /// own.
    pub fn run(&self, chunk: &SessionChunk, trigger: ExtractionTrigger) -> ExtractionOutcome {
        let mut outcome = ExtractionOutcome::empty(trigger, self.model.describe(), chunk);

        if chunk.is_empty() {
            outcome.failure = Some(ExtractionFailure::NothingToExtract);
            return outcome;
        }

        let existing = match self.existing_bodies() {
            Ok(existing) => existing,
            Err(err) => {
                outcome.failure = Some(ExtractionFailure::StoreUnreadable(err.to_string()));
                return outcome;
            }
        };

        let prompt = Prompt::build(chunk, &existing);

        let reply = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            self.model.complete(&prompt)
        })) {
            Ok(Ok(reply)) => reply,
            Ok(Err(err)) => {
                outcome.failure = Some(ExtractionFailure::Model(err));
                return outcome;
            }
            Err(_) => {
                outcome.failure = Some(ExtractionFailure::ModelPanicked);
                return outcome;
            }
        };

        let elements = match schema::parse(&reply) {
            Ok(elements) => elements,
            Err(refusal) => {
                outcome.failure = Some(ExtractionFailure::Reply(refusal));
                return outcome;
            }
        };

        // Normalized bodies of everything currently active, so a duplicate
        // is detected against the project rather than only against this
        // reply — and against this reply too, as memories are added.
        let mut seen: Vec<String> = existing.iter().map(|body| normalize(body)).collect();

        for element in &elements {
            match schema::judge(element) {
                Ok(Verdict::Speculative) => outcome.speculative += 1,
                Ok(Verdict::Keep(memory)) => {
                    self.store_one(memory, chunk, &mut seen, &mut outcome);
                }
                Err(refusal) => outcome.rejected.push(Rejection::Contract(refusal)),
            }
        }

        outcome
    }

    /// Record one validated memory, unless the project already holds it.
    fn store_one(
        &self,
        memory: ExtractedMemory,
        chunk: &SessionChunk,
        seen: &mut Vec<String>,
        outcome: &mut ExtractionOutcome,
    ) {
        let body = memory.body.clone();
        let key = normalize(&body);

        // Phase 21: avoid duplicating an existing active memory when nothing
        // materially changed. Normalized equality is the floor — it is the
        // part that is mechanically decidable. Anything subtler is a
        // judgment, and `super::policy`'s module documentation explains why
        // this layer does not fake those.
        if seen.contains(&key) {
            outcome.duplicates += 1;
            return;
        }

        let classification = authority::conservative(
            memory.declared_authority,
            memory.confidence,
            memory.disposition,
        );

        let new = NewMemory::new(memory.kind, body)
            .with_subject(memory.subject.clone())
            .with_authority(Some(classification.stored))
            .with_source_session(Some(chunk.session_id()))
            .with_source_commit(chunk.commit())
            // Phase 21: *store the originating session and event references
            // so extracted memory retains provenance.* The session was
            // already carried; the event range is what says **which part** of
            // it this memory came from, and it is the chunk's because the
            // chunk is the only thing that knows what was actually shown to
            // the model.
            .with_source_events(chunk.source_events())
            // Phase 21B, in one move. The provenance is validated on the way
            // out of `schema::judge` and stored as it stands; nothing here
            // re-derives or defaults any of it, because an assumption
            // Glasshouse invented would be indistinguishable in the store
            // from one a session established.
            .with_provenance(memory.provenance.clone());

        match self.store.record(new) {
            Ok(record) => {
                seen.push(key);
                if classification.was_lowered() {
                    outcome
                        .lowered
                        .push((record.id.clone(), classification.clone()));
                }
                outcome.recorded.push(record.id);
            }
            Err(err) => outcome.rejected.push(Rejection::Store(err.to_string())),
        }
    }

    /// The bodies of this project's current memories, newest first.
    ///
    /// Read through the store's own connection, so the project scoping is
    /// the one `ProjectMemory::open` established — there is no path argument
    /// and no project argument here either.
    fn existing_bodies(&self) -> Result<Vec<String>, MemoryStoreError> {
        use super::store::MemoryStatus;

        let mut statement = self
            .store
            .connection()
            .prepare(
                "SELECT subject, body FROM memories \
                 WHERE project_id = ?1 AND status = ?2 \
                 ORDER BY updated_at DESC, id ASC LIMIT ?3",
            )
            .map_err(|source| MemoryStoreError::Sql {
                action: "prepare the duplicate check",
                source,
            })?;

        let rows = statement
            .query_map(
                rusqlite::params![
                    self.store.project_id(),
                    MemoryStatus::Active.as_str(),
                    // Read more than the prompt quotes: the prompt list is an
                    // efficiency and this one is the control.
                    i64::try_from(EXISTING_MEMORIES_QUOTED * 25).unwrap_or(i64::MAX),
                ],
                |row| {
                    let subject: Option<String> = row.get(0)?;
                    let body: String = row.get(1)?;
                    Ok(match subject {
                        Some(subject) => format!("{subject}: {body}"),
                        None => body,
                    })
                },
            )
            .and_then(|rows| rows.collect::<Result<Vec<_>, _>>())
            .map_err(|source| MemoryStoreError::Sql {
                action: "read existing memories for the duplicate check",
                source,
            })?;

        Ok(rows)
    }
}

/// Reduce text to what a duplicate check should compare.
///
/// Case, whitespace runs and trailing sentence punctuation are all
/// presentation. Two memories differing only in those has *nothing
/// materially changed*, which is exactly the phrase Phase 21 uses.
///
/// Nothing cleverer: stemming or synonym matching would start deciding that
/// two different statements are the same, and a duplicate check that
/// silently discards a real memory is worse than one that occasionally
/// stores a near-duplicate.
fn normalize(text: &str) -> String {
    let collapsed: String = text
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase();
    collapsed.trim_end_matches(['.', '!', ';', ',']).to_owned()
}

/// The subject line a duplicate check compares against, for callers building
/// their own comparison. Exposed so a future retrieval surface uses the same
/// normalization rather than inventing a second one.
pub fn duplicate_key(subject: Option<&str>, body: &str) -> String {
    match subject {
        Some(subject) => normalize(&format!("{subject}: {body}")),
        None => normalize(body),
    }
}

/// The kinds extraction is allowed to produce.
///
/// Every one of Phase 20's six. Stated as a function rather than left
/// implicit so that a future restriction is a visible decision: nothing
/// today restricts it, and a reader should not have to prove that by
/// reading [`schema::judge`].
pub fn extractable_kinds() -> &'static [MemoryKind] {
    MemoryKind::ALL
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Silent;

    impl ExtractionModel for Silent {
        fn describe(&self) -> String {
            "test/silent".to_owned()
        }
        fn complete(&self, _prompt: &Prompt) -> Result<String, ModelError> {
            Err(ModelError::Unavailable)
        }
    }

    fn chunk_of(activity: &[&str]) -> SessionChunk {
        SessionChunk::build(
            "s1",
            Some("a938fcc"),
            activity.iter().map(|s| (*s).to_owned()),
            chunk::ChunkLimits::default(),
        )
    }

    #[test]
    fn normalization_ignores_case_whitespace_and_a_trailing_stop() {
        assert_eq!(
            normalize("The  gateway holds\n the key."),
            normalize("the gateway holds the key")
        );
        assert_ne!(
            normalize("the gateway holds the key"),
            normalize("the harness holds the key")
        );
    }

    #[test]
    fn the_prompt_carries_the_contract_the_schema_and_the_activity() {
        let prompt = Prompt::build(&chunk_of(&["we chose blocking threads"]), &[]);
        let text = prompt.as_str();

        assert!(text.contains("NEVER include a credential"));
        assert!(text.contains("\"memories\""));
        assert!(text.contains("we chose blocking threads"));
        assert!(text.contains("Session s1 at commit a938fcc"));
        assert!(text.contains("None."), "an empty project should say so");
    }

    /// The chunk is scrubbed by construction, so this is really a test that
    /// the prompt does not reintroduce what the chunk removed.
    #[test]
    fn the_prompt_never_carries_a_credential_from_session_activity() {
        let planted = "hunter2xyzabcdefghijklmn";
        let prompt = Prompt::build(
            &chunk_of(&[
                "export API_KEY=hunter2xyzabcdefghijklmn",
                "then we launched",
            ]),
            &[],
        );
        assert!(!prompt.as_str().contains(planted));
        assert!(prompt.as_str().contains("then we launched"));
    }

    /// A memory written before this module existed never passed a screen, so
    /// quoting it back to a model has to scrub it here.
    #[test]
    fn the_prompt_never_carries_a_credential_from_an_existing_memory() {
        let planted = "hunter2xyzabcdefghijklmn";
        let prompt = Prompt::build(
            &chunk_of(&["nothing interesting"]),
            &[format!("legacy row: API_KEY={planted}")],
        );
        assert!(
            !prompt.as_str().contains(planted),
            "a stored memory leaked a credential into the prompt"
        );
    }

    #[test]
    fn the_prompt_says_when_the_slice_is_partial() {
        let limits = chunk::ChunkLimits {
            max_entries: 2,
            max_entry_chars: 20,
            max_total_chars: 40,
        };
        let chunk = SessionChunk::build(
            "s1",
            None::<String>,
            (0..20).map(|i| format!("entry number {i} with some length to it")),
            limits,
        );
        let prompt = Prompt::build(&chunk, &[]);
        assert!(prompt.as_str().contains("earlier entries omitted"));
    }

    #[test]
    fn an_extraction_trigger_names_itself_for_the_record() {
        assert_eq!(ExtractionTrigger::Manual.to_string(), "manual");
        assert_eq!(
            ExtractionTrigger::TaskCompleted.to_string(),
            "task_completed"
        );
    }

    #[test]
    fn a_model_error_never_carries_a_provider_body() {
        // `Failed` takes a `&'static str`, so there is no way to put a
        // provider's response into one. This is a compile-time property; the
        // test records the intent for a reader.
        let err = ModelError::Failed {
            phrase: "the gateway refused the connection",
        };
        assert!(err.to_string().contains("the gateway refused"));
    }

    #[test]
    fn a_silent_model_still_names_itself_on_the_outcome() {
        assert_eq!(Silent.describe(), "test/silent");
    }
}
