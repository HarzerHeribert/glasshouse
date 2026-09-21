use super::{
    Action, Document, Geometry, Tone, Workbench, document::clip, settings::CATEGORIES, theme,
};
use crate::contract::{Conversation, ServedBy};
use crate::tui::{Activity, Notebook, ScreenState, StatusLine, Theme};
use ratatui::{
    Frame,
    layout::Rect,
    text::Span,
    widgets::{Clear, Paragraph},
};

fn row(f: &mut Frame<'_>, r: Rect, text: &str, tone: Tone, theme: Theme) {
    let r = r.intersection(f.area());
    if r.height > 0 && r.width > 0 {
        f.render_widget(
            Paragraph::new(text.chars().filter(|c| !c.is_control()).collect::<String>())
                .style(super::theme::style(tone, theme)),
            Rect::new(r.x, r.y, r.width, 1),
        );
    }
}
fn button(
    f: &mut Frame<'_>,
    g: &mut Geometry,
    r: Rect,
    text: &str,
    action: Action,
    selected: bool,
    t: Theme,
) {
    let r = r.intersection(f.area());
    row(
        f,
        r,
        text,
        if selected { Tone::Accent } else { Tone::Normal },
        t,
    );
    if r.width > 0 && r.height > 0 {
        g.hits.push((Rect::new(r.x, r.y, r.width, 1), action));
    }
}
fn full_row(area: Rect, y: u16) -> Rect {
    Rect::new(area.x, y, area.width, u16::from(y < area.bottom()))
}
fn label(f: &mut Frame<'_>, a: Rect, y: u16, text: &str, tone: Tone, t: Theme) {
    if y < a.bottom() {
        row(f, full_row(a, y), text, tone, t);
    }
}
// Keep drawing and the matching hit rectangle in one leaf helper.
#[allow(clippy::too_many_arguments)]
fn add(
    f: &mut Frame<'_>,
    g: &mut Geometry,
    a: Rect,
    y: u16,
    text: &str,
    action: Action,
    selected: bool,
    t: Theme,
) {
    if y < a.bottom() {
        button(f, g, full_row(a, y), text, action, selected, t);
    }
}

/// One word for the work mode, as the footer and the session bar both name it.
fn work_word(s: &ScreenState) -> &'static str {
    match s.mode {
        crate::tui::Mode::Execute => "Build",
        crate::tui::Mode::Explore => "Explore",
        crate::tui::Mode::Plan => "Plan",
    }
}
/// What this session can reach, in words a person can act on.
///
/// **The label a control wears is the whole of its information scent.** What
/// stood here was `sandbox 3p/1c YOLO unconfined`, which is a path-rule
/// count, a command-pattern count, a joke, and the name of an internal
/// applier. Nobody can read that and know whether their home directory is
/// safe, so nobody clicked it, and the one surface that would have explained
/// it was the one thing the label gave no reason to open. The mechanism has
/// not gone anywhere -- it is on the Access surface, under this sentence.
fn access_word(s: &ScreenState) -> &'static str {
    if s.full_access {
        "FULL ACCESS"
    } else {
        "This project"
    }
}
/// The one sentence that says what [`access_word`] costs or protects.
fn access_sentence(s: &ScreenState) -> &'static str {
    if s.full_access {
        "Reads and writes anywhere on this machine. Nothing outside this project is protected."
    } else {
        "Reads, writes and runs commands inside this project. Anything outside it is refused."
    }
}
/// Whether host tools may leave the machine, as a clause rather than `net:`.
fn network_word(s: &ScreenState) -> &'static str {
    match s.network.as_deref() {
        Some("off") => "no network",
        Some("unknown") | None => "network unknown",
        _ => "network on",
    }
}
/// One word for how often this session asks before it acts.
fn ask_word(s: &ScreenState) -> &'static str {
    s.permissions.rung().label()
}
/// **The two controls that can be dangerous carry their weight in the tone,
/// and in the word.** Colour alone would be a lie on a monochrome terminal
/// and invisible to anyone who cannot see it, so `FULL ACCESS` is shouted in
/// capitals and `Never asks` says what it does -- the colour is the second
/// signal, never the only one.
fn access_tone(s: &ScreenState) -> Tone {
    if s.full_access {
        Tone::Failure
    } else {
        Tone::Normal
    }
}
fn ask_tone(s: &ScreenState) -> Tone {
    if s.permissions.rung() == crate::permissions::Rung::Full {
        Tone::Warning
    } else {
        Tone::Normal
    }
}
/// The one line on this screen that teaches, and the only place several of
/// these keys are advertised at all.
///
/// **Teaching happens at the moment of use, never in a tour.** The hint turns
/// with the session -- one more cell, one more notice -- rather than with the
/// clock, so it holds still while someone reads it, and so a screenshot and a
/// test see the same line twice.
fn hint(n: &Notebook, s: &ScreenState) -> &'static str {
    const HINTS: [&str; 8] = [
        "Shift-Tab changes how often Pane asks before it acts",
        "F2 opens settings · every choice there applies to this session now",
        "Ctrl-T opens the instruments · Esc closes them",
        "Click any control in the top bar to change it",
        "Esc once stops after the current cell · twice cancels the call",
        "Ctrl-B shows or hides the session card · Ctrl-F hides the chrome",
        "/diff opens the last cell's changes · F4 does the same",
        "Type / for commands · @ for a path in this project",
    ];
    HINTS[(n.cells.len() + s.history.len()) % HINTS.len()]
}
/// Right-aligned controls that give room up in a fixed order, so a narrow
/// terminal loses the least useful control rather than the leftmost one.
///
/// The order is the order a person reaches for them: which model answers,
/// what the session may do, how often it asks, what confines it. The way
/// into settings is never dropped, because it is the way to everything else.
fn controls(
    f: &mut Frame<'_>,
    g: &mut Geometry,
    a: Rect,
    reserved: usize,
    items: &[(String, Action, Tone)],
    t: Theme,
) {
    let room = (a.width as usize).saturating_sub(reserved);
    let mut keep: Vec<usize> = (0..items.len()).collect();
    let width = |keep: &Vec<usize>| -> usize {
        keep.iter()
            .map(|i| Span::raw(items[*i].0.as_str()).width() + 2)
            .sum()
    };
    // Drop the last-named control first; the list is written most useful
    // first, and the final entry -- settings -- is exempt.
    //
    // **A control drawn in a warning tone is exempt too.** A boundary that
    // can be lifted has to be continuously visible or it is a mode error
    // waiting to happen, and at 80 columns the old rule dropped exactly
    // that one: `FULL ACCESS` disappeared and the session looked ordinary.
    while width(&keep) > room && keep.len() > 1 {
        // The last ordinary control still standing, never the final entry.
        let Some(drop) = keep[..keep.len() - 1]
            .iter()
            .rposition(|i| matches!(items[*i].2, Tone::Normal | Tone::Muted))
        else {
            break;
        };
        keep.remove(drop);
    }
    if width(&keep) > room {
        return;
    }
    let mut x = a.right() - width(&keep) as u16;
    for i in keep {
        let (text, action, tone) = &items[i];
        let w = Span::raw(text.as_str()).width() as u16;
        let r = Rect::new(x, a.y, w, 1).intersection(f.area());
        row(f, r, text, *tone, t);
        if r.width > 0 && r.height > 0 {
            g.hits
                .push((Rect::new(r.x, r.y, r.width, 1), action.clone()));
        }
        x += w + 2;
    }
}
/// `⠿ PANE / project` and the four session facts, one row, every width.
///
/// The facts are the ones a person acts on -- which model answers, what this
/// session may do, how often it asks, and whether it is confined -- and each
/// of them is the control that changes it.
fn session_bar(f: &mut Frame<'_>, g: &mut Geometry, a: Rect, s: &ScreenState) {
    // **A long project name must not cost you the session's controls.** The
    // name is the one thing on this row that cannot be acted on, and it was
    // taking its full length out of the budget before the controls were
    // measured -- so a checkout called `pane-live-95138-15` pushed both the
    // boundary and the approval rung off an eighty-column screen. It gets a
    // third of the row and clips.
    let project = clip(
        s.project.as_deref().unwrap_or("workspace"),
        (a.width as usize / 4).max(8),
    );
    let brand = format!(" ⠿ PANE / {project}");
    row(f, a, &brand, Tone::Accent, s.theme);
    // The model name is the widest control and the least often read of the
    // four, so it clips rather than pushing the others off the row.
    let model = clip(s.model.as_deref().unwrap_or("choose model"), 18);
    // **The value alone, with no `Work:` or `Ask:` in front of it.** A label
    // that repeats what the control obviously is spends columns a narrow
    // terminal needs, and these four are already distinguishable by their
    // own words -- nobody reads `Build` and wonders whether it is the model.
    controls(
        f,
        g,
        a,
        Span::raw(brand.as_str()).width() + 2,
        &[
            (format!("{model} ▾"), Action::Models, Tone::Normal),
            // **How often it asks outranks which mode it is in.** The old
            // order gave up the approval rung before the work mode, so at
            // eighty columns a session kept telling you it was in `Build`
            // -- which the composer makes obvious -- and stopped telling
            // you whether it would ask before running anything.
            (ask_word(s).to_string(), Action::Approvals, ask_tone(s)),
            (work_word(s).to_string(), Action::Work, Tone::Normal),
            (access_word(s).to_string(), Action::Access, access_tone(s)),
            ("[ Settings ]".to_string(), Action::Settings, Tone::Normal),
        ],
        s.theme,
    );
}
pub struct Layout {
    pub transcript: Rect,
    sidebar: Option<Rect>,
    header: u16,
    footer: u16,
    textwidth: u16,
    input_lines: Vec<String>,
    composer_height: u16,
    queue_height: u16,
    body_height: u16,
}
pub fn layout(a: Rect, s: &ScreenState) -> Layout {
    let chrome = !s.fullscreen;
    // The bar and the rule under it: `.sessionbar` has a bottom border in
    // the mockup, and without it the first notice reads as another control.
    let header = if chrome && a.height > 7 { 2 } else { 0 };
    let footer = if chrome {
        match s.status_line {
            StatusLine::Full => 2,
            StatusLine::Compact => 1,
            StatusLine::Hidden => 0,
        }
    } else {
        0
    };
    let textwidth = a.width.saturating_sub(2).max(1);
    // Two columns belong to the composer's prompt mark.
    let input_lines = wrap_input(&s.input, textwidth.saturating_sub(2).max(1) as usize);
    let composer_height = (input_lines.len() as u16)
        .clamp(1, 5)
        .saturating_add(1)
        .min(a.height.saturating_sub(header + footer).max(1));
    let queue_height = (s.queued.len() as u16)
        .min(2)
        .min(a.height.saturating_sub(header + footer + composer_height));
    let body_height = a
        .height
        .saturating_sub(header + footer + composer_height + queue_height);
    let transcript = Rect::new(
        a.x + u16::from(a.width > 1),
        a.y + header,
        textwidth,
        body_height,
    );
    let side_width = if !s.fullscreen
        && body_height >= 10
        && a.width >= 100
        && match s.sidebar {
            crate::tui::SidebarVisibility::Shown => true,
            crate::tui::SidebarVisibility::Hidden => false,
            crate::tui::SidebarVisibility::Auto => a.width >= 120,
        } {
        30
    } else {
        0
    };
    let mut transcript = transcript;
    transcript.width = transcript.width.saturating_sub(side_width);
    Layout {
        header,
        footer,
        textwidth,
        input_lines,
        composer_height,
        queue_height,
        body_height,
        transcript,
        sidebar: (side_width > 0).then(|| {
            Rect::new(
                transcript.right() + 1,
                transcript.y,
                side_width - 1,
                body_height,
            )
        }),
    }
}

pub fn render(
    f: &mut Frame<'_>,
    c: &Conversation,
    n: &Notebook,
    s: &ScreenState,
    _served: &ServedBy,
    ui: &mut Workbench,
) {
    let a = f.area();
    f.render_widget(Clear, a);
    f.buffer_mut()
        .set_style(a, theme::style(Tone::Normal, s.theme));
    let mut g = Geometry::default();
    if a.height == 0 || a.width == 0 {
        ui.geometry = g;
        return;
    }
    let Layout {
        header,
        footer,
        textwidth,
        input_lines,
        composer_height,
        queue_height,
        body_height,
        transcript,
        sidebar,
    } = layout(a, s);
    g.transcript = transcript;
    let d = Document::build(c, n, s, ui, transcript.width as usize);
    g.rows = d.rows.len();
    g.start = g
        .rows
        .saturating_sub(body_height as usize)
        .saturating_sub(s.scrollback);
    ui.anchor = d.rows.get(g.start).map(|row| (row.key, row.text.clone()));
    ui.last_scrollback = s.scrollback;
    if header > 0 {
        session_bar(f, &mut g, Rect::new(a.x, a.y, a.width, 1), s);
        row(
            f,
            Rect::new(a.x, a.y + 1, a.width, 1),
            &"─".repeat(a.width as usize),
            Tone::Line,
            s.theme,
        );
    }
    for (j, r) in d
        .rows
        .iter()
        .skip(g.start)
        .take(body_height as usize)
        .enumerate()
    {
        let area = Rect::new(
            g.transcript.x,
            g.transcript.y + j as u16,
            g.transcript.width,
            1,
        );
        if let (Some(Action::Tab(cell, current)), false) = (&r.action, r.tabs.is_empty()) {
            let (cell, current) = (*cell, *current);
            let mut x = area.x;
            for (label, tab) in &r.tabs {
                let label = if current == *tab {
                    format!("[ {label} ]")
                } else {
                    format!("  {label}  ")
                };
                let w =
                    (Span::raw(label.as_str()).width() as u16).min(area.right().saturating_sub(x));
                button(
                    f,
                    &mut g,
                    Rect::new(x, area.y, w, 1),
                    &label,
                    Action::Tab(cell, *tab),
                    current == *tab,
                    s.theme,
                );
                x += w;
            }
            // Whatever the strip left over -- the route to the whole diff.
            let rest: String = r.spans.iter().map(|(t, _)| t.as_str()).collect();
            let w = (Span::raw(rest.as_str()).width() as u16).min(area.right().saturating_sub(x));
            if w > 0 {
                button(
                    f,
                    &mut g,
                    Rect::new(x, area.y, w, 1),
                    &rest,
                    Action::Command("/diff".into()),
                    false,
                    s.theme,
                );
            }
        } else if !r.spans.is_empty() {
            let mut x = area.x;
            for (text, tone) in &r.spans {
                let w =
                    (Span::raw(text.as_str()).width() as u16).min(area.right().saturating_sub(x));
                if w == 0 {
                    break;
                }
                row(f, Rect::new(x, area.y, w, 1), text, *tone, s.theme);
                x += w;
            }
            if let Some(act) = &r.action {
                g.hits.push((area, act.clone()));
            }
        } else {
            row(f, area, &r.text, r.tone, s.theme);
            if let Some(act) = &r.action {
                g.hits.push((area, act.clone()));
            }
            if let Some(root) = &s.settings_root {
                for (start, end) in crate::tui::found_paths(&r.text, root) {
                    let prefix: String = r.text.chars().take(start).collect();
                    let text: String = r.text.chars().skip(start).take(end - start).collect();
                    let x = area.x.saturating_add(Span::raw(&prefix).width() as u16);
                    let width =
                        (Span::raw(&text).width() as u16).min(area.right().saturating_sub(x));
                    if width > 0
                        && let Some(path) = crate::tui::resolve_path(&text, root)
                    {
                        let rect = Rect::new(x, area.y, width, 1);
                        f.buffer_mut().set_style(
                            rect,
                            theme::style(r.tone, s.theme)
                                .add_modifier(ratatui::style::Modifier::UNDERLINED),
                        );
                        g.hits
                            .push((rect, Action::Path(path.display().to_string())));
                    }
                }
            }
        }
    }
    if s.scrolling && g.rows > body_height as usize && body_height > 0 && g.transcript.width > 0 {
        let height = body_height as usize;
        let thumb = (height * height / g.rows).max(1);
        let top = g.start.min(g.rows - height) * (height - thumb) / (g.rows - height);
        for offset in top..(top + thumb).min(height) {
            row(
                f,
                Rect::new(
                    g.transcript.right() - 1,
                    g.transcript.y + offset as u16,
                    1,
                    1,
                ),
                "▐",
                Tone::Accent,
                s.theme,
            );
        }
    }
    if s.scrollback > 0 && body_height > 0 {
        let text = "↓ Latest";
        let r = Rect::new(
            g.transcript.right().saturating_sub(text.len() as u16),
            g.transcript.bottom() - 1,
            text.len() as u16,
            1,
        );
        button(f, &mut g, r, text, Action::Latest, true, s.theme);
    }
    if let Some(side) = sidebar {
        // The mockup's standing card: what this session is, not what it did.
        // Everything here is also reachable from a control, so hiding the
        // sidebar never hides a fact.
        label(f, side, side.y, "THIS SESSION", Tone::Accent, s.theme);
        let mut lines: Vec<(String, Tone)> = vec![
            (format!("Work {}", work_word(s)), Tone::Normal),
            (format!("Ask {}", ask_word(s)), Tone::Normal),
            // One vocabulary. The card used to print the name of an internal
            // confinement applier here while the bar three rows up said
            // something else about the same boundary.
            (access_word(s).to_string(), access_tone(s)),
            (network_word(s).to_string(), Tone::Muted),
            (String::new(), Tone::Normal),
            ("Model & source".to_string(), Tone::Accent),
            (
                s.model.as_deref().unwrap_or("choose model").to_string(),
                Tone::Normal,
            ),
            (String::new(), Tone::Normal),
            ("Cells".to_string(), Tone::Accent),
            (format!("{} recorded", n.cells.len()), Tone::Normal),
            (
                format!("{:.1}s elapsed", s.pulse.elapsed_ms as f64 / 1000.0),
                Tone::Muted,
            ),
            // The spend, which the status line used to carry and nobody
            // read there. This card is where someone comes to ask.
            (
                n.tokens
                    .as_ref()
                    .map(|t| format!("{} tokens", t.used))
                    .unwrap_or_default(),
                Tone::Muted,
            ),
            (String::new(), Tone::Normal),
            ("Little helpers".to_string(), Tone::Helper),
        ];
        for helper in n.cells.last().map(|c| c.helpers.as_slice()).unwrap_or(&[]) {
            lines.push((
                format!(
                    "◇ {} · {}",
                    helper.helper,
                    if helper.outcome.ok {
                        "returned"
                    } else if helper.outcome.text.is_empty() {
                        "waiting"
                    } else {
                        "failed"
                    }
                ),
                Tone::Muted,
            ));
        }
        for (i, (text, tone)) in lines.iter().enumerate() {
            label(f, side, side.y + 2 + i as u16, text, *tone, s.theme);
        }
        // The sidebar's own routes: the notices it summarises, and the
        // instruments behind the numbers it shows.
        add(
            f,
            &mut g,
            side,
            side.bottom().saturating_sub(2),
            "Activity / local notices",
            Action::Activity,
            false,
            s.theme,
        );
        label(
            f,
            side,
            side.bottom().saturating_sub(1),
            "Ctrl-T telemetry",
            Tone::Muted,
            s.theme,
        );
    }
    let mut y = g.transcript.bottom();
    for q in s.queued.iter().rev().take(queue_height as usize).rev() {
        row(
            f,
            Rect::new(a.x, y, a.width, 1),
            &format!(" QUEUED › {}", q.lines().next().unwrap_or("")),
            Tone::Normal,
            s.theme,
        );
        y += 1;
    }
    if y < a.bottom() {
        let running = matches!(
            s.activity,
            Activity::Thinking | Activity::Executing | Activity::Compacting | Activity::Searching
        );
        let still =
            s.reduced_motion || s.selection.is_some() || s.pulse.elapsed_ms > 60_000 || !running;
        // The live edge, in the words the rest of the session already uses:
        // a person watching this line is asking *is it alive and on what*,
        // and the answer is the activity's own name, never a claim about
        // progress toward an end nobody can see.
        let activity = match s.activity {
            Activity::Compacting => "compacting · preparing bounded context",
            Activity::Executing => "executing cell",
            Activity::Thinking => "thinking",
            Activity::Searching => "searching",
            Activity::Failed => "action failed — inspect the cell",
            Activity::Complete => "complete",
            Activity::Streaming => "receiving response",
            Activity::Waiting => "waiting on a response · estimate unknown",
            Activity::Starting => "starting session",
            _ => "ready",
        };
        let bar = format!(
            " {} {}{} ",
            theme::orbit(s.animation_frame, still),
            activity,
            if s.stopping { " · stop requested" } else { "" }
        );
        // **This line is also the composer's top edge** (`.composer-wrap` has
        // one in the mockup). Drawing the rule as the ribbon's own tail costs
        // no row and leaves the one place that takes typing unmistakable.
        let mut used = Span::raw(bar.as_str()).width() as u16;
        row(
            f,
            Rect::new(a.x, y, a.width, 1),
            &bar,
            if running { Tone::Accent } else { Tone::Muted },
            s.theme,
        );
        // A standing notice rides this line, where the eye already is and
        // beside the composer it answers -- never in place of the two status
        // rows, which say what the session *is* rather than what just
        // happened. It is also a row in the transcript, to scroll back to.
        let notice = if !ui.notice.is_empty() {
            Some(ui.notice.clone())
        } else if let Some(notice) = &s.notice {
            Some(notice.clone())
        } else if s.mouse_off {
            Some("Mouse released · Ctrl-G captures again".to_string())
        } else {
            None
        };
        if let Some(notice) = &notice
            && used + 8 < a.width
        {
            let text = format!(
                "── {} ",
                clip(notice, a.width.saturating_sub(used + 6) as usize)
            );
            let w = (Span::raw(text.as_str()).width() as u16).min(a.width - used);
            row(
                f,
                Rect::new(a.x + used, y, w, 1),
                &text,
                Tone::Accent,
                s.theme,
            );
            used += w;
        }
        if used < a.width {
            row(
                f,
                Rect::new(a.x + used, y, a.width - used, 1),
                &"─".repeat(a.width.saturating_sub(used) as usize),
                if running { Tone::Accent } else { Tone::Line },
                s.theme,
            );
        }
        y += 1;
    }
    // `❯` marks where typing lands, exactly as it marks what was already
    // said in the transcript; the editor starts two columns in.
    let prompt = u16::from(a.width > 6) * 3;
    if prompt > 0 && y < a.bottom() {
        row(f, Rect::new(a.x, y, 3, 1), " ❯ ", Tone::Accent, s.theme);
    }
    g.composer = Rect::new(
        a.x + prompt.max(u16::from(a.width > 1)),
        y,
        textwidth.saturating_sub(prompt.saturating_sub(1)),
        composer_height
            .saturating_sub(1)
            .min(a.bottom().saturating_sub(y)),
    );
    let visible = g.composer.height as usize;
    let cursor = s.cursor.unwrap_or(s.input.len()).min(s.input.len());
    let before = &s.input[..s.input.floor_char_boundary(cursor)];
    let cursor_lines = wrap_input(before, textwidth.saturating_sub(2).max(1) as usize);
    let cursor_row = cursor_lines.len().saturating_sub(1);
    let skip = cursor_row.saturating_sub(visible.saturating_sub(1));
    for (i, l) in input_lines.iter().skip(skip).take(visible).enumerate() {
        row(
            f,
            Rect::new(g.composer.x, g.composer.y + i as u16, g.composer.width, 1),
            l,
            Tone::Normal,
            s.theme,
        );
    }
    if s.input.is_empty() {
        row(
            f,
            g.composer,
            "Describe the next step — a message or / for commands",
            Tone::Muted,
            s.theme,
        );
    }
    g.hits.push((g.composer, Action::Composer));
    if !ui.is_local() && s.panel.is_none() && s.secret_prompt.is_none() && visible > 0 {
        let x = cursor_lines
            .last()
            .map(|l| Span::raw(l.as_str()).width())
            .unwrap_or(0)
            .min(textwidth.saturating_sub(1) as usize);
        let position = (
            g.composer.x + x as u16,
            g.composer.y + (cursor_row - skip) as u16,
        );
        if super::contains(a, position.0, position.1) {
            f.set_cursor_position(position);
        }
    }
    if footer > 0 && a.height >= footer {
        let y = a.bottom() - footer;
        // **The strip is controls, and the line under it is what just
        // happened.** What stood here was two rows of read-only facts --
        // `sandbox 3p/1c YOLO unconfined · net:off` over `Build ·
        // auto-review · effort default   ◇ 1 helper call   N tokens` -- in
        // which the only things a person could act on were already in the
        // session bar two rows further up, and the rest were counters.
        //
        // The drawing's own `renderStatus` is a row of buttons, a live
        // notice and an Undo, and this is that: the three settings a person
        // reaches for that the session bar has no room for, each one the
        // control that changes it, over one line that carries whatever the
        // session last said.
        let context = n.context.map(|tokens| {
            crate::tui::status::context_summary(
                tokens,
                if a.width >= 160 { 12 } else { 7 },
                s.animation_frame,
                matches!(s.activity, Activity::Thinking | Activity::Streaming),
            )
        });
        let right = context.unwrap_or_default();
        let rw = Span::raw(right.as_str()).width() as u16;
        if rw > 0 {
            row(
                f,
                Rect::new(a.right().saturating_sub(rw + 1), y, rw, 1),
                &right,
                Tone::Muted,
                s.theme,
            );
        }
        let mut x = a.x + 1;
        for (text, action) in [
            (format!("effort {}", s.effort.name()), Action::Effort),
            (
                format!("◇ helpers {}", if s.helpers_on { "on" } else { "off" }),
                Action::SettingsAt(2),
            ),
            (
                s.subagents
                    .as_deref()
                    .map(|word| format!("subagents {word}"))
                    .unwrap_or_default(),
                Action::SettingsAt(4),
            ),
        ] {
            // A fact nobody measured is not a control; an empty label is
            // how this strip says "no" rather than saying `unknown`.
            if text.is_empty() {
                continue;
            }
            let w = Span::raw(text.as_str()).width() as u16;
            if x + w + rw + 3 > a.right() {
                break;
            }
            let r = Rect::new(x, y, w, 1);
            row(f, r, &text, Tone::Muted, s.theme);
            g.hits.push((r, action));
            x += w + 3;
        }
        if footer > 1 {
            // **One muted line that teaches, instead of one that counts.**
            // What was here was `◇ 1 helper call   4210 tokens this
            // session`, and neither number was ever the answer to a question
            // anyone had at the moment they read it -- the spend lives on
            // the session card and in the instruments, where someone goes
            // when they want it.
            //
            // A rotating hint is what both neighbouring products use to keep
            // teaching after the first minute, and it is the only thing on
            // this screen that advertises Shift-Tab, F2 or Ctrl-T at all. It
            // turns with the session rather than with the clock, so it is
            // stable inside one screenshot and inside one test.
            let text = hint(n, s).to_string();
            let tone = Tone::Muted;
            let motion = if s.reduced_motion {
                "still"
            } else {
                "calm motion"
            };
            let mw = motion.len() as u16;
            row(
                f,
                Rect::new(a.x + 1, y + 1, a.width.saturating_sub(mw + 3), 1),
                &text,
                tone,
                s.theme,
            );
            row(
                f,
                Rect::new(a.right().saturating_sub(mw + 1), y + 1, mw, 1),
                motion,
                Tone::Muted,
                s.theme,
            );
        }
    }
    if !ui.is_local() && s.panel.is_none() && !s.input.contains(char::is_whitespace) {
        let completions = crate::tui::slash_matches(&s.input);
        let capacity = g.transcript.height.min(7) as usize;
        let first = s
            .completion_selected
            .saturating_sub(capacity.saturating_sub(1));
        let count = completions.len().min(capacity);
        let top = g.transcript.bottom().saturating_sub(count as u16);
        for (i, (command, help)) in completions.iter().enumerate().skip(first).take(count) {
            let area = Rect::new(
                g.transcript.x,
                top + (i - first) as u16,
                g.transcript.width,
                1,
            );
            f.render_widget(Clear, area);
            button(
                f,
                &mut g,
                area,
                &format!("{command:14} {help}"),
                Action::Insert(command.clone()),
                i == s.completion_selected,
                s.theme,
            );
        }
    }
    if s.telemetry_open && !ui.is_local() && s.panel.is_none() {
        // The instruments take the transcript's room, never the status line
        // that carries the context reading they are read against.
        let area = Rect::new(a.x, a.y + header, a.width, body_height + queue_height);
        f.render_widget(Clear, area);
        f.buffer_mut()
            .set_style(area, theme::style(Tone::Normal, s.theme));
        g.hits.clear();
        g.local = Some(area);
        crate::tui::telemetry::expanded(f, area, c, _served, n, s);
    }
    if ui.is_local() || s.panel.is_some() {
        let area = if a.width >= 100 && a.height >= 25 {
            Rect::new(a.x + 2, a.y + 1, a.width - 4, a.height - 2)
        } else {
            a
        };
        // A local surface is modal: the conversation behind it is not
        // half-visible around its edges, which would read as damage.
        f.render_widget(Clear, a);
        f.buffer_mut()
            .set_style(a, theme::style(Tone::Normal, s.theme));
        g.hits.clear();
        g.local = Some(area);
        let inner = Rect::new(
            area.x + u16::from(area.width > 1),
            area.y,
            area.width.saturating_sub(2),
            area.height,
        );
        let (title, sub) = if ui.preferences.is_some() {
            (
                "SETTINGS",
                "every choice applies now and saves itself; there is no Apply",
            )
        } else if ui.models.is_some() {
            ("MODELS", "one Enter commits one selection")
        } else if ui.work {
            ("WORK", "what this session may do")
        } else if ui.approvals {
            ("ASK", "how often it stops to ask")
        } else if ui.access {
            ("ACCESS", "the boundaries it is actually running under")
        } else if ui.activity {
            ("ACTIVITY", "local notices, newest last")
        } else if ui.confirm.is_some() {
            ("CONFIRM", "this one is not undone by Esc")
        } else {
            (
                s.panel
                    .as_ref()
                    .map(|p| p.title.as_str())
                    .unwrap_or("DETAILS"),
                "",
            )
        };
        // One head for every surface: what it is, what it does, and the way
        // out -- in the same three places each time.
        row(
            f,
            Rect::new(inner.x, inner.y, inner.width, 1),
            &format!("{title}  "),
            Tone::Accent,
            s.theme,
        );
        if inner.width as usize > title.len() + sub.len() + 16 {
            row(
                f,
                Rect::new(
                    inner.x + title.len() as u16 + 2,
                    inner.y,
                    inner.width.saturating_sub(title.len() as u16 + 2),
                    1,
                ),
                sub,
                Tone::Muted,
                s.theme,
            );
        }
        if inner.width > 12 {
            button(
                f,
                &mut g,
                Rect::new(inner.right() - 10, inner.y, 10, 1),
                "Esc · Back",
                Action::Close,
                false,
                s.theme,
            );
        }
        row(
            f,
            Rect::new(inner.x, inner.y + 1, inner.width, 1),
            &"─".repeat(inner.width as usize),
            Tone::Line,
            s.theme,
        );
        // And one foot: what just happened, and what leaving will do.
        if inner.height > 3 {
            let y = inner.bottom() - 1;
            row(
                f,
                Rect::new(inner.x, y - 1, inner.width, 1),
                &"─".repeat(inner.width as usize),
                Tone::Line,
                s.theme,
            );
            let notice = if !ui.notice.is_empty() {
                ui.notice.clone()
            } else {
                s.notice.clone().unwrap_or_default()
            };
            row(
                f,
                Rect::new(inner.x, y, inner.width, 1),
                &notice,
                Tone::Accent,
                s.theme,
            );
            let hint = if ui.models.is_some() {
                "Esc back · the assignment stays unchanged"
            } else {
                "Esc closes · saved choices stay"
            };
            row(
                f,
                Rect::new(
                    inner.right().saturating_sub(hint.len() as u16 + 1),
                    y,
                    hint.len() as u16,
                    1,
                ),
                hint,
                Tone::Muted,
                s.theme,
            );
        }
        let inner = Rect::new(
            inner.x,
            inner.y,
            inner.width,
            inner.height.saturating_sub(2),
        );
        if let Some(p) = &ui.preferences {
            draw_settings(f, &mut g, inner, p, s);
        } else if let Some(m) = &ui.models {
            draw_models(f, &mut g, inner, m, s);
        } else if ui.work {
            for (i, (word, help, command)) in [
                (
                    "Build",
                    "Edit and execute within the session's grants.",
                    "execute",
                ),
                (
                    "Explore",
                    "Inspect; scratch and configured write exceptions still apply.",
                    "explore",
                ),
                (
                    "Plan",
                    "Inspect and write the designated plan file only.",
                    "plan",
                ),
            ]
            .into_iter()
            .enumerate()
            {
                add(
                    f,
                    &mut g,
                    inner,
                    inner.y + 3 + i as u16 * 3,
                    word,
                    Action::Command(format!("/mode {command}")),
                    i == ui.local_scroll.min(2),
                    s.theme,
                );
                label(
                    f,
                    inner,
                    inner.y + 4 + i as u16 * 3,
                    help,
                    Tone::Normal,
                    s.theme,
                );
            }
        } else if ui.approvals {
            for (i, rung) in [
                crate::permissions::Rung::Manual,
                crate::permissions::Rung::AcceptEdits,
                crate::permissions::Rung::Auto,
                crate::permissions::Rung::Full,
            ]
            .into_iter()
            .enumerate()
            {
                let (word, help, mode) = (rung.label(), rung.sentence(), rung.name());
                add(
                    f,
                    &mut g,
                    inner,
                    inner.y + 3 + i as u16 * 3,
                    word,
                    Action::Rung(mode.into()),
                    i == ui.local_scroll.min(3),
                    s.theme,
                );
                label(
                    f,
                    inner,
                    inner.y + 4 + i as u16 * 3,
                    help,
                    Tone::Normal,
                    s.theme,
                );
            }
        } else if ui.access {
            // **The plain sentence first, the mechanism under it, and a way
            // to act at the end.** This surface used to be five colon-lines
            // of internal vocabulary with nothing to press: the one place a
            // person came to find out whether their home directory was safe
            // answered with `Sandbox profile: 3p/1c` and no exit but Escape.
            label(
                f,
                inner,
                inner.y + 2,
                access_word(s),
                access_tone(s),
                s.theme,
            );
            label(
                f,
                inner,
                inner.y + 3,
                access_sentence(s),
                Tone::Normal,
                s.theme,
            );
            label(
                f,
                inner,
                inner.y + 5,
                &format!("{} · asks {}", network_word(s), ask_word(s).to_lowercase()),
                Tone::Normal,
                s.theme,
            );
            // Under the rule: what the boundary is actually made of, for
            // the reader who came here for exactly that.
            label(
                f,
                inner,
                inner.y + 7,
                "How it is enforced",
                Tone::Muted,
                s.theme,
            );
            for (i, l) in [
                format!(
                    "Sandbox profile   {}",
                    s.sandbox.as_deref().unwrap_or("unknown")
                ),
                format!(
                    "Child processes   {}",
                    s.confinement.as_deref().unwrap_or("unknown")
                ),
                format!(
                    "Host tools        {}",
                    s.network.as_deref().unwrap_or("unknown")
                ),
            ]
            .iter()
            .enumerate()
            {
                label(f, inner, inner.y + 8 + i as u16, l, Tone::Muted, s.theme);
            }
            label(
                f,
                inner,
                inner.y + 12,
                "Asking less often never widens this boundary, and a saved permission never widens the one already running.",
                Tone::Muted,
                s.theme,
            );
            add(
                f,
                &mut g,
                inner,
                inner.y + 14,
                "Change how often it asks",
                Action::Approvals,
                true,
                s.theme,
            );
            add(
                f,
                &mut g,
                inner,
                inner.y + 15,
                "Open settings",
                Action::Settings,
                false,
                s.theme,
            );
        } else if ui.activity {
            let lines: Vec<_> = s.history.iter().flat_map(|n| n.text.lines()).collect();
            for (i, l) in lines.iter().skip(ui.local_scroll).enumerate() {
                label(
                    f,
                    inner,
                    inner.y + 2 + i as u16,
                    l,
                    if l.starts_with("ERROR:") {
                        Tone::Failure
                    } else {
                        Tone::Normal
                    },
                    s.theme,
                );
            }
        } else if let Some(command) = &ui.confirm {
            label(
                f,
                inner,
                inner.y + 3,
                "This removes approval prompts, not sandbox restrictions.",
                Tone::Warning,
                s.theme,
            );
            add(
                f,
                &mut g,
                inner,
                inner.y + 5,
                "Confirm · Never ask",
                Action::Rung(command.clone()),
                true,
                s.theme,
            );
        } else if let Some(panel) = &s.panel {
            let start = panel
                .selected
                .saturating_sub(inner.height.saturating_sub(5) as usize);
            for (i, r) in panel.rows.iter().enumerate().skip(start) {
                add(
                    f,
                    &mut g,
                    inner,
                    inner.y + 2 + (i - start) as u16,
                    &format!("{} {}", if i == panel.selected { "›" } else { " " }, r.text),
                    Action::PanelRow(i),
                    i == panel.selected,
                    s.theme,
                );
            }
        }
    }
    g.screen = Some(f.buffer_mut().clone());
    if let Some(sel) = s.selection {
        g.copied = crate::tui::draw_selection(f.buffer_mut(), a, sel);
    }
    ui.geometry = g;
}

fn draw_settings(
    f: &mut Frame<'_>,
    g: &mut Geometry,
    a: Rect,
    p: &super::Preferences,
    s: &ScreenState,
) {
    let y = a.y + 2;
    // Both destinations are named, and the selected one is marked: a Global
    // label must never be able to conceal a Project write.
    let mut x = a.x;
    for scope in [
        crate::settings::Scope::Global,
        crate::settings::Scope::Local,
    ] {
        let here = p.scope == scope;
        let text = if here {
            format!("[▸{} ]  ", scope.label())
        } else {
            format!("[ {} ]  ", scope.label())
        };
        let w = (text.len() as u16).min(a.right().saturating_sub(x));
        button(
            f,
            g,
            Rect::new(x, y, w, 1),
            &text,
            Action::Scope(scope == crate::settings::Scope::Global),
            here,
            s.theme,
        );
        x += w;
    }
    row(
        f,
        Rect::new(x, y, a.right().saturating_sub(x).min(12), 1),
        "F6 switches",
        Tone::Muted,
        s.theme,
    );
    if a.width > 32 {
        button(
            f,
            g,
            Rect::new(a.right() - 12, y, 12, 1),
            "Undo · ^Z",
            Action::Undo,
            false,
            s.theme,
        );
    }
    label(
        f,
        a,
        y + 1,
        &format!("{}", p.path.display()),
        Tone::Muted,
        s.theme,
    );
    let cat = CATEGORIES[p.category.min(5)];
    let next = (p.category + 1) % CATEGORIES.len();
    add(
        f,
        g,
        a,
        y + 3,
        &format!("‹ {cat} ›   Tab: next category · type to search"),
        Action::Category(next),
        true,
        s.theme,
    );
    if !p.query.is_empty() {
        label(
            f,
            a,
            y + 4,
            &format!("Search: {}", p.query),
            Tone::Normal,
            s.theme,
        );
    }
    let rows = p.rows();
    // **Two lines per row, because the second one is the whole point.** The
    // consequence used to be printed once, at the foot, for the selected row
    // only -- so reading what four settings did meant moving the cursor four
    // times and remembering what the last three had said. Guidance that
    // disappears forces a person to re-derive it, and a settings panel is
    // exactly where nobody should be re-deriving anything.
    let stride = 2u16;
    let capacity = ((a.height.saturating_sub(14).max(2)) / stride).max(1) as usize;
    let start = p.selected.saturating_sub(capacity.saturating_sub(1));
    for (i, spec) in rows.iter().enumerate().skip(start).take(capacity) {
        let y = y + 5 + (i - start) as u16 * stride;
        let selected = i == p.selected;
        let text = format!("{} {}", if selected { "›" } else { " " }, spec.label);
        let lw = (a.width / 2).min(31);
        button(
            f,
            g,
            Rect::new(a.x, y, lw, 1),
            &text,
            Action::Setting(i, None),
            selected,
            s.theme,
        );
        let options = super::Preferences::choices(spec);
        let current = p.effective(spec.key);
        let mut x = a.x + lw;
        if options.is_empty() {
            // `unset` is what a TOML table says when a key is absent, and it
            // was reaching the screen as if it were a value someone chose.
            let shown = if current == "unset" {
                match spec.kind {
                    crate::settings::Kind::Model => "choose a model".to_string(),
                    _ => "not set".to_string(),
                }
            } else {
                current.clone()
            };
            button(
                f,
                g,
                Rect::new(x, y, a.right().saturating_sub(x), 1),
                &format!("{shown}  ›"),
                Action::Setting(i, None),
                selected,
                s.theme,
            );
        } else {
            for value in options {
                let text = if value == current {
                    format!("[ {value} ] ")
                } else {
                    format!("{value} ")
                };
                let w = (text.len() as u16).min(a.right().saturating_sub(x));
                if w > 0 {
                    button(
                        f,
                        g,
                        Rect::new(x, y, w, 1),
                        &text,
                        Action::Setting(i, Some(value.clone())),
                        value == current,
                        s.theme,
                    );
                    x += w;
                }
            }
        }
        // The consequence, under the row it belongs to, and the one mark
        // that says a choice will not reach the session already running.
        let mark = if crate::settings::applies_now(spec.key) {
            String::new()
        } else {
            "  ·  next session".to_string()
        };
        row(
            f,
            Rect::new(a.x + 3, y + 1, a.width.saturating_sub(3), 1),
            &clip(
                &format!("{}{mark}", spec.description),
                a.width.saturating_sub(4) as usize,
            ),
            if selected { Tone::Normal } else { Tone::Muted },
            s.theme,
        );
    }
    let bottom = a.bottom().saturating_sub(7);
    row(
        f,
        Rect::new(a.x, bottom.saturating_sub(1), a.width, 1),
        &"─".repeat(a.width as usize),
        Tone::Line,
        s.theme,
    );
    if let Some(spec) = rows.get(p.selected) {
        // **Which layer won, in words.** The foot used to read
        // `session.effort · Saved: inherited · Effective: default
        // (built-in)`: a dotted key nobody typed, and a three-valued
        // provenance in a vocabulary the panel never defines. A person
        // looking at a value wants one thing from this line -- where it
        // came from, and whether it is theirs.
        let effective = p.effective(spec.key);
        label(
            f,
            a,
            bottom,
            &if effective == "unset" {
                "Nothing has chosen this yet.".to_string()
            } else {
                match p.saved(spec.key) {
                    Some(_) => format!(
                        "{effective} — set here, in this {} file",
                        p.scope.label().to_lowercase()
                    ),
                    None => format!(
                        "{effective} — inherited from {}",
                        match p.origin(spec.key) {
                            "built-in" => "Pane's own default",
                            "global" => "your global settings",
                            "project" => "this project's settings",
                            other => other,
                        }
                    ),
                }
            },
            Tone::Normal,
            s.theme,
        );
        label(
            f,
            a,
            bottom + 1,
            if crate::settings::applies_now(spec.key) {
                "In force now, and saved for next time."
            } else {
                "Saved. This session keeps what it started with; the next one takes it."
            },
            Tone::Muted,
            s.theme,
        );
        label(f, a, bottom + 2, spec.key, Tone::Muted, s.theme);
    }
    if let Some((key, value)) = &p.editing {
        label(
            f,
            a,
            bottom + 3,
            &format!("{key} = {value}"),
            Tone::Accent,
            s.theme,
        );
        label(
            f,
            a,
            bottom + 4,
            "Enter confirms this field · Esc cancels field only",
            Tone::Warning,
            s.theme,
        );
    } else {
        label(
            f,
            a,
            bottom + 3,
            "↑↓ Select · ←→ Change · Enter Edit · Backspace Inherit",
            Tone::Normal,
            s.theme,
        );
    }
    label(
        f,
        a,
        a.bottom().saturating_sub(1),
        &p.notice,
        Tone::Normal,
        s.theme,
    );
}
fn draw_models(
    f: &mut Frame<'_>,
    g: &mut Geometry,
    a: Rect,
    m: &super::Navigator,
    s: &ScreenState,
) {
    let roles = [
        ("Main", m.current.parent.as_str()),
        ("Helper", m.current.helper.as_deref().unwrap_or("off")),
        (
            "Subagent",
            m.current
                .subagent
                .as_deref()
                .unwrap_or("explicit model required"),
        ),
    ];
    let width = a.width / 3;
    for (i, (name, value)) in roles.into_iter().enumerate() {
        let r = Rect::new(a.x + i as u16 * width, a.y + 2, width, 1);
        let name = if i == m.role {
            format!("[▸{name} ]")
        } else {
            format!("[ {name} ]")
        };
        if m.target_key.is_none() {
            button(f, g, r, &name, Action::ModelRole(i), i == m.role, s.theme);
        } else {
            row(
                f,
                r,
                &name,
                if i == m.role {
                    Tone::Accent
                } else {
                    Tone::Normal
                },
                s.theme,
            );
        }
        row(
            f,
            Rect::new(r.x, r.y + 1, r.width, 1),
            value,
            Tone::Normal,
            s.theme,
        );
    }
    label(
        f,
        a,
        a.y + 5,
        &format!(
            "{}/{}  Search: {}",
            m.candidates().len(),
            m.catalogue_len(),
            if m.query.is_empty() {
                "type model, provider or account"
            } else {
                &m.query
            }
        ),
        Tone::Normal,
        s.theme,
    );
    add(
        f,
        g,
        a,
        a.y + 6,
        if m.all_sources {
            "Sources: all · includes unavailable"
        } else {
            "Sources: connected only"
        },
        Action::Sources,
        false,
        s.theme,
    );
    add(
        f,
        g,
        a,
        a.y + 7,
        if m.measured_order {
            "Order: AA intelligence · missing ≠ zero"
        } else {
            "Order: account / model · Ctrl-O changes"
        },
        Action::Scores,
        false,
        s.theme,
    );
    let providers = m.providers();
    let provider = providers
        .get(m.provider)
        .map(String::as_str)
        .unwrap_or("Connected accounts");
    add(
        f,
        g,
        a,
        a.y + 8,
        &format!("‹ {provider} ›   ←→ provider · Tab role"),
        Action::Provider((m.provider + 1) % providers.len()),
        true,
        s.theme,
    );
    let extra = if m.role == 2 && m.target_key.is_none() {
        let names = [
            None,
            Some("quick"),
            Some("balanced"),
            Some("deep"),
            Some("heavy"),
        ];
        let width = (a.width / 5).max(1);
        for (i, slot) in names.into_iter().enumerate() {
            let label = slot.unwrap_or("Pinned");
            button(
                f,
                g,
                Rect::new(a.x + i as u16 * width, a.y + 9, width, 1),
                label,
                Action::Slot(slot.map(str::to_owned)),
                m.slot.as_deref() == slot,
                s.theme,
            );
        }
        let chosen = m.slot.as_ref().and_then(|s| m.assignment.slots.get(s));
        let text = match (&m.slot, chosen) {
            (Some(slot), Some(value)) => format!(
                "{slot}: {} / {} effort · F7 changes slot",
                value.model,
                value.effort.name()
            ),
            (Some(slot), None) => format!("{slot}: not assigned · empty slots never inherit Main"),
            _ => "Pinned: one concrete model · no inheritance".into(),
        };
        label(f, a, a.y + 10, &text, Tone::Normal, s.theme);
        let enabled = m.assignment.mode == crate::config::AgentsMode::Roster;
        add(
            f,
            g,
            a,
            a.y + 11,
            if enabled {
                "Favorites enabled · disable"
            } else {
                "Enable configured favorites (does not use Main)"
            },
            Action::Command(format!("/subagents {}", if enabled { "off" } else { "on" })),
            false,
            s.theme,
        );
        3
    } else {
        0
    };
    let rows = m.candidates();
    // The selected row's account in full, and -- when it is locked -- why it
    // cannot be chosen, in the catalogue's own words rather than a glyph.
    if let Some(c) = rows.get(m.selected) {
        label(
            f,
            a,
            a.bottom().saturating_sub(2),
            &format!(
                "{}{}",
                c.route,
                c.reason
                    .as_deref()
                    .filter(|_| !c.available)
                    .map(|r| format!(" · {r}"))
                    .unwrap_or_default()
            ),
            if c.available {
                Tone::Muted
            } else {
                Tone::Warning
            },
            s.theme,
        );
    }
    if rows.is_empty() {
        label(
            f,
            a,
            a.y + 10 + extra,
            "No models match this search. Backspace removes a term; Ctrl-U clears it.",
            Tone::Warning,
            s.theme,
        );
    }
    let capacity = a.height.saturating_sub(18 + extra).max(1) as usize;
    let start = m.selected.saturating_sub(capacity.saturating_sub(1));
    for (i, c) in rows.iter().enumerate().skip(start).take(capacity) {
        let score = c.score.map(|v| format!(" · AA {v:.1}")).unwrap_or_default();
        add(
            f,
            g,
            a,
            a.y + 10 + extra + (i - start) as u16,
            // The lock comes before the route: a narrow terminal may cut the
            // account off the end, and *cannot be chosen* must never be what
            // it cuts. The reason follows the row, in full, below.
            &format!(
                "{} {:<34}{}{}{}",
                if m.selected == i { "›" } else { " " },
                c.model,
                if c.available { "" } else { "LOCK · " },
                c.route,
                score,
            ),
            Action::Model(i),
            m.selected == i,
            s.theme,
        );
    }
    let y = a.bottom().saturating_sub(5);
    if let Some(c) = rows.get(m.selected) {
        label(f, a, y, &c.route, Tone::Normal, s.theme);
    }
    label(
        f,
        a,
        y + 1,
        if m.target_key.is_some() {
            "Save to the selected settings scope; current runtime stays unchanged."
        } else {
            "Account shown is availability evidence; gateway chooses the route."
        },
        Tone::Normal,
        s.theme,
    );
    add(
        f,
        g,
        a,
        y + 2,
        if m.role == 2 && m.slot.is_some() {
            "Enter · Save model to this favorite"
        } else {
            "Enter · Use selected model"
        },
        Action::ChooseModel,
        true,
        s.theme,
    );
    if m.target_key.is_some() {
        add(
            f,
            g,
            a,
            y + 3,
            "Use inherited value · remove this scoped override",
            Action::UnsetModel,
            false,
            s.theme,
        );
    } else if m.role > 0 {
        add(
            f,
            g,
            a,
            y + 3,
            if m.role == 2 && m.slot.is_some() {
                "Remove this favorite"
            } else {
                "Off · do not run this tier"
            },
            Action::Command(
                if let Some(slot) = m.slot.as_ref().filter(|_| m.role == 2) {
                    format!("/subagents {slot} off")
                } else {
                    format!(
                        "/model {} off",
                        if m.role == 1 { "helper" } else { "subagent" }
                    )
                },
            ),
            false,
            s.theme,
        );
    }
    label(f, a, y + 4, &m.notice, Tone::Warning, s.theme);
}
pub(super) fn wrap_input(text: &str, width: usize) -> Vec<String> {
    let mut d = Document::default();
    d.push(text, Tone::Normal, None, width, 0);
    d.rows.into_iter().map(|r| r.text).collect()
}

/// Credential display is mask-only; no plaintext reaches a render buffer.
pub fn render_secret(f: &mut Frame<'_>, prompt: &crate::tui::SecretPrompt, theme: Theme) {
    let a = f.area();
    f.render_widget(Clear, a);
    row(
        f,
        Rect::new(a.x, a.y, a.width, 1),
        prompt.title(),
        Tone::Accent,
        theme,
    );
    if a.height > 2 {
        row(
            f,
            Rect::new(a.x, a.y + 2, a.width, 1),
            &prompt.mask(),
            Tone::Normal,
            theme,
        );
    }
    if a.height > 4 {
        row(
            f,
            Rect::new(a.x, a.y + 4, a.width, 1),
            "Enter submits · Esc cancels · Ctrl-U clears",
            Tone::Normal,
            theme,
        );
    }
}
