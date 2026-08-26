//! The extraction contract: what a model is asked for, and what is accepted
//! back.
//!
//! # The response is untrusted input
//!
//! A model's reply is text from outside the program. It is parsed like any
//! other untrusted document: per element, so one bad memory costs one memory
//! rather than the whole extraction, and **screened for credential material
//! before any field of it is read**.
//!
//! That ordering is deliberate and is the reason [`judge`] does the screen
//! first. Once an element has passed, every field in it is known to be
//! credential-free, so a later error may safely name the value it rejected.
//! Screening field by field would leave the fields nobody reads unscreened,
//! and an error raised before the screen could echo a credential into a log.
//!
//! # Required fields, and why five of them are enums
//!
//! `kind` is Phase 21's *"classify every emitted memory into one supported
//! memory kind"*. The other three enums each exist to make a distinction the
//! map asks for **representable, and therefore checkable**:
//!
//! | field | the line it serves |
//! |---|---|
//! | `support` | *omit speculative claims that were not established* |
//! | `disposition` | *distinguish failed approaches from accepted decisions* |
//! | `confidence` | *treat uncertain authority classification conservatively* |
//!
//! A prompt that merely *asks* a model to distinguish two things produces no
//! evidence that it did. A schema that makes the distinction a required
//! field, and a validator that refuses a response contradicting itself,
//! produces a test. Which half is enforced and which half is only asked for
//! is stated per rule below, because the difference is the whole value.
//!
//! # What is asked for and cannot be enforced here
//!
//! Whether a statement is *true*, whether it is an obvious source-code fact,
//! and whether rediscovering it would be expensive are judgments about the
//! project that only the producer can make — the same three
//! `memory::policy` declined to fake at the storage layer, for the
//! same reason. They are stated in [`PROMPT_CONTRACT`] and evaluated
//! nowhere. Saying so is more useful than a keyword heuristic that would
//! refuse real memories and admit fake ones.

use serde::Deserialize;

use super::super::store::{DecisionProvenance, MemoryAuthority, MemoryKind, ProjectPhase};
use super::credentials::{self, CredentialFound};

/// The longest subject accepted.
pub const MAX_SUBJECT_CHARS: usize = 120;

/// The longest body accepted.
///
/// A durable memory is a sentence or two. Something far longer is a
/// transcript, and Phase 20 forbids storing raw conversation as project
/// memory — so the cap is a refusal rather than a truncation: cutting a
/// transcript to a thousand characters produces a memory that reads as
/// complete and is not.
pub const MAX_BODY_CHARS: usize = 1_000;

/// The longest rationale accepted. Phase 21 says *concise*.
pub const MAX_RATIONALE_CHARS: usize = 400;

/// The longest any single Phase 21B provenance field may be.
///
/// Shorter than the body, because each of these is one clause of an answer
/// and not the memory itself: *what problem this solved*, *what it assumed
/// about scale*. A paragraph in one of them is a sign the model is
/// summarising the session again, which is the thing this whole module
/// exists not to do.
pub const MAX_PROVENANCE_CHARS: usize = 300;

/// The longest source excerpt accepted.
///
/// Larger than the other provenance fields because it is a **quotation**
/// rather than a summary — Phase 21B asks to *"preserve the original wording
/// … sufficiently to audit how a remembered decision was derived"*, and a
/// clipped quotation audits nothing. Still bounded, and refused rather than
/// truncated for the reason [`MAX_BODY_CHARS`] is: a quotation cut short
/// reads as complete and is not.
pub const MAX_EXCERPT_CHARS: usize = 500;

/// Whether a claim was established during the session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Support {
    /// Settled during the session — decided, observed, measured, or agreed.
    Established,
    /// Guessed, proposed, or inferred without being confirmed. Never stored.
    Speculative,
}

/// What happened to the thing being remembered.
///
/// The field that separates *"we chose this"* from *"we tried this and it
/// did not work"* and from *"someone suggested this"*.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Disposition {
    /// Chosen, agreed, or in force.
    Accepted,
    /// Tried and given up, or ruled out.
    Abandoned,
    /// Raised and not resolved.
    Proposed,
}

/// How sure the model says it is.
///
/// Read as a *presentation characteristic and never as evidence* — Phase
/// 21K's own words. It is used in exactly one direction: to lower an
/// authority class, never to raise one. See
/// [`super::authority::conservative`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Confidence {
    Certain,
    Probable,
    Unsure,
}

macro_rules! contract_enum {
    ($ty:ty { $($variant:ident => $text:literal),+ $(,)? }) => {
        impl $ty {
            pub fn as_str(self) -> &'static str {
                match self {
                    $(Self::$variant => $text,)+
                }
            }

            /// Parse the spelling used in the JSON contract.
            pub fn from_contract(value: &str) -> Option<Self> {
                match value {
                    $($text => Some(Self::$variant),)+
                    _ => None,
                }
            }

            /// Every variant, in contract order. The prompt's schema is
            /// pinned against this, so a variant added without updating
            /// [`RESPONSE_SCHEMA`] fails a test instead of silently never
            /// being asked for.
            pub const ALL: &'static [Self] = &[$(Self::$variant,)+];
        }

        impl std::fmt::Display for $ty {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.pad(self.as_str())
            }
        }
    };
}

contract_enum!(Support {
    Established => "established",
    Speculative => "speculative",
});

contract_enum!(Disposition {
    Accepted => "accepted",
    Abandoned => "abandoned",
    Proposed => "proposed",
});

contract_enum!(Confidence {
    Certain => "certain",
    Probable => "probable",
    Unsure => "unsure",
});

/// One memory a model emitted, after validation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtractedMemory {
    pub kind: MemoryKind,
    /// What the model *declared*. Not what will be stored — see
    /// [`super::authority::conservative`], which may only lower it.
    pub declared_authority: MemoryAuthority,
    pub disposition: Disposition,
    pub confidence: Confidence,
    pub subject: Option<String>,
    pub body: String,
    /// Phase 21B: why the decision was made, and what it assumed.
    ///
    /// **The rationale used to be folded into `body`** behind a marker,
    /// because `memories` had nowhere else to put it and folding kept it in
    /// the FTS5 index. Migration 6 gave it a column and rebuilt the index
    /// over it, so the fold is gone: a consumer can now ask for a decision
    /// without its reasoning, and the reasoning can be revised without
    /// rewriting the decision.
    pub provenance: DecisionProvenance,
}

/// Why an emitted memory was not accepted.
///
/// Every variant is reported in [`super::ExtractionOutcome`], so a refusal
/// is visible rather than a memory that quietly never appeared.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Refusal {
    /// The element was not an object with the fields the contract requires.
    #[error("the model emitted something this contract cannot read: {detail}")]
    Malformed { detail: String },

    #[error("a memory needs `{field}`; this one omitted it")]
    MissingField { field: &'static str },

    #[error("`{field}` was `{value}`, which is not one of {expected}")]
    UnknownValue {
        field: &'static str,
        value: String,
        expected: &'static str,
    },

    #[error("`{field}` was {chars} characters; the contract allows {limit}")]
    TooLong {
        field: &'static str,
        chars: usize,
        limit: usize,
    },

    /// Phase 21: *distinguish failed approaches from accepted decisions*.
    ///
    /// Refused rather than reclassified. An element that calls the same
    /// thing an accepted decision and an abandoned approach has not made the
    /// distinction the contract requires, and guessing which half it meant
    /// would put Glasshouse's judgment behind the model's confusion.
    #[error(
        "a memory of kind `{kind}` cannot have disposition `{disposition}`: an \
         abandoned approach is a `failed_attempt` and nothing else, and a \
         `failed_attempt` is never `accepted`"
    )]
    ConflatedDisposition {
        kind: MemoryKind,
        disposition: Disposition,
    },

    /// Phase 21: *preserve concise rationale when a decision's rationale is
    /// important*.
    ///
    /// Enforced where importance is decidable: a decision that is being
    /// declared binding. A binding decision with no recorded reason is the
    /// exact shape Phase 21B calls lower-confidence and Phase 21E has to
    /// adjudicate later without the information it needs.
    #[error(
        "a decision declared as `{declared}` must carry its rationale: a \
         binding decision whose reason was not recorded cannot be revalidated"
    )]
    MissingRationale { declared: MemoryAuthority },

    /// The acceptance condition. Never stored, never redacted, never logged
    /// with its text.
    #[error(transparent)]
    Credential(#[from] CredentialFound),
}

/// What validation decided about one emitted element.
///
/// `Keep` is much larger than `Speculative`, and deliberately not boxed: a
/// verdict is produced and consumed within one loop iteration in
/// [`super::Extractor::run`], never stored and never collected, so boxing it
/// would buy an allocation per emitted memory and save nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(clippy::large_enum_variant)]
pub enum Verdict {
    /// Store it.
    Keep(ExtractedMemory),
    /// Phase 21: *omit speculative claims that were not established during
    /// the session*. Dropped, and counted — not an error, because a model
    /// labelling its own guess correctly is the contract working.
    Speculative,
}

/// The envelope the contract asks for.
#[derive(Debug, Deserialize)]
struct Envelope {
    /// Untyped on purpose: each element is judged on its own, so one
    /// unreadable memory does not discard the readable ones beside it.
    ///
    /// **Required**, and deliberately not `#[serde(default)]`. A default was
    /// the first version and it hid a real failure, found by a
    /// subcontractor probing envelope shapes: `extract_json_object` takes
    /// the first `{` wherever it sits, so a model that wrapped its reply in
    /// an array — `[{"kind": "finding"}]`, the shape you get from one
    /// mistaken bracket — had that inner object read as the whole envelope,
    /// found no `memories` key, defaulted to empty, and reported **"found
    /// nothing" with no failure at all**. Indistinguishable from a model
    /// that looked and found nothing.
    ///
    /// Requiring the key means there is exactly one way to say "nothing
    /// worth remembering" — `{"memories": []}` — and every other shape is a
    /// visible failure. Phase 21's promise is that the outcome describes
    /// what happened, and a silent zero does not.
    memories: Vec<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
struct RawMemory {
    /// Every field is optional here so that "absent" is this module's error
    /// with this module's wording, rather than serde's — and so that a
    /// missing `kind` is one refused memory instead of a failed document.
    #[serde(default)]
    kind: Option<String>,
    #[serde(default)]
    authority: Option<String>,
    #[serde(default)]
    disposition: Option<String>,
    #[serde(default)]
    support: Option<String>,
    #[serde(default)]
    confidence: Option<String>,
    #[serde(default)]
    subject: Option<String>,
    #[serde(default)]
    body: Option<String>,
    #[serde(default)]
    rationale: Option<String>,
    #[serde(default)]
    project_phase: Option<String>,
    #[serde(default)]
    problem: Option<String>,
    #[serde(default)]
    assumptions: Option<String>,
    #[serde(default)]
    scale_assumptions: Option<String>,
    #[serde(default)]
    security_assumptions: Option<String>,
    #[serde(default)]
    compatibility_assumptions: Option<String>,
    #[serde(default)]
    operational_assumptions: Option<String>,
    #[serde(default)]
    evidence: Option<String>,
    #[serde(default)]
    source_excerpt: Option<String>,
}

/// Read a model's whole reply into elements, or fail.
///
/// Tolerates the two things every model does to JSON: surrounding prose, and
/// a ```` ```json ```` fence. Nothing else — a reply this cannot find an
/// object in is a failure, not something to guess at.
pub fn parse(reply: &str) -> Result<Vec<serde_json::Value>, Refusal> {
    let body = extract_json_object(reply).ok_or_else(|| Refusal::Malformed {
        detail: "no JSON object in the reply".to_owned(),
    })?;

    let envelope: Envelope = serde_json::from_str(body).map_err(|err| Refusal::Malformed {
        // `err` names a line, a column and a type. It does not echo values,
        // and the whole reply is screened per element after this point.
        detail: err.to_string(),
    })?;
    Ok(envelope.memories)
}

/// The outermost `{…}` in `reply`, by brace balance.
///
/// Brace counting rather than `find('{')` and `rfind('}')`: a reply whose
/// prose contains a `}` after the object would otherwise capture it.
fn extract_json_object(reply: &str) -> Option<&str> {
    let start = reply.find('{')?;
    let mut depth = 0usize;
    let mut in_string = false;
    let mut escaped = false;

    for (offset, c) in reply[start..].char_indices() {
        if in_string {
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == '"' {
                in_string = false;
            }
            continue;
        }
        match c {
            '"' => in_string = true,
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(&reply[start..start + offset + c.len_utf8()]);
                }
            }
            _ => {}
        }
    }
    None
}

/// Validate one emitted element.
///
/// The credential screen runs **first**, over the element's own serialized
/// text, before a single field is read. See the module documentation.
pub fn judge(element: &serde_json::Value) -> Result<Verdict, Refusal> {
    let serialized = element.to_string();
    credentials::screen("memory", &serialized)?;

    let raw: RawMemory =
        serde_json::from_value(element.clone()).map_err(|err| Refusal::Malformed {
            detail: err.to_string(),
        })?;

    let kind = required_enum("kind", raw.kind.as_deref(), MemoryKind::from_stored, KINDS)?;
    let declared_authority = required_enum(
        "authority",
        raw.authority.as_deref(),
        MemoryAuthority::from_stored,
        AUTHORITIES,
    )?;
    let disposition = required_enum(
        "disposition",
        raw.disposition.as_deref(),
        Disposition::from_contract,
        DISPOSITIONS,
    )?;
    let support = required_enum(
        "support",
        raw.support.as_deref(),
        Support::from_contract,
        SUPPORTS,
    )?;
    let confidence = required_enum(
        "confidence",
        raw.confidence.as_deref(),
        Confidence::from_contract,
        CONFIDENCES,
    )?;

    // Phase 21: omit speculative claims. Checked after the enums so that a
    // speculative element with a broken `kind` is still reported as broken —
    // a model that cannot fill the schema in is worth knowing about even
    // when the memory was going to be dropped anyway.
    if support == Support::Speculative {
        return Ok(Verdict::Speculative);
    }

    let body = non_empty("body", raw.body)?;
    bound("body", &body, MAX_BODY_CHARS)?;

    let subject = raw.subject.filter(|s| !s.trim().is_empty());
    if let Some(subject) = &subject {
        bound("subject", subject, MAX_SUBJECT_CHARS)?;
    }

    let rationale = optional("rationale", raw.rationale, MAX_RATIONALE_CHARS)?;

    // Phase 21B, one field per line of the map. Every one is optional: the
    // map says "when known", "when they can be extracted reliably", "when
    // they influenced the decision", and a model that invents an assumption
    // to fill a required field has manufactured exactly the speculative
    // claim rule 3 forbids.
    let provenance = DecisionProvenance {
        rationale,
        project_phase: match raw.project_phase.as_deref().map(str::trim) {
            None | Some("") => None,
            Some(value) => {
                Some(
                    ProjectPhase::from_stored(value).ok_or_else(|| Refusal::UnknownValue {
                        field: "project_phase",
                        value: value.to_owned(),
                        expected: PROJECT_PHASES,
                    })?,
                )
            }
        },
        problem: optional("problem", raw.problem, MAX_PROVENANCE_CHARS)?,
        assumptions: optional("assumptions", raw.assumptions, MAX_PROVENANCE_CHARS)?,
        scale_assumptions: optional(
            "scale_assumptions",
            raw.scale_assumptions,
            MAX_PROVENANCE_CHARS,
        )?,
        security_assumptions: optional(
            "security_assumptions",
            raw.security_assumptions,
            MAX_PROVENANCE_CHARS,
        )?,
        compatibility_assumptions: optional(
            "compatibility_assumptions",
            raw.compatibility_assumptions,
            MAX_PROVENANCE_CHARS,
        )?,
        operational_assumptions: optional(
            "operational_assumptions",
            raw.operational_assumptions,
            MAX_PROVENANCE_CHARS,
        )?,
        evidence: optional("evidence", raw.evidence, MAX_PROVENANCE_CHARS)?,
        source_excerpt: optional("source_excerpt", raw.source_excerpt, MAX_EXCERPT_CHARS)?,
    };

    // Phase 21: distinguish failed approaches from accepted decisions.
    let conflated = match (kind, disposition) {
        (MemoryKind::FailedAttempt, Disposition::Abandoned) => false,
        (MemoryKind::FailedAttempt, _) => true,
        (_, Disposition::Abandoned) => true,
        _ => false,
    };
    if conflated {
        return Err(Refusal::ConflatedDisposition { kind, disposition });
    }

    // Phase 21: preserve concise rationale where importance is decidable.
    if kind == MemoryKind::Decision
        && declared_authority.is_binding()
        && provenance.rationale.is_none()
    {
        return Err(Refusal::MissingRationale {
            declared: declared_authority,
        });
    }

    Ok(Verdict::Keep(ExtractedMemory {
        kind,
        declared_authority,
        disposition,
        confidence,
        subject: subject.map(|s| s.trim().to_owned()),
        body: body.trim().to_owned(),
        provenance,
    }))
}

/// A field the contract does not require: absent, or present, trimmed and
/// bounded.
///
/// Whitespace-only becomes `None` rather than `Some("")` for the reason
/// `NewMemory::with_subject` gives — "nobody recorded one" and "the empty
/// string" are the same fact and only one of them should be representable —
/// and the bound is a **refusal**, not a truncation, for the reason
/// [`MAX_BODY_CHARS`] gives.
fn optional(
    field: &'static str,
    value: Option<String>,
    limit: usize,
) -> Result<Option<String>, Refusal> {
    let Some(text) = value
        .map(|text| text.trim().to_owned())
        .filter(|text| !text.is_empty())
    else {
        return Ok(None);
    };
    bound(field, &text, limit)?;
    Ok(Some(text))
}

fn required_enum<T>(
    field: &'static str,
    value: Option<&str>,
    parse: impl Fn(&str) -> Option<T>,
    expected: &'static str,
) -> Result<T, Refusal> {
    let value = value
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .ok_or(Refusal::MissingField { field })?;
    parse(value).ok_or_else(|| Refusal::UnknownValue {
        field,
        value: value.to_owned(),
        expected,
    })
}

fn non_empty(field: &'static str, value: Option<String>) -> Result<String, Refusal> {
    value
        .filter(|v| !v.trim().is_empty())
        .ok_or(Refusal::MissingField { field })
}

fn bound(field: &'static str, value: &str, limit: usize) -> Result<(), Refusal> {
    let chars = value.chars().count();
    if chars > limit {
        return Err(Refusal::TooLong {
            field,
            chars,
            limit,
        });
    }
    Ok(())
}

const KINDS: &str = "decision, constraint, feature, finding, failed_attempt, todo";
const AUTHORITIES: &str =
    "invariant, constraint, decision, preference, hypothesis, idea, historical";
const DISPOSITIONS: &str = "accepted, abandoned, proposed";
const SUPPORTS: &str = "established, speculative";
const CONFIDENCES: &str = "certain, probable, unsure";
const PROJECT_PHASES: &str = "prototype, alpha, beta, production, migration";

/// The JSON Schema the model is given.
///
/// A literal rather than something generated, because it is prose a model
/// reads as much as a schema a validator enforces — but it is **pinned
/// against the enums** by `the_response_schema_names_every_value_the_parser_
/// accepts`, so a kind or authority added to the store without being added
/// here fails a test rather than silently never being asked for.
pub const RESPONSE_SCHEMA: &str = r#"{
  "type": "object",
  "required": ["memories"],
  "properties": {
    "memories": {
      "type": "array",
      "items": {
        "type": "object",
        "required": ["kind", "authority", "disposition", "support", "confidence", "body"],
        "properties": {
          "kind": {
            "enum": ["decision", "constraint", "feature",
                     "finding", "failed_attempt", "todo"]
          },
          "authority": {
            "enum": ["invariant", "constraint", "decision", "preference",
                     "hypothesis", "idea", "historical"]
          },
          "disposition": { "enum": ["accepted", "abandoned", "proposed"] },
          "support":     { "enum": ["established", "speculative"] },
          "confidence":  { "enum": ["certain", "probable", "unsure"] },
          "subject":   { "type": "string", "maxLength": 120 },
          "body":      { "type": "string", "maxLength": 1000 },
          "rationale": { "type": "string", "maxLength": 400 },

          "project_phase": {
            "enum": ["prototype", "alpha", "beta", "production", "migration"]
          },
          "problem":                   { "type": "string", "maxLength": 300 },
          "assumptions":               { "type": "string", "maxLength": 300 },
          "scale_assumptions":         { "type": "string", "maxLength": 300 },
          "security_assumptions":      { "type": "string", "maxLength": 300 },
          "compatibility_assumptions": { "type": "string", "maxLength": 300 },
          "operational_assumptions":   { "type": "string", "maxLength": 300 },
          "evidence":                  { "type": "string", "maxLength": 300 },
          "source_excerpt":            { "type": "string", "maxLength": 500 }
        }
      }
    }
  }
}"#;

/// What the model is told, above the schema.
///
/// Every numbered rule here is either enforced by [`judge`] or explicitly
/// marked as unenforceable. Where a rule *is* enforced, saying so in the
/// prompt is not redundant: a model told the validator's rule produces a
/// usable memory instead of a refused one.
pub const PROMPT_CONTRACT: &str = "\
You are extracting durable project memory from one bounded slice of a coding \
session. You are not summarising the session and not writing a report.

Emit only what a future agent working on this project would need and could \
not cheaply rediscover by reading the code.

Rules, in order of importance:

 1. NEVER include a credential, API key, token, password or the value of any \
    secret-shaped variable, in any field. A memory containing one is \
    discarded whole. Do not describe where a credential is stored in terms \
    that would identify the credential.
 2. Classify every memory into exactly one `kind`. `decision` is a choice \
    that was accepted; `constraint` is a limit the project must work within; \
    `feature` is something the project has or will have; `finding` is \
    something established by investigation; `failed_attempt` is an approach \
    that was tried and given up; `todo` is work known to be outstanding.
 3. Omit anything that was not established during this slice. A guess, a \
    proposal nobody accepted, or an inference you cannot point at is \
    `support: speculative`, and speculative memories are discarded. If in \
    doubt, mark it speculative rather than establishing it by assertion.
 4. An approach that was tried and abandoned is `kind: failed_attempt` with \
    `disposition: abandoned`, and it is never a decision. An accepted \
    decision is never `disposition: abandoned`. Recording a failed approach \
    is valuable: it is what stops it being tried again.
 5. Something raised and not resolved is `disposition: proposed`. \
    Enthusiasm is not acceptance. A proposal discussed at length and never \
    agreed is `authority: idea`.
 6. `authority` is how binding the memory is, and it is not the same \
    question as `kind`. `invariant` must not be violated without review; \
    `constraint` is a current technical, security, legal or product limit; \
    `decision` may later be revisited; `preference` is a direction that must \
    not force complexity; `hypothesis` still needs validation; `idea` is \
    exploratory and must never be followed as an instruction; `historical` \
    explains the project without directing it.
 7. Distinguish a hard requirement from a convenient implementation choice. \
    A requirement imposed from outside the code — by security, by a platform, \
    by a person, by law — is `constraint`. A choice made because it was the \
    simplest thing that worked is `decision` or `preference`.
 8. Give `rationale` whenever the reason is what makes the memory worth \
    keeping. It is required for a decision you declare as `invariant`, \
    `constraint` or `decision`. Keep it to one sentence.
 9. Do not repeat a memory the project already holds unless something \
    material changed. The existing memories are listed below, if any.
10. Be honest in `confidence`. It is used only to make a memory less \
    binding, never more.
11. Record what would let a future agent decide whether the memory is still \
    true, and leave every one of these out when you do not know it. An \
    invented assumption is a speculative claim under rule 3. \
    `project_phase` is the stage the project was in — one of `prototype`, \
    `alpha`, `beta`, `production`, `migration`. `problem` is the task the \
    decision was meant to solve. `assumptions` is what made it reasonable, \
    and there are four narrower fields for the kinds that go stale on their \
    own: `scale_assumptions` (user count, request volume, data size, latency \
    target, deployment topology), `security_assumptions`, \
    `compatibility_assumptions` and `operational_assumptions` \
    (single-instance versus distributed, and the like). `evidence` names \
    benchmark results, incidents, tests, commits or external requirements \
    the memory rests on.
12. `source_excerpt` is the original wording, quoted, when it is what makes \
    the memory auditable later — not a paraphrase and not the whole \
    exchange. Rule 1 applies to it exactly as it applies to `body`: a \
    quotation containing a credential is a memory discarded whole.
13. A decision that records neither a rationale nor any assumption is \
    treated as lower-confidence than one that does. That is not a reason to \
    invent either.

Reply with one JSON object matching this schema and nothing else:
";

#[cfg(test)]
mod tests {
    use super::*;

    fn element(json: &str) -> serde_json::Value {
        serde_json::from_str(json).unwrap()
    }

    fn good() -> serde_json::Value {
        element(
            r#"{"kind":"finding","authority":"constraint","disposition":"accepted",
                "support":"established","confidence":"certain",
                "subject":"ConPTY reflows","body":"ConPTY renders into a screen buffer."}"#,
        )
    }

    #[test]
    fn a_well_formed_memory_is_kept() {
        let Verdict::Keep(memory) = judge(&good()).unwrap() else {
            panic!("expected a kept memory");
        };
        assert_eq!(memory.kind, MemoryKind::Finding);
        assert_eq!(memory.declared_authority, MemoryAuthority::Constraint);
        assert_eq!(memory.subject.as_deref(), Some("ConPTY reflows"));
    }

    #[test]
    fn every_memory_must_name_a_supported_kind() {
        let missing = element(
            r#"{"authority":"decision","disposition":"accepted","support":"established",
                "confidence":"certain","body":"x"}"#,
        );
        assert!(matches!(
            judge(&missing),
            Err(Refusal::MissingField { field: "kind" })
        ));

        let unknown = element(
            r#"{"kind":"architecture","authority":"decision","disposition":"accepted",
                "support":"established","confidence":"certain","body":"x"}"#,
        );
        assert!(matches!(
            judge(&unknown),
            Err(Refusal::UnknownValue { field: "kind", .. })
        ));
    }

    #[test]
    fn a_speculative_claim_is_dropped_rather_than_stored() {
        let speculative = element(
            r#"{"kind":"finding","authority":"hypothesis","disposition":"proposed",
                "support":"speculative","confidence":"unsure",
                "body":"ConPTY probably reflows."}"#,
        );
        assert_eq!(judge(&speculative).unwrap(), Verdict::Speculative);
    }

    #[test]
    fn an_abandoned_approach_cannot_be_filed_as_a_decision() {
        let conflated = element(
            r#"{"kind":"decision","authority":"decision","disposition":"abandoned",
                "support":"established","confidence":"certain",
                "rationale":"it did not work","body":"Use a second thread."}"#,
        );
        assert!(matches!(
            judge(&conflated),
            Err(Refusal::ConflatedDisposition {
                kind: MemoryKind::Decision,
                disposition: Disposition::Abandoned
            })
        ));
    }

    #[test]
    fn a_failed_attempt_cannot_claim_to_have_been_accepted() {
        let conflated = element(
            r#"{"kind":"failed_attempt","authority":"historical","disposition":"accepted",
                "support":"established","confidence":"certain","body":"Use a second thread."}"#,
        );
        assert!(matches!(
            judge(&conflated),
            Err(Refusal::ConflatedDisposition { .. })
        ));
    }

    #[test]
    fn a_binding_decision_must_carry_its_rationale() {
        let bare = element(
            r#"{"kind":"decision","authority":"constraint","disposition":"accepted",
                "support":"established","confidence":"certain","body":"Use blocking threads."}"#,
        );
        assert!(matches!(
            judge(&bare),
            Err(Refusal::MissingRationale { .. })
        ));

        let with_reason = element(
            r#"{"kind":"decision","authority":"constraint","disposition":"accepted",
                "support":"established","confidence":"certain",
                "rationale":"no async runtime is in the dependency set",
                "body":"Use blocking threads."}"#,
        );
        assert!(matches!(judge(&with_reason), Ok(Verdict::Keep(_))));
    }

    /// A preference is not binding, so its reason is not load-bearing in the
    /// way Phase 21B describes. Requiring it everywhere would turn a rule
    /// with a purpose into a form to fill in.
    #[test]
    fn a_non_binding_decision_needs_no_rationale() {
        let preference = element(
            r#"{"kind":"decision","authority":"preference","disposition":"accepted",
                "support":"established","confidence":"probable",
                "body":"Prefer explicit matches over wildcards."}"#,
        );
        assert!(matches!(judge(&preference), Ok(Verdict::Keep(_))));
    }

    #[test]
    fn a_memory_carrying_a_credential_is_refused_before_any_field_is_read() {
        let planted = "hunter2xyzabcdefghijklmn";
        let leaking = element(&format!(
            r#"{{"kind":"finding","authority":"constraint","disposition":"accepted",
                 "support":"established","confidence":"certain",
                 "body":"the gateway needs API_KEY={planted}"}}"#
        ));
        let err = judge(&leaking).unwrap_err();
        assert!(matches!(err, Refusal::Credential(_)));
        assert!(!format!("{err}").contains(planted));
    }

    /// The screen runs before parsing, so a credential in a field this
    /// contract does not even read is still refused.
    #[test]
    fn a_credential_in_an_unread_field_is_still_refused() {
        let planted = "hunter2xyzabcdefghijklmn";
        let leaking = element(&format!(
            r#"{{"kind":"finding","authority":"constraint","disposition":"accepted",
                 "support":"established","confidence":"certain","body":"fine",
                 "debug_note":"API_KEY={planted}"}}"#
        ));
        assert!(matches!(judge(&leaking), Err(Refusal::Credential(_))));
    }

    #[test]
    fn an_over_long_body_is_refused_rather_than_truncated() {
        let long = "x".repeat(MAX_BODY_CHARS + 1);
        let element = element(&format!(
            r#"{{"kind":"finding","authority":"constraint","disposition":"accepted",
                 "support":"established","confidence":"certain","body":"{long}"}}"#
        ));
        assert!(matches!(
            judge(&element),
            Err(Refusal::TooLong { field: "body", .. })
        ));
    }

    /// Migration 6 gave the rationale a column, so the body is the body.
    ///
    /// The assertion that matters is the **negative** one: this test replaced
    /// `the_rationale_is_folded_into_the_stored_body`, and a fold that came
    /// back would put the reason into the text every duplicate check
    /// normalizes and every consumer prints as the memory itself.
    #[test]
    fn a_rationale_is_kept_beside_the_body_and_never_inside_it() {
        let Verdict::Keep(memory) = judge(&element(
            r#"{"kind":"decision","authority":"decision","disposition":"accepted",
                "support":"established","confidence":"certain",
                "rationale":"no async runtime is in the dependency set",
                "body":"Use blocking threads."}"#,
        ))
        .unwrap() else {
            panic!("expected a kept memory");
        };

        assert_eq!(memory.body, "Use blocking threads.");
        assert_eq!(
            memory.provenance.rationale.as_deref(),
            Some("no async runtime is in the dependency set")
        );
        assert!(
            !memory.body.contains("async runtime"),
            "the rationale must not be folded into the body"
        );
    }

    /// Phase 21B, every field the map names, through the contract in one go.
    #[test]
    fn every_provenance_field_the_map_names_survives_validation() {
        let Verdict::Keep(memory) = judge(&element(
            r#"{"kind":"decision","authority":"decision","disposition":"accepted",
                "support":"established","confidence":"certain",
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
                "source_excerpt":"we agreed checkpoints go in the project db"}"#,
        ))
        .unwrap() else {
            panic!("expected a kept memory");
        };

        let provenance = &memory.provenance;
        assert_eq!(provenance.project_phase, Some(ProjectPhase::Alpha));
        assert_eq!(
            provenance.problem.as_deref(),
            Some("handing a session's context to a fresh one")
        );
        assert_eq!(
            provenance.assumptions.as_deref(),
            Some("one machine holds the project")
        );
        assert_eq!(
            provenance.scale_assumptions.as_deref(),
            Some("tens of sessions, not thousands")
        );
        assert_eq!(
            provenance.security_assumptions.as_deref(),
            Some("the database file is owner-only")
        );
        assert_eq!(
            provenance.compatibility_assumptions.as_deref(),
            Some("SQLite ships with the binary")
        );
        assert_eq!(
            provenance.operational_assumptions.as_deref(),
            Some("single-instance, no daemon")
        );
        assert_eq!(
            provenance.evidence.as_deref(),
            Some("the size cap is enforced by a CHECK")
        );
        assert_eq!(
            provenance.source_excerpt.as_deref(),
            Some("we agreed checkpoints go in the project db")
        );
        assert!(!provenance.is_thin());
    }

    /// The map says *"when known"* of every one of them, so a memory that
    /// knows none of them is a valid memory and not a refused one — and it
    /// is the shape Phase 21B calls lower-confidence.
    #[test]
    fn a_memory_that_records_no_provenance_is_kept_and_reads_as_thin() {
        let Verdict::Keep(memory) = judge(&element(
            r#"{"kind":"decision","authority":"preference","disposition":"accepted",
                "support":"established","confidence":"probable",
                "body":"Prefer explicit matches over wildcards."}"#,
        ))
        .unwrap() else {
            panic!("expected a kept memory");
        };
        assert!(memory.provenance.is_empty());
        assert!(memory.provenance.is_thin());
    }

    /// An assumption on its own is enough: the map's condition is *"missing
    /// rationale **and** missing assumptions"*.
    #[test]
    fn an_assumption_alone_is_enough_to_stop_a_decision_reading_as_thin() {
        let Verdict::Keep(memory) = judge(&element(
            r#"{"kind":"decision","authority":"preference","disposition":"accepted",
                "support":"established","confidence":"probable",
                "security_assumptions":"no user data crosses this path",
                "body":"Log the request path."}"#,
        ))
        .unwrap() else {
            panic!("expected a kept memory");
        };
        assert!(memory.provenance.rationale.is_none());
        assert!(memory.provenance.has_assumptions());
        assert!(!memory.provenance.is_thin());
    }

    /// `project_phase` is the one provenance field with a fixed vocabulary,
    /// and it is refused here rather than at the `CHECK` — a constraint
    /// violation on an extraction thread names a column, not a memory.
    #[test]
    fn a_project_phase_outside_the_maps_five_is_refused_by_name() {
        let unknown = element(
            r#"{"kind":"decision","authority":"preference","disposition":"accepted",
                "support":"established","confidence":"certain",
                "project_phase":"late-night","body":"Ship it."}"#,
        );
        assert!(matches!(
            judge(&unknown),
            Err(Refusal::UnknownValue {
                field: "project_phase",
                ..
            })
        ));

        for phase in ProjectPhase::ALL {
            let accepted = element(&format!(
                r#"{{"kind":"decision","authority":"preference","disposition":"accepted",
                     "support":"established","confidence":"certain",
                     "project_phase":"{}","body":"Ship it."}}"#,
                phase.as_str()
            ));
            let Verdict::Keep(memory) = judge(&accepted).unwrap() else {
                panic!("expected `{phase}` to be accepted");
            };
            assert_eq!(memory.provenance.project_phase, Some(*phase));
        }
    }

    /// A quotation is the field most likely to carry one, because it is
    /// verbatim session text rather than a model's paraphrase — and the
    /// screen runs over the whole element before any field is read, so it
    /// needs no per-field rule to cover it.
    #[test]
    fn a_credential_inside_a_source_excerpt_is_refused_like_any_other() {
        let planted = "hunter2xyzabcdefghijklmn";
        let leaking = element(&format!(
            r#"{{"kind":"decision","authority":"preference","disposition":"accepted",
                 "support":"established","confidence":"certain",
                 "source_excerpt":"we set API_KEY={planted} and it worked",
                 "body":"The gateway needs a key."}}"#
        ));
        let err = judge(&leaking).unwrap_err();
        assert!(matches!(err, Refusal::Credential(_)));
        assert!(!format!("{err}").contains(planted));
    }

    /// Over-long provenance is refused rather than truncated, for the reason
    /// an over-long body is: a clipped quotation audits nothing and a
    /// clipped assumption reads as the whole assumption.
    #[test]
    fn over_long_provenance_is_refused_rather_than_truncated() {
        let long = "x".repeat(MAX_PROVENANCE_CHARS + 1);
        let element = element(&format!(
            r#"{{"kind":"decision","authority":"preference","disposition":"accepted",
                 "support":"established","confidence":"certain",
                 "scale_assumptions":"{long}","body":"Ship it."}}"#
        ));
        assert!(matches!(
            judge(&element),
            Err(Refusal::TooLong {
                field: "scale_assumptions",
                ..
            })
        ));
    }

    #[test]
    fn a_reply_wrapped_in_prose_and_a_fence_still_parses() {
        let reply =
            "Here is what I found:\n```json\n{\"memories\": [{\"kind\": \"todo\"}]}\n```\nDone.";
        let elements = parse(reply).unwrap();
        assert_eq!(elements.len(), 1);
    }

    /// Brace counting rather than `rfind('}')`: prose after the object may
    /// contain a closing brace, and taking the last one would swallow it.
    #[test]
    fn trailing_prose_containing_a_brace_does_not_break_the_parse() {
        let reply = "{\"memories\": []}\nNote: the `}` above closes the object.";
        assert_eq!(parse(reply).unwrap().len(), 0);
    }

    #[test]
    fn a_reply_with_no_object_is_a_failure_rather_than_a_guess() {
        assert!(matches!(
            parse("I could not find anything worth remembering."),
            Err(Refusal::Malformed { .. })
        ));
    }

    /// A model that returns valid JSON in the wrong shape must not look like
    /// a model that found nothing.
    ///
    /// The first version of this test asserted the opposite — that a missing
    /// `memories` key yielded zero elements and no error — and it was wrong.
    /// See [`Envelope::memories`]: that choice made one real failure mode
    /// silent, and a subcontractor found it by probing envelope shapes rather
    /// than by reading the code.
    #[test]
    fn an_envelope_without_a_memories_key_is_a_failure_and_not_an_empty_result() {
        assert!(matches!(
            parse("{\"result\": \"none\"}"),
            Err(Refusal::Malformed { .. })
        ));
    }

    /// One mistaken bracket is the difference between a memory and silence,
    /// so it gets its own test: `extract_json_object` finds the first `{`
    /// wherever it sits, and that object must not be accepted as an envelope
    /// merely because it parses.
    #[test]
    fn a_reply_wrapped_in_an_array_is_a_failure_rather_than_a_silent_zero() {
        assert!(matches!(
            parse(r#"[{"kind": "finding", "body": "something real"}]"#),
            Err(Refusal::Malformed { .. })
        ));
    }

    /// And the one shape that *does* mean "nothing worth remembering" still
    /// works, because otherwise the fix would have made honesty impossible.
    #[test]
    fn an_empty_memories_array_is_the_one_way_to_say_nothing_was_found() {
        assert_eq!(parse("{\"memories\": []}").unwrap().len(), 0);
    }

    /// The prompt and the parser must agree. A kind or authority the store
    /// supports and the schema never mentions is a memory no model will ever
    /// emit.
    #[test]
    fn the_response_schema_names_every_value_the_parser_accepts() {
        for kind in MemoryKind::ALL {
            assert!(
                RESPONSE_SCHEMA.contains(kind.as_str()),
                "the schema never names the kind `{kind}`"
            );
        }
        for authority in MemoryAuthority::ALL {
            assert!(
                RESPONSE_SCHEMA.contains(authority.as_str()),
                "the schema never names the authority `{authority}`"
            );
        }
        for support in Support::ALL {
            assert!(RESPONSE_SCHEMA.contains(support.as_str()));
        }
        for disposition in Disposition::ALL {
            assert!(RESPONSE_SCHEMA.contains(disposition.as_str()));
        }
        for confidence in Confidence::ALL {
            assert!(RESPONSE_SCHEMA.contains(confidence.as_str()));
        }
    }

    /// The schema's own caps must be the ones the validator enforces, or a
    /// model obeying the schema gets refused by the parser.
    #[test]
    fn the_schema_declares_the_same_limits_the_parser_enforces() {
        assert!(RESPONSE_SCHEMA.contains(&MAX_SUBJECT_CHARS.to_string()));
        assert!(RESPONSE_SCHEMA.contains(&MAX_BODY_CHARS.to_string()));
        assert!(RESPONSE_SCHEMA.contains(&MAX_RATIONALE_CHARS.to_string()));
        assert!(RESPONSE_SCHEMA.contains(&MAX_PROVENANCE_CHARS.to_string()));
        assert!(RESPONSE_SCHEMA.contains(&MAX_EXCERPT_CHARS.to_string()));
    }

    /// A provenance field the store has a column for and the schema never
    /// mentions is a column no model will ever fill — the same argument
    /// `the_response_schema_names_every_value_the_parser_accepts` makes for
    /// the enums, applied to Phase 21B's nine fields.
    #[test]
    fn the_response_schema_asks_for_every_provenance_field_the_store_keeps() {
        for field in [
            "rationale",
            "project_phase",
            "problem",
            "assumptions",
            "scale_assumptions",
            "security_assumptions",
            "compatibility_assumptions",
            "operational_assumptions",
            "evidence",
            "source_excerpt",
        ] {
            assert!(
                RESPONSE_SCHEMA.contains(&format!("\"{field}\"")),
                "the schema never asks for `{field}`"
            );
            assert!(
                PROMPT_CONTRACT.contains(field),
                "the contract never explains `{field}` to the model"
            );
        }

        for phase in ProjectPhase::ALL {
            assert!(
                RESPONSE_SCHEMA.contains(phase.as_str()),
                "the schema never names the project phase `{phase}`"
            );
        }
    }

    #[test]
    fn the_contract_states_the_credential_rule_first() {
        let credential_rule = PROMPT_CONTRACT.find("NEVER include a credential").unwrap();
        let next_rule = PROMPT_CONTRACT.find("Classify every memory").unwrap();
        assert!(
            credential_rule < next_rule,
            "the credential rule must lead the contract"
        );
    }
}
