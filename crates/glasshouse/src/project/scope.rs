//! The project boundary.
//!
//! [`ProjectScope`] is the single object every Glasshouse component must go
//! through before touching a path on behalf of a session, an agent, or a
//! harness. It resolves symlinks first and then enforces containment, so a
//! symlink inside the project cannot be used to reach outside it.

use std::path::{Component, Path, PathBuf};

use crate::project::platform;

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
}

/// A canonical project root plus the containment rules that apply to it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectScope {
    root: PathBuf,
}

impl ProjectScope {
    /// Build a scope from an already canonical root.
    pub fn new(canonical_root: PathBuf) -> Self {
        Self {
            root: canonical_root,
        }
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
    pub fn resolve(&self, candidate: impl AsRef<Path>) -> Result<PathBuf, ScopeError> {
        let candidate = candidate.as_ref();
        let joined = if candidate.is_absolute() {
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
        ProjectScope::new(std::fs::canonicalize(dir).unwrap())
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
