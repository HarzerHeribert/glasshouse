use std::collections::VecDeque;
use std::io;
use std::time::{Duration, Instant};

use crossterm::event::{
    self, Event, KeyCode, KeyEventKind, KeyModifiers, MouseEvent, MouseEventKind,
};

/// How long a fragment may pause before it is judged to be typed text; the
/// clock restarts on every character that keeps the fragment a possible
/// report, so a report split into several reads is still reassembled.
const ESCAPE_GRACE: Duration = Duration::from_millis(20);
/// Ceiling on one reassembly. A stream that opens `ESC [ <` and never
/// terminates cannot hold the input loop, and the buffer cannot grow past
/// `MAX_TAIL_EVENTS`, whatever the sender does.
const MAX_TAIL_WAIT: Duration = Duration::from_millis(200);
const MAX_TAIL_EVENTS: usize = 64;

/// Read one logical event while repairing the one crossterm boundary case
/// observed in a real PTY: a lone Escape event followed immediately by the
/// printable tail of an SGR mouse report.
///
/// The invariant: **no part of an SGR mouse report is ever handed back as
/// text.** The tail is held until its terminator arrives — `M` for a press,
/// `m` for a release, and any button code, not only the wheel's — and a report
/// the UI has no handler for is dropped rather than typed. Only a tail that
/// cannot be a mouse report is queued for the editor behind the Escape.
pub(super) fn read(queue: &mut VecDeque<Event>) -> io::Result<Option<Event>> {
    if let Some(event) = queue.pop_front() {
        return Ok(Some(event));
    }
    let first = event::read()?;
    if !is_plain_escape(&first) {
        return Ok(Some(first));
    }

    let mut tail = Vec::new();
    let limit = Instant::now() + MAX_TAIL_WAIT;
    let mut deadline = Instant::now() + ESCAPE_GRACE;
    let mut shape = Sgr::Opening;
    while tail.len() < MAX_TAIL_EVENTS {
        let Some(left) = deadline.min(limit).checked_duration_since(Instant::now()) else {
            break;
        };
        if !event::poll(left)? {
            break;
        }
        tail.push(event::read()?);
        shape = key_text(&tail).map_or(Sgr::Other, |text| classify(&text));
        match shape {
            Sgr::Opening | Sgr::Body => deadline = Instant::now() + ESCAPE_GRACE,
            Sgr::Report | Sgr::Other => break,
        }
    }

    match shape {
        // Consumed either way: a wheel report scrolls, and a button report is
        // dropped because no arm of the caller's `Event::Mouse` match reads
        // one. Neither may reach the editor.
        Sgr::Report => Ok(key_text(&tail)
            .as_deref()
            .and_then(sgr_wheel_event)
            .map(Event::Mouse)),
        // A body that ran out of time is still a report, not something a hand
        // typed between two 20 ms ticks; dropping it is what keeps the
        // invariant total.
        Sgr::Body => Ok(None),
        Sgr::Opening | Sgr::Other => {
            queue.extend(tail);
            Ok(Some(first))
        }
    }
}

/// What the characters after a lone Escape can still turn into.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Sgr {
    /// The opening of a report and nothing yet committed: ``, `[`, `[<`.
    Opening,
    /// Committed: `[<` and the start of the numeric body.
    Body,
    /// `[<code;column;row` closed by `M` or `m`, all three fields well formed.
    Report,
    /// Cannot be an SGR mouse report.
    Other,
}

fn classify(text: &str) -> Sgr {
    if text.is_empty() || text == "[" || text == "[<" {
        return Sgr::Opening;
    }
    let Some(body) = text.strip_prefix("[<") else {
        return Sgr::Other;
    };
    let terminated = body.ends_with(['M', 'm']);
    let fields = if terminated {
        &body[..body.len() - 1]
    } else {
        body
    };
    if !fields
        .bytes()
        .all(|byte| byte.is_ascii_digit() || byte == b';')
    {
        return Sgr::Other;
    }
    match (terminated, parse_report(text)) {
        (false, _) => Sgr::Body,
        (true, Some(_)) => Sgr::Report,
        (true, None) => Sgr::Other,
    }
}

/// One SGR mouse report: `ESC [ < code ; column ; row` closed by `M` for a
/// press or `m` for a release. Coordinates are 1-based, so a zero is
/// malformed and not a report at all.
struct Report {
    code: u16,
    column: u16,
    row: u16,
    press: bool,
}

fn parse_report(text: &str) -> Option<Report> {
    let body = text.strip_prefix("[<")?;
    let press = body.ends_with('M');
    let body = body.strip_suffix(['M', 'm'])?;
    let mut fields = body.split(';');
    let code = fields.next()?.parse::<u16>().ok()?;
    let column = fields.next()?.parse::<u16>().ok()?;
    let row = fields.next()?.parse::<u16>().ok()?;
    if fields.next().is_some() || column == 0 || row == 0 {
        return None;
    }
    Some(Report {
        code,
        column,
        row,
        press,
    })
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

/// The wheel is reported as a press with code 64 or 65; every other code is a
/// button this UI does not handle.
fn sgr_wheel_event(text: &str) -> Option<MouseEvent> {
    let report = parse_report(text)?;
    if !report.press {
        return None;
    }
    let kind = match report.code {
        64 => MouseEventKind::ScrollUp,
        65 => MouseEventKind::ScrollDown,
        _ => return None,
    };
    Some(MouseEvent {
        kind,
        column: report.column - 1,
        row: report.row - 1,
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

    fn shape(text: &str) -> Sgr {
        classify(&key_text(&keys(text)).unwrap())
    }

    #[test]
    fn fragmented_sgr_mouse_tail_is_recognized_exactly() {
        let down = sgr_wheel_event("[<65;101;28M").unwrap();
        assert_eq!(down.kind, MouseEventKind::ScrollDown);
        assert_eq!((down.column, down.row), (100, 27));
        let up = sgr_wheel_event("[<64;1;2M").unwrap();
        assert_eq!(up.kind, MouseEventKind::ScrollUp);
        assert_eq!((up.column, up.row), (0, 1));
        for malformed in [
            "[<65;101;28",
            "[<65;101;28;M",
            "[<65;0;2M",
            "[<0;1;2M",
            "[<65;1;2m",
            "[200~",
            "literal[<65;101;28M",
        ] {
            assert!(sgr_wheel_event(malformed).is_none(), "accepted {malformed}");
        }
        assert!(key_text(&[Event::Paste("[<65;101;28M".into())]).is_none());
    }

    /// A click is a complete report whatever its button code and whichever of
    /// `M`/`m` closes it, so it is consumed rather than typed — the defect
    /// this classification exists to remove.
    #[test]
    fn every_terminated_report_is_consumed_and_every_prefix_is_held() {
        for report in [
            "[<0;10;5M",
            "[<0;10;5m",
            "[<2;1;1M",
            "[<32;80;24m",
            "[<64;1;2M",
            "[<65;101;28M",
        ] {
            assert_eq!(shape(report), Sgr::Report, "typed {report}");
        }
        for prefix in ["", "[", "[<"] {
            assert_eq!(shape(prefix), Sgr::Opening, "dropped {prefix}");
        }
        for prefix in ["[<0", "[<0;", "[<65;101", "[<65;101;28"] {
            assert_eq!(shape(prefix), Sgr::Body, "released {prefix}");
        }
        for text in ["[200~", "[<a", "[<65;1;2M;", "[<65;0;2M", "ok", "[<M"] {
            assert_eq!(shape(text), Sgr::Other, "swallowed {text}");
        }
    }
}
