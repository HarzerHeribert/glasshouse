//! The project boundary.
//!
//! [`ProjectScope`] is the single object every Glasshouse component must go
//! through before touching a path on behalf of a session, an agent, or a
//! harness. It resolves symlinks first and then enforces containment, so a
//! symlink inside the project cannot be used to reach outside it.

use std::path::{Component, Path, PathBuf};

use crate::platform::paths as platform;

/// A path was rejected because it does not resolve inside the project root.
#[derive(Debug, thiserror::Error)]
pub enum ScopeError {
    #[error("path `{path}` resolves to `{resolved}`, which is outside the project root `{root}`")]
    OutsideProject {
        path: PathBuf,
        resolved: PathBuf,
        root: PathBuf,
    },
    #[error("path `{path}` traverses above the project root `{root}`")]
    Traversal { path: PathBuf, root: PathBuf },
    #[error("could not resolve path `{path}`: {source}")]
    Resolve {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("path must not be empty")]
    EmptyPath,
    #[error("path `{path}` contains a NUL byte")]
    ContainsNul { path: PathBuf },
}

/// A canonical project root plus the containment rules that apply to it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectScope {
    root: PathBuf,
}

impl ProjectScope {
    /// Build a scope from an already canonical root.
    ///
    /// The entire containment guarantee rests on `canonical_root` truly being
    /// canonical, and that was previously asserted only in this doc comment
    /// on a public constructor — a footgun on the crate's security-critical
    /// type. This stays crate-private so the only public way to build a
    /// scope is [`ProjectScope::for_root`], which enforces the invariant
    /// instead of merely documenting it.
    pub(crate) fn new(canonical_root: PathBuf) -> Self {
        Self {
            root: canonical_root,
        }
    }

    /// Build a scope for `root`, canonicalizing it first.
    ///
    /// This is the public constructor. Callers that already hold a canonical
    /// path (such as [`crate::project::Project::discover`], which
    /// canonicalizes before making any access-control decision) can still
    /// reach the crate-private [`ProjectScope::new`] to skip the redundant
    /// syscall.
    pub fn for_root(root: &Path) -> std::io::Result<Self> {
        Ok(Self::new(std::fs::canonicalize(root)?))
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// True when `candidate` is already canonical and inside the project.
    ///
    /// Prefer [`ProjectScope::resolve`] for anything that has not been resolved
    /// yet; this is the cheap check for paths the caller canonicalized itself.
    pub fn contains_canonical(&self, candidate: &Path) -> bool {
        platform::is_within(&self.root, candidate)
    }

    /// Resolve a caller-supplied path against the project root and prove it
    /// stays inside.
    ///
    /// Relative paths are interpreted against the project root. Symlinks are
    /// resolved before the containment check for every component that exists on
    /// disk, and any `..` in the not-yet-existing tail is folded lexically and
    /// re-checked. The path does not have to exist.
    ///
    /// This returns a *path*, not an open handle, and the guarantee ends the
    /// moment it returns: nothing stops the filesystem underneath it from
    /// changing before a caller opens it. When real file operations are
    /// added on top of this, they should open with `O_NOFOLLOW` /
    /// `openat2(RESOLVE_BENEATH)` on Linux and `FILE_FLAG_OPEN_REPARSE_POINT`
    /// on Windows rather than re-checking the returned string — that is
    /// future work, not something this method can promise on its own.
    pub fn resolve(&self, candidate: impl AsRef<Path>) -> Result<PathBuf, ScopeError> {
        let candidate = candidate.as_ref();

        if candidate.as_os_str().is_empty() {
            // `resolve("")` would otherwise join to nothing and return the
            // project root itself — silently handing back the one path a
            // caller almost certainly did not mean to name.
            return Err(ScopeError::EmptyPath);
        }
        if contains_nul(candidate) {
            // An interior NUL is never a valid path component; catch it here
            // with a clear error instead of letting it resolve `Ok` and fail
            // later, opaquely, at the syscall boundary.
            return Err(ScopeError::ContainsNul {
                path: candidate.to_path_buf(),
            });
        }

        let joined = if is_anchored(candidate) {
            candidate.to_path_buf()
        } else {
            self.root.join(candidate)
        };

        let (existing, tail) = split_at_existing_ancestor(&joined);

        // `canonicalize` resolves every symlink in the existing prefix,
        // including a symlinked final component.
        let canonical_existing =
            std::fs::canonicalize(&existing).map_err(|source| ScopeError::Resolve {
                path: existing.clone(),
                source,
            })?;

        let mut resolved = canonical_existing;
        for component in tail {
            match component {
                Component::CurDir => {}
                Component::ParentDir => {
                    // A `..` in the non-existent tail cannot cross a symlink,
                    // so folding it lexically is safe. Popping past the root is
                    // caught by the containment check below, but reject it
                    // explicitly for a clearer error.
                    if !resolved.pop() {
                        return Err(ScopeError::Traversal {
                            path: candidate.to_path_buf(),
                            root: self.root.clone(),
                        });
                    }
                }
                Component::Normal(part) => resolved.push(part),
                Component::RootDir | Component::Prefix(_) => {
                    return Err(ScopeError::Traversal {
                        path: candidate.to_path_buf(),
                        root: self.root.clone(),
                    });
                }
            }
        }

        if platform::is_within(&self.root, &resolved) {
            Ok(resolved)
        } else {
            Err(ScopeError::OutsideProject {
                path: candidate.to_path_buf(),
                resolved,
                root: self.root.clone(),
            })
        }
    }
}

/// True when `candidate`'s first component anchors it independently of the
/// project root: a root directory, or a Windows drive/UNC prefix.
///
/// `Path::is_absolute` misses a Windows drive-relative path like `C:foo`
/// (a `Component::Prefix` with no following `Component::RootDir`) — and
/// `PathBuf::push` *replaces* the whole path when the pushed argument itself
/// carries a prefix, so joining such a candidate onto the project root
/// silently discards the root and resolves against the process's per-drive
/// current directory instead. Treating any prefixed-or-rooted candidate as
/// already anchored (a superset of `is_absolute`) means it is never joined
/// onto `root` in the first place; containment is still enforced afterwards
/// by the check at the end of `resolve`.
fn is_anchored(candidate: &Path) -> bool {
    matches!(
        candidate.components().next(),
        Some(Component::Prefix(_) | Component::RootDir)
    )
}

/// True when `path` contains an interior NUL byte, which no valid path
/// component can contain.
fn contains_nul(path: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        path.as_os_str().as_bytes().contains(&0)
    }
    #[cfg(not(unix))]
    {
        use std::os::windows::ffi::OsStrExt;
        path.as_os_str().encode_wide().any(|c| c == 0)
    }
}

/// Split `path` into its deepest existing ancestor and the remaining components.
fn split_at_existing_ancestor(path: &Path) -> (PathBuf, Vec<Component<'_>>) {
    let mut components: Vec<Component<'_>> = path.components().collect();
    let mut tail: Vec<Component<'_>> = Vec::new();

    loop {
        let candidate: PathBuf = components.iter().collect();
        if !candidate.as_os_str().is_empty() && candidate.symlink_metadata().is_ok() {
            tail.reverse();
            return (candidate, tail);
        }
        match components.pop() {
            Some(last) => tail.push(last),
            None => {
                tail.reverse();
                // Nothing on this path exists; fall back to the filesystem root
                // so `canonicalize` produces a meaningful error.
                return (PathBuf::from(std::path::MAIN_SEPARATOR.to_string()), tail);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scope(dir: &Path) -> ProjectScope {
        ProjectScope::for_root(dir).unwrap()
    }

    #[test]
    fn resolves_paths_inside_the_project() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir(tmp.path().join("src")).unwrap();
        std::fs::write(tmp.path().join("src/main.rs"), "fn main() {}").unwrap();
        let scope = scope(tmp.path());

        let resolved = scope.resolve("src/main.rs").unwrap();
        assert!(resolved.ends_with("src/main.rs"));
        assert!(scope.contains_canonical(&resolved));
    }

    #[test]
    fn resolves_paths_that_do_not_exist_yet() {
        let tmp = tempfile::tempdir().unwrap();
        let scope = scope(tmp.path());
        let resolved = scope.resolve("build/output/report.json").unwrap();
        assert!(resolved.ends_with("build/output/report.json"));
    }

    #[test]
    fn rejects_dot_dot_traversal() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir(tmp.path().join("src")).unwrap();
        let scope = scope(tmp.path());

        assert!(scope.resolve("../secrets.txt").is_err());
        assert!(scope.resolve("src/../../secrets.txt").is_err());
        assert!(scope.resolve("src/../..").is_err());
    }

    #[test]
    fn rejects_absolute_paths_outside_the_project() {
        let tmp = tempfile::tempdir().unwrap();
        let scope = scope(tmp.path());
        assert!(scope.resolve("/etc").is_err());
    }

    #[test]
    fn accepts_absolute_paths_inside_the_project() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("a.txt"), "x").unwrap();
        let scope = scope(tmp.path());
        let inside = scope.root().join("a.txt");
        assert!(scope.resolve(&inside).is_ok());
    }

    #[test]
    fn rejects_an_empty_candidate() {
        let tmp = tempfile::tempdir().unwrap();
        let scope = scope(tmp.path());
        assert!(matches!(scope.resolve(""), Err(ScopeError::EmptyPath)));
    }

    #[test]
    fn rejects_a_candidate_containing_a_nul_byte() {
        let tmp = tempfile::tempdir().unwrap();
        let scope = scope(tmp.path());
        let err = scope.resolve("foo\0bar").unwrap_err();
        assert!(matches!(err, ScopeError::ContainsNul { .. }), "{err}");
    }

    #[test]
    fn is_anchored_classifies_rooted_paths() {
        assert!(is_anchored(Path::new(std::path::MAIN_SEPARATOR_STR)));
        assert!(is_anchored(
            &PathBuf::from(std::path::MAIN_SEPARATOR_STR).join("foo")
        ));
        assert!(!is_anchored(Path::new("foo")));
        assert!(!is_anchored(Path::new("foo/bar")));
    }

    #[cfg(windows)]
    #[test]
    fn is_anchored_classifies_drive_relative_windows_paths() {
        // `C:foo` carries a drive prefix with no root component — the shape
        // `Path::is_absolute` misses and that `PathBuf::push` mishandles.
        assert!(is_anchored(Path::new("C:foo")));
        assert!(is_anchored(Path::new(r"C:\foo")));
    }

    #[cfg(windows)]
    #[test]
    fn drive_relative_candidates_do_not_silently_drop_the_root() {
        let tmp = tempfile::tempdir().unwrap();
        let scope = scope(tmp.path());
        // Whatever this resolves to (it depends on the per-drive current
        // directory), it must not be treated as `root.join("C:foo")` — the
        // `PathBuf::push` semantics that would silently discard the root.
        let candidate = Path::new("C:foo");
        if let Ok(resolved) = scope.resolve(candidate) {
            assert_ne!(resolved, scope.root().join(candidate));
        }
    }

    #[cfg(unix)]
    #[test]
    fn rejects_symlink_escaping_the_project() {
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("secret.txt"), "secret").unwrap();

        let tmp = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink(outside.path(), tmp.path().join("escape")).unwrap();
        let scope = scope(tmp.path());

        assert!(scope.resolve("escape").is_err());
        assert!(scope.resolve("escape/secret.txt").is_err());
        // Also true for a path that does not exist behind the symlink.
        assert!(scope.resolve("escape/not-created-yet.txt").is_err());
    }

    #[cfg(unix)]
    #[test]
    fn accepts_symlink_that_stays_inside_the_project() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir(tmp.path().join("real")).unwrap();
        std::fs::write(tmp.path().join("real/file.txt"), "x").unwrap();
        std::os::unix::fs::symlink(tmp.path().join("real"), tmp.path().join("link")).unwrap();
        let scope = scope(tmp.path());

        assert!(scope.resolve("link/file.txt").is_ok());
    }
}
