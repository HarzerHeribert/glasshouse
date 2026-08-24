//! Platform-correct path normalization.
//!
//! Path comparison rules differ per operating system. These helpers keep that
//! knowledge in one place so the project-boundary check never has to guess, and
//! so normalization can never be *more* permissive than the underlying
//! filesystem actually is.

use std::path::{Component, Path, PathBuf};

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
        // ASCII folding, for the same reason as `casefold` below: full Unicode
        // lowercasing merges characters NTFS keeps distinct, which would give
        // two different directories the same project identity.
        let s = s.strip_prefix(r"\\?\").unwrap_or(&s);
        s.to_ascii_lowercase()
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

/// Case-fold one path component for comparison.
///
/// Windows folding is ASCII-only on purpose. `str::to_lowercase` applies full
/// Unicode case mapping, which folds characters NTFS keeps distinct — U+212A
/// KELVIN SIGN lowercases to `k`, so `Proj\u{212a}` and `ProjK` would compare
/// equal even though they are two different directories. In a containment
/// check that is the wrong direction to be wrong in. ASCII folding matches
/// NTFS exactly in the ASCII range and can only ever fail closed outside it.
fn casefold(s: &str) -> String {
    #[cfg(windows)]
    {
        s.to_ascii_lowercase()
    }
    #[cfg(not(windows))]
    {
        s.to_owned()
    }
}

/// Strip Windows' extended-length `\\?\` prefix from a canonical path.
///
/// `fs::canonicalize` on Windows always returns a verbatim path. That form is
/// correct for identity and comparison, but several Win32 consumers reject it:
/// `CreateProcessW`'s `lpCurrentDirectory` does not reliably accept it, and
/// `cmd.exe` refuses to run with a verbatim current directory — which is
/// exactly the path Glasshouse takes to launch a `.cmd` harness shim in the
/// project root. It also reads as noise to a user.
///
/// So the verbatim form is kept internally and stripped at the boundary: when
/// handing a path to another process, or showing it to a person. A verbatim
/// UNC path (`\\?\UNC\server\share`) becomes a normal UNC path
/// (`\\server\share`). On other platforms this is the identity function.
pub fn strip_verbatim_prefix(path: &Path) -> PathBuf {
    #[cfg(windows)]
    {
        let text = path.to_string_lossy();
        if let Some(rest) = text.strip_prefix(r"\\?\UNC\") {
            return PathBuf::from(format!(r"\\{rest}"));
        }
        if let Some(rest) = text.strip_prefix(r"\\?\") {
            return PathBuf::from(rest);
        }
        path.to_path_buf()
    }
    #[cfg(not(windows))]
    {
        path.to_path_buf()
    }
}

/// True when both paths name the same file or directory on disk.
///
/// This asks the filesystem instead of comparing strings, which is the only
/// answer that is correct on a case-insensitive volume. Whether macOS
/// `realpath` normalizes case is not something Glasshouse can assume, and a
/// safety refusal that can be sidestepped by typing `/users/me` instead of
/// `/Users/me` would not be a safety refusal at all.
///
/// Returns `false` when either path cannot be read, so a caller that uses this
/// for a refusal fails closed only in the sense of not refusing — callers must
/// therefore use it for identity, never for containment.
pub fn same_file(a: &Path, b: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        match (std::fs::metadata(a), std::fs::metadata(b)) {
            (Ok(x), Ok(y)) => x.dev() == y.dev() && x.ino() == y.ino(),
            _ => false,
        }
    }
    #[cfg(not(unix))]
    {
        // Windows has no stable inode. Canonical paths plus the ASCII case
        // folding above are the best available comparison here, and NTFS
        // case-insensitivity makes it correct for the ASCII names these checks
        // actually deal with.
        match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
            (Ok(x), Ok(y)) => same_path(&x, &y),
            _ => false,
        }
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

    #[test]
    fn same_file_identifies_a_directory_through_two_paths() {
        let tmp = tempfile::tempdir().unwrap();
        let real = tmp.path().join("real");
        std::fs::create_dir(&real).unwrap();
        let indirect = tmp.path().join("real/../real");

        assert!(same_file(&real, &indirect));
        assert!(!same_file(&real, tmp.path()));
        assert!(!same_file(&real, &tmp.path().join("missing")));
    }

    #[test]
    fn strip_verbatim_prefix_is_identity_off_windows() {
        let p = PathBuf::from("/tmp/proj");
        assert_eq!(strip_verbatim_prefix(&p), p);
    }

    // The verbatim forms below can only be exercised where `\\?\` paths are
    // meaningful; on other platforms `strip_verbatim_prefix` is the identity
    // function and these inputs would round-trip unchanged.
    #[cfg(windows)]
    #[test]
    fn strip_verbatim_prefix_strips_drive_prefix_on_windows() {
        let p = PathBuf::from(r"\\?\C:\p");
        assert_eq!(strip_verbatim_prefix(&p), PathBuf::from(r"C:\p"));
    }

    #[cfg(windows)]
    #[test]
    fn strip_verbatim_prefix_rewrites_unc_prefix_on_windows() {
        let p = PathBuf::from(r"\\?\UNC\srv\share\p");
        assert_eq!(strip_verbatim_prefix(&p), PathBuf::from(r"\\srv\share\p"));
    }

    #[cfg(windows)]
    #[test]
    fn strip_verbatim_prefix_leaves_plain_paths_alone_on_windows() {
        let p = PathBuf::from(r"C:\p");
        assert_eq!(strip_verbatim_prefix(&p), p);
    }
}
