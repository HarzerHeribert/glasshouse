//! Fullscreen presentation. The caller owns terminal lifecycle, input and ticks.

mod controls;
mod history;
pub use history::HistoryNote;
mod composer;
mod regions;
pub(crate) use composer::composer_offset;
use composer::{composer_cursor, wrapped_input};
mod hit;
pub(crate) use hit::{Hit, ScreenGeometry, StatusField};

mod inspection;
pub use inspection::Inspection;
mod lane;
mod markdown;
mod ribbon;
mod scroll;
pub use scroll::SCROLL_INDICATOR_LINGER;
use scroll::render_scrollbar;
mod status;
use regions::{
    push_changes, push_error_region, push_folded_region, push_output_region, push_text_region,
};
use status::{compact_tokens, context_summary, footer_right_span, footer_row};
mod telemetry;
pub(crate) use controls::PanelHit;
pub use controls::{Assignment, Mode, ModelGroup, Panel, PanelRow, StatusLine, TierModels};
pub(crate) use lane::helper_in_flight;
use lane::{helper_fold, helper_lane, push_helper_lane};
pub use telemetry::Pulse;

use crate::commands::{BUILT_INS, BuiltIn};
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};

use crate::contract::{Block as ContentBlock, Conversation, Message, Role, ServedBy};
use crate::helpers::HelperRecord;
use crate::prompt::{Extracted, extract_program};
use crate::runtime::handles::{HandleTable, render_table};
use crate::runtime::preview::PREVIEW_TOKEN_CAP;
use crate::runtime::preview::TABLE_TOKEN_CAP;

const ACCENT: Color = Color::LightGreen;
const MUTED: Color = Color::Gray;
const NOT_CONNECTED: &str = "Glasshouse not connected.";

/// Modal confirmation drawn last, above all other surfaces. The terminal
/// owner retains the request and is the only code that can answer it.
///
/// `hint` is the approval hint (F4, `decision-model.md`): the caller passes
/// `Request::hint_line()`, already gated by `mode` -- `None` here never
/// distinguishes "no model", "not answered yet" and "shadow" from each other,
/// because none of the three ever change what is drawn.
pub fn render_approval(
    frame: &mut Frame<'_>,
    confirmation: &crate::approval::Confirmation,
    scroll: u16,
    hint: Option<crate::approval::Hint>,
) {
    let area = frame.area();
    let width = area.width.saturating_sub(4).min(100);
    let height = area.height.saturating_sub(2).min(28);
    let overlay = Rect::new(
        area.x + area.width.saturating_sub(width) / 2,
        area.y + area.height.saturating_sub(height) / 2,
        width,
        height,
    );
    frame.render_widget(Clear, overlay);
    let block = Block::default()
        .title(" Approve exact tool call ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(ACCENT));
    let inner = block.inner(overlay);
    frame.render_widget(block, overlay);
    if inner.height == 0 || inner.width == 0 {
        return;
    }
    let footer_height = inner.height.min(3);
    let body = Rect::new(
        inner.x,
        inner.y,
        inner.width,
        inner.height.saturating_sub(footer_height),
    );
    let footer = Rect::new(inner.x, inner.y + body.height, inner.width, footer_height);
    let estimated_rows: usize = confirmation
        .text
        .lines()
        .map(|line| {
            line.chars()
                .count()
                .max(1)
                .div_ceil(usize::from(body.width).max(1))
        })
        .sum();
    let maximum_scroll = estimated_rows
        .saturating_sub(usize::from(body.height))
        .min(u16::MAX as usize) as u16;
    frame.render_widget(
        Paragraph::new(confirmation.text.as_str())
            .wrap(Wrap { trim: false })
            .scroll((scroll.min(maximum_scroll), 0)),
        body,
    );
    let choices = if confirmation.complete {
        "[o] Allow once  [s] Allow this exact call for session  [d/Esc] Deny\n↑/↓ PgUp/PgDn scroll · Expires after 10 min · Sandbox unchanged"
    } else {
        "[d/Esc] Deny · This action cannot be approved because its complete details exceed the display limit"
    };
    let footer_text = match hint {
        Some(hint) => format!(
            "fits the request: {:.2} (decision, {} ms)\n{choices}",
            hint.fits, hint.asked_ms
        ),
        None => choices.to_string(),
    };
    frame.render_widget(
        Paragraph::new(footer_text)
            .style(Style::default().fg(ACCENT))
            .wrap(Wrap { trim: false }),
        footer,
    );
}

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
    /// A keystroke hint the next keystroke replaces; everything a person may
    /// want to read again is a [`HistoryNote`] instead.
    pub notice: Option<String>,
    /// Notices kept in the conversation, in arrival order.
    pub history: Vec<HistoryNote>,
    /// Messages in the conversation this state last drew, where a new note goes.
    pub messages_seen: usize,
    /// A modal masked prompt, open over the composer. While it is set the
    /// composer is not what the keyboard reaches.
    pub secret_prompt: Option<SecretPrompt>,
    /// Fold long code and previews locally; never changes the model messages.
    pub compact: bool,
    pub pretty: bool,
    pub activity: Activity,
    /// Partial provider text for the active response; never persisted as a completed turn.
    /// The streaming caller replaces this accumulated text and clears it on completion.
    pub streaming_text: Option<String>,
    /// Raw native tool-input fragments, presentation-only; never executed or shown as results.
    pub streaming_tool_input: Option<String>,
    pub animation_frame: usize,
    pub completion_tick: Option<usize>,
    /// Rows back from the transcript's end; zero follows the current turn.
    pub scrollback: usize,
    /// A selected notebook cell, inspected locally without model traffic.
    pub inspection: Option<Inspection>,
    /// User preference, retained across resizes. The caller toggles this field.
    pub sidebar: SidebarVisibility,
    /// Chrome off, composer kept: the transcript takes the whole terminal.
    ///
    /// It overrides `sidebar`, `status_line` and the activity ribbon for as
    /// long as it is set rather than writing to them, which is why leaving
    /// fullscreen restores exactly the layout the user had before entering.
    /// The composer is never part of the hide-set: a screen that cannot be
    /// typed into has taken something away rather than given room back.
    pub fullscreen: bool,
    /// A scroll happened recently, so the position indicator is up. The
    /// caller owns the clock and clears this after
    /// [`SCROLL_INDICATOR_LINGER`]; the renderer only obeys it, which keeps
    /// the drawing pure and testable.
    pub scrolling: bool,
    /// Mouse reporting is released to the terminal, so the person can select
    /// and copy with a drag. Clicks do not land while this is set, which is
    /// why the status line says so (the user, 2026-09-17: click-and-drag
    /// selection must stay available).
    pub mouse_off: bool,
    pub theme: Theme,
    pub settings_root: Option<std::path::PathBuf>,
    pub settings_profile: Option<String>,
    pub settings_models: Vec<String>,
    pub mode: Mode,
    /// The permission rung, shared live with the approval gate: Shift-Tab
    /// moves it from this thread while a task runs.
    pub permissions: crate::permissions::Ladder,
    pub effort: crate::wire::Effort,
    pub status_line: StatusLine,
    pub panel: Option<Panel>,
    pub telemetry_open: bool,
    pub telemetry_selected: Option<usize>,
    pub reduced_motion: bool,
    pub pulse: Pulse,
    /// The completion gate's recap of the task just accepted, when
    /// `[helpers] completion = "recap"` asked for one.
    ///
    /// `None` -- the silent default, no helper model, or a call that never
    /// came back -- renders nothing at all, and neither does a record that
    /// failed: a recap must never replace or delay the answer it follows.
    /// The caller clears it when the next task begins.
    pub recap: Option<HelperRecord>,
}

/// A modal masked prompt: the one place a session takes a secret from the
/// keyboard.
///
/// **What was typed leaves this value only through [`Self::take`].** The
/// renderer is given [`Self::mask`] -- one `•` per character -- and `Debug`
/// redacts, so a screen state that is cloned, logged or dumped carries the
/// bullets and never the key.
#[derive(Clone, Default)]
pub struct SecretPrompt {
    title: String,
    entered: String,
}

impl std::fmt::Debug for SecretPrompt {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SecretPrompt")
            .field("title", &self.title)
            .field("entered", &self.mask())
            .finish()
    }
}

impl SecretPrompt {
    #[must_use]
    pub fn new(title: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            entered: String::new(),
        }
    }
    #[must_use]
    pub fn title(&self) -> &str {
        &self.title
    }
    /// Typed or pasted text, control characters dropped: a pasted key carries
    /// whatever line ending the place it was copied from used, and none of it
    /// belongs in a credential.
    pub fn push(&mut self, text: &str) {
        self.entered
            .extend(text.chars().filter(|c| !c.is_control()));
    }
    pub fn backspace(&mut self) {
        self.entered.pop();
    }
    pub fn clear(&mut self) {
        self.entered.clear();
    }
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entered.is_empty()
    }
    /// One bullet per character -- all the renderer is ever given.
    #[must_use]
    pub fn mask(&self) -> String {
        "•".repeat(self.entered.chars().count())
    }
    /// What was entered, consuming the prompt with it.
    #[must_use]
    pub fn take(self) -> String {
        self.entered
    }
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
    pub(crate) fn accent(self) -> Color {
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
    pub activity: Rect,
    pub input: Rect,
    pub status: Rect,
}

pub fn screen_regions(area: Rect, state: &ScreenState) -> ScreenRegions {
    let status_h = if state.fullscreen {
        0
    } else {
        area.height.min(match state.status_line {
            StatusLine::Full => {
                if area.width < 140 {
                    3
                } else {
                    2
                }
            }
            StatusLine::Compact => 1,
            StatusLine::Hidden => 0,
        })
    };
    let status = Rect::new(area.x, area.bottom() - status_h, area.width, status_h);
    let remaining = status.y - area.y;
    let input_h = (wrapped_input(state, area.width)
        .len()
        .min(usize::from((area.height / 3).max(1))) as u16)
        .saturating_add(2)
        .max(3)
        .min(remaining);
    let input = Rect::new(area.x, status.y - input_h, area.width, input_h);
    // A masked prompt owns the composer: the editor's text is still there,
    // but it is not what the next keystroke goes to, so completions for it
    // would be an offer the keyboard cannot take.
    let completion_h = (completions_shown(state).len() as u16)
        .min(7)
        .min((input.y - area.y).saturating_sub(5));
    let completions = Rect::new(area.x, input.y - completion_h, area.width, completion_h);
    let notice_h = if state.notice.is_some() {
        3.min((completions.y - area.y).saturating_sub(4))
    } else {
        0
    };
    let notice = Rect::new(area.x, completions.y - notice_h, area.width, notice_h);
    let header_h = if state.fullscreen {
        0
    } else {
        (notice.y - area.y).min(2)
    };
    let header = Rect::new(area.x, area.y, area.width, header_h);
    let available_body = notice.y - header.bottom();
    let moving = matches!(
        state.activity,
        Activity::Thinking
            | Activity::Streaming
            | Activity::Executing
            | Activity::Waiting
            | Activity::Searching
            | Activity::Compacting
    ) || state.completion_tick.is_some();
    let activity_h = if moving
        && !state.fullscreen
        && !state.telemetry_open
        && area.height >= 28
        && area.width >= 60
        && available_body >= 18
    {
        if area.width >= 100 { 3 } else { 2 }
    } else {
        0
    };
    let activity = Rect::new(
        area.x,
        notice.y - activity_h,
        area.width.min(180),
        activity_h,
    );
    let body_h = activity.y - header.bottom();
    let sidebar_visible = !state.fullscreen
        && match state.sidebar {
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
        activity,
        input,
        status,
    }
}

/// Only real built-ins, with descriptions of their vocabulary rather than
/// claims that a command was successfully executed.
/// The completions this screen is offering, which is none at all while a
/// masked prompt has the keyboard.
fn completions_shown(state: &ScreenState) -> Vec<(String, &'static str)> {
    if state.secret_prompt.is_some() {
        return Vec::new();
    }
    slash_matches(&state.input)
}

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
                    BuiltIn::Model => "set the parent, helper or subagent model",
                    BuiltIn::Models => "browse models by agent, provider or intelligence",
                    BuiltIn::Entitlements => "inspect available entitlements",
                    BuiltIn::Login => "connect a subscription account",
                    BuiltIn::Handles => "inspect runtime handles",
                    BuiltIn::Supervisor => "inspect supervisor settings",
                    BuiltIn::Rollback => "roll back to a checkpoint",
                    BuiltIn::Budget => "inspect cumulative task spend",
                    BuiltIn::Memory => "read or save project memory",
                    BuiltIn::Exit => "end the session, printing its resume id",
                },
            )
        })
        .chain(
            [
                (
                    "/handlers".to_string(),
                    "inspect standing handlers · /handlers off <name>",
                ),
                ("/help".to_string(), "show available commands"),
                ("/exit".to_string(), "leave Pane"),
                ("/sidebar".to_string(), "auto, show or hide telemetry"),
                ("/theme".to_string(), "choose a palette · eight themes"),
                (
                    "/telemetry".to_string(),
                    "live activity, requests and execution · Ctrl-T",
                ),
                ("/motion".to_string(), "on or off · reduce animation"),
                (
                    "/cells".to_string(),
                    "inspect code and real results by cell",
                ),
                ("/cell".to_string(), "inspect a numbered cell · /cell 12"),
                ("/chat".to_string(), "return to the conversation"),
                (
                    "/key".to_string(),
                    "enter a provider API key · /key anthropic",
                ),
                ("/effort".to_string(), "configure response reasoning effort"),
                (
                    "/context".to_string(),
                    "inspect current context and token usage",
                ),
                ("/status".to_string(), "inspect session status"),
                ("/settings".to_string(), "Global / Project settings"),
                ("/config".to_string(), "inspect or edit advanced settings"),
                ("/statusline".to_string(), "full, compact or hidden status"),
                (
                    "/fullscreen".to_string(),
                    "transcript only, composer kept · Ctrl-F",
                ),
                (
                    "/permissions".to_string(),
                    "inspect or configure next-session grants",
                ),
                ("/mode".to_string(), "execute, explore (reads only) or plan"),
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
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct CellView {
    /// Helper calls this cell made, in call order -- the lane while they run
    /// and the `HELPERS` inspector section afterwards read this one field.
    pub helpers: Vec<crate::helpers::HelperRecord>,
    /// Corrected source for an executed pane-edit; display only, never another model message.
    pub executed_source: Option<String>,
    /// The one line the model wrote about what this cell is for, drawn above
    /// the cell (`docs/product/pane/legibility.md` §2). `None` for a cell
    /// whose model said nothing and for every rollout row written before the
    /// field existed; the screen then falls back to what it drew before.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Who authored the frame this view shows.
    ///
    /// The screen may not imply that a direct provider call was written as
    /// JavaScript by the model (`tool-abi.md` §19): the source of a lowered
    /// frame is pane's spelling of the call, and labelling it the model's own
    /// would misreport what happened. A rollout written before this field
    /// existed deserializes as an authored cell, which is what it was.
    pub origin: crate::abi::Origin,
    pub repaired_from: Option<u64>,
    /// Local before/after file diff. Never included in model context.
    pub changes: Option<String>,
    /// The handle table as this cell ended, already rendered by the one
    /// renderer. `None` for a cell the notebook never saw run -- a resumed
    /// session's earlier cells came from the rollout file.
    pub table: Option<String>,
    /// Already bounded by the runtime; display only, never an extra model message.
    pub stdout: Option<String>,
    /// A non-terminal structured value returned for notebook inspection.
    pub output: Option<String>,
    /// Recorded tool outcomes, not calls inferred from generated source.
    pub execution: Option<String>,
    /// Host call count from the runtime record; never inferred from display lines.
    pub call_count: Option<usize>,
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
    /// The task capsule as it stood when this cell ended — goal, state,
    /// verified facts, risks and next action (`runtime::capsule`). Display
    /// and rollout state; the model receives it through the result block,
    /// never through this field. Absent on rows written before it existed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capsule: Option<serde_json::Value>,
}

/// A throw's class, message and position inside the model's own program --
/// `runtime-contract.md` §5's first two items, and nothing from inside the
/// runtime. The position is optional because the runtime could not always
/// attribute a throw to a line of the model's program.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
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
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Counted::Gateway => "reported",
            Counted::Estimated => "estimated",
            Counted::Mixed => "part estimated",
        }
    }
}

/// Cumulative task spend and its provenance. It deliberately has no cap.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskTokens {
    pub used: u64,
    /// Parent task turns only. `used - parent_used` is never needed to infer
    /// helper spend because the complete helper breakdown is carried below.
    pub parent_used: u64,
    pub helpers: HelperTokens,
    pub counted: Counted,
}

/// Known helper usage and the coverage required to interpret it honestly.
/// Missing provider usage contributes no invented tokens.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HelperTokens {
    pub calls: u32,
    pub usage_known_calls: u32,
    pub used: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub requests: u32,
    pub reported_requests: u32,
    pub cache_read_input_tokens: u64,
    pub cache_creation_input_tokens: u64,
    pub cache_read_reported_requests: u32,
    pub cache_creation_reported_requests: u32,
    pub models: Vec<HelperModelTokens>,
}

impl HelperTokens {
    pub fn complete(&self) -> bool {
        self.usage_known_calls == self.calls
            && self.reported_requests == self.requests
            && self.cache_read_reported_requests == self.reported_requests
            && self.cache_creation_reported_requests == self.reported_requests
    }
}

/// One helper model's contribution to [`HelperTokens`]. The model name is
/// provider configuration, not a price tier: Pane reports what ran and never
/// infers a rate from it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HelperModelTokens {
    pub model: String,
    pub calls: u32,
    pub usage_known_calls: u32,
    pub used: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub requests: u32,
    pub reported_requests: u32,
    pub cache_read_input_tokens: u64,
    pub cache_creation_input_tokens: u64,
    pub cache_read_reported_requests: u32,
    pub cache_creation_reported_requests: u32,
}

impl HelperModelTokens {
    pub fn complete(&self) -> bool {
        self.usage_known_calls == self.calls
            && self.reported_requests == self.requests
            && self.cache_read_reported_requests == self.reported_requests
            && self.cache_creation_reported_requests == self.reported_requests
    }
}

/// Occupancy of the most recent (or currently assembling) provider request.
/// This is intentionally separate from [`TaskTokens`], which accumulates the
/// cost of every request made for the task.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ContextTokens {
    pub used: u64,
    pub cap: Option<u64>,
    /// Where `cap` came from. The meter draws a percentage only against a
    /// figure somebody measured -- see [`crate::models::WindowSource`].
    pub cap_source: crate::models::WindowSource,
    pub counted: Counted,
}

/// The supervisor's own sidebar line -- `docs/product/pane/supervisor.md` §4
/// and §5: a nudge's own reason, a look that ran and did not intervene, a look
/// that produced no answer at all, or off because no model is configured or
/// the switch is off.
///
/// [`SupervisorStatus::LookFailed`] is its own state because §3 records an
/// unanswered look **as such**: it answers *not intervene* like a healthy
/// look, so folding the two together makes a supervisor that fails every
/// request -- and spends one every `every` cells -- indistinguishable from one
/// that is watching.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SupervisorStatus {
    Nudged(String),
    LookedNoNudge,
    LookFailed(String),
    Off,
}

/// What the session knows about the conversation beyond the messages
/// themselves: one view per assistant cell, in cell order, the task's token
/// total, and the supervisor's latest status.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Notebook {
    /// The pushed Scout running before the task model's first turn. This is
    /// presentation-only and is never persisted as a model-authored cell.
    pub preflight: Option<HelperRecord>,
    pub inbox_depth: usize,
    pub batches_delivered: u64,
    pub handlers: Vec<crate::runtime::handlers::HandlerInfo>,
    pub requests: Vec<crate::telemetry::RequestMeasurement>,
    pub cells: Vec<CellView>,
    pub tokens: Option<TaskTokens>,
    pub context: Option<ContextTokens>,
    pub supervisor: Option<SupervisorStatus>,
    /// The decision model's summary line for this task
    /// (`decide::summary_line`), `None` only when no decision model is
    /// configured at all.
    pub decision: Option<String>,
}

pub fn handlers_panel(handlers: &[crate::runtime::handlers::HandlerInfo]) -> Panel {
    let mut panel = Panel::text(
        "Standing handlers",
        if handlers.is_empty() {
            "No handlers in this task."
        } else {
            "Task-scoped · /handlers off <name>"
        },
    );
    for h in handlers {
        panel.rows.push(PanelRow {
            text: format!(
                "{} · {} · {} runs · {} drained{}",
                h.name,
                if h.active { "active" } else { "stale" },
                h.runs,
                h.drained,
                h.error
                    .as_ref()
                    .map(|e| format!(" · {e}"))
                    .unwrap_or_default()
            ),
            command: h.active.then(|| format!("/handlers off {}", h.name)),
        });
    }
    panel
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
    let _ = render_screen_with_geometry(frame, conversation, served_by, handles, notebook, state);
}

pub(crate) fn render_screen_with_geometry(
    frame: &mut Frame,
    conversation: &Conversation,
    served_by: &ServedBy,
    handles: &HandleTable,
    notebook: &Notebook,
    state: &ScreenState,
) -> ScreenGeometry {
    let mut geometry = ScreenGeometry::default();
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
            &mut geometry,
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
    if let Some(inspection) = &state.inspection {
        inspection::render(
            frame,
            regions.transcript,
            conversation,
            notebook,
            inspection,
        );
    }
    if let Some(panel) = &state.panel {
        frame.render_widget(Clear, regions.transcript);
        frame.render_widget(
            Block::default().style(Style::default().fg(Color::White).bg(Color::Reset)),
            regions.transcript,
        );
        geometry.panel = controls::render_panel(frame, regions.transcript, panel, state.theme);
    }
    ribbon::activity(frame, regions.activity, state);
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
    let matches: Vec<Line> = completions_shown(state)
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
    // A masked prompt puts the caret after the last bullet, from the mask
    // alone: the entered text is not available to this function.
    let cursor = match state.secret_prompt.as_ref() {
        Some(prompt) => {
            let mask = prompt.mask();
            Some(composer_cursor(
                &mask,
                mask.len(),
                regions.input.width.saturating_sub(2),
            ))
        }
        None => state.cursor.map(|offset| {
            composer_cursor(&state.input, offset, regions.input.width.saturating_sub(2))
        }),
    };
    let skip = cursor
        .map(|(row, _)| row.saturating_sub(visible.saturating_sub(1)))
        .unwrap_or_else(|| input_lines.len().saturating_sub(visible));
    frame.render_widget(
        Block::default().style(Style::default().bg(state.theme.dock())),
        regions.input,
    );
    if regions.input.height > 0 {
        let width = usize::from(regions.input.width);
        // The masked prompt's title rides the composer's own top rule: it is
        // modal, so it belongs where the keyboard now goes rather than in the
        // notice line a task could overwrite.
        let top = match state.secret_prompt.as_ref() {
            Some(prompt) => {
                let title = format!("─ {} ", prompt.title());
                let filled = title.chars().count();
                format!("{title}{}", "─".repeat(width.saturating_sub(filled)))
            }
            None => "─".repeat(width),
        };
        for (y, rule) in [
            (regions.input.y, top),
            (regions.input.bottom() - 1, "─".repeat(width)),
        ] {
            frame.render_widget(
                Paragraph::new(rule).style(Style::default().fg(ACCENT).bg(state.theme.dock())),
                Rect::new(regions.input.x, y, regions.input.width, 1),
            );
        }
    }
    if regions.input.height > 2 {
        // Text column 0 sits two cells in, where the cursor is placed below;
        // recorded from the same arithmetic so a click lands on the character
        // the caret would.
        geometry.record_composer(
            Rect::new(
                regions.input.x + 2,
                regions.input.y + 1,
                regions.input.width.saturating_sub(3),
                regions.input.height - 2,
            ),
            skip,
        );
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
        None if served_by.is_known() => "Glasshouse routed",
        None => NOT_CONNECTED,
    };
    let width = usize::from(regions.status.width);
    // The request mode, the rung, and the effort, in that order: what this
    // request may do, how often you are asked, how hard the model thinks.
    // The rung is here rather than in the sidebar because a person in `full`
    // must never be able to forget it, and the sidebar can be hidden.
    let mode = format!(
        "{} · {} · effort {}",
        state.mode.name(),
        state.permissions.rung().name(),
        state.effort.name()
    );
    let identity = format!(" {} · {}", abbreviate(model, 28), abbreviate(project, 24));
    let posture_head = format!(
        " sandbox {} · net:{}",
        abbreviate(sandbox, 16),
        abbreviate(network, 8)
    );
    // **Only the released state is news.** Captured is the default and a
    // permanent "· mouse" would be furniture, the same objection the scroll
    // indicator answers. Released must be visible, or dead clicks read as a
    // broken TUI -- and it is withheld below 90 columns because the context
    // reading owns that row's right edge and a longer left half would collapse
    // it away (`tui_live::telemetry_and_motion_are_local_controls_with_real_
    // response_usage` pins that priority).
    let mouse_mark = if state.mouse_off && width >= 90 {
        " · mouse off"
    } else {
        ""
    };
    let posture = format!("{posture_head}{mouse_mark}");
    let spent = notebook
        .tokens
        .as_ref()
        .filter(|_| width >= 78)
        .map(|tokens| {
            let scopes = if tokens.helpers.calls == 0 {
                format!("spent {}", compact_tokens(tokens.used))
            } else {
                format!(
                    "spent {} · parent {} + helpers {}{}",
                    compact_tokens(tokens.used),
                    compact_tokens(tokens.parent_used),
                    compact_tokens(tokens.helpers.used),
                    if tokens.helpers.complete() {
                        ""
                    } else {
                        " partial"
                    }
                )
            };
            format!("{scopes} · {}", tokens.counted.as_str())
        });
    let context = notebook.context.map(|tokens| {
        context_summary(
            tokens,
            if width >= 160 { 12 } else { 7 },
            state.animation_frame,
            matches!(state.activity, Activity::Thinking | Activity::Streaming),
        )
    });
    // Which status row carries the mode as its right half, if any: the field
    // is clickable only where it is actually drawn, and these three branches
    // draw it in different places (`hit.rs`).
    let mode_at: Option<usize> = if state.status_line == StatusLine::Compact {
        context.is_none().then_some(0)
    } else if width < 140 {
        if width >= 100 {
            Some(0)
        } else {
            context.is_none().then_some(1)
        }
    } else {
        Some(0)
    };
    let identity_model = abbreviate(model, 28);
    // Cloned before the rows consume them: the recorder below needs the same
    // left halves the rows were laid out with.
    let identity_for_width = identity.clone();
    let posture_for_width = posture.clone();
    let status = if state.status_line == StatusLine::Compact {
        vec![footer_row(
            identity,
            context.unwrap_or(mode.clone()),
            width,
            ACCENT,
        )]
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
                context.clone().unwrap_or_else(|| {
                    if width < 100 {
                        mode.clone()
                    } else {
                        String::new()
                    }
                }),
                width,
                ACCENT,
            ),
            footer_row(
                format!(" {connection}"),
                spent.unwrap_or_else(|| {
                    "PgUp/PgDn chat · /cells inspect · /mouse frees drag-select".into()
                }),
                width,
                MUTED,
            ),
        ]
    } else {
        vec![
            footer_row(identity, mode.clone(), width, ACCENT),
            footer_row(
                format!("{posture} · {connection}"),
                match (context, spent) {
                    (Some(context), Some(spent)) => format!("{context} · {spent}"),
                    (Some(context), None) => context,
                    (None, Some(spent)) => spent,
                    (None, None) => {
                        "PgUp/PgDn chat · /cells inspect · /mouse frees drag-select".into()
                    }
                },
                width,
                ACCENT,
            ),
        ]
    };
    // Recorded from the same strings and the same arithmetic `footer_row`
    // lays the row out with, so what is clickable is what is on screen.
    if regions.status.height > 0 && !identity_model.is_empty() {
        let model_width = Line::from(identity_model.as_str())
            .width()
            .min(width.saturating_sub(1));
        geometry.record_status(
            Rect::new(
                regions.status.x + 1,
                regions.status.y,
                model_width as u16,
                1,
            ),
            StatusField::Model,
        );
    }
    if let Some(row) =
        mode_at.filter(|row| u16::try_from(*row).is_ok_and(|row| row < regions.status.height))
    {
        let left = match (state.status_line == StatusLine::Compact, width < 140, row) {
            (true, _, _) => identity_for_width.clone(),
            (false, true, 1) => posture_for_width.clone(),
            _ => identity_for_width.clone(),
        };
        if let Some((x, span)) = footer_right_span(&left, &mode, width) {
            geometry.record_status(
                Rect::new(regions.status.x + x, regions.status.y + row as u16, span, 1),
                StatusField::Mode,
            );
        }
    }
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
    geometry
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
    notebook_lines(
        conversation,
        handles,
        notebook,
        &[],
        false,
        false,
        98,
        0,
        &mut Vec::new(),
    )
    .len()
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
            Role::User if is_tool_feedback(message) => {}
            Role::User => after_return = false,
        }
    }
    cells
}

pub fn conversation_rows(
    conversation: &Conversation,
    handles: &HandleTable,
    notebook: &Notebook,
    state: &ScreenState,
    width: u16,
) -> usize {
    conversation_lines(
        conversation,
        handles,
        notebook,
        state,
        width,
        &mut Vec::new(),
    )
    .len()
}

fn conversation_lines(
    conversation: &Conversation,
    handles: &HandleTable,
    notebook: &Notebook,
    state: &ScreenState,
    width: u16,
    headers: &mut Vec<(usize, usize)>,
) -> Vec<Line<'static>> {
    let mut content = notebook_lines(
        conversation,
        handles,
        notebook,
        &state.history,
        state.compact,
        state.pretty,
        usize::from(width.saturating_sub(2)),
        if state.reduced_motion {
            0
        } else {
            state.animation_frame
        },
        headers,
    );
    if let Some(raw_partial) = state.streaming_text.as_deref() {
        let visible = streaming_message_text(raw_partial);
        let partial = visible.as_str();
        turn_header(
            &mut content,
            format!(
                "PANE / RECEIVING  {} · {} bytes",
                Activity::Streaming.indicator(state.animation_frame),
                raw_partial.len()
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
                usize::from(width.saturating_sub(2)),
            ));
        }
    }
    if let Some(input) = &state.streaming_tool_input {
        turn_header(
            &mut content,
            format!(
                "CELL {} · preparing · nothing has run",
                cell_ordinal(conversation, notebook) + 1
            ),
            Color::LightCyan,
        );
        content.push(Line::styled(
            format!(
                "Receiving program · {} bytes · waiting for execution handoff",
                input.len()
            ),
            Style::default().fg(MUTED),
        ));
        if !state.compact
            && let Ok(value) = serde_json::from_str::<serde_json::Value>(input)
            && let Some(code) = value.get("code").and_then(|value| value.as_str())
        {
            content.extend(markdown::code(code));
        }
    }
    push_recap(&mut content, state.recap.as_ref());
    let mut active = false;
    for line in &mut content {
        let mut cell_header = false;
        if let Some(first) = line.spans.first_mut() {
            if first.content.starts_with("╭─ ") {
                cell_header = first.content.contains("Cell ")
                    || first.content.contains("CELL ")
                    || first.content.contains("Action failed")
                    || first.content.contains("PANE / CODE");
                if state.activity == Activity::Executing
                    && first.content.contains("◇ Cell preparing · nothing has run")
                {
                    first.content = first
                        .content
                        .replace(
                            "◇ Cell preparing · nothing has run",
                            &format!(
                                "{} Cell running",
                                Activity::Executing.indicator(state.animation_frame)
                            ),
                        )
                        .into();
                }
                first.content = first.content.trim_start_matches("╭─ ").to_string().into();
                active = true;
            } else if first.content == "╰─" {
                *line = Line::default();
                active = false;
            }
        }
        if active {
            line.style = line.style.bg(if cell_header {
                state.theme.dock()
            } else {
                state.theme.backlight()
            });
        }
    }
    wrap_lines(content, width.saturating_sub(2))
}

fn render_conversation(
    frame: &mut Frame,
    area: Rect,
    conversation: &Conversation,
    handles: &HandleTable,
    notebook: &Notebook,
    state: &ScreenState,
    geometry: &mut ScreenGeometry,
) {
    let mut headers = Vec::new();
    let lines = conversation_lines(
        conversation,
        handles,
        notebook,
        state,
        area.width,
        &mut headers,
    );
    let start = lines
        .len()
        .saturating_sub(usize::from(area.height))
        .saturating_sub(state.scrollback);
    let total_rows = lines.len();
    // The same `lines` and the same `start` the draw below uses, so a header
    // is clickable exactly where it is drawn and nowhere else (`hit.rs`).
    for (index, cell) in headers {
        if let Some(offset) = index.checked_sub(start)
            && offset < usize::from(area.height)
        {
            geometry.record_cell(
                Rect::new(area.x, area.y + offset as u16, area.width, 1),
                cell,
            );
        }
    }
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
    render_scrollbar(frame, area, total_rows, start, state.scrolling);
}

/// Preserve the viewed rows as new content arrives; zero remains live-follow.
pub fn anchor_scrollback(scroll: usize, previous: usize, current: usize, height: usize) -> usize {
    if scroll == 0 {
        return 0;
    }
    let adjusted = if current >= previous {
        scroll.saturating_add(current - previous)
    } else {
        scroll.saturating_sub(previous - current)
    };
    adjusted.min(current.saturating_sub(height))
}

/// Pushes a turn's header and answers **which line it is**, so a caller that
/// knows the header belongs to a cell can record that line as clickable
/// without searching for it afterwards (`hit.rs`: the map is built by the
/// draw). Callers that have nothing to record ignore the index.
fn turn_header(lines: &mut Vec<Line<'static>>, label: String, color: Color) -> usize {
    if !lines.is_empty() {
        lines.push(Line::styled("╰─", Style::default().fg(MUTED)));
        lines.push(Line::from(""));
    }
    lines.push(Line::styled(
        format!("╭─ {label}"),
        Style::default().fg(color).add_modifier(Modifier::BOLD),
    ));
    lines.len() - 1
}

/// The recap's own header. It names the author and denies the mistake,
/// because a recap read as the assistant's answer is worse than no recap:
/// the answer is the model's, this is a cheap helper's summary of it.
const RECAP_LABEL: &str = "RECAP · a helper's summary, not the assistant";

/// The session's closing output when `[helpers] completion = "recap"` asked
/// for one: what the session did, then the `Next:` line the preamble asks
/// for, under the transcript they summarise.
///
/// **Nothing at all unless a recap was asked for and came back.** A missing
/// or failed record renders no header, no reason and no blank frame -- the
/// screen is the one the silent default already draws, because a recap must
/// never replace or delay the answer above it.
///
/// It is drawn in [`MUTED`] with the helper lane's indent rather than in the
/// model's own prose style, and nothing here adds a tick, a colour or a word
/// that would claim more than the sentences themselves do.
fn push_recap(lines: &mut Vec<Line<'static>>, recap: Option<&HelperRecord>) {
    let Some(record) = recap.filter(|record| record.outcome.ok) else {
        return;
    };
    let text = record.outcome.text.trim();
    if text.is_empty() {
        return;
    }
    turn_header(lines, RECAP_LABEL.to_string(), MUTED);
    for line in text.lines() {
        lines.push(Line::styled(
            format!("  {}", line.trim_end()),
            Style::default().fg(MUTED),
        ));
    }
    lines.push(Line::styled("╰─", Style::default().fg(MUTED)));
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
#[allow(clippy::too_many_arguments)]
fn notebook_lines(
    conversation: &Conversation,
    handles: &HandleTable,
    notebook: &Notebook,
    notes: &[HistoryNote],
    compact: bool,
    pretty: bool,
    width: usize,
    tick: usize,
    // Line index -> the cell that line's header belongs to, filled as the
    // headers are pushed. A caller with no use for it passes a scratch vector.
    headers: &mut Vec<(usize, usize)>,
) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    let mut messages = conversation.messages.iter();
    let mut next_note = 0usize;

    history::push_notes(&mut lines, notes, &mut next_note, 0);
    if let Some(task) = messages.next() {
        turn_header(&mut lines, "USER".into(), ACCENT);
        push_text_region(&mut lines, &message_text(task));
    }
    let total_cells = cell_ordinal(conversation, notebook);

    let mut cell = 0usize;
    let mut answered = false;
    let mut after_return = false;
    for (index, message) in messages.enumerate() {
        // At a turn boundary only: a note drawn between a cell and its
        // feedback would split the cell's block.
        if message.role == Role::User && !is_tool_feedback(message) {
            history::push_notes(&mut lines, notes, &mut next_note, index + 1);
        }
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
                if matches!(message_program(message), Extracted::Prose)
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
                    match message_program(message) {
                        Extracted::Program(source) | Extracted::Edit(source) => {
                            let repairing = matches!(message_program(message), Extracted::Edit(_));
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
                                    "✓ Cell executed"
                                } else {
                                    "◇ Cell preparing · nothing has run"
                                };
                                headers.push((
                                    turn_header(
                                        &mut lines,
                                        format!("{label}  · {cell}{}", helper_fold(view)),
                                        if failed { Color::Red } else { ACCENT },
                                    ),
                                    cell,
                                ));
                                // The one line the model wrote about what this
                                // cell is for, directly under its header and
                                // above the record of what ran — the order is
                                // the claim: intention first, evidence below
                                // (`legibility.md` §2, §8 rule 2).
                                if let Some(description) = view
                                    .and_then(|v| v.description.as_deref())
                                    .map(str::trim)
                                    .filter(|description| !description.is_empty())
                                {
                                    lines.push(Line::styled(
                                        description.to_string(),
                                        Style::default().fg(Color::White),
                                    ));
                                }
                                push_helper_lane(&mut lines, view, tick, width);
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
                                        let count = view.and_then(|v| v.call_count).unwrap_or(0);
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
                                // The model's own sentence about why it
                                // stopped here. It exists on 40 of the 123
                                // views of the corpus behind `legibility.md`
                                // and the default screen drew none of them:
                                // the expanded path had it, the compact path
                                // returned before reaching it.
                                if let Some(reason) = view.and_then(|v| v.yield_reason.as_deref()) {
                                    lines.push(Line::styled(
                                        format!("yielded: {reason}"),
                                        Style::default().fg(MUTED),
                                    ));
                                }
                                lines.push(Line::styled(
                                    format!(
                                        "Ctrl-O · code and results · /cell {cell} or click this header"
                                    ),
                                    Style::default().fg(MUTED),
                                ));
                            }
                        }
                        Extracted::Invalid(error) => {
                            turn_header(
                                &mut lines,
                                "Response format rejected".into(),
                                Color::Yellow,
                            );
                            push_text_region(&mut lines, &format!("{error} Nothing ran."));
                        }
                        Extracted::TwoBlocks => {
                            turn_header(
                                &mut lines,
                                "Response format rejected".into(),
                                Color::Yellow,
                            );
                            push_text_region(
                                &mut lines,
                                "Ambiguous repair blocks · nothing ran. Ctrl-O shows the response.",
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
                                "Expected complete Pane code · nothing ran. Ctrl-O shows the response.",
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
                    if let Some(output) = view.and_then(|v| v.output.as_deref()) {
                        turn_header(&mut lines, "OUTPUT".into(), MUTED);
                        lines.extend(markdown::render(&pretty_json(output), width));
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
                    message_program(message),
                    Extracted::Program(_) | Extracted::Edit(_)
                ) {
                    "PANE / CODE"
                } else if matches!(
                    message_program(message),
                    Extracted::TwoBlocks | Extracted::Invalid(_)
                ) {
                    "PANE / NOT EXECUTED: invalid protocol"
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
                headers.push((
                    turn_header(
                        &mut lines,
                        format!("{role}  [{cell}] in{execution}{}", helper_fold(view)),
                        ACCENT,
                    ),
                    cell,
                ));
                push_helper_lane(&mut lines, view, tick, width);
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
                    match message_program(message) {
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
                headers.push((
                    turn_header(&mut lines, format!("TOOL / PREVIEW  [{cell}] out"), MUTED),
                    cell,
                ));
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
                if let Some(output) = view.and_then(|view| view.output.as_deref()) {
                    headers.push((
                        turn_header(&mut lines, format!("OUTPUT  [{cell}]"), MUTED),
                        cell,
                    ));
                    lines.extend(markdown::render(&pretty_json(output), width));
                }
                if let Some(changes) = view.and_then(|v| v.changes.as_deref()) {
                    push_changes(&mut lines, changes, compact);
                }
                if let Some(reason) = view.and_then(|view| view.yield_reason.as_deref()) {
                    lines.push(Line::from(format!("yielded: {reason}")));
                }

                if let Some(error) = view.and_then(|view| view.error.as_ref()) {
                    headers.push((
                        turn_header(&mut lines, format!("ERROR  [{cell}] error"), Color::Red),
                        cell,
                    ));
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
            Role::User if is_tool_feedback(message) => {
                answered = false;
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

    // Preflight belongs to the newest submitted request, after all completed
    // history. Keeping it at the tail also keeps it in the followed viewport
    // during a later task in the same session.
    if let Some(record) = notebook.preflight.as_ref() {
        turn_header(&mut lines, "PREFLIGHT · SCOUT".into(), MUTED);
        lines.push(helper_lane(record, tick, width));
    }
    history::push_notes(&mut lines, notes, &mut next_note, usize::MAX);

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
    if message
        .content
        .iter()
        .any(|block| matches!(block, ContentBlock::ToolUse { .. }))
    {
        return (text, String::new());
    }
    if !matches!(
        extract_program(&text),
        Extracted::Program(_) | Extracted::Edit(_)
    ) {
        return (String::new(), String::new());
    }
    let lines: Vec<_> = text.lines().collect();
    let mut before = Vec::new();
    let mut after = Vec::new();
    let mut saw_program = false;
    let mut i = 0;
    while i < lines.len() {
        if let Some(info) = lines[i].strip_prefix("```") {
            let mut end = i + 1;
            while end < lines.len() && lines[end] != "```" {
                end += 1;
            }
            if matches!(info.trim(), "pane" | "pane-edit") {
                saw_program = true;
            } else {
                let target = if saw_program { &mut after } else { &mut before };
                target.extend_from_slice(&lines[i..(end + 1).min(lines.len())]);
            }
            i = end + 1;
        } else {
            if saw_program {
                after.push(lines[i]);
            } else {
                before.push(lines[i]);
            }
            i += 1;
        }
    }
    (before.join("\n"), after.join("\n"))
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

fn message_program(message: &Message) -> Extracted {
    let calls: Vec<_> = message
        .content
        .iter()
        .filter_map(|block| match block {
            ContentBlock::ToolUse { name, input, .. } => Some((name, input)),
            _ => None,
        })
        .collect();
    if calls.is_empty() {
        return extract_program(&message_text(message));
    }
    if calls.len() != 1 {
        return Extracted::Invalid("Multiple native cell calls; no cell ran.".into());
    }
    let (name, input) = calls[0];
    if name != "execute_cell" {
        return Extracted::Invalid(format!("Unknown native tool {name}"));
    }
    match input.get("code").and_then(|value| value.as_str()) {
        Some(code) => Extracted::Program(code.to_string()),
        None => Extracted::Invalid("Cell call has no valid source.".into()),
    }
}

fn is_tool_feedback(message: &Message) -> bool {
    message
        .content
        .iter()
        .any(|block| matches!(block, ContentBlock::ToolResult { .. }))
}

fn input_region(message: &Message) -> String {
    let text = message_text(message);
    match message_program(message) {
        Extracted::Program(source) | Extracted::Edit(source) => source,
        Extracted::Prose | Extracted::TwoBlocks | Extracted::Invalid(_) => text,
    }
}

/// Hide framing even when its last line arrives over several stream chunks.
fn streaming_message_text(text: &str) -> String {
    if let Some((prefix, last)) = text.rsplit_once('\n')
        && !last.is_empty()
        && crate::prompt::COMPLETE_MARKER.starts_with(last)
    {
        let framed = format!("{prefix}\n{}", crate::prompt::COMPLETE_MARKER);
        if let Some(visible) = crate::prompt::completion_text(&framed) {
            return visible;
        }
    }
    if let Some(completed) = crate::prompt::completion_text(text) {
        return completed;
    }
    text.to_string()
}

fn message_text(message: &Message) -> String {
    let text = message
        .content
        .iter()
        .filter_map(|block| match block {
            ContentBlock::Text(text) | ContentBlock::ToolResult { content: text, .. } => {
                Some(text.as_str())
            }
            ContentBlock::ToolUse { .. } => None,
            ContentBlock::Image { .. } => Some("\n[image attachment]\n"),
        })
        .collect::<Vec<_>>()
        .join("");
    if message.role == Role::Assistant {
        crate::prompt::completion_text(&text).unwrap_or(text)
    } else {
        text
    }
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

/// §4 and §5's fixed lines, and nothing else -- the sidebar shows this one
/// line under the task-spend line, whatever `served_by` says.
fn supervisor_line(status: &SupervisorStatus) -> String {
    match status {
        SupervisorStatus::Nudged(reason) => format!("supervisor: {reason}"),
        SupervisorStatus::LookedNoNudge => "supervisor: looked, no nudge".to_string(),
        SupervisorStatus::LookFailed(reason) => format!("supervisor: FAILED {reason}"),
        SupervisorStatus::Off => "supervisor: off (no model)".to_string(),
    }
}
