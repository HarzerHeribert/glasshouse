//! The decision model: typed questions over the gateway, and the one hold a
//! read-only intent buys against an effectful cell or direct frame
//! (`docs/product/pane/decision-model.md`).
//!
//! A decision adds a hold or a count. It never adds a capability, grant or
//! approval; a held cell re-issued passes exactly the checks it passed
//! before. The request carries the request text and goes only to the
//! gateway's base URL with the gateway's own credential -- no secret enters
//! this module, and [`DecideError`] and every notice or telemetry string
//! carry no body beyond [`ERROR_BODY_LIMIT`] bytes.

use std::collections::BTreeMap;
use std::fmt;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::config::DecisionMode;
use crate::wire;

/// The decision endpoint's path, appended to [`wire::base_url`] exactly as
/// the Messages path is appended in `wire.rs`.
const DECISION_PATH: &str = "/v1/systemone";

/// The header the gateway routes the decision request on, mirroring
/// `wire::MODEL_HEADER`'s name -- private there, so it is spelled once more
/// here rather than widening that module's visibility for one literal.
const MODEL_HEADER: &str = "x-glasshouse-model";

/// The purpose header, so the gateway and the ledger can tell a decision
/// request from a task turn or a helper call (`helpers.rs::PURPOSE_HEADER`
/// is the same shape for `"helper"`).
const PURPOSE_HEADER: (&str, &str) = ("x-glasshouse-purpose", "decision");

/// The decision request's own bound -- a side errand, not the 120 s task
/// turn ceiling `helpers.rs` uses.
const DECISION_TIMEOUT: Duration = Duration::from_secs(2);

/// The most of a request or response body an error or notice carries.
const ERROR_BODY_LIMIT: usize = 200;

/// The most of the task diff the completion question's `state.diff` carries
/// (2616). Cut at a hunk boundary (`bound_diff`) rather than a raw byte
/// count, so a kept hunk is never split mid-way; a cut diff still gets a
/// question, with `state.diff_truncated = true`.
pub const DIFF_STATE_BYTES: usize = 64 * 1024;

/// One question sent to the decision model. `Noul` is a numeric confidence
/// question the wire protocol supports; this package asks only `Choice`
/// questions, and `Noul` stays here as the wire's other documented shape.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum Question {
    Noul {
        instructions: String,
    },
    Choice {
        instructions: String,
        criteria: BTreeMap<String, String>,
    },
}

/// One answer, decoded from the documented response shape. `probabilities`
/// is the model's distribution over every named criterion, `confidence` its
/// probability on the returned `choice`.
#[derive(Debug, Clone, PartialEq)]
pub enum Answer {
    Noul(f64),
    Choice {
        choice: String,
        probabilities: BTreeMap<String, f64>,
        confidence: f64,
    },
}

/// One question's answer plus the round-trip latency of the request that
/// carried it -- every answer from one call shares the same latency.
#[derive(Debug, Clone, PartialEq)]
pub struct Decision {
    pub key: String,
    pub answer: Answer,
    pub latency_ms: u64,
}

/// One `decide` call's answers, keyed by the questions asked.
#[derive(Debug, Clone, PartialEq)]
pub struct Answers {
    pub model: String,
    pub decisions: Vec<Decision>,
}

/// Everything that can go wrong asking the decision model. Every variant's
/// `Display` carries no body beyond [`ERROR_BODY_LIMIT`] bytes -- the state a
/// caller sends may hold the request verbatim.
#[derive(Debug)]
pub enum DecideError {
    Status { status: u16, body_head: String },
    Transport(String),
    Timeout,
    Parse(String),
}

impl fmt::Display for DecideError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DecideError::Status { status, body_head } => {
                write!(f, "http {status} — {body_head}")
            }
            DecideError::Transport(message) => write!(f, "transport error: {message}"),
            DecideError::Timeout => write!(f, "timeout after {} ms", DECISION_TIMEOUT.as_millis()),
            DecideError::Parse(message) => write!(f, "could not parse response: {message}"),
        }
    }
}

fn truncate(text: &str) -> String {
    let mut cut = text.len().min(ERROR_BODY_LIMIT);
    while !text.is_char_boundary(cut) {
        cut -= 1;
    }
    text[..cut].to_string()
}

#[derive(Serialize)]
struct RequestBody<'a> {
    state: &'a Value,
    model: &'a str,
    questions: BTreeMap<String, Question>,
}

#[derive(Deserialize)]
struct ResponseBody {
    model: String,
    answers: BTreeMap<String, RawAnswer>,
    #[allow(dead_code)]
    usage: UsageFields,
}

#[derive(Deserialize)]
struct UsageFields {
    #[allow(dead_code)]
    input_tokens: u64,
    #[allow(dead_code)]
    output_tokens: u64,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
enum RawAnswer {
    Noul {
        noul: f64,
    },
    Choice {
        choice: String,
        probabilities: BTreeMap<String, f64>,
        confidence: f64,
    },
}

impl From<RawAnswer> for Answer {
    fn from(raw: RawAnswer) -> Self {
        match raw {
            RawAnswer::Noul { noul } => Answer::Noul(noul),
            RawAnswer::Choice {
                choice,
                probabilities,
                confidence,
            } => Answer::Choice {
                choice,
                probabilities,
                confidence,
            },
        }
    }
}

/// One metered request to the decision model: `state` plus every question,
/// answered with one [`Decision`] per question. Bounded to
/// [`DECISION_TIMEOUT`], never the 120 s side-errand timeout -- a decision is
/// asked before the model's first turn, and nothing about a task waits on it
/// past this bound.
pub fn decide(
    model: &str,
    state: Value,
    questions: &[(String, Question)],
) -> Result<Answers, DecideError> {
    let url = format!("{}{DECISION_PATH}", wire::base_url());
    let body = RequestBody {
        state: &state,
        model,
        questions: questions.iter().cloned().collect(),
    };
    let payload = serde_json::to_vec(&body)
        .map_err(|error| DecideError::Parse(truncate(&error.to_string())))?;

    let mut request = ureq::post(&url)
        .config()
        .http_status_as_error(false)
        .timeout_global(Some(DECISION_TIMEOUT))
        .build()
        .header("content-type", "application/json")
        .header(MODEL_HEADER, model)
        .header(PURPOSE_HEADER.0, PURPOSE_HEADER.1);
    if let Some((name, value)) = wire::credential_header() {
        request = request.header(name, value);
    }

    let started = Instant::now();
    let mut response = request
        .send(payload.as_slice())
        .map_err(|error| match error {
            ureq::Error::Timeout(_) => DecideError::Timeout,
            other => DecideError::Transport(truncate(&other.to_string())),
        })?;
    let status = response.status().as_u16();
    let text = response
        .body_mut()
        .read_to_string()
        .map_err(|error| DecideError::Transport(truncate(&error.to_string())))?;
    if !response.status().is_success() {
        return Err(DecideError::Status {
            status,
            body_head: truncate(&text),
        });
    }
    let latency_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);

    let parsed: ResponseBody = serde_json::from_str(&text)
        .map_err(|error| DecideError::Parse(truncate(&error.to_string())))?;
    let decisions = questions
        .iter()
        .map(|(key, _)| {
            let raw = parsed
                .answers
                .get(key)
                .cloned()
                .ok_or_else(|| DecideError::Parse(format!("missing answer for `{key}`")))?;
            Ok(Decision {
                key: key.clone(),
                answer: Answer::from(raw),
                latency_ms,
            })
        })
        .collect::<Result<Vec<_>, DecideError>>()?;

    Ok(Answers {
        model: parsed.model,
        decisions,
    })
}

/// The one string the intent question's answer holds when this package
/// should consider holding: `Hold::hold_for` compares against this exact
/// spelling, which is also the criterion's own key below.
/// `session/task.rs::gate` compares against it too, to choose the
/// completion question's answer state over its diff state (2641/2642's
/// addendum to 2616).
pub const READ_ONLY: &str = "read_only";

const INTENT_KEY: &str = "intent";

/// The choice `preflight::should_scout`'s decided signal fires on -- the
/// criterion's own key below, mirroring [`READ_ONLY`]'s shape for the other
/// question this package asks in the same request.
pub const NEEDS_EXPLORATION: &str = "needs_exploration";

const COMPLEXITY_KEY: &str = "complexity";

fn intent_question() -> Question {
    let mut criteria = BTreeMap::new();
    criteria.insert(
        READ_ONLY.to_string(),
        "answering, explaining, reading, searching or inspecting files, output or history; \
         nothing on disk is meant to change"
            .to_string(),
    );
    criteria.insert(
        "modify".to_string(),
        "creating, editing, deleting, moving or renaming files, or changing configuration"
            .to_string(),
    );
    criteria.insert(
        "run".to_string(),
        "building, testing, running or executing commands whose side effects are the point"
            .to_string(),
    );
    criteria.insert(
        "other".to_string(),
        "none of the above, or too unclear to say".to_string(),
    );
    Question::Choice {
        instructions: "What does this request intend?".to_string(),
        criteria,
    }
}

/// The complexity question (F2, map 2614/2615's paragraph): asked in the
/// same request as [`intent_question`], never a second round trip
/// (`docs/product/pane/decision-model.md`).
fn complexity_question() -> Question {
    let mut criteria = BTreeMap::new();
    criteria.insert(
        "trivial".to_string(),
        "one obvious edit or answer, no exploration needed".to_string(),
    );
    criteria.insert(
        "routine".to_string(),
        "a known shape of change in a known place".to_string(),
    );
    criteria.insert(
        NEEDS_EXPLORATION.to_string(),
        "the request needs the project read or searched before any change is safe".to_string(),
    );
    Question::Choice {
        instructions: "How much exploration does this request need before it is safe to act?"
            .to_string(),
        criteria,
    }
}

/// What the decision model answered about one request's intent.
#[derive(Debug, Clone, PartialEq)]
pub struct Intent {
    pub choice: String,
    pub confidence: f64,
    pub latency_ms: u64,
}

/// What the decision model answered about how much exploration one request
/// needs -- asked beside [`Intent`] in the same request, never on its own
/// (`preflight::should_scout`'s fifth signal, F2).
#[derive(Debug, Clone, PartialEq)]
pub struct Complexity {
    pub choice: String,
    pub confidence: f64,
}

/// Both answers to the one request asked before a task's first turn.
#[derive(Debug, Clone, PartialEq)]
pub struct TaskDecision {
    pub intent: Intent,
    pub complexity: Complexity,
}

/// Asks the intent and complexity questions about `request` in one request
/// and answers with what came back, or the reason it did not. Never
/// surfaced as a task failure -- the caller records the error in a notice
/// and proceeds exactly as if no decision model were configured.
pub fn task_questions(model: &str, request: &str) -> Result<TaskDecision, DecideError> {
    let state = serde_json::json!({ "request": request });
    let questions = [
        (INTENT_KEY.to_string(), intent_question()),
        (COMPLEXITY_KEY.to_string(), complexity_question()),
    ];
    let answers = decide(model, state, &questions)?;
    task_decision_of(answers)
}

/// The decoding half of [`task_questions`], pulled out so a unit test can
/// exercise it against a scripted [`Answers`] with no network involved.
fn task_decision_of(answers: Answers) -> Result<TaskDecision, DecideError> {
    let mut intent = None;
    let mut complexity = None;
    for decision in answers.decisions {
        let Answer::Choice {
            choice, confidence, ..
        } = decision.answer
        else {
            return Err(DecideError::Parse(format!(
                "the `{}` question was answered as a noul, not a choice",
                decision.key
            )));
        };
        match decision.key.as_str() {
            key if key == INTENT_KEY => {
                intent = Some(Intent {
                    choice,
                    confidence,
                    latency_ms: decision.latency_ms,
                });
            }
            key if key == COMPLEXITY_KEY => {
                complexity = Some(Complexity { choice, confidence });
            }
            other => {
                return Err(DecideError::Parse(format!(
                    "unexpected answer key `{other}`"
                )));
            }
        }
    }
    let intent =
        intent.ok_or_else(|| DecideError::Parse(format!("no answer for `{INTENT_KEY}`")))?;
    let complexity = complexity
        .ok_or_else(|| DecideError::Parse(format!("no answer for `{COMPLEXITY_KEY}`")))?;
    Ok(TaskDecision { intent, complexity })
}

/// The completion question's key, mirroring [`INTENT_KEY`]'s shape for the
/// other question this package asks.
const SATISFIED_KEY: &str = "satisfied";

/// The five diff-hygiene questions' keys (2641).
const HYGIENE_HAS_TESTS_KEY: &str = "has_tests";
const HYGIENE_OUT_OF_SCOPE_KEY: &str = "out_of_scope";
const HYGIENE_DEBUG_LEFTOVERS_KEY: &str = "debug_leftovers";
const HYGIENE_DELETES_TESTS_KEY: &str = "deletes_tests";
const HYGIENE_CHANGES_SIGNATURE_KEY: &str = "changes_signature";

/// What the completion question's shared `state` carries: the diff and the
/// mechanical findings already found, when there is a diff to show -- or,
/// when the diff is empty or the task's intent was [`READ_ONLY`], the
/// model's own answer text instead. Asking about a diff that does not exist
/// is the empty-diff defect Phase 66's shadow calibration measured: a
/// read-only, answer-only task scored 0.10 against a diff state that was
/// never anything but the placeholder sentence.
pub enum CompletionState<'a> {
    Diff {
        diff: &'a str,
        findings: &'a [String],
    },
    Answer {
        answer: &'a str,
    },
}

fn satisfied_question(state: &CompletionState<'_>) -> Question {
    let instructions = match state {
        CompletionState::Diff { .. } => {
            "Does the diff satisfy what the request asked for? Answer near 1.0 when \
             nothing the request asked for is missing and nothing unasked was changed; \
             answer near 0.0 when the diff clearly does not satisfy the request."
        }
        CompletionState::Answer { .. } => {
            "Does the answer satisfy what the request asked for? Answer near 1.0 when the \
             answer fully addresses the request; answer near 0.0 when it clearly does not."
        }
    };
    Question::Noul {
        instructions: instructions.to_string(),
    }
}

/// The five diff-hygiene questions (2641), asked in the same request as
/// [`satisfied_question`] whenever [`CompletionState::Diff`] is in force --
/// never for [`CompletionState::Answer`], which has no diff to ask about.
fn hygiene_questions() -> [(&'static str, Question); 5] {
    let noul = |text: &str| Question::Noul {
        instructions: text.to_string(),
    };
    [
        (
            HYGIENE_HAS_TESTS_KEY,
            noul(
                "Does the diff add or change tests for the behaviour it changes? Answer \
                 near 1.0 when it does; answer near 0.0 when it does not.",
            ),
        ),
        (
            HYGIENE_OUT_OF_SCOPE_KEY,
            noul(
                "Does the diff change files the request did not ask about? Answer near \
                 1.0 when it does; answer near 0.0 when it does not.",
            ),
        ),
        (
            HYGIENE_DEBUG_LEFTOVERS_KEY,
            noul(
                "Does the diff leave debugging artefacts: prints, commented-out code, or \
                 TODO markers? Answer near 1.0 when it does; answer near 0.0 when it does \
                 not.",
            ),
        ),
        (
            HYGIENE_DELETES_TESTS_KEY,
            noul(
                "Does the diff delete or disable tests? Answer near 1.0 when it does; \
                 answer near 0.0 when it does not.",
            ),
        ),
        (
            HYGIENE_CHANGES_SIGNATURE_KEY,
            noul(
                "Does the diff change a public function or type signature? Answer near \
                 1.0 when it does; answer near 0.0 when it does not.",
            ),
        ),
    ]
}

fn judge_key(index: usize) -> String {
    format!("judge_{index}")
}

/// One judge item's question (2642): the item's own text and whatever
/// evidence the acceptance list already gathered for it -- embedded in the
/// instructions, since [`decide`]'s `state` is shared across every question
/// in the request.
fn judge_question(item: &str, evidence: &str) -> Question {
    let instructions = if evidence.is_empty() {
        format!(
            "Does this acceptance item hold, given the diff or answer above? Answer near \
             1.0 when it clearly holds; answer near 0.0 when it clearly does not. \
             Item: {item}"
        )
    } else {
        format!(
            "Does this acceptance item hold, given the diff or answer above? Answer near \
             1.0 when it clearly holds; answer near 0.0 when it clearly does not. \
             Item: {item}\nEvidence already gathered: {evidence}"
        )
    };
    Question::Noul { instructions }
}

/// The five diff-hygiene questions' answers (2641), `None` for
/// [`CompletionState::Answer`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HygieneAnswer {
    pub has_tests: f64,
    pub out_of_scope: f64,
    pub debug_leftovers: f64,
    pub deletes_tests: f64,
    pub changes_signature: f64,
}

/// What the completion question answered (2616, extended 2641/2642): a
/// probability that the diff or answer satisfies the request, the five
/// diff-hygiene answers when a diff was asked about, and one noul per
/// acceptance judge item asked in the same request. `session/task.rs::gate`
/// decides what to do with the numbers -- this module only asks and parses.
#[derive(Debug, Clone, PartialEq)]
pub struct CompletionAnswer {
    pub noul: f64,
    pub latency_ms: u64,
    pub truncated: bool,
    pub hygiene: Option<HygieneAnswer>,
    /// One noul per judge item, in the same order they were given to
    /// [`completion_satisfied`].
    pub judge: Vec<f64>,
}

/// Cuts `diff` to at most [`DIFF_STATE_BYTES`], at the last hunk header
/// (`"\n@@ "`) at or before the bound, so a kept hunk is never split
/// mid-way. Returns the (possibly unchanged) text and whether it was cut.
fn bound_diff(diff: &str) -> (String, bool) {
    if diff.len() <= DIFF_STATE_BYTES {
        return (diff.to_string(), false);
    }
    let mut cut = DIFF_STATE_BYTES;
    while cut > 0 && !diff.is_char_boundary(cut) {
        cut -= 1;
    }
    let head = &diff[..cut];
    match head.rfind("\n@@ ") {
        Some(newline) => (diff[..=newline].to_string(), true),
        None => (head.to_string(), true),
    }
}

fn extract_noul(answers: &mut BTreeMap<String, Answer>, key: &str) -> Result<f64, DecideError> {
    match answers.remove(key) {
        Some(Answer::Noul(value)) => Ok(value),
        Some(Answer::Choice { .. }) => Err(DecideError::Parse(format!(
            "the `{key}` question was answered as a choice, not a noul"
        ))),
        None => Err(DecideError::Parse(format!("no answer for `{key}`"))),
    }
}

/// Asks whether `state` satisfies `request` -- the diff, or the model's own
/// answer when there is no diff to show or the task's intent was read-only
/// -- together with the diff-hygiene questions (2641, only for
/// [`CompletionState::Diff`]) and one question per `judge_items` (2642),
/// all in the one request 2616 already bounds to [`DECISION_TIMEOUT`].
/// Never surfaced as a task failure -- the caller (`session/task.rs::gate`)
/// records the error and proceeds exactly as it would with no decision
/// model.
pub fn completion_satisfied(
    model: &str,
    request: &str,
    state: CompletionState<'_>,
    judge_items: &[(String, String)],
) -> Result<CompletionAnswer, DecideError> {
    let (shared_state, truncated, ask_hygiene) = match &state {
        CompletionState::Diff { diff, findings } => {
            let (bounded, truncated) = bound_diff(diff);
            let mut value = serde_json::json!({
                "request": request,
                "diff": bounded,
                "findings": findings,
            });
            if truncated {
                value["diff_truncated"] = Value::Bool(true);
            }
            (value, truncated, true)
        }
        CompletionState::Answer { answer } => (
            serde_json::json!({ "request": request, "answer": answer }),
            false,
            false,
        ),
    };

    let mut questions: Vec<(String, Question)> =
        vec![(SATISFIED_KEY.to_string(), satisfied_question(&state))];
    if ask_hygiene {
        questions.extend(
            hygiene_questions()
                .into_iter()
                .map(|(key, question)| (key.to_string(), question)),
        );
    }
    for (index, (item, evidence)) in judge_items.iter().enumerate() {
        questions.push((judge_key(index), judge_question(item, evidence)));
    }

    let answers = decide(model, shared_state, &questions)?;
    let latency_ms = answers
        .decisions
        .first()
        .map_or(0, |decision| decision.latency_ms);
    let mut by_key: BTreeMap<String, Answer> = answers
        .decisions
        .into_iter()
        .map(|decision| (decision.key, decision.answer))
        .collect();

    let noul = extract_noul(&mut by_key, SATISFIED_KEY)?;
    let hygiene = if ask_hygiene {
        Some(HygieneAnswer {
            has_tests: extract_noul(&mut by_key, HYGIENE_HAS_TESTS_KEY)?,
            out_of_scope: extract_noul(&mut by_key, HYGIENE_OUT_OF_SCOPE_KEY)?,
            debug_leftovers: extract_noul(&mut by_key, HYGIENE_DEBUG_LEFTOVERS_KEY)?,
            deletes_tests: extract_noul(&mut by_key, HYGIENE_DELETES_TESTS_KEY)?,
            changes_signature: extract_noul(&mut by_key, HYGIENE_CHANGES_SIGNATURE_KEY)?,
        })
    } else {
        None
    };
    let mut judge = Vec::with_capacity(judge_items.len());
    for index in 0..judge_items.len() {
        judge.push(extract_noul(&mut by_key, &judge_key(index))?);
    }

    Ok(CompletionAnswer {
        noul,
        latency_ms,
        truncated,
        hygiene,
        judge,
    })
}

/// Every tool `registry::ALL` declares [`crate::tools::registry::Purity::Effectful`],
/// plus the non-tool doors whose own effect is not a registry fact:
/// `checks` runs a verification command, `agent` spawns a subagent, and
/// `mcp` calls an external tool. A literal, not derived: `tests::
/// effectful_names_matches_the_registry_plus_the_non_tool_doors` pins it
/// against the live registry, so a new `Purity::Effectful` tool fails that
/// test until this list says so on purpose.
pub const EFFECTFUL_NAMES: &[&str] = &["bash", "write", "edit", "checks", "agent", "mcp"];

/// The first of a cell's free names that names an effectful capability, with
/// the byte offset [`crate::runtime::cell::compile`] recorded for it.
pub fn names_effect(compiled_free_names: &[(String, u32)]) -> Option<(String, u32)> {
    compiled_free_names
        .iter()
        .find(|(name, _)| EFFECTFUL_NAMES.contains(&name.as_str()))
        .cloned()
}

/// The provider spelling of the first lowered direct call whose capability
/// id is effectful, or `None`.
pub fn direct_frame_names_effect(calls: &[crate::abi::LoweredCall]) -> Option<String> {
    calls
        .iter()
        .find(|call| EFFECTFUL_NAMES.contains(&call.capability))
        .map(|call| call.provider_name.clone())
}

/// What this turn's effectful cell or frame does, given the task's decision
/// state.
#[derive(Debug, Clone, PartialEq)]
pub enum Hold {
    /// Nothing about this turn is held.
    Run,
    /// `mode = on`, held for the first time this task: the caller does not
    /// run it and answers with this block instead.
    Held(String),
    /// `mode = on`, already held once this task: the caller runs it and
    /// counts an override.
    Overridden,
    /// `mode = shadow`: the caller runs it as it would have anyway and
    /// counts a would-be hold; this text never reaches the model.
    Shadow(String),
}

/// Decides what happens to one effectful cell or frame. `effect` is the
/// capability name found and, for a cell, the line it was found on -- `None`
/// when nothing effectful was found, in which case nothing is ever held.
pub fn hold_for(
    mode: DecisionMode,
    intent: Option<&Intent>,
    hold_above: f64,
    effect: Option<(&str, Option<u32>)>,
    already_held: bool,
) -> Hold {
    if mode == DecisionMode::Off {
        return Hold::Run;
    }
    let Some(intent) = intent else {
        return Hold::Run;
    };
    if intent.choice != READ_ONLY || intent.confidence < hold_above {
        return Hold::Run;
    }
    let Some((name, line)) = effect else {
        return Hold::Run;
    };
    match mode {
        DecisionMode::Off => Hold::Run,
        DecisionMode::Shadow => Hold::Shadow(held_block(intent.confidence, name, line)),
        DecisionMode::On if already_held => Hold::Overridden,
        DecisionMode::On => Hold::Held(held_block(intent.confidence, name, line)),
    }
}

/// The `/cell` inspector's one line for this task's decision state
/// (`session/ui.rs`'s `"/cell"` arm).
pub fn summary_line(intent: Option<&Intent>, holds: u32, overrides: u32) -> String {
    match intent {
        Some(intent) => format!(
            "decision: {} {:.2} · holds {holds} · overrides {overrides}",
            intent.choice, intent.confidence
        ),
        None => "decision: none".to_string(),
    }
}

fn effect_description(name: &str) -> &'static str {
    match name.to_ascii_lowercase().as_str() {
        "write" | "edit" => "changes files",
        "bash" => "runs a command",
        "agent" => "spawns a subagent",
        "mcp" => "calls an external tool",
        "checks" => "runs a verification command",
        _ => "has effects",
    }
}

fn held_block(confidence: f64, name: &str, line: Option<u32>) -> String {
    let what = match line {
        Some(line) => format!(
            "This cell calls `{name}` (line {line}), which {}.",
            effect_description(name)
        ),
        None => format!(
            "This request calls `{name}`, which {}.",
            effect_description(name)
        ),
    };
    format!(
        "## Held (decision)\n\
         The request reads as read-only (intent read_only, confidence {confidence:.2}). {what}\n\
         If the request needs it, run the cell again unchanged and it will run. Otherwise answer \
         without changing anything.\n"
    )
}

/// The drift question's key (2643), mirroring [`SATISFIED_KEY`]'s shape for
/// the other single-question request this package asks mid-task.
const DRIFT_KEY: &str = "drift";

/// The most of a cell's source the drift question's `state.cell` carries
/// (2643). Cut at the last newline at or before the bound (never
/// [`bound_diff`]'s hunk boundary -- a cell is plain source, not a diff), so
/// a kept prefix never splits a line mid-way.
pub const DRIFT_CELL_BYTES: usize = 8 * 1024;

fn bound_cell(cell: &str) -> String {
    if cell.len() <= DRIFT_CELL_BYTES {
        return cell.to_string();
    }
    let mut cut = DRIFT_CELL_BYTES;
    while cut > 0 && !cell.is_char_boundary(cut) {
        cut -= 1;
    }
    let head = &cell[..cut];
    match head.rfind('\n') {
        Some(newline) => cell[..=newline].to_string(),
        None => head.to_string(),
    }
}

fn drift_question() -> Question {
    Question::Noul {
        instructions: "The cell does what the plan's current step says and nothing else. \
                        Answer near 1.0 when it does; answer near 0.0 when it clearly does not."
            .to_string(),
    }
}

/// Asks whether `cell` does what `step` (the plan's current `Active` item)
/// says, given `request` -- one question, synchronous, bounded to
/// [`DECISION_TIMEOUT`] exactly as every other call into [`decide`]. Never
/// surfaced as a task failure -- the caller (`session/system.rs::
/// apply_decision_hold`) counts the error and runs the cell exactly as it
/// would with no decision model.
pub fn drift_satisfied(
    model: &str,
    request: &str,
    step: &str,
    cell: &str,
) -> Result<f64, DecideError> {
    let state = serde_json::json!({
        "request": request,
        "step": step,
        "cell": bound_cell(cell),
    });
    let questions = [(DRIFT_KEY.to_string(), drift_question())];
    let answers = decide(model, state, &questions)?;
    let decision = answers
        .decisions
        .into_iter()
        .next()
        .ok_or_else(|| DecideError::Parse(format!("no answer for `{DRIFT_KEY}`")))?;
    match decision.answer {
        Answer::Noul(value) => Ok(value),
        Answer::Choice { .. } => Err(DecideError::Parse(format!(
            "the `{DRIFT_KEY}` question was answered as a choice, not a noul"
        ))),
    }
}

/// What one effectful cell or frame does about the plan's current step,
/// given the task's drift state (2643).
#[derive(Debug, Clone, PartialEq)]
pub enum Drift {
    /// Nothing about this cell is held.
    Run,
    /// `mode = shadow`: the caller runs the cell as it would have anyway and
    /// counts a would-be hold; the noul that would have held it.
    Shadow(f64),
    /// `mode = on`, held for the first time this task: the caller does not
    /// run the cell and answers with this block instead.
    Held(String),
}

/// Decides what happens to one effectful cell or frame, mirroring
/// [`hold_for`]'s once rule: `already_held` (`Held` already returned once
/// this task) always runs the cell, exactly as `Hold::Overridden` does for
/// the intent hold, even if `answer` is a fresh confident no -- the once
/// rule wins over the answer, not the other way round. `answer` is `None`
/// when the request failed, which also leaves the cell running. `step` is
/// the active plan item's own text, carried through only to build the held
/// block.
pub fn drift_for(
    mode: DecisionMode,
    answer: Option<f64>,
    drift_no_below: f64,
    step: &str,
    already_held: bool,
) -> Drift {
    if mode == DecisionMode::Off || already_held {
        return Drift::Run;
    }
    let Some(noul) = answer else {
        return Drift::Run;
    };
    match mode {
        DecisionMode::Off => Drift::Run,
        DecisionMode::Shadow => Drift::Shadow(noul),
        DecisionMode::On if noul <= drift_no_below => Drift::Held(drift_block(noul, step)),
        DecisionMode::On => Drift::Run,
    }
}

fn drift_block(confidence: f64, step: &str) -> String {
    format!(
        "decision: this cell may not do what the plan's current step says ({confidence:.2}) — \
         step: {step}. Held once; run it again if it does, or update the plan first.\n"
    )
}

// --- the supervisor's question (`docs/product/pane/supervisor.md` §3) ------

const SUPERVISION_KEY: &str = "supervision";

/// The supervision criterion that means nothing is wrong. Every other
/// criterion is a reason to nudge, which is why this one is named here and
/// the rest are not: [`supervision_for`] tests against this single word.
pub const MAKING_PROGRESS: &str = "making_progress";

/// The most of a compressed trajectory the supervision question carries. Cut
/// at a line boundary like [`bound_cell`], because `supervisor::compress`
/// renders exactly one line per cell and half a line names half a call.
pub const SUPERVISION_TRAJECTORY_BYTES: usize = 8 * 1024;

/// The tail, not the head: the supervisor asks what the agent is doing *now*,
/// and the newest cells are the ones that answer it.
fn bound_trajectory(trajectory: &str) -> String {
    if trajectory.len() <= SUPERVISION_TRAJECTORY_BYTES {
        return trajectory.to_string();
    }
    let mut cut = trajectory.len() - SUPERVISION_TRAJECTORY_BYTES;
    while cut < trajectory.len() && !trajectory.is_char_boundary(cut) {
        cut += 1;
    }
    let tail = &trajectory[cut..];
    match tail.find('\n') {
        Some(newline) => tail[newline + 1..].to_string(),
        None => tail.to_string(),
    }
}

fn supervision_question() -> Question {
    let mut criteria = BTreeMap::new();
    criteria.insert(
        MAKING_PROGRESS.to_string(),
        "each cell moves the work on: it reads something it has not read, changes something, \
         or checks something it just changed"
            .to_string(),
    );
    criteria.insert(
        "repeating_a_failing_call".to_string(),
        "the same call fails the same way more than once and nothing about the approach changes"
            .to_string(),
    );
    criteria.insert(
        "looping_over_the_same_reads".to_string(),
        "the same files or searches are read again and again without a change or a check \
         following from them"
            .to_string(),
    );
    criteria.insert(
        "stopped_without_returning".to_string(),
        "the cells do nothing that advances the task and nothing that ends it — waiting, \
         polling, or restating what is already known"
            .to_string(),
    );
    Question::Choice {
        instructions: "What is this coding agent's recent trajectory doing?".to_string(),
        criteria,
    }
}

/// What the decision model answered about a trajectory.
#[derive(Debug, Clone, PartialEq)]
pub struct Supervision {
    pub choice: String,
    pub confidence: f64,
    pub latency_ms: u64,
}

/// Asks the supervision question about `trajectory` — one `Choice` question,
/// synchronous, bounded to [`DECISION_TIMEOUT`] like every other call into
/// [`decide`]. `cells_without_change` is the deterministic stall counter
/// (`progress::Stall::since_progress`) handed over as evidence rather than as
/// a gate: a model repeating a *failing* call while still writing files looks
/// like progress to that counter, so gating the question on it would hide
/// exactly the loop worth catching.
///
/// Never surfaced as a task failure — the caller records the error as a
/// failed look and does not nudge.
pub fn supervision(
    model: &str,
    trajectory: &str,
    cells_without_change: u32,
) -> Result<Supervision, DecideError> {
    let state = serde_json::json!({
        "trajectory": bound_trajectory(trajectory),
        "cells_without_change": cells_without_change,
    });
    let questions = [(SUPERVISION_KEY.to_string(), supervision_question())];
    let answers = decide(model, state, &questions)?;
    let decision = answers
        .decisions
        .into_iter()
        .next()
        .ok_or_else(|| DecideError::Parse(format!("no answer for `{SUPERVISION_KEY}`")))?;
    match decision.answer {
        Answer::Choice {
            choice, confidence, ..
        } => Ok(Supervision {
            choice,
            confidence,
            latency_ms: decision.latency_ms,
        }),
        Answer::Noul(_) => Err(DecideError::Parse(format!(
            "the `{SUPERVISION_KEY}` question was answered as a noul, not a choice"
        ))),
    }
}

/// Whether one [`Supervision`] answer is a reason to nudge, and which
/// criterion it is: `Some(choice)` at or above `supervision_above` for any
/// criterion but [`MAKING_PROGRESS`], `None` otherwise.
///
/// **`mode` gates whether the question may be asked, not what its answer may
/// do.** `shadow` exists so a decision cannot change what *runs* — a hold, a
/// narrowing, a refused cell. A supervisor nudge runs nothing and blocks
/// nothing: it is one sentence at the head of the next turn, which the
/// supervisor could already add before this question existed. So `shadow`
/// and `on` behave alike here and only `off` silences it.
#[must_use]
pub fn supervision_for(
    mode: DecisionMode,
    answer: Option<&Supervision>,
    supervision_above: f64,
) -> Option<String> {
    if mode == DecisionMode::Off {
        return None;
    }
    let answer = answer?;
    if answer.choice == MAKING_PROGRESS || answer.confidence < supervision_above {
        return None;
    }
    Some(answer.choice.clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::registry;

    #[test]
    fn effectful_names_matches_the_registry_plus_the_non_tool_doors() {
        let mut expected: Vec<&str> = registry::ALL
            .iter()
            .filter(|tool| tool.purity() == registry::Purity::Effectful)
            .map(|tool| tool.name())
            .collect();
        expected.extend(["checks", "agent", "mcp"]);
        expected.sort_unstable();
        let mut actual: Vec<&str> = EFFECTFUL_NAMES.to_vec();
        actual.sort_unstable();
        assert_eq!(actual, expected);
    }

    #[test]
    fn names_effect_finds_write_and_nothing_for_read() {
        let free = vec![("write".to_string(), 12u32), ("cwd".to_string(), 0)];
        assert_eq!(names_effect(&free), Some(("write".to_string(), 12)));
        let free = vec![("read".to_string(), 3u32)];
        assert_eq!(names_effect(&free), None);
    }

    #[test]
    fn request_body_matches_the_documented_shape() {
        let mut criteria = BTreeMap::new();
        criteria.insert("read_only".to_string(), "reads only".to_string());
        let questions: BTreeMap<String, Question> = [(
            "intent".to_string(),
            Question::Choice {
                instructions: "What does this request intend?".to_string(),
                criteria,
            },
        )]
        .into_iter()
        .collect();
        let body = RequestBody {
            state: &serde_json::json!({"request": "read the file"}),
            model: "jev-latest",
            questions,
        };
        let value = serde_json::to_value(&body).unwrap();
        assert_eq!(value["model"], "jev-latest");
        assert_eq!(value["state"]["request"], "read the file");
        assert_eq!(value["questions"]["intent"]["type"], "choice");
        assert_eq!(
            value["questions"]["intent"]["instructions"],
            "What does this request intend?"
        );
        assert_eq!(
            value["questions"]["intent"]["criteria"]["read_only"],
            "reads only"
        );
    }

    #[test]
    fn the_task_questions_request_carries_intent_and_complexity_in_one_map() {
        let questions: BTreeMap<String, Question> = [
            (INTENT_KEY.to_string(), intent_question()),
            (COMPLEXITY_KEY.to_string(), complexity_question()),
        ]
        .into_iter()
        .collect();
        let body = RequestBody {
            state: &serde_json::json!({"request": "read the file"}),
            model: "jev-latest",
            questions,
        };
        let value = serde_json::to_value(&body).unwrap();
        assert_eq!(
            value["questions"].as_object().unwrap().len(),
            2,
            "one request, both questions: {value}"
        );
        assert_eq!(value["questions"]["intent"]["type"], "choice");
        assert_eq!(value["questions"]["complexity"]["type"], "choice");
        assert!(
            value["questions"]["complexity"]["criteria"][NEEDS_EXPLORATION]
                .as_str()
                .is_some()
        );
    }

    #[test]
    fn task_decision_of_pairs_both_answers_by_key() {
        let answers = Answers {
            model: "jev-latest".to_string(),
            decisions: vec![
                Decision {
                    key: INTENT_KEY.to_string(),
                    answer: Answer::Choice {
                        choice: "read_only".to_string(),
                        probabilities: BTreeMap::new(),
                        confidence: 0.94,
                    },
                    latency_ms: 640,
                },
                Decision {
                    key: COMPLEXITY_KEY.to_string(),
                    answer: Answer::Choice {
                        choice: "routine".to_string(),
                        probabilities: BTreeMap::new(),
                        confidence: 0.81,
                    },
                    latency_ms: 640,
                },
            ],
        };
        let decision = task_decision_of(answers).unwrap();
        assert_eq!(decision.intent.choice, "read_only");
        assert_eq!(decision.intent.confidence, 0.94);
        assert_eq!(decision.complexity.choice, "routine");
        assert_eq!(decision.complexity.confidence, 0.81);
    }

    #[test]
    fn task_decision_of_missing_complexity_is_a_parse_error() {
        let answers = Answers {
            model: "jev-latest".to_string(),
            decisions: vec![Decision {
                key: INTENT_KEY.to_string(),
                answer: Answer::Choice {
                    choice: "read_only".to_string(),
                    probabilities: BTreeMap::new(),
                    confidence: 0.94,
                },
                latency_ms: 640,
            }],
        };
        let error = task_decision_of(answers).unwrap_err();
        assert!(error.to_string().contains("complexity"), "{error}");
    }

    #[test]
    fn the_documented_choice_response_parses() {
        let text = serde_json::json!({
            "model": "jev-latest",
            "answers": {
                "intent": {
                    "type": "choice",
                    "choice": "read_only",
                    "probabilities": {"read_only": 0.94, "modify": 0.04, "run": 0.01, "other": 0.01},
                    "confidence": 0.94,
                }
            },
            "usage": {"input_tokens": 40, "output_tokens": 12},
        })
        .to_string();
        let parsed: ResponseBody = serde_json::from_str(&text).unwrap();
        let answer = Answer::from(parsed.answers.get("intent").cloned().unwrap());
        match answer {
            Answer::Choice {
                choice, confidence, ..
            } => {
                assert_eq!(choice, "read_only");
                assert_eq!(confidence, 0.94);
            }
            Answer::Noul(_) => panic!("expected a choice answer"),
        }
    }

    #[test]
    fn a_response_missing_confidence_is_a_parse_error() {
        let text = serde_json::json!({
            "model": "jev-latest",
            "answers": {
                "intent": {
                    "type": "choice",
                    "choice": "read_only",
                    "probabilities": {"read_only": 0.94},
                }
            },
            "usage": {"input_tokens": 40, "output_tokens": 12},
        })
        .to_string();
        assert!(serde_json::from_str::<ResponseBody>(&text).is_err());
    }

    #[test]
    fn hold_for_applies_the_threshold_and_the_once_rule() {
        let intent = Intent {
            choice: "read_only".to_string(),
            confidence: 0.94,
            latency_ms: 180,
        };
        let effect = Some(("write", Some(3)));
        assert!(matches!(
            hold_for(DecisionMode::On, Some(&intent), 0.85, effect, false),
            Hold::Held(_)
        ));
        assert!(matches!(
            hold_for(DecisionMode::On, Some(&intent), 0.85, effect, true),
            Hold::Overridden
        ));
        assert!(matches!(
            hold_for(DecisionMode::Shadow, Some(&intent), 0.85, effect, false),
            Hold::Shadow(_)
        ));
        let low_confidence = Intent {
            confidence: 0.80,
            ..intent.clone()
        };
        assert_eq!(
            hold_for(DecisionMode::On, Some(&low_confidence), 0.85, effect, false),
            Hold::Run
        );
        let modify = Intent {
            choice: "modify".to_string(),
            ..intent.clone()
        };
        assert_eq!(
            hold_for(DecisionMode::On, Some(&modify), 0.85, effect, false),
            Hold::Run
        );
        assert_eq!(
            hold_for(DecisionMode::On, Some(&intent), 0.85, None, false),
            Hold::Run
        );
    }

    #[test]
    fn drift_for_applies_the_threshold_and_the_once_rule() {
        assert!(matches!(
            drift_for(
                DecisionMode::On,
                Some(0.06),
                0.10,
                "write the README",
                false
            ),
            Drift::Held(_)
        ));
        assert_eq!(
            drift_for(DecisionMode::On, Some(0.06), 0.10, "write the README", true),
            Drift::Run,
            "the once rule wins even if a caller mistakenly asks again"
        );
        assert_eq!(
            drift_for(
                DecisionMode::On,
                Some(0.50),
                0.10,
                "write the README",
                false
            ),
            Drift::Run,
            "an in-between answer is not confident enough to hold"
        );
        assert_eq!(
            drift_for(
                DecisionMode::Shadow,
                Some(0.06),
                0.10,
                "write the README",
                false
            ),
            Drift::Shadow(0.06)
        );
        assert_eq!(
            drift_for(DecisionMode::On, None, 0.10, "write the README", false),
            Drift::Run,
            "no answer (skipped or failed) leaves the cell running"
        );
        assert_eq!(
            drift_for(
                DecisionMode::Off,
                Some(0.06),
                0.10,
                "write the README",
                false
            ),
            Drift::Run
        );
    }

    #[test]
    fn drift_block_names_the_step_and_the_confidence() {
        let block = drift_block(0.06, "write the README");
        assert!(block.contains("write the README"), "{block}");
        assert!(block.contains("0.06"), "{block}");
        assert!(
            block.contains("this cell may not do what the plan's current step says"),
            "{block}"
        );
    }

    #[test]
    fn the_satisfied_body_serializes_to_the_documented_shape() {
        let (bounded, truncated) = bound_diff("+one line\n");
        assert!(!truncated);
        let state = CompletionState::Diff {
            diff: "+one line\n",
            findings: &[],
        };
        let questions: BTreeMap<String, Question> =
            [(SATISFIED_KEY.to_string(), satisfied_question(&state))]
                .into_iter()
                .collect();
        let body = RequestBody {
            state: &serde_json::json!({"request": "fix the bug", "diff": bounded, "findings": Vec::<String>::new()}),
            model: "jev-latest",
            questions,
        };
        let value = serde_json::to_value(&body).unwrap();
        assert_eq!(value["questions"]["satisfied"]["type"], "noul");
        assert!(
            value["questions"]["satisfied"]["instructions"]
                .as_str()
                .unwrap()
                .contains("satisfy")
        );
        assert_eq!(value["state"]["request"], "fix the bug");
        assert_eq!(value["state"]["diff"], "+one line\n");
    }

    #[test]
    fn the_answer_state_question_asks_about_the_answer_not_a_diff() {
        let state = CompletionState::Answer { answer: "done" };
        let question = satisfied_question(&state);
        let Question::Noul { instructions } = question else {
            panic!("expected a noul question");
        };
        assert!(instructions.contains("answer"), "{instructions}");
        assert!(!instructions.contains("diff"), "{instructions}");
    }

    #[test]
    fn hygiene_questions_cover_the_five_keys() {
        let questions = hygiene_questions();
        let keys: Vec<&str> = questions.iter().map(|(key, _)| *key).collect();
        assert_eq!(
            keys,
            vec![
                "has_tests",
                "out_of_scope",
                "debug_leftovers",
                "deletes_tests",
                "changes_signature",
            ]
        );
    }

    #[test]
    fn a_judge_question_embeds_the_item_and_its_evidence() {
        let Question::Noul { instructions } =
            judge_question("the tone is friendly", "no evidence gathered")
        else {
            panic!("expected a noul question");
        };
        assert!(
            instructions.contains("the tone is friendly"),
            "{instructions}"
        );
        assert!(
            instructions.contains("no evidence gathered"),
            "{instructions}"
        );
        assert_eq!(judge_key(0), "judge_0");
        assert_eq!(judge_key(3), "judge_3");
    }

    fn supervision_answer(choice: &str, confidence: f64) -> Supervision {
        Supervision {
            choice: choice.to_string(),
            confidence,
            latency_ms: 700,
        }
    }

    #[test]
    fn a_progress_answer_is_never_a_reason_to_nudge() {
        let answer = supervision_answer(MAKING_PROGRESS, 0.99);
        assert_eq!(
            supervision_for(DecisionMode::On, Some(&answer), 0.85),
            None,
            "the one criterion that means nothing is wrong"
        );
    }

    #[test]
    fn a_loop_below_the_threshold_is_not_decisive_and_above_it_is() {
        let below = supervision_answer("looping_over_the_same_reads", 0.84);
        assert_eq!(supervision_for(DecisionMode::On, Some(&below), 0.85), None);
        let above = supervision_answer("looping_over_the_same_reads", 0.85);
        assert_eq!(
            supervision_for(DecisionMode::On, Some(&above), 0.85),
            Some("looping_over_the_same_reads".to_string()),
            "at the threshold is decisive, as every other `_above` here is"
        );
    }

    #[test]
    fn shadow_still_nudges_and_off_never_asks() {
        let answer = supervision_answer("repeating_a_failing_call", 0.95);
        assert_eq!(
            supervision_for(DecisionMode::Shadow, Some(&answer), 0.85),
            Some("repeating_a_failing_call".to_string()),
            "a nudge runs nothing, so `shadow` does not silence it"
        );
        assert_eq!(
            supervision_for(DecisionMode::Off, Some(&answer), 0.85),
            None
        );
        assert_eq!(supervision_for(DecisionMode::On, None, 0.85), None);
    }

    #[test]
    fn a_bounded_trajectory_keeps_the_newest_cells_whole() {
        let line = format!("cell 1 yielded · {} · calls: (none)\n", "x".repeat(400));
        let mut trajectory = String::new();
        let mut cells = 0;
        while trajectory.len() <= SUPERVISION_TRAJECTORY_BYTES {
            cells += 1;
            trajectory.push_str(&line.replace("cell 1", &format!("cell {cells}")));
        }
        let last = format!("cell {cells} yielded");
        let bounded = bound_trajectory(&trajectory);
        assert!(bounded.len() <= SUPERVISION_TRAJECTORY_BYTES);
        assert!(
            bounded.contains(&last),
            "the newest cell is what the question is about"
        );
        assert!(
            bounded.starts_with("cell "),
            "a kept line is never cut mid-way: {}",
            &bounded[..40]
        );
    }

    #[test]
    fn bound_diff_cuts_at_a_hunk_boundary_and_sets_the_flag() {
        let hunk = format!("@@ -1,1 +1,1 @@\n-{}\n+after\n", "x".repeat(2_000));
        let mut diff = String::new();
        while diff.len() <= DIFF_STATE_BYTES {
            diff.push_str(&hunk);
        }
        let (bounded, truncated) = bound_diff(&diff);
        assert!(truncated);
        assert!(bounded.len() <= DIFF_STATE_BYTES);
        assert!(diff.starts_with(&bounded), "a prefix of the original diff");
        assert_eq!(
            bounded.len() % hunk.len(),
            0,
            "kept only whole hunks, none split mid-way: {} of {}",
            bounded.len(),
            hunk.len()
        );

        let (short, truncated) = bound_diff("@@ -1,1 +1,1 @@\n-a\n+b\n");
        assert!(!truncated);
        assert_eq!(short, "@@ -1,1 +1,1 @@\n-a\n+b\n");
    }

    #[test]
    fn a_completion_answer_missing_a_noul_is_a_parse_error() {
        let text = serde_json::json!({
            "model": "jev-latest",
            "answers": {
                "satisfied": {
                    "type": "choice",
                    "choice": "yes",
                    "probabilities": {"yes": 0.9},
                    "confidence": 0.9,
                }
            },
            "usage": {"input_tokens": 10, "output_tokens": 4},
        })
        .to_string();
        let parsed: ResponseBody = serde_json::from_str(&text).unwrap();
        let answer = Answer::from(parsed.answers.get("satisfied").cloned().unwrap());
        assert!(matches!(answer, Answer::Choice { .. }));
    }
}
