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
const READ_ONLY: &str = "read_only";

const INTENT_KEY: &str = "intent";

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

/// What the decision model answered about one request's intent.
#[derive(Debug, Clone, PartialEq)]
pub struct Intent {
    pub choice: String,
    pub confidence: f64,
    pub latency_ms: u64,
}

/// Asks the intent question about `request` and answers with what came
/// back, or the reason it did not. Never surfaced as a task failure -- the
/// caller records the error in a notice and proceeds exactly as if no
/// decision model were configured.
pub fn intent_of(model: &str, request: &str) -> Result<Intent, DecideError> {
    let state = serde_json::json!({ "request": request });
    let questions = [(INTENT_KEY.to_string(), intent_question())];
    let answers = decide(model, state, &questions)?;
    let decision = answers
        .decisions
        .into_iter()
        .find(|decision| decision.key == INTENT_KEY)
        .ok_or_else(|| DecideError::Parse(format!("no answer for `{INTENT_KEY}`")))?;
    match decision.answer {
        Answer::Choice {
            choice, confidence, ..
        } => Ok(Intent {
            choice,
            confidence,
            latency_ms: decision.latency_ms,
        }),
        Answer::Noul(_) => Err(DecideError::Parse(
            "the intent question was answered as a noul, not a choice".to_string(),
        )),
    }
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
}
