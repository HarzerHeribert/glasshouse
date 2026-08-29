//! Portable session checkpoints, exercised the way a caller reaches them:
//! through a real `Runtime` and a real project database on disk.
//!
//! Behavioral contract: given a session with work in progress, when a
//! checkpoint is taken, it holds the objective, state, decisions, failed
//! approaches, files, test state, next actions and Git position; it is small
//! enough to bootstrap a fresh session cheaply; it is stored apart from
//! durable project memory; and a checkpoint written while one harness was
//! running can start work in a different one.

use std::path::Path;
use std::time::{Duration, Instant};

use clap::Parser;

use glasshouse::checkpoint::{Checkpoint, CheckpointReason, GitPosition, Handoff};
use glasshouse::events::MessageOrigin;
use glasshouse::launch::HarnessLaunch;
use glasshouse::memory::search::SearchScope;
use glasshouse::memory::{MemoryKind, NewMemory, ProjectMemory};
use glasshouse::platform::exec;
use glasshouse::session::{
    LiveSession, NewSession, SessionId, SessionLifecycle, SessionPresentation, SessionRuntime,
};
use glasshouse::{Cli, Runtime};

/// The same bootstrapped-project idiom `tests/memory_store.rs` and
/// `src/checkpoint/store.rs`'s own tests use.
struct Fixture {
    runtime: Runtime,
}

impl Fixture {
    fn new(base: &Path, name: &str) -> Self {
        let root = base.join("workspace").join(name);
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
        let runtime = glasshouse::bootstrap(&cli, &root).unwrap();
        Fixture { runtime }
    }

    fn checkpoints(&self) -> rusqlite::Connection {
        rusqlite::Connection::open(self.runtime.database_path()).unwrap()
    }
}

fn tempdir() -> tempfile::TempDir {
    tempfile::tempdir().unwrap()
}

/// A checkpoint with every field populated, for the round-trip tests.
fn full_checkpoint(session: &str, harness: &str, at: i64) -> Checkpoint {
    Checkpoint {
        session: SessionId::new(session),
        harness: harness.to_owned(),
        reason: CheckpointReason::Manual,
        created_at: at,
        git: Some(GitPosition {
            branch: Some("main".to_owned()),
            commit: "0123456789abcdef0123456789abcdef01234567".to_owned(),
        }),
        working_tree: None,
        handoff: Handoff {
            objective: "close out the sub-record test packet".to_owned(),
            implementation_state: "the event log tests pass; the checkpoint tests are next"
                .to_owned(),
            decisions: vec!["use the sibling idiom for project isolation".to_owned()],
            memory: vec!["constraint: checkpoints and memory never share a table".to_owned()],
            failed_approaches: vec!["tried mocking the database, discarded it".to_owned()],
            files: vec!["tests/events_log.rs".to_owned()],
            test_state: Some("6 of 6 events_log tests passing".to_owned()),
            next_actions: vec![
                "write checkpoint_portability.rs".to_owned(),
                "run the verification commands".to_owned(),
            ],
        },
        trimmed: false,
    }
}

/// 1. Round trip through a real database: save a checkpoint with every field
///    populated, reopen the project, read it back, and assert equality field
///    by field — not just on the whole value — so a lost field names itself.
#[test]
fn every_field_survives_a_round_trip_through_a_real_database() {
    let tmp = tempdir();
    let fixture = Fixture::new(tmp.path(), "alpha");

    let original = full_checkpoint("session-a", "a-harness", 1_700_000_000);
    let project_checkpoints =
        glasshouse::checkpoint::ProjectCheckpoints::open(&fixture.runtime).unwrap();
    let stored = project_checkpoints.store().save(original.clone()).unwrap();

    // Reopen the project, standing in for a fresh process.
    let reopened = Fixture::new(tmp.path(), "alpha").runtime;
    let reopened_checkpoints = glasshouse::checkpoint::ProjectCheckpoints::open(&reopened).unwrap();
    let store = reopened_checkpoints.store();
    let read_back = store
        .get(&stored.id)
        .unwrap()
        .expect("still there")
        .checkpoint;

    assert_eq!(read_back.session, original.session, "session");
    assert_eq!(read_back.harness, original.harness, "harness");
    assert_eq!(read_back.reason, original.reason, "reason");
    assert_eq!(read_back.created_at, original.created_at, "created_at");
    assert_eq!(read_back.git, original.git, "git");
    assert_eq!(
        read_back.handoff.objective, original.handoff.objective,
        "objective"
    );
    assert_eq!(
        read_back.handoff.implementation_state, original.handoff.implementation_state,
        "implementation_state"
    );
    assert_eq!(
        read_back.handoff.decisions, original.handoff.decisions,
        "decisions"
    );
    assert_eq!(read_back.handoff.memory, original.handoff.memory, "memory");
    assert_eq!(
        read_back.handoff.failed_approaches, original.handoff.failed_approaches,
        "failed_approaches"
    );
    assert_eq!(read_back.handoff.files, original.handoff.files, "files");
    assert_eq!(
        read_back.handoff.test_state, original.handoff.test_state,
        "test_state"
    );
    assert_eq!(
        read_back.handoff.next_actions, original.handoff.next_actions,
        "next_actions"
    );
    assert_eq!(read_back.trimmed, original.trimmed, "trimmed");
}

/// 2. The size bound, from the outside: an enormous handoff is trimmed to
///    `MAX_BYTES` and says so, while a minimal checkpoint stays genuinely
///    small — "bounded" and "small" are different claims.
#[test]
fn the_size_bound_holds_from_the_outside_and_a_minimal_checkpoint_is_genuinely_small() {
    let tmp = tempdir();
    let fixture = Fixture::new(tmp.path(), "alpha");
    let project_checkpoints =
        glasshouse::checkpoint::ProjectCheckpoints::open(&fixture.runtime).unwrap();
    let store = project_checkpoints.store();

    // Thousands of items and tens of thousands of characters, chosen small
    // enough per item that `fit`'s repeated re-render (it is not written to
    // be asymptotically efficient, and does not need to be for a real
    // handoff) still completes quickly in a test.
    let mut huge = full_checkpoint("session-a", "a-harness", 1_700_000_000);
    huge.handoff.failed_approaches = (0..800).map(|i| format!("failed approach {i}")).collect();
    huge.handoff.files = (0..800).map(|i| format!("some/file/{i}.rs")).collect();
    huge.handoff.decisions = (0..400).map(|i| format!("decision {i}")).collect();
    huge.handoff.next_actions = (0..400).map(|i| format!("next action {i}")).collect();

    let stored = store.save(huge).unwrap();
    let document = stored.checkpoint.render();
    assert!(
        document.len() <= glasshouse::checkpoint::MAX_BYTES,
        "a trimmed checkpoint is still {} bytes",
        document.len()
    );
    assert!(stored.checkpoint.trimmed, "trimming must be reported");
    assert!(!stored.checkpoint.handoff.objective.is_empty());
    assert!(!stored.checkpoint.handoff.implementation_state.is_empty());

    let minimal = Checkpoint {
        session: SessionId::new("session-b"),
        harness: "a-harness".to_owned(),
        reason: CheckpointReason::TaskBoundary,
        created_at: 1,
        git: None,
        working_tree: None,
        handoff: Handoff {
            objective: "o".to_owned(),
            implementation_state: "s".to_owned(),
            ..Handoff::default()
        },
        trimmed: false,
    };
    let stored_minimal = store.save(minimal).unwrap();
    assert!(
        stored_minimal.checkpoint.render().len() < 1024,
        "a minimal checkpoint must be well under 1 KiB, was {} bytes",
        stored_minimal.checkpoint.render().len()
    );
    assert!(!stored_minimal.checkpoint.trimmed);
}

/// 3. Stored separately from durable project memory: a checkpoint and a
///    memory record in the same project never leak into each other's
///    surface. Checked through the public API, and again at the SQL level —
///    the two tables are different tables, which is the structural half of
///    the claim.
#[test]
fn checkpoints_and_memory_are_stored_and_retrieved_separately() {
    let tmp = tempdir();
    let fixture = Fixture::new(tmp.path(), "alpha");

    let project_checkpoints =
        glasshouse::checkpoint::ProjectCheckpoints::open(&fixture.runtime).unwrap();
    let checkpoint_store = project_checkpoints.store();
    let mut checkpoint = full_checkpoint("session-a", "a-harness", 1_700_000_000);
    checkpoint.handoff.objective =
        "a checkpoint objective mentioning a distinctive token GLASSHOUSE_CHECKPOINT_MARKER"
            .to_owned();
    checkpoint_store.save(checkpoint).unwrap();

    let memory = ProjectMemory::open(&fixture.runtime).unwrap();
    let memory_store = memory.store();
    memory_store
        .record(NewMemory::new(
            MemoryKind::Decision,
            "a durable memory mentioning a distinctive token GLASSHOUSE_MEMORY_MARKER",
        ))
        .unwrap();

    // Public API: a memory search never returns checkpoint content.
    let memory_hits = memory_store
        .search("GLASSHOUSE_CHECKPOINT_MARKER", SearchScope::Historical, 50)
        .unwrap();
    assert!(
        memory_hits.is_empty(),
        "a memory search returned checkpoint content: {memory_hits:?}"
    );
    let memory_hits = memory_store
        .search("GLASSHOUSE_MEMORY_MARKER", SearchScope::Historical, 50)
        .unwrap();
    assert_eq!(
        memory_hits.len(),
        1,
        "the actual memory must still be found"
    );

    // Public API: the checkpoint list never returns memory content.
    let all_checkpoints = checkpoint_store.list().unwrap();
    assert_eq!(all_checkpoints.len(), 1);
    for stored in &all_checkpoints {
        let rendered = stored.checkpoint.render();
        assert!(
            !rendered.contains("GLASSHOUSE_MEMORY_MARKER"),
            "a checkpoint's document contained memory content"
        );
        assert!(rendered.contains("GLASSHOUSE_CHECKPOINT_MARKER"));
    }

    // SQL level: two different tables, and neither's rows appear in the
    // other's.
    let conn = fixture.checkpoints();
    let checkpoint_rows: i64 = conn
        .query_row("SELECT COUNT(*) FROM checkpoints", [], |row| row.get(0))
        .unwrap();
    let memory_rows: i64 = conn
        .query_row("SELECT COUNT(*) FROM memories", [], |row| row.get(0))
        .unwrap();
    assert_eq!(checkpoint_rows, 1);
    assert_eq!(memory_rows, 1);
    let cross_hit: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM checkpoints \
             WHERE document LIKE '%GLASSHOUSE_MEMORY_MARKER%'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(cross_hit, 0);
}

/// 4. **The one that matters — cross-harness bootstrap.** Record a session
///    under one harness slug, save a checkpoint for it, and, reading only the
///    stored checkpoint, produce the bootstrap prompt: it carries the
///    objective, the state, every failed approach and the next actions in
///    order; it names no harness at all; it is plain text, not JSON; and two
///    checkpoints differing only in `harness` produce byte-identical prompts.
#[test]
fn a_checkpoint_bootstraps_the_same_prompt_regardless_of_which_harness_wrote_it() {
    let tmp = tempdir();
    let fixture = Fixture::new(tmp.path(), "alpha");

    let project_sessions = glasshouse::session::ProjectSessions::open(&fixture.runtime).unwrap();
    let sessions = project_sessions.store();
    let recorded = sessions
        .create(NewSession::embedded("claude-code"))
        .unwrap();

    let project_checkpoints =
        glasshouse::checkpoint::ProjectCheckpoints::open(&fixture.runtime).unwrap();
    let checkpoints = project_checkpoints.store();
    let mut checkpoint = full_checkpoint(recorded.id.as_str(), "claude-code", 1_700_000_000);
    checkpoint.handoff.failed_approaches = vec![
        "tried A first".to_owned(),
        "then tried B".to_owned(),
        "then C, which finally worked".to_owned(),
    ];
    checkpoint.handoff.next_actions = vec![
        "first do this".to_owned(),
        "then do that".to_owned(),
        "then ship it".to_owned(),
    ];
    let stored = checkpoints.save(checkpoint).unwrap();

    // Read back through the stored form only, not the value built above.
    let read_back = checkpoints.get(&stored.id).unwrap().unwrap().checkpoint;
    let prompt = read_back.bootstrap_prompt();

    assert!(prompt.contains(&read_back.handoff.objective));
    assert!(prompt.contains(&read_back.handoff.implementation_state));

    let approach_positions: Vec<usize> = read_back
        .handoff
        .failed_approaches
        .iter()
        .map(|approach| prompt.find(approach).expect("approach missing from prompt"))
        .collect();
    assert!(
        approach_positions.windows(2).all(|pair| pair[0] < pair[1]),
        "failed approaches must appear in order: {prompt}"
    );

    let action_positions: Vec<usize> = read_back
        .handoff
        .next_actions
        .iter()
        .map(|action| {
            prompt
                .find(action)
                .expect("next action missing from prompt")
        })
        .collect();
    assert!(
        action_positions.windows(2).all(|pair| pair[0] < pair[1]),
        "next actions must appear in order: {prompt}"
    );

    let lowered = prompt.to_ascii_lowercase();
    for harness in [
        "claude",
        "codex",
        "antigravity",
        "opencode",
        "cursor",
        "gemini",
    ] {
        assert!(
            !lowered.contains(harness),
            "the bootstrap prompt names `{harness}`, so it is not portable"
        );
    }
    assert!(
        !prompt.trim_start().starts_with('{'),
        "the bootstrap prompt must be plain text, not JSON"
    );

    // The assertion that matters most: the same handoff, differing only in
    // which harness wrote it, produces byte-identical prompts.
    let mut under_claude = full_checkpoint("session-x", "claude-code", 1_700_000_000);
    under_claude.handoff = checkpoint_handoff();
    let mut under_codex = full_checkpoint("session-x", "codex", 1_700_000_000);
    under_codex.handoff = checkpoint_handoff();

    assert_ne!(under_claude.harness, under_codex.harness);
    assert_eq!(
        under_claude.bootstrap_prompt(),
        under_codex.bootstrap_prompt(),
        "the same handoff must produce the same prompt no matter which \
         harness the checkpoint was written under"
    );
}

fn checkpoint_handoff() -> Handoff {
    Handoff {
        objective: "identical objective".to_owned(),
        implementation_state: "identical state".to_owned(),
        decisions: vec!["identical decision".to_owned()],
        memory: vec!["identical memory record".to_owned()],
        failed_approaches: vec!["identical failed approach".to_owned()],
        files: vec!["identical/file.rs".to_owned()],
        test_state: Some("identical test state".to_owned()),
        next_actions: vec!["identical next action".to_owned()],
    }
}

/// 5. A checkpoint outlives the session that made it: even once the session
///    is moved to `Failed`, `latest_for` still returns the checkpoint whole.
#[test]
fn a_checkpoint_outlives_the_session_that_made_it() {
    let tmp = tempdir();
    let fixture = Fixture::new(tmp.path(), "alpha");

    let project_sessions = glasshouse::session::ProjectSessions::open(&fixture.runtime).unwrap();
    let sessions = project_sessions.store();
    let recorded = sessions
        .create(NewSession::embedded("claude-code"))
        .unwrap();

    let project_checkpoints =
        glasshouse::checkpoint::ProjectCheckpoints::open(&fixture.runtime).unwrap();
    let checkpoints = project_checkpoints.store();
    let saved = checkpoints
        .save(full_checkpoint(
            recorded.id.as_str(),
            "claude-code",
            1_700_000_000,
        ))
        .unwrap();

    sessions
        .set_lifecycle(&recorded.id, SessionLifecycle::Failed)
        .unwrap();
    let failed = sessions.get(&recorded.id).unwrap().unwrap();
    assert_eq!(failed.lifecycle, SessionLifecycle::Failed);

    let latest = checkpoints
        .latest_for(&recorded.id)
        .unwrap()
        .expect("the checkpoint must still be there");
    assert_eq!(latest, saved);
}

/// 6. Git position, against a real repository: run `GitPosition::detect`
///    against the actual checkout these tests are running in, which is
///    normally a linked git worktree — the case fixtures are worst at.
#[test]
fn git_position_reads_the_real_checkout_this_test_runs_in() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("the manifest is two directories below the checkout root")
        .to_path_buf();

    if !the_repository_is_actually_readable(&root) {
        // A copied tree — see the helper below. Reporting no position is the
        // right answer here, and asserting it is what keeps this from being a
        // silent skip.
        assert_eq!(GitPosition::detect(&root), None);
        return;
    }

    let position = GitPosition::detect(&root)
        .expect("this checkout's HEAD is readable, so a position must be readable");
    assert_eq!(
        position.commit.len(),
        40,
        "expected a SHA-1 object name, got {:?}",
        position.commit
    );
    assert!(
        position.commit.chars().all(|c| c.is_ascii_hexdigit()),
        "commit is not a plausible object name: {}",
        position.commit
    );
    eprintln!(
        "checkpoint_portability: this checkout reads as branch {:?}, commit {}",
        position.branch, position.commit
    );
}

/// **"Include the current Git branch and commit when available."**
///
/// Asserted against `Checkpoint::capture`, which is the function every
/// production caller uses, rather than against `GitPosition::detect` alone.
/// The distinction is not academic: a round-trip test that builds a
/// `Checkpoint` literal with a `git` field already filled in proves the field
/// survives storage and proves nothing whatever about anybody reading a
/// repository — a mutation replacing `capture`'s detection with `None`
/// survived exactly that test.
#[test]
fn capturing_a_checkpoint_reads_the_repository_it_is_standing_in() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("repo");
    let commit = "0123456789abcdef0123456789abcdef01234567";
    std::fs::create_dir_all(root.join(".git/refs/heads")).unwrap();
    std::fs::write(root.join(".git/HEAD"), "ref: refs/heads/a-branch\n").unwrap();
    std::fs::write(root.join(".git/refs/heads/a-branch"), format!("{commit}\n")).unwrap();

    let captured = glasshouse::checkpoint::Checkpoint::capture(
        &glasshouse::session::SessionId::new("s-1"),
        "a-harness",
        glasshouse::checkpoint::CheckpointReason::Manual,
        1_700_000_000,
        &root,
        glasshouse::checkpoint::Handoff {
            objective: "o".to_owned(),
            implementation_state: "s".to_owned(),
            ..Default::default()
        },
    );

    let git = captured
        .git
        .as_ref()
        .expect("a checkpoint taken inside a repository must record where it stands");
    assert_eq!(git.branch.as_deref(), Some("a-branch"));
    assert_eq!(git.commit, commit);

    // And "when available" is a real condition: outside a repository the
    // capture still succeeds and simply records no position.
    let bare = tmp.path().join("not-a-repo");
    std::fs::create_dir_all(&bare).unwrap();
    let outside = glasshouse::checkpoint::Checkpoint::capture(
        &glasshouse::session::SessionId::new("s-1"),
        "a-harness",
        glasshouse::checkpoint::CheckpointReason::Manual,
        1_700_000_000,
        &bare,
        glasshouse::checkpoint::Handoff {
            objective: "o".to_owned(),
            implementation_state: "s".to_owned(),
            ..Default::default()
        },
    );
    assert!(
        outside.git.is_none(),
        "a project that is not a repository must record no position, not a fake one"
    );
}

/// **Line 1640 — "include git status and relevant diff references in the
/// handoff when useful."**
///
/// Asserted against `Checkpoint::capture`, for the same reason the position
/// test above is: a literal built with `working_tree` already filled in
/// would prove storage and nothing about capture actually reading the
/// working tree. This builds a real `.git/index` recording one file, changes
/// that file's size on disk, and checks that `capture` — the one function
/// both `glasshouse checkpoint save` and a task-boundary checkpoint call —
/// notices.
#[test]
fn capturing_a_checkpoint_reads_the_working_tree_status_of_the_repository_it_is_standing_in() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("repo");
    std::fs::create_dir_all(root.join(".git/refs/heads")).unwrap();
    std::fs::write(root.join(".git/HEAD"), "ref: refs/heads/main\n").unwrap();
    std::fs::write(
        root.join(".git/refs/heads/main"),
        "0123456789abcdef0123456789abcdef01234567\n",
    )
    .unwrap();
    std::fs::write(root.join("tracked.txt"), "short\n").unwrap();

    // An index recording a size this file no longer has, once the file below
    // is rewritten longer — a version-2 index with exactly one entry, built
    // the same way `checkpoint::git`'s own tests build one.
    let mut index = Vec::new();
    index.extend_from_slice(b"DIRC");
    index.extend_from_slice(&2u32.to_be_bytes());
    index.extend_from_slice(&1u32.to_be_bytes());
    index.extend_from_slice(&[0u8; 8]); // ctime
    index.extend_from_slice(&[0u8; 8]); // mtime: never matches, forcing a size check
    index.extend_from_slice(&[0u8; 4]); // dev
    index.extend_from_slice(&[0u8; 4]); // ino
    index.extend_from_slice(&0o100644u32.to_be_bytes()); // mode
    index.extend_from_slice(&[0u8; 4]); // uid
    index.extend_from_slice(&[0u8; 4]); // gid
    index.extend_from_slice(&1u32.to_be_bytes()); // size the index remembers: 1 byte
    index.extend_from_slice(&[0u8; 20]); // sha1, unread
    let name = b"tracked.txt";
    index.extend_from_slice(&(name.len() as u16).to_be_bytes());
    index.extend_from_slice(name);
    let entry_len = 62 + name.len();
    let pad = match entry_len % 8 {
        0 => 8,
        rem => 8 - rem,
    };
    index.extend(std::iter::repeat_n(0u8, pad));
    std::fs::write(root.join(".git/index"), &index).unwrap();

    let captured = glasshouse::checkpoint::Checkpoint::capture(
        &glasshouse::session::SessionId::new("s-1"),
        "a-harness",
        glasshouse::checkpoint::CheckpointReason::Manual,
        1_700_000_000,
        &root,
        glasshouse::checkpoint::Handoff {
            objective: "o".to_owned(),
            implementation_state: "s".to_owned(),
            ..Default::default()
        },
    );

    let status = captured
        .working_tree
        .as_ref()
        .expect("a checkpoint taken inside a repository must record working-tree status");
    assert!(
        status.dirty,
        "the index disagrees with the file on disk, so this must read dirty"
    );
    assert_eq!(status.changed_files, vec!["tracked.txt".to_owned()]);

    // And "when available" is a real condition here too: outside a
    // repository, capture still succeeds and simply records no status.
    let bare = tmp.path().join("not-a-repo");
    std::fs::create_dir_all(&bare).unwrap();
    let outside = glasshouse::checkpoint::Checkpoint::capture(
        &glasshouse::session::SessionId::new("s-1"),
        "a-harness",
        glasshouse::checkpoint::CheckpointReason::Manual,
        1_700_000_000,
        &bare,
        glasshouse::checkpoint::Handoff {
            objective: "o".to_owned(),
            implementation_state: "s".to_owned(),
            ..Default::default()
        },
    );
    assert!(
        outside.working_tree.is_none(),
        "a project with no index must record no status, not a fake one"
    );
}

/// Render a project's binding memory the way a checkpoint's `Handoff::memory`
/// does — `main.rs::binding_memory_lines` and
/// `api/unix.rs::binding_memory_lines`, verbatim. Kept as its own small
/// function here rather than imported, for the same reason the two
/// production copies are not unified: this is a data shape, not a shared
/// dependency, and three small copies are cheaper than the coupling a fourth
/// caller across a crate boundary would need.
fn memory_lines(records: Vec<glasshouse::memory::MemoryRecord>) -> Vec<String> {
    records
        .into_iter()
        .map(|record| match record.subject {
            Some(subject) => format!("{subject}: {}", record.body),
            None => record.body,
        })
        .collect()
}

/// 7. **Line 1641 — binding project memory reaches the handoff.** A project
///    with binding memory records carries them into `Handoff::memory`, and
///    `bootstrap_prompt()` names them under their own heading.
#[test]
fn a_checkpoint_captured_with_binding_memory_carries_it_into_the_prompt() {
    let tmp = tempdir();
    let fixture = Fixture::new(tmp.path(), "alpha");

    let memory = ProjectMemory::open(&fixture.runtime).unwrap();
    memory
        .store()
        .record(
            NewMemory::new(
                MemoryKind::Constraint,
                "checkpoints never carry a native session's transcript",
            )
            .with_subject(Some("checkpoint format"))
            .with_authority(Some(glasshouse::memory::MemoryAuthority::Constraint)),
        )
        .unwrap();

    let binding = memory.store().binding(20).unwrap();
    assert_eq!(binding.len(), 1, "the record must be classified as binding");
    let lines = memory_lines(binding);

    let project_checkpoints =
        glasshouse::checkpoint::ProjectCheckpoints::open(&fixture.runtime).unwrap();
    let store = project_checkpoints.store();
    let mut checkpoint = full_checkpoint("session-a", "a-harness", 1_700_000_000);
    checkpoint.handoff.memory = lines.clone();
    let stored = store.save(checkpoint).unwrap();

    let prompt = stored.checkpoint.bootstrap_prompt();
    assert!(prompt.contains("RELEVANT MEMORY"));
    for line in &lines {
        assert!(prompt.contains(line), "{line} missing from:\n{prompt}");
    }
}

/// 8. A project with no binding memory produces a prompt with no `RELEVANT
///    MEMORY` section at all — not an empty heading. Exercised against a real
///    project that genuinely has zero binding records, not a hand-built empty
///    `Vec`.
#[test]
fn a_project_with_no_binding_memory_renders_no_relevant_memory_section() {
    let tmp = tempdir();
    let fixture = Fixture::new(tmp.path(), "alpha");

    // A memory that exists but was never classified as binding — Phase 21A's
    // "an unclassified memory is not a rule" — must not appear either.
    let memory = ProjectMemory::open(&fixture.runtime).unwrap();
    memory
        .store()
        .record(NewMemory::new(
            MemoryKind::Finding,
            "the sandbox network is flaky on Tuesdays",
        ))
        .unwrap();
    let binding = memory.store().binding(20).unwrap();
    assert!(binding.is_empty(), "an unclassified memory must not bind");

    let project_checkpoints =
        glasshouse::checkpoint::ProjectCheckpoints::open(&fixture.runtime).unwrap();
    let store = project_checkpoints.store();
    let mut checkpoint = full_checkpoint("session-a", "a-harness", 1_700_000_000);
    checkpoint.handoff.memory = memory_lines(binding);
    let stored = store.save(checkpoint).unwrap();

    let prompt = stored.checkpoint.bootstrap_prompt();
    assert!(
        !prompt.contains("RELEVANT MEMORY"),
        "a project with no binding memory must render no section at all:\n{prompt}"
    );
}

/// 9. A checkpoint document written before this field existed still parses —
///    round-tripping the literal shape `Checkpoint::render` produced prior to
///    line 1641, with no `memory` key present at all.
#[test]
fn a_document_written_before_the_memory_field_existed_still_parses() {
    let older_document = serde_json::json!({
        "version": 1,
        "session": "session-a",
        "harness": "a-harness",
        "reason": "manual",
        "created_at": 1_700_000_000,
        "objective": "an objective from before line 1641",
        "implementation_state": "some state",
    })
    .to_string();

    let parsed = Checkpoint::parse(&older_document)
        .expect("a document with no `memory` key must still parse");
    assert!(
        parsed.handoff.memory.is_empty(),
        "an absent `memory` key must default to empty, not fail or fabricate content"
    );
}

/// 10. `fit()` sheds memory at the documented point and the result stays
///     within [`glasshouse::checkpoint::MAX_BYTES`]. Enough decisions, failed
///     approaches and files are given to cover the whole overshoot by
///     themselves, so a single surviving memory record proves it was not
///     touched while there was still less-protected content to give up first
///     — the wrong shed order would have reached into it instead.
#[test]
fn fit_sheds_memory_only_once_less_protected_content_is_used_up() {
    let tmp = tempdir();
    let fixture = Fixture::new(tmp.path(), "alpha");
    let project_checkpoints =
        glasshouse::checkpoint::ProjectCheckpoints::open(&fixture.runtime).unwrap();
    let store = project_checkpoints.store();

    let checkpoint = Checkpoint {
        handoff: Handoff {
            objective: "the objective".to_owned(),
            implementation_state: "the state".to_owned(),
            decisions: (0..2000).map(|i| format!("decision {i}")).collect(),
            memory: vec!["a binding constraint that must not be lost lightly".to_owned()],
            failed_approaches: (0..2000).map(|i| format!("failed approach {i}")).collect(),
            files: (0..2000).map(|i| format!("some/file/{i}.rs")).collect(),
            test_state: Some("6 of 6 passing".to_owned()),
            next_actions: vec!["do the thing".to_owned()],
        },
        ..full_checkpoint("session-a", "a-harness", 1_700_000_000)
    };

    let stored = store.save(checkpoint).unwrap();
    assert!(
        stored.checkpoint.render().len() <= glasshouse::checkpoint::MAX_BYTES,
        "still {} bytes",
        stored.checkpoint.render().len()
    );
    assert!(stored.checkpoint.trimmed);
    assert!(
        stored.checkpoint.handoff.decisions.len() < 2000,
        "nothing was given up at all"
    );
    assert_eq!(
        stored.checkpoint.handoff.memory,
        vec!["a binding constraint that must not be lost lightly".to_owned()],
        "memory must not be shed while there was still plenty of less-protected \
         content to give up first"
    );
    assert_eq!(
        stored.checkpoint.handoff.test_state,
        Some("6 of 6 passing".to_owned())
    );
    assert_eq!(
        stored.checkpoint.handoff.next_actions,
        vec!["do the thing".to_owned()]
    );
}

/// Whether this checkout's `.git` entry actually leads anywhere.
///
/// **A `.git` entry existing is not the same as a repository being readable**,
/// and the difference is not exotic — it is the ordinary state of a *copied*
/// tree. Glasshouse is developed in linked git worktrees, where `.git` is a
/// file holding `gitdir: <absolute path>`; copy that tree into a container, a
/// source tarball, or an image build, and the file arrives while the directory
/// it names does not.
///
/// A Linux container run caught exactly that, on a test that had asserted a
/// position must be readable "because a `.git` entry exists". Reporting no
/// position there is the *correct* answer, so the premise has to be checked by
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

// -------------------------------------------------------------------------
// Line 1731 — "Preserve the most recent checkpoint after a worker crashes."
//
// The sibling line, *"preserve terminal output and event history after a
// worker crashes"*, is proved in `tests/events_lifecycle.rs` against a real
// child process, and these are written in its shape deliberately: the claim
// is about what survives a process dying, and a store-level test that moves a
// session's lifecycle enum to `Failed` has not had anything die.
//
// `a_checkpoint_outlives_the_session_that_made_it` above is that store-level
// test. It is kept — it says the row is not keyed to a running session — and
// it is not this line, for the reason practice §65 gives: presence is not
// behaviour, and nothing in it starts, kills, or restarts a process.
// -------------------------------------------------------------------------

/// A project, a place to put fake harnesses, and a real `Runtime` over both.
struct CrashFixture {
    _tmp: tempfile::TempDir,
    runtime: Runtime,
    bin_dir: std::path::PathBuf,
}

impl CrashFixture {
    fn new(name: &str) -> Self {
        let tmp = tempdir();
        let fixture = Fixture::new(tmp.path(), name);
        let bin_dir = tmp.path().join("bin");
        std::fs::create_dir_all(&bin_dir).expect("create bin dir");
        Self {
            _tmp: tmp,
            runtime: fixture.runtime,
            bin_dir,
        }
    }

    /// A launch of `path` inside this fixture's project, through the same seam
    /// `shell::run` uses — no explicit working directory and no explicit
    /// program appear anywhere in these tests.
    fn launch(&self, path: &Path) -> HarnessLaunch<'_> {
        let resolved = exec::resolve_explicit(path).expect("resolve fake harness");
        HarnessLaunch::new(resolved, self.runtime.project())
    }

    /// A fresh handle on the project database — a second `Connection`, which
    /// is what a `glasshouse checkpoint show` in a new process would open.
    fn reopen(&self) -> glasshouse::checkpoint::ProjectCheckpoints {
        glasshouse::checkpoint::ProjectCheckpoints::open(&self.runtime).expect("reopen the project")
    }
}

/// A harness that announces itself and then dies badly.
///
/// The same two shapes `tests/events_lifecycle.rs` has used since the sibling
/// line closed: `SIGKILL` on Unix, which is what a crash looks like there, and
/// a non-zero code on Windows, which has no signals. Both are
/// `ProcessExit::is_crash`.
#[cfg(unix)]
fn install_crashing_harness(bin_dir: &Path) -> std::path::PathBuf {
    unix_script(bin_dir, "crasher", "#!/bin/sh\necho STARTED\nkill -9 $$\n")
}

#[cfg(windows)]
fn install_crashing_harness(bin_dir: &Path) -> std::path::PathBuf {
    windows_script(
        bin_dir,
        "crasher",
        "@echo off\r\necho STARTED\r\nexit /b 3\r\n",
    )
}

/// A harness that comes up, stays up until it is spoken to, and then dies.
///
/// **Waiting on input rather than sleeping, and that is the portable half.**
/// The restart this drives needs the harness to have been alive for
/// `SessionRuntime::HEALTHY_AFTER` before it dies — a harness that was never
/// healthy is a start that did not work and is deliberately not restarted —
/// and a sleep long enough to cross that window is a race on a loaded runner
/// and a second variable inside ConPTY. Blocking on a read is neither: the
/// test decides when the harness dies, on both platforms, by writing to it.
#[cfg(unix)]
fn install_harness_that_dies_when_spoken_to(bin_dir: &Path) -> std::path::PathBuf {
    unix_script(
        bin_dir,
        "diesonword",
        "#!/bin/sh\necho STARTED\nIFS= read -r line\nexit 3\n",
    )
}

#[cfg(windows)]
fn install_harness_that_dies_when_spoken_to(bin_dir: &Path) -> std::path::PathBuf {
    windows_script(
        bin_dir,
        "diesonword",
        "@echo off\r\necho STARTED\r\nset /p line=\r\nexit /b 3\r\n",
    )
}

#[cfg(unix)]
fn unix_script(bin_dir: &Path, name: &str, body: &str) -> std::path::PathBuf {
    use std::os::unix::fs::PermissionsExt;

    let path = bin_dir.join(name);
    std::fs::write(&path, body).expect("write fake harness");
    let mut perms = std::fs::metadata(&path).unwrap().permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&path, perms).unwrap();
    path
}

#[cfg(windows)]
fn windows_script(bin_dir: &Path, name: &str, body: &str) -> std::path::PathBuf {
    let path = bin_dir.join(format!("{name}.cmd"));
    std::fs::write(&path, body).expect("write fake harness");
    path
}

/// Long enough for a child to start, be seen healthy and die on a loaded
/// machine; short enough that a hang is a failure rather than a wait.
const CRASH_TIMEOUT: Duration = Duration::from_secs(30);
const CRASH_POLL: Duration = Duration::from_millis(20);

/// Drive the runtime the way `shell::run`'s tick does until `done`, or fail
/// saying what was seen.
///
/// `answer_terminal_queries` is in the loop because it is in the production
/// tick — see `tests/events_lifecycle.rs`, which says why leaving it out hangs
/// on Windows for a reason unrelated to the assertion.
fn drive(
    runtime: &mut SessionRuntime,
    what: &str,
    mut done: impl FnMut(&mut SessionRuntime) -> bool,
) {
    let deadline = Instant::now() + CRASH_TIMEOUT;
    loop {
        runtime.answer_terminal_queries();
        runtime.poll_exits();
        if done(runtime) {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for {what}; sessions: {runtime:?}"
        );
        std::thread::sleep(CRASH_POLL);
    }
}

/// The checkpoint a worker leaves behind, with an objective a test can point
/// at when it comes back.
fn checkpoint_before_the_crash(session: &SessionId, at: i64) -> Checkpoint {
    Checkpoint {
        created_at: at,
        handoff: Handoff {
            objective: "the work that was in flight when the harness died".to_owned(),
            ..checkpoint_handoff()
        },
        ..full_checkpoint(session.as_str(), "claude-code", at)
    }
}

/// 8. **Line 1731, first half — a real process dies and the checkpoint is
///    still there.**
///
/// A harness started through `HarnessLaunch` and driven through
/// `SessionRuntime` — the type `shell::run` owns in the shipped binary — is
/// checkpointed and then dies badly. The checkpoint is read back through a
/// **second connection to the project database**, which is the handle a
/// `glasshouse checkpoint show` in a new process opens; reading it back
/// through the same `CheckpointStore` that wrote it would prove only that the
/// value was still in this test's memory.
///
/// Three things, and the third is the one that says a crash is not a task
/// boundary: nothing new is written for a harness that died, because Phase
/// 19's automatic checkpoint is taken after a *completed turn* and a crash is
/// not one.
#[test]
fn the_most_recent_checkpoint_survives_the_worker_that_made_it_crashing() {
    let fixture = CrashFixture::new("crash-survives");
    let harness = install_crashing_harness(&fixture.bin_dir);
    let id = SessionId::new("crashed-worker");

    let mut live = SessionRuntime::new();
    live.start(
        id.clone(),
        SessionPresentation::Embedded,
        &fixture.launch(&harness),
    )
    .expect("start the crashing harness");

    // The checkpoint the worker had before it died. Written through the same
    // store `glasshouse checkpoint save` uses, while the process is alive.
    let before = {
        let project = fixture.reopen();
        let store = project.store();
        store
            .save(checkpoint_before_the_crash(&id, store.now()))
            .expect("save the checkpoint")
    };

    drive(&mut live, "the harness to crash", |live| {
        live.get(&id).is_some_and(|s| s.exit().is_some())
    });
    let crashed = live.get(&id).expect("the session is still held");
    let exit = glasshouse::events::ProcessExit::from_status(
        crashed.exit().expect("the harness has exited"),
    );
    assert!(exit.is_crash(), "{exit:?} is not a crash");

    // A second connection, opened after the process died: the view a new
    // `glasshouse` has of this project.
    let after = fixture.reopen();
    let store = after.store();

    let latest = store
        .latest_for(&id)
        .expect("read the crashed worker's checkpoints")
        .expect("the most recent checkpoint must survive the worker that made it");
    assert_eq!(
        latest, before,
        "the checkpoint that came back is not the one the worker had"
    );
    assert_eq!(
        latest.checkpoint.handoff.objective, "the work that was in flight when the harness died",
        "the handoff must come back whole, not merely as a row"
    );

    // Reachable without knowing which session died, too — this is what
    // `glasshouse checkpoint show` and `--from-checkpoint latest` resolve.
    assert_eq!(
        store.latest().expect("read the project's checkpoints"),
        Some(before.clone()),
        "the project's most recent checkpoint must be the crashed worker's"
    );
    assert_eq!(
        store.get(&before.id).expect("look the checkpoint up"),
        Some(before.clone()),
        "and it must still resolve by its own identifier"
    );

    // A crash is not a task boundary, so nothing may have written a second
    // one. Without this the test would pass on a build that had replaced the
    // handoff with something invented from the dead harness's scrollback,
    // which is the one thing this project refuses to do everywhere else.
    assert_eq!(
        store.list().expect("list the project's checkpoints").len(),
        1,
        "a crash is not a completed turn and must not produce a checkpoint of its own"
    );

    live.close(&id).expect("close the session");
}

/// 9. **Line 1731, second half — and it is still reachable after the restart.**
///
/// Surviving is not the whole claim. `CheckpointStore::latest_for` is keyed on
/// the session identifier, and Phase 10A's tenth line puts a crashed harness
/// *back* — so a restart that decided it was a new session would leave the
/// checkpoint present in the project and unreachable from the work it
/// describes. That is the shape `session/runtime.rs` already guards for
/// scrollback, in as many words, and nothing was watching the same question
/// for the checkpoint.
///
/// The harness has to be seen healthy before it dies or there is no restart at
/// all: a harness that never came up is a start that did not work, and
/// `consider_restart` deliberately leaves it alone. So this one waits to be
/// spoken to rather than racing a sleep against `HEALTHY_AFTER`.
#[test]
fn a_restarted_worker_can_still_reach_the_checkpoint_it_had_before_it_crashed() {
    let fixture = CrashFixture::new("crash-restart");
    let harness = install_harness_that_dies_when_spoken_to(&fixture.bin_dir);
    let id = SessionId::new("restarted-worker");

    let mut live = SessionRuntime::new();
    live.start(
        id.clone(),
        SessionPresentation::Embedded,
        &fixture.launch(&harness),
    )
    .expect("start the harness");

    let before = {
        let project = fixture.reopen();
        let store = project.store();
        store
            .save(checkpoint_before_the_crash(&id, store.now()))
            .expect("save the checkpoint")
    };

    // Health is decided by a poll that finds the process still running after
    // `HEALTHY_AFTER`, so the loop has to keep ticking through that window.
    drive(&mut live, "the harness to be verified healthy", |live| {
        live.get(&id).is_some_and(LiveSession::verified_healthy)
    });

    // Now kill it, by saying something to it.
    //
    // **`\r`, not `\n`, and through the sender the binary itself uses.** A
    // Windows console never completes a `set /p` on `\n`, so a test that
    // writes its own terminator exercises a line nothing in production sends
    // and hangs on the one platform it was meant to cover — the same reasoning
    // `tests/events_lifecycle.rs` records beside its own `ping\r`.
    live.send_text_from(&id, "go\r", MessageOrigin::Machine)
        .expect("write to the harness");
    // **Waited for by session count, not by identifier**, and that is
    // practice §80's fifth case rather than a style choice: the identity is
    // the thing under test here, so a build that renamed the session on
    // restart would make `get(&id)` answer `None` forever and this would fail
    // as *"timed out waiting for the harness to crash and be put back"* —
    // a true verdict credited to an assertion that never ran. Watching the
    // runtime's own list instead lets the failure land on the line below,
    // which says what is actually wrong.
    drive(&mut live, "the harness to crash and be put back", |live| {
        live.sessions()
            .iter()
            .any(|session| session.restarts() >= 1)
    });

    let restarted = live.get(&id).expect(
        "a restarted harness must come back under the session identity it had: \
         `CheckpointStore::latest_for` is keyed on it, so a session that came back \
         under a new one has left its checkpoint present in the project and \
         unreachable from the work it describes",
    );
    assert_eq!(
        restarted.restarts(),
        1,
        "the harness must have been put back"
    );
    assert!(
        restarted.restart_halted().is_none(),
        "the restart must have worked: {:?}",
        restarted.restart_halted()
    );
    assert!(
        restarted.is_running(),
        "and the new harness must be running"
    );

    // The claim: the session that came back is the same session, so what it
    // had before the crash is still its own.
    let after = fixture.reopen();
    let store = after.store();
    let latest = store
        .latest_for(&id)
        .expect("read the restarted worker's checkpoints")
        .expect("a restarted worker must still reach the checkpoint it had before it crashed");
    assert_eq!(
        latest, before,
        "the restarted session reached a different checkpoint than the one it had"
    );

    live.close(&id).expect("close the session");
}

// -------------------------------------------------------------------------
// "The most recent checkpoint" inside one second.
//
// `created_at` is whole seconds, so two checkpoints written back to back
// nearly always tie on it. What breaks the tie decides what
// `glasshouse checkpoint show`, `--from-checkpoint latest` and the
// task-boundary carry-forward hand the user, so the tie is not a detail of
// the store — it is the answer.
//
// These are **rate** tests, not single-pair tests (§60). One ordered pair
// against a coin flip passes half the time and proves nothing.
// -------------------------------------------------------------------------

/// How many back-to-back pairs each resolution probe writes.
///
/// Sized so that "0 wrong" means something: against a defect that resolves
/// wrongly about half the time within a second, 200 independent pairs put the
/// chance of a clean run by luck at 2^-200. Read the other way, a clean run
/// of 200 bounds the residual wrong-resolution rate at roughly 1.5% with 95%
/// confidence (the rule of three, 3/200).
const RESOLUTION_PAIRS: usize = 200;

/// Two checkpoints written into the same second, and the second one wins.
///
/// The clock is pinned rather than raced, so this reproduces the *state* the
/// defect lives in — two rows with equal `created_at` — on any machine, at
/// any load, with no timing assumption at all (§59). The sibling test below
/// takes the real clock and measures how often that state arises on its own.
#[test]
fn the_second_of_two_checkpoints_written_in_one_second_is_the_most_recent() {
    let tmp = tempdir();
    let fixture = Fixture::new(tmp.path(), "same-second");
    let pinned: glasshouse::session::store::Clock = std::sync::Arc::new(|| 1_700_000_000);
    let project = glasshouse::checkpoint::ProjectCheckpoints::open_with_clock(
        &fixture.runtime,
        std::sync::Arc::clone(&pinned),
    )
    .expect("open the project checkpoints");
    let store = project.store();

    let mut wrong_for_session = 0usize;
    let mut wrong_overall = 0usize;
    let mut last_written = None;

    for pair in 0..RESOLUTION_PAIRS {
        let session = SessionId::new(format!("session-{pair}"));
        let first = store
            .save(checkpoint_before_the_crash(&session, store.now()))
            .expect("save the first checkpoint of the pair");
        let second = store
            .save(checkpoint_before_the_crash(&session, store.now()))
            .expect("save the second checkpoint of the pair");
        assert_eq!(
            first.checkpoint.created_at, second.checkpoint.created_at,
            "the pinned clock must put both checkpoints of pair {pair} in one second"
        );
        assert_ne!(first.id, second.id, "each save must get its own identifier");

        let latest = store
            .latest_for(&session)
            .expect("resolve the session's most recent checkpoint")
            .expect("the session has two checkpoints");
        if latest.id != second.id {
            wrong_for_session += 1;
        }
        let overall = store
            .latest()
            .expect("resolve the project's most recent checkpoint")
            .expect("the project has checkpoints");
        if overall.id != second.id {
            wrong_overall += 1;
        }
        last_written = Some(second);
    }

    assert_eq!(
        wrong_for_session, 0,
        "`latest_for` returned the older checkpoint of a same-second pair in \
         {wrong_for_session} of {RESOLUTION_PAIRS} pairs; inside one second it \
         must still be the one written second"
    );
    assert_eq!(
        wrong_overall, 0,
        "`latest` returned the older checkpoint of a same-second pair in \
         {wrong_overall} of {RESOLUTION_PAIRS} pairs"
    );

    // And the listing agrees with the resolution, rather than ordering one way
    // while `latest` answers another.
    let listed = store.list().expect("list every checkpoint");
    assert_eq!(listed.len(), RESOLUTION_PAIRS * 2);
    assert_eq!(
        listed.first().map(|s| &s.id),
        last_written.as_ref().map(|s| &s.id),
        "the listing's first row must be the checkpoint `latest` resolves to"
    );
}

/// The same claim through the real clock, which is how the defect was found.
///
/// This one measures rather than pins: it records how many of the pairs
/// actually landed in one second, and refuses to pass if too few did — a run
/// where every pair straddled a second boundary would be ordered correctly by
/// `created_at` alone and would prove nothing about the tie.
#[test]
fn back_to_back_checkpoints_resolve_to_the_one_written_second() {
    let tmp = tempdir();
    let fixture = Fixture::new(tmp.path(), "back-to-back");
    let project = glasshouse::checkpoint::ProjectCheckpoints::open(&fixture.runtime)
        .expect("open the project checkpoints");
    let store = project.store();

    let mut shared_a_second = 0usize;
    let mut wrong = 0usize;

    for pair in 0..RESOLUTION_PAIRS {
        let session = SessionId::new(format!("session-{pair}"));
        let first = store
            .save(checkpoint_before_the_crash(&session, store.now()))
            .expect("save the first checkpoint of the pair");
        let second = store
            .save(checkpoint_before_the_crash(&session, store.now()))
            .expect("save the second checkpoint of the pair");
        if first.checkpoint.created_at == second.checkpoint.created_at {
            shared_a_second += 1;
        }
        let latest = store
            .latest_for(&session)
            .expect("resolve the session's most recent checkpoint")
            .expect("the session has two checkpoints");
        if latest.id != second.id {
            wrong += 1;
        }
    }

    // Non-vacuity, and deliberately lenient: a machine slow enough to put
    // fewer than half of these pairs in one second is a machine on which every
    // other test in this file has already timed out.
    assert!(
        shared_a_second * 2 >= RESOLUTION_PAIRS,
        "only {shared_a_second} of {RESOLUTION_PAIRS} pairs landed in one second, \
         so this run never entered the state under test"
    );
    assert_eq!(
        wrong, 0,
        "`latest_for` resolved to the older checkpoint in {wrong} of \
         {RESOLUTION_PAIRS} back-to-back pairs, {shared_a_second} of which \
         shared a second"
    );
}

/// The counter is a write order, not a clock reading.
///
/// A clock that steps backwards is ordinary — NTP correcting a drift, a laptop
/// resuming, a container starting with a bad time. Under the old ordering the
/// checkpoint written second would then lose by a whole second rather than by
/// a coin flip, which is the same defect with a rarer trigger and a longer
/// reach. This drives the clock deliberately backwards and requires the last
/// write to win anyway.
#[test]
fn a_clock_that_steps_backwards_does_not_resurrect_an_older_checkpoint() {
    use std::sync::Mutex;

    let tmp = tempdir();
    let fixture = Fixture::new(tmp.path(), "backwards-clock");

    // 3000, then 2000, then 1000: every write is stamped *earlier* than the
    // one before it.
    let readings = std::sync::Arc::new(Mutex::new(vec![1_000i64, 2_000, 3_000]));
    let stepping: glasshouse::session::store::Clock = {
        let readings = std::sync::Arc::clone(&readings);
        std::sync::Arc::new(move || readings.lock().unwrap().pop().unwrap_or(1_000))
    };
    let project =
        glasshouse::checkpoint::ProjectCheckpoints::open_with_clock(&fixture.runtime, stepping)
            .expect("open the project checkpoints");
    let store = project.store();
    let session = SessionId::new("session-a");

    let first = store
        .save(checkpoint_before_the_crash(&session, store.now()))
        .expect("save the first checkpoint");
    let second = store
        .save(checkpoint_before_the_crash(&session, store.now()))
        .expect("save the second checkpoint");
    let third = store
        .save(checkpoint_before_the_crash(&session, store.now()))
        .expect("save the third checkpoint");

    assert!(
        third.checkpoint.created_at < second.checkpoint.created_at
            && second.checkpoint.created_at < first.checkpoint.created_at,
        "the fixture must actually have stepped the clock backwards: {}, {}, {}",
        first.checkpoint.created_at,
        second.checkpoint.created_at,
        third.checkpoint.created_at
    );

    assert_eq!(
        store.latest_for(&session).unwrap().unwrap().id,
        third.id,
        "the checkpoint written last must win even though its timestamp is the oldest"
    );
    assert_eq!(store.latest().unwrap().unwrap().id, third.id);
    assert_eq!(
        store
            .list()
            .unwrap()
            .into_iter()
            .map(|s| s.id)
            .collect::<Vec<_>>(),
        vec![third.id, second.id, first.id],
        "the listing must be write order too, not clock order"
    );
}

/// Ordering by a project-wide counter must not widen what `latest` can see.
///
/// `latest` has no `WHERE` clause — its scope is the database file, one per
/// project, and the counter is now the only thing it orders by. So the claim
/// worth proving is that two projects number their checkpoints independently
/// and neither can be handed the other's, including when the *other* project's
/// checkpoint is the one written most recently in wall-clock terms.
#[test]
fn one_projects_counter_never_reaches_another_projects_checkpoints() {
    let tmp = tempdir();
    let alpha = Fixture::new(tmp.path(), "alpha");
    let beta = Fixture::new(tmp.path(), "beta");

    let alpha_project =
        glasshouse::checkpoint::ProjectCheckpoints::open(&alpha.runtime).expect("open alpha");
    let beta_project =
        glasshouse::checkpoint::ProjectCheckpoints::open(&beta.runtime).expect("open beta");

    // Alpha writes three; beta writes one, last, so beta's is the newest by
    // any clock and alpha's counter has run further.
    let alpha_store = alpha_project.store();
    let shared = SessionId::new("session-a");
    let mut alpha_ids = Vec::new();
    for _ in 0..3 {
        alpha_ids.push(
            alpha_store
                .save(checkpoint_before_the_crash(&shared, alpha_store.now()))
                .expect("save an alpha checkpoint")
                .id,
        );
    }
    let beta_store = beta_project.store();
    let beta_only = beta_store
        .save(checkpoint_before_the_crash(&shared, beta_store.now()))
        .expect("save the beta checkpoint");

    assert_eq!(
        alpha_store.latest().unwrap().unwrap().id,
        *alpha_ids.last().unwrap(),
        "alpha's latest must be alpha's own last write, not beta's newer one"
    );
    assert_eq!(
        beta_store.latest().unwrap().unwrap().id,
        beta_only.id,
        "beta's latest must be its only checkpoint"
    );
    assert_eq!(alpha_store.list().unwrap().len(), 3);
    assert_eq!(beta_store.list().unwrap().len(), 1);

    // The same session identifier exists in both projects, which is the case
    // a widened predicate would get wrong.
    assert_eq!(
        beta_store.latest_for(&shared).unwrap().unwrap().id,
        beta_only.id,
        "a session name shared between projects must resolve inside its own project"
    );
    assert!(
        !alpha_ids.contains(&beta_store.latest_for(&shared).unwrap().unwrap().id),
        "no alpha checkpoint may be reachable from beta"
    );
}

/// Two processes writing checkpoints at once never get the same number.
///
/// The counter is `MAX(seq) + 1`, and read-then-write is the classic shape of
/// a lost update. It is computed **inside** the `INSERT` rather than in Rust
/// for exactly that reason: SQLite takes the database's write lock at the
/// start of a writing statement, so the subquery reads under the same lock
/// that will do the write, and `database::open` gives every connection a
/// five-second busy timeout so the loser waits rather than failing.
///
/// That is a claim about SQLite's locking, so it is measured rather than
/// asserted. Two connections in two threads interleave 100 saves each; a
/// counter read outside the lock would collide almost immediately, and a
/// collision is visible as two rows sharing a `seq` — which is the state in
/// which `latest` becomes a coin flip again.
#[test]
fn two_writers_racing_never_stamp_the_same_write_order() {
    const PER_WRITER: usize = 100;

    let tmp = tempdir();
    let base = tmp.path().to_path_buf();
    // Bootstrap once up front, so the threads race on `save` and not on the
    // first-launch migration, which has its own test.
    let fixture = Fixture::new(&base, "racing-writers");

    // Both writers open their own connection first and then start together.
    // Without the barrier the second thread's bootstrap can finish after the
    // first has already written everything, and the test would pass on a race
    // that never happened.
    let start = std::sync::Arc::new(std::sync::Barrier::new(2));
    let writers: Vec<_> = ["writer-a", "writer-b"]
        .into_iter()
        .map(|name| {
            let base = base.clone();
            let start = std::sync::Arc::clone(&start);
            std::thread::spawn(move || {
                let fixture = Fixture::new(&base, "racing-writers");
                let project = glasshouse::checkpoint::ProjectCheckpoints::open(&fixture.runtime)
                    .expect("open the project checkpoints");
                let store = project.store();
                let session = SessionId::new(name);
                start.wait();
                for _ in 0..PER_WRITER {
                    store
                        .save(checkpoint_before_the_crash(&session, store.now()))
                        .expect("a racing save must not fail");
                }
            })
        })
        .collect();
    for writer in writers {
        writer.join().expect("a writer thread panicked");
    }

    let conn = fixture.checkpoints();
    let (rows, distinct, highest): (i64, i64, i64) = conn
        .query_row(
            "SELECT COUNT(*), COUNT(DISTINCT seq), MAX(seq) FROM checkpoints",
            [],
            |row| Ok((row.get_unwrap(0), row.get_unwrap(1), row.get_unwrap(2))),
        )
        .unwrap();

    let expected = (PER_WRITER * 2) as i64;
    assert_eq!(rows, expected, "every save must have landed");
    assert_eq!(
        distinct,
        expected,
        "two writers stamped the same write order {} times; the counter was read \
         outside the write lock",
        expected - distinct
    );
    assert_eq!(
        highest, expected,
        "the counter must run 1..{expected} with no gaps and no restarts"
    );
}
