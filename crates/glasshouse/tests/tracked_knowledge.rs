//! Phase 50 — tracked project knowledge, map lines 1810-1816.
//!
//! # Why this enters through the binary
//!
//! `glasshouse memory export --tracked` is the only production caller of
//! `glasshouse::memory::TrackedKnowledge::write` (practice §35): calling the
//! library function directly and reading the files back would prove the
//! renderer works and nothing about whether an operator can reach it, or
//! whether the opt-in gate the map asks for actually gates the shipped
//! binary rather than a test harness that bypasses it.
//!
//! Seeding memories goes straight through `glasshouse::memory::ProjectMemory`,
//! the same shape `tests/memory_conflict_cli.rs` uses: this file proves the
//! export door, not memory storage itself.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use glasshouse::memory::{
    Classifier, MemoryAuthority, MemoryId, MemoryKind, NewMemory, ProjectMemory,
};

struct Fixture {
    base: PathBuf,
    root: PathBuf,
}

impl Fixture {
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

    fn record(&self, kind: MemoryKind, subject: &str, body: &str) -> MemoryId {
        let runtime = self.runtime();
        let memory = ProjectMemory::open(&runtime).expect("open the project memory");
        memory
            .store()
            .record(NewMemory::new(kind, body).with_subject(Some(subject)))
            .expect("record a memory")
            .id
    }

    fn promote(&self, id: &MemoryId, authority: MemoryAuthority) {
        let runtime = self.runtime();
        let memory = ProjectMemory::open(&runtime).expect("open the project memory");
        memory
            .store()
            .set_authority(id, Some(authority), Classifier::Reviewed)
            .expect("promote the memory");
    }

    fn knowledge_dir(&self) -> PathBuf {
        self.root.join(".glasshouse").join("knowledge")
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

    /// Every file under `.glasshouse/knowledge/`, by file name, with its
    /// bytes — for comparing two exports without depending on directory
    /// iteration order.
    fn exported_files(&self) -> std::collections::BTreeMap<String, Vec<u8>> {
        let dir = self.knowledge_dir();
        let mut files = std::collections::BTreeMap::new();
        if !dir.exists() {
            return files;
        }
        for entry in std::fs::read_dir(&dir).expect("read the knowledge directory") {
            let entry = entry.expect("read a directory entry");
            if entry.file_type().expect("file type").is_file() {
                let name = entry.file_name().to_string_lossy().into_owned();
                let bytes = std::fs::read(entry.path()).expect("read an exported file");
                files.insert(name, bytes);
            }
        }
        files
    }
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

/// Map lines 1810 and 1811: runtime memory stays outside the repository
/// whether or not tracked knowledge is ever used, and `memory export`
/// without `--tracked` writes nothing.
#[test]
fn runtime_memory_lives_outside_the_repository_by_default_and_nothing_is_exported_without_opting_in()
 {
    let tmp = tempfile::tempdir().expect("tempdir");
    let fixture = Fixture::new(tmp.path(), "workspace");
    fixture.record(
        MemoryKind::Decision,
        "use SQLite for project state",
        "SQLite keeps every project's state in one file with no server to run",
    );

    // Premise: the runtime's own state directory is never inside the project
    // root, with or without tracked knowledge in the picture.
    let runtime = fixture.runtime();
    assert!(
        !runtime.state_dir().starts_with(&fixture.root),
        "runtime state must live outside the project root: {} is under {}",
        runtime.state_dir().display(),
        fixture.root.display()
    );

    let result = fixture.glasshouse(&["memory", "export"]);
    assert!(
        result.status.success(),
        "`memory export` without --tracked must still succeed: {}",
        stderr(&result)
    );
    assert!(
        stdout(&result).contains("off by default"),
        "must say tracked knowledge is off by default:\n{}",
        stdout(&result)
    );
    assert!(
        !fixture.knowledge_dir().exists(),
        "nothing may be written to .glasshouse/knowledge without --tracked"
    );
}

/// Map lines 1811 and 1812: opting in exports decisions and constraints, and
/// leaves findings out unless asked for.
#[test]
fn opting_in_exports_decisions_and_constraints_as_readable_files_and_not_findings_by_default() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let fixture = Fixture::new(tmp.path(), "workspace");
    let decision = fixture.record(
        MemoryKind::Decision,
        "own migrations rather than an ORM",
        "hand-written migrations keep the schema legible in a diff",
    );
    let second_decision = fixture.record(
        MemoryKind::Decision,
        "prefer composition over inheritance here",
        "the trait hierarchy this replaced could not express the cross-cutting case",
    );
    let constraint = fixture.record(
        MemoryKind::Constraint,
        "single SQLite writer",
        "only one connection may hold the write lock at a time",
    );
    let finding = fixture.record(
        MemoryKind::Finding,
        "the flaky rollback test",
        "rollback tests that delete rows out of order leave a hole `MAX(version)` skips",
    );

    let exported = fixture.glasshouse(&["memory", "export", "--tracked"]);
    assert!(
        exported.status.success(),
        "`memory export --tracked` must succeed: {}",
        stderr(&exported)
    );

    let files = fixture.exported_files();
    let decision_file = format!("decision-{decision}.md");
    let constraint_file = format!("constraint-{constraint}.md");
    let finding_file = format!("finding-{finding}.md");

    assert!(
        files.contains_key(&decision_file),
        "a decision must be exported: {files:?}"
    );
    assert!(
        files.contains_key(&constraint_file),
        "a constraint must be exported: {files:?}"
    );
    assert!(
        !files.contains_key(&finding_file),
        "a finding must not be exported by default: {files:?}"
    );
    assert!(
        files.contains_key("README.md"),
        "a README explaining the projection must be written: {files:?}"
    );

    let decision_text = String::from_utf8_lossy(&files[&decision_file]).into_owned();
    assert!(
        decision_text.contains("own migrations rather than an ORM"),
        "the decision's subject must reach the file:\n{decision_text}"
    );
    assert!(
        decision_text.contains("hand-written migrations keep the schema legible"),
        "the decision's body must reach the file:\n{decision_text}"
    );

    // Ordering must be deterministic — by kind, then by id — never
    // filesystem or insertion order. Both decisions are listed before the
    // constraint, and between the two decisions the lower id is listed
    // first, regardless of which one was recorded first.
    let printed = stdout(&exported);
    let position_of = |needle: &str| {
        printed
            .find(needle)
            .unwrap_or_else(|| panic!("`{needle}` must appear in the manifest:\n{printed}"))
    };
    let mut decision_ids = [decision.to_string(), second_decision.to_string()];
    decision_ids.sort();
    assert!(
        position_of(&decision_ids[0]) < position_of(&decision_ids[1]),
        "decisions must be listed in id order:\n{printed}"
    );
    assert!(
        position_of(&decision_ids[1]) < position_of(&constraint.to_string()),
        "every decision must be listed before the constraint:\n{printed}"
    );

    // `--include-findings` widens the selection to the finding too, without
    // dropping what was already there.
    let exported_with_findings =
        fixture.glasshouse(&["memory", "export", "--tracked", "--include-findings"]);
    assert!(
        exported_with_findings.status.success(),
        "including findings must still succeed: {}",
        stderr(&exported_with_findings)
    );
    let files_with_findings = fixture.exported_files();
    assert!(
        files_with_findings.contains_key(&finding_file),
        "--include-findings must export the finding: {files_with_findings:?}"
    );
    assert!(
        files_with_findings.contains_key(&decision_file)
            && files_with_findings.contains_key(&constraint_file),
        "the decision and constraint must still be present"
    );
}

/// Map line 1816: a team reviews tracked knowledge through an ordinary Git
/// workflow, which only means something if a diff is meaningful — no change,
/// no diff; one change, one file's diff.
#[test]
fn two_exports_are_byte_identical_and_one_memory_change_is_one_file_diff() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let fixture = Fixture::new(tmp.path(), "workspace");
    let promoted = fixture.record(
        MemoryKind::Decision,
        "retries are capped at three",
        "a fourth retry rarely succeeds and only delays the failure",
    );
    let untouched = fixture.record(
        MemoryKind::Constraint,
        "no synchronous network calls on the render thread",
        "a network stall must never freeze the TUI",
    );

    let first = fixture.glasshouse(&["memory", "export", "--tracked"]);
    assert!(first.status.success(), "{}", stderr(&first));
    let first_files = fixture.exported_files();

    let second = fixture.glasshouse(&["memory", "export", "--tracked"]);
    assert!(second.status.success(), "{}", stderr(&second));
    let second_files = fixture.exported_files();

    assert_eq!(
        first_files, second_files,
        "two exports of an unchanged store must be byte-identical"
    );

    // Change one memory's authority — a real, recorded change that updates
    // `updated_at` — and leave the other alone.
    fixture.promote(&promoted, MemoryAuthority::Decision);

    let third = fixture.glasshouse(&["memory", "export", "--tracked"]);
    assert!(third.status.success(), "{}", stderr(&third));
    let third_files = fixture.exported_files();

    let promoted_file = format!("decision-{promoted}.md");
    let untouched_file = format!("constraint-{untouched}.md");

    assert_ne!(
        second_files[&promoted_file], third_files[&promoted_file],
        "the changed memory's file must differ after its authority changed"
    );
    assert_eq!(
        second_files[&untouched_file], third_files[&untouched_file],
        "an untouched memory's file must not change when another memory does"
    );
    assert_eq!(
        second_files["README.md"], third_files["README.md"],
        "the README carries no per-run timestamp and must not change either"
    );
}

/// Map lines 1813 and 1814: no session history, and no credential or
/// provider metadata, ever reaches a tracked-knowledge file.
#[test]
fn no_session_history_credential_or_provider_metadata_reaches_the_files() {
    // A source-scan of the exporter itself: it must never even import the
    // modules that could hand it a session, an event or a checkpoint,
    // because a caller that cannot reach the type cannot leak it by mistake
    // later. This is checked against the source rather than only behaviour,
    // per practice §35: a caller every test bypasses is not a caller, and the
    // matching failure mode here is an import nobody's test happens to
    // exercise.
    let source =
        std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/src/memory/export.rs"))
            .expect("read the exporter's own source");
    for forbidden in [
        "crate::session",
        "crate::events",
        "crate::checkpoint",
        "super::session",
        "EventLog",
        "ProjectSessions",
        "SessionStore",
    ] {
        assert!(
            !source.contains(forbidden),
            "memory/export.rs must never name `{forbidden}`; it may read only \
             the memory store"
        );
    }

    let tmp = tempfile::tempdir().expect("tempdir");
    let fixture = Fixture::new(tmp.path(), "workspace");
    let planted = fixture.record(
        MemoryKind::Decision,
        "rotate the deploy key",
        "the deploy key is sk-abcdefghijklmnopqrstuvwxyz0123 for openai; rotate it monthly",
    );

    let exported = fixture.glasshouse(&["memory", "export", "--tracked"]);
    assert!(exported.status.success(), "{}", stderr(&exported));

    let files = fixture.exported_files();
    let text = String::from_utf8_lossy(&files[&format!("decision-{planted}.md")]).into_owned();
    assert!(
        !text.contains("sk-abcdefghijklmnopqrstuvwxyz0123"),
        "a secret-shaped token must be redacted out of the exported file:\n{text}"
    );
    assert!(
        text.contains("[REDACTED]"),
        "the redaction must be visible, not silently dropped:\n{text}"
    );
    assert!(
        text.contains("rotate it monthly"),
        "surrounding prose must survive redaction:\n{text}"
    );
}

/// Map line 1815: tracked knowledge says, on its face, that it is a
/// projection rather than the canonical store.
#[test]
fn the_export_says_it_is_a_projection_of_the_canonical_store() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let fixture = Fixture::new(tmp.path(), "workspace");
    let decision = fixture.record(
        MemoryKind::Decision,
        "logs are newline-delimited JSON",
        "a structured log line is greppable and parseable at the same time",
    );

    let exported = fixture.glasshouse(&["memory", "export", "--tracked"]);
    assert!(exported.status.success(), "{}", stderr(&exported));

    let files = fixture.exported_files();
    let readme = String::from_utf8_lossy(&files["README.md"]).into_owned();
    assert!(
        readme.to_lowercase().contains("projection"),
        "the README must call this a projection, not the source of truth:\n{readme}"
    );

    let record_text =
        String::from_utf8_lossy(&files[&format!("decision-{decision}.md")]).into_owned();
    assert!(
        record_text.contains("projection of glasshouse project memory"),
        "every exported file must carry the projection header:\n{record_text}"
    );
    assert!(
        record_text.contains("canonical store:"),
        "every exported file must name the canonical store:\n{record_text}"
    );
}
