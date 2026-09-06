//! Decorative pixel stretching. This is activity artwork, never a rate chart.
use super::{ACCENT, ScreenState};
use ratatui::{
    Frame,
    layout::Rect,
    style::{Color, Style},
    text::{Line, Span},
    widgets::Paragraph,
};

pub(super) fn lines(width: usize, height: usize, state: &ScreenState) -> Vec<Line<'static>> {
    let rgb = match state.theme.accent() {
        Color::Rgb(r, g, b) => (r, g, b),
        Color::LightYellow => (255, 210, 100),
        Color::LightCyan => (100, 220, 240),
        _ => (225, 230, 232),
    };
    // The stripes are the source edge; stretching carries their colors intact.
    let palette = [
        Color::Rgb(rgb.0, rgb.1, rgb.2),
        Color::Rgb(239, 244, 240),
        Color::Rgb(rgb.2, rgb.0, rgb.1),
        Color::Rgb(rgb.0 / 2, rgb.1 / 2, rgb.2 / 2),
        Color::Rgb(rgb.0, rgb.1, rgb.2),
    ];
    let phase = if state.reduced_motion {
        0.8
    } else {
        state.animation_frame as f64 * 0.075
    };
    let pixels = height as f64 * 2.0;
    let radius = (pixels * 0.19).clamp(0.7, 3.8);
    let amplitude = (pixels / 2.0 - radius - 0.5).max(0.0);
    let arrival = state.pulse.deliveries.last().copied().unwrap_or(0).min(128) as f64 / 128.0;
    let pixel = |x: usize, y: usize| -> Option<Color> {
        let t = x as f64 / width.max(1) as f64;
        let eased = (t * 4.0).min(1.0);
        let bend = ((t * 6.4 - phase).sin() * 0.72
            + (t * 11.0 + phase * 0.67 + arrival).sin() * 0.28)
            * eased;
        let center = pixels / 2.0 + amplitude * bend;
        let edge = (y as f64 + 0.5 - center) / radius;
        if edge.abs() > 1.0 {
            return None;
        }
        let stripe = (((edge + 1.0) / 2.0) * palette.len() as f64) as usize;
        Some(palette[stripe.min(palette.len() - 1)])
    };
    (0..height)
        .map(|row| {
            Line::from(
                (0..width)
                    .map(|x| {
                        let top = pixel(x, row * 2);
                        let bottom = pixel(x, row * 2 + 1);
                        match (top, bottom) {
                            (Some(top), Some(bottom)) => {
                                Span::styled("▀", Style::default().fg(top).bg(bottom))
                            }
                            (Some(top), None) => Span::styled("▀", Style::default().fg(top)),
                            (None, Some(bottom)) => Span::styled("▄", Style::default().fg(bottom)),
                            (None, None) => Span::raw(" "),
                        }
                    })
                    .collect::<Vec<_>>(),
            )
        })
        .collect()
}

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
        Paragraph::new(format!(" {label}")).style(Style::default().fg(ACCENT)),
        Rect::new(area.x, area.y + area.height / 2, 13, 1),
    );
    frame.render_widget(
        Paragraph::new(lines(
            usize::from(area.width - 14),
            usize::from(area.height),
            state,
        )),
        Rect::new(area.x + 14, area.y, area.width - 14, area.height),
    );
}
