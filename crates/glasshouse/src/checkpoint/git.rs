//! Where the repository is standing, read cheaply.
//!
//! The map asks a checkpoint to *include the current Git branch and commit
//! when available*, and "when available" is doing real work: a project need
//! not be a Git repository at all, and Glasshouse must still be able to take
//! a checkpoint.
//!
//! # No subprocess
//!
//! This opens two or three small files and parses them. It does not run
//! `git`, and that is deliberate rather than incidental:
//!
//! - a checkpoint can be taken at a task boundary, on a thread that is also
//!   serving a terminal, and spawning a process there is a latency nobody
//!   asked for;
//! - `git` need not be installed for a `.git` directory to exist and be
//!   readable — a repository cloned onto a machine whose Git was uninstalled
//!   is still a repository;
//! - a subprocess inherits an environment, and `GIT_DIR` in that environment
//!   would silently point this at another repository.
//!
//! # The deliberate exceptions, and what they are scoped to
//!
//! [`last_change_commit`], [`is_ancestor`] and [`changed_paths`] **do** run
//! `git`, and the objections above are answered rather than waived. None is
//! on the checkpoint path: nothing takes a checkpoint through them, and no
//! thread serving a terminal calls them. `last_change_commit` and
//! `is_ancestor`'s caller is memory retrieval (`crate::memory::inject`'s file
//! section and `glasshouse memory search --path`), which is already several
//! database reads deep and is bounded at one `git log` per path and one
//! `merge-base` per memory. `changed_paths`'s caller is the guardrail door's
//! transition handler, bounded to one call per rollback-or-refutation
//! transition — an assumption ledger write, not a terminal-serving path
//! either. A machine with no `git`, or a project that is no repository,
//! makes every one of the three answer `None`, which their consumers render
//! as *unknown* rather than assuming a clean tree or fresh memory. And the
//! environment objection is met head-on: all three clear `GIT_DIR`,
//! `GIT_WORK_TREE`, `GIT_INDEX_FILE` and `GIT_COMMON_DIR` from the child
//! rather than trusting the caller's, so an inherited `GIT_DIR` cannot
//! silently point them at another repository.
//!
//! `changed_paths` does not reuse [`WorkingTreeStatus::detect`] — the index
//! reader already on this path — because that reader is deliberately bounded
//! to `MAX_CHANGED_FILES` tracked entries and never reports an untracked
//! file at all; a preserve set that silently omitted a new, unclaimed file
//! would be the one wrong direction line 1044 forbids.
//!
//! There is no file-reading version of *"which commit last changed this
//! path"*: answering it means walking the commit graph and diffing trees out
//! of packfiles, which is a decompressor and a delta resolver, not two small
//! files. Map line 1142's freshness is worth one bounded subprocess and is
//! not worth that.
//!
//! # Worktrees, which is the case that actually bites
//!
//! In a linked worktree `.git` is a **file** holding `gitdir: <path>`, that
//! directory has its own `HEAD` and its own `commondir`, and the refs live in
//! the *common* directory rather than beside the HEAD. Glasshouse's own
//! development happens in linked worktrees, so a reader that only handled the
//! `.git`-is-a-directory case would have reported nothing in exactly the
//! situation this project runs in every day. Both shapes are handled, and
//! both are tested against real fixtures.

use std::path::{Path, PathBuf};

/// A repository position: the commit, and the branch pointing at it when
/// HEAD is not detached.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitPosition {
    /// The short branch name — `main`, not `refs/heads/main`. `None` on a
    /// detached HEAD, which is a different fact from "no branch recorded" and
    /// stays distinguishable because the commit is always present.
    pub branch: Option<String>,
    /// The full object name HEAD resolves to.
    pub commit: String,
}

impl GitPosition {
    /// Rebuild from what a checkpoint document stored.
    ///
    /// A commit is what makes a position a position, so a document with a
    /// branch and no commit yields `None` rather than half a position.
    pub(crate) fn from_parts(branch: Option<String>, commit: Option<String>) -> Option<Self> {
        commit.map(|commit| Self { branch, commit })
    }

    /// Read the repository containing `project_root`, or `None`.
    ///
    /// Never fails: every way of not finding a position — no repository, an
    /// unreadable HEAD, a branch whose ref file is missing — is the same
    /// answer to the caller, because a checkpoint is worth taking either way.
    pub fn detect(project_root: &Path) -> Option<Self> {
        let (git_dir, common_dir) = resolve_git_dir(project_root)?;
        let head = read_trimmed(&git_dir.join("HEAD"))?;

        let Some(reference) = head.strip_prefix("ref: ") else {
            // Detached: HEAD holds the object name itself.
            if !is_object_name(&head) {
                return None;
            }
            return Some(Self {
                branch: None,
                commit: head,
            });
        };
        let reference = reference.trim();

        // A loose ref lives under the directory that owns it — the worktree's
        // own for `HEAD`-adjacent refs, the common one for shared branches —
        // and a packed one lives only in `packed-refs`, in the common
        // directory. Both are ordinary, so both are looked for.
        let commit = read_trimmed(&git_dir.join(reference))
            .or_else(|| read_trimmed(&common_dir.join(reference)))
            .or_else(|| packed_ref(&common_dir, reference))?;
        if !is_object_name(&commit) {
            return None;
        }

        Some(Self {
            branch: Some(
                reference
                    .strip_prefix("refs/heads/")
                    .unwrap_or(reference)
                    .to_owned(),
            ),
            commit,
        })
    }
}

/// Whether the working tree holds changes the index does not, read the same
/// cheap way [`GitPosition::detect`] reads `HEAD`: by opening a small file
/// directly and parsing it, never by spawning `git`. The reasons are the
/// module's own — see the module doc.
///
/// # What this compares, and what it does not
///
/// This compares the **working tree against the index** — unstaged
/// modifications and deletions of files Git already tracks. It does **not**
/// compare the index against `HEAD` (staged-but-uncommitted changes), and it
/// does **not** detect untracked files: both would need Git's object store
/// (loose objects and packfiles, decompressed and walked as trees) rather
/// than the two small files this module reads, which is a different and much
/// larger mechanism than a checkpoint's cheap best-effort status is worth
/// building. A dirty tree is always reported correctly; a clean tree is
/// reported only as "no *unstaged* changes found" — real for what it checks,
/// silent about what it does not.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct WorkingTreeStatus {
    /// Whether any tracked file differs from what the index recorded, or has
    /// been deleted.
    pub dirty: bool,
    /// Paths of the files that differ, relative to the repository root, in
    /// index order and capped at `MAX_CHANGED_FILES` so an enormous change
    /// cannot make a checkpoint proportional to its own size. `dirty` stays
    /// `true` even once the cap is reached; only the *list* stops growing.
    pub changed_files: Vec<String>,
}

/// How many changed paths [`WorkingTreeStatus::detect`] records by name.
///
/// A checkpoint is a pointer back to the work, not a copy of it — see the
/// module-level `Checkpoint` doc's "small is a constraint, not an
/// aspiration". Naming a bounded handful of paths is enough to tell a fresh
/// session where to look; naming all of them would not tell it anything
/// more useful and would cost bytes doing it.
pub(crate) const MAX_CHANGED_FILES: usize = 20;

impl WorkingTreeStatus {
    /// Read the repository containing `project_root`, or `None`.
    ///
    /// `None` covers every way of not being able to answer — no repository,
    /// no index yet (a repository with nothing ever added), an index format
    /// this reader does not recognize — because a checkpoint reporting no
    /// status is honest and a checkpoint guessing one is not.
    pub fn detect(project_root: &Path) -> Option<Self> {
        let (git_dir, _common_dir) = resolve_git_dir(project_root)?;
        // The index is per-worktree, unlike the refs read for `GitPosition`:
        // each linked worktree stages its own changes independently, so this
        // is read from `git_dir`, never from the common directory.
        let bytes = std::fs::read(git_dir.join("index")).ok()?;
        let entries = parse_index(&bytes)?;

        let mut status = WorkingTreeStatus::default();
        for entry in entries {
            if entry_changed(project_root, &entry) {
                status.dirty = true;
                if status.changed_files.len() < MAX_CHANGED_FILES {
                    status.changed_files.push(entry.path);
                }
            }
        }
        Some(status)
    }
}

/// One file the index tracks, with just enough recorded state to tell
/// whether the working tree still agrees with it.
struct IndexEntry {
    path: String,
    mtime_secs: u32,
    mtime_nanos: u32,
    size: u32,
    /// The index's own file mode, so a submodule (`160000`) can be skipped
    /// rather than compared against a regular file's metadata and reported
    /// as changed for a reason that has nothing to do with its content.
    mode: u32,
}

/// Whether `entry`'s file, as it stands on disk right now, still matches
/// what the index recorded.
///
/// This is the same *racily* correct check Git itself performs on its fast
/// path: comparing size and modification time rather than content. It can
/// theoretically miss a change made and reverted within one filesystem
/// timer tick without ever changing size — the same limitation Git accepts
/// for the same reason — but it never reports a change that did not happen.
fn entry_changed(project_root: &Path, entry: &IndexEntry) -> bool {
    const GITLINK: u32 = 0o160000;
    if entry.mode & 0o170000 == GITLINK {
        // A submodule's "content" is which commit it points to, which this
        // reader has no cheap way to check; skipping it means never
        // reporting a false change, at the cost of never reporting a real
        // one either.
        return false;
    }
    let Ok(metadata) = std::fs::symlink_metadata(project_root.join(&entry.path)) else {
        // Tracked, and gone: a deletion is exactly the kind of change a
        // handoff should mention.
        return true;
    };
    if metadata.len() != u64::from(entry.size) {
        return true;
    }
    let Ok(modified) = metadata.modified() else {
        // A platform that cannot report an mtime at all cannot be compared;
        // reporting "changed" on every entry, on every checkpoint, on such a
        // platform would be noise rather than signal.
        return false;
    };
    let Ok(duration) = modified.duration_since(std::time::UNIX_EPOCH) else {
        return true;
    };
    duration.as_secs() != u64::from(entry.mtime_secs)
        || duration.subsec_nanos() != entry.mtime_nanos
}

/// Parse a Git index file far enough to read every entry's path, size and
/// modification time.
///
/// Deliberately narrow: this reads the fixed-length per-entry stat data and
/// the name that follows it, for format versions 2 and 3, and stops the
/// moment the declared entry count is reached — the extensions that follow
/// (the cache-tree, the untracked-file cache, and the rest) are never read,
/// because nothing here needs them. Version 4's compressed-name entries are
/// a different byte layout this does not implement; encountering one is
/// treated the same as any other unreadable index, `None`, rather than
/// parsed wrongly.
fn parse_index(bytes: &[u8]) -> Option<Vec<IndexEntry>> {
    const HEADER: usize = 12;
    const ENTRY_FIXED: usize = 62;
    if bytes.len() < HEADER || &bytes[0..4] != b"DIRC" {
        return None;
    }
    let version = u32::from_be_bytes(bytes[4..8].try_into().ok()?);
    if !(2..=3).contains(&version) {
        return None;
    }
    let count = u32::from_be_bytes(bytes[8..12].try_into().ok()?) as usize;

    let mut entries = Vec::with_capacity(count.min(4096));
    let mut offset = HEADER;
    for _ in 0..count {
        if offset + ENTRY_FIXED > bytes.len() {
            return None;
        }
        let field = |start: usize| -> Option<u32> {
            bytes
                .get(offset + start..offset + start + 4)
                .and_then(|slice| slice.try_into().ok())
                .map(u32::from_be_bytes)
        };
        // Fixed layout: ctime sec/nsec at 0/4, mtime sec/nsec at 8/12, dev at
        // 16, ino at 20, mode at 24, uid at 28, gid at 32, size at 36, a
        // 20-byte SHA-1 at 40, flags at 60. Only what this reader needs is
        // pulled out; ctime, dev, ino, uid and gid are read by nothing here.
        let mtime_secs = field(8)?;
        let mtime_nanos = field(12)?;
        let mode = field(24)?;
        let size = field(36)?;
        let flags = u16::from_be_bytes(bytes.get(offset + 60..offset + 62)?.try_into().ok()?);

        let mut name_start = offset + ENTRY_FIXED;
        // Version 3 may mark an entry "extended", adding a second flags word
        // before the name; version 2 never sets this bit.
        if flags & 0x4000 != 0 {
            name_start += 2;
        }
        let name_len = (flags & 0x0FFF) as usize;
        let name_bytes = if name_len < 0x0FFF {
            bytes.get(name_start..name_start + name_len)?
        } else {
            // A name of 4094 bytes or longer is not length-prefixed; read up
            // to the next NUL instead.
            let end = bytes[name_start..].iter().position(|&b| b == 0)?;
            bytes.get(name_start..name_start + end)?
        };
        let path = std::str::from_utf8(name_bytes).ok()?.to_owned();

        // Padding always advances to the next multiple of 8 bytes from the
        // start of the entry, with at least one NUL byte included even when
        // the unpadded length already lands on a boundary.
        let entry_len = name_start - offset + name_bytes.len();
        let pad_to = match entry_len % 8 {
            0 => 8,
            rem => 8 - rem,
        };
        offset += entry_len + pad_to;

        entries.push(IndexEntry {
            path,
            mtime_secs,
            mtime_nanos,
            size,
            mode,
        });
    }
    Some(entries)
}

/// The directory holding this checkout's `HEAD`, and the one holding the
/// repository's shared refs. They are the same directory except in a linked
/// worktree.
fn resolve_git_dir(project_root: &Path) -> Option<(PathBuf, PathBuf)> {
    let dot_git = project_root.join(".git");
    let metadata = std::fs::symlink_metadata(&dot_git).ok()?;

    let git_dir = if metadata.is_dir() {
        dot_git
    } else {
        // A linked worktree, or a submodule: `.git` is a file naming the real
        // directory. The path may be relative, and is then relative to the
        // project root rather than to the file.
        let pointer = read_trimmed(&dot_git)?;
        let target = Path::new(pointer.strip_prefix("gitdir:")?.trim());
        if target.is_absolute() {
            target.to_path_buf()
        } else {
            project_root.join(target)
        }
    };

    // `commondir` is present only in a linked worktree, and names the main
    // `.git` — relative to the worktree's own git directory when it is not
    // absolute.
    let common_dir = match read_trimmed(&git_dir.join("commondir")) {
        Some(common) => {
            let common = Path::new(&common);
            if common.is_absolute() {
                common.to_path_buf()
            } else {
                git_dir.join(common)
            }
        }
        None => git_dir.clone(),
    };

    Some((git_dir, common_dir))
}

/// The object name `reference` is packed at, if it is packed.
///
/// `packed-refs` is one ref per line, `<object-name> <ref>`, with `#` comment
/// lines and `^`-prefixed peeled tag lines that are deliberately skipped —
/// a peeled line describes the *previous* line's tag, and reading one as a
/// ref would attribute the wrong object to the wrong name.
fn packed_ref(common_dir: &Path, reference: &str) -> Option<String> {
    let packed = std::fs::read_to_string(common_dir.join("packed-refs")).ok()?;
    for line in packed.lines() {
        let line = line.trim_end();
        if line.starts_with('#') || line.starts_with('^') {
            continue;
        }
        if let Some((object, name)) = line.split_once(' ')
            && name == reference
        {
            return Some(object.to_owned());
        }
    }
    None
}

fn read_trimmed(path: &Path) -> Option<String> {
    let text = std::fs::read_to_string(path).ok()?;
    let trimmed = text.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_owned())
}

/// Whether `value` looks like a Git object name.
///
/// Checked so that a `HEAD` holding something unexpected — a stale
/// `ref: refs/…` written by a tool mid-operation, a truncated file — is
/// reported as no position rather than recorded as a commit that does not
/// exist. Both the 40-character SHA-1 form and the 64-character SHA-256 form
/// are accepted, because a repository may be either.
fn is_object_name(value: &str) -> bool {
    matches!(value.len(), 40 | 64) && value.chars().all(|c| c.is_ascii_hexdigit())
}

/// Run one `git` subcommand in `root` and return its trimmed stdout, or
/// `None`.
///
/// The single place this module spawns a process, so the environment scrub
/// the module documentation promises is made once rather than remembered
/// twice.
///
/// - **`current_dir(root)` and no `-C`, no `--git-dir`.** The repository is
///   named by the working directory and by nothing a caller can smuggle in.
/// - **Four variables removed.** `GIT_DIR`, `GIT_WORK_TREE`,
///   `GIT_COMMON_DIR` and `GIT_INDEX_FILE` each override the working
///   directory, and Glasshouse's own development runs inside linked
///   worktrees where at least one of them is routinely set. Inheriting them
///   would answer about whichever repository the parent happened to be
///   pointed at — silently, and with a real commit.
/// - **No shell, ever.** `args` are argv elements, so a path is a literal
///   however it is spelled; the caller puts a `--` in the list before any
///   path so a file named `-n` cannot become a flag.
/// - **`stdin(null)`.** `git` must never block waiting for input on a path
///   whose whole purpose is to answer a label quickly.
///
/// `None` for every way of not getting an answer — `git` absent, not a
/// repository, a nonzero exit, output that is not UTF-8, an empty answer —
/// because the one consumer renders all of them as *unknown* and a caller
/// that could tell them apart would still do nothing different.
fn git_output(root: &Path, args: &[&str]) -> Option<String> {
    let output = std::process::Command::new("git")
        .args(args)
        .current_dir(root)
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_COMMON_DIR")
        .env_remove("GIT_INDEX_FILE")
        .stdin(std::process::Stdio::null())
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8(output.stdout).ok()?;
    let trimmed = text.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_owned())
    }
}

/// Whether a memory is older than the file it is about — map line 1142's
/// *"never treat stale memory as stronger evidence than the current source
/// code"*, done the one way the refusal register's rulings allow.
///
/// # A fact about commits, and deliberately not a claim about conflict
///
/// Nothing here reads a line of source or compares a memory's statement to
/// anything. Reading the source and deciding whether a memory still holds was
/// refused four times over (register lines 828, 829, 862, 932) as a judgement
/// this project has no honest producer for, and this does not attempt it. It
/// answers one question — *did the file change after the memory was recorded?*
/// — and hands the reader the answer.
///
/// # It is a label, and it never withholds
///
/// [`Self::Stale`] does not drop a memory, move it, or lower its score. The
/// comparator in `crate::memory::search` never sees this type; it is computed
/// after ranking, per row, purely to be printed. A stale memory may still be
/// the most important thing a session is told — it just says which of the two
/// is older, and that the source decides.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Freshness {
    /// The file has not changed since the memory was recorded: its last
    /// change is the memory's own commit, or an ancestor of it.
    Current,
    /// The file changed **after** the memory was recorded — the memory's
    /// commit is a strict ancestor of the file's last change.
    Stale,
    /// Not answerable. Either side missing (a memory recorded with no commit,
    /// a path git has never tracked), no repository, no `git`, or two commits
    /// on histories that do not contain one another.
    ///
    /// Distinct from [`Self::Current`] on purpose: *"nothing changed"* and
    /// *"nobody could check"* are different things to tell a reader, and
    /// collapsing the second into the first is the one direction map line
    /// 1142 forbids.
    Unknown,
}

impl Freshness {
    /// The word printed on a file-aware row.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Current => "current",
            Self::Stale => "stale",
            Self::Unknown => "unknown",
        }
    }

    /// Compare a memory's `source_commit` against the commit that last
    /// changed the file, which the caller reads once per path with
    /// [`last_change_commit`].
    ///
    /// Split from that read rather than folded into it because the two have
    /// different arities: a briefing asks about a handful of memories per
    /// path, so the `git log` is paid once and this is paid per memory.
    ///
    /// # How many subprocesses this costs
    ///
    /// **None** when the two commits are equal, which is the ordinary case —
    /// a memory extracted at the commit that last touched its file. **One**
    /// `merge-base` when they differ and the answer is `Current`, which is
    /// the next most common: the memory was extracted later than the file's
    /// last change. **Two** only to distinguish `Stale` from `Unknown`, and
    /// the second call is what stops two divergent branches being reported as
    /// staleness they are not.
    pub fn compare(root: &Path, last_change: Option<&str>, source_commit: Option<&str>) -> Self {
        let (Some(last_change), Some(source)) = (last_change, source_commit) else {
            return Self::Unknown;
        };
        if last_change == source {
            return Self::Current;
        }
        // Asked in this order because it is the cheaper of the two to be
        // right about: the file's last change being an ancestor of the
        // memory's commit is exactly "the memory is at least as new", and it
        // settles `Current` in one call.
        match is_ancestor(root, last_change, source) {
            Some(true) => Self::Current,
            Some(false) => match is_ancestor(root, source, last_change) {
                // Strict, because equality was handled above.
                Some(true) => Self::Stale,
                // Neither contains the other: two branches, and neither is
                // older than the other in any sense a reader could act on.
                _ => Self::Unknown,
            },
            None => Self::Unknown,
        }
    }
}

impl std::fmt::Display for Freshness {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.pad(self.as_str())
    }
}

/// The commit that last changed `path`, as an object name — map line 1142's
/// first question.
///
/// `path` is repo-relative, `/`-separated — `memory_files.path`'s own
/// spelling, which is what every caller has. It goes after `--` so a name
/// that begins with `-` is a path and not a flag, and a path git has never
/// tracked answers `None` (`git log` prints nothing and exits zero, which
/// `git_output`'s empty-output rule turns into `None`).
///
/// The answer is validated as an object name before it is returned. `git`
/// with `--format=%H` cannot print anything else, so this is not defensive
/// about `git` — it is the same check [`GitPosition::detect`] applies to a
/// ref file, kept here so that every commit this module hands out has passed
/// the same test whatever produced it.
pub fn last_change_commit(root: &Path, path: &str) -> Option<String> {
    let commit = git_output(root, &["log", "-1", "--format=%H", "--", path])?;
    is_object_name(&commit).then_some(commit)
}

/// Whether `ancestor` is an ancestor of `descendant` — map line 1142's
/// second question, and the one that makes *stale* a fact about commits
/// rather than a judgement about content.
///
/// `Some(true)`, `Some(false)`, and `None` are three different answers and
/// the third is the one worth being careful about. `git merge-base
/// --is-ancestor` exits 0 for yes and 1 for no; **every other exit code is a
/// refusal to answer**, not a no — an unknown revision, unrelated histories,
/// a corrupt object, no repository, no `git`. Collapsing that into `false`
/// would report *current* for a memory nothing could be checked against,
/// which is the one direction map line 1142 forbids getting wrong.
///
/// A commit is its own ancestor by this definition, as `git` defines it, and
/// the caller relies on that: a file whose last change **is** the commit a
/// memory was extracted at is current, not stale.
pub fn is_ancestor(root: &Path, ancestor: &str, descendant: &str) -> Option<bool> {
    // Not through `git_output`: that helper reads stdout and treats a nonzero
    // exit as no answer, and this question's whole answer *is* the exit code
    // — `--is-ancestor` prints nothing at all in either direction.
    let status = std::process::Command::new("git")
        .args(["merge-base", "--is-ancestor", ancestor, descendant])
        .current_dir(root)
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_COMMON_DIR")
        .env_remove("GIT_INDEX_FILE")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .ok()?;
    match status.code() {
        Some(0) => Some(true),
        Some(1) => Some(false),
        _ => None,
    }
}

/// Every repo-relative, `/`-separated path the working tree reports as
/// changed against the index — tracked or not — for the guardrail door's
/// preserve set (`crate::guardrails::preserve_set`, capability map line
/// 1044; see `docs/product/design-decisions.md`, *Rollback preserves what is
/// not yours*).
///
/// `git status --porcelain=v1 -z --untracked-files=all`: `-z` gives NUL-
/// terminated, unquoted records, which is the only spelling that survives a
/// path with a space or a non-ASCII byte in it undamaged; `--untracked-files
/// =all` is what makes a brand-new file the transitioning session never
/// staged show up at all, which the index-only [`WorkingTreeStatus`] cannot
/// do. A rename or copy prints two `-z` records — the old path with the
/// status, then the bare new path — and this reports the new path, which is
/// what the working tree currently holds at.
///
/// **Not through `git_output`**: that helper answers `None` for empty
/// stdout, which is exactly what a clean tree prints, and collapsing *clean*
/// into *unknown* is the one confusion line 1044 forbids — a caller reading
/// `None` as "nothing to preserve" on an unreadable tree would preserve
/// nothing when it should preserve everything. So this reads the process
/// output itself: `None` for every way of not getting an answer (`git`
/// absent, not a repository, a nonzero exit, output that is not UTF-8), and
/// `Some(vec![])` only for a clean tree.
pub fn changed_paths(root: &Path) -> Option<Vec<String>> {
    let output = std::process::Command::new("git")
        .args([
            "status",
            "--porcelain=v1",
            "-z",
            "--untracked-files=all",
            "--",
        ])
        .current_dir(root)
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_COMMON_DIR")
        .env_remove("GIT_INDEX_FILE")
        .stdin(std::process::Stdio::null())
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8(output.stdout).ok()?;

    let mut records = text.split('\0').filter(|record| !record.is_empty());
    let mut paths = Vec::new();
    while let Some(record) = records.next() {
        if record.len() < 3 {
            continue;
        }
        let status = &record[..2];
        if status.starts_with('R') || status.starts_with('C') {
            // The old path is this record (with the status); the new path —
            // where the working tree holds the file now — is the next `-z`
            // record, with no status prefix of its own.
            if let Some(new_path) = records.next() {
                paths.push(new_path.to_owned());
            }
        } else {
            paths.push(record[3..].to_owned());
        }
    }
    Some(paths)
}

#[cfg(test)]
mod tests {
    use super::*;

    const COMMIT: &str = "0123456789abcdef0123456789abcdef01234567";
    const OTHER: &str = "fedcba9876543210fedcba9876543210fedcba98";

    fn write(path: &Path, contents: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, contents).unwrap();
    }

    /// The ordinary case: a checkout with a `.git` directory, on a branch.
    #[test]
    fn a_plain_checkout_reports_its_branch_and_commit() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        write(&root.join(".git/HEAD"), "ref: refs/heads/main\n");
        write(&root.join(".git/refs/heads/main"), &format!("{COMMIT}\n"));

        assert_eq!(
            GitPosition::detect(root),
            Some(GitPosition {
                branch: Some("main".to_owned()),
                commit: COMMIT.to_owned(),
            })
        );
    }

    /// **The case Glasshouse's own development runs in.** A linked worktree's
    /// `.git` is a file, its `HEAD` is beside the file it names, and the
    /// branch it is on lives in the *common* directory — so a reader that
    /// looked only next to `HEAD` would find nothing here.
    #[test]
    fn a_linked_worktree_resolves_through_its_pointer_and_common_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let main = tmp.path().join("main");
        let linked = tmp.path().join("linked");
        let worktree_git = main.join(".git/worktrees/linked");

        write(&main.join(".git/HEAD"), "ref: refs/heads/main\n");
        write(&main.join(".git/refs/heads/main"), &format!("{OTHER}\n"));
        write(&worktree_git.join("HEAD"), "ref: refs/heads/side\n");
        write(&worktree_git.join("commondir"), &format!("{}\n", "../.."));
        write(&main.join(".git/refs/heads/side"), &format!("{COMMIT}\n"));
        write(
            &linked.join(".git"),
            &format!("gitdir: {}\n", worktree_git.display()),
        );

        assert_eq!(
            GitPosition::detect(&linked),
            Some(GitPosition {
                branch: Some("side".to_owned()),
                commit: COMMIT.to_owned(),
            }),
            "a linked worktree must report its own branch, resolved through the \
             common directory"
        );
        // And the main checkout still reports its own, unchanged.
        assert_eq!(
            GitPosition::detect(&main).unwrap().commit,
            OTHER,
            "reading the worktree must not have disturbed the main checkout"
        );
    }

    /// A relative `gitdir:` pointer resolves against the project root, which
    /// is what Git itself writes for a worktree beside its repository.
    #[test]
    fn a_relative_gitdir_pointer_resolves_against_the_project_root() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("wt");
        write(&root.join("real/HEAD"), "ref: refs/heads/main\n");
        write(&root.join("real/refs/heads/main"), &format!("{COMMIT}\n"));
        write(&root.join(".git"), "gitdir: real\n");

        assert_eq!(
            GitPosition::detect(&root).unwrap().commit,
            COMMIT.to_owned()
        );
    }

    /// A branch that has been packed has no loose ref file at all. Reading
    /// only loose refs would report nothing for a freshly cloned repository,
    /// where every branch is packed.
    #[test]
    fn a_packed_branch_is_found_in_packed_refs() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        write(&root.join(".git/HEAD"), "ref: refs/heads/main\n");
        write(
            &root.join(".git/packed-refs"),
            &format!(
                "# pack-refs with: peeled fully-peeled sorted\n\
                 {OTHER} refs/heads/other\n\
                 {COMMIT} refs/heads/main\n\
                 {OTHER} refs/tags/v1\n\
                 ^{COMMIT}\n"
            ),
        );

        assert_eq!(
            GitPosition::detect(root),
            Some(GitPosition {
                branch: Some("main".to_owned()),
                commit: COMMIT.to_owned(),
            })
        );
    }

    /// A peeled tag line must not be read as a ref. `^<object>` describes the
    /// line above it, and taking it as a `<object> <name>` pair would
    /// attribute one tag's object to whatever parsed next.
    #[test]
    fn a_peeled_tag_line_is_not_mistaken_for_a_ref() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        write(&root.join(".git/HEAD"), "ref: refs/heads/missing\n");
        write(
            &root.join(".git/packed-refs"),
            &format!("{OTHER} refs/tags/v1\n^{COMMIT}\n"),
        );
        assert_eq!(
            GitPosition::detect(root),
            None,
            "a branch that is not in packed-refs must not resolve to a tag's object"
        );
    }

    /// A detached HEAD has a commit and no branch, and the two facts stay
    /// separable — `None` here means detached, never "not recorded", because
    /// a position without a commit is not returned at all.
    #[test]
    fn a_detached_head_reports_its_commit_and_no_branch() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        write(&root.join(".git/HEAD"), &format!("{COMMIT}\n"));

        assert_eq!(
            GitPosition::detect(root),
            Some(GitPosition {
                branch: None,
                commit: COMMIT.to_owned(),
            })
        );
    }

    /// Every way of having no position answers the same way, because a
    /// checkpoint is worth taking without one.
    #[test]
    fn anything_unreadable_is_simply_no_position() {
        let tmp = tempfile::tempdir().unwrap();

        // Not a repository at all.
        let plain = tmp.path().join("plain");
        std::fs::create_dir_all(&plain).unwrap();
        assert_eq!(GitPosition::detect(&plain), None);

        // A repository mid-operation, whose branch ref is not written yet.
        let pending = tmp.path().join("pending");
        write(&pending.join(".git/HEAD"), "ref: refs/heads/main\n");
        assert_eq!(GitPosition::detect(&pending), None);

        // A HEAD holding something that is not an object name. Reporting it
        // would put a commit that does not exist into a handoff document.
        let nonsense = tmp.path().join("nonsense");
        write(&nonsense.join(".git/HEAD"), "not a commit\n");
        assert_eq!(GitPosition::detect(&nonsense), None);

        let truncated = tmp.path().join("truncated");
        write(&truncated.join(".git/HEAD"), "ref: refs/heads/main\n");
        write(&truncated.join(".git/refs/heads/main"), "0123456\n");
        assert_eq!(GitPosition::detect(&truncated), None);
    }

    /// A SHA-256 repository's object names are 64 characters. Accepting only
    /// 40 would silently report no position for every one of them.
    #[test]
    fn a_sha256_object_name_is_accepted() {
        assert!(is_object_name(&"a".repeat(64)));
        assert!(is_object_name(COMMIT));
        assert!(!is_object_name(&"a".repeat(41)));
        assert!(!is_object_name(&"g".repeat(40)));
    }

    /// The real repository this test is running in.
    ///
    /// Every case above is a fixture, and a fixture only proves the parser
    /// against what the test author believed Git writes. This one reads the
    /// actual checkout — which, for this project, is normally a linked
    /// worktree — and holds the result to what the repository itself says.
    #[test]
    fn the_repository_this_test_runs_in_is_readable() {
        // `CARGO_MANIFEST_DIR` is `<checkout>/crates/glasshouse`.
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(Path::parent)
            .expect("the manifest is two directories below the checkout root")
            .to_path_buf();
        if !the_repository_is_actually_readable(&root) {
            // A copied tree — see the helper below. Reporting no position is
            // the correct answer here, and asserting it is what keeps this
            // from being a silent skip.
            assert_eq!(
                GitPosition::detect(&root),
                None,
                "a repository whose HEAD cannot be read must report no \
                 position, never a half-read one"
            );
            return;
        }

        let position = GitPosition::detect(&root)
            .expect("this checkout's HEAD is readable, so a position must be readable from it");
        assert!(
            is_object_name(&position.commit),
            "read a commit that is not an object name: {position:?}"
        );
    }

    /// Whether this checkout's `.git` entry actually leads anywhere.
    ///
    /// **A `.git` entry existing is not the same as a repository being
    /// readable**, and the difference is not exotic — it is the ordinary state
    /// of a *copied* tree. Glasshouse is developed in linked git worktrees,
    /// where `.git` is a file holding `gitdir: <absolute path>`; copy that
    /// tree into a container, a source tarball, or an image build, and the
    /// file arrives while the directory it names does not.
    ///
    /// A Linux container run caught exactly that, on the version of this test
    /// that asserted a position must be readable *because a `.git` entry
    /// exists*. It passed on the machine it was written on, where the pointer
    /// resolves, and could not have failed there. Reporting no position in a
    /// copied tree is the **correct** answer, so the premise is now checked by
    /// resolving the pointer rather than by testing for the entry.
    fn the_repository_is_actually_readable(root: &Path) -> bool {
        let dot_git = root.join(".git");
        let Ok(metadata) = std::fs::symlink_metadata(&dot_git) else {
            return false;
        };
        let git_dir = if metadata.is_dir() {
            dot_git
        } else {
            let Ok(pointer) = std::fs::read_to_string(&dot_git) else {
                return false;
            };
            let Some(target) = pointer.trim().strip_prefix("gitdir:") else {
                return false;
            };
            let target = Path::new(target.trim());
            if target.is_absolute() {
                target.to_path_buf()
            } else {
                root.join(target)
            }
        };
        git_dir.join("HEAD").is_file()
    }

    /// Build a version-2 index file byte for byte, so a test can assert
    /// against a known, exact input rather than one only `git` could have
    /// produced.
    ///
    /// Each entry is `(path, mtime_secs, mtime_nanos, size, mode)`; every
    /// other stat field the real format carries (ctime, dev, ino, uid, gid,
    /// the object hash) is written as zero, because [`parse_index`] never
    /// reads them.
    fn write_index_v2(entries: &[(&str, u32, u32, u32, u32)]) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(b"DIRC");
        out.extend_from_slice(&2u32.to_be_bytes()); // version
        out.extend_from_slice(&(entries.len() as u32).to_be_bytes());

        for &(path, mtime_secs, mtime_nanos, size, mode) in entries {
            out.extend_from_slice(&0u32.to_be_bytes()); // ctime secs
            out.extend_from_slice(&0u32.to_be_bytes()); // ctime nsecs
            out.extend_from_slice(&mtime_secs.to_be_bytes());
            out.extend_from_slice(&mtime_nanos.to_be_bytes());
            out.extend_from_slice(&0u32.to_be_bytes()); // dev
            out.extend_from_slice(&0u32.to_be_bytes()); // ino
            out.extend_from_slice(&mode.to_be_bytes());
            out.extend_from_slice(&0u32.to_be_bytes()); // uid
            out.extend_from_slice(&0u32.to_be_bytes()); // gid
            out.extend_from_slice(&size.to_be_bytes());
            out.extend_from_slice(&[0u8; 20]); // sha1, unread by this parser
            let name_len = (path.len() as u16).min(0x0FFF);
            out.extend_from_slice(&name_len.to_be_bytes());

            let entry_start = out.len() - 62;
            out.extend_from_slice(path.as_bytes());
            let entry_len = out.len() - entry_start;
            let pad = match entry_len % 8 {
                0 => 8,
                rem => 8 - rem,
            };
            out.extend(std::iter::repeat_n(0u8, pad));
        }
        out
    }

    /// A tracked file whose size and modification time still match the
    /// index is not reported as changed.
    ///
    /// The file's real metadata is read back after writing it, rather than a
    /// value chosen by the test, because filesystem mtime resolution varies
    /// by platform: asserting against whatever the filesystem actually
    /// recorded is the only way this is not occasionally flaky.
    #[test]
    fn a_file_matching_the_index_is_not_reported_changed() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        write(&root.join("tracked.txt"), "unchanged\n");
        let metadata = std::fs::metadata(root.join("tracked.txt")).unwrap();
        let modified = metadata.modified().unwrap();
        let since_epoch = modified.duration_since(std::time::UNIX_EPOCH).unwrap();

        let index = write_index_v2(&[(
            "tracked.txt",
            since_epoch.as_secs() as u32,
            since_epoch.subsec_nanos(),
            metadata.len() as u32,
            0o100644,
        )]);
        std::fs::create_dir_all(root.join(".git")).unwrap();
        std::fs::write(root.join(".git/index"), &index).unwrap();

        let status = WorkingTreeStatus::detect(root).expect("a readable index");
        assert!(!status.dirty, "reported dirty against a matching file");
        assert!(status.changed_files.is_empty());
    }

    /// A tracked file whose size no longer matches the index is reported
    /// dirty, and named.
    #[test]
    fn a_file_whose_size_changed_is_reported_dirty_and_named() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        write(&root.join("changed.txt"), "now much longer than before\n");

        // The index remembers a size this file no longer has; the actual
        // recorded mtime does not matter for this assertion; zero is fine.
        let index = write_index_v2(&[("changed.txt", 0, 0, 1, 0o100644)]);
        std::fs::create_dir_all(root.join(".git")).unwrap();
        std::fs::write(root.join(".git/index"), &index).unwrap();

        let status = WorkingTreeStatus::detect(root).expect("a readable index");
        assert!(status.dirty);
        assert_eq!(status.changed_files, vec!["changed.txt".to_owned()]);
    }

    /// A tracked file the index still names but that no longer exists on
    /// disk — a deletion — is reported dirty rather than silently ignored.
    #[test]
    fn a_deleted_tracked_file_is_reported_dirty() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(root.join(".git")).unwrap();

        let index = write_index_v2(&[("gone.txt", 0, 0, 5, 0o100644)]);
        std::fs::write(root.join(".git/index"), &index).unwrap();

        let status = WorkingTreeStatus::detect(root).expect("a readable index");
        assert!(status.dirty);
        assert_eq!(status.changed_files, vec!["gone.txt".to_owned()]);
    }

    /// A submodule entry (`160000`, a "gitlink") is never compared against a
    /// regular file on disk: there normally is no such file, and reporting
    /// one changed for a reason that has nothing to do with its content
    /// would be worse than saying nothing.
    #[test]
    fn a_submodule_gitlink_is_never_reported_changed() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(root.join(".git")).unwrap();

        let index = write_index_v2(&[("vendor/lib", 0, 0, 0, 0o160000)]);
        std::fs::write(root.join(".git/index"), &index).unwrap();

        let status = WorkingTreeStatus::detect(root).expect("a readable index");
        assert!(!status.dirty, "a gitlink entry must never be compared");
    }

    /// The changed-files list stops growing at the cap, but `dirty` keeps
    /// reporting the truth: a checkpoint naming twenty files still says the
    /// tree is dirty even though a twenty-first file also changed.
    #[test]
    fn changed_files_is_capped_but_dirty_still_reports_true() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(root.join(".git")).unwrap();

        let entries: Vec<(String, u32, u32, u32, u32)> = (0..(MAX_CHANGED_FILES + 5))
            .map(|i| (format!("gone{i}.txt"), 0, 0, 5, 0o100644))
            .collect();
        let borrowed: Vec<(&str, u32, u32, u32, u32)> = entries
            .iter()
            .map(|(path, a, b, c, d)| (path.as_str(), *a, *b, *c, *d))
            .collect();
        let index = write_index_v2(&borrowed);
        std::fs::write(root.join(".git/index"), &index).unwrap();

        let status = WorkingTreeStatus::detect(root).expect("a readable index");
        assert!(status.dirty);
        assert_eq!(status.changed_files.len(), MAX_CHANGED_FILES);
    }

    /// An index this reader does not understand — here, a future format
    /// version — reads as no status available, never a wrong one.
    #[test]
    fn an_unsupported_index_version_yields_no_status() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(root.join(".git")).unwrap();
        let mut header = Vec::new();
        header.extend_from_slice(b"DIRC");
        header.extend_from_slice(&4u32.to_be_bytes());
        header.extend_from_slice(&0u32.to_be_bytes());
        std::fs::write(root.join(".git/index"), &header).unwrap();

        assert_eq!(WorkingTreeStatus::detect(root), None);
    }

    /// A repository with no index yet — nothing has ever been added — reads
    /// as no status available rather than a false "clean".
    #[test]
    fn a_repository_with_no_index_yields_no_status() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(root.join(".git")).unwrap();
        assert_eq!(WorkingTreeStatus::detect(root), None);
    }

    /// The real repository this test is running in, the same way
    /// [`the_repository_this_test_runs_in_is_readable`] proves `GitPosition`
    /// against it: a hand-built fixture only proves the parser against what
    /// the test author believed an index looks like, and this project's own
    /// index — normally at a linked worktree, normally with real, ordinary
    /// changes sitting in it during development — is a fixture nobody
    /// authored.
    #[test]
    fn working_tree_status_reads_the_real_checkout_this_test_runs_in() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(Path::parent)
            .expect("the manifest is two directories below the checkout root")
            .to_path_buf();
        if !the_repository_is_actually_readable(&root) {
            // Same copied-tree case `GitPosition`'s own test guards against.
            return;
        }

        let status = WorkingTreeStatus::detect(&root)
            .expect("this checkout's index is readable, so a status must be readable from it");
        for path in &status.changed_files {
            assert!(!path.is_empty());
            assert!(
                !path.starts_with('/'),
                "a changed-file path must be relative to the repository root: {path}"
            );
        }
    }

    // -------------------------------------------------------------------
    // `changed_paths` — a real repository, a real `git` subprocess.
    // -------------------------------------------------------------------

    fn git(root: &Path, args: &[&str]) -> String {
        let output = std::process::Command::new("git")
            .args(args)
            .current_dir(root)
            .output()
            .expect("git must be installed on every leg this gate runs");
        assert!(
            output.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap().trim().to_owned()
    }

    /// A repository with an identity of its own, so the test never depends
    /// on the machine's `user.name`/`user.email` being configured.
    fn git_init(root: &Path) {
        git(root, &["init", "--quiet"]);
        git(root, &["config", "user.name", "Glasshouse Test"]);
        git(root, &["config", "user.email", "test@example.invalid"]);
        git(root, &["config", "commit.gpgsign", "false"]);
    }

    /// A clean checkout — nothing staged, nothing untracked — reports an
    /// empty list, not `None`: *clean* and *unknown* are different answers.
    #[test]
    fn a_clean_tree_is_some_empty_not_none() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        git_init(root);
        std::fs::write(root.join("committed.txt"), "steady\n").unwrap();
        git(root, &["add", "--", "committed.txt"]);
        git(root, &["commit", "--quiet", "-m", "initial"]);

        assert_eq!(changed_paths(root), Some(Vec::new()));
    }

    /// An untracked file the transitioning session never staged still shows
    /// up — the gap `WorkingTreeStatus::detect` cannot close.
    #[test]
    fn an_untracked_file_is_reported() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        git_init(root);
        std::fs::write(root.join("committed.txt"), "steady\n").unwrap();
        git(root, &["add", "--", "committed.txt"]);
        git(root, &["commit", "--quiet", "-m", "initial"]);

        std::fs::write(root.join("notes.md"), "someone else's edit\n").unwrap();

        assert_eq!(changed_paths(root), Some(vec!["notes.md".to_owned()]));
    }

    /// A tracked file modified but not staged is reported too.
    #[test]
    fn a_modified_tracked_file_is_reported() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        git_init(root);
        std::fs::write(root.join("tracked.txt"), "before\n").unwrap();
        git(root, &["add", "--", "tracked.txt"]);
        git(root, &["commit", "--quiet", "-m", "initial"]);

        std::fs::write(root.join("tracked.txt"), "after\n").unwrap();

        assert_eq!(changed_paths(root), Some(vec!["tracked.txt".to_owned()]));
    }

    /// A path nested under a subdirectory is reported repo-relative and
    /// `/`-separated, the same spelling `FileClaim::path` uses.
    #[test]
    fn a_nested_path_is_repo_relative_and_slash_separated() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        git_init(root);
        std::fs::write(root.join("README.md"), "root\n").unwrap();
        git(root, &["add", "--", "README.md"]);
        git(root, &["commit", "--quiet", "-m", "initial"]);

        std::fs::create_dir_all(root.join("src/nested")).unwrap();
        std::fs::write(root.join("src/nested/new.rs"), "// new\n").unwrap();

        assert_eq!(
            changed_paths(root),
            Some(vec!["src/nested/new.rs".to_owned()])
        );
    }

    /// Not a repository at all: `None`, never an empty list that would read
    /// as "nothing to preserve".
    #[test]
    fn a_non_repository_is_none() {
        let tmp = tempfile::tempdir().unwrap();
        assert_eq!(changed_paths(tmp.path()), None);
    }

    /// A path that does not exist at all is still no repository, and still
    /// `None`.
    #[test]
    fn a_missing_root_is_none() {
        let tmp = tempfile::tempdir().unwrap();
        assert_eq!(changed_paths(&tmp.path().join("does-not-exist")), None);
    }
}
