//! The activity band: a raking dither sweep, in the transcript's own ramp.
//!
//! **This is the one thing on screen whose whole job is "is it alive".** It
//! carries no measurement -- the numbers live in the telemetry rail and the
//! status row -- so it may be decorative, but it may never read as stopped
//! while a turn is running. Everything below serves that one question.
//!
//! **It speaks the poster's language.** `░▒▓█` are [`super::poster`]'s ramp,
//! accent-coloured glyphs on the normal ground rather than a filled slab,
//! which is what keeps Mono legible: Mono's accent is white, and a white
//! background band would be a searchlight across the composer. The sine
//! ribbon this replaces baked its own RGB -- including a channel-swapped
//! second colour -- so it was the one surface that ignored the theme.
//!
//! **It reacts to arrivals rather than only to the clock.** The head advances
//! every frame, so a thinking turn that has produced nothing yet still moves;
//! and the band widens with the bytes actually delivered, so a turn that is
//! streaming hard looks different from one that is waiting on a first token.
//! A purely byte-driven band would freeze during thinking, which is the one
//! failure this surface cannot have.

use super::ScreenState;
use ratatui::{
    Frame,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
};

/// The dither ramp, lightest first. Four steps and no more: a hard band is
/// the house style, and a longer ramp would read as a gradient.
const RAMP: [char; 4] = ['░', '▒', '▓', '█'];

/// How far the head moves per frame. One column per frame at the 50ms draw
/// budget crosses an eighty-column band in four seconds, which reads as
/// purposeful rather than frantic.
const SPEED: usize = 1;

/// The band's width before arrivals widen it, and the most they may widen it
/// by. A floor, because a thinking turn with no deliveries still has to move
/// something; a ceiling, because a band as wide as the region is a fill and
/// not a sweep.
const CORE: usize = 6;
const SWELL: usize = 10;

/// How wide the band is this frame, given the bytes recently delivered.
///
/// Deliveries are the transport's own record of what arrived. A turn that is
/// thinking has none and gets [`CORE`]; one that is streaming swells toward
/// `CORE + SWELL`. The scale is deliberately coarse -- this is a sign of
/// life, not a rate chart, and the telemetry rail is where a number belongs.
fn band_width(deliveries: &[usize]) -> usize {
    let recent: usize = deliveries.iter().rev().take(4).sum();
    CORE + (recent / 96).min(SWELL)
}

/// The ramp step at `distance` columns behind the head, or `None` for ground.
///
/// The falloff is by whole steps rather than by a curve: the band is four
/// hard edges, which is what makes it read as print rather than as glow.
fn step(distance: usize, band: usize) -> Option<char> {
    if distance >= band {
        return None;
    }
    let bucket = distance * RAMP.len() / band.max(1);
    Some(RAMP[(RAMP.len() - 1).saturating_sub(bucket.min(RAMP.len() - 1))])
}

/// One row of the sweep.
///
/// Each row's head trails the one above it by `row * 2`, so the band rakes
/// across rather than marching square -- the diagonal is the only thing here
/// that is purely a look, and it costs nothing.
fn row(width: usize, head: usize, band: usize, offset: usize) -> String {
    let span = width + band;
    (0..width)
        .map(|x| {
            let head = (head + span - (offset % span)) % span;
            let distance = (x + span - head) % span;
            step(distance, band).unwrap_or(' ')
        })
        .collect()
}

/// The band, still or moving.
///
/// `reduced_motion` fixes the head rather than emptying the band: a still
/// screen still shows what the surface is, and a reader who turned motion off
/// asked for stillness, not for a blank strip.
pub(super) fn lines(width: usize, height: usize, state: &ScreenState) -> Vec<Line<'static>> {
    let band = band_width(&state.pulse.deliveries);
    let head = if state.reduced_motion {
        width / 3
    } else {
        state.animation_frame.wrapping_mul(SPEED)
    };
    let ink = Style::default().fg(state.theme.accent());
    (0..height)
        .map(|index| {
            Line::from(Span::styled(
                row(width, head, band, index * 2),
                if index == 0 {
                    ink.add_modifier(Modifier::BOLD)
                } else {
                    ink
                },
            ))
        })
        .collect()
}

/// The label column's width, including its leading space.
const LABEL: u16 = 14;

pub(super) fn activity(frame: &mut Frame, area: Rect, state: &ScreenState) {
    if area.width < 24 || area.height == 0 {
        return;
    }
    let label = if state.completion_tick.is_some() {
        "complete"
    } else {
        state.activity.label()
    };
    frame.render_widget(
        Paragraph::new(format!(" {}", label.to_uppercase())).style(
            Style::default()
                .fg(state.theme.accent())
                .add_modifier(Modifier::BOLD),
        ),
        Rect::new(area.x, area.y + area.height / 2, LABEL - 1, 1),
    );
    frame.render_widget(
        Paragraph::new(lines(
            usize::from(area.width - LABEL),
            usize::from(area.height),
            state,
        )),
        Rect::new(area.x + LABEL, area.y, area.width - LABEL, area.height),
    );
}

#[cfg(test)]
mod tests {
    use super::super::Theme;
    use super::*;

    fn theme_state(theme: Theme, frame: usize, still: bool, deliveries: Vec<usize>) -> ScreenState {
        let mut state = ScreenState {
            theme,
            animation_frame: frame,
            reduced_motion: still,
            ..Default::default()
        };
        state.pulse.deliveries = deliveries;
        state
    }

    #[test]
    fn the_band_moves_between_consecutive_frames() {
        let a = lines(60, 2, &theme_state(Theme::Neon, 7, false, vec![]));
        let b = lines(60, 2, &theme_state(Theme::Neon, 8, false, vec![]));
        assert_ne!(
            format!("{a:?}"),
            format!("{b:?}"),
            "a live band that does not move reads as a stalled session"
        );
    }

    #[test]
    fn a_thinking_turn_with_no_arrivals_still_moves() {
        let a = lines(60, 2, &theme_state(Theme::Neon, 3, false, vec![]));
        let b = lines(60, 2, &theme_state(Theme::Neon, 4, false, vec![]));
        assert_ne!(format!("{a:?}"), format!("{b:?}"));
    }

    #[test]
    fn reduced_motion_freezes_the_band_without_emptying_it() {
        let a = lines(60, 2, &theme_state(Theme::Neon, 3, true, vec![]));
        let b = lines(60, 2, &theme_state(Theme::Neon, 44, true, vec![]));
        assert_eq!(format!("{a:?}"), format!("{b:?}"));
        let drawn: String = a
            .iter()
            .flat_map(|line| line.spans.iter().map(|span| span.content.to_string()))
            .collect();
        assert!(
            drawn.chars().any(|c| RAMP.contains(&c)),
            "a still band must still show what it is: {drawn:?}"
        );
    }

    #[test]
    fn arrivals_widen_the_band_and_a_quiet_turn_keeps_the_core() {
        assert_eq!(band_width(&[]), CORE);
        let busy = band_width(&[400, 400, 400, 400]);
        assert!(busy > CORE, "streaming must look different: {busy}");
        assert!(
            busy <= CORE + SWELL,
            "a sweep must not become a fill: {busy}"
        );
    }

    #[test]
    fn every_row_is_exactly_the_width_it_was_given() {
        for width in [24usize, 46, 80, 160] {
            for height in [1usize, 2, 3] {
                for line in lines(
                    width,
                    height,
                    &theme_state(Theme::Amber, 11, false, vec![64]),
                ) {
                    assert_eq!(line.width(), width, "{width}x{height} drew past its region");
                }
            }
        }
    }

    #[test]
    fn every_theme_inks_the_band_with_its_own_accent_and_no_background() {
        for theme in Theme::ALL {
            for line in lines(48, 2, &theme_state(theme, 5, false, vec![])) {
                for span in &line.spans {
                    assert_eq!(span.style.fg, Some(theme.accent()), "{theme:?} off-theme");
                    assert_eq!(
                        span.style.bg, None,
                        "{theme:?} filled a slab; Mono would be a searchlight"
                    );
                }
            }
        }
    }
}
