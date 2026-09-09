//! The Settings overlay's **actions**, drawn as buttons, and the one line that
//! says what the section in front of you is for.
//!
//! **The problem this exists for, in the user's words: "there are like 5
//! million buttons and i still dont see a way to actually add providers or
//! subscriptions".** Both halves of that sentence were true at once. The
//! Providers section has bound `a` to *add a provider* since Phase 9D, beside
//! nine other bare letters — `e c f space d t m s x` — and the whole
//! documentation of all ten was the footer's phrase `section keys edit`. A
//! capability nobody can see is, from where the user sits, a capability that
//! does not exist.
//!
//! **The rule this module applies, and it is the same one the control-mode
//! footer applies: the act a user came to perform is drawn as a button; the
//! keys that only move the cursor are not.** So each section gets a row of
//! pills carrying its *actions*, most-consequential first — `a add provider`
//! leads the Providers row, `c connect` leads the Subscriptions row — and Tab,
//! the arrow keys, `w` and `esc` stay in the footer hint where they were. No
//! binding moved and none was removed: a pill replays exactly the key it
//! spells, through `hotspot::Hotspot`, so the letters keep working for
//! everyone who already learned them.
//!
//! The tip line is the other half of the same complaint — *"should be a tips
//! and tricks"*. One sentence, about the section actually on screen, changing
//! with the state it is in, is worth more than a help page nobody opens; and
//! it costs one row of a list that already scrolls.

use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Wrap};

use crossterm::event::KeyCode;

use super::super::hotspot::{self, Hotspot, Pill};
use super::super::state::{SettingsSection, SettingsState, ShellState, subscription_tip};

/// One line saying what this section configures, and why a user would touch
/// it.
///
/// **Written for someone who has not read the CLI help**, because that is
/// exactly the person who cannot get past the first step. Each names the thing
/// the section owns, not the widgets it contains.
pub(super) fn section_tip(settings: &SettingsState) -> String {
    match settings.section() {
        SettingsSection::Harnesses => "The coding agents Glasshouse can launch. Enable one and \
             Glasshouse finds it on your PATH, or set an explicit path with Enter."
            .to_owned(),
        SettingsSection::Integrations => {
            "Optional tools Glasshouse uses when they are present. Read-only: nothing here \
             is configured, only detected."
                .to_owned()
        }
        SettingsSection::Providers => {
            "API keys you pay per token for. Press `a` to add one — that is the step a new \
             install needs before routing can choose anything."
                .to_owned()
        }
        SettingsSection::Subscriptions => {
            subscription_tip(settings.subscriptions(), settings.broker())
        }
        SettingsSection::LaunchProfiles => {
            "Named launch recipes: which harness, which backend, which model. A profile \
             backed by the gateway is how a subscription pays for a session."
                .to_owned()
        }
        SettingsSection::Routing => {
            "How Glasshouse picks a destination when a profile does not name one. Ceilings \
             refuse; they never silently downgrade."
                .to_owned()
        }
        SettingsSection::Memory => {
            "What Glasshouse remembers between sessions. Read-only in this build.".to_owned()
        }
    }
}

/// The actions this section offers, most-consequential first.
///
/// **Order is the whole design.** The first pill in each row is the one a
/// person who has just installed Glasshouse needs; everything after it is for
/// someone already running. `a add provider` and `c connect` therefore lead
/// their rows even though neither is the section's most-used key, because a
/// user who cannot perform the first act never reaches the others.
///
/// Sections with no actions of their own return an empty row and are given no
/// band at all — a button bar over a read-only list is decoration, and
/// `tui-actionables.md`'s rule is that a surface with nothing to press gets
/// nothing drawn.
pub(super) fn section_pills(settings: &SettingsState) -> Vec<Pill> {
    match settings.section() {
        SettingsSection::Harnesses => vec![
            Pill::key("space", "enable", KeyCode::Char(' ')),
            Pill::key("enter", "set path", KeyCode::Enter),
        ],
        SettingsSection::Integrations => Vec::new(),
        SettingsSection::Providers => vec![
            Pill::key("a", "add provider", KeyCode::Char('a')),
            Pill::key("s", "set key", KeyCode::Char('s')),
            Pill::key("t", "test", KeyCode::Char('t')),
            Pill::key("m", "models", KeyCode::Char('m')),
            Pill::key("e", "base url", KeyCode::Char('e')),
            Pill::key("c", "key variable", KeyCode::Char('c')),
            Pill::key("f", "free models", KeyCode::Char('f')),
            Pill::key("space", "enable", KeyCode::Char(' ')),
            Pill::key("x", "forget key", KeyCode::Char('x')),
            Pill::key("d", "remove", KeyCode::Char('d')),
        ],
        SettingsSection::Subscriptions => {
            // The connect pill leads even on a connected row: the label
            // follows the row's own state so the button never offers the act
            // that already happened.
            let primary = settings
                .selected_subscription_row()
                .map_or("connect", |row| row.primary_action());
            vec![
                Pill::key(if primary == "connect" { "c" } else { "x" }, primary, {
                    if primary == "connect" {
                        KeyCode::Char('c')
                    } else {
                        KeyCode::Char('x')
                    }
                }),
                Pill::key("b", "adopt broker", KeyCode::Char('b')),
            ]
        }
        SettingsSection::LaunchProfiles => vec![
            Pill::key("a", "add profile", KeyCode::Char('a')),
            Pill::key("b", "backend", KeyCode::Char('b')),
            Pill::key("e", "model", KeyCode::Char('e')),
            Pill::key("p", "approval", KeyCode::Char('p')),
            Pill::key("u", "duplicate", KeyCode::Char('u')),
            Pill::key("space", "enable", KeyCode::Char(' ')),
            Pill::key("d", "remove", KeyCode::Char('d')),
        ],
        SettingsSection::Routing => vec![
            Pill::key("m", "model", KeyCode::Char('m')),
            Pill::key("f", "prefer free", KeyCode::Char('f')),
            Pill::key("c", "max cost", KeyCode::Char('c')),
            Pill::key("l", "max latency", KeyCode::Char('l')),
            Pill::key("p", "premium reserve", KeyCode::Char('p')),
            Pill::key("o", "free order", KeyCode::Char('o')),
            Pill::key("d", "free disabled", KeyCode::Char('d')),
            Pill::key("n", "free pin", KeyCode::Char('n')),
        ],
        SettingsSection::Memory => vec![Pill::key("space", "extraction", KeyCode::Char(' '))],
    }
}

/// Rows the tip needs at this width, wrapped the way it is painted, and never
/// more than two.
///
/// **Measured, not assumed to be one.** A fixed row would clip the sentence
/// exactly where its answer begins — the Subscriptions tip ends in the literal
/// `adopt-binary` command, so a clipped tip is a tip that names the step and
/// withholds how to take it. Two is the ceiling because the band is taking
/// these rows from the list of accounts underneath it.
fn tip_rows(settings: &SettingsState, width: u16) -> u16 {
    if width == 0 {
        return 1;
    }
    super::wrapped_height(&[Line::from(section_tip(settings))], width).clamp(1, 2)
}

/// Rows the action band needs: the tip's rows, plus however many the pills wrap
/// onto, clamped so a short overlay keeps its list.
///
/// Asked before the split, and answered from the same pill list and the same
/// wrap the band then paints, so what is reserved and what is drawn cannot
/// disagree — the property whose absence clipped seven of the control-mode
/// footer's fifteen actions off the screen.
pub(super) fn band_rows(settings: &SettingsState, area: Rect) -> u16 {
    let pills = section_pills(settings);
    let ceiling = (area.height / 2).max(1);
    let bar = if pills.is_empty() {
        0
    } else {
        hotspot::rows_for(&pills, area.width)
    };
    (bar + tip_rows(settings, area.width)).min(ceiling)
}

/// Paint the tip and the action row over the top of the section's list, and
/// record a hotspot for every pill drawn.
///
/// Returns the rectangle the list itself is left with. Splitting here rather
/// than in the caller keeps the reservation and the painting in one function,
/// for the reason [`band_rows`] gives.
pub(super) fn render_band(
    settings: &SettingsState,
    frame: &mut Frame,
    area: Rect,
    theme: super::super::appearance::Theme,
    sink: &mut Vec<Hotspot>,
) -> Rect {
    if area.height == 0 || area.width == 0 {
        return area;
    }
    let pills = section_pills(settings);
    let rows = band_rows(settings, area);
    let regions = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(rows), Constraint::Min(0)])
        .split(area);
    let band = regions[0];
    let list = regions[1];

    let tip_height = tip_rows(settings, band.width).min(band.height);
    let tip_area = Rect {
        height: tip_height,
        ..band
    };
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            section_tip(settings),
            Style::default().fg(theme.quiet()),
        )))
        .wrap(Wrap { trim: false }),
        tip_area,
    );
    if band.height > tip_height && !pills.is_empty() {
        let bar = Rect {
            y: band.y + tip_height,
            height: band.height - tip_height,
            ..band
        };
        hotspot::render_bar(frame, bar, &pills, theme, sink);
    }
    list
}

/// The Subscriptions section's rows, and — under the cursor — the exact
/// command that changes the account it is on.
///
/// **The command is on screen without a keystroke.** A user who has to press
/// something to find out what to press has been given a puzzle, not an
/// interface; and this is the one section whose act Glasshouse deliberately
/// does not perform for them, so the string to type *is* the affordance.
pub(super) fn render_subscription_rows(
    settings: &SettingsState,
    frame: &mut Frame,
    area: Rect,
    theme: super::super::appearance::Theme,
) {
    let mut lines: Vec<Line<'static>> = Vec::new();
    if settings.subscriptions().is_empty() {
        lines.push(Line::from(Span::styled(
            "No subscription account is configured.",
            Style::default().fg(theme.quiet()),
        )));
        lines.push(Line::default());
        lines.push(Line::from(Span::styled(
            "A plan you already pay for — Claude, ChatGPT or Gemini — can pay for a \
             Glasshouse session, through a broker whose token Glasshouse never reads. \
             Three steps, in this order:",
            Style::default().fg(theme.quiet()),
        )));
        lines.push(Line::default());
        for (index, step) in [
            crate::subscription::ADOPT_BINARY_COMMAND.to_owned(),
            "add an [entitlements.<name>] table with subscription_broker = \"cliproxyapi\""
                .to_owned(),
            "glasshouse subscriptions login <anthropic|openai|google> --entitlement <name>"
                .to_owned(),
        ]
        .into_iter()
        .enumerate()
        {
            lines.push(Line::from(vec![
                Span::styled(
                    format!("  {}. ", index + 1),
                    Style::default().fg(theme.quiet()),
                ),
                Span::styled(step, Style::default().fg(theme.accent())),
            ]));
        }
    }
    for (index, row) in settings.subscriptions().iter().enumerate() {
        let selected = index == settings.selected_subscription();
        let cursor = if selected { "> " } else { "  " };
        let mut style = Style::default();
        if selected {
            style = style.add_modifier(Modifier::BOLD);
        }
        lines.push(Line::from(Span::styled(
            format!(
                "{cursor}{:<24} {:<10} {}",
                row.entitlement,
                row.provider,
                row.status()
            ),
            style,
        )));
        if selected {
            lines.push(Line::from(vec![
                Span::styled("    run: ", Style::default().fg(theme.quiet())),
                Span::styled(
                    row.primary_command().to_owned(),
                    Style::default()
                        .fg(theme.accent())
                        .add_modifier(Modifier::BOLD),
                ),
            ]));
        }
    }
    // **Wrapped, unlike every other section's list.** The others render fixed
    // columns that a narrow terminal may truncate without losing an answer;
    // this one's second line is a command to type, and a command clipped at
    // `--entitlement claud` is worse than no command at all.
    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), area);
}

/// The bottom panel a subscription command is spelled into once one is asked
/// for, with the sentence that says why Glasshouse is not running it.
pub(super) fn account_command_lines(command: &str) -> Vec<Line<'static>> {
    vec![
        Line::from(Span::styled(
            "Run this in another terminal — the login opens your browser and Glasshouse \
             never sees the token:"
                .to_owned(),
            Style::default().add_modifier(Modifier::BOLD),
        )),
        Line::from(Span::raw(format!("  {command}"))),
    ]
}

/// The one line the whole overlay owes a user who has connected nothing: what
/// to press next, on the surface where they are already standing.
///
/// Rendered by the landing panel rather than by Settings, which is the point —
/// see [`crate::shell::view::chrome::render_landing`].
pub(super) fn first_run_next_step(state: &ShellState) -> Option<String> {
    let accounts = state.accounts();
    if !accounts.nothing_connected() {
        return None;
    }
    Some(if accounts.is_first_run() {
        "Next step: connect an account. Press `s`, then Tab to Subscriptions.".to_owned()
    } else {
        format!(
            "Next step: connect one of your {} accounts. Press `s`, then Tab to Subscriptions.",
            accounts.configured
        )
    })
}

#[cfg(test)]
#[path = "../tests/accounts_tests.rs"]
mod tests;
