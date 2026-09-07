//! Local presentation only. Artwork carries no rate or progress measurement.
use ratatui::{
    style::{Color, Style},
    text::{Line, Span},
};

#[derive(Clone, Copy, Default)]
pub(super) struct Theme(u8);
impl Theme {
    pub(super) fn next(self) -> Self {
        Self((self.0 + 1) % 8)
    }
    pub(super) fn name(self) -> &'static str {
        [
            "neon", "amber", "ice", "violet", "cobalt", "mint", "rose", "mono",
        ][self.0 as usize]
    }
    pub(super) fn accent(self) -> Color {
        let (r, g, b) = [
            (176, 255, 94),
            (255, 197, 91),
            (98, 223, 250),
            (190, 148, 255),
            (108, 161, 255),
            (102, 239, 187),
            (255, 147, 183),
            (225, 230, 232),
        ][self.0 as usize];
        Color::Rgb(r, g, b)
    }
    pub(super) fn secondary(self) -> Color {
        match self.0 {
            0 => Color::Rgb(102, 210, 224),
            7 => Color::Gray,
            _ => Color::Rgb(208, 223, 219),
        }
    }
    pub(super) fn quiet(self) -> Color {
        Color::Rgb(134, 151, 157)
    }
}

pub(super) fn ribbon(width: u16, height: u16, tick: u64, theme: Theme) -> Vec<Line<'static>> {
    let Color::Rgb(r, g, b) = theme.accent() else {
        return vec![];
    };
    let palette = [
        theme.accent(),
        Color::Rgb(235, 240, 233),
        theme.secondary(),
        Color::Rgb(r / 2, g / 2, b / 2),
    ];
    let pixels = f64::from(height) * 2.0;
    let radius = (pixels * 0.2).clamp(0.7, 3.8);
    let amplitude = (pixels / 2.0 - radius - 0.5).max(0.0);
    let phase = tick as f64 * 0.075;
    let pixel = |x: u16, y: u16| {
        let t = f64::from(x) / f64::from(width.max(1));
        let bend = ((t * 6.4 - phase).sin() * 0.72 + (t * 11.0 + phase * 0.67).sin() * 0.28)
            * (t * 4.0).min(1.0);
        let edge = (f64::from(y) + 0.5 - pixels / 2.0 - amplitude * bend) / radius;
        if edge.abs() > 1.0 {
            None
        } else {
            Some(palette[(((edge + 1.0) * 2.0) as usize).min(3)])
        }
    };
    (0..height)
        .map(|row| {
            Line::from(
                (0..width)
                    .map(|x| match (pixel(x, row * 2), pixel(x, row * 2 + 1)) {
                        (Some(top), Some(bottom)) => {
                            Span::styled("▀", Style::default().fg(top).bg(bottom))
                        }
                        (Some(top), None) => Span::styled("▀", Style::default().fg(top)),
                        (None, Some(bottom)) => Span::styled("▄", Style::default().fg(bottom)),
                        _ => Span::raw(" "),
                    })
                    .collect::<Vec<_>>(),
            )
        })
        .collect()
}
