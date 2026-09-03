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
mod tests;
