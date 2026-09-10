use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::Span;
use ratatui::widgets::{Block, Borders, Clear, Paragraph};

use crate::profile::BackendResource;
use crate::session::SessionPresentation;

use super::super::hotspot::{self, Hotspot, Pill};
use super::super::state::ShellState;

pub(super) fn render(state: &ShellState, frame: &mut Frame, area: Rect, sink: &mut Vec<Hotspot>) {
    let Some(choice) = state.profile_choice() else {
        return;
    };
    let height = (choice.options.len() as u16 + 4).min(area.height);
    let width = 66.min(area.width);
    let popup = Rect::new(
        area.x + area.width.saturating_sub(width) / 2,
        area.y + area.height.saturating_sub(height) / 2,
        width,
        height,
    );
    frame.render_widget(Clear, popup);
    let title = match choice.presentation {
        SessionPresentation::Headless => " choose launch profile for headless session ",
        _ => " choose launch profile ",
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .title(title)
        .border_style(Style::default().fg(state.theme().accent()))
        .style(Style::default().bg(Color::Reset));
    let inner = block.inner(popup);
    frame.render_widget(block, popup);

    // Keep two rows for the gap and keyboard hint when the popup is tall
    // enough. A short terminal instead spends every available inner row on
    // choices. The window follows the cursor, so keyboard navigation can
    // never select an option that has scrolled out of sight.
    let option_rows = usize::from(if inner.height >= 3 {
        inner.height - 2
    } else {
        inner.height
    });
    let first = choice
        .cursor
        .saturating_add(1)
        .saturating_sub(option_rows)
        .min(choice.options.len().saturating_sub(option_rows));
    for (visible_index, (index, profile)) in choice
        .options
        .iter()
        .enumerate()
        .skip(first)
        .take(option_rows)
        .enumerate()
    {
        let row = Rect::new(inner.x, inner.y + visible_index as u16, inner.width, 1);
        let backend = match &profile.backend {
            BackendResource::Native => "native".to_owned(),
            BackendResource::DirectProvider { provider } => format!("provider:{provider}"),
            BackendResource::GlasshouseGateway => "gateway".to_owned(),
        };
        let label = match &profile.model {
            Some(model) => format!("{}  {backend} / {model}", profile.name),
            None => format!("{}  {backend}", profile.name),
        };
        let mut keys = hotspot::walk_to(choice.cursor, index, KeyCode::Up, KeyCode::Down);
        keys.push(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        let pill = Pill::run("", &label, keys).focused(index == choice.cursor);
        hotspot::render_bar(frame, row, std::slice::from_ref(&pill), state.theme(), sink);
    }
    let hint_row = inner.y + option_rows as u16 + 1;
    if hint_row < inner.bottom() {
        frame.render_widget(
            Paragraph::new(Span::styled(
                "up/down pick   enter start   esc cancel",
                Style::default().fg(state.theme().quiet()),
            )),
            Rect::new(inner.x, hint_row, inner.width, 1),
        );
    }
}
