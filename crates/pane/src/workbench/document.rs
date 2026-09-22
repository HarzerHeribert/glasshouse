//! Converts observed conversation/cell records to a readable, local document.
//!
//! **A row says what kind of thing it is, and the view draws the shape.**
//! The conversation has a grammar -- your turn, Pane's turn, a cell's card,
//! a helper's lane, a local note, the answer -- and each row carries its
//! [`RowKind`] so the view can draw a gutter, a card edge or a label without
//! the text itself having to spell it. Selection and copying read
//! [`Row::text`], which stays the words alone.
use super::{Action, CellTab, Workbench, voice};
use crate::contract::{Block, Conversation, Message, Role};
use crate::prompt::{Extracted, extract_program};
use crate::tui::{Activity, CellView, Notebook, ScreenState};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    Normal,
    Code,
    /// Emphasis without hue: a cell's number, a final answer's first line.
    Strong,
    Accent,
    /// Little helpers and the evidence they return.
    Helper,
    Failure,
    Warning,
    Success,
    Muted,
    /// Rules and separators only.
    Line,
    /// The person's own turn: its gutter and its label.
    You,
}
/// What shape the view gives a row.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum RowKind {
    #[default]
    Plain,
    /// A line of the person's turn: a coloured bar on the left.
    You,
    /// The label row of one of Pane's turns.
    Pane,
    /// A cell card's title row. `open` draws the card's top edge; closed it
    /// is one line with a `▸`. `right` is the state word at the far end.
    CardTop {
        open: bool,
        right: Vec<(String, Tone)>,
    },
    /// A row inside a card: edges on both sides.
    CardBody,
    /// A cell card's bottom edge, with a short summary in it.
    CardBottom,
    /// A helper's lane under a card.
    Helper,
    /// A local notice, where it happened.
    Note,
    /// The first line of a finished turn's answer.
    Answer,
}
#[derive(Debug, Clone)]
pub struct Row {
    /// The whole line as it is drawn. Anchoring, selection and copying read
    /// this, so it stays the concatenation of [`Row::spans`] when those are set.
    pub text: String,
    pub tone: Tone,
    /// Per-segment colour for one line. Empty means the line is [`Row::tone`].
    pub spans: Vec<(String, Tone)>,
    /// A cell's tab strip: the view turns each entry into its own click
    /// target, which one flat string could not express.
    pub tabs: Vec<(String, CellTab)>,
    /// A row of chips: label, what it does, and whether it is the current one.
    pub chips: Vec<(String, Action, bool)>,
    pub action: Option<Action>,
    pub key: (usize, usize),
    pub kind: RowKind,
}
#[derive(Default)]
pub struct Document {
    pub rows: Vec<Row>,
    /// The kind every ordinary row takes while a card is open.
    container: RowKind,
    /// Whether the last turn labelled was Pane's, so one turn of several
    /// messages carries the name once.
    pane_open: bool,
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
        self.wrapped(text, tone, action, width, id, 0);
    }
    /// Wrap into the width, and hang every produced line under one indent so
    /// that a cell's body stays visibly inside the cell.
    pub fn wrapped(
        &mut self,
        text: impl Into<String>,
        tone: Tone,
        action: Option<Action>,
        width: usize,
        id: usize,
        indent: usize,
    ) {
        let text = text.into();
        let width = width.saturating_sub(indent).max(1);
        let pad = " ".repeat(indent);
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
                    let done = format!("{pad}{}", std::mem::take(&mut part));
                    self.row(done, tone, action.clone(), id);
                    n = 0;
                }
                part.push_str(&s);
                n += w;
            }
            let done = if part.is_empty() {
                part
            } else {
                format!("{pad}{part}")
            };
            self.row(done, tone, action.clone(), id);
        }
    }
    fn row(&mut self, text: String, tone: Tone, action: Option<Action>, id: usize) {
        let kind = self.container.clone();
        self.emit(
            Row {
                text,
                tone,
                spans: Vec::new(),
                tabs: Vec::new(),
                chips: Vec::new(),
                action,
                key: (0, 0),
                kind,
            },
            id,
        );
    }
    fn emit(&mut self, mut row: Row, id: usize) {
        let n = self.rows.iter().filter(|r| r.key.0 == id).count();
        row.key = (id, n);
        if row.kind == RowKind::Plain {
            row.kind = self.container.clone();
        }
        self.rows.push(row);
    }
    fn line(&mut self, spans: Vec<(String, Tone)>, action: Option<Action>, id: usize) {
        self.kinded(spans, action, id, RowKind::Plain);
    }
    fn kinded(
        &mut self,
        spans: Vec<(String, Tone)>,
        action: Option<Action>,
        id: usize,
        kind: RowKind,
    ) {
        self.emit(
            Row {
                text: spans.iter().map(|(t, _)| t.as_str()).collect(),
                tone: spans.first().map_or(Tone::Normal, |(_, t)| *t),
                spans,
                tabs: Vec::new(),
                chips: Vec::new(),
                action,
                key: (0, 0),
                kind,
            },
            id,
        );
    }
    /// A row that is nothing but chips.
    fn chips(&mut self, chips: Vec<(String, Action, bool)>, id: usize) {
        let text = chips
            .iter()
            .map(|(label, _, _)| format!("⟨ {label} ⟩"))
            .collect::<Vec<_>>()
            .join("  ");
        self.emit(
            Row {
                text,
                tone: Tone::Accent,
                spans: Vec::new(),
                tabs: Vec::new(),
                chips,
                action: None,
                key: (0, 0),
                kind: RowKind::Plain,
            },
            id,
        );
    }
    fn blank(&mut self, id: usize) {
        self.row(String::new(), Tone::Normal, None, id);
    }
    pub fn build(
        c: &Conversation,
        n: &Notebook,
        s: &ScreenState,
        ui: &Workbench,
        width: usize,
    ) -> Self {
        let mut d = Self::default();
        let mut note = 0usize;
        d.card(c, s, &mut note, width);
        // What the card drew is the session's own header, not conversation:
        // an empty conversation is still empty underneath it.
        let card_rows = d.rows.len();
        // A card's body sits inside two edges, a three-column indent and the
        // column kept clear before the gutter: nine columns in all.
        let inner = width.saturating_sub(9).max(1);
        let mut cell: usize = 0;
        let mut after_return = false;
        let mut feedback = false;
        let mut returned_text: Option<String> = None;
        let last_assistant = c
            .messages
            .iter()
            .rposition(|m| m.role == Role::Assistant && m.historical.is_none());
        for (idx, m) in c.messages.iter().enumerate() {
            let id = idx + 1;
            d.notes(s, &mut note, idx, width);
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
                d.turn_you(&prose(m), width, id);
                continue;
            }
            if after_return {
                after_return = false;
                let text = prose(m);
                if returned_text.as_deref() != Some(text.trim()) {
                    d.wrapped(text, Tone::Normal, None, width, id, 2);
                    d.blank(id);
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
            d.turn_pane(id);
            if !has_cell {
                d.wrapped(prose(m), Tone::Normal, None, width, id, 2);
                d.blank(id);
                continue;
            }
            feedback = v.is_some_and(|v| v.answered);
            after_return = v.is_some_and(|v| v.returned.is_some());
            let explanation = explanation(m);
            if !explanation.trim().is_empty() {
                d.wrapped(explanation, Tone::Normal, None, width, id, 2);
                d.blank(id);
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
            // A cell whose only work was the answer -- `answer(...)` and
            // maybe a todo, no call on the world -- is folded by default:
            // the answer is the block under it, and its card would only
            // show the same words again inside a string literal.
            let answer_only = v.is_some_and(|v| {
                v.returned.is_some()
                    && v.call_count.unwrap_or(0) == 0
                    && v.error.is_none()
                    && v.changes.as_deref().is_none_or(str::is_empty)
            });
            let open = ui.expanded.contains(&cell)
                || (cell >= n.cells.len() && !ui.collapsed.contains(&cell) && !answer_only)
                || v.is_some_and(|v| v.error.is_some());
            let failed = v.is_some_and(|v| v.error.is_some());
            let state = if failed {
                vec![("✕ FAILED".to_string(), Tone::Failure)]
            } else if running {
                vec![
                    ("● RUNNING".to_string(), Tone::Accent),
                    (format!(" {}", clock(s.pulse.elapsed_ms)), Tone::Muted),
                ]
            } else if v.is_some_and(|v| v.execution.is_some()) {
                vec![("✓ EXECUTED".to_string(), Tone::Success)]
            } else {
                vec![("RECORDED".to_string(), Tone::Muted)]
            };
            let tone = if failed {
                Tone::Failure
            } else if running || ui.selected_cell == Some(cell) {
                Tone::Accent
            } else {
                Tone::Line
            };
            let program_now = v
                .and_then(|v| v.executed_source.as_deref())
                .or(src.as_deref())
                .unwrap_or("");
            // A cell the model did not name is described by its own size,
            // which is a fact about it rather than a guess at its intent.
            let size = format!("{} lines", program_now.lines().count());
            let description = v
                .and_then(|v| v.description.as_deref())
                .filter(|d| !d.trim().is_empty())
                .unwrap_or(&size);
            d.kinded(
                vec![
                    (format!("{cell:03}"), Tone::Strong),
                    (" · ".to_string(), tone),
                    (
                        clip(description, width.saturating_sub(30)),
                        if open { Tone::Normal } else { Tone::Muted },
                    ),
                ],
                Some(Action::Cell(cell)),
                id,
                RowKind::CardTop { open, right: state },
            );
            if open {
                d.container = RowKind::CardBody;
                let tab = ui.tabs.get(&cell).copied().unwrap_or(CellTab::Code);
                d.tabstrip(cell, tab, v.and_then(|v| v.changes.as_deref()), inner, id);
                if v.is_some_and(|v| v.origin != crate::abi::Origin::AuthoredCell) {
                    d.push(
                        if s.voice.playful() {
                            "built-in step (I didn't write this one) · Host-lowered tool frame"
                        } else {
                            "Host-lowered tool frame · not model-authored source"
                        },
                        Tone::Muted,
                        None,
                        inner,
                        id,
                    );
                }
                let program = v
                    .and_then(|v| v.executed_source.as_deref())
                    .or(src.as_deref())
                    .unwrap_or("");
                match tab {
                    CellTab::Code => d.program(program, inner, id),
                    CellTab::Diff => d.diff(v.and_then(|v| v.changes.as_deref()), inner, id),
                    CellTab::Output => {
                        if let Some(v) = v {
                            for (name, value, tone) in [
                                ("Observed calls", &v.execution, Tone::Normal),
                                ("Result", &v.output, Tone::Normal),
                                ("stdout", &v.stdout, Tone::Muted),
                                ("Handles", &v.table, Tone::Muted),
                            ] {
                                if let Some(value) = value {
                                    d.wrapped(name, Tone::Accent, None, inner, id, 2);
                                    d.wrapped(value, tone, None, inner, id, 4);
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
                                inner,
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
                            inner,
                            id,
                        );
                    }
                    if let Some(why) = &v.yield_reason {
                        d.push(format!("↳ {why}"), Tone::Normal, None, inner, id);
                    }
                    if tab == CellTab::Code {
                        // What actually happened, in order, then what came
                        // back: the chain of calls is the cell's own record,
                        // never read off the program's text.
                        if let Some(execution) = &v.execution {
                            d.calls(execution, inner, id);
                        }
                        // The returned value is the answer block under the
                        // card when it is the answer; it is not printed twice.
                        if let Some(output) = &v.output
                            && v.returned.as_deref().map(str::trim) != Some(output.trim())
                        {
                            d.result(output, inner, id);
                        }
                    }
                }
                if running {
                    d.work(s, v, inner, id);
                }
                d.container = RowKind::Plain;
                d.kinded(
                    v.map(Self::summary).unwrap_or_default(),
                    None,
                    id,
                    RowKind::CardBottom,
                );
            }
            if let Some(v) = v {
                d.helpers(cell, v, ui, width, id);
                if let Some(answer) = v.returned.as_ref() {
                    let answer =
                        crate::prompt::completion_text(answer).unwrap_or_else(|| answer.clone());
                    d.blank(id);
                    d.answer(&answer, v, cell, s, last_assistant == Some(idx), width, id);
                    returned_text = Some(answer.trim().to_string());
                }
            }
            d.blank(id);
        }
        d.notes(s, &mut note, usize::MAX, width);
        if let Some(p) = &n.preflight {
            d.kinded(
                vec![
                    ("◇ PREFLIGHT · SCOUT  ".to_string(), Tone::Helper),
                    (
                        clip(&format!("{} {}", p.verb, p.asked), width.saturating_sub(24)),
                        Tone::Muted,
                    ),
                ],
                None,
                usize::MAX - 2,
                RowKind::Helper,
            );
            d.wrapped(
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
                5,
            );
        }
        if let Some(fragment) = &s.streaming_tool_input {
            d.turn_pane(usize::MAX - 3);
            d.streaming(fragment, n.cells.len() + 1, s, width);
        }
        if let Some(text) = &s.streaming_text {
            let text = crate::prompt::completion_text(text).unwrap_or_else(|| text.clone());
            if d.rows.len() == card_rows {
                d.turn_pane(usize::MAX - 1);
            }
            d.wrapped(text, Tone::Normal, None, width, usize::MAX - 1, 2);
        }
        if d.rows.len() == card_rows {
            d.opening(s, width);
        }
        d
    }
    /// The person's turn: a label, then the words, each under the gutter.
    fn turn_you(&mut self, text: &str, width: usize, id: usize) {
        self.pane_open = false;
        self.kinded(
            vec![(voice::YOU.to_string(), Tone::You)],
            None,
            id,
            RowKind::You,
        );
        let before = self.rows.len();
        self.wrapped(text, Tone::Strong, None, width.saturating_sub(3), id, 0);
        for row in &mut self.rows[before..] {
            row.kind = RowKind::You;
        }
        self.blank(id);
    }
    /// Pane's turn begins: the mark and the name, once, above whatever it
    /// says and does.
    fn turn_pane(&mut self, id: usize) {
        if self.pane_open {
            return;
        }
        self.pane_open = true;
        self.kinded(
            vec![(format!("⠿ {}", voice::PANE), Tone::Accent)],
            None,
            id,
            RowKind::Pane,
        );
    }
    /// A finished turn's answer: the first line as the result a person came
    /// back for, the rest as prose, a line of what it cost, and -- on the
    /// latest turn only -- what to do next.
    #[allow(clippy::too_many_arguments)]
    fn answer(
        &mut self,
        answer: &str,
        v: &CellView,
        cell: usize,
        s: &ScreenState,
        latest: bool,
        width: usize,
        id: usize,
    ) {
        let mut lines = answer.splitn(2, '\n');
        let first = lines.next().unwrap_or("").trim();
        let first = if first.is_empty() {
            voice::done_line(s.voice, v.error.is_some()).to_string()
        } else {
            first.to_string()
        };
        // The words as the model returned them, on their own line: a test
        // or a person reading a line back finds exactly that line.
        let before = self.rows.len();
        self.wrapped(first, Tone::Strong, None, width, id, 2);
        if let Some(row) = self.rows.get_mut(before) {
            row.kind = RowKind::Answer;
        }
        if let Some(rest) = lines.next().filter(|r| !r.trim().is_empty()) {
            self.wrapped(rest, Tone::Normal, None, width, id, 4);
        }
        let files = changed_files(v);
        let (added, removed) = v.changes.as_deref().map_or((0, 0), count_changes);
        let mut facts = Vec::new();
        if files > 0 {
            facts.push(format!(
                "{files} {} · +{added} −{removed}",
                if files == 1 { "file" } else { "files" }
            ));
        }
        if !v.helpers.is_empty() {
            facts.push(format!(
                "{} {}",
                v.helpers.len(),
                if v.helpers.len() == 1 {
                    "helper"
                } else {
                    "helpers"
                }
            ));
        }
        if let Some(calls) = v.call_count.filter(|c| *c > 0) {
            facts.push(format!(
                "{calls} {}",
                if calls == 1 { "call" } else { "calls" }
            ));
        }
        let (mark, tone) = if v.error.is_some() {
            ("✕", Tone::Failure)
        } else {
            ("✓", Tone::Success)
        };
        let facts = if facts.is_empty() {
            voice::done_line(s.voice, v.error.is_some()).to_lowercase()
        } else {
            facts.join(" · ")
        };
        self.line(
            vec![(format!("  {mark} "), tone), (facts, Tone::Muted)],
            None,
            id,
        );
        if latest {
            let mut chips = Vec::new();
            if files > 0 {
                chips.push((
                    "show the diff".to_string(),
                    Action::Command("/diff".into()),
                    false,
                ));
                chips.push((
                    "commit this".to_string(),
                    Action::Insert("commit this".into()),
                    false,
                ));
            }
            chips.push((
                "full output".to_string(),
                Action::Tab(cell, CellTab::Output),
                false,
            ));
            if !chips.is_empty() {
                self.chips(chips, id);
            }
        }
    }
    /// The cell's own result, marked pass or fail from what it actually
    /// said. Nothing here judges the work -- the mark reports the words the
    /// run produced, and the words stay on the line beside it.
    fn result(&mut self, output: &str, width: usize, id: usize) {
        // A value that is JSON is shown as JSON is read, one field to a
        // line; the runtime's own cut marker stays as it came.
        let pretty = serde_json::from_str::<serde_json::Value>(output.trim())
            .ok()
            .filter(|v| v.is_object() || v.is_array())
            .and_then(|v| serde_json::to_string_pretty(&v).ok());
        let text = pretty.as_deref().unwrap_or(output);
        let head: String = text.lines().take(4).collect::<Vec<_>>().join("\n");
        let lower = head.to_lowercase();
        let bad = ["fail", "error", "panic", "✕"]
            .iter()
            .any(|w| lower.contains(w));
        let mut lines = text.lines();
        if let Some(first) = lines.next() {
            self.line(
                vec![
                    (
                        format!("{} ", if bad { "✕" } else { "✓" }),
                        if bad { Tone::Failure } else { Tone::Success },
                    ),
                    (clip(first, width.saturating_sub(4)), Tone::Normal),
                ],
                None,
                id,
            );
        }
        let rest: Vec<&str> = lines.collect();
        for line in rest.iter().take(9) {
            self.line(
                vec![
                    ("  ".to_string(), Tone::Normal),
                    (clip(line, width.saturating_sub(4)), Tone::Muted),
                ],
                None,
                id,
            );
        }
        if rest.len() > 9 {
            self.line(
                vec![(
                    format!(
                        "  … {} more lines · Full output has them all",
                        rest.len() - 9
                    ),
                    Tone::Muted,
                )],
                None,
                id,
            );
        }
    }
    /// The program, with the calls that act on the world lit up: `read`,
    /// `edit`, `bash`, a helper, a check -- every `await`ed call and every
    /// call on one of the runtime's own objects -- so the chain of events a
    /// cell will cause is read off it at a glance.
    fn program(&mut self, program: &str, width: usize, id: usize) {
        // `answer("…")` carries the whole answer as one string literal; the
        // block under the card shows it, so here the call is folded to its
        // opening words.
        let mut folded = false;
        for line in program.split('\n') {
            if folded {
                if line.trim_end().ends_with(");") {
                    folded = false;
                }
                continue;
            }
            if let Some(start) = line.find("answer(\"") {
                let head: String = line[start + 8..].chars().take(36).collect();
                let one_line = line.trim_end().ends_with(");");
                folded = !one_line;
                self.spans_wrapped(
                    vec![
                        (line[..start].to_string(), Tone::Code),
                        ("answer".to_string(), Tone::Accent),
                        (format!("(\"{head}"), Tone::Code),
                        ("…\")  · the answer is below".to_string(), Tone::Muted),
                    ],
                    width,
                    2,
                    id,
                );
                continue;
            }
            self.spans_wrapped(highlight_calls(line), width, 2, id);
        }
    }
    /// Spans on one logical line, wrapped into the width under a hanging
    /// indent, so a highlighted call survives the wrap.
    fn spans_wrapped(
        &mut self,
        spans: Vec<(String, Tone)>,
        width: usize,
        indent: usize,
        id: usize,
    ) {
        let avail = width.saturating_sub(indent).max(1);
        let pad = " ".repeat(indent);
        let mut row: Vec<(String, Tone)> = vec![(pad.clone(), Tone::Code)];
        let mut used = 0;
        for (text, tone) in spans {
            let mut part = String::new();
            for c in text.chars() {
                if c.is_control() && c != '\t' {
                    continue;
                }
                let s = if c == '\t' {
                    "    ".to_string()
                } else {
                    c.to_string()
                };
                let w = span_width(&s);
                if used + w > avail && used > 0 {
                    if !part.is_empty() {
                        row.push((std::mem::take(&mut part), tone));
                    }
                    self.line(
                        std::mem::replace(&mut row, vec![(pad.clone(), Tone::Code)]),
                        None,
                        id,
                    );
                    used = 0;
                }
                part.push_str(&s);
                used += w;
            }
            if !part.is_empty() {
                row.push((part, tone));
            }
        }
        self.line(row, None, id);
    }
    /// The chain of calls a cell actually made, one per line: what ran, on
    /// what, and how it ended. Read off the cell's record, so a call the
    /// program names but never reached is not on it.
    fn calls(&mut self, execution: &str, width: usize, id: usize) {
        if execution.starts_with("No tool calls") {
            self.line(
                vec![(
                    "  · no tool calls ran in this cell".to_string(),
                    Tone::Muted,
                )],
                None,
                id,
            );
            return;
        }
        for line in execution.lines() {
            let line = line
                .trim_start()
                .trim_start_matches("├─ ")
                .trim_start_matches("└─ ")
                .trim_start_matches("├─")
                .trim_start_matches("└─")
                .trim();
            let (what, status) = match line.find(" · ") {
                Some(i) => (&line[..i], &line[i + " · ".len()..]),
                None => (line, ""),
            };
            let word = status.split(" · ").next().unwrap_or("");
            let (mark, tone) = match word {
                "returned" => ("✓", Tone::Success),
                "started" => ("●", Tone::Accent),
                "failed" => ("✕", Tone::Failure),
                "denied" => ("⊘", Tone::Warning),
                _ => ("·", Tone::Muted),
            };
            let (tool, arg) = match what.split_once(' ') {
                Some((tool, arg)) => (tool.to_string(), arg.to_string()),
                None => (what.to_string(), String::new()),
            };
            let detail = status
                .split_once(" · ")
                .map(|(_, rest)| rest.to_string())
                .unwrap_or_default();
            let mut left = vec![
                (format!("  {mark} "), tone),
                (format!("{tool:<7} "), Tone::Accent),
                (arg, Tone::Normal),
            ];
            if !detail.is_empty() {
                left.push((format!("  {detail}"), tone));
            }
            let right = vec![(word.to_string(), tone)];
            let row = justify(left, right, width);
            self.line(row, None, id);
        }
    }
    /// What the screen shows of a cell the model is still writing.
    ///
    /// The provider sends the program as fragments of a JSON string, and
    /// that text is nobody's to read. Decoded, the fragments are the program
    /// as it forms; `ui.stream` chooses between that, one quiet line, and
    /// the raw text for anyone debugging the protocol itself.
    fn streaming(&mut self, fragment: &str, cell: usize, s: &ScreenState, width: usize) {
        let id = usize::MAX - 3;
        let code = partial_code(fragment);
        match (s.stream, code) {
            (crate::tui::Stream::Raw, _) | (_, None) => {
                self.wrapped(
                    "Receiving cell input · not executed",
                    Tone::Accent,
                    None,
                    width,
                    id,
                    2,
                );
                // Fragments are protocol text, never a claimed valid program.
                let shown: String = if s.stream == crate::tui::Stream::Raw {
                    fragment.to_string()
                } else {
                    fragment.lines().take(3).collect::<Vec<_>>().join("\n")
                };
                self.wrapped(shown, Tone::Muted, None, width, id, 2);
            }
            (crate::tui::Stream::Quiet, Some(code)) => {
                self.line(
                    vec![(
                        format!(
                            "  ● writing cell {cell:03} · {} lines so far · not executed",
                            code.lines().count().max(1)
                        ),
                        Tone::Accent,
                    )],
                    None,
                    id,
                );
            }
            (crate::tui::Stream::Code, Some(code)) => {
                let lines: Vec<&str> = code.lines().collect();
                self.line(
                    vec![(
                        format!(
                            "  ● writing cell {cell:03} · {} lines so far · not executed",
                            lines.len().max(1)
                        ),
                        Tone::Accent,
                    )],
                    None,
                    id,
                );
                let skip = lines.len().saturating_sub(12);
                if skip > 0 {
                    self.line(
                        vec![(format!("    … {skip} lines above"), Tone::Muted)],
                        None,
                        id,
                    );
                }
                for line in lines.iter().skip(skip) {
                    self.spans_wrapped(highlight_calls(line), width, 4, id);
                }
            }
        }
    }
    /// What a session says about itself before anyone has said anything to
    /// it: the bird, a greeting, and the two facts that are not already on
    /// the status line, each on exactly one line.
    ///
    /// **A startup note is a card, not a paragraph.** These arrive before the
    /// first message — a resume id, the rung, three sentences about the
    /// sandbox — and drawn as prose they are the first and largest thing in
    /// the conversation, saying what the status line already says. So they
    /// are clipped to one line each, capped, and the rest is one keystroke
    /// away. Everything after the conversation starts is a notice where it
    /// happened (see [`Document::notes`]).
    fn card(&mut self, c: &Conversation, s: &ScreenState, next: &mut usize, width: usize) {
        // Before the first keystroke there is nothing to have caused a
        // notice, so an opening set that has not been frozen yet is the
        // whole of it -- and a conversation that already has messages (a
        // resumed session) never had an opening to collect.
        let opening = s.startup_notes.unwrap_or(if c.messages.is_empty() {
            s.history.iter().take_while(|n| n.after == 0).count()
        } else {
            0
        });
        let startup: Vec<&str> = s
            .history
            .iter()
            .take(opening)
            .flat_map(|n| n.text.lines().next())
            .collect();
        *next += opening;
        let asking = false;
        let still = s.reduced_motion || s.selection.is_some();
        let art = voice::face(
            voice::Face::of(s.activity, asking),
            s.animation_frame,
            still,
        );
        // **The affordance beside the fact.** A card that states what the
        // session is and says nothing about how to change it makes a reader
        // go looking; naming the control on the same line is the cheapest
        // teaching this screen can do.
        let model = match s.model.as_deref() {
            Some(model) => format!("{model}   /model changes it"),
            None => "no model chosen   /models picks one".to_string(),
        };
        let second = startup
            .first()
            .copied()
            .map(str::to_string)
            .unwrap_or_else(|| model.clone());
        let project = s.project.as_deref().unwrap_or("no project");
        let facts = [
            (
                voice::greeting(s.voice, s.local_hour, project),
                Tone::Strong,
            ),
            (
                clip(&second, width.saturating_sub(voice::FACE_WIDTH + 4)),
                Tone::Muted,
            ),
            (
                if startup.len() > 1 {
                    format!("+{} more · /activity", startup.len() - 1)
                } else if s.voice.playful() {
                    "code · cells · little helpers · one small bird".to_string()
                } else {
                    "code · cells · little helpers".to_string()
                },
                Tone::Line,
            ),
            (
                // The model line, when the startup note took its place.
                if startup.is_empty() {
                    String::new()
                } else {
                    clip(&model, width.saturating_sub(voice::FACE_WIDTH + 4))
                },
                Tone::Muted,
            ),
        ];
        for (i, (glyph, (text, tone))) in art.iter().zip(facts).enumerate() {
            // The bird is the row's only target: a click anywhere on it is a
            // remark, never a surface opening under a stray report.
            let _ = i;
            self.line(
                vec![(format!(" {glyph}  "), Tone::Accent), (text, tone)],
                Some(Action::Quip),
                0,
            );
        }
        self.rule(width, 0);
        self.blank(0);
    }
    /// An empty conversation: one line, then the things to press. A
    /// paragraph on an empty screen is read once and never again; a row of
    /// chips is read every time someone does not know what to type.
    fn opening(&mut self, s: &ScreenState, width: usize) {
        self.wrapped(voice::invitation(s.voice), Tone::Normal, None, width, 0, 2);
        self.blank(0);
        let chips: Vec<(String, Action, bool)> = if s.suggestions.is_empty() {
            voice::suggestions(s.voice, None, 0, false)
        } else {
            s.suggestions.clone()
        }
        .into_iter()
        .map(|(label, message)| (label, Action::Insert(message), false))
        .collect();
        self.chips(chips, 0);
        self.blank(0);
        for (key, what) in [
            (
                "/settings",
                "everything about this session, applied as you choose it",
            ),
            ("/models", "which model answers, and at what effort"),
            ("/diff", "what the last cell actually changed"),
            ("/help", "every command"),
        ] {
            self.line(
                vec![
                    (format!("  {key:<11}"), Tone::Accent),
                    (what.to_string(), Tone::Muted),
                ],
                Some(Action::Insert(key.to_string())),
                0,
            );
        }
        self.blank(0);
        self.wrapped(
            if s.voice.playful() {
                "or just type. / for commands · @ for a file in this project"
            } else {
                "Type / for commands · @ for a path in this project"
            },
            Tone::Muted,
            None,
            width,
            0,
            2,
        );
    }
    /// Local notices, drawn where they happened.
    ///
    /// **A notice is not a message and never pretends to be one.** It is
    /// quiet, it is marked, and it stays in the transcript where a person
    /// can scroll back to it -- the same notes the Activity surface lists.
    fn notes(&mut self, s: &ScreenState, next: &mut usize, upto: usize, width: usize) {
        while let Some(note) = s.history.get(*next).filter(|n| n.after <= upto) {
            *next += 1;
            let bad = note.text.starts_with("ERROR:");
            for line in note.text.lines() {
                self.kinded(
                    vec![
                        (
                            format!("  {} ", if bad { "✕" } else { "·" }),
                            if bad { Tone::Failure } else { Tone::Line },
                        ),
                        (
                            clip(line, width.saturating_sub(5)),
                            if bad { Tone::Failure } else { Tone::Muted },
                        ),
                    ],
                    None,
                    usize::MAX - 4,
                    RowKind::Note,
                );
            }
        }
    }
    /// A full-width separator in the one colour reserved for separators.
    fn rule(&mut self, width: usize, id: usize) {
        self.line(
            vec![("─".repeat(width.saturating_sub(1)), Tone::Line)],
            None,
            id,
        );
    }
    /// The cell's own navigation: what this cell changed, what it ran, what
    /// came back, and a route to the full diff. The counts are the observed
    /// patch's, so an empty capture says so by showing no counts at all.
    fn tabstrip(
        &mut self,
        cell: usize,
        current: CellTab,
        changes: Option<&str>,
        width: usize,
        id: usize,
    ) {
        let (added, removed) = changes.map_or((0, 0), count_changes);
        let changed = if added + removed > 0 {
            format!("Changes +{added} −{removed}")
        } else {
            "Changes".to_string()
        };
        let tabs = vec![
            (changed, CellTab::Diff),
            ("Cell program".to_string(), CellTab::Code),
            ("Full output".to_string(), CellTab::Output),
            ("Helpers".to_string(), CellTab::Helpers),
        ];
        let text = tabs
            .iter()
            .map(|(label, _)| format!("⟨ {label} ⟩ "))
            .collect::<String>();
        let pad = width
            .saturating_sub(span_width(&text))
            .saturating_sub(OPEN_DIFF.chars().count() + 1);
        self.emit(
            Row {
                text: format!("{text}{}{OPEN_DIFF}", " ".repeat(pad)),
                tone: Tone::Normal,
                spans: vec![
                    (" ".repeat(pad), Tone::Normal),
                    (OPEN_DIFF.into(), Tone::Muted),
                ],
                tabs,
                chips: Vec::new(),
                action: Some(Action::Tab(cell, current)),
                key: (0, 0),
                kind: RowKind::CardBody,
            },
            id,
        );
    }
    /// The bird at work inside a running cell, and beside it what is
    /// actually happening, what was last observed, and at whose cost.
    fn work(&mut self, s: &ScreenState, v: Option<&CellView>, width: usize, id: usize) {
        let still = s.reduced_motion || s.selection.is_some();
        let helper = v.and_then(|v| v.helpers.last());
        let waiting = helper.is_some_and(|h| !h.outcome.ok && h.outcome.text.is_empty());
        let (label, detail) =
            voice::working(s.voice, s.activity, waiting, &clock(s.pulse.elapsed_ms));
        let cost = helper.map_or_else(
            || {
                s.model
                    .as_deref()
                    .map_or_else(|| "model unknown".into(), |m| format!("this session · {m}"))
            },
            |h| {
                format!(
                    "{} · {}",
                    h.helper,
                    if h.usage.model.is_empty() {
                        "captured model unknown"
                    } else {
                        h.usage.model.as_str()
                    }
                )
            },
        );
        let art = voice::face(voice::Face::Working, s.animation_frame, still);
        for (glyph, (text, tone)) in art.into_iter().zip([
            (format!("◈ {label}"), Tone::Accent),
            (detail, Tone::Normal),
            (cost, Tone::Muted),
            (String::new(), Tone::Normal),
        ]) {
            let text = clip(&text, width.saturating_sub(voice::FACE_WIDTH + 4));
            self.line(
                vec![
                    (
                        format!(" {glyph} "),
                        if still { Tone::Line } else { Tone::Accent },
                    ),
                    (text, tone),
                ],
                None,
                id,
            );
        }
    }
    /// A unified patch with both line numbers, the way the mockup reads it:
    /// where the line was, where it is now, and which side it belongs to.
    ///
    /// The numbers come from the patch's own hunk headers. A patch without
    /// them still renders -- the gutter is simply blank, which is honest --
    fn diff(&mut self, value: Option<&str>, width: usize, id: usize) {
        self.line(
            vec![(
                "Observed changes · before → after this cell · already applied".to_string(),
                Tone::Muted,
            )],
            None,
            id,
        );
        let Some(diff) = value.filter(|v| !v.is_empty()) else {
            self.line(
                vec![(
                    "No textual diff captured. This does not prove no files changed.".to_string(),
                    Tone::Warning,
                )],
                None,
                id,
            );
            return;
        };
        let (mut old_no, mut new_no) = (0usize, 0usize);
        for line in diff.lines() {
            if let Some(path) = line.strip_prefix("+++ ") {
                self.line(
                    vec![(path.trim_start_matches("b/").to_string(), Tone::Accent)],
                    None,
                    id,
                );
                continue;
            }
            if line.starts_with("--- ") {
                continue;
            }
            if let Some(header) = line.strip_prefix("@@") {
                (old_no, new_no) = hunk(header);
                self.line(vec![(line.to_string(), Tone::Line)], None, id);
                continue;
            }
            let (mark, tone, old_cell, new_cell) = match line.chars().next() {
                Some('+') => {
                    new_no += 1;
                    ("+", Tone::Success, String::new(), new_no.to_string())
                }
                Some('-') => {
                    old_no += 1;
                    ("−", Tone::Failure, old_no.to_string(), String::new())
                }
                _ => {
                    old_no += 1;
                    new_no += 1;
                    (" ", Tone::Muted, old_no.to_string(), new_no.to_string())
                }
            };
            let body: String = line.chars().skip(1).collect();
            self.line(
                vec![
                    (format!("{old_cell:>5} {new_cell:>5} {mark} "), Tone::Line),
                    (clip(&body, width.saturating_sub(14)), tone),
                ],
                None,
                id,
            );
        }
    }
    /// The words in a card's bottom edge: how it ended, and how many files
    /// it touched. Every claim here is observed.
    fn summary(v: &CellView) -> Vec<(String, Tone)> {
        let mut out = Vec::new();
        if let Some(e) = &v.error {
            out.push((format!("✕ {}", e.class), Tone::Failure));
        } else if v.execution.is_some() {
            out.push(("✓ executed".to_string(), Tone::Success));
        }
        let files = changed_files(v);
        let files = match files {
            0 => "no captured file changes".to_string(),
            1 => "1 file changed".to_string(),
            n => format!("{n} files changed"),
        };
        if !out.is_empty() {
            out.push((" · ".to_string(), Tone::Line));
        }
        out.push((files, Tone::Muted));
        out
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
                    "{} {} · {:.1}s · estimate unknown",
                    h.verb,
                    h.asked,
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
            let open =
                ui.helper == Some((cell, i)) || ui.tabs.get(&cell) == Some(&CellTab::Helpers);
            let right = format!(
                "{} {}",
                if waiting || h.outcome.elapsed_ms == 0 {
                    String::new()
                } else {
                    format!("{:.1}s", h.outcome.elapsed_ms as f64 / 1000.)
                },
                if open { "▾" } else { "▸" }
            );
            let left = vec![
                (
                    format!("     ◇ {}  ", h.helper),
                    if tone == Tone::Normal {
                        Tone::Helper
                    } else {
                        tone
                    },
                ),
                (
                    clip(&result, width.saturating_sub(14 + h.helper.len())),
                    if tone == Tone::Normal {
                        Tone::Muted
                    } else {
                        tone
                    },
                ),
            ];
            self.kinded(
                justify(left, vec![(right, Tone::Muted)], width),
                Some(Action::Helper(cell, i)),
                id,
                RowKind::Helper,
            );
            if open {
                self.push(
                    format!("       Asked: {}", h.asked),
                    Tone::Normal,
                    None,
                    width,
                    id,
                );
                if !h.usage.model.is_empty() {
                    let model = &h.usage.model;
                    self.push(
                        format!("       Captured model: {model}"),
                        Tone::Normal,
                        None,
                        width,
                        id,
                    );
                }
                for step in &h.looked {
                    self.push(
                        format!("       Observed: {step}"),
                        Tone::Normal,
                        None,
                        width,
                        id,
                    );
                }
                if waiting {
                    self.push(
                        "       Waiting for a returned value; no completed result yet.",
                        Tone::Normal,
                        None,
                        width,
                        id,
                    );
                } else {
                    self.push(
                        format!("       Returned: {}", h.outcome.text),
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
const OPEN_DIFF: &str = "open diff ↗";

/// The runtime's own objects: a call on one of these acts on the world or
/// on the session, whether or not the program awaits it.
const ACTING: [&str; 22] = [
    "read", "write", "edit", "bash", "fd", "grep", "glob", "fetch", "search", "ssh", "web",
    "checks", "helper", "agent", "handles", "bg", "decide", "mcp", "send", "print", "ask", "plan",
];
/// One line of a program as spans: acting calls in the accent, the three
/// words that shape a cell quiet, everything else as code.
pub(crate) fn highlight_calls(line: &str) -> Vec<(String, Tone)> {
    let chars: Vec<char> = line.chars().collect();
    let mut out: Vec<(String, Tone)> = Vec::new();
    let mut plain = String::new();
    let mut i = 0;
    let ident = |c: char| c.is_alphanumeric() || c == '_' || c == '.';
    while i < chars.len() {
        let c = chars[i];
        if (c.is_alphabetic() || c == '_') && (i == 0 || !ident(chars[i - 1])) {
            let start = i;
            while i < chars.len() && ident(chars[i]) {
                i += 1;
            }
            let word: String = chars[start..i].iter().collect();
            let call = chars.get(i) == Some(&'(');
            let base = word.split('.').next().unwrap_or("");
            let awaited = plain.trim_end().ends_with("await");
            let tone = if call && (awaited || ACTING.contains(&base)) {
                Some(Tone::Accent)
            } else if matches!(word.as_str(), "await" | "const" | "return" | "let") && !call {
                Some(Tone::Muted)
            } else {
                None
            };
            match tone {
                Some(tone) => {
                    if !plain.is_empty() {
                        out.push((std::mem::take(&mut plain), Tone::Code));
                    }
                    out.push((word, tone));
                }
                None => plain.push_str(&word),
            }
            continue;
        }
        plain.push(c);
        i += 1;
    }
    if !plain.is_empty() || out.is_empty() {
        out.push((plain, Tone::Code));
    }
    out
}
/// The program inside a half-arrived tool call: the JSON string under
/// `"code"`, decoded as far as it has come. `None` until the key is there,
/// or for a call that carries no program.
pub(crate) fn partial_code(fragment: &str) -> Option<String> {
    let key = fragment.find("\"code\"")?;
    let rest = &fragment[key + "\"code\"".len()..];
    let rest = rest.trim_start().strip_prefix(':')?.trim_start();
    let body = rest.strip_prefix('"')?;
    let mut out = String::new();
    let mut chars = body.chars();
    while let Some(c) = chars.next() {
        match c {
            '"' => break,
            '\\' => match chars.next() {
                Some('n') => out.push('\n'),
                Some('t') => out.push('\t'),
                Some('r') => out.push('\r'),
                Some('"') => out.push('"'),
                Some('\\') => out.push('\\'),
                Some('/') => out.push('/'),
                Some('b') | Some('f') => {}
                Some('u') => {
                    let hex: String = chars.by_ref().take(4).collect();
                    if hex.len() < 4 {
                        break;
                    }
                    if let Some(ch) = u32::from_str_radix(&hex, 16).ok().and_then(char::from_u32) {
                        out.push(ch);
                    }
                }
                // An escape cut in half by the fragment boundary.
                _ => break,
            },
            c => out.push(c),
        }
    }
    Some(out)
}

fn changed_files(v: &CellView) -> usize {
    v.changes
        .as_deref()
        .map_or(0, |d| d.lines().filter(|l| l.starts_with("+++ ")).count())
}
fn span_width(text: &str) -> usize {
    ratatui::text::Span::raw(text).width()
}
/// Cut to a column budget on a character boundary; never mid-escape, because
/// control characters never reach a row in the first place.
pub(crate) fn clip(text: &str, width: usize) -> String {
    if span_width(text) <= width {
        return text.to_string();
    }
    let mut out = String::new();
    for c in text.chars() {
        if span_width(&out) + span_width(&c.to_string()) > width.saturating_sub(1) {
            out.push('…');
            break;
        }
        out.push(c);
    }
    out
}
/// Two groups of spans on one line, the second pushed to the right edge. The
/// left group loses characters first: a right-edge state word is the one thing
/// a narrow terminal must not drop.
pub(super) fn justify(
    left: Vec<(String, Tone)>,
    right: Vec<(String, Tone)>,
    width: usize,
) -> Vec<(String, Tone)> {
    let rw: usize = right.iter().map(|(t, _)| span_width(t)).sum();
    let budget = width.saturating_sub(rw).saturating_sub(1);
    let mut out: Vec<(String, Tone)> = Vec::new();
    let mut used = 0;
    for (text, tone) in left {
        let room = budget.saturating_sub(used);
        if room == 0 {
            break;
        }
        let text = clip(&text, room);
        used += span_width(&text);
        out.push((text, tone));
    }
    out.push((" ".repeat(budget.saturating_sub(used)), Tone::Normal));
    out.extend(right);
    out
}
pub(super) fn clock(ms: u64) -> String {
    let seconds = ms / 1000;
    format!("{:02}:{:02}", seconds / 60, seconds % 60)
}
/// `@@ -old,n +new,n @@` -- the first line number on each side.
fn hunk(header: &str) -> (usize, usize) {
    let mut sides = header.split_whitespace().filter_map(|part| {
        let digits = part.trim_start_matches(['-', '+']);
        digits
            .split(',')
            .next()
            .and_then(|n| n.parse::<usize>().ok())
    });
    let old = sides.next().unwrap_or(1);
    let new = sides.next().unwrap_or(old);
    (old.saturating_sub(1), new.saturating_sub(1))
}
pub(super) fn count_changes(diff: &str) -> (usize, usize) {
    diff.lines().fold((0, 0), |(a, r), line| {
        if line.starts_with("+++") || line.starts_with("---") {
            (a, r)
        } else if line.starts_with('+') {
            (a + 1, r)
        } else if line.starts_with('-') {
            (a, r + 1)
        } else {
            (a, r)
        }
    })
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
