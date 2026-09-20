//! Converts observed conversation/cell records to a readable, local document.
use super::{Action, CellTab, Workbench};
use crate::contract::{Block, Conversation, Message, Role};
use crate::prompt::{Extracted, extract_program};
use crate::tui::{Activity, CellView, Notebook, ScreenState};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    Normal,
    Code,
    Accent,
    Failure,
    Warning,
    Success,
    Muted,
}
#[derive(Debug, Clone)]
pub struct Row {
    pub text: String,
    pub tone: Tone,
    pub action: Option<Action>,
    pub key: (usize, usize),
}
#[derive(Default)]
pub struct Document {
    pub rows: Vec<Row>,
}
impl Document {
    pub fn push(
        &mut self,
        text: impl Into<String>,
        tone: Tone,
        action: Option<Action>,
        width: usize,
        id: usize,
    ) {
        let text = text.into();
        for line in text.split('\n') {
            let mut part = String::new();
            let mut n = 0;
            for c in line.chars() {
                if c.is_control() && c != '\t' {
                    continue;
                }
                let s = if c == '\t' {
                    "    ".to_string()
                } else {
                    c.to_string()
                };
                let w = ratatui::text::Span::raw(s.clone()).width();
                if n + w > width.max(1) && !part.is_empty() {
                    self.row(std::mem::take(&mut part), tone, action.clone(), id);
                    n = 0;
                }
                part.push_str(&s);
                n += w;
            }
            self.row(part, tone, action.clone(), id);
        }
    }
    fn row(&mut self, text: String, tone: Tone, action: Option<Action>, id: usize) {
        let ordinal = self
            .rows
            .last()
            .filter(|r| r.key.0 == id)
            .map_or(0, |r| r.key.1 + 1);
        self.rows.push(Row {
            text,
            tone,
            action,
            key: (id, ordinal),
        });
    }
    pub fn build(
        c: &Conversation,
        n: &Notebook,
        s: &ScreenState,
        ui: &Workbench,
        width: usize,
    ) -> Self {
        let mut d = Self::default();
        let mut cell: usize = 0;
        let mut after_return = false;
        let mut feedback = false;
        let mut returned_text: Option<String> = None;
        for (idx, m) in c.messages.iter().enumerate() {
            let id = idx + 1;
            if m.historical.is_some() {
                continue;
            }
            if m.role == Role::User {
                if m.content
                    .iter()
                    .any(|b| matches!(b, Block::ToolResult { .. }))
                {
                    feedback = false;
                    continue;
                }
                if feedback {
                    feedback = false;
                    continue;
                }
                after_return = false;
                d.push(format!("› {}", prose(m)), Tone::Normal, None, width, id);
                d.push("", Tone::Normal, None, width, id);
                continue;
            }
            if after_return {
                after_return = false;
                let text = prose(m);
                if returned_text.as_deref() != Some(text.trim()) {
                    d.push(text, Tone::Normal, None, width, id);
                    d.push("", Tone::Normal, None, width, id);
                }
                continue;
            }
            if idx > 0 {
                cell += 1;
            }
            let v = cell.checked_sub(1).and_then(|i| n.cells.get(i));
            let src = source(m);
            let has_cell = src.is_some()
                || v.is_some_and(|v| {
                    v.execution.is_some() || v.error.is_some() || v.executed_source.is_some()
                });
            if !has_cell {
                d.push(prose(m), Tone::Normal, None, width, id);
                d.push("", Tone::Normal, None, width, id);
                continue;
            }
            feedback = v.is_some_and(|v| v.answered);
            after_return = v.is_some_and(|v| v.returned.is_some());
            let explanation = explanation(m);
            if !explanation.trim().is_empty() {
                d.push(explanation, Tone::Normal, None, width, id);
            }
            let running = cell >= n.cells.len()
                && !c.messages[idx + 1..]
                    .iter()
                    .any(|next| next.role == Role::Assistant)
                && !matches!(
                    s.activity,
                    Activity::Idle | Activity::Complete | Activity::Failed
                )
                && v.is_none_or(|v| {
                    v.execution.is_none() && v.error.is_none() && v.returned.is_none()
                });
            let open = ui.expanded.contains(&cell)
                || (cell >= n.cells.len() && !ui.collapsed.contains(&cell))
                || v.is_some_and(|v| v.error.is_some());
            let state = if v.is_some_and(|v| v.error.is_some()) {
                "FAILED"
            } else if running {
                "RUNNING"
            } else if v.is_some_and(|v| v.execution.is_some()) {
                "EXECUTED"
            } else {
                "RECORDED"
            };
            let tone = if state == "FAILED" {
                Tone::Failure
            } else if running || ui.selected_cell == Some(cell) {
                Tone::Accent
            } else {
                Tone::Normal
            };
            let description = v.and_then(|v| v.description.as_deref()).unwrap_or("");
            d.push(
                format!(
                    "{} {cell:03}  {state}  {description}",
                    if open { "▾" } else { "▸" }
                ),
                tone,
                Some(Action::Cell(cell)),
                width,
                id,
            );
            if open {
                d.push("━".repeat(width.saturating_sub(1)), tone, None, width, id);
                let tab = ui.tabs.get(&cell).copied().unwrap_or(CellTab::Code);
                if v.is_some_and(|v| v.origin != crate::abi::Origin::AuthoredCell) {
                    d.push(
                        "Host-lowered tool frame · not model-authored source",
                        Tone::Normal,
                        None,
                        width,
                        id,
                    );
                }
                let program = v
                    .and_then(|v| v.executed_source.as_deref())
                    .or(src.as_deref())
                    .unwrap_or("");
                match tab {
                    CellTab::Code => d.push(program, Tone::Code, None, width, id),
                    CellTab::Diff => d.diff(v.and_then(|v| v.changes.as_deref()), width, id),
                    CellTab::Output => {
                        if let Some(v) = v {
                            for (name, value, tone) in [
                                ("Observed calls", &v.execution, Tone::Normal),
                                ("Result", &v.output, Tone::Normal),
                                ("stdout", &v.stdout, Tone::Muted),
                                ("Handles", &v.table, Tone::Muted),
                            ] {
                                if let Some(value) = value {
                                    d.push(name, Tone::Accent, None, width, id);
                                    d.push(value, tone, None, width, id);
                                }
                            }
                        }
                    }
                    CellTab::Helpers => {
                        if v.is_none_or(|v| v.helpers.is_empty()) {
                            d.push(
                                "No helper calls recorded for this cell.",
                                Tone::Normal,
                                None,
                                width,
                                id,
                            );
                        }
                    }
                }
                if let Some(v) = v {
                    if let Some(e) = &v.error {
                        d.push(
                            format!("✕ {}: {}", e.class, e.message),
                            Tone::Failure,
                            None,
                            width,
                            id,
                        );
                    }
                    if let Some(why) = &v.yield_reason {
                        d.push(format!("↳ {why}"), Tone::Normal, None, width, id);
                    }
                    if tab == CellTab::Code {
                        if let Some(output) = &v.output {
                            d.push(
                                output.lines().take(4).collect::<Vec<_>>().join("\n"),
                                Tone::Normal,
                                None,
                                width,
                                id,
                            );
                        }
                        if let Some(execution) = &v.execution {
                            for line in execution
                                .lines()
                                .filter(|l| l.contains(" · failed") || l.contains(" · denied"))
                            {
                                d.push(line, Tone::Failure, None, width, id);
                            }
                        }
                    }
                }
                d.push("─".repeat(width.saturating_sub(1)), tone, None, width, id);
                for (label, t) in [
                    ("Cell program", CellTab::Code),
                    ("Diff · before/after this cell", CellTab::Diff),
                    ("Output", CellTab::Output),
                    ("Helpers", CellTab::Helpers),
                ] {
                    // One row becomes four clickable spans in the view.
                    if t == tab {
                        d.push(
                            format!("[ {label} ]"),
                            Tone::Accent,
                            Some(Action::Tab(cell, t)),
                            width,
                            id,
                        );
                    }
                }
            }
            if let Some(v) = v {
                d.helpers(cell, v, ui, width, id);
                if let Some(answer) = v.returned.as_ref() {
                    let answer =
                        crate::prompt::completion_text(answer).unwrap_or_else(|| answer.clone());
                    d.push("", Tone::Normal, None, width, id);
                    d.push(&answer, Tone::Normal, None, width, id);
                    returned_text = Some(answer.trim().to_string());
                }
            }
            d.push("", Tone::Normal, None, width, id);
        }
        if let Some(p) = &n.preflight {
            d.push(
                format!("◇ SCOUT · {}", p.asked),
                Tone::Accent,
                None,
                width,
                usize::MAX - 2,
            );
            d.push(
                if p.outcome.ok {
                    p.outcome.text.clone()
                } else if p.outcome.text.is_empty() {
                    format!(
                        "Waiting on helper · {:.1}s · estimate unknown",
                        p.outcome.elapsed_ms as f64 / 1000.
                    )
                } else {
                    p.outcome.text.clone()
                },
                if !p.outcome.ok && !p.outcome.text.is_empty() {
                    Tone::Failure
                } else {
                    Tone::Normal
                },
                None,
                width,
                usize::MAX - 2,
            );
        }
        if let Some(fragment) = &s.streaming_tool_input {
            d.push(
                "Receiving cell input · not executed",
                Tone::Accent,
                None,
                width,
                usize::MAX - 3,
            );
            // Fragments are protocol text, never a claimed valid program.
            d.push(fragment, Tone::Muted, None, width, usize::MAX - 3);
        }
        if let Some(text) = &s.streaming_text {
            let text = crate::prompt::completion_text(text).unwrap_or_else(|| text.clone());
            d.push(text, Tone::Normal, None, width, usize::MAX - 1);
        }
        if d.rows.is_empty() {
            d.push(
                "P A N E   /   CODE · CELLS · LITTLE HELPERS",
                Tone::Accent,
                None,
                width,
                0,
            );
            d.push("\nDescribe the work. Inspect a cell or its helpers without leaving the conversation.\n\n/settings  Preferences    /models  Models    /help  Commands", Tone::Normal, None, width, 0);
        }
        d
    }
    fn diff(&mut self, value: Option<&str>, width: usize, id: usize) {
        self.push(
            "Observed changes · before → after this cell · already applied",
            Tone::Normal,
            None,
            width,
            id,
        );
        if let Some(diff) = value.filter(|v| !v.is_empty()) {
            for line in diff.lines() {
                let tone =
                    if line.starts_with("+++") || line.starts_with("---") || line.starts_with("@@")
                    {
                        Tone::Accent
                    } else if line.starts_with('+') {
                        Tone::Success
                    } else if line.starts_with('-') {
                        Tone::Failure
                    } else {
                        Tone::Normal
                    };
                self.push(line, tone, None, width, id);
            }
        } else {
            self.push(
                "No textual diff captured. This does not prove no files changed.",
                Tone::Normal,
                None,
                width,
                id,
            );
        }
    }
    fn helpers(&mut self, cell: usize, v: &CellView, ui: &Workbench, width: usize, id: usize) {
        for (i, h) in v.helpers.iter().enumerate() {
            let waiting = !h.outcome.ok && h.outcome.text.is_empty();
            let tone = if waiting {
                Tone::Accent
            } else if !h.outcome.ok {
                Tone::Failure
            } else {
                Tone::Normal
            };
            let result = if waiting {
                format!(
                    "{} · {:.1}s · estimate unknown",
                    h.verb,
                    h.outcome.elapsed_ms as f64 / 1000.
                )
            } else {
                h.outcome
                    .text
                    .lines()
                    .next()
                    .unwrap_or("Returned")
                    .to_string()
            };
            self.push(
                format!("◆ {}  {result}", h.helper),
                tone,
                Some(Action::Helper(cell, i)),
                width,
                id,
            );
            if ui.helper == Some((cell, i)) || ui.tabs.get(&cell) == Some(&CellTab::Helpers) {
                self.push(
                    format!("  Asked: {}", h.asked),
                    Tone::Normal,
                    None,
                    width,
                    id,
                );
                if !h.usage.model.is_empty() {
                    let model = &h.usage.model;
                    self.push(
                        format!("  Captured model: {model}"),
                        Tone::Normal,
                        None,
                        width,
                        id,
                    );
                }
                for step in &h.looked {
                    self.push(format!("  Observed: {step}"), Tone::Normal, None, width, id);
                }
                if waiting {
                    self.push(
                        "  Waiting for a returned value; no completed result yet.",
                        Tone::Normal,
                        None,
                        width,
                        id,
                    );
                } else {
                    self.push(
                        format!("  Returned: {}", h.outcome.text),
                        tone,
                        None,
                        width,
                        id,
                    );
                }
            }
        }
    }
}
fn prose(m: &Message) -> String {
    let s = m
        .content
        .iter()
        .filter_map(|b| match b {
            Block::Text(s) => Some(s.as_str()),
            Block::Image { .. } => Some("[image attachment]"),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("");
    crate::prompt::completion_text(&s).unwrap_or(s)
}
fn source(m: &Message) -> Option<String> {
    let calls: Vec<_> = m
        .content
        .iter()
        .filter_map(|b| match b {
            Block::ToolUse { name, input, .. } => Some((name, input)),
            _ => None,
        })
        .collect();
    if !calls.is_empty() {
        return (calls.len() == 1 && calls[0].0 == "execute_cell")
            .then(|| {
                calls[0]
                    .1
                    .get("code")
                    .and_then(|v| v.as_str())
                    .map(str::to_string)
            })
            .flatten();
    }
    match extract_program(&prose(m)) {
        Extracted::Program(p) | Extracted::Edit(p) => Some(p),
        _ => None,
    }
}

/// Only public prose outside the executable fence; no synthesized reasoning.
fn explanation(m: &Message) -> String {
    let text = prose(m);
    if m.content.iter().any(|b| matches!(b, Block::ToolUse { .. })) {
        return text;
    }
    let mut inside = false;
    let mut out = Vec::new();
    for line in text.lines() {
        if line.trim_start().starts_with("```") {
            inside = !inside;
            continue;
        }
        if !inside {
            out.push(line);
        }
    }
    out.join("\n")
}
