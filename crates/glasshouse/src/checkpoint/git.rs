//! Where the repository is standing, read cheaply.
//!
//! The map asks a checkpoint to *include the current Git branch and commit
//! when available*, and "when available" is doing real work: a project need
//! not be a Git repository at all, and Glasshouse must still be able to take
//! a checkpoint.
//!
//! # No subprocess
//!
//! This opens two or three small files and parses them. It does not run
//! `git`, and that is deliberate rather than incidental:
//!
//! - a checkpoint can be taken at a task boundary, on a thread that is also
//!   serving a terminal, and spawning a process there is a latency nobody
//!   asked for;
//! - `git` need not be installed for a `.git` directory to exist and be
//!   readable — a repository cloned onto a machine whose Git was uninstalled
//!   is still a repository;
//! - a subprocess inherits an environment, and `GIT_DIR` in that environment
//!   would silently point this at another repository.
//!
//! # Worktrees, which is the case that actually bites
//!
//! In a linked worktree `.git` is a **file** holding `gitdir: <path>`, that
//! directory has its own `HEAD` and its own `commondir`, and the refs live in
//! the *common* directory rather than beside the HEAD. Glasshouse's own
//! development happens in linked worktrees, so a reader that only handled the
//! `.git`-is-a-directory case would have reported nothing in exactly the
//! situation this project runs in every day. Both shapes are handled, and
//! both are tested against real fixtures.

use std::path::{Path, PathBuf};

/// A repository position: the commit, and the branch pointing at it when
/// HEAD is not detached.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitPosition {
    /// The short branch name — `main`, not `refs/heads/main`. `None` on a
    /// detached HEAD, which is a different fact from "no branch recorded" and
    /// stays distinguishable because the commit is always present.
    pub branch: Option<String>,
    /// The full object name HEAD resolves to.
    pub commit: String,
}

impl GitPosition {
    /// Rebuild from what a checkpoint document stored.
    ///
    /// A commit is what makes a position a position, so a document with a
    /// branch and no commit yields `None` rather than half a position.
    pub(crate) fn from_parts(branch: Option<String>, commit: Option<String>) -> Option<Self> {
        commit.map(|commit| Self { branch, commit })
    }

    /// Read the repository containing `project_root`, or `None`.
    ///
    /// Never fails: every way of not finding a position — no repository, an
    /// unreadable HEAD, a branch whose ref file is missing — is the same
    /// answer to the caller, because a checkpoint is worth taking either way.
    pub fn detect(project_root: &Path) -> Option<Self> {
        let (git_dir, common_dir) = resolve_git_dir(project_root)?;
        let head = read_trimmed(&git_dir.join("HEAD"))?;

        let Some(reference) = head.strip_prefix("ref: ") else {
            // Detached: HEAD holds the object name itself.
            if !is_object_name(&head) {
                return None;
            }
            return Some(Self {
                branch: None,
                commit: head,
            });
        };
        let reference = reference.trim();

        // A loose ref lives under the directory that owns it — the worktree's
        // own for `HEAD`-adjacent refs, the common one for shared branches —
        // and a packed one lives only in `packed-refs`, in the common
        // directory. Both are ordinary, so both are looked for.
        let commit = read_trimmed(&git_dir.join(reference))
            .or_else(|| read_trimmed(&common_dir.join(reference)))
            .or_else(|| packed_ref(&common_dir, reference))?;
        if !is_object_name(&commit) {
            return None;
        }

        Some(Self {
            branch: Some(
                reference
                    .strip_prefix("refs/heads/")
                    .unwrap_or(reference)
                    .to_owned(),
            ),
            commit,
        })
    }
}

/// The directory holding this checkout's `HEAD`, and the one holding the
/// repository's shared refs. They are the same directory except in a linked
/// worktree.
fn resolve_git_dir(project_root: &Path) -> Option<(PathBuf, PathBuf)> {
    let dot_git = project_root.join(".git");
    let metadata = std::fs::symlink_metadata(&dot_git).ok()?;

    let git_dir = if metadata.is_dir() {
        dot_git
    } else {
        // A linked worktree, or a submodule: `.git` is a file naming the real
        // directory. The path may be relative, and is then relative to the
        // project root rather than to the file.
        let pointer = read_trimmed(&dot_git)?;
        let target = Path::new(pointer.strip_prefix("gitdir:")?.trim());
        if target.is_absolute() {
            target.to_path_buf()
        } else {
            project_root.join(target)
        }
    };

    // `commondir` is present only in a linked worktree, and names the main
    // `.git` — relative to the worktree's own git directory when it is not
    // absolute.
    let common_dir = match read_trimmed(&git_dir.join("commondir")) {
        Some(common) => {
            let common = Path::new(&common);
            if common.is_absolute() {
                common.to_path_buf()
            } else {
                git_dir.join(common)
            }
        }
        None => git_dir.clone(),
    };

    Some((git_dir, common_dir))
}

/// The object name `reference` is packed at, if it is packed.
///
/// `packed-refs` is one ref per line, `<object-name> <ref>`, with `#` comment
/// lines and `^`-prefixed peeled tag lines that are deliberately skipped —
/// a peeled line describes the *previous* line's tag, and reading one as a
/// ref would attribute the wrong object to the wrong name.
fn packed_ref(common_dir: &Path, reference: &str) -> Option<String> {
    let packed = std::fs::read_to_string(common_dir.join("packed-refs")).ok()?;
    for line in packed.lines() {
        let line = line.trim_end();
        if line.starts_with('#') || line.starts_with('^') {
            continue;
        }
        if let Some((object, name)) = line.split_once(' ')
            && name == reference
        {
            return Some(object.to_owned());
        }
    }
    None
}

fn read_trimmed(path: &Path) -> Option<String> {
    let text = std::fs::read_to_string(path).ok()?;
    let trimmed = text.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_owned())
}

/// Whether `value` looks like a Git object name.
///
/// Checked so that a `HEAD` holding something unexpected — a stale
/// `ref: refs/…` written by a tool mid-operation, a truncated file — is
/// reported as no position rather than recorded as a commit that does not
/// exist. Both the 40-character SHA-1 form and the 64-character SHA-256 form
/// are accepted, because a repository may be either.
fn is_object_name(value: &str) -> bool {
    matches!(value.len(), 40 | 64) && value.chars().all(|c| c.is_ascii_hexdigit())
}

#[cfg(test)]
mod tests {
    use super::*;

    const COMMIT: &str = "0123456789abcdef0123456789abcdef01234567";
    const OTHER: &str = "fedcba9876543210fedcba9876543210fedcba98";

    fn write(path: &Path, contents: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, contents).unwrap();
    }

    /// The ordinary case: a checkout with a `.git` directory, on a branch.
    #[test]
    fn a_plain_checkout_reports_its_branch_and_commit() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        write(&root.join(".git/HEAD"), "ref: refs/heads/main\n");
        write(&root.join(".git/refs/heads/main"), &format!("{COMMIT}\n"));

        assert_eq!(
            GitPosition::detect(root),
            Some(GitPosition {
                branch: Some("main".to_owned()),
                commit: COMMIT.to_owned(),
            })
        );
    }

    /// **The case Glasshouse's own development runs in.** A linked worktree's
    /// `.git` is a file, its `HEAD` is beside the file it names, and the
    /// branch it is on lives in the *common* directory — so a reader that
    /// looked only next to `HEAD` would find nothing here.
    #[test]
    fn a_linked_worktree_resolves_through_its_pointer_and_common_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let main = tmp.path().join("main");
        let linked = tmp.path().join("linked");
        let worktree_git = main.join(".git/worktrees/linked");

        write(&main.join(".git/HEAD"), "ref: refs/heads/main\n");
        write(&main.join(".git/refs/heads/main"), &format!("{OTHER}\n"));
        write(&worktree_git.join("HEAD"), "ref: refs/heads/side\n");
        write(&worktree_git.join("commondir"), &format!("{}\n", "../.."));
        write(&main.join(".git/refs/heads/side"), &format!("{COMMIT}\n"));
        write(
            &linked.join(".git"),
            &format!("gitdir: {}\n", worktree_git.display()),
        );

        assert_eq!(
            GitPosition::detect(&linked),
            Some(GitPosition {
                branch: Some("side".to_owned()),
                commit: COMMIT.to_owned(),
            }),
            "a linked worktree must report its own branch, resolved through the \
             common directory"
        );
        // And the main checkout still reports its own, unchanged.
        assert_eq!(
            GitPosition::detect(&main).unwrap().commit,
            OTHER,
            "reading the worktree must not have disturbed the main checkout"
        );
    }

    /// A relative `gitdir:` pointer resolves against the project root, which
    /// is what Git itself writes for a worktree beside its repository.
    #[test]
    fn a_relative_gitdir_pointer_resolves_against_the_project_root() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("wt");
        write(&root.join("real/HEAD"), "ref: refs/heads/main\n");
        write(&root.join("real/refs/heads/main"), &format!("{COMMIT}\n"));
        write(&root.join(".git"), "gitdir: real\n");

        assert_eq!(
            GitPosition::detect(&root).unwrap().commit,
            COMMIT.to_owned()
        );
    }

    /// A branch that has been packed has no loose ref file at all. Reading
    /// only loose refs would report nothing for a freshly cloned repository,
    /// where every branch is packed.
    #[test]
    fn a_packed_branch_is_found_in_packed_refs() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        write(&root.join(".git/HEAD"), "ref: refs/heads/main\n");
        write(
            &root.join(".git/packed-refs"),
            &format!(
                "# pack-refs with: peeled fully-peeled sorted\n\
                 {OTHER} refs/heads/other\n\
                 {COMMIT} refs/heads/main\n\
                 {OTHER} refs/tags/v1\n\
                 ^{COMMIT}\n"
            ),
        );

        assert_eq!(
            GitPosition::detect(root),
            Some(GitPosition {
                branch: Some("main".to_owned()),
                commit: COMMIT.to_owned(),
            })
        );
    }

    /// A peeled tag line must not be read as a ref. `^<object>` describes the
    /// line above it, and taking it as a `<object> <name>` pair would
    /// attribute one tag's object to whatever parsed next.
    #[test]
    fn a_peeled_tag_line_is_not_mistaken_for_a_ref() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        write(&root.join(".git/HEAD"), "ref: refs/heads/missing\n");
        write(
            &root.join(".git/packed-refs"),
            &format!("{OTHER} refs/tags/v1\n^{COMMIT}\n"),
        );
        assert_eq!(
            GitPosition::detect(root),
            None,
            "a branch that is not in packed-refs must not resolve to a tag's object"
        );
    }

    /// A detached HEAD has a commit and no branch, and the two facts stay
    /// separable — `None` here means detached, never "not recorded", because
    /// a position without a commit is not returned at all.
    #[test]
    fn a_detached_head_reports_its_commit_and_no_branch() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        write(&root.join(".git/HEAD"), &format!("{COMMIT}\n"));

        assert_eq!(
            GitPosition::detect(root),
            Some(GitPosition {
                branch: None,
                commit: COMMIT.to_owned(),
            })
        );
    }

    /// Every way of having no position answers the same way, because a
    /// checkpoint is worth taking without one.
    #[test]
    fn anything_unreadable_is_simply_no_position() {
        let tmp = tempfile::tempdir().unwrap();

        // Not a repository at all.
        let plain = tmp.path().join("plain");
        std::fs::create_dir_all(&plain).unwrap();
        assert_eq!(GitPosition::detect(&plain), None);

        // A repository mid-operation, whose branch ref is not written yet.
        let pending = tmp.path().join("pending");
        write(&pending.join(".git/HEAD"), "ref: refs/heads/main\n");
        assert_eq!(GitPosition::detect(&pending), None);

        // A HEAD holding something that is not an object name. Reporting it
        // would put a commit that does not exist into a handoff document.
        let nonsense = tmp.path().join("nonsense");
        write(&nonsense.join(".git/HEAD"), "not a commit\n");
        assert_eq!(GitPosition::detect(&nonsense), None);

        let truncated = tmp.path().join("truncated");
        write(&truncated.join(".git/HEAD"), "ref: refs/heads/main\n");
        write(&truncated.join(".git/refs/heads/main"), "0123456\n");
        assert_eq!(GitPosition::detect(&truncated), None);
    }

    /// A SHA-256 repository's object names are 64 characters. Accepting only
    /// 40 would silently report no position for every one of them.
    #[test]
    fn a_sha256_object_name_is_accepted() {
        assert!(is_object_name(&"a".repeat(64)));
        assert!(is_object_name(COMMIT));
        assert!(!is_object_name(&"a".repeat(41)));
        assert!(!is_object_name(&"g".repeat(40)));
    }

    /// The real repository this test is running in.
    ///
    /// Every case above is a fixture, and a fixture only proves the parser
    /// against what the test author believed Git writes. This one reads the
    /// actual checkout — which, for this project, is normally a linked
    /// worktree — and holds the result to what the repository itself says.
    #[test]
    fn the_repository_this_test_runs_in_is_readable() {
        // `CARGO_MANIFEST_DIR` is `<checkout>/crates/glasshouse`.
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(Path::parent)
            .expect("the manifest is two directories below the checkout root")
            .to_path_buf();
        if !the_repository_is_actually_readable(&root) {
            // A copied tree — see the helper below. Reporting no position is
            // the correct answer here, and asserting it is what keeps this
            // from being a silent skip.
            assert_eq!(
                GitPosition::detect(&root),
                None,
                "a repository whose HEAD cannot be read must report no \
                 position, never a half-read one"
            );
            return;
        }

        let position = GitPosition::detect(&root)
            .expect("this checkout's HEAD is readable, so a position must be readable from it");
        assert!(
            is_object_name(&position.commit),
            "read a commit that is not an object name: {position:?}"
        );
    }

    /// Whether this checkout's `.git` entry actually leads anywhere.
    ///
    /// **A `.git` entry existing is not the same as a repository being
    /// readable**, and the difference is not exotic — it is the ordinary state
    /// of a *copied* tree. Glasshouse is developed in linked git worktrees,
    /// where `.git` is a file holding `gitdir: <absolute path>`; copy that
    /// tree into a container, a source tarball, or an image build, and the
    /// file arrives while the directory it names does not.
    ///
    /// A Linux container run caught exactly that, on the version of this test
    /// that asserted a position must be readable *because a `.git` entry
    /// exists*. It passed on the machine it was written on, where the pointer
    /// resolves, and could not have failed there. Reporting no position in a
    /// copied tree is the **correct** answer, so the premise is now checked by
    /// resolving the pointer rather than by testing for the entry.
    fn the_repository_is_actually_readable(root: &Path) -> bool {
        let dot_git = root.join(".git");
        let Ok(metadata) = std::fs::symlink_metadata(&dot_git) else {
            return false;
        };
        let git_dir = if metadata.is_dir() {
            dot_git
        } else {
            let Ok(pointer) = std::fs::read_to_string(&dot_git) else {
                return false;
            };
            let Some(target) = pointer.trim().strip_prefix("gitdir:") else {
                return false;
            };
            let target = Path::new(target.trim());
            if target.is_absolute() {
                target.to_path_buf()
            } else {
                root.join(target)
            }
        };
        git_dir.join("HEAD").is_file()
    }
}
