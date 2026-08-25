//! Rendering for the wizard, kept entirely separate from state and from the
//! event loop.
//!
//! [`render`] is a pure function of a [`WizardState`] and a [`Frame`]: it
//! reads, it never mutates, and it never blocks. That is what lets
//! [`super::run`] redraw only when [`super::Action`] says something changed,
//! and what lets the tests in this module drive it with
//! [`ratatui::backend::TestBackend`] instead of a real terminal.

use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph, Wrap};

use crate::integrations::{IntegrationKind, IntegrationStatus};

use super::state::{PathInputView, RowView, Step, WizardState};

/// Draw the current step of `state` into `frame`.
///
/// Every screen fits an 80x24 terminal without scrolling. Below that, the
/// integration list scrolls to follow the selection, so every row stays
/// reachable rather than being cut off at the bottom edge; the other regions
/// simply get less space from Ratatui's layout solver rather than panicking —
/// nothing here computes a size by subtraction, which is the usual way a
/// "must not panic on a tiny terminal" requirement gets violated.
pub fn render(state: &WizardState, frame: &mut Frame) {
    let area = frame.area();
    let [title_area, body_area, footer_area] = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Min(0),
            Constraint::Length(1),
        ])
        .areas(area);

    render_title(state, frame, title_area);
    match state.step() {
        Step::Welcome => render_welcome(state, frame, body_area),
        Step::Harnesses => render_harnesses(state, frame, body_area),
        Step::Summary => render_summary(state, frame, body_area),
    }
    render_footer(state, frame, footer_area);
}

fn render_title(state: &WizardState, frame: &mut Frame, area: Rect) {
    let label = match state.step() {
        Step::Welcome => "Glasshouse setup — welcome",
        Step::Harnesses => "Glasshouse setup — harnesses & integrations",
        Step::Summary => "Glasshouse setup — review",
    };
    frame.render_widget(
        Paragraph::new(label).style(Style::default().add_modifier(Modifier::BOLD)),
        area,
    );
}

fn render_welcome(state: &WizardState, frame: &mut Frame, area: Rect) {
    let lines = vec![
        Line::from(""),
        Line::from(
            "Glasshouse launches your existing Claude Code, Codex, Antigravity, and \
             OpenCode installations directly. It never installs replacement copies and \
             never hides them behind a proprietary agent loop — every session you start \
             is the real, native harness, fully interactive exactly as you already use it.",
        ),
        Line::from(""),
        Line::from(Span::styled(
            "One instance, one project",
            Style::default().add_modifier(Modifier::BOLD),
        )),
        Line::from(format!(
            "This Glasshouse instance is scoped to exactly one project: \"{}\" at {}. Its \
             state and memory are kept physically separate per project — nothing here is \
             ever retrieved from, or shared with, another project.",
            state.project_name(),
            state.project_root().display(),
        )),
        Line::from(""),
        Line::from(
            "No account, cloud sign-in, or Glasshouse-hosted service is used anywhere in \
             this setup — everything stays on this machine.",
        ),
    ];
    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), area);
}

fn render_harnesses(state: &WizardState, frame: &mut Frame, area: Rect) {
    let input = state.path_input();
    let constraints = if input.is_some() {
        vec![Constraint::Min(0), Constraint::Length(2)]
    } else {
        vec![Constraint::Min(0), Constraint::Length(0)]
    };
    let regions = Layout::default()
        .direction(Direction::Vertical)
        .constraints(constraints)
        .split(area);
    let list_area = regions[0];
    let input_area = regions[1];

    let mut items = Vec::new();
    let mut current_kind: Option<IntegrationKind> = None;
    // Which *item* the selected row became. Headers are interleaved with
    // rows, so this is not the row index, and the list has to be told the
    // item index or it would scroll to the wrong place.
    let mut selected_item = None;
    for row in state.rows() {
        if current_kind != Some(row.kind) {
            current_kind = Some(row.kind);
            let header = match row.kind {
                IntegrationKind::Harness => "Harnesses",
                IntegrationKind::Multiplexer | IntegrationKind::LocalInference => {
                    "Optional integrations"
                }
            };
            items.push(ListItem::new(Line::from(Span::styled(
                header,
                Style::default().add_modifier(Modifier::BOLD),
            ))));
        }
        if row.selected {
            selected_item = Some(items.len());
        }
        items.push(ListItem::new(row_line(row)));
    }

    // Rendered with the selection so the list scrolls to keep it on screen.
    // Without this the catalogue is only fully reachable in a terminal tall
    // enough to hold all of it at once, and an integration past the bottom
    // edge is one the user can neither see nor toggle — silently, because
    // Ratatui simply draws fewer rows rather than complaining.
    //
    // No `highlight_style`: the `> ` cursor in `row_line` is already the
    // selection marker, and the state is here for the scrolling alone.
    let mut list_state = ListState::default();
    list_state.select(selected_item);
    frame.render_stateful_widget(
        List::new(items).block(Block::default().borders(Borders::NONE)),
        list_area,
        &mut list_state,
    );

    if let Some(input) = input {
        render_path_input(&input, frame, input_area);
    }
}

fn row_line(row: RowView<'_>) -> Line<'static> {
    let cursor = if row.selected { "> " } else { "  " };
    let mark = match row.decision {
        Some(true) => "[x]",
        Some(false) => "[ ]",
        None => "[?]",
    };
    let mark_color = match row.decision {
        Some(true) => Color::Green,
        Some(false) => Color::DarkGray,
        None => Color::Yellow,
    };
    let status = describe_status(row.status, row.usable);
    let path = row
        .executable
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| "-".to_owned());
    let version = row.version.unwrap_or("-");

    let mut style = Style::default();
    if row.selected {
        style = style.add_modifier(Modifier::BOLD);
    }

    Line::from(vec![
        Span::styled(cursor.to_owned(), style),
        Span::styled(format!("{mark} "), Style::default().fg(mark_color)),
        Span::styled(format!("{:<12}", row.id.display_name()), style),
        Span::raw(format!(" {status:<12} {path:<28} {version}")),
    ])
}

fn describe_status(status: IntegrationStatus, usable: bool) -> &'static str {
    if usable {
        match status {
            IntegrationStatus::Configured => "configured",
            IntegrationStatus::Unconfigured => "unconfigured",
            IntegrationStatus::Available => "available",
            IntegrationStatus::UnsupportedVersion => "old version",
            IntegrationStatus::NotFound | IntegrationStatus::Unknown => "path added",
        }
    } else {
        "not found"
    }
}

fn render_path_input(input: &PathInputView<'_>, frame: &mut Frame, area: Rect) {
    let mut lines = vec![Line::from(format!(
        "Path to {} executable: {}_",
        input.integration_name, input.buffer
    ))];
    if let Some(error) = input.error {
        lines.push(Line::from(Span::styled(
            error.to_owned(),
            Style::default().fg(Color::Red),
        )));
    }
    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), area);
}

fn render_summary(state: &WizardState, frame: &mut Frame, area: Rect) {
    let mut lines = vec![Line::from(
        "Setup is complete once you finish. These choices are saved to your \
         user-level Glasshouse configuration and can be changed later by reopening \
         this wizard.",
    )];
    lines.push(Line::from(""));
    for row in state.rows() {
        let decision = match row.decision {
            Some(true) => "enabled",
            Some(false) | None => "ignored",
        };
        let extra = if row.decision == Some(true) {
            row.executable
                .map(|p| format!(" ({})", p.display()))
                .unwrap_or_default()
        } else {
            String::new()
        };
        lines.push(Line::from(format!(
            "  {:<12} {decision}{extra}",
            row.id.display_name()
        )));
    }
    lines.push(Line::from(""));
    lines.push(Line::from(
        "Provider, gateway, and routing-model configuration are not part of this \
         setup yet. No Glasshouse API key is required to finish; enabled native \
         harnesses keep using their existing authentication.",
    ));
    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), area);
}

fn render_footer(state: &WizardState, frame: &mut Frame, area: Rect) {
    let text = if state.path_input().is_some() {
        "Type path   Enter confirm   Esc cancel input   Ctrl+C quit setup"
    } else {
        match state.step() {
            Step::Welcome => "Enter / Tab continue   Esc cancel",
            Step::Harnesses => {
                "↑/↓ or j/k move   Space/Enter toggle or add path   Tab continue   Esc cancel"
            }
            Step::Summary => "Enter / Tab finish   Esc cancel",
        }
    };
    frame.render_widget(
        Paragraph::new(text).style(Style::default().fg(Color::DarkGray)),
        area,
    );
}

#[cfg(test)]
mod tests {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    use crate::config::UserConfig;
    use crate::integrations::{IntegrationId, IntegrationStatus};

    use super::super::state::{IntegrationDetection, WizardState};
    use super::*;

    fn sample_state() -> WizardState {
        let detected = vec![
            IntegrationDetection {
                id: IntegrationId::ClaudeCode,
                status: IntegrationStatus::Configured,
                executable: Some("/usr/bin/claude".into()),
                version: Some("1.2.3".to_owned()),
            },
            IntegrationDetection {
                id: IntegrationId::Codex,
                status: IntegrationStatus::NotFound,
                executable: None,
                version: None,
            },
        ];
        WizardState::new(
            &detected,
            &UserConfig::default(),
            "glasshouse".to_owned(),
            "/home/user/glasshouse".into(),
            "0.1.0".to_owned(),
        )
    }

    fn render_at(state: &WizardState, width: u16, height: u16) {
        let backend = TestBackend::new(width, height);
        let mut terminal = Terminal::new(backend).expect("terminal");
        terminal
            .draw(|frame| render(state, frame))
            .expect("draw must not panic");
    }

    /// Every integration the wizard offers has a row on an 80x24 screen.
    ///
    /// The catalogue grew from seven integrations to ten this session, which
    /// moved this materially closer to its limit: ten rows plus two section
    /// headers against the twenty-two the body gets. Not panicking is not the
    /// same as being usable — Ratatui silently draws fewer rows when a list
    /// outgrows its area, so an integration past the bottom edge would be one
    /// the user can neither see nor toggle, with every other test green.
    ///
    /// cmux is included here by giving it a detected executable, because the
    /// wizard deliberately does not offer an undetected cmux at all (see
    /// `build_rows`).
    #[test]
    fn every_offered_integration_has_a_row_at_80x24() {
        let state = advance_to_harnesses(all_detected_state());
        let screen = rendered_lines(&state, 80, 24);

        for &id in IntegrationId::ALL {
            let name = id.display_name();
            assert!(
                screen.iter().any(|line| line.contains(name)),
                "`{name}` has no visible row at 80x24; the catalogue has outgrown the \
                 wizard's list"
            );
        }
    }

    /// Below 80x24 the list scrolls to follow the selection, so every row
    /// stays reachable rather than being cut off at the bottom edge.
    ///
    /// Twelve items into ten rows: this height genuinely truncates, which is
    /// what makes the assertion mean something. Reverting the list to a
    /// stateless `render_widget` fails this while leaving the test above
    /// passing.
    #[test]
    fn a_short_terminal_still_reaches_every_integration() {
        let mut state = advance_to_harnesses(all_detected_state());

        for (step, &id) in IntegrationId::ALL.iter().enumerate() {
            let name = id.display_name();
            let screen = rendered_lines(&state, 80, 12);
            assert!(
                screen.iter().any(|line| line.contains(name)),
                "after {step} moves down, `{name}` is off a 80x12 screen and cannot be \
                 reached"
            );
            state.handle_key(crossterm::event::KeyEvent::new(
                crossterm::event::KeyCode::Down,
                crossterm::event::KeyModifiers::NONE,
            ));
        }
    }

    /// A wizard offered every integration in the catalogue. cmux needs a
    /// detected executable or the wizard will not offer it.
    fn all_detected_state() -> WizardState {
        let detected: Vec<IntegrationDetection> = IntegrationId::ALL
            .iter()
            .map(|&id| IntegrationDetection {
                id,
                status: IntegrationStatus::NotFound,
                executable: (id == IntegrationId::Cmux).then(|| "/usr/bin/cmux".into()),
                version: None,
            })
            .collect();
        WizardState::new(
            &detected,
            &UserConfig::default(),
            "glasshouse".to_owned(),
            "/home/user/glasshouse".into(),
            "0.1.0".to_owned(),
        )
    }

    /// Draw `state` and read the screen back as lines of text.
    fn rendered_lines(state: &WizardState, width: u16, height: u16) -> Vec<String> {
        let backend = TestBackend::new(width, height);
        let mut terminal = Terminal::new(backend).expect("terminal");
        terminal
            .draw(|frame| render(state, frame))
            .expect("draw must not panic");
        let buffer = terminal.backend().buffer().clone();
        (0..buffer.area.height)
            .map(|y| {
                (0..buffer.area.width)
                    .map(|x| buffer[(x, y)].symbol())
                    .collect::<String>()
            })
            .collect()
    }

    /// Move a fresh wizard to the harnesses step.
    fn advance_to_harnesses(mut state: WizardState) -> WizardState {
        for _ in 0..4 {
            if matches!(state.step(), Step::Harnesses) {
                return state;
            }
            state.handle_key(crossterm::event::KeyEvent::new(
                crossterm::event::KeyCode::Enter,
                crossterm::event::KeyModifiers::NONE,
            ));
        }
        panic!("the wizard never reached the harnesses step");
    }

    #[test]
    fn every_step_renders_at_80x24_without_panicking() {
        let mut state = sample_state();
        render_at(&state, 80, 24);

        state.handle_key(crossterm::event::KeyEvent::new(
            crossterm::event::KeyCode::Enter,
            crossterm::event::KeyModifiers::NONE,
        ));
        render_at(&state, 80, 24);

        state.handle_key(crossterm::event::KeyEvent::new(
            crossterm::event::KeyCode::Tab,
            crossterm::event::KeyModifiers::NONE,
        ));
        render_at(&state, 80, 24);
    }

    #[test]
    fn renders_without_panicking_at_a_tiny_size() {
        let state = sample_state();
        render_at(&state, 20, 5);
    }

    #[test]
    fn renders_without_panicking_at_a_large_size() {
        let state = sample_state();
        render_at(&state, 300, 100);
    }

    #[test]
    fn renders_without_panicking_with_the_path_input_open() {
        let mut state = sample_state();
        state.handle_key(crossterm::event::KeyEvent::new(
            crossterm::event::KeyCode::Enter,
            crossterm::event::KeyModifiers::NONE,
        ));
        // Move onto Codex (not detected) and open the path input.
        state.handle_key(crossterm::event::KeyEvent::new(
            crossterm::event::KeyCode::Down,
            crossterm::event::KeyModifiers::NONE,
        ));
        state.handle_key(crossterm::event::KeyEvent::new(
            crossterm::event::KeyCode::Enter,
            crossterm::event::KeyModifiers::NONE,
        ));
        assert!(state.path_input().is_some());
        render_at(&state, 80, 24);
        render_at(&state, 20, 5);
    }

    #[test]
    fn zero_area_does_not_panic() {
        let state = sample_state();
        render_at(&state, 0, 0);
    }
}
