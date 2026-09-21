//! One palette, read from the workbench mockup, expressed as terminal colour.
//!
//! **The background is never painted and normal prose keeps the terminal's own
//! foreground.** Everything else is a role — accent, helper, failure, warning,
//! success, muted, rule — and a role is one fixed colour so that two surfaces
//! drawn by different code cannot disagree about what "muted" looks like. Only
//! the accent moves with the chosen theme, exactly as the mockup's `--accent`
//! custom property does.
use super::Tone;
use crate::tui::Theme;
use ratatui::style::{Color, Modifier, Style};

/// `--accent` per theme, as the mockup's dark shell defines it.
pub(super) fn accent(theme: Theme) -> Color {
    match theme {
        Theme::Neon => Color::Rgb(0xda, 0xff, 0x50),
        Theme::Amber => Color::Rgb(0xff, 0xce, 0x72),
        Theme::Ice => Color::Rgb(0x8b, 0xe3, 0xff),
        // Mono keeps the terminal's own foreground: its accent is emphasis,
        // not hue, and a user who chose mono asked for exactly that.
        Theme::Mono => Color::Reset,
        Theme::Violet => Color::Rgb(0xd4, 0xb4, 0xff),
        Theme::Cobalt => Color::Rgb(0x9e, 0xc9, 0xff),
        Theme::Mint => Color::Rgb(0x86, 0xf1, 0xd0),
        Theme::Rose => Color::Rgb(0xff, 0xb3, 0xd4),
    }
}
/// `--muted`: technical detail that must stay readable, never decoration.
pub(super) const MUTED: Color = Color::Rgb(0x99, 0xa6, 0xb7);
/// `--line`: rules and separators, the only thing quieter than muted.
pub(super) const LINE: Color = Color::Rgb(0x55, 0x64, 0x76);
/// `--cyan`: little helpers and their returned evidence.
pub(super) const HELPER: Color = Color::Rgb(0x8b, 0xe3, 0xff);
const WARN: Color = Color::Rgb(0xff, 0xca, 0x80);
const RED: Color = Color::Rgb(0xff, 0x84, 0x94);
const GREEN: Color = Color::Rgb(0xa4, 0xf1, 0xbd);

pub(super) fn style(tone: Tone, theme: Theme) -> Style {
    // Never paint a background: Ghostty and other terminals own opacity.
    let base = Style::default().fg(Color::Reset).bg(Color::Reset);
    match tone {
        Tone::Normal | Tone::Code => base,
        Tone::Strong => base.add_modifier(Modifier::BOLD),
        Tone::Accent => base.fg(accent(theme)).add_modifier(Modifier::BOLD),
        Tone::Helper => base.fg(HELPER),
        Tone::Failure => base.fg(RED).add_modifier(Modifier::BOLD),
        Tone::Warning => base.fg(WARN).add_modifier(Modifier::BOLD),
        Tone::Success => base.fg(GREEN),
        Tone::Muted => base.fg(MUTED),
        Tone::Line => base.fg(LINE),
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
/// The mockup's three-line working mark: a still bowl, or a turning one.
///
/// It is decoration and it is the one thing reduced motion freezes, so the
/// still frame is a complete shape rather than a paused animation frame.
pub(super) fn mark(tick: usize, still: bool) -> [&'static str; 3] {
    const FRAMES: [[&str; 3]; 4] = [
        ["  ⢀⣀⣀⡀", "⢀⡴⠋  ⠙⢦⡀", "  ⠙⠶⣤⠶⠋"],
        [" ⢀⡴⠛⠛⢦⡀", " ⡞ ⠐⠒ ⢳", " ⠈⠻⢤⡤⠟"],
        ["  ⢀⣠⣄⡀", "⢰⠋ ⢠⠄ ⠙⡆", " ⠈⠓⠶⠖⠋"],
        ["  ⣀⣀⣀⣀", "⡞  ⢀⡀  ⢳", " ⠙⠳⠤⠞⠋"],
    ];
    if still {
        ["  ⢀⣤⡀", " ⠐⢿⣿⡿⠂", "  ⠈⠛⠁"]
    } else {
        FRAMES[(tick / 3) % FRAMES.len()]
    }
}
/// The mark's three lines, each padded to the same column count, so the text
/// beside it starts at one x on every frame.
pub(super) fn padded_mark(tick: usize, still: bool) -> [String; 3] {
    let art = mark(tick, still);
    let wide = art
        .iter()
        .map(|line| ratatui::text::Span::raw(*line).width())
        .max()
        .unwrap_or(0);
    art.map(|line| {
        let pad = wide - ratatui::text::Span::raw(line).width();
        format!("{line}{}", " ".repeat(pad))
    })
}
