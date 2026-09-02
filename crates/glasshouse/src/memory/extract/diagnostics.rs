//! Extraction diagnostics — capability map line 1769: one JSON line per
//! extraction run, appended to `<state_dir>/memory-extraction.jsonl` only
//! when `[memory] extraction_diagnostics` is on
//! ([`crate::config::EffectiveConfig::memory_extraction_diagnostics`]).
//!
//! Modeled on [`crate::memory::rerank::append_diagnostics`]'s own shape —
//! one `create(true).append(true)` open and one `write_all`, fail-soft, a
//! path under `runtime.state_dir()` and therefore project-scoped — and on
//! that module's own `serde`-encoded record: this module, unlike
//! `crate::evaluation`, carries no pin against a general-purpose
//! serializer (verified: `memory::extract::schema` and
//! `memory::extract::model` already depend on `serde`/`serde_json` for the
//! extraction contract itself), so the line is encoded the same way
//! `rerank`'s is rather than hand-assembled.
//!
//! # What never reaches this file
//!
//! The prompt, a memory's body or subject, and a rejection's own free text
//! (a model's malformed reply, an unknown field value, a store's rendered
//! error) never appear here — only ids, the closed vocabulary words this
//! module maps each reason to, and counts. [`ExtractionOutcome`] itself
//! carries the prompt nowhere ([`super::Prompt`] has no accessor that would
//! let it), so the guarantee this module adds is narrower: every *reason*
//! recorded here is a fixed word or a schema field name, never a value
//! copied from the model's reply.

use std::io::Write as _;

use serde::Serialize;

use super::authority::Classification;
use super::schema::Refusal;
use super::{ExtractionFailure, ExtractionOutcome, ModelError, Rejection};

/// One stored memory, as this diagnostics line names it — the id and the
/// kind, never the body or subject. [`ExtractionOutcome::recorded`] carries
/// only the id, so the kind is resolved by a fresh, fail-soft store lookup
/// in `build` (private to this module) rather than by widening that type for one debugging
/// surface.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DiagnosticsRecorded {
    pub id: String,
    pub kind: &'static str,
}

/// One memory whose stored authority was weaker than the model declared —
/// [`Classification::declared`] and [`Classification::stored`], both closed
/// vocabulary words.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DiagnosticsLowered {
    pub id: String,
    pub from: &'static str,
    pub to: &'static str,
}

/// One rejected memory, by `rejection_reason`'s (private to this module) closed vocabulary —
/// never [`Rejection`]'s own `Display`, which renders a model's or a
/// store's free text.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DiagnosticsRejected {
    pub kind: &'static str,
    pub reason: String,
}

/// What one provider call reported, present only when
/// [`ExtractionOutcome::call`] is [`Some`] — a run that reached no
/// provider has no call to describe.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DiagnosticsCall {
    pub input_tokens: Option<i64>,
    pub output_tokens: Option<i64>,
    /// `None` unless both endpoints of the call were timed. One-second
    /// resolution: [`super::ModelCall`]'s own fields are whole Unix
    /// seconds.
    pub duration_ms: Option<i64>,
}

/// One [`ExtractionOutcome`], as one line of `memory-extraction.jsonl`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ExtractionDiagnostics {
    pub trigger: &'static str,
    pub model: String,
    pub session_id: String,
    pub commit: Option<String>,
    pub activity_dropped: usize,
    pub activity_truncated: usize,
    pub redactions: usize,
    pub speculative: usize,
    pub duplicates: usize,
    pub recorded: Vec<DiagnosticsRecorded>,
    pub lowered: Vec<DiagnosticsLowered>,
    pub rejected: Vec<DiagnosticsRejected>,
    pub failure: Option<&'static str>,
    pub call: Option<DiagnosticsCall>,
}

/// [`Rejection`] reduced to a closed vocabulary word — the field names a
/// [`Refusal`] carries (`kind`, `field`, `declared`, ...) are `&'static
/// str` literals fixed in `schema.rs`, safe to include; the values beside
/// them (`detail`, `value`, a store's rendered message) are not, and are
/// dropped.
fn rejection_reason(rejection: &Rejection) -> DiagnosticsRejected {
    match rejection {
        Rejection::Contract(refusal) => DiagnosticsRejected {
            kind: "contract",
            reason: refusal_reason(refusal),
        },
        Rejection::Store(_) => DiagnosticsRejected {
            kind: "store",
            reason: "refused".to_owned(),
        },
    }
}

fn refusal_reason(refusal: &Refusal) -> String {
    match refusal {
        Refusal::Malformed { .. } => "malformed".to_owned(),
        Refusal::MissingField { field } => format!("missing_field:{field}"),
        Refusal::UnknownValue { field, .. } => format!("unknown_value:{field}"),
        Refusal::TooLong { field, .. } => format!("too_long:{field}"),
        Refusal::ConflatedDisposition { .. } => "conflated_disposition".to_owned(),
        Refusal::MissingRationale { .. } => "missing_rationale".to_owned(),
        Refusal::Credential(_) => "credential_found".to_owned(),
    }
}

/// [`ExtractionFailure`] reduced to a fixed word — never [`Rejection`]'s or
/// [`ExtractionFailure`]'s own `Display`, which can carry a
/// [`ModelError::Failed`] phrase or a store's rendered message.
fn failure_word(failure: &ExtractionFailure) -> &'static str {
    match failure {
        ExtractionFailure::NothingToExtract => "nothing_to_extract",
        ExtractionFailure::Model(ModelError::Unavailable) => "model_unavailable",
        ExtractionFailure::Model(ModelError::Refused) => "model_refused",
        ExtractionFailure::Model(ModelError::TimedOut) => "model_timed_out",
        ExtractionFailure::Model(ModelError::Failed { .. }) => "model_failed",
        ExtractionFailure::Model(ModelError::Declined { .. }) => "model_declined",
        ExtractionFailure::Reply(_) => "reply_unreadable",
        ExtractionFailure::ModelPanicked => "model_panicked",
        ExtractionFailure::StoreUnreadable(_) => "store_unreadable",
    }
}

/// Assemble [`ExtractionDiagnostics`] from `outcome`, resolving each
/// recorded id's kind through one fresh, fail-soft store connection.
///
/// A second connection rather than threading the `Extractor`'s own store
/// through: by the time a caller decides whether to write diagnostics, the
/// extraction that produced `outcome` has already finished and its store
/// borrow has gone out of scope (`main.rs::run_extraction` runs the
/// extractor on its own thread and only the outcome crosses back). A
/// lookup that fails — the project database is gone, a record was deleted
/// between the store and this read — renders that one entry's kind as
/// `"unknown"` rather than dropping the id or aborting the whole line: the
/// id itself came from `outcome`, which is trusted, and a partial kind is
/// still a diagnostics line worth having.
fn build(runtime: &crate::Runtime, outcome: &ExtractionOutcome) -> ExtractionDiagnostics {
    let memory = crate::memory::ProjectMemory::open(runtime).ok();
    let store = memory.as_ref().map(crate::memory::ProjectMemory::store);

    let recorded = outcome
        .recorded
        .iter()
        .map(|id| DiagnosticsRecorded {
            id: id.as_str().to_owned(),
            kind: store
                .as_ref()
                .and_then(|store| store.get(id).ok().flatten())
                .map_or("unknown", |record| record.kind.as_str()),
        })
        .collect();

    let lowered = outcome
        .lowered
        .iter()
        .map(
            |(id, classification): &(_, Classification)| DiagnosticsLowered {
                id: id.as_str().to_owned(),
                from: classification.declared.as_str(),
                to: classification.stored.as_str(),
            },
        )
        .collect();

    let rejected = outcome.rejected.iter().map(rejection_reason).collect();

    let call = outcome.call.as_ref().map(|call| DiagnosticsCall {
        input_tokens: call.usage.input_tokens,
        output_tokens: call.usage.output_tokens,
        duration_ms: match (call.dispatched_at_unix, call.completed_at_unix) {
            (Some(dispatched), Some(completed)) => Some((completed - dispatched) * 1000),
            _ => None,
        },
    });

    ExtractionDiagnostics {
        trigger: outcome.trigger.as_str(),
        model: outcome.model.clone(),
        session_id: outcome.session_id.clone(),
        commit: outcome.commit.clone(),
        activity_dropped: outcome.activity_dropped,
        activity_truncated: outcome.activity_truncated,
        redactions: outcome.redactions,
        speculative: outcome.speculative,
        duplicates: outcome.duplicates,
        recorded,
        lowered,
        rejected,
        failure: outcome.failure.as_ref().map(failure_word),
        call,
    }
}

/// Append one line describing `outcome` to
/// `<state_dir>/memory-extraction.jsonl` — project-scoped, since `runtime`
/// resolves exactly one project's state directory.
///
/// **The caller gates this on the knob; this function does not.** Matching
/// [`crate::memory::rerank::append_diagnostics`]'s own division of labor:
/// the config read belongs to the call site that already has a `Runtime`
/// and a `Layered` reader in scope
/// (`main.rs::memory_extraction_diagnostics_enabled`), not to the writer.
///
/// Fail-soft throughout, matching every other bookkeeping producer in this
/// crate (`crate::evaluation::record_session_route`'s own doc comment
/// states the rule this follows): a write error is one `tracing::warn!`
/// and a return, never a propagated error. The extraction this describes
/// has already finished and already been reported to its caller; a
/// diagnostics line that could not be written must not make that outcome
/// look like it failed.
pub fn append_diagnostics(runtime: &crate::Runtime, outcome: &ExtractionOutcome) {
    let record = build(runtime, outcome);
    let path = runtime.state_dir().join("memory-extraction.jsonl");
    let line = match serde_json::to_string(&record) {
        Ok(line) => line,
        Err(err) => {
            tracing::warn!(error = %err, "could not encode an extraction diagnostics record");
            return;
        }
    };
    let mut file = match std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    {
        Ok(file) => file,
        Err(err) => {
            tracing::warn!(
                error = %err,
                path = %path.display(),
                "could not open the extraction diagnostics file"
            );
            return;
        }
    };
    if let Err(err) = writeln!(file, "{line}") {
        tracing::warn!(
            error = %err,
            path = %path.display(),
            "could not append an extraction diagnostics record"
        );
    }
}
