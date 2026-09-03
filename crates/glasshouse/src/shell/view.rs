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
#[path = "tests/view_tests.rs"]
mod tests;
