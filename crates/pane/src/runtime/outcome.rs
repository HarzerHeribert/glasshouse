//! What one cell produced — `runtime-contract.md` §1's two endings, §5's
//! third, and §4's rollout line.
//!
//! **A throw is a result, not an error.** [`CellOutcome::Threw`] carries the
//! same turn a yield would have carried — the elapsed time, the rendered
//! handle table, the captured stdout and the rollout record — and the
//! bindings the cell completed before throwing are in that table. Nothing
//! here is a `Result`, because none of the three endings is a failure of the
//! runtime.

use serde::Serialize;

use crate::runtime::handles::Provenance;
use crate::runtime::preview::{ErrorValue, Value};

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
    /// The one rollout line this cell owes — appended by the wiring package,
    /// never by this one.
    pub record: CellRecord,
}

/// How a cell ended.
#[derive(Debug, Clone, PartialEq)]
pub enum CellOutcome {
    /// It ran off the end. The model gets the table and another turn, and
    /// the isolate stays warm (§1).
    Yielded { turn: CellTurn },
    /// It executed a top-level `return`. The task ends with this value (§1).
    Returned { value: Value, turn: CellTurn },
    /// It threw. The turn slot a yield would have used carries the error
    /// instead, and the bindings made before the throw are in `turn.table`
    /// (§5).
    Threw { error: ErrorValue, turn: CellTurn },
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
        };
        let json = serde_json::to_string(&record).unwrap();
        assert!(json.contains(r#""cell":4"#), "{json}");
        assert!(json.contains(r#""outcome":"yielded""#), "{json}");
        assert!(json.contains(r#""type":"Grep.Match[]""#), "{json}");
        assert!(json.contains(r#""pure":true"#), "{json}");
    }

    #[test]
    fn only_a_return_ends_the_task() {
        let turn = CellTurn {
            elapsed_ms: 0,
            table: String::new(),
            stdout_tail: String::new(),
            stdout_dropped_tokens: 0,
            record: CellRecord {
                cell: 1,
                source: String::new(),
                outcome: CellOutcomeKind::Yielded,
                handles: Vec::new(),
            },
        };
        assert!(!CellOutcome::Yielded { turn: turn.clone() }.ends_the_task());
        assert!(
            CellOutcome::Returned {
                value: Value::Null,
                turn: turn.clone()
            }
            .ends_the_task()
        );
        assert!(
            !CellOutcome::Threw {
                error: ErrorValue::default(),
                turn
            }
            .ends_the_task()
        );
    }
}
