//! Fullscreen presentation. The caller owns terminal lifecycle, input and ticks.

mod controls;
mod markdown;
mod telemetry;
pub use controls::{Mode, Panel, PanelRow, StatusLine};
pub use telemetry::Pulse;

use crate::commands::{BUILT_INS, BuiltIn};
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};

use crate::contract::{Conversation, Message, Role, ServedBy};
use crate::prompt::{Extracted, extract_program};
use crate::runtime::handles::{HandleTable, render_table};
use crate::runtime::preview::PREVIEW_TOKEN_CAP;
use crate::runtime::preview::TABLE_TOKEN_CAP;

const ACCENT: Color = Color::LightGreen;
const MUTED: Color = Color::Gray;
const NOT_CONNECTED: &str = "Glasshouse not connected.";

/// Session-owned presentation state. Missing instrumentation stays unknown.
/// Pass this to `render_screen` on input, resize, runtime events and activity ticks.
#[derive(Debug, Clone, Default)]
pub struct ScreenState {
    pub model: Option<String>,
    pub project: Option<String>,
    pub sandbox: Option<String>,
    pub network: Option<String>,
    pub connected: Option<bool>,
    pub input: String,
    /// UTF-8 byte offset supplied by the live editor; None hides the cursor.
    pub cursor: Option<usize>,
    pub completion_selected: usize,
    pub notice: Option<String>,
    /// Fold long code and previews locally; never changes the model messages.
    pub compact: bool,
    pub pretty: bool,
    pub activity: Activity,
    /// Partial provider text for the active response; never persisted as a completed turn.
    /// The streaming caller replaces this accumulated text and clears it on completion.
    pub streaming_text: Option<String>,
    pub animation_frame: usize,
    pub completion_tick: Option<usize>,
    /// Rows back from the transcript's end; zero follows the current turn.
    pub scrollback: usize,
    /// User preference, retained across resizes. The caller toggles this field.
    pub sidebar: SidebarVisibility,
    pub theme: Theme,
    pub mode: Mode,
    pub effort: crate::wire::Effort,
    pub status_line: StatusLine,
    pub panel: Option<Panel>,
    pub telemetry_open: bool,
    pub telemetry_selected: Option<usize>,
    pub reduced_motion: bool,
    pub pulse: Pulse,
}

/// Accent-only themes inherit the terminal background and its transparency.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Theme {
    #[default]
    Neon,
    Amber,
    Ice,
    Mono,
    Violet,
    Cobalt,
    Mint,
    Rose,
}
impl Theme {
    pub const ALL: [Self; 8] = [
        Self::Neon,
        Self::Amber,
        Self::Ice,
        Self::Mono,
        Self::Violet,
        Self::Cobalt,
        Self::Mint,
        Self::Rose,
    ];
    pub fn parse(name: &str) -> Option<Self> {
        match name {
            "neon" => Some(Self::Neon),
            "amber" => Some(Self::Amber),
            "ice" => Some(Self::Ice),
            "mono" => Some(Self::Mono),
            "violet" => Some(Self::Violet),
            "cobalt" => Some(Self::Cobalt),
            "mint" => Some(Self::Mint),
            "rose" => Some(Self::Rose),
            _ => None,
        }
    }
    pub fn name(self) -> &'static str {
        match self {
            Self::Neon => "neon",
            Self::Amber => "amber",
            Self::Ice => "ice",
            Self::Mono => "mono",
            Self::Violet => "violet",
            Self::Cobalt => "cobalt",
            Self::Mint => "mint",
            Self::Rose => "rose",
        }
    }
    fn backlight(self) -> Color {
        Color::Reset
    }
    fn dock(self) -> Color {
        match self {
            Self::Neon => Color::Rgb(20, 32, 26),
            Self::Amber => Color::Rgb(38, 29, 19),
            Self::Ice => Color::Rgb(17, 30, 39),
            Self::Mono => Color::Rgb(27, 29, 30),
            Self::Violet => Color::Rgb(30, 23, 43),
            Self::Cobalt => Color::Rgb(18, 26, 44),
            Self::Mint => Color::Rgb(16, 34, 30),
            Self::Rose => Color::Rgb(38, 22, 34),
        }
    }
    fn accent(self) -> Color {
        match self {
            Self::Neon => Color::Rgb(223, 255, 0),
            Self::Amber => Color::LightYellow,
            Self::Ice => Color::LightCyan,
            Self::Mono => Color::White,
            Self::Violet => Color::Rgb(191, 154, 255),
            Self::Cobalt => Color::Rgb(114, 155, 255),
            Self::Mint => Color::Rgb(100, 231, 187),
            Self::Rose => Color::Rgb(242, 156, 218),
        }
    }
}

/// Auto needs a comfortable reading column; Shown can use a tighter one.
/// Below 80 columns even an explicit request collapses to preserve the editor.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum SidebarVisibility {
    #[default]
    Auto,
    Hidden,
    Shown,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Activity {
    #[default]
    Idle,
    Starting,
    Thinking,
    Streaming,
    Executing,
    Searching,
    Waiting,
    Compacting,
    Complete,
    Failed,
}

impl Activity {
    /// Fixed four-cell machinery; only the current header moves.
    pub fn indicator(self, tick: usize) -> &'static str {
        let frames = match self {
            Self::Starting => [".  .", "+--.", "+--+", "|/|/"],
            Self::Thinking => [" .  ", " <> ", "<..>", " <> "],
            Self::Streaming => [">...", ".>..", "..>.", "...>"],
            Self::Executing => ["|>..", "|=>.", "|==>", "|..>"],
            Self::Searching => ["/.. ", "./. ", "../ ", "./. "],
            Self::Waiting => ["(  )", "( .)", "(..)", "(. )"],
            Self::Compacting => [">  <", " >< ", " [] ", " >< "],
            Self::Idle => [" -- "; 4],
            Self::Complete => [" OK "; 4],
            Self::Failed => [" !! "; 4],
        };
        frames[tick % frames.len()]
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Idle => "ready",
            Self::Starting => "assembling",
            Self::Thinking => "thinking",
            Self::Streaming => "receiving",
            Self::Executing => "executing",
            Self::Searching => "searching",
            Self::Waiting => "waiting",
            Self::Compacting => "compacting",
            Self::Complete => "complete",
            Self::Failed => "failed",
        }
    }
}

/// Disjoint hard bounds shared by the renderer and its structural tests.
#[derive(Debug, Clone, Copy)]
pub struct ScreenRegions {
    pub header: Rect,
    pub transcript: Rect,
    pub details: Rect,
    pub completions: Rect,
    pub notice: Rect,
    pub input: Rect,
    pub status: Rect,
}

pub fn screen_regions(area: Rect, state: &ScreenState) -> ScreenRegions {
    let status_h = area.height.min(match state.status_line {
        StatusLine::Full => {
            if area.width < 140 {
                3
            } else {
                2
            }
        }
        StatusLine::Compact => 1,
        StatusLine::Hidden => 0,
    });
    let status = Rect::new(area.x, area.bottom() - status_h, area.width, status_h);
    let remaining = status.y - area.y;
    let input_h = (wrapped_input(state, area.width)
        .len()
        .min(usize::from((area.height / 3).max(1))) as u16)
        .saturating_add(2)
        .max(3)
        .min(remaining);
    let input = Rect::new(area.x, status.y - input_h, area.width, input_h);
    let completion_h = (slash_matches(&state.input).len() as u16)
        .min(7)
        .min((input.y - area.y).saturating_sub(5));
    let completions = Rect::new(area.x, input.y - completion_h, area.width, completion_h);
    let notice_h = if state.notice.is_some() {
        3.min((completions.y - area.y).saturating_sub(4))
    } else {
        0
    };
    let notice = Rect::new(area.x, completions.y - notice_h, area.width, notice_h);
    let header_h = (notice.y - area.y).min(2);
    let header = Rect::new(area.x, area.y, area.width, header_h);
    let body_h = notice.y - header.bottom();
    let sidebar_visible = match state.sidebar {
        SidebarVisibility::Auto => area.width >= 120,
        SidebarVisibility::Hidden => false,
        SidebarVisibility::Shown => area.width >= 80,
    };
    let transcript_w = if sidebar_visible {
        area.width.saturating_sub(36)
    } else {
        area.width
    };
    let transcript = Rect::new(area.x, header.bottom(), transcript_w, body_h);
    let details = Rect::new(
        if sidebar_visible {
            area.right() - 34
        } else {
            area.right()
        },
        header.bottom(),
        if sidebar_visible { 34 } else { 0 },
        body_h,
    );
    ScreenRegions {
        header,
        transcript,
        details,
        completions,
        notice,
        input,
        status,
    }
}

/// Only real built-ins, with descriptions of their vocabulary rather than
/// claims that a command was successfully executed.
pub fn slash_matches(input: &str) -> Vec<(String, &'static str)> {
    let Some(prefix) = input.strip_prefix('/') else {
        return Vec::new();
    };
    if prefix.chars().any(char::is_whitespace) {
        return Vec::new();
    }
    BUILT_INS
        .iter()
        .filter(|command| command.name().starts_with(prefix))
        .map(|command| {
            (
                format!("/{}", command.name()),
                match command {
                    BuiltIn::Model => "select the active model",
                    BuiltIn::Entitlements => "inspect available entitlements",
                    BuiltIn::Handles => "inspect runtime handles",
                    BuiltIn::Supervisor => "inspect supervisor settings",
                    BuiltIn::Rollback => "roll back to a checkpoint",
                    BuiltIn::Budget => "inspect the task budget",
                    BuiltIn::Memory => "read or save project memory",
                },
            )
        })
        .chain(
            [
                ("/help".to_string(), "show available commands"),
                ("/exit".to_string(), "leave Pane"),
                ("/sidebar".to_string(), "auto, show or hide telemetry"),
                ("/theme".to_string(), "choose a palette · eight themes"),
                (
                    "/telemetry".to_string(),
                    "live activity, requests and execution · Ctrl-T",
                ),
                ("/motion".to_string(), "on or off · reduce animation"),
                ("/effort".to_string(), "configure response reasoning effort"),
                (
                    "/context".to_string(),
                    "inspect current context and token usage",
                ),
                ("/status".to_string(), "inspect session status"),
                ("/statusline".to_string(), "full, compact or hidden status"),
                (
                    "/permissions".to_string(),
                    "inspect or configure next-session grants",
                ),
                ("/mode".to_string(), "execute or plan without running code"),
                (
                    "/config".to_string(),
                    "inspect session limits and configuration",
                ),
            ]
            .into_iter()
            .filter(|(name, _)| name.trim_start_matches('/').starts_with(prefix)),
        )
        .collect()
}

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
    /// Corrected source for an executed pane-edit; display only, never another model message.
    pub executed_source: Option<String>,
    pub repaired_from: Option<u64>,
    /// Local before/after file diff. Never included in model context.
    pub changes: Option<String>,
    /// The handle table as this cell ended, already rendered by the one
    /// renderer. `None` for a cell the notebook never saw run -- a resumed
    /// session's earlier cells came from the rollout file.
    pub table: Option<String>,
    /// Already bounded by the runtime; display only, never an extra model message.
    pub stdout: Option<String>,
    /// Recorded tool outcomes, not calls inferred from generated source.
    pub execution: Option<String>,
    /// A throw, `runtime-contract.md` §5.
    pub error: Option<CellError>,
    /// A top-level `return`'s terminal response, already rendered by the
    /// caller (§1, `runtime-contract.md` §9.2).
    pub returned: Option<String>,
    /// Why the cell yielded on purpose (§9.3), drawn in the output region
    /// beside the table and never in the error region: it is not an error.
    pub yield_reason: Option<String>,
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
    /// Short provenance label shared by telemetry and compact status.
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

/// The supervisor's own sidebar line -- `docs/product/pane/supervisor.md` §4
/// and §5: a nudge's own reason, a look that did not intervene (which also
/// covers an unparseable or failed look -- both answer `not_intervene` and so
/// render identically, correctly as "not a nudge"), or off because no model
/// is configured or the switch is off.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SupervisorStatus {
    Nudged(String),
    LookedNoNudge,
    Off,
}

/// What the session knows about the conversation beyond the messages
/// themselves: one view per assistant cell, in cell order, the task's token
/// total, and the supervisor's latest status.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Notebook {
    pub requests: Vec<crate::telemetry::RequestMeasurement>,
    pub cells: Vec<CellView>,
    pub tokens: Option<TaskTokens>,
    pub supervisor: Option<SupervisorStatus>,
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

/// Compatibility entry point: the old caller supplies no live editor or
/// session instrumentation. Never infer sandbox authority from the transcript.
pub fn render(
    frame: &mut Frame,
    conversation: &Conversation,
    served_by: &ServedBy,
    handles: &HandleTable,
    notebook: &Notebook,
) {
    render_screen(
        frame,
        conversation,
        served_by,
        handles,
        notebook,
        &ScreenState::default(),
    );
}

pub fn render_screen(
    frame: &mut Frame,
    conversation: &Conversation,
    served_by: &ServedBy,
    handles: &HandleTable,
    notebook: &Notebook,
    state: &ScreenState,
) {
    let regions = screen_regions(frame.area(), state);
    // An explicit canvas also paints blank cells when the caller creates a
    // fresh Terminal over pre-existing stdout; default blank cells do not.
    frame.render_widget(
        Block::default().style(Style::default().bg(Color::Reset).fg(Color::White)),
        frame.area(),
    );
    let header = Line::from(vec![
        Span::styled(
            " PANE / ",
            Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
        ),
        Span::raw(abbreviate(
            state.project.as_deref().unwrap_or("project unknown"),
            usize::from(regions.header.width.saturating_sub(32)),
        )),
        Span::styled(
            format!(
                "   {} {}",
                if let Some(tick) = state.completion_tick {
                    ["[> ]", "[>>]", "[><]", "[<>]", "[+ ]", "[ +]"][tick.min(5)]
                } else {
                    state.activity.indicator(
                        if matches!(state.activity, Activity::Starting | Activity::Streaming) {
                            3
                        } else {
                            state.animation_frame
                        },
                    )
                },
                if state.completion_tick.is_some() {
                    "cell completed"
                } else {
                    state.activity.label()
                }
            ),
            Style::default().fg(if state.activity == Activity::Failed {
                Color::Red
            } else {
                ACCENT
            }),
        ),
    ]);
    frame.render_widget(
        Paragraph::new(header).block(Block::default().borders(Borders::BOTTOM)),
        regions.header,
    );
    if let Some(tick) = state.completion_tick {
        let width = regions.header.width.min(12);
        if width > 0 && regions.header.height > 0 {
            let travel = regions.header.width.saturating_sub(width);
            let x = regions.header.x + (u32::from(travel) * tick.min(5) as u32 / 5) as u16;
            frame.render_widget(
                Paragraph::new("╶──━━━━━━──╴")
                    .style(Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)),
                Rect::new(x, regions.header.bottom() - 1, width, 1),
            );
        }
    }
    if state.activity == Activity::Starting && conversation.messages.is_empty() {
        render_startup(frame, regions.transcript, state.animation_frame);
    } else {
        render_conversation(
            frame,
            regions.transcript,
            conversation,
            handles,
            notebook,
            state,
        );
    }
    if regions.details.width > 0 {
        telemetry::rail(frame, regions.details, served_by, notebook, state);
    }
    if state.telemetry_open {
        let area = Rect::new(
            regions.transcript.x,
            regions.transcript.y,
            regions.transcript.width
                + if regions.details.width > 0 {
                    regions.details.width + 2
                } else {
                    0
                },
            regions.transcript.height,
        );
        frame.render_widget(Clear, area);
        telemetry::expanded(frame, area, conversation, served_by, notebook, state);
    }
    if let Some(panel) = &state.panel {
        frame.render_widget(Clear, regions.transcript);
        frame.render_widget(
            Block::default().style(Style::default().fg(Color::White).bg(Color::Reset)),
            regions.transcript,
        );
        controls::render_panel(frame, regions.transcript, panel);
    }
    if let Some(notice) = &state.notice {
        let error = notice.starts_with("ERROR:");
        frame.render_widget(
            Paragraph::new(notice.as_str())
                .wrap(Wrap { trim: false })
                .block(Block::default().borders(Borders::TOP).title(if error {
                    " ERROR "
                } else {
                    " notice "
                }))
                .style(Style::default().fg(if error { Color::Red } else { MUTED })),
            regions.notice,
        );
    }
    let completion_skip = state
        .completion_selected
        .saturating_sub(usize::from(regions.completions.height).saturating_sub(1));
    let matches: Vec<Line> = slash_matches(&state.input)
        .into_iter()
        .enumerate()
        .skip(completion_skip)
        .map(|(index, (name, description))| {
            Line::from(format!(
                "{} {name:<15}{description}",
                if index == state.completion_selected {
                    "›"
                } else {
                    " "
                }
            ))
            .style(if index == state.completion_selected {
                Style::default().fg(Color::Black).bg(ACCENT)
            } else {
                Style::default().fg(MUTED)
            })
        })
        .collect();
    frame.render_widget(Paragraph::new(matches), regions.completions);
    let input_lines = wrapped_input(state, regions.input.width);
    let visible = usize::from(regions.input.height.saturating_sub(2));
    let cursor = state
        .cursor
        .map(|offset| composer_cursor(&state.input, offset, regions.input.width.saturating_sub(2)));
    let skip = cursor
        .map(|(row, _)| row.saturating_sub(visible.saturating_sub(1)))
        .unwrap_or_else(|| input_lines.len().saturating_sub(visible));
    frame.render_widget(
        Block::default().style(Style::default().bg(state.theme.dock())),
        regions.input,
    );
    if regions.input.height > 0 {
        for y in [regions.input.y, regions.input.bottom() - 1] {
            frame.render_widget(
                Paragraph::new("─".repeat(usize::from(regions.input.width)))
                    .style(Style::default().fg(ACCENT).bg(state.theme.dock())),
                Rect::new(regions.input.x, y, regions.input.width, 1),
            );
        }
    }
    if regions.input.height > 2 {
        frame.render_widget(
            Paragraph::new(input_lines.into_iter().skip(skip).collect::<Vec<_>>())
                .style(Style::default().bg(state.theme.dock())),
            Rect::new(
                regions.input.x,
                regions.input.y + 1,
                regions.input.width,
                regions.input.height - 2,
            ),
        );
    }
    if let Some((row, column)) = cursor
        && regions.input.width > 2
        && visible > 0
    {
        frame.set_cursor_position((
            regions.input.x + 2 + column.min(usize::from(regions.input.width - 3)) as u16,
            regions.input.y + 1 + (row - skip).min(visible - 1) as u16,
        ));
    }
    let model = state
        .model
        .as_deref()
        .or(served_by.model.as_deref())
        .unwrap_or("unknown");
    let project = state.project.as_deref().unwrap_or("unknown");
    let sandbox = state.sandbox.as_deref().unwrap_or("unknown");
    let network = state.network.as_deref().unwrap_or("unknown");
    let connection = match state.connected {
        Some(true) => "Glasshouse connected",
        Some(false) => "Glasshouse offline",
        None if served_by.is_known() => "Glasshouse metered",
        None => NOT_CONNECTED,
    };
    let width = usize::from(regions.status.width);
    let mode = format!("{} · effort {}", state.mode.name(), state.effort.name());
    let identity = format!(" {} · {}", abbreviate(model, 28), abbreviate(project, 24));
    let posture = format!(
        " sandbox {} · net:{}",
        abbreviate(sandbox, 16),
        abbreviate(network, 8)
    );
    let budget = notebook.tokens.filter(|_| width >= 100).map(|tokens| {
        format!(
            "budget: {}/{} tok · counted: {}",
            tokens.used,
            tokens.cap,
            tokens.counted.as_str()
        )
    });
    let status = if state.status_line == StatusLine::Compact {
        vec![footer_row(identity, mode, width, ACCENT)]
    } else if width < 140 {
        vec![
            footer_row(
                identity,
                if width >= 100 {
                    mode.clone()
                } else {
                    String::new()
                },
                width,
                ACCENT,
            ),
            footer_row(
                posture,
                budget
                    .clone()
                    .unwrap_or_else(|| if width < 100 { mode } else { String::new() }),
                width,
                MUTED,
            ),
            footer_row(
                format!(" {connection}"),
                "Shift-Tab mode · / commands".into(),
                width,
                MUTED,
            ),
        ]
    } else {
        vec![
            footer_row(identity, mode, width, ACCENT),
            footer_row(
                format!("{posture} · {connection}"),
                budget.unwrap_or_else(|| "Shift-Tab mode   / commands".into()),
                width,
                MUTED,
            ),
        ]
    };
    frame.render_widget(
        Paragraph::new(status).style(Style::default().fg(MUTED)),
        regions.status,
    );
    // Apply the palette once so every accent follows the same local preference.
    for cell in &mut frame.buffer_mut().content {
        if cell.fg == ACCENT {
            cell.set_fg(state.theme.accent());
        }
        if cell.bg == ACCENT {
            cell.set_bg(state.theme.accent());
        }
        if cell.bg == ACCENT {
            cell.set_bg(state.theme.accent());
        }
    }
}

/// Keep controls at the edge; omit optional hints when metadata fills the row.
fn footer_row(left: String, right: String, width: usize, right_color: Color) -> Line<'static> {
    let occupied = Line::from(left.as_str()).width() + Line::from(right.as_str()).width() + 2;
    if right.is_empty() || occupied > width {
        return Line::styled(abbreviate(&left, width), Style::default().fg(MUTED));
    }
    Line::from(vec![
        Span::styled(left, Style::default().fg(MUTED)),
        Span::raw(" ".repeat(width - occupied + 1)),
        Span::styled(right, Style::default().fg(right_color)),
        Span::raw(" "),
    ])
}

/// Startup is caller-driven and immediately replaced by any real transcript.
/// One small assembling wireframe; no timer, sleep or terminal ownership here.
fn render_startup(frame: &mut Frame, area: Rect, tick: usize) {
    let frames = [
        "     .     .\n\n  .     .\n\n  .     .",
        "     +-----+\n    /     /\n  +-----+\n  |     |\n  +-----+",
        "     +-----+\n    /     /|\n  +-----+  +\n  |  /  | /\n  +-----+/",
        "     +-----+\n    /     /|\n  +-----+  +\n  |     | /\n  +-----+/",
    ];
    frame.render_widget(
        Paragraph::new(frames[tick % frames.len()]).style(Style::default().fg(ACCENT)),
        area,
    );
}

fn abbreviate(text: &str, width: usize) -> String {
    if Line::from(text).width() <= width {
        return text.to_string();
    }
    let mut out = String::new();
    for glyph in Span::raw(text).styled_graphemes(Style::default()) {
        if Line::from(out.as_str()).width() + Span::raw(glyph.symbol).width() + 1 > width {
            break;
        }
        out.push_str(glyph.symbol);
    }
    if width > 0 {
        out.push('…');
    }
    out
}

fn composer_cursor(input: &str, offset: usize, width: u16) -> (usize, usize) {
    let mut offset = offset.min(input.len());
    while !input.is_char_boundary(offset) {
        offset -= 1;
    }
    let prefix = format!("{} ", &input[..offset]);
    let rows = wrap_lines(
        prefix
            .split('\n')
            .map(|line| Line::from(line.to_string()))
            .collect(),
        width,
    );
    (
        rows.len().saturating_sub(1),
        rows.last()
            .map(|line| line.width().saturating_sub(1))
            .unwrap_or(0),
    )
}

fn wrapped_input(state: &ScreenState, width: u16) -> Vec<Line<'static>> {
    let text = if state.input.is_empty() {
        "message or / for commands"
    } else {
        &state.input
    };
    let text = if state.cursor == Some(state.input.len()) && !state.input.is_empty() {
        format!("{text} ")
    } else {
        text.to_string()
    };
    wrap_lines(
        text.split('\n')
            .map(|line| Line::from(line.to_string()))
            .collect(),
        width.saturating_sub(2),
    )
    .into_iter()
    .enumerate()
    .map(|(i, mut line)| {
        line.spans.insert(
            0,
            Span::styled(
                if i == 0 { "› " } else { "│ " },
                Style::default().fg(ACCENT),
            ),
        );
        line
    })
    .collect()
}

/// Wrap graphemes before viewport slicing: newest rows cannot be lost to a
/// logical-line scroll offset, and wide/combining characters keep cell bounds.
fn wrap_lines(lines: Vec<Line<'static>>, width: u16) -> Vec<Line<'static>> {
    if width == 0 {
        return Vec::new();
    }
    let mut out = Vec::new();
    for line in lines {
        let mut row = Vec::new();
        let mut used = 0;
        for span in line.spans {
            for glyph in span.styled_graphemes(line.style) {
                let size = Span::raw(glyph.symbol).width();
                if size > usize::from(width) {
                    continue;
                }
                if used + size > usize::from(width) {
                    out.push(Line::from(std::mem::take(&mut row)).style(line.style));
                    used = 0;
                }
                row.push(Span::styled(glyph.symbol.to_string(), glyph.style));
                used += size;
            }
        }
        out.push(Line::from(row).style(line.style));
    }
    out
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
    notebook_lines(conversation, handles, notebook, false, false, 98).len()
}

/// One line with nothing to show. Never collapses to no line at all -- the
/// same rule the sidebar keeps for an unmetered request.
const NO_OUTPUTS: &str = "(no outputs)";

/// How many cells the conversation holds, counted the one way the screen
/// numbers them: the task is the first message and is drawn as a header, a
/// cell is an assistant message after it -- **except the terminal
/// response**, the assistant message that follows a cell whose view
/// returned (`runtime-contract.md` §9.2). That message is the model's reply,
/// not a program, and numbering it would put the next task's first cell one
/// off from its view. The session reads this to place each new view.
pub fn cell_ordinal(conversation: &Conversation, notebook: &Notebook) -> usize {
    let mut cells = 0usize;
    let mut after_return = false;
    for message in conversation.messages.iter().skip(1) {
        match message.role {
            Role::Assistant if after_return => after_return = false,
            Role::Assistant => {
                cells += 1;
                after_return = notebook
                    .cell(cells)
                    .is_some_and(|view| view.returned.is_some());
            }
            Role::User => after_return = false,
        }
    }
    cells
}

fn render_conversation(
    frame: &mut Frame,
    area: Rect,
    conversation: &Conversation,
    handles: &HandleTable,
    notebook: &Notebook,
    state: &ScreenState,
) {
    let mut content = notebook_lines(
        conversation,
        handles,
        notebook,
        state.compact,
        state.pretty,
        usize::from(area.width.saturating_sub(1)),
    );
    if let Some(partial) = state.streaming_text.as_deref() {
        turn_header(
            &mut content,
            format!(
                "PANE / RECEIVING  {} · {} bytes",
                Activity::Streaming.indicator(state.animation_frame),
                partial.len()
            ),
            ACCENT,
        );
        if state.compact && (partial.contains("```") || partial.contains("<php-pane>")) {
            let prose = partial
                .split("```")
                .next()
                .unwrap_or_default()
                .split("<php-pane>")
                .next()
                .unwrap_or_default();
            if !prose.trim().is_empty() {
                push_text_region(&mut content, prose.trim());
            }
            push_text_region(
                &mut content,
                "Preparing actions · Ctrl-O shows incoming code",
            );
        } else if partial.contains("```") {
            push_text_region(&mut content, partial);
        } else {
            content.extend(markdown::render(
                partial,
                usize::from(area.width.saturating_sub(1)),
            ));
        }
    }
    let mut active = false;
    for line in &mut content {
        if let Some(first) = line.spans.first_mut() {
            if first.content.starts_with("╭─ ") {
                first.content = first.content.trim_start_matches("╭─ ").to_string().into();
                active = true;
            } else if first.content == "╰─" {
                *line = Line::default();
                active = false;
            }
        }
        if active {
            line.style = line.style.bg(state.theme.backlight());
        }
    }
    let lines = wrap_lines(content, area.width.saturating_sub(1));
    let start = lines
        .len()
        .saturating_sub(usize::from(area.height))
        .saturating_sub(state.scrollback);
    let lines: Vec<Line> = lines
        .into_iter()
        .skip(start)
        .take(usize::from(area.height))
        .map(|mut line| {
            if line.style.bg.is_some() {
                line.spans.insert(0, Span::raw(" "));
                let padding = usize::from(area.width).saturating_sub(line.width());
                line.spans.push(Span::raw(" ".repeat(padding)));
            }
            line
        })
        .collect();
    frame.render_widget(Paragraph::new(lines), area);
}

fn turn_header(lines: &mut Vec<Line<'static>>, label: String, color: Color) {
    if !lines.is_empty() {
        lines.push(Line::styled("╰─", Style::default().fg(MUTED)));
        lines.push(Line::from(""));
    }
    lines.push(Line::styled(
        format!("╭─ {label}"),
        Style::default().fg(color).add_modifier(Modifier::BOLD),
    ));
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
    compact: bool,
    pretty: bool,
    width: usize,
) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    let mut messages = conversation.messages.iter();

    if let Some(task) = messages.next() {
        turn_header(&mut lines, "USER".into(), ACCENT);
        push_text_region(&mut lines, &message_text(task));
    }

    let total_cells = cell_ordinal(conversation, notebook);

    let mut cell = 0usize;
    let mut answered = false;
    let mut after_return = false;
    for message in messages {
        match message.role {
            // The assistant message after a cell that returned is the
            // terminal response -- the model's reply, drawn as its turn
            // rather than as a cell (`runtime-contract.md` §9.2).
            Role::Assistant if after_return => {
                after_return = false;
                // The identical terminal response is already visible in its return region.
            }
            Role::Assistant => {
                cell += 1;
                let view = notebook.cell(cell);
                answered = view.is_some_and(|view| view.answered);
                after_return = view.is_some_and(|view| view.returned.is_some());

                let original = message_text(message);
                if matches!(extract_program(&original), Extracted::Prose)
                    && view.is_none_or(|v| {
                        v.table.is_none()
                            && v.execution.is_none()
                            && v.error.is_none()
                            && v.returned.is_none()
                    })
                    && !original.contains("<php-pane>")
                    && !original.contains("```pane")
                {
                    turn_header(&mut lines, "PANE".into(), Color::White);
                    lines.extend(markdown::render(&original, width));
                    continue;
                }
                let (before, after) = natural_message(message);
                if !before.trim().is_empty() {
                    turn_header(&mut lines, "PANE".into(), ACCENT);
                    lines.extend(markdown::render(before.trim(), width));
                }
                if compact {
                    let original = message_text(message);
                    match extract_program(&original) {
                        Extracted::Program(source) | Extracted::Edit(source) => {
                            let repairing =
                                matches!(extract_program(&original), Extracted::Edit(_));
                            let source = view
                                .and_then(|v| v.executed_source.as_ref())
                                .unwrap_or(&source);
                            let possible = possible_tool_calls(source);
                            let only_answer = !repairing
                                && possible.is_empty()
                                && view.is_some_and(|v| {
                                    v.returned.is_some()
                                        && v.execution
                                            .as_deref()
                                            .is_some_and(|calls| calls.starts_with("No tool"))
                                });
                            if !only_answer {
                                let failed = view.is_some_and(|v| v.error.is_some());
                                let evaluated = view.is_some_and(|v| v.execution.is_some());
                                let label = if repairing && failed {
                                    "× Cell repair failed"
                                } else if repairing && evaluated {
                                    "◆ Cell repaired"
                                } else if repairing {
                                    "◇ Preparing cell repair"
                                } else if failed {
                                    "× Action failed"
                                } else if evaluated {
                                    "◆ Tool results"
                                } else {
                                    "◇ Preparing actions"
                                };
                                turn_header(
                                    &mut lines,
                                    format!("{label}  · {cell}"),
                                    if failed { Color::Red } else { ACCENT },
                                );
                                let none_ran = view
                                    .and_then(|v| v.execution.as_deref())
                                    .is_some_and(|calls| calls.starts_with("No tool"));
                                if let Some(target) = view.and_then(|v| v.repaired_from) {
                                    lines.push(Line::styled(
                                        format!("Amends syntax-failed cell {target}"),
                                        Style::default().fg(MUTED),
                                    ));
                                }
                                if !possible.is_empty()
                                    && (possible.len() > 1 || !evaluated || none_ran)
                                {
                                    lines.push(Line::styled(
                                        format!(
                                            "◇ planned: {} · conditional calls may not run",
                                            possible.join(" → ")
                                        ),
                                        Style::default().fg(Color::LightCyan),
                                    ));
                                }
                                if let Some(actual) = view.and_then(|v| v.execution.as_deref()) {
                                    if actual.starts_with("No tool") {
                                        if failed || !possible.is_empty() {
                                            push_text_region(&mut lines, "No tools ran.");
                                        }
                                    } else {
                                        push_text_region(&mut lines, actual);
                                        let count = actual.lines().count();
                                        if count > 1 {
                                            lines.push(Line::styled(
                                                format!(
                                                    "◆ {count} tool calls in one inference turn"
                                                ),
                                                Style::default().fg(ACCENT),
                                            ));
                                        }
                                    }
                                }
                                lines.push(Line::styled(
                                    "Ctrl-O · code and results",
                                    Style::default().fg(MUTED),
                                ));
                            }
                        }
                        Extracted::TwoBlocks => {
                            turn_header(
                                &mut lines,
                                "Response format rejected".into(),
                                Color::Yellow,
                            );
                            push_text_region(
                                &mut lines,
                                "Multiple code blocks · nothing ran. Ctrl-O shows the response.",
                            );
                        }
                        Extracted::Prose
                            if original.contains("<php-pane>") || original.contains("```pane") =>
                        {
                            turn_header(
                                &mut lines,
                                "Response format rejected".into(),
                                Color::Yellow,
                            );
                            push_text_region(
                                &mut lines,
                                "Expected one Pane code block · nothing ran. Ctrl-O shows the response.",
                            );
                        }
                        Extracted::Prose => {
                            turn_header(&mut lines, "PANE".into(), ACCENT);
                            push_text_region(&mut lines, &original);
                        }
                    }
                    if let Some(plan) = view
                        .and_then(|v| v.table.as_deref())
                        .filter(|s| s.starts_with("Planning mode"))
                    {
                        push_text_region(&mut lines, plan);
                    }
                    if let Some(error) = view.and_then(|v| v.error.as_ref()) {
                        push_text_region(
                            &mut lines,
                            &format!(
                                "{}: {}",
                                error.class,
                                error.message.lines().next().unwrap_or_default()
                            ),
                        );
                    }
                    if let Some(stdout) = view
                        .and_then(|v| v.stdout.as_deref())
                        .filter(|s| !s.trim().is_empty() && s.trim() != "undefined")
                    {
                        push_folded_region(&mut lines, stdout, 6, true);
                    }
                    if let Some(changes) = view.and_then(|v| v.changes.as_deref()) {
                        push_changes(&mut lines, changes, true);
                    }
                    if let Some(returned) = view.and_then(|v| v.returned.as_deref()) {
                        turn_header(&mut lines, "PANE".into(), ACCENT);
                        lines.extend(markdown::render(&pretty_json(returned), width));
                    }
                    if !after.trim().is_empty() {
                        lines.extend(markdown::render(after.trim(), width));
                    }
                    continue;
                }
                let role = if matches!(
                    extract_program(&message_text(message)),
                    Extracted::Program(_) | Extracted::Edit(_)
                ) {
                    "PANE / CODE"
                } else if matches!(
                    extract_program(&message_text(message)),
                    Extracted::TwoBlocks
                ) {
                    "PANE / NOT EXECUTED: multiple code blocks"
                } else if message_text(message).contains("<php-pane>") {
                    "PANE / NOT EXECUTED"
                } else {
                    "PANE"
                };
                let execution = if role == "PANE / CODE" {
                    if view.is_some_and(|v| v.error.is_some()) {
                        " · failed ×"
                    } else if view.is_some_and(|v| v.execution.is_some()) {
                        " · executed ◆"
                    } else {
                        " · proposed ◇"
                    }
                } else {
                    ""
                };
                turn_header(
                    &mut lines,
                    format!("{role}  [{cell}] in{execution}"),
                    ACCENT,
                );
                if let Some(target) = view.and_then(|v| v.repaired_from) {
                    lines.push(Line::styled(
                        format!("Amends syntax-failed cell {target}"),
                        Style::default().fg(MUTED),
                    ));
                }
                let source = view
                    .and_then(|v| v.executed_source.clone())
                    .unwrap_or_else(|| input_region(message));
                let display = if pretty && view.is_none_or(|v| v.error.is_none()) {
                    match extract_program(&message_text(message)) {
                        Extracted::Program(_) | Extracted::Edit(_) => pretty_code(&source),
                        _ => source,
                    }
                } else {
                    source
                };
                if role == "PANE / CODE" {
                    let code = markdown::code(&display);
                    let limit = if compact { 10 } else { usize::MAX };
                    let remaining = code.len().saturating_sub(limit);
                    lines.extend(code.into_iter().take(limit));
                    if remaining > 0 {
                        lines.push(Line::styled(
                            format!("… {remaining} more lines · Ctrl-O expands"),
                            Style::default().fg(MUTED),
                        ));
                    }
                } else {
                    push_folded_region(&mut lines, &display, 10, compact);
                }
                if role == "PANE / CODE" {
                    let candidates = possible_tool_calls(
                        view.and_then(|v| v.executed_source.as_deref())
                            .unwrap_or(&input_region(message)),
                    );
                    if !candidates.is_empty() {
                        lines.push(Line::styled(
                            format!("◇ possible: {} · branches may skip", candidates.join(" → ")),
                            Style::default().fg(Color::LightCyan),
                        ));
                    }
                }

                if let Some(execution) = view.and_then(|view| view.execution.as_deref()) {
                    let count = execution
                        .lines()
                        .filter(|line| line.starts_with("├─") || line.starts_with("└─"))
                        .count();
                    turn_header(
                        &mut lines,
                        format!("TOOL / ACTUAL  [{cell}] ◆ {count} calls · one cell"),
                        ACCENT,
                    );
                    push_text_region(&mut lines, execution);
                }
                turn_header(&mut lines, format!("TOOL / PREVIEW  [{cell}] out"), MUTED);
                match view.and_then(|view| view.table.as_deref()) {
                    Some(table) => push_output_region(&mut lines, table.to_string(), compact),
                    None if cell == total_cells => push_output_region(
                        &mut lines,
                        render_table(handles, PREVIEW_TOKEN_CAP, TABLE_TOKEN_CAP),
                        compact,
                    ),
                    None => lines.push(Line::from(NO_OUTPUTS)),
                }
                if let Some(stdout) = view.and_then(|view| view.stdout.as_deref()) {
                    turn_header(&mut lines, "OUTPUT".into(), MUTED);
                    push_folded_region(&mut lines, stdout, 6, compact);
                }
                if let Some(changes) = view.and_then(|v| v.changes.as_deref()) {
                    push_changes(&mut lines, changes, compact);
                }
                if let Some(reason) = view.and_then(|view| view.yield_reason.as_deref()) {
                    lines.push(Line::from(format!("yielded: {reason}")));
                }

                if let Some(error) = view.and_then(|view| view.error.as_ref()) {
                    turn_header(&mut lines, format!("ERROR  [{cell}] error"), Color::Red);
                    push_error_region(&mut lines, error);
                }
                if let Some(returned) = view.and_then(|view| view.returned.as_deref()) {
                    turn_header(
                        &mut lines,
                        format!("PANE / RETURN  [{cell}] return"),
                        ACCENT,
                    );
                    let display = if compact {
                        pretty_json(returned)
                    } else {
                        returned.to_string()
                    };
                    lines.extend(markdown::render(&display, width));
                }
                if !after.trim().is_empty() {
                    turn_header(&mut lines, "PANE".into(), ACCENT);
                    lines.extend(markdown::render(after.trim(), width));
                }
            }
            Role::User => {
                after_return = false;
                if answered {
                    answered = false;
                    continue;
                }
                turn_header(&mut lines, "USER".into(), ACCENT);
                push_text_region(&mut lines, &format!("you: {}", message_text(message)));
            }
        }
    }

    if !lines.is_empty() {
        lines.push(Line::styled("╰─", Style::default().fg(MUTED)));
    }
    lines
}

/// A cell's input region: the program the message carried, or its prose when
/// it carried none. `model-contract.md` §5's parser is the one that decides
/// which -- the notebook shows what actually ran, not the explanation around
/// it, and a message with two blocks (where neither ran) shows its whole text
/// rather than picking one of them.
/// Show existing narration around the one executable fence, without asking
/// the model for any narration or changing its program/source positions.
fn natural_message(message: &Message) -> (String, String) {
    let text = message_text(message);
    if !matches!(
        extract_program(&text),
        Extracted::Program(_) | Extracted::Edit(_)
    ) {
        return (String::new(), String::new());
    }
    let lines: Vec<_> = text.lines().collect();
    let mut i = 0;
    while i < lines.len() {
        if let Some(info) = lines[i].strip_prefix("```") {
            let mut end = i + 1;
            while end < lines.len() && lines[end] != "```" {
                end += 1;
            }
            if matches!(info.trim(), "pane" | "pane-edit") {
                return (
                    lines[..i].join("\n"),
                    lines[(end + 1).min(lines.len())..].join("\n"),
                );
            }
            i = end + 1;
        } else {
            i += 1;
        }
    }
    (String::new(), String::new())
}

/// Syntactic candidates only: an untaken branch is never execution evidence.
fn possible_tool_calls(source: &str) -> Vec<String> {
    use oxc::{
        allocator::Allocator,
        ast::ast::{CallExpression, Expression},
        ast_visit::{Visit, walk},
        parser::{ParseOptions, Parser},
        span::SourceType,
    };
    struct Calls(Vec<String>);
    impl<'a> Visit<'a> for Calls {
        fn visit_call_expression(&mut self, call: &CallExpression<'a>) {
            if let Expression::Identifier(name) = &call.callee
                && crate::tools::registry::names().contains(&name.name.as_str())
            {
                self.0.push(name.name.to_string());
            }
            if let Expression::StaticMemberExpression(member) = &call.callee
                && let Expression::Identifier(owner) = &member.object
                && matches!(
                    (owner.name.as_str(), member.property.name.as_str()),
                    ("agent", "run") | ("bg", "run" | "watch" | "cancel")
                )
            {
                self.0
                    .push(format!("{}.{}", owner.name, member.property.name));
            }
            walk::walk_call_expression(self, call);
        }
    }
    let allocator = Allocator::default();
    let parsed = Parser::new(&allocator, source, SourceType::ts())
        .with_options(ParseOptions {
            allow_return_outside_function: true,
            ..ParseOptions::default()
        })
        .parse();
    if !parsed.diagnostics.is_empty() {
        return Vec::new();
    }
    let mut calls = Calls(Vec::new());
    calls.visit_program(&parsed.program);
    calls.0
}

/// Display only. The conversation and executable source remain byte-for-byte intact.
/// Expanded view (Ctrl-O) shows original code, as do cells with source-position errors.
fn push_changes(lines: &mut Vec<Line<'static>>, changes: &str, compact: bool) {
    turn_header(lines, "CHANGES OBSERVED".into(), Color::LightCyan);
    let limit = if compact { 18 } else { usize::MAX };
    for line in changes.lines().take(limit) {
        let color = if line.starts_with("+++") || line.starts_with("---") {
            Color::LightCyan
        } else if line.starts_with('+') {
            Color::LightGreen
        } else if line.starts_with('-') {
            Color::LightRed
        } else {
            MUTED
        };
        // A diff is data: terminal controls must never affect rendering.
        let text: String = line
            .chars()
            .map(|c| if c.is_control() { ' ' } else { c })
            .collect();
        lines.push(Line::styled(text, Style::default().fg(color)));
    }
    let remaining = changes.lines().count().saturating_sub(limit);
    if remaining > 0 {
        lines.push(Line::styled(
            format!("… {remaining} more diff lines · Ctrl-O expands"),
            Style::default().fg(MUTED),
        ));
    }
}

fn pretty_code(source: &str) -> String {
    use oxc::{
        allocator::Allocator,
        codegen::{Codegen, CodegenOptions, IndentChar},
        parser::{ParseOptions, Parser},
        span::SourceType,
    };
    let allocator = Allocator::default();
    let parsed = Parser::new(&allocator, source, SourceType::ts())
        .with_options(ParseOptions {
            allow_return_outside_function: true,
            ..ParseOptions::default()
        })
        .parse();
    if !parsed.diagnostics.is_empty() {
        return source.to_string();
    }
    Codegen::new()
        .with_options(CodegenOptions {
            indent_char: IndentChar::Space,
            indent_width: 2,
            ..CodegenOptions::default()
        })
        .build(&parsed.program)
        .code
        .trim_end()
        .to_string()
}

fn pretty_json(text: &str) -> String {
    match serde_json::from_str::<serde_json::Value>(text) {
        Ok(value) if value.is_object() || value.is_array() => {
            serde_json::to_string_pretty(&value).unwrap_or_else(|_| text.to_string())
        }
        _ => text.to_string(),
    }
}

fn input_region(message: &Message) -> String {
    let text = message_text(message);
    match extract_program(&text) {
        Extracted::Program(source) | Extracted::Edit(source) => source,
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
fn push_output_region(lines: &mut Vec<Line<'static>>, table: String, compact: bool) {
    if table.is_empty() {
        lines.push(Line::from(NO_OUTPUTS));
        return;
    }
    push_folded_region(lines, &table, 6, compact);
}

fn push_folded_region(lines: &mut Vec<Line<'static>>, text: &str, limit: usize, compact: bool) {
    if !compact || text.lines().count() <= limit {
        push_text_region(lines, text);
        return;
    }
    for line in text.lines().take(limit) {
        lines.push(Line::from(line.to_string()));
    }
    lines.push(Line::styled(
        format!(
            "… {} more lines · Ctrl-O expands",
            text.lines().count() - limit
        ),
        Style::default().fg(MUTED),
    ));
}

fn message_text(message: &Message) -> String {
    message
        .content
        .iter()
        .map(|block| block.text())
        .collect::<Vec<_>>()
        .join("")
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
    if let Some(route) = &served_by.route {
        lines.push(Line::from(format!("route: {route}")));
    }
    if let Some(cached) = served_by.cached_input_tokens {
        lines.push(Line::from(format!("cached input: {cached} tok")));
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

/// §4 and §5's three fixed lines, and nothing else -- the sidebar shows this
/// one line under the budget line, whatever `served_by` says.
fn supervisor_line(status: &SupervisorStatus) -> String {
    match status {
        SupervisorStatus::Nudged(reason) => format!("supervisor: {reason}"),
        SupervisorStatus::LookedNoNudge => "supervisor: looked, no nudge".to_string(),
        SupervisorStatus::Off => "supervisor: off (no model)".to_string(),
    }
}
