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
//! # Two shapes, and why the difference is in the type
//!
//! Not every harness keeps one record per session. Antigravity keeps every
//! project's last conversation identifier in a single shared index, and its
//! records are SQLite databases holding the user's private conversations.
//! [`NativeSessionSource`] is therefore an enum, and [`discover`] dispatches
//! on it: the shared-index arm opens exactly one named file and never calls
//! the private directory walk, so it cannot reach a conversation database — a
//! property of the code path rather than of a rule someone has to remember.
//!
//! [`capture`] is the production entry point: it resolves where a harness
//! keeps its session identity, calls [`discover`], and records what it finds.
//! Both session producers (`main.rs: launch_session` and `shell::run`) call
//! it exactly once, when a session ends — and call [`snapshot`] once when a
//! session starts, because a harness that keeps its identifiers in one shared
//! index gives Glasshouse no per-entry timestamp to bound a candidate with,
//! and the only bound left is having read the entry both before and after.
//!
//! This module knows no harness: [`crate::harness::NativeSessionRecord`],
//! [`crate::harness::NativeSessionKind`] and [`NativeSessionSource`] — the
//! vocabulary a [`HarnessAdapter`] speaks to describe what it found — live in
//! `harness/mod.rs` beside the rest of that vocabulary, and this module only
//! ever consumes them through the adapter trait.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use crate::harness::{
    HarnessAdapter, NativeSessionKind, NativeSessionSource, RecordPerSessionSource,
};
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

/// Find `adapter`'s own identifier for the session Glasshouse ran in
/// `project_root` between `started_at` and `ended_at`.
///
/// `source_path` is the already-resolved place this adapter's
/// [`NativeSessionSource`] points at — a records directory for
/// [`NativeSessionSource::RecordPerSession`], one index *file* for
/// [`NativeSessionSource::SharedIndex`] — and `before` is what that index
/// held for this project when the session started (see [`snapshot`]; empty,
/// and unused, for the record-per-session shape).
///
/// Reads no environment and no global state, so it is exercised entirely
/// through fixture files in this module's tests.
///
/// # The two shapes
///
/// Which shape a harness declares decides what is opened, and the difference
/// is a security property rather than a performance one:
///
/// - [`NativeSessionSource::RecordPerSession`] walks the directory and opens
///   every file surviving the name filter, reading **only** the first line:
///   everything after it is the user's own conversation, and this module has
///   no business reading any of it.
/// - [`NativeSessionSource::SharedIndex`] opens **exactly one file**, the
///   index named in the declaration. It never calls the directory walk, so it
///   cannot list or open a session record — which matters because
///   Antigravity's records are SQLite databases holding the user's private
///   conversations.
pub fn discover(
    adapter: &dyn HarnessAdapter,
    source_path: &Path,
    project_root: &Path,
    started_at: SystemTime,
    ended_at: SystemTime,
    before: &IndexSnapshot,
) -> Discovered {
    match adapter.session_id_source() {
        None => Discovered::NotFound,
        Some(NativeSessionSource::RecordPerSession(source)) => discover_record_per_session(
            adapter,
            &source,
            source_path,
            project_root,
            started_at,
            ended_at,
        ),
        Some(NativeSessionSource::SharedIndex(_)) => discover_shared_index(
            adapter,
            source_path,
            project_root,
            started_at,
            ended_at,
            before,
        ),
    }
}

/// The walk-and-filter shape, byte for byte what [`discover`] did before the
/// source became an enum over two shapes.
fn discover_record_per_session(
    adapter: &dyn HarnessAdapter,
    source: &RecordPerSessionSource,
    records_root: &Path,
    project_root: &Path,
    started_at: SystemTime,
    ended_at: SystemTime,
) -> Discovered {
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

/// What a harness's shared index held for one project before its session ran.
///
/// The whole reason this type exists is that a shared index has no per-entry
/// timestamp. The record-per-session shape can bound a candidate by the start
/// time the record states about *itself*; an index entry states nothing, so
/// the bound has to come from having read the entry twice — see [`discover`]'s
/// shared-index arm, and [`snapshot`] for where the first read happens.
///
/// [`Default`] is "nothing was there, or nothing was looked at": the honest
/// value for a harness with no shared index at all, and the one that makes
/// the record-per-session path ignore this parameter entirely.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct IndexSnapshot {
    entry: Option<String>,
}

/// Read what `harness`'s shared index holds for `project_root` right now, to
/// be handed back to [`capture`] when the session ends.
///
/// Called by both session producers at session *start*. Costs one open and
/// one read of a few hundred bytes for a harness that has a shared index, and
/// not even a path resolution for every other harness — including a harness
/// Glasshouse cannot discover identifiers for, which is most of them.
pub fn snapshot(harness: &str, project_root: &Path) -> IndexSnapshot {
    let Some(adapter) = crate::harness::all().find(|a| a.id().slug() == harness) else {
        return IndexSnapshot::default();
    };
    let Some(NativeSessionSource::SharedIndex(source)) = adapter.session_id_source() else {
        return IndexSnapshot::default();
    };
    let Some(index_path) = resolve_source_path(&NativeSessionSource::SharedIndex(source)) else {
        return IndexSnapshot::default();
    };
    IndexSnapshot {
        entry: read_index_capped(&index_path)
            .and_then(|text| adapter.read_index_entry(&text, project_root)),
    }
}

/// The shared-index shape: read **one** named file and ask the adapter what
/// it says about this project.
///
/// # The identity guard
///
/// Recording the wrong identifier here means `glasshouse resume` reopening a
/// stranger's conversation, and Antigravity's resume does not fail closed —
/// an unknown identifier starts a *fresh* conversation and exits 0 — so a
/// mistake is silent. The record-per-session shape bounds a candidate by the
/// start time the record states about itself. An index entry states nothing
/// about when it was written, so two rules stand in for that bound, and both
/// are required:
///
/// 1. **The index file's own mtime falls inside the session's window.** The
///    same prefilter [`discover_record_per_session`] applies to a rollout,
///    applied to the index instead.
/// 2. **This project's entry changed during the session.** Read once at
///    session start ([`snapshot`]) and again here; record only if the second
///    read is `Some` and differs from the first.
///
/// Rule 1 alone is not enough, and the hole is worth stating: the index's
/// mtime moves when *any* project's entry changes, so somebody else's session
/// in another project during our window refreshes it and could make a stale
/// entry for *our* project look fresh. Rule 2 closes that, because a stale
/// entry is by definition unchanged.
///
/// Rule 2's one false negative is acceptable, and points the safe way:
/// resuming the *same* conversation leaves the entry unchanged, so nothing
/// new is recorded — but Glasshouse only ever resumes an identifier it
/// already holds, so the record already has it.
///
/// Nothing here logs the index's contents. A conversation UUID is the user's
/// own data.
fn discover_shared_index(
    adapter: &dyn HarnessAdapter,
    index_path: &Path,
    project_root: &Path,
    started_at: SystemTime,
    ended_at: SystemTime,
    before: &IndexSnapshot,
) -> Discovered {
    // Rule 1. Both bounds, not just the lower one: an index last written
    // after this session ended belongs to something that outlived it.
    let Ok(modified) = std::fs::metadata(index_path).and_then(|meta| meta.modified()) else {
        return Discovered::NotFound;
    };
    if modified < started_at || modified > ended_at {
        return Discovered::NotFound;
    }

    let Some(text) = read_index_capped(index_path) else {
        return Discovered::NotFound;
    };
    let Some(current) = adapter.read_index_entry(&text, project_root) else {
        return Discovered::NotFound;
    };

    // Rule 2. An unchanged entry is a stale entry, whatever the mtime says.
    if before.entry.as_deref() == Some(current.as_str()) {
        return Discovered::NotFound;
    }

    Discovered::Found(current)
}

/// Read one whole index file, capped at [`MAX_HEADER_BYTES`].
///
/// Unlike [`read_first_line`] this reads past the first newline, because an
/// index is one JSON document rather than a line-delimited log and nothing
/// says a harness must write it on a single line. It is still capped: a file
/// this large is not the few-hundred-byte index Glasshouse is looking for,
/// and truncating one would only hand the adapter invalid JSON anyway.
///
/// Exactly one file is opened, by name. This function takes a path and never
/// a directory, and has no counterpart to [`walk`] — that is what makes a
/// conversation database unreachable from the shared-index path.
fn read_index_capped(path: &Path) -> Option<String> {
    use std::io::Read;

    let file = std::fs::File::open(path).ok()?;
    let mut text = String::new();
    std::io::BufReader::new(file)
        .take(MAX_HEADER_BYTES)
        .read_to_string(&mut text)
        .ok()?;
    // The cap arrived before the file ended: not the index, and a
    // truncated JSON document besides.
    ((text.len() as u64) < MAX_HEADER_BYTES).then_some(text)
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
/// it. `None` when the file could not be opened, when it is empty, or when the
/// cap arrived before a newline did — a line the cap truncated is not a header
/// Glasshouse will trust.
///
/// End-of-file without a newline is **accepted**. A harness writes its header
/// before it has anything to append, so a record whose only line is a complete
/// one is ordinary rather than suspicious. Requiring the newline is how this
/// first failed on Windows and nowhere else: every fixture here ended in one,
/// so eight passing tests said nothing about the case, and the harness that
/// wrote without one looked to Glasshouse like a session that never happened.
fn read_first_line(path: &Path) -> Option<Vec<u8>> {
    use std::io::{BufRead, BufReader, Read};

    let file = std::fs::File::open(path).ok()?;
    let mut reader = BufReader::new(file).take(MAX_HEADER_BYTES);
    let mut line = Vec::new();
    reader.read_until(b'\n', &mut line).ok()?;
    if line.last() == Some(&b'\n') {
        return Some(line);
    }
    // No newline: the file ended, or the cap did. Only the cap is a refusal.
    if line.len() as u64 >= MAX_HEADER_BYTES {
        return None;
    }
    (!line.is_empty()).then_some(line)
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
///
/// `before` is what [`snapshot`] read at session start. For a harness with a
/// shared index it is half the identity guard; for every other harness it is
/// unused, and [`IndexSnapshot::default`] is the right thing to pass.
pub fn capture(
    store: &SessionStore<'_>,
    record: &SessionRecord,
    project_root: &Path,
    before: &IndexSnapshot,
) {
    if record.native_session_id.is_some() {
        return;
    }
    let Some(adapter) = crate::harness::all().find(|a| a.id().slug() == record.harness) else {
        return;
    };
    let Some(source) = adapter.session_id_source() else {
        return;
    };
    let Some(source_path) = resolve_source_path(&source) else {
        return;
    };

    let started_at = seconds_to_system_time(record.created_at);
    let ended_at = SystemTime::now();

    match discover(
        adapter,
        &source_path,
        project_root,
        started_at,
        ended_at,
        before,
    ) {
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

/// Resolve where a harness's source actually lives: its `home_env` if the
/// harness honours one and it is set and non-empty, else the user's home
/// directory joined with `home_default` — then joined with the part that
/// differs by shape.
///
/// What comes back is a *directory* of records for
/// [`NativeSessionSource::RecordPerSession`] and a single *file* for
/// [`NativeSessionSource::SharedIndex`]. [`discover`] dispatches on the same
/// variant, so the two can never be crossed.
fn resolve_source_path(source: &NativeSessionSource) -> Option<PathBuf> {
    let (home_env, home_default) = source.home();
    let relocated = home_env
        .and_then(|name| std::env::var(name).ok())
        .filter(|value| !value.is_empty())
        .map(PathBuf::from);
    let home = match relocated {
        Some(root) => root,
        None => directories::UserDirs::new()?.home_dir().join(home_default),
    };
    Some(match source {
        NativeSessionSource::RecordPerSession(source) => home.join(source.subdirectory),
        NativeSessionSource::SharedIndex(source) => home.join(source.index_path),
    })
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
        let result =
            fixture.discover_with(&snapshot_of(PREVIOUS), fixture.window_around_the_index());
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
        std::fs::set_permissions(cache, std::fs::Permissions::from_mode(0o111))
            .expect("deny listing");
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
        const SOURCE: &str = include_str!("native_id.rs");

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
        // over the same source with CRLF endings must find the same body. On
        // an LF checkout this is the only thing that exercises that path, so
        // without it the fix would be untested everywhere it was needed.
        let crlf: String = SOURCE.replace('\n', "\r\n");
        for name in ["discover_shared_index", "read_index_capped", "snapshot"] {
            assert_eq!(
                body_in(&crlf, name),
                body_of(name),
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
}
