//! Converts observed conversation/cell records to a readable, local document.
use super::{Action, CellTab, Workbench, theme};
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
        self.emit(
            Row {
                text,
                tone,
                spans: Vec::new(),
                tabs: Vec::new(),
                action,
                key: (0, 0),
            },
            id,
        );
    }
    fn emit(&mut self, mut row: Row, id: usize) {
        let ordinal = self
            .rows
            .last()
            .filter(|r| r.key.0 == id)
            .map_or(0, |r| r.key.1 + 1);
        row.key = (id, ordinal);
        self.rows.push(row);
    }
    /// One pre-sized line of coloured segments; never wrapped, because its
    /// padding was computed against the width it was built for.
    fn line(&mut self, spans: Vec<(String, Tone)>, action: Option<Action>, id: usize) {
        let text = spans.iter().map(|(t, _)| t.as_str()).collect::<String>();
        self.emit(
            Row {
                text,
                tone: spans.first().map_or(Tone::Normal, |(_, t)| *t),
                spans,
                tabs: Vec::new(),
                action,
                key: (0, 0),
            },
            id,
        );
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
        let mut cell: usize = 0;
        let mut after_return = false;
        let mut feedback = false;
        let mut returned_text: Option<String> = None;
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
                d.push(format!("❯ {}", prose(m)), Tone::Strong, None, width, id);
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
            let clock = if running {
                clock(s.pulse.elapsed_ms)
            } else {
                String::new()
            };
            d.line(
                justify(
                    vec![
                        (format!(" {} ", if open { "▾" } else { "▸" }), tone),
                        (format!("{cell:03}"), Tone::Strong),
                        (format!("  {description}"), tone),
                    ],
                    vec![
                        (state.to_string(), tone),
                        (format!("  {clock} "), Tone::Muted),
                    ],
                    width,
                ),
                Some(Action::Cell(cell)),
                id,
            );
            if open {
                d.rule('━', width, id);
                let tab = ui.tabs.get(&cell).copied().unwrap_or(CellTab::Code);
                d.tabstrip(cell, tab, v.and_then(|v| v.changes.as_deref()), width, id);
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
                    CellTab::Code => d.wrapped(program, Tone::Code, None, width, id, 2),
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
                                    d.wrapped(name, Tone::Accent, None, width, id, 2);
                                    d.wrapped(value, tone, None, width, id, 4);
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
                            d.result(output, width, id);
                        }
                        if let Some(execution) = &v.execution {
                            for line in execution
                                .lines()
                                .filter(|l| l.contains(" · failed") || l.contains(" · denied"))
                            {
                                d.line(
                                    vec![
                                        ("  ✕ ".to_string(), Tone::Failure),
                                        (clip(line, width.saturating_sub(6)), Tone::Failure),
                                    ],
                                    None,
                                    id,
                                );
                            }
                        }
                    }
                }
                if running {
                    d.work(s, v, width, id);
                }
                d.rule('─', width, id);
            }
            if let Some(v) = v {
                if open {
                    d.summary(v, width, id);
                }
                d.helpers(cell, v, ui, width, id);
                if let Some(answer) = v.returned.as_ref() {
                    let answer =
                        crate::prompt::completion_text(answer).unwrap_or_else(|| answer.clone());
                    d.push("", Tone::Normal, None, width, id);
                    // The answer's opening line is the result a person came
                    // back for; the rest is its body, in ordinary prose.
                    let mut lines = answer.splitn(2, '\n');
                    if let Some(first) = lines.next() {
                        d.push(first, Tone::Strong, None, width, id);
                    }
                    if let Some(rest) = lines.next().filter(|r| !r.trim().is_empty()) {
                        d.push(rest, Tone::Normal, None, width, id);
                    }
                    returned_text = Some(answer.trim().to_string());
                }
            }
            d.push("", Tone::Normal, None, width, id);
        }
        d.notes(s, &mut note, usize::MAX, width);
        if let Some(p) = &n.preflight {
            d.line(
                vec![
                    (" ◇ PREFLIGHT · SCOUT  ".to_string(), Tone::Helper),
                    (
                        clip(&format!("{} {}", p.verb, p.asked), width.saturating_sub(24)),
                        Tone::Muted,
                    ),
                ],
                None,
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
        if d.rows.len() == card_rows {
            // One line, then the things to press. A paragraph on an empty
            // screen is read once and never again; a list of four commands
            // is read every time someone does not know what to type.
            d.wrapped(
                "Describe a task, or try one of these:",
                Tone::Normal,
                None,
                width,
                0,
                2,
            );
            d.push("", Tone::Normal, None, width, 0);
            for (key, what) in [
                (
                    "/settings",
                    "everything about this session, applied as you choose it",
                ),
                ("/models", "which model answers, and at what effort"),
                ("/diff", "what the last cell actually changed"),
                ("/help", "every command"),
            ] {
                d.line(
                    vec![
                        (format!("  {key:<11}"), Tone::Accent),
                        (what.to_string(), Tone::Muted),
                    ],
                    Some(Action::Insert(key.to_string())),
                    0,
                );
            }
        }
        d
    }
    /// The cell's own result, marked pass or fail from what it actually
    /// said. Nothing here judges the work -- the mark reports the words the
    /// run produced, and the words stay on the line beside it.
    fn result(&mut self, output: &str, width: usize, id: usize) {
        for line in output.lines().take(4) {
            let lower = line.to_lowercase();
            let bad = ["fail", "error", "panic", "✕"]
                .iter()
                .any(|w| lower.contains(w));
            self.line(
                vec![
                    (
                        format!("  {} ", if bad { "✕" } else { "✓" }),
                        if bad { Tone::Failure } else { Tone::Success },
                    ),
                    (clip(line, width.saturating_sub(6)), Tone::Normal),
                ],
                None,
                id,
            );
        }
    }
    /// What a session says about itself before anyone has said anything to
    /// it: the mark, and the two facts that are not already on the status
    /// line, each on exactly one line.
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
        let mark = theme::padded_mark(0, true);
        // **The affordance beside the fact.** A card that states what the
        // session is and says nothing about how to change it makes a reader
        // go looking; naming the control on the same line is the cheapest
        // teaching this screen can do, and it is what the neighbouring
        // product does on its own opening card.
        let model = match s.model.as_deref() {
            Some(model) => format!("{model}   /model changes it"),
            None => "no model chosen   /models picks one".to_string(),
        };
        let second = startup
            .first()
            .copied()
            .map(str::to_string)
            .unwrap_or(model);
        let facts = [
            Some(("P A N E".to_string(), Tone::Accent)),
            Some((
                format!(
                    "{}  ·  code · cells · little helpers",
                    s.project.as_deref().unwrap_or("no project")
                ),
                Tone::Muted,
            )),
            Some((clip(&second, width.saturating_sub(14)), Tone::Muted)),
        ];
        for (glyph, fact) in mark.iter().zip(facts) {
            let (text, tone) = fact.unwrap_or((String::new(), Tone::Muted));
            self.line(
                vec![(format!(" {glyph}  "), Tone::Accent), (text, tone)],
                None,
                0,
            );
        }
        if startup.len() > 1 {
            self.line(
                vec![(
                    format!("         +{} more · /activity", startup.len() - 1),
                    Tone::Line,
                )],
                Some(Action::Activity),
                0,
            );
        }
        self.rule('─', width, 0);
        self.push("", Tone::Normal, None, width, 0);
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
                self.line(
                    vec![
                        (
                            format!(" {} ", if bad { "✕" } else { "·" }),
                            if bad { Tone::Failure } else { Tone::Line },
                        ),
                        (
                            clip(line, width.saturating_sub(4)),
                            if bad { Tone::Failure } else { Tone::Muted },
                        ),
                    ],
                    None,
                    usize::MAX - 4,
                );
            }
        }
    }
    /// A full-width separator in the one colour reserved for separators.
    fn rule(&mut self, glyph: char, width: usize, id: usize) {
        self.line(
            vec![(
                glyph.to_string().repeat(width.saturating_sub(1)),
                Tone::Line,
            )],
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
            .map(|(label, _)| format!("  {label}  "))
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
                action: Some(Action::Tab(cell, current)),
                key: (0, 0),
            },
            id,
        );
    }
    /// The mockup's working mark: decoration on the left, and beside it what
    /// is actually happening, what was last observed, and at whose cost.
    fn work(&mut self, s: &ScreenState, v: Option<&CellView>, width: usize, id: usize) {
        let still = s.reduced_motion || s.selection.is_some();
        let helper = v.and_then(|v| v.helpers.last());
        let waiting = helper.is_some_and(|h| !h.outcome.ok && h.outcome.text.is_empty());
        let label = match s.activity {
            Activity::Executing => "Executing this cell",
            Activity::Waiting => "Waiting on the provider",
            Activity::Searching => "Searching",
            Activity::Compacting => "Preparing bounded context",
            _ if waiting => "Little helper working",
            _ => "Model is responding",
        };
        let detail = if waiting {
            "Request sent · completion estimate unknown".to_string()
        } else {
            format!(
                "Elapsed {} · nothing is assumed complete",
                clock(s.pulse.elapsed_ms)
            )
        };
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
        let art = theme::padded_mark(s.animation_frame, still);
        for (glyph, (text, tone)) in art.into_iter().zip([
            (format!("◈ {label}"), Tone::Accent),
            (detail, Tone::Normal),
            (cost, Tone::Muted),
        ]) {
            let text = clip(&text, width.saturating_sub(12));
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
    /// because a captured diff is evidence and must never be dropped for
    /// failing to parse.
    fn diff(&mut self, value: Option<&str>, width: usize, id: usize) {
        self.line(
            vec![(
                "  Observed changes · before → after this cell · already applied".to_string(),
                Tone::Muted,
            )],
            None,
            id,
        );
        let Some(diff) = value.filter(|v| !v.is_empty()) else {
            self.line(
                vec![(
                    "  No textual diff captured. This does not prove no files changed.".to_string(),
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
                    vec![(format!("  {}", path.trim_start_matches("b/")), Tone::Accent)],
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
                self.line(vec![(format!("  {line}"), Tone::Line)], None, id);
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
                    (format!("  {old_cell:>5} {new_cell:>5} {mark} "), Tone::Line),
                    (clip(&body, width.saturating_sub(16)), tone),
                ],
                None,
                id,
            );
        }
    }
    /// One line under an open cell: what its helpers did, what its own result
    /// said, and how many files it changed. Every claim here is observed.
    fn summary(&mut self, v: &CellView, width: usize, id: usize) {
        let mut left = vec![(" ".to_string(), Tone::Normal)];
        if let Some(e) = &v.error {
            left.push((format!("✕ {}   ", e.class), Tone::Failure));
        } else if v.execution.is_some() {
            left.push(("✓ executed   ".to_string(), Tone::Success));
        }
        let files = v
            .changes
            .as_deref()
            .map_or(0, |d| d.lines().filter(|l| l.starts_with("+++ ")).count());
        let right = match files {
            0 => vec![("no captured file changes ".to_string(), Tone::Muted)],
            1 => vec![("1 changed file ↗ ".to_string(), Tone::Muted)],
            n => vec![(format!("{n} changed files ↗ "), Tone::Muted)],
        };
        self.line(justify(left, right, width), None, id);
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
            self.line(
                vec![
                    (
                        format!(" {} ◇ {} ", if open { "▾" } else { "▸" }, h.helper),
                        if tone == Tone::Normal {
                            Tone::Helper
                        } else {
                            tone
                        },
                    ),
                    (clip(&result, width.saturating_sub(16)), Tone::Muted),
                ],
                Some(Action::Helper(cell, i)),
                id,
            );
            if open {
                self.push(
                    format!("    Asked: {}", h.asked),
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
const OPEN_DIFF: &str = "Open diff ↗";

fn span_width(text: &str) -> usize {
    ratatui::text::Span::raw(text).width()
}
/// Cut to a column budget on a character boundary; never mid-escape, because
/// control characters never reach a row in the first place.
pub(super) fn clip(text: &str, width: usize) -> String {
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
fn justify(
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
fn clock(ms: u64) -> String {
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
fn count_changes(diff: &str) -> (usize, usize) {
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
