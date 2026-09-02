//! `GH-MEMORY-EXPORT` — map line 2040, Phase 58 item 6: *"An opt-in export
//! of remembered constraints and failed approaches into a marker-delimited
//! block of the harness's native local instruction file, gitignored by
//! default, replacing only its own block on re-export."*
//!
//! # Why this enters through the binary
//!
//! `glasshouse memory export-local` is the only production caller of
//! `glasshouse::memory::export_local::export` (practice §35): calling the
//! library function directly and reading the file back would prove the
//! renderer works and nothing about whether an operator can reach it through
//! the shipped CLI, or whether the opt-in gate the map asks for actually
//! gates the binary rather than a test harness that bypasses it.
//!
//! Seeding memories goes straight through `glasshouse::memory::ProjectMemory`,
//! the same shape `tests/tracked_knowledge.rs` and `tests/memory_rating.rs`
//! use: this file proves the export-local door, not memory storage itself.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use glasshouse::memory::{
    MemoryAuthority, MemoryId, MemoryKind, MemoryStatus, NewMemory, ProjectMemory,
};

struct Fixture {
    base: PathBuf,
    root: PathBuf,
}

impl Fixture {
    /// A project root with a `.git` directory that is not necessarily a
    /// runnable repository — the shape every other shipped-binary test in
    /// this crate uses, and enough for every assertion here except the
    /// `git status` ones, which use [`Fixture::with_real_git`].
    fn new(base: &Path, name: &str) -> Self {
        let root = base.join("workspace").join(name);
        std::fs::create_dir_all(root.join(".git")).expect("create project root");
        std::fs::create_dir_all(base.join("config")).expect("create config dir");
        let root = std::fs::canonicalize(&root).expect("canonicalize the project root");
        Self {
            base: base.to_path_buf(),
            root,
        }
    }

    /// A project root under a real `git init`, for the one test that reads
    /// `git status` back — a directory merely named `.git` is not a
    /// repository `git` itself will operate on.
    fn with_real_git(base: &Path, name: &str) -> Self {
        let root = base.join("workspace").join(name);
        std::fs::create_dir_all(&root).expect("create project root");
        std::fs::create_dir_all(base.join("config")).expect("create config dir");
        let status = Command::new("git")
            .args(["init", "-q"])
            .current_dir(&root)
            .status()
            .expect("git must be runnable");
        assert!(status.success(), "git init failed");
        let root = std::fs::canonicalize(&root).expect("canonicalize the project root");
        Self {
            base: base.to_path_buf(),
            root,
        }
    }

    fn cli(&self) -> glasshouse::cli::Cli {
        glasshouse::cli::Cli {
            scope: Some(self.root.clone()),
            allow_unsafe_scope: false,
            data_dir: Some(self.base.join("data")),
            config_dir: Some(self.base.join("config")),
            log_level: None,
            log_file: None,
            log_stderr: false,
            command: None,
        }
    }

    fn runtime(&self) -> glasshouse::Runtime {
        glasshouse::bootstrap(&self.cli(), &self.root).expect("bootstrap")
    }

    /// Record a memory directly through the store, bypassing extraction —
    /// the same shortcut `tests/tracked_knowledge.rs` takes, because what
    /// this file proves is the export door, not how a memory is learned.
    fn record(&self, kind: MemoryKind, body: &str, authority: Option<MemoryAuthority>) -> MemoryId {
        let runtime = self.runtime();
        let memory = ProjectMemory::open(&runtime).expect("open the project memory");
        memory
            .store()
            .record(NewMemory::new(kind, body).with_authority(authority))
            .expect("record a memory")
            .id
    }

    fn supersede(&self, id: &MemoryId) {
        let runtime = self.runtime();
        let memory = ProjectMemory::open(&runtime).expect("open the project memory");
        memory
            .store()
            .set_status(id, MemoryStatus::Superseded)
            .expect("supersede the memory");
    }

    fn glasshouse(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_glasshouse"))
            .arg("--scope")
            .arg(&self.root)
            .arg("--data-dir")
            .arg(self.base.join("data"))
            .arg("--config-dir")
            .arg(self.base.join("config"))
            .args(args)
            .output()
            .expect("the glasshouse binary must be runnable")
    }

    fn export_local(&self) -> Output {
        self.glasshouse(&["memory", "export-local"])
    }

    fn local_instruction_file(&self) -> PathBuf {
        self.root.join("CLAUDE.local.md")
    }

    fn read_local_instruction_file(&self) -> Option<String> {
        std::fs::read_to_string(self.local_instruction_file()).ok()
    }
}

fn tempdir() -> tempfile::TempDir {
    tempfile::tempdir().unwrap()
}

fn assert_success(output: &Output, label: &str) {
    assert!(
        output.status.success(),
        "`{label}` failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

// -------------------------------------------------------------------------
// (a) — only current binding memories and failed attempts, nothing else
// -------------------------------------------------------------------------

#[test]
fn exports_only_binding_memories_and_failed_attempts_never_decisions() {
    let tmp = tempdir();
    let fixture = Fixture::new(tmp.path(), "alpha");

    fixture.record(
        MemoryKind::Constraint,
        "onyx requests are capped at 64kb",
        Some(MemoryAuthority::Constraint),
    );
    fixture.record(
        MemoryKind::FailedAttempt,
        "onyx tried a global lock and it deadlocked under load",
        None,
    );
    // Left unclassified — the extraction default, and the realistic shape
    // of a decision nobody has promoted. `MemoryAuthority::Decision` is
    // itself one of `binding`'s three classes (a decision may be presented
    // as binding once promoted), so this test's claim is about the ordinary
    // unclassified case, not every decision unconditionally.
    fixture.record(
        MemoryKind::Decision,
        "onyx caching is keyed by content hash",
        None,
    );

    let output = fixture.export_local();
    assert_success(&output, "memory export-local");

    let content = fixture
        .read_local_instruction_file()
        .expect("the local instruction file must exist");

    assert!(content.contains("<!-- glasshouse:memory:begin -->"));
    assert!(content.contains("<!-- glasshouse:memory:end -->"));
    assert!(
        content.contains("kind=constraint"),
        "the constraint must appear: {content}"
    );
    assert!(
        content.contains("kind=failed_attempt"),
        "the failed attempt must appear: {content}"
    );
    assert!(
        !content.contains("kind=decision"),
        "a decision must never be exported: {content}"
    );
    // Every entry `render_entry` produces opens with `[position/total ...]` —
    // the same head line an injected memory carries.
    assert!(content.contains("[1/2"), "got: {content}");
    assert!(content.contains("[2/2"), "got: {content}");
}

// -------------------------------------------------------------------------
// (b) — re-export replaces only the block
// -------------------------------------------------------------------------

#[test]
fn reexport_replaces_only_the_block_leaving_surrounding_bytes_identical() {
    let tmp = tempdir();
    let fixture = Fixture::new(tmp.path(), "alpha");

    fixture.record(
        MemoryKind::Constraint,
        "onyx requests are capped at 64kb",
        Some(MemoryAuthority::Constraint),
    );

    let first = fixture.export_local();
    assert_success(&first, "memory export-local");
    let after_first = fixture.read_local_instruction_file().unwrap();

    // The user edits the file by hand: text above and below the block.
    let hand_edited = format!("# my own notes\n\n{after_first}\n# more notes below\n");
    std::fs::write(fixture.local_instruction_file(), &hand_edited).unwrap();

    fixture.record(
        MemoryKind::Constraint,
        "onyx also caps concurrent connections at 200",
        Some(MemoryAuthority::Constraint),
    );

    let second = fixture.export_local();
    assert_success(&second, "memory export-local");
    let after_second = fixture.read_local_instruction_file().unwrap();

    assert!(
        after_second.starts_with("# my own notes\n\n"),
        "text above the block must survive untouched: {after_second}"
    );
    assert!(
        after_second.ends_with("\n# more notes below\n"),
        "text below the block must survive untouched: {after_second}"
    );
    assert!(
        after_second.contains("concurrent connections"),
        "the block itself must have been regenerated: {after_second}"
    );
}

// -------------------------------------------------------------------------
// (c) — supersession drops a memory from the block; nothing left removes it
// -------------------------------------------------------------------------

#[test]
fn superseding_the_only_constraint_removes_it_from_the_block() {
    let tmp = tempdir();
    let fixture = Fixture::new(tmp.path(), "alpha");

    let constraint = fixture.record(
        MemoryKind::Constraint,
        "onyx requests are capped at 64kb",
        Some(MemoryAuthority::Constraint),
    );
    fixture.record(
        MemoryKind::FailedAttempt,
        "onyx tried a global lock and it deadlocked under load",
        None,
    );

    let first = fixture.export_local();
    assert_success(&first, "memory export-local");
    let first_content = fixture.read_local_instruction_file().unwrap();
    assert!(first_content.contains("kind=constraint"));

    fixture.supersede(&constraint);

    let second = fixture.export_local();
    assert_success(&second, "memory export-local");
    let second_content = fixture.read_local_instruction_file().unwrap();
    assert!(
        !second_content.contains("kind=constraint"),
        "a superseded constraint must not appear: {second_content}"
    );
    assert!(
        second_content.contains("kind=failed_attempt"),
        "the still-current failed attempt must remain: {second_content}"
    );
}

#[test]
fn exporting_with_nothing_left_removes_the_block_and_keeps_user_text() {
    let tmp = tempdir();
    let fixture = Fixture::new(tmp.path(), "alpha");

    let constraint = fixture.record(
        MemoryKind::Constraint,
        "onyx requests are capped at 64kb",
        Some(MemoryAuthority::Constraint),
    );

    let first = fixture.export_local();
    assert_success(&first, "memory export-local");
    let after_first = fixture.read_local_instruction_file().unwrap();

    let hand_edited = format!("# my own notes\n\n{after_first}\n# more notes below\n");
    std::fs::write(fixture.local_instruction_file(), &hand_edited).unwrap();

    fixture.supersede(&constraint);

    let second = fixture.export_local();
    assert_success(&second, "memory export-local");
    let after_second = fixture.read_local_instruction_file().unwrap();

    assert!(
        !after_second.contains("glasshouse:memory:begin"),
        "the block must be gone entirely: {after_second}"
    );
    assert_eq!(
        after_second, "# my own notes\n\n# more notes below\n",
        "the user's own text must be exactly what remains"
    );
}

// -------------------------------------------------------------------------
// (d) — gitignored by default, once, and `--no-exclude` skips it
// -------------------------------------------------------------------------

#[test]
fn the_exclude_file_gains_the_pattern_once_and_git_status_sees_nothing() {
    let tmp = tempdir();
    let fixture = Fixture::with_real_git(tmp.path(), "alpha");

    fixture.record(
        MemoryKind::Constraint,
        "onyx requests are capped at 64kb",
        Some(MemoryAuthority::Constraint),
    );

    let first = fixture.export_local();
    assert_success(&first, "memory export-local");

    let exclude_path = fixture.root.join(".git").join("info").join("exclude");
    let exclude_contents = std::fs::read_to_string(&exclude_path).unwrap();
    assert_eq!(
        exclude_contents.matches("CLAUDE.local.md").count(),
        1,
        "got: {exclude_contents}"
    );

    let second = fixture.export_local();
    assert_success(&second, "memory export-local");
    let exclude_contents_again = std::fs::read_to_string(&exclude_path).unwrap();
    assert_eq!(
        exclude_contents_again.matches("CLAUDE.local.md").count(),
        1,
        "a second export must not duplicate the pattern: {exclude_contents_again}"
    );

    let status = Command::new("git")
        .args(["status", "--porcelain"])
        .current_dir(&fixture.root)
        .output()
        .expect("git must be runnable");
    let status_text = String::from_utf8_lossy(&status.stdout);
    assert!(
        !status_text.contains("CLAUDE.local.md"),
        "an ignored file must not appear in git status: {status_text}"
    );
}

#[test]
fn no_exclude_leaves_the_exclude_file_untouched() {
    let tmp = tempdir();
    let fixture = Fixture::with_real_git(tmp.path(), "alpha");

    fixture.record(
        MemoryKind::Constraint,
        "onyx requests are capped at 64kb",
        Some(MemoryAuthority::Constraint),
    );

    let output = fixture.glasshouse(&["memory", "export-local", "--no-exclude"]);
    assert_success(&output, "memory export-local --no-exclude");

    let exclude_path = fixture.root.join(".git").join("info").join("exclude");
    let exists_with_pattern = std::fs::read_to_string(&exclude_path)
        .map(|contents| contents.contains("CLAUDE.local.md"))
        .unwrap_or(false);
    assert!(
        !exists_with_pattern,
        "--no-exclude must never write the exclude file"
    );
}

// -------------------------------------------------------------------------
// (e) — an unsupported harness is refused by name
// -------------------------------------------------------------------------

#[test]
fn an_unsupported_harness_is_refused_by_name_and_nothing_is_written() {
    let tmp = tempdir();
    let fixture = Fixture::new(tmp.path(), "alpha");

    fixture.record(
        MemoryKind::Constraint,
        "onyx requests are capped at 64kb",
        Some(MemoryAuthority::Constraint),
    );

    let output = fixture.glasshouse(&["memory", "export-local", "--harness", "codex"]);
    assert!(
        !output.status.success(),
        "an unsupported harness must be refused"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("codex"),
        "the refusal should name the harness it refused; got: {stderr}"
    );

    assert!(
        fixture.read_local_instruction_file().is_none(),
        "nothing must be written on a refused harness"
    );
}
