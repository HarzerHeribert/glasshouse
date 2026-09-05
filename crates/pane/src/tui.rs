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

use crate::contract::{Conversation, Message, Role, ServedBy};
use crate::runtime::handles::{HandleTable, render_table};
use crate::runtime::preview::PREVIEW_TOKEN_CAP;
use crate::runtime::preview::TABLE_TOKEN_CAP;

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
pub fn render(
    frame: &mut Frame,
    conversation: &Conversation,
    served_by: &ServedBy,
    handles: &HandleTable,
) {
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

    render_conversation(frame, columns[0], conversation, handles);
    render_sidebar(frame, columns[1], served_by);
}

/// One line with nothing to show. Never collapses to no line at all -- the
/// same rule the sidebar keeps for an unmetered request.
const NO_OUTPUTS: &str = "(no outputs)";

fn render_conversation(
    frame: &mut Frame,
    area: Rect,
    conversation: &Conversation,
    handles: &HandleTable,
) {
    let lines = notebook_lines(conversation, handles);
    let paragraph = Paragraph::new(lines)
        .block(Block::default().borders(Borders::ALL).title("Conversation"))
        .wrap(Wrap { trim: false });
    frame.render_widget(paragraph, area);
}

/// A cell has exactly two regions today: an input region carrying the
/// assistant message's text and an output region carrying `(no outputs)` or,
/// for the latest cell, the live handle table -- `runtime-contract.md` §1 and
/// §3. The first user message is the task, drawn once as a header rather
/// than a cell; a later user message is the person typing mid-task, drawn as
/// `you: <text>` between cells. An error region (a throw, §5) and a return
/// region (a terminal value, §1) are not rendered yet: there is no isolate to
/// produce either, and both arrive with `GH-PANE-61E-ISOLATE`.
fn notebook_lines(conversation: &Conversation, handles: &HandleTable) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    let mut messages = conversation.messages.iter();

    if let Some(task) = messages.next() {
        lines.push(Line::from(message_text(task)));
    }

    let total_cells = conversation
        .messages
        .iter()
        .skip(1)
        .filter(|message| message.role == Role::Assistant)
        .count();

    let mut cell = 0usize;
    for message in messages {
        match message.role {
            Role::Assistant => {
                cell += 1;
                lines.push(Line::from(format!("[{cell}] in")));
                lines.push(Line::from(message_text(message)));
                lines.push(Line::from(format!("[{cell}] out")));
                if cell == total_cells {
                    push_output_region(
                        &mut lines,
                        render_table(handles, PREVIEW_TOKEN_CAP, TABLE_TOKEN_CAP),
                    );
                } else {
                    lines.push(Line::from(NO_OUTPUTS));
                }
            }
            Role::User => {
                lines.push(Line::from(format!("you: {}", message_text(message))));
            }
        }
    }

    lines
}

/// Draws `table` -- `render_table`'s own return value, line for line -- or
/// `NO_OUTPUTS` when it is empty. The only place a handle's rendering enters
/// the conversation column; nothing else here previews a value on its own.
fn push_output_region(lines: &mut Vec<Line<'static>>, table: String) {
    if table.is_empty() {
        lines.push(Line::from(NO_OUTPUTS));
        return;
    }
    for line in table.lines() {
        lines.push(Line::from(line.to_string()));
    }
}

fn message_text(message: &Message) -> String {
    message
        .content
        .iter()
        .map(|block| block.text())
        .collect::<Vec<_>>()
        .join("")
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
