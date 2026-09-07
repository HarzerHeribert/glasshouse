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
    source: Vec<ModelGroup>,
    matched: Vec<ModelGroup>,
    providers: Vec<String>,
    active: usize,
    choices: Vec<usize>,
}
#[derive(Debug, Clone)]
pub struct ModelGroup {
    pub provider: String,
    pub account: String,
    pub scope: String,
    pub models: Vec<String>,
    pub selectable: Option<bool>,
    pub unavailable_reason: Option<String>,
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

    pub fn models(title: impl Into<String>, mut groups: Vec<ModelGroup>) -> Self {
        groups.sort_by(|a, b| {
            (&a.provider, &a.account, &a.scope).cmp(&(&b.provider, &b.account, &b.scope))
        });
        for group in &mut groups {
            group.models.retain(|id| {
                !id.is_empty() && !id.chars().any(|c| c.is_whitespace() || c.is_control())
            });
            group.models.sort();
            group.models.dedup();
        }
        let mut panel = Self {
            title: title.into(),
            search: Some(PanelSearch {
                source: groups,
                ..PanelSearch::default()
            }),
            ..Self::default()
        };
        panel.filter();
        panel
    }

    pub fn move_provider(&mut self, forward: bool) {
        let Some(search) = self.search.as_mut() else {
            return;
        };
        search.active = if forward {
            (search.active + 1).min(search.providers.len().saturating_sub(1))
        } else {
            search.active.saturating_sub(1)
        };
        self.provider_rows();
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
        let Some(search) = &mut self.search else {
            return;
        };
        let previous = search.providers.get(search.active).cloned();
        let query = search.query.to_lowercase();
        let terms: Vec<_> = query.split_whitespace().collect();
        search.matched = search
            .source
            .iter()
            .filter_map(|group| {
                let prefix = format!("{} {}", group.provider, group.account).to_lowercase();
                let models: Vec<_> = group
                    .models
                    .iter()
                    .filter(|id| {
                        let haystack = format!("{prefix} {}", id.to_lowercase());
                        terms.iter().all(|term| haystack.contains(term))
                    })
                    .cloned()
                    .collect();
                (!models.is_empty()).then(|| ModelGroup {
                    models,
                    ..group.clone()
                })
            })
            .collect();
        search.providers = search
            .matched
            .iter()
            .map(|group| group.provider.clone())
            .collect();
        search.providers.dedup();
        search.active = previous
            .and_then(|name| search.providers.iter().position(|p| p == &name))
            .unwrap_or(0);
        self.provider_rows();
    }

    fn provider_rows(&mut self) {
        let Some(search) = &mut self.search else {
            return;
        };
        self.rows.clear();
        search.choices.clear();
        for group in search
            .matched
            .iter()
            .filter(|group| Some(&group.provider) == search.providers.get(search.active))
        {
            self.rows.push(PanelRow {
                text: format!(
                    "{} · {} · {}",
                    group.provider,
                    group.account,
                    if group.selectable == Some(false) {
                        group
                            .unavailable_reason
                            .as_deref()
                            .unwrap_or("locked to another route")
                    } else {
                        &group.scope
                    }
                ),
                command: None,
            });
            for id in &group.models {
                search.choices.push(self.rows.len());
                self.rows.push(PanelRow {
                    text: format!(
                        "  {}{id}",
                        if group.selectable == Some(false) {
                            "× "
                        } else {
                            ""
                        }
                    ),
                    command: (group.selectable != Some(false)).then(|| format!("/model {id}")),
                });
            }
        }
        if self.rows.is_empty() {
            self.rows.push(PanelRow {
                text: "No models match. Ctrl-U clears search.".into(),
                command: None,
            });
        }
        self.selected = search.choices.first().copied().unwrap_or(0);
    }

    pub fn move_selection(&mut self, forward: bool, count: usize) {
        for _ in 0..count {
            let next = if forward {
                (self.selected + 1..self.rows.len()).find(|&i| {
                    self.search
                        .as_ref()
                        .is_none_or(|search| search.choices.contains(&i))
                })
            } else {
                (0..self.selected).rev().find(|&i| {
                    self.search
                        .as_ref()
                        .is_none_or(|search| search.choices.contains(&i))
                })
            };
            if let Some(next) = next {
                self.selected = next
            } else {
                break;
            }
        }
    }
}
pub(super) fn render_panel(frame: &mut Frame, area: Rect, panel: &Panel, theme: super::Theme) {
    let block = if panel.search.is_some() {
        Block::default()
            .borders(Borders::TOP | Borders::BOTTOM)
            .title(format!(" {} ", panel.title))
            .title_bottom(" ↑↓ model · Enter apply · Esc close ")
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
        render_providers(frame, &mut inner, search, theme);
        let matches: usize = search.matched.iter().map(|g| g.models.len()).sum();
        let total: usize = search.source.iter().map(|g| g.models.len()).sum();
        let prompt = if search.query.is_empty() {
            "type to filter"
        } else {
            &search.query
        };
        let group = panel
            .rows
            .iter()
            .enumerate()
            .take(panel.selected + 1)
            .rev()
            .find(|(i, _)| !search.choices.contains(i))
            .map_or("", |(_, row)| row.text.as_str());
        let lines = [
            format!("Search: {prompt}  · {matches}/{total}"),
            "← → provider · Ctrl-U clear · route unchanged".into(),
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
                        theme.accent()
                    } else {
                        Color::White
                    },
                ),
            )
        })
        .collect();
    frame.render_widget(Paragraph::new(rows), inner);
}

fn render_providers(frame: &mut Frame, area: &mut Rect, search: &PanelSearch, theme: super::Theme) {
    if area.height < 3 || area.width < 6 || search.providers.is_empty() {
        return;
    }
    let available = usize::from(area.width.saturating_sub(4));
    let visible = (available / 22).max(1).min(search.providers.len());
    let first = search
        .active
        .saturating_sub(visible / 2)
        .min(search.providers.len().saturating_sub(visible));
    let card_width = (available.saturating_sub(visible - 1) / visible) as u16;
    for (slot, provider) in search
        .providers
        .iter()
        .enumerate()
        .skip(first)
        .take(visible)
    {
        let selected = slot == search.active;
        let card = Rect::new(
            area.x + 2 + (slot - first) as u16 * (card_width + 1),
            area.y,
            card_width,
            3,
        );
        let locked = search
            .matched
            .iter()
            .filter(|group| &group.provider == provider)
            .all(|group| group.selectable == Some(false));
        let style = if selected && !locked {
            Style::default().bg(theme.accent()).fg(Color::Black)
        } else {
            Style::default().bg(theme.dock()).fg(theme.accent())
        };
        let count: usize = search
            .matched
            .iter()
            .filter(|group| &group.provider == provider)
            .map(|group| group.models.len())
            .sum();
        let label = super::abbreviate(provider, card_width.saturating_sub(4) as usize);
        frame.render_widget(
            Paragraph::new(vec![
                Line::from(format!(" {} {label}", if selected { "◆" } else { "·" })),
                Line::from(format!(
                    " {count} models{}",
                    if locked { " · locked" } else { "" }
                )),
                Line::from(if selected { " ━━━━━" } else { "" }),
            ])
            .style(style),
            card,
        );
    }
    if first > 0 {
        frame.render_widget(
            Paragraph::new("◀").style(Style::default().fg(theme.accent())),
            Rect::new(area.x, area.y + 1, 1, 1),
        );
    }
    if first + visible < search.providers.len() {
        frame.render_widget(
            Paragraph::new("▶").style(Style::default().fg(theme.accent())),
            Rect::new(area.right() - 1, area.y + 1, 1, 1),
        );
    }
    area.y += 3;
    area.height -= 3;
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::{Terminal, backend::TestBackend};

    fn catalogue() -> Panel {
        Panel::models(
            "Models",
            (0..5)
                .map(|i| ModelGroup {
                    provider: format!("provider-{i}"),
                    account: format!("account-{i}"),
                    scope: "declared".into(),
                    models: vec![format!("model-{i}/exact"), "shared/flash".into()],
                    selectable: None,
                    unavailable_reason: None,
                })
                .collect(),
        )
    }

    #[test]
    fn carousel_search_crosses_providers_and_preserves_exact_selection() {
        let mut panel = catalogue();
        assert!(panel.rows[0].text.contains("provider-0"));
        panel.move_provider(true);
        assert!(panel.rows[0].text.contains("provider-1"));
        panel.search_insert("ACCOUNT-4 exact");
        assert_eq!(panel.search.as_ref().unwrap().providers, ["provider-4"]);
        assert_eq!(
            panel.rows[panel.selected].command.as_deref(),
            Some("/model model-4/exact")
        );
        panel.search_insert("x");
        assert!(panel.rows[0].text.starts_with("No models match"));
        panel.search_backspace();
        assert_eq!(panel.rows.len(), 2);
        panel.search_clear();
        panel.search_insert("flash");
        assert_eq!(panel.search.as_ref().unwrap().providers.len(), 5);
        panel.move_provider(true);
        assert_eq!(
            panel.rows[panel.selected].command.as_deref(),
            Some("/model shared/flash")
        );
    }

    #[test]
    fn locked_accounts_are_searchable_but_have_no_apply_command() {
        let mut panel = Panel::models(
            "Models",
            vec![ModelGroup {
                provider: "google".into(),
                account: "other-sub".into(),
                scope: "subscription".into(),
                models: vec!["gemini/exact".into(), "gemini/second".into()],
                selectable: Some(false),
                unavailable_reason: Some("Pinned to another account".into()),
            }],
        );
        assert!(panel.rows[0].text.contains("Pinned to another account"));
        assert!(panel.rows.iter().all(|row| row.command.is_none()));
        assert_eq!(panel.selected, 1);
        panel.move_selection(true, 1);
        assert_eq!(panel.selected, 2, "locked models remain inspectable");
        panel.search_insert("other-sub exact");
        assert_eq!(panel.rows.len(), 2);
        assert!(panel.rows[panel.selected].command.is_none());
    }

    #[test]
    fn provider_cards_follow_the_theme_and_show_offscreen_directions_without_overflow() {
        for theme in super::super::Theme::ALL {
            let mut panel = catalogue();
            let mut wide = Terminal::new(TestBackend::new(80, 22)).unwrap();
            wide.draw(|frame| render_panel(frame, frame.area(), &panel, theme))
                .unwrap();
            assert_eq!(wide.backend().buffer()[(2, 1)].bg, theme.accent());
            assert_eq!(wide.backend().buffer()[(27, 1)].bg, theme.dock());
            let mut terminal = Terminal::new(TestBackend::new(44, 22)).unwrap();
            for (moves, left, right) in [(0, false, true), (2, true, true), (2, true, false)] {
                for _ in 0..moves {
                    panel.move_provider(true);
                }
                terminal
                    .draw(|frame| render_panel(frame, Rect::new(2, 1, 40, 20), &panel, theme))
                    .unwrap();
                let buffer = terminal.backend().buffer();
                assert_eq!(buffer[(2, 3)].symbol() == "◀", left);
                assert_eq!(buffer[(41, 3)].symbol() == "▶", right);
                assert_eq!(buffer[(4, 2)].bg, theme.accent());
                assert_eq!(buffer[(4, 2)].fg, Color::Black);
                assert_eq!(buffer[(0, 3)].symbol(), " ");
                assert_eq!(buffer[(43, 3)].symbol(), " ");
            }
        }
        for width in [1, 5, 12, 40, 80, 160] {
            for height in [1, 4, 8, 24] {
                let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
                terminal
                    .draw(|frame| {
                        render_panel(frame, frame.area(), &catalogue(), super::super::Theme::Neon)
                    })
                    .unwrap();
            }
        }
    }
}
