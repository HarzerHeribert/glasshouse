//! Local session controls and scrollable command panels.
use ratatui::{
    Frame,
    layout::Rect,
    style::{Color, Style},
    text::Line,
    widgets::{Block, Borders, Paragraph},
};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Mode {
    #[default]
    Execute,
    Plan,
}
impl Mode {
    pub fn name(self) -> &'static str {
        match self {
            Self::Execute => "execute",
            Self::Plan => "plan",
        }
    }
    pub fn next(self) -> Self {
        match self {
            Self::Execute => Self::Plan,
            Self::Plan => Self::Execute,
        }
    }
}
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum StatusLine {
    #[default]
    Full,
    Compact,
    Hidden,
}
#[derive(Debug, Clone, Default)]
pub struct Panel {
    pub title: String,
    pub rows: Vec<PanelRow>,
    pub selected: usize,
}
#[derive(Debug, Clone)]
pub struct PanelRow {
    pub text: String,
    /// A user-selected local slash command, never model-supplied executable text.
    pub command: Option<String>,
}
impl Panel {
    pub fn text(title: impl Into<String>, text: impl AsRef<str>) -> Self {
        Self {
            title: title.into(),
            rows: text
                .as_ref()
                .lines()
                .map(|line| PanelRow {
                    text: line.into(),
                    command: None,
                })
                .collect(),
            selected: 0,
        }
    }
}
pub(super) fn render_panel(frame: &mut Frame, area: Rect, panel: &Panel) {
    let block = Block::default()
        .borders(Borders::TOP | Borders::BOTTOM)
        .title(format!(
            " {} · ↑↓ scroll · Enter select · Esc close ",
            panel.title
        ));
    let inner = block.inner(area);
    let start = panel
        .selected
        .saturating_sub(usize::from(inner.height).saturating_sub(1));
    let rows: Vec<_> = panel
        .rows
        .iter()
        .enumerate()
        .skip(start)
        .take(usize::from(inner.height))
        .map(|(i, row)| {
            let text = format!(
                "{} {}",
                if i == panel.selected { "›" } else { " " },
                row.text
            );
            Line::styled(
                super::abbreviate(&text, inner.width as usize),
                Style::default().fg(
                    if let Some(theme) = row
                        .command
                        .as_deref()
                        .and_then(|command| command.strip_prefix("/theme "))
                        .and_then(super::Theme::parse)
                    {
                        theme.accent()
                    } else if i == panel.selected {
                        super::ACCENT
                    } else {
                        Color::White
                    },
                ),
            )
        })
        .collect();
    frame.render_widget(Paragraph::new(rows).block(block), area);
}
