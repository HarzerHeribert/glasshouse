//! Render the actual shell to an ANSI file for local visual review.
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use glasshouse::shell::{ShellState, view};
use ratatui::{Terminal, backend::TestBackend, style::Color};
fn color(c: Color, bg: bool) -> String {
    let slot = if bg { 48 } else { 38 };
    match c {
        Color::Rgb(r, g, b) => format!("\x1b[{slot};2;{r};{g};{b}m"),
        Color::Black => format!("\x1b[{}m", if bg { 40 } else { 30 }),
        Color::White => format!("\x1b[{}m", if bg { 107 } else { 97 }),
        Color::Gray => format!("\x1b[{}m", if bg { 47 } else { 37 }),
        Color::DarkGray => format!("\x1b[{}m", if bg { 100 } else { 90 }),
        _ => format!("\x1b[{}m", if bg { 49 } else { 39 }),
    }
}
fn main() {
    let args: Vec<_> = std::env::args().collect();
    let width = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(140);
    let height = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(32);
    let mut state = ShellState::new(
        "pane-demo-project",
        "/workspace/pane-demo-project",
        "0.1.0",
        vec![],
    );
    for _ in 0..args.get(3).and_then(|s| s.parse().ok()).unwrap_or(0) {
        state.handle_key(KeyEvent::new(KeyCode::Char('t'), KeyModifiers::NONE));
    }
    let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
    terminal.draw(|frame| view::render(&state, frame)).unwrap();
    let b = terminal.backend().buffer();
    for y in 0..height {
        for x in 0..width {
            let cell = &b[(x, y)];
            print!(
                "{}{}{}",
                color(cell.fg, false),
                color(cell.bg, true),
                cell.symbol()
            );
        }
        println!("\x1b[0m");
    }
}
