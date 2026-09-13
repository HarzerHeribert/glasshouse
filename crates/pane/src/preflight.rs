//! The scouting preflight -- `smarter-cheaper-roadmap.md`, *Preflight Helper*
//! and *Adaptive orchestration*.
//!
//! The invariant: **the scout never impersonates the parent.** It is handed a
//! brief that quotes the request under a heading saying not to perform it and
//! asks for five sections of facts; the pilot's scouts that "did not run
//! Valgrind or modify files" were handed the raw request and tried to be the
//! parent. The second invariant: **direct execution stays the fast path.**
//! Under `PreflightScope::Auto` a task pays for a scout only when its request
//! carries an uncertainty signal [`should_scout`] can name.

use std::path::Path;

use crate::config::PreflightScope;
use crate::manifest::Manifest;

/// Whether to run the scout, and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    /// Run, for these signals (at least one).
    Run(Vec<&'static str>),
    /// Skip, for this reason.
    Skip(&'static str),
}

/// The signal names `Decision::Run` carries, spelled once so the machine
/// output and the inspector agree with the tests.
pub const SIGNAL_ALWAYS: &str = "preflight_scope = always";
pub const SIGNAL_MISSING_PATH: &str = "names a path that does not exist";
pub const SIGNAL_ABSENT_EXECUTABLE: &str = "names an executable the session lacks";
pub const SIGNAL_UNSEEN_VERIFICATION: &str = "asks for verification with no checks configured";
pub const SIGNAL_LONG_REQUEST: &str = "exceeds 80 words";

/// The one reason to skip: nothing above held.
pub const SKIP_NO_SIGNAL: &str = "no uncertainty signal: named paths exist, no absent executable or unseen verification, under 80 words";

/// A request longer than this is assumed to carry more than the parent should
/// orient on unaided.
pub const LONG_REQUEST_WORDS: usize = 80;

/// The most lines of the scout's five sections [`render`] carries.
pub const RENDER_LINE_BOUND: usize = 24;

/// The heading the brief quotes the request under, and the one the rendered
/// block leads with.
pub const REQUEST_HEADING: &str = "## Request (do not perform it)";
/// Ends the quoted request in the brief, so [`request_in`] needs no guess.
const ANSWER_HEADING: &str = "## Answer with exactly these five sections";
const RENDER_REQUEST_HEADING: &str = "## Request (verbatim, authoritative)";

/// The sentence that keeps a scout a scout, stated once in the brief.
pub const DO_NOT_PERFORM: &str = "You are scouting for the model that will act. Do not attempt the task, do not \
     build, test or fix anything, and do not report on work you did not do.";

/// The five sections, by exact heading, with what each may hold.
const SECTIONS: [(&str, &str); 5] = [
    (
        "## Constraints",
        "limits the request states or the environment imposes, as spans or facts",
    ),
    (
        "## Files",
        "the files the request concerns, as `path/to/file.rs:120 — what is there`",
    ),
    (
        "## Tests",
        "tests and check commands that already exist for this area, as spans",
    ),
    (
        "## Capabilities",
        "what the environment below does or does not provide that the request needs",
    ),
    (
        "## Risks",
        "facts that could make the request fail, each as a span or a fact you read",
    ),
];

/// Words that ask for verification the session may not be able to see.
const VERIFICATION_WORDS: [&str; 7] = [
    "test",
    "tests",
    "verify",
    "coverage",
    "benchmark",
    "valgrind",
    "pytest",
];

/// Dotted tokens that are prose, not paths.
const DOTTED_ABBREVIATIONS: [&str; 4] = ["e.g", "i.e", "etc", "vs"];

/// Phrases that make a section an open question rather than an answer.
const OPEN_QUESTION: [&str; 6] = [
    "could not determine",
    "cannot determine",
    "can't determine",
    "unable to determine",
    "could not find",
    "not sure",
];

/// Decide whether this task pays for a scout.
///
/// `Always` runs. `Auto` runs when any signal holds: a path-like token that
/// does not exist under the manifest's root, an executable the manifest lists
/// as absent, a verification word while no checks are configured, or a
/// request over [`LONG_REQUEST_WORDS`] words. A short request naming only
/// files that exist is the fast path and skips.
#[must_use]
pub fn should_scout(
    task: &str,
    manifest: &Manifest,
    scope: PreflightScope,
    checks_configured: bool,
) -> Decision {
    if scope == PreflightScope::Always {
        return Decision::Run(vec![SIGNAL_ALWAYS]);
    }
    let words: Vec<&str> = task.split_whitespace().collect();
    let mut signals = Vec::new();
    if words
        .iter()
        .map(|word| trim_token(word))
        .any(|token| path_like(token) && !exists(token, manifest))
    {
        signals.push(SIGNAL_MISSING_PATH);
    }
    if words
        .iter()
        .any(|word| names_absent_executable(word, manifest))
    {
        signals.push(SIGNAL_ABSENT_EXECUTABLE);
    }
    if !checks_configured && asks_for_verification(task, &words) {
        signals.push(SIGNAL_UNSEEN_VERIFICATION);
    }
    if words.len() > LONG_REQUEST_WORDS {
        signals.push(SIGNAL_LONG_REQUEST);
    }
    if signals.is_empty() {
        Decision::Skip(SKIP_NO_SIGNAL)
    } else {
        Decision::Run(signals)
    }
}

/// One line for the machine output and the `HELPERS` inspector.
#[must_use]
pub fn signals_summary(decision: &Decision) -> String {
    match decision {
        Decision::Run(signals) => format!("preflight: run ({})", signals.join("; ")),
        Decision::Skip(reason) => format!("preflight: skipped ({reason})"),
    }
}

/// The scout's input: the request quoted verbatim under a heading that says
/// not to perform it, the five sections asked for by exact heading, and the
/// manifest's environment lines so the scout knows what is readable and
/// available. [`DO_NOT_PERFORM`] is stated once.
#[must_use]
pub fn scouting_brief(task: &str, manifest: &Manifest) -> String {
    let mut brief = String::new();
    brief.push_str(DO_NOT_PERFORM);
    brief.push_str("\n\n");
    brief.push_str(REQUEST_HEADING);
    brief.push('\n');
    brief.push_str(task);
    brief.push_str("\n\n");
    brief.push_str(ANSWER_HEADING);
    brief.push_str(
        "\nBy these headings, one span or fact per line, and `(none found)` under a heading you \
         have nothing for. A span is `path/to/file.rs:120` followed by one short sentence in the \
         file's own words. Never list candidates you rejected and never pose a question back.\n",
    );
    for (heading, holds) in SECTIONS {
        brief.push('\n');
        brief.push_str(heading);
        brief.push('\n');
        brief.push_str(holds);
        brief.push('\n');
    }
    brief.push('\n');
    brief.push_str(&manifest.render());
    brief.push('\n');
    brief
}

/// The verbatim request inside a brief [`scouting_brief`] produced, so a
/// record can say what was asked rather than quoting the brief's first line.
#[must_use]
pub fn request_in(brief: &str) -> Option<&str> {
    let start = brief.find(REQUEST_HEADING)? + REQUEST_HEADING.len();
    let rest = brief[start..].strip_prefix('\n')?;
    let end = rest
        .find(&format!("\n\n{ANSWER_HEADING}"))
        .unwrap_or(rest.len());
    Some(rest[..end].trim_end())
}

/// The block appended to the system prompt: the request first and
/// authoritative, the scout's five sections bounded to [`RENDER_LINE_BOUND`]
/// lines, the files the session chose to serve (as `(path, contents)`), and
/// one line of record.
///
/// A section that reads as an open question renders as `(none found)`: a
/// question in the prompt gets answered and a list of rejected candidates is
/// a menu to browse, so neither is carried (`little-helpers.md`).
#[must_use]
pub fn render(task: &str, report: &str, served: &[(String, String)]) -> String {
    let sections = sections_of(report);
    let mut block = String::from("\n\n");
    block.push_str(RENDER_REQUEST_HEADING);
    block.push('\n');
    block.push_str(task);
    block.push('\n');

    let mut kept = 0usize;
    let mut cut = 0usize;
    let mut answered = 0usize;
    for (heading, lines) in &sections {
        block.push('\n');
        block.push_str(heading);
        block.push('\n');
        let lines: Vec<&str> = if lines.is_empty() || is_open_question(lines) {
            vec!["(none found)"]
        } else {
            answered += 1;
            lines.iter().map(String::as_str).collect()
        };
        let room = RENDER_LINE_BOUND.saturating_sub(kept);
        let carried = lines.len().min(room);
        cut += lines.len() - carried;
        if carried == 0 {
            block.push_str(&format!("({} lines cut)\n", lines.len()));
            continue;
        }
        for line in &lines[..carried] {
            block.push_str(line);
            block.push('\n');
        }
        kept += carried;
    }
    if cut > 0 {
        block.push_str(&format!(
            "\n({cut} lines of the scout's report were cut at the {RENDER_LINE_BOUND}-line bound)\n"
        ));
    }

    block.push_str(&format!("\n## Served in full ({})\n", served.len()));
    for (path, text) in served {
        block.push_str(&format!("### {path}\n```\n{}\n```\n\n", text.trim_end()));
    }
    block.push_str(&format!(
        "## Scouting record\nscout · {answered} of {} sections answered · {kept} lines carried · {cut} cut · {} spans named · {} served in full\n",
        SECTIONS.len(),
        spans(report).len(),
        served.len(),
    ));
    block
}

/// The `path:line` spans a scout named, in its own order, one entry per path
/// with the rest of that line as its single line of why.
///
/// Text is what a scout returns, so this reads lines: `src/a.rs:12 — why`
/// and `src/a.rs:12: why` are spans, a line naming no `path.ext:N` is prose
/// and is not one.
#[must_use]
pub fn spans(report: &str) -> Vec<(String, String)> {
    let mut named: Vec<(String, String)> = Vec::new();
    for line in report.lines() {
        let Some((path, why)) = span(line) else {
            continue;
        };
        if named.iter().any(|(seen, _)| *seen == path) {
            continue;
        }
        named.push((path, why));
    }
    named
}

/// The first `path:line` on one line, and the rest of that line as its why.
///
/// A path must carry an extension: `line 12:3` and a bare `Makefile:9` are
/// not spans, and serving nothing is the safe direction.
fn span(line: &str) -> Option<(String, String)> {
    let words: Vec<&str> = line.split_whitespace().collect();
    for (index, word) in words.iter().enumerate() {
        let token = word
            .trim_matches(|c: char| {
                !c.is_ascii_alphanumeric() && !matches!(c, '.' | '/' | '_' | '-' | ':')
            })
            .trim_end_matches([':', '.', ',', ';']);
        let Some((path, number)) = token.rsplit_once(':') else {
            continue;
        };
        if path.is_empty()
            || !path.contains('.')
            || number.is_empty()
            || !number.chars().all(|c| c.is_ascii_digit())
        {
            continue;
        }
        let rest = words[index + 1..].join(" ");
        let why = rest.trim_start_matches(['-', '—', '–', ':', ' ']).trim();
        return Some((
            path.to_string(),
            if why.is_empty() {
                "named by the scout.".to_string()
            } else {
                why.to_string()
            },
        ));
    }
    None
}

/// The five sections in [`SECTIONS`] order, each with its non-empty lines;
/// a heading the report lacks is present and empty. Text before the first
/// recognised heading belongs to no section and is not carried.
fn sections_of(report: &str) -> Vec<(&'static str, Vec<String>)> {
    let mut sections: Vec<(&'static str, Vec<String>)> = SECTIONS
        .iter()
        .map(|(heading, _)| (*heading, Vec::new()))
        .collect();
    let mut current: Option<usize> = None;
    for raw in report.lines() {
        let line = raw.trim();
        if let Some(index) = heading_index(line) {
            current = Some(index);
            continue;
        }
        if line.is_empty() {
            continue;
        }
        if let Some(index) = current {
            sections[index].1.push(line.to_string());
        }
    }
    sections
}

/// Which of the five headings this line is, tolerating `#` depth, a trailing
/// colon and case.
fn heading_index(line: &str) -> Option<usize> {
    let bare = line.trim_start_matches('#').trim().trim_end_matches(':');
    if bare.len() == line.len() {
        return None;
    }
    SECTIONS
        .iter()
        .position(|(heading, _)| heading.trim_start_matches("## ").eq_ignore_ascii_case(bare))
}

fn is_open_question(lines: &[String]) -> bool {
    lines.iter().any(|line| {
        let lower = line.to_ascii_lowercase();
        lower.ends_with('?') || OPEN_QUESTION.iter().any(|phrase| lower.contains(phrase))
    })
}

/// A word with its quoting and sentence punctuation removed: `` `gdb`. `` is
/// `gdb`, `src/lib.rs,` is `src/lib.rs`.
fn trim_token(word: &str) -> &str {
    word.trim_matches(|c: char| !c.is_ascii_alphanumeric() && !matches!(c, '.' | '/' | '_' | '-'))
        .trim_end_matches(['.', ',', ';', ':'])
}

/// A token is path-like when it holds a `/` or ends in a file extension: a
/// dotted last segment whose extension is one to eight alphanumerics with a
/// letter in it, and is not a spelled abbreviation.
fn path_like(token: &str) -> bool {
    if token.is_empty() || !token.chars().any(|c| c.is_ascii_alphabetic()) {
        return false;
    }
    if token.contains('/') {
        return token.len() > 1;
    }
    if DOTTED_ABBREVIATIONS
        .iter()
        .any(|abbreviation| token.eq_ignore_ascii_case(abbreviation))
    {
        return false;
    }
    let Some((stem, extension)) = token.rsplit_once('.') else {
        return false;
    };
    !stem.is_empty()
        && (1..=8).contains(&extension.len())
        && extension.chars().all(|c| c.is_ascii_alphanumeric())
        && extension.chars().any(|c| c.is_ascii_alphabetic())
}

/// Whether a path-like token exists: as given when absolute, and under the
/// manifest's root and each readable root otherwise. An empty root is not
/// joined, so a default manifest never resolves against the process's cwd.
fn exists(token: &str, manifest: &Manifest) -> bool {
    let path = Path::new(token);
    if path.is_absolute() && path.exists() {
        return true;
    }
    let relative = token.trim_start_matches('/');
    std::iter::once(manifest.root.as_str())
        .chain(manifest.readable_roots.iter().map(String::as_str))
        .filter(|root| !root.is_empty())
        .any(|root| Path::new(root).join(relative).exists())
}

fn names_absent_executable(word: &str, manifest: &Manifest) -> bool {
    let token = trim_token(word);
    !token.is_empty()
        && manifest.executables.iter().any(|executable| {
            executable.path.is_none() && executable.name.eq_ignore_ascii_case(token)
        })
}

fn asks_for_verification(task: &str, words: &[&str]) -> bool {
    task.to_ascii_lowercase().contains("cargo test")
        || words.iter().any(|word| {
            let token = trim_token(word).to_ascii_lowercase();
            VERIFICATION_WORDS.contains(&token.as_str())
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_dotted_abbreviation_and_a_version_number_are_not_paths() {
        assert!(!path_like("e.g"));
        assert!(!path_like("1.2.3"));
        assert!(!path_like("3.14"));
        assert!(path_like("main.rs"));
        assert!(path_like("src/lib"));
        assert!(path_like("/build"));
        assert!(!path_like("/"));
    }

    #[test]
    fn a_heading_is_recognised_at_any_depth_and_case() {
        assert_eq!(heading_index("## Files"), Some(1));
        assert_eq!(heading_index("### files:"), Some(1));
        assert_eq!(heading_index("Files"), None);
        assert_eq!(heading_index("## Reading"), None);
    }

    #[test]
    fn an_empty_manifest_root_never_resolves_against_the_cwd() {
        let manifest = Manifest::default();
        assert!(!exists("Cargo.toml", &manifest));
        assert!(!exists("src", &manifest));
    }
}
