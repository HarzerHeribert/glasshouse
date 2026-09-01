//! The deterministic half of the ladder — map line 1982, conservative by
//! construction (line 1983).
//!
//! One mechanism, applied at line granularity, covers every case the box
//! names: a duplicate search hit, a repeated log line, a repeated
//! test-progress line, and a repeated stack-trace line are all, at the
//! byte level, **the same exact line appearing again**. A stack trace that
//! recurs verbatim across several failures collapses because its own lines
//! are each exact repeats of the first occurrence's lines — no separate
//! "this looks like a stack trace" heuristic is needed or wanted, because a
//! heuristic is exactly the kind of guess line 1983 forbids: a line this
//! module cannot positively prove is a repeat is never touched.
//!
//! Two more rules, independently conservative: a run of blank lines
//! collapses to its first line, and a single unbroken, whitespace-free line
//! long enough to be generated noise (a base64 or hex dump on one line) has
//! its middle elided, prefix and suffix kept verbatim.
//!
//! Every byte this module forwards is a verbatim slice of the original —
//! `split_inclusive('\n')` hands out slices, not copies, and the blob rule
//! is the only place content is ever rewritten, and it rewrites with a
//! clearly marked elision note, never with generated replacement text (the
//! evidence ledger's "never generate evidence" constraint).

const BLOB_MIN_CHARS: usize = 500;
const BLOB_KEEP_CHARS: usize = 60;

/// One line-granular unit the deterministic ladder actually forwarded, kept
/// verbatim and addressed by a stable id — the seam Phase 57B's semantic
/// reducer plugs into (map lines 1997-2003). `id` is this candidate's
/// position among the *retained* output, in forwarding order, so a reducer
/// that names an id back can only ever refer to bytes this module itself
/// produced; nothing downstream ever rebuilds a result from anything but
/// this list (map line 1999's "never generate evidence" containment
/// guarantee, extended one stage further).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    pub id: usize,
    pub text: String,
}

/// The forwarded text plus how many line/run/blob decisions the ladder made
/// and how many of them survived byte-for-byte — the provenance header's
/// "retained/total candidate counts" (map line 1986). A blob elision counts
/// toward `total_candidates` but not `retained_candidates`: it forwarded
/// real bytes, but not all of them, so it is not "retained" in the sense
/// the header promises.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reduction {
    pub forwarded: String,
    pub total_candidates: usize,
    pub retained_candidates: usize,
    /// Every candidate this reduction actually forwarded, in the same order
    /// concatenating their `text` fields reproduces `forwarded` exactly.
    /// Populated even when nothing calls the semantic reducer — a `Vec` here
    /// costs nothing a passthrough-mode caller has to pay for, and having it
    /// unconditionally is what keeps `forwarded` and `candidates` from ever
    /// being computed by two different passes that could disagree.
    pub candidates: Vec<Candidate>,
}

/// Append `text` to both the forwarded body and the candidate list, as one
/// operation — the two must never drift, since [`Candidate::text`]'s whole
/// purpose is to be an exact slice of what `forwarded` actually carries.
fn forward(forwarded: &mut String, candidates: &mut Vec<Candidate>, text: &str) {
    forwarded.push_str(text);
    candidates.push(Candidate {
        id: candidates.len(),
        text: text.to_owned(),
    });
}

/// Run the deterministic ladder over `original`. Never called below the
/// passthrough threshold — [`crate::firewall::process`] owns that
/// decision.
pub fn reduce(original: &str) -> Reduction {
    let mut forwarded = String::with_capacity(original.len());
    let mut candidates = Vec::new();
    let mut seen_nonblank: std::collections::HashSet<&str> = std::collections::HashSet::new();
    let mut prev_line_was_blank = false;
    let mut total_candidates = 0usize;
    let mut retained_candidates = 0usize;

    for line in original.split_inclusive('\n') {
        let content = line.strip_suffix('\n').unwrap_or(line);
        let is_blank = content.trim().is_empty();

        if is_blank {
            total_candidates += 1;
            if prev_line_was_blank {
                // A later line in the same blank run: dropped, positively —
                // "blank" is decided by `trim().is_empty()`, nothing else.
            } else {
                forward(&mut forwarded, &mut candidates, line);
                retained_candidates += 1;
            }
            prev_line_was_blank = true;
            continue;
        }
        prev_line_was_blank = false;
        total_candidates += 1;

        if seen_nonblank.contains(content) {
            // An exact repeat of an earlier line. This is the whole rule:
            // the flagship needle fixture relies on a line seen exactly
            // once never landing here.
            continue;
        }
        seen_nonblank.insert(content);

        if content.len() >= BLOB_MIN_CHARS && !content.chars().any(char::is_whitespace) {
            let mut blob = String::new();
            elide_blob(&mut blob, line, content);
            forwarded.push_str(&blob);
            candidates.push(Candidate {
                id: candidates.len(),
                text: blob,
            });
            continue;
        }

        forward(&mut forwarded, &mut candidates, line);
        retained_candidates += 1;
    }

    Reduction {
        forwarded,
        total_candidates,
        retained_candidates,
        candidates,
    }
}

/// Rebuild a forwarded body from exactly the candidates in `keep`, in their
/// original order — map line 1999's "rebuild the final result from trusted
/// original candidates by id". `candidates` is always the untouched list
/// [`reduce`] produced; nothing here ever reads reducer-generated text, only
/// reducer-generated ids.
pub fn rebuild(candidates: &[Candidate], keep: &std::collections::HashSet<usize>) -> String {
    let mut out = String::new();
    for candidate in candidates {
        if keep.contains(&candidate.id) {
            out.push_str(&candidate.text);
        }
    }
    out
}

/// Keep `content`'s first and last [`BLOB_KEEP_CHARS`] bytes verbatim,
/// joined by a marker that cannot be mistaken for original content, and
/// preserve `line`'s own trailing newline (or its absence, on the file's
/// last line) exactly.
fn elide_blob(forwarded: &mut String, line: &str, content: &str) {
    let prefix_end = floor_char_boundary(content, BLOB_KEEP_CHARS);
    let suffix_start = ceil_char_boundary(content, content.len() - BLOB_KEEP_CHARS);
    let elided = suffix_start.saturating_sub(prefix_end);

    forwarded.push_str(&content[..prefix_end]);
    forwarded.push_str(&format!(
        "...[glasshouse context firewall: {elided} bytes of an unbroken blob elided]..."
    ));
    forwarded.push_str(&content[suffix_start..]);
    if line.len() > content.len() {
        forwarded.push('\n');
    }
}

fn floor_char_boundary(s: &str, index: usize) -> usize {
    let mut i = index.min(s.len());
    while i > 0 && !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

fn ceil_char_boundary(s: &str, index: usize) -> usize {
    let mut i = index.min(s.len());
    while i < s.len() && !s.is_char_boundary(i) {
        i += 1;
    }
    i
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_line_seen_once_always_survives() {
        let original = "unique line\n";
        let out = reduce(original);
        assert_eq!(out.forwarded, original);
        assert_eq!(out.total_candidates, 1);
        assert_eq!(out.retained_candidates, 1);
    }

    #[test]
    fn the_needle_survives_among_thousands_of_duplicate_hits() {
        let mut original = String::new();
        for _ in 0..3000 {
            original.push_str("src/generated/bundle.js: TODO cleanup\n");
        }
        original.push_str("src/app/handler.rs: TODO fix the race here\n");
        for _ in 0..3000 {
            original.push_str("src/generated/bundle.js: TODO cleanup\n");
        }

        let out = reduce(&original);
        assert!(
            out.forwarded
                .contains("src/app/handler.rs: TODO fix the race here"),
            "the needle line must survive dedup: {}",
            out.forwarded
        );
        // exactly one copy of the noise line, plus the needle
        assert_eq!(
            out.forwarded
                .matches("src/generated/bundle.js: TODO cleanup")
                .count(),
            1
        );
        assert_eq!(out.forwarded.matches("handler.rs").count(), 1);
        assert_eq!(out.total_candidates, 6001);
        assert_eq!(out.retained_candidates, 2);
    }

    #[test]
    fn a_repeated_stack_trace_collapses_to_one_copy() {
        let trace = "Traceback (most recent call last):\n  File \"a.py\", line 1\n  File \"b.py\", line 2\nValueError: boom\n";
        let original = trace.repeat(20);
        let out = reduce(&original);
        assert_eq!(out.forwarded, trace, "only the first occurrence survives");
    }

    #[test]
    fn repeated_test_progress_lines_collapse() {
        let mut original = String::new();
        for _ in 0..500 {
            original.push_str("PASS src/foo.test.ts\n");
        }
        let out = reduce(&original);
        assert_eq!(out.forwarded, "PASS src/foo.test.ts\n");
        assert_eq!(out.retained_candidates, 1);
        assert_eq!(out.total_candidates, 500);
    }

    #[test]
    fn a_run_of_blank_lines_collapses_to_one() {
        let original = "a\n\n\n\n\nb\n";
        let out = reduce(original);
        assert_eq!(out.forwarded, "a\n\nb\n");
    }

    #[test]
    fn a_single_blank_line_is_never_touched() {
        let original = "a\n\nb\n";
        let out = reduce(original);
        assert_eq!(out.forwarded, original);
    }

    #[test]
    fn an_unbroken_blob_is_elided_in_the_middle_and_kept_at_the_edges() {
        let blob = "a".repeat(20) + &"x".repeat(600) + &"b".repeat(20);
        let original = format!("{blob}\n");
        let out = reduce(&original);
        assert!(out.forwarded.starts_with(&"a".repeat(20)));
        assert!(out.forwarded.trim_end().ends_with(&"b".repeat(20)));
        assert!(out.forwarded.contains("elided"));
        assert!(out.forwarded.len() < original.len());
    }

    #[test]
    fn ordinary_long_lines_with_whitespace_are_never_treated_as_blobs() {
        let line = "word ".repeat(200); // long, but has whitespace throughout
        let original = format!("{line}\n");
        let out = reduce(&original);
        assert_eq!(out.forwarded, original);
    }

    #[test]
    fn every_forwarded_byte_is_a_verbatim_slice_of_the_original() {
        let original = "one\ntwo\none\nthree\n\n\nfour\n";
        let out = reduce(original);
        for line in out.forwarded.lines() {
            if line.contains("elided") {
                continue;
            }
            assert!(
                original.contains(line),
                "forwarded line `{line}` is not a substring of the original"
            );
        }
    }

    #[test]
    fn a_last_line_with_no_trailing_newline_is_preserved_when_not_a_blob() {
        let original = "one\ntwo";
        let out = reduce(original);
        assert_eq!(out.forwarded, original);
    }

    /// The seam Phase 57B's semantic reducer plugs into: every candidate's
    /// text, concatenated in id order, reproduces `forwarded` exactly.
    #[test]
    fn concatenating_every_candidate_in_id_order_reproduces_forwarded() {
        let original = "alpha\nalpha\nbeta\n\n\ngamma\n";
        let out = reduce(original);
        let mut ids: Vec<usize> = out.candidates.iter().map(|c| c.id).collect();
        ids.sort_unstable();
        assert_eq!(ids, (0..out.candidates.len()).collect::<Vec<_>>());

        let rejoined: String = out.candidates.iter().map(|c| c.text.as_str()).collect();
        assert_eq!(rejoined, out.forwarded);
    }

    #[test]
    fn rebuild_keeps_only_the_ids_named_and_preserves_order() {
        let original = "one\ntwo\nthree\n";
        let out = reduce(original);
        assert_eq!(out.candidates.len(), 3);

        let keep: std::collections::HashSet<usize> = [0, 2].into_iter().collect();
        assert_eq!(rebuild(&out.candidates, &keep), "one\nthree\n");
    }

    #[test]
    fn rebuild_from_an_empty_keep_set_is_empty() {
        let original = "one\ntwo\n";
        let out = reduce(original);
        assert_eq!(
            rebuild(&out.candidates, &std::collections::HashSet::new()),
            ""
        );
    }

    #[test]
    fn rebuild_ignores_an_id_the_original_never_had() {
        let original = "one\ntwo\n";
        let out = reduce(original);
        let keep: std::collections::HashSet<usize> = [0, 9999].into_iter().collect();
        assert_eq!(
            rebuild(&out.candidates, &keep),
            "one\n",
            "an unknown id must never invent content — it is simply never matched"
        );
    }
}
