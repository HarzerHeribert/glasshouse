//! Phase 50: tracked project knowledge — an explicit, opt-in projection of
//! durable memory into human-readable files a Git workflow can review.
//!
//! This is the only door that copies memory into the project tree, and it
//! reads exactly the active project's [`super::MemoryStore`] — no session
//! history, event log or checkpoint, and no credential or provider metadata
//! (`tests/tracked_knowledge.rs` scans this file's own source for the module
//! paths it must never name). Every free-text field is redacted a second
//! time on the way out, independent of whatever the extractor already
//! screened, because this is the boundary where text leaves Glasshouse's
//! database and becomes a file any tool that reads the repository can open.
//!
//! The projection is deterministic: every file name derives from the
//! memory's own stable identifier and every timestamp comes from the
//! memory's `updated_at`, never the wall clock — so re-running the export
//! with nothing changed produces no diff, and changing one memory changes
//! exactly one file.
// History: design-decisions.md, "Trims: memory export and extraction module docs", memory/export.rs module doc.

use std::path::{Path, PathBuf};

use super::store::{MemoryAuthority, MemoryId, MemoryKind, MemoryRecord, MemoryStatus};
use super::{MemoryStoreError, ProjectMemory};

/// Which kinds of durable memory to project into tracked knowledge.
///
/// Decisions and constraints are always included — map line 1812 names them
/// specifically. Findings are a much larger and noisier category (Phase 20's
/// catch-all for "established by investigation"), so they are left out unless
/// asked for, which is the only knob this selection carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Selection {
    pub include_findings: bool,
}

impl Selection {
    /// The kinds this selection exports, in the fixed order files are
    /// written and listed in — never derived from a `HashSet` or any other
    /// order that could vary between runs.
    fn kinds(self) -> Vec<MemoryKind> {
        let mut kinds = vec![MemoryKind::Decision, MemoryKind::Constraint];
        if self.include_findings {
            kinds.push(MemoryKind::Finding);
        }
        kinds
    }
}

/// One file [`TrackedKnowledge::write`] wrote, or would write under
/// `dry_run`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WrittenFile {
    /// Repository-relative path, always under `.glasshouse/knowledge/`.
    pub path: PathBuf,
    pub kind: MemoryKind,
    pub id: MemoryId,
}

/// What one export did or would do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Manifest {
    /// One entry per exported memory, in the deterministic order the
    /// selected kinds (decision, then constraint, then finding when
    /// included) and memory id define — never filesystem order.
    pub written: Vec<WrittenFile>,
    /// The `README.md` this run wrote or would write, alongside `written`.
    pub readme: PathBuf,
    /// Whether the project's own `.gitignore` ignores `.glasshouse/` (or a
    /// parent of it). Reported so the caller can say so; never acted on —
    /// see [`TrackedKnowledge::write`]'s documentation for why this module
    /// does not edit `.gitignore`.
    pub gitignored: bool,
    /// Whether the project root has no `.git` directory. Reported, never a
    /// refusal: map line 1816 is about Git workflows when they exist, not a
    /// requirement that one does.
    pub git_absent: bool,
    /// Nothing was written to disk; [`Manifest::written`] and
    /// [`Manifest::readme`] describe what a non-dry run would produce.
    pub dry_run: bool,
}

/// Failures exporting tracked knowledge.
#[derive(Debug, thiserror::Error)]
pub enum ExportError {
    #[error(transparent)]
    Store(#[from] MemoryStoreError),
    #[error("could not {action} `{path}`")]
    Io {
        action: &'static str,
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

/// The projection of durable memory into `.glasshouse/knowledge/` —
/// Phase 50's tracked project knowledge.
pub struct TrackedKnowledge;

impl TrackedKnowledge {
    /// Project `selection`'s memories from `memory` into `.glasshouse/knowledge/`
    /// under `root`.
    ///
    /// `root` is the caller's project root and nothing else: there is no
    /// argument here that could resolve to another project's tree, matching
    /// the same discipline [`ProjectMemory::open`] enforces for the database
    /// itself.
    ///
    /// With `dry_run`, nothing is written — [`Manifest`] describes what would
    /// have been.
    pub fn write(
        memory: &ProjectMemory,
        root: &Path,
        selection: Selection,
        dry_run: bool,
    ) -> Result<Manifest, ExportError> {
        let store = memory.store();
        let canonical_store = store.project_id().to_owned();

        let knowledge_dir = root.join(".glasshouse").join("knowledge");

        // One read of every current memory, filtered client-side by kind.
        // `with_status` is the same read `memory revalidate --list` and
        // `memory conflicts` already use; nothing new is asked of the store.
        let active: Vec<MemoryRecord> = store.with_status(MemoryStatus::Active, usize::MAX)?;

        let mut written = Vec::new();
        for kind in selection.kinds() {
            let mut records: Vec<&MemoryRecord> =
                active.iter().filter(|record| record.kind == kind).collect();
            // Deterministic: by kind (the fixed order above), then by id.
            records.sort_by(|a, b| a.id.cmp(&b.id));

            for record in records {
                let path = knowledge_dir.join(file_name(kind, &record.id));
                let contents = render_record(&canonical_store, record);
                if !dry_run {
                    write_file(&path, &contents)?;
                }
                written.push(WrittenFile {
                    path,
                    kind,
                    id: record.id.clone(),
                });
            }
        }

        let readme = knowledge_dir.join("README.md");
        if !dry_run {
            std::fs::create_dir_all(&knowledge_dir).map_err(|source| ExportError::Io {
                action: "create",
                path: knowledge_dir.clone(),
                source,
            })?;
            write_file(&readme, README_CONTENTS)?;
        }

        Ok(Manifest {
            written,
            readme,
            gitignored: gitignores_tracked_knowledge(root),
            git_absent: !root.join(".git").exists(),
            dry_run,
        })
    }
}

fn write_file(path: &Path, contents: &str) -> Result<(), ExportError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|source| ExportError::Io {
            action: "create",
            path: parent.to_path_buf(),
            source,
        })?;
    }
    std::fs::write(path, contents).map_err(|source| ExportError::Io {
        action: "write",
        path: path.to_path_buf(),
        source,
    })
}

/// A stable file name derived only from the memory's kind and id — never from
/// its subject or body, both of which can change without the memory itself
/// being superseded.
fn file_name(kind: MemoryKind, id: &MemoryId) -> String {
    format!("{}-{}.md", kind.as_str(), id.as_str())
}

const README_CONTENTS: &str = "\
# Tracked project knowledge

This directory is a **projection**, not the source of truth. The canonical
store is Glasshouse's own project database, kept outside this repository by
default (`glasshouse` state directory). These files are a human-readable,
point-in-time copy of selected durable memories — decisions and constraints,
and findings only when explicitly included — written here because a project
opted in with `glasshouse memory export --tracked`.

Glasshouse does not require this directory to operate: deleting it loses
nothing but a convenience, and every file here can be regenerated by running
the export again. Edit the canonical memory through `glasshouse memory`, not
these files directly — a hand edit here is not read back.
";

/// Render one memory as its tracked-knowledge Markdown file.
///
/// Every free-text field is passed through [`redact_secrets`] before it
/// reaches the page: the schema keeps a credential out of the `memories`
/// table (see `super::store`'s module documentation), but body and rationale
/// are free text, and this is the boundary where that text leaves
/// Glasshouse's own database.
fn render_record(canonical_store: &str, record: &MemoryRecord) -> String {
    use std::fmt::Write as _;

    let exported_at = iso8601_utc(record.updated_at);
    let mut out = String::new();
    let _ = writeln!(
        out,
        "<!-- projection of glasshouse project memory; canonical store: {canonical_store}; \
         exported {exported_at} -->"
    );
    let _ = writeln!(out);
    let subject = record.subject.as_deref().unwrap_or("(no subject)");
    let _ = writeln!(out, "# {}: {}", record.kind, redact_secrets(subject));
    let _ = writeln!(out);
    let _ = writeln!(out, "- id: {}", record.id);
    let _ = writeln!(out, "- authority: {}", authority_label(record.authority));
    let _ = writeln!(out, "- status: {}", record.status);
    let _ = writeln!(out);
    let _ = writeln!(out, "{}", redact_secrets(&record.body));

    if let Some(rationale) = record.provenance.rationale.as_deref() {
        let _ = writeln!(out);
        let _ = writeln!(out, "## Rationale");
        let _ = writeln!(out);
        let _ = writeln!(out, "{}", redact_secrets(rationale));
    }

    out
}

fn authority_label(authority: Option<MemoryAuthority>) -> String {
    match authority {
        Some(authority) => authority.to_string(),
        None => "unclassified".to_owned(),
    }
}

/// Whether the project's own `.gitignore` ignores `.glasshouse/` (or a
/// parent of it).
///
/// A bounded, line-oriented check rather than a full gitignore glob engine:
/// this only ever informs the manifest's own note, and this module commits to
/// never editing `.gitignore` on the strength of it (see
/// [`Manifest::gitignored`]'s documentation) — a false negative here costs a
/// missed note, never a wrong write.
fn gitignores_tracked_knowledge(root: &Path) -> bool {
    let Ok(contents) = std::fs::read_to_string(root.join(".gitignore")) else {
        return false;
    };
    contents.lines().any(|line| {
        let pattern = line.trim();
        if pattern.is_empty() || pattern.starts_with('#') {
            return false;
        }
        let pattern = pattern.trim_start_matches('/').trim_end_matches('/');
        pattern == ".glasshouse" || pattern == ".glasshouse/**" || pattern == ".glasshouse/*"
    })
}

/// The bounded prefixes this module treats as secret-shaped. Not an attempt
/// at a general credential scanner — that control is the producer's, per
/// this module's own documentation — but a token carrying one of these
/// prefixes is unambiguous enough to redact outright rather than publish.
const SECRET_PREFIXES: &[&str] = &[
    "sk-", "sk_", "pk_", "rk_", "ghp_", "gho_", "ghu_", "ghs_", "ghr_", "xoxb-", "xoxp-", "xoxa-",
    "xoxs-", "AKIA", "ASIA",
];

/// Replace any run of token characters (ASCII alphanumeric, `_`, `-`) that
/// starts with a [`SECRET_PREFIXES`] entry and is long enough to be a real
/// key, rather than just the prefix word itself, with a fixed placeholder.
///
/// Everything else — punctuation, whitespace, the rest of the sentence — is
/// passed through byte-for-byte, so a memory that happens to mention "sk-" in
/// prose is not mangled; only a token that actually looks like a live key is.
fn redact_secrets(text: &str) -> String {
    const MIN_TOKEN_LEN: usize = 20;

    let is_token_char = |c: char| c.is_ascii_alphanumeric() || c == '_' || c == '-';
    let mut out = String::with_capacity(text.len());
    let chars: Vec<char> = text.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        if is_token_char(chars[i]) {
            let start = i;
            while i < chars.len() && is_token_char(chars[i]) {
                i += 1;
            }
            let token: String = chars[start..i].iter().collect();
            let looks_secret = token.len() >= MIN_TOKEN_LEN
                && SECRET_PREFIXES
                    .iter()
                    .any(|prefix| token.starts_with(prefix));
            if looks_secret {
                out.push_str("[REDACTED]");
            } else {
                out.push_str(&token);
            }
        } else {
            out.push(chars[i]);
            i += 1;
        }
    }
    out
}

/// Seconds since the Unix epoch as an ISO 8601 UTC timestamp
/// (`YYYY-MM-DDTHH:MM:SSZ`), with no external time crate.
///
/// Tied to a memory's own `updated_at` rather than to the wall clock at
/// export time — see this module's own documentation for why that is what
/// keeps two exports of an unchanged store byte-identical.
fn iso8601_utc(seconds_since_epoch: i64) -> String {
    let days = seconds_since_epoch.div_euclid(86_400);
    let secs_of_day = seconds_since_epoch.rem_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    let hour = secs_of_day / 3600;
    let minute = (secs_of_day % 3600) / 60;
    let second = secs_of_day % 60;
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}Z")
}

/// Howard Hinnant's `civil_from_days`: days since the Unix epoch to a
/// proleptic-Gregorian (year, month, day), reproduced here rather than
/// pulled in as a dependency for one conversion this module needs once.
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097); // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365; // [0, 399]
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32; // [1, 31]
    let m = (if mp < 10 { mp + 3 } else { mp - 9 }) as u32; // [1, 12]
    let year = if m <= 2 { y + 1 } else { y };
    (year, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn iso8601_formats_known_instants() {
        assert_eq!(iso8601_utc(0), "1970-01-01T00:00:00Z");
        assert_eq!(iso8601_utc(1), "1970-01-01T00:00:01Z");
        assert_eq!(iso8601_utc(86_399), "1970-01-01T23:59:59Z");
        assert_eq!(iso8601_utc(946_684_800), "2000-01-01T00:00:00Z");
        assert_eq!(iso8601_utc(1_700_000_000), "2023-11-14T22:13:20Z");
    }

    #[test]
    fn redact_secrets_removes_only_the_shaped_token() {
        let body = "the deploy key is sk-abcdefghijklmnopqrstuvwxyz for openai, keep it out";
        let redacted = redact_secrets(body);
        assert!(!redacted.contains("sk-abcdefghijklmnopqrstuvwxyz"));
        assert!(redacted.contains("[REDACTED]"));
        assert!(
            redacted.contains("for openai"),
            "surrounding prose must survive: {redacted}"
        );
    }

    #[test]
    fn redact_secrets_leaves_short_and_unprefixed_tokens_alone() {
        let body = "sk-too-short and a normal-looking-identifier stay as they are";
        assert_eq!(redact_secrets(body), body);
    }

    #[test]
    fn file_names_are_derived_only_from_kind_and_id() {
        let id = MemoryId::new("abc123");
        assert_eq!(file_name(MemoryKind::Decision, &id), "decision-abc123.md");
        assert_eq!(
            file_name(MemoryKind::Constraint, &id),
            "constraint-abc123.md"
        );
    }

    /// `with_status`'s own query orders by `updated_at DESC, id ASC` — so a
    /// test that lets every record land in the same second would still read
    /// as "ordered by id" even if this module's own sort were deleted. This
    /// gives each record a strictly *decreasing* timestamp, the one shape
    /// that puts insertion order, timestamp order and id order at odds with
    /// each other, so only a real by-id sort in this module can pass.
    #[test]
    fn export_orders_deterministically_by_id_even_when_timestamps_disagree() {
        use std::sync::Arc;
        use std::sync::atomic::{AtomicI64, Ordering};

        use clap::Parser as _;

        use crate::memory::store::NewMemory;

        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path().join("workspace");
        std::fs::create_dir_all(root.join(".git")).expect("create project root");
        let root = std::fs::canonicalize(&root).expect("canonicalize the project root");
        let cli = crate::Cli::try_parse_from([
            "glasshouse",
            "--scope",
            root.to_str().expect("utf-8 tempdir path"),
            "--data-dir",
            tmp.path().join("data").to_str().expect("utf-8 path"),
            "--config-dir",
            tmp.path().join("config").to_str().expect("utf-8 path"),
        ])
        .expect("parse a minimal cli");
        let runtime = crate::bootstrap(&cli, &root).expect("bootstrap");

        let counter = Arc::new(AtomicI64::new(1_000));
        let clock: crate::memory::store::Clock = {
            let counter = Arc::clone(&counter);
            Arc::new(move || counter.fetch_sub(1, Ordering::SeqCst))
        };
        let memory = ProjectMemory::open_with_clock(&runtime, clock).expect("open project memory");
        let store = memory.store();

        let mut ids = Vec::new();
        for _ in 0..3 {
            let record = store
                .record(NewMemory::new(MemoryKind::Decision, "a decision body"))
                .expect("record a memory");
            ids.push(record.id);
        }
        // Each record was written with a *later* (smaller) timestamp than
        // the one before it, so `updated_at DESC` would return them in
        // reverse of recording order — and recording order here is
        // unrelated to id order, since ids are random. Ascending id is the
        // only ordering this sort could have produced on purpose.
        ids.sort();

        let manifest = TrackedKnowledge::write(&memory, &root, Selection::default(), true)
            .expect("plan the export");
        let exported_ids: Vec<MemoryId> = manifest
            .written
            .iter()
            .map(|file| file.id.clone())
            .collect();
        assert_eq!(
            exported_ids, ids,
            "export order must be ascending by id, regardless of recording or \
             timestamp order"
        );
    }
}
