//! The look -- `docs/product/pane/supervisor.md` §2 and §3: a compressed
//! trajectory, one metered request through the wire, and one decision.
//!
//! **Never a preview's bytes and never a payload.** [`compress`] renders only
//! a program's first line, its outcome and its call trajectory -- the same
//! three things `runtime-contract.md` §9.4 already puts in the rollout -- so
//! the supervisor sees exactly what the rollout records and nothing a program
//! read or wrote.

use serde::Deserialize;

use crate::contract::{Conversation, Message, Role};
use crate::runtime::outcome::{CallRecord, CellOutcomeKind, CellRecord, Ended};
use crate::wire;

/// §3's fixed system preamble, verbatim.
const PREAMBLE: &str = "You watch a coding agent's trajectory. Answer with one JSON object \
    {\"intervene\": bool, \"reason\": \"<one line>\"}: intervene only when the agent is looping, \
    repeating a failing call, or has stopped making progress toward the task.";

/// The look's `max_tokens` -- small, because the only valid answer is one
/// short JSON object.
const LOOK_MAX_TOKENS: u32 = 200;

/// The header that lets the ledger tell a look apart from a task turn before
/// the gateway itself reads it (the FEASIBILITY's Class A note).
const PURPOSE_HEADER: (&str, &str) = ("x-glasshouse-purpose", "supervisor");

/// One decision from one look: intervene, or not, and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Decision {
    pub intervene: bool,
    pub reason: String,
}

impl Decision {
    fn not_intervene(reason: impl Into<String>) -> Self {
        Self {
            intervene: false,
            reason: reason.into(),
        }
    }
}

/// One `[supervisor]` session's look. Holds nothing today -- a type rather
/// than a free function so a later model choice has somewhere to live without
/// changing every call site.
pub struct Supervisor;

impl Supervisor {
    pub fn new() -> Self {
        Self
    }

    /// One look through the wire, against `model` -- `[supervisor] model`,
    /// the **cheaper** model map line 2469 names, never the task's own. The
    /// fixed preamble is the system block, `trajectory` the one user message,
    /// `max_tokens` small, the purpose header set. Anything unparseable, a
    /// transport error, or a refused status answers [`Decision::not_intervene`]
    /// -- never a nudge.
    pub fn look(&self, model: &str, trajectory: &str) -> Decision {
        let conversation = Conversation {
            system: PREAMBLE.to_string(),
            messages: vec![Message::text(Role::User, trajectory)],
        };
        match wire::send_turn_with(&conversation, model, LOOK_MAX_TOKENS, Some(PURPOSE_HEADER)) {
            Ok(message) => parse_decision(&message),
            Err(err) => Decision::not_intervene(format!("request failed: {err}")),
        }
    }
}

impl Default for Supervisor {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Deserialize)]
struct RawDecision {
    intervene: bool,
    reason: String,
}

fn parse_decision(message: &Message) -> Decision {
    let text: String = message
        .content
        .iter()
        .map(crate::contract::Block::text)
        .collect::<Vec<_>>()
        .join("");
    match serde_json::from_str::<RawDecision>(text.trim()) {
        Ok(raw) => Decision {
            intervene: raw.intervene,
            reason: raw.reason,
        },
        Err(_) => Decision::not_intervene("unparseable"),
    }
}

/// §2: one line per cell since the last look, in cell order.
pub fn compress(cells: &[CellRecord]) -> String {
    cells
        .iter()
        .map(compress_one)
        .collect::<Vec<_>>()
        .join("\n")
}

fn compress_one(cell: &CellRecord) -> String {
    let head = cell.source.lines().next().unwrap_or("").trim();
    let outcome = match cell.outcome {
        CellOutcomeKind::Yielded => "yielded",
        CellOutcomeKind::Returned => "returned",
        CellOutcomeKind::Threw => "threw",
    };
    let calls = if cell.calls.is_empty() {
        "(none)".to_string()
    } else {
        cell.calls
            .iter()
            .map(render_call)
            .collect::<Vec<_>>()
            .join(", ")
    };
    format!("cell {} {outcome} · {head} · calls: {calls}", cell.cell)
}

fn render_call(call: &CallRecord) -> String {
    let args: String = call
        .args
        .iter()
        .map(|(key, value)| format!("{key}={value}"))
        .collect::<Vec<_>>()
        .join(", ");
    let ended = match &call.ended {
        Ended::Ok => "ended".to_string(),
        Ended::Threw { class } => format!("threw {class}"),
        Ended::Denied { rule } => format!("denied {rule}"),
    };
    format!("{}({args})→{ended}", call.tool)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    #[test]
    fn compress_renders_the_head_the_outcome_and_the_calls_never_a_payload() {
        let cell = CellRecord {
            cell: 1,
            source: "const hits = await grep({ pattern: \"x\" });\nconst n = hits.length;".into(),
            outcome: CellOutcomeKind::Yielded,
            handles: Vec::new(),
            calls: vec![CallRecord {
                tool: "grep".into(),
                args: BTreeMap::from([("pattern".to_string(), "x".to_string())]),
                ended: Ended::Ok,
            }],
        };
        let line = compress(&[cell]);
        assert_eq!(
            line,
            "cell 1 yielded · const hits = await grep({ pattern: \"x\" }); · calls: \
             grep(pattern=x)→ended"
        );
    }

    #[test]
    fn an_unparseable_answer_is_not_intervene() {
        let message = Message::text(Role::Assistant, "not json");
        assert_eq!(
            parse_decision(&message),
            Decision::not_intervene("unparseable")
        );
    }

    #[test]
    fn a_well_formed_answer_carries_its_own_reason() {
        let message = Message::text(
            Role::Assistant,
            r#"{"intervene": true, "reason": "looping"}"#,
        );
        assert_eq!(
            parse_decision(&message),
            Decision {
                intervene: true,
                reason: "looping".to_string()
            }
        );
    }
}
