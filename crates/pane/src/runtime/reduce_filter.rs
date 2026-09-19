//! Checking a filter the model wrote before its output is believed.
//!
//! A model-written filter is only safe because what it returns is checked
//! against what it was given. The check is the whole reason the design is an
//! improvement rather than a new way to be wrong: a reducer that retypes its
//! evidence can misquote it, and a filter that *selects* lines cannot —
//! provided something confirms it selected rather than composed.
//!
//! **Every line of a filter's output must occur verbatim in its input. There
//! is no exception, and that is deliberate.** The obvious design gives the
//! filter a way to say "N lines removed here", and that one carve-out is a
//! hole the size of the whole guarantee: anything a filter wants to invent,
//! it prefixes with an elision marker. So the filter returns selected lines
//! and nothing else, and [`elision`] is computed here, from the difference
//! between what went in and what came out, by code that had no part in
//! choosing.

use std::collections::HashSet;

use super::preview::{estimate_tokens, thousands};

/// Why a filter's output was not accepted.
///
/// The text is handed back to the model on its one retry, so each variant
/// says what to do differently rather than only what was wrong.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Rejected {
    /// Nothing, or only whitespace.
    Empty,
    /// A line that was not in the input. The first one, with its number.
    Composed { line: String, at: usize },
    /// More lines out than in: a filter must select, never repeat.
    Longer { output: usize, input: usize },
    /// A line the caller required is missing.
    Dropped(String),
    /// It ran, it selected, and it is still too big to be worth sending.
    TooLarge { tokens: usize, threshold: usize },
}

impl Rejected {
    /// The sentence the model is shown on its retry.
    #[must_use]
    pub fn sentence(&self) -> String {
        match self {
            Self::Empty => "your filter returned nothing. It must return the lines that \
                 carry the failures, selected from the text it was given."
                .to_string(),
            Self::Composed { line, at } => format!(
                "line {at} of your filter's output does not occur in the input: {line:?}. \
                 A filter selects whole lines from the text it is given and never \
                 composes, reformats or trims them. Return the line as it stands or \
                 leave it out."
            ),
            Self::Longer { output, input } => format!(
                "your filter returned {} lines from an input of {}. A filter removes \
                 lines; it never repeats them.",
                thousands(*output as u64),
                thousands(*input as u64),
            ),
            Self::Dropped(line) => format!(
                "your filter dropped a line the caller requires: {line:?}. Every line \
                 listed as one that must be kept has to appear in the output exactly as \
                 it is written there."
            ),
            Self::TooLarge { tokens, threshold } => format!(
                "your filter's output is still about {} tokens against a budget of {}. \
                 Select fewer lines: keep the distinct failures and drop the rest.",
                thousands(*tokens as u64),
                thousands(*threshold as u64),
            ),
        }
    }
}

/// How a filter's line is compared to the input's.
///
/// **Trailing whitespace only.** A line differing solely in trailing spaces
/// carries the same evidence and no editor or pipe preserves them reliably,
/// so requiring them would reject correct filters for nothing. Leading
/// whitespace is *not* forgiven: indentation is meaning in a diff, a nested
/// test report and a stack trace, and a filter that re-indents a line has
/// changed what it says about where it came from.
fn comparable(line: &str) -> &str {
    line.trim_end()
}

/// Check a filter's output against the text it was given.
///
/// `must_survive` is the caller's own list of lines that carry failures; the
/// model was shown all of them, so a filter that drops one is wrong rather
/// than unlucky.
pub fn validate(
    input: &str,
    output: &str,
    must_survive: &[String],
    threshold_tokens: usize,
) -> Result<(), Rejected> {
    if output.trim().is_empty() {
        return Err(Rejected::Empty);
    }

    let input_lines: HashSet<&str> = input.lines().map(comparable).collect();
    let output_lines: Vec<&str> = output.lines().collect();

    let input_count = input.lines().count();
    if output_lines.len() > input_count {
        return Err(Rejected::Longer {
            output: output_lines.len(),
            input: input_count,
        });
    }

    for (index, line) in output_lines.iter().enumerate() {
        if !input_lines.contains(comparable(line)) {
            return Err(Rejected::Composed {
                line: (*line).to_string(),
                at: index + 1,
            });
        }
    }

    let kept: HashSet<&str> = output_lines.iter().copied().map(comparable).collect();
    for required in must_survive {
        if !kept.contains(comparable(required)) {
            return Err(Rejected::Dropped(required.clone()));
        }
    }

    let tokens = estimate_tokens(output);
    if tokens > threshold_tokens {
        return Err(Rejected::TooLarge {
            tokens,
            threshold: threshold_tokens,
        });
    }
    Ok(())
}

/// What the filter removed, counted here rather than claimed there.
///
/// The filter cannot say this itself — a line it emits has to be a line it
/// was given, so it has no way to write a sentence about its own work. That
/// is the point: the number a reader relies on is computed by code that did
/// not choose what to keep.
#[must_use]
pub fn elision(input: &str, output: &str) -> String {
    let input_lines = input.lines().count();
    let output_lines = output.lines().count();
    let removed = input_lines.saturating_sub(output_lines);
    format!(
        "… {} of {} lines not selected by the filter; `stdout` and `stderr` on this \
         result are complete and unchanged …",
        thousands(removed as u64),
        thousands(input_lines as u64),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const LOG: &str = "\
Compiling thing v1.0
test a ... ok
test b ... ok
test c ... FAILED
assertion failed: left == right
test result: FAILED. 2 passed; 1 failed
";

    fn must() -> Vec<String> {
        vec!["test c ... FAILED".to_string()]
    }

    #[test]
    fn a_filter_that_selects_real_lines_is_accepted() {
        let output = "test c ... FAILED\nassertion failed: left == right\n";
        assert_eq!(validate(LOG, output, &must(), 4_000), Ok(()));
    }

    /// The guarantee. A line that reads like a perfectly good summary, and
    /// never occurred, is refused on that ground alone.
    #[test]
    fn a_line_the_input_never_held_is_refused_however_true_it_looks() {
        let output = "test c ... FAILED\n1 test failed out of 3\n";
        let Err(Rejected::Composed { line, at }) = validate(LOG, output, &must(), 4_000) else {
            panic!("a composed line must be refused");
        };
        assert_eq!(line, "1 test failed out of 3");
        assert_eq!(at, 2);
        assert!(line_is_plausible(&line));
    }

    /// A composed line is refused for not occurring, never for looking
    /// wrong — so the check cannot be talked out of by a better-worded lie.
    fn line_is_plausible(line: &str) -> bool {
        !line.is_empty()
    }

    #[test]
    fn an_elision_marker_is_no_longer_a_way_in() {
        // The shape a carve-out would have admitted.
        let output = "test c ... FAILED\n… and 40 similar failures elsewhere …\n";
        assert!(
            matches!(
                validate(LOG, output, &must(), 4_000),
                Err(Rejected::Composed { .. })
            ),
            "a filter has no marker of its own, so this is simply a composed line",
        );
    }

    #[test]
    fn a_filter_that_drops_a_required_line_is_refused() {
        let output = "assertion failed: left == right\n";
        assert_eq!(
            validate(LOG, output, &must(), 4_000),
            Err(Rejected::Dropped("test c ... FAILED".to_string()))
        );
    }

    #[test]
    fn trailing_space_is_forgiven_and_indentation_is_not() {
        let trailing = "test c ... FAILED   \nassertion failed: left == right\n";
        assert_eq!(validate(LOG, trailing, &must(), 4_000), Ok(()));

        let reindented = "  test c ... FAILED\n";
        assert!(
            matches!(
                validate(LOG, reindented, &must(), 4_000),
                Err(Rejected::Composed { .. })
            ),
            "indentation is meaning in a diff and a trace",
        );
    }

    #[test]
    fn a_filter_that_repeats_lines_is_refused() {
        let output = LOG.repeat(2);
        assert!(matches!(
            validate(LOG, &output, &must(), 40_000),
            Err(Rejected::Longer { .. })
        ));
    }

    #[test]
    fn an_empty_filter_is_refused() {
        assert_eq!(validate(LOG, "  \n ", &must(), 4_000), Err(Rejected::Empty));
    }

    #[test]
    fn a_selection_still_over_budget_is_refused_with_its_numbers() {
        let big: String = (0..2_000).map(|n| format!("line {n}\n")).collect();
        let Err(Rejected::TooLarge { tokens, threshold }) = validate(&big, &big, &[], 100) else {
            panic!("an over-budget selection is refused");
        };
        assert!(tokens > threshold);
    }

    #[test]
    fn every_refusal_tells_the_model_what_to_do_differently() {
        for rejected in [
            Rejected::Empty,
            Rejected::Composed {
                line: "x".into(),
                at: 1,
            },
            Rejected::Longer {
                output: 2,
                input: 1,
            },
            Rejected::Dropped("y".into()),
            Rejected::TooLarge {
                tokens: 10,
                threshold: 5,
            },
        ] {
            let sentence = rejected.sentence();
            assert!(sentence.len() > 40, "{rejected:?} says too little");
            assert!(
                sentence.contains("filter"),
                "{rejected:?} must name what it is about",
            );
        }
    }

    #[test]
    fn the_elision_is_counted_here_not_claimed_by_the_filter() {
        let output = "test c ... FAILED\n";
        let note = elision(LOG, output);
        assert!(note.contains("5 of 6 lines"), "{note}");
        assert!(note.contains("complete and unchanged"));
    }
}
