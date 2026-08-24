//! Project-root detection and hard project isolation.
//!
//! One Glasshouse instance operates on exactly one project root. Everything
//! downstream — state directories, databases, sessions, memory — is keyed by a
//! [`ProjectId`] derived from that root, so cross-project access is prevented
//! by physical separation rather than by query filters.

pub mod scope;

use std::fmt;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use sha2::{Digest, Sha256};

use crate::platform::paths as platform;

pub use scope::{ProjectScope, ScopeError};

/// How the active project root was selected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RootSource {
    /// Discovered by walking up to the containing Git repository.
    GitRepository,
    /// Selected explicitly with `--scope`.
    ExplicitScope,
    /// No Git repository was found; the working directory was used.
    WorkingDirectory,
}

impl fmt::Display for RootSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            RootSource::GitRepository => "git repository",
            RootSource::ExplicitScope => "explicit --scope",
            RootSource::WorkingDirectory => "working directory",
        };
        f.write_str(s)
    }
}

/// A project root that Glasshouse refuses to use without an explicit override.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum UnsafeRoot {
    #[error(
        "`{0}` is a filesystem root; scoping Glasshouse to it would place every project on the machine in one scope"
    )]
    FilesystemRoot(PathBuf),
    #[error(
        "`{0}` is your home directory; scoping Glasshouse to it would mix unrelated projects into one memory database"
    )]
    HomeDirectory(PathBuf),
    #[error(
        "`{0}` is the parent of home directories (it holds every user's home directory on this machine); scoping Glasshouse to it would mix every user's projects into one memory database"
    )]
    HomeDirectoryParent(PathBuf),
    #[error(
        "`{root}` looks like a container for multiple projects (found Git repositories: {})",
        .repositories.join(", ")
    )]
    MultiProjectContainer {
        root: PathBuf,
        repositories: Vec<String>,
    },
}

impl UnsafeRoot {
    /// Guidance printed alongside the refusal.
    pub fn remedy(&self) -> &'static str {
        match self {
            UnsafeRoot::MultiProjectContainer { .. } => {
                "Run Glasshouse from inside one project, or pass --scope <project-path> to select a narrower root."
            }
            _ => {
                "Run Glasshouse from inside a project, or pass --scope <project-path>. \
                 Use --allow-unsafe-scope only if you really mean this root."
            }
        }
    }
}

/// A stable identifier for a canonical project root.
///
/// The identifier is a short readable name plus a digest of the
/// platform-normalized canonical path. It is stable for a given canonical path
/// within the same operating-system environment, and distinct for any two
/// different roots.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ProjectId(String);

impl ProjectId {
    /// Derive the identifier from a canonical project root.
    pub fn from_canonical_root(root: &Path) -> Self {
        let normalized = platform::normalize(root);
        let digest = Sha256::digest(normalized.as_bytes());
        // 128 bits: two projects sharing an id would share one state
        // directory, one memory database, and one set of resumable sessions,
        // so the id needs to be collision-resistant, not just short. 64 bits
        // made an accidental collision negligible but left a deliberately
        // found one within reach (~2^32 work) for zero benefit over this.
        let short = hex::encode(&digest[..16]);

        let name = root
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let slug = slugify(&name);

        if slug.is_empty() {
            Self(short)
        } else {
            Self(format!("{slug}-{short}"))
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ProjectId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Reduce a directory name to a filesystem-safe, readable slug.
fn slugify(name: &str) -> String {
    let mut out = String::new();
    let mut last_dash = true;
    for ch in name.chars() {
        let mapped = if ch.is_ascii_alphanumeric() {
            ch.to_ascii_lowercase()
        } else {
            '-'
        };
        if mapped == '-' {
            if last_dash {
                continue;
            }
            last_dash = true;
        } else {
            last_dash = false;
        }
        out.push(mapped);
        if out.len() >= 32 {
            break;
        }
    }
    out.trim_matches('-').to_owned()
}

/// The active project: one canonical root, one identifier, one scope guard.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Project {
    id: ProjectId,
    scope: ProjectScope,
    source: RootSource,
    is_git_repository: bool,
    overridden_refusal: Option<UnsafeRoot>,
}

impl Project {
    /// Resolve the active project.
    ///
    /// `explicit_scope` corresponds to `--scope`. When it is absent, the
    /// containing Git repository of `cwd` is used, falling back to `cwd`
    /// itself. `allow_unsafe` corresponds to `--allow-unsafe-scope` and is the
    /// only way past the refusals in [`UnsafeRoot`].
    pub fn discover(cwd: &Path, explicit_scope: Option<&Path>, allow_unsafe: bool) -> Result<Self> {
        let (raw_root, source) = match explicit_scope {
            Some(path) => {
                let path = if path.is_absolute() {
                    path.to_path_buf()
                } else {
                    cwd.join(path)
                };
                (path, RootSource::ExplicitScope)
            }
            None => match find_git_root(cwd) {
                Some(root) => (root, RootSource::GitRepository),
                None => (cwd.to_path_buf(), RootSource::WorkingDirectory),
            },
        };

        // Canonicalize before any access-control decision is made.
        let root = std::fs::canonicalize(&raw_root)
            .with_context(|| format!("project root `{}` is not accessible", raw_root.display()))?;

        if !root.is_dir() {
            anyhow::bail!("project root `{}` is not a directory", root.display());
        }

        let is_git_repository = is_git_root(&root);

        let mut overridden_refusal = None;
        if let Some(unsafe_root) = check_root_safety(&root, is_git_repository) {
            if allow_unsafe {
                tracing::warn!(
                    root = %root.display(),
                    reason = %unsafe_root,
                    "using an unsafe project root because --allow-unsafe-scope was given"
                );
                // `discover` runs before logging is initialized and logging is
                // off by default, so the `tracing::warn!` above is frequently
                // invisible. Carry the refusal forward so the caller can put a
                // warning somewhere a user actually sees it (see
                // `Project::overridden_refusal`).
                overridden_refusal = Some(unsafe_root);
            } else {
                anyhow::bail!("{unsafe_root}\n\n{}", unsafe_root.remedy());
            }
        }

        Ok(Self {
            id: ProjectId::from_canonical_root(&root),
            scope: ProjectScope::new(root),
            source,
            is_git_repository,
            overridden_refusal,
        })
    }

    pub fn id(&self) -> &ProjectId {
        &self.id
    }

    /// The canonical project root, in the exact form `fs::canonicalize`
    /// produced.
    ///
    /// This is the identity and containment form: it is what [`ProjectId`]
    /// hashes and what [`ProjectScope`] compares against, and it must stay
    /// that way. On Windows this is the verbatim `\\?\...` path.
    /// `CreateProcessW`'s `lpCurrentDirectory` does not reliably accept that
    /// form and `cmd.exe` refuses it outright, so it is the wrong value to
    /// hand to another process or show to a person — use
    /// [`Project::display_root`] for that instead.
    pub fn root(&self) -> &Path {
        self.scope.root()
    }

    /// The project root in the form suitable for a child process or a
    /// person: a process's working directory, or text printed to the user.
    ///
    /// This differs from [`Project::root`] only on Windows, where it strips
    /// the verbatim `\\?\` prefix. Never substitute this into [`ProjectId`]
    /// derivation or a [`ProjectScope`] containment check — those must keep
    /// using [`Project::root`].
    pub fn display_root(&self) -> PathBuf {
        platform::strip_verbatim_prefix(self.root())
    }

    pub fn scope(&self) -> &ProjectScope {
        &self.scope
    }

    pub fn source(&self) -> RootSource {
        self.source
    }

    pub fn is_git_repository(&self) -> bool {
        self.is_git_repository
    }

    /// The safety refusal that `--allow-unsafe-scope` overrode to select this
    /// root, if any. `None` means the root needed no override.
    pub fn overridden_refusal(&self) -> Option<&UnsafeRoot> {
        self.overridden_refusal.as_ref()
    }

    /// Short display name for the project.
    pub fn name(&self) -> String {
        self.root()
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| self.root().display().to_string())
    }
}

/// Return the refusal reason for a root, if any.
fn check_root_safety(root: &Path, is_git_repository: bool) -> Option<UnsafeRoot> {
    root_safety(root, is_git_repository, home_dir().as_deref())
}

/// The actual safety logic, parameterized on the home directory so it is
/// testable with a fabricated one instead of the process's real `$HOME`.
fn root_safety(root: &Path, is_git_repository: bool, home: Option<&Path>) -> Option<UnsafeRoot> {
    if root.parent().is_none() {
        return Some(UnsafeRoot::FilesystemRoot(root.to_path_buf()));
    }

    if let Some(home) = home {
        // Compare by device+inode, not by string: on a case-insensitive
        // macOS volume, `--scope /users/me` names the same directory as
        // `/Users/me` without comparing equal as strings, and whether
        // Darwin's `realpath` case-corrects is not something a safety guard
        // should bet on.
        if platform::same_file(home, root) {
            return Some(UnsafeRoot::HomeDirectory(root.to_path_buf()));
        }
        // The parent of every user's home directory (`/home`, `/Users`,
        // `C:\Users`) is not itself a home directory and typically holds no
        // immediate Git repositories, so neither check above catches it —
        // but scoping Glasshouse there is exactly as bad as scoping it to
        // one user's home directory.
        if let Some(home_parent) = home.parent() {
            if platform::same_file(home_parent, root) {
                return Some(UnsafeRoot::HomeDirectoryParent(root.to_path_buf()));
            }
        }
    }

    // A directory that is itself a repository is a project even when it
    // contains nested repositories such as submodules or vendored checkouts.
    if !is_git_repository {
        let repositories = child_git_repositories(root, 3);
        if repositories.len() > 1 {
            return Some(UnsafeRoot::MultiProjectContainer {
                root: root.to_path_buf(),
                repositories,
            });
        }
    }

    None
}

/// The user's home directory, canonicalized when possible.
fn home_dir() -> Option<PathBuf> {
    let home = directories::UserDirs::new().map(|d| d.home_dir().to_path_buf())?;
    Some(std::fs::canonicalize(&home).unwrap_or(home))
}

/// Depth to descend while looking for nested Git repositories. The common
/// "container of containers" layout (`~/code/work/repoA`,
/// `~/code/personal/repoB`) has zero repositories at depth 1, so depth 1
/// alone accepts it as a single project. Two levels catches that layout while
/// staying cheap.
const REPOSITORY_SCAN_DEPTH: u32 = 2;

/// Hard cap on directories visited while scanning for nested repositories,
/// independent of how many are found. This bounds startup time even for a
/// huge, repository-free tree, on top of the `limit` on repositories found.
const REPOSITORY_SCAN_MAX_VISITS: usize = 4000;

/// Names (or `parent/name` for depth-2 matches) of Git repositories found by
/// scanning up to [`REPOSITORY_SCAN_DEPTH`] levels below `root`, stopping as
/// soon as `limit` repositories are found.
fn child_git_repositories(root: &Path, limit: usize) -> Vec<String> {
    let mut found = Vec::new();
    let mut visited = 0usize;
    scan_for_repositories(
        root,
        "",
        REPOSITORY_SCAN_DEPTH,
        limit,
        &mut visited,
        &mut found,
    );
    found.sort();
    found
}

/// Recursive worker for [`child_git_repositories`]. `prefix` is the relative
/// path already descended, used to label a depth-2 match as `parent/name`.
fn scan_for_repositories(
    dir: &Path,
    prefix: &str,
    remaining_depth: u32,
    limit: usize,
    visited: &mut usize,
    found: &mut Vec<String>,
) {
    if found.len() >= limit || *visited >= REPOSITORY_SCAN_MAX_VISITS {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        if found.len() >= limit || *visited >= REPOSITORY_SCAN_MAX_VISITS {
            return;
        }
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        *visited += 1;

        let name = entry.file_name().to_string_lossy().into_owned();
        if is_scan_noise(&name) {
            continue;
        }
        let label = if prefix.is_empty() {
            name
        } else {
            format!("{prefix}/{name}")
        };

        if is_git_root(&path) {
            found.push(label);
            // A repository's own contents (submodules, vendored checkouts)
            // are not another container level to descend into.
            continue;
        }
        if remaining_depth > 1 {
            scan_for_repositories(&path, &label, remaining_depth - 1, limit, visited, found);
        }
    }
}

/// True for hidden directories (including `.git` itself) and common
/// non-project noise that is never worth descending into while looking for
/// nested repositories.
fn is_scan_noise(name: &str) -> bool {
    name.starts_with('.') || matches!(name, "node_modules" | "target")
}

/// True when `dir` holds a `.git` entry. A file is accepted because Git
/// worktrees and submodules use a `.git` file pointing at the real directory.
fn is_git_root(dir: &Path) -> bool {
    dir.join(".git").symlink_metadata().is_ok()
}

/// Walk up from `start` to the nearest containing Git repository.
fn find_git_root(start: &Path) -> Option<PathBuf> {
    let mut current = Some(start);
    while let Some(dir) = current {
        if is_git_root(dir) {
            return Some(dir.to_path_buf());
        }
        current = dir.parent();
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn init_git_repo(dir: &Path) {
        std::fs::create_dir_all(dir.join(".git")).unwrap();
    }

    #[test]
    fn discovers_the_containing_git_repository() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("repo");
        std::fs::create_dir_all(repo.join("src/deep")).unwrap();
        init_git_repo(&repo);

        let project = Project::discover(&repo.join("src/deep"), None, false).unwrap();
        assert_eq!(project.source(), RootSource::GitRepository);
        assert!(platform::same_path(
            project.root(),
            &std::fs::canonicalize(&repo).unwrap()
        ));
        assert!(project.is_git_repository());
    }

    #[test]
    fn falls_back_to_the_working_directory_without_git() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("plain");
        std::fs::create_dir_all(&dir).unwrap();

        let project = Project::discover(&dir, None, false).unwrap();
        assert_eq!(project.source(), RootSource::WorkingDirectory);
        assert!(!project.is_git_repository());
    }

    #[test]
    fn explicit_scope_overrides_git_discovery() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("repo");
        std::fs::create_dir_all(repo.join("sub")).unwrap();
        init_git_repo(&repo);

        let project = Project::discover(&repo, Some(Path::new("sub")), false).unwrap();
        assert_eq!(project.source(), RootSource::ExplicitScope);
        assert!(project.root().ends_with("sub"));
    }

    #[test]
    fn refuses_a_multi_project_container() {
        let tmp = tempfile::tempdir().unwrap();
        let container = tmp.path().join("code");
        for name in ["alpha", "beta"] {
            let repo = container.join(name);
            std::fs::create_dir_all(&repo).unwrap();
            init_git_repo(&repo);
        }

        let err = Project::discover(&container, None, false).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("container for multiple projects"), "{msg}");
        assert!(msg.contains("alpha") && msg.contains("beta"), "{msg}");

        // The override is the only way past it.
        assert!(Project::discover(&container, None, true).is_ok());
    }

    #[test]
    fn allows_a_repository_that_contains_nested_repositories() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("super");
        std::fs::create_dir_all(&repo).unwrap();
        init_git_repo(&repo);
        for name in ["vendor-a", "vendor-b"] {
            let nested = repo.join(name);
            std::fs::create_dir_all(&nested).unwrap();
            init_git_repo(&nested);
        }

        assert!(Project::discover(&repo, None, false).is_ok());
    }

    #[test]
    fn refuses_a_container_with_nested_repositories_two_levels_deep() {
        // The common "container of containers" layout: zero repositories as
        // immediate children, so a scan that only looked one level deep would
        // accept this as a single project.
        let tmp = tempfile::tempdir().unwrap();
        let container = tmp.path().join("code");
        for (group, name) in [("work", "repo-a"), ("personal", "repo-b")] {
            let repo = container.join(group).join(name);
            std::fs::create_dir_all(&repo).unwrap();
            init_git_repo(&repo);
        }

        let err = Project::discover(&container, None, false).unwrap_err();
        assert!(
            err.to_string().contains("container for multiple projects"),
            "{err}"
        );
    }

    #[test]
    fn repository_scan_skips_hidden_and_noise_directories() {
        let tmp = tempfile::tempdir().unwrap();
        let container = tmp.path().join("code");
        // Two real nested repos, plus noise that must not be descended into.
        for (group, name) in [("work", "repo-a"), ("personal", "repo-b")] {
            let repo = container.join(group).join(name);
            std::fs::create_dir_all(&repo).unwrap();
            init_git_repo(&repo);
        }
        for noisy in [".hidden/should-not-count", "node_modules/should-not-count"] {
            let repo = container.join(noisy);
            std::fs::create_dir_all(&repo).unwrap();
            init_git_repo(&repo);
        }

        let repos = child_git_repositories(&container, 10);
        assert_eq!(repos, vec!["personal/repo-b", "work/repo-a"]);
    }

    #[test]
    fn refuses_the_parent_of_the_home_directory() {
        // `/home`, `/Users`, `C:\Users`: not the home directory itself, has a
        // parent, and typically has no immediate repositories — but scoping
        // Glasshouse there mixes every user's projects into one scope, just
        // like scoping it to one user's home directory would.
        let tmp = tempfile::tempdir().unwrap();
        let users_dir = tmp.path().join("Users");
        let home = users_dir.join("me");
        std::fs::create_dir_all(&home).unwrap();

        let refusal = root_safety(&users_dir, false, Some(&home));
        assert!(
            matches!(refusal, Some(UnsafeRoot::HomeDirectoryParent(_))),
            "{refusal:?}"
        );
    }

    #[test]
    fn refuses_the_filesystem_root() {
        let root = Path::new(std::path::MAIN_SEPARATOR_STR);
        let err = Project::discover(root, Some(root), false).unwrap_err();
        assert!(err.to_string().contains("filesystem root"), "{err}");
    }

    #[test]
    fn refuses_the_home_directory() {
        let Some(home) = home_dir() else {
            return;
        };
        let err = Project::discover(&home, Some(&home), false).unwrap_err();
        assert!(err.to_string().contains("home directory"), "{err}");
    }

    #[test]
    fn project_ids_are_stable_and_distinct() {
        let tmp = tempfile::tempdir().unwrap();
        let a = tmp.path().join("alpha");
        let b = tmp.path().join("beta");
        std::fs::create_dir_all(&a).unwrap();
        std::fs::create_dir_all(&b).unwrap();
        let a = std::fs::canonicalize(&a).unwrap();
        let b = std::fs::canonicalize(&b).unwrap();

        let id_a = ProjectId::from_canonical_root(&a);
        let id_b = ProjectId::from_canonical_root(&b);

        assert_eq!(id_a, ProjectId::from_canonical_root(&a));
        assert_ne!(id_a, id_b);
        assert!(id_a.as_str().starts_with("alpha-"));
        assert!(
            id_a.as_str()
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-')
        );

        // 128-bit digest (32 hex chars), not the old 64-bit one.
        let (_, digest) = id_a.as_str().rsplit_once('-').unwrap();
        assert_eq!(
            digest.len(),
            32,
            "expected a 128-bit digest, got `{digest}`"
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_symlinked_root_canonicalizes_to_the_real_project() {
        let tmp = tempfile::tempdir().unwrap();
        let real = tmp.path().join("real-project");
        std::fs::create_dir_all(&real).unwrap();
        init_git_repo(&real);
        let link = tmp.path().join("link-to-project");
        std::os::unix::fs::symlink(&real, &link).unwrap();

        let via_link = Project::discover(&link, None, false).unwrap();
        let direct = Project::discover(&real, None, false).unwrap();
        assert_eq!(via_link.id(), direct.id());
        assert_eq!(via_link.root(), direct.root());
    }

    #[test]
    fn slugify_produces_readable_safe_names() {
        assert_eq!(slugify("My Project!"), "my-project");
        assert_eq!(slugify("..."), "");
        assert_eq!(slugify("glasshouse"), "glasshouse");
    }
}
