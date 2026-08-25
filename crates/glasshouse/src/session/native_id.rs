//! Discovering a harness's own identifier for a session Glasshouse started.
//!
//! Glasshouse cannot tell every harness what its session's identifier should
//! be — see [`crate::harness::HarnessAdapter::assign_session_id`]'s doc
//! comment. A harness that names its own sessions instead writes some kind of
//! session record to its own state, and [`discover`] is how Glasshouse finds
//! the one that belongs to a session it just ran, without reading anything
//! beyond that record's own header.
//!
//! # Why this refuses rather than guesses
//!
//! A harness's session store holds records for things Glasshouse never
//! started: subagent threads, sessions from another client (a desktop app,
//! another terminal), sessions in another project. [`discover`] is
//! deliberately narrow — cwd, a time window, and the harness's own notion of
//! "interactive" — and deliberately refuses when more than one record
//! survives that filter. Guessing here would let `glasshouse resume` reopen a
//! stranger's conversation, which is a worse outcome than recording nothing
//! at all.
//!
//! [`capture`] is the production entry point: it resolves a harness's own
//! records root, calls [`discover`], and records what it finds. Both session
//! producers (`main.rs: launch_session` and `shell::run`) call it exactly
//! once, when a session ends.
//!
//! This module knows no harness: [`crate::harness::NativeSessionRecord`],
//! [`crate::harness::NativeSessionKind`] and [`NativeSessionSource`] — the
//! vocabulary a [`HarnessAdapter`] speaks to describe what it found — live in
//! `harness/mod.rs` beside the rest of that vocabulary, and this module only
//! ever consumes them through the adapter trait.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use crate::harness::{HarnessAdapter, NativeSessionKind, NativeSessionSource};
use crate::platform::paths::same_path;
use crate::session::store::{SessionRecord, SessionStore};

/// What discovery concluded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Discovered {
    Found(String),
    /// Nothing in the window matched. The honest answer, and the common one:
    /// a session that never took a turn writes no record at all.
    NotFound,
    /// More than one candidate matched. Glasshouse records nothing —
    /// recording the wrong identifier would make `glasshouse resume` reopen
    /// someone else's conversation.
    Ambiguous {
        candidates: usize,
    },
}

/// The most a header is ever trusted to be. Every real rollout header sampled
/// for this module is a few hundred bytes; anything claiming to be a first
/// line this large is not a header Glasshouse will parse — see [`discover`]'s
/// doc comment.
const MAX_HEADER_BYTES: u64 = 1024 * 1024;

/// Find the one session record under `records_root` that is `adapter`'s own
/// interactive session for `project_root`, started inside
/// `[started_at, ended_at]`.
///
/// Takes an already-resolved root and reads no environment and no global
/// state, so it is exercised entirely through fixture files in this module's
/// tests. Never reads past a candidate file's first line: everything after it
/// is the user's own conversation, and this module has no business reading
/// any of it.
pub fn discover(
    adapter: &dyn HarnessAdapter,
    records_root: &Path,
    project_root: &Path,
    started_at: SystemTime,
    ended_at: SystemTime,
) -> Discovered {
    let Some(source) = adapter.session_id_source() else {
        return Discovered::NotFound;
    };

    let mut files = Vec::new();
    walk(records_root, &mut files);

    let mut found = Vec::new();
    for path in files {
        let Some(file_name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if !file_name.starts_with(source.file_prefix) || !file_name.ends_with(source.file_extension)
        {
            continue;
        }

        // Cheap prefilter: a rollout is appended to throughout its session, so
        // its mtime is a sound lower bound on when it was last written. One
        // last touched before our session began cannot be ours, and this
        // check costs nothing more than a `stat`.
        let Ok(modified) = std::fs::metadata(&path).and_then(|meta| meta.modified()) else {
            continue;
        };
        if modified < started_at {
            continue;
        }

        let Some(line) = read_first_line(&path) else {
            continue;
        };
        let Ok(header) = std::str::from_utf8(&line) else {
            continue;
        };
        let Some(record) = adapter.read_session_record(header.trim_end_matches('\n')) else {
            continue;
        };

        if !matches!(record.kind, NativeSessionKind::Interactive) {
            continue;
        }
        let Ok(cwd) = std::fs::canonicalize(&record.cwd) else {
            continue;
        };
        if !same_path(project_root, &cwd) {
            continue;
        }
        if record.started_at < started_at || record.started_at > ended_at {
            continue;
        }

        found.push(record.id);
    }

    // Deliberately not sorted-and-take-the-newest: the same refusal
    // `session::select` and the resume identifier resolver already apply.
    match found.len() {
        0 => Discovered::NotFound,
        1 => Discovered::Found(found.into_iter().next().expect("length checked above")),
        candidates => Discovered::Ambiguous { candidates },
    }
}

/// Recursively collect every file under `dir`. A root that does not exist (or
/// cannot be read) yields no files, which is how [`discover`] turns "this
/// harness has never written a session" into `NotFound` rather than an error.
fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if file_type.is_dir() {
            walk(&entry.path(), out);
        } else if file_type.is_file() {
            out.push(entry.path());
        }
    }
}

/// Read at most the first line of `path`, and at most [`MAX_HEADER_BYTES`] of
/// it. `None` when the file could not be opened, when end-of-file arrived
/// before a newline, or when the cap did — either of the last two means this
/// is not a header Glasshouse will trust.
fn read_first_line(path: &Path) -> Option<Vec<u8>> {
    use std::io::{BufRead, BufReader, Read};

    let file = std::fs::File::open(path).ok()?;
    let mut reader = BufReader::new(file).take(MAX_HEADER_BYTES);
    let mut line = Vec::new();
    reader.read_until(b'\n', &mut line).ok()?;
    if line.last() != Some(&b'\n') {
        return None;
    }
    Some(line)
}

/// Discover and record a just-ended session's native identifier, if
/// `record`'s harness has one to discover.
///
/// Called exactly once, when a session ends — see the call sites in
/// `main.rs: launch_session` and `shell::run`. Best effort throughout: this
/// runs after the harness has already run to completion, so a failure here
/// must never surface as an error to the user. It only ever logs, following
/// the same convention as `main.rs: note_lifecycle`.
///
/// Does nothing when `record` already carries a native identifier, or when
/// its harness has no [`HarnessAdapter::session_id_source`] — most harnesses
/// either assign their own identifier up front or have none of this to do.
pub fn capture(store: &SessionStore<'_>, record: &SessionRecord, project_root: &Path) {
    if record.native_session_id.is_some() {
        return;
    }
    let Some(adapter) = crate::harness::all().find(|a| a.id().slug() == record.harness) else {
        return;
    };
    let Some(source) = adapter.session_id_source() else {
        return;
    };
    let Some(records_root) = resolve_records_root(&source) else {
        return;
    };

    let started_at = seconds_to_system_time(record.created_at);
    let ended_at = SystemTime::now();

    match discover(adapter, &records_root, project_root, started_at, ended_at) {
        Discovered::Found(native_id) => match store.set_native_session_id(&record.id, &native_id) {
            Ok(_) => {
                tracing::info!(
                    session = %record.id,
                    native_session = %native_id,
                    "captured the harness's native session identifier"
                );
            }
            Err(err) => {
                tracing::warn!(
                    session = %record.id,
                    error = %err,
                    "discovered a native session identifier but could not record it"
                );
            }
        },
        Discovered::NotFound => {
            tracing::debug!(
                session = %record.id,
                "found no native session identifier for this session"
            );
        }
        Discovered::Ambiguous { candidates } => {
            tracing::warn!(
                session = %record.id,
                candidates,
                "refusing to record a native session identifier: more than one candidate matched"
            );
        }
    }
}

/// Resolve a harness's session-records root: `source.home_env` if it is set
/// and non-empty, else the user's home directory joined with
/// `source.home_default`, then joined with `source.subdirectory`.
fn resolve_records_root(source: &NativeSessionSource) -> Option<PathBuf> {
    let home = match std::env::var(source.home_env) {
        Ok(value) if !value.is_empty() => PathBuf::from(value),
        _ => directories::UserDirs::new()?
            .home_dir()
            .join(source.home_default),
    };
    Some(home.join(source.subdirectory))
}

/// Convert Glasshouse's own stored `created_at` (seconds since the Unix
/// epoch — see `session::store`'s `system_clock`) back into a `SystemTime`.
fn seconds_to_system_time(seconds: i64) -> SystemTime {
    match u64::try_from(seconds) {
        Ok(seconds) => SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(seconds),
        Err(_) => SystemTime::UNIX_EPOCH,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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
        );
        assert_eq!(result, Discovered::Found("real-id".to_owned()));
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
        );
        assert_eq!(result, Discovered::NotFound);
    }

    #[test]
    fn an_unreadable_records_root_is_not_an_error() {
        let fixture = Fixture::new();
        let missing = fixture.tmp.path().join("does-not-exist");

        let (started_at, ended_at) = window();
        let canonical_root = std::fs::canonicalize(&fixture.project_root).expect("canonicalize");
        let result = discover(&Codex, &missing, &canonical_root, started_at, ended_at);
        assert_eq!(result, Discovered::NotFound);
    }
}
