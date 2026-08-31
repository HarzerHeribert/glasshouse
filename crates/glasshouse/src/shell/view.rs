//! Rendering for the session shell.
//!
//! [`render`] is a pure function of a [`ShellState`] and a [`Frame`]: it reads,
//! never mutates, and never blocks, so [`super::run`] can redraw only when
//! [`super::Action`] says something changed and the tests below can drive it
//! with [`ratatui::backend::TestBackend`] instead of a real terminal.
//!
//! Nothing here computes a size by subtraction. That is the usual way a "must
//! not panic on a tiny terminal" requirement gets violated, and the tests at
//! the bottom render at 1x1 to keep it honest.

use ratatui::Frame;
use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Widget, Wrap};

use crate::config::{Layer, RoutingModelChoice};
use crate::provider::discovery::ProbeOutcome;
use crate::session::{
    SessionDisposition, SessionLifecycle, SessionPairingClass, SessionPresentation, SessionRecord,
    SessionRole,
};

use super::state::{
    KnowledgeSection, MemoryDetail, Mode, Overlay, OverviewState, ProbeKind, ProviderRow,
    SettingsPathInputView, SettingsSection, SettingsState, ShellState, ViewportGrid, format_usd,
};

/// The shell's fixed vertical chrome: title, root, session bar, viewport,
/// footer, in that order. The one place this split is computed, so
/// [`viewport_slot`] can hand the run loop the same rectangle [`render`]
/// hands [`render_viewport`] without the two ever drifting apart.
fn regions(area: Rect) -> [Rect; 5] {
    Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Min(0),
            Constraint::Length(1),
        ])
        .areas(area)
}

/// The rectangle [`render`] reserves for the session viewport, before any
/// border the viewport itself draws.
///
/// The run loop uses this — not the terminal's outer size — to tell a
/// session's pseudo-terminal and its `vt100` emulator how large a screen
/// they actually have: whichever chrome surrounds the viewport must be
/// excluded first, or the harness draws for space it does not have.
pub fn viewport_slot(area: Rect) -> Rect {
    regions(area)[3]
}

/// Draw the shell.
pub fn render(state: &ShellState, frame: &mut Frame) {
    let area = frame.area();
    let [title_area, root_area, bar_area, viewport_area, footer_area] = regions(area);

    render_title(state, frame, title_area);
    render_root(state, frame, root_area);
    render_session_bar(state, frame, bar_area);
    render_viewport(state, frame, viewport_area);
    render_footer(state, frame, footer_area);

    match state.overlay() {
        Some(Overlay::Overview) => render_overview(state, frame, area),
        Some(Overlay::Settings) => render_settings(state, frame, area),
        Some(Overlay::ProjectOverview) => render_project_overview(state, frame, area),
        Some(Overlay::SessionEvents) => render_session_events(state, frame, area),
        Some(Overlay::ProjectKnowledge) => render_project_knowledge(state, frame, area),
        Some(Overlay::RouteEvidence) => render_route_evidence(state, frame, area),
        Some(Overlay::RouteHealth) => render_route_health(state, frame, area),
        Some(Overlay::RouteDecisions) => render_route_decisions(state, frame, area),
        Some(Overlay::ProjectMemory) => render_project_memory(state, frame, area),
        None => {}
    }
}

/// The project's name and the session currently presented.
fn render_title(state: &ShellState, frame: &mut Frame, area: Rect) {
    let mut spans = vec![
        Span::styled(
            format!("glasshouse {}", state.version()),
            Style::default()
                .fg(Color::Cyan)
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
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

/// The active canonical project root, on its own line, on every frame.
///
/// This is the value the entire isolation model rests on — which project's
/// memory, state, and sessions the user is looking at — so it gets a dedicated
/// line rather than being tucked into a corner. When the line is too narrow the
/// *head* is dropped, not the tail: `…/work/glasshouse` still identifies the
/// project, while `/Users/someone/very/long/…` does not.
fn render_root(state: &ShellState, frame: &mut Frame, area: Rect) {
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
fn render_session_bar(state: &ShellState, frame: &mut Frame, area: Rect) {
    if state.sessions().is_empty() {
        frame.render_widget(
            Paragraph::new(Span::styled(
                "no sessions yet — start one with `glasshouse launch`",
                Style::default().fg(Color::DarkGray),
            )),
            area,
        );
        return;
    }

    let mut spans = Vec::new();
    for (index, session) in state.sessions().iter().enumerate() {
        let active = index == state.selected_index();
        let style = if active {
            Style::default()
                .fg(Color::Black)
                .bg(Color::Cyan)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::Gray)
        };
        spans.push(Span::styled(
            format!(" {} {} ", index + 1, session.harness),
            style,
        ));
        spans.push(Span::raw(" "));
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

/// Draws a [`ViewportGrid`] cell by cell into whatever area it is given.
///
/// A plain [`Widget`] rather than a free function taking `&mut Buffer`
/// directly, so [`render_viewport`] can hand it to [`Frame::render_widget`]
/// exactly like every other widget in this module.
struct GridView<'a> {
    grid: &'a ViewportGrid,
    /// Only in [`Mode::Session`] is the keyboard actually reaching this
    /// screen, so only then does the cursor mean anything to point at — a
    /// blinking cursor while Glasshouse itself owns the keyboard would be
    /// showing where nothing is being typed.
    show_cursor: bool,
}

impl Widget for GridView<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        // Clip to whichever is smaller: the render area or the screen the
        // session actually has. The two should agree — the run loop resizes the
        // emulator to match the viewport — but a resize in flight should not be
        // drawing past either one.
        //
        // Honest about what this is worth: it is cheap insurance, not the thing
        // keeping the frame intact. `Buffer::cell_mut` already refuses anything
        // outside the buffer, and the chrome below the viewport is drawn after
        // it, so removing this clamp changes no rendered frame — verified by
        // mutation. It is kept because bounding a loop by the space it was
        // given is right regardless, and because the render order it currently
        // relies on is not a property this widget can see or enforce.
        let rows = area.height.min(self.grid.rows());
        let cols = area.width.min(self.grid.cols());
        for row in 0..rows {
            for col in 0..cols {
                let Some((text, style)) = self.grid.cell(row, col) else {
                    continue;
                };
                let Some(cell) = buf.cell_mut((area.x + col, area.y + row)) else {
                    continue;
                };
                cell.set_symbol(if text.is_empty() { " " } else { text.as_str() });
                cell.set_style(*style);
            }
        }

        if !self.show_cursor {
            return;
        }
        let Some((row, col)) = self.grid.cursor() else {
            return;
        };
        if row >= rows || col >= cols {
            return;
        }
        if let Some(cell) = buf.cell_mut((area.x + col, area.y + row)) {
            cell.set_style(cell.style().add_modifier(Modifier::REVERSED));
        }
    }
}

/// The region reserved for the active session's terminal.
///
/// Once the run loop has converted the focused session's `vt100` screen to a
/// [`ViewportGrid`] via [`ShellState::set_viewport_grid`], that grid is drawn
/// cell by cell — colours, bold/italic/underline/inverse, and the cursor all
/// carried over from the emulator. A session that has not produced one yet,
/// or none is active, falls back to the placeholder below, which explains
/// what will occupy the space rather than faking a terminal that would
/// suggest more is attached than really is.
///
/// A live grid gets the *whole* area, with no border: Phase 5 requires the
/// embedded harness to stay visually dominant while Glasshouse's own chrome
/// stays minimal, and a border spends one row and two columns of every frame
/// on Glasshouse instead of the product it is hosting. The placeholder keeps
/// its border and title, since there is no harness screen yet for a border
/// to compete with.
fn render_viewport(state: &ShellState, frame: &mut Frame, area: Rect) {
    // A headless session never becomes the viewport's, and this is the last
    // place that could go wrong. `shell::run` already declines to *build* a
    // grid from one, which keeps `viewport_grid` an honest description of
    // what is on screen; the check here is what makes the guarantee hold for
    // a grid that is merely stale — the grid is rebuilt on the tick, so the
    // frame drawn immediately after the bar moves onto a headless session
    // still carries the previous session's screen.
    let headless = state
        .active_session()
        .is_some_and(|session| session.presentation == SessionPresentation::Headless);
    let grid = state.viewport_grid();
    if !headless && !grid.is_empty() {
        frame.render_widget(
            GridView {
                grid,
                show_cursor: state.mode() == Mode::Session,
            },
            area,
        );
        return;
    }

    let block = Block::default().borders(Borders::ALL).title(
        state
            .active_session()
            .map(|session| format!(" {} ", session.harness))
            .unwrap_or_else(|| " session ".to_owned()),
    );
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let lines = match state.active_session() {
        Some(session) => vec![
            Line::from(Span::styled(
                format!("session {}", session.id),
                Style::default().add_modifier(Modifier::BOLD),
            )),
            Line::from(format!("harness       {}", session.harness)),
            Line::from(format!("state         {}", session.lifecycle)),
            Line::from(format!("presented     {}", session.presentation)),
            Line::from(format!("role          {}", session.role)),
            Line::from(""),
            Line::from(Span::styled(
                // A headless session has a screen and is deliberately not
                // shown it — see `shell::run`'s viewport-grid build. Saying
                // which of the two cases this is matters: an empty viewport
                // for a session that is running fine otherwise reads as a
                // broken renderer.
                if session.presentation == SessionPresentation::Headless {
                    "This session is headless: it runs with no viewport."
                } else {
                    "This viewport is reserved for the session's own terminal."
                },
                Style::default().fg(Color::DarkGray),
            )),
        ],
        None => vec![
            Line::from(Span::styled(
                "No session is active.",
                Style::default().add_modifier(Modifier::BOLD),
            )),
            Line::from(""),
            Line::from(Span::styled(
                "Run `glasshouse launch` to start one.",
                Style::default().fg(Color::DarkGray),
            )),
        ],
    };
    frame.render_widget(Paragraph::new(lines), inner);
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
fn render_footer(state: &ShellState, frame: &mut Frame, area: Rect) {
    let hint = match (state.mode(), state.overlay()) {
        (Mode::Session, _) => "SESSION MODE -- keys go to the session -- ctrl-] for glasshouse",
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
            "tab session   enter session   n new   N headless   o overview   p project   \
             k knowledge   M memory   e events   r routes   h health   d decisions   q quit"
        }
    };
    let mut spans = vec![Span::styled(hint, Style::default().fg(Color::DarkGray))];

    if let Some(status) = state.status() {
        spans.push(Span::raw("   "));
        spans.push(Span::styled(status, Style::default().fg(Color::Yellow)));
    }

    // Order is the whole mechanism: the bindings are written first, so when the
    // row is too narrow it is the note that gets clipped away — and within the
    // bindings, the ones that come first are the ones that survive. Adding
    // `t`/`m` for Phase 9D pushed the row past a realistic hundred columns and
    // clipped `w save` off the end, which is how the ordering below came to put
    // saving ahead of the secret-store keys: losing "how do I keep my edits" is
    // worse than losing "how do I store a credential", and something had to go. The bindings are
    // needed permanently and the note only once, so that is the right thing to
    // lose. An earlier version measured the remaining width and truncated the
    // note itself, which turned out to be an elaborate way of duplicating the
    // clipping Ratatui already does — and a mutation removing the measurement
    // changed nothing on screen, which is how it was found.
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

/// The session overview, drawn over the shell rather than replacing it.
///
/// Over, not instead of: the shell stays visible around the edges so it is
/// obvious the session is still there and still running, and that leaving the
/// overlay goes back to it.
fn render_overview(state: &ShellState, frame: &mut Frame, area: Rect) {
    let popup = centered(area, 80, 70);
    frame.render_widget(Clear, popup);

    let block = Block::default()
        .borders(Borders::ALL)
        .title(" sessions ")
        .style(Style::default().bg(Color::Black));
    let inner = block.inner(popup);
    frame.render_widget(block, popup);

    // `NAME` and `ACTIVE` are written last, after `SESSION`, on purpose: this
    // popup's width scales with the terminal's, and at a realistic width the
    // row is the first thing to run out of room. Everything the interrupt,
    // send and resume keys need to identify a row — the harness, the state,
    // whether it is presented, and the identifier those keys' own status
    // notes quote — comes first and is what survives a narrow terminal; the
    // two columns this phase adds are what clips. See
    // `the_new_overview_columns_survive_a_realistic_and_a_wide_width` for the
    // proof, and its doc comment for what "realistic" turned out to mean here.
    let mut lines = vec![Line::from(Span::styled(
        format!(
            "  {:<14}  {:<12}  {:<10}  {:<12}  {:<12}  {:<14}  {:<16}",
            "HARNESS", "STATE", "ROLE", "PRESENTED", "SESSION", "ACTIVE", "NAME"
        ),
        Style::default().add_modifier(Modifier::BOLD),
    ))];

    if state.sessions().is_empty() {
        lines.push(Line::from(Span::styled(
            "This project has no recorded sessions.",
            Style::default().fg(Color::DarkGray),
        )));
    }

    let cursor = state.overview().map(OverviewState::cursor);
    let now = crate::provider::cache::now_unix_seconds();
    for (index, session) in state.sessions().iter().enumerate() {
        // Two different facts, and conflating them is what would make the
        // overview useless for its own capability: the *cursor* is the row a
        // sent line or an interrupt acts on, and `(viewport)` marks the
        // session the shell is presenting. They are usually different rows —
        // that is the point — so each gets its own mark.
        let selected = cursor == Some(index);
        let style = if selected {
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default()
        };
        let presented = if index == state.selected_index() {
            "  (viewport)"
        } else {
            ""
        };
        lines.push(Line::from(Span::styled(
            format!(
                "{} {:<14}  {:<12}  {:<10}  {:<12}  {:<12}{}  {:<14}  {}",
                if selected { ">" } else { " " },
                session.harness,
                state_label(session),
                session.role,
                session.presentation,
                short_id(session),
                presented,
                describe_age(now, session.last_activity_at),
                name_or_purpose(session),
            ),
            style,
        )));
    }

    // The activity section: recent lifecycle events, under the session table
    // and above the status note. Omitted entirely when there is nothing to
    // show — an empty "ACTIVITY" heading is worse than none, because it reads
    // as "nothing has happened" rather than "nothing has been observed yet".
    if !state.activity().is_empty() {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "ACTIVITY",
            Style::default().add_modifier(Modifier::BOLD),
        )));
        let now = crate::provider::cache::now_unix_seconds();
        for recorded in state.activity() {
            lines.push(Line::from(format!(
                "  {:<28} {:<12} {}",
                super::state::describe_event(recorded.event()),
                super::state::short_session_id(recorded.session()),
                describe_age(now, recorded.at()),
            )));
        }
    }

    // A refusal about a row belongs beside the rows. The footer shows notes
    // too, but the footer writes the key bindings first and lets the note
    // clip — which is the right trade there and the wrong one here, because
    // "`ab12cd34` is stopped, not running" clipped to its first thirty
    // columns loses the identifier that makes it an answer. There is room
    // inside the popup, so this is where the note is guaranteed to be
    // readable.
    if let Some(status) = state.status() {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            status.to_owned(),
            Style::default().fg(Color::Yellow),
        )));
    }

    // The one-line field for sending text to the session under the cursor.
    // Drawn under the table rather than as its own popup so the row it is
    // aimed at stays on screen while the line is typed — a field that hides
    // its own target is how text ends up in the wrong session.
    if let Some(entry) = state.overview().and_then(OverviewState::entry) {
        let target = state
            .overview_target()
            .map(short_id)
            .unwrap_or_else(|| "?".to_owned());
        lines.push(Line::from(""));
        lines.push(Line::from(vec![
            Span::styled(
                format!("send to {target}: "),
                Style::default().add_modifier(Modifier::BOLD),
            ),
            // A block cursor after the text, so an empty field still looks
            // like a field waiting for input rather than a finished line.
            Span::raw(format!("{entry}_")),
        ]));
        lines.push(Line::from(Span::styled(
            "enter sends   esc cancels",
            Style::default().fg(Color::DarkGray),
        )));
    }

    frame.render_widget(Paragraph::new(lines), inner);
}

/// The project overview — Phase 41's project-level sibling of
/// [`render_overview`]: where that popup answers "what is this session
/// doing", this one answers "what is this project doing".
///
/// Read-only by construction (map line 1664: no decorative AI commentary) —
/// every line here is either a [`SessionRecord`] field already recorded on
/// the production launch and lifecycle paths, or a memory the run loop read
/// straight from [`crate::memory::store::MemoryStore`] with no summarizing
/// model in between. A section with nothing to show says so in words rather
/// than being silently absent, for the same reason the session overview's
/// "no recorded sessions" line exists.
fn render_project_overview(state: &ShellState, frame: &mut Frame, area: Rect) {
    let popup = centered(area, 84, 78);
    frame.render_widget(Clear, popup);

    let block = Block::default()
        .borders(Borders::ALL)
        .title(" project ")
        .style(Style::default().bg(Color::Black));
    let inner = block.inner(popup);
    frame.render_widget(block, popup);

    let mut lines = Vec::new();

    let orchestrator = state
        .sessions()
        .iter()
        .find(|session| session.role == SessionRole::Orchestrator);
    lines.push(Line::from(Span::styled(
        "ORCHESTRATOR",
        Style::default().add_modifier(Modifier::BOLD),
    )));
    match orchestrator {
        Some(session) => {
            lines.push(Line::from(format!(
                "  {}  {}",
                short_id(session),
                session_detail(session)
            )));
        }
        None => lines.push(Line::from(Span::styled(
            "  no session is designated as this project's orchestrator",
            Style::default().fg(Color::DarkGray),
        ))),
    }

    let workers: Vec<&SessionRecord> = state
        .sessions()
        .iter()
        .filter(|session| session.role == SessionRole::Worker)
        .collect();
    let running: Vec<&&SessionRecord> = workers
        .iter()
        .filter(|session| session.lifecycle == SessionLifecycle::Running)
        .collect();
    let waiting: Vec<&&SessionRecord> = workers
        .iter()
        .filter(|session| session.lifecycle == SessionLifecycle::WaitingForUser)
        .collect();
    let mut completed: Vec<&&SessionRecord> = workers
        .iter()
        .filter(|session| {
            matches!(
                session.disposition(),
                SessionDisposition::Closed | SessionDisposition::Resumable
            )
        })
        .collect();
    completed.sort_by_key(|session| std::cmp::Reverse(session.last_activity_at));
    completed.truncate(RECENTLY_COMPLETED_ROWS);

    lines.push(Line::from(""));
    push_worker_section(
        &mut lines,
        "RUNNING WORKERS",
        &running,
        "no workers running",
    );
    lines.push(Line::from(""));
    push_worker_section(
        &mut lines,
        "WORKERS WAITING FOR INPUT",
        &waiting,
        "no workers waiting for input",
    );
    lines.push(Line::from(""));
    push_worker_section(
        &mut lines,
        "RECENTLY COMPLETED WORKERS",
        &completed,
        "no completed workers recorded",
    );

    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "ACTIVE DECISIONS AND CONSTRAINTS",
        Style::default().add_modifier(Modifier::BOLD),
    )));
    if state
        .project_overview()
        .is_some_and(|o| !o.decisions().is_empty())
    {
        for line in state
            .project_overview()
            .into_iter()
            .flat_map(|o| o.decisions())
        {
            lines.push(Line::from(format!("  {line}")));
        }
    } else {
        lines.push(Line::from(Span::styled(
            "  no current binding decisions or constraints",
            Style::default().fg(Color::DarkGray),
        )));
    }

    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "UNRESOLVED MEMORY TODOS",
        Style::default().add_modifier(Modifier::BOLD),
    )));
    if let Some(overview) = state.project_overview() {
        if overview.todos().is_empty() {
            lines.push(Line::from(Span::styled(
                "  no open todos in project memory",
                Style::default().fg(Color::DarkGray),
            )));
        } else {
            for line in overview.todos() {
                lines.push(Line::from(format!("  {line}")));
            }
            if overview.todos_omitted() > 0 {
                lines.push(Line::from(Span::styled(
                    format!("  ...and {} more", overview.todos_omitted()),
                    Style::default().fg(Color::DarkGray),
                )));
            }
        }
    }

    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "RESOURCE STATE",
        Style::default().add_modifier(Modifier::BOLD),
    )));
    let resources = state
        .project_overview()
        .map(crate::shell::state::ProjectOverviewState::resources)
        .unwrap_or_default();
    if resources.is_empty() {
        lines.push(Line::from(Span::styled(
            "  no resources configured for this project",
            Style::default().fg(Color::DarkGray),
        )));
    } else {
        for line in resources {
            lines.push(Line::from(line.clone()));
        }
    }
    // Line 1661 — the currently selected routing model and its recent
    // latency. Built by `shell::build_project_overview_routing`, which always
    // has something honest to say (a model name, or why there is not one) —
    // an empty string here means only the test fixtures above that never set
    // one, never a real overview the run loop opened.
    if let Some(routing) = state
        .project_overview()
        .map(crate::shell::state::ProjectOverviewState::routing)
        .filter(|line| !line.is_empty())
    {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "ROUTING MODEL",
            Style::default().add_modifier(Modifier::BOLD),
        )));
        lines.push(Line::from(routing.to_owned()));
    }

    if let Some(note) = state.project_overview().and_then(|o| o.memory_note()) {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            note.to_owned(),
            Style::default().fg(Color::Yellow),
        )));
    }

    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
}

/// Map line 1758: the *presented* session's own recent lifecycle events —
/// deliberately its own overlay, reached by its own key, rather than folded
/// into [`render_overview`]'s cross-session ACTIVITY feed. That feed already
/// shows recent events across every session and is drawn every time the
/// routine session-management popup opens; a diagnostic surface answering
/// "what has this one session been doing" stays a diagnostic surface, per
/// line 1770, only if opening it is a separate, deliberate act.
///
/// Filters [`ShellState::activity`] rather than reading
/// [`crate::events::log::EventLog`]: the buffer already holds exactly the
/// events this build records, in memory, with no file I/O this pure render
/// function is not allowed to perform (see the module doc). A session with
/// more history than [`super::state::ACTIVITY_ROWS`] keeps only its most
/// recent rows, the same bound `render_overview`'s feed already accepts.
fn render_session_events(state: &ShellState, frame: &mut Frame, area: Rect) {
    let popup = centered(area, 80, 60);
    frame.render_widget(Clear, popup);

    let block = Block::default()
        .borders(Borders::ALL)
        .title(" session events ")
        .style(Style::default().bg(Color::Black));
    let inner = block.inner(popup);
    frame.render_widget(block, popup);

    let mut lines = Vec::new();

    let Some(session) = state.active_session() else {
        lines.push(Line::from(Span::styled(
            "no session is presented",
            Style::default().fg(Color::DarkGray),
        )));
        frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
        return;
    };

    lines.push(Line::from(Span::styled(
        format!("{}  {}", short_id(session), session_detail(session)),
        Style::default().add_modifier(Modifier::BOLD),
    )));
    lines.push(Line::from(""));

    let now = crate::provider::cache::now_unix_seconds();
    let events: Vec<_> = state
        .activity()
        .iter()
        .filter(|recorded| recorded.session() == &session.id)
        .collect();

    if events.is_empty() {
        lines.push(Line::from(Span::styled(
            "no recent lifecycle events recorded for this session",
            Style::default().fg(Color::DarkGray),
        )));
    } else {
        for recorded in events {
            lines.push(Line::from(format!(
                "  {:<28} {}",
                super::state::describe_event(recorded.event()),
                describe_age(now, recorded.at()),
            )));
        }
    }

    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
}

/// Phase 25, map lines 1098-1107: the project's durable knowledge — active
/// decisions, known constraints, implemented-or-planned features, failed
/// approaches (historical), and unresolved todos — each grouped under its
/// own labelled section of plain text.
///
/// **Map line 1107, by construction.** This function draws [`Line`]s of
/// text and nothing else: no canvas, no coordinates, no box-drawing
/// characters standing in for a "node". A relationship between two memories
/// — the only one this view has, supersession — is said in a sentence
/// (`knowledge_line` in `shell::mod`), never drawn as an edge. See
/// `the_project_knowledge_view_renders_no_decorative_graph_glyphs` below,
/// proven at a realistic width and a wide one per practice §17.
fn render_project_knowledge(state: &ShellState, frame: &mut Frame, area: Rect) {
    let popup = centered(area, 84, 78);
    frame.render_widget(Clear, popup);

    let knowledge = state.project_knowledge();
    let showing_detail = knowledge.is_some_and(super::state::ProjectKnowledgeState::detail_open);

    let block = Block::default()
        .borders(Borders::ALL)
        .title(if showing_detail {
            " memory detail "
        } else {
            " project knowledge "
        })
        .style(Style::default().bg(Color::Black));
    let inner = block.inner(popup);
    frame.render_widget(block, popup);

    if showing_detail {
        render_memory_detail_popup(
            knowledge.and_then(super::state::ProjectKnowledgeState::selected),
            frame,
            inner,
        );
        return;
    }

    let mut lines = Vec::new();
    let cursor = knowledge.map(super::state::ProjectKnowledgeState::cursor);
    let mut index = 0usize;

    push_knowledge_section(
        &mut lines,
        "ACTIVE DECISIONS",
        knowledge.map(super::state::ProjectKnowledgeState::decisions),
        "no active decisions recorded",
        cursor,
        &mut index,
    );
    lines.push(Line::from(""));
    push_knowledge_section(
        &mut lines,
        "KNOWN CONSTRAINTS",
        knowledge.map(super::state::ProjectKnowledgeState::constraints),
        "no known constraints recorded",
        cursor,
        &mut index,
    );
    lines.push(Line::from(""));
    push_knowledge_section(
        &mut lines,
        "FEATURES (IMPLEMENTED OR PLANNED)",
        knowledge.map(super::state::ProjectKnowledgeState::features),
        "no features recorded",
        cursor,
        &mut index,
    );
    lines.push(Line::from(""));
    push_knowledge_section(
        &mut lines,
        "FAILED APPROACHES (HISTORICAL)",
        knowledge.map(super::state::ProjectKnowledgeState::failed_attempts),
        "no failed approaches recorded",
        cursor,
        &mut index,
    );
    lines.push(Line::from(""));
    push_knowledge_section(
        &mut lines,
        "UNRESOLVED TODOS",
        knowledge.map(super::state::ProjectKnowledgeState::todos),
        "no unresolved todos",
        cursor,
        &mut index,
    );

    if let Some(note) = knowledge.and_then(super::state::ProjectKnowledgeState::memory_note) {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            note.to_owned(),
            Style::default().fg(Color::Yellow),
        )));
    }

    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
}

/// Map line 1105: the detail popup for whichever memory the cursor is on —
/// its rationale, source session, source commit and lifecycle state, each
/// said honestly as "not recorded" rather than left blank when the producer
/// never captured one (`MemoryDetail`'s own doc comment). `lifecycle` is
/// never absent, so it never gets that treatment.
///
/// Shared by [`render_project_knowledge`] and [`render_project_memory`] —
/// both popups show the same fields for the same reason: this function
/// takes the already-selected `(line, detail)` pair rather than either
/// overlay's own state type, so neither has to import the other's.
fn render_memory_detail_popup(
    selected: Option<(&str, &MemoryDetail)>,
    frame: &mut Frame,
    inner: Rect,
) {
    let mut lines = Vec::new();
    match selected {
        Some((text, detail)) => {
            lines.push(Line::from(Span::styled(
                text.to_owned(),
                Style::default().add_modifier(Modifier::BOLD),
            )));
            lines.push(Line::from(""));
            lines.push(Line::from(format!(
                "rationale: {}",
                detail.rationale.as_deref().unwrap_or("not recorded")
            )));
            lines.push(Line::from(format!(
                "source session: {}",
                detail.source_session.as_deref().unwrap_or("not recorded")
            )));
            lines.push(Line::from(format!(
                "source commit: {}",
                detail.source_commit.as_deref().unwrap_or("not recorded")
            )));
            lines.push(Line::from(format!("lifecycle: {}", detail.lifecycle)));
        }
        None => {
            lines.push(Line::from(Span::styled(
                "nothing selected",
                Style::default().fg(Color::DarkGray),
            )));
        }
    }
    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
}

/// One labelled section of the project-knowledge view: a bold heading, then
/// either its entries (each on its own line, with a trailing "...and N more"
/// when the section's budget left some out) or `empty_note` when there is
/// nothing to show — the same honest-empty-state shape
/// [`push_worker_section`] uses for the project overview.
///
/// `cursor` and `index` are map line 1105's selection: `index` is the
/// running position across every section rendered so far (the same order
/// [`super::state::ProjectKnowledgeState::selected`] walks), and the entry
/// at `cursor` gets a leading `> ` instead of two spaces so the cursor is
/// visible without a second pass over the rendered lines.
fn push_knowledge_section(
    lines: &mut Vec<Line<'static>>,
    title: &'static str,
    section: Option<&KnowledgeSection>,
    empty_note: &'static str,
    cursor: Option<usize>,
    index: &mut usize,
) {
    lines.push(Line::from(Span::styled(
        title,
        Style::default().add_modifier(Modifier::BOLD),
    )));
    let entries = section.map(|s| s.lines.as_slice()).unwrap_or_default();
    if entries.is_empty() {
        lines.push(Line::from(Span::styled(
            format!("  {empty_note}"),
            Style::default().fg(Color::DarkGray),
        )));
        return;
    }
    for line in entries {
        let marker = if cursor == Some(*index) { "> " } else { "  " };
        lines.push(Line::from(format!("{marker}{line}")));
        *index += 1;
    }
    if let Some(omitted) = section.map(|s| s.omitted).filter(|omitted| *omitted > 0) {
        lines.push(Line::from(Span::styled(
            format!("  ...and {omitted} more"),
            Style::default().fg(Color::DarkGray),
        )));
    }
}

/// Map line 234: the project's raw memory — every kind, at every status,
/// unfiltered — the `M` key opens.
///
/// [`render_project_knowledge`]'s sibling with the five labelled, curated
/// sections collapsed into the one list [`push_knowledge_section`] already
/// knows how to draw: this overlay answers "what does this project
/// remember", not "what has this project learned", so there is nothing here
/// to group by status or kind. Deliberately plain text, the same map line
/// 1107 rule `render_project_knowledge` follows — see
/// `the_project_memory_view_renders_no_decorative_graph_glyphs` below.
fn render_project_memory(state: &ShellState, frame: &mut Frame, area: Rect) {
    let popup = centered(area, 84, 78);
    frame.render_widget(Clear, popup);

    let memory = state.project_memory();
    let showing_detail = memory.is_some_and(super::state::ProjectMemoryState::detail_open);

    let block = Block::default()
        .borders(Borders::ALL)
        .title(if showing_detail {
            " memory detail "
        } else {
            " project memory "
        })
        .style(Style::default().bg(Color::Black));
    let inner = block.inner(popup);
    frame.render_widget(block, popup);

    if showing_detail {
        render_memory_detail_popup(
            memory.and_then(super::state::ProjectMemoryState::selected),
            frame,
            inner,
        );
        return;
    }

    let mut lines = Vec::new();
    let cursor = memory.map(super::state::ProjectMemoryState::cursor);
    let mut index = 0usize;

    push_knowledge_section(
        &mut lines,
        "MEMORY",
        memory.map(super::state::ProjectMemoryState::memory),
        "no memory recorded for this project",
        cursor,
        &mut index,
    );

    if let Some(note) = memory.and_then(super::state::ProjectMemoryState::memory_note) {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            note.to_owned(),
            Style::default().fg(Color::Yellow),
        )));
    }

    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
}

/// Phase 47, map lines 1762 and 1764: a compact table of the distinct
/// routing identities this project's gateway has actually recorded.
///
/// **Deliberately three columns, not line 1762's seven.** SAMPLES and WINDOW
/// are the two of the line's seven this producer can supply at all — TTFC,
/// effective TTFC, TTFT, decode throughput and rounds-per-minute have no
/// producer on this gateway (see `crate::routing::evidence`'s own module
/// header) — plus CONTEXT for line 1764. Rendering a column for any of the
/// five absent figures would be a fabricated measurement, which is exactly
/// what this phase's own "observability without spectacle" heading forbids,
/// so this function has no code path that could draw one. See
/// `no_fabricated_columns_appear_in_the_route_evidence_table` below, proved
/// at a wide viewport per practice §17.
///
/// Line 1764's honesty: CONTEXT shows exactly what
/// [`crate::shell::state::RouteEvidenceRow::context_state`] already carries as a
/// plain string (`"warm"`, `"cold"`, or `"unknown"`) — and today, in real
/// production data, every row reads `"unknown"`, because nothing that
/// records a routing observation ever calls
/// `crate::routing::evidence::NewObservation::with_context_state`. This
/// table shows that plainly rather than omitting the column or guessing.
fn render_route_evidence(state: &ShellState, frame: &mut Frame, area: Rect) {
    let popup = centered(area, 84, 60);
    frame.render_widget(Clear, popup);

    let block = Block::default()
        .borders(Borders::ALL)
        .title(" route evidence ")
        .style(Style::default().bg(Color::Black));
    let inner = block.inner(popup);
    frame.render_widget(block, popup);

    let mut lines = Vec::new();
    let evidence = state.route_evidence();
    let rows = evidence
        .map(crate::shell::state::RouteEvidenceState::rows)
        .unwrap_or_default();

    lines.push(Line::from(Span::styled(
        format!(
            "  {:<20} {:<20} {:<16} {:<8} {:<8} {}",
            "PROVIDER", "MODEL", "ROUTE", "SAMPLES", "CONTEXT", "WINDOW"
        ),
        Style::default().add_modifier(Modifier::BOLD),
    )));

    if rows.is_empty() {
        lines.push(Line::from(Span::styled(
            "  no routing evidence recorded yet",
            Style::default().fg(Color::DarkGray),
        )));
    } else {
        let now = crate::provider::cache::now_unix_seconds();
        for row in rows {
            let (window_start, window_end) = (row.window_start_unix, row.window_end_unix);
            lines.push(Line::from(format!(
                "  {:<20} {:<20} {:<16} {:<8} {:<8} {}",
                row.provider,
                row.model,
                row.route.as_deref().unwrap_or("(no route)"),
                row.sample_count,
                row.context_state,
                describe_window(now, window_start, window_end),
            )));
        }
    }

    if let Some(note) = evidence.and_then(crate::shell::state::RouteEvidenceState::note) {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            note.to_owned(),
            Style::default().fg(Color::Yellow),
        )));
    }

    frame.render_widget(Paragraph::new(lines), inner);
}

/// Why Glasshouse routed its own recent support jobs the way it did.
///
/// # This draws stored text and computes nothing
///
/// The rationale on screen is the sentence
/// `main.rs::disposable_extraction_model` rendered at the moment it decided,
/// carried through `crate::evaluation` and
/// `crate::shell::build_route_decision_table` unchanged. This function splits
/// it into rendered rows and indents it under its own heading; it does not
/// parse it, does not rank the contributions, and does not summarise them.
///
/// That is not modesty about effort — it is the same invariant
/// [`crate::shell::state::RouteDecisionRow`] documents. The decision behind a
/// row is a `crate::routing::disposable::DisposableChoice`, which nothing
/// outside its own module can construct, so there is no version of this
/// function that could re-derive a field the producer did not write. What was
/// not recorded is drawn as *not recorded*, never as a blank column.
///
/// # The newest decisions are at the top, and a long list is clipped
///
/// A decision is a heading plus one line per named contribution, so a full
/// [`crate::shell::ROUTE_DECISION_ROW_LIMIT`] of them is longer than a
/// terminal. Newest first means the clipping falls on the oldest, which is
/// the order a reader wants — the same trade `render_route_health` already
/// makes for a project with many resources.
fn render_route_decisions(state: &ShellState, frame: &mut Frame, area: Rect) {
    let popup = centered(area, 84, 60);
    frame.render_widget(Clear, popup);

    let block = Block::default()
        .borders(Borders::ALL)
        .title(" routing decisions ")
        .style(Style::default().bg(Color::Black));
    let inner = block.inner(popup);
    frame.render_widget(block, popup);

    let decisions = state.route_decisions();
    let rows = decisions
        .map(crate::shell::state::RouteDecisionsState::rows)
        .unwrap_or_default();

    let mut lines = Vec::new();
    if rows.is_empty() {
        lines.push(Line::from(Span::styled(
            "  no routing decision has been recorded yet",
            Style::default().fg(Color::DarkGray),
        )));
    } else {
        let now = crate::provider::cache::now_unix_seconds();
        for row in rows {
            let session = row.session_id.as_deref().unwrap_or("(no session recorded)");
            lines.push(Line::from(Span::styled(
                format!(
                    "  {} — {}, for session {session}",
                    row.job,
                    describe_age(now, row.observed_at_unix)
                ),
                Style::default().add_modifier(Modifier::BOLD),
            )));
            match row.rationale.as_deref() {
                Some(rationale) => {
                    for line in rationale.lines() {
                        lines.push(Line::from(format!("    {}", line.trim_end())));
                    }
                }
                None => lines.push(Line::from(Span::styled(
                    "    (no rationale recorded)",
                    Style::default().fg(Color::DarkGray),
                ))),
            }
            lines.push(Line::from(""));
        }
    }

    if let Some(note) = decisions.and_then(crate::shell::state::RouteDecisionsState::note) {
        lines.push(Line::from(Span::styled(
            note.to_owned(),
            Style::default().fg(Color::Yellow),
        )));
    }

    frame.render_widget(Paragraph::new(lines), inner);
}

/// The observation window `(start, end)` in words, using real recorded
/// timestamps — never a placeholder. A single-sample identity has `start ==
/// end` and says how long ago that one observation was; a wider window says
/// both ends, so two identities with different windows read differently.
fn describe_window(now: i64, start: i64, end: i64) -> String {
    if start == end {
        describe_age(now, end)
    } else {
        format!("{} – {}", describe_age(now, start), describe_age(now, end))
    }
}

/// Phase 47, map line 1765: *"show route health, immediate availability,
/// cadence, quota reset, and failure-domain evidence as separate concepts."*
///
/// # The line is about separation, and separation is the whole implementation
///
/// Every resource gets **five labelled lines**, one per concept, in the order
/// the line names them. Nothing here computes a summary word across them,
/// because the five genuinely disagree in ordinary operation: a resource with
/// a refused credential is *unavailable* while its failure streak reads zero;
/// a resource Glasshouse is pacing is *available later* and perfectly healthy;
/// a provider's quota reset and Glasshouse's own cooldown are two clocks owned
/// by two parties. `crate::provider::resources::render_health` prints the
/// first three as one `status` word on one line today — that is the shape this
/// function exists not to reproduce, and
/// `route_health_keeps_line_1765s_five_concepts_on_separate_lines` fails if it
/// ever does.
///
/// # Nothing is derived, and "unknown" is printed rather than guessed
///
/// Every value comes from a field
/// [`crate::shell::state::RouteHealthRow`] already carries, which the run loop
/// read from a cache a gateway process wrote. Three of the five concepts
/// depend on headers most providers never send, and each of those prints
/// `unknown` — never `0`, never `never`, never an estimate. A zero would read
/// as a measurement, which is the thing this phase is named after not doing.
///
/// # Two scope facts, both said on screen rather than assumed
///
/// The caches are keyed by provider under
/// [`crate::paths::RuntimePaths::data_dir`], so these readings belong to the
/// installation, not to this project; the header line says so. And
/// failure-domain evidence can never read `independent` — see
/// [`crate::routing::domain::FailureDomain`], whose only producer cannot
/// return it — so the line says what the absence of evidence does and does
/// not mean.
fn render_route_health(state: &ShellState, frame: &mut Frame, area: Rect) {
    let popup = centered(area, 84, 70);
    frame.render_widget(Clear, popup);

    let block = Block::default()
        .borders(Borders::ALL)
        .title(" route health ")
        .style(Style::default().bg(Color::Black));
    let inner = block.inner(popup);
    frame.render_widget(block, popup);

    let mut lines = vec![Line::from(Span::styled(
        "  observed by this installation's gateways, per provider — not scoped to this project",
        Style::default().fg(Color::DarkGray),
    ))];

    let rows = state
        .route_health()
        .map(crate::shell::state::RouteHealthState::rows)
        .unwrap_or_default();

    if rows.is_empty() {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "  no gateway exchange has been observed for any resource yet",
            Style::default().fg(Color::DarkGray),
        )));
        frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
        return;
    }

    let now = crate::provider::cache::now_unix_seconds();
    for row in rows {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            format!(
                "  {} / {} ({})",
                row.provider, row.model, row.credential_label
            ),
            Style::default().add_modifier(Modifier::BOLD),
        )));

        // 1. Route health. A streak, and it says so: `consecutive_failures`
        //    resets to zero on any success, so calling it a total would be a
        //    number more precise than the evidence behind it.
        lines.push(Line::from(format!(
            "    route health           {} consecutive failure(s) since the last success; \
             credential rejected: {}",
            row.consecutive_failures,
            yes_no(row.credential_rejected),
        )));

        // 2. Immediate availability. The producer's own answer, kept apart
        //    from the health above because the two disagree in both
        //    directions.
        lines.push(Line::from(format!(
            "    immediate availability {}",
            if row.available_now {
                "yes — may be scheduled right now"
            } else {
                "no — not schedulable right now"
            }
        )));

        // 3. Cadence: two pacing facts owned by two parties, neither derived
        //    from the other.
        lines.push(Line::from(format!(
            "    cadence                glasshouse pacing: {}; provider stated: {}",
            match row.cooling_down_until_unix {
                Some(until) if until > now =>
                    format!("cooling down, ends {}", describe_deadline(now, until)),
                Some(_) => "cooldown elapsed".to_owned(),
                None => "none".to_owned(),
            },
            match (row.stated_limit, row.stated_window_seconds) {
                (Some(limit), Some(window)) => format!("{limit} request(s) per {window}s"),
                (Some(limit), None) => format!("{limit} request(s) per an unknown window"),
                (None, Some(window)) => format!("an unknown ceiling per {window}s"),
                (None, None) => "unknown".to_owned(),
            }
        )));

        // 4. Quota reset: the provider's own clock, on its own line.
        lines.push(Line::from(format!(
            "    quota reset            {}",
            match row.quota_resets_at_unix {
                Some(at) => format!("{} (unix {at})", describe_deadline(now, at)),
                None => "unknown — no response has stated one".to_owned(),
            }
        )));

        // 5. Failure-domain evidence, in `FailureDomain`'s own vocabulary,
        //    and never claiming independence.
        lines.push(Line::from(format!(
            "    failure domain         {} — {} other observed resource(s) on `{}`; a different \
             provider is `unknown`, never `independent`",
            row.failure_domain, row.failure_domain_peers, row.provider,
        )));
    }

    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
}

/// `yes` or `no`, so a boolean fact reads as one rather than as `true`.
fn yes_no(value: bool) -> &'static str {
    if value { "yes" } else { "no" }
}

/// A future instant in words — [`describe_age`]'s forward twin.
///
/// Separate from `describe_age` rather than folded into it: that function
/// treats a timestamp ahead of `now` as a **clock fault**, which is right for
/// a reading that was supposedly observed in the past and wrong for a deadline
/// that is supposed to be ahead. One function answering both would have to
/// guess which it was holding.
fn describe_deadline(now: i64, at: i64) -> String {
    let seconds = at.saturating_sub(now);
    if seconds <= 0 {
        return "already elapsed".to_owned();
    }
    let (count, unit) = match seconds {
        1..=59 => (seconds, "second"),
        60..=3_599 => (seconds / 60, "minute"),
        3_600..=86_399 => (seconds / 3_600, "hour"),
        _ => (seconds / 86_400, "day"),
    };
    let plural = if count == 1 { "" } else { "s" };
    format!("in {count} {unit}{plural}")
}

/// How many recently completed workers the project overview shows.
///
/// Not the same bound as [`super::state::ACTIVITY_ROWS`]: that list is every
/// kind of lifecycle event across every session, this is worker sessions
/// only, so the two are free to differ without either meaning the other is
/// wrong.
const RECENTLY_COMPLETED_ROWS: usize = 5;

fn push_worker_section(
    lines: &mut Vec<Line<'static>>,
    title: &'static str,
    sessions: &[&&SessionRecord],
    empty_note: &'static str,
) {
    lines.push(Line::from(Span::styled(
        title,
        Style::default().add_modifier(Modifier::BOLD),
    )));
    if sessions.is_empty() {
        lines.push(Line::from(Span::styled(
            format!("  {empty_note}"),
            Style::default().fg(Color::DarkGray),
        )));
        return;
    }
    let now = crate::provider::cache::now_unix_seconds();
    for session in sessions {
        lines.push(Line::from(format!(
            "  {}  {}  {}",
            short_id(session),
            describe_age(now, session.last_activity_at),
            session_detail(session)
        )));
    }
}

/// Map line 1662's harness, backend, model, pairing class and response
/// profile, one session at a time — the detail [`render_overview`]'s table
/// has no room for because it is one line per session across every session
/// in the project, not one block per session in a project-level summary.
fn session_detail(session: &SessionRecord) -> String {
    let backend = session.backend_resource.as_deref().unwrap_or("unresolved");
    let model = session
        .model
        .as_ref()
        .map_or("unresolved".to_owned(), |m| m.label().to_owned());
    format!(
        "harness={} backend={} model={} pairing={} profile={}",
        session.harness,
        backend,
        model,
        pairing_label(session.pairing_class),
        response_profile_label(session.response_profile.as_ref())
    )
}

fn pairing_label(pairing: Option<SessionPairingClass>) -> &'static str {
    match pairing {
        None => "not recorded",
        Some(SessionPairingClass::VendorNative) => "vendor-native",
        Some(SessionPairingClass::VendorSupported) => "vendor-supported",
        Some(SessionPairingClass::ProtocolNative) => "protocol-native",
        Some(SessionPairingClass::ProtocolCompatible) => "protocol-compatible",
        Some(SessionPairingClass::ProtocolTranslated) => "protocol-translated",
        Some(SessionPairingClass::Unknown) => "unknown (recorded)",
    }
}

fn response_profile_label(profile: Option<&crate::profile::response::ResponseProfile>) -> String {
    match profile {
        None => "not recorded".to_owned(),
        Some(profile) => format!(
            "{:?}/{:?}/{:?}/{:?}/{:?}",
            profile.verbosity(),
            profile.audience(),
            profile.narration(),
            profile.evidence(),
            profile.format()
        ),
    }
}

fn disposition_label(session: &SessionRecord) -> &'static str {
    match session.disposition() {
        SessionDisposition::Active => "active",
        SessionDisposition::Resumable => "resumable",
        SessionDisposition::Closed => "closed",
        SessionDisposition::Failed => "failed",
    }
}

/// One word for each of [`crate::session::SessionLifecycle`]'s seven values.
///
/// The map's line 683 asks for *"the current lifecycle state"*, and
/// [`disposition_label`] answers a different, coarser question — see
/// [`state_label`]'s doc comment. This is the finer one, kept as its own
/// function rather than folded into `state_label` so a reader can see the
/// seven-way match is exhaustive at a glance.
fn lifecycle_word(lifecycle: SessionLifecycle) -> &'static str {
    match lifecycle {
        SessionLifecycle::Starting => "starting",
        SessionLifecycle::Running => "running",
        SessionLifecycle::Idle => "idle",
        // Shortened from the stored `waiting_for_user`: this is a column in a
        // fixed-width table, not the stored word, and the parenthesised
        // disposition beside it already says whose session this is.
        SessionLifecycle::WaitingForUser => "waiting",
        SessionLifecycle::Stopped => "stopped",
        SessionLifecycle::Failed => "failed",
        SessionLifecycle::Closed => "closed",
    }
}

/// The STATE column's full text: the disposition a resume or an interrupt
/// acts on, and — for the two dispositions that do not already pin it down —
/// the finer lifecycle state the map's line 683 actually asks for.
///
/// Two answers, not one, because they are two different questions. `active`
/// alone cannot distinguish a session waiting on the user from one mid-turn,
/// and `disposition()`'s own doc comment explains why collapsing that
/// distinction is deliberate for the *resumable/interrupt* question — but it
/// means a column that shows only the disposition is not "the current
/// lifecycle state" the box asks for. Keeping both rather than replacing one
/// with the other answers both questions honestly instead of picking a
/// winner.
///
/// `Resumable` and `Failed` are skipped here, not because they are less
/// important, but because `disposition()`'s own match makes each of them
/// answer to exactly one [`SessionLifecycle`] — `Resumable` only ever means
/// `Stopped` with a native identifier, `Failed` only ever means `Failed` —
/// so appending the lifecycle word would repeat the disposition rather than
/// add to it. `Active` (four lifecycles) and `Closed` (`Stopped` with no
/// native identifier, or `Closed` itself) are genuinely ambiguous without it.
fn state_label(session: &SessionRecord) -> String {
    match session.disposition() {
        SessionDisposition::Active | SessionDisposition::Closed => {
            format!(
                "{}/{}",
                disposition_label(session),
                lifecycle_word(session.lifecycle)
            )
        }
        SessionDisposition::Resumable | SessionDisposition::Failed => {
            disposition_label(session).to_owned()
        }
    }
}

/// The NAME column: what a person called this session, or what they tagged
/// it with, or an explicit sentinel — never a blank cell.
///
/// The map's line 682 asks for "the user-assigned session name or purpose",
/// one fact with two possible sources, so this is one column rather than two:
/// a session with both shows the name first with the purpose alongside for
/// context, and a session with neither says so in words. An empty cell would
/// be indistinguishable from a name that happened to render as nothing, or
/// from a column truncated away — `"(unnamed)"` cannot be confused with
/// either.
fn name_or_purpose(session: &SessionRecord) -> String {
    match (&session.display_name, &session.purpose) {
        (Some(name), Some(purpose)) => format!("{name} ({purpose})"),
        (Some(name), None) => name.to_string(),
        (None, Some(purpose)) => purpose.to_string(),
        (None, None) => "(unnamed)".to_owned(),
    }
}

/// The abbreviated identifier a session is shown and referred to by.
///
/// Delegates to `state::short_session_id` rather than truncating here: every
/// status note that names a session uses the same function, so a refusal can
/// be matched by eye to the row it is about.
fn short_id(session: &SessionRecord) -> String {
    super::state::short_session_id(&session.id)
}

/// The Settings overlay: Harnesses and Integrations, drawn over the shell
/// exactly like the Overview — see its doc comment for why "over", not
/// "instead of".
///
/// Unlike the Overview, this overlay owns every key while open (see
/// `ShellState::handle_settings_key`), so nothing underneath it is reachable
/// while it is shown — there is no passthrough navigation to account for
/// here.
fn render_settings(state: &ShellState, frame: &mut Frame, area: Rect) {
    let Some(settings) = state.settings() else {
        return;
    };
    let popup = centered(area, 90, 80);
    frame.render_widget(Clear, popup);

    let block = Block::default()
        .borders(Borders::ALL)
        .title(" settings ")
        .style(Style::default().bg(Color::Black));
    let inner = block.inner(popup);
    frame.render_widget(block, popup);

    // An active input always outranks the passive reachability-check
    // banner: `SettingsState::handle_key` clears that banner on every key
    // its general dispatcher handles (which is how a wizard ever gets to
    // open in the first place), but checking `provider_input`/
    // `profile_input` first here too means this priority holds regardless
    // of that clearing — two independent reasons the banner never shadows
    // an editor, not one relying on the other.
    let bottom_lines: Vec<Line> = if let Some(input) = settings.path_input() {
        settings_path_input_lines(&input)
    } else if let Some(provider) = settings.confirming_credential_delete() {
        credential_delete_confirm_lines(provider)
    } else if settings.confirming_project_write() {
        project_write_confirm_lines(state)
    } else if let Some(input) = settings.provider_input() {
        // `input.buffer` is already masked for a credential field — see
        // `SettingsState::provider_input`. Nothing here decides that, which
        // is the point: this renderer never sees a typed credential.
        labeled_text_input_lines(&input.label, &input.buffer, input.error)
    } else if let Some(input) = settings.profile_input() {
        labeled_text_input_lines(&input.label, input.buffer, input.error)
    } else if let Some(input) = settings.routing_input() {
        labeled_text_input_lines(input.label, input.buffer, input.error)
    } else if let Some((name, outcome)) = settings.provider_test_result() {
        provider_test_result_lines(name, outcome)
    } else if let Some((name, refresh)) = settings.provider_models_result() {
        provider_models_result_lines(name, refresh)
    } else {
        Vec::new()
    };

    // Measured, not guessed: a fixed constant here is exactly what let an
    // earlier version of this panel silently clip its own error line
    // whenever the label above it (the Providers/Launch-Profiles inputs can
    // run to a long, name-enumerating prompt) wrapped onto a second line by
    // itself, leaving no room left for the one after it. `wrapped_height`
    // wraps the same way the `Paragraph` below actually renders (`Wrap {
    // trim: false }`), so the height asked for and the height used never
    // drift apart. Capped to the panel's own available height so an
    // absurdly narrow terminal shrinks the list above rather than
    // requesting more room than `inner` has to give.
    let bottom_height = wrapped_height(&bottom_lines, inner.width).min(inner.height);

    let regions = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Min(0),
            Constraint::Length(bottom_height),
        ])
        .split(inner);
    let tabs_area = regions[0];
    let list_area = regions[1];
    let bottom_area = regions[2];

    render_settings_tabs(settings, frame, tabs_area);
    match settings.section() {
        SettingsSection::Harnesses => render_harness_rows(settings, frame, list_area),
        SettingsSection::Integrations => render_integration_rows(settings, frame, list_area),
        SettingsSection::Providers => render_provider_rows(settings, frame, list_area),
        SettingsSection::LaunchProfiles => render_profile_rows(settings, frame, list_area),
        SettingsSection::Routing => render_routing(settings, frame, list_area),
        SettingsSection::Memory => render_memory(settings, frame, list_area),
    }

    if !bottom_lines.is_empty() {
        frame.render_widget(
            Paragraph::new(bottom_lines).wrap(Wrap { trim: false }),
            bottom_area,
        );
    }
}

fn render_settings_tabs(settings: &SettingsState, frame: &mut Frame, area: Rect) {
    let profiles_label = if area.width >= 75 {
        "Launch Profiles"
    } else {
        "Profiles"
    };
    let tab = |label: &str, active: bool| {
        let style = if active {
            Style::default()
                .fg(Color::Black)
                .bg(Color::Cyan)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::Gray)
        };
        Span::styled(format!(" {label} "), style)
    };
    let spans = vec![
        tab(
            "Harnesses",
            settings.section() == SettingsSection::Harnesses,
        ),
        Span::raw(" "),
        tab(
            "Integrations",
            settings.section() == SettingsSection::Integrations,
        ),
        Span::raw(" "),
        tab(
            "Providers",
            settings.section() == SettingsSection::Providers,
        ),
        Span::raw(" "),
        tab(
            profiles_label,
            settings.section() == SettingsSection::LaunchProfiles,
        ),
        Span::raw(" "),
        tab("Routing", settings.section() == SettingsSection::Routing),
        Span::raw(" "),
        tab("Memory", settings.section() == SettingsSection::Memory),
    ];
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

/// Text for a value's provenance — the design decision's "provenance is
/// shown, not inferred".
fn layer_label(layer: Layer) -> &'static str {
    match layer {
        Layer::Project => "(project)",
        Layer::User => "(user)",
        Layer::Default => "(default)",
    }
}

fn render_harness_rows(settings: &SettingsState, frame: &mut Frame, area: Rect) {
    let mut lines = Vec::new();
    if settings.harnesses().is_empty() {
        lines.push(Line::from(Span::styled(
            "No harnesses known.",
            Style::default().fg(Color::DarkGray),
        )));
    }
    for (index, row) in settings.harnesses().iter().enumerate() {
        let selected = index == settings.selected_harness();
        let cursor = if selected { "> " } else { "  " };
        let mut style = Style::default();
        if selected {
            style = style.add_modifier(Modifier::BOLD);
        }
        let detected = if row.detected {
            "detected"
        } else {
            "not detected"
        };
        let enabled = format!(
            "{} {}",
            if row.enabled { "enabled" } else { "disabled" },
            layer_label(row.enabled_layer)
        );
        let path = match (&row.executable, row.executable_layer) {
            (Some(path), Some(layer)) => format!("{} {}", path.display(), layer_label(layer)),
            _ => "no explicit path".to_owned(),
        };
        lines.push(Line::from(Span::styled(
            format!(
                "{cursor}{:<14} {detected:<13} {enabled:<22} {path}",
                row.id.display_name(),
            ),
            style,
        )));
    }
    frame.render_widget(Paragraph::new(lines), area);
}

fn render_integration_rows(settings: &SettingsState, frame: &mut Frame, area: Rect) {
    let mut lines = Vec::new();
    if settings.integrations().is_empty() {
        lines.push(Line::from(Span::styled(
            "No optional integrations known.",
            Style::default().fg(Color::DarkGray),
        )));
    }
    for (index, row) in settings.integrations().iter().enumerate() {
        let selected = index == settings.selected_integration();
        let cursor = if selected { "> " } else { "  " };
        let mut style = Style::default();
        if selected {
            style = style.add_modifier(Modifier::BOLD);
        }
        let detected = if row.detected {
            "detected"
        } else {
            "not detected"
        };
        lines.push(Line::from(Span::styled(
            format!(
                "{cursor}{:<14} {detected:<13} {}",
                row.id.display_name(),
                row.status
            ),
            style,
        )));
    }
    frame.render_widget(Paragraph::new(lines), area);
}

/// The Settings "Providers" section. Every credential status shown is
/// `set`/`not set` only, read with [`std::env::var_os`] — never
/// [`std::env::var`], and never the value itself. See the module-level rule
/// this mirrors in `integrations::write_provider_report`.
fn render_provider_rows(settings: &SettingsState, frame: &mut Frame, area: Rect) {
    // Read once per frame rather than once per row, so every row on one
    // screen describes its age against the same instant.
    let now = crate::provider::cache::now_unix_seconds();
    let mut lines = Vec::new();
    if settings.providers().is_empty() {
        lines.push(Line::from(Span::styled(
            "No providers configured.",
            Style::default().fg(Color::DarkGray),
        )));
    }
    for (index, row) in settings.providers().iter().enumerate() {
        let selected = index == settings.selected_provider();
        let cursor = if selected { "> " } else { "  " };
        let mut style = Style::default();
        if selected {
            style = style.add_modifier(Modifier::BOLD);
        }

        let enabled = format!(
            "{} {}",
            if row.config.enabled() {
                "enabled"
            } else {
                "disabled"
            },
            layer_label(row.layer),
        );

        let (base_url, credential) = match row.config.to_provider(&row.name) {
            Ok(provider) => {
                let base_url = provider
                    .protocols
                    .first()
                    .map(|p| p.base_url.as_str())
                    .filter(|url| !url.is_empty())
                    .unwrap_or("(no base URL)")
                    .to_owned();
                let credential = if provider.credential_env.is_empty() {
                    "no credential variable".to_owned()
                } else {
                    provider
                        .credential_env
                        .iter()
                        .map(|var| {
                            // `var_os` only: presence must never decode a value.
                            // Worded as "in the environment" rather than the
                            // bare "set"/"not set" this said before there was
                            // a second place a credential could be: a row
                            // reading "(not set) — stored in the OS secure
                            // store" is a contradiction on its face, and was
                            // one until running the real binary showed it.
                            let set = std::env::var_os(var).is_some();
                            let where_ = if set {
                                "in the environment"
                            } else {
                                "not in the environment"
                            };
                            format!("{var} ({where_})")
                        })
                        .collect::<Vec<_>>()
                        .join(", ")
                };
                // Line 2 at the row level: read from the provider's own
                // configuration, never by asking the keychain. A row is
                // redrawn on every frame, and a keychain round trip per
                // provider per frame would be both slow and — on macOS,
                // where reading an item can consult its access control list
                // — a way to make the TUI ask for permission while the user
                // is scrolling.
                let credential = match row.config.credential_store() {
                    Some(stored) => format!(
                        "{credential} — stored in the OS secure store as {}/{}",
                        stored.service(),
                        stored.account()
                    ),
                    None => credential,
                };
                (base_url, credential)
            }
            Err(err) => (format!("invalid template: {err}"), "-".to_owned()),
        };

        lines.push(Line::from(Span::styled(
            format!(
                "{cursor}{:<14} {:<14} {enabled:<16} {base_url}",
                row.name,
                row.config.template(),
            ),
            style,
        )));
        lines.push(Line::from(Span::styled(
            format!("      credential: {credential}"),
            Style::default().fg(Color::DarkGray),
        )));
        lines.push(provider_models_line(row, now));
        lines.push(Line::from(Span::styled(
            format!(
                "      free-tier models: {}",
                if row.config.free_models().is_empty() {
                    "none marked".to_owned()
                } else {
                    row.config.free_models().join(", ")
                }
            ),
            Style::default().fg(Color::DarkGray),
        )));
    }
    frame.render_widget(Paragraph::new(lines), area);
}

/// One provider row's model-list line: what is cached, and **when it was
/// fetched**.
///
/// # The timestamp is not optional
///
/// Phase 9D line 3 says "with a timestamp", and this is the only place a user
/// ever sees one. A cached list rendered without its age is the exact failure
/// the line exists to prevent: four hundred models from three weeks ago look
/// identical to four hundred from three seconds ago, and a user picking a
/// model override from the stale one gets a name the provider has since
/// retired. So both forms are shown — the absolute instant, which is what you
/// quote, and the age, which is what you act on.
///
/// A request in flight outranks everything else here. It is on the row rather
/// than in the banner below precisely so that scrolling the list cannot make
/// a running request invisible — see [`ProviderRow::activity`].
fn provider_models_line(row: &ProviderRow, now: i64) -> Line<'static> {
    if let Some(kind) = row.activity {
        let what = match kind {
            ProbeKind::Connectivity => "testing connectivity",
            ProbeKind::ModelRefresh => "refreshing the model list",
        };
        return Line::from(Span::styled(
            format!("      models: {what}..."),
            Style::default().fg(Color::Cyan),
        ));
    }

    // **Found running the real binary.** A provider with no established
    // model-list endpoint used to render "none cached — press m to fetch",
    // which advertises a key that cannot ever fetch anything for it. The
    // design decision this batch works to forbids "a silently disabled
    // control with no explanation"; a control that is loudly *advertised*
    // and then refuses is worse, because the user presses it first.
    let offers_discovery = row
        .config
        .to_provider(&row.name)
        .is_ok_and(|provider| provider.model_list_endpoint.is_known_present());

    let Some(models) = &row.models else {
        return Line::from(Span::styled(
            if offers_discovery {
                "      models: none cached — press m to fetch".to_owned()
            } else {
                "      models: no model-discovery endpoint established for this provider".to_owned()
            },
            Style::default().fg(Color::DarkGray),
        ));
    };

    // The base URL the row would use now, against the one the catalogue was
    // fetched from. A user who repointed a provider at a different service is
    // looking at another service's models, and saying so is the difference
    // between a stale cache and a wrong one.
    let current_base_url = row
        .config
        .to_provider(&row.name)
        .ok()
        .and_then(|provider| provider.protocols.first().map(|p| p.base_url.clone()))
        .unwrap_or_default();
    let moved = !models.was_fetched_from(&current_base_url);

    let text = format!(
        "      models: {} cached, fetched {} ({}){}",
        models.len(),
        format_unix_utc(models.fetched_at()),
        describe_age(now, models.fetched_at()),
        if moved {
            format!(
                " — from {}, which is no longer this provider's base URL",
                models.base_url()
            )
        } else {
            String::new()
        }
    );
    Line::from(Span::styled(
        text,
        Style::default().fg(if moved {
            Color::Yellow
        } else {
            Color::DarkGray
        }),
    ))
}

/// The Settings "Launch Profiles" section.
fn render_profile_rows(settings: &SettingsState, frame: &mut Frame, area: Rect) {
    let mut lines = Vec::new();
    if settings.profiles().is_empty() {
        lines.push(Line::from(Span::styled(
            "No launch profiles configured.",
            Style::default().fg(Color::DarkGray),
        )));
    }
    for (index, row) in settings.profiles().iter().enumerate() {
        let selected = index == settings.selected_profile();
        let cursor = if selected { "> " } else { "  " };
        let mut style = Style::default();
        if selected {
            style = style.add_modifier(Modifier::BOLD);
        }

        let backend = match row.config.backend() {
            crate::config::ProfileBackend::Native => "native".to_owned(),
            crate::config::ProfileBackend::DirectProvider { provider } => {
                format!("provider:{provider}")
            }
            crate::config::ProfileBackend::GlasshouseGateway => "gateway".to_owned(),
        };
        let model = row.config.model().unwrap_or("(default)");
        let approval = match row.config.approval() {
            crate::config::ProfileApproval::Default => "default",
            crate::config::ProfileApproval::AutomaticReview => "automatic-review",
            crate::config::ProfileApproval::Bypass => "bypass",
        };
        let enabled = format!(
            "{} {}",
            if row.config.enabled() {
                "enabled"
            } else {
                "disabled"
            },
            layer_label(row.layer),
        );

        lines.push(Line::from(Span::styled(
            format!(
                "{cursor}{:<14} {:<11} {backend:<16} {model:<14} {approval:<16} {enabled}",
                row.name,
                row.config.harness_slug(),
            ),
            style,
        )));
    }
    frame.render_widget(Paragraph::new(lines), area);
}

fn render_routing(settings: &SettingsState, frame: &mut Frame, area: Rect) {
    let routing = settings.routing();
    let model = match &routing.model {
        RoutingModelChoice::Automatic => "automatic".to_owned(),
        RoutingModelChoice::Deterministic => "deterministic heuristics".to_owned(),
        RoutingModelChoice::Pinned { provider, model } => format!("{provider}:{model}"),
    };
    let prefer_free = if routing.prefer_free { "yes" } else { "no" };
    let mut lines = vec![
        Line::from(format!(
            "  Routing model             {model} {}",
            layer_label(routing.model_layer)
        )),
        Line::from(format!(
            "  Maximum router latency    {} ms {}",
            routing.max_latency.get(),
            layer_label(routing.max_latency_layer)
        )),
        Line::from(format!(
            "  Maximum marginal cost     ${} per decision {}",
            format_usd(routing.max_cost),
            layer_label(routing.max_cost_layer)
        )),
        Line::from(format!(
            "  Prefer free resources     {prefer_free} {}",
            layer_label(routing.prefer_free_layer)
        )),
        Line::from(Span::styled(
            "    Applied only after capability, health, rate-limit, and latency checks pass.",
            Style::default().fg(Color::DarkGray),
        )),
        Line::from(format!(
            "  Protect premium capacity  below {}% remaining {}",
            routing.premium_reserve.get(),
            layer_label(routing.premium_reserve_layer)
        )),
        Line::from(""),
        Line::from(format!(
            "  Free resource order      {} {}",
            free_resource_list_label(&routing.free_order),
            layer_label(routing.free_order_layer)
        )),
        Line::from(format!(
            "  Disabled free resources  {} {}",
            free_resource_list_label(&routing.free_disabled),
            layer_label(routing.free_disabled_layer)
        )),
        Line::from(format!(
            "  Pinned free resource     {} {}",
            routing
                .free_pin
                .as_ref()
                .map(|pin| format!("{}:{}", pin.provider(), pin.model()))
                .unwrap_or_else(|| "(none)".to_owned()),
            layer_label(routing.free_pin_layer)
        )),
    ];
    if let Some(choice) = settings.last_disposable_choice() {
        lines.push(Line::from(format!(
            "  Free resource in use     {} on {} — {}",
            choice.model(),
            choice.provider(),
            choice.reason()
        )));
    }
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "  m model   l latency   c cost   f prefer-free   p premium reserve   o free order   \
         d disabled   n pin",
        Style::default().fg(Color::DarkGray),
    )));
    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), area);
}

/// A comma-separated `provider:model` list for the Routing section, or a
/// plain sentence when there is nothing to show — matching
/// [`provider_models_line`]'s "none cached" phrasing for the same reason: an
/// empty field reads as missing data, not as a deliberate empty list.
fn free_resource_list_label(entries: &[crate::config::FreeResourceRef]) -> String {
    if entries.is_empty() {
        "(none)".to_owned()
    } else {
        entries
            .iter()
            .map(|entry| format!("{}:{}", entry.provider(), entry.model()))
            .collect::<Vec<_>>()
            .join(", ")
    }
}

fn render_memory(settings: &SettingsState, frame: &mut Frame, area: Rect) {
    let memory = settings.memory();
    let enabled = if memory.memory_extraction {
        "yes"
    } else {
        "no"
    };
    let lines = vec![
        Line::from(format!(
            "  Automatic memory extraction   {enabled} {}",
            layer_label(memory.memory_extraction_layer)
        )),
        Line::from(Span::styled(
            "    Extracts durable facts from a completed turn into project or user memory.",
            Style::default().fg(Color::DarkGray),
        )),
        Line::from(""),
        Line::from(Span::styled(
            "  space toggle",
            Style::default().fg(Color::DarkGray),
        )),
    ];
    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), area);
}

/// The `x` confirmation. Names the provider and says plainly that this one
/// is not undone by declining to save, because it is the only action in the
/// Settings overlay that reaches outside Glasshouse's own configuration.
fn credential_delete_confirm_lines(provider: &str) -> Vec<Line<'static>> {
    vec![
        Line::from(Span::styled(
            format!(
                "Delete `{provider}`'s credential from the OS secure store? \
                 This cannot be undone by not saving."
            ),
            Style::default().fg(Color::Yellow),
        )),
        Line::from(Span::styled(
            "y/enter delete   esc/n cancel",
            Style::default().fg(Color::DarkGray),
        )),
    ]
}

/// A single labeled text field with an optional error line, shared by every
/// Providers/Launch-Profiles inline input — see
/// [`crate::shell::state::ProviderInputView`] and
/// [`crate::shell::state::ProfileInputView`]. Returns lines rather than
/// rendering directly, so [`render_settings`] can measure the wrapped height
/// this content actually needs before it decides how tall to make the panel
/// — see that function's own comment on why a fixed guess is not enough.
fn labeled_text_input_lines(label: &str, buffer: &str, error: Option<&str>) -> Vec<Line<'static>> {
    let mut lines = vec![Line::from(format!("{label}: {buffer}_"))];
    if let Some(error) = error {
        lines.push(Line::from(Span::styled(
            error.to_owned(),
            Style::default().fg(Color::Red),
        )));
    }
    lines
}

/// One [`ProbeOutcome`], as a sentence.
///
/// `pub(crate)` because the run loop composes a failed model refresh out of
/// the same words: a `401` should read identically whether the user pressed
/// `t` or `m`, and two independent phrasings of the same outcome would
/// eventually disagree.
///
/// **A rejected credential and an unreachable host get different sentences**,
/// which is the distinction Phase 9D line 1 exists to draw. They are
/// different problems with different fixes, and a user told only "failed" has
/// to guess which they have.
pub(crate) fn describe_probe_outcome(outcome: &ProbeOutcome) -> String {
    match outcome {
        ProbeOutcome::Reached { status } => format!("answered {status}"),
        ProbeOutcome::Rejected { status } => {
            format!("answered {status} — reachable, but it did not accept the credential")
        }
        ProbeOutcome::Unexpected { status } => {
            format!("answered {status}, which is not a success and not a rejection")
        }
        ProbeOutcome::TimedOut { waited_ms } => format!(
            "no answer within {waited_ms}ms — the connection was accepted but nothing came back"
        ),
        ProbeOutcome::Unreachable { reason } => format!("unreachable — {reason}"),
    }
}

/// The colour an outcome earns.
///
/// A rejection is amber rather than red on purpose: the provider is *there*,
/// which is most of what the user wanted to find out, and the remaining
/// problem is one credential away from fixed.
fn probe_outcome_color(outcome: &ProbeOutcome) -> Color {
    match outcome {
        ProbeOutcome::Reached { .. } => Color::Green,
        ProbeOutcome::Rejected { .. } | ProbeOutcome::Unexpected { .. } => Color::Yellow,
        ProbeOutcome::TimedOut { .. } | ProbeOutcome::Unreachable { .. } => Color::Red,
    }
}

/// Phase 9D line 1's connectivity result.
///
/// This used to carry a disclaimer saying Glasshouse had no HTTP client and
/// that nothing had really been requested. It has one now, the request is
/// real, and the disclaimer is gone — its absence is asserted by
/// `the_connectivity_result_names_what_it_reached_and_carries_no_disclaimer`,
/// because a line that apologises for a check it is no longer failing to make
/// is worse than no line.
fn provider_test_result_lines(
    name: &str,
    outcome: &crate::shell::state::ReachabilityCheck,
) -> Vec<Line<'static>> {
    use crate::shell::state::ReachabilityCheck;

    match outcome {
        ReachabilityCheck::InFlight {
            protocol,
            base_url,
            endpoint,
        } => vec![
            Line::from(Span::styled(
                format!("`{name}`: testing {protocol} at {base_url} — request in flight..."),
                Style::default().fg(Color::Cyan),
            )),
            Line::from(Span::styled(
                format!("GET {endpoint} — the interface stays usable while this runs."),
                Style::default().fg(Color::DarkGray),
            )),
        ],
        ReachabilityCheck::Answered {
            protocol,
            base_url,
            endpoint,
            outcome,
        } => vec![
            Line::from(Span::styled(
                format!(
                    // The verb follows the outcome, and this is not
                    // cosmetic. **Found running the real binary**: with
                    // "reached" hard-coded, a refused connection rendered as
                    // "`dead-host`: reached openai-chat at ... — GET ...
                    // unreachable — the connection was refused", which
                    // contradicts itself inside one sentence. It is the same
                    // shape of defect as the "(not set) — stored in the OS
                    // secure store" row an earlier batch found the same way,
                    // and `ProbeOutcome::answered` already carries exactly
                    // the distinction the wording needs.
                    "`{name}`: {} {protocol} at {base_url} — GET {endpoint} {}",
                    if outcome.answered() {
                        "reached"
                    } else {
                        "could not reach"
                    },
                    describe_probe_outcome(outcome)
                ),
                Style::default().fg(probe_outcome_color(outcome)),
            )),
            Line::from(Span::styled(
                "Testing does not enable or disable anything — that stays your decision."
                    .to_owned(),
                Style::default().fg(Color::DarkGray),
            )),
        ],
        ReachabilityCheck::Failed(reason) => vec![Line::from(Span::styled(
            format!("`{name}`: nothing was requested — {reason}"),
            Style::default().fg(Color::Red),
        ))],
    }
}

/// Phase 9D line 2's manual model refresh.
fn provider_models_result_lines(
    name: &str,
    refresh: &crate::shell::state::ModelRefresh,
) -> Vec<Line<'static>> {
    use crate::shell::state::ModelRefresh;

    match refresh {
        ModelRefresh::InFlight { endpoint } => vec![
            Line::from(Span::styled(
                format!("`{name}`: refreshing the model list — request in flight..."),
                Style::default().fg(Color::Cyan),
            )),
            Line::from(Span::styled(
                format!("GET {endpoint} — the interface stays usable while this runs."),
                Style::default().fg(Color::DarkGray),
            )),
        ],
        ModelRefresh::Refreshed {
            count,
            fetched_at,
            endpoint,
        } => vec![Line::from(Span::styled(
            format!(
                "`{name}`: {count} models from GET {endpoint}, cached at {}",
                format_unix_utc(*fetched_at)
            ),
            Style::default().fg(Color::Green),
        ))],
        // Grey and plainly worded, not red: a provider that offers no model
        // discovery is not a failure, and colouring it like one would send
        // the user looking for a problem they do not have.
        ModelRefresh::NotOffered(reason) => vec![Line::from(Span::styled(
            format!("`{name}`: {reason}"),
            Style::default().fg(Color::DarkGray),
        ))],
        ModelRefresh::Failed(reason) => vec![Line::from(Span::styled(
            format!("`{name}`: could not refresh the model list — {reason}"),
            Style::default().fg(Color::Red),
        ))],
    }
}

/// A Unix timestamp as `YYYY-MM-DD HH:MM:SSZ`.
///
/// Hand-rolled because this crate has no date library and adding one to print
/// one line would be a poor trade. The civil-from-days conversion is Howard
/// Hinnant's, which is exact for every day in the proleptic Gregorian
/// calendar and is the same algorithm every date library uses underneath.
///
/// UTC, never local time. A cache timestamp is compared against another
/// machine's, quoted into a bug report, and read months later; a local time
/// with no zone on it is ambiguous in all three situations.
fn format_unix_utc(seconds: i64) -> String {
    let days = seconds.div_euclid(86_400);
    let time_of_day = seconds.rem_euclid(86_400);
    let (hour, minute, second) = (
        time_of_day / 3_600,
        (time_of_day % 3_600) / 60,
        time_of_day % 60,
    );

    // Howard Hinnant's `civil_from_days`, with the era shifted so the
    // arithmetic is correct for dates before 1970 as well as after.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let day_of_era = z.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let mp = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if month <= 2 { year + 1 } else { year };

    format!("{year:04}-{month:02}-{day:02} {hour:02}:{minute:02}:{second:02}Z")
}

/// How long ago `then` was, from `now`, in words.
///
/// The absolute timestamp says *when*; this says *how stale*, which is the
/// question a user actually has. "2026-08-26 09:31:04Z" requires arithmetic
/// to act on and "17 days ago" does not, so both are shown and this is the
/// one that carries the warning.
fn describe_age(now: i64, then: i64) -> String {
    let seconds = now.saturating_sub(then);
    if seconds < 0 {
        // A cache stamped in the future means a clock moved, on this machine
        // or the one that wrote it. Saying so is better than rendering a
        // negative age or silently clamping it to "just now".
        return "timestamped in the future — check this machine's clock".to_owned();
    }
    let (count, unit) = match seconds {
        0..=59 => return "just now".to_owned(),
        60..=3_599 => (seconds / 60, "minute"),
        3_600..=86_399 => (seconds / 3_600, "hour"),
        86_400..=2_591_999 => (seconds / 86_400, "day"),
        _ => (seconds / 2_592_000, "month"),
    };
    let plural = if count == 1 { "" } else { "s" };
    format!("{count} {unit}{plural} ago")
}

fn settings_path_input_lines(input: &SettingsPathInputView<'_>) -> Vec<Line<'static>> {
    let mut lines = vec![Line::from(format!(
        "Path to {} executable: {}_",
        input.harness_name, input.buffer
    ))];
    if let Some(error) = input.error {
        lines.push(Line::from(Span::styled(
            error.to_owned(),
            Style::default().fg(Color::Red),
        )));
    }
    lines
}

/// The design decision requires naming the exact path before a project-level
/// write, so this is not a generic "are you sure": it spells out
/// `<project root>/.glasshouse/config.toml` and says the file lands inside
/// the repository.
fn project_write_confirm_lines(state: &ShellState) -> Vec<Line<'static>> {
    let path = state.project_root().join(".glasshouse").join("config.toml");
    vec![
        Line::from(Span::styled(
            format!("Write project settings to {}?", path.display()),
            Style::default().add_modifier(Modifier::BOLD),
        )),
        Line::from("This file is inside your project repository. y/Enter confirms, Esc/n cancels."),
    ]
}

/// Total rows `lines` need once word-wrapped to `width` columns, the same
/// way [`Wrap`] `{ trim: false }` wraps them when actually drawn — see
/// [`render_settings`]'s comment on why this has to match rather than guess.
fn wrapped_height(lines: &[Line], width: u16) -> u16 {
    lines
        .iter()
        .map(|line| wrapped_row_count(&String::from(line.clone()), width))
        .sum::<usize>()
        .min(u16::MAX as usize) as u16
}

/// Rows `text` needs word-wrapped to `width` columns: whole words packed
/// onto a row, a new row started only when the next word would not fit —
/// the same rule [`ratatui`]'s own word-wrapper follows for `Wrap { trim:
/// false }`.
///
/// Not [`ratatui::widgets::Paragraph::line_count`] — that method carries an
/// upstream `instability::unstable` marker, which makes it private outside
/// its own crate without a feature flag this workspace has no path to
/// enabling (Cargo manifests are a forbidden file for this packet). This is
/// this module's own small stand-in, used only to size a panel this module
/// already renders with that exact wrap setting — never to change what is
/// actually drawn.
///
/// A single word wider than `width` still costs only one row here, never a
/// loop: nothing this module ever wraps contains a word anywhere near a
/// realistic terminal's width, and undercounting by a row on an
/// unrealistically narrow terminal is the same accepted degradation the
/// rest of this file's absurd-size tests already tolerate — clipped
/// content, never a panic.
fn wrapped_row_count(text: &str, width: u16) -> usize {
    let width = usize::from(width.max(1));
    let mut rows: usize = 1;
    let mut current: usize = 0;
    for word in text.split(' ') {
        let word_len = word.chars().count();
        if current == 0 {
            current = word_len;
            continue;
        }
        let needed = current + 1 + word_len;
        if needed <= width {
            current = needed;
        } else {
            rows += 1;
            current = word_len;
        }
    }
    rows
}

/// A rectangle covering `percent_x` by `percent_y` of `area`, centred.
///
/// Computed by multiplication and division rather than by subtracting a fixed
/// margin, so a terminal smaller than any assumed minimum simply produces a
/// small rectangle instead of an underflow.
fn centered(area: Rect, percent_x: u16, percent_y: u16) -> Rect {
    let width = area.width * percent_x / 100;
    let height = area.height * percent_y / 100;
    Rect {
        x: area.x + (area.width - width) / 2,
        y: area.y + (area.height - height) / 2,
        width,
        height,
    }
}

/// Keep the last `width` characters of `text`, marking what was dropped.
///
/// Counts characters, not bytes: a multi-byte path would otherwise be cut mid
/// character.
fn truncate_start(text: &str, width: usize) -> String {
    let length = text.chars().count();
    if length <= width {
        return text.to_owned();
    }
    if width <= 1 {
        return "…".chars().take(width).collect();
    }
    let skip = length - (width - 1);
    std::iter::once('…')
        .chain(text.chars().skip(skip))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::{
        SessionId, SessionLifecycle, SessionPresentation, SessionRecord, SessionRole,
    };
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    fn record(id: &str, harness: &str, lifecycle: SessionLifecycle) -> SessionRecord {
        SessionRecord {
            id: SessionId::new(id),
            project_id: "project".to_owned(),
            harness: harness.to_owned(),
            native_session_id: None,
            role: SessionRole::Normal,
            lifecycle,
            presentation: SessionPresentation::Embedded,
            created_at: 1_000,
            last_activity_at: 1_000,
            launch_profile: None,
            backend_resource: None,
            model: None,
            pairing_class: None,
            protocol: None,
            response_profile: None,
            response_mechanism: None,
            display_name: None,
            purpose: None,
            source_session_id: None,
            observed_compactions: None,
            presentation_ref: None,
            last_seen_commit: None,
            entitlement: None,
        }
    }

    /// One session, for the cases that need a project with nothing to navigate
    /// between.
    fn lone_session() -> SessionRecord {
        record("only-one", "claude-code", SessionLifecycle::Running)
    }

    /// The bottom row of a rendered frame, trimmed.
    fn last_row(state: &ShellState, width: u16, height: u16) -> String {
        rendered(state, width, height)
            .lines()
            .last()
            .expect("a frame has rows")
            .trim_end()
            .to_owned()
    }

    fn sample() -> ShellState {
        ShellState::new(
            "glasshouse",
            "/Users/someone/projects/glasshouse",
            "0.1.0",
            vec![
                record("aaaaaaaaaaaa1", "claude-code", SessionLifecycle::Running),
                record("bbbbbbbbbbbb2", "codex", SessionLifecycle::Stopped),
            ],
        )
    }

    /// Render and flatten the buffer to text, so a test can assert on what the
    /// user would actually see rather than only that drawing did not panic.
    fn rendered(state: &ShellState, width: u16, height: u16) -> String {
        let backend = TestBackend::new(width, height);
        let mut terminal = Terminal::new(backend).expect("terminal");
        terminal
            .draw(|frame| render(state, frame))
            .expect("draw must not panic");
        let buffer = terminal.backend().buffer();
        (0..buffer.area.height)
            .map(|y| {
                (0..buffer.area.width)
                    .map(|x| buffer[(x, y)].symbol())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Phase 1: "Display the active canonical project root prominently in the
    /// TUI." Asserted against the rendered buffer, not against the state.
    #[test]
    fn the_project_root_is_displayed_on_every_frame() {
        let state = sample();
        let text = rendered(&state, 100, 24);
        assert!(
            text.contains("/Users/someone/projects/glasshouse"),
            "the canonical root must be on screen:\n{text}"
        );
        assert!(
            text.contains("glasshouse"),
            "the project name must be shown"
        );
    }

    /// It has to stay visible from every screen, including behind an overlay,
    /// or "prominently" would mean "until you open something".
    #[test]
    fn the_project_root_stays_visible_while_an_overlay_is_open() {
        let mut state = sample();
        state.handle_key(KeyEvent::new(KeyCode::Char('o'), KeyModifiers::NONE));
        let text = rendered(&state, 100, 24);
        assert!(
            text.contains("/Users/someone/projects/glasshouse"),
            "the root must survive an overlay:\n{text}"
        );
    }

    /// The row the root is drawn on, so an assertion about the root cannot be
    /// satisfied by the same word appearing in the title bar.
    fn root_row(state: &ShellState, width: u16, height: u16) -> String {
        rendered(state, width, height)
            .lines()
            .nth(1)
            .expect("the root occupies the second row")
            .trim_end()
            .to_owned()
    }

    /// A narrow terminal must keep the identifying tail of the path, not the
    /// useless head.
    ///
    /// Asserted against the root row alone. An earlier version searched the
    /// whole frame, which meant "glasshouse" matched the title bar and the test
    /// passed even when the path was truncated from the wrong end.
    #[test]
    fn a_narrow_terminal_keeps_the_end_of_the_project_root() {
        let row = root_row(&sample(), 28, 12);
        assert!(row.contains('…'), "truncation must be visible: `{row}`");
        assert!(
            row.ends_with("glasshouse"),
            "the tail identifies the project and must survive: `{row}`"
        );
        assert!(
            !row.contains("/Users/someone"),
            "the head should have been dropped, not the tail: `{row}`"
        );
    }

    /// A wide terminal shows the root untouched — otherwise the test above
    /// could be satisfied by always truncating.
    #[test]
    fn a_wide_terminal_shows_the_whole_project_root() {
        let row = root_row(&sample(), 120, 24);
        assert_eq!(row, "root /Users/someone/projects/glasshouse");
    }

    #[test]
    fn the_session_bar_lists_every_known_session() {
        let text = rendered(&sample(), 100, 24);
        assert!(
            text.contains("claude-code"),
            "missing first session:\n{text}"
        );
        assert!(text.contains("codex"), "missing second session:\n{text}");
    }

    #[test]
    fn an_empty_project_says_so_instead_of_showing_an_empty_bar() {
        let state = ShellState::new("p", "/p", "0.1.0", Vec::new());
        let text = rendered(&state, 80, 24);
        assert!(text.contains("no sessions yet"), "got:\n{text}");
        assert!(text.contains("No session is active"), "got:\n{text}");
    }

    #[test]
    fn the_overview_shows_detail_the_session_bar_has_no_room_for() {
        let mut state = sample();
        state.handle_key(KeyEvent::new(KeyCode::Char('o'), KeyModifiers::NONE));
        let text = rendered(&state, 100, 24);
        assert!(text.contains("HARNESS"), "overview header missing:\n{text}");
        assert!(
            text.contains("PRESENTED"),
            "overview header missing:\n{text}"
        );
        // A stopped session with no native identifier is over, not resumable.
        assert!(text.contains("closed"), "disposition missing:\n{text}");
    }

    /// The overview marks two different things, and confusing them would make
    /// it useless for its own capability: `>` is the row an interrupt or a
    /// sent line acts on, and `(viewport)` is the session the shell is
    /// showing. This drives them apart and checks both are visible at once.
    #[test]
    fn the_overview_distinguishes_the_row_it_acts_on_from_the_one_on_screen() {
        let mut state = two_live_sessions();
        state.handle_key(KeyEvent::new(KeyCode::Char('o'), KeyModifiers::NONE));
        state.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));

        // Matched on the identifier *and* a column only the overview draws:
        // the top bar names the presented session too, and it is the first
        // line in the frame.
        let text = rendered(&state, 120, 24);
        let row = |needle: &str| {
            text.lines()
                .find(|line| line.contains(needle) && line.contains("embedded"))
                .unwrap_or_else(|| panic!("no overview row for {needle}:\n{text}"))
        };
        let cursor_row = row("bbbbbbbbbbbb");
        let viewport_row = row("aaaaaaaaaaaa");

        assert!(
            cursor_row.contains("> codex"),
            "the cursor row must be marked:\n{text}"
        );
        assert!(
            viewport_row.contains("(viewport)"),
            "the presented session must say so:\n{text}"
        );
        assert!(
            !viewport_row.contains("> claude-code"),
            "the cursor is not on the presented session here:\n{text}"
        );
        assert!(
            !cursor_row.contains("(viewport)"),
            "the session under the cursor is not the one on screen:\n{text}"
        );
    }

    /// Two *live* sessions, unlike [`sample`], whose second one has stopped:
    /// every overview action is refused against a session that is not
    /// running, so a stopped fixture would prove nothing about the actions.
    fn two_live_sessions() -> ShellState {
        ShellState::new(
            "glasshouse",
            "/Users/someone/projects/glasshouse",
            "0.1.0",
            vec![
                record("aaaaaaaaaaaa1", "claude-code", SessionLifecycle::Running),
                record("bbbbbbbbbbbb2", "codex", SessionLifecycle::Running),
            ],
        )
    }

    /// A refusal about a row in the overview must be readable at an ordinary
    /// terminal width — identifier and state both. The footer clips a note
    /// once the bindings have had their share, which is why the overview
    /// draws it inside the popup as well.
    #[test]
    fn a_refusal_is_readable_inside_the_overview_at_a_hundred_columns() {
        let mut state = ShellState::new(
            "glasshouse",
            "/Users/someone/projects/glasshouse",
            "0.1.0",
            vec![
                record("aaaaaaaaaaaa1", "claude-code", SessionLifecycle::Running),
                record("bbbbbbbbbbbb2", "codex", SessionLifecycle::Stopped),
            ],
        );
        state.handle_key(KeyEvent::new(KeyCode::Char('o'), KeyModifiers::NONE));
        state.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        state.handle_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::NONE));

        let text = rendered(&state, 100, 30);
        let note = text
            .lines()
            .find(|line| line.contains("cannot interrupt"))
            .unwrap_or_else(|| panic!("the refusal never reached the screen:\n{text}"));
        assert!(
            note.contains("bbbbbbbbbbbb"),
            "the refusal must still name the session at 100 columns: `{note}`"
        );
        assert!(
            note.contains("stopped"),
            "the refusal must still name the state at 100 columns: `{note}`"
        );
    }

    /// The field for sending a line names the session it is aimed at. A field
    /// that hides its own target is how text ends up in the wrong session.
    #[test]
    fn the_send_field_names_the_session_it_is_aimed_at() {
        let mut state = two_live_sessions();
        state.handle_key(KeyEvent::new(KeyCode::Char('o'), KeyModifiers::NONE));
        state.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        state.handle_key(KeyEvent::new(KeyCode::Char('m'), KeyModifiers::NONE));
        for c in "run the tests".chars() {
            state.handle_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
        }

        let text = rendered(&state, 120, 30);
        assert!(
            text.contains("send to bbbbbbbbbbbb"),
            "the field must name its target:\n{text}"
        );
        assert!(
            text.contains("run the tests"),
            "the typed line must be visible:\n{text}"
        );
        assert!(
            text.contains("enter sends"),
            "the field must say how to send it:\n{text}"
        );
    }

    /// A headless session's screen must never reach the viewport, even when
    /// a grid for it is sitting in the state — which is exactly what happens
    /// on the frame drawn between the bar moving onto it and the next tick
    /// rebuilding the grid.
    ///
    /// Rendered wide as well as narrow: an absence assertion against a
    /// truncated frame is trivially true, which this project has already paid
    /// for once (practice §17).
    #[test]
    fn a_headless_sessions_screen_never_reaches_the_viewport() {
        let mut headless = record("hidden-one", "claude-code", SessionLifecycle::Running);
        headless.presentation = SessionPresentation::Headless;
        let mut state = ShellState::new("p", "/p", "0.1.0", vec![headless]);
        state.set_viewport_grid(grid_from_lines(&["HEADLESS-SCREEN-BYTES"]));

        for width in [100, 400] {
            let text = rendered(&state, width, 24);
            assert!(
                !text.contains("HEADLESS-SCREEN-BYTES"),
                "a headless session drew into the viewport at {width} columns:\n{text}"
            );
            assert!(
                text.contains("headless"),
                "and the viewport must say why it is empty:\n{text}"
            );
        }
    }

    /// The same grid, on an embedded session, *is* drawn — without this the
    /// absence above would pass for a renderer that draws nothing at all.
    #[test]
    fn an_embedded_sessions_screen_does_reach_the_viewport() {
        let mut state = ShellState::new(
            "p",
            "/p",
            "0.1.0",
            vec![record(
                "shown-one",
                "claude-code",
                SessionLifecycle::Running,
            )],
        );
        state.set_viewport_grid(grid_from_lines(&["HEADLESS-SCREEN-BYTES"]));

        let text = rendered(&state, 400, 24);
        assert!(
            text.contains("HEADLESS-SCREEN-BYTES"),
            "an embedded session's screen must be drawn:\n{text}"
        );
    }

    /// The keys are only discoverable if the footer says they exist.
    #[test]
    fn the_footer_names_the_overview_and_headless_keys() {
        let mut state = sample();
        assert!(
            last_row(&state, 120, 10).contains("N headless"),
            "control mode must offer the headless key"
        );

        state.handle_key(KeyEvent::new(KeyCode::Char('o'), KeyModifiers::NONE));
        let row = last_row(&state, 120, 10);
        assert!(row.contains("m send text"), "got {row:?}");
        assert!(row.contains("c interrupt"), "got {row:?}");
    }

    /// Ratatui will happily panic on a zero-sized or one-cell area if any
    /// layout maths underflows. None here does, and this proves it.
    #[test]
    fn renders_without_panicking_at_absurd_sizes() {
        let mut state = sample();
        for (w, h) in [(1, 1), (1, 40), (40, 1), (3, 3), (200, 60)] {
            rendered(&state, w, h);
        }
        state.handle_key(KeyEvent::new(KeyCode::Char('o'), KeyModifiers::NONE));
        for (w, h) in [(1, 1), (1, 40), (40, 1), (3, 3), (200, 60)] {
            rendered(&state, w, h);
        }
        // The send field adds rows under the table, which is exactly the kind
        // of growth that overruns a popup nobody re-measured.
        state.handle_key(KeyEvent::new(KeyCode::Char('m'), KeyModifiers::NONE));
        for (w, h) in [(1, 1), (1, 40), (40, 1), (3, 3), (200, 60)] {
            rendered(&state, w, h);
        }
    }

    #[test]
    fn truncate_start_keeps_the_tail_and_counts_characters() {
        assert_eq!(truncate_start("/a/b/c", 10), "/a/b/c");
        assert_eq!(truncate_start("/a/b/c", 6), "/a/b/c");
        assert_eq!(truncate_start("/a/b/c", 4), "…b/c");
        assert_eq!(truncate_start("/a/b/c", 1), "…");
        assert_eq!(truncate_start("/a/b/c", 0), "");
        // Multi-byte characters must not be cut in half.
        let text = truncate_start("/päth/tö/pröject", 8);
        assert_eq!(text.chars().count(), 8, "counted characters, not bytes");
        assert!(text.ends_with("öject"));
    }

    #[test]
    fn wrapped_row_count_matches_simple_word_wrap() {
        assert_eq!(wrapped_row_count("hello world", 20), 1);
        assert_eq!(
            wrapped_row_count("hello world", 5),
            2,
            "each word wraps to its own row"
        );
        assert_eq!(
            wrapped_row_count("", 20),
            1,
            "an empty line still occupies one row"
        );
        assert_eq!(
            wrapped_row_count("a b c d", 3),
            2,
            "\"a b\" then \"c d\" at width 3"
        );
        assert_eq!(
            wrapped_row_count("a b c d", 1000),
            1,
            "a generous width never wraps"
        );
        // Never panics or loops forever even at the narrowest width.
        assert!(wrapped_row_count("a fairly long sentence indeed", 0) > 0);
        assert!(wrapped_row_count("a fairly long sentence indeed", 1) > 0);
    }

    #[test]
    fn wrapped_height_sums_every_lines_own_row_count() {
        let lines = vec![
            Line::from("hello world"),
            Line::from("a somewhat longer second line here"),
        ];
        let narrow = wrapped_height(&lines, 5);
        let wide = wrapped_height(&lines, 200);
        assert_eq!(wide, 2, "a generous width needs exactly one row per line");
        assert!(
            narrow > wide,
            "a narrower width must never need fewer rows: narrow={narrow} wide={wide}"
        );
        assert_eq!(wrapped_height(&[], 80), 0, "no lines needs no height");
    }

    /// The regression this function exists to prevent: a long label that
    /// wraps by itself must not leave the error line beneath it with no
    /// room, the way a fixed `2` did before `wrapped_height` replaced it —
    /// found by driving the real binary, not by a unit test.
    #[test]
    fn wrapped_height_leaves_room_for_a_wrapped_label_and_its_error_line() {
        let label_line = Line::from(
            "Harness for `custom` (claude-code, codex, antigravity, opencode, cursor, pi, \
             hermes): not-a-real-harness_",
        );
        let error_line = Line::from(
            "`not-a-real-harness` is not a harness Glasshouse knows; known harnesses are: \
             claude-code, codex, antigravity, opencode, cursor, pi, hermes",
        );
        let width = 88;
        let height = wrapped_height(&[label_line.clone(), error_line.clone()], width);
        let label_rows = wrapped_row_count(&String::from(label_line), width);
        let error_rows = wrapped_row_count(&String::from(error_line), width);
        assert!(label_rows > 1, "the label alone must wrap in this scenario");
        assert_eq!(
            height as usize,
            label_rows + error_rows,
            "the error line's rows must be included, not clipped by a fixed height"
        );
    }

    /// The bottom bar carries the key bindings on every screen, including from
    /// inside an overlay where the bindings change.
    #[test]
    fn the_status_bar_always_shows_the_key_bindings() {
        let mut state = sample();
        // 132, not 120: Phase 25's `k knowledge` binding took the row past a
        // hundred columns, the same trade recorded on
        // `the_status_bar_shows_a_note_next_to_the_bindings` below; map line
        // 234's `M memory` pushed it past 120 in turn, so this became 132.
        // Phase 47 line 1765's `h health` took the whole row to exactly 140
        // columns, so this was 142; `d decisions` adds fourteen more, taking
        // it to exactly 154, so this is 156 — measured against the row, not
        // guessed.
        let bottom = last_row(&state, 156, 24);
        assert!(bottom.contains("tab"), "bindings missing: `{bottom}`");
        assert!(bottom.contains("overview"), "bindings missing: `{bottom}`");
        assert!(bottom.contains("quit"), "bindings missing: `{bottom}`");

        state.handle_key(KeyEvent::new(KeyCode::Char('o'), KeyModifiers::NONE));
        let bottom = last_row(&state, 156, 24);
        assert!(
            bottom.contains("esc") && bottom.contains("quit"),
            "the overlay's bindings must be shown too: `{bottom}`"
        );
    }

    /// A key that could not do anything must explain itself in the status bar,
    /// or it reads as a broken keyboard.
    ///
    /// Measured at 120 columns rather than 100. Phase 4's `N headless` took
    /// the control-mode bindings to about seventy columns, and the bindings
    /// are written first on purpose, so at a hundred there is no longer room
    /// for a whole note beside them — the same trade the footer's own doc
    /// comment records paying when `t`/`m` arrived. It is paid here rather
    /// than by dropping the binding because a binding nobody can see is a
    /// feature nobody has, while a clipped note is still a note; and the
    /// refusals *this* phase depends on are shown inside the overview
    /// popup, where they are never clipped — see `render_overview`.
    #[test]
    fn the_status_bar_shows_a_note_next_to_the_bindings() {
        let mut state = ShellState::new("p", "/p", "0.1.0", vec![lone_session()]);
        state.handle_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));

        // 120 columns fit the note alongside the bindings before Phase 47
        // added `e events`, and 132 after; Phase 25's `k knowledge` pushed
        // the row past 132, so this became 150; Phase 47's `r routes`
        // (batch 43) pushed it past 150, so this became 170; line 1765's
        // `h health` added eleven more columns, so this was 182; `d
        // decisions` adds fourteen, so this is 196 — the same margin this
        // test always had, measured against the longer row rather than
        // assumed.
        let bottom = last_row(&state, 196, 24);
        assert!(
            bottom.contains("only one session"),
            "the note must reach the status bar: `{bottom}`"
        );
        assert!(
            bottom.contains("tab"),
            "the note must not displace the bindings: `{bottom}`"
        );
    }

    /// On a narrow terminal the bindings win: they are needed permanently, the
    /// note only once. This holds because the bindings are written first and
    /// the row clips on the right — swap the order and this fails.
    #[test]
    fn a_note_is_dropped_rather_than_crowding_out_the_bindings() {
        let mut state = ShellState::new("p", "/p", "0.1.0", vec![lone_session()]);
        state.handle_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
        let bottom = last_row(&state, 30, 12);
        assert!(
            !bottom.contains("only one session"),
            "there was no room for the note: `{bottom}`"
        );
        assert!(bottom.contains("tab"), "bindings must survive: `{bottom}`");
    }

    /// "Keep the visual design text-first and avoid decorative graph
    /// visualizations that do not expose actionable state." — map line 1771,
    /// which this test proved for the shell and the session overview before
    /// Phase 47's two overlays existed to add.
    ///
    /// Enforced mechanically rather than by inspection: Ratatui's decorative
    /// widgets — `Gauge`, `Sparkline`, `BarChart` — all draw with the Unicode
    /// block elements, so a frame that contains none of those, on any screen,
    /// cannot be rendering one. Box-drawing characters used for borders are a
    /// different range and stay allowed.
    #[test]
    fn nothing_draws_with_block_elements_so_the_design_stays_text_first() {
        let mut state = sample();
        let mut screens = vec![rendered(&state, 100, 30)];
        state.handle_key(KeyEvent::new(KeyCode::Char('o'), KeyModifiers::NONE));
        screens.push(rendered(&state, 100, 30));
        state.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        state.handle_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
        screens.push(rendered(&state, 100, 30));

        // The two diagnostic overlays map line 1771 names by name — a
        // "knowledge-graph visualization" is exactly what a `Gauge` or
        // `Sparkline` would be reaching for here.
        state.handle_key(KeyEvent::new(KeyCode::Char('p'), KeyModifiers::NONE));
        state.open_project_overview(Vec::new(), Vec::new(), 0, Vec::new(), String::new(), None);
        screens.push(rendered(&state, 100, 30));
        state.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        state.handle_key(KeyEvent::new(KeyCode::Char('e'), KeyModifiers::NONE));
        screens.push(rendered(&state, 100, 30));
        // Phase 25's own diagnostic overlay, map line 1107: a "decorative
        // node graph" is exactly the shape this sweep already guards
        // against for every other overlay.
        state.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        state.handle_key(KeyEvent::new(KeyCode::Char('k'), KeyModifiers::NONE));
        state.open_project_knowledge(
            KnowledgeSection::default(),
            KnowledgeSection::default(),
            KnowledgeSection::default(),
            KnowledgeSection::default(),
            KnowledgeSection::default(),
            None,
        );
        screens.push(rendered(&state, 100, 30));
        // Phase 47 line 1771 is a standing property, so every diagnostic
        // surface added afterwards has to re-prove it. This one is line
        // 1765's, and a "route health" display is exactly where somebody
        // would reach for a `Gauge`.
        state.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        state.handle_key(KeyEvent::new(KeyCode::Char('h'), KeyModifiers::NONE));
        state.open_route_health(vec![crate::shell::state::RouteHealthRow {
            provider: "anyrouter".to_owned(),
            credential_label: "anyrouter/API_KEY".to_owned(),
            model: "claude-opus-4-1".to_owned(),
            consecutive_failures: 2,
            credential_rejected: false,
            available_now: true,
            cooling_down_until_unix: None,
            stated_limit: Some(300),
            stated_window_seconds: Some(60),
            quota_resets_at_unix: None,
            failure_domain: "unknown".to_owned(),
            failure_domain_peers: 0,
        }]);
        screens.push(rendered(&state, 100, 30));

        for screen in screens {
            if let Some(found) = screen.chars().find(|c| {
                // U+2580..U+259F: block elements, which is what gauges,
                // sparklines and bar charts are made of.
                ('\u{2580}'..='\u{259F}').contains(c)
            }) {
                panic!("a decorative block element ({found:?}) was drawn:\n{screen}");
            }
        }
    }

    // -----------------------------------------------------------------
    // The viewport and the session-mode footer.
    // -----------------------------------------------------------------

    /// Build a plain, unstyled grid from lines of ASCII text, padding every
    /// row to the widest one with blanks — good enough for rendering tests,
    /// which are about placement and clipping, not about `vt100` conversion
    /// (that lives in `shell::mod`'s own tests, against a real parser).
    fn grid_from_lines(lines: &[&str]) -> ViewportGrid {
        let rows = lines.len() as u16;
        let cols = lines
            .iter()
            .map(|line| line.chars().count())
            .max()
            .unwrap_or(0) as u16;
        let mut cells = Vec::with_capacity(usize::from(rows) * usize::from(cols));
        for line in lines {
            let chars: Vec<char> = line.chars().collect();
            for col in 0..usize::from(cols) {
                let ch = chars.get(col).copied().unwrap_or(' ');
                cells.push((ch.to_string(), Style::default()));
            }
        }
        ViewportGrid::new(rows, cols, cells, None)
    }

    #[test]
    fn the_viewport_shows_the_set_grid_once_non_empty() {
        let mut state = sample();
        state.set_viewport_grid(grid_from_lines(&[
            "first line",
            "second line",
            "third line",
        ]));
        let text = rendered(&state, 100, 24);
        assert!(text.contains("first line"), "got:\n{text}");
        assert!(text.contains("second line"), "got:\n{text}");
        assert!(text.contains("third line"), "got:\n{text}");
        assert!(
            !text.contains("This viewport is reserved"),
            "the placeholder must not show once there is a live screen:\n{text}"
        );
    }

    /// A screen taller than the render area is clipped, not overflowed — the
    /// top of the screen stays visible, since a `vt100` screen has no notion
    /// of "the most recent lines" the way raw scrollback text did.
    #[test]
    fn a_grid_taller_than_the_area_is_clipped_not_overflowed() {
        let mut state = sample();
        let lines: Vec<String> = (0..50).map(|i| format!("line-{i}")).collect();
        let borrowed: Vec<&str> = lines.iter().map(String::as_str).collect();
        state.set_viewport_grid(grid_from_lines(&borrowed));

        let text = rendered(&state, 100, 10);
        assert!(
            text.contains("line-0"),
            "the top of the screen must stay visible:\n{text}"
        );
    }

    /// "Keep the existing placeholder for the no-session case" — and for the
    /// active-but-silent-so-far case too: the placeholder is only replaced
    /// once a live session has a screen to show.
    #[test]
    fn an_empty_viewport_keeps_the_existing_placeholder() {
        let state = sample();
        assert!(state.viewport_grid().is_empty());
        let text = rendered(&state, 100, 24);
        assert!(text.contains("This viewport is reserved"), "got:\n{text}");
    }

    /// A live grid gets the whole viewport area with no border, so the
    /// harness it belongs to is the dominant thing on screen; the
    /// placeholder keeps its border since there is no harness screen yet to
    /// compete with it.
    #[test]
    fn the_viewport_border_is_dropped_once_a_live_grid_is_shown() {
        let mut state = sample();
        let placeholder = rendered(&state, 40, 10);
        assert!(
            placeholder.contains('┌'),
            "the placeholder keeps its border:\n{placeholder}"
        );

        state.set_viewport_grid(grid_from_lines(&["hello"]));
        let live = rendered(&state, 40, 10);
        assert!(
            !live.contains('┌'),
            "a live session's screen must not be boxed in:\n{live}"
        );
        assert!(live.contains("hello"), "got:\n{live}");
    }

    /// A grid full of real content, at sizes much smaller than the content,
    /// must not panic the clipping that keeps it bounded to the area.
    #[test]
    fn the_viewport_does_not_panic_with_a_real_grid_at_absurd_sizes() {
        let mut state = sample();
        let lines: Vec<String> = (0..500)
            .map(|i| format!("line-{i}-{}", "x".repeat(i % 50)))
            .collect();
        let borrowed: Vec<&str> = lines.iter().map(String::as_str).collect();
        state.set_viewport_grid(grid_from_lines(&borrowed));
        for (w, h) in [(1, 1), (1, 40), (40, 1), (3, 3), (200, 60)] {
            rendered(&state, w, h);
        }
    }

    /// A cursor position the emulator reports but that the render area
    /// cannot contain (a resize race) must be ignored, not panic.
    #[test]
    fn a_cursor_outside_the_render_area_does_not_panic() {
        let mut state = sample();
        state.set_viewport_grid(ViewportGrid::new(
            1,
            1,
            vec![("x".to_owned(), Style::default())],
            Some((99, 99)),
        ));
        state.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert_eq!(state.mode(), Mode::Session);
        rendered(&state, 1, 1);
    }

    /// The design note: "A user who cannot see how to get out is the
    /// failure this design exists to prevent" — so the mode and the escape
    /// chord are on screen in session mode at all times.
    #[test]
    fn the_status_bar_names_session_mode_and_the_escape_chord() {
        let mut state = sample();
        state.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert_eq!(state.mode(), Mode::Session);

        let bottom = last_row(&state, 100, 24).to_lowercase();
        assert!(
            bottom.contains("session"),
            "the active mode must be named: `{bottom}`"
        );
        assert!(
            bottom.contains("ctrl-]"),
            "the escape chord must always be on screen in session mode: `{bottom}`"
        );
    }

    /// Control mode's own footer must not claim to be session mode.
    #[test]
    fn the_status_bar_shows_control_mode_bindings_by_default() {
        let state = sample();
        assert_eq!(state.mode(), Mode::Control);
        // 156, not 100 — see `the_status_bar_always_shows_the_key_bindings`.
        let bottom = last_row(&state, 156, 24).to_lowercase();
        assert!(!bottom.contains("session mode"), "got: `{bottom}`");
        assert!(bottom.contains("quit"), "got: `{bottom}`");
    }

    /// The negative from practice §17: an empty activity list draws no
    /// `ACTIVITY` heading at all, not an empty section — because an empty
    /// section reads as "nothing has happened" rather than "nothing has been
    /// observed yet".
    #[test]
    fn the_overview_draws_no_activity_heading_with_no_events() {
        let mut state = sample();
        state.handle_key(KeyEvent::new(KeyCode::Char('o'), KeyModifiers::NONE));
        assert!(state.activity().is_empty());

        for (width, height) in [(100, 24), (400, 24)] {
            let text = rendered(&state, width, height);
            assert!(
                !text.contains("ACTIVITY"),
                "no events were recorded, so no heading should draw at width {width}:\n{text}"
            );
        }
    }

    /// Practice §17: a row can be truncated off-screen at a narrow width,
    /// which makes a `contains` assertion pass or fail for reasons that have
    /// nothing to do with the code — so this asserts at a realistic width
    /// *and* at 400 columns.
    #[test]
    fn the_overview_shows_recorded_activity_at_a_realistic_and_a_wide_width() {
        use crate::events::{EventBus, LifecycleEvent, MessageOrigin, TurnOutcome};
        use crate::session::SessionId;
        use crate::shell::Action;

        let mut state = sample();
        let bus = EventBus::new();
        let session = SessionId::new("aaaaaaaaaaaa1");
        let recorded = vec![
            bus.publish(&session, LifecycleEvent::SessionStarted),
            bus.publish(
                &session,
                LifecycleEvent::TextDelivered {
                    origin: MessageOrigin::Machine,
                    bytes: 41,
                },
            ),
            bus.publish(
                &session,
                LifecycleEvent::TurnEnded {
                    outcome: TurnOutcome::Completed,
                },
            ),
        ];
        assert_eq!(state.note_events(&recorded), Action::Redraw);
        state.handle_key(KeyEvent::new(KeyCode::Char('o'), KeyModifiers::NONE));

        for (width, height) in [(100, 24), (400, 24)] {
            let text = rendered(&state, width, height);
            assert!(text.contains("ACTIVITY"), "width {width}:\n{text}");
            assert!(text.contains("session started"), "width {width}:\n{text}");
            assert!(
                text.contains("sent 41 bytes (machine)"),
                "width {width}:\n{text}"
            );
            assert!(
                text.contains("turn ended (completed)"),
                "width {width}:\n{text}"
            );
            assert!(
                text.contains(&super::super::state::short_session_id(&session)),
                "the session must be named beside its event, width {width}:\n{text}"
            );
        }
    }

    /// Phase 11 lines 682 and 684: a session's name/purpose and its last
    /// activity time, both already on [`SessionRecord`] since Phase 10, shown
    /// in the overview table rather than nowhere.
    ///
    /// Asserted at a realistic width and a wide one — see §17. "Realistic"
    /// here is wider than the 100 columns the activity-feed test above uses:
    /// with `NAME` and `ACTIVE` written last in the row (see `render_overview`'s
    /// comment on why), a 100-column terminal clips them before the identifier
    /// the interrupt/send/resume keys act on, which is the trade this makes on
    /// purpose. 160 is where they first survive on this fixture — recorded
    /// here as a measurement, not assumed.
    #[test]
    fn the_new_overview_columns_survive_a_realistic_and_a_wide_width() {
        use crate::session::{SessionName, SessionPurpose};

        let now = crate::provider::cache::now_unix_seconds();
        let named = SessionRecord {
            display_name: Some(SessionName::parse("payment retries").expect("valid name")),
            last_activity_at: now - 90,
            ..record("aaaaaaaaaaaa1", "claude-code", SessionLifecycle::Running)
        };
        let purposed = SessionRecord {
            purpose: Some(SessionPurpose::parse("tests").expect("valid purpose")),
            last_activity_at: now - 3 * 86_400,
            ..record("bbbbbbbbbbbb2", "codex", SessionLifecycle::Stopped)
        };
        let mut state = ShellState::new(
            "glasshouse",
            "/Users/someone/projects/glasshouse",
            "0.1.0",
            vec![named, purposed],
        );
        state.handle_key(KeyEvent::new(KeyCode::Char('o'), KeyModifiers::NONE));

        for (width, height) in [(160, 30), (400, 30)] {
            let text = rendered(&state, width, height);
            assert!(text.contains("payment retries"), "width {width}:\n{text}");
            assert!(text.contains("tests"), "width {width}:\n{text}");
            assert!(text.contains("1 minute ago"), "width {width}:\n{text}");
            assert!(text.contains("3 days ago"), "width {width}:\n{text}");
            // Line 683: a `Running` session's fine state, not only its
            // coarse `active` disposition — and a `Stopped`-with-no-native-id
            // one's, which `Closed` alone does not distinguish from a
            // session that was explicitly closed.
            assert!(text.contains("active/running"), "width {width}:\n{text}");
            assert!(text.contains("closed/stopped"), "width {width}:\n{text}");
        }

        // A session with neither is a spoken sentinel, not a blank cell — an
        // empty cell here would be indistinguishable from one truncated away.
        let mut unnamed = sample();
        unnamed.handle_key(KeyEvent::new(KeyCode::Char('o'), KeyModifiers::NONE));
        let no_name = rendered(&unnamed, 400, 30);
        assert!(
            no_name.contains("(unnamed)"),
            "an absent name must say so, not render nothing:\n{no_name}"
        );
    }

    /// Map lines 1651-1654: the orchestrator, running workers, waiting
    /// workers and recently completed workers must each be distinguishable
    /// in the project overview — not collapsed into one undifferentiated
    /// session list, which a reader could not act on.
    #[test]
    fn the_project_overview_separates_orchestrator_and_workers_by_role_and_lifecycle() {
        // Exactly 12 characters each — `short_session_id` truncates there,
        // and this test asserts on the truncated identifier a real overlay
        // would show, not the untruncated fixture id.
        let mut orchestrator = record("orchestrator", "claude-code", SessionLifecycle::Running);
        orchestrator.role = SessionRole::Orchestrator;

        let mut running_worker = record("run-worker01", "codex", SessionLifecycle::Running);
        running_worker.role = SessionRole::Worker;

        let mut waiting_worker = record("wait-worker1", "codex", SessionLifecycle::WaitingForUser);
        waiting_worker.role = SessionRole::Worker;

        let mut completed_worker = record("done-worker1", "codex", SessionLifecycle::Stopped);
        completed_worker.role = SessionRole::Worker;
        completed_worker.native_session_id = Some("native-completed".to_owned());

        let mut state = ShellState::new(
            "glasshouse",
            "/work",
            "0.1.0",
            vec![
                orchestrator,
                running_worker,
                waiting_worker,
                completed_worker,
            ],
        );
        assert_eq!(
            state.handle_key(KeyEvent::new(KeyCode::Char('p'), KeyModifiers::NONE)),
            crate::shell::state::Action::OpenProjectOverview
        );
        state.open_project_overview(Vec::new(), Vec::new(), 0, Vec::new(), String::new(), None);

        let text = rendered(&state, 120, 40);
        assert!(text.contains("orchestrator"), "orchestrator row:\n{text}");
        assert!(text.contains("run-worker01"), "running row:\n{text}");
        assert!(text.contains("wait-worker1"), "waiting row:\n{text}");
        assert!(text.contains("done-worker1"), "completed row:\n{text}");
        assert!(
            !text.contains("no session is designated"),
            "an orchestrator was designated; the overlay must not say otherwise:\n{text}"
        );
    }

    /// The same box, with no orchestrator and no workers at all: every
    /// section says so honestly instead of rendering an empty heading — the
    /// same rule `render_overview` follows for its own activity section.
    #[test]
    fn the_project_overview_says_so_when_a_section_has_nothing() {
        let mut state = ShellState::new("glasshouse", "/work", "0.1.0", vec![lone_session()]);
        state.handle_key(KeyEvent::new(KeyCode::Char('p'), KeyModifiers::NONE));
        state.open_project_overview(Vec::new(), Vec::new(), 0, Vec::new(), String::new(), None);

        let text = rendered(&state, 120, 40);
        assert!(text.contains("no session is designated"), "{text}");
        assert!(text.contains("no workers running"), "{text}");
        assert!(text.contains("no workers waiting for input"), "{text}");
        assert!(text.contains("no completed workers recorded"), "{text}");
        assert!(text.contains("no current binding decisions"), "{text}");
        assert!(text.contains("no open todos"), "{text}");
        assert!(
            text.contains("no resources configured for this project"),
            "{text}"
        );
    }

    /// Map lines 1657-1660 and 1663: a resource line the run loop handed in
    /// reaches the popup — through [`ShellState::open_project_overview`], the
    /// same call the real run loop makes, not a value the view invents.
    /// Practice §17: asserted at a realistic width and a wide one, because a
    /// row truncated off-screen at 120 columns would make a later `!contains`
    /// absence assertion pass for the wrong reason.
    #[test]
    fn the_project_overview_shows_resource_capacity_the_run_loop_handed_it() {
        for (width, height) in [(120, 40), (400, 40)] {
            let mut state = ShellState::new("glasshouse", "/work", "0.1.0", vec![lone_session()]);
            state.handle_key(KeyEvent::new(KeyCode::Char('p'), KeyModifiers::NONE));
            state.open_project_overview(
                Vec::new(),
                Vec::new(),
                0,
                vec!["  openrouter (remote)  plenty 82% [measured], reset in 3600s".to_owned()],
                String::new(),
                None,
            );

            let text = rendered(&state, width, height);
            assert!(
                text.contains("openrouter (remote)"),
                "resource label, width {width}:\n{text}"
            );
            assert!(text.contains("82% [measured]"), "width {width}:\n{text}");
            assert!(text.contains("reset in 3600s"), "width {width}:\n{text}");
        }
    }

    /// Map lines 1658 and 1659: a resource with no telemetry renders
    /// `"unknown"` and no number at all — asserted at both widths so a
    /// truncated row cannot make the absence trivially true (practice §17).
    #[test]
    fn an_unknown_resource_never_shows_a_number_at_a_realistic_and_a_wide_width() {
        for (width, height) in [(120, 40), (400, 40)] {
            let mut state = ShellState::new("glasshouse", "/work", "0.1.0", vec![lone_session()]);
            state.handle_key(KeyEvent::new(KeyCode::Char('p'), KeyModifiers::NONE));
            state.open_project_overview(
                Vec::new(),
                Vec::new(),
                0,
                vec!["  some-provider (remote)  capacity unknown".to_owned()],
                String::new(),
                None,
            );

            let text = rendered(&state, width, height);
            assert!(text.contains("capacity unknown"), "width {width}:\n{text}");
            assert!(!text.contains('%'), "width {width}:\n{text}");
        }
    }

    /// Map line 1661: the run loop's routing line reaches the screen as its
    /// own labelled section, at a realistic width and a wide one (practice
    /// §17), and fits an 80-column terminal without corrupting the rest of
    /// the overview.
    #[test]
    fn the_project_overview_shows_the_routing_line_the_run_loop_handed_it() {
        for (width, height) in [(80, 40), (120, 40), (400, 40)] {
            let mut state = ShellState::new("glasshouse", "/work", "0.1.0", vec![lone_session()]);
            state.handle_key(KeyEvent::new(KeyCode::Char('p'), KeyModifiers::NONE));
            state.open_project_overview(
                Vec::new(),
                Vec::new(),
                0,
                Vec::new(),
                "  routing model  anyrouter:claude-opus-4-1, recent latency median 340ms, \
                 p95 410ms (12 sample(s))"
                    .to_owned(),
                None,
            );

            let text = rendered(&state, width, height);
            assert!(text.contains("ROUTING MODEL"), "width {width}:\n{text}");
            assert!(
                flattened(&text).contains("anyrouter:claude-opus-4-1"),
                "width {width}:\n{text}"
            );
            assert!(
                flattened(&text).contains("median 340ms"),
                "width {width}:\n{text}"
            );
        }
    }

    /// Ruling 1, at the view: an unknown latency reads `unknown`, never a
    /// fabricated `0ms` — the same honesty rule
    /// [`an_unknown_resource_never_shows_a_number_at_a_realistic_and_a_wide_width`]
    /// proves for resources, proven here for the routing line specifically
    /// because it is a different builder and a different render branch.
    #[test]
    fn the_routing_lines_unknown_latency_never_reads_as_zero() {
        for (width, height) in [(80, 40), (120, 40), (400, 40)] {
            let mut state = ShellState::new("glasshouse", "/work", "0.1.0", vec![lone_session()]);
            state.handle_key(KeyEvent::new(KeyCode::Char('p'), KeyModifiers::NONE));
            state.open_project_overview(
                Vec::new(),
                Vec::new(),
                0,
                Vec::new(),
                "  routing model  anyrouter:claude-opus-4-1, recent latency unknown — not \
                 enough observations yet"
                    .to_owned(),
                None,
            );

            let text = rendered(&state, width, height);
            assert!(
                flattened(&text).contains("recent latency unknown"),
                "width {width}:\n{text}"
            );
            assert!(!text.contains("0ms"), "width {width}:\n{text}");
            assert!(!text.contains("0 ms"), "width {width}:\n{text}");
        }
    }

    /// The routing section is absent when the run loop never set anything —
    /// the fixtures elsewhere in this module that pass `String::new()`
    /// because the routing line is not what they are testing, distinct from
    /// a real overview the run loop opened.
    #[test]
    fn the_project_overview_omits_the_routing_section_when_nothing_was_handed_to_it() {
        let mut state = ShellState::new("glasshouse", "/work", "0.1.0", vec![lone_session()]);
        state.handle_key(KeyEvent::new(KeyCode::Char('p'), KeyModifiers::NONE));
        state.open_project_overview(Vec::new(), Vec::new(), 0, Vec::new(), String::new(), None);

        let text = rendered(&state, 120, 40);
        assert!(!text.contains("ROUTING MODEL"), "{text}");
    }

    /// Map lines 1655 and 1656: decisions/constraints and unresolved todos
    /// the run loop read from project memory must actually reach the
    /// screen — through [`ShellState::open_project_overview`], the same call
    /// the real run loop makes, not a value the view invents.
    #[test]
    fn the_project_overview_shows_memory_the_run_loop_handed_it() {
        let mut state = ShellState::new("glasshouse", "/work", "0.1.0", vec![lone_session()]);
        state.handle_key(KeyEvent::new(KeyCode::Char('p'), KeyModifiers::NONE));
        state.open_project_overview(
            vec!["constraint: never run ci-local beside cargo".to_owned()],
            vec!["todo: wire the shell into main".to_owned()],
            3,
            Vec::new(),
            String::new(),
            None,
        );

        let text = rendered(&state, 120, 40);
        assert!(
            text.contains("never run ci-local beside cargo"),
            "decision/constraint line:\n{text}"
        );
        assert!(
            text.contains("wire the shell into main"),
            "todo line:\n{text}"
        );
        assert!(
            text.contains("...and 3 more"),
            "omitted todos must be counted, not silently dropped:\n{text}"
        );
    }

    /// A memory read failure still opens the overlay — sessions are still
    /// worth showing — and says plainly that memory could not be read,
    /// rather than presenting empty sections as if there were nothing to
    /// show.
    #[test]
    fn a_project_memory_read_failure_still_opens_with_an_honest_note() {
        let mut state = ShellState::new("glasshouse", "/work", "0.1.0", vec![lone_session()]);
        state.handle_key(KeyEvent::new(KeyCode::Char('p'), KeyModifiers::NONE));
        state.open_project_overview(
            Vec::new(),
            Vec::new(),
            0,
            Vec::new(),
            String::new(),
            Some("project memory unavailable: disk full".to_owned()),
        );

        let text = rendered(&state, 120, 40);
        assert!(
            text.contains("project memory unavailable: disk full"),
            "{text}"
        );
    }

    /// A [`MemoryDetail`] fixture with something recorded in every field —
    /// paired with [`one_entry`] so `lines` and `details` stay index-aligned
    /// the way production [`super::super::build_project_knowledge_memory`]
    /// keeps them.
    fn fixture_detail() -> MemoryDetail {
        MemoryDetail {
            rationale: Some("fixture rationale".to_owned()),
            source_session: Some("sess_fixture".to_owned()),
            source_commit: Some("abc1234".to_owned()),
            lifecycle: "active".to_owned(),
        }
    }

    /// One [`KnowledgeSection`] with a single fixture line, for the five
    /// section-presence tests below — each supplies this to exactly one of
    /// the project-knowledge view's five sections and leaves the other four
    /// at their `Default`, so a mutation that deletes only one section's
    /// heading fails only that section's test.
    fn one_entry(text: &str) -> KnowledgeSection {
        KnowledgeSection {
            lines: vec![text.to_owned()],
            details: vec![fixture_detail()],
            omitted: 0,
        }
    }

    /// Map lines 1098 and 1100: the project-knowledge view opens and shows
    /// active decisions in their own labelled section.
    #[test]
    fn the_project_knowledge_view_shows_active_decisions_in_their_own_section() {
        let mut state = ShellState::new("glasshouse", "/work", "0.1.0", vec![lone_session()]);
        state.handle_key(KeyEvent::new(KeyCode::Char('k'), KeyModifiers::NONE));
        state.open_project_knowledge(
            one_entry("decision: adopt the grouped-text project-knowledge view"),
            KnowledgeSection::default(),
            KnowledgeSection::default(),
            KnowledgeSection::default(),
            KnowledgeSection::default(),
            None,
        );

        let text = rendered(&state, 120, 40);
        assert!(text.contains("ACTIVE DECISIONS"), "{text}");
        assert!(
            text.contains("adopt the grouped-text project-knowledge view"),
            "{text}"
        );
    }

    /// Map line 1101: known constraints show in their own labelled section.
    #[test]
    fn the_project_knowledge_view_shows_known_constraints_in_their_own_section() {
        let mut state = ShellState::new("glasshouse", "/work", "0.1.0", vec![lone_session()]);
        state.handle_key(KeyEvent::new(KeyCode::Char('k'), KeyModifiers::NONE));
        state.open_project_knowledge(
            KnowledgeSection::default(),
            one_entry("constraint: never run ci-local beside cargo"),
            KnowledgeSection::default(),
            KnowledgeSection::default(),
            KnowledgeSection::default(),
            None,
        );

        let text = rendered(&state, 120, 40);
        assert!(text.contains("KNOWN CONSTRAINTS"), "{text}");
        assert!(text.contains("never run ci-local beside cargo"), "{text}");
    }

    /// Map line 1102: implemented-or-planned features show in their own
    /// labelled section.
    #[test]
    fn the_project_knowledge_view_shows_features_in_their_own_section() {
        let mut state = ShellState::new("glasshouse", "/work", "0.1.0", vec![lone_session()]);
        state.handle_key(KeyEvent::new(KeyCode::Char('k'), KeyModifiers::NONE));
        state.open_project_knowledge(
            KnowledgeSection::default(),
            KnowledgeSection::default(),
            one_entry("feature: the project-knowledge overlay"),
            KnowledgeSection::default(),
            KnowledgeSection::default(),
            None,
        );

        let text = rendered(&state, 120, 40);
        assert!(text.contains("FEATURES"), "{text}");
        assert!(text.contains("the project-knowledge overlay"), "{text}");
    }

    /// Map line 1103: failed approaches show in their own, dedicated
    /// historical section.
    #[test]
    fn the_project_knowledge_view_shows_failed_approaches_in_a_historical_section() {
        let mut state = ShellState::new("glasshouse", "/work", "0.1.0", vec![lone_session()]);
        state.handle_key(KeyEvent::new(KeyCode::Char('k'), KeyModifiers::NONE));
        state.open_project_knowledge(
            KnowledgeSection::default(),
            KnowledgeSection::default(),
            KnowledgeSection::default(),
            one_entry("failed_attempt: a single global lock deadlocked"),
            KnowledgeSection::default(),
            None,
        );

        let text = rendered(&state, 120, 40);
        assert!(text.contains("FAILED APPROACHES"), "{text}");
        assert!(text.contains("HISTORICAL"), "{text}");
        assert!(text.contains("a single global lock deadlocked"), "{text}");
    }

    /// Map line 1104: unresolved todos show in their own labelled section.
    #[test]
    fn the_project_knowledge_view_shows_unresolved_todos_in_their_own_section() {
        let mut state = ShellState::new("glasshouse", "/work", "0.1.0", vec![lone_session()]);
        state.handle_key(KeyEvent::new(KeyCode::Char('k'), KeyModifiers::NONE));
        state.open_project_knowledge(
            KnowledgeSection::default(),
            KnowledgeSection::default(),
            KnowledgeSection::default(),
            KnowledgeSection::default(),
            one_entry("todo: wire the knowledge view into main"),
            None,
        );

        let text = rendered(&state, 120, 40);
        assert!(text.contains("UNRESOLVED TODOS"), "{text}");
        assert!(text.contains("wire the knowledge view into main"), "{text}");
    }

    /// Map line 1098's empty-state half: with nothing recorded in any kind,
    /// every one of the five sections says so honestly rather than
    /// rendering an empty heading — the same rule
    /// `the_project_overview_says_so_when_a_section_has_nothing` proves for
    /// the project overview.
    #[test]
    fn the_project_knowledge_view_says_so_when_every_section_is_empty() {
        let mut state = ShellState::new("glasshouse", "/work", "0.1.0", vec![lone_session()]);
        state.handle_key(KeyEvent::new(KeyCode::Char('k'), KeyModifiers::NONE));
        state.open_project_knowledge(
            KnowledgeSection::default(),
            KnowledgeSection::default(),
            KnowledgeSection::default(),
            KnowledgeSection::default(),
            KnowledgeSection::default(),
            None,
        );

        let text = rendered(&state, 120, 40);
        assert!(text.contains("no active decisions recorded"), "{text}");
        assert!(text.contains("no known constraints recorded"), "{text}");
        assert!(text.contains("no features recorded"), "{text}");
        assert!(text.contains("no failed approaches recorded"), "{text}");
        assert!(text.contains("no unresolved todos"), "{text}");
    }

    /// Map line 1106, at the render layer: whatever line
    /// `shell::knowledge_line` hands the overlay reaches the screen
    /// unchanged — a supersession note included when it names one, and no
    /// note appended when it does not. The query-layer proof that the note
    /// is only ever added when a real `superseded_by` exists lives in
    /// `shell::project_knowledge_tests::
    /// failed_approaches_are_shown_regardless_of_status_and_name_their_successor`;
    /// this proves the text that function produces is not lost or altered
    /// on the way to the terminal.
    #[test]
    fn the_project_knowledge_view_shows_a_supersession_note_when_present_and_omits_it_when_absent()
    {
        let mut state = ShellState::new("glasshouse", "/work", "0.1.0", vec![lone_session()]);
        state.handle_key(KeyEvent::new(KeyCode::Char('k'), KeyModifiers::NONE));
        state.open_project_knowledge(
            KnowledgeSection::default(),
            KnowledgeSection::default(),
            KnowledgeSection::default(),
            KnowledgeSection {
                lines: vec![
                    "failed_attempt: a global lock — superseded by mem_01AAAAAAAAAAAAAAAAAAAAAAAA"
                        .to_owned(),
                    "failed_attempt: a distinct approach, still true".to_owned(),
                ],
                details: vec![fixture_detail(), fixture_detail()],
                omitted: 0,
            },
            KnowledgeSection::default(),
            None,
        );

        let text = rendered(&state, 120, 40);
        assert!(
            text.contains("superseded by mem_01AAAAAAAAAAAAAAAAAAAAAAAA"),
            "{text}"
        );
        assert_eq!(
            text.matches("superseded by").count(),
            1,
            "the line with no successor must not also carry a note:\n{text}"
        );
    }

    /// Map line 1107, and practice §17: the project-knowledge view draws
    /// plain text and never a decorative node graph — proved by scanning the
    /// rendered output for the glyphs a node-and-edge visualization would
    /// need (node markers, arrows/connectors), at a realistic width *and* a
    /// wide one, so a value truncated off a narrow render cannot make this
    /// pass for the wrong reason.
    ///
    /// Deliberately **not** scanning for ordinary box-drawing border
    /// characters (`┌│└` and friends): every overlay in this shell,
    /// including this one, draws inside a single bordered `Block` — see
    /// `nothing_draws_with_block_elements_so_the_design_stays_text_first`'s
    /// own comment, "Box-drawing characters used for borders are a
    /// different range and stay allowed." What line 1107 rules out is a
    /// graph of scattered, connected nodes, not this popup's own frame.
    #[test]
    fn the_project_knowledge_view_renders_no_decorative_graph_glyphs() {
        let mut state = ShellState::new("glasshouse", "/work", "0.1.0", vec![lone_session()]);
        state.handle_key(KeyEvent::new(KeyCode::Char('k'), KeyModifiers::NONE));
        state.open_project_knowledge(
            one_entry("decision: adopt the grouped-text project-knowledge view"),
            one_entry("constraint: never run ci-local beside cargo"),
            one_entry("feature: the project-knowledge overlay"),
            KnowledgeSection {
                lines: vec![
                    "failed_attempt: a global lock — superseded by mem_01AAAAAAAAAAAAAAAAAAAAAAAA"
                        .to_owned(),
                ],
                details: vec![fixture_detail()],
                omitted: 0,
            },
            one_entry("todo: wire the knowledge view into main"),
            None,
        );

        let graph_glyphs = [
            '●', '○', '◆', '◇', '■', '□', '▲', '▼', '→', '←', '↑', '↓', '↔', '↕',
        ];
        for (width, height) in [(120, 40), (400, 60)] {
            let text = rendered(&state, width, height);
            for glyph in graph_glyphs {
                assert!(
                    !text.contains(glyph),
                    "map line 1107: no decorative graph glyph `{glyph}`, width {width}:\n{text}"
                );
            }
        }
    }

    /// A project-knowledge read failure still opens the overlay — the same
    /// contract `a_project_memory_read_failure_still_opens_with_an_honest_note`
    /// proves for the project overview — and says plainly that memory could
    /// not be read rather than presenting empty sections as if there were
    /// nothing to show.
    #[test]
    fn a_project_knowledge_read_failure_still_opens_with_an_honest_note() {
        let mut state = ShellState::new("glasshouse", "/work", "0.1.0", vec![lone_session()]);
        state.handle_key(KeyEvent::new(KeyCode::Char('k'), KeyModifiers::NONE));
        state.open_project_knowledge(
            KnowledgeSection::default(),
            KnowledgeSection::default(),
            KnowledgeSection::default(),
            KnowledgeSection::default(),
            KnowledgeSection::default(),
            Some("project memory unavailable: disk full".to_owned()),
        );

        let text = rendered(&state, 120, 40);
        assert!(
            text.contains("project memory unavailable: disk full"),
            "{text}"
        );
    }

    /// Map line 1105, through the production key path: pressing Enter on
    /// the cursor's entry opens a detail popup showing its rationale,
    /// source session, source commit and lifecycle state.
    #[test]
    fn opening_a_memory_from_the_knowledge_view_shows_its_full_provenance() {
        let mut state = ShellState::new("glasshouse", "/work", "0.1.0", vec![lone_session()]);
        state.handle_key(KeyEvent::new(KeyCode::Char('k'), KeyModifiers::NONE));
        state.open_project_knowledge(
            KnowledgeSection {
                lines: vec!["decision: adopt the drill-down view".to_owned()],
                details: vec![MemoryDetail {
                    rationale: Some("keeps the popup answering one question at a time".to_owned()),
                    source_session: Some("sess_01AAAAAAAAAAAAAAAAAAAAAAAA".to_owned()),
                    source_commit: Some("d34db33f".to_owned()),
                    lifecycle: "active".to_owned(),
                }],
                omitted: 0,
            },
            KnowledgeSection::default(),
            KnowledgeSection::default(),
            KnowledgeSection::default(),
            KnowledgeSection::default(),
            None,
        );

        state.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

        let text = rendered(&state, 120, 40);
        assert!(text.contains("adopt the drill-down view"), "{text}");
        assert!(
            text.contains("rationale: keeps the popup answering one question at a time"),
            "{text}"
        );
        assert!(
            text.contains("source session: sess_01AAAAAAAAAAAAAAAAAAAAAAAA"),
            "{text}"
        );
        assert!(text.contains("source commit: d34db33f"), "{text}");
        assert!(text.contains("lifecycle: active"), "{text}");
    }

    /// Map line 1105's honesty half: a memory recorded with no rationale, no
    /// source session and no source commit says so in the detail popup
    /// rather than rendering an empty field or fabricating one — the same
    /// rule `knowledge_line`'s absent-supersession case follows for line
    /// 1106.
    #[test]
    fn the_memory_detail_view_says_so_honestly_when_a_field_is_absent() {
        let mut state = ShellState::new("glasshouse", "/work", "0.1.0", vec![lone_session()]);
        state.handle_key(KeyEvent::new(KeyCode::Char('k'), KeyModifiers::NONE));
        state.open_project_knowledge(
            KnowledgeSection {
                lines: vec!["todo: wire the knowledge view into main".to_owned()],
                details: vec![MemoryDetail {
                    rationale: None,
                    source_session: None,
                    source_commit: None,
                    lifecycle: "active".to_owned(),
                }],
                omitted: 0,
            },
            KnowledgeSection::default(),
            KnowledgeSection::default(),
            KnowledgeSection::default(),
            KnowledgeSection::default(),
            None,
        );

        state.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

        let text = rendered(&state, 120, 40);
        assert!(text.contains("rationale: not recorded"), "{text}");
        assert!(text.contains("source session: not recorded"), "{text}");
        assert!(text.contains("source commit: not recorded"), "{text}");
        assert!(
            !text.contains("rationale: \n") && !text.contains("rationale: source"),
            "an absent rationale must never render as an empty or run-together field:\n{text}"
        );
    }

    /// Esc closes the detail popup and returns to the entry list, leaving
    /// the cursor where it was rather than closing the whole overlay — the
    /// same "close the innermost thing first" shape
    /// `handle_overview_entry_key`'s own Esc arm follows for the send
    /// field.
    #[test]
    fn esc_closes_the_memory_detail_popup_without_closing_the_knowledge_view() {
        let mut state = ShellState::new("glasshouse", "/work", "0.1.0", vec![lone_session()]);
        state.handle_key(KeyEvent::new(KeyCode::Char('k'), KeyModifiers::NONE));
        state.open_project_knowledge(
            one_entry("decision: adopt the drill-down view"),
            KnowledgeSection::default(),
            KnowledgeSection::default(),
            KnowledgeSection::default(),
            KnowledgeSection::default(),
            None,
        );

        state.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert!(rendered(&state, 120, 40).contains("memory detail"));

        state.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        let text = rendered(&state, 120, 40);
        assert!(text.contains("project knowledge"), "{text}");
        assert!(text.contains("adopt the drill-down view"), "{text}");
    }

    /// Map line 234, through the production key path: pressing `M` opens the
    /// project-memory view — the same assert-the-premise-first shape
    /// (practice §17) `the_project_knowledge_view_shows_active_decisions_in_their_own_section`
    /// uses, checked here first as "not open before the press" — and it
    /// shows a record's kind and status on the line, the one thing
    /// `ProjectKnowledge`'s curated sections never have to say because their
    /// section membership already implies both.
    #[test]
    fn the_m_key_opens_the_project_memory_view_showing_kind_and_status() {
        let mut state = ShellState::new("glasshouse", "/work", "0.1.0", vec![lone_session()]);
        assert_eq!(
            state.overlay(),
            None,
            "must not already be open before the key is pressed"
        );

        state.handle_key(KeyEvent::new(KeyCode::Char('M'), KeyModifiers::NONE));
        state.open_project_memory(
            one_entry("[active] finding: the local gate must run alone"),
            None,
        );

        assert_eq!(state.overlay(), Some(Overlay::ProjectMemory));
        let text = rendered(&state, 120, 40);
        assert!(text.contains("finding:"), "{text}");
        assert!(text.contains("[active]"), "{text}");
        assert!(text.contains("the local gate must run alone"), "{text}");
    }

    /// Map line 234's empty-state half: a project with nothing recorded says
    /// so honestly rather than rendering an empty list — the same rule
    /// `the_project_knowledge_view_says_so_when_every_section_is_empty`
    /// proves for its sibling.
    #[test]
    fn the_project_memory_view_says_so_when_there_is_nothing_recorded() {
        let mut state = ShellState::new("glasshouse", "/work", "0.1.0", vec![lone_session()]);
        state.handle_key(KeyEvent::new(KeyCode::Char('M'), KeyModifiers::NONE));
        state.open_project_memory(KnowledgeSection::default(), None);

        let text = rendered(&state, 120, 40);
        assert!(
            text.contains("no memory recorded for this project"),
            "{text}"
        );
    }

    /// A project-memory-view read failure still opens the overlay with an
    /// honest note — the same contract
    /// `a_project_knowledge_read_failure_still_opens_with_an_honest_note`
    /// keeps for its sibling, and never fails the shell.
    #[test]
    fn a_project_memory_view_read_failure_still_opens_with_an_honest_note() {
        let mut state = ShellState::new("glasshouse", "/work", "0.1.0", vec![lone_session()]);
        state.handle_key(KeyEvent::new(KeyCode::Char('M'), KeyModifiers::NONE));
        state.open_project_memory(
            KnowledgeSection::default(),
            Some("project memory unavailable: disk full".to_owned()),
        );

        let text = rendered(&state, 120, 40);
        assert!(
            text.contains("project memory unavailable: disk full"),
            "{text}"
        );
    }

    /// Esc closes the project-memory detail popup and returns to the entry
    /// list without closing the overlay — the same shape
    /// `esc_closes_the_memory_detail_popup_without_closing_the_knowledge_view`
    /// proves for `ProjectKnowledge`.
    #[test]
    fn esc_closes_the_project_memory_detail_popup_without_closing_the_view() {
        let mut state = ShellState::new("glasshouse", "/work", "0.1.0", vec![lone_session()]);
        state.handle_key(KeyEvent::new(KeyCode::Char('M'), KeyModifiers::NONE));
        state.open_project_memory(one_entry("[active] decision: adopt the memory view"), None);

        state.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert!(rendered(&state, 120, 40).contains("memory detail"));

        state.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        let text = rendered(&state, 120, 40);
        assert!(text.contains("project memory"), "{text}");
        assert!(text.contains("adopt the memory view"), "{text}");
    }

    /// Map line 1107, the same rule
    /// `the_project_knowledge_view_renders_no_decorative_graph_glyphs`
    /// proves for its sibling: plain text, no decorative node graph, at a
    /// realistic width and a wide one (practice §17).
    #[test]
    fn the_project_memory_view_renders_no_decorative_graph_glyphs() {
        let mut state = ShellState::new("glasshouse", "/work", "0.1.0", vec![lone_session()]);
        state.handle_key(KeyEvent::new(KeyCode::Char('M'), KeyModifiers::NONE));
        state.open_project_memory(one_entry("[active] finding: no cycles here"), None);

        let graph_glyphs = [
            '●', '○', '◆', '◇', '■', '□', '▲', '▼', '→', '←', '↑', '↓', '↔', '↕',
        ];
        for (width, height) in [(120, 40), (400, 60)] {
            let text = rendered(&state, width, height);
            for glyph in graph_glyphs {
                assert!(
                    !text.contains(glyph),
                    "map line 1107: no decorative graph glyph `{glyph}`, width {width}:\n{text}"
                );
            }
        }
    }

    /// Map line 234's keyboard-reachability half: a keyboard-reachable view
    /// nobody is told about is not reachable in the sense the line means, so
    /// the footer must advertise `M`.
    #[test]
    fn the_footer_advertises_the_project_memory_key() {
        let state = sample();
        let bottom = last_row(&state, 132, 24);
        assert!(bottom.contains("M memory"), "bindings missing: `{bottom}`");
    }

    /// Map line 1768: the project overview must never show a lifetime token
    /// or spend total, and never present one as an achievement counter.
    ///
    /// Practice §17: a settings test once passed for the wrong reason
    /// because a narrow render clipped the very value it asserted was
    /// absent, so this is proven at a realistic width *and* a wide one, and
    /// mutation-proofed by hand: a temporary line pushed into
    /// `render_project_overview` reading `lines.push(Line::from("lifetime
    /// tokens: 999999"))` made this fail at both widths before it was
    /// reverted (see the evidence ledger for the run).
    #[test]
    fn the_project_overview_never_shows_a_lifetime_token_or_spend_total() {
        let mut orchestrator = record("orchestrator", "claude-code", SessionLifecycle::Running);
        orchestrator.role = SessionRole::Orchestrator;
        let mut worker = record("run-worker01", "codex", SessionLifecycle::Running);
        worker.role = SessionRole::Worker;

        let mut state = ShellState::new("glasshouse", "/work", "0.1.0", vec![orchestrator, worker]);
        state.handle_key(KeyEvent::new(KeyCode::Char('p'), KeyModifiers::NONE));
        state.open_project_overview(
            vec!["decision: ship the six closeable lines".to_owned()],
            vec!["todo: close the rest next round".to_owned()],
            2,
            Vec::new(),
            String::new(),
            None,
        );

        for (width, height) in [(120, 40), (400, 40)] {
            let text = rendered(&state, width, height).to_lowercase();
            for forbidden in ["token", "spend", "achievement", "lifetime"] {
                assert!(
                    !text.contains(forbidden),
                    "the project overview must never show `{forbidden}`, width {width}:\n{text}"
                );
            }
        }
    }

    /// Map line 1768's "never" as a property of the source, not only of one
    /// fixture's render: a future PR could add a lifetime counter under
    /// session-state most existing fixtures never populate. Scoped to
    /// `render_project_overview` specifically — `session_detail`'s
    /// per-session model/backend line and Settings' own per-decision
    /// "Maximum marginal cost" knob are both legitimate and must not trip
    /// this.
    ///
    /// # Scanned by lines, deliberately
    ///
    /// Same idiom as `shell::mod::run_loop_passes_the_default_timeouts`: a
    /// multi-line literal search breaks on a CRLF checkout because Git
    /// converts line endings and the literal no longer matches, and that has
    /// already taken Windows CI red once on this project. [`str::lines`]
    /// strips the carriage return, so this is CRLF-agnostic by construction.
    fn project_overview_never_names_a_lifetime_total(source: &str) -> bool {
        let production = source.split("#[cfg(test)]").next().unwrap_or(source);
        let lines: Vec<&str> = production.lines().collect();
        let Some(start) = lines
            .iter()
            .position(|line| line.trim_start().starts_with("fn render_project_overview("))
        else {
            return false;
        };
        let body: Vec<&str> = lines[start + 1..]
            .iter()
            .take_while(|line| !line.trim_start().starts_with("fn "))
            .copied()
            .collect();

        const FORBIDDEN: [&str; 4] = ["token", "spend", "achievement", "lifetime"];
        !body.iter().any(|line| {
            let lower = line.to_lowercase();
            FORBIDDEN.iter().any(|term| lower.contains(term))
        })
    }

    /// The control that keeps the scan above honest — both that it says yes
    /// on the real file and that it is capable of saying no, in both an LF
    /// and a CRLF checkout.
    #[test]
    fn the_lifetime_total_scan_is_crlf_agnostic_and_can_say_no() {
        let normalised = include_str!("view.rs").replace("\r\n", "\n");
        let crlf = normalised.replace('\n', "\r\n");
        assert!(
            project_overview_never_names_a_lifetime_total(&normalised),
            "the real file must pass its own guard"
        );
        assert!(
            project_overview_never_names_a_lifetime_total(&crlf),
            "the scan must still pass in a CRLF checkout"
        );

        let tainted = "fn render_project_overview(x: i32) {\n    let lifetime_tokens = 1;\n}\n";
        assert!(
            !project_overview_never_names_a_lifetime_total(tainted),
            "a source naming a lifetime total inside the function must be rejected"
        );

        // And a term appearing *outside* the function — Settings' own "cost"
        // knob, say — must not trip it, or the guard is too broad to survive
        // the rest of this file.
        let outside = "fn render_project_overview(x: i32) {\n    let y = 1;\n}\n\
                        fn render_settings() {\n    let cost = 1;\n}\n";
        assert!(
            project_overview_never_names_a_lifetime_total(outside),
            "a term outside render_project_overview must not trip the guard"
        );
    }

    /// Map line 1664: the overview must be derived state, never generated
    /// commentary — proven the same way `nothing_draws_with_block_elements`
    /// proves the rest of the shell stays text-first: every character in the
    /// popup traces to a session field or a memory string the run loop
    /// handed in, never a phrase invented at render time beyond the fixed,
    /// literal section headings and empty-state notes already asserted
    /// above.
    #[test]
    fn the_project_overview_footer_names_its_own_key() {
        let mut state = ShellState::new("glasshouse", "/work", "0.1.0", vec![lone_session()]);
        state.handle_key(KeyEvent::new(KeyCode::Char('p'), KeyModifiers::NONE));
        state.open_project_overview(Vec::new(), Vec::new(), 0, Vec::new(), String::new(), None);
        let text = rendered(&state, 120, 40);
        assert!(
            text.contains("esc back to session"),
            "project overview footer:\n{text}"
        );

        let control_text = rendered(&sample(), 120, 24);
        assert!(
            control_text.contains("p project"),
            "control-mode footer must advertise the key:\n{control_text}"
        );
    }

    /// Map line 1758. Practice §17: asserted at a realistic width and at 400
    /// columns, exactly like `the_overview_shows_recorded_activity_...`,
    /// because a row clipped off-screen at 100 columns would make the
    /// `contains` assertions below pass or fail for reasons that have
    /// nothing to do with the code.
    ///
    /// Also proves the filter: `sample()`'s two sessions each get an event,
    /// and only the *presented* one's (`aaaaaaaaaaaa1`, `sample()`'s default
    /// `selected_index`) text must appear — the other session's event text
    /// must not, which is what distinguishes this overlay from
    /// `render_overview`'s cross-session ACTIVITY feed.
    #[test]
    fn session_events_shows_only_the_presented_sessions_events_at_a_realistic_and_a_wide_width() {
        use crate::events::{EventBus, LifecycleEvent, MessageOrigin, TurnOutcome};
        use crate::session::SessionId;
        use crate::shell::Action;

        let mut state = sample();
        let bus = EventBus::new();
        let presented = SessionId::new("aaaaaaaaaaaa1");
        let other = SessionId::new("bbbbbbbbbbbb2");
        let recorded = vec![
            bus.publish(&presented, LifecycleEvent::SessionStarted),
            bus.publish(
                &other,
                LifecycleEvent::TextDelivered {
                    origin: MessageOrigin::Machine,
                    bytes: 99,
                },
            ),
            bus.publish(
                &presented,
                LifecycleEvent::TurnEnded {
                    outcome: TurnOutcome::Completed,
                },
            ),
        ];
        assert_eq!(state.note_events(&recorded), Action::Redraw);
        assert_eq!(
            state.handle_key(KeyEvent::new(KeyCode::Char('e'), KeyModifiers::NONE)),
            Action::Redraw
        );

        for (width, height) in [(100, 24), (400, 24)] {
            let text = rendered(&state, width, height);
            assert!(text.contains("session events"), "width {width}:\n{text}");
            assert!(text.contains("session started"), "width {width}:\n{text}");
            assert!(
                text.contains("turn ended (completed)"),
                "width {width}:\n{text}"
            );
            assert!(
                !text.contains("sent 99 bytes (machine)"),
                "the other session's event must not appear, width {width}:\n{text}"
            );
        }
    }

    /// The honest empty state, matching `render_overview`'s own convention
    /// for a section with nothing to show.
    #[test]
    fn session_events_says_so_when_the_presented_session_has_none() {
        let mut state = sample();
        state.handle_key(KeyEvent::new(KeyCode::Char('e'), KeyModifiers::NONE));

        let text = rendered(&state, 120, 24);
        assert!(
            text.contains("no recent lifecycle events recorded for this session"),
            "{text}"
        );
    }

    /// Map line 1770 for this overlay specifically: reached only by its own
    /// key, never present on the screen a user sees without asking for it.
    /// Asserted at both widths for the same reason as the test above — an
    /// absence assertion is only as strong as the viewport it renders into
    /// (practice §17).
    #[test]
    fn session_events_is_absent_from_the_default_screen_at_a_realistic_and_a_wide_width() {
        let state = sample();
        for (width, height) in [(100, 24), (400, 24)] {
            let text = rendered(&state, width, height);
            assert!(
                !text.contains("session events"),
                "the default screen must not show the session-events overlay, \
                 width {width}:\n{text}"
            );
        }
    }

    /// The overlay's own footer, and the control-mode footer advertising the
    /// key that opens it — the same pair `the_project_overview_footer_...`
    /// proves for `p`/`Overlay::ProjectOverview`.
    #[test]
    fn the_session_events_footer_names_its_own_key() {
        let mut state = sample();
        state.handle_key(KeyEvent::new(KeyCode::Char('e'), KeyModifiers::NONE));
        let text = rendered(&state, 120, 24);
        assert!(
            text.contains("esc back to session"),
            "session events footer:\n{text}"
        );

        let control_text = rendered(&sample(), 120, 24);
        assert!(
            control_text.contains("e events"),
            "control-mode footer must advertise the key:\n{control_text}"
        );
    }

    /// `e` toggles like every other overlay key: pressing it again while open
    /// closes it, exactly as `o`/`Overlay::Overview` and `p`/
    /// `Overlay::ProjectOverview` already do. A regression here would leave
    /// the key un-openable a second time in the same session.
    #[test]
    fn e_opens_and_esc_closes_the_session_events_overlay() {
        use crate::shell::Action;

        let mut state = sample();
        assert_eq!(
            state.handle_key(KeyEvent::new(KeyCode::Char('e'), KeyModifiers::NONE)),
            Action::Redraw
        );
        assert_eq!(
            state.overlay(),
            Some(crate::shell::state::Overlay::SessionEvents)
        );
        assert_eq!(
            state.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)),
            Action::Redraw
        );
        assert_eq!(state.overlay(), None);
    }

    fn route_row(
        provider: &str,
        model: &str,
        route: Option<&str>,
        context_state: &str,
        sample_count: usize,
        window_start_unix: i64,
        window_end_unix: i64,
    ) -> crate::shell::state::RouteEvidenceRow {
        crate::shell::state::RouteEvidenceRow {
            provider: provider.to_owned(),
            model: model.to_owned(),
            route: route.map(str::to_owned),
            context_state: context_state.to_owned(),
            sample_count,
            window_start_unix,
            window_end_unix,
        }
    }

    /// Acceptance test 4: the table renders sample count and window from
    /// real recorded data, and two identities with different counts render
    /// differently.
    #[test]
    fn the_route_evidence_table_renders_sample_count_and_window_and_distinguishes_identities() {
        let mut state = sample();
        state.handle_key(KeyEvent::new(KeyCode::Char('r'), KeyModifiers::NONE));
        let now = crate::provider::cache::now_unix_seconds();
        state.open_route_evidence(
            vec![
                route_row(
                    "anyrouter",
                    "claude-opus-4-1",
                    Some("anthropic-messages"),
                    "unknown",
                    5,
                    now - 3_600,
                    now - 60,
                ),
                route_row(
                    "openai-router",
                    "gpt-5",
                    None,
                    "unknown",
                    1,
                    now - 30,
                    now - 30,
                ),
            ],
            None,
        );

        let text = rendered(&state, 120, 24);
        assert!(text.contains("anyrouter"), "{text}");
        assert!(text.contains("claude-opus-4-1"), "{text}");
        assert!(text.contains("anthropic-messages"), "{text}");
        assert!(text.contains('5'), "{text}");
        assert!(text.contains("openai-router"), "{text}");
        assert!(text.contains("gpt-5"), "{text}");
        assert!(
            text.contains("(no route)"),
            "an identity with no recorded route must say so honestly:\n{text}"
        );

        let rows: Vec<&str> = text
            .lines()
            .filter(|line| line.contains("anyrouter") || line.contains("openai-router"))
            .collect();
        assert_eq!(rows.len(), 2);
        assert_ne!(
            rows[0], rows[1],
            "two identities with different counts and windows must render differently:\n{text}"
        );
    }

    /// Acceptance test 5, capability map line 1764: an `Unknown` row renders
    /// as `unknown`, neither omitted nor dressed up as a measurement.
    #[test]
    fn the_route_evidence_table_renders_unknown_context_state_plainly() {
        let mut state = sample();
        state.handle_key(KeyEvent::new(KeyCode::Char('r'), KeyModifiers::NONE));
        state.open_route_evidence(
            vec![route_row(
                "anyrouter",
                "m",
                Some("anthropic-messages"),
                "unknown",
                5,
                1_000,
                1_000,
            )],
            None,
        );

        let text = rendered(&state, 120, 24);
        assert!(text.contains("unknown"), "{text}");
    }

    /// Acceptance test 6, and practice §17: the rendered table names no
    /// TTFC/TTFT/throughput/rounds-per-minute column, at a viewport wide
    /// enough that such a column *would* have been visible rather than
    /// clipped off-screen for the wrong reason.
    #[test]
    fn no_fabricated_columns_appear_in_the_route_evidence_table() {
        let mut state = sample();
        state.handle_key(KeyEvent::new(KeyCode::Char('r'), KeyModifiers::NONE));
        state.open_route_evidence(
            vec![route_row(
                "anyrouter",
                "claude-opus-4-1",
                Some("anthropic-messages"),
                "unknown",
                5,
                1_000,
                1_050,
            )],
            None,
        );

        for (width, height) in [(120, 24), (400, 30)] {
            let text = rendered(&state, width, height).to_lowercase();
            for forbidden in [
                "ttfc",
                "ttft",
                "throughput",
                "rounds per minute",
                "rounds/min",
                "decode",
            ] {
                assert!(
                    !text.contains(forbidden),
                    "map line 1762: no fabricated `{forbidden}` column, width {width}:\n{text}"
                );
            }
        }
    }

    /// Acceptance test 7, empty half: an empty ledger renders an honest
    /// empty state, the same convention every other overlay section here
    /// uses.
    #[test]
    fn the_route_evidence_table_says_so_when_there_is_no_evidence_yet() {
        let mut state = sample();
        state.handle_key(KeyEvent::new(KeyCode::Char('r'), KeyModifiers::NONE));
        state.open_route_evidence(Vec::new(), None);

        let text = rendered(&state, 120, 24);
        assert!(text.contains("no routing evidence recorded yet"), "{text}");
    }

    // -----------------------------------------------------------------
    // The routing-decisions overlay — the reader half of the disposable
    // routing sink. Every test here hands the view a row it invented, so
    // none of them says anything about whether the run loop reads the
    // ledger; that is `tests/disposable_route_sink.rs`'s job, and practice
    // §35 is why the split is deliberate rather than an omission.
    // -----------------------------------------------------------------

    fn decision_row(
        job: &str,
        session: Option<&str>,
        rationale: Option<&str>,
        observed_at_unix: i64,
    ) -> crate::shell::state::RouteDecisionRow {
        crate::shell::state::RouteDecisionRow {
            observed_at_unix,
            job: job.to_owned(),
            session_id: session.map(str::to_owned),
            rationale: rationale.map(str::to_owned),
        }
    }

    /// The whole stored rationale reaches the screen — the heading *and* the
    /// named contributions under it.
    ///
    /// The heading alone would be a view that says which resource won and
    /// not one reason it did, which is the shape map line 1766 asks this not
    /// to be. Asserted at a wide viewport too, per practice §17: a
    /// contribution line is long, and a match that only survives at 400
    /// columns is a layout finding rather than a rendering one.
    #[test]
    fn the_routing_decisions_view_draws_the_whole_stored_rationale() {
        let mut state = sample();
        state.handle_key(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE));
        state.open_route_decisions(
            vec![decision_row(
                "memory extraction",
                Some("session-abc"),
                Some(
                    "a-free-model on a-provider — free, used by user preference\n  \
                     +1.000  cost — free — line 530 prefers free capacity\n  \
                     +0.000  user pin — the user pinned this exact free resource",
                ),
                1_000,
            )],
            None,
        );

        for (width, height) in [(120, 30), (400, 30)] {
            let text = rendered(&state, width, height);
            assert!(text.contains("memory extraction"), "width {width}:\n{text}");
            assert!(text.contains("session-abc"), "width {width}:\n{text}");
            assert!(text.contains("a-free-model"), "width {width}:\n{text}");
            assert!(
                text.contains("user preference"),
                "the reason the policy gave must be on screen, width {width}:\n{text}"
            );
            assert!(
                text.contains("line 530 prefers free capacity"),
                "a decision drawn without its contributions is an outcome, not a \
                 rationale, width {width}:\n{text}"
            );
            assert!(
                text.contains("user pin"),
                "every contribution is drawn, not only the first, width {width}:\n{text}"
            );
        }
    }

    /// A row the producer could not fill says so, at both widths — practice
    /// §17, and map line 1294's rule that an absent value is drawn as absent
    /// rather than as an empty column a reader would take for a value.
    #[test]
    fn a_routing_decision_with_nothing_recorded_says_so_rather_than_drawing_a_blank() {
        let mut state = sample();
        state.handle_key(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE));
        state.open_route_decisions(
            vec![decision_row("memory extraction", None, None, 1_000)],
            None,
        );

        for (width, height) in [(120, 24), (400, 24)] {
            let text = rendered(&state, width, height);
            assert!(
                text.contains("(no session recorded)"),
                "width {width}:\n{text}"
            );
            assert!(
                text.contains("(no rationale recorded)"),
                "width {width}:\n{text}"
            );
        }
    }

    /// The empty half: a project that has recorded no decision is told so,
    /// which is the honest and most common answer rather than a failure.
    #[test]
    fn the_routing_decisions_view_says_so_when_nothing_is_recorded() {
        let mut state = sample();
        state.handle_key(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE));
        state.open_route_decisions(Vec::new(), None);

        let text = rendered(&state, 120, 24);
        assert!(
            text.contains("no routing decision has been recorded yet"),
            "{text}"
        );
    }

    /// The failure half: an unreadable ledger still opens the overlay with an
    /// honest note, the same contract
    /// `a_route_evidence_read_failure_still_opens_with_an_honest_note` proves
    /// for its own view.
    #[test]
    fn a_routing_decisions_read_failure_still_opens_with_an_honest_note() {
        let mut state = sample();
        state.handle_key(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE));
        state.open_route_decisions(
            Vec::new(),
            Some("routing decisions unavailable: disk full".to_owned()),
        );

        let text = rendered(&state, 120, 24);
        assert!(
            text.contains("routing decisions unavailable: disk full"),
            "{text}"
        );
    }

    /// Reached only by its own key, never on the screen a user sees without
    /// asking — asserted at both widths per practice §17.
    #[test]
    fn routing_decisions_are_absent_from_the_default_screen_at_a_realistic_and_a_wide_width() {
        let state = sample();
        for (width, height) in [(100, 24), (400, 24)] {
            let text = rendered(&state, width, height);
            assert!(
                !text.contains("routing decisions"),
                "the default screen must not show the routing-decisions overlay, \
                 width {width}:\n{text}"
            );
        }
    }

    /// The overlay's own footer, and the control-mode footer advertising `d`
    /// — the same pair `the_route_evidence_footer_names_its_own_key` proves.
    #[test]
    fn the_routing_decisions_footer_names_its_own_key() {
        let mut state = sample();
        state.handle_key(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE));
        state.open_route_decisions(Vec::new(), None);
        let text = rendered(&state, 120, 24);
        assert!(
            text.contains("esc back to session"),
            "routing decisions footer:\n{text}"
        );

        // 156 for the reason `the_status_bar_always_shows_the_key_bindings`
        // records: the control row is exactly 154 columns now.
        let control_text = rendered(&sample(), 156, 24);
        assert!(
            control_text.contains("d decisions"),
            "control-mode footer must advertise the key:\n{control_text}"
        );
    }

    /// Acceptance test 7, failure half: a read failure still opens the
    /// overlay with an honest note — the same contract
    /// `a_project_knowledge_read_failure_still_opens_with_an_honest_note`
    /// proves for the project-knowledge view.
    #[test]
    fn a_route_evidence_read_failure_still_opens_with_an_honest_note() {
        let mut state = sample();
        state.handle_key(KeyEvent::new(KeyCode::Char('r'), KeyModifiers::NONE));
        state.open_route_evidence(
            Vec::new(),
            Some("routing evidence unavailable: disk full".to_owned()),
        );

        let text = rendered(&state, 120, 24);
        assert!(
            text.contains("routing evidence unavailable: disk full"),
            "{text}"
        );
    }

    /// Map line 1770 for this overlay specifically: reached only by its own
    /// key, never present on the screen a user sees without asking for it —
    /// asserted at both widths per practice §17.
    #[test]
    fn route_evidence_is_absent_from_the_default_screen_at_a_realistic_and_a_wide_width() {
        let state = sample();
        for (width, height) in [(100, 24), (400, 24)] {
            let text = rendered(&state, width, height);
            assert!(
                !text.contains("route evidence"),
                "the default screen must not show the route-evidence overlay, \
                 width {width}:\n{text}"
            );
        }
    }

    /// The overlay's own footer, and the control-mode footer advertising the
    /// key that opens it — the same pair `the_session_events_footer_...`
    /// proves for `e`/`Overlay::SessionEvents`.
    #[test]
    fn the_route_evidence_footer_names_its_own_key() {
        let mut state = sample();
        state.handle_key(KeyEvent::new(KeyCode::Char('r'), KeyModifiers::NONE));
        state.open_route_evidence(Vec::new(), None);
        let text = rendered(&state, 120, 24);
        assert!(
            text.contains("esc back to session"),
            "route evidence footer:\n{text}"
        );

        let control_text = rendered(&sample(), 120, 24);
        assert!(
            control_text.contains("r routes"),
            "control-mode footer must advertise the key:\n{control_text}"
        );
    }

    /// `r` toggles like every other overlay key: pressing it again while open
    /// closes it, exactly as `e`/`Overlay::SessionEvents` already does.
    #[test]
    fn r_opens_and_esc_closes_the_route_evidence_overlay() {
        use crate::shell::Action;

        let mut state = sample();
        state.handle_key(KeyEvent::new(KeyCode::Char('r'), KeyModifiers::NONE));
        state.open_route_evidence(Vec::new(), None);
        assert_eq!(
            state.overlay(),
            Some(crate::shell::state::Overlay::RouteEvidence)
        );

        assert_eq!(
            state.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)),
            Action::Redraw
        );
        assert_eq!(state.overlay(), None);
    }

    // -----------------------------------------------------------------
    // Phase 47 line 1765 — route health, immediate availability, cadence,
    // quota reset and failure-domain evidence as SEPARATE concepts.
    // -----------------------------------------------------------------

    /// The rendered screen with the popup's own border columns dropped and
    /// runs of whitespace collapsed, so a phrase the `Wrap` broke across two
    /// rows can still be asserted on.
    ///
    /// Needed because line 1765 wants five *labelled* concepts and the labels
    /// plus their evidence are longer than a realistic popup is wide. The
    /// per-line assertions below deliberately use the **raw** text instead —
    /// "these two concepts are not on the same line" is a claim about lines,
    /// and flattening would make it unfalsifiable.
    fn flattened(text: &str) -> String {
        text.replace('│', " ")
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
    }

    /// A healthy, available, entirely unstated resource — the state a fresh
    /// installation actually observes. Every test below starts here and
    /// overrides only the fields its own case is about, with struct-update
    /// syntax, so what a case is *testing* is visible at its call site.
    fn health_row(provider: &str, model: &str) -> crate::shell::state::RouteHealthRow {
        crate::shell::state::RouteHealthRow {
            provider: provider.to_owned(),
            credential_label: format!("{provider}/API_KEY"),
            model: model.to_owned(),
            consecutive_failures: 0,
            credential_rejected: false,
            available_now: true,
            cooling_down_until_unix: None,
            stated_limit: None,
            stated_window_seconds: None,
            quota_resets_at_unix: None,
            failure_domain: "unknown".to_owned(),
            failure_domain_peers: 0,
        }
    }

    /// **Line 1765's whole content.** Five labelled concepts, five separate
    /// lines, for a resource where they genuinely disagree: healthy (zero
    /// failures) yet unavailable (credential refused), paced by Glasshouse
    /// while the provider's own reset is at a different time.
    ///
    /// Rendered at a realistic width and a wide one (practice §17), because a
    /// label that happened to clip off-screen would make this pass for the
    /// wrong reason.
    #[test]
    fn route_health_keeps_line_1765s_five_concepts_on_separate_lines() {
        let now = crate::provider::cache::now_unix_seconds();
        for (width, height) in [(120, 40), (400, 40)] {
            let mut state = sample();
            state.handle_key(KeyEvent::new(KeyCode::Char('h'), KeyModifiers::NONE));
            // Never failed, yet unavailable; paced by Glasshouse until a
            // different instant from the provider's own reset.
            state.open_route_health(vec![crate::shell::state::RouteHealthRow {
                credential_rejected: true,
                available_now: false,
                cooling_down_until_unix: Some(now + 300),
                stated_limit: Some(300),
                stated_window_seconds: Some(60),
                quota_resets_at_unix: Some(now + 1_800),
                failure_domain: "shared".to_owned(),
                failure_domain_peers: 1,
                ..health_row("anyrouter", "claude-opus-4-1")
            }]);
            let text = rendered(&state, width, height);

            for concept in [
                "route health",
                "immediate availability",
                "cadence",
                "quota reset",
                "failure domain",
            ] {
                assert!(
                    text.contains(concept),
                    "line 1765 names `{concept}` and it must be its own labelled \
                     concept, width {width}:\n{text}"
                );
            }

            // Each concept on its own line: no line may carry two of the five
            // labels, which is exactly what collapsing them would produce.
            for line in text.lines() {
                let found = [
                    "route health",
                    "immediate availability",
                    "cadence",
                    "quota reset",
                    "failure domain",
                ]
                .iter()
                .filter(|concept| line.contains(*concept))
                .count();
                assert!(
                    found <= 1,
                    "two of line 1765's concepts were folded onto one line, \
                     width {width}:\n{line}"
                );
            }

            // And the five really do disagree here, which is the point: a
            // single status word could not have carried all of this.
            let flat = flattened(&text);
            assert!(
                flat.contains("0 consecutive failure(s)"),
                "the failure streak must be shown as a streak, width {width}:\n{text}"
            );
            assert!(
                flat.contains("credential rejected: yes"),
                "a refused credential is a health fact of its own, \
                 width {width}:\n{text}"
            );
            assert!(
                flat.contains("not schedulable right now"),
                "availability must be its own answer, width {width}:\n{text}"
            );
            assert!(
                flat.contains("300 request(s) per 60s"),
                "the provider-stated cadence must be shown, width {width}:\n{text}"
            );
            assert!(
                flat.contains("cooling down, ends in 5 minutes"),
                "glasshouse's own pacing is a separate clock from the \
                 provider's, width {width}:\n{text}"
            );
        }
    }

    /// **The honesty half.** A provider that has stated no rate-limit headers
    /// at all — which is most of them — must read `unknown`, never `0` and
    /// never an invented reset. Line 1765 sits under a phase whose whole
    /// heading is about not presenting a number the evidence does not carry.
    #[test]
    fn route_health_says_unknown_rather_than_zero_for_what_no_provider_stated() {
        let mut state = sample();
        state.handle_key(KeyEvent::new(KeyCode::Char('h'), KeyModifiers::NONE));
        // The default is the entirely-unstated case, which is the point.
        state.open_route_health(vec![health_row("openrouter", "some-free-model")]);
        let text = rendered(&state, 120, 40);
        let flat = flattened(&text);

        assert!(
            flat.contains("quota reset unknown — no response has stated one"),
            "an unstated quota reset must read `unknown`:\n{text}"
        );
        assert!(
            flat.contains("provider stated: unknown"),
            "an unstated cadence must read `unknown`:\n{text}"
        );
        assert!(
            flat.contains("glasshouse pacing: none"),
            "no cooldown must read `none`, not an elapsed deadline:\n{text}"
        );
        // The three unstated concepts must not have been filled in with a
        // number: `0` would read as a measurement, which is the whole of what
        // this phase is named after not doing.
        for invented in ["quota reset in", "per 0s", "0 request(s)"] {
            assert!(
                !flat.contains(invented),
                "an unstated value was rendered as `{invented}`:\n{text}"
            );
        }
    }

    /// `crate::routing::domain::FailureDomain::Independent` is a state this
    /// build cannot earn — nothing does the temporal correlation it would
    /// need — so no fixture, and no future edit, may make this view print it.
    /// Proved at a wide viewport per practice §17, because an absence
    /// assertion is only as strong as the screen it renders into.
    #[test]
    fn route_health_never_claims_two_resources_are_independent() {
        for (width, height) in [(120, 40), (400, 40)] {
            let mut state = sample();
            state.handle_key(KeyEvent::new(KeyCode::Char('h'), KeyModifiers::NONE));
            state.open_route_health(vec![
                crate::shell::state::RouteHealthRow {
                    consecutive_failures: 2,
                    failure_domain: "shared".to_owned(),
                    failure_domain_peers: 1,
                    ..health_row("anyrouter", "model-a")
                },
                health_row("openrouter", "model-b"),
            ]);
            let flat = flattened(&rendered(&state, width, height));
            assert!(
                flat.contains("never `independent`"),
                "the view must say what the absence of evidence does not mean, \
                 width {width}:\n{flat}"
            );
            // The only permitted occurrence is inside that refusal.
            assert_eq!(
                flat.matches("independent").count(),
                flat.matches("never `independent`").count(),
                "`independent` may appear only inside the sentence refusing it, \
                 width {width}:\n{flat}"
            );
        }
    }

    /// The empty state, which is what a fresh installation actually shows:
    /// honest words, not a table of zeroes.
    #[test]
    fn route_health_says_so_when_no_gateway_exchange_has_been_observed() {
        let mut state = sample();
        state.handle_key(KeyEvent::new(KeyCode::Char('h'), KeyModifiers::NONE));
        state.open_route_health(Vec::new());
        let text = rendered(&state, 120, 30);
        assert!(
            text.contains("no gateway exchange has been observed"),
            "an empty cache must say so:\n{text}"
        );
    }

    /// The scope label, which is a fact about these caches and not decoration:
    /// they live under the installation's data directory, keyed by provider,
    /// so a reading written while a gateway served another project is visible
    /// here. A view that let a reader assume otherwise would be the spectacle
    /// this phase is named after.
    #[test]
    fn route_health_labels_its_own_installation_wide_scope() {
        let mut state = sample();
        state.handle_key(KeyEvent::new(KeyCode::Char('h'), KeyModifiers::NONE));
        state.open_route_health(Vec::new());
        let text = rendered(&state, 120, 30);
        assert!(
            text.contains("not scoped to this project"),
            "the view must name its own scope:\n{text}"
        );
    }

    /// Map line 1770 for this overlay: reached only by its own key, never on
    /// the default screen — at both widths, per practice §17.
    #[test]
    fn route_health_is_absent_from_the_default_screen_at_a_realistic_and_a_wide_width() {
        let state = sample();
        for (width, height) in [(100, 24), (400, 24)] {
            let text = rendered(&state, width, height);
            assert!(
                !text.contains("immediate availability"),
                "the default screen must not show the route-health overlay, \
                 width {width}:\n{text}"
            );
        }
    }

    /// The overlay's own footer, and the control-mode footer advertising `h`.
    #[test]
    fn the_route_health_footer_names_its_own_key() {
        let mut state = sample();
        state.handle_key(KeyEvent::new(KeyCode::Char('h'), KeyModifiers::NONE));
        state.open_route_health(Vec::new());
        let text = rendered(&state, 132, 24);
        assert!(
            text.contains("esc back to session"),
            "route health footer:\n{text}"
        );

        let control_text = rendered(&sample(), 132, 24);
        assert!(
            control_text.contains("h health"),
            "control-mode footer must advertise the key:\n{control_text}"
        );
    }

    /// `h` opens it and `esc` closes it, the same toggle every other overlay
    /// key already has.
    #[test]
    fn h_opens_and_esc_closes_the_route_health_overlay() {
        use crate::shell::Action;

        let mut state = sample();
        assert_eq!(
            state.handle_key(KeyEvent::new(KeyCode::Char('h'), KeyModifiers::NONE)),
            Action::OpenRouteHealth
        );
        state.open_route_health(Vec::new());
        assert_eq!(
            state.overlay(),
            Some(crate::shell::state::Overlay::RouteHealth)
        );

        assert_eq!(
            state.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)),
            Action::Redraw
        );
        assert_eq!(state.overlay(), None);
    }
}

#[cfg(test)]
mod settings_tests {
    use crate::config::{
        Layered, PremiumReservePercent, ProfileConfig, ProviderConfig, RouterCostMicroUsd,
        RouterLatencyMs, RoutingModelChoice,
    };
    use crate::integrations::{IntegrationId, IntegrationStatus};
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    use super::super::state::{
        HarnessRow, IntegrationRow, MemoryRow, ProfileRow, ProviderRow, RoutingRow,
    };
    use super::*;

    fn press(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn harness_rows() -> Vec<HarnessRow> {
        vec![
            HarnessRow {
                id: IntegrationId::ClaudeCode,
                detected: true,
                enabled: true,
                enabled_layer: Layer::Project,
                executable: Some("/opt/bin/claude".into()),
                executable_layer: Some(Layer::User),
            },
            HarnessRow {
                id: IntegrationId::Codex,
                detected: false,
                enabled: false,
                enabled_layer: Layer::Default,
                executable: None,
                executable_layer: None,
            },
        ]
    }

    fn integration_rows() -> Vec<IntegrationRow> {
        vec![IntegrationRow {
            id: IntegrationId::Ollama,
            detected: false,
            status: IntegrationStatus::NotFound,
        }]
    }

    fn state_with_settings_open() -> ShellState {
        let mut state = ShellState::new("glasshouse", "/work/glasshouse", "0.1.0", Vec::new());
        state.open_settings(harness_rows(), integration_rows(), Vec::new(), Vec::new());
        state
    }

    fn rendered(state: &ShellState, width: u16, height: u16) -> String {
        let backend = TestBackend::new(width, height);
        let mut terminal = Terminal::new(backend).expect("terminal");
        terminal
            .draw(|frame| render(state, frame))
            .expect("draw must not panic");
        let buffer = terminal.backend().buffer();
        (0..buffer.area.height)
            .map(|y| {
                (0..buffer.area.width)
                    .map(|x| buffer[(x, y)].symbol())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn settings_shows_both_section_labels_and_the_harness_rows() {
        let state = state_with_settings_open();
        let text = rendered(&state, 100, 30);
        assert!(text.contains("Harnesses"), "got:\n{text}");
        assert!(text.contains("Integrations"), "got:\n{text}");
        assert!(text.contains("Claude Code"), "got:\n{text}");
    }

    /// The design decision: "provenance is shown, not inferred" — every
    /// displayed value must carry a layer tag.
    #[test]
    fn every_harness_value_shown_carries_its_layer() {
        let state = state_with_settings_open();
        let text = rendered(&state, 100, 30);
        assert!(text.contains("(project)"), "enabled layer missing:\n{text}");
        assert!(text.contains("(user)"), "executable layer missing:\n{text}");
        assert!(
            text.contains("(default)"),
            "the never-configured row's layer must still show:\n{text}"
        );
    }

    #[test]
    fn switching_to_the_integrations_section_shows_its_rows() {
        let mut state = state_with_settings_open();
        state.handle_key(press(KeyCode::Tab));
        let text = rendered(&state, 100, 30);
        assert!(text.contains("Ollama"), "got:\n{text}");
        assert!(text.contains("not found"), "got:\n{text}");
    }

    #[test]
    fn the_path_editor_renders_the_typed_buffer() {
        let mut state = state_with_settings_open();
        state.handle_key(press(KeyCode::Enter));
        for c in "/opt/bin".chars() {
            state.handle_key(press(KeyCode::Char(c)));
        }
        let text = rendered(&state, 100, 30);
        assert!(text.contains("/opt/bin"), "got:\n{text}");
    }

    /// The confirmation must name the exact path the design decision
    /// requires, not a generic "are you sure".
    #[test]
    fn the_project_write_confirmation_names_the_exact_path() {
        let mut state = state_with_settings_open();
        state.handle_key(press(KeyCode::Char('W')));
        let text = rendered(&state, 100, 30);

        // Built with `join`, exactly as `render_project_write_confirmation`
        // builds it. A hard-coded `/work/glasshouse/.glasshouse/config.toml`
        // passed everywhere except Windows, where `join` yields backslashes —
        // a test-portability fault, not a product one, but it turned CI red.
        let expected = std::path::Path::new("/work/glasshouse")
            .join(".glasshouse")
            .join("config.toml");
        let expected = expected.display().to_string();
        assert!(
            text.contains(&expected),
            "the exact path `{expected}` must be shown:\n{text}"
        );
        assert!(
            text.to_lowercase().contains("repository"),
            "must say the file is inside the repository:\n{text}"
        );
    }

    #[test]
    fn the_footer_names_the_settings_bindings() {
        let state = state_with_settings_open();
        let bottom = rendered(&state, 100, 30)
            .lines()
            .last()
            .unwrap()
            .trim_end()
            .to_owned();
        assert!(bottom.contains("save"), "got: `{bottom}`");
        assert!(bottom.contains("toggle"), "got: `{bottom}`");
    }

    #[test]
    fn settings_renders_without_panicking_at_absurd_sizes() {
        let mut state = state_with_settings_open();
        for (w, h) in [(1, 1), (1, 40), (40, 1), (3, 3), (200, 60)] {
            rendered(&state, w, h);
        }
        state.handle_key(press(KeyCode::Enter));
        for (w, h) in [(1, 1), (1, 40), (40, 1), (3, 3)] {
            rendered(&state, w, h);
        }
        state.handle_key(press(KeyCode::Esc));
        state.handle_key(press(KeyCode::Char('W')));
        for (w, h) in [(1, 1), (1, 40), (40, 1), (3, 3)] {
            rendered(&state, w, h);
        }
    }

    /// Same design-first guarantee as the rest of the shell: no decorative
    /// block-element glyphs anywhere in the Settings overlay.
    #[test]
    fn settings_draws_with_no_decorative_block_elements() {
        let mut state = state_with_settings_open();
        let mut screens = vec![rendered(&state, 100, 30)];
        state.handle_key(press(KeyCode::Tab));
        screens.push(rendered(&state, 100, 30));
        state.handle_key(press(KeyCode::BackTab));
        state.handle_key(press(KeyCode::Enter));
        screens.push(rendered(&state, 100, 30));

        for screen in screens {
            if let Some(found) = screen
                .chars()
                .find(|c| ('\u{2580}'..='\u{259F}').contains(c))
            {
                panic!("a decorative block element ({found:?}) was drawn:\n{screen}");
            }
        }
    }

    // -----------------------------------------------------------------
    // Phase 2D: Providers and Launch Profiles.
    // -----------------------------------------------------------------

    fn provider_rows() -> Vec<ProviderRow> {
        let mut config = ProviderConfig::new("openrouter");
        config.set_base_url(Some("https://mirror.example.com/v1".to_owned()));
        vec![ProviderRow::new("my-router", config, Layer::User)]
    }

    fn profile_rows() -> Vec<ProfileRow> {
        vec![ProfileRow {
            name: "fast".to_owned(),
            config: ProfileConfig::new(IntegrationId::ClaudeCode),
            layer: Layer::User,
        }]
    }

    fn state_with_full_settings_open() -> ShellState {
        let mut state = ShellState::new("glasshouse", "/work/glasshouse", "0.1.0", Vec::new());
        state.open_settings(
            harness_rows(),
            integration_rows(),
            provider_rows(),
            profile_rows(),
        );
        state
    }

    /// Routing shows every policy with its own provenance and explains the
    /// conditions on the free-resource preference. Memory shows its one real
    /// setting and layer the same way, and a toggle both changes the value
    /// and promotes its layer to `(user)` — the placeholder "not available"
    /// text this test used to require is the defect Phase 2D line 190 closes.
    #[test]
    fn routing_and_memory_sections_render_their_complete_honest_states() {
        let routing = RoutingRow::new(
            Layered::new(
                RoutingModelChoice::Pinned {
                    provider: "my-router".to_owned(),
                    model: "openai/gpt-5-mini".to_owned(),
                },
                Layer::Project,
            ),
            Layered::new(RouterLatencyMs::try_from(800).unwrap(), Layer::User),
            Layered::new(RouterCostMicroUsd::try_from(2_500).unwrap(), Layer::Default),
            Layered::new(false, Layer::Project),
            Layered::new(PremiumReservePercent::try_from(12).unwrap(), Layer::User),
            vec!["my-router".to_owned()],
        );
        // Premise, per §17: the memory row starts disabled at the project
        // layer, so a later assertion that a toggle changed it to "yes" and
        // `(user)` actually proves the toggle did something.
        let memory = MemoryRow::new(Layered::new(false, Layer::Project));
        let mut state = ShellState::new("glasshouse", "/work/glasshouse", "0.1.0", Vec::new());
        state.open_settings_with_routing(
            harness_rows(),
            integration_rows(),
            provider_rows(),
            profile_rows(),
            routing,
            memory,
        );
        for _ in 0..4 {
            state.handle_key(press(KeyCode::Tab));
        }
        let routing_text = rendered(&state, 120, 32);
        for expected in [
            "my-router:openai/gpt-5-mini",
            "800 ms",
            "$0.002500",
            "health, rate-limit, and latency",
            "below 12%",
            "(project)",
            "(user)",
            "(default)",
        ] {
            assert!(
                routing_text.contains(expected),
                "missing {expected:?}:\n{routing_text}"
            );
        }

        state.handle_key(press(KeyCode::Tab));
        let memory_text = rendered(&state, 120, 32);
        assert!(memory_text.contains("Memory"), "{memory_text}");
        assert!(memory_text.contains("no (project)"), "{memory_text}");

        state.handle_key(press(KeyCode::Char(' ')));
        let toggled_text = rendered(&state, 120, 32);
        assert!(toggled_text.contains("yes (user)"), "{toggled_text}");
    }

    /// Acceptance 1 (the render half): an empty Providers section shows an
    /// explanatory empty state rather than a blank list, and nothing panics.
    #[test]
    fn an_empty_providers_section_renders_an_empty_state() {
        let mut state = state_with_settings_open();
        state.handle_key(press(KeyCode::Tab));
        state.handle_key(press(KeyCode::Tab));
        let text = rendered(&state, 100, 30);
        assert!(text.contains("No providers configured."), "got:\n{text}");
        assert!(
            text.contains("Providers"),
            "the tab label must show:\n{text}"
        );

        for (w, h) in [(1, 1), (1, 40), (40, 1), (3, 3), (200, 60)] {
            rendered(&state, w, h);
        }
    }

    /// The Launch Profiles counterpart to the test above.
    #[test]
    fn an_empty_launch_profiles_section_renders_an_empty_state() {
        let mut state = state_with_settings_open();
        for _ in 0..3 {
            state.handle_key(press(KeyCode::Tab));
        }
        let text = rendered(&state, 100, 30);
        assert!(
            text.contains("No launch profiles configured."),
            "got:\n{text}"
        );
        assert!(text.contains("Launch Profiles"), "got:\n{text}");

        for (w, h) in [(1, 1), (1, 40), (40, 1), (3, 3), (200, 60)] {
            rendered(&state, w, h);
        }
    }

    #[test]
    fn providers_and_profiles_appear_in_their_sections() {
        let mut state = state_with_full_settings_open();
        state.handle_key(press(KeyCode::Tab));
        state.handle_key(press(KeyCode::Tab));
        let text = rendered(&state, 100, 30);
        assert!(text.contains("my-router"), "got:\n{text}");
        assert!(text.contains("openrouter"), "got:\n{text}");
        assert!(
            text.contains("https://mirror.example.com/v1"),
            "got:\n{text}"
        );

        state.handle_key(press(KeyCode::Tab));
        let text = rendered(&state, 100, 30);
        assert!(text.contains("fast"), "got:\n{text}");
        assert!(text.contains("claude-code"), "got:\n{text}");
    }

    /// Acceptance 7 — the one test this file exists to make pass for
    /// providers and launch profiles: a credential variable set to an
    /// unmistakable secret-shaped value must never appear on any Settings
    /// screen this phase adds, across every section and every inline editor,
    /// while its NAME must. Asserted with `!contains`, never `assert_eq!` on
    /// the secret material itself — see `integrations`'s
    /// `the_doctor_report_names_variable_names_and_never_values`, which this
    /// mirrors for the TUI.
    #[test]
    fn no_credential_value_is_ever_rendered_across_every_settings_screen() {
        const VAR: &str = "GLASSHOUSE_VIEW_TEST_ONLY_SECRET_VAR";
        const SECRET_VALUE: &str = "sk-view-test-totally-real-looking-secret-xyz123";
        /// A second credential, typed straight into the Settings overlay
        /// rather than read from the environment — the Phase 9E path, and
        /// the one that would echo on screen if the field were not masked.
        const TYPED_VALUE: &str = "sk-view-test-typed-into-the-field-abc789";

        // SAFETY: `VAR` is unique to this test and is removed again below,
        // including before every early return this test has (it has none).
        unsafe {
            std::env::set_var(VAR, SECRET_VALUE);
        }

        let mut config = ProviderConfig::new("openrouter");
        config.set_credential_env(vec![VAR.to_owned()]);
        let rows = vec![ProviderRow::new("secret-test", config, Layer::User)];

        let mut state = ShellState::new("glasshouse", "/work/glasshouse", "0.1.0", Vec::new());
        state.open_settings(harness_rows(), integration_rows(), rows, profile_rows());

        // Every screen is captured at a realistic width AND at a wide one.
        // At 100 columns the providers row is truncated, so a rendering that
        // leaked the credential's value would be clipped off-screen and this
        // test would pass for the wrong reason — verified by mutation: render
        // the value instead of set/not-set and only the wide capture fails.
        let mut screens = Vec::new();

        // Harnesses (the default section).
        screens.push(rendered(&state, 100, 30));
        screens.push(rendered(&state, 400, 60));

        // Integrations.
        state.handle_key(press(KeyCode::Tab));
        screens.push(rendered(&state, 100, 30));
        screens.push(rendered(&state, 400, 60));

        // Providers — the row itself must name the variable.
        state.handle_key(press(KeyCode::Tab));
        let providers_screen = rendered(&state, 100, 30);
        screens.push(rendered(&state, 400, 60));
        assert!(
            providers_screen.contains(VAR),
            "the variable NAME must be shown: {providers_screen}"
        );
        screens.push(providers_screen);

        // The "add a provider" wizard, both steps.
        state.handle_key(press(KeyCode::Char('a')));
        for c in "another".chars() {
            state.handle_key(press(KeyCode::Char(c)));
        }
        screens.push(rendered(&state, 100, 30));
        screens.push(rendered(&state, 400, 60));
        state.handle_key(press(KeyCode::Enter));
        for c in "openrouter".chars() {
            state.handle_key(press(KeyCode::Char(c)));
        }
        screens.push(rendered(&state, 100, 30));
        screens.push(rendered(&state, 400, 60));
        state.handle_key(press(KeyCode::Enter));

        // Editing the credential variable names of the original row ("another"
        // now sorts first, so one Down reaches "secret-test").
        state.handle_key(press(KeyCode::Down));
        state.handle_key(press(KeyCode::Char('c')));
        screens.push(rendered(&state, 100, 30));
        screens.push(rendered(&state, 400, 60));
        state.handle_key(press(KeyCode::Esc));

        // The connectivity test — its result names the provider, the
        // protocol and the exact URL, but must never carry the credential's
        // value. The environment variable above is set, so this plans a real
        // request and renders the in-flight line; no socket is opened here,
        // because opening one is the run loop's job and this test has no run
        // loop.
        state.handle_key(press(KeyCode::Char('t')));
        let test_screen = rendered(&state, 400, 60);
        screens.push(rendered(&state, 100, 30));
        assert!(
            test_screen.contains("request in flight"),
            "the test result must say a request is running: {test_screen}"
        );
        screens.push(test_screen);

        // A finished probe's rendered outcome, one per shape, since each
        // formats a different set of fields. The first also clears the row's
        // in-flight marker, which is what lets `m` below start a second
        // request rather than being refused as a duplicate.
        for outcome in [
            crate::provider::discovery::ProbeOutcome::Rejected { status: 401 },
            crate::provider::discovery::ProbeOutcome::TimedOut { waited_ms: 10_000 },
            crate::provider::discovery::ProbeOutcome::Reached { status: 200 },
        ] {
            state.apply_provider_probe_result(crate::shell::state::ProviderProbeResult {
                provider: "secret-test".to_owned(),
                notice: crate::shell::state::ProviderNotice::Reachability(
                    crate::shell::state::ReachabilityCheck::Answered {
                        protocol: "openai-chat",
                        base_url: "https://openrouter.ai/api/v1".to_owned(),
                        endpoint: "https://openrouter.ai/api/v1/models".to_owned(),
                        outcome,
                    },
                ),
                catalogue: None,
            });
            screens.push(rendered(&state, 400, 60));
            screens.push(rendered(&state, 100, 30));
        }

        // A model refresh, on a provider whose model-list endpoint is
        // verified, and the same rule applies to its rendering.
        state.handle_key(press(KeyCode::Char('m')));
        let models_screen = rendered(&state, 400, 60);
        assert!(
            models_screen.contains("refreshing the model list"),
            "a running refresh must say so: {models_screen}"
        );
        screens.push(rendered(&state, 100, 30));
        screens.push(models_screen);

        // A cached catalogue on the row, timestamp and all — the one place a
        // model list is rendered, and therefore the one place a credential
        // could ride along with it.
        state.apply_provider_probe_result(crate::shell::state::ProviderProbeResult {
            provider: "secret-test".to_owned(),
            notice: crate::shell::state::ProviderNotice::Models(
                crate::shell::state::ModelRefresh::Refreshed {
                    count: 2,
                    fetched_at: 1_787_336_476,
                    endpoint: "https://openrouter.ai/api/v1/models".to_owned(),
                },
            ),
            catalogue: Some(crate::provider::cache::ModelCatalogue::new(
                "secret-test",
                "https://openrouter.ai/api/v1",
                "https://openrouter.ai/api/v1/models",
                1_787_336_476,
                vec![
                    crate::provider::cache::ModelEntry::new("vendor/one"),
                    crate::provider::cache::ModelEntry::new("vendor/two"),
                ],
            )),
        });
        let cached_screen = rendered(&state, 400, 60);
        assert!(
            cached_screen.contains("2 cached, fetched 2026-08-21 18:21:16Z"),
            "a cached model list must be rendered with the instant it was fetched: \
             {cached_screen}"
        );
        screens.push(cached_screen);
        screens.push(rendered(&state, 100, 30));

        // Typing a credential into the OS-secure-store field. The masked
        // rendering is asserted positively as well as with `!contains`, so
        // a field that rendered nothing at all could not pass this by
        // accident.
        state.handle_key(press(KeyCode::Char('s')));
        for c in TYPED_VALUE.chars() {
            state.handle_key(press(KeyCode::Char(c)));
        }
        let credential_screen = rendered(&state, 100, 30);
        screens.push(rendered(&state, 400, 60));
        assert!(
            credential_screen.contains(&"*".repeat(16)),
            "a credential field must render as mask characters: {credential_screen}"
        );
        screens.push(credential_screen);
        state.handle_key(press(KeyCode::Esc));

        // The delete-a-stored-credential confirmation.
        state.handle_key(press(KeyCode::Char('x')));
        let confirm_screen = rendered(&state, 100, 30);
        screens.push(rendered(&state, 400, 60));
        assert!(
            confirm_screen.contains("secure store"),
            "the confirmation must say what it is about to do: {confirm_screen}"
        );
        screens.push(confirm_screen);
        state.handle_key(press(KeyCode::Esc));

        // A provider whose credential is recorded as living in the OS store
        // says so on its row — line 2 at the row level — and still without
        // a value anywhere near it.
        state.record_provider_credential_stored(
            "secret-test",
            crate::config::StoredCredentialRef::new("glasshouse", VAR),
        );
        let stored_screen = rendered(&state, 400, 60);
        assert!(
            stored_screen.contains("stored in the OS secure store"),
            "the row must say where the credential is kept: {stored_screen}"
        );
        screens.push(stored_screen);
        screens.push(rendered(&state, 100, 30));

        // Launch Profiles.
        state.handle_key(press(KeyCode::Tab));
        screens.push(rendered(&state, 100, 30));
        screens.push(rendered(&state, 400, 60));

        // The project-write confirmation.
        state.handle_key(press(KeyCode::Char('W')));
        screens.push(rendered(&state, 100, 30));
        screens.push(rendered(&state, 400, 60));

        unsafe {
            std::env::remove_var(VAR);
        }

        for screen in &screens {
            for value in [SECRET_VALUE, TYPED_VALUE] {
                assert!(
                    !screen.contains(value),
                    "a credential value was rendered on screen:\n{screen}"
                );
            }
        }
    }

    /// Every line's text, joined — for asserting on what a helper produced
    /// without going through a `Buffer`, which pads and clips.
    fn lines_to_string(lines: &[Line<'_>]) -> String {
        lines
            .iter()
            .map(|line| {
                line.spans
                    .iter()
                    .map(|span| span.content.as_ref())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// **Acceptance test 1.** The result names what it actually reached —
    /// the protocol, the base URL, and the exact URL requested — and the
    /// disclaimer that used to sit under it is gone.
    ///
    /// The `!contains` half is the load-bearing one. The old line said
    /// "Glasshouse has no HTTP client yet", which stopped being true the
    /// moment `ureq` arrived with the gateway; a line that apologises for a
    /// check it is no longer failing to make is worse than no line, because
    /// it teaches the user to disbelieve a result that is now real.
    #[test]
    fn the_connectivity_result_names_what_it_reached_and_carries_no_disclaimer() {
        let lines = provider_test_result_lines(
            "router",
            &crate::shell::state::ReachabilityCheck::Answered {
                protocol: "openai-chat",
                base_url: "https://openrouter.ai/api/v1".to_owned(),
                endpoint: "https://openrouter.ai/api/v1/models".to_owned(),
                outcome: ProbeOutcome::Reached { status: 200 },
            },
        );
        let text = lines_to_string(&lines);

        assert!(text.contains("openai-chat"), "{text}");
        assert!(text.contains("https://openrouter.ai/api/v1"), "{text}");
        assert!(
            text.contains("GET https://openrouter.ai/api/v1/models"),
            "the exact URL requested must be named, or `reached` is unverifiable: {text}"
        );
        assert!(text.contains("answered 200"), "{text}");

        for disclaimer in [
            "Precondition check only",
            "not a real network request",
            "no HTTP client",
            "preconditions met",
        ] {
            assert!(
                !text.contains(disclaimer),
                "the disclaimer {disclaimer:?} must be gone — the request is real now: {text}"
            );
        }
    }

    /// **Acceptance test 2, on screen.** A rejected credential and an
    /// unreachable host must read as different problems, because they are:
    /// one is fixed with a key, the other with a URL or a network.
    #[test]
    fn a_rejection_and_an_unreachable_host_read_as_different_problems() {
        let rejected = describe_probe_outcome(&ProbeOutcome::Rejected { status: 401 });
        let unreachable = describe_probe_outcome(&ProbeOutcome::Unreachable {
            reason: "the connection was refused".to_owned(),
        });
        let timed_out = describe_probe_outcome(&ProbeOutcome::TimedOut { waited_ms: 10_000 });

        assert!(
            rejected.contains("reachable"),
            "a 401 must say the provider is there: {rejected}"
        );
        assert!(
            rejected.contains("401"),
            "and must name the status: {rejected}"
        );
        assert!(
            unreachable.contains("unreachable"),
            "and an unreachable host must say so: {unreachable}"
        );
        assert!(
            !unreachable.contains("did not accept the credential"),
            "an unreachable host says nothing about the credential: {unreachable}"
        );
        assert!(
            timed_out.contains("nothing came back"),
            "a timeout is its own third thing: {timed_out}"
        );
        assert_ne!(rejected, unreachable);
        assert_ne!(rejected, timed_out);

        // And they are coloured differently, so the difference survives being
        // skim-read: a provider that is there is not a red failure.
        assert_ne!(
            probe_outcome_color(&ProbeOutcome::Rejected { status: 401 }),
            probe_outcome_color(&ProbeOutcome::Unreachable {
                reason: String::new()
            })
        );
    }

    /// **Found running the real binary.** With the verb hard-coded, a
    /// refused connection rendered as "reached openai-chat at ... —
    /// unreachable — the connection was refused": a sentence that
    /// contradicts itself, and the same shape of defect as the "(not set) —
    /// stored in the OS secure store" row an earlier batch found the same
    /// way.
    ///
    /// Both directions are asserted. Checking only that "could not reach"
    /// appears would pass on a renderer that said it for a `200` too.
    #[test]
    fn a_result_that_never_reached_the_provider_does_not_claim_it_did() {
        let render = |outcome| {
            lines_to_string(&provider_test_result_lines(
                "p",
                &crate::shell::state::ReachabilityCheck::Answered {
                    protocol: "openai-chat",
                    base_url: "http://127.0.0.1:1/v1".to_owned(),
                    endpoint: "http://127.0.0.1:1/v1".to_owned(),
                    outcome,
                },
            ))
        };

        for outcome in [
            ProbeOutcome::Unreachable {
                reason: "the connection was refused".to_owned(),
            },
            ProbeOutcome::TimedOut { waited_ms: 10_003 },
        ] {
            let text = render(outcome.clone());
            assert!(
                text.contains("could not reach"),
                "a probe that got no answer must not say it reached anything: {text}"
            );
            assert!(
                !text.contains(": reached "),
                "and it must not contradict itself inside one sentence: {text}"
            );
        }

        for outcome in [
            ProbeOutcome::Reached { status: 200 },
            ProbeOutcome::Rejected { status: 401 },
            ProbeOutcome::Unexpected { status: 404 },
        ] {
            let text = render(outcome.clone());
            assert!(
                text.contains(": reached "),
                "an endpoint that answered {outcome:?} really was reached: {text}"
            );
            assert!(!text.contains("could not reach"), "{text}");
        }
    }

    /// **Found running the real binary.** A provider with no established
    /// model-list endpoint used to render "press m to fetch" — advertising
    /// a key that cannot ever fetch anything for it.
    #[test]
    fn a_row_never_advertises_a_refresh_key_for_a_provider_that_cannot_refresh() {
        // `ollama`'s model list is Unverified; `openrouter`'s is Verified.
        let row = ProviderRow::new(
            "local",
            crate::config::ProviderConfig::new("ollama"),
            Layer::User,
        );
        let text = lines_to_string(&[provider_models_line(&row, 1_000)]);
        assert!(
            !text.contains("press m"),
            "a provider that cannot refresh must not be told to press m: {text}"
        );
        assert!(
            text.contains("no model-discovery endpoint established"),
            "and it must say why instead of leaving a dead control unexplained: {text}"
        );

        // The counterpart, so this cannot pass by never offering the key.
        let text = lines_to_string(&[provider_models_line(&provider_row_with(None), 1_000)]);
        assert!(
            text.contains("press m"),
            "a provider that CAN refresh must still say so: {text}"
        );
    }

    /// A running request says it is running, and names the URL it is waiting
    /// on. This is the line that separates "slow" from "frozen".
    #[test]
    fn a_request_in_flight_says_so_on_screen() {
        let text = lines_to_string(&provider_test_result_lines(
            "router",
            &crate::shell::state::ReachabilityCheck::InFlight {
                protocol: "openai-chat",
                base_url: "https://openrouter.ai/api/v1".to_owned(),
                endpoint: "https://openrouter.ai/api/v1/models".to_owned(),
            },
        ));
        assert!(text.contains("in flight"), "{text}");
        assert!(text.contains("stays usable"), "{text}");
    }

    /// **Acceptance test 6, on screen.** A provider with no model discovery
    /// gets a sentence, and it is not styled as a failure.
    #[test]
    fn a_provider_without_model_discovery_is_explained_rather_than_reported_as_an_error() {
        let lines = provider_models_result_lines(
            "local",
            &crate::shell::state::ModelRefresh::NotOffered(
                "no model-discovery endpoint has been established for `local`".to_owned(),
            ),
        );
        let text = lines_to_string(&lines);
        assert!(text.contains("has been established"), "{text}");
        assert!(
            !text.contains("could not"),
            "not phrased as a failure: {text}"
        );
        assert_eq!(
            lines[0].spans[0].style.fg,
            Some(Color::DarkGray),
            "an explanation must not be coloured like an error, or the user goes looking \
             for a problem they do not have"
        );

        // ... where a genuine failure is.
        let failed = provider_models_result_lines(
            "router",
            &crate::shell::state::ModelRefresh::Failed("the connection was refused".to_owned()),
        );
        assert_eq!(failed[0].spans[0].style.fg, Some(Color::Red));
    }

    // --- the timestamp ----------------------------------------------------

    /// **Phase 9D line 3's "with a timestamp", rendered.**
    #[test]
    fn a_unix_timestamp_renders_as_an_unambiguous_utc_instant() {
        // Checked against `date -u -r <seconds>`.
        assert_eq!(format_unix_utc(0), "1970-01-01 00:00:00Z");
        assert_eq!(format_unix_utc(1_787_336_476), "2026-08-21 18:21:16Z");
        assert_eq!(format_unix_utc(1_000_000_000), "2001-09-09 01:46:40Z");
        // A leap day, which is where a hand-rolled calendar goes wrong.
        assert_eq!(format_unix_utc(1_709_164_800), "2024-02-29 00:00:00Z");
        // The day after, so the leap day is not merely being clamped.
        assert_eq!(format_unix_utc(1_709_251_200), "2024-03-01 00:00:00Z");
        // 2100 is not a leap year, which is the century rule most
        // hand-rolled conversions get wrong.
        assert_eq!(format_unix_utc(4_107_542_400), "2100-03-01 00:00:00Z");
        // Before the epoch, because `div_euclid` is what makes that work and
        // a plain `/` would not.
        assert_eq!(format_unix_utc(-1), "1969-12-31 23:59:59Z");
    }

    /// The age is what makes a stale cache *visibly* stale. The instant says
    /// when; this says how long ago, which is the question a user has.
    #[test]
    fn an_age_is_rendered_in_the_largest_unit_that_still_says_something() {
        assert_eq!(describe_age(1_000, 1_000), "just now");
        assert_eq!(describe_age(1_059, 1_000), "just now");
        assert_eq!(describe_age(1_060, 1_000), "1 minute ago");
        assert_eq!(describe_age(1_120, 1_000), "2 minutes ago");
        assert_eq!(describe_age(4_600, 1_000), "1 hour ago");
        assert_eq!(describe_age(87_400, 1_000), "1 day ago");
        assert_eq!(describe_age(1_729_000, 1_000), "20 days ago");
        assert_eq!(describe_age(20_000_000, 1_000), "7 months ago");
    }

    /// A clock that moved is said out loud rather than rendered as a negative
    /// age or quietly clamped to "just now" — the second would make a cache
    /// from the future look freshly fetched.
    #[test]
    fn a_timestamp_in_the_future_is_reported_rather_than_clamped() {
        let text = describe_age(1_000, 9_000);
        assert!(text.contains("future"), "{text}");
        assert!(text.contains("clock"), "{text}");
        assert_ne!(text, "just now");
    }

    // --- the model line on a provider row ---------------------------------

    fn provider_row_with(models: Option<crate::provider::cache::ModelCatalogue>) -> ProviderRow {
        let mut config = crate::config::ProviderConfig::new("openrouter");
        config.set_base_url(Some("https://openrouter.ai/api/v1".to_owned()));
        ProviderRow::new("router", config, Layer::User).with_models(models)
    }

    fn catalogue_at(
        base_url: &str,
        fetched_at: i64,
        count: usize,
    ) -> crate::provider::cache::ModelCatalogue {
        crate::provider::cache::ModelCatalogue::new(
            "router",
            base_url,
            format!("{base_url}/models"),
            fetched_at,
            (0..count)
                .map(|i| crate::provider::cache::ModelEntry::new(format!("vendor/model-{i}")))
                .collect(),
        )
    }

    #[test]
    fn a_cached_model_list_is_never_shown_without_when_it_was_fetched() {
        let row = provider_row_with(Some(catalogue_at(
            "https://openrouter.ai/api/v1",
            1_787_336_476,
            417,
        )));
        let text = lines_to_string(&[provider_models_line(&row, 1_787_336_476 + 86_400)]);
        assert!(text.contains("417 cached"), "{text}");
        assert!(
            text.contains("2026-08-21 18:21:16Z"),
            "the instant must be there: {text}"
        );
        assert!(
            text.contains("1 day ago"),
            "and so must the age, which is the half a user acts on: {text}"
        );
    }

    #[test]
    fn a_provider_with_no_cached_models_says_how_to_get_some() {
        let text = lines_to_string(&[provider_models_line(&provider_row_with(None), 1_000)]);
        assert!(text.contains("none cached"), "{text}");
        assert!(
            text.contains("press m"),
            "an empty state must name the key that fills it: {text}"
        );
    }

    /// A catalogue fetched from a base URL the provider no longer uses is a
    /// *wrong* list, not merely a stale one, and the row says which.
    #[test]
    fn a_catalogue_from_a_base_url_the_provider_no_longer_uses_is_flagged() {
        let row = provider_row_with(Some(catalogue_at("https://old.example/v1", 1_000, 9)));
        let line = provider_models_line(&row, 2_000);
        let text = lines_to_string(std::slice::from_ref(&line));
        assert!(text.contains("https://old.example/v1"), "{text}");
        assert!(
            text.contains("no longer this provider's base URL"),
            "{text}"
        );
        assert_eq!(line.spans[0].style.fg, Some(Color::Yellow));
    }

    #[test]
    fn a_request_in_flight_outranks_the_cached_list_on_the_row() {
        let mut row =
            provider_row_with(Some(catalogue_at("https://openrouter.ai/api/v1", 1_000, 9)));
        row.activity = Some(ProbeKind::ModelRefresh);
        let text = lines_to_string(&[provider_models_line(&row, 2_000)]);
        assert!(text.contains("refreshing the model list"), "{text}");

        row.activity = Some(ProbeKind::Connectivity);
        let text = lines_to_string(&[provider_models_line(&row, 2_000)]);
        assert!(text.contains("testing connectivity"), "{text}");
    }

    /// **The two-orders-of-magnitude range the packet named**, rendered at a
    /// realistic width and a narrow one.
    ///
    /// Nine models and four hundred and seventeen must both produce exactly
    /// one line: a renderer that grew with the catalogue would push every row
    /// below it off a short terminal, and the count is what makes the line
    /// length independent of the list length.
    #[test]
    fn a_catalogue_of_nine_and_of_four_hundred_and_seventeen_both_render_as_one_short_line() {
        let mut lengths = Vec::new();
        for count in [9usize, 417] {
            let row = provider_row_with(Some(catalogue_at(
                "https://openrouter.ai/api/v1",
                1_787_336_476,
                count,
            )));
            let line = provider_models_line(&row, 1_787_336_476);
            let text = lines_to_string(&[line]);
            assert_eq!(text.lines().count(), 1, "{text}");
            assert!(
                !text.contains("vendor/model-0"),
                "a row summarises a catalogue; it must never list it: {text}"
            );
            lengths.push(text.chars().count());
        }
        assert!(
            lengths[1] - lengths[0] <= 2,
            "the line's length must follow the count's digits and nothing else, got {lengths:?}"
        );
    }

    /// The whole Providers section, with a large catalogue cached, at a
    /// realistic terminal width and an absurdly narrow one. Ratatui clips;
    /// the assertion is that nothing panics and the rows are still rows.
    #[test]
    fn the_providers_section_survives_a_large_catalogue_at_every_width() {
        let rows = vec![provider_row_with(Some(catalogue_at(
            "https://openrouter.ai/api/v1",
            1_787_336_476,
            417,
        )))];
        let mut state = ShellState::new("glasshouse", "/work/glasshouse", "0.1.0", Vec::new());
        state.open_settings(harness_rows(), integration_rows(), rows, profile_rows());
        state.handle_key(press(KeyCode::Tab));
        state.handle_key(press(KeyCode::Tab));

        for (w, h) in [(1, 1), (20, 5), (80, 24), (100, 30), (400, 60)] {
            let text = rendered(&state, w, h);
            assert!(
                !text.contains("vendor/model-200"),
                "a four-hundred-model catalogue must never be enumerated on screen"
            );
        }
        assert!(rendered(&state, 400, 60).contains("417 cached"));
    }

    /// Found running the real binary: refusing an unknown harness produces a
    /// long label (a name-enumerating prompt) that wraps by itself on a
    /// realistic terminal width, and a fixed bottom-panel height clipped the
    /// error line beneath it into invisibility. This renders the exact
    /// scenario and asserts the error text is actually on screen.
    #[test]
    fn an_unknown_harness_error_is_visible_not_clipped_by_a_wrapped_label() {
        let mut state = state_with_settings_open();
        state.handle_key(press(KeyCode::Tab));
        state.handle_key(press(KeyCode::Tab));
        state.handle_key(press(KeyCode::Tab));
        state.handle_key(press(KeyCode::Char('a')));
        for c in "custom".chars() {
            state.handle_key(press(KeyCode::Char(c)));
        }
        state.handle_key(press(KeyCode::Enter));
        for c in "not-a-real-harness".chars() {
            state.handle_key(press(KeyCode::Char(c)));
        }
        state.handle_key(press(KeyCode::Enter));

        let text = rendered(&state, 100, 30);
        assert!(
            text.contains("not a harness Glasshouse knows"),
            "the refusal error must actually be visible on screen, not clipped:\n{text}"
        );
    }
}
