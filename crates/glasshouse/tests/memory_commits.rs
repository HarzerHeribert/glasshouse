//! Phase 29 — **memory commits**, driven through the built binary.
//!
//! # What this file has to show, and why a unit test cannot
//!
//! A memory commit is not a new mechanism; it is Phase 21's extraction with a
//! trigger, and map lines 1147-1154 are almost entirely about **who starts
//! one and what gets recorded about it**. Every one of those facts lives on
//! the production path: `glasshouse memory commit` in a person's shell,
//! `glasshouse hook` spawned by a harness, HEAD read out of a real `.git`
//! directory, and the trigger written to a column a real query reads back.
//! A test that constructed an `Extractor` and handed it a trigger would prove
//! the extractor stores what it is told and nothing about whether anything
//! tells it — practice §35's shape exactly.
//!
//! So this spawns the binary, stands up a socket, and writes a real
//! repository, on the criterion `docs/product/evidence/phase-21.md` set for
//! the trigger lines: *"the test is not whether a model is called. It is
//! whether the capability completes and produces its result in the shipped
//! binary."*
//!
//! The canned model server is `memory_extract_triggers.rs`'s, deliberately
//! duplicated rather than shared: these two files assert about different
//! capabilities and a shared fixture is the thing §35 warns about — the
//! helper that makes a test convenient is the helper that reproduces the
//! production step being proven.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use glasshouse::config::{ExtractionModelRef, ProviderConfig, UserConfig};
use glasshouse::memory::ProjectMemory;
use glasshouse::memory::search::SearchScope;
use glasshouse::session::{NewSession, ProjectSessions, SessionId, SessionLifecycle};
use glasshouse::{Cli, Runtime};

use clap::Parser;

const CREDENTIAL_VAR: &str = "GLASSHOUSE_TEST_ONLY_MEMORY_COMMIT_MODEL_KEY";
const CREDENTIAL: &str = "sk-fabricated-test-value-not-a-real-credential";
const MODEL: &str = "a-cheap-local-model";
const PROVIDER: &str = "memory-commit-test-runner";

/// Where the fixture repository's HEAD starts, and where it moves to. Full
/// forty-character object names, because that is what a repository holds and
/// what `GitPosition::detect` refuses anything else in place of.
const FIRST_COMMIT: &str = "0123456789abcdef0123456789abcdef01234567";
const SECOND_COMMIT: &str = "fedcba9876543210fedcba9876543210fedcba98";

/// What a cheap model answers. One finding, in the extraction contract's own
/// shape, with a body no other test in this repository stores.
const ONE_FINDING: &str = r#"{"memories":[{"kind":"finding","authority":"constraint",
     "disposition":"accepted","support":"established","confidence":"certain",
     "rationale":"the hook process is the only thing that sees a turn end",
     "project_phase":"alpha",
     "body":"A memory commit reached this project's store."}]}"#;

/// A second, different finding, for the runs that must produce a *new*
/// memory rather than a duplicate of the one before it.
const ANOTHER_FINDING: &str = r#"{"memories":[{"kind":"finding","authority":"constraint",
     "disposition":"accepted","support":"established","confidence":"certain",
     "rationale":"a landed commit is a boundary the hook can see",
     "project_phase":"alpha",
     "body":"A commit landing put this in the project's store."}]}"#;

// ---------------------------------------------------------------------------
// A canned OpenAI chat-completions endpoint.
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
struct Seen {
    body: String,
}

struct FakeModel {
    address: SocketAddr,
    seen: Arc<Mutex<Vec<Seen>>>,
    stop: Arc<AtomicBool>,
}

impl FakeModel {
    /// A model that answers `content` to every request.
    fn answering(content: &str) -> Self {
        Self::answering_each(&[content])
    }

    /// A model whose *n*th request gets the *n*th answer, the last one
    /// repeating once the list runs out.
    ///
    /// Needed because the duplicate check is real: a model that answers the
    /// same finding twice produces one memory, which is line 1154 working and
    /// is exactly what stops a test from *observing* the second run's
    /// trigger. Distinct answers are what a real model gives two different
    /// slices of a session, and they are what makes both memories visible.
    fn answering_each(contents: &[&str]) -> Self {
        let contents: Vec<String> = contents.iter().map(|text| (*text).to_owned()).collect();
        assert!(!contents.is_empty(), "a model must have something to say");
        let listener = TcpListener::bind("127.0.0.1:0").expect("loopback must bind");
        listener
            .set_nonblocking(true)
            .expect("the accept loop polls its stop flag");
        let address = listener.local_addr().unwrap();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let stop = Arc::new(AtomicBool::new(false));

        let thread_seen = Arc::clone(&seen);
        let thread_stop = Arc::clone(&stop);
        std::thread::spawn(move || {
            while !thread_stop.load(Ordering::SeqCst) {
                match listener.accept() {
                    Ok((stream, _)) => {
                        let _ = stream.set_nonblocking(false);
                        let answered = thread_seen.lock().unwrap().len();
                        let content = contents[answered.min(contents.len() - 1)].clone();
                        serve(stream, &thread_seen, &content);
                    }
                    Err(ref err) if err.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(std::time::Duration::from_millis(5));
                    }
                    Err(_) => break,
                }
            }
        });

        Self {
            address,
            seen,
            stop,
        }
    }

    fn base_url(&self) -> String {
        format!("http://{}/v1", self.address)
    }

    fn requests(&self) -> Vec<Seen> {
        self.seen.lock().unwrap().clone()
    }
}

impl Drop for FakeModel {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
    }
}

/// Read one request head byte-oriented, find `content-length` without help,
/// read exactly that many bytes, and answer.
fn serve(mut stream: TcpStream, seen: &Arc<Mutex<Vec<Seen>>>, content: &str) {
    let mut reader = BufReader::new(stream.try_clone().expect("the stream clones"));
    let mut request_line = String::new();
    if reader.read_line(&mut request_line).is_err() || request_line.is_empty() {
        return;
    }

    let mut length = 0usize;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).is_err() {
            return;
        }
        let line = line.trim_end_matches(['\r', '\n']);
        if line.is_empty() {
            break;
        }
        if let Some((name, value)) = line.split_once(':')
            && name.trim().eq_ignore_ascii_case("content-length")
        {
            length = value.trim().parse().unwrap_or(0);
        }
    }

    let mut body = vec![0u8; length];
    if reader.read_exact(&mut body).is_err() {
        return;
    }
    seen.lock().unwrap().push(Seen {
        body: String::from_utf8_lossy(&body).into_owned(),
    });

    let document = serde_json::json!({
        "choices": [{ "message": { "role": "assistant", "content": content } }]
    })
    .to_string();
    let response = format!(
        "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\
         connection: close\r\n\r\n{document}",
        document.len()
    );
    let _ = stream.write_all(response.as_bytes());
    let _ = stream.flush();
}

// ---------------------------------------------------------------------------
// A project that is a real repository, and the binary run against it.
// ---------------------------------------------------------------------------

struct Fixture {
    _tmp: tempfile::TempDir,
    base: PathBuf,
    root: PathBuf,
    runtime: Runtime,
}

impl Fixture {
    /// A project whose `.git` is one `checkpoint::git` can actually read: a
    /// `HEAD` pointing at a branch, and a loose ref holding an object name.
    ///
    /// Deliberately a real on-disk repository rather than an injected
    /// position. `GitPosition::detect` opens these exact two files, and a
    /// seam here would let the HEAD comparison be proven against a value no
    /// repository ever produced.
    fn new() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path().to_path_buf();
        let root = base.join("workspace");
        std::fs::create_dir_all(root.join(".git/refs/heads")).unwrap();
        std::fs::write(root.join(".git/HEAD"), "ref: refs/heads/main\n").unwrap();
        let root = std::fs::canonicalize(&root).unwrap();
        let fixture = Self {
            _tmp: tmp,
            base: base.clone(),
            root: root.clone(),
            runtime: bootstrap(&base, &root),
        };
        fixture.move_head_to(FIRST_COMMIT);
        fixture
    }

    /// The same project with **no** repository at all, for the discriminating
    /// half of the boundary tests.
    fn without_a_repository() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path().to_path_buf();
        let root = base.join("workspace");
        std::fs::create_dir_all(&root).unwrap();
        let root = std::fs::canonicalize(&root).unwrap();
        let runtime = bootstrap(&base, &root);
        Self {
            _tmp: tmp,
            base,
            root,
            runtime,
        }
    }

    /// Land a commit: rewrite the branch ref, exactly as Git does.
    fn move_head_to(&self, commit: &str) {
        std::fs::write(
            self.root.join(".git/refs/heads/main"),
            format!("{commit}\n"),
        )
        .unwrap();
    }

    fn choose_model(&self, base_url: &str) {
        let mut user = UserConfig::load(self.runtime.paths()).unwrap();
        let mut provider = ProviderConfig::new("openai-compatible");
        provider.set_base_url(Some(base_url.to_owned()));
        provider.set_credential_env(vec![CREDENTIAL_VAR.to_owned()]);
        user.providers_mut().set(PROVIDER, provider);
        user.set_memory_extraction_model(Some(ExtractionModelRef::new(PROVIDER, MODEL)));
        user.save(self.runtime.paths()).unwrap();
    }

    /// `glasshouse <args...>`, run the way a person runs it, returning its
    /// standard output. Panics with the binary's own stderr on a non-zero
    /// exit, so a broken command reports what it said rather than a bare
    /// status.
    fn run(&self, args: &[&str]) -> String {
        let output = self
            .command(args)
            .output()
            .expect("the glasshouse binary must be runnable");
        assert!(
            output.status.success(),
            "`glasshouse {}` failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).into_owned()
    }

    fn command(&self, args: &[&str]) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_glasshouse"));
        command
            .env(CREDENTIAL_VAR, CREDENTIAL)
            .arg("--scope")
            .arg(&self.root)
            .arg("--data-dir")
            .arg(self.base.join("data"))
            .arg("--config-dir")
            .arg(self.base.join("config"))
            .args(args);
        command
    }

    /// Run `glasshouse hook`, exactly as a harness runs it.
    fn hook(&self, session: &SessionId, event: &str) {
        let mut child = self
            .command(&["hook", "--session", session.as_str(), "--event", event])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("the glasshouse binary must be runnable");
        child
            .stdin
            .as_mut()
            .expect("stdin was piped")
            .write_all(PAYLOAD.as_bytes())
            .expect("the handler must read its payload rather than closing the pipe");
        let output = child.wait_with_output().expect("the hook must finish");
        assert!(
            output.status.success(),
            "a hook must exit zero whatever extraction did: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn memories(&self) -> Vec<glasshouse::memory::MemoryRecord> {
        ProjectMemory::open(&self.runtime)
            .unwrap()
            .store()
            .search("store", SearchScope::Current, 20)
            .unwrap()
    }

    fn checkpoint_count(&self) -> i64 {
        let conn = rusqlite::Connection::open(self.runtime.database_path()).unwrap();
        conn.query_row("SELECT COUNT(*) FROM checkpoints", [], |row| row.get(0))
            .unwrap()
    }

    fn memory_count(&self) -> i64 {
        let conn = rusqlite::Connection::open(self.runtime.database_path()).unwrap();
        conn.query_row("SELECT COUNT(*) FROM memories", [], |row| row.get(0))
            .unwrap()
    }

    fn last_seen_commit(&self, id: &SessionId) -> Option<String> {
        ProjectSessions::open(&self.runtime)
            .unwrap()
            .store()
            .get(id)
            .unwrap()
            .expect("the session is in this project")
            .last_seen_commit
    }
}

const PAYLOAD: &str = concat!(
    r#"{"session_id":"native-1","transcript_path":"/somewhere/rollout.jsonl","#,
    r#""hook_event_name":"Stop","cwd":"/somewhere","model":"a-model"}"#
);

fn bootstrap(base: &Path, root: &Path) -> Runtime {
    let cli = Cli::try_parse_from([
        "glasshouse",
        "--data-dir",
        base.join("data").to_str().unwrap(),
        "--config-dir",
        base.join("config").to_str().unwrap(),
    ])
    .unwrap();
    glasshouse::bootstrap(&cli, root).unwrap()
}

fn running_session(fixture: &Fixture, harness: &str) -> SessionId {
    let sessions = ProjectSessions::open(&fixture.runtime).unwrap();
    let store = sessions.store();
    let record = store.create(NewSession::embedded(harness)).unwrap();
    store
        .set_lifecycle(&record.id, SessionLifecycle::Running)
        .unwrap();
    record.id
}

/// Give the session something to extract from: one recorded turn.
fn one_recorded_turn(fixture: &Fixture, id: &SessionId) {
    fixture.hook(id, "UserPromptSubmit");
}

// ---------------------------------------------------------------------------
// Line 1148 — a memory commit a person asks for — and line 1154 — asking
// twice.
// ---------------------------------------------------------------------------

/// **Lines 1147, 1148 and 1154.**
///
/// `glasshouse memory commit` runs the extraction pipeline over what the
/// session has done, stamps `manual` on what it stores, and a second run over
/// the same work stores nothing new.
///
/// The second half is line 1154's whole content, and it is asserted as a
/// **count that does not grow** rather than as a `duplicates` number, because
/// the map's phrase is *"does not create uncontrolled duplicate knowledge"* —
/// a claim about the store, not about the report. The model is asked twice
/// and answers the same thing twice, which is the case that would produce the
/// duplicate if nothing stopped it.
#[test]
fn a_manual_commit_extracts_and_a_second_run_adds_nothing() {
    let model = FakeModel::answering(ONE_FINDING);
    let fixture = Fixture::new();
    fixture.choose_model(&model.base_url());
    let id = running_session(&fixture, "claude-code");
    one_recorded_turn(&fixture, &id);

    let first = fixture.run(&["memory", "commit", "--session", id.as_str()]);
    assert!(
        first.contains("trigger manual"),
        "a commit a person asked for is a manual one: {first}"
    );
    assert!(first.contains("stored 1"), "{first}");

    let asked = model.requests();
    assert_eq!(
        asked.len(),
        1,
        "a manual commit asks the model exactly once"
    );
    assert!(
        asked[0].body.contains(MODEL),
        "the request must name the model the user chose: {}",
        asked[0].body
    );

    let stored = fixture.memories();
    assert_eq!(stored.len(), 1, "the memory must reach the project's store");
    assert_eq!(
        stored[0].extraction_trigger.as_deref(),
        Some("manual"),
        "every trigger names itself on the memory it produced"
    );
    assert_eq!(stored[0].source_session_id.as_deref(), Some(id.as_str()));

    // Again, over the same work, against a model that answers the same thing.
    let second = fixture.run(&["memory", "commit", "--session", id.as_str()]);
    assert!(
        second.contains("stored 0, 1 duplicate"),
        "a second commit over the same work stores nothing: {second}"
    );
    assert_eq!(
        model.requests().len(),
        2,
        "the second run really did ask the model again — the count below is \
         not an artefact of nothing having happened"
    );
    assert_eq!(
        fixture.memory_count(),
        1,
        "rerunning a memory commit must not grow the project's knowledge"
    );
}

/// The default: no `--session` commits the most recently active one.
#[test]
fn a_commit_with_no_session_named_takes_the_most_recently_active_one() {
    let model = FakeModel::answering(ONE_FINDING);
    let fixture = Fixture::new();
    fixture.choose_model(&model.base_url());

    let older = running_session(&fixture, "claude-code");
    one_recorded_turn(&fixture, &older);
    let newer = running_session(&fixture, "codex");
    one_recorded_turn(&fixture, &newer);

    // Both sessions were touched inside the same second, and
    // `last_activity_at` has second resolution — so without this the two
    // stamps tie and `SessionStore::list` falls back to `id ASC`, which is a
    // random identifier. Age the older one deliberately, which is the
    // precondition the assertion below is actually about.
    {
        let conn = rusqlite::Connection::open(fixture.runtime.database_path()).unwrap();
        conn.execute(
            "UPDATE sessions SET last_activity_at = last_activity_at - 3600 WHERE id = ?1",
            [older.as_str()],
        )
        .unwrap();
    }

    let report = fixture.run(&["memory", "commit"]);
    assert!(
        report.contains(&format!("session: {newer}")),
        "the default is the session the person was just in: {report}"
    );
    assert!(
        !report.contains(older.as_str()),
        "the older session must not be the one committed: {report}"
    );
}

// ---------------------------------------------------------------------------
// Lines 1149 and 1153 — a commit landing is a code-change boundary.
// ---------------------------------------------------------------------------

/// **Lines 1149 and 1153.**
///
/// A turn ends with HEAD somewhere new: the extraction that runs is a
/// `git_commit` one, the memory it produces carries **that commit**, and the
/// session remembers the position so the next turn is not a boundary too.
///
/// Nothing here installs a Git hook, and that is the point of driving it this
/// way: the boundary is detected by the harness hook Glasshouse already
/// receives, reading a repository nobody told it about.
#[test]
fn a_new_head_at_turn_end_is_a_code_change_boundary_and_the_commit_is_recorded_on_the_memory() {
    // Two different answers: the duplicate check is real, and a model
    // repeating itself would collapse the second memory into the first —
    // line 1154 working, and this test unable to see line 1153.
    let model = FakeModel::answering_each(&[ONE_FINDING, ANOTHER_FINDING]);
    let fixture = Fixture::new();
    fixture.choose_model(&model.base_url());
    let id = running_session(&fixture, "claude-code");

    // The first turn: HEAD is read for the first time. A position is learned,
    // and it is not a boundary — there is nothing to have changed from.
    fixture.hook(&id, "UserPromptSubmit");
    assert_eq!(
        fixture.last_seen_commit(&id).as_deref(),
        None,
        "a turn that has not ended has not looked at HEAD"
    );
    fixture.hook(&id, "Stop");
    assert_eq!(
        fixture.last_seen_commit(&id).as_deref(),
        Some(FIRST_COMMIT),
        "the first completed turn records where HEAD stood"
    );
    let after_first = fixture.memories();
    assert_eq!(after_first.len(), 1);
    assert_eq!(
        after_first[0].extraction_trigger.as_deref(),
        Some("task_completed"),
        "a first sighting of HEAD is not a commit landing"
    );

    // A commit lands, and another turn ends.
    fixture.move_head_to(SECOND_COMMIT);
    fixture.hook(&id, "Stop");

    assert_eq!(
        fixture.last_seen_commit(&id).as_deref(),
        Some(SECOND_COMMIT),
        "the new position must be stored, or every later turn repeats this boundary"
    );
    let boundary: Vec<_> = fixture
        .memories()
        .into_iter()
        .filter(|record| record.extraction_trigger.as_deref() == Some("git_commit"))
        .collect();
    assert_eq!(
        boundary.len(),
        1,
        "a landed commit must produce a memory the trigger names"
    );
    assert_eq!(
        boundary[0].source_commit.as_deref(),
        Some(SECOND_COMMIT),
        "line 1153: the memory carries the commit that made the boundary, \
         not the one before it"
    );

    // And the person can see it.
    let listed = fixture.run(&["memory", "search", "landing"]);
    assert!(
        listed.contains("trigger git_commit"),
        "a memory's trigger must name itself where a person reads it: {listed}"
    );
    assert!(
        listed.contains(SECOND_COMMIT),
        "and the commit beside it: {listed}"
    );
}

/// The discriminating half of line 1149, in both of its forms.
///
/// A turn that ends with HEAD exactly where it was runs the ordinary
/// task-completion extraction and **no** Git one; and a project that is not a
/// repository at all behaves exactly as it did before this existed. Without
/// this, "a commit landed" would be satisfied by "a turn ended", which is
/// every turn.
#[test]
fn an_unchanged_head_triggers_only_the_task_completion_extraction() {
    let model = FakeModel::answering(ONE_FINDING);
    let fixture = Fixture::new();
    fixture.choose_model(&model.base_url());
    let id = running_session(&fixture, "claude-code");

    // Two completed turns, and HEAD never moves.
    fixture.hook(&id, "UserPromptSubmit");
    fixture.hook(&id, "Stop");
    fixture.hook(&id, "UserPromptSubmit");
    fixture.hook(&id, "Stop");

    assert_eq!(
        model.requests().len(),
        2,
        "both turns really did run an extraction"
    );
    let triggers: Vec<_> = fixture
        .memories()
        .into_iter()
        .filter_map(|record| record.extraction_trigger)
        .collect();
    assert!(
        !triggers.is_empty() && triggers.iter().all(|trigger| trigger == "task_completed"),
        "HEAD did not move, so nothing may claim a code-change boundary: {triggers:?}"
    );
    assert_eq!(fixture.last_seen_commit(&id).as_deref(), Some(FIRST_COMMIT));

    // And a project with no repository at all.
    let model = FakeModel::answering(ONE_FINDING);
    let bare = Fixture::without_a_repository();
    bare.choose_model(&model.base_url());
    let id = running_session(&bare, "claude-code");
    bare.hook(&id, "UserPromptSubmit");
    bare.hook(&id, "Stop");

    let stored = bare.memories();
    assert_eq!(
        stored.len(),
        1,
        "extraction still runs without a repository"
    );
    assert_eq!(
        stored[0].extraction_trigger.as_deref(),
        Some("task_completed")
    );
    assert_eq!(
        stored[0].source_commit, None,
        "a project with no repository has no commit to record"
    );
    assert_eq!(
        bare.last_seen_commit(&id),
        None,
        "and no position to remember"
    );
}

// ---------------------------------------------------------------------------
// Line 1151 — before an intentional native compaction.
// ---------------------------------------------------------------------------

/// **Line 1151, and its discriminating half.**
///
/// Codex's `PreCompact` — the harness saying it is *about to* compact — runs
/// a memory commit and the memory says so. `PostCompact`, which Glasshouse
/// also asks for and also receives, runs nothing: the line says *before*, the
/// event log a compaction does not change is the material extraction reads,
/// and running on both would be two commits over identical material inside
/// the user's session.
///
/// Claude Code has no compaction event at all
/// (`session::lifecycle::event_for`), so for that harness this line is
/// answered by the harness having nothing to say, not by Glasshouse.
#[test]
fn the_pre_compaction_event_triggers_extraction_and_the_post_event_does_not() {
    let model = FakeModel::answering(ONE_FINDING);
    let fixture = Fixture::new();
    fixture.choose_model(&model.base_url());
    let id = running_session(&fixture, "codex");
    one_recorded_turn(&fixture, &id);

    fixture.hook(&id, "PreCompact");
    assert_eq!(
        model.requests().len(),
        1,
        "a harness about to compact commits"
    );
    let stored = fixture.memories();
    assert_eq!(stored.len(), 1);
    assert_eq!(
        stored[0].extraction_trigger.as_deref(),
        Some("before_compaction"),
        "the memory names the trigger that produced it"
    );

    fixture.hook(&id, "PostCompact");
    assert_eq!(
        model.requests().len(),
        1,
        "the post-compaction event must run nothing"
    );
    assert_eq!(fixture.memory_count(), 1, "and must store nothing");
}

// ---------------------------------------------------------------------------
// Line 1152 — two stores, neither writing into the other.
// ---------------------------------------------------------------------------

/// **Line 1152** — *"separate durable project memories from transient session
/// checkpoints during a memory commit."*
///
/// Asserted in both directions and by counting rows in the two tables, rather
/// than by reading the code that writes them: a memory commit leaves the
/// checkpoint table exactly as it found it, and taking a checkpoint leaves
/// the memory table exactly as it found it. The two are separate stores with
/// separate lifetimes — a checkpoint is a session's transient handoff and is
/// pruned by sequence, a memory is durable project knowledge that outlives
/// the session — and this is the assertion that they stay that way.
#[test]
fn memories_and_checkpoints_never_write_into_each_other() {
    let model = FakeModel::answering(ONE_FINDING);
    let fixture = Fixture::new();
    fixture.choose_model(&model.base_url());
    let id = running_session(&fixture, "claude-code");
    one_recorded_turn(&fixture, &id);

    assert_eq!(fixture.checkpoint_count(), 0);
    assert_eq!(fixture.memory_count(), 0);

    // A memory commit writes a memory and no checkpoint.
    fixture.run(&["memory", "commit", "--session", id.as_str()]);
    assert_eq!(fixture.memory_count(), 1, "the commit stored its memory");
    assert_eq!(
        fixture.checkpoint_count(),
        0,
        "a memory commit must not write a checkpoint row"
    );

    // A checkpoint writes a checkpoint and no memory.
    fixture.run(&[
        "checkpoint",
        "save",
        "--session",
        id.as_str(),
        "--objective",
        "prove the two stores stay apart",
        "--state",
        "one memory recorded, no checkpoints yet",
        "--next",
        "take this checkpoint",
    ]);
    assert_eq!(
        fixture.checkpoint_count(),
        1,
        "the checkpoint reached its own store"
    );
    assert_eq!(
        fixture.memory_count(),
        1,
        "a checkpoint must not write a memory row"
    );
}
