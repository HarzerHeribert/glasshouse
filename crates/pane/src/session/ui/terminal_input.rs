use std::collections::VecDeque;
use std::io;
use std::time::Duration;

use crossterm::event::{
    self, Event, KeyCode, KeyEventKind, KeyModifiers, MouseEvent, MouseEventKind,
};

/// The longest tail an SGR mouse report can have: `[<`, three fields wide
/// enough for `u16::MAX`, two separators and the terminator, which is
/// `[<65535;65535;65535M`. Nothing longer can become a report, so nothing
/// longer is ever held — this is the bound, and it is a length rather than a
/// duration because waiting changes what a run *is* not at all.
const MAX_TAIL: usize = 20;

/// How long an Escape with nothing after it yet waits before it is handed to
/// the UI as a key press.
///
/// It decides one thing: whether the UI sees an Escape that belonged to a
/// report. It cannot decide whether report characters are typed, because the
/// matcher stays armed across the delivery — see [`TerminalInput::quiet`]. Too
/// short costs a stray Escape when a report is fragmented *and* slow, which
/// closes an open panel and otherwise does nothing; too long costs Escape
/// latency. Both answers are cheap, which is why a duration is allowed here
/// and nowhere else in this file.
const ESCAPE_GRACE: Duration = Duration::from_millis(20);

/// Reassembles SGR mouse reports that arrive split across terminal reads.
///
/// **The invariant: no character of an SGR mouse report is ever handed back as
/// text, however long its pieces take to arrive.** An SGR report is
/// `ESC [ < digits ; digits ; digits (M|m)`, so while the run after an Escape
/// is a strict prefix of that grammar it is unambiguously incomplete and is
/// held — with no timer, across as many reads as it takes, because no amount
/// of waiting changes what a prefix is. The moment a character arrives that
/// the grammar cannot accept, the run was never a report: it is released to
/// the editor at once, ahead of that character. A complete report is consumed
/// either way — the wheel scrolls, and a button report is dropped because no
/// arm of the caller's `Event::Mouse` match reads one.
///
/// The predecessor made this a race: a 20 ms grace that restarted on every
/// character, with a 200 ms ceiling, and a release into the editor when either
/// expired. It was green on this machine and red on a loaded CI runner, which
/// typed `[<65;101;28M` into the composer and sent it to a model.
#[derive(Default)]
pub(super) struct TerminalInput {
    /// Resolved events waiting to be handed to the caller, in arrival order.
    ready: VecDeque<Event>,
    hold: Hold,
}

/// What may still be assembling. `Idle` cannot become a report; `Open` is a
/// run that still can, holding the Escape that opened it until either the
/// report completes (the Escape is never delivered at all) or the grace
/// expires (it is delivered and `escape` becomes `None`, the matcher staying
/// armed for a tail that is merely late).
#[derive(Default)]
enum Hold {
    #[default]
    Idle,
    Open {
        escape: Option<Event>,
        tail: Vec<Event>,
    },
}

impl TerminalInput {
    /// True while resolved events are waiting; the caller must drain them
    /// before it blocks on the terminal again.
    pub(super) fn queued(&self) -> bool {
        !self.ready.is_empty()
    }

    /// Read one logical event, or `None` when the bytes read so far belong to
    /// a report that is not finished. The caller loops on `None`.
    pub(super) fn read(&mut self) -> io::Result<Option<Event>> {
        if let Some(event) = self.ready.pop_front() {
            return Ok(Some(event));
        }
        self.accept(event::read()?);
        if self.ready.is_empty() && self.escape_alone() && !event::poll(ESCAPE_GRACE)? {
            self.quiet();
        }
        Ok(self.ready.pop_front())
    }

    /// The stream went silent while a run was open.
    ///
    /// Only an Escape with *nothing* after it is resolved by silence: an
    /// Escape alone is already a complete key press, so it is delivered. The
    /// matcher stays armed, because silence is not evidence — on a loaded
    /// machine the rest of a fragmented report arrives after this, and it must
    /// still be recognised rather than typed. A run that has started its tail
    /// is not resolved by silence at all.
    fn quiet(&mut self) {
        if let Hold::Open { escape, tail } = &mut self.hold
            && tail.is_empty()
        {
            self.ready.extend(escape.take());
        }
    }

    fn escape_alone(&self) -> bool {
        matches!(&self.hold, Hold::Open { escape: Some(_), tail } if tail.is_empty())
    }

    /// Offer one event to the state machine. Everything it resolves is pushed
    /// to `ready`, in order; anything still ambiguous stays in `hold`.
    fn accept(&mut self, event: Event) {
        let Hold::Open { escape, mut tail } = std::mem::take(&mut self.hold) else {
            if is_plain_escape(&event) {
                self.hold = Hold::Open {
                    escape: Some(event),
                    tail: Vec::new(),
                };
            } else {
                self.ready.push_back(event);
            }
            return;
        };
        if report_char(&event).is_none() {
            // Not a character a report can contain: the run was text.
            self.ready.extend(escape);
            self.ready.extend(tail);
            self.accept(event);
            return;
        }
        if tail.len() == MAX_TAIL {
            // The bound, and the only place a run is dropped rather than
            // released. A run this long is not one any terminal emits, and
            // typing what could still have been a report is the worse of the
            // two wrong answers. Input resumes from the character that
            // overflowed.
            self.accept(event);
            return;
        }
        tail.push(event);
        match classify(&text(&tail)) {
            Sgr::Opening | Sgr::Body => self.hold = Hold::Open { escape, tail },
            Sgr::Report => {
                if let Some(mouse) = sgr_wheel_event(&text(&tail)) {
                    self.ready.push_back(Event::Mouse(mouse));
                }
            }
            Sgr::Other => {
                self.ready.extend(escape);
                self.ready.extend(tail);
            }
        }
    }
}

/// What the characters after an Escape can still turn into.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Sgr {
    /// The opening of a report and nothing yet committed: ``, `[`, `[<`.
    Opening,
    /// Committed: `[<` and a well-formed start of the numeric body.
    Body,
    /// `[<code;column;row` closed by `M` or `m`, all three fields well formed.
    Report,
    /// Cannot be an SGR mouse report, whatever arrives next.
    Other,
}

/// Classify a run against `[ < digits ; digits ; digits (M|m)`. Everything
/// that is not a prefix of that grammar is `Other`, and `Other` is decided as
/// early as the grammar allows — a fourth field, a non-digit, or an empty
/// field that is not the one still being typed all end the run at once rather
/// than waiting for a terminator that would only confirm it.
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
    let fields: Vec<&str> = fields.split(';').collect();
    if fields.len() > 3 {
        return Sgr::Other;
    }
    for (index, field) in fields.iter().enumerate() {
        let growing = !terminated && index + 1 == fields.len();
        if (field.is_empty() && !growing) || !field.bytes().all(|byte| byte.is_ascii_digit()) {
            return Sgr::Other;
        }
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

/// The character an event contributes to a run, or `None` if the event cannot
/// be part of a report at all. `M` is the only uppercase letter in the
/// grammar, so it is the only character crossterm reports with Shift.
fn report_char(event: &Event) -> Option<char> {
    let Event::Key(key) = event else { return None };
    if key.kind == KeyEventKind::Release {
        return None;
    }
    let KeyCode::Char(character) = key.code else {
        return None;
    };
    let shifted = character == 'M' && key.modifiers == KeyModifiers::SHIFT;
    (key.modifiers == KeyModifiers::NONE || shifted).then_some(character)
}

/// Every element of a held tail came through `report_char`, so none is lost.
fn text(tail: &[Event]) -> String {
    tail.iter().filter_map(report_char).collect()
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

    fn escape() -> Event {
        Event::Key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE))
    }

    /// The events crossterm produces for a run of printable bytes: `M` is the
    /// only one it reports with Shift.
    fn chars(text: &str) -> Vec<Event> {
        text.chars()
            .map(|character| {
                Event::Key(KeyEvent::new(
                    KeyCode::Char(character),
                    if character.is_uppercase() {
                        KeyModifiers::SHIFT
                    } else {
                        KeyModifiers::NONE
                    },
                ))
            })
            .collect()
    }

    /// An Escape and the printable tail a terminal sends after it.
    fn report(tail: &str) -> Vec<Event> {
        let mut events = vec![escape()];
        events.extend(chars(tail));
        events
    }

    /// Drive the reassembler over `stream` with the terminal falling silent
    /// after each index in `quiet_after`. Silence is the only thing a read
    /// boundary can change, and it is the one thing a PTY cannot be made to do
    /// on purpose — which is why the coverage lives here. Returns what the UI
    /// would receive, in order.
    fn drive(stream: &[Event], quiet_after: &[usize]) -> Vec<Event> {
        let mut input = TerminalInput::default();
        let mut seen = Vec::new();
        for (index, event) in stream.iter().enumerate() {
            input.accept(event.clone());
            if quiet_after.contains(&(index + 1)) {
                input.quiet();
            }
            seen.extend(input.ready.drain(..));
        }
        seen
    }

    /// What the composer would be left holding.
    fn typed(events: &[Event]) -> String {
        events
            .iter()
            .filter_map(|event| match event {
                Event::Key(key) => match key.code {
                    KeyCode::Char(character) => Some(character),
                    _ => None,
                },
                _ => None,
            })
            .collect()
    }

    fn scrolls(events: &[Event]) -> Vec<MouseEventKind> {
        events
            .iter()
            .filter_map(|event| match event {
                Event::Mouse(mouse) => Some(mouse.kind),
                _ => None,
            })
            .collect()
    }

    fn escapes(events: &[Event]) -> usize {
        events.iter().filter(|event| is_plain_escape(event)).count()
    }

    /// **The regression this rewrite exists for, at every boundary a read can
    /// fall on.** A report split anywhere, with the terminal then silent for as
    /// long as it likes, still reaches the UI as a mouse event and never as
    /// text. CI failed on the first boundary: the Escape alone, then the rest
    /// once the 20 ms grace had already released it.
    #[test]
    fn a_report_split_at_any_boundary_is_never_typed() {
        for (tail, scroll) in [
            ("[<65;101;28M", Some(MouseEventKind::ScrollDown)),
            ("[<64;1;2M", Some(MouseEventKind::ScrollUp)),
            ("[<0;10;5M", None),
            ("[<0;10;5m", None),
            ("[<65535;65535;65535M", None),
        ] {
            let stream = report(tail);
            let expected: Vec<MouseEventKind> = scroll.into_iter().collect();
            for split in 1..stream.len() {
                let seen = drive(&stream, &[split]);
                assert_eq!(typed(&seen), "", "{tail} split after {split}");
                assert_eq!(scrolls(&seen), expected, "{tail} split after {split}");
            }
        }
    }

    /// Three reads, with both pauses wherever the kernel happened to put them.
    #[test]
    fn a_report_split_three_ways_is_never_typed() {
        let stream = report("[<65;101;28M");
        for first in 1..stream.len() {
            for second in first + 1..stream.len() {
                let seen = drive(&stream, &[first, second]);
                assert_eq!(typed(&seen), "", "split after {first} and {second}");
                assert_eq!(
                    scrolls(&seen),
                    vec![MouseEventKind::ScrollDown],
                    "split after {first} and {second}"
                );
            }
        }
    }

    /// The exact shape CI hit: the Escape arrives alone, the runner is loaded,
    /// the grace expires and the Escape is delivered — and the tail that turns
    /// up afterwards still scrolls instead of being typed.
    #[test]
    fn a_tail_that_arrives_after_the_escape_was_delivered_still_scrolls() {
        let seen = drive(&report("[<65;101;28M"), &[1]);
        assert_eq!(typed(&seen), "");
        assert_eq!(scrolls(&seen), vec![MouseEventKind::ScrollDown]);
        assert_eq!(escapes(&seen), 1);
    }

    /// A run stops being a report the moment a character the grammar cannot
    /// accept arrives — at *any* position — and everything held is released to
    /// the editor at once, in order, behind the Escape that opened it. Nothing
    /// is released before that character, so holding costs no text and no
    /// reordering.
    #[test]
    fn a_run_that_stops_matching_is_released_whole_and_immediately() {
        let good = "[<65;101;28M";
        for cut in 0..good.len() {
            let text = format!("{}x", &good[..cut]);
            let stream = report(&text);
            assert!(
                drive(&stream[..stream.len() - 1], &[]).is_empty(),
                "released before the run was decided, cut {cut}"
            );
            let seen = drive(&stream, &[]);
            assert_eq!(typed(&seen), text, "cut {cut}");
            assert_eq!(escapes(&seen), 1, "cut {cut}");
            assert_eq!(seen.len(), text.len() + 1, "cut {cut}");
        }
    }

    /// Read whole, the report costs the UI nothing at all: no Escape, no text.
    #[test]
    fn a_report_read_whole_never_shows_the_ui_an_escape() {
        let seen = drive(&report("[<65;101;28M"), &[]);
        assert_eq!(scrolls(&seen), vec![MouseEventKind::ScrollDown]);
        assert_eq!(seen.len(), 1);
    }

    #[test]
    fn two_reports_in_one_read_are_both_consumed() {
        let mut stream = report("[<64;1;2M");
        stream.extend(report("[<65;3;4M"));
        let seen = drive(&stream, &[]);
        assert_eq!(
            scrolls(&seen),
            vec![MouseEventKind::ScrollUp, MouseEventKind::ScrollDown]
        );
        assert_eq!(typed(&seen), "");
        assert_eq!(escapes(&seen), 0);
    }

    #[test]
    fn text_after_a_report_in_the_same_read_reaches_the_editor_intact() {
        let mut stream = report("[<0;10;5M");
        stream.extend(chars("hello"));
        let seen = drive(&stream, &[]);
        assert_eq!(typed(&seen), "hello");
        assert_eq!(escapes(&seen), 0);
        assert!(scrolls(&seen).is_empty());
    }

    /// A real Escape key press, resolved by silence. With no silence at all it
    /// is released the moment the first character proves the run is not a
    /// report, so the answer does not depend on the pause either way.
    #[test]
    fn a_lone_escape_reaches_the_ui_and_the_text_after_it_is_intact() {
        let mut stream = vec![escape()];
        stream.extend(chars("hi"));
        for quiet in [vec![1usize], vec![]] {
            let seen = drive(&stream, &quiet);
            assert_eq!(typed(&seen), "hi", "quiet {quiet:?}");
            assert_eq!(escapes(&seen), 1, "quiet {quiet:?}");
            assert!(seen.first().is_some_and(is_plain_escape), "quiet {quiet:?}");
        }
    }

    /// After a delivered Escape the matcher stays armed — that is what makes a
    /// late tail safe — so a `[` typed next is held for exactly one character
    /// and released, in order, as soon as the next character decides it.
    #[test]
    fn a_bracket_typed_after_a_lone_escape_is_released_by_the_next_character() {
        let mut stream = vec![escape()];
        stream.extend(chars("[a]"));
        let seen = drive(&stream, &[1]);
        assert_eq!(typed(&seen), "[a]");
        assert_eq!(escapes(&seen), 1);
    }

    /// A prefix whose terminator never arrives is held, not typed, and silence
    /// anywhere in it changes nothing: waiting cannot decide what a prefix is.
    #[test]
    fn a_prefix_whose_terminator_never_arrives_is_never_typed() {
        let stream = report("[<65;101;28");
        for quiet in 1..=stream.len() {
            let seen = drive(&stream, &[quiet]);
            assert_eq!(typed(&seen), "", "quiet after {quiet}");
            assert!(scrolls(&seen).is_empty(), "quiet after {quiet}");
        }
    }

    /// Anything that cannot be part of a report ends the run, including an
    /// event carrying no character at all, and it arrives behind what was held.
    #[test]
    fn a_non_key_event_ends_a_run_and_follows_what_was_held() {
        let mut stream = report("[<65;101");
        stream.push(Event::Resize(80, 24));
        let seen = drive(&stream, &[]);
        assert_eq!(typed(&seen), "[<65;101");
        assert_eq!(escapes(&seen), 1);
        assert!(matches!(seen.last(), Some(Event::Resize(80, 24))));
    }

    /// The bound is a size, and it is the grammar's own maximum. A run that
    /// reaches it can never become a report, so it is dropped rather than
    /// typed, and input resumes from the character that overflowed.
    #[test]
    fn an_unterminated_prefix_is_dropped_at_the_size_bound() {
        assert_eq!("[<65535;65535;65535M".len(), MAX_TAIL);
        let digits = 30;
        let stream = report(&format!("[<{}", "1".repeat(digits)));
        let seen = drive(&stream, &[]);
        assert_eq!(typed(&seen), "1".repeat(digits - (MAX_TAIL - 2)));
        assert_eq!(escapes(&seen), 0);
    }

    #[test]
    fn the_grammar_admits_exactly_the_reports_and_their_prefixes() {
        for text in [
            "[<0;10;5M",
            "[<0;10;5m",
            "[<2;1;1M",
            "[<32;80;24m",
            "[<64;1;2M",
            "[<65;101;28M",
            "[<65535;65535;65535M",
        ] {
            assert_eq!(classify(text), Sgr::Report, "rejected {text}");
        }
        for text in ["", "[", "[<"] {
            assert_eq!(classify(text), Sgr::Opening, "committed {text}");
        }
        for text in ["[<0", "[<0;", "[<65;101", "[<65;101;28"] {
            assert_eq!(classify(text), Sgr::Body, "released {text}");
        }
        for text in [
            "[200~",
            "[<a",
            "[<;1;1",
            "[<1;2;3;",
            "[<1;2;3;4M",
            "[<65;1;2M;",
            "[<65;0;2M",
            "[<0;1;0M",
            "ok",
            "[<M",
            "literal[<65;101;28M",
        ] {
            assert_eq!(classify(text), Sgr::Other, "held {text}");
        }
    }

    #[test]
    fn only_a_wheel_press_becomes_a_scroll() {
        let down = sgr_wheel_event("[<65;101;28M").unwrap();
        assert_eq!(down.kind, MouseEventKind::ScrollDown);
        assert_eq!((down.column, down.row), (100, 27));
        let up = sgr_wheel_event("[<64;1;2M").unwrap();
        assert_eq!(up.kind, MouseEventKind::ScrollUp);
        assert_eq!((up.column, up.row), (0, 1));
        for text in [
            "[<65;101;28",
            "[<65;101;28;M",
            "[<65;0;2M",
            "[<0;1;2M",
            "[<65;1;2m",
            "[200~",
            "literal[<65;101;28M",
        ] {
            assert!(sgr_wheel_event(text).is_none(), "accepted {text}");
        }
    }

    /// A paste carries no report character, so it can never be swallowed.
    #[test]
    fn a_paste_is_never_mistaken_for_a_report() {
        let stream = vec![escape(), Event::Paste("[<65;101;28M".into())];
        let seen = drive(&stream, &[]);
        assert_eq!(escapes(&seen), 1);
        assert!(matches!(seen.last(), Some(Event::Paste(text)) if text == "[<65;101;28M"));
    }
}
