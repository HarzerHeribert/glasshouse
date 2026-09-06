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
use crate::prompt::{Extracted, extract_program};
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

/// What one cell produced, beside the assistant message the notebook already
/// draws.
///
/// **Every field arrives already rendered or already plain.** The runtime
/// hands its caller a rendered handle table and a rendered preview and never
/// its table or its value, so this module turns no live object into text --
/// which is the invariant `tests/tui.rs::the_tui_renders_no_handle_itself`
/// scans this file for.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CellView {
    /// The handle table as this cell ended, already rendered by the one
    /// renderer. `None` for a cell the notebook never saw run -- a resumed
    /// session's earlier cells came from the rollout file.
    pub table: Option<String>,
    /// A throw, `runtime-contract.md` §5.
    pub error: Option<CellError>,
    /// A top-level `return`'s value, already previewed by the caller through
    /// the runtime's one preview renderer (§1).
    pub returned: Option<String>,
    /// Whether the user message that follows this cell is the runtime's own
    /// answer to it rather than a person typing. Every section of that answer
    /// is already on screen as this cell's output, error and return regions,
    /// so drawing it again would put the handle table on the screen twice.
    pub answered: bool,
}

/// A throw's class, message and position inside the model's own program --
/// `runtime-contract.md` §5's first two items, and nothing from inside the
/// runtime. The position is optional because the runtime could not always
/// attribute a throw to a line of the model's program.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CellError {
    pub class: String,
    pub message: String,
    pub line: Option<u32>,
    pub column: Option<u32>,
}

/// Where the task's token total came from.
///
/// **It is a field rather than a footnote because the two figures are not
/// comparable.** `model-contract.md` §6 reads the gateway's own usage row
/// when there is one and estimates otherwise; a total that silently mixed
/// them would be the one number on this screen a reader would trust without
/// knowing what it was.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Counted {
    /// Every turn so far carried a provider-reported usage row.
    Gateway,
    /// No turn did; every figure in the total is `estimate_tokens`'.
    Estimated,
    /// Some turns reported and some did not.
    Mixed,
}

impl Counted {
    /// Short enough to fit the collapsed sidebar's 25 usable columns on one
    /// line: a provenance that wraps is a provenance half a reader skips.
    fn as_str(self) -> &'static str {
        match self {
            Counted::Gateway => "reported",
            Counted::Estimated => "estimated",
            Counted::Mixed => "part estimated",
        }
    }
}

/// The task budget's two figures and their provenance.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TaskTokens {
    pub used: u64,
    pub cap: u64,
    pub counted: Counted,
}

/// What the session knows about the conversation beyond the messages
/// themselves: one view per assistant cell, in cell order, and the task's
/// token total.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Notebook {
    pub cells: Vec<CellView>,
    pub tokens: Option<TaskTokens>,
}

impl Notebook {
    /// Records `view` as cell `ordinal`'s (1-based), padding with empty views
    /// for any earlier cell this notebook never saw -- a resumed session's
    /// cells came back from the rollout file, and padding is what keeps a new
    /// cell's view under the cell the screen numbers it.
    pub fn set(&mut self, ordinal: usize, view: CellView) {
        if ordinal == 0 {
            return;
        }
        if self.cells.len() < ordinal {
            self.cells.resize(ordinal, CellView::default());
        }
        self.cells[ordinal - 1] = view;
    }

    fn cell(&self, ordinal: usize) -> Option<&CellView> {
        self.cells.get(ordinal.checked_sub(1)?)
    }
}

/// Renders the conversation column and the telemetry sidebar into `frame`.
pub fn render(
    frame: &mut Frame,
    conversation: &Conversation,
    served_by: &ServedBy,
    handles: &HandleTable,
    notebook: &Notebook,
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

    render_conversation(frame, columns[0], conversation, handles, notebook);
    render_sidebar(frame, columns[1], served_by, notebook);
}

/// How many rows the notebook column needs before any wrapping.
///
/// A caller drawing into an off-screen buffer sizes it by this: a fixed
/// height clips the newest cell away exactly when a task has run long enough
/// to be worth reading, and the pipe that gets the clipped frame has no
/// scrollback to recover it from.
pub fn notebook_height(
    conversation: &Conversation,
    handles: &HandleTable,
    notebook: &Notebook,
) -> usize {
    notebook_lines(conversation, handles, notebook).len()
}

/// One line with nothing to show. Never collapses to no line at all -- the
/// same rule the sidebar keeps for an unmetered request.
const NO_OUTPUTS: &str = "(no outputs)";

fn render_conversation(
    frame: &mut Frame,
    area: Rect,
    conversation: &Conversation,
    handles: &HandleTable,
    notebook: &Notebook,
) {
    let lines = notebook_lines(conversation, handles, notebook);
    let paragraph = Paragraph::new(lines)
        .block(Block::default().borders(Borders::ALL).title("Conversation"))
        .wrap(Wrap { trim: false });
    frame.render_widget(paragraph, area);
}

/// A cell's regions, in the order `runtime-contract.md` §1 and §5 put them:
/// an input region carrying the **program** the message contained (its prose
/// when it contained none), an output region carrying the handle table as
/// that cell ended, then an error region for a throw and a return region for
/// a top-level `return`. The first user message is the task, drawn once as a
/// header rather than a cell; a later user message is a person typing, drawn
/// as `you: <text>` between cells -- unless the cell before it says the
/// runtime answered it, in which case that message *is* the answer whose
/// sections are already drawn above.
///
/// **A cell with no view of its own falls back to the pre-runtime rendering**
/// (the latest cell shows `handles`, an earlier one says `(no outputs)`), so
/// a caller that holds the live table itself -- every test in `tests/tui.rs`
/// -- still gets it drawn through the one renderer.
fn notebook_lines(
    conversation: &Conversation,
    handles: &HandleTable,
    notebook: &Notebook,
) -> Vec<Line<'static>> {
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
    let mut answered = false;
    for message in messages {
        match message.role {
            Role::Assistant => {
                cell += 1;
                let view = notebook.cell(cell);
                answered = view.is_some_and(|view| view.answered);

                lines.push(Line::from(format!("[{cell}] in")));
                push_text_region(&mut lines, &input_region(message));

                lines.push(Line::from(format!("[{cell}] out")));
                match view.and_then(|view| view.table.as_deref()) {
                    Some(table) => push_output_region(&mut lines, table.to_string()),
                    None if cell == total_cells => push_output_region(
                        &mut lines,
                        render_table(handles, PREVIEW_TOKEN_CAP, TABLE_TOKEN_CAP),
                    ),
                    None => lines.push(Line::from(NO_OUTPUTS)),
                }

                if let Some(error) = view.and_then(|view| view.error.as_ref()) {
                    lines.push(Line::from(format!("[{cell}] error")));
                    push_error_region(&mut lines, error);
                }
                if let Some(returned) = view.and_then(|view| view.returned.as_deref()) {
                    lines.push(Line::from(format!("[{cell}] return")));
                    push_text_region(&mut lines, returned);
                }
            }
            Role::User => {
                if answered {
                    answered = false;
                    continue;
                }
                lines.push(Line::from(format!("you: {}", message_text(message))));
            }
        }
    }

    lines
}

/// A cell's input region: the program the message carried, or its prose when
/// it carried none. `model-contract.md` §5's parser is the one that decides
/// which -- the notebook shows what actually ran, not the explanation around
/// it, and a message with two blocks (where neither ran) shows its whole text
/// rather than picking one of them.
fn input_region(message: &Message) -> String {
    let text = message_text(message);
    match extract_program(&text) {
        Extracted::Program(source) => source,
        Extracted::Prose | Extracted::TwoBlocks => text,
    }
}

/// A throw's own region: the class and message, then the position when the
/// runtime attributed one. An unattributed throw gets no position line rather
/// than a zero -- the sidebar's rule about absent figures, applied here.
fn push_error_region(lines: &mut Vec<Line<'static>>, error: &CellError) {
    lines.push(Line::from(format!("{}: {}", error.class, error.message)));
    if let (Some(line), Some(column)) = (error.line, error.column) {
        lines.push(Line::from(format!("line {line}, column {column}")));
    }
}

/// Pushes `text` one line per line, so a program or a multi-line preview is
/// as many rows as it has lines rather than one row carrying its newlines.
/// Empty text still pushes one empty row, so a region never disappears.
fn push_text_region(lines: &mut Vec<Line<'static>>, text: &str) {
    if text.is_empty() {
        lines.push(Line::from(String::new()));
        return;
    }
    for line in text.lines() {
        lines.push(Line::from(line.to_string()));
    }
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

fn render_sidebar(frame: &mut Frame, area: Rect, served_by: &ServedBy, notebook: &Notebook) {
    let mut lines: Vec<Line> = if served_by.is_known() {
        known_sidebar_lines(served_by)
    } else {
        vec![Line::from(NOT_CONNECTED)]
    };
    if let Some(tokens) = notebook.tokens {
        lines.push(Line::from(format!(
            "budget: {}/{} tok",
            tokens.used, tokens.cap
        )));
        lines.push(Line::from(format!("counted: {}", tokens.counted.as_str())));
    }

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
