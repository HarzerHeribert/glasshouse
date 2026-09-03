use super::*;
use crate::harness::antigravity::Antigravity;
use crate::harness::codex::Codex;
use std::time::Duration;

/// Epoch seconds for `2026-06-15T12:00:00Z`, independently computed —
/// the window's lower bound (`window()` below is `[at(0), at(3600)]`,
/// i.e. through `2026-06-15T13:00:00Z`). Every fixture timestamp is this
/// plus or minus a plain second offset, so the RFC3339 strings baked into
/// fixture headers (parsed by the real `Codex` adapter under test, never
/// by anything in this test module) and the `SystemTime` window bounds
/// passed to `discover` cannot drift apart from each other.
const BASE_EPOCH: i64 = 1_781_524_800;
const IN_WINDOW: &str = "2026-06-15T12:30:00Z";
const BEFORE_WINDOW: &str = "2026-06-15T11:00:00Z";

fn at(offset_seconds: i64) -> SystemTime {
    let seconds = u64::try_from(BASE_EPOCH + offset_seconds).expect("a real test timestamp");
    SystemTime::UNIX_EPOCH + Duration::from_secs(seconds)
}

/// `(started_at, ended_at)`: `BASE_EPOCH .. BASE_EPOCH + 3600`, i.e.
/// `2026-06-15T12:00:00Z..=2026-06-15T13:00:00Z`.
fn window() -> (SystemTime, SystemTime) {
    (at(0), at(3600))
}

/// A real project directory and a real records root, both empty, ready
/// for fixtures. Both must exist on disk: `discover` canonicalizes a
/// candidate's `cwd` before comparing it against `project_root`, so a
/// fabricated, nonexistent path would never match anything.
struct Fixture {
    tmp: tempfile::TempDir,
    records_root: PathBuf,
    project_root: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let tmp = tempfile::tempdir().expect("tempdir");
        let records_root = tmp.path().join("sessions");
        std::fs::create_dir_all(&records_root).expect("create records root");
        let project_root = tmp.path().join("project");
        std::fs::create_dir_all(&project_root).expect("create project root");
        Self {
            tmp,
            records_root,
            project_root,
        }
    }

    fn other_dir(&self, name: &str) -> PathBuf {
        let dir = self.tmp.path().join(name);
        std::fs::create_dir_all(&dir).expect("create directory");
        dir
    }
}

fn json_escape_path(path: &Path) -> String {
    path.display().to_string().replace('\\', "\\\\")
}

/// A real rollout-shaped `session_meta` header, with the field names read
/// directly off a real Codex install — see `harness::codex`'s evidence.
fn header(
    id_field: &str,
    id: &str,
    cwd: &Path,
    timestamp: &str,
    originator: &str,
    parent_thread_id: Option<&str>,
) -> String {
    let mut payload = format!(
        r#""{id_field}":"{id}","cwd":"{cwd}","timestamp":"{timestamp}","originator":"{originator}""#,
        cwd = json_escape_path(cwd),
    );
    if let Some(parent) = parent_thread_id {
        payload.push_str(&format!(r#","parent_thread_id":"{parent}""#));
    }
    format!(r#"{{"type":"session_meta","payload":{{{payload}}}}}"#)
}

fn write_rollout(dir: &Path, name: &str, header_line: &str, trailer: &[u8]) -> PathBuf {
    let path = dir.join(name);
    let mut contents = header_line.as_bytes().to_vec();
    contents.push(b'\n');
    contents.extend_from_slice(trailer);
    std::fs::write(&path, &contents).expect("write fixture");
    path
}

#[test]
fn the_interactive_session_in_the_window_is_the_one_captured() {
    let fixture = Fixture::new();
    write_rollout(
        &fixture.records_root,
        "rollout-subagent.jsonl",
        &header(
            "id",
            "subagent-id",
            &fixture.project_root,
            IN_WINDOW,
            "codex-tui",
            Some("parent-thread-1"),
        ),
        b"",
    );
    write_rollout(
        &fixture.records_root,
        "rollout-desktop.jsonl",
        &header(
            "id",
            "desktop-id",
            &fixture.project_root,
            IN_WINDOW,
            "Codex Desktop",
            None,
        ),
        b"",
    );
    write_rollout(
        &fixture.records_root,
        "rollout-real.jsonl",
        &header(
            "id",
            "real-id",
            &fixture.project_root,
            IN_WINDOW,
            "codex-tui",
            None,
        ),
        b"",
    );

    let (started_at, ended_at) = window();
    let canonical_root = std::fs::canonicalize(&fixture.project_root).expect("canonicalize");
    let result = discover(
        &Codex,
        &fixture.records_root,
        &canonical_root,
        started_at,
        ended_at,
        // Codex is a `RecordPerSession` source: the shared-index
        // snapshot is not part of its identity guard and it must not
        // start being one.
        &IndexSnapshot::default(),
    );
    assert_eq!(result, Discovered::Found("real-id".to_owned()));
}

/// A rollout whose only line is its header, with nothing after it — not
/// even a newline. Every other fixture in this module ends in one, which
/// is why requiring it passed eight tests here and then failed on Windows
/// CI alone, where the fake harness wrote without a trailing newline.
#[test]
fn a_header_with_no_trailing_newline_is_still_read() {
    let fixture = Fixture::new();
    let line = header(
        "id",
        "eof-id",
        &fixture.project_root,
        IN_WINDOW,
        "codex-tui",
        None,
    );
    std::fs::write(
        fixture.records_root.join("rollout-eof.jsonl"),
        line.as_bytes(),
    )
    .expect("write fixture with no trailing newline");

    let (started_at, ended_at) = window();
    let canonical_root = std::fs::canonicalize(&fixture.project_root).expect("canonicalize");
    let result = discover(
        &Codex,
        &fixture.records_root,
        &canonical_root,
        started_at,
        ended_at,
        // Codex is a `RecordPerSession` source: the shared-index
        // snapshot is not part of its identity guard and it must not
        // start being one.
        &IndexSnapshot::default(),
    );
    assert_eq!(result, Discovered::Found("eof-id".to_owned()));
}

/// A file written on Windows ends its line `\r\n`, so the header handed to
/// the adapter keeps a trailing carriage return.
///
/// What makes that work is `serde_json`, which treats it as trailing
/// whitespace — not any trimming here. That was established by mutation:
/// widening the trim to `str::trim` did **not** make this test fail, so the
/// wider trim was removed as the dead code it was. The test stays, because
/// the property it pins — a rollout written on Windows is read — is real
/// and worth keeping true however it is achieved.
#[test]
fn a_header_terminated_by_crlf_is_read() {
    let fixture = Fixture::new();
    let line = header(
        "id",
        "crlf-id",
        &fixture.project_root,
        IN_WINDOW,
        "codex-tui",
        None,
    );
    let mut contents = line.into_bytes();
    contents.extend_from_slice(b"\r\n");
    std::fs::write(fixture.records_root.join("rollout-crlf.jsonl"), &contents)
        .expect("write CRLF fixture");

    let (started_at, ended_at) = window();
    let canonical_root = std::fs::canonicalize(&fixture.project_root).expect("canonicalize");
    let result = discover(
        &Codex,
        &fixture.records_root,
        &canonical_root,
        started_at,
        ended_at,
        // Codex is a `RecordPerSession` source: the shared-index
        // snapshot is not part of its identity guard and it must not
        // start being one.
        &IndexSnapshot::default(),
    );
    assert_eq!(result, Discovered::Found("crlf-id".to_owned()));
}

#[test]
fn a_subagent_thread_is_never_captured() {
    let fixture = Fixture::new();
    write_rollout(
        &fixture.records_root,
        "rollout-subagent.jsonl",
        &header(
            "id",
            "subagent-id",
            &fixture.project_root,
            IN_WINDOW,
            "codex-tui",
            Some("parent-thread-1"),
        ),
        b"",
    );

    let (started_at, ended_at) = window();
    let canonical_root = std::fs::canonicalize(&fixture.project_root).expect("canonicalize");
    let result = discover(
        &Codex,
        &fixture.records_root,
        &canonical_root,
        started_at,
        ended_at,
        // Codex is a `RecordPerSession` source: the shared-index
        // snapshot is not part of its identity guard and it must not
        // start being one.
        &IndexSnapshot::default(),
    );
    assert_eq!(result, Discovered::NotFound);
}

#[test]
fn another_project_s_session_is_never_captured() {
    let fixture = Fixture::new();
    let other_project = fixture.other_dir("other-project");
    write_rollout(
        &fixture.records_root,
        "rollout-elsewhere.jsonl",
        &header(
            "id",
            "elsewhere-id",
            &other_project,
            IN_WINDOW,
            "codex-tui",
            None,
        ),
        b"",
    );

    let (started_at, ended_at) = window();
    let canonical_root = std::fs::canonicalize(&fixture.project_root).expect("canonicalize");
    let result = discover(
        &Codex,
        &fixture.records_root,
        &canonical_root,
        started_at,
        ended_at,
        // Codex is a `RecordPerSession` source: the shared-index
        // snapshot is not part of its identity guard and it must not
        // start being one.
        &IndexSnapshot::default(),
    );
    assert_eq!(result, Discovered::NotFound);
}

#[test]
fn a_session_that_started_before_this_one_is_never_captured() {
    let fixture = Fixture::new();
    write_rollout(
        &fixture.records_root,
        "rollout-early.jsonl",
        &header(
            "id",
            "early-id",
            &fixture.project_root,
            BEFORE_WINDOW,
            "codex-tui",
            None,
        ),
        b"",
    );

    let (started_at, ended_at) = window();
    let canonical_root = std::fs::canonicalize(&fixture.project_root).expect("canonicalize");
    let result = discover(
        &Codex,
        &fixture.records_root,
        &canonical_root,
        started_at,
        ended_at,
        // Codex is a `RecordPerSession` source: the shared-index
        // snapshot is not part of its identity guard and it must not
        // start being one.
        &IndexSnapshot::default(),
    );
    assert_eq!(result, Discovered::NotFound);
}

#[test]
fn two_candidates_are_refused_rather_than_guessed() {
    let fixture = Fixture::new();
    write_rollout(
        &fixture.records_root,
        "rollout-one.jsonl",
        &header(
            "id",
            "id-one",
            &fixture.project_root,
            IN_WINDOW,
            "codex-tui",
            None,
        ),
        b"",
    );
    write_rollout(
        &fixture.records_root,
        "rollout-two.jsonl",
        &header(
            "id",
            "id-two",
            &fixture.project_root,
            IN_WINDOW,
            "codex-tui",
            None,
        ),
        b"",
    );

    let (started_at, ended_at) = window();
    let canonical_root = std::fs::canonicalize(&fixture.project_root).expect("canonicalize");
    let result = discover(
        &Codex,
        &fixture.records_root,
        &canonical_root,
        started_at,
        ended_at,
        // Codex is a `RecordPerSession` source: the shared-index
        // snapshot is not part of its identity guard and it must not
        // start being one.
        &IndexSnapshot::default(),
    );
    assert_eq!(result, Discovered::Ambiguous { candidates: 2 });
}

#[test]
fn nothing_is_read_past_the_first_line() {
    let fixture = Fixture::new();
    // Invalid UTF-8 followed by unterminated JSON: a whole-file parse (or
    // a whole-file UTF-8 decode) would fail on this outright.
    let trailer: &[u8] = &[0xFF, 0xFE, b'{', b'"', b'x', b'\n'];
    write_rollout(
        &fixture.records_root,
        "rollout-real.jsonl",
        &header(
            "id",
            "real-id",
            &fixture.project_root,
            IN_WINDOW,
            "codex-tui",
            None,
        ),
        trailer,
    );

    let (started_at, ended_at) = window();
    let canonical_root = std::fs::canonicalize(&fixture.project_root).expect("canonicalize");
    let result = discover(
        &Codex,
        &fixture.records_root,
        &canonical_root,
        started_at,
        ended_at,
        // Codex is a `RecordPerSession` source: the shared-index
        // snapshot is not part of its identity guard and it must not
        // start being one.
        &IndexSnapshot::default(),
    );
    assert_eq!(result, Discovered::Found("real-id".to_owned()));
}

#[test]
fn a_record_without_a_session_id_field_is_skipped() {
    let fixture = Fixture::new();
    write_rollout(
        &fixture.records_root,
        "rollout-no-id.jsonl",
        &header(
            "session_id",
            "not-an-id",
            &fixture.project_root,
            IN_WINDOW,
            "codex-tui",
            None,
        ),
        b"",
    );

    let (started_at, ended_at) = window();
    let canonical_root = std::fs::canonicalize(&fixture.project_root).expect("canonicalize");
    let result = discover(
        &Codex,
        &fixture.records_root,
        &canonical_root,
        started_at,
        ended_at,
        // Codex is a `RecordPerSession` source: the shared-index
        // snapshot is not part of its identity guard and it must not
        // start being one.
        &IndexSnapshot::default(),
    );
    assert_eq!(result, Discovered::NotFound);
}

// --- the shared-index shape -------------------------------------------
//
// Exercised through the real `Antigravity` adapter rather than a stub:
// `read_index_entry` delegating to `Antigravity::read_last_conversation`
// is half of what this batch wired up, and a stub would prove the
// dispatch works while leaving that delegation unproven.

/// A real index file, and a window derived from the file's **own** mtime
/// rather than from a wall clock.
///
/// Deriving it the other way round is what makes these tests reliable
/// without a `filetime` dependency: the file is written first, its mtime
/// is read back, and the window is placed around it. Nothing here can
/// drift with how long the test takes to run.
struct IndexFixture {
    _tmp: tempfile::TempDir,
    index: PathBuf,
    project_root: PathBuf,
}

impl IndexFixture {
    /// An index whose sole entry is for this fixture's own project.
    fn with_entry(id: &str) -> Self {
        let fixture = Self::empty();
        fixture.write(&format!(
            r#"{{{:?}:{:?}}}"#,
            fixture.project_root.display().to_string(),
            id
        ));
        fixture
    }

    fn empty() -> Self {
        let tmp = tempfile::tempdir().expect("tempdir");
        // The real shape: `<root>/cache/last_conversations.json`.
        let cache = tmp.path().join("cache");
        std::fs::create_dir_all(&cache).expect("create cache dir");
        let project_root = tmp.path().join("project");
        std::fs::create_dir_all(&project_root).expect("create project root");
        let project_root = std::fs::canonicalize(&project_root).expect("canonicalize");
        Self {
            index: cache.join("last_conversations.json"),
            project_root,
            _tmp: tmp,
        }
    }

    fn write(&self, contents: &str) {
        std::fs::write(&self.index, contents).expect("write index");
    }

    fn modified(&self) -> SystemTime {
        std::fs::metadata(&self.index)
            .and_then(|meta| meta.modified())
            .expect("index mtime")
    }

    /// A window with the index's own mtime comfortably inside it.
    fn window_around_the_index(&self) -> (SystemTime, SystemTime) {
        let modified = self.modified();
        (
            modified - Duration::from_secs(60),
            modified + Duration::from_secs(60),
        )
    }

    fn discover_with(
        &self,
        before: &IndexSnapshot,
        window: (SystemTime, SystemTime),
    ) -> Discovered {
        discover(
            &Antigravity,
            &self.index,
            &self.project_root,
            window.0,
            window.1,
            before,
        )
    }
}

fn snapshot_of(id: &str) -> IndexSnapshot {
    IndexSnapshot {
        entry: Some(id.to_owned()),
    }
}

/// Acceptance 1: the entry changed during the session and the index was
/// written inside it, so this is our conversation.
#[test]
fn a_shared_index_entry_that_changed_inside_the_window_is_captured() {
    const ID: &str = "5f8c1a2b-1234-4abc-8abc-abcdefabcdef";
    let fixture = IndexFixture::with_entry(ID);
    let result =
        fixture.discover_with(&IndexSnapshot::default(), fixture.window_around_the_index());
    assert_eq!(result, Discovered::Found(ID.to_owned()));
}

/// The same rule for a project that already had a conversation: the
/// session opened a *new* one, so the entry changed from one identifier
/// to another rather than from nothing to something.
#[test]
fn a_shared_index_entry_replaced_during_the_session_is_captured() {
    const PREVIOUS: &str = "11111111-0000-4000-8000-000000000000";
    const CURRENT: &str = "22222222-0000-4000-8000-000000000000";
    let fixture = IndexFixture::with_entry(CURRENT);
    let result = fixture.discover_with(&snapshot_of(PREVIOUS), fixture.window_around_the_index());
    assert_eq!(result, Discovered::Found(CURRENT.to_owned()));
}

/// Acceptance 2, and the rule the whole two-part guard exists for.
///
/// A shared index has no per-entry timestamp, and its mtime moves when
/// *any* project's entry changes. So an Antigravity session in a
/// different project during our window refreshes the file and makes our
/// project's stale entry look fresh by mtime alone. Only "this entry
/// changed" separates the two, and recording the stale one would mean
/// `glasshouse resume` reopening a conversation this session never had.
#[test]
fn a_shared_index_entry_that_did_not_change_is_never_captured() {
    const ID: &str = "5f8c1a2b-1234-4abc-8abc-abcdefabcdef";
    let fixture = IndexFixture::with_entry(ID);
    // The file's mtime is inside the window — another project's session
    // touched it — but our own entry is exactly what it was before.
    let result = fixture.discover_with(&snapshot_of(ID), fixture.window_around_the_index());
    assert_eq!(result, Discovered::NotFound);
}

/// Acceptance 3: the same mtime prefilter the record-per-session shape
/// applies to a rollout, applied to the index.
#[test]
fn a_shared_index_last_written_before_the_session_is_never_captured() {
    const ID: &str = "5f8c1a2b-1234-4abc-8abc-abcdefabcdef";
    let fixture = IndexFixture::with_entry(ID);
    let modified = fixture.modified();
    // The session began a minute after the index was last written.
    let window = (
        modified + Duration::from_secs(60),
        modified + Duration::from_secs(120),
    );
    let result = fixture.discover_with(&IndexSnapshot::default(), window);
    assert_eq!(result, Discovered::NotFound);
}

/// The upper bound too: an index written after this session ended belongs
/// to something that outlived it.
#[test]
fn a_shared_index_written_after_the_session_ended_is_never_captured() {
    const ID: &str = "5f8c1a2b-1234-4abc-8abc-abcdefabcdef";
    let fixture = IndexFixture::with_entry(ID);
    let modified = fixture.modified();
    let window = (
        modified - Duration::from_secs(120),
        modified - Duration::from_secs(60),
    );
    let result = fixture.discover_with(&IndexSnapshot::default(), window);
    assert_eq!(result, Discovered::NotFound);
}

/// Acceptance 4: the index exists and is perfectly valid, it simply says
/// nothing about this project. Absence is a correct answer.
#[test]
fn a_shared_index_with_no_entry_for_this_project_is_never_captured() {
    let fixture = IndexFixture::empty();
    fixture.write(r#"{"/somewhere/else":"aaaaaaaa-0000-4000-8000-000000000000"}"#);
    let result =
        fixture.discover_with(&IndexSnapshot::default(), fixture.window_around_the_index());
    assert_eq!(result, Discovered::NotFound);
}

/// A missing index is the ordinary case for a harness that has never run,
/// and is not an error.
#[test]
fn a_shared_index_that_does_not_exist_is_not_an_error() {
    let fixture = IndexFixture::empty();
    let window = (
        SystemTime::UNIX_EPOCH,
        SystemTime::UNIX_EPOCH + Duration::from_secs(1),
    );
    let result = fixture.discover_with(&IndexSnapshot::default(), window);
    assert_eq!(result, Discovered::NotFound);
}

/// Acceptance 5, first of two proofs: **the shared-index path opens one
/// file, by name, and never lists a directory.**
///
/// The directory holding the index is stripped of read permission but
/// keeps execute, which on Unix is exactly the difference between "can
/// open a known name inside" and "can enumerate". Any implementation that
/// found the index by listing — a glob, a `read_dir`, a walk — gets
/// nothing here and returns `NotFound`, so asserting `Found` is a
/// positive proof that the file was opened by its declared name.
///
/// This matters because the record-per-session shape *does* list and open
/// everything that survives a name filter, and Antigravity's records are
/// `conversations/<uuid>.db`: SQLite databases holding the user's private
/// conversations. A decoy `conversations/` directory would have been the
/// weaker fixture — discovery swallows every filesystem error, so decoys
/// that cannot be read produce `NotFound` whether they were reached or
/// not, and prove nothing. Denying the listing proves it the other way
/// round, by making success impossible for a listing implementation.
#[cfg(unix)]
#[test]
fn the_shared_index_path_opens_one_file_by_name_and_never_lists_a_directory() {
    use std::os::unix::fs::PermissionsExt;

    const ID: &str = "5f8c1a2b-1234-4abc-8abc-abcdefabcdef";
    let fixture = IndexFixture::with_entry(ID);
    let window = fixture.window_around_the_index();
    let cache = fixture.index.parent().expect("the index has a parent");

    // A decoy that a walk would find, sitting beside the index in the
    // very directory being read. Empty and harmless — the point is that
    // nothing goes looking for it, and this test's real proof is the
    // permission below.
    std::fs::create_dir_all(cache.join("conversations")).expect("create decoy directory");

    // Execute, not read: openable by name, impossible to enumerate.
    std::fs::set_permissions(cache, std::fs::Permissions::from_mode(0o111)).expect("deny listing");
    assert!(
        std::fs::read_dir(cache).is_err(),
        "this test proves nothing unless the directory really cannot be listed"
    );

    let result = fixture.discover_with(&IndexSnapshot::default(), window);

    // Restore before asserting, so a failure still leaves a removable
    // temporary directory behind.
    std::fs::set_permissions(cache, std::fs::Permissions::from_mode(0o755))
        .expect("restore permissions");

    assert_eq!(result, Discovered::Found(ID.to_owned()));
}

/// Acceptance 5, second of two proofs: the shared-index code path does not
/// so much as mention the directory walk.
///
/// A structural scan of this module's own source, which is what the
/// behavioural test above cannot give: it shows the walk is never reached
/// on *any* input, not merely on the inputs a test thought to supply.
/// Reading `walk` out of `discover_shared_index` would be a compile-time
/// visible change, and this fails the moment someone makes it.
#[test]
fn the_shared_index_code_path_never_mentions_the_directory_walk() {
    const SOURCE: &str = include_str!("mod.rs");

    /// The body of `fn name`, from its signature to the first
    /// column-zero `}`.
    ///
    /// Scanned line by line rather than by searching for a literal
    /// `"\n}\n"`. That search cost a red Windows CI run: `include_str!`
    /// reads the file exactly as checked out, and on a runner where Git
    /// converts line endings the source contains `\r\n`, so the literal
    /// never matched and the whole guard panicked with "could not find
    /// the end". `str::lines` strips the `\r` for us, which makes this
    /// CRLF-agnostic by construction rather than by remembering.
    fn body_in(source: &str, name: &str) -> String {
        let start = source
            .find(&format!("fn {name}("))
            .unwrap_or_else(|| panic!("this module no longer defines `{name}`"));
        let mut body = String::new();
        for line in source[start..].lines() {
            body.push_str(line);
            body.push('\n');
            // A column-zero `}` ends the item; an indented one does not,
            // which is why this compares the whole line rather than
            // trimming both ends.
            if line.trim_end() == "}" {
                return body;
            }
        }
        panic!("could not find the end of `{name}`")
    }

    fn body_of(name: &str) -> String {
        body_in(SOURCE, name)
    }

    // The regression guard for the Windows failure itself: the same scan
    // over the same source must find the same body under either line
    // ending. On an LF checkout nothing else exercises that path, so
    // without this the fix would be untested exactly where it was needed.
    //
    // Both copies are built from a normalised base rather than from
    // `SOURCE` directly. The first version of this guard did
    // `SOURCE.replace('\n', "\r\n")` and went red on Windows for its own
    // reason: there `SOURCE` is *already* CRLF, so that produced `\r\r\n`,
    // and `lines` strips only one `\r`. An assertion that depends on how
    // the file happened to be checked out is a flake generator — the same
    // lesson a subcontractor taught this project one batch earlier, with a
    // test that scanned a randomly generated token.
    let lf: String = SOURCE.replace("\r\n", "\n");
    let crlf: String = lf.replace('\n', "\r\n");
    for name in ["discover_shared_index", "read_index_capped", "snapshot"] {
        assert_eq!(
            body_in(&crlf, name),
            body_in(&lf, name),
            "the source scan must not depend on line endings; `{name}` \
             scanned differently under CRLF, which is exactly how this \
             test failed on Windows CI"
        );
    }

    // Every function the `SharedIndex` variant can reach.
    for name in ["discover_shared_index", "read_index_capped", "snapshot"] {
        let body = body_of(name);
        for forbidden in ["walk(", "read_dir"] {
            assert!(
                !body.contains(forbidden),
                "`{name}` reaches `{forbidden}`: the shared-index path must \
                 open exactly one named file, never enumerate a directory. \
                 Antigravity's session records are SQLite databases holding \
                 the user's own conversations."
            );
        }
    }

    // And the guard above is only meaningful while `walk` is still what
    // the other shape calls — a rename would otherwise make every
    // assertion above vacuously true.
    assert!(
        body_of("discover_record_per_session").contains("walk("),
        "the record-per-session shape no longer calls `walk`, so scanning \
         for it proves nothing; update this test to name whatever \
         replaced it"
    );
}

/// The snapshot is scaffolding for the shared-index shape alone. Handing
/// the record-per-session path a populated one must change nothing, or
/// the two shapes have started sharing a guard that means different
/// things in each.
#[test]
fn the_record_per_session_path_ignores_the_index_snapshot() {
    let fixture = Fixture::new();
    write_rollout(
        &fixture.records_root,
        "rollout-real.jsonl",
        &header(
            "id",
            "real-id",
            &fixture.project_root,
            IN_WINDOW,
            "codex-tui",
            None,
        ),
        b"",
    );

    let (started_at, ended_at) = window();
    let canonical_root = std::fs::canonicalize(&fixture.project_root).expect("canonicalize");
    let with_snapshot = discover(
        &Codex,
        &fixture.records_root,
        &canonical_root,
        started_at,
        ended_at,
        &snapshot_of("real-id"),
    );
    assert_eq!(with_snapshot, Discovered::Found("real-id".to_owned()));
}

#[test]
fn an_unreadable_records_root_is_not_an_error() {
    let fixture = Fixture::new();
    let missing = fixture.tmp.path().join("does-not-exist");

    let (started_at, ended_at) = window();
    let canonical_root = std::fs::canonicalize(&fixture.project_root).expect("canonicalize");
    let result = discover(
        &Codex,
        &missing,
        &canonical_root,
        started_at,
        ended_at,
        &IndexSnapshot::default(),
    );
    assert_eq!(result, Discovered::NotFound);
}
