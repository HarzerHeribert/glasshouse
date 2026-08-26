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

use crate::config::Layer;
use crate::session::{SessionDisposition, SessionRecord};

use super::state::{
    Mode, Overlay, SettingsPathInputView, SettingsSection, SettingsState, ShellState, ViewportGrid,
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
    let grid = state.viewport_grid();
    if !grid.is_empty() {
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
                "This viewport is reserved for the session's own terminal.",
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
        (Mode::Control, Some(Overlay::Overview)) => "esc back to session   q quit",
        (Mode::Control, Some(Overlay::Settings)) => {
            "tab section   up/down move   space toggle   enter/a/e/c/b/p/u/d/t edit   \
             s/x secret   w save   W project   r setup   esc close"
        }
        (Mode::Control, None) => "tab session   enter session   n new   o overview   q quit",
    };
    let mut spans = vec![Span::styled(hint, Style::default().fg(Color::DarkGray))];

    if let Some(status) = state.status() {
        spans.push(Span::raw("   "));
        spans.push(Span::styled(status, Style::default().fg(Color::Yellow)));
    }

    // Order is the whole mechanism: the bindings are written first, so when the
    // row is too narrow it is the note that gets clipped away. The bindings are
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

    let mut lines = vec![Line::from(Span::styled(
        format!(
            "{:<14}  {:<12}  {:<10}  {:<12}  {}",
            "HARNESS", "STATE", "ROLE", "PRESENTED", "SESSION"
        ),
        Style::default().add_modifier(Modifier::BOLD),
    ))];

    if state.sessions().is_empty() {
        lines.push(Line::from(Span::styled(
            "This project has no recorded sessions.",
            Style::default().fg(Color::DarkGray),
        )));
    }

    for (index, session) in state.sessions().iter().enumerate() {
        let active = index == state.selected_index();
        let style = if active {
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default()
        };
        lines.push(Line::from(Span::styled(
            format!(
                "{:<14}  {:<12}  {:<10}  {:<12}  {}",
                session.harness,
                disposition_label(session),
                session.role,
                session.presentation,
                short_id(session),
            ),
            style,
        )));
    }

    frame.render_widget(Paragraph::new(lines), inner);
}

fn disposition_label(session: &SessionRecord) -> &'static str {
    match session.disposition() {
        SessionDisposition::Active => "active",
        SessionDisposition::Resumable => "resumable",
        SessionDisposition::Closed => "closed",
        SessionDisposition::Failed => "failed",
    }
}

fn short_id(session: &SessionRecord) -> String {
    session.id.as_str().chars().take(12).collect()
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
    } else if let Some((name, outcome)) = settings.provider_test_result() {
        provider_test_result_lines(name, outcome)
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
    }

    if !bottom_lines.is_empty() {
        frame.render_widget(
            Paragraph::new(bottom_lines).wrap(Wrap { trim: false }),
            bottom_area,
        );
    }
}

fn render_settings_tabs(settings: &SettingsState, frame: &mut Frame, area: Rect) {
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
            "Launch Profiles",
            settings.section() == SettingsSection::LaunchProfiles,
        ),
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
    }
    frame.render_widget(Paragraph::new(lines), area);
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

/// Line 5's connectivity check result. Named honestly as a precondition
/// check, never as proof the network is reachable — see
/// [`crate::shell::state::ReachabilityCheck`]'s own doc for exactly what is
/// and is not established.
fn provider_test_result_lines(
    name: &str,
    outcome: &crate::shell::state::ReachabilityCheck,
) -> Vec<Line<'static>> {
    use crate::shell::state::ReachabilityCheck;

    let (message, color) = match outcome {
        ReachabilityCheck::PreconditionsMet { protocol, base_url } => (
            format!("`{name}`: reachability preconditions met for {protocol} at {base_url}"),
            Color::Green,
        ),
        ReachabilityCheck::Failed(reason) => (
            format!("`{name}`: reachability precondition failed — {reason}"),
            Color::Red,
        ),
    };
    vec![
        Line::from(Span::styled(message, Style::default().fg(color))),
        Line::from(Span::styled(
            "Precondition check only — not a real network request; Glasshouse has no HTTP \
             client yet.",
            Style::default().fg(Color::DarkGray),
        )),
    ]
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
        let bottom = last_row(&state, 100, 24);
        assert!(bottom.contains("tab"), "bindings missing: `{bottom}`");
        assert!(bottom.contains("overview"), "bindings missing: `{bottom}`");
        assert!(bottom.contains("quit"), "bindings missing: `{bottom}`");

        state.handle_key(KeyEvent::new(KeyCode::Char('o'), KeyModifiers::NONE));
        let bottom = last_row(&state, 100, 24);
        assert!(
            bottom.contains("esc") && bottom.contains("quit"),
            "the overlay's bindings must be shown too: `{bottom}`"
        );
    }

    /// A key that could not do anything must explain itself in the status bar,
    /// or it reads as a broken keyboard.
    #[test]
    fn the_status_bar_shows_a_note_next_to_the_bindings() {
        let mut state = ShellState::new("p", "/p", "0.1.0", vec![lone_session()]);
        state.handle_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));

        let bottom = last_row(&state, 100, 24);
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
    /// visualizations that do not expose actionable state."
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
        let bottom = last_row(&state, 100, 24).to_lowercase();
        assert!(!bottom.contains("session mode"), "got: `{bottom}`");
        assert!(bottom.contains("quit"), "got: `{bottom}`");
    }
}

#[cfg(test)]
mod settings_tests {
    use crate::config::{ProfileConfig, ProviderConfig};
    use crate::integrations::{IntegrationId, IntegrationStatus};
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    use super::super::state::{HarnessRow, IntegrationRow, ProfileRow, ProviderRow};
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
        vec![ProviderRow {
            name: "my-router".to_owned(),
            config,
            layer: Layer::User,
        }]
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
        let rows = vec![ProviderRow {
            name: "secret-test".to_owned(),
            config,
            layer: Layer::User,
        }];

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

        // The reachability precondition test — its result names the
        // provider and protocol but must never carry the credential's value.
        state.handle_key(press(KeyCode::Char('t')));
        let test_screen = rendered(&state, 100, 30);
        screens.push(rendered(&state, 400, 60));
        assert!(
            test_screen.contains("preconditions met")
                || test_screen.contains("precondition failed"),
            "the test result must say something about the check: {test_screen}"
        );
        screens.push(test_screen);

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
