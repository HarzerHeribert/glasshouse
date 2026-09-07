use std::collections::VecDeque;
use std::io;
use std::time::{Duration, Instant};

use crossterm::event::{
    self, Event, KeyCode, KeyEventKind, KeyModifiers, MouseEvent, MouseEventKind,
};

const ESCAPE_GRACE: Duration = Duration::from_millis(20);
const MAX_TAIL_EVENTS: usize = 64;

/// Read one logical event while repairing the one crossterm boundary case
/// observed in a real PTY: a lone Escape event followed immediately by the
/// printable tail of an SGR mouse report.
pub(super) fn read(queue: &mut VecDeque<Event>) -> io::Result<Option<Event>> {
    if let Some(event) = queue.pop_front() {
        return Ok(Some(event));
    }
    let first = event::read()?;
    if !is_plain_escape(&first) {
        return Ok(Some(first));
    }

    let deadline = Instant::now() + ESCAPE_GRACE;
    let mut tail = Vec::new();
    while tail.len() < MAX_TAIL_EVENTS {
        let Some(left) = deadline.checked_duration_since(Instant::now()) else {
            break;
        };
        if !event::poll(left)? {
            break;
        }
        let next = event::read()?;
        tail.push(next);
        let Some(prefix) = key_text(&tail) else {
            break;
        };
        if !possible_sgr_mouse_prefix(&prefix) || prefix.ends_with('M') || prefix.ends_with('m') {
            break;
        }
    }
    if let Some(mouse) = sgr_wheel_event(&tail) {
        Ok(Some(Event::Mouse(mouse)))
    } else {
        queue.extend(tail);
        Ok(Some(first))
    }
}

fn is_plain_escape(event: &Event) -> bool {
    matches!(event, Event::Key(key) if key.kind != KeyEventKind::Release
        && key.code == KeyCode::Esc && key.modifiers == KeyModifiers::NONE)
}

fn key_text(events: &[Event]) -> Option<String> {
    let mut text = String::new();
    for event in events {
        let Event::Key(key) = event else { return None };
        if key.kind == KeyEventKind::Release
            || !(key.modifiers == KeyModifiers::NONE
                || (key.code == KeyCode::Char('M') && key.modifiers == KeyModifiers::SHIFT))
        {
            return None;
        }
        let KeyCode::Char(c) = key.code else {
            return None;
        };
        text.push(c);
    }
    Some(text)
}

fn possible_sgr_mouse_prefix(text: &str) -> bool {
    if text.is_empty() || text == "[" || text == "[<" {
        return true;
    }
    let Some(body) = text.strip_prefix("[<") else {
        return false;
    };
    body.bytes()
        .all(|byte| byte.is_ascii_digit() || byte == b';' || byte == b'M' || byte == b'm')
        && !body[..body.len().saturating_sub(1)]
            .bytes()
            .any(|byte| byte == b'M' || byte == b'm')
}

fn sgr_wheel_event(events: &[Event]) -> Option<MouseEvent> {
    let text = key_text(events)?;
    let body = text.strip_prefix("[<")?;
    let body = body.strip_suffix('M')?;
    let mut fields = body.split(';');
    let code = fields.next()?.parse::<u16>().ok()?;
    let column = fields.next()?.parse::<u16>().ok()?;
    let row = fields.next()?.parse::<u16>().ok()?;
    if fields.next().is_some() || column == 0 || row == 0 {
        return None;
    }
    let kind = match code {
        64 => MouseEventKind::ScrollUp,
        65 => MouseEventKind::ScrollDown,
        _ => return None,
    };
    Some(MouseEvent {
        kind,
        column: column - 1,
        row: row - 1,
        modifiers: KeyModifiers::NONE,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::KeyEvent;

    fn keys(text: &str) -> Vec<Event> {
        text.chars()
            .map(|c| {
                Event::Key(KeyEvent::new(
                    KeyCode::Char(c),
                    if c.is_uppercase() {
                        KeyModifiers::SHIFT
                    } else {
                        KeyModifiers::NONE
                    },
                ))
            })
            .collect()
    }

    #[test]
    fn fragmented_sgr_mouse_tail_is_recognized_exactly() {
        let down = sgr_wheel_event(&keys("[<65;101;28M")).unwrap();
        assert_eq!(down.kind, MouseEventKind::ScrollDown);
        assert_eq!((down.column, down.row), (100, 27));
        let up = sgr_wheel_event(&keys("[<64;1;2M")).unwrap();
        assert_eq!(up.kind, MouseEventKind::ScrollUp);
        assert_eq!((up.column, up.row), (0, 1));
        for malformed in [
            "[<65;101;28",
            "[<65;101;28;M",
            "[<65;0;2M",
            "[<0;1;2M",
            "[200~",
            "literal[<65;101;28M",
        ] {
            assert!(
                sgr_wheel_event(&keys(malformed)).is_none(),
                "accepted {malformed}"
            );
        }
        assert!(sgr_wheel_event(&[Event::Paste("[<65;101;28M".into())]).is_none());
    }
}
