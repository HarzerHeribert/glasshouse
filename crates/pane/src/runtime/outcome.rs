//! What one cell produced — `runtime-contract.md` §1's two endings, §5's
//! third, §4's rollout line, and §9's terminal response and trajectory.
//!
//! **A throw is a result, not an error.** [`CellOutcome::Threw`] carries the
//! same turn a yield would have carried — the elapsed time, the rendered
//! handle table, the captured stdout and the rollout record — and the
//! bindings the cell completed before throwing are in that table. Nothing
//! here is a `Result`, because none of the three endings is a failure of the
//! runtime.

use std::collections::BTreeMap;

use serde::ser::{SerializeMap, Serializer};
use serde::{Deserialize, Serialize};

use crate::runtime::handles::Provenance;
use crate::runtime::preview::{self, ErrorValue, PREVIEW_TOKEN_CAP, Value};

/// Everything a cell hands back whatever way it ended.
#[derive(Debug, Clone, PartialEq)]
pub struct CellTurn {
    pub elapsed_ms: u64,
    /// The turn's whole rendering of the handle table, from
    /// `handles::render_table`. The isolate renders no handle itself.
    pub table: String,
    /// The last [`crate::runtime::preview::STDOUT_TOKEN_CAP`] tokens of the
    /// cell's `console` output.
    pub stdout_tail: String,
    /// How many tokens of `console` output were dropped ahead of
    /// [`stdout_tail`](Self::stdout_tail).
    pub stdout_dropped_tokens: usize,
    /// Why the cell yielded on purpose — `yieldNow(reason)`'s reason, or the
    /// response cap's sentence (`runtime-contract.md` §9.3, §9.2). `None` for
    /// a fall-off and for every ending that is not a yield. It rides the turn
    /// rather than [`CellOutcome::Yielded`] because callers outside this
    /// package match `Yielded { turn }` exhaustively.
    pub yield_reason: Option<String>,
    /// What the cell said with `answer(text)`, and the **only** thing that
    /// ends the person's task. `None` for every cell that was still working
    /// -- which is every cell that did not say otherwise.
    pub answer: Option<String>,
    /// The one rollout line this cell owes — appended by the wiring package,
    /// never by this one.
    pub record: CellRecord,
    /// The model's own plan as it stood when the cell ended, newest write
    /// wins. Empty until a cell calls `todo.write`. It rides the turn rather
    /// than the record because it is task state the screen re-renders every
    /// cell, not a line the rollout owes.
    pub plan: Vec<PlanItem>,
    /// The canonical typed result of each capability call, in call order,
    /// captured only for a frame lowered from direct provider tool calls.
    ///
    /// It rides the turn because the turn is the frame's own value and dies
    /// with it: `CellTurn` is not `Serialize`, so this cannot reach the
    /// rollout, whose rule is programs and previews and never objects
    /// (`runtime-contract.md` §4). An authored cell captures nothing and this
    /// stays empty — the model already holds those results as live handles.
    pub capability_results: Vec<String>,
    /// What rendering `table` as a delta cost and saved this turn
    /// (`smarter-cheaper-roadmap.md`, *Observation delta*).
    pub observation: crate::runtime::observation::ObservationStats,
}

/// One item of the model's own plan — `todo.write`'s unit.
///
/// The invariant: **a status is one of three states and there is no fourth.**
/// A plan the model can put arbitrary text in the status of is a plan nothing
/// downstream can render or count, so an unknown status is refused at the
/// binding rather than stored and puzzled over later.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanItem {
    pub text: String,
    pub status: PlanStatus,
}

/// A plan item's state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlanStatus {
    Pending,
    Active,
    Done,
}

impl PlanStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            PlanStatus::Pending => "pending",
            PlanStatus::Active => "active",
            PlanStatus::Done => "done",
        }
    }

    /// The status named by `text`, or `None` for anything else. The three
    /// spellings are the whole vocabulary and the model is told them.
    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "pending" => Some(PlanStatus::Pending),
            "active" => Some(PlanStatus::Active),
            "done" => Some(PlanStatus::Done),
            _ => None,
        }
    }

    /// The mark this status carries in a rendered plan.
    pub fn mark(self) -> &'static str {
        match self {
            PlanStatus::Pending => "[ ]",
            PlanStatus::Active => "[~]",
            PlanStatus::Done => "[x]",
        }
    }
}

/// How a cell ended.
#[derive(Debug, Clone, PartialEq)]
pub enum CellOutcome {
    /// It ran off the end, or asked to hand back. The model gets the table
    /// and another turn, and the isolate stays warm (§1, §9.3).
    Yielded { turn: CellTurn },
    /// It executed a top-level `return`. Text and scalars end the task; a
    /// structured value is bounded notebook output (§1, §9.2).
    Returned {
        value: Value,
        terminal: Terminal,
        turn: CellTurn,
    },
    /// It threw. The turn slot a yield would have used carries the error
    /// instead, and the bindings made before the throw are in `turn.table`
    /// (§5).
    Threw { error: ErrorValue, turn: CellTurn },
}

/// A top-level return rendered at the isolate boundary. Text is a terminal
/// response; bounded JSON is notebook output for the next turn.
#[derive(Debug, Clone, PartialEq)]
pub enum Terminal {
    /// A returned string: the response, verbatim.
    Text(String),
    /// Any other returned value: its JSON with values, `cut` when the text
    /// is a prefix stopped at [`TERMINAL_JSON_CAP`] bytes on a character
    /// boundary.
    Json { text: String, cut: bool },
}

/// How many bytes of notebook JSON the output carries before it is cut.
pub const TERMINAL_JSON_CAP: usize = 2 * 1024;

impl Terminal {
    /// The response as the person reads it and the rollout keeps it. `whole`
    /// is the marshalled sample of the same value; when the JSON was cut, its
    /// type-only preview says what the cut removed.
    pub fn render(&self, whole: &Value) -> String {
        match self {
            Terminal::Text(text) | Terminal::Json { text, cut: false } => text.clone(),
            Terminal::Json { text, cut: true } => format!(
                "{text}\n…(cut at {} bytes; the whole value, by type:)\n{}",
                preview::thousands(TERMINAL_JSON_CAP as u64),
                preview::render_preview(whole, PREVIEW_TOKEN_CAP)
            ),
        }
    }
}

impl CellOutcome {
    pub fn turn(&self) -> &CellTurn {
        match self {
            CellOutcome::Yielded { turn }
            | CellOutcome::Returned { turn, .. }
            | CellOutcome::Threw { turn, .. } => turn,
        }
    }

    pub fn kind(&self) -> CellOutcomeKind {
        match self {
            CellOutcome::Yielded { .. } => CellOutcomeKind::Yielded,
            CellOutcome::Returned { .. } => CellOutcomeKind::Returned,
            CellOutcome::Threw { .. } => CellOutcomeKind::Threw,
        }
    }

    /// Whether the task is over, and it is over **only because the cell
    /// said so** with `answer(text)`.
    ///
    /// It used to be read off the returned value's type -- an array or an
    /// object was notebook output, anything else was the final answer. That
    /// made `return result.stdout`, written to look at a command's output,
    /// publish that output as the answer and end the session; a returned
    /// value's shape says nothing about whether the work is finished. A
    /// throw never ends a task however it answered: something failed after
    /// the claim, and the claim has not survived it.
    #[must_use]
    pub fn ends_the_task(&self) -> bool {
        match self {
            CellOutcome::Returned { turn, .. } | CellOutcome::Yielded { turn } => {
                turn.answer.is_some()
            }
            CellOutcome::Threw { .. } => false,
        }
    }

    /// What the cell answered, when it did.
    #[must_use]
    pub fn answer(&self) -> Option<&str> {
        match self {
            CellOutcome::Returned { turn, .. } | CellOutcome::Yielded { turn } => {
                turn.answer.as_deref()
            }
            CellOutcome::Threw { .. } => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum CellOutcomeKind {
    Yielded,
    Returned,
    Threw,
}

/// One rollout line — `runtime-contract.md` §4, whose shape this struct is.
///
/// It records the model's **program** and the handles' **previews**, never a
/// live object and never a payload: a resumed session rebuilds nothing by
/// re-running a cell, so there is nothing here for it to re-run.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CellRecord {
    pub cell: u64,
    /// The model's own TypeScript, as it wrote it — never the erased or
    /// wrapped JavaScript, which is pane's spelling and not the model's.
    pub source: String,
    /// The one line the model wrote about what this cell is for, in the
    /// person's language — `docs/product/pane/legibility.md` §2.
    ///
    /// **It is the model's stated intention, not a record of what ran.**
    /// `calls` is the record; when the two disagree the trajectory is the
    /// truth, and that disagreement is exactly what the supervisor's question
    /// is meant to see. Absent for a cell whose model said nothing and for
    /// every rollout row written before this field existed, and absent is
    /// never an error: the notebook falls back to the cell's first source
    /// line, which is what it drew before.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub outcome: CellOutcomeKind,
    pub handles: Vec<HandleRecord>,
    /// §9.4's trajectory: every call that actually ran in this cell, in
    /// order. An untaken branch ran nothing and records nothing; the answer
    /// itself is on the `turn` line, never here.
    pub calls: Vec<CallRecord>,
}

/// One live handle as the rollout records it.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct HandleRecord {
    pub name: String,
    #[serde(rename = "type")]
    pub type_name: String,
    pub preview: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provenance: Option<Provenance>,
}

/// One call of the trajectory — `runtime-contract.md` §9.4.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CallRecord {
    /// The registry name.
    pub tool: String,
    /// The arguments **as checked**: a path is the resolved path the child
    /// was given, never the program's spelling. A refused call carries only
    /// what was admitted before the refusing argument.
    pub args: BTreeMap<String, String>,
    /// Exact source material made visible by an inspection call before a
    /// semantic edit. Absent for ordinary calls and older rollout rows.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub evidence: Option<SourceEvidence>,
    /// The program the model actually wrote, when pane proved the command's
    /// meaning and ran a capability instead.
    ///
    /// One word, never the command line: `semantic-command-lifting.md` asks
    /// the ledger to explain that a shell-shaped request was lifted without
    /// persisting its payload, and `tool` already names what really ran.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lifted_from: Option<String>,
    /// The child's exit status, for a call that ran a process (`bash`,
    /// `checks.run`, a search or a read). Absent for an in-process call,
    /// whose `Some(0)` is a convention rather than an observation.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    /// The cell of an earlier pure call with the same tool, the same checked
    /// arguments and a byte-identical result. The call still ran — the file
    /// may have changed — and the hash is what decided it had not.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub repeat_of: Option<u64>,
    /// The one-line message a failed call threw, so a frame that isolates
    /// each call can still answer the provider with what went wrong. Absent
    /// for a call that ended `ok`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    pub ended: Ended,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceEvidence {
    pub path: String,
    pub sha256: String,
    pub complete: bool,
    pub ranges: Vec<SourceRange>,
    pub omissions: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceRange {
    pub path: String,
    pub start: usize,
    pub end: usize,
    pub role: String,
}

/// How one call ended: `"ok"`, `{"threw": "<class>"}` or
/// `{"denied": "<rule>"}` on the line. A cancelled call is a throw (§9.1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ended {
    Ok,
    Threw { class: String },
    Denied { rule: String },
}

impl Serialize for Ended {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Ended::Ok => serializer.serialize_str("ok"),
            Ended::Threw { class } => {
                let mut map = serializer.serialize_map(Some(1))?;
                map.serialize_entry("threw", class)?;
                map.end()
            }
            Ended::Denied { rule } => {
                let mut map = serializer.serialize_map(Some(1))?;
                map.serialize_entry("denied", rule)?;
                map.end()
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    #[test]
    fn a_rollout_line_serialises_to_the_contracts_own_shape() {
        let record = CellRecord {
            cell: 4,
            source: "const hits = await grep({});\n".into(),
            description: None,
            outcome: CellOutcomeKind::Yielded,
            handles: vec![HandleRecord {
                name: "hits".into(),
                type_name: "Grep.Match[]".into(),
                preview: "n=1195".into(),
                provenance: Some(Provenance {
                    tool: "grep".into(),
                    args: BTreeMap::from([("pattern".to_string(), "IntegrationId".to_string())]),
                    sha256: "9f2c".into(),
                    pure: true,
                }),
            }],
            calls: vec![
                CallRecord {
                    tool: "grep".into(),
                    args: BTreeMap::from([("path".to_string(), "/tmp/root".to_string())]),
                    evidence: None,
                    lifted_from: None,
                    exit_code: None,
                    repeat_of: None,
                    error: None,
                    ended: Ended::Ok,
                },
                CallRecord {
                    tool: "bash".into(),
                    args: BTreeMap::new(),
                    evidence: None,
                    lifted_from: None,
                    exit_code: None,
                    repeat_of: None,
                    error: None,
                    ended: Ended::Denied {
                        rule: "no allow".into(),
                    },
                },
                CallRecord {
                    tool: "read".into(),
                    args: BTreeMap::new(),
                    evidence: None,
                    lifted_from: None,
                    exit_code: None,
                    repeat_of: None,
                    error: None,
                    ended: Ended::Threw {
                        class: "Cancelled".into(),
                    },
                },
            ],
        };
        let json = serde_json::to_string(&record).unwrap();
        assert!(json.contains(r#""cell":4"#), "{json}");
        assert!(json.contains(r#""outcome":"yielded""#), "{json}");
        assert!(json.contains(r#""type":"Grep.Match[]""#), "{json}");
        assert!(json.contains(r#""pure":true"#), "{json}");
        assert!(
            json.contains(r#""calls":[{"tool":"grep","args":{"path":"/tmp/root"},"ended":"ok"}"#),
            "{json}"
        );
        assert!(json.contains(r#""ended":{"denied":"no allow"}"#), "{json}");
        assert!(json.contains(r#""ended":{"threw":"Cancelled"}"#), "{json}");
        // The optional per-call facts are absent from a line that has none.
        for absent in ["exit_code", "repeat_of", "\"error\""] {
            assert!(!json.contains(absent), "{absent} leaked into {json}");
        }
    }

    /// The three optional facts appear only when set, so an older rollout
    /// row and a row for a plain in-process call read the same.
    #[test]
    fn a_call_line_carries_its_exit_code_repeat_and_error_only_when_set() {
        let call = CallRecord {
            tool: "bash".into(),
            args: BTreeMap::new(),
            evidence: None,
            lifted_from: None,
            exit_code: Some(7),
            repeat_of: Some(3),
            error: Some("`read` failed with exit 1: no such file".into()),
            ended: Ended::Ok,
        };
        let json = serde_json::to_string(&call).unwrap();
        assert!(json.contains(r#""exit_code":7"#), "{json}");
        assert!(json.contains(r#""repeat_of":3"#), "{json}");
        assert!(json.contains(r#""error":"`read` failed"#), "{json}");
    }

    fn turn() -> CellTurn {
        CellTurn {
            elapsed_ms: 0,
            table: String::new(),
            stdout_tail: String::new(),
            stdout_dropped_tokens: 0,
            yield_reason: None,
            answer: None,
            record: CellRecord {
                cell: 1,
                source: String::new(),
                description: None,
                outcome: CellOutcomeKind::Yielded,
                handles: Vec::new(),
                calls: Vec::new(),
            },
            plan: Vec::new(),
            capability_results: Vec::new(),
            observation: Default::default(),
        }
    }

    /// **No returned value ends the task, whatever its type.** A cell ends
    /// it only by saying so with `answer(text)`, and a throw ends nothing
    /// even when the cell answered before it failed.
    #[test]
    fn only_an_answer_ends_the_task_and_no_returned_value_does() {
        let answered = || CellTurn {
            answer: Some("done".into()),
            ..turn()
        };
        for value in [
            Value::Null,
            Value::Number(3.0),
            Value::string("done"),
            Value::object(vec![("a".to_string(), Value::Number(1.0))]),
        ] {
            let terminal = Terminal::Json {
                text: "…".into(),
                cut: false,
            };
            assert!(
                !CellOutcome::Returned {
                    value: value.clone(),
                    terminal: terminal.clone(),
                    turn: turn(),
                }
                .ends_the_task(),
                "a returned {value:?} must not end the task"
            );
            assert!(
                CellOutcome::Returned {
                    value,
                    terminal,
                    turn: answered(),
                }
                .ends_the_task()
            );
        }
        assert!(!CellOutcome::Yielded { turn: turn() }.ends_the_task());
        assert!(CellOutcome::Yielded { turn: answered() }.ends_the_task());
        assert_eq!(
            CellOutcome::Yielded { turn: answered() }.answer(),
            Some("done")
        );
        assert!(
            !CellOutcome::Threw {
                error: ErrorValue::default(),
                turn: answered(),
            }
            .ends_the_task(),
            "a throw after an answer has not survived its own cell"
        );
    }

    #[test]
    fn a_cut_result_says_so_and_describes_the_whole_by_type() {
        let whole = Value::object(vec![
            ("matches".to_string(), Value::Number(3.0)),
            ("files".to_string(), Value::Number(2.0)),
        ]);
        let intact = Terminal::Json {
            text: r#"{"matches":3,"files":2}"#.into(),
            cut: false,
        };
        assert_eq!(intact.render(&whole), r#"{"matches":3,"files":2}"#);
        let cut = Terminal::Json {
            text: r#"{"matches":3,"fil"#.into(),
            cut: true,
        };
        let rendered = cut.render(&whole);
        assert!(rendered.starts_with(r#"{"matches":3,"fil"#), "{rendered}");
        assert!(rendered.contains("cut at 2,048 bytes"), "{rendered}");
        assert!(rendered.contains("\"matches\": number"), "{rendered}");
        assert_eq!(
            Terminal::Text("verbatim".into()).render(&Value::Null),
            "verbatim"
        );
    }
}
