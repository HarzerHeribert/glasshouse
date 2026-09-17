//! The status line's row layout: one left half, one right half, and where
//! each lands -- so what is clickable is recorded from the same arithmetic
//! that draws it.

use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};

use super::{ContextTokens, MUTED, abbreviate};

pub(super) fn footer_row(
    left: String,
    right: String,
    width: usize,
    right_color: Color,
) -> Line<'static> {
    let occupied = Line::from(left.as_str()).width() + Line::from(right.as_str()).width() + 2;
    if right.is_empty() || occupied > width {
        return Line::styled(abbreviate(&left, width), Style::default().fg(MUTED));
    }
    Line::from(vec![
        Span::styled(left, Style::default().fg(MUTED)),
        Span::raw(" ".repeat(width - occupied + 1)),
        Span::styled(right, Style::default().fg(right_color)),
        Span::raw(" "),
    ])
}

/// Where [`footer_row`] puts `right`, as (x, width), or `None` when the row
/// was too narrow and collapsed to its left half alone. It repeats that
/// function's arithmetic deliberately rather than guessing at the layout.
pub(super) fn footer_right_span(left: &str, right: &str, width: usize) -> Option<(u16, u16)> {
    let occupied = Line::from(left).width() + Line::from(right).width() + 2;
    if right.is_empty() || occupied > width {
        return None;
    }
    let right_width = Line::from(right).width();
    Some((
        (width - right_width - 1) as u16,
        u16::try_from(right_width).unwrap_or(u16::MAX),
    ))
}

pub(super) fn compact_tokens(value: u64) -> String {
    if value >= 1_000_000 {
        format!("{:.1}M", value as f64 / 1_000_000.0)
    } else if value >= 1_000 {
        format!("{:.1}k", value as f64 / 1_000.0)
    } else {
        value.to_string()
    }
}

/// A truthful, fixed-width occupancy trace. Motion changes only the marker at
/// the measured boundary; it never changes how many cells appear filled.
pub(super) fn context_summary(
    tokens: ContextTokens,
    width: usize,
    tick: usize,
    moving: bool,
) -> String {
    let Some(cap) = tokens.cap else {
        return format!(
            "ctx {} / window ? · {}",
            compact_tokens(tokens.used),
            tokens.counted.as_str()
        );
    };
    let (bar, percent) = context_bar(tokens, width, tick, moving);
    format!(
        "ctx {bar} {}/{} {percent}%",
        compact_tokens(tokens.used),
        compact_tokens(cap)
    )
}

pub(super) fn context_bar(
    tokens: ContextTokens,
    width: usize,
    tick: usize,
    moving: bool,
) -> (String, u64) {
    let cap = tokens.cap.unwrap_or(0);
    let eighths = if cap == 0 {
        0
    } else {
        ((tokens.used.min(cap) as u128 * (width * 8) as u128) / cap as u128) as usize
    };
    let full = eighths / 8;
    let partial = eighths % 8;
    let parts = [' ', '▏', '▎', '▍', '▌', '▋', '▊', '▉'];
    let mut bar = String::with_capacity(width);
    for index in 0..width {
        if index < full {
            let glint = moving && full > 1 && index == tick % full;
            bar.push(if glint { '◆' } else { '━' });
        } else if index == full && partial > 0 {
            bar.push(parts[partial]);
        } else {
            bar.push('─');
        }
    }
    let percent = if cap == 0 {
        0
    } else {
        ((tokens.used.min(cap) as u128 * 100) / cap as u128) as u64
    };
    (bar, percent)
}
