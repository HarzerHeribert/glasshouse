//! The opening: a structure of panes, rotating into place.
//!
//! **The art is glyphs and the theme is the ink.** `assets/startup-panes.txt`
//! holds 26 frames of `░▒▓█` and not one colour, so all eight palettes draw
//! the same structure in their own accent. That is the same rule
//! [`super::poster`] keeps, and it is why the opening and the transcript read
//! as one program rather than two.
//!
//! **It never delays the session.** The frames are drawn into the transcript
//! region while the composer is already live; the caller advances `tick` on
//! the animation clock it already had and stops the moment a message exists.
//! [`skipped`] is what a keypress sets, and a skipped opening is the resolved
//! frame, not a blank -- a person who pressed a key wanted the session, not an
//! empty rectangle.
//!
//! **Motion off is the last frame.** It is the one the rotation settles on,
//! squared up and whole, so `/motion off` loses the movement and keeps the
//! picture. Nothing here carries information, so losing the movement costs a
//! reader nothing -- the standing rule that a frozen glyph must not take a
//! fact with it is satisfied by having no facts to take.
//!
//! The asset's provenance, and the command that regenerates it, is in
//! `crates/pane/assets/README.md`. Nothing runs at build or run time: the
//! frames are committed text.

use std::sync::OnceLock;

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::Paragraph;

use super::Theme;

/// The baked frames, form-feed separated.
const FRAMES: &str = include_str!("../../assets/startup-panes.txt");

/// Every frame, each as its own rows.
///
/// Parsed once. A frame is not padded to a rectangle here: rows are stored as
/// written and [`render`] centres each one, so a trailing-space run cannot
/// become a visible block on a themed ground.
fn frames() -> &'static [Vec<&'static str>] {
    static PARSED: OnceLock<Vec<Vec<&'static str>>> = OnceLock::new();
    PARSED.get_or_init(|| {
        FRAMES
            .split('\u{c}')
            .map(|frame| frame.trim_matches('\n').lines().collect::<Vec<_>>())
            .filter(|rows: &Vec<&str>| !rows.is_empty())
            .collect()
    })
}

/// The widest row and the tallest frame in the asset.
///
/// Both are needed before a single frame is chosen, because the opening must
/// decide whether it fits *at all* from the art's bounds rather than from
/// whichever frame happens to be current -- an opening that started and then
/// clipped on frame nine would be worse than one that never started.
pub(super) fn bounds() -> (usize, usize) {
    static BOUNDS: OnceLock<(usize, usize)> = OnceLock::new();
    *BOUNDS.get_or_init(|| {
        let frames = frames();
        let width = frames
            .iter()
            .flat_map(|rows| rows.iter().map(|row| row.chars().count()))
            .max()
            .unwrap_or(0);
        let height = frames.iter().map(Vec::len).max().unwrap_or(0);
        (width, height)
    })
}

/// How many frames the opening has.
fn count() -> usize {
    frames().len()
}

/// Whether the opening can be drawn in `width` by `height` at all.
///
/// It never crops and never wraps: an opening that lost its left column would
/// read as a rendering fault rather than as art. Below the art's own bounds
/// the caller draws nothing, and the session simply starts.
pub(super) fn fits(width: usize, height: usize) -> bool {
    let (art_width, art_height) = bounds();
    width >= art_width && height >= art_height
}

/// The frame this tick shows, resolved for the two states that are not a
/// rotation: motion off, and a skipped opening, both of which are the settled
/// last frame.
fn frame_at(tick: usize, still: bool) -> &'static [&'static str] {
    let frames = frames();
    let index = if still || tick >= frames.len() {
        frames.len() - 1
    } else {
        tick
    };
    &frames[index]
}

/// Whether the rotation has reached its settled frame.
///
/// **The opening holds rather than loops.** A structure that assembled and
/// then flew apart again to reassemble would read as a busy indicator, and
/// this is not one -- the session is what happens next, and the art should be
/// visibly finished waiting for it.
pub(super) fn settled(tick: usize) -> bool {
    tick + 1 >= count()
}

/// The opening's lines, centred in `width`, in the theme's accent.
///
/// `still` is `reduced_motion` or a keypress; both land on the settled frame.
pub(super) fn render(tick: usize, width: usize, theme: Theme, still: bool) -> Vec<Line<'static>> {
    let ink = Style::default()
        .fg(theme.accent())
        .add_modifier(Modifier::BOLD);
    frame_at(tick, still)
        .iter()
        .map(|row| {
            let pad = width.saturating_sub(row.chars().count()) / 2;
            Line::styled(format!("{}{}", " ".repeat(pad), row), ink)
        })
        .collect()
}

/// The opening, centred in the transcript while the session has nothing to
/// show yet.
///
/// **It declines rather than crops.** [`fits`] is asked about the
/// art's own bounds and not about the current frame, so an opening either
/// plays whole or never starts -- a clipped structure would read as a
/// rendering fault. Below its bounds the region stays empty and the session
/// simply begins, which is what a person typing `pane` came for anyway.
///
/// `still` is `reduced_motion` or a keypress; both land on the settled frame
/// rather than on a blank.
pub(super) fn render_into(frame: &mut Frame, area: Rect, tick: usize, theme: Theme, still: bool) {
    let width = usize::from(area.width);
    let height = usize::from(area.height);
    if !fits(width, height) {
        return;
    }
    let lines = render(tick, width, theme, still || settled(tick));
    let top = area.y + (area.height.saturating_sub(lines.len() as u16)) / 2;
    frame.render_widget(
        Paragraph::new(lines),
        Rect::new(area.x, top, area.width, area.bottom() - top),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_frame_parses_and_shares_the_arts_bounds() {
        let frames = frames();
        assert!(
            frames.len() > 8,
            "an opening needs frames: {}",
            frames.len()
        );
        let (width, height) = bounds();
        assert_eq!((width, height), (30, 14), "the asset's own bounds moved");
        for (index, rows) in frames.iter().enumerate() {
            assert!(
                rows.len() <= height,
                "frame {index} is {} rows, past the bound {height}",
                rows.len()
            );
            for row in rows {
                assert!(
                    row.chars().count() <= width,
                    "frame {index} has a row of {} past the bound {width}",
                    row.chars().count()
                );
            }
        }
    }

    #[test]
    fn a_still_opening_is_the_settled_frame_and_not_a_blank() {
        let last = frames().last().expect("frames");
        assert_eq!(frame_at(0, true), last.as_slice());
        assert_eq!(frame_at(3, true), last.as_slice());
        assert!(
            last.iter().any(|row| row.contains('█')),
            "the settled frame must actually carry the structure"
        );
    }

    #[test]
    fn the_rotation_moves_between_consecutive_ticks() {
        assert_ne!(
            frame_at(0, false),
            frame_at(1, false),
            "consecutive frames must differ or the opening reads as stalled"
        );
    }

    #[test]
    fn an_opening_never_draws_wider_than_the_width_it_was_given() {
        for width in [30usize, 44, 80, 120] {
            for tick in [0usize, 5, 25, 99] {
                for line in render(tick, width, Theme::Neon, false) {
                    assert!(
                        line.width() <= width,
                        "tick {tick} drew {} into {width}",
                        line.width()
                    );
                }
            }
        }
    }

    #[test]
    fn it_declines_rather_than_crops_when_the_terminal_is_too_small() {
        let (width, height) = bounds();
        assert!(fits(width, height));
        assert!(!fits(width - 1, height));
        assert!(!fits(width, height - 1));
    }

    #[test]
    fn every_theme_draws_the_structure_in_its_own_ink() {
        for theme in Theme::ALL {
            let lines = render(4, 40, theme, false);
            assert!(!lines.is_empty(), "{theme:?} drew nothing");
            for line in &lines {
                assert_eq!(
                    line.style.fg,
                    Some(theme.accent()),
                    "{theme:?} drew off-theme ink"
                );
                assert_eq!(
                    line.style.bg, None,
                    "{theme:?} filled a ground; Mono's accent is white and would blind"
                );
            }
        }
    }
}
