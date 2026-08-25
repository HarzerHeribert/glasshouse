//! Rendering for the session shell.
//!
//! [`render`] is a pure function of a [`ShellState`] and a [`Frame`]: it reads,
//! never mutates, and never blocks, so [`super::run`] can redraw only when
//! [`super::Action`] says something changed and the tests below can drive it
//! with [`ratatui::backend::TestBackend`] instead of a real terminal.
//!
//! Nothing here computes a size by subtraction. That is the usual way a "must
//! not panic on a tiny terminal" requirement gets violated, and the tests at
//! the bottom render at 1x1 to keep it honest.

use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};

use crate::config::Layer;
use crate::session::{SessionDisposition, SessionRecord};

use super::state::{
    Mode, Overlay, SettingsPathInputView, SettingsSection, SettingsState, ShellState,
};

/// Draw the shell.
pub fn render(state: &ShellState, frame: &mut Frame) {
    let area = frame.area();
    let [title_area, root_area, bar_area, viewport_area, footer_area] = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Min(0),
            Constraint::Length(1),
        ])
        .areas(area);

    render_title(state, frame, title_area);
    render_root(state, frame, root_area);
    render_session_bar(state, frame, bar_area);
    render_viewport(state, frame, viewport_area);
    render_footer(state, frame, footer_area);

    match state.overlay() {
        Some(Overlay::Overview) => render_overview(state, frame, area),
        Some(Overlay::Settings) => render_settings(state, frame, area),
        None => {}
    }
}

/// The project's name and the session currently presented.
fn render_title(state: &ShellState, frame: &mut Frame, area: Rect) {
    let mut spans = vec![
        Span::styled(
            format!("glasshouse {}", state.version()),
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw("  "),
        Span::styled(
            state.project_name().to_owned(),
            Style::default().add_modifier(Modifier::BOLD),
        ),
    ];
    if let Some(session) = state.active_session() {
        spans.push(Span::raw("  ·  "));
        spans.push(Span::styled(
            format!("{} {}", session.harness, short_id(session)),
            Style::default().fg(Color::Yellow),
        ));
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

/// The active canonical project root, on its own line, on every frame.
///
/// This is the value the entire isolation model rests on — which project's
/// memory, state, and sessions the user is looking at — so it gets a dedicated
/// line rather than being tucked into a corner. When the line is too narrow the
/// *head* is dropped, not the tail: `…/work/glasshouse` still identifies the
/// project, while `/Users/someone/very/long/…` does not.
fn render_root(state: &ShellState, frame: &mut Frame, area: Rect) {
    let root = state.project_root().display().to_string();
    let label = "root ";
    let available = usize::from(area.width).saturating_sub(label.chars().count());
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(label, Style::default().fg(Color::DarkGray)),
            Span::styled(
                truncate_start(&root, available),
                Style::default().fg(Color::White),
            ),
        ])),
        area,
    );
}

/// Every session known to the project, as a bar of tabs.
fn render_session_bar(state: &ShellState, frame: &mut Frame, area: Rect) {
    if state.sessions().is_empty() {
        frame.render_widget(
            Paragraph::new(Span::styled(
                "no sessions yet — start one with `glasshouse launch`",
                Style::default().fg(Color::DarkGray),
            )),
            area,
        );
        return;
    }

    let mut spans = Vec::new();
    for (index, session) in state.sessions().iter().enumerate() {
        let active = index == state.selected_index();
        let style = if active {
            Style::default()
                .fg(Color::Black)
                .bg(Color::Cyan)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::Gray)
        };
        spans.push(Span::styled(
            format!(" {} {} ", index + 1, session.harness),
            style,
        ));
        spans.push(Span::raw(" "));
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

/// The region reserved for the active session's terminal.
///
/// Once the run loop has handed over any scrollback via
/// [`ShellState::set_viewport`], that text fills the space — the last lines
/// that fit, most recent at the bottom. Raw bytes, not a rendered terminal:
/// Glasshouse does not emulate one yet (see
/// [`crate::session::runtime::Scrollback`]'s doc comment), so escape
/// sequences show up as themselves. Until then, or for a session that has
/// produced nothing yet, the placeholder below explains what will occupy the
/// space rather than faking a terminal that would suggest more is attached
/// than really is.
fn render_viewport(state: &ShellState, frame: &mut Frame, area: Rect) {
    let block = Block::default().borders(Borders::ALL).title(
        state
            .active_session()
            .map(|session| format!(" {} ", session.harness))
            .unwrap_or_else(|| " session ".to_owned()),
    );
    let inner = block.inner(area);
    frame.render_widget(block, area);

    if !state.viewport().is_empty() {
        let all: Vec<&str> = state.viewport().lines().collect();
        let start = all.len().saturating_sub(usize::from(inner.height));
        let lines: Vec<Line> = all[start..].iter().map(|line| Line::from(*line)).collect();
        frame.render_widget(Paragraph::new(lines), inner);
        return;
    }

    let lines = match state.active_session() {
        Some(session) => vec![
            Line::from(Span::styled(
                format!("session {}", session.id),
                Style::default().add_modifier(Modifier::BOLD),
            )),
            Line::from(format!("harness       {}", session.harness)),
            Line::from(format!("state         {}", session.lifecycle)),
            Line::from(format!("presented     {}", session.presentation)),
            Line::from(format!("role          {}", session.role)),
            Line::from(""),
            Line::from(Span::styled(
                "This viewport is reserved for the session's own terminal.",
                Style::default().fg(Color::DarkGray),
            )),
        ],
        None => vec![
            Line::from(Span::styled(
                "No session is active.",
                Style::default().add_modifier(Modifier::BOLD),
            )),
            Line::from(""),
            Line::from(Span::styled(
                "Run `glasshouse launch` to start one.",
                Style::default().fg(Color::DarkGray),
            )),
        ],
    };
    frame.render_widget(Paragraph::new(lines), inner);
}

/// The bottom status bar: which mode owns the keyboard, Glasshouse's own key
/// bindings, plus a note when the last key needs explaining.
///
/// All on one compact row. A note takes the right-hand side rather than
/// replacing the hints, so learning the keys and being told why one did nothing
/// are not mutually exclusive.
///
/// In session mode this is the *only* thing on screen that says how to get
/// back — see the design note: "A user who cannot see how to get out is the
/// failure this design exists to prevent." so the escape chord is shown here
/// on every frame session mode is active, not just the first.
fn render_footer(state: &ShellState, frame: &mut Frame, area: Rect) {
    let hint = match (state.mode(), state.overlay()) {
        (Mode::Session, _) => "SESSION MODE -- keys go to the session -- ctrl-] for glasshouse",
        (Mode::Control, Some(Overlay::Overview)) => "esc back to session   q quit",
        (Mode::Control, Some(Overlay::Settings)) => {
            "tab section   up/down move   space toggle   enter edit path   w save (user)   \
             W save (project)   esc close"
        }
        (Mode::Control, None) => "tab session   enter session   n new   o overview   q quit",
    };
    let mut spans = vec![Span::styled(hint, Style::default().fg(Color::DarkGray))];

    if let Some(status) = state.status() {
        spans.push(Span::raw("   "));
        spans.push(Span::styled(status, Style::default().fg(Color::Yellow)));
    }

    // Order is the whole mechanism: the bindings are written first, so when the
    // row is too narrow it is the note that gets clipped away. The bindings are
    // needed permanently and the note only once, so that is the right thing to
    // lose. An earlier version measured the remaining width and truncated the
    // note itself, which turned out to be an elaborate way of duplicating the
    // clipping Ratatui already does — and a mutation removing the measurement
    // changed nothing on screen, which is how it was found.
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

/// The session overview, drawn over the shell rather than replacing it.
///
/// Over, not instead of: the shell stays visible around the edges so it is
/// obvious the session is still there and still running, and that leaving the
/// overlay goes back to it.
fn render_overview(state: &ShellState, frame: &mut Frame, area: Rect) {
    let popup = centered(area, 80, 70);
    frame.render_widget(Clear, popup);

    let block = Block::default()
        .borders(Borders::ALL)
        .title(" sessions ")
        .style(Style::default().bg(Color::Black));
    let inner = block.inner(popup);
    frame.render_widget(block, popup);

    let mut lines = vec![Line::from(Span::styled(
        format!(
            "{:<14}  {:<12}  {:<10}  {:<12}  {}",
            "HARNESS", "STATE", "ROLE", "PRESENTED", "SESSION"
        ),
        Style::default().add_modifier(Modifier::BOLD),
    ))];

    if state.sessions().is_empty() {
        lines.push(Line::from(Span::styled(
            "This project has no recorded sessions.",
            Style::default().fg(Color::DarkGray),
        )));
    }

    for (index, session) in state.sessions().iter().enumerate() {
        let active = index == state.selected_index();
        let style = if active {
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default()
        };
        lines.push(Line::from(Span::styled(
            format!(
                "{:<14}  {:<12}  {:<10}  {:<12}  {}",
                session.harness,
                disposition_label(session),
                session.role,
                session.presentation,
                short_id(session),
            ),
            style,
        )));
    }

    frame.render_widget(Paragraph::new(lines), inner);
}

fn disposition_label(session: &SessionRecord) -> &'static str {
    match session.disposition() {
        SessionDisposition::Active => "active",
        SessionDisposition::Resumable => "resumable",
        SessionDisposition::Closed => "closed",
        SessionDisposition::Failed => "failed",
    }
}

fn short_id(session: &SessionRecord) -> String {
    session.id.as_str().chars().take(12).collect()
}

/// The Settings overlay: Harnesses and Integrations, drawn over the shell
/// exactly like the Overview — see its doc comment for why "over", not
/// "instead of".
///
/// Unlike the Overview, this overlay owns every key while open (see
/// `ShellState::handle_settings_key`), so nothing underneath it is reachable
/// while it is shown — there is no passthrough navigation to account for
/// here.
fn render_settings(state: &ShellState, frame: &mut Frame, area: Rect) {
    let Some(settings) = state.settings() else {
        return;
    };
    let popup = centered(area, 90, 80);
    frame.render_widget(Clear, popup);

    let block = Block::default()
        .borders(Borders::ALL)
        .title(" settings ")
        .style(Style::default().bg(Color::Black));
    let inner = block.inner(popup);
    frame.render_widget(block, popup);

    let bottom_height = if settings.path_input().is_some() {
        2
    } else if settings.confirming_project_write() {
        3
    } else {
        0
    };
    let regions = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Min(0),
            Constraint::Length(bottom_height),
        ])
        .split(inner);
    let tabs_area = regions[0];
    let list_area = regions[1];
    let bottom_area = regions[2];

    render_settings_tabs(settings, frame, tabs_area);
    match settings.section() {
        SettingsSection::Harnesses => render_harness_rows(settings, frame, list_area),
        SettingsSection::Integrations => render_integration_rows(settings, frame, list_area),
    }

    if let Some(input) = settings.path_input() {
        render_settings_path_input(&input, frame, bottom_area);
    } else if settings.confirming_project_write() {
        render_project_write_confirm(state, frame, bottom_area);
    }
}

fn render_settings_tabs(settings: &SettingsState, frame: &mut Frame, area: Rect) {
    let tab = |label: &str, active: bool| {
        let style = if active {
            Style::default()
                .fg(Color::Black)
                .bg(Color::Cyan)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::Gray)
        };
        Span::styled(format!(" {label} "), style)
    };
    let spans = vec![
        tab(
            "Harnesses",
            settings.section() == SettingsSection::Harnesses,
        ),
        Span::raw(" "),
        tab(
            "Integrations",
            settings.section() == SettingsSection::Integrations,
        ),
    ];
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

/// Text for a value's provenance — the design decision's "provenance is
/// shown, not inferred".
fn layer_label(layer: Layer) -> &'static str {
    match layer {
        Layer::Project => "(project)",
        Layer::User => "(user)",
        Layer::Default => "(default)",
    }
}

fn render_harness_rows(settings: &SettingsState, frame: &mut Frame, area: Rect) {
    let mut lines = Vec::new();
    if settings.harnesses().is_empty() {
        lines.push(Line::from(Span::styled(
            "No harnesses known.",
            Style::default().fg(Color::DarkGray),
        )));
    }
    for (index, row) in settings.harnesses().iter().enumerate() {
        let selected = index == settings.selected_harness();
        let cursor = if selected { "> " } else { "  " };
        let mut style = Style::default();
        if selected {
            style = style.add_modifier(Modifier::BOLD);
        }
        let detected = if row.detected {
            "detected"
        } else {
            "not detected"
        };
        let enabled = format!(
            "{} {}",
            if row.enabled { "enabled" } else { "disabled" },
            layer_label(row.enabled_layer)
        );
        let path = match (&row.executable, row.executable_layer) {
            (Some(path), Some(layer)) => format!("{} {}", path.display(), layer_label(layer)),
            _ => "no explicit path".to_owned(),
        };
        lines.push(Line::from(Span::styled(
            format!(
                "{cursor}{:<14} {detected:<13} {enabled:<22} {path}",
                row.id.display_name(),
            ),
            style,
        )));
    }
    frame.render_widget(Paragraph::new(lines), area);
}

fn render_integration_rows(settings: &SettingsState, frame: &mut Frame, area: Rect) {
    let mut lines = Vec::new();
    if settings.integrations().is_empty() {
        lines.push(Line::from(Span::styled(
            "No optional integrations known.",
            Style::default().fg(Color::DarkGray),
        )));
    }
    for (index, row) in settings.integrations().iter().enumerate() {
        let selected = index == settings.selected_integration();
        let cursor = if selected { "> " } else { "  " };
        let mut style = Style::default();
        if selected {
            style = style.add_modifier(Modifier::BOLD);
        }
        let detected = if row.detected {
            "detected"
        } else {
            "not detected"
        };
        lines.push(Line::from(Span::styled(
            format!(
                "{cursor}{:<14} {detected:<13} {}",
                row.id.display_name(),
                row.status
            ),
            style,
        )));
    }
    frame.render_widget(Paragraph::new(lines), area);
}

fn render_settings_path_input(input: &SettingsPathInputView<'_>, frame: &mut Frame, area: Rect) {
    let mut lines = vec![Line::from(format!(
        "Path to {} executable: {}_",
        input.harness_name, input.buffer
    ))];
    if let Some(error) = input.error {
        lines.push(Line::from(Span::styled(
            error.to_owned(),
            Style::default().fg(Color::Red),
        )));
    }
    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), area);
}

/// The design decision requires naming the exact path before a project-level
/// write, so this is not a generic "are you sure": it spells out
/// `<project root>/.glasshouse/config.toml` and says the file lands inside
/// the repository.
fn render_project_write_confirm(state: &ShellState, frame: &mut Frame, area: Rect) {
    let path = state.project_root().join(".glasshouse").join("config.toml");
    let lines = vec![
        Line::from(Span::styled(
            format!("Write project settings to {}?", path.display()),
            Style::default().add_modifier(Modifier::BOLD),
        )),
        Line::from("This file is inside your project repository. y/Enter confirms, Esc/n cancels."),
    ];
    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), area);
}

/// A rectangle covering `percent_x` by `percent_y` of `area`, centred.
///
/// Computed by multiplication and division rather than by subtracting a fixed
/// margin, so a terminal smaller than any assumed minimum simply produces a
/// small rectangle instead of an underflow.
fn centered(area: Rect, percent_x: u16, percent_y: u16) -> Rect {
    let width = area.width * percent_x / 100;
    let height = area.height * percent_y / 100;
    Rect {
        x: area.x + (area.width - width) / 2,
        y: area.y + (area.height - height) / 2,
        width,
        height,
    }
}

/// Keep the last `width` characters of `text`, marking what was dropped.
///
/// Counts characters, not bytes: a multi-byte path would otherwise be cut mid
/// character.
fn truncate_start(text: &str, width: usize) -> String {
    let length = text.chars().count();
    if length <= width {
        return text.to_owned();
    }
    if width <= 1 {
        return "…".chars().take(width).collect();
    }
    let skip = length - (width - 1);
    std::iter::once('…')
        .chain(text.chars().skip(skip))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::{
        SessionId, SessionLifecycle, SessionPresentation, SessionRecord, SessionRole,
    };
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    fn record(id: &str, harness: &str, lifecycle: SessionLifecycle) -> SessionRecord {
        SessionRecord {
            id: SessionId::new(id),
            project_id: "project".to_owned(),
            harness: harness.to_owned(),
            native_session_id: None,
            role: SessionRole::Normal,
            lifecycle,
            presentation: SessionPresentation::Embedded,
            created_at: 1_000,
            last_activity_at: 1_000,
        }
    }

    /// One session, for the cases that need a project with nothing to navigate
    /// between.
    fn lone_session() -> SessionRecord {
        record("only-one", "claude-code", SessionLifecycle::Running)
    }

    /// The bottom row of a rendered frame, trimmed.
    fn last_row(state: &ShellState, width: u16, height: u16) -> String {
        rendered(state, width, height)
            .lines()
            .last()
            .expect("a frame has rows")
            .trim_end()
            .to_owned()
    }

    fn sample() -> ShellState {
        ShellState::new(
            "glasshouse",
            "/Users/someone/projects/glasshouse",
            "0.1.0",
            vec![
                record("aaaaaaaaaaaa1", "claude-code", SessionLifecycle::Running),
                record("bbbbbbbbbbbb2", "codex", SessionLifecycle::Stopped),
            ],
        )
    }

    /// Render and flatten the buffer to text, so a test can assert on what the
    /// user would actually see rather than only that drawing did not panic.
    fn rendered(state: &ShellState, width: u16, height: u16) -> String {
        let backend = TestBackend::new(width, height);
        let mut terminal = Terminal::new(backend).expect("terminal");
        terminal
            .draw(|frame| render(state, frame))
            .expect("draw must not panic");
        let buffer = terminal.backend().buffer();
        (0..buffer.area.height)
            .map(|y| {
                (0..buffer.area.width)
                    .map(|x| buffer[(x, y)].symbol())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Phase 1: "Display the active canonical project root prominently in the
    /// TUI." Asserted against the rendered buffer, not against the state.
    #[test]
    fn the_project_root_is_displayed_on_every_frame() {
        let state = sample();
        let text = rendered(&state, 100, 24);
        assert!(
            text.contains("/Users/someone/projects/glasshouse"),
            "the canonical root must be on screen:\n{text}"
        );
        assert!(
            text.contains("glasshouse"),
            "the project name must be shown"
        );
    }

    /// It has to stay visible from every screen, including behind an overlay,
    /// or "prominently" would mean "until you open something".
    #[test]
    fn the_project_root_stays_visible_while_an_overlay_is_open() {
        let mut state = sample();
        state.handle_key(KeyEvent::new(KeyCode::Char('o'), KeyModifiers::NONE));
        let text = rendered(&state, 100, 24);
        assert!(
            text.contains("/Users/someone/projects/glasshouse"),
            "the root must survive an overlay:\n{text}"
        );
    }

    /// The row the root is drawn on, so an assertion about the root cannot be
    /// satisfied by the same word appearing in the title bar.
    fn root_row(state: &ShellState, width: u16, height: u16) -> String {
        rendered(state, width, height)
            .lines()
            .nth(1)
            .expect("the root occupies the second row")
            .trim_end()
            .to_owned()
    }

    /// A narrow terminal must keep the identifying tail of the path, not the
    /// useless head.
    ///
    /// Asserted against the root row alone. An earlier version searched the
    /// whole frame, which meant "glasshouse" matched the title bar and the test
    /// passed even when the path was truncated from the wrong end.
    #[test]
    fn a_narrow_terminal_keeps_the_end_of_the_project_root() {
        let row = root_row(&sample(), 28, 12);
        assert!(row.contains('…'), "truncation must be visible: `{row}`");
        assert!(
            row.ends_with("glasshouse"),
            "the tail identifies the project and must survive: `{row}`"
        );
        assert!(
            !row.contains("/Users/someone"),
            "the head should have been dropped, not the tail: `{row}`"
        );
    }

    /// A wide terminal shows the root untouched — otherwise the test above
    /// could be satisfied by always truncating.
    #[test]
    fn a_wide_terminal_shows_the_whole_project_root() {
        let row = root_row(&sample(), 120, 24);
        assert_eq!(row, "root /Users/someone/projects/glasshouse");
    }

    #[test]
    fn the_session_bar_lists_every_known_session() {
        let text = rendered(&sample(), 100, 24);
        assert!(
            text.contains("claude-code"),
            "missing first session:\n{text}"
        );
        assert!(text.contains("codex"), "missing second session:\n{text}");
    }

    #[test]
    fn an_empty_project_says_so_instead_of_showing_an_empty_bar() {
        let state = ShellState::new("p", "/p", "0.1.0", Vec::new());
        let text = rendered(&state, 80, 24);
        assert!(text.contains("no sessions yet"), "got:\n{text}");
        assert!(text.contains("No session is active"), "got:\n{text}");
    }

    #[test]
    fn the_overview_shows_detail_the_session_bar_has_no_room_for() {
        let mut state = sample();
        state.handle_key(KeyEvent::new(KeyCode::Char('o'), KeyModifiers::NONE));
        let text = rendered(&state, 100, 24);
        assert!(text.contains("HARNESS"), "overview header missing:\n{text}");
        assert!(
            text.contains("PRESENTED"),
            "overview header missing:\n{text}"
        );
        // A stopped session with no native identifier is over, not resumable.
        assert!(text.contains("closed"), "disposition missing:\n{text}");
    }

    /// Ratatui will happily panic on a zero-sized or one-cell area if any
    /// layout maths underflows. None here does, and this proves it.
    #[test]
    fn renders_without_panicking_at_absurd_sizes() {
        let mut state = sample();
        for (w, h) in [(1, 1), (1, 40), (40, 1), (3, 3), (200, 60)] {
            rendered(&state, w, h);
        }
        state.handle_key(KeyEvent::new(KeyCode::Char('o'), KeyModifiers::NONE));
        for (w, h) in [(1, 1), (1, 40), (40, 1), (3, 3), (200, 60)] {
            rendered(&state, w, h);
        }
    }

    #[test]
    fn truncate_start_keeps_the_tail_and_counts_characters() {
        assert_eq!(truncate_start("/a/b/c", 10), "/a/b/c");
        assert_eq!(truncate_start("/a/b/c", 6), "/a/b/c");
        assert_eq!(truncate_start("/a/b/c", 4), "…b/c");
        assert_eq!(truncate_start("/a/b/c", 1), "…");
        assert_eq!(truncate_start("/a/b/c", 0), "");
        // Multi-byte characters must not be cut in half.
        let text = truncate_start("/päth/tö/pröject", 8);
        assert_eq!(text.chars().count(), 8, "counted characters, not bytes");
        assert!(text.ends_with("öject"));
    }

    /// The bottom bar carries the key bindings on every screen, including from
    /// inside an overlay where the bindings change.
    #[test]
    fn the_status_bar_always_shows_the_key_bindings() {
        let mut state = sample();
        let bottom = last_row(&state, 100, 24);
        assert!(bottom.contains("tab"), "bindings missing: `{bottom}`");
        assert!(bottom.contains("overview"), "bindings missing: `{bottom}`");
        assert!(bottom.contains("quit"), "bindings missing: `{bottom}`");

        state.handle_key(KeyEvent::new(KeyCode::Char('o'), KeyModifiers::NONE));
        let bottom = last_row(&state, 100, 24);
        assert!(
            bottom.contains("esc") && bottom.contains("quit"),
            "the overlay's bindings must be shown too: `{bottom}`"
        );
    }

    /// A key that could not do anything must explain itself in the status bar,
    /// or it reads as a broken keyboard.
    #[test]
    fn the_status_bar_shows_a_note_next_to_the_bindings() {
        let mut state = ShellState::new("p", "/p", "0.1.0", vec![lone_session()]);
        state.handle_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));

        let bottom = last_row(&state, 100, 24);
        assert!(
            bottom.contains("only one session"),
            "the note must reach the status bar: `{bottom}`"
        );
        assert!(
            bottom.contains("tab"),
            "the note must not displace the bindings: `{bottom}`"
        );
    }

    /// On a narrow terminal the bindings win: they are needed permanently, the
    /// note only once. This holds because the bindings are written first and
    /// the row clips on the right — swap the order and this fails.
    #[test]
    fn a_note_is_dropped_rather_than_crowding_out_the_bindings() {
        let mut state = ShellState::new("p", "/p", "0.1.0", vec![lone_session()]);
        state.handle_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
        let bottom = last_row(&state, 30, 12);
        assert!(
            !bottom.contains("only one session"),
            "there was no room for the note: `{bottom}`"
        );
        assert!(bottom.contains("tab"), "bindings must survive: `{bottom}`");
    }

    /// "Keep the visual design text-first and avoid decorative graph
    /// visualizations that do not expose actionable state."
    ///
    /// Enforced mechanically rather than by inspection: Ratatui's decorative
    /// widgets — `Gauge`, `Sparkline`, `BarChart` — all draw with the Unicode
    /// block elements, so a frame that contains none of those, on any screen,
    /// cannot be rendering one. Box-drawing characters used for borders are a
    /// different range and stay allowed.
    #[test]
    fn nothing_draws_with_block_elements_so_the_design_stays_text_first() {
        let mut state = sample();
        let mut screens = vec![rendered(&state, 100, 30)];
        state.handle_key(KeyEvent::new(KeyCode::Char('o'), KeyModifiers::NONE));
        screens.push(rendered(&state, 100, 30));
        state.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        state.handle_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
        screens.push(rendered(&state, 100, 30));

        for screen in screens {
            if let Some(found) = screen.chars().find(|c| {
                // U+2580..U+259F: block elements, which is what gauges,
                // sparklines and bar charts are made of.
                ('\u{2580}'..='\u{259F}').contains(c)
            }) {
                panic!("a decorative block element ({found:?}) was drawn:\n{screen}");
            }
        }
    }

    // -----------------------------------------------------------------
    // The viewport and the session-mode footer.
    // -----------------------------------------------------------------

    #[test]
    fn the_viewport_shows_the_set_text_once_non_empty() {
        let mut state = sample();
        state.set_viewport("first line\nsecond line\nthird line".to_owned());
        let text = rendered(&state, 100, 24);
        assert!(text.contains("first line"), "got:\n{text}");
        assert!(text.contains("second line"), "got:\n{text}");
        assert!(text.contains("third line"), "got:\n{text}");
        assert!(
            !text.contains("This viewport is reserved"),
            "the placeholder must not show once there is real output:\n{text}"
        );
    }

    /// Only the lines that fit are shown, most recent at the bottom.
    #[test]
    fn the_viewport_shows_only_the_most_recent_lines_that_fit() {
        let mut state = sample();
        let lines: Vec<String> = (0..50).map(|i| format!("line-{i}")).collect();
        state.set_viewport(lines.join("\n"));

        let text = rendered(&state, 100, 10);
        assert!(
            text.contains("line-49"),
            "the most recent line must be visible:\n{text}"
        );
        assert!(
            !text.contains("line-0"),
            "the oldest lines should have scrolled off:\n{text}"
        );
    }

    /// "Keep the existing placeholder for the no-session case" — and for the
    /// active-but-silent-so-far case too: the placeholder is only replaced
    /// once there is real output to show.
    #[test]
    fn an_empty_viewport_keeps_the_existing_placeholder() {
        let state = sample();
        assert_eq!(state.viewport(), "");
        let text = rendered(&state, 100, 24);
        assert!(text.contains("This viewport is reserved"), "got:\n{text}");
    }

    /// A viewport full of real output, at sizes much smaller than the
    /// content, must not panic the slicing that keeps it bounded to what
    /// fits.
    #[test]
    fn the_viewport_does_not_panic_with_real_output_at_absurd_sizes() {
        let mut state = sample();
        let lines: Vec<String> = (0..500)
            .map(|i| format!("line-{i}-{}", "x".repeat(i % 50)))
            .collect();
        state.set_viewport(lines.join("\n"));
        for (w, h) in [(1, 1), (1, 40), (40, 1), (3, 3), (200, 60)] {
            rendered(&state, w, h);
        }
    }

    /// The design note: "A user who cannot see how to get out is the
    /// failure this design exists to prevent" — so the mode and the escape
    /// chord are on screen in session mode at all times.
    #[test]
    fn the_status_bar_names_session_mode_and_the_escape_chord() {
        let mut state = sample();
        state.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert_eq!(state.mode(), Mode::Session);

        let bottom = last_row(&state, 100, 24).to_lowercase();
        assert!(
            bottom.contains("session"),
            "the active mode must be named: `{bottom}`"
        );
        assert!(
            bottom.contains("ctrl-]"),
            "the escape chord must always be on screen in session mode: `{bottom}`"
        );
    }

    /// Control mode's own footer must not claim to be session mode.
    #[test]
    fn the_status_bar_shows_control_mode_bindings_by_default() {
        let state = sample();
        assert_eq!(state.mode(), Mode::Control);
        let bottom = last_row(&state, 100, 24).to_lowercase();
        assert!(!bottom.contains("session mode"), "got: `{bottom}`");
        assert!(bottom.contains("quit"), "got: `{bottom}`");
    }
}

#[cfg(test)]
mod settings_tests {
    use crate::integrations::{IntegrationId, IntegrationStatus};
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    use super::super::state::{HarnessRow, IntegrationRow};
    use super::*;

    fn press(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn harness_rows() -> Vec<HarnessRow> {
        vec![
            HarnessRow {
                id: IntegrationId::ClaudeCode,
                detected: true,
                enabled: true,
                enabled_layer: Layer::Project,
                executable: Some("/opt/bin/claude".into()),
                executable_layer: Some(Layer::User),
            },
            HarnessRow {
                id: IntegrationId::Codex,
                detected: false,
                enabled: false,
                enabled_layer: Layer::Default,
                executable: None,
                executable_layer: None,
            },
        ]
    }

    fn integration_rows() -> Vec<IntegrationRow> {
        vec![IntegrationRow {
            id: IntegrationId::Ollama,
            detected: false,
            status: IntegrationStatus::NotFound,
        }]
    }

    fn state_with_settings_open() -> ShellState {
        let mut state = ShellState::new("glasshouse", "/work/glasshouse", "0.1.0", Vec::new());
        state.open_settings(harness_rows(), integration_rows());
        state
    }

    fn rendered(state: &ShellState, width: u16, height: u16) -> String {
        let backend = TestBackend::new(width, height);
        let mut terminal = Terminal::new(backend).expect("terminal");
        terminal
            .draw(|frame| render(state, frame))
            .expect("draw must not panic");
        let buffer = terminal.backend().buffer();
        (0..buffer.area.height)
            .map(|y| {
                (0..buffer.area.width)
                    .map(|x| buffer[(x, y)].symbol())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn settings_shows_both_section_labels_and_the_harness_rows() {
        let state = state_with_settings_open();
        let text = rendered(&state, 100, 30);
        assert!(text.contains("Harnesses"), "got:\n{text}");
        assert!(text.contains("Integrations"), "got:\n{text}");
        assert!(text.contains("Claude Code"), "got:\n{text}");
    }

    /// The design decision: "provenance is shown, not inferred" — every
    /// displayed value must carry a layer tag.
    #[test]
    fn every_harness_value_shown_carries_its_layer() {
        let state = state_with_settings_open();
        let text = rendered(&state, 100, 30);
        assert!(text.contains("(project)"), "enabled layer missing:\n{text}");
        assert!(text.contains("(user)"), "executable layer missing:\n{text}");
        assert!(
            text.contains("(default)"),
            "the never-configured row's layer must still show:\n{text}"
        );
    }

    #[test]
    fn switching_to_the_integrations_section_shows_its_rows() {
        let mut state = state_with_settings_open();
        state.handle_key(press(KeyCode::Tab));
        let text = rendered(&state, 100, 30);
        assert!(text.contains("Ollama"), "got:\n{text}");
        assert!(text.contains("not found"), "got:\n{text}");
    }

    #[test]
    fn the_path_editor_renders_the_typed_buffer() {
        let mut state = state_with_settings_open();
        state.handle_key(press(KeyCode::Enter));
        for c in "/opt/bin".chars() {
            state.handle_key(press(KeyCode::Char(c)));
        }
        let text = rendered(&state, 100, 30);
        assert!(text.contains("/opt/bin"), "got:\n{text}");
    }

    /// The confirmation must name the exact path the design decision
    /// requires, not a generic "are you sure".
    #[test]
    fn the_project_write_confirmation_names_the_exact_path() {
        let mut state = state_with_settings_open();
        state.handle_key(press(KeyCode::Char('W')));
        let text = rendered(&state, 100, 30);
        assert!(
            text.contains("/work/glasshouse/.glasshouse/config.toml"),
            "the exact path must be shown:\n{text}"
        );
        assert!(
            text.to_lowercase().contains("repository"),
            "must say the file is inside the repository:\n{text}"
        );
    }

    #[test]
    fn the_footer_names_the_settings_bindings() {
        let state = state_with_settings_open();
        let bottom = rendered(&state, 100, 30)
            .lines()
            .last()
            .unwrap()
            .trim_end()
            .to_owned();
        assert!(bottom.contains("save"), "got: `{bottom}`");
        assert!(bottom.contains("toggle"), "got: `{bottom}`");
    }

    #[test]
    fn settings_renders_without_panicking_at_absurd_sizes() {
        let mut state = state_with_settings_open();
        for (w, h) in [(1, 1), (1, 40), (40, 1), (3, 3), (200, 60)] {
            rendered(&state, w, h);
        }
        state.handle_key(press(KeyCode::Enter));
        for (w, h) in [(1, 1), (1, 40), (40, 1), (3, 3)] {
            rendered(&state, w, h);
        }
        state.handle_key(press(KeyCode::Esc));
        state.handle_key(press(KeyCode::Char('W')));
        for (w, h) in [(1, 1), (1, 40), (40, 1), (3, 3)] {
            rendered(&state, w, h);
        }
    }

    /// Same design-first guarantee as the rest of the shell: no decorative
    /// block-element glyphs anywhere in the Settings overlay.
    #[test]
    fn settings_draws_with_no_decorative_block_elements() {
        let mut state = state_with_settings_open();
        let mut screens = vec![rendered(&state, 100, 30)];
        state.handle_key(press(KeyCode::Tab));
        screens.push(rendered(&state, 100, 30));
        state.handle_key(press(KeyCode::BackTab));
        state.handle_key(press(KeyCode::Enter));
        screens.push(rendered(&state, 100, 30));

        for screen in screens {
            if let Some(found) = screen
                .chars()
                .find(|c| ('\u{2580}'..='\u{259F}').contains(c))
            {
                panic!("a decorative block element ({found:?}) was drawn:\n{screen}");
            }
        }
    }
}
