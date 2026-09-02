//! GH-FILE-AWARE-MEMORY — Phase 28's last three lines: a memory that knows
//! which files it is about, ordered for an intended edit, and always
//! labelled advisory.
//!
//! - **1139** — the context firewall's `PostToolUse` hook records a
//!   `file_touched` event per path a writing tool named (migration 26), the
//!   extraction chunk renders it as `edited <path>`, and a path the model
//!   returns becomes a `referenced` association **only** when it is
//!   byte-equal to one of them.
//! - **1141** — `for_path` under `RetrievalIntent::CodeEdit` puts
//!   constraints, decisions and failed attempts ahead of features, findings
//!   and todos *within* a ladder rung.
//! - **1142** — every file-aware row is advisory and carries a commit-order
//!   freshness that never withholds, reorders or rescores.
//!
//! # Why the recording half runs the shipped binary
//!
//! `context_firewall_hook` is a `main.rs` function whose whole contract is
//! about a subprocess: it reads a `PostToolUse` document on stdin, writes a
//! response on stdout, and must do the second identically whether or not the
//! first produced anything to record. An in-process call would prove the
//! parsing and none of that, so the recording tests here spawn
//! `glasshouse context-firewall hook` for real and read its stdout —
//! `tests/firewall_bridge.rs`'s own `Fixture` shape.
//!
//! # Why the freshness half builds a real repository
//!
//! `checkpoint::git::last_change_commit` and `is_ancestor` run `git`. A
//! fixture that faked either would be testing this file rather than the
//! product, so these build a real two-commit repository in a temporary
//! directory. `git` is present on every leg this project's gate runs.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::Mutex;

use clap::Parser;

use glasshouse::checkpoint::git::{Freshness, is_ancestor, last_change_commit};
use glasshouse::events::{EventLog, LifecycleEvent};
use glasshouse::memory::extract::chunk::ChunkLimits;
use glasshouse::memory::extract::lifecycle::{EVENT_WINDOW, chunk_for_session};
use glasshouse::memory::extract::{
    ExtractionModel, ExtractionTrigger, Extractor, ModelError, Prompt,
};
use glasshouse::memory::search::{RetrievalIntent, SearchScope};
use glasshouse::memory::{FileAssociation, MemoryAuthority, MemoryKind, NewMemory, ProjectMemory};
use glasshouse::session::SessionId;
use glasshouse::{Cli, Runtime};

// ===========================================================================
// Fixtures
// ===========================================================================

/// A bootstrapped project whose root is also a real git repository, so the
/// same fixture serves the recording tests (which need a project root to
/// normalise paths against) and the freshness tests (which need commits).
struct Fixture {
    root: PathBuf,
    base: PathBuf,
    runtime: Runtime,
}

impl Fixture {
    fn new(base: &Path, name: &str) -> Self {
        let root = base.join("workspace").join(name);
        std::fs::create_dir_all(&root).unwrap();
        let root = std::fs::canonicalize(&root).unwrap();
        git_init(&root);

        let cli = Cli::try_parse_from([
            "glasshouse",
            "--data-dir",
            base.join("data").to_str().unwrap(),
            "--config-dir",
            base.join("config").to_str().unwrap(),
        ])
        .unwrap();
        let runtime = glasshouse::bootstrap(&cli, &root).unwrap();
        Fixture {
            root,
            base: base.to_path_buf(),
            runtime,
        }
    }

    fn memory(&self) -> ProjectMemory {
        ProjectMemory::open(&self.runtime).unwrap()
    }

    /// Run the shipped binary against this project, with the event on stdin.
    fn run(&self, args: &[&str], stdin: &[u8]) -> Output {
        use std::io::Write;
        let mut child = Command::new(env!("CARGO_BIN_EXE_glasshouse"))
            .arg("--scope")
            .arg(&self.root)
            .arg("--data-dir")
            .arg(self.base.join("data"))
            .arg("--config-dir")
            .arg(self.base.join("config"))
            .args(args)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .expect("glasshouse must be spawnable");
        child
            .stdin
            .take()
            .unwrap()
            .write_all(stdin)
            .expect("stdin must accept the event");
        child.wait_with_output().expect("glasshouse must exit")
    }

    /// One `PostToolUse` event through the real hook, returning its raw
    /// stdout — raw, not parsed, because the byte-identical assertion below
    /// is about the bytes.
    fn hook(&self, event: &serde_json::Value, extra: &[&str]) -> Vec<u8> {
        let mut args = vec!["context-firewall", "hook"];
        args.extend_from_slice(extra);
        let output = self.run(&args, &serde_json::to_vec(event).unwrap());
        assert!(
            output.status.success(),
            "the hook must always exit 0 (fail open): stdout={} stderr={}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        output.stdout
    }

    /// Every `file_touched` path this project's event log holds, in order.
    fn touched(&self) -> Vec<String> {
        let log = EventLog::open(&self.runtime).unwrap();
        log.all()
            .unwrap()
            .into_iter()
            .filter_map(|logged| match logged.event {
                LifecycleEvent::FileTouched { path } => Some(path),
                _ => None,
            })
            .collect()
    }
}

fn git(root: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
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

/// A repository with an identity of its own, so the test never depends on
/// the machine's `user.name`/`user.email` being configured.
fn git_init(root: &Path) {
    git(root, &["init", "--quiet"]);
    git(root, &["config", "user.name", "Glasshouse Test"]);
    git(root, &["config", "user.email", "test@example.invalid"]);
    git(root, &["config", "commit.gpgsign", "false"]);
}

fn commit_file(root: &Path, relative: &str, contents: &str, message: &str) -> String {
    let path = root.join(relative);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(&path, contents).unwrap();
    git(root, &["add", "--", relative]);
    git(root, &["commit", "--quiet", "-m", message]);
    git(root, &["rev-parse", "HEAD"])
}

fn post_tool_use(tool_name: &str, file_path: &str) -> serde_json::Value {
    serde_json::json!({
        "tool_name": tool_name,
        "tool_input": { "file_path": file_path },
        "tool_response": { "type": "text", "text": "done" },
        "tool_use_id": "tu-1",
        "session_id": "claude-code-own-id",
        "cwd": "/tmp",
    })
}

/// Answers with a fixed reply. The paths a test wants the model to claim go
/// straight into that reply, which is what makes the guard's own behaviour —
/// not the model's — the thing under test.
struct Canned {
    reply: String,
    seen: Mutex<Vec<String>>,
}

impl Canned {
    fn new(reply: impl Into<String>) -> Self {
        Self {
            reply: reply.into(),
            seen: Mutex::new(Vec::new()),
        }
    }

    fn last_prompt(&self) -> String {
        self.seen.lock().unwrap().last().cloned().unwrap()
    }
}

impl ExtractionModel for Canned {
    fn describe(&self) -> String {
        "fake/canned".to_owned()
    }

    fn complete(&self, prompt: &Prompt) -> Result<String, ModelError> {
        self.seen.lock().unwrap().push(prompt.as_str().to_owned());
        Ok(self.reply.clone())
    }
}

/// One memory the model emits, claiming `paths`.
fn memory_claiming(body: &str, paths: &[&str]) -> String {
    let paths = paths
        .iter()
        .map(|p| format!("{p:?}"))
        .collect::<Vec<_>>()
        .join(",");
    format!(
        "{{\"memories\": [{{\"kind\": \"finding\", \"authority\": \"decision\", \
         \"disposition\": \"accepted\", \"support\": \"established\", \
         \"confidence\": \"certain\", \"body\": {body:?}, \"paths\": [{paths}]}}]}}"
    )
}

/// The whole production shape of an automatic extraction: real
/// `file_touched` events, read back through the real `EventLog`, rendered by
/// the real `chunk_for_session`, and extracted with the real `Extractor`.
///
/// This is what makes the guard's proof non-vacuous. Building the chunk by
/// hand would let a test hand `touched_paths` whatever it liked, which is
/// exactly the fixture-reproduces-the-production-step trap (practice §35).
fn extract_from_session(
    fixture: &Fixture,
    session: &SessionId,
    model: &dyn ExtractionModel,
    commit: Option<&str>,
) -> glasshouse::memory::ExtractionOutcome {
    let log = EventLog::open(&fixture.runtime).unwrap();
    let events = log.recent_for_session(session, EVENT_WINDOW).unwrap();
    drop(log);
    let chunk = chunk_for_session(session, &events, commit, ChunkLimits::default());
    let memory = fixture.memory();
    let store = memory.store();
    Extractor::new(&store, model).run(&chunk, ExtractionTrigger::Manual)
}

// ===========================================================================
// 1139, the producer — the hook records what a writing tool edited.
// ===========================================================================

/// The end-to-end line 1139 rests on, driven through the shipped binary.
///
/// **Mutation target `record-nothing`**: remove the `FileTouched` append from
/// `record_file_touches` and no `referenced` row exists at the end of this
/// test, because there is nothing for the guard to match the model's path
/// against.
#[test]
fn an_edit_through_the_hook_becomes_a_referenced_association_on_the_memory_that_names_it() {
    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path(), "alpha");
    let session = SessionId::new("s-hook-1");

    let edited = fixture.root.join("crates").join("x.rs");
    fixture.hook(
        &post_tool_use("Edit", edited.to_str().unwrap()),
        &["--session", session.as_str()],
    );

    assert_eq!(
        fixture.touched(),
        vec!["crates/x.rs".to_owned()],
        "an absolute Edit path inside the project root records one repo-relative event"
    );

    // The model names the file it edited, and one it never touched.
    let model = Canned::new(memory_claiming(
        "the loader mmaps the index in threes",
        &["crates/x.rs", "crates/y.rs"],
    ));
    let outcome = extract_from_session(&fixture, &session, &model, None);

    assert!(
        model.last_prompt().contains("edited crates/x.rs"),
        "the model must be shown the edited path verbatim, or the guard has nothing to \
         compare against: {}",
        model.last_prompt()
    );
    assert_eq!(outcome.recorded.len(), 1, "{outcome:?}");
    assert_eq!(
        outcome.paths_dropped, 1,
        "the path the session never edited must be dropped and counted: {outcome:?}"
    );

    let memory = fixture.memory();
    let store = memory.store();
    let touched = store
        .for_path(
            "crates/x.rs",
            SearchScope::Current,
            10,
            RetrievalIntent::Lookup,
        )
        .unwrap();
    let id = &outcome.recorded[0];
    assert_eq!(
        touched.association(id),
        Some(FileAssociation::Referenced),
        "the edited path the model named must be stored as referenced"
    );

    let untouched = store
        .for_path(
            "crates/y.rs",
            SearchScope::Current,
            10,
            RetrievalIntent::Lookup,
        )
        .unwrap();
    assert!(
        untouched.invariants_and_constraints.is_empty() && untouched.other.is_empty(),
        "a path the session never edited must hold no association at all"
    );
}

/// **Mutation target `guard-off`**: accept every path the model returns, and
/// this test fails — a `referenced` row appears for a file the session never
/// touched.
///
/// Separate from the test above rather than folded into it, because this one
/// must fail even when the session touched *nothing*: the guard's floor is
/// that an empty touched set admits nothing at all.
#[test]
fn a_path_the_session_never_edited_is_never_stored_however_confidently_the_model_names_it() {
    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path(), "alpha");
    let session = SessionId::new("s-guard-1");

    // A read, not a write. The hook sees a path and records nothing.
    let read = fixture.root.join("crates").join("z.rs");
    fixture.hook(
        &post_tool_use("Read", read.to_str().unwrap()),
        &["--session", session.as_str()],
    );
    assert!(
        fixture.touched().is_empty(),
        "a Read must record nothing: touched means the session changed the file"
    );

    // Something for the chunk to be non-empty about, so the extraction runs.
    {
        let log = EventLog::open(&fixture.runtime).unwrap();
        let bus = glasshouse::events::EventBus::with_history(0);
        let recorded = bus.publish(&session, LifecycleEvent::TurnStarted);
        log.append(&recorded, None).unwrap();
    }

    let model = Canned::new(memory_claiming(
        "the reader walks the index backwards",
        &["crates/z.rs"],
    ));
    let outcome = extract_from_session(&fixture, &session, &model, None);
    assert_eq!(outcome.recorded.len(), 1, "{outcome:?}");
    assert_eq!(outcome.paths_dropped, 1, "{outcome:?}");

    let memory = fixture.memory();
    let store = memory.store();
    let grouped = store
        .for_path(
            "crates/z.rs",
            SearchScope::Current,
            10,
            RetrievalIntent::Lookup,
        )
        .unwrap();
    assert!(
        grouped.invariants_and_constraints.is_empty() && grouped.other.is_empty(),
        "the model named a path the session only READ; no association may exist"
    );
}

/// The isolation invariant, and the Windows separator fold, in one test —
/// both are properties of `project_relative_path` and both are tested with
/// literals on every platform, because a path is a string here.
#[test]
fn a_path_outside_the_project_root_is_never_stored_and_backslashes_fold() {
    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path(), "alpha");
    let session = SessionId::new("s-scope-1");

    for outside in ["/etc/passwd", "/tmp/somewhere-else/secret.rs"] {
        fixture.hook(
            &post_tool_use("Write", outside),
            &["--session", session.as_str()],
        );
    }
    assert!(
        fixture.touched().is_empty(),
        "nothing outside the project root may be stored, not even to be filtered later"
    );

    // A repo-relative path spelled the Windows way. Claude Code on Windows
    // hands the hook `\`-separated paths, and the fold is what makes them the
    // same file as the `/`-separated ones every other producer writes.
    fixture.hook(
        &post_tool_use("Write", r"crates\glasshouse\src\a.rs"),
        &["--session", session.as_str()],
    );
    assert_eq!(
        fixture.touched(),
        vec!["crates/glasshouse/src/a.rs".to_owned()],
        "backslashes fold to the one spelling memory_files.path stores"
    );
}

/// `MultiEdit` names one file once per edit. Sixty rows saying one file
/// changed is sixty times the storage for one fact.
#[test]
fn one_tool_call_naming_a_file_twice_records_it_once() {
    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path(), "alpha");
    let session = SessionId::new("s-dedupe-1");

    let event = serde_json::json!({
        "tool_name": "MultiEdit",
        "tool_input": {
            "file_path": "crates/a.rs",
            "notebook_path": "./crates//a.rs",
        },
        "tool_response": { "type": "text", "text": "done" },
        "tool_use_id": "tu-1",
        "session_id": "cc",
        "cwd": "/tmp",
    });
    fixture.hook(&event, &["--session", session.as_str()]);

    assert_eq!(fixture.touched(), vec!["crates/a.rs".to_owned()]);
}

/// The hook's response is never affected by recording — the invariant that
/// makes this producer safe to put on the path of every tool call.
///
/// Proven across every recording outcome a hook invocation can actually
/// reach, on one project, with the response bytes compared directly:
///
/// 1. **recorded** — an `Edit` on a path inside the project root;
/// 2. **skipped, no session** — the same event with no `--session`, which is
///    what an old settings document produces;
/// 3. **skipped, not a writing tool** — a `Read` of the same path;
/// 4. **skipped, outside the root** — an `Edit` of `/etc/passwd`.
///
/// # What this deliberately does not try to inject, and why
///
/// A *failed* append. It is unreachable from outside the process: the
/// binary's own bootstrap opens the project database before any subcommand
/// runs and refuses to start at all if it cannot, so a database made
/// unwritable never gets as far as the hook — verified, and the reason this
/// test is shaped the way it is rather than the way the packet sketched.
/// `record_file_touches_never_propagates_a_failure` in the binary's own
/// tests covers that branch where it is reachable.
#[test]
fn the_hook_response_is_identical_across_every_recording_outcome() {
    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path(), "alpha");
    let session = SessionId::new("s-resp-1");
    let edited = fixture.root.join("crates").join("x.rs");
    let edited = edited.to_str().unwrap();

    let recorded = fixture.hook(
        &post_tool_use("Edit", edited),
        &["--session", session.as_str()],
    );
    assert_eq!(
        fixture.touched(),
        vec!["crates/x.rs".to_owned()],
        "the recorded case must actually have recorded, or the comparison proves nothing"
    );

    let cases: [(&str, serde_json::Value, Vec<&str>); 3] = [
        ("no --session", post_tool_use("Edit", edited), Vec::new()),
        (
            "not a writing tool",
            post_tool_use("Read", edited),
            vec!["--session", session.as_str()],
        ),
        (
            "outside the project root",
            post_tool_use("Edit", "/etc/passwd"),
            vec!["--session", session.as_str()],
        ),
    ];
    for (label, event, args) in cases {
        let response = fixture.hook(&event, &args);
        assert_eq!(
            String::from_utf8_lossy(&recorded),
            String::from_utf8_lossy(&response),
            "recording must not be visible in the hook's response ({label})"
        );
    }

    assert_eq!(
        fixture.touched(),
        vec!["crates/x.rs".to_owned()],
        "and none of the three skipped cases may have recorded anything"
    );
}

// ===========================================================================
// 1139, the association — the strongest of the rows a memory holds.
// ===========================================================================

/// One memory may hold both rows for one file: the automatic path writes the
/// dirty tree as `observed` and the model's guarded choice as `referenced`.
/// The door reports the claim, not the correlation.
///
/// **Mutation target `label-observed`**: make `association()` answer
/// `Observed` for a referenced row and this fails.
#[test]
fn a_memory_carrying_both_rows_for_one_file_reports_referenced() {
    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path(), "alpha");
    let project = fixture.memory();
    let store = project.store();

    let both = store
        .record(NewMemory::new(MemoryKind::Finding, "both rows"))
        .unwrap()
        .id;
    let observed_only = store
        .record(NewMemory::new(MemoryKind::Finding, "observed only"))
        .unwrap()
        .id;

    // Written in the weaker-last order on purpose: a reader that took
    // whichever row came back first would pass on one ordering and fail on
    // the other, and `group_concat` gives no ordering guarantee.
    store
        .record_referenced_files(std::slice::from_ref(&both), &["src/a.rs".to_owned()])
        .unwrap();
    store
        .record_observed_files(
            &[both.clone(), observed_only.clone()],
            &["src/a.rs".to_owned()],
        )
        .unwrap();

    let grouped = store
        .for_path(
            "src/a.rs",
            SearchScope::Current,
            10,
            RetrievalIntent::Lookup,
        )
        .unwrap();
    assert_eq!(
        grouped.association(&both),
        Some(FileAssociation::Referenced)
    );
    assert_eq!(
        grouped.association(&observed_only),
        Some(FileAssociation::Observed)
    );

    // Once, not twice: grouping by memory is what keeps a memory holding two
    // rows from spending the caller's limit on itself.
    let returned = grouped.invariants_and_constraints.len() + grouped.other.len();
    assert_eq!(returned, 2, "each memory comes back exactly once");
}

// ===========================================================================
// 1141 — the kind preference, inside the rung, only under CodeEdit.
// ===========================================================================

/// A `Finding` that outranks a `Constraint` on `retrieval_weight` alone.
/// Both sit on the same ladder rung, so the rung cannot be what separates
/// them and the kind preference is the only thing that can.
///
/// **Mutation target `kind-preference-dropped`**: make `CodeEdit` fall
/// through to `Lookup`'s comparison and the `CodeEdit` half fails while the
/// `Lookup` half keeps passing — which is the shape that says the preference,
/// and not something else, is what moved the row.
#[test]
fn code_edit_puts_a_constraint_ahead_of_a_finding_that_outweighs_it_and_lookup_does_not() {
    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path(), "alpha");
    let project = fixture.memory();
    let store = project.store();

    // Both `MemoryAuthority::Decision` and both current, so both land on
    // `LadderRung::CurrentDecision` and the rung cannot be what separates
    // them.
    //
    // The constraint is then made to weigh **less**: an unvalidated
    // `Prototype`-phase memory takes `policy::phase_penalty`'s exploratory
    // penalty, and `retrieval_weight` never sees a kind. So under `Lookup`
    // the finding wins on the number, and under `CodeEdit` the constraint has
    // to come first anyway — which is the claim, that the preference sits
    // *above* the weight rather than nudging it. A tie decided by the
    // candidate order would have proven neither, and would have depended on
    // two random ids.
    let constraint = store
        .record(
            NewMemory::new(
                MemoryKind::Constraint,
                "the loader must not mmap over 2 GiB",
            )
            .with_authority(Some(MemoryAuthority::Decision))
            .with_provenance(glasshouse::memory::DecisionProvenance {
                project_phase: Some(glasshouse::memory::ProjectPhase::Prototype),
                ..Default::default()
            }),
        )
        .unwrap()
        .id;
    let finding = store
        .record(
            NewMemory::new(MemoryKind::Finding, "the loader mmaps the index")
                .with_authority(Some(MemoryAuthority::Decision)),
        )
        .unwrap()
        .id;
    store
        .record_observed_files(
            &[constraint.clone(), finding.clone()],
            &["src/loader.rs".to_owned()],
        )
        .unwrap();

    let order = |intent| {
        let grouped = store
            .for_path("src/loader.rs", SearchScope::Current, 10, intent)
            .unwrap();
        grouped
            .invariants_and_constraints
            .iter()
            .chain(grouped.other.iter())
            .map(|record| record.id.clone())
            .collect::<Vec<_>>()
    };

    assert_eq!(
        order(RetrievalIntent::Lookup),
        vec![finding.clone(), constraint.clone()],
        "Lookup is today's order and this package must not have changed it"
    );
    assert_eq!(
        order(RetrievalIntent::CodeEdit),
        vec![constraint, finding],
        "for an intended edit, the constraint comes first"
    );
}

/// The rung stays primary — Phase 21E's rule that an idea never outranks an
/// invariant is not something `CodeEdit` may reach past.
///
/// A `Constraint`-kind memory that is **not current** sits on
/// `LadderRung::NotCurrent`, below a current `Finding`. Under `CodeEdit` the
/// preference would promote it if it acted across rungs; it must not.
#[test]
fn the_kind_preference_never_reaches_across_a_ladder_rung() {
    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path(), "alpha");
    let project = fixture.memory();
    let store = project.store();

    let superseded = store
        .record(NewMemory::new(
            MemoryKind::Constraint,
            "an old limit nobody works within any more",
        ))
        .unwrap()
        .id;
    let current = store
        .record(NewMemory::new(
            MemoryKind::Finding,
            "what the loader does today",
        ))
        .unwrap()
        .id;
    store
        .set_status(&superseded, glasshouse::memory::MemoryStatus::Superseded)
        .unwrap();
    store
        .record_observed_files(
            &[superseded.clone(), current.clone()],
            &["src/loader.rs".to_owned()],
        )
        .unwrap();

    let grouped = store
        .for_path(
            "src/loader.rs",
            SearchScope::Historical,
            10,
            RetrievalIntent::CodeEdit,
        )
        .unwrap();
    let order: Vec<_> = grouped
        .invariants_and_constraints
        .iter()
        .chain(grouped.other.iter())
        .map(|record| record.id.clone())
        .collect();
    assert_eq!(
        order,
        vec![current, superseded],
        "a not-current constraint stays below a current finding whatever the intent"
    );
}

// ===========================================================================
// 1142 — advisory, and freshness by commit order.
// ===========================================================================

/// The whole table, on a real repository with two commits.
///
/// **Mutation target `stale-never`**: make `Freshness::compare` always answer
/// `Current` and the stale row here fails.
#[test]
fn freshness_is_commit_order_and_unknown_is_not_current() {
    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path(), "alpha");
    let root = &fixture.root;

    let first = commit_file(root, "src/loader.rs", "fn one() {}\n", "one");
    let second = commit_file(root, "src/loader.rs", "fn two() {}\n", "two");
    // A file changed only in the first commit, so a memory from the second is
    // newer than the file's last change — the ordinary `current` case that is
    // *not* commit equality.
    let other = commit_file(root, "src/other.rs", "fn other() {}\n", "other");

    let loader_last = last_change_commit(root, "src/loader.rs").expect("git tracks this file");
    assert_eq!(
        loader_last, second,
        "the file's last change is the second commit"
    );

    // Stale: the memory's commit is a strict ancestor of the file's last
    // change.
    assert_eq!(
        Freshness::compare(root, Some(&loader_last), Some(&first)),
        Freshness::Stale
    );
    // Current, by equality.
    assert_eq!(
        Freshness::compare(root, Some(&loader_last), Some(&second)),
        Freshness::Current
    );
    // Current, by ancestry: the file's last change is an ancestor of the
    // memory's commit. This is the case a single `merge-base` in the other
    // direction would have reported as unknown.
    let other_last = last_change_commit(root, "src/other.rs").unwrap();
    assert_eq!(other_last, other);
    assert_eq!(
        Freshness::compare(root, Some(&first), Some(&other_last)),
        Freshness::Current
    );

    // Unknown, three ways, and none of them is `Current`.
    assert_eq!(
        Freshness::compare(root, Some(&loader_last), None),
        Freshness::Unknown,
        "a memory with no source commit"
    );
    assert_eq!(
        Freshness::compare(root, None, Some(&second)),
        Freshness::Unknown,
        "a path git has never tracked"
    );
    assert_eq!(
        last_change_commit(root, "src/never-existed.rs"),
        None,
        "git tracks nothing at that path, and an empty answer is None"
    );

    // `is_ancestor` itself, including the reflexive case the table leans on.
    assert_eq!(is_ancestor(root, &first, &second), Some(true));
    assert_eq!(is_ancestor(root, &second, &first), Some(false));
    assert_eq!(is_ancestor(root, &first, &first), Some(true));
    assert_eq!(
        is_ancestor(root, "0".repeat(40).as_str(), &second),
        None,
        "an unknown revision is a refusal to answer, never a `false`"
    );
}

/// Outside a repository, and with no `git` able to answer, everything is
/// `Unknown` — and nothing is withheld.
#[test]
fn no_repository_answers_unknown_rather_than_current() {
    let tmp = tempfile::tempdir().unwrap();
    let bare = tmp.path().join("not-a-repo");
    std::fs::create_dir_all(&bare).unwrap();

    assert_eq!(last_change_commit(&bare, "src/a.rs"), None);
    assert_eq!(
        Freshness::compare(&bare, Some(&"a".repeat(40)), Some(&"b".repeat(40))),
        Freshness::Unknown
    );
}

/// Map line 1142's own words: a stale memory is **shown, labelled, in its
/// rank** — never dropped, never moved.
///
/// **Mutation target `stale-withholds`**: drop a stale row from the section
/// and this fails. **Mutation target `stale-never`** fails it too, from the
/// other side: the label would read `current`.
#[test]
fn a_stale_memory_is_shown_in_its_rank_and_labelled_rather_than_withheld() {
    use glasshouse::memory::inject::briefing;
    use std::collections::HashSet;

    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path(), "alpha");
    let root = &fixture.root;

    let first = commit_file(root, "src/loader.rs", "fn one() {}\n", "one");
    commit_file(root, "src/loader.rs", "fn two() {}\n", "two");

    let project = fixture.memory();
    let store = project.store();
    // Extracted at the FIRST commit; the file changed in the second.
    let stale = store
        .record(
            NewMemory::new(
                MemoryKind::Finding,
                "walrus batching happens in threes, never singly",
            )
            .with_subject(Some("walrus batching"))
            .with_source_commit(Some(first.as_str())),
        )
        .unwrap()
        .id;
    store
        .record_observed_files(std::slice::from_ref(&stale), &["src/loader.rs".to_owned()])
        .unwrap();

    let injection = briefing(
        &store,
        "add a test for src/loader.rs",
        &HashSet::new(),
        None,
        Some(root.as_path()),
    )
    .unwrap()
    .into_injection()
    .expect("a task naming an associated path must inject something");

    let text = injection.text();
    assert!(
        injection.memories().contains(&stale),
        "the stale memory must still be delivered: {text}"
    );
    assert!(
        text.contains("freshness=stale"),
        "and it must say so on its own row: {text}"
    );
    assert!(
        text.contains("Advisory: the source at that path is the evidence"),
        "the section heading must carry map line 1142's own sentence: {text}"
    );
}

/// The same briefing with no project root: the section is identical except
/// that every row reads `unknown`. `None` is a supported answer, not a
/// degraded one, and it is what makes git optional for this whole feature.
#[test]
fn a_briefing_with_no_project_root_labels_every_row_unknown_and_withholds_nothing() {
    use glasshouse::memory::inject::briefing;
    use std::collections::HashSet;

    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path(), "alpha");
    let project = fixture.memory();
    let store = project.store();

    let memory = store
        .record(
            NewMemory::new(MemoryKind::Finding, "walrus batching happens in threes")
                .with_subject(Some("walrus batching")),
        )
        .unwrap()
        .id;
    store
        .record_observed_files(std::slice::from_ref(&memory), &["src/loader.rs".to_owned()])
        .unwrap();

    let injection = briefing(
        &store,
        "add a test for src/loader.rs",
        &HashSet::new(),
        None,
        None,
    )
    .unwrap()
    .into_injection()
    .expect("a task naming an associated path must inject something");

    assert!(injection.memories().contains(&memory));
    assert!(
        injection.text().contains("freshness=unknown"),
        "{}",
        injection.text()
    );
}

// ===========================================================================
// The CLI — `glasshouse memory search --path [--for-edit]`.
// ===========================================================================

/// The flag the 1143 evidence entry recorded as missing, driven through the
/// shipped binary: association, freshness and the advisory line per answer.
#[test]
fn memory_search_by_path_prints_the_association_the_freshness_and_the_advisory_line() {
    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path(), "alpha");
    let root = &fixture.root;
    let first = commit_file(root, "src/loader.rs", "fn one() {}\n", "one");
    commit_file(root, "src/loader.rs", "fn two() {}\n", "two");

    let _referenced = {
        let project = fixture.memory();
        let store = project.store();
        let id = store
            .record(
                NewMemory::new(MemoryKind::Finding, "walrus batching happens in threes")
                    .with_source_commit(Some(first.as_str())),
            )
            .unwrap()
            .id;
        store
            .record_referenced_files(std::slice::from_ref(&id), &["src/loader.rs".to_owned()])
            .unwrap();
        id
    };

    let output = fixture.run(&["memory", "search", "--path", "src/loader.rs"], b"");
    assert!(output.status.success(), "{output:?}");
    let text = String::from_utf8(output.stdout).unwrap();

    assert!(
        text.contains("assoc=referenced"),
        "the row must report the association it actually holds: {text}"
    );
    assert!(
        text.contains("freshness=stale"),
        "the file changed after the memory was recorded: {text}"
    );
    assert!(
        text.contains("advisory: the source at src/loader.rs is the evidence"),
        "map line 1142's own sentence must head the answer: {text}"
    );
    assert!(
        text.contains("walrus batching happens in threes"),
        "and the memory itself must be printed: {text}"
    );
}

/// `--for-edit` without `--path` is an error naming `--path`, rather than a
/// flag that silently did nothing.
#[test]
fn for_edit_without_a_path_is_an_error_that_names_the_flag_it_needs() {
    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path(), "alpha");

    let output = fixture.run(&["memory", "search", "--for-edit", "anything"], b"");
    assert!(
        !output.status.success(),
        "it must fail rather than run a text search: {output:?}"
    );
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("--path"), "{stderr}");
}

/// A path lookup with nothing to say names the path and says which question
/// was asked — the same distinction `render_memory_report` draws for a text
/// search, so "nothing" never reads as "this project remembers nothing".
#[test]
fn a_path_with_no_associations_says_so_without_claiming_the_project_is_empty() {
    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path(), "alpha");

    let output = fixture.run(&["memory", "search", "--path", "src/nothing.rs"], b"");
    assert!(output.status.success(), "{output:?}");
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(text.contains("src/nothing.rs"), "{text}");
    assert!(text.contains("--history"), "{text}");
}

/// The existing text search is byte-for-byte unchanged without `--path`.
#[test]
fn a_search_with_no_path_is_unchanged() {
    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path(), "alpha");
    {
        let project = fixture.memory();
        project
            .store()
            .record(NewMemory::new(
                MemoryKind::Finding,
                "walrus batching happens in threes",
            ))
            .unwrap();
    }

    let output = fixture.run(&["memory", "search", "walrus"], b"");
    assert!(output.status.success(), "{output:?}");
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(text.contains("walrus batching"), "{text}");
    assert!(
        !text.contains("assoc=") && !text.contains("freshness="),
        "a text search retrieved by no file and must claim neither: {text}"
    );
}

// ===========================================================================
// Migration 26 — a schema-25 database upgrades with every `seq` preserved.
// ===========================================================================

/// The stop condition this package was given: *do not ship a migration that
/// renumbers*.
///
/// Built by bootstrapping at 26, writing real events, then **rolling the
/// table back** to migration 7's shape (which is what a schema-25 database
/// holds) and setting `user_version` to 25 — the same fixture shape
/// `database.rs`'s own rollback tests use. Re-bootstrapping then runs
/// migration 26 for real, and every `seq` must come back unchanged.
#[test]
fn a_schema_25_database_migrates_in_place_with_every_seq_preserved() {
    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path(), "alpha");
    let session = SessionId::new("s-seq-1");

    let before: Vec<(i64, String)> = {
        let log = EventLog::open(&fixture.runtime).unwrap();
        let bus = glasshouse::events::EventBus::with_history(0);
        for event in [
            LifecycleEvent::SessionStarted,
            LifecycleEvent::TurnStarted,
            LifecycleEvent::TurnEnded {
                outcome: glasshouse::events::TurnOutcome::Completed,
            },
            LifecycleEvent::OutputEnded,
        ] {
            let recorded = bus.publish(&session, event);
            log.append(&recorded, None).unwrap();
        }
        log.all()
            .unwrap()
            .into_iter()
            .map(|logged| (logged.seq, logged.event.kind().to_owned()))
            .collect()
    };
    assert_eq!(before.len(), 4, "{before:?}");

    // A memory whose provenance range points at those events, so the claim
    // "provenance still resolves to the same events" is about real rows.
    let (memory_id, range) = {
        let project = fixture.memory();
        let store = project.store();
        let range =
            glasshouse::memory::SourceEvents::new(before[0].0, before[3].0).expect("a real range");
        let id = store
            .record(
                NewMemory::new(MemoryKind::Finding, "extracted from those four events")
                    .with_source_events(Some(range)),
            )
            .unwrap()
            .id;
        (id, range)
    };

    // Roll `lifecycle_events` back to migration 7's shape and claim 25.
    //
    // The version this build reads is the highest row in `schema_migrations`,
    // not `user_version` — so undoing a migration means deleting its row,
    // and the `pragma` a reader might reach for first does nothing at all.
    {
        let conn = rusqlite::Connection::open(fixture.runtime.database_path()).unwrap();
        conn.execute_batch(SCHEMA_25_LIFECYCLE_EVENTS).unwrap();
        conn.execute("DELETE FROM schema_migrations WHERE version = 26", [])
            .unwrap();
        let claimed: i64 = conn
            .query_row(
                "SELECT COALESCE(MAX(version), 0) FROM schema_migrations",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(claimed, 25, "the fixture must really claim to be a 25");
    }
    // Forward, through the real bootstrap — the way an upgrade actually
    // happens.
    let cli = Cli::try_parse_from([
        "glasshouse",
        "--data-dir",
        tmp.path().join("data").to_str().unwrap(),
        "--config-dir",
        tmp.path().join("config").to_str().unwrap(),
    ])
    .unwrap();
    let migrated = glasshouse::bootstrap(&cli, &fixture.root).unwrap();

    {
        let conn = rusqlite::Connection::open(migrated.database_path()).unwrap();
        let columns: Vec<String> = conn
            .prepare("SELECT name FROM pragma_table_info('lifecycle_events')")
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert!(
            columns.contains(&"path".to_owned()),
            "migration 26 must have run: {columns:?}"
        );
    }
    let after: Vec<(i64, String)> = {
        let log = EventLog::open(&migrated).unwrap();
        log.all()
            .unwrap()
            .into_iter()
            .map(|logged| (logged.seq, logged.event.kind().to_owned()))
            .collect()
    };
    assert_eq!(
        before, after,
        "migration 26 must preserve every seq and every kind"
    );

    let project = ProjectMemory::open(&migrated).unwrap();
    let store = project.store();
    let record = store.get(&memory_id).unwrap().expect("the memory survives");
    assert_eq!(
        record.source_events,
        Some(range),
        "a memory's provenance range must still name the same events"
    );

    // And the new kind is admitted after the upgrade, which is what the
    // migration was for.
    {
        let log = EventLog::open(&migrated).unwrap();
        let bus = glasshouse::events::EventBus::with_history(0);
        let recorded = bus.publish(
            &session,
            LifecycleEvent::FileTouched {
                path: "src/after.rs".to_owned(),
            },
        );
        log.append(&recorded, None).unwrap();
        let seqs: Vec<i64> = log.all().unwrap().into_iter().map(|l| l.seq).collect();
        assert!(
            seqs.last().copied().unwrap() > before[3].0,
            "the next event continues from the old high-water mark rather than \
             restarting at it: {seqs:?}"
        );
    }
}

/// `lifecycle_events` exactly as migration 7 left it, which is what a
/// schema-25 database holds. Written out rather than derived so the fixture
/// cannot drift with the table it is pretending to be older than.
const SCHEMA_25_LIFECYCLE_EVENTS: &str = "
    CREATE TABLE lifecycle_events_old (
        seq              INTEGER PRIMARY KEY AUTOINCREMENT,
        project_id       TEXT    NOT NULL,
        session_id       TEXT    NOT NULL,
        at               INTEGER NOT NULL,
        kind             TEXT    NOT NULL
            CHECK (kind IN ('session_started', 'session_resumed',
                            'turn_started', 'turn_ended',
                            'waiting_for_user', 'text_delivered',
                            'interrupt_delivered', 'process_exited',
                            'output_ended', 'gateway_unhealthy',
                            'gateway_backend_changed')),
        turn_outcome     TEXT
            CHECK (turn_outcome IS NULL OR
                   turn_outcome IN ('completed', 'failed')),
        origin           TEXT
            CHECK (origin IS NULL OR
                   origin IN ('user_keystroke', 'machine')),
        bytes            INTEGER,
        exit_code        INTEGER,
        exit_signal      TEXT,
        resource         TEXT,
        gateway_reason   TEXT
            CHECK (gateway_reason IS NULL OR
                   gateway_reason IN ('unreachable', 'timed_out', 'rejected')),
        gateway_provider TEXT,
        gateway_model    TEXT,
        gateway_cause    TEXT,
        observed_harness TEXT,
        observed_event   TEXT,
        CHECK ((observed_harness IS NULL) = (observed_event IS NULL))
    );
    INSERT INTO lifecycle_events_old (
        seq, project_id, session_id, at, kind,
        turn_outcome, origin, bytes, exit_code, exit_signal,
        resource, gateway_reason, gateway_provider, gateway_model,
        gateway_cause, observed_harness, observed_event
    )
    SELECT
        seq, project_id, session_id, at, kind,
        turn_outcome, origin, bytes, exit_code, exit_signal,
        resource, gateway_reason, gateway_provider, gateway_model,
        gateway_cause, observed_harness, observed_event
    FROM lifecycle_events;
    DROP TABLE lifecycle_events;
    ALTER TABLE lifecycle_events_old RENAME TO lifecycle_events;
    CREATE INDEX lifecycle_events_by_session
        ON lifecycle_events (session_id, seq);
    CREATE TRIGGER lifecycle_events_reject_foreign_project_insert
    BEFORE INSERT ON lifecycle_events
    FOR EACH ROW
    WHEN NEW.project_id IS NOT (
        SELECT value FROM project_metadata WHERE key = 'project_id'
    )
    BEGIN
        SELECT RAISE(ABORT, 'event belongs to a different project');
    END;
    CREATE TRIGGER lifecycle_events_are_append_only_update
    BEFORE UPDATE ON lifecycle_events
    FOR EACH ROW
    BEGIN
        SELECT RAISE(ABORT, 'the project event log is append-only');
    END;
    CREATE TRIGGER lifecycle_events_are_append_only_delete
    BEFORE DELETE ON lifecycle_events
    FOR EACH ROW
    BEGIN
        SELECT RAISE(ABORT, 'the project event log is append-only');
    END;
";
