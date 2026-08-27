//! Writing, and unwriting, the generated configuration documents a launch
//! overlay carries.
//!
//! # Why this is its own file
//!
//! `profile/mod.rs` may not name `std::fs`, `std::env`, or anything that
//! opens a file — `harness::resolving_a_launch_profile_touches_no_files`
//! enforces it, and the reason is worth restating rather than working
//! around: a module that never opens a file cannot modify the user's global
//! harness configuration, which is a structural guarantee rather than a
//! promise to avoid a list of paths.
//!
//! Line 362 asks for a generated configuration file, so *something* has to
//! write one. Putting it here rather than beside [`crate::profile::resolve`]
//! keeps the original guarantee exactly as strong as it was: **resolution
//! still opens nothing**, and the single function that does is one screen
//! long, takes the paths it is given, and is forbidden the ambient
//! environment by its own scan
//! (`harness::the_only_writer_in_profile_takes_its_paths_from_its_caller`).
//!
//! # What is guaranteed here, and what is guaranteed elsewhere
//!
//! Here: a document is written owner-only, and removed again when the
//! returned guard drops or when Glasshouse is forced down.
//!
//! Elsewhere: that the path is inside a directory Glasshouse owns. This file
//! never composes a path — it is handed
//! [`crate::profile::PendingConfig`]s whose paths came from
//! [`crate::harness::GeneratedConfigSite::file`], which is the only thing
//! allowed to decide where a generated document may live.

use std::path::{Path, PathBuf};

use super::PendingConfig;

/// The generated configuration documents written for one child process,
/// removed again when this drops.
///
/// Created only by [`crate::profile::LaunchOverlay::install`]. Holding one is
/// the whole contract: keep it for as long as the harness runs, drop it when
/// the session ends.
///
/// # Why removal is best effort
///
/// A file that cannot be removed — because the state directory went away
/// underneath the session, or the platform is holding it open — is logged and
/// left. The alternative is a panic in a destructor on the way out of a
/// session the user has just finished, which is a worse outcome than a stale
/// document, and `Drop` has nowhere to return an error to.
#[derive(Debug)]
pub struct EphemeralConfigs {
    paths: Vec<PathBuf>,
    /// Removes the same files if Glasshouse is forced down instead of
    /// exiting. Dropping this guard unregisters exactly that callback, so a
    /// finished session leaves nothing registered behind it.
    _forced_exit: crate::shutdown::ForcedExitGuard,
}

impl EphemeralConfigs {
    fn owning(paths: Vec<PathBuf>) -> Self {
        let registered = paths.clone();
        let forced_exit = crate::shutdown::on_forced_exit(move || {
            // Best effort and non-blocking, as `on_forced_exit` requires:
            // one `remove_file` per document, waiting on nothing.
            for path in &registered {
                let _ = std::fs::remove_file(path);
            }
        });
        Self {
            paths,
            _forced_exit: forced_exit,
        }
    }

    /// The documents this guard owns, for a caller that wants to say what was
    /// written. Paths, never contents.
    pub fn paths(&self) -> &[PathBuf] {
        &self.paths
    }
}

impl Drop for EphemeralConfigs {
    fn drop(&mut self) {
        for path in &self.paths {
            if let Err(err) = std::fs::remove_file(path)
                && err.kind() != std::io::ErrorKind::NotFound
            {
                tracing::warn!(
                    path = %path.display(),
                    error = %err,
                    "could not remove a generated harness configuration"
                );
            }
        }
    }
}

/// Write every document in `configs`, returning the guard that removes them.
///
/// The parent directory is created if it does not exist, because the
/// Glasshouse-owned session directory is created lazily by whichever writer
/// gets there first — `session::select::install_session_document` does the
/// same for the harness's settings document.
///
/// A partial failure still hands back a guard for what was written, so a
/// launch that is about to be abandoned does not also leave a document
/// behind: the caller drops the guard along with the error.
pub(super) fn write_all(
    configs: &[PendingConfig],
    paths: &[PathBuf],
) -> std::io::Result<EphemeralConfigs> {
    debug_assert_eq!(configs.len(), paths.len());
    let mut written: Vec<PathBuf> = Vec::new();
    for (config, path) in configs.iter().zip(paths) {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        if let Err(err) = write_private(path, config.contents()) {
            // Drop what has already been written before returning, rather
            // than leaving the earlier documents of a failed launch behind.
            drop(EphemeralConfigs::owning(written));
            return Err(err);
        }
        written.push(path.clone());
    }
    Ok(EphemeralConfigs::owning(written))
}

/// Write `contents` to `path`, owner-only where the platform has a mode to
/// set.
///
/// The mode is part of the `open` call rather than a `set_permissions`
/// afterwards: the two-step form leaves a window in which the file exists
/// with the default mode, and this file's whole purpose is to be a place a
/// harness reads configuration from.
///
/// A generated document carries no credential — an adapter is never handed
/// one, see [`crate::harness::DirectProviderRequest`] — but it does name a
/// user's provider, base URL and headers, and it is one edit away from
/// somebody deciding a value would be easier than a variable name. On Windows
/// the document inherits the state directory's own protection; there is no
/// mode to set.
fn write_private(path: &Path, contents: &str) -> std::io::Result<()> {
    use std::io::Write as _;

    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    file.write_all(contents.as_bytes())
}
