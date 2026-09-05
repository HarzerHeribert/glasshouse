//! The two-region screen: a conversation column and a telemetry sidebar.
//!
//! **`ServedBy::is_known()` is the only predicate that decides whether the
//! sidebar shows or collapses.** It holds because the type was frozen for
//! exactly this reason (`contract.rs`): every field is optional and absent is
//! not zero, so a second test (an empty string, a zero token count) would be
//! a second way of asking the same question and could drift from the first.
//!
//! Nothing here opens a terminal or reads a key: [`render`] takes a
//! `ratatui::Frame` a caller already owns, so every test in
//! `tests/tui.rs` drives it through a `TestBackend` buffer.

use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::text::Line;
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};

use crate::contract::{Conversation, ServedBy};

/// The sidebar's width when it has something to show.
const SIDEBAR_WIDTH: u16 = 34;

/// The sidebar's width when it has collapsed to the one not-connected
/// statement -- narrower than [`SIDEBAR_WIDTH`] so the conversation column
/// gets the difference back. Wide enough that [`NOT_CONNECTED`] fits on one
/// line rather than wrapping.
const COLLAPSED_SIDEBAR_WIDTH: u16 = 27;

/// What the collapsed sidebar says. The whole line, and the only line: no
/// zero, no dash, no field that looks measured.
const NOT_CONNECTED: &str = "Glasshouse not connected.";

/// Renders the conversation column and the telemetry sidebar into `frame`.
pub fn render(frame: &mut Frame, conversation: &Conversation, served_by: &ServedBy) {
    let area = frame.area();
    let sidebar_width = if served_by.is_known() {
        SIDEBAR_WIDTH
    } else {
        COLLAPSED_SIDEBAR_WIDTH
    };

    let columns = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Min(0), Constraint::Length(sidebar_width)])
        .split(area);

    render_conversation(frame, columns[0], conversation);
    render_sidebar(frame, columns[1], served_by);
}

fn render_conversation(frame: &mut Frame, area: Rect, conversation: &Conversation) {
    let lines: Vec<Line> = conversation
        .messages
        .iter()
        .map(|message| {
            let text = message
                .content
                .iter()
                .map(|block| block.text())
                .collect::<Vec<_>>()
                .join("");
            Line::from(format!("{}: {text}", message.role.as_str()))
        })
        .collect();

    let paragraph = Paragraph::new(lines)
        .block(Block::default().borders(Borders::ALL).title("Conversation"))
        .wrap(Wrap { trim: false });
    frame.render_widget(paragraph, area);
}

fn render_sidebar(frame: &mut Frame, area: Rect, served_by: &ServedBy) {
    let lines: Vec<Line> = if served_by.is_known() {
        known_sidebar_lines(served_by)
    } else {
        vec![Line::from(NOT_CONNECTED)]
    };

    let paragraph = Paragraph::new(lines)
        .block(Block::default().borders(Borders::ALL))
        .wrap(Wrap { trim: true });
    frame.render_widget(paragraph, area);
}

/// Only the fields Glasshouse actually reported become a line. A field it
/// never sent is omitted rather than shown as `0` or a placeholder --
/// [`ServedBy`]'s own rule, applied per field rather than only at the top.
fn known_sidebar_lines(served_by: &ServedBy) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    if let Some(quota_context) = &served_by.quota_context {
        lines.push(Line::from(format!("entitlement: {quota_context}")));
    }
    if let Some(provider) = &served_by.provider {
        lines.push(Line::from(format!("provider: {provider}")));
    }
    if let Some(model) = &served_by.model {
        lines.push(Line::from(format!("model: {model}")));
    }
    match (served_by.input_tokens, served_by.output_tokens) {
        (Some(input), Some(output)) => {
            lines.push(Line::from(format!("tokens: {input} in / {output} out")));
        }
        (Some(input), None) => lines.push(Line::from(format!("tokens: {input} in"))),
        (None, Some(output)) => lines.push(Line::from(format!("tokens: {output} out"))),
        (None, None) => {}
    }
    lines
}
