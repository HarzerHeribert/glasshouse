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

use clap::Parser;

use glasshouse::checkpoint::{Checkpoint, CheckpointReason, GitPosition, Handoff};
use glasshouse::memory::search::SearchScope;
use glasshouse::memory::{MemoryKind, NewMemory, ProjectMemory};
use glasshouse::session::{NewSession, SessionId, SessionLifecycle};
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
        handoff: Handoff {
            objective: "close out the sub-record test packet".to_owned(),
            implementation_state: "the event log tests pass; the checkpoint tests are next"
                .to_owned(),
            decisions: vec!["use the sibling idiom for project isolation".to_owned()],
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
