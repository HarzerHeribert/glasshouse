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

/// Every session known to the project, as a bar of tabs — one pill each.
///
/// Clicking a tab is `Tab`/`BackTab` pressed the right number of times, which
/// is the same door the keyboard uses rather than a second way to move the
/// cursor; see [`hotspot::walk_to`].
///
/// **The strip pans, it does not wrap** — [`hotspot::render_panned_bar`], not
/// [`hotspot::render_bar`]. It is given one row in both the places it is
/// drawn (`view::regions` reserves one, and [`render_header`] has only one
/// line to give), so a wrapped tab lands on a row that does not exist: with
/// three sessions at eighty columns the cursor moved to the third, the
/// viewport switched to it, and the strip went on showing the first two with
/// the focus marker on neither.
pub(super) fn render_session_bar(
    state: &ShellState,
    frame: &mut Frame,
    area: Rect,
    sink: &mut Vec<Hotspot>,
) {
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
    let pills: Vec<Pill> = state
        .sessions()
        .iter()
        .enumerate()
        .map(|(index, row)| tab_pill(index, row, selected))
        .collect();
    hotspot::render_panned_bar(frame, area, &pills, selected, state.theme(), sink);
}

/// One tab. Written once so [`render_header`] can reserve exactly what
/// [`render_session_bar`] is going to draw rather than restating its
/// arithmetic — get that wrong and the header drops the project root a few
/// columns early, or clips the strip.
fn tab_pill(index: usize, row: &SessionRecord, selected: usize) -> Pill {
    // The number is part of the label, not a mnemonic: a tab has no key of
    // its own to press — reaching it is `Tab` however many times — and one
    // span keeps `2 claude-code · running` the single styled run it has
    // always been. See `Pill::pieces`.
    Pill::run(
        "",
        &format!("{} {} · {}", index + 1, row.harness, row.lifecycle),
        hotspot::walk_to(selected, index, KeyCode::BackTab, KeyCode::Tab),
    )
    .focused(index == selected)
}

/// The room one tab takes, as a string of that many columns.
///
/// The header's fields are measured as text, and the tab strip's "text" is
/// only ever a width — [`render_session_bar`] draws the strip itself. Taken
/// from the pill rather than restated, so the two cannot disagree.
fn tab_label(index: usize, row: &SessionRecord) -> String {
    " ".repeat(usize::from(tab_pill(index, row, index).width()))
}

/// A field of session mode's collapsed header.
#[derive(Clone, Copy, PartialEq, Eq)]
enum HeaderField {
    Wordmark,
    Tabs,
    Root,
    Model,
    /// The chord that moves the keyboard between this header and the session
    /// under it, named for the direction it would move it *now*.
    Focus,
    Exit,
    /// `ctrl-]`, the other spelling of the byte [`ESCAPE_CHORD`] names.
    ///
    /// Its own field, and the first one dropped, because it is the only
    /// entry on this row that adds no capability: it is the same `0x1D`
    /// [`HeaderField::Exit`] already names, and on the German Mac layout this
    /// is developed on it is the spelling the keyboard cannot produce. With a
    /// second chord on the row the header can no longer afford to spell one
    /// of them twice at every width — and a narrow terminal that kept the
    /// alias would be spending columns on nothing.
    ExitAlias,
}

/// The order the header sheds fields as the terminal narrows — first here is
/// first to go, and whatever is last is what a one-column terminal still gets.
///
/// This ordering is the decision the collapse makes. The tab strip is how one
/// focused session is left for another and `ctrl-] back` is the only way out
/// of session mode at all, so both sit behind the branding, the root and the
/// model rather than competing with them for room. Move the wordmark past
/// them and a narrow terminal keeps the logo while losing the way back, which
/// is the failure [`render_footer`]'s design note exists to prevent.
///
/// [`HeaderField::Focus`] goes before the tab strip and the way out, and
/// after the branding: it is the only field here that is a convenience rather
/// than an escape — `Enter` moves the keyboard back into the session from a
/// focused header, and the escape chord leaves entirely, so a terminal too
/// narrow to name the focus chord has lost a label and not a capability.
/// [`HeaderField::ExitAlias`] goes before even the branding, because it is
/// the one field that names nothing the row does not already name.
const DROP_ORDER: [HeaderField; 7] = [
    HeaderField::ExitAlias,
    HeaderField::Wordmark,
    HeaderField::Root,
    HeaderField::Model,
    HeaderField::Focus,
    HeaderField::Tabs,
    HeaderField::Exit,
];

/// The gap between header fields, and the width every field reserves for it.
const GAP: &str = "   ";

/// The ground the header is painted on while it holds the keyboard.
///
/// Dark and neutral on purpose — see [`render_header`]: it has to sit under
/// spans that already carry accent, white and quiet foregrounds, so it lifts
/// the row without taking a colour away from anything on it.
const FOCUS_FILL: Color = Color::Rgb(30, 40, 46);

/// Session mode's single line of chrome: the wordmark, the tab strip, what
/// survived from the title and root bands, and the way out.
///
/// A ribbon that drops fields rather than wrapping — a second line would take
/// back a row this mode exists to hand the harness — in [`DROP_ORDER`], and
/// never past the last field standing, so `ctrl-] back` is on screen at every
/// width a terminal can actually draw.
pub(super) fn render_header(
    state: &ShellState,
    frame: &mut Frame,
    area: Rect,
    sink: &mut Vec<Hotspot>,
) {
    let mut kept = header_fields(state);
    for field in DROP_ORDER {
        if kept.len() <= 1 || header_width(&kept) <= usize::from(area.width) {
            break;
        }
        kept.retain(|(candidate, _)| *candidate != field);
    }
    let value = |wanted: HeaderField| {
        kept.iter()
            .find(|(field, _)| *field == wanted)
            .map(|(_, text)| text.clone())
    };

    let theme = state.theme();
    let focused = state.header_focused();
    // **The band itself says who has the keyboard.** Painted before anything
    // else and across the whole row, so it reads as one lit strip rather than
    // as a highlight on some word — the user's requirement was that the state
    // be unmistakable. A dark fill rather than the accent: every span in this
    // row carries a foreground colour of its own and an accent ground would
    // have made the accent-coloured ones vanish into it.
    if focused {
        frame.render_widget(
            Block::default().style(Style::default().bg(FOCUS_FILL)),
            area,
        );
    }
    let mut accessory: Vec<Span> = Vec::new();
    for text in [value(HeaderField::Root), value(HeaderField::Model)]
        .into_iter()
        .flatten()
    {
        if !accessory.is_empty() {
            accessory.push(Span::raw(GAP));
        }
        accessory.push(Span::styled(text, Style::default().fg(Color::White)));
    }
    if let Some(focus) = value(HeaderField::Focus) {
        if !accessory.is_empty() {
            accessory.push(Span::raw(GAP));
        }
        let (chord, target) = focus.split_once(' ').unwrap_or((focus.as_str(), ""));
        accessory.push(Span::styled(
            chord.to_owned(),
            Style::default()
                .fg(theme.accent())
                .add_modifier(Modifier::BOLD),
        ));
        accessory.push(Span::styled(
            format!(" {target}"),
            Style::default().fg(theme.quiet()),
        ));
    }
    if let Some(exit) = value(HeaderField::Exit) {
        if !accessory.is_empty() {
            accessory.push(Span::raw(GAP));
        }
        let (chord, back) = exit.split_once(' ').unwrap_or((exit.as_str(), ""));
        accessory.push(Span::styled(
            chord.to_owned(),
            Style::default()
                .fg(theme.accent())
                .add_modifier(Modifier::BOLD),
        ));
        accessory.push(Span::styled(
            format!(" {back}"),
            Style::default().fg(theme.quiet()),
        ));
    }
    if let Some(alias) = value(HeaderField::ExitAlias) {
        accessory.push(Span::styled(
            format!(" {alias}"),
            Style::default().fg(theme.quiet()),
        ));
    }
    let accessory = Line::from(accessory);

    let [wordmark_area, tabs_area, accessory_area] = Layout::horizontal([
        Constraint::Length(value(HeaderField::Wordmark).map_or(0, |text| field_cells(&text))),
        Constraint::Min(0),
        Constraint::Length(u16::try_from(accessory.width()).unwrap_or(u16::MAX)),
    ])
    .areas(area);

    if let Some(wordmark) = value(HeaderField::Wordmark) {
        // The second half of the focus signal, and the one that survives a
        // terminal too narrow to fill much: the wordmark becomes a solid
        // chip. Kept to the wordmark rather than applied to the whole line,
        // because the tab strip beside it draws its own selected-tab
        // affordance and two competing inversions read as neither.
        let style = if focused {
            Style::default()
                .bg(theme.accent())
                .fg(Color::Black)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default()
                .fg(theme.accent())
                .add_modifier(Modifier::BOLD)
        };
        frame.render_widget(Paragraph::new(Span::styled(wordmark, style)), wordmark_area);
    }
    if value(HeaderField::Tabs).is_some() {
        render_session_bar(state, frame, tabs_area, sink);
    }
    frame.render_widget(Paragraph::new(accessory), accessory_area);
}

/// Every field the header could carry, with the text it would draw.
///
/// The tab strip's entry carries the *selected* tab's label: the strip pans
/// and clips itself, so the width it needs reserved is the one tab it must
/// always show, not the whole row of them.
fn header_fields(state: &ShellState) -> Vec<(HeaderField, String)> {
    let selected = state.selected_index();
    let mut fields = vec![
        (HeaderField::Wordmark, "GLASSHOUSE".to_owned()),
        (
            HeaderField::Tabs,
            state
                .sessions()
                .get(selected)
                .map_or_else(String::new, |row| tab_label(selected, row)),
        ),
    ];
    // The tail identifies the project and the head does not — the same
    // reasoning `render_root` truncates by, at the width a ribbon can afford.
    if let Some(name) = state.project_root().file_name() {
        fields.push((HeaderField::Root, format!("…/{}", name.to_string_lossy())));
    }
    if let Some(model) = state
        .active_session()
        .and_then(|session| session.model.as_ref())
    {
        fields.push((HeaderField::Model, model.label().to_owned()));
    }
    // `ctrl-5` first, and `ctrl-]` kept beside it. They are the same byte
    // (0x1D), but `]` on a German Mac layout is Right-Option-6 and the chord
    // is unreachable there — a user who cannot type the way out has not been
    // given one. `F12` escapes too (see `state::overview::is_session_escape`)
    // and is named in the wider hints below, where there is room for it —
    // but it is named *after* the other two and never as the reliable one:
    // measured on the development Mac, `defaults read -g
    // com.apple.keyboard.fnState` does not exist, so the F-row defaults to
    // its media keys and F12 is Volume-Up. It reaches the application only
    // with Fn held. `ctrl-5` is the chord that works there, and it is the
    // byte the pty tests send.
    // Named for what pressing it would do *from here*, not for what it is:
    // a chord labelled "focus" tells the user nothing about which of the two
    // things on screen would end up with the keyboard.
    fields.push((
        HeaderField::Focus,
        format!(
            "{FOCUS_CHORD} {}",
            if state.header_focused() {
                "session"
            } else {
                "header"
            }
        ),
    ));
    fields.push((HeaderField::Exit, format!("{ESCAPE_CHORD} back")));
    fields.push((HeaderField::ExitAlias, "· ctrl-] back".to_owned()));
    fields
}

/// What a set of header fields needs, each one's gap included.
fn header_width(fields: &[(HeaderField, String)]) -> usize {
    fields
        .iter()
        .map(|(_, text)| text.chars().count() + GAP.len())
        .sum()
}

/// One field's own columns, gap included, clamped rather than wrapped: a
/// width no terminal has is still a width the layout must be given.
fn field_cells(text: &str) -> u16 {
    u16::try_from(text.chars().count() + GAP.len()).unwrap_or(u16::MAX)
}

/// The status note, painted over the session viewport's own top row.
///
/// The session layout has one line of chrome and it is spoken for, so a note
/// set while a session is on screen has nowhere of its own to go — and
/// `Enter` on a stopped session sets exactly such a note (*"resuming `abc123`
/// …"*, then the run loop's answer). Without this the key that resumes a
/// session would say nothing at all about what it did.
///
/// It costs no row: `super::viewport_slot` has already handed the session
/// every line below the header, and these cells are repainted from the
/// emulator on the next frame. Right-aligned because a harness draws its own
/// title on the left.
///
/// **What it no longer carries is the way out.** This began as fullscreen's
/// entire chrome — a persistent `ctrl-5 back` badge, added after a user
/// entered a frame-free session, typed, and could not leave. The header is
/// now on screen in every session layout with that chord on it, so the badge
/// would be a second copy of a fact that no longer disappears. See
/// `design-decisions.md`, "The fullscreen escape chord is chrome", which this
/// supersedes rather than contradicts: the chord is chrome, and it is chrome
/// that is always drawn.
pub(super) fn render_status_badge(state: &ShellState, frame: &mut Frame, area: Rect) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let Some(note) = state.status() else {
        return;
    };
    let line = Line::from(Span::styled(
        format!(" {note} "),
        Style::default().fg(Color::Black).bg(Color::Yellow),
    ));
    let width = u16::try_from(line.width())
        .unwrap_or(u16::MAX)
        .min(area.width);
    let [_, badge_area] = Layout::horizontal([Constraint::Min(0), Constraint::Length(width)])
        .areas(Rect::new(area.x, area.y, area.width, 1));
    frame.render_widget(Paragraph::new(line), badge_area);
}

/// The bottom band: which mode owns the keyboard, Glasshouse's own actions as
/// a row of pills, plus a note when the last key needs explaining.
///
/// **Every action is drawn or none is: the bar wraps rather than clips.** The
/// hint it replaces was one 168-column `Paragraph` with no `.wrap()`, so at 80
/// columns it stopped after `q quit` and eight of the fifteen actions —
/// settings, memory, project, knowledge, events, routes, health, decisions —
/// were on no screen at all, with nothing to say they existed. `regions`
/// reserves the rows [`hotspot::control_bar_rows`] says this needs.
///
/// **A note gets a full-width row of its own**, and the bar keeps the whole
/// width on every row it has. It used to take the right-hand
/// side instead — up to half the terminal — and that was two defects at once.
/// The bar was then painted into a `keys_area` narrower than the width
/// `hotspot::control_band_rows` had measured the reservation at, so it
/// wrapped past the rows it had and stopped: measured at 80x24 with a note
/// showing, eight of the fifteen actions were drawn and seven — `M memory`,
/// `p project`, `k knowledge`, `e events`, `r routes`, `h health`,
/// `d decisions` — were on no screen and had no hotspot. And the note itself
/// was clamped to `width / 2`, which at eighty columns cut every one of them
/// mid-answer: `session \`c0d538f1877a\` exited — back i`. The clause a note
/// exists for is its last one.
///
/// The row is reserved whether or not a note is showing, so the band's height
/// still does not depend on the shell's state — see
/// [`hotspot::control_band_rows`] — and the split is taken from the band that
/// was handed in, so the reservation and the paint are the same rectangles.
///
/// **The reserved row is the band's first and the bar owns its last**, which
/// is the terminal's last: a row nothing writes is a row ratatui's diff never
/// mentions, so the note's row at the foot left the bottom line of the
/// terminal unaddressed in every state where no note was showing. See
/// [`hotspot::split_band`], which carries the measurement and the one state
/// that moves the note back.
///
/// Control mode's band. Session mode collapses its four rows of chrome into
/// [`render_header`], which is what carries the escape chord there. The
/// session arm below survives because the mode is a runtime value and a band
/// drawn without one would say nothing at all.
pub(super) fn render_footer(
    state: &ShellState,
    frame: &mut Frame,
    area: Rect,
    sink: &mut Vec<Hotspot>,
) {
    // The note moves to the foot of the band only when there is a note to put
    // there: an overlay covers the band's first rows, and giving up the last
    // row to something nobody drew is the defect this whole split exists for.
    let (keys_area, note_area) =
        hotspot::split_band(area, state.overlay().is_some() && state.status().is_some());

    if state.mode() == Mode::Control && state.overlay().is_none() {
        let pills = hotspot::control_pills(state.theme());
        hotspot::render_tiered_bar(
            frame,
            keys_area,
            &pills,
            hotspot::CONTROL_PRIMARY,
            state.theme(),
            sink,
        );
    } else {
        render_hint(state, frame, keys_area);
    }

    if let (Some(status), Some(note_area)) = (state.status(), note_area) {
        frame.render_widget(
            Paragraph::new(format!("  {status}")).style(Style::default().fg(Color::Yellow)),
            Rect {
                height: 1,
                ..note_area
            },
        );
    }
}

/// The gap between two hint items, in columns. Two spaces, which is what the
/// unwrapped hint put between them; [`hotspot::wrap_items`] gives it to the
/// item on its right so a wrapped row never starts with it.
const HINT_GAP: u16 = 2;

/// Draw the hint over the **foot** of the rows the bar would have had,
/// wrapped, and never over more of them than the band actually has.
///
/// **The foot, because an overlay is painted over the band's head.** An
/// overlay is centred on the whole terminal and drawn after the bands, so its
/// bottom border lands on the band's first row as soon as the band is taller
/// than the bar it used to be: measured at 100x30, `└──────┘` drawn straight
/// through `tab section … w save`.
///
/// **Wrapped, because the Settings hint is 97 columns.** Drawn as one
/// unwrapped line it stopped inside `r setup` at 80 columns and ` esc close`
/// was on no screen — and `esc` is the only key that closes that overlay, so
/// the one way out was the thing the clip hid. Below 70 columns `w save` went
/// too. `hotspot::wrap_items` is the same greedy walk the action bar wraps
/// its pills by, one row above.
///
/// When even the wrapped hint needs more rows than the band has, the **last**
/// rows are the ones kept: the way out is the last item of every hint here,
/// and an overlay whose exit is on no screen is the complaint this fixes.
fn render_hint(state: &ShellState, frame: &mut Frame, keys_area: Rect) {
    if keys_area.width == 0 || keys_area.height == 0 {
        return;
    }
    let mut lines = hotspot::wrap_items(hint_items(state), keys_area.width, HINT_GAP);
    let rows = u16::try_from(lines.len())
        .unwrap_or(u16::MAX)
        .min(keys_area.height);
    if rows == 0 {
        return;
    }
    let lines = lines.split_off(lines.len() - usize::from(rows));
    frame.render_widget(
        Paragraph::new(lines),
        Rect {
            y: keys_area.bottom() - rows,
            height: rows,
            ..keys_area
        },
    );
}

/// The hint the modes that are not "control mode with nothing open" draw: an
/// overlay's own keys, or session mode's reminder of whose keyboard it is —
/// one entry per item, so [`hotspot::wrap_items`] can break between two of
/// them and never inside one.
///
/// Still a string split on three spaces: the split is what makes the items,
/// and the item is what makes the key and its description one unbreakable
/// unit.
fn hint_items(state: &ShellState) -> Vec<Vec<Span<'static>>> {
    let hint = match (state.mode(), state.overlay()) {
        (Mode::Session, _) => {
            "SESSION MODE   ctrl-5 (or ctrl-] or F12) for glasshouse   keys go to the session"
        }
        (Mode::Control, Some(Overlay::Overview)) => {
            "up/down pick   m send text   c interrupt   esc back to session   q quit"
        }
        (Mode::Control, Some(Overlay::Settings)) => {
            "tab section   up/down move   space toggle   section keys edit   \
             w save   W project   r setup   esc close"
        }
        (Mode::Control, Some(Overlay::HarnessChoice)) => "up/down pick   enter start   esc cancel",
        (Mode::Control, _) => "esc back to session   q quit",
    };
    hint.split("   ")
        .map(|item| {
            let (key, description) = item.split_once(' ').unwrap_or((item, ""));
            let mut spans = vec![Span::styled(
                key.to_owned(),
                Style::default()
                    .fg(state.theme().accent())
                    .add_modifier(Modifier::BOLD),
            )];
            if !description.is_empty() {
                spans.push(Span::styled(
                    format!(" {description}"),
                    Style::default().fg(state.theme().quiet()),
                ));
            }
            spans
        })
        .collect()
}

/// A quiet launch surface; a live harness grid never passes through this function.
pub(super) fn render_landing(state: &ShellState, frame: &mut Frame, area: Rect) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let theme = state.theme();
    let margin = if area.width >= 50 { 2 } else { 0 };
    // The vertical margin is airy when there is air and tight when there is
    // not: the wrapped action bar took two rows off this panel, and a margin
    // that stayed at two spent them on whitespace instead of on the session's
    // own facts. The status note's own row took a third, and the thresholds
    // moved again for it — measured, because the row that fell off an
    // eighty-by-twenty-four panel was `This session is headless: it runs with
    // no viewport.`, which is the one line stopping an empty viewport from
    // looking broken. A panel this short spends its rows on facts.
    let inner = area.inner(ratatui::layout::Margin::new(
        margin,
        match area.height {
            22.. => 2,
            18.. => 1,
            _ => 0,
        },
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
        // **The next step is named before the menu of things one could do.**
        // A first-time user cannot get a session to run until an account or a
        // provider pays for it, and the panel that told them "n start a
        // session" first was sending them at the second step. Only shown when
        // the run loop actually read the accounts and found none connected —
        // `Summary::unknown` says nothing, because an unread summary is not
        // evidence of an empty one.
        if let Some(next) = super::settings_actions::first_run_next_step(state) {
            lines.push(Line::from(Span::styled(
                next,
                Style::default()
                    .fg(theme.accent())
                    .add_modifier(Modifier::BOLD),
            )));
            lines.push(Line::default());
        }
        lines.push(Line::from(Span::styled(
            "n  start a session",
            Style::default()
                .fg(theme.accent())
                .add_modifier(Modifier::BOLD),
        )));
        lines.push(Line::from("s  settings — providers, accounts and defaults"));
        lines.push(Line::from("o  inspect recorded sessions"));
        lines.push(Line::default());
        lines.push(Line::from(Span::styled(
            "Run `glasshouse launch` to start one from your shell.",
            Style::default().fg(theme.quiet()),
        )));
    }
    let navigation = vec![
        Line::from(Span::styled(
            "OBSERVE & NAVIGATE",
            Style::default()
                .fg(theme.secondary())
                .add_modifier(Modifier::BOLD),
        )),
        Line::from("p project   e events   h health   d decisions   r routes"),
        // **The two session chords are not named here.** They belong to a
        // session and a session always draws them: `render_header` puts
        // `FOCUS_CHORD` and `ESCAPE_CHORD` on screen for as long as one is on
        // screen, so repeating them in the fleet view spends two of this
        // panel's lines on keys that do nothing yet — and makes the words
        // that mean *"you are inside a session"* mean nothing, which is what
        // three pty tests were reading them as.
        Line::from("tab session   enter focus"),
        Line::default(),
        Line::from(Span::styled(
            format!(
                "t theme: {}   a motion: {}",
                theme.name(),
                if state.motion_paused() { "off" } else { "on" }
            ),
            Style::default().fg(theme.quiet()),
        )),
    ];
    // **The keys sit at the foot of the panel, not after the detail.**
    // While they were the tail of one wrapped `Paragraph`, a session's own
    // detail block pushed them off the bottom the moment the viewport lost a
    // row — which is precisely what the wrapped action bar did. What a panel
    // clips must be the description of a session, never the way to reach one.
    // Below ten rows there is no room to reserve, and everything goes back
    // into one flow rather than being cut in half.
    let navigation_rows = u16::try_from(navigation.len() + 1).unwrap_or(u16::MAX);
    if body.height >= navigation_rows + 5 {
        let [detail, keys] =
            Layout::vertical([Constraint::Min(0), Constraint::Length(navigation_rows)]).areas(body);
        frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), detail);
        // Styled as a block, not line by line: these are hints, and a widget
        // style paints the whole band rather than only the glyphs, which is
        // what keeps the band one visual object instead of five sentences
        // that happen to be adjacent.
        frame.render_widget(
            Paragraph::new(navigation).style(Style::default().fg(theme.quiet())),
            Rect {
                y: keys.y + 1,
                height: keys.height - 1,
                ..keys
            },
        );
    } else {
        lines.push(Line::default());
        lines.extend(navigation);
        frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), body);
    }
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
