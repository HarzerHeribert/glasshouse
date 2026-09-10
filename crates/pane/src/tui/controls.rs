//! Local session controls and scrollable command panels.
use crate::spend::Tier;
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
    /// Present only on the model panel: which tier a chosen model is being
    /// assigned to, and what all three run on now.
    pub assignment: Option<Assignment>,
}

/// What picking a model in this panel will do, and to which tier.
///
/// A session is three models, not one. Without this the panel could only
/// ever set the parent, and the other two tiers existed solely in a file
/// most people never open — so most people never met them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Assignment {
    /// The tier Enter assigns to. Tab moves it.
    pub active: Tier,
    pub models: TierModels,
}

/// What each tier runs on right now.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TierModels {
    pub parent: String,
    /// `None` when helpers are off, which is a state rather than a missing
    /// value: no helper model means no helper ever runs.
    pub helper: Option<String>,
    /// `None` when a delegated goal inherits the parent's model.
    pub subagent: Option<String>,
}

impl TierModels {
    /// One tier's model, and the word for having none.
    #[must_use]
    pub fn describe(&self, tier: Tier) -> &str {
        match tier {
            Tier::Parent => &self.parent,
            Tier::Helpers => self.helper.as_deref().unwrap_or("off"),
            Tier::Subagents => self.subagent.as_deref().unwrap_or("inherits parent"),
        }
    }
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
    /// The provider whose login flow would connect this account, when it is
    /// connectable and not yet connected. `None` for every other row.
    pub connect: Option<String>,
}
#[derive(Debug, Clone)]
pub struct PanelRow {
    pub text: String,
    /// A user-selected local slash command, never model-supplied executable text.
    pub command: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct PanelGeometry {
    providers: Vec<(Rect, usize)>,
    models: Vec<(Rect, usize)>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PanelHit {
    Provider(usize),
    Model(usize),
}

impl PanelGeometry {
    pub(crate) fn hit(&self, column: u16, row: u16) -> Option<PanelHit> {
        self.providers
            .iter()
            .find(|(area, _)| contains(*area, column, row))
            .map(|(_, index)| PanelHit::Provider(*index))
            .or_else(|| {
                self.models
                    .iter()
                    .find(|(area, _)| contains(*area, column, row))
                    .map(|(_, index)| PanelHit::Model(*index))
            })
    }
}

fn contains(area: Rect, column: u16, row: u16) -> bool {
    area.width > 0
        && area.height > 0
        && column >= area.x
        && column < area.right()
        && row >= area.y
        && row < area.bottom()
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
            assignment: None,
        }
    }

    /// A panel whose rows the caller built, including any that are selectable
    /// because they carry a command.
    ///
    /// [`Self::text`] splits prose into inert lines; this is for a list whose
    /// entries are meant to be chosen.
    pub fn rows(title: impl Into<String>, rows: Vec<PanelRow>) -> Self {
        Self {
            title: title.into(),
            rows,
            selected: 0,
            search: None,
            assignment: None,
        }
    }

    pub fn models(
        title: impl Into<String>,
        mut groups: Vec<ModelGroup>,
        models: TierModels,
    ) -> Self {
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
        // The title states all three tiers, because it is the one part of the
        // panel that also reaches the piped path, where nothing rendered is
        // drawn at all.
        let summary = Tier::every()
            .map(|tier| format!("{} {}", tier.singular(), models.describe(tier)))
            .join(" · ");
        let mut panel = Self {
            title: format!("{} · {summary}", title.into()),
            search: Some(PanelSearch {
                source: groups,
                ..PanelSearch::default()
            }),
            assignment: Some(Assignment {
                active: Tier::Parent,
                models,
            }),
            ..Self::default()
        };
        panel.filter();
        if let Some(search) = panel.search.as_mut()
            && let Some(provider) = search
                .matched
                .iter()
                .find(|group| group.selectable != Some(false))
                .map(|group| group.provider.clone())
            && let Some(index) = search
                .providers
                .iter()
                .position(|candidate| candidate == &provider)
        {
            search.active = index;
            panel.provider_rows();
        }
        panel
    }

    /// Moves the assignment to the next tier, wrapping.
    ///
    /// The rows do not change -- the same catalogue serves all three tiers --
    /// but the command each row carries does, which is the whole mechanism.
    pub fn cycle_tier(&mut self) -> bool {
        let Some(assignment) = self.assignment.as_mut() else {
            return false;
        };
        assignment.active = assignment.active.next();
        self.provider_rows();
        true
    }

    /// The tier a chosen row would be assigned to.
    #[must_use]
    pub fn tier(&self) -> Tier {
        self.assignment
            .as_ref()
            .map_or(Tier::Parent, |assignment| assignment.active)
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

    pub(crate) fn select_provider(&mut self, index: usize) -> bool {
        let Some(search) = self.search.as_mut() else {
            return false;
        };
        if index >= search.providers.len() || search.active == index {
            return index < search.providers.len();
        }
        search.active = index;
        self.provider_rows();
        true
    }

    pub(crate) fn select_model_row(&mut self, index: usize) -> bool {
        let selectable = self
            .search
            .as_ref()
            .is_some_and(|search| search.choices.contains(&index));
        if selectable {
            self.selected = index;
        }
        selectable
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
        let tier = self.tier();
        let Some(search) = &mut self.search else {
            return;
        };
        self.rows.clear();
        search.choices.clear();
        // A tier that can be *unset* offers that as its first row, because
        // otherwise the panel could turn helpers on and never off again --
        // and "off" is the state helpers ship in.
        let clearing = match tier {
            Tier::Parent => None,
            Tier::Helpers => Some(("  ⊘ off — run no helpers", "/model helper off")),
            Tier::Subagents => Some(("  ↳ inherit the parent's model", "/model subagent inherit")),
        };
        if let Some((text, command)) = clearing {
            search.choices.push(self.rows.len());
            self.rows.push(PanelRow {
                text: text.to_string(),
                command: Some(command.to_string()),
            });
        }
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
            // An account that could be connected and is not has no models to
            // list, so the row that would have been empty is the connect
            // action instead. This is where a person looks for models, so it
            // is where the reason there are none belongs.
            if let Some(provider) = &group.connect {
                search.choices.push(self.rows.len());
                self.rows.push(PanelRow {
                    text: format!("  ⊕ connect this {provider} account"),
                    command: Some(format!("/login {}", group.account)),
                });
                continue;
            }
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
                    command: (group.selectable != Some(false)).then(|| match tier {
                        Tier::Parent => format!("/model {id}"),
                        assigned => format!("/model {} {id}", assigned.singular()),
                    }),
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
/// The pill vocabulary, the twin of Glasshouse's `shell::hotspot`.
///
/// **A resting affordance, not only a selected one.** Every surface in either
/// product already drew *selection*; none drew "this is a thing you can
/// press", so an unselected row and a line of prose were the same pixels.
/// A pill is `[`, a one-cell marker, the label, a pad and `]`: the marker slot
/// is why the width never changes as the cursor moves, and brackets rather
/// than half blocks because both half blocks are East Asian *Ambiguous* and a
/// CJK terminal may draw them two cells wide.
///
/// Design: `docs/product/tui-actionables.md`.
const CAP_LEFT: &str = "[";
const CAP_RIGHT: &str = "]";
const MARK_FOCUSED: &str = "▸";
const MARK_RESTING: &str = " ";

/// One actionable drawn as a button: focused fills with the accent, resting
/// fills with the dock so it still reads as pressable.
fn pill(label: &str, focused: bool, theme: super::Theme) -> (String, Style) {
    let marker = if focused { MARK_FOCUSED } else { MARK_RESTING };
    let style = if focused {
        Style::default().bg(theme.accent()).fg(Color::Black)
    } else {
        Style::default().bg(theme.dock()).fg(theme.accent())
    };
    (format!("{CAP_LEFT}{marker}{label} {CAP_RIGHT}"), style)
}

pub(super) fn render_panel(
    frame: &mut Frame,
    area: Rect,
    panel: &Panel,
    theme: super::Theme,
) -> PanelGeometry {
    let mut geometry = PanelGeometry::default();
    let block = if panel.search.is_some() {
        let hint = if area.width >= 100 {
            " ↑↓/click select · Enter apply · text selection: terminal modifier (usually Shift) · Esc close "
        } else {
            " ↑↓/click · Enter apply · Esc "
        };
        Block::default()
            .borders(Borders::TOP | Borders::BOTTOM)
            .title(format!(" {} ", panel.title))
            .title_bottom(hint)
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
        geometry.providers = render_providers(frame, &mut inner, search, theme);
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
        // The two assignment lines are why this panel is worth opening even
        // when nobody means to change anything: they are the only place the
        // three tiers of a session are stated together.
        // One line, and it is why this panel is worth opening even when
        // nobody means to change anything: the only place a session's three
        // tiers are stated together. `▸` marks the one Enter would assign.
        // The title already names all three; this says which one Enter would
        // change, and it is the half that moves when Tab is pressed.
        let assignment = panel
            .assignment
            .as_ref()
            .map_or_else(String::new, |assignment| {
                format!("Tab ⇄ assigning to {} · ", assignment.active.singular())
            });
        // Folded into the existing hint rather than added as a line of its
        // own: an extra header row costs a model row, and on an 80x24
        // terminal that pushed the last account off the panel.
        let lines = [
            format!("Search: {prompt}  · {matches}/{total}"),
            format!("{assignment}← → provider · Ctrl-U clear"),
            if matches > 0 {
                group.to_string()
            } else {
                String::new()
            },
        ];
        // The panel is a model list first. On a short terminal the hint lines
        // yield to it rather than squeezing it to nothing -- which two extra
        // header lines did, leaving a ten-row panel with no models at all.
        const RESERVED_FOR_MODELS: u16 = 3;
        for (index, text) in lines
            .into_iter()
            .filter(|text| !text.is_empty())
            .enumerate()
        {
            if inner.height == 0 || (index > 0 && inner.height <= RESERVED_FOR_MODELS) {
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
        .enumerate()
        .map(|(visible_row, (i, row))| {
            if panel
                .search
                .as_ref()
                .is_some_and(|search| search.choices.contains(&i))
            {
                geometry.models.push((
                    Rect::new(inner.x, inner.y + visible_row as u16, inner.width, 1),
                    i,
                ));
            }
            let focused = i == panel.selected;
            // A row that carries a command is a button; a group heading, a
            // "no models match" note and a locked entry are prose, and
            // drawing prose as a button is exactly the confusion the pill
            // exists to remove. The theme rows keep their own swatch colour,
            // which is the one place the label is the value.
            let Some(command) = row.command.as_deref() else {
                let text = format!("{} {}", if focused { "›" } else { " " }, row.text);
                return Line::styled(
                    super::abbreviate(&text, inner.width as usize),
                    Style::default().fg(if focused {
                        theme.accent()
                    } else {
                        Color::White
                    }),
                );
            };
            let (text, mut style) = pill(&row.text, focused, theme);
            if let Some(swatch) = command
                .strip_prefix("/theme ")
                .and_then(super::Theme::parse)
                && !focused
            {
                style = style.fg(swatch.accent());
            }
            Line::styled(super::abbreviate(&text, inner.width as usize), style)
        })
        .collect();
    frame.render_widget(Paragraph::new(rows), inner);
    geometry
}

fn render_providers(
    frame: &mut Frame,
    area: &mut Rect,
    search: &PanelSearch,
    theme: super::Theme,
) -> Vec<(Rect, usize)> {
    let mut geometry = Vec::new();
    if area.height < 3 || area.width < 6 || search.providers.is_empty() {
        return geometry;
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
        geometry.push((card, slot));
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
    geometry
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
                    connect: None,
                })
                .collect(),
            TierModels::default(),
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
                connect: None,
            }],
            TierModels::default(),
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

    /// The panel sets three models, not one.
    ///
    /// Same catalogue, same rows, same keys -- the tier changes only what the
    /// chosen row *does*, which is why a person can discover the ladder
    /// without being taught it.
    #[test]
    fn tab_moves_which_tier_a_chosen_model_is_assigned_to() {
        let mut panel = Panel::models(
            "Models",
            vec![ModelGroup {
                provider: "openai".into(),
                account: "chatgpt-subscription".into(),
                scope: "subscription".into(),
                models: vec!["gpt-5.6-luna".into()],
                selectable: Some(true),
                unavailable_reason: None,
                connect: None,
            }],
            TierModels {
                parent: "opus-5".into(),
                helper: None,
                subagent: None,
            },
        );
        let command = |panel: &Panel| {
            panel
                .rows
                .iter()
                .filter_map(|row| row.command.clone())
                .collect::<Vec<_>>()
        };

        assert_eq!(panel.tier(), Tier::Parent);
        assert_eq!(command(&panel), ["/model gpt-5.6-luna"]);

        assert!(panel.cycle_tier());
        assert_eq!(panel.tier(), Tier::Helpers);
        assert_eq!(
            command(&panel),
            ["/model helper off", "/model helper gpt-5.6-luna"],
            "a tier that can be unset offers that too"
        );

        assert!(panel.cycle_tier());
        assert_eq!(
            command(&panel),
            ["/model subagent inherit", "/model subagent gpt-5.6-luna"]
        );

        assert!(panel.cycle_tier());
        assert_eq!(panel.tier(), Tier::Parent, "Tab wraps");
    }

    /// Opening the panel is how a person finds out the other two tiers exist,
    /// so it states all three whether or not they are set.
    #[test]
    fn the_panel_states_every_tier_including_the_ones_that_are_unset() {
        let panel = Panel::models(
            "Models",
            vec![ModelGroup {
                provider: "openai".into(),
                account: "chatgpt-subscription".into(),
                scope: "subscription".into(),
                models: vec!["gpt-5.6-luna".into()],
                selectable: Some(true),
                unavailable_reason: None,
                connect: None,
            }],
            TierModels {
                parent: "opus-5".into(),
                helper: None,
                subagent: Some("claude-sonnet-5".into()),
            },
        );
        let mut terminal = Terminal::new(TestBackend::new(90, 24)).unwrap();
        terminal
            .draw(|frame| {
                render_panel(frame, frame.area(), &panel, super::super::Theme::Neon);
            })
            .unwrap();
        let buffer = terminal.backend().buffer();
        let screen: String = (0..24)
            .map(|y| (0..90).map(|x| buffer[(x, y)].symbol()).collect::<String>() + "\n")
            .collect();
        assert!(screen.contains("parent opus-5"), "{screen}");
        assert!(screen.contains("helper off"), "{screen}");
        assert!(
            screen.contains("subagent claude-sonnet-5"),
            "an unset helper and a set subagent are both named: {screen}"
        );
        assert!(screen.contains("Tab ⇄ assigning to parent"), "{screen}");
    }

    #[test]
    fn the_current_selectable_provider_opens_ahead_of_locked_subscriptions() {
        let panel = Panel::models(
            "Models",
            vec![
                ModelGroup {
                    provider: "anthropic".into(),
                    account: "claude-sub".into(),
                    scope: "account-declared".into(),
                    models: vec!["claude/exact".into()],
                    selectable: Some(false),
                    unavailable_reason: Some("another route is active".into()),
                    connect: None,
                },
                ModelGroup {
                    provider: "google".into(),
                    account: "gemini-sub".into(),
                    scope: "account-declared".into(),
                    models: vec!["gemini/exact".into()],
                    selectable: Some(true),
                    unavailable_reason: None,
                    connect: None,
                },
            ],
            TierModels::default(),
        );
        assert!(panel.rows[0].text.starts_with("google · gemini-sub"));
        assert_eq!(
            panel.rows[panel.selected].command.as_deref(),
            Some("/model gemini/exact")
        );
    }

    /// **The finding this vocabulary exists for: a resting actionable still
    /// looks pressable.**
    ///
    /// Before this, an unselected panel row and a line of prose were the same
    /// pixels — selection was drawn, "you can press this" was not. Asserted on
    /// the background, because that is the affordance: a row that carries a
    /// command is filled with `dock` when resting and `accent` when focused,
    /// and a heading is filled with neither.
    ///
    /// Rows are located by the text they drew rather than by arithmetic on
    /// `inner`: a models panel opens with its provider carousel and search
    /// lines above the list, so counting from the top asserts about the wrong
    /// row and passes for the wrong reason.
    #[test]
    fn an_actionable_row_looks_pressable_even_when_it_is_not_selected() {
        let theme = super::super::Theme::Neon;
        let panel = Panel::models(
            "Models",
            vec![ModelGroup {
                provider: "google".into(),
                account: "gemini-sub".into(),
                scope: "declared".into(),
                models: vec!["gemini/one".into(), "gemini/two".into()],
                selectable: Some(true),
                unavailable_reason: None,
                connect: None,
            }],
            TierModels::default(),
        );
        let mut terminal = Terminal::new(TestBackend::new(60, 20)).unwrap();
        terminal
            .draw(|frame| {
                render_panel(frame, frame.area(), &panel, theme);
            })
            .unwrap();
        let buffer = terminal.backend().buffer();
        let lines: Vec<String> = (0..20)
            .map(|y| (0..60).map(|x| buffer[(x, y)].symbol()).collect())
            .collect();

        // The background of the first non-blank cell of the row that drew
        // `needle` — the pill's own left cap when there is one.
        let fill = |needle: &str| -> Color {
            let y = lines
                .iter()
                .position(|line| line.contains(needle))
                .unwrap_or_else(|| panic!("`{needle}` was not drawn:\n{}", lines.join("\n")));
            let x = lines[y]
                .chars()
                .position(|c| c != ' ')
                .expect("a drawn row has a first character");
            buffer[(u16::try_from(x).unwrap(), u16::try_from(y).unwrap())].bg
        };

        assert_eq!(
            panel.selected, 1,
            "the fixture opens on the first model, so row 2 is the resting one"
        );
        assert_eq!(
            fill("gemini/one"),
            theme.accent(),
            "the focused button is filled with the accent"
        );
        assert_eq!(
            fill("gemini/two"),
            theme.dock(),
            "and an unselected button is still filled — this is the whole point"
        );
        assert_eq!(
            fill("gemini-sub · declared"),
            Color::Reset,
            "a heading is prose and must not be filled"
        );
    }

    #[test]
    fn provider_cards_follow_the_theme_and_show_offscreen_directions_without_overflow() {
        for theme in super::super::Theme::ALL {
            let mut panel = catalogue();
            let mut wide = Terminal::new(TestBackend::new(80, 22)).unwrap();
            wide.draw(|frame| {
                render_panel(frame, frame.area(), &panel, theme);
            })
            .unwrap();
            assert_eq!(wide.backend().buffer()[(2, 1)].bg, theme.accent());
            assert_eq!(wide.backend().buffer()[(27, 1)].bg, theme.dock());
            let mut terminal = Terminal::new(TestBackend::new(44, 22)).unwrap();
            for (moves, left, right) in [(0, false, true), (2, true, true), (2, true, false)] {
                for _ in 0..moves {
                    panel.move_provider(true);
                }
                terminal
                    .draw(|frame| {
                        render_panel(frame, Rect::new(2, 1, 40, 20), &panel, theme);
                    })
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
                        render_panel(frame, frame.area(), &catalogue(), super::super::Theme::Neon);
                    })
                    .unwrap();
            }
        }
    }

    fn geometry(panel: &Panel, width: u16, height: u16) -> PanelGeometry {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        let mut geometry = PanelGeometry::default();
        terminal
            .draw(|frame| {
                geometry = render_panel(frame, frame.area(), panel, super::super::Theme::Neon);
            })
            .unwrap();
        geometry
    }

    #[test]
    fn hit_geometry_is_the_visible_carousel_and_scrolled_model_slice() {
        let mut panel = catalogue();
        let visible = geometry(&panel, 80, 22);
        assert_eq!(
            visible
                .providers
                .iter()
                .map(|(_, index)| *index)
                .collect::<Vec<_>>(),
            [0, 1, 2]
        );
        let first = visible.providers[0].0;
        assert_eq!(visible.hit(first.x, first.y), Some(PanelHit::Provider(0)));
        assert_eq!(
            visible.hit(first.right() - 1, first.bottom() - 1),
            Some(PanelHit::Provider(0))
        );
        assert_eq!(
            visible.hit(first.right(), first.y),
            None,
            "the one-column gap between provider cards is not a hidden target"
        );

        for _ in 0..4 {
            panel.move_provider(true);
        }
        let shifted = geometry(&panel, 80, 22);
        assert_eq!(
            shifted
                .providers
                .iter()
                .map(|(_, index)| *index)
                .collect::<Vec<_>>(),
            [2, 3, 4],
            "offscreen provider tabs must not retain hit targets"
        );

        let mut long = Panel::models(
            "Models",
            vec![ModelGroup {
                provider: "provider".into(),
                account: "account".into(),
                scope: "declared".into(),
                models: (0..20).map(|index| format!("model-{index:02}")).collect(),
                selectable: Some(true),
                unavailable_reason: None,
                connect: None,
            }],
            TierModels::default(),
        );
        long.move_selection(true, 19);
        let scrolled = geometry(&long, 40, 10);
        let drawn: Vec<_> = scrolled.models.iter().map(|(_, index)| *index).collect();
        // Three, because the hint lines now yield to the list on a short
        // panel; the property under test is that what is drawn is hittable
        // and what scrolled off is not.
        assert_eq!(drawn, [18, 19, 20]);
        for (area, index) in &scrolled.models {
            assert_eq!(scrolled.hit(area.x, area.y), Some(PanelHit::Model(*index)));
        }
        assert_eq!(scrolled.hit(40, scrolled.models[0].0.y), None);
        assert_eq!(scrolled.hit(0, 10), None);

        let narrow = geometry(&catalogue(), 5, 10);
        assert!(narrow.providers.is_empty());
        assert!(
            !matches!(narrow.hit(0, 1), Some(PanelHit::Provider(_))),
            "a provider card that was clipped away cannot be clicked"
        );
    }
}
