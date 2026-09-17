//! The composer's text: where the caret is drawn, and which character a click
//! at a given row and column means.

use ratatui::style::Style;
use ratatui::text::{Line, Span};

use super::{ACCENT, ScreenState, wrap_lines};

pub(super) fn composer_cursor(input: &str, offset: usize, width: u16) -> (usize, usize) {
    let mut offset = offset.min(input.len());
    while !input.is_char_boundary(offset) {
        offset -= 1;
    }
    let prefix = format!("{} ", &input[..offset]);
    let rows = wrap_lines(
        prefix
            .split('\n')
            .map(|line| Line::from(line.to_string()))
            .collect(),
        width,
    );
    (
        rows.len().saturating_sub(1),
        rows.last()
            .map(|line| line.width().saturating_sub(1))
            .unwrap_or(0),
    )
}

/// The offset in `input` that [`composer_cursor`] would draw at `row` and
/// `column` -- the inverse a click needs, by binary search over the same
/// function, because the wrap is that function's answer and not a rule this
/// one should restate.
///
/// A click past the end of a row lands at the end of that row, and a click
/// below the last row lands at the end of the text: both are where a person
/// pointing at empty space means to put the caret.
pub(crate) fn composer_offset(input: &str, row: usize, column: u16, width: u16) -> usize {
    let target = (row, usize::from(column));
    let mut low = 0usize;
    let mut high = input.len();
    while low < high {
        let mut middle = low + (high - low).div_ceil(2);
        while !input.is_char_boundary(middle) {
            middle -= 1;
        }
        if middle == low {
            break;
        }
        if composer_cursor(input, middle, width) <= target {
            low = middle;
        } else {
            high = middle - 1;
            while !input.is_char_boundary(high) {
                high -= 1;
            }
        }
    }
    low
}

pub(super) fn wrapped_input(state: &ScreenState, width: u16) -> Vec<Line<'static>> {
    // The masked prompt's arm is the whole reason this function takes the
    // state rather than the text: `mask()` is the only spelling of an entered
    // secret that exists outside the prompt itself.
    let text = match state.secret_prompt.as_ref() {
        Some(prompt) if prompt.is_empty() => "the key is not shown as you type".to_string(),
        Some(prompt) => prompt.mask(),
        None if state.input.is_empty() => "message or / for commands".to_string(),
        None if state.cursor == Some(state.input.len()) => format!("{} ", state.input),
        None => state.input.clone(),
    };
    wrap_lines(
        text.split('\n')
            .map(|line| Line::from(line.to_string()))
            .collect(),
        width.saturating_sub(2),
    )
    .into_iter()
    .enumerate()
    .map(|(i, mut line)| {
        line.spans.insert(
            0,
            Span::styled(
                if i == 0 { "› " } else { "│ " },
                Style::default().fg(ACCENT),
            ),
        );
        line
    })
    .collect()
}
