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

use crate::integrations::{IntegrationId, IntegrationKind, IntegrationStatus};

use super::state::{
    BypassRowView, PathInputView, ProviderRow, ProviderStepView, ProviderTemplateRow,
    RoutingChoice, RoutingProviderRow, RoutingSelectionView, RoutingStepView, RowView, Step,
    WizardState,
};

/// Draw the current step of `state` into `frame`.
///
/// Every screen fits an 80x24 terminal without scrolling, except the
/// Summary's worst case (`render_summary`), which scrolls and announces
/// that it has more to show rather than cutting a row silently. Below
/// 80x24, the integration list scrolls to follow the selection, so every
/// row stays reachable rather than being cut off at the bottom edge; the
/// other regions simply get less space from Ratatui's layout solver rather
/// than panicking — nothing here computes a size by subtraction, which is
/// the usual way a "must not panic on a tiny terminal" requirement gets
/// violated.
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
        Step::Bypass => render_bypass_step(state, frame, body_area),
        Step::Provider => render_provider_step(state, frame, body_area),
        Step::Routing => render_routing_step(state, frame, body_area),
        Step::Summary => render_summary(state, frame, body_area),
    }
    render_footer(state, frame, footer_area, body_area);
}

fn render_title(state: &WizardState, frame: &mut Frame, area: Rect) {
    let label = match state.step() {
        Step::Welcome => "Glasshouse setup — welcome",
        Step::Harnesses => "Glasshouse setup — harnesses & integrations",
        Step::Bypass => "Glasshouse setup — bypass acknowledgement (optional)",
        Step::Provider => "Glasshouse setup — provider (optional)",
        Step::Routing => "Glasshouse setup — routing model (optional)",
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

/// The optional bypass-acknowledgement step ([`Step::Bypass`]).
///
/// Shows each qualifying harness's own declared description and argv —
/// never a paraphrase, per Amendment 1 line 2 — so the user sees exactly
/// what will be passed to the harness before acknowledging anything.
fn render_bypass_step(state: &WizardState, frame: &mut Frame, area: Rect) {
    let mut lines = vec![
        Line::from(""),
        Line::from(
            "Optional, off by default: a harness with no automatic-review mode can \
             only run unattended with a blanket approval bypass, which skips every \
             check entirely. Acknowledging is per harness and changes nothing else — \
             declining is fine, and every enabled harness keeps working exactly as it \
             already does.",
        ),
        Line::from(""),
    ];

    let rows: Vec<_> = state.bypass_rows().collect();
    if rows.is_empty() {
        lines.push(Line::from(
            "No known harness needs this — nothing to decide here.",
        ));
    } else {
        for row in rows {
            lines.push(bypass_row_line(row));
            lines.push(Line::from(Span::styled(
                format!("        argv: {}", row.args.join(" ")),
                Style::default().fg(Color::DarkGray),
            )));
        }
    }

    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), area);
}

fn bypass_row_line(row: BypassRowView<'_>) -> Line<'static> {
    let cursor = if row.selected { "> " } else { "  " };
    let mark = if row.acknowledged { "[x]" } else { "[ ]" };
    let mark_color = if row.acknowledged {
        Color::Green
    } else {
        Color::DarkGray
    };
    let mut style = Style::default();
    if row.selected {
        style = style.add_modifier(Modifier::BOLD);
    }
    Line::from(vec![
        Span::styled(cursor.to_owned(), style),
        Span::styled(format!("{mark} "), Style::default().fg(mark_color)),
        Span::styled(format!("{:<14}", row.id.display_name()), style),
        Span::raw(format!(" {}", row.description)),
    ])
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

/// The optional provider step ([`Step::Provider`]): "Configure now" against
/// "Do later", and whichever sub-screen "Configure now" opened.
fn render_provider_step(state: &WizardState, frame: &mut Frame, area: Rect) {
    match state.provider_step() {
        ProviderStepView::Choice {
            configure_now_selected,
            providers,
        } => render_provider_choice(configure_now_selected, &providers, frame, area),
        ProviderStepView::PickTemplate { options } => {
            render_provider_templates(&options, frame, area)
        }
        ProviderStepView::BaseUrlInput {
            template,
            buffer,
            error,
        } => render_provider_base_url(&template, &buffer, error.as_deref(), frame, area),
    }
}

fn render_provider_choice(
    configure_now_selected: bool,
    providers: &[ProviderRow],
    frame: &mut Frame,
    area: Rect,
) {
    let mut lines = vec![
        Line::from(""),
        Line::from(
            "Provider and gateway configuration is optional. Every native, \
             subscription-backed harness enabled on the previous step already works \
             without it, and finishing with \"Do later\" asks for no API key of any kind.",
        ),
        Line::from(""),
        choice_line("Configure now", configure_now_selected),
        choice_line("Do later", !configure_now_selected),
    ];

    if !providers.is_empty() {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "Already configured",
            Style::default().add_modifier(Modifier::BOLD),
        )));
        for provider in providers {
            let url = provider.base_url.as_deref().unwrap_or("template default");
            lines.push(Line::from(format!(
                "  {:<16} template {:<20} {url}",
                provider.name, provider.template
            )));
        }
    }

    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), area);
}

fn choice_line(label: &str, selected: bool) -> Line<'static> {
    let cursor = if selected { "> " } else { "  " };
    let mut style = Style::default();
    if selected {
        style = style.add_modifier(Modifier::BOLD);
    }
    Line::from(Span::styled(format!("{cursor}{label}"), style))
}

fn render_provider_templates(options: &[ProviderTemplateRow], frame: &mut Frame, area: Rect) {
    let mut lines = vec![Line::from("Choose a provider template:"), Line::from("")];
    for option in options {
        let cursor = if option.selected { "> " } else { "  " };
        let url = if option.base_url.is_empty() {
            "(you supply the base URL)".to_owned()
        } else {
            option.base_url.clone()
        };
        let mut style = Style::default();
        if option.selected {
            style = style.add_modifier(Modifier::BOLD);
        }
        lines.push(Line::from(Span::styled(
            format!("{cursor}{:<20} {:<24} {url}", option.name, option.protocols),
            style,
        )));
    }
    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), area);
}

fn render_provider_base_url(
    template: &str,
    buffer: &str,
    error: Option<&str>,
    frame: &mut Frame,
    area: Rect,
) {
    let mut lines = vec![Line::from(format!("Base URL for `{template}`: {buffer}_"))];
    if let Some(error) = error {
        lines.push(Line::from(Span::styled(
            error.to_owned(),
            Style::default().fg(Color::Red),
        )));
    }
    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), area);
}

/// The optional routing-model step ([`Step::Routing`]): which model, if any,
/// classifies a request before Glasshouse spends capacity on it, plus
/// whichever sub-screen "Choose model" opened.
///
/// Deliberately the same shape as [`render_provider_step`] one step earlier —
/// three offers, a picker, a text field — because the two optional steps are
/// answered the same way and so should look and move the same way.
fn render_routing_step(state: &WizardState, frame: &mut Frame, area: Rect) {
    match state.routing_step() {
        RoutingStepView::Choice {
            selected,
            recorded,
            can_choose_model,
            notice,
        } => render_routing_choice(
            selected,
            &recorded,
            can_choose_model,
            notice.as_deref(),
            frame,
            area,
        ),
        RoutingStepView::PickProvider { options } => {
            render_routing_providers(&options, frame, area)
        }
        RoutingStepView::ModelInput {
            provider,
            buffer,
            error,
        } => render_routing_model_input(&provider, &buffer, error.as_deref(), frame, area),
    }
}

/// The three offers of [`RoutingChoice`], why the last key press did nothing,
/// and what is recorded right now.
///
/// `can_choose_model` never removes the "Choose model" row. An option that
/// vanishes reads as a bug in the wizard; an option that is present and says
/// what it needs tells the user how to get it, which is why the unavailable
/// wording names the missing prerequisite instead of the row simply not
/// being drawn.
fn render_routing_choice(
    selected: RoutingChoice,
    recorded: &RoutingSelectionView,
    can_choose_model: bool,
    notice: Option<&str>,
    frame: &mut Frame,
    area: Rect,
) {
    let choose_model = if can_choose_model {
        "Choose model — pin classification to one specific model".to_owned()
    } else {
        "Choose model — pin classification to one specific model (unavailable: needs a \
         configured provider)"
            .to_owned()
    };

    let mut lines = vec![
        Line::from(""),
        Line::from(
            "Optional. Before spending premium agent capacity on a request, Glasshouse \
             can ask one cheap, fast model to classify it and say which resource should \
             handle it. Nothing is routed by this setting yet — it records the intent — \
             and leaving it for later keeps a fully working system on deterministic \
             routing heuristics.",
        ),
        Line::from(""),
        choice_line(
            "Automatic — the cheapest sufficiently fast configured resource, chosen when a \
             decision is actually needed",
            selected == RoutingChoice::Automatic,
        ),
        choice_line(&choose_model, selected == RoutingChoice::ChooseModel),
        choice_line(
            "Do later — deterministic routing heuristics until configured",
            selected == RoutingChoice::DoLater,
        ),
    ];

    // Yellow, not the red the input errors use: a notice reports a press that
    // was refused for an ordinary reason, and nothing the user did was wrong.
    // Red here would tell them they had made a mistake.
    if let Some(notice) = notice {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            notice.to_owned(),
            Style::default().fg(Color::Yellow),
        )));
    }

    lines.push(Line::from(""));
    lines.extend(routing_selection_lines("Currently recorded:", recorded));

    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), area);
}

/// One sentence for whatever [`WizardState::routing_selection`] currently
/// reports, prefixed by `label`.
///
/// Shared by the routing step and the Summary so the two cannot describe the
/// same recorded choice differently. The
/// [`RoutingSelectionView::PinnedUnavailable`] arm renders its `message`
/// **verbatim** on its own line: that string is
/// [`crate::config::RoutingFallback`]'s own explanation of the degrade, and
/// restating it here in the wizard's words is exactly the drift the view type
/// carries it to prevent.
fn routing_selection_lines(label: &str, selection: &RoutingSelectionView) -> Vec<Line<'static>> {
    let headline = match selection {
        RoutingSelectionView::NotConfigured => "none configured; deterministic routing \
             heuristics classify requests until one is, which is a working system rather \
             than a gap."
            .to_owned(),
        RoutingSelectionView::Deterministic => "deterministic-only, on purpose — no model \
             is asked, and deterministic routing heuristics classify requests."
            .to_owned(),
        RoutingSelectionView::Automatic => "automatic — the resource is chosen at the \
             moment a decision is actually needed, not now."
            .to_owned(),
        RoutingSelectionView::Pinned { provider, model }
        | RoutingSelectionView::PinnedUnavailable {
            provider, model, ..
        } => format!("`{model}` from provider `{provider}`."),
    };
    let mut lines = vec![Line::from(format!("{label} {headline}"))];
    if let RoutingSelectionView::PinnedUnavailable { message, .. } = selection {
        lines.push(Line::from(Span::styled(
            message.clone(),
            Style::default().fg(Color::Yellow),
        )));
    }
    lines
}

/// The providers a pinned routing model may be chosen from — the same set
/// [`WizardState::configured_providers`] reports, so a provider configured a
/// step earlier in this same run is offered here immediately.
fn render_routing_providers(options: &[RoutingProviderRow], frame: &mut Frame, area: Rect) {
    let mut lines = vec![
        Line::from("Configured providers — choose the one the routing model belongs to:"),
        Line::from(""),
    ];
    for option in options {
        let cursor = if option.selected { "> " } else { "  " };
        let mut style = Style::default();
        if option.selected {
            style = style.add_modifier(Modifier::BOLD);
        }
        lines.push(Line::from(Span::styled(
            format!("{cursor}{:<20} template {}", option.name, option.template),
            style,
        )));
    }
    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), area);
}

/// The model-name field, which names the provider being pinned to in its own
/// prompt — a model name alone would not say who is being asked.
fn render_routing_model_input(
    provider: &str,
    buffer: &str,
    error: Option<&str>,
    frame: &mut Frame,
    area: Rect,
) {
    let mut lines = vec![Line::from(format!(
        "Routing model to pin from `{provider}`: {buffer}_"
    ))];
    if let Some(error) = error {
        lines.push(Line::from(Span::styled(
            error.to_owned(),
            Style::default().fg(Color::Red),
        )));
    }
    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), area);
}

/// Draw the Summary, scrolled to `state.summary_scroll()` and clamped to
/// what actually fits — see [`render_summary`] for the overflow decision.
fn render_summary(state: &WizardState, frame: &mut Frame, area: Rect) {
    let lines = summary_lines(state, area.width);

    // `Paragraph::line_count` would answer this directly, but it is
    // `pub(crate)` in this ratatui version unless the `unstable-rendered-
    // line-info` feature is enabled, and enabling a new feature is outside
    // this packet. `wrap_summary_rows` hand-rolls the same `Wrap { trim:
    // false }` word-boundary wrapping instead — see its doc.
    let rows = wrap_summary_rows(&lines, area.width);
    let total = rows.len() as u16;

    if total <= area.height {
        // Byte-for-byte what this function has always rendered: no scroll
        // is possible, so none is applied, and the hand-rolled wrap above
        // was only consulted to reach this branch.
        frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), area);
        return;
    }

    let max_scroll = total.saturating_sub(area.height);
    let offset = state.summary_scroll().min(max_scroll);
    let start = offset as usize;
    let end = (start + area.height as usize).min(rows.len());
    let mut visible: Vec<Line<'static>> = rows[start..end].to_vec();

    let remaining_below = total.saturating_sub(end as u16);
    if remaining_below > 0 {
        let noun = if remaining_below == 1 { "row" } else { "rows" };
        let last = visible.len() - 1;
        visible[last] = Line::from(format!("↓ {remaining_below} more {noun} — Down / PageDown"))
            .right_aligned();
    }
    if offset > 0 {
        let noun = if offset == 1 { "row" } else { "rows" };
        visible[0] = Line::from(format!("↑ {offset} {noun} above")).right_aligned();
    }

    frame.render_widget(Paragraph::new(visible), area);
}

/// Whether the Summary body, at `state.summary_scroll()` and the given
/// area, has more content than the area shows — the condition
/// `render_footer` needs to decide whether the scroll hint belongs in the
/// footer, without duplicating [`render_summary`]'s own wrapping.
fn summary_overflows(state: &WizardState, width: u16, height: u16) -> bool {
    wrap_summary_rows(&summary_lines(state, width), width).len() as u16 > height
}

fn summary_lines(state: &WizardState, width: u16) -> Vec<Line<'static>> {
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
        let head = format!("  {:<12} {decision}", row.id.display_name());
        let path = if row.decision == Some(true) {
            row.executable.map(|p| p.display().to_string())
        } else {
            None
        };
        lines.push(Line::from(summary_row(&head, path.as_deref(), width)));
    }

    let acknowledged: Vec<&str> = state
        .bypass_rows()
        .filter(|row| row.acknowledged)
        .map(|row| row.id.display_name())
        .collect();
    if !acknowledged.is_empty() {
        lines.push(Line::from(""));
        lines.push(Line::from(format!(
            "Bypass acknowledged: {}",
            acknowledged.join(", ")
        )));
    }

    lines.push(Line::from(""));
    let providers = state.configured_providers();
    if providers.is_empty() {
        lines.push(Line::from(
            "No provider is configured. No Glasshouse API key is required to finish; \
             enabled native harnesses keep using their existing authentication.",
        ));
    } else {
        lines.push(Line::from(Span::styled(
            "Providers",
            Style::default().add_modifier(Modifier::BOLD),
        )));
        for provider in &providers {
            let url = provider.base_url.as_deref().unwrap_or("template default");
            lines.push(Line::from(format!(
                "  {:<16} template {:<20} {url}",
                provider.name, provider.template
            )));
        }
    }
    lines.push(Line::from(""));
    lines.extend(routing_selection_lines(
        "Routing model:",
        &state.routing_selection(),
    ));
    // No blank separator before the gateway note — see
    // `every_summary_section_survives_the_worst_case_at_80x24`.
    lines.push(Line::from(
        "The Glasshouse gateway is not part of this setup yet.",
    ));
    lines
}

/// Wraps `lines` to `width` columns the way `Paragraph`'s
/// `Wrap { trim: false }` would, one output row per element.
///
/// Hand-rolled because `Paragraph::line_count` is `pub(crate)` in this
/// ratatui version unless the `unstable-rendered-line-info` feature is
/// enabled, and this packet does not touch `Cargo.toml` to add it (GH-
/// SUMMARY-SCROLL). Every Summary line is plain, single-spaced text — the
/// one styled line, "Providers", is a short heading that never approaches
/// `width` — so a line that already fits keeps its own styling untouched,
/// and only a line that overflows is re-wrapped as plain text: split on
/// whitespace, greedily pack words onto a row, and break a word mid-way
/// only when the word alone is longer than `width`.
fn wrap_summary_rows(lines: &[Line<'static>], width: u16) -> Vec<Line<'static>> {
    if width == 0 {
        return vec![Line::from("")];
    }
    let width = width as usize;
    let mut rows = Vec::new();
    for line in lines {
        if line.width() <= width {
            rows.push(line.clone());
            continue;
        }
        for row in wrap_plain_text(&line.to_string(), width) {
            rows.push(Line::from(row));
        }
    }
    if rows.is_empty() {
        rows.push(Line::from(""));
    }
    rows
}

/// Greedy word-wrap for one already-overflowing line — see
/// [`wrap_summary_rows`].
fn wrap_plain_text(text: &str, width: usize) -> Vec<String> {
    if text.is_empty() {
        return vec![String::new()];
    }
    let mut rows = Vec::new();
    let mut current = String::new();
    for word in text.split_whitespace() {
        let word_len = word.chars().count();
        if word_len > width {
            if !current.is_empty() {
                rows.push(std::mem::take(&mut current));
            }
            let mut rest = word;
            while rest.chars().count() > width {
                let split_at = rest
                    .char_indices()
                    .nth(width)
                    .map_or(rest.len(), |(idx, _)| idx);
                let (head, tail) = rest.split_at(split_at);
                rows.push(head.to_owned());
                rest = tail;
            }
            current = rest.to_owned();
            continue;
        }
        if current.is_empty() {
            current = word.to_owned();
        } else if current.chars().count() + 1 + word_len <= width {
            current.push(' ');
            current.push_str(word);
        } else {
            rows.push(std::mem::take(&mut current));
            current = word.to_owned();
        }
    }
    rows.push(current);
    rows
}

/// One integration's Summary row, guaranteed to occupy exactly one terminal
/// row.
///
/// The Summary is rendered as a wrapped paragraph, and a wrapped paragraph has
/// no scrollback: anything past the bottom edge is not drawn, and nothing says
/// so. An executable path is the one part of this screen whose length is set
/// by the user's machine rather than by us — a harness installed under a
/// macOS temporary directory produces a path near 90 characters, which wraps
/// onto three rows at 80 columns. Ten such rows pushed the routing model, the
/// providers and the gateway line clean off an 80x24 terminal, which is how
/// the review screen came to omit the decision the user had just made. Found
/// by running the binary; no rendering test caught it, because each one
/// rendered a fixture with short paths.
///
/// So the path is elided from the *left*, keeping its tail: the executable's
/// own name is what identifies it, and the directory prefix is both the long
/// part and the part already shown in full on the Harnesses step.
fn summary_row(head: &str, path: Option<&str>, width: u16) -> String {
    let Some(path) = path else {
        return head.to_owned();
    };
    let width = width as usize;
    let head_len = head.chars().count();
    // " (" + ")" around the path, and at least one character of path left to
    // show. Below that there is no room for a path at all, and the decision
    // itself matters more than where the binary lives.
    let budget = width.saturating_sub(head_len + 3);
    if budget == 0 {
        return head.to_owned();
    }
    let path_len = path.chars().count();
    if path_len <= budget {
        return format!("{head} ({path})");
    }
    let tail: String = path
        .chars()
        .skip(path_len - budget.saturating_sub(1))
        .collect();
    format!("{head} (\u{2026}{tail})")
}

fn render_footer(state: &WizardState, frame: &mut Frame, area: Rect, body_area: Rect) {
    let text = if state.path_input().is_some() {
        "Type path   Enter confirm   Esc cancel input   Ctrl+C quit setup"
    } else {
        match state.step() {
            Step::Welcome => "Enter / Tab continue   Esc cancel",
            Step::Harnesses => {
                if state.rows().any(|row| row.id == IntegrationId::Cmux) {
                    "↑/↓ or j/k move   Space/Enter toggle or add path   Tab continue   Esc cancel"
                } else {
                    "↑/↓ or j/k move   Space/Enter toggle or add path   c add cmux   Tab \
                     continue   Esc cancel"
                }
            }
            Step::Bypass => "↑/↓ or j/k move   Space/Enter toggle   Tab continue   Esc cancel",
            Step::Provider => match state.provider_step() {
                ProviderStepView::Choice { .. } => {
                    "↑/↓ choose   Enter/Space select   Tab skip   Esc cancel"
                }
                ProviderStepView::PickTemplate { .. } => "↑/↓ move   Enter/Space choose   Esc back",
                ProviderStepView::BaseUrlInput { .. } => "Type URL   Enter confirm   Esc back",
            },
            Step::Routing => match state.routing_step() {
                RoutingStepView::Choice { .. } => {
                    "↑/↓ choose   Enter/Space select   Tab skip   Esc cancel"
                }
                RoutingStepView::PickProvider { .. } => "↑/↓ move   Enter/Space choose   Esc back",
                RoutingStepView::ModelInput { .. } => "Type model   Enter confirm   Esc back",
            },
            Step::Summary => {
                if summary_overflows(state, body_area.width, body_area.height) {
                    "Enter / Tab finish   ↑↓ PgUp PgDn scroll   Esc cancel"
                } else {
                    "Enter / Tab finish   Esc cancel"
                }
            }
        }
    };
    frame.render_widget(
        Paragraph::new(text).style(Style::default().fg(Color::DarkGray)),
        area,
    );
}

#[cfg(test)]
mod tests;
