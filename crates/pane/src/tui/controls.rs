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
    pub search: Option<PanelSearch>,
}
#[derive(Debug, Clone, Default)]
pub struct PanelSearch {
    query: String,
    source: Vec<PanelRow>,
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
            search: None,
        }
    }

    /// Search model rows with the preceding provider/account heading as context.
    /// Keep the source catalogue intact, including duplicate IDs on distinct accounts.
    pub fn searchable(mut self) -> Self {
        self.search = Some(PanelSearch {
            query: String::new(),
            source: self.rows.clone(),
        });
        self.filter();
        self
    }

    pub fn search_insert(&mut self, text: &str) -> bool {
        let Some(search) = self.search.as_mut() else {
            return false;
        };
        search
            .query
            .extend(text.chars().filter(|c| !c.is_control()));
        self.filter();
        true
    }

    pub fn search_backspace(&mut self) {
        if let Some(search) = self.search.as_mut() {
            search.query.pop();
            self.filter();
        }
    }

    pub fn search_clear(&mut self) {
        if let Some(search) = self.search.as_mut() {
            search.query.clear();
            self.filter();
        }
    }

    fn filter(&mut self) {
        let Some(search) = &self.search else { return };
        let query = search.query.to_lowercase();
        let terms: Vec<_> = query.split_whitespace().collect();
        self.rows.clear();
        let mut heading: Option<&PanelRow> = None;
        let mut heading_added = false;
        for row in &search.source {
            if row.command.is_none() {
                heading = Some(row);
                heading_added = false;
                continue;
            }
            let haystack =
                format!("{} {}", heading.map_or("", |r| &r.text), row.text).to_lowercase();
            if !terms.iter().all(|term| haystack.contains(term)) {
                continue;
            }
            if !heading_added {
                if let Some(heading) = heading {
                    self.rows.push(heading.clone());
                }
                heading_added = true;
            }
            self.rows.push(row.clone());
        }
        if self.rows.is_empty() {
            self.rows.push(PanelRow {
                text: "No models match. Clear search with Ctrl-U.".into(),
                command: None,
            });
        }
        self.selected = self
            .rows
            .iter()
            .position(|r| r.command.is_some())
            .unwrap_or(0);
    }

    pub fn move_selection(&mut self, forward: bool, count: usize) {
        for _ in 0..count {
            let next = if forward {
                (self.selected + 1..self.rows.len())
                    .find(|&i| self.search.is_none() || self.rows[i].command.is_some())
            } else {
                (0..self.selected)
                    .rev()
                    .find(|&i| self.search.is_none() || self.rows[i].command.is_some())
            };
            if let Some(next) = next {
                self.selected = next
            } else {
                break;
            }
        }
    }
}
pub(super) fn render_panel(frame: &mut Frame, area: Rect, panel: &Panel) {
    let block = if panel.search.is_some() {
        Block::default()
            .borders(Borders::TOP | Borders::BOTTOM)
            .title(format!(" {} ", panel.title))
            .title_bottom(" ↑↓ choose · Enter apply · Esc close ")
    } else {
        Block::default()
            .borders(Borders::TOP | Borders::BOTTOM)
            .title(format!(
                " {} · ↑↓ scroll · Enter select · Esc close ",
                panel.title
            ))
    };
    let mut inner = block.inner(area);
    frame.render_widget(block, area);
    if let Some(search) = &panel.search {
        let matches = panel.rows.iter().filter(|r| r.command.is_some()).count();
        let total = search.source.iter().filter(|r| r.command.is_some()).count();
        let prompt = if search.query.is_empty() {
            "type to filter"
        } else {
            &search.query
        };
        let group = panel
            .rows
            .iter()
            .take(panel.selected + 1)
            .rev()
            .find(|row| row.command.is_none())
            .map_or("", |row| row.text.as_str());
        let lines = [
            format!("Search: {prompt}  · {matches}/{total}"),
            "Ctrl-U clear · account route unchanged".into(),
            if matches > 0 {
                group.to_string()
            } else {
                String::new()
            },
        ];
        for text in lines.into_iter().filter(|text| !text.is_empty()) {
            if inner.height == 0 {
                break;
            }
            frame.render_widget(
                Paragraph::new(super::abbreviate(&text, inner.width as usize)),
                Rect { height: 1, ..inner },
            );
            inner.y += 1;
            inner.height -= 1;
        }
    }
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
    frame.render_widget(Paragraph::new(rows), inner);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn search_matches_provider_account_and_model_without_losing_groups_or_selection() {
        let mut panel = Panel {
            rows: vec![
                PanelRow {
                    text: "OpenRouter · personal".into(),
                    command: None,
                },
                PanelRow {
                    text: "deepseek/flash".into(),
                    command: Some("/model deepseek/flash".into()),
                },
                PanelRow {
                    text: "gemini/flash".into(),
                    command: Some("/model gemini/flash".into()),
                },
                PanelRow {
                    text: "OpenRouter · work".into(),
                    command: None,
                },
                PanelRow {
                    text: "deepseek/flash".into(),
                    command: Some("/model deepseek/flash".into()),
                },
            ],
            ..Panel::default()
        }
        .searchable();
        panel.move_selection(true, 2);
        assert_eq!(panel.selected, 4, "navigation must skip account headings");
        panel.search_insert("OPENROUTER work FLASH");
        assert_eq!(panel.rows.len(), 2);
        assert_eq!(panel.rows[0].text, "OpenRouter · work");
        assert_eq!(
            panel.rows[panel.selected].command.as_deref(),
            Some("/model deepseek/flash")
        );
        panel.search_insert("x");
        assert!(panel.rows[0].text.starts_with("No models match"));
        assert!(panel.rows[panel.selected].command.is_none());
        panel.search_backspace();
        assert_eq!(panel.rows.len(), 2);
        panel.search_clear();
        assert_eq!(panel.rows.len(), 5);
        assert_eq!(panel.selected, 1);
    }
}
