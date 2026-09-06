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

use serde::Serialize;
use serde::ser::{SerializeMap, Serializer};

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
    /// The one rollout line this cell owes — appended by the wiring package,
    /// never by this one.
    pub record: CellRecord,
}

/// How a cell ended.
#[derive(Debug, Clone, PartialEq)]
pub enum CellOutcome {
    /// It ran off the end, or asked to hand back. The model gets the table
    /// and another turn, and the isolate stays warm (§1, §9.3).
    Yielded { turn: CellTurn },
    /// It executed a top-level `return`. The task ends with this value (§1),
    /// and `terminal` is what the person reads (§9.2).
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

/// The task's terminal response — `runtime-contract.md` §9.2 — read at the
/// isolate boundary in full, never through `marshal`'s sample.
#[derive(Debug, Clone, PartialEq)]
pub enum Terminal {
    /// A returned string: the response, verbatim.
    Text(String),
    /// Any other returned value: its JSON with values, `cut` when the text
    /// is a prefix stopped at [`TERMINAL_JSON_CAP`] bytes on a character
    /// boundary.
    Json { text: String, cut: bool },
}

/// How many bytes of a non-string result's JSON the response carries before
/// it is cut — a *render* cap that never yields, unlike the response cap on a
/// returned string, which always does (§9.2).
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

    /// Whether the task is over — the one question the session loop asks of
    /// this value (§1: `return` ends the task, a yield and a throw do not).
    pub fn ends_the_task(&self) -> bool {
        matches!(self, CellOutcome::Returned { .. })
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
    pub ended: Ended,
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
                    ended: Ended::Ok,
                },
                CallRecord {
                    tool: "bash".into(),
                    args: BTreeMap::new(),
                    ended: Ended::Denied {
                        rule: "no allow".into(),
                    },
                },
                CallRecord {
                    tool: "read".into(),
                    args: BTreeMap::new(),
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
    }

    fn turn() -> CellTurn {
        CellTurn {
            elapsed_ms: 0,
            table: String::new(),
            stdout_tail: String::new(),
            stdout_dropped_tokens: 0,
            yield_reason: None,
            record: CellRecord {
                cell: 1,
                source: String::new(),
                outcome: CellOutcomeKind::Yielded,
                handles: Vec::new(),
                calls: Vec::new(),
            },
        }
    }

    #[test]
    fn only_a_return_ends_the_task() {
        assert!(!CellOutcome::Yielded { turn: turn() }.ends_the_task());
        assert!(
            CellOutcome::Returned {
                value: Value::Null,
                terminal: Terminal::Json {
                    text: "null".into(),
                    cut: false
                },
                turn: turn()
            }
            .ends_the_task()
        );
        assert!(
            !CellOutcome::Threw {
                error: ErrorValue::default(),
                turn: turn()
            }
            .ends_the_task()
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
