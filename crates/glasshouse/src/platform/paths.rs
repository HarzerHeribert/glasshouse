//! Platform-correct path normalization.
//!
//! Path comparison rules differ per operating system. These helpers keep that
//! knowledge in one place so the project-boundary check never has to guess, and
//! so normalization can never be *more* permissive than the underlying
//! filesystem actually is.

use std::path::{Component, Path};

/// Normalize a canonical path into the string used for identity and comparison.
///
/// On Windows the extended-length `\\?\` prefix produced by `canonicalize` is
/// stripped and the result is lowercased, because Windows paths are
/// case-insensitive. Elsewhere the canonical path is used verbatim: Linux is
/// case-sensitive, and macOS canonicalization is case-preserving and stable for
/// a given file, so lowercasing there would merge paths the boundary check must
/// keep distinct.
pub fn normalize(path: &Path) -> String {
    let s = path.to_string_lossy();

    #[cfg(windows)]
    {
        let s = s.strip_prefix(r"\\?\").unwrap_or(&s);
        return s.to_lowercase();
    }

    #[cfg(not(windows))]
    {
        s.into_owned()
    }
}

/// True when `path` is `root` itself or lies inside `root`.
///
/// Both arguments must already be canonical (symlinks resolved). The comparison
/// is component-wise so `/proj` never matches `/project-two`.
pub fn is_within(root: &Path, path: &Path) -> bool {
    let root_components = comparable_components(root);
    let path_components = comparable_components(path);

    if path_components.len() < root_components.len() {
        return false;
    }
    root_components
        .iter()
        .zip(path_components.iter())
        .all(|(a, b)| a == b)
}

/// True when both canonical paths denote the same location.
pub fn same_path(a: &Path, b: &Path) -> bool {
    comparable_components(a) == comparable_components(b)
}

fn comparable_components(path: &Path) -> Vec<String> {
    path.components()
        .map(|c| match c {
            // Prefix and root components carry Windows drive letters, which are
            // case-insensitive like the rest of the path.
            Component::Prefix(_) | Component::RootDir => casefold(&c.as_os_str().to_string_lossy()),
            other => casefold(&other.as_os_str().to_string_lossy()),
        })
        .collect()
}

fn casefold(s: &str) -> String {
    #[cfg(windows)]
    {
        s.to_lowercase()
    }
    #[cfg(not(windows))]
    {
        s.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn within_requires_component_boundary() {
        let root = PathBuf::from("/tmp/proj");
        assert!(is_within(&root, &PathBuf::from("/tmp/proj")));
        assert!(is_within(&root, &PathBuf::from("/tmp/proj/src/main.rs")));
        assert!(!is_within(&root, &PathBuf::from("/tmp/project-two/src")));
        assert!(!is_within(&root, &PathBuf::from("/tmp")));
        assert!(!is_within(&root, &PathBuf::from("/etc/passwd")));
    }

    #[test]
    fn same_path_is_reflexive_and_strict() {
        assert!(same_path(
            &PathBuf::from("/tmp/proj"),
            &PathBuf::from("/tmp/proj")
        ));
        assert!(!same_path(
            &PathBuf::from("/tmp/proj"),
            &PathBuf::from("/tmp/proj/sub")
        ));
    }
}
