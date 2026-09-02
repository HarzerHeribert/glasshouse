//! `memory_files` — which files were being worked on when a memory was
//! learned (migration 17).
//!
//! # This file proves a producer, and deliberately closes no capability box
//!
//! Phase 28's five lines (1139–1143) all rest on one absence: nothing
//! associated a memory with a file. This package builds the association and
//! claims none of the lines, because **1139 asks for the files a memory
//! *"explicitly references"*** and what this build can honestly produce is
//! *"the files that were dirty when it was learned"*. On the automatic
//! extraction path the model's input contains no prose at all
//! (`memory::extract::lifecycle`'s own doc; `lifecycle_events` has no text
//! column), so a model asked to name files there would fabricate from an empty
//! input — map line 1294's rule, where a fabricated value inverts the policy
//! rather than degrading it. So every row says `observed`, and the test that
//! keeps it that way is
//! `database::tests::every_file_association_the_type_supports_is_one_the_schema_records`.
//!
//! # The evidence goes through the shipped binary
//!
//! Practice §35: a caller every test bypasses is not a caller. The claim here
//! is that *extraction as a user runs it* now leaves a durable, specific
//! association — so `glasshouse memory extract` is run as a **process**, in a
//! project whose git index this file controls, and the rows are read back out
//! of the database afterwards. Asserting that
//! `MemoryStore::record_observed_files` inserts a row would prove nothing
//! about that.
//!
//! # What is *not* here, and why
//!
//! There is no read door. `MemoryStore::for_path` was this package's fourth
//! item and was **stopped**: it must reuse `memory::search`'s `group()` so a
//! path-scoped retrieval cannot rank differently from the other two doors, and
//! `group` is private to `search.rs`, which this package may not edit. See
//! `.agent-runtime/report-memory-file-observer.md`. Until that ruling lands,
//! these tests read the table directly — which is the right shape for proving
//! a *producer* and the wrong shape for proving a retrieval.
//!
//! The hook path's call site (`main.rs::run_extraction`) shares
//! `record_observed_files` with the command exercised here but is not itself
//! covered: driving it needs a configured extraction provider, which the
//! default configuration deliberately does not have.

use std::path::{Path, PathBuf};
use std::process::Command;

use clap::Parser;
use rusqlite::Connection;

use glasshouse::memory::{FileAssociation, normalize_observed_path};
use glasshouse::{Cli, Runtime, bootstrap};

// -------------------------------------------------------------------------
// Fixtures
// -------------------------------------------------------------------------

/// A bootstrapped project inside `base`, sharing `base`'s data and config
/// roots — the shape `tests/evaluation_observations.rs` uses, so two fixtures
/// over one `base` are two real projects on one machine, each with its own
/// canonicalised root and its own `glasshouse.db`.
struct Fixture {
    base: PathBuf,
    runtime: Runtime,
}

impl Fixture {
    fn new(base: &Path, name: &str) -> Self {
        let root: PathBuf = base.join("workspace").join(name);
        std::fs::create_dir_all(root.join(".git")).unwrap();
        let root = std::fs::canonicalize(&root).unwrap();

        let cli = Cli::try_parse_from([
            "glasshouse",
            "--data-dir",
            base.join("data").to_str().unwrap(),
            "--config-dir",
            base.join("config").to_str().unwrap(),
        ])
        .unwrap();
        let runtime = bootstrap(&cli, &root).unwrap();
        Fixture {
            base: base.to_path_buf(),
            runtime,
        }
    }

    fn root(&self) -> &Path {
        self.runtime.project().root()
    }

    fn db(&self) -> Connection {
        Connection::open(self.runtime.database_path()).unwrap()
    }

    /// Run the shipped binary in this project, exactly as a user would.
    fn run(&self, args: &[&str]) -> std::process::Output {
        Command::new(env!("CARGO_BIN_EXE_glasshouse"))
            .current_dir(self.root())
            .arg("--data-dir")
            .arg(self.base.join("data"))
            .arg("--config-dir")
            .arg(self.base.join("config"))
            .args(args)
            .output()
            .expect("the glasshouse binary must run")
    }

    /// Make `paths` the working tree's changed set, by writing a git index
    /// that tracks them and leaving them absent from disk.
    ///
    /// A **tracked file that is gone** is exactly what
    /// `checkpoint::git::entry_changed` reports as changed, and it is the one
    /// way of being dirty that does not depend on filesystem mtime resolution
    /// — so this is deterministic on every platform rather than
    /// occasionally flaky on one.
    fn dirty(&self, paths: &[&str]) {
        let entries: Vec<(&str, u32, u32, u32, u32)> = paths
            .iter()
            .map(|path| (*path, 0u32, 0u32, 1u32, 0o100644u32))
            .collect();
        std::fs::write(self.root().join(".git/index"), write_index_v2(&entries)).unwrap();
    }

    /// Make the working tree clean: one tracked file that exists and matches
    /// what the index recorded about it.
    ///
    /// The index is built from the file's **real** metadata rather than from
    /// numbers chosen here, because mtime resolution varies by platform —
    /// `checkpoint::git`'s own unit test makes the same argument.
    fn clean(&self, path: &str) {
        let full = self.root().join(path);
        std::fs::create_dir_all(full.parent().unwrap()).unwrap();
        std::fs::write(&full, "tracked and unchanged\n").unwrap();

        let metadata = std::fs::metadata(&full).unwrap();
        let since_epoch = metadata
            .modified()
            .unwrap()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap();
        let entries = [(
            path,
            since_epoch.as_secs() as u32,
            since_epoch.subsec_nanos(),
            metadata.len() as u32,
            0o100644u32,
        )];
        std::fs::write(self.root().join(".git/index"), write_index_v2(&entries)).unwrap();
    }

    /// Run `glasshouse memory extract` over one line of activity and one
    /// canned model reply, and return the identifiers of what it stored.
    ///
    /// This is the production command: everything except the model call is
    /// the real path — the chunk is bounded and scrubbed, the reply goes
    /// through contract validation, the credential screen, conservative
    /// classification and the duplicate check, and what survives is written
    /// to the project's real store.
    fn extract(&self, session: &str, body: &str) -> Vec<String> {
        let dir = self.base.join("extraction").join(session);
        std::fs::create_dir_all(&dir).unwrap();
        let activity = dir.join("activity.txt");
        let reply = dir.join("reply.json");
        std::fs::write(&activity, format!("the session did some work: {body}\n")).unwrap();
        std::fs::write(
            &reply,
            format!(
                r#"{{"memories":[{{"kind":"finding","authority":"constraint",
                    "disposition":"accepted","support":"established",
                    "confidence":"certain","body":"{body}"}}]}}"#
            ),
        )
        .unwrap();

        let output = self.run(&[
            "memory",
            "extract",
            "--session",
            session,
            "--activity",
            activity.to_str().unwrap(),
            "--reply-from",
            reply.to_str().unwrap(),
        ]);
        assert!(
            output.status.success(),
            "`glasshouse memory extract` failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );

        let conn = self.db();
        let mut statement = conn
            .prepare("SELECT id FROM memories WHERE body = ?1")
            .unwrap();
        let ids: Vec<String> = statement
            .query_map([body], |row| row.get::<_, String>(0))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        assert_eq!(
            ids.len(),
            1,
            "the fixture must have stored exactly one memory for `{body}`"
        );
        ids
    }

    /// Which memories this project associates with `path`, straight out of
    /// `memory_files`.
    ///
    /// **This is not a read door and must not become one.** It exists because
    /// `MemoryStore::for_path` was stopped — see this file's header — and a
    /// producer still has to be provable. Everything a real retrieval owes
    /// (project scoping in the query, the ladder, `group()`) is absent here on
    /// purpose.
    fn associated_with(&self, path: &str) -> Vec<String> {
        let conn = self.db();
        let mut statement = conn
            .prepare("SELECT memory_id FROM memory_files WHERE path = ?1 ORDER BY memory_id")
            .unwrap();
        statement
            .query_map([path], |row| row.get::<_, String>(0))
            .unwrap()
            .map(Result::unwrap)
            .collect()
    }

    fn association_rows(&self) -> i64 {
        self.db()
            .query_row("SELECT COUNT(*) FROM memory_files", [], |row| row.get(0))
            .unwrap()
    }
}

/// A git index file, version 2, holding exactly `entries`.
///
/// A transcription of `checkpoint::git`'s own test helper, which lives inside
/// that module's `#[cfg(test)]` and so cannot be reached from an integration
/// test. Each entry is `(path, mtime_secs, mtime_nanos, size, mode)`; every
/// other stat field the real format carries is zero, because `parse_index`
/// never reads them.
fn write_index_v2(entries: &[(&str, u32, u32, u32, u32)]) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(b"DIRC");
    out.extend_from_slice(&2u32.to_be_bytes());
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

// -------------------------------------------------------------------------
// 1 — the association is durable, and it varies with what was being worked on
// -------------------------------------------------------------------------

/// **The characteristic test of this package.** Two extractions, run through
/// the shipped binary, against **disjoint** working-tree changed sets. The
/// associations they leave behind must be disjoint too.
///
/// This fails on `main` because `memory_files` does not exist there. It fails
/// after any change that makes the writer store something other than *this
/// session's* changed subset — a producer that associated every memory with
/// every file would be worse than none, because it would surface an unrelated
/// memory as authoritative for a file somebody is about to edit.
///
/// # Why the assertions are two-sided
///
/// Each path's set is asserted to contain exactly the memory learned while it
/// was dirty **and** not to contain the other one. An "is disjoint" check
/// alone passes when both sets are empty, which is precisely what a silently
/// broken writer produces. The row count is asserted last for the same reason:
/// two memories and one dirty path each is two rows, and a writer that stored
/// the union would leave four.
#[test]
fn two_sessions_with_disjoint_dirty_sets_get_disjoint_file_associations() {
    let tmp = tempfile::tempdir().unwrap();
    let project = Fixture::new(tmp.path(), "alpha");

    project.dirty(&["src/one.rs"]);
    let first = project.extract("session-one", "the first thing this project learned");

    project.dirty(&["src/two.rs"]);
    let second = project.extract("session-two", "the second thing this project learned");

    assert_ne!(first, second, "the fixture must have stored two memories");

    let for_one = project.associated_with("src/one.rs");
    let for_two = project.associated_with("src/two.rs");

    assert_eq!(
        for_one, first,
        "src/one.rs was dirty for the first extraction and only that one"
    );
    assert_eq!(
        for_two, second,
        "src/two.rs was dirty for the second extraction and only that one"
    );
    assert!(
        !for_one.iter().any(|id| second.contains(id)),
        "a memory learned while src/two.rs was dirty must not be associated with src/one.rs"
    );
    assert!(
        !for_two.iter().any(|id| first.contains(id)),
        "a memory learned while src/one.rs was dirty must not be associated with src/two.rs"
    );

    assert_eq!(
        project.association_rows(),
        2,
        "two memories, one dirty path each: a writer storing the union of both \
         sessions' paths would leave four rows here"
    );
}

/// A hand-fed extraction writes only `observed` rows.
///
/// `referenced` exists since `GH-FILE-AWARE-MEMORY` (capability-map line
/// 1139): the extraction model may name paths, kept only when byte-equal to
/// the session's own `file_touched` set. A chunk built by hand has no such
/// set, so the observer is the only writer here — and *observed-dirty* is
/// still not *explicitly referenced*, which is why the two words stay apart.
#[test]
fn a_hand_fed_extraction_writes_only_observed_associations() {
    let tmp = tempfile::tempdir().unwrap();
    let project = Fixture::new(tmp.path(), "alpha");

    project.dirty(&["src/one.rs", "docs/notes.md"]);
    project.extract("session-one", "something learned mid-edit");

    let conn = project.db();
    let mut statement = conn
        .prepare("SELECT DISTINCT provenance FROM memory_files")
        .unwrap();
    let provenances: Vec<String> = statement
        .query_map([], |row| row.get::<_, String>(0))
        .unwrap()
        .map(Result::unwrap)
        .collect();

    assert_eq!(
        provenances,
        vec![FileAssociation::Observed.as_str().to_owned()],
        "a hand-fed chunk has no touched set, so only the observer writes"
    );
    assert_eq!(
        FileAssociation::from_stored("referenced"),
        Some(FileAssociation::Referenced),
        "`referenced` is line 1139's word and, since its producer landed, a stored value"
    );

    // Non-vacuity: the query above found rows at all, and both dirty paths
    // are associated with the one memory that was learned.
    assert_eq!(project.association_rows(), 2);
}

// -------------------------------------------------------------------------
// 2 — a clean tree is an absence, never an empty-string path
// -------------------------------------------------------------------------

/// A memory extracted against a clean working tree gets **no rows**.
///
/// The non-vacuity is the memory: extraction really ran and really stored
/// something, so an empty `memory_files` is the writer declining rather than
/// the fixture failing to extract.
#[test]
fn a_memory_extracted_against_a_clean_tree_records_no_files() {
    let tmp = tempfile::tempdir().unwrap();
    let project = Fixture::new(tmp.path(), "alpha");

    project.clean("src/tracked.rs");
    let stored = project.extract("session-one", "learned with nothing in flight");
    assert_eq!(stored.len(), 1, "extraction must have stored a memory");

    assert_eq!(
        project.association_rows(),
        0,
        "a clean tree names no files, and an absence is never an empty-string path"
    );

    let empty: i64 = project
        .db()
        .query_row(
            "SELECT COUNT(*) FROM memory_files WHERE path = ''",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(empty, 0);
}

/// A project with no readable git index at all records nothing either, and
/// does not fail the command.
///
/// `WorkingTreeStatus::detect` answers `None` for every way of being unable to
/// tell — no repository, no index, an unrecognised format — and a producer
/// that cannot see must write nothing rather than guess.
#[test]
fn a_project_with_no_index_records_no_files_and_still_extracts() {
    let tmp = tempfile::tempdir().unwrap();
    let project = Fixture::new(tmp.path(), "alpha");

    // `.git` exists (the fixture makes it) but there is no index in it.
    let stored = project.extract("session-one", "learned in a repository with no index");
    assert_eq!(stored.len(), 1);
    assert_eq!(project.association_rows(), 0);
}

// -------------------------------------------------------------------------
// 3 — project isolation, proved by the trigger and by two real projects
// -------------------------------------------------------------------------

/// Migration 17's project-scope trigger aborts an association belonging to
/// another project, and nothing is left behind.
///
/// Modeled on `database::tests::migration_eleven_rejects_a_routing_observation_
/// from_a_foreign_project`: the guard is structural rather than a convention
/// a future query could forget, and `IS NOT` rather than `<>` means a missing
/// binding row aborts the write instead of the comparison evaluating to NULL
/// and letting it through.
#[test]
fn an_association_from_another_project_is_refused_by_the_schema() {
    let tmp = tempfile::tempdir().unwrap();
    let alpha = Fixture::new(tmp.path(), "alpha");
    let beta = Fixture::new(tmp.path(), "beta");

    let alpha_id = alpha.runtime.project().id().as_str().to_owned();
    let beta_id = beta.runtime.project().id().as_str().to_owned();
    assert_ne!(alpha_id, beta_id);

    let conn = beta.db();
    let refused = conn.execute(
        "INSERT INTO memory_files (project_id, memory_id, path, provenance, observed_at) \
         VALUES (?1, 'planted-memory', 'src/secret.rs', 'observed', 0)",
        [&alpha_id],
    );
    // Matched rather than `unwrap_err`'d, so that a trigger which silently
    // stopped firing fails on this test's own claim instead of on a panic
    // whose message names nothing (practice §80, case 5).
    let message = match refused {
        Ok(_) => {
            panic!("the project-scope trigger must refuse an association bound to another project")
        }
        Err(err) => format!("{err}"),
    };
    assert!(
        message.contains("belongs to a different project"),
        "the trigger must name what it refused: {message}"
    );
    assert_eq!(
        beta.association_rows(),
        0,
        "a refused write must leave no row behind"
    );

    // And the same insert against its own project is accepted, so the test
    // above is refusing the project and not the statement.
    conn.execute(
        "INSERT INTO memory_files (project_id, memory_id, path, provenance, observed_at) \
         VALUES (?1, 'beta-memory', 'src/secret.rs', 'observed', 0)",
        [&beta_id],
    )
    .unwrap();
    assert_eq!(beta.association_rows(), 1);
}

/// Two real projects, the same path dirty in both, and neither project's
/// database ever names the other's memory.
///
/// The path is deliberately identical — `src/shared.rs` — because a
/// repo-relative path is not unique across projects and the index this table
/// carries is on `path` alone. What keeps them apart is that each project has
/// its own database file and its own binding, and this asserts that rather
/// than assuming it.
#[test]
fn two_projects_with_the_same_path_dirty_never_see_each_others_memories() {
    let tmp = tempfile::tempdir().unwrap();
    let alpha = Fixture::new(tmp.path(), "alpha");
    let beta = Fixture::new(tmp.path(), "beta");

    alpha.dirty(&["src/shared.rs"]);
    beta.dirty(&["src/shared.rs"]);

    let alpha_memories = alpha.extract("session-alpha", "alpha's own finding");
    let beta_memories = beta.extract("session-beta", "beta's own finding");

    assert_eq!(alpha.associated_with("src/shared.rs"), alpha_memories);
    assert_eq!(beta.associated_with("src/shared.rs"), beta_memories);
    assert_eq!(alpha.association_rows(), 1);
    assert_eq!(beta.association_rows(), 1);

    // Every row in each file is bound to that file's own project.
    for fixture in [&alpha, &beta] {
        let expected = fixture.runtime.project().id().as_str().to_owned();
        let conn = fixture.db();
        let mut statement = conn
            .prepare("SELECT DISTINCT project_id FROM memory_files")
            .unwrap();
        let bindings: Vec<String> = statement
            .query_map([], |row| row.get::<_, String>(0))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        assert_eq!(bindings, vec![expected]);
    }
}

// -------------------------------------------------------------------------
// The column's contract — a spelling that is not the canonical one is a row
// the index will silently miss
// -------------------------------------------------------------------------

/// `memory_files.path` is repo-relative, `/`-separated, UTF-8 and never
/// absolute, and `normalize_observed_path` is where that is enforced.
///
/// Two spellings of one file become two rows, and the exact-match index then
/// misses one of them — **a missed association is invisible, where a wrong one
/// is at least wrong out loud**. So anything that cannot be brought to the
/// canonical spelling with certainty is refused rather than guessed at.
#[test]
fn only_a_repo_relative_slash_separated_path_reaches_the_column() {
    // Already canonical — what git's index gives on every platform, Windows
    // included, and the only shape this build's own producer ever emits.
    assert_eq!(
        normalize_observed_path("src/memory/store.rs").as_deref(),
        Some("src/memory/store.rs")
    );

    // Brought to the canonical spelling: separators, a leading `./`, doubled
    // and trailing separators, and surrounding whitespace.
    assert_eq!(
        normalize_observed_path(r"src\memory\store.rs").as_deref(),
        Some("src/memory/store.rs")
    );
    assert_eq!(
        normalize_observed_path("./src/memory/store.rs").as_deref(),
        Some("src/memory/store.rs")
    );
    assert_eq!(
        normalize_observed_path("src//memory/./store.rs").as_deref(),
        Some("src/memory/store.rs")
    );
    assert_eq!(
        normalize_observed_path("  src/memory/store.rs  ").as_deref(),
        Some("src/memory/store.rs")
    );

    // Refused. An absolute path is not repo-relative, and the project root is
    // exactly where the `/var` versus `/private/var` ambiguity lives — which
    // is the reason this column stores no root at all.
    assert_eq!(
        normalize_observed_path("/home/someone/project/src/a.rs"),
        None
    );
    assert_eq!(normalize_observed_path(r"C:\src\a.rs"), None);
    assert_eq!(normalize_observed_path("C:/src/a.rs"), None);
    assert_eq!(normalize_observed_path(r"\\?\C:\src\a.rs"), None);

    // Refused: a `..` can leave the repository and nothing here can tell
    // whether it did.
    assert_eq!(normalize_observed_path("../other/src/a.rs"), None);
    assert_eq!(normalize_observed_path("src/../../a.rs"), None);

    // Refused: nothing at all is not a path.
    assert_eq!(normalize_observed_path(""), None);
    assert_eq!(normalize_observed_path("   "), None);
    assert_eq!(normalize_observed_path("."), None);
    assert_eq!(normalize_observed_path("./"), None);
}

/// The contract reaches the column, not only the function: a path the
/// normaliser refuses leaves no row, and the ones it accepts are stored in
/// their canonical spelling.
///
/// Driven through `MemoryStore` rather than the binary because the binary's
/// producer cannot emit a non-canonical path — git's index has none — and a
/// contract nothing exercises is a contract the next producer will discover
/// the hard way.
#[test]
fn a_refused_path_leaves_no_row_and_an_accepted_one_is_stored_canonically() {
    use glasshouse::memory::{MemoryKind, NewMemory, ProjectMemory};

    let tmp = tempfile::tempdir().unwrap();
    let project = Fixture::new(tmp.path(), "alpha");

    let memory = ProjectMemory::open(&project.runtime).unwrap();
    let store = memory.store();
    let recorded = store
        .record(NewMemory::new(MemoryKind::Finding, "a memory to associate"))
        .unwrap();

    let written = store
        .record_observed_files(
            std::slice::from_ref(&recorded.id),
            &[
                r"src\memory\store.rs".to_owned(),
                "/etc/passwd".to_owned(),
                "../outside.rs".to_owned(),
                "".to_owned(),
            ],
        )
        .unwrap();
    assert_eq!(
        written, 1,
        "exactly one of those four is a repo-relative path"
    );

    let conn = project.db();
    let mut statement = conn.prepare("SELECT path FROM memory_files").unwrap();
    let paths: Vec<String> = statement
        .query_map([], |row| row.get::<_, String>(0))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    assert_eq!(paths, vec!["src/memory/store.rs".to_owned()]);
}

/// A writer with nothing to say writes nothing: no memories, or no paths, is
/// zero rows and no error.
#[test]
fn an_empty_side_of_the_association_writes_nothing() {
    use glasshouse::memory::{MemoryKind, NewMemory, ProjectMemory};

    let tmp = tempfile::tempdir().unwrap();
    let project = Fixture::new(tmp.path(), "alpha");

    let memory = ProjectMemory::open(&project.runtime).unwrap();
    let store = memory.store();
    let recorded = store
        .record(NewMemory::new(MemoryKind::Finding, "a memory to associate"))
        .unwrap();

    assert_eq!(store.record_observed_files(&[], &[]).unwrap(), 0);
    assert_eq!(
        store
            .record_observed_files(&[], &["src/a.rs".to_owned()])
            .unwrap(),
        0
    );
    assert_eq!(
        store
            .record_observed_files(std::slice::from_ref(&recorded.id), &[])
            .unwrap(),
        0
    );
    assert_eq!(project.association_rows(), 0);
}
