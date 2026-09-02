use std::fs;
use std::path::{Path, PathBuf};

use rusqlite::{Connection, OpenFlags};

use super::migrations::{bind_project, migrate, verify_identity};
use super::{DatabaseError, configure};

/// Inspect the final database path; refuse symlinks and non-regular entries.
///
/// Returns `None` only when the path definitively does not exist (so the
/// caller should create it), `Some(metadata)` for an existing regular file.
/// Any other inspection failure — permission denied and friends — is
/// preserved with its source rather than being mistaken for permission to
/// create the file. Deliberately says nothing about the file's *length* —
/// see [`check_existing`], its only caller that also needs that judgment.
fn inspect_existing(db_path: &Path) -> Result<Option<fs::Metadata>, DatabaseError> {
    let metadata = match fs::symlink_metadata(db_path) {
        Ok(metadata) => metadata,
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => {
            return Ok(None);
        }
        Err(source) => {
            return Err(DatabaseError::Inspect {
                path: db_path.to_path_buf(),
                source,
            });
        }
    };
    if metadata.file_type().is_symlink() {
        return Err(DatabaseError::Symlinked {
            path: db_path.to_path_buf(),
        });
    }
    if !metadata.is_file() {
        // Anything that is not a regular file — a directory, a device, a
        // FIFO, a socket — must not be opened as (or replaced by) a
        // database. Special files in particular could block or misbehave
        // when SQLite tries to read and write them.
        return Err(DatabaseError::NotARegularFile {
            path: db_path.to_path_buf(),
            actual: describe_entry(&metadata),
        });
    }
    // An existing regular file keeps whatever permissions it has;
    // like `create_state_dir`, this call neither widens nor narrows.
    Ok(Some(metadata))
}

/// Inspect the final database path for the case where it is expected to
/// predate this launch: refuses symlinks and non-regular entries (via
/// [`inspect_existing`]), and additionally refuses a zero-byte existing
/// file, which is never what a genuinely new project looks like.
///
/// Returns `Ok(false)` only when the path definitively does not exist (so the
/// caller should create it), `Ok(true)` when an existing regular, nonempty
/// file is ready to open.
///
/// **A zero-byte file at this path has exactly one meaning, and it is
/// "truncated".** Glasshouse never creates a database *here*: a first creation
/// happens on a private sibling and arrives at this path whole, in one
/// [`hard link`](publish), with its schema and its project binding already
/// committed behind it. So there is no such thing as a database in the making
/// at this path, nothing to wait for, and nothing to tell apart — a zero-byte
/// file is a database that used to hold this project's sessions, memories and
/// checkpoints and was truncated by a crashed copy, an interrupted restore or
/// a disk-full write.
///
/// It is therefore refused on the spot, without opening a connection and
/// without waiting: nothing here reads, writes or locks the file, because a
/// refusal that touched the file it refused would destroy the evidence the
/// user needs to recover it, and a refusal that waited would only be
/// pretending the question is still open. (This is what wave 108's
/// `wait_out_a_concurrent_creation` existed for, and what the private-file
/// creation below retired: the two meanings it had to tell apart no longer
/// both exist.)
fn check_existing(db_path: &Path) -> Result<bool, DatabaseError> {
    let Some(metadata) = inspect_existing(db_path)? else {
        return Ok(false);
    };
    if metadata.len() > 0 {
        return Ok(true);
    }
    Err(DatabaseError::EmptyExisting {
        path: db_path.to_path_buf(),
    })
}

/// Human-readable kind of a final-path entry, for error messages.
fn describe_entry(metadata: &fs::Metadata) -> &'static str {
    let file_type = metadata.file_type();
    if file_type.is_dir() {
        "a directory"
    } else if file_type.is_file() {
        "a regular file"
    } else if file_type.is_symlink() {
        "a symbolic link"
    } else {
        "a special file (device, FIFO, socket, ...)"
    }
}

/// The infix that marks a private, in-progress database beside a final one.
///
/// The whole name is `<final file name>.tmp-<pid>-<start time>-<nonce>`, with
/// the start time and the nonce each sixteen lowercase hex digits. Three
/// things are encoded on purpose:
///
/// * the **pid**, so a leftover can be asked about;
/// * the creator's **process start time** (`ObservedProcess::started_at_ms`,
///   `0` when this machine would not say), so that a pid answering the probe
///   can be told apart from *the pid this file was created by* — a recycled
///   pid is otherwise indistinguishable from a live sibling still working;
/// * a **nonce**, so that two creations from one process (a test, a hook
///   subprocess re-entering) cannot collide on the name.
///
/// The shape deliberately matches `firewall::store`'s temp files
/// (`<name>.tmp-<pid>-<hex>`), which is this crate's existing convention for
/// "mine, in progress, beside the real thing".
pub(super) const PRIVATE_INFIX: &str = ".tmp-";

/// Make sure a complete database exists at `db_path`, creating one privately
/// and publishing it whole if there is none, and without following a symlink
/// that may sit at the final component.
///
/// Only a definitive `NotFound` from the inspection counts as "absent"; any
/// other failure — a zero-byte file included, see [`check_existing`] — is
/// preserved rather than mistaken for permission to create the file.
///
/// When the path *is* absent this never creates the file at `db_path`. It
/// creates `<db_path>.tmp-<pid>-<start>-<nonce>` instead, migrates and binds
/// **that**, and then publishes it with one hard link ([`publish`]). The
/// invariant the rest of this module rests on falls out of that: a file at
/// `db_path` is always a complete, migrated, project-bound database or a
/// truncated one, and never one in the making. A caller arriving mid-creation
/// sees no file at all, does its own creation, and one of the two wins the
/// link; the loser discards its own finished database and opens the winner's,
/// which is complete by construction.
///
/// In a burst of *n* first bootstraps this runs *n* small migrations on *n*
/// private files rather than making *n* − 1 callers queue on one lock behind a
/// migration of unbounded length, which is what [`configure`]'s five second
/// busy timeout used to be a bet against.
pub(super) fn prepare_file(db_path: &Path, project_id: &str) -> Result<(), DatabaseError> {
    if check_existing(db_path)? {
        return Ok(());
    }

    // Only ever on the path that is about to create one of these itself, so a
    // launch that finds a database already there never enumerates anything.
    sweep_abandoned_private_files(db_path);

    let private = private_creation_path(db_path)?;

    // Create the file rather than letting SQLite do it, because SQLite would
    // use plain `0644 &! umask` — world-readable, which no project memory ever
    // should be. `create_new` on a name carrying this process's pid and a
    // fresh nonce cannot collide with a sibling's; if it somehow does, that is
    // a real error and not a race to absorb.
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options
        .open(&private)
        .map_err(|source| DatabaseError::Create {
            path: private.clone(),
            source,
        })?;

    // A test-only seam. On any build without `cfg(test)` — every shipped
    // binary, and every integration test, which links the library compiled
    // without it — this line and the hook it calls do not exist at all, so
    // there is no branch, no thread-local and no state in production. It gives
    // the stress test below a window in which a private file provably exists
    // and has provably not been published.
    #[cfg(test)]
    hold_private_file(&private);

    if let Err(err) = migrate_privately(&private, project_id) {
        discard_private_file(&private);
        return Err(err);
    }

    publish(&private, db_path)
}

/// Where this process's private copy of `db_path` goes: beside it, in the same
/// directory, never in a shared temp directory.
///
/// Same directory because the publish is a hard link and a hard link cannot
/// cross a filesystem — and because a database holding a project's memory has
/// no business passing through a world-readable `/tmp` even briefly.
fn private_creation_path(db_path: &Path) -> Result<PathBuf, DatabaseError> {
    // Failing to name the private file is failing to create the database, and
    // that is what the user needs to be told; the private name is an
    // implementation detail of getting there.
    let create_err = |source| DatabaseError::Create {
        path: db_path.to_path_buf(),
        source,
    };
    let dir = db_path
        .parent()
        .ok_or_else(|| create_err(std::io::Error::other("the database path has no directory")))?;
    let name = db_path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| create_err(std::io::Error::other("the database path has no file name")))?;

    let mut nonce = [0u8; 8];
    getrandom::fill(&mut nonce).map_err(|source| create_err(std::io::Error::other(source)))?;

    Ok(dir.join(format!(
        "{name}{PRIVATE_INFIX}{}-{:016x}-{}",
        std::process::id(),
        own_start_time_ms() as u64,
        hex::encode(nonce),
    )))
}

/// This process's start time as the liveness probe reports it, or `0` when the
/// machine would not say.
///
/// `0` is a deliberate "unknown" rather than a guess: [`creator_is_gone`]
/// refuses to sweep a leftover carrying it whenever its pid is live, which
/// leaks a file in the one case the encoding cannot decide. Sweeping it would
/// risk deleting a live sibling's work, and that trade is not close.
fn own_start_time_ms() -> i64 {
    crate::session::supervision::observe(std::process::id())
        .map(|observed| observed.started_at_ms)
        .unwrap_or(0)
}

/// Run everything [`open`] runs on a brand-new database, on the private file,
/// and close it again.
///
/// The same sequence in the same order as `open`'s, deliberately: there is one
/// way this project brings a database up, and a second one that drifted would
/// be exactly the kind of difference nobody notices until a migration behaves
/// differently on a first launch than on every later one. [`verify_identity`]
/// passes trivially on a file created moments ago and is run anyway for that
/// reason.
fn migrate_privately(private: &Path, project_id: &str) -> Result<(), DatabaseError> {
    let sql_err = |source| DatabaseError::Sql {
        path: private.to_path_buf(),
        source,
    };

    let mut conn = Connection::open_with_flags(private, OpenFlags::SQLITE_OPEN_READ_WRITE)
        .map_err(|source| DatabaseError::Open {
            path: private.to_path_buf(),
            source,
        })?;
    configure(&conn, private)?;
    verify_identity(&conn, private, project_id)?;

    let tx = conn
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .map_err(sql_err)?;
    migrate(&tx, private)?;
    bind_project(&tx, private, project_id)?;
    tx.commit().map_err(sql_err)?;

    // Load-bearing, not tidiness: Windows refuses to remove or rename a file
    // while a handle is open on it unless that handle was opened with
    // FILE_SHARE_DELETE, and SQLite's is not. Both the publish and the
    // discard that follow remove this file, so the connection goes first.
    drop(conn);
    Ok(())
}

/// Make the finished private database appear at the final path, whole.
///
/// [`std::fs::hard_link`] is `link(2)` on unix and `CreateHardLinkW` on
/// Windows (NTFS); the two paths are siblings, so this is never a cross-volume
/// link, which is the one thing either call refuses outright. A hard link is
/// the primitive this whole design turns on: the final directory entry appears
/// with the full, committed content already behind it, so there is no instant
/// at which that path exists and is incomplete. It shares the inode, so
/// removing the private name afterwards leaves the final one intact — with the
/// `0600` mode the private file was created with, since the mode belongs to
/// the inode and not to the name.
///
/// **Never a rename.** A rename would silently *replace* whatever is at the
/// final path, and refusing a truncated database rather than overwriting it is
/// a promise this project keeps ([`DatabaseError::EmptyExisting`]).
///
/// `AlreadyExists` is the race signal, and it is `AlreadyExists` on both
/// platforms: a sibling published first. That sibling's file is a complete
/// migrated database by construction, so this process discards its own
/// finished work and lets its caller open the sibling's. Losing here is
/// ordinary and costs one small migration; it is not an error.
fn publish(private: &Path, db_path: &Path) -> Result<(), DatabaseError> {
    let outcome = match fs::hard_link(private, db_path) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => Ok(()),
        Err(source) => Err(DatabaseError::Publish {
            path: db_path.to_path_buf(),
            private: private.to_path_buf(),
            source,
        }),
    };
    // Whichever way the link went, the private name has done its job. On the
    // winning path the content survives it, because the final name is now the
    // second link to the same inode; on the losing path it is this process's
    // own discarded work; on the failing path it is what the error is about
    // and leaving it would leak a database file per attempt.
    discard_private_file(private);
    outcome
}

/// Remove a private file and the rollback journal SQLite may have left beside
/// it, best-effort.
///
/// Best-effort on purpose: this runs on the success path, where the content
/// already survives under the final name, and on error paths, where the error
/// being reported is the thing that matters. A failure to unlink here leaks a
/// file that the next bootstrap's sweep will collect.
fn discard_private_file(private: &Path) {
    let _ = fs::remove_file(private);
    let _ = fs::remove_file(journal_beside(private));
}

/// The rollback journal SQLite creates beside a database while a transaction
/// is open. Glasshouse never sets `journal_mode`, so every connection it opens
/// is in SQLite's default rollback mode and `-journal` is the only sidecar
/// there can be — no `-wal`, no `-shm`.
pub(super) fn journal_beside(private: &Path) -> PathBuf {
    let mut journal = private.as_os_str().to_os_string();
    journal.push("-journal");
    PathBuf::from(journal)
}

/// Remove private creation files beside `db_path` whose creator is provably
/// gone, and leave every other one exactly where it is.
///
/// A creator killed between `create_new` and its publish leaves its private
/// file (and possibly its `-journal`) behind. Nothing downstream will ever
/// look at those files again — the name carries a nonce, so the next creation
/// picks a different one — so they are pure leakage, and collecting them is
/// the honest thing to do the next time a launch finds no database here.
///
/// The dangerous mistake is collecting a file whose creator is *still
/// working*: that is a live sibling's private database mid-migration, and
/// deleting it destroys work in flight for no gain. So the question asked here
/// is not "is this file old" or "does this look abandoned" but "is the process
/// named in this file's own name gone", answered by the crate's one production
/// liveness probe ([`crate::session::supervision::observe`], which has a
/// macOS, a Linux and a Windows arm).
///
/// Every failure here is swallowed: a directory that cannot be listed, a file
/// that cannot be removed, a name that does not parse. None of them is a
/// reason to refuse to bootstrap a project, and the cost of each is a few
/// kilobytes.
fn sweep_abandoned_private_files(db_path: &Path) {
    let (Some(dir), Some(name)) = (
        db_path.parent(),
        db_path.file_name().and_then(|name| name.to_str()),
    ) else {
        return;
    };
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let candidate = entry.file_name();
        let Some(candidate) = candidate.to_str() else {
            continue;
        };
        // Never anything that does not match the pattern exactly. Neighbours
        // of a database file are a user's business — a backup, an export, a
        // sidecar from some other tool — and this sweep is not entitled to any
        // of them.
        let Some((pid, started_at_ms)) = parse_private_name(name, candidate) else {
            continue;
        };
        match creator_is_gone(pid, started_at_ms) {
            CreatorLiveness::Working => {}
            CreatorLiveness::Recycled => {
                // Provably not the creator: something else answers to that pid
                // now, so the creator exited without publishing. Worth saying
                // once, because it means a Glasshouse died mid-creation *and*
                // the machine has cycled through a whole pid space since.
                tracing::warn!(
                    private = %entry.path().display(),
                    pid,
                    "removing a private database left by a crashed Glasshouse; \
                     its process id now belongs to an unrelated process"
                );
                discard_private_file(&entry.path());
            }
            CreatorLiveness::Gone => discard_private_file(&entry.path()),
        }
    }
}

/// What the liveness probe says about the process named in a private file's
/// name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum CreatorLiveness {
    /// The creator is running. Its file is its own; do not touch it. Also the
    /// answer when the file records no start time (`0`) and something is
    /// running under its pid, because that pair cannot be told apart from the
    /// creator and the safe direction is to leak.
    Working,
    /// Nothing is running under that pid, or what is there is a zombie —
    /// which holds nothing, answers nothing, and is never a creator.
    Gone,
    /// Something is running under that pid and it is not the process that
    /// created this file: the pid was recycled. The creator is as gone as
    /// `Gone`, and this is worth a word in the log.
    Recycled,
}

/// Ask the machine about the process a private file names.
///
/// The start time is what makes this more than `kill(pid, 0)`. Pids are
/// recycled, and a leftover that outlives a full turn of the pid space would
/// otherwise pin itself in place forever behind an unrelated process — the
/// design note accepted that leak; recording the creator's start time removes
/// it, because a live pid whose start time is not the recorded one is
/// *provably* not the creator.
pub(super) fn creator_is_gone(pid: u32, started_at_ms: i64) -> CreatorLiveness {
    match crate::session::supervision::observe(pid) {
        None => CreatorLiveness::Gone,
        Some(observed) if !observed.is_live() => CreatorLiveness::Gone,
        Some(_) if started_at_ms == 0 => CreatorLiveness::Working,
        Some(observed) if observed.started_at_ms == started_at_ms => CreatorLiveness::Working,
        Some(_) => CreatorLiveness::Recycled,
    }
}

/// Read a private file's name back, or `None` if `candidate` is not one.
///
/// Exact, not approximate: the name must be the database's own file name, then
/// [`PRIVATE_INFIX`], then a decimal pid, then sixteen hex digits of start
/// time, then sixteen hex digits of nonce, and nothing else. Anything that
/// does not parse is somebody else's file.
pub(super) fn parse_private_name(db_file_name: &str, candidate: &str) -> Option<(u32, i64)> {
    let suffix = candidate
        .strip_prefix(db_file_name)?
        .strip_prefix(PRIVATE_INFIX)?;
    let mut fields = suffix.split('-');
    let pid = fields.next()?;
    let started = fields.next()?;
    let nonce = fields.next()?;
    if fields.next().is_some() {
        return None;
    }
    if started.len() != 16 || nonce.len() != 16 {
        return None;
    }
    // `from_str_radix` accepts a leading `+`; these fields never carry one.
    if !pid.bytes().all(|byte| byte.is_ascii_digit())
        || !started.bytes().all(|byte| byte.is_ascii_hexdigit())
        || !nonce.bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        return None;
    }
    Some((
        pid.parse().ok()?,
        u64::from_str_radix(started, 16).ok()? as i64,
    ))
}

/// A test-only hook, run on the creating thread once its private file exists
/// and before anything has been written to it.
///
/// Thread-local rather than global because the tests that use it need exactly
/// one thread held while another runs the same code path unheld. Compiled out
/// entirely of any build without `cfg(test)`.
#[cfg(test)]
type PrivateFileHold = Box<dyn FnMut(&Path) + Send>;

#[cfg(test)]
thread_local! {
    static PRIVATE_FILE_HOLD: std::cell::RefCell<Option<PrivateFileHold>> =
        const { std::cell::RefCell::new(None) };
}

/// Install a hold on this thread. Returns the previous one, if any.
#[cfg(test)]
pub(super) fn install_private_file_hold(hold: PrivateFileHold) -> Option<PrivateFileHold> {
    PRIVATE_FILE_HOLD.with(|cell| cell.borrow_mut().replace(hold))
}

/// Run this thread's hold, if it has one. The hook is taken out of the cell
/// for the duration so that a re-entrant creation cannot double-borrow it.
#[cfg(test)]
fn hold_private_file(private: &Path) {
    let hold = PRIVATE_FILE_HOLD.with(|cell| cell.borrow_mut().take());
    if let Some(mut hold) = hold {
        hold(private);
        PRIVATE_FILE_HOLD.with(|cell| *cell.borrow_mut() = Some(hold));
    }
}
