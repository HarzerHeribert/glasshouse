use super::*;

/// The project's name and the session currently presented.
pub(super) fn render_title(state: &ShellState, frame: &mut Frame, area: Rect) {
    let mut spans = vec![
        Span::styled(
            "GLASSHOUSE".to_owned(),
            Style::default()
                .fg(state.theme().accent())
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
    let title = Line::from(spans);
    let room = usize::from(area.width).saturating_sub(title.width());
    let accessory = if state.mode() == Mode::Control && state.overlay().is_none() {
        format!(
            "t {}  ·  a motion {}  ·  v{} ",
            state.theme().name(),
            if state.motion_paused() { "off" } else { "on" },
            state.version()
        )
    } else {
        format!("{}  ·  v{} ", state.theme().name(), state.version())
    };
    if room >= accessory.len() + 3 {
        let [left, right] = Layout::horizontal([
            Constraint::Min(0),
            Constraint::Length(accessory.len() as u16),
        ])
        .areas(area);
        frame.render_widget(Paragraph::new(title), left);
        frame.render_widget(
            Paragraph::new(accessory).style(Style::default().fg(state.theme().quiet())),
            right,
        );
    } else {
        frame.render_widget(Paragraph::new(title), area);
    }
}

/// The active canonical project root, on its own line, on every frame.
///
/// This is the value the entire isolation model rests on — which project's
/// memory, state, and sessions the user is looking at — so it gets a dedicated
/// line rather than being tucked into a corner. When the line is too narrow the
/// *head* is dropped, not the tail: `…/work/glasshouse` still identifies the
/// project, while `/Users/someone/very/long/…` does not.
pub(super) fn render_root(state: &ShellState, frame: &mut Frame, area: Rect) {
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
pub(super) fn render_session_bar(state: &ShellState, frame: &mut Frame, area: Rect) {
    if state.sessions().is_empty() {
        frame.render_widget(
            Paragraph::new(Span::styled(
                "no sessions yet  /  n new  ·  o overview",
                Style::default().fg(Color::DarkGray),
            )),
            area,
        );
        return;
    }

    let selected = state.selected_index();
    let tabs: Vec<_> = state
        .sessions()
        .iter()
        .enumerate()
        .map(|(i, row)| format!(" {} {} · {} ", i + 1, row.harness, row.lifecycle))
        .collect();
    let mut start = 0;
    while start < selected
        && tabs[start..=selected]
            .iter()
            .map(|s| s.chars().count() + 1)
            .sum::<usize>()
            > usize::from(area.width)
    {
        start += 1;
    }
    let mut spans = Vec::new();
    if start > 0 {
        spans.push(Span::styled(
            "‹ ",
            Style::default().fg(state.theme().quiet()),
        ));
    }
    for (index, label) in tabs.iter().enumerate().skip(start) {
        let row = &state.sessions()[index];
        let color = match row.lifecycle {
            SessionLifecycle::WaitingForUser => Color::Yellow,
            SessionLifecycle::Failed => Color::Red,
            SessionLifecycle::Running | SessionLifecycle::Starting => state.theme().accent(),
            _ => state.theme().quiet(),
        };
        let style = if index == selected {
            Style::default()
                .fg(Color::Black)
                .bg(color)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(color)
        };
        spans.push(Span::styled(label.clone(), style));
        spans.push(Span::raw(" "));
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
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
pub(super) fn render_footer(state: &ShellState, frame: &mut Frame, area: Rect) {
    let hint = match (state.mode(), state.overlay()) {
        (Mode::Session, _) => "SESSION MODE   ctrl-] for glasshouse   keys go to the session",
        (Mode::Control, Some(Overlay::Overview)) => {
            "up/down pick   m send text   c interrupt   esc back to session   q quit"
        }
        (Mode::Control, Some(Overlay::Settings)) => {
            "tab section   up/down move   space toggle   section keys edit   \
             w save   W project   r setup   esc close"
        }
        (Mode::Control, Some(Overlay::ProjectOverview)) => "esc back to session   q quit",
        (Mode::Control, Some(Overlay::SessionEvents)) => "esc back to session   q quit",
        (Mode::Control, Some(Overlay::ProjectKnowledge)) => "esc back to session   q quit",
        (Mode::Control, Some(Overlay::RouteEvidence)) => "esc back to session   q quit",
        (Mode::Control, Some(Overlay::RouteHealth)) => "esc back to session   q quit",
        (Mode::Control, Some(Overlay::RouteDecisions)) => "esc back to session   q quit",
        (Mode::Control, Some(Overlay::ProjectMemory)) => "esc back to session   q quit",
        (Mode::Control, None) => {
            "tab session   enter session   n new   N headless   o overview   q quit   s settings   \
             M memory   p project   k knowledge   e events   r routes   h health   d decisions"
        }
    };
    let mut spans = Vec::new();
    for (index, item) in hint.split("   ").enumerate() {
        if index > 0 {
            spans.push(Span::raw("  "));
        }
        let (key, description) = item.split_once(' ').unwrap_or((item, ""));
        spans.push(Span::styled(
            key.to_owned(),
            Style::default()
                .fg(state.theme().accent())
                .add_modifier(Modifier::BOLD),
        ));
        if !description.is_empty() {
            spans.push(Span::styled(
                format!(" {description}"),
                Style::default().fg(state.theme().quiet()),
            ));
        }
    }

    // Keep the first navigation keys visible while giving feedback real room.
    // Appending it after every shortcut hid launch failures on normal terminals.
    let (keys_area, note_area) = if state.status().is_some() && area.width >= 50 {
        let note_width = state
            .status()
            .map_or(0, |text| text.chars().count() + 2)
            .min(usize::from(area.width / 2)) as u16;
        let [keys, note] =
            Layout::horizontal([Constraint::Min(0), Constraint::Length(note_width)]).areas(area);
        (keys, Some(note))
    } else {
        (area, None)
    };
    frame.render_widget(Paragraph::new(Line::from(spans)), keys_area);
    if let (Some(status), Some(note_area)) = (state.status(), note_area) {
        frame.render_widget(
            Paragraph::new(format!("  {status}")).style(Style::default().fg(Color::Yellow)),
            note_area,
        );
    }
}

/// A quiet launch surface; a live harness grid never passes through this function.
pub(super) fn render_landing(state: &ShellState, frame: &mut Frame, area: Rect) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let theme = state.theme();
    let margin = if area.width >= 50 { 2 } else { 0 };
    let inner = area.inner(ratatui::layout::Margin::new(
        margin,
        if area.height >= 16 { 2 } else { 0 },
    ));
    let roomy = inner.width >= 100 && inner.height >= 20;
    let [body, art] = Layout::horizontal(if roomy {
        [Constraint::Percentage(60), Constraint::Percentage(40)]
    } else {
        [Constraint::Percentage(100), Constraint::Length(0)]
    })
    .areas(inner);
    let mut lines = vec![
        Line::from(Span::styled(
            "YOUR WORK, IN VIEW",
            Style::default()
                .fg(theme.accent())
                .add_modifier(Modifier::BOLD),
        )),
        Line::default(),
    ];
    if let Some(session) = state.active_session() {
        let lifecycle_color = match session.lifecycle {
            SessionLifecycle::Failed => Color::Red,
            SessionLifecycle::WaitingForUser => Color::Yellow,
            _ => theme.secondary(),
        };
        lines.push(Line::from(Span::styled(
            format!("session {}", session.id),
            Style::default().add_modifier(Modifier::BOLD),
        )));
        lines.push(Line::from(format!("harness       {}", session.harness)));
        lines.push(Line::from(vec![
            Span::raw("state         "),
            Span::styled(
                session.lifecycle.to_string(),
                Style::default().fg(lifecycle_color),
            ),
        ]));
        lines.push(Line::from(format!(
            "presented     {}",
            session.presentation
        )));
        lines.push(Line::from(format!("role          {}", session.role)));
        lines.push(Line::default());
        lines.push(Line::from(Span::styled(
            if session.presentation == SessionPresentation::Headless {
                "This session is headless: it runs with no viewport."
            } else {
                "This viewport is reserved for the session's own terminal."
            },
            Style::default().fg(theme.quiet()),
        )));
        lines.push(Line::from(format!(
            "model         {}",
            session
                .model
                .as_ref()
                .map_or("not recorded", |model| model.label())
        )));
        lines.push(Line::from(format!(
            "entitlement   {}",
            session.entitlement.as_deref().unwrap_or("not recorded")
        )));
        lines.push(Line::from(format!(
            "backend       {}",
            session
                .backend_resource
                .as_deref()
                .unwrap_or("not recorded")
        )));
    } else {
        lines.push(Line::from("No session is active."));
        lines.push(Line::from(Span::styled(
            "One project. Your harnesses. A shared view of the work.",
            Style::default().fg(theme.quiet()),
        )));
        lines.push(Line::default());
        lines.push(Line::from(Span::styled(
            "n  start a session",
            Style::default()
                .fg(theme.accent())
                .add_modifier(Modifier::BOLD),
        )));
        lines.push(Line::from("s  configure providers, accounts and defaults"));
        lines.push(Line::from("o  inspect recorded sessions"));
        lines.push(Line::default());
        lines.push(Line::from(Span::styled(
            "Run `glasshouse launch` to start one from your shell.",
            Style::default().fg(theme.quiet()),
        )));
    }
    lines.push(Line::default());
    lines.push(Line::from(Span::styled(
        "OBSERVE & NAVIGATE",
        Style::default()
            .fg(theme.secondary())
            .add_modifier(Modifier::BOLD),
    )));
    lines.push(Line::from(
        "p project   e events   h health   d decisions   r routes",
    ));
    lines.push(Line::from("tab session   enter focus   ctrl-] back here"));
    lines.push(Line::default());
    lines.push(Line::from(Span::styled(
        format!(
            "t theme: {}   a motion: {}",
            theme.name(),
            if state.motion_paused() { "off" } else { "on" }
        ),
        Style::default().fg(theme.quiet()),
    )));
    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), body);
    if roomy {
        let [label, drawing, facts] = Layout::vertical([
            Constraint::Length(2),
            Constraint::Length(8),
            Constraint::Min(0),
        ])
        .areas(art);
        frame.render_widget(
            Paragraph::new("GLASSHOUSE / CONTROL").style(
                Style::default()
                    .fg(theme.accent())
                    .add_modifier(Modifier::BOLD),
            ),
            label,
        );
        frame.render_widget(
            Paragraph::new(super::super::appearance::ribbon(
                drawing.width,
                drawing.height,
                state.artwork_frame(),
                theme,
            )),
            drawing,
        );
        let waiting = state
            .sessions()
            .iter()
            .filter(|s| s.lifecycle == SessionLifecycle::WaitingForUser)
            .count();
        let failed = state
            .sessions()
            .iter()
            .filter(|s| s.lifecycle == SessionLifecycle::Failed)
            .count();
        frame.render_widget(
            Paragraph::new(vec![
                Line::default(),
                Line::from(Span::styled(
                    format!("{} recorded sessions", state.sessions().len()),
                    Style::default().fg(theme.quiet()),
                )),
                Line::from(Span::styled(
                    format!("{waiting} waiting for you"),
                    Style::default().fg(if waiting > 0 {
                        Color::Yellow
                    } else {
                        theme.quiet()
                    }),
                )),
                Line::from(Span::styled(
                    format!("{failed} failed"),
                    Style::default().fg(if failed > 0 {
                        Color::Red
                    } else {
                        theme.quiet()
                    }),
                )),
                Line::default(),
                Line::from(Span::styled(
                    "o opens the session list",
                    Style::default().fg(theme.quiet()),
                )),
            ]),
            facts,
        );
    }
}
