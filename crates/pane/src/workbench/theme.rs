use super::Tone;
use crate::tui::Theme;
use ratatui::style::{Color, Modifier, Style};

pub(super) fn accent(theme: Theme) -> Color {
    match theme {
        Theme::Neon => Color::LightGreen,
        Theme::Amber => Color::LightYellow,
        Theme::Ice => Color::LightCyan,
        Theme::Mono => Color::Reset,
        Theme::Violet => Color::LightMagenta,
        Theme::Cobalt => Color::LightBlue,
        Theme::Mint => Color::Cyan,
        Theme::Rose => Color::Magenta,
    }
}
pub(super) fn style(tone: Tone, theme: Theme) -> Style {
    // Never paint a background: Ghostty and other terminals own opacity.
    let base = Style::default().fg(Color::Reset).bg(Color::Reset);
    match tone {
        Tone::Normal | Tone::Code => base,
        Tone::Accent => base.fg(accent(theme)).add_modifier(Modifier::BOLD),
        Tone::Failure => base.fg(Color::LightRed).add_modifier(Modifier::BOLD),
        Tone::Warning => base.fg(Color::LightYellow).add_modifier(Modifier::BOLD),
        Tone::Success => base.fg(Color::LightGreen),
        Tone::Muted => base.fg(Color::DarkGray),
    }
}
/// A bounded braille orbit; only the active glyph changes, never text geometry.
pub(super) fn orbit(tick: usize, still: bool) -> &'static str {
    const FRAMES: [&str; 12] = [
        "⠈⢆⡀",
        "⠐⢄⡁",
        "⠠⢂⠃",
        "⢀⠡⠆",
        "⡀⠑⠤",
        "⡁⠊⠰",
        "⠃⠔⢠",
        "⠆⠢⢀",
        "⠤⠡⡀",
        "⠰⠊⡁",
        "⢠⠔⠃",
        "⢀⠢⠆",
    ];
    if still {
        "⠈⢆⡀"
    } else {
        FRAMES[(tick / 2) % FRAMES.len()]
    }
}
