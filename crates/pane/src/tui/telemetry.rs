//! Local instruments. Measurements and decorative motion have separate inputs.
use super::{ACCENT, MUTED, Notebook, ScreenState};
use crate::contract::{Conversation, Role, ServedBy};
use ratatui::{
    Frame,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
};

#[derive(Debug, Clone, Default)]
pub struct Pulse {
    pub elapsed_ms: u64,
    pub deltas: usize,
    pub bytes: usize,
    /// Last 32 actual transport deliveries, measured in UTF-8 bytes.
    pub deliveries: Vec<usize>,
}
impl Pulse {
    pub fn receive(&mut self, bytes: usize) {
        self.deltas += 1;
        self.bytes += bytes;
        self.deliveries.push(bytes);
        if self.deliveries.len() > 32 {
            self.deliveries.remove(0);
        }
    }
}
fn label(text: impl Into<String>) -> Line<'static> {
    Line::styled(
        text.into(),
        Style::default()
            .fg(Color::LightCyan)
            .add_modifier(Modifier::BOLD),
    )
}
fn muted(text: impl Into<String>) -> Line<'static> {
    Line::styled(text.into(), Style::default().fg(MUTED))
}
fn metric(value: Option<u64>) -> String {
    value
        .map(|v| v.to_string())
        .unwrap_or_else(|| "unreported".into())
}
fn graph(samples: &[usize], width: usize, height: usize) -> Vec<Line<'static>> {
    let glyphs = [' ', '▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];
    let tail = &samples[samples.len().saturating_sub(width)..];
    let peak = tail.iter().copied().max().unwrap_or(1).max(1);
    let padding = width.saturating_sub(tail.len());
    (0..height)
        .map(|row| {
            let mut spans = vec![Span::styled(
                if row + 1 == height { "·" } else { " " }.repeat(padding),
                Style::default().fg(Color::DarkGray),
            )];
            for (i, n) in tail.iter().enumerate() {
                let units = ((*n as u128 * (height * 8) as u128) / peak as u128) as usize;
                let fill = units.saturating_sub((height - row - 1) * 8).min(8);
                spans.push(Span::styled(
                    glyphs[fill].to_string(),
                    Style::default().fg(if i + 1 == tail.len() {
                        ACCENT
                    } else if row + 1 == height {
                        Color::Cyan
                    } else {
                        Color::LightCyan
                    }),
                ));
            }
            Line::from(spans)
        })
        .collect()
}
fn budget(notebook: &Notebook, width: usize) -> Vec<Line<'static>> {
    let Some(tokens) = notebook.tokens else {
        return vec![muted("Task budget · no usage yet")];
    };
    let filled = if tokens.cap == 0 {
        0
    } else {
        ((tokens.used as u128 * width as u128) / tokens.cap as u128).min(width as u128) as usize
    };
    vec![
        Line::from(vec![
            Span::styled(
                "━".repeat(filled),
                Style::default().fg(if filled * 5 >= width * 4 {
                    Color::LightYellow
                } else {
                    ACCENT
                }),
            ),
            Span::styled(
                "─".repeat(width - filled),
                Style::default().fg(Color::DarkGray),
            ),
        ]),
        muted(format!("budget: {}/{} tok", tokens.used, tokens.cap)),
        muted(format!("counted: {}", tokens.counted.as_str())),
    ]
}

pub(super) fn rail(
    frame: &mut Frame,
    area: Rect,
    served: &ServedBy,
    notebook: &Notebook,
    state: &ScreenState,
) {
    let border = Block::default()
        .borders(Borders::LEFT)
        .title(" telemetry ")
        .border_style(Style::default().fg(Color::DarkGray));
    let inner = border.inner(area);
    frame.render_widget(border, area);
    let width = usize::from(inner.width);
    let mut lines = vec![Line::styled(
        format!(
            " {} {} ",
            state.activity.indicator(if state.reduced_motion {
                0
            } else {
                state.animation_frame
            }),
            state.activity.label().to_uppercase()
        ),
        Style::default()
            .fg(ACCENT)
            .bg(state.theme.dock())
            .add_modifier(Modifier::BOLD),
    )];
    if state.pulse.elapsed_ms > 0 {
        lines.push(muted(format!(
            "{:.1}s · {} deliveries",
            state.pulse.elapsed_ms as f64 / 1000.,
            state.pulse.deltas
        )));
    }
    if inner.height >= 24 {
        lines.extend(graph(
            &state.pulse.deliveries,
            width.min(30),
            if inner.height >= 32 { 3 } else { 1 },
        ));
        lines.push(muted("text delivery sizes · bytes"));
        lines.push(Line::default());
    }
    lines.push(label("01 / REQUEST"));
    if let Some(request) = notebook.requests.last() {
        lines.push(muted(super::abbreviate(&request.model, width)));
        lines.push(muted(format!(
            "{} in / {} out",
            metric(request.input_tokens),
            metric(request.output_tokens)
        )));
        lines.push(muted(format!(
            "{:.2}s · {}",
            request.elapsed_ms as f64 / 1000.,
            match (request.input_tokens, request.output_tokens) {
                (Some(_), Some(_)) => "reported usage",
                (None, None) => "usage unreported",
                _ => "partial usage",
            }
        )));
    } else if served.is_known() {
        // This fallback is a project observation, not a correlated response.
        lines.pop();
        lines.push(label("01 / PROJECT ROUTING"));
        let available = inner.height.saturating_sub(10) as usize;
        lines.extend(
            super::known_sidebar_lines(served)
                .into_iter()
                .take(available),
        );
    } else {
        lines.push(muted("No request telemetry yet."));
    }
    lines.push(Line::default());
    lines.push(label("02 / TASK BUDGET"));
    lines.extend(budget(notebook, width.min(30)));
    if let Some(status) = &notebook.supervisor {
        lines.push(muted(super::supervisor_line(status)));
    }
    if inner.height >= 24
        && let Some(cell) = notebook.cells.iter().rfind(|cell| cell.execution.is_some())
    {
        lines.push(Line::default());
        lines.push(label("03 / LAST ACTION"));
        if let Some(execution) = &cell.execution {
            lines.extend(
                execution
                    .lines()
                    .take(4)
                    .map(|l| muted(super::abbreviate(l, width))),
            );
        }
    }
    lines.push(Line::default());
    lines.push(muted("Ctrl-T  open instruments"));
    frame.render_widget(Paragraph::new(super::wrap_lines(lines, inner.width)), inner);
}

fn selected_cell_source(
    conversation: &Conversation,
    notebook: &Notebook,
    wanted: usize,
) -> Option<String> {
    let mut cell = 0;
    let mut terminal = false;
    for message in conversation.messages.iter().skip(1) {
        if message.role == Role::User {
            terminal = false;
            continue;
        }
        if terminal {
            terminal = false;
            continue;
        }
        cell += 1;
        if cell == wanted {
            return Some(
                notebook
                    .cell(cell)
                    .and_then(|v| v.executed_source.clone())
                    .unwrap_or_else(|| super::input_region(message)),
            );
        }
        terminal = notebook.cell(cell).is_some_and(|v| v.returned.is_some());
    }
    None
}
fn execution(conversation: &Conversation, notebook: &Notebook, cell: usize) -> Vec<Line<'static>> {
    let mut lines = vec![
        label(format!("EXECUTION / RESPONSE {cell:02}")),
        Line::default(),
    ];
    let view = notebook.cell(cell);
    let possible = selected_cell_source(conversation, notebook, cell)
        .map(|s| super::possible_tool_calls(&s))
        .unwrap_or_default();
    if !possible.is_empty() {
        lines.push(muted("◇ proposed in generated code"));
        lines.extend(possible.iter().map(|name| {
            Line::styled(format!("  ┄┄ {name}"), Style::default().fg(Color::DarkGray))
        }));
    }
    lines.push(Line::default());
    if let Some(actual) = view.and_then(|v| v.execution.as_deref()) {
        lines.push(label("◆ observed"));
        for row in actual.lines() {
            let color = if row.contains(" · failed") || row.contains(" · denied") {
                Color::LightRed
            } else {
                Color::LightCyan
            };
            lines.push(Line::styled(row.to_owned(), Style::default().fg(color)));
        }
        let count = actual.lines().filter(|row| row.contains(" · ")).count();
        if count > 1 {
            lines.push(Line::default());
            lines.push(Line::styled(
                format!(" ◆ {count} tool calls · one response "),
                Style::default().fg(Color::Black).bg(ACCENT),
            ));
        }
        if !possible.is_empty() && possible.len() > count {
            lines.push(muted("Unobserved proposals may be skipped."));
        }
    } else {
        lines.push(muted("No recorded execution for this response."));
    }
    if let Some(error) = view.and_then(|v| v.error.as_ref()) {
        lines.push(Line::styled(
            format!("× {}: {}", error.class, error.message),
            Style::default().fg(Color::LightRed),
        ));
    }
    if let Some(changes) = view.and_then(|v| v.changes.as_deref()) {
        lines.push(Line::default());
        lines.push(label("FILES CHANGED"));
        for path in changes
            .lines()
            .filter_map(|line| line.strip_prefix("+++ "))
            .take(8)
        {
            lines.push(muted(path));
        }
    }
    lines
}

pub(super) fn expanded(
    frame: &mut Frame,
    area: Rect,
    conversation: &Conversation,
    served: &ServedBy,
    notebook: &Notebook,
    state: &ScreenState,
) {
    if area.width < 4 || area.height < 4 {
        return;
    }
    let heading = Line::from(vec![
        Span::styled(
            " TELEMETRY ",
            Style::default()
                .fg(Color::Black)
                .bg(ACCENT)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            "  LIVE INSTRUMENTS     ↑↓ request · Esc returns",
            Style::default().fg(MUTED),
        ),
    ]);
    frame.render_widget(
        Paragraph::new(heading),
        Rect::new(area.x, area.y, area.width, 1),
    );
    let body = Rect::new(
        area.x + 1,
        area.y + 2,
        area.width.saturating_sub(2),
        area.height.saturating_sub(2),
    );
    let wide = body.width >= 100;
    let left_width = if wide { 38 } else { body.width };
    let trace_height = if wide && body.height >= 32 {
        4
    } else if body.height >= 36 {
        3
    } else if wide && body.height >= 24 {
        2
    } else {
        1
    };
    let left = Rect::new(
        body.x,
        body.y,
        left_width,
        if wide {
            body.height
        } else {
            body.height.min(trace_height as u16 + 3)
        },
    );
    let right = if wide {
        Rect::new(
            body.x + 41,
            body.y,
            body.width.saturating_sub(41),
            body.height,
        )
    } else {
        Rect::new(
            body.x,
            left.bottom(),
            body.width,
            body.bottom().saturating_sub(left.bottom()),
        )
    };
    let index = state
        .telemetry_selected
        .unwrap_or(notebook.requests.len().saturating_sub(1))
        .min(notebook.requests.len().saturating_sub(1));
    let selected = notebook.requests.get(index);
    let mut instruments = vec![label(format!(
        "01 / {}",
        state.activity.label().to_uppercase()
    ))];
    let roomy = wide && body.width >= 150 && body.height >= 32;
    if wide && !roomy {
        instruments.extend(super::ribbon::lines(
            34,
            if body.height >= 32 { 8 } else { 4 },
            state,
        ));
    }
    instruments.push(Line::from(vec![
        Span::styled(
            format!("{:.1}s", state.pulse.elapsed_ms as f64 / 1000.),
            Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
        ),
        Span::raw(format!(
            "  {} bytes / {} deliveries",
            state.pulse.bytes, state.pulse.deltas
        )),
    ]));
    instruments.extend(graph(
        &state.pulse.deliveries,
        usize::from(left.width).min(34),
        trace_height,
    ));
    instruments.push(muted("Text delivery sizes · bytes"));
    instruments.push(Line::default());
    instruments.push(label("02 / TASK BUDGET"));
    instruments.extend(budget(notebook, usize::from(left.width).min(34)));
    if wide {
        instruments.push(Line::default());
        instruments.push(label("03 / REQUEST HISTORY"));
        let start = index.saturating_sub(5);
        for (i, request) in notebook.requests.iter().enumerate().skip(start).take(10) {
            instruments.push(Line::styled(
                format!(
                    "{} {:02}  {:>7}ms  {} out",
                    if i == index { "›" } else { " " },
                    request.cell,
                    request.elapsed_ms,
                    metric(request.output_tokens)
                ),
                Style::default()
                    .fg(if i == index { Color::Black } else { MUTED })
                    .bg(if i == index { ACCENT } else { Color::Reset }),
            ));
        }
        if notebook.requests.is_empty() {
            instruments.push(muted("No live response measurements yet."));
        }
    }
    frame.render_widget(
        Paragraph::new(super::wrap_lines(instruments, left.width)),
        left,
    );
    let mut detail = Vec::new();
    if let Some(request) = selected {
        detail.push(label(format!(
            "REQUEST {:02} / {}",
            request.cell, request.model
        )));
        detail.push(muted(format!(
            "{:.2}s response · input {} · output {}",
            request.elapsed_ms as f64 / 1000.,
            metric(request.input_tokens),
            metric(request.output_tokens)
        )));
        detail.push(muted(format!(
            "Cached input {} · cost unreported",
            metric(request.cached_input_tokens)
        )));
        detail.push(muted(format!(
            "Provider {} · route {}",
            request.served.provider.as_deref().unwrap_or("unreported"),
            request.served.route.as_deref().unwrap_or("unreported")
        )));
        detail.push(Line::default());
        detail.extend(execution(conversation, notebook, request.cell));
    } else {
        detail.push(label("EXECUTION HISTORY"));
        detail.push(muted(
            "Request timings are collected live, not reconstructed from prose.",
        ));
        if served.is_known() {
            detail.extend(super::known_sidebar_lines(served));
        }
        if !notebook.cells.is_empty() {
            detail.extend(execution(conversation, notebook, notebook.cells.len()));
        }
    }
    let detail = super::wrap_lines(detail, right.width);
    let used = (detail.len() as u16).min(right.height);
    frame.render_widget(Paragraph::new(detail), right);
    let remaining = right.height.saturating_sub(used + 2);
    if roomy && remaining >= 8 {
        let area = Rect::new(
            right.x,
            right.y + used + 2,
            right.width.min(160),
            remaining.min(14),
        );
        frame.render_widget(
            Paragraph::new(super::ribbon::lines(
                usize::from(area.width),
                usize::from(area.height),
                state,
            )),
            area,
        );
    }
}
