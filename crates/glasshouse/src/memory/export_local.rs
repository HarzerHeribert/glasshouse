//! `glasshouse memory export-local` — Phase 58 item 6, map line 2040: *"An
//! opt-in export of remembered constraints and failed approaches into a
//! marker-delimited block of the harness's native local instruction file,
//! gitignored by default, replacing only its own block on re-export."*
//! A sibling verb, not `export`: [`super::export::TrackedKnowledge`] is
//! Phase 50's projection into `.glasshouse/knowledge/`, a **tracked**
//! directory reviewed through an ordinary Git workflow, while this module
//! writes `CLAUDE.local.md`, the harness's own **local**, conventionally
//! untracked instruction file read at launch. The two share no flag and no
//! destination. Never merge them.
//! Reuses `super::inject::render_entry` rather than a copy, so an exported
//! entry and an injected one are the same shape byte for byte; `render_entry`,
//! `standing` and `quote` were widened from private to `pub(crate)` for it.
//! Nothing in this module runs unless `glasshouse memory export-local` is
//! typed: no hook, no launch-time call, no timer. It reads
//! [`super::MemoryStore::binding`] and [`super::MemoryStore::current_of_kind`],
//! both already scoped to the active project. Every byte outside the marker
//! block is copied forward unchanged, and the user's own `.gitignore` is
//! never opened for writing, only read.
//! History: design-decisions.md, "Trims: memory and session module docs", memory/export_local.rs module doc.

use std::path::{Path, PathBuf};

use super::store::{MemoryAuthority, MemoryRecord};

/// Opens the exported block. Anything between this and
/// [`MARKER_END`] is regenerated on every export; anything outside it is the
/// user's own and is never touched.
pub const MARKER_BEGIN: &str = "<!-- glasshouse:memory:begin -->";
/// Closes the exported block — see [`MARKER_BEGIN`].
pub const MARKER_END: &str = "<!-- glasshouse:memory:end -->";

/// A harness this build knows how to export into, and the one decision this
/// module makes about it: whether it has a **local** instruction file at
/// all.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LocalHarness {
    /// Reads `CLAUDE.local.md` at the project root — conventionally
    /// untracked, and the only harness this build exports into.
    ClaudeCode,
}

impl LocalHarness {
    /// The slug this command defaults `--harness` to when none is given.
    /// Claude Code is the only harness with a native local instruction file
    /// in this build, so it is also the only sensible default; see the
    /// module documentation for why "the project's configured default
    /// harness" collapses to this one name today.
    pub const DEFAULT_SLUG: &'static str = "claude-code";

    /// Parse a `--harness` value, refusing by name any harness this build
    /// knows but which has no *local* instruction file — line 2040's own
    /// distinction between a file the user opts into and one the repository
    /// tracks.
    pub fn parse(slug: &str) -> Result<Self, ExportLocalError> {
        match slug {
            "claude-code" => Ok(Self::ClaudeCode),
            "codex" => Err(ExportLocalError::UnsupportedHarness {
                harness: slug.to_owned(),
                reason: "codex reads a tracked `AGENTS.md`; a tracked instruction file is not a \
                         local one, and `memory export-local` is not for the repository"
                    .to_owned(),
            }),
            "gemini-cli" => Err(ExportLocalError::UnsupportedHarness {
                harness: slug.to_owned(),
                reason: "gemini-cli reads a tracked `GEMINI.md`; a tracked instruction file is \
                         not a local one, and `memory export-local` is not for the repository"
                    .to_owned(),
            }),
            other => Err(ExportLocalError::UnsupportedHarness {
                harness: other.to_owned(),
                reason: "no harness by this name has a native local instruction file Glasshouse \
                         can export into"
                    .to_owned(),
            }),
        }
    }

    /// The file, relative to the project root, this harness reads at
    /// launch.
    pub fn local_instruction_file(self) -> &'static str {
        match self {
            Self::ClaudeCode => "CLAUDE.local.md",
        }
    }
}

/// Failures exporting into a harness's local instruction file.
#[derive(Debug, thiserror::Error)]
pub enum ExportLocalError {
    #[error("harness `{harness}` has no native local instruction file to export into: {reason}")]
    UnsupportedHarness { harness: String, reason: String },
    #[error("could not {action} `{path}`")]
    Io {
        action: &'static str,
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

/// What adding the target file to the repository's exclude list did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExcludeAction {
    /// The pattern was not present anywhere and is now in
    /// `.git/info/exclude`.
    Added,
    /// The file was already ignored, by `.git/info/exclude` or by
    /// `.gitignore` — nothing was written.
    AlreadyExcluded,
    /// `--no-exclude` was given; neither file was read for writing.
    Skipped,
    /// The project root has no `.git` directory, so there is no exclude
    /// list to add to.
    NotGitRepo,
}

/// What one `export-local` run did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outcome {
    /// The harness's local instruction file, relative to the project root
    /// joined on.
    pub path: PathBuf,
    /// How many memories the block now holds. Zero means the block was
    /// removed (or never existed).
    pub exported: usize,
    /// Whether the file now carries a marker block at all.
    pub block_present: bool,
    pub exclude: ExcludeAction,
}

/// Export `records` — already read from the store by the caller — into
/// `harness_slug`'s local instruction file under `root`.
///
/// `records` is filtered here, not trusted as pre-filtered: only
/// [`MemoryRecord::is_current`] records survive, and Phase 27's own
/// exclusion — an [`MemoryAuthority::Idea`] nobody has reaffirmed
/// (`last_validated_at.is_none()`) — is applied exactly as
/// [`super::inject::briefing`] applies it, so an idea that would never reach
/// a session at launch does not reach one through this file either.
pub fn export(
    root: &Path,
    harness_slug: &str,
    records: &[MemoryRecord],
    now: i64,
    exclude: bool,
) -> Result<Outcome, ExportLocalError> {
    let harness = LocalHarness::parse(harness_slug)?;
    let filename = harness.local_instruction_file();
    let path = root.join(filename);

    let selected: Vec<&MemoryRecord> = records
        .iter()
        .filter(|record| record.is_current() && !is_unreaffirmed_idea(record))
        .collect();

    let block = if selected.is_empty() {
        None
    } else {
        let total = selected.len();
        let mut lines = Vec::with_capacity(total + 1);
        lines.push(header_line(now, total));
        for (index, record) in selected.iter().enumerate() {
            // No association and no freshness: an export is not a
            // file-aware retrieval — it names no file — so both fields would
            // be claims about a relationship this caller never established.
            lines.push(super::inject::render_entry(
                index + 1,
                total,
                record,
                None,
                None,
            ));
        }
        Some(lines.join("\n"))
    };
    let exported = selected.len();

    let original = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(source) => {
            return Err(ExportLocalError::Io {
                action: "read",
                path,
                source,
            });
        }
    };

    if let Some(new_content) = splice(&original, block.as_deref()) {
        write_atomic(&path, &new_content)?;
    }
    let block_present = block.is_some();

    let exclude = if exclude {
        ensure_excluded(root, filename)?
    } else {
        ExcludeAction::Skipped
    };

    Ok(Outcome {
        path,
        exported,
        block_present,
        exclude,
    })
}

/// Line 934's exclusion, reproduced rather than called: `is_unreaffirmed_idea`
/// is private to [`super::inject`] and this package's only sanctioned
/// visibility change there is `render_entry`, `standing` and `quote` (see
/// the module documentation). The predicate itself is one line and this is
/// its only other reader.
fn is_unreaffirmed_idea(record: &MemoryRecord) -> bool {
    record.authority == Some(MemoryAuthority::Idea) && record.last_validated_at.is_none()
}

/// The one line every block opens with — what it is, when it was written,
/// and that it regenerates.
fn header_line(now: i64, exported: usize) -> String {
    format!(
        "{exported} current binding memories and failed attempts from this project's \
         Glasshouse record, exported by `glasshouse memory export-local` at unix time {now}. \
         Regenerated on every export; everything outside this block is yours."
    )
}

/// Wrap `inner` — the header line plus every entry, one per line — between
/// the markers, with a trailing newline.
fn render_block(inner: &str) -> String {
    format!("{MARKER_BEGIN}\n{inner}\n{MARKER_END}\n")
}

/// The byte range of an existing marker block inside `content`, aligned to
/// whole lines.
struct BlockSpan {
    /// The start of the line carrying [`MARKER_BEGIN`] — where a *replace*
    /// begins.
    line_start: usize,
    /// One byte earlier than [`Self::line_start`] when the line
    /// immediately before it is blank — where a *removal* begins, so
    /// removing the block also removes the separator blank line this module
    /// itself inserted on the append that created it.
    remove_start: usize,
    /// The end of the line carrying [`MARKER_END`], including its trailing
    /// newline when there is one.
    line_end: usize,
}

fn locate_block(content: &str) -> Option<BlockSpan> {
    let begin_idx = content.find(MARKER_BEGIN)?;
    let line_start = content[..begin_idx].rfind('\n').map_or(0, |i| i + 1);

    let end_marker_idx = content[begin_idx..].find(MARKER_END)? + begin_idx;
    let after_end = end_marker_idx + MARKER_END.len();
    let line_end = match content[after_end..].find('\n') {
        Some(offset) => after_end + offset + 1,
        None => content.len(),
    };

    let remove_start = if line_start >= 1 && content.as_bytes().get(line_start - 1) == Some(&b'\n')
    {
        line_start - 1
    } else {
        line_start
    };

    Some(BlockSpan {
        line_start,
        remove_start,
        line_end,
    })
}

/// Splice `block` into `original`, returning the new whole-file content, or
/// `None` when nothing about the file should change (no block, nothing to
/// add — the case that must never create a file).
///
/// Byte for byte outside the block, in every case: a replace copies
/// `original[..line_start]` and `original[line_end..]` around the new block
/// untouched; a removal does the same around [`BlockSpan::remove_start`].
fn splice(original: &str, block: Option<&str>) -> Option<String> {
    let existing = locate_block(original);

    match (existing, block) {
        (None, None) => None,
        (None, Some(inner)) => {
            let body = render_block(inner);
            Some(if original.is_empty() {
                body
            } else if original.ends_with('\n') {
                format!("{original}\n{body}")
            } else {
                format!("{original}\n\n{body}")
            })
        }
        (Some(span), None) => {
            let mut result = String::with_capacity(original.len());
            result.push_str(&original[..span.remove_start]);
            result.push_str(&original[span.line_end..]);
            Some(result)
        }
        (Some(span), Some(inner)) => {
            let body = render_block(inner);
            let mut result = String::with_capacity(original.len() + body.len());
            result.push_str(&original[..span.line_start]);
            result.push_str(&body);
            result.push_str(&original[span.line_end..]);
            Some(result)
        }
    }
}

/// Write `contents` to `path` whole, through a same-directory temporary file
/// and a rename — never a partial write a reader could observe mid-write.
fn write_atomic(path: &Path, contents: &str) -> Result<(), ExportLocalError> {
    let dir = path.parent().unwrap_or_else(|| Path::new("."));
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("export-local");
    let tmp_path = dir.join(format!(".{file_name}.tmp-{}", std::process::id()));

    std::fs::write(&tmp_path, contents).map_err(|source| ExportLocalError::Io {
        action: "write",
        path: tmp_path.clone(),
        source,
    })?;
    std::fs::rename(&tmp_path, path).map_err(|source| ExportLocalError::Io {
        action: "rename into place",
        path: path.to_path_buf(),
        source,
    })
}

/// Add `filename` to `<root>/.git/info/exclude` when `root` is a git
/// project and the pattern is not already covered by either that file or
/// `.gitignore` — objective 4. Never opens `.gitignore` for writing.
///
/// A bounded, line-oriented match against both files rather than a
/// `git check-ignore` subprocess call: [`super::export::TrackedKnowledge`]'s
/// own `.gitignore` check (`gitignores_tracked_knowledge`) already reads
/// `.gitignore` directly for the same reason this reads `.git/info/exclude`
/// directly — a project root that has a `.git` directory but is not (yet) a
/// runnable repository, which is exactly the shape every shipped-binary test
/// fixture in this crate constructs, is not something `git check-ignore` can
/// answer at all.
fn ensure_excluded(root: &Path, filename: &str) -> Result<ExcludeAction, ExportLocalError> {
    let git_dir = root.join(".git");
    if !git_dir.exists() {
        return Ok(ExcludeAction::NotGitRepo);
    }

    if gitignore_matches(root, filename) || exclude_file_matches(&git_dir, filename)? {
        return Ok(ExcludeAction::AlreadyExcluded);
    }

    let exclude_path = git_dir.join("info").join("exclude");
    if let Some(parent) = exclude_path.parent() {
        std::fs::create_dir_all(parent).map_err(|source| ExportLocalError::Io {
            action: "create",
            path: parent.to_path_buf(),
            source,
        })?;
    }

    let mut contents = std::fs::read_to_string(&exclude_path).unwrap_or_default();
    if !contents.is_empty() && !contents.ends_with('\n') {
        contents.push('\n');
    }
    contents.push_str(filename);
    contents.push('\n');

    std::fs::write(&exclude_path, contents).map_err(|source| ExportLocalError::Io {
        action: "write",
        path: exclude_path,
        source,
    })?;
    Ok(ExcludeAction::Added)
}

fn gitignore_matches(root: &Path, filename: &str) -> bool {
    let Ok(contents) = std::fs::read_to_string(root.join(".gitignore")) else {
        return false;
    };
    contents.lines().any(|line| {
        let pattern = line.trim();
        if pattern.is_empty() || pattern.starts_with('#') {
            return false;
        }
        pattern.trim_start_matches('/') == filename
    })
}

fn exclude_file_matches(git_dir: &Path, filename: &str) -> Result<bool, ExportLocalError> {
    let path = git_dir.join("info").join("exclude");
    match std::fs::read_to_string(&path) {
        Ok(contents) => Ok(contents.lines().any(|line| line.trim() == filename)),
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(source) => Err(ExportLocalError::Io {
            action: "read",
            path,
            source,
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splice_appends_after_one_blank_line_when_no_block_exists() {
        let original = "# notes\ntext here\n";
        let out = splice(original, Some("[glasshouse:memory:begin]body")).unwrap();
        assert!(out.starts_with("# notes\ntext here\n\n<!-- glasshouse:memory:begin -->\n"));
        assert!(out.ends_with("<!-- glasshouse:memory:end -->\n"));
    }

    #[test]
    fn splice_creates_no_change_when_nothing_to_export_and_no_block_exists() {
        assert_eq!(splice("# notes\n", None), None);
        assert_eq!(splice("", None), None);
    }

    #[test]
    fn splice_replaces_only_the_block_leaving_surrounding_bytes_identical() {
        let original = format!(
            "above\n\n{}\nold entry\n{}\n\nbelow\n",
            MARKER_BEGIN, MARKER_END
        );
        let out = splice(&original, Some("new entry")).unwrap();
        assert_eq!(
            out,
            format!(
                "above\n\n{}\nnew entry\n{}\n\nbelow\n",
                MARKER_BEGIN, MARKER_END
            )
        );
    }

    #[test]
    fn splice_removes_the_block_and_its_own_separator_blank_line() {
        let original = format!(
            "above\n\n{}\nentry\n{}\n\nbelow\n",
            MARKER_BEGIN, MARKER_END
        );
        let out = splice(&original, None).unwrap();
        assert_eq!(out, "above\n\nbelow\n");
    }

    #[test]
    fn local_harness_refuses_codex_and_gemini_by_name() {
        let codex = LocalHarness::parse("codex").unwrap_err();
        assert!(codex.to_string().contains("codex"));
        let gemini = LocalHarness::parse("gemini-cli").unwrap_err();
        assert!(gemini.to_string().contains("gemini-cli"));
        assert!(LocalHarness::parse("claude-code").is_ok());
    }
}
