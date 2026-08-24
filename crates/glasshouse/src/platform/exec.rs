//! Cross-platform harness executable resolution.
//!
//! Finding "the `claude` executable" is not just a `PATH` lookup:
//!
//! - On Windows, a great many CLIs installed through npm are `.cmd` (or
//!   `.bat`) shim scripts. `CreateProcess` cannot launch those directly —
//!   Windows itself only knows how to exec `.exe`/`.com` binaries — so the
//!   caller needs to know it must go through the command interpreter
//!   (`cmd.exe /C <script> <args...>`) instead of spawning the script path.
//! - Under WSL, `PATH` usually contains Windows interop entries
//!   (`/mnt/c/...`) alongside the real Linux ones, because Windows appends
//!   its own `PATH` to the WSL one by default. An executable found there
//!   would run in the *Windows* process namespace: a different filesystem, a
//!   different working-directory model, a different process tree. Spawning
//!   it as if it were a normal Linux child process would silently break
//!   Glasshouse's project isolation, because the Linux project-root path set
//!   as the working directory means nothing to a Windows process. Resolution
//!   under WSL must filter those out of the *usable* result, but still
//!   report them, so `glasshouse doctor` can tell the user what happened
//!   instead of just saying "not found".
//!
//! This module keeps the PATH search itself on the well-tested [`which`]
//! crate (it already implements Windows `PATHEXT` correctly), and layers the
//! WSL-interop filter and the `.cmd`/`.bat` launch-kind classification on
//! top.

use std::ffi::{OsStr, OsString};
use std::path::{Component, Path, PathBuf};

use crate::platform::HostPlatform;

/// How a resolved executable must actually be invoked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LaunchKind {
    /// Spawned directly: the resolved path is itself the program to run.
    Direct,
    /// A Windows `.cmd`/`.bat` script that must run through the command
    /// interpreter rather than being exec'd on its own.
    WindowsScript,
}

/// A harness executable that has been located and classified.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedExecutable {
    path: PathBuf,
    kind: LaunchKind,
}

impl ResolvedExecutable {
    /// The absolute path to the resolved file (canonicalized where possible).
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// How this executable must be launched.
    pub fn kind(&self) -> LaunchKind {
        self.kind
    }

    /// Translate a logical `(program, args)` invocation into the concrete
    /// `(program, args)` the OS can actually spawn.
    ///
    /// For [`LaunchKind::Direct`] this is the resolved path and the given
    /// arguments, unchanged. For [`LaunchKind::WindowsScript`] the program
    /// becomes the command interpreter (`%COMSPEC%`, falling back to
    /// `cmd.exe`) and the arguments become `["/C", <script path>, ...args]`,
    /// which is how `.cmd`/`.bat` files are actually launched.
    ///
    /// This branch is deliberately not `#[cfg(windows)]`-gated: the classic
    /// deployment story for one of these launchers is npm-installed CLI
    /// shims, and this logic needs to be exercised by tests on any host, not
    /// just when actually compiled for Windows. What *is* platform-specific
    /// is which files ever get classified as [`LaunchKind::WindowsScript`]
    /// in the first place — see [`classify_launch_kind`].
    pub fn spawn_command<I, S>(&self, args: I) -> (PathBuf, Vec<OsString>)
    where
        I: IntoIterator<Item = S>,
        S: Into<OsString>,
    {
        match self.kind {
            LaunchKind::Direct => (
                self.path.clone(),
                args.into_iter().map(Into::into).collect(),
            ),
            LaunchKind::WindowsScript => {
                let interpreter = std::env::var_os("COMSPEC")
                    .map(PathBuf::from)
                    .unwrap_or_else(|| PathBuf::from("cmd.exe"));
                let mut full_args = Vec::with_capacity(2);
                full_args.push(OsString::from("/C"));
                full_args.push(self.path.clone().into_os_string());
                full_args.extend(args.into_iter().map(Into::into));
                (interpreter, full_args)
            }
        }
    }
}

/// Why resolving a harness executable failed.
#[derive(Debug, thiserror::Error)]
pub enum ResolveError {
    /// Nothing usable was found under this name.
    #[error("no usable `{name}` executable was found")]
    NotFound { name: String },

    /// Found only in the Windows interop area of a WSL `PATH`.
    ///
    /// The message explicitly tells the user why the hit was rejected and
    /// what to do about it, rather than just reporting "not found" and
    /// leaving them to guess why an executable they can clearly see on their
    /// `PATH` was not picked up.
    #[error(
        "`{name}` was found only in the Windows side of PATH ({}), which Glasshouse cannot launch \
         from WSL: it would run in the Windows process namespace, where this project's Linux path \
         is meaningless and project isolation could not be enforced. Install the Linux build of \
         `{name}` inside this WSL distro instead.",
        .found_at.iter().map(|p| p.display().to_string()).collect::<Vec<_>>().join(", ")
    )]
    WindowsInteropOnly {
        name: String,
        found_at: Vec<PathBuf>,
    },

    /// The path exists but the current user cannot execute it.
    #[error("`{}` exists but is not executable", .path.display())]
    NotExecutable { path: PathBuf },
}

/// Resolve a bare command name against the current `PATH`, using the current
/// [`HostPlatform`].
pub fn resolve(name: &str) -> Result<ResolvedExecutable, ResolveError> {
    let platform = HostPlatform::detect();
    let path_list = std::env::var_os("PATH").unwrap_or_default();
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    resolve_with(platform, name, &path_list, &cwd)
}

/// Verify a user-configured explicit executable path, and classify its
/// [`LaunchKind`].
///
/// Unlike [`resolve`], the WSL Windows-interop filter is *not* applied here:
/// if the user explicitly pointed Glasshouse at a path, that is a deliberate
/// choice, not an ambiguous `PATH` hit to be second-guessed. A Windows
/// interop path is still logged as a warning under WSL, though, since it is
/// very likely a mistake with the same project-isolation consequences
/// described in the module documentation.
pub fn resolve_explicit(path: &Path) -> Result<ResolvedExecutable, ResolveError> {
    let platform = HostPlatform::detect();
    resolve_explicit_with(platform, path)
}

pub(crate) fn resolve_explicit_with(
    platform: HostPlatform,
    path: &Path,
) -> Result<ResolvedExecutable, ResolveError> {
    if !path.is_file() {
        return Err(ResolveError::NotFound {
            name: path.display().to_string(),
        });
    }
    if !is_executable(path) {
        return Err(ResolveError::NotExecutable {
            path: path.to_path_buf(),
        });
    }

    if platform == HostPlatform::Wsl && is_windows_interop_path(platform, path) {
        tracing::warn!(
            path = %path.display(),
            "explicit executable path is in the Windows interop area of WSL; it runs in the \
             Windows process namespace, not the Linux one this project is scoped to"
        );
    }

    let resolved = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let kind = classify_launch_kind(platform, &resolved);
    Ok(ResolvedExecutable {
        path: resolved,
        kind,
    })
}

/// Resolve `name` against `path_list` (an OS `PATH`-style string) for
/// `platform`, using `cwd` to resolve any path fragments in `name`.
///
/// Kept separate from [`resolve`] — and taking the platform and `PATH` as
/// plain parameters rather than reading the real environment — so Windows
/// and WSL behavior can be exercised in tests on any host.
pub(crate) fn resolve_with(
    platform: HostPlatform,
    name: &str,
    path_list: &OsStr,
    cwd: &Path,
) -> Result<ResolvedExecutable, ResolveError> {
    resolve_with_interop_predicate(platform, name, path_list, cwd, is_windows_interop_path)
}

/// Core of [`resolve_with`], with the Windows-interop predicate injected so
/// it can be tested independently of the real `/mnt/<drive>` heuristic (see
/// the `resolver_reports_windows_interop_only_hits` test).
fn resolve_with_interop_predicate(
    platform: HostPlatform,
    name: &str,
    path_list: &OsStr,
    cwd: &Path,
    is_interop: impl Fn(HostPlatform, &Path) -> bool,
) -> Result<ResolvedExecutable, ResolveError> {
    let candidates: Vec<PathBuf> = which::which_in_all(name, Some(path_list), cwd)
        .map(Iterator::collect)
        .unwrap_or_default();

    if candidates.is_empty() {
        return Err(ResolveError::NotFound {
            name: name.to_string(),
        });
    }

    let mut usable = Vec::new();
    let mut interop_only = Vec::new();
    for candidate in candidates {
        if platform == HostPlatform::Wsl && is_interop(platform, &candidate) {
            interop_only.push(candidate);
        } else {
            usable.push(candidate);
        }
    }

    let chosen = match usable.into_iter().next() {
        Some(path) => path,
        None if !interop_only.is_empty() => {
            return Err(ResolveError::WindowsInteropOnly {
                name: name.to_string(),
                found_at: interop_only,
            });
        }
        None => {
            return Err(ResolveError::NotFound {
                name: name.to_string(),
            });
        }
    };

    let resolved = std::fs::canonicalize(&chosen).unwrap_or(chosen);
    let kind = classify_launch_kind(platform, &resolved);
    Ok(ResolvedExecutable {
        path: resolved,
        kind,
    })
}

/// Classify how a resolved path must be spawned.
///
/// A `.cmd`/`.bat` extension is only special on Windows — `CreateProcess`
/// cannot exec those directly, but on every other platform a same-named file
/// is just a regular file with an unusual extension and gets spawned
/// normally. Gating on `platform` (a value, not a `#[cfg(windows)]` compile
/// target) is what keeps this testable from a Linux host: a test can ask
/// "how would this be classified *if* the platform were Windows" without
/// needing to actually compile for Windows.
fn classify_launch_kind(platform: HostPlatform, path: &Path) -> LaunchKind {
    if !platform.is_windows() {
        return LaunchKind::Direct;
    }
    match path.extension().and_then(OsStr::to_str) {
        Some(ext) if ext.eq_ignore_ascii_case("cmd") || ext.eq_ignore_ascii_case("bat") => {
            LaunchKind::WindowsScript
        }
        _ => LaunchKind::Direct,
    }
}

/// Heuristic for whether `path` sits in the Windows interoperability area of
/// a WSL `PATH`.
///
/// This is honestly a heuristic, not a precise classification:
///
/// - The primary signal is the well-known WSL drive-mount convention,
///   `/mnt/<single-letter>/...` (e.g. `/mnt/c/Users/...`), which is where
///   WSL exposes the Windows filesystem and where the Windows-appended
///   `PATH` entries point. This is checked case-insensitively on the drive
///   letter, matching how Windows drive letters work.
/// - As a secondary signal, any `.exe` extension found while resolving under
///   WSL is also treated as Windows-side, even outside `/mnt`, since WSL
///   binaries essentially never carry that extension. This is deliberately
///   broad: it will also flag a genuinely Linux-built cross-compiled `.exe`
///   artifact (e.g. a MinGW build sitting in a build output directory) as
///   Windows-interop even though it could, in principle, run fine as a Linux
///   PE-loader oddity or simply be irrelevant noise on `PATH`. Given the
///   product requirement, false positives here (treating something as
///   Windows-side when it might be usable) are the safe failure mode;
///   false negatives (silently launching a real Windows process into a
///   Linux project's working directory) are the one this function exists to
///   prevent.
///
/// Returns `false` outright for every platform other than
/// [`HostPlatform::Wsl`]: `/mnt/c` and `.exe` have no special meaning on
/// real Linux, macOS, or native Windows itself.
pub(crate) fn is_windows_interop_path(platform: HostPlatform, path: &Path) -> bool {
    if platform != HostPlatform::Wsl {
        return false;
    }
    is_under_windows_drive_mount(path) || has_windows_exe_extension(path)
}

fn is_under_windows_drive_mount(path: &Path) -> bool {
    let mut components = path.components();
    matches!(components.next(), Some(Component::RootDir))
        && matches!(components.next(), Some(Component::Normal(s)) if s == "mnt")
        && matches!(components.next(), Some(Component::Normal(s)) if is_single_drive_letter(s))
}

fn is_single_drive_letter(s: &OsStr) -> bool {
    match s.to_str() {
        Some(s) => {
            let mut chars = s.chars();
            matches!(chars.next(), Some(c) if c.is_ascii_alphabetic()) && chars.next().is_none()
        }
        None => false,
    }
}

fn has_windows_exe_extension(path: &Path) -> bool {
    path.extension()
        .and_then(OsStr::to_str)
        .is_some_and(|ext| ext.eq_ignore_ascii_case("exe"))
}

/// Whether the current user can execute `path`.
///
/// On Unix this is the execute permission bit. On Windows, executability is
/// governed by extension and ACLs rather than a permission bit, so existence
/// as a regular file is what actually matters for spawning it.
fn is_executable(path: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(path)
            .map(|meta| meta.permissions().mode() & 0o111 != 0)
            .unwrap_or(false)
    }
    #[cfg(not(unix))]
    {
        path.is_file()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[cfg(unix)]
    fn make_executable(path: &Path) {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(path).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(path, perms).unwrap();
    }

    fn write_file(path: &Path) {
        let mut f = std::fs::File::create(path).unwrap();
        writeln!(f, "#!/bin/sh\necho hi").unwrap();
    }

    // --- spawn_command ---------------------------------------------------

    #[test]
    fn spawn_command_direct_passes_through_unchanged() {
        let resolved = ResolvedExecutable {
            path: PathBuf::from("/usr/bin/claude"),
            kind: LaunchKind::Direct,
        };
        let (program, args) = resolved.spawn_command(["--resume", "abc"]);
        assert_eq!(program, PathBuf::from("/usr/bin/claude"));
        assert_eq!(
            args,
            vec![OsString::from("--resume"), OsString::from("abc")]
        );
    }

    #[test]
    fn spawn_command_windows_script_wraps_through_the_interpreter() {
        let script = PathBuf::from(r"C:\Users\me\AppData\Roaming\npm\claude.cmd");
        let resolved = ResolvedExecutable {
            path: script.clone(),
            kind: LaunchKind::WindowsScript,
        };
        let (program, args) = resolved.spawn_command(["--resume", "abc"]);

        let expected_interpreter = std::env::var_os("COMSPEC")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("cmd.exe"));
        assert_eq!(program, expected_interpreter);
        assert_eq!(
            args,
            vec![
                OsString::from("/C"),
                script.into_os_string(),
                OsString::from("--resume"),
                OsString::from("abc"),
            ]
        );
    }

    #[test]
    fn spawn_command_windows_script_with_no_extra_args() {
        let resolved = ResolvedExecutable {
            path: PathBuf::from(r"C:\tools\codex.bat"),
            kind: LaunchKind::WindowsScript,
        };
        let (_program, args) = resolved.spawn_command(Vec::<OsString>::new());
        assert_eq!(args[0], OsString::from("/C"));
        assert_eq!(args[1], OsString::from(r"C:\tools\codex.bat"));
        assert_eq!(args.len(), 2);
    }

    // --- classify_launch_kind --------------------------------------------

    #[test]
    fn cmd_and_bat_are_windows_scripts_only_on_windows() {
        assert_eq!(
            classify_launch_kind(HostPlatform::Windows, Path::new(r"C:\bin\claude.cmd")),
            LaunchKind::WindowsScript
        );
        assert_eq!(
            classify_launch_kind(HostPlatform::Windows, Path::new(r"C:\bin\claude.BAT")),
            LaunchKind::WindowsScript
        );
        assert_eq!(
            classify_launch_kind(HostPlatform::Windows, Path::new(r"C:\bin\claude.exe")),
            LaunchKind::Direct
        );
        // Same extension, non-Windows platform: never special.
        assert_eq!(
            classify_launch_kind(HostPlatform::Linux, Path::new("/usr/bin/claude.cmd")),
            LaunchKind::Direct
        );
        assert_eq!(
            classify_launch_kind(HostPlatform::Wsl, Path::new("/usr/bin/claude.bat")),
            LaunchKind::Direct
        );
    }

    // --- is_windows_interop_path ------------------------------------------

    #[test]
    fn interop_path_detected_only_under_wsl() {
        assert!(is_windows_interop_path(
            HostPlatform::Wsl,
            Path::new("/mnt/c/Users/x/claude.exe")
        ));
        assert!(!is_windows_interop_path(
            HostPlatform::Linux,
            Path::new("/mnt/c/Users/x/claude.exe")
        ));
        assert!(!is_windows_interop_path(
            HostPlatform::MacOs,
            Path::new("/mnt/c/Users/x/claude.exe")
        ));
        assert!(!is_windows_interop_path(
            HostPlatform::Windows,
            Path::new("/mnt/c/Users/x/claude.exe")
        ));
    }

    #[test]
    fn ordinary_unix_bin_is_never_interop() {
        assert!(!is_windows_interop_path(
            HostPlatform::Wsl,
            Path::new("/usr/local/bin/claude")
        ));
    }

    #[test]
    fn drive_mount_requires_a_single_letter_component() {
        // A two-letter directory under /mnt is not a drive mount.
        assert!(!is_windows_interop_path(
            HostPlatform::Wsl,
            Path::new("/mnt/wsl/some-share/tool")
        ));
        // Case-insensitive drive letter.
        assert!(is_windows_interop_path(
            HostPlatform::Wsl,
            Path::new("/mnt/C/Windows/System32/tool.exe")
        ));
    }

    #[test]
    fn exe_extension_is_a_secondary_signal_under_wsl() {
        assert!(is_windows_interop_path(
            HostPlatform::Wsl,
            Path::new("/home/user/bin/tool.exe")
        ));
        assert!(!is_windows_interop_path(
            HostPlatform::Wsl,
            Path::new("/home/user/bin/tool")
        ));
    }

    // --- resolve_with / resolve_with_interop_predicate ---------------------

    #[test]
    fn resolves_a_real_executable_on_the_injected_path() {
        // "sh" is expected to exist on any Unix CI runner.
        let dir = tempfile::tempdir().unwrap();
        let path_list = std::env::var_os("PATH").unwrap_or_default();
        let resolved = resolve_with(HostPlatform::Linux, "sh", &path_list, dir.path())
            .expect("sh should resolve on PATH");
        assert!(resolved.path().is_absolute());
        assert_eq!(resolved.kind(), LaunchKind::Direct);
    }

    #[test]
    fn missing_binary_is_reported_as_not_found() {
        let dir = tempfile::tempdir().unwrap();
        let path_list = OsString::from(dir.path()); // empty search path
        let err = resolve_with(
            HostPlatform::Linux,
            "definitely-not-a-real-glasshouse-harness",
            &path_list,
            dir.path(),
        )
        .unwrap_err();
        assert!(
            matches!(err, ResolveError::NotFound { name } if name.contains("definitely-not-a-real-glasshouse-harness"))
        );
    }

    #[test]
    fn resolver_reports_windows_interop_only_hits() {
        // Build a fake PATH entry (an ordinary tempdir — not a real /mnt/c,
        // since is_windows_interop_path is tested against real path shapes
        // above) holding one candidate, and inject a predicate that marks
        // that exact candidate as Windows-interop. This exercises the
        // resolver's aggregation/reporting behavior independently of the
        // real drive-mount heuristic.
        let dir = tempfile::tempdir().unwrap();
        let candidate = dir.path().join("claude");
        write_file(&candidate);
        #[cfg(unix)]
        make_executable(&candidate);

        let path_list = OsString::from(dir.path());
        let marked = candidate.clone();

        let err = resolve_with_interop_predicate(
            HostPlatform::Wsl,
            "claude",
            &path_list,
            dir.path(),
            move |_platform, p| p == marked,
        )
        .unwrap_err();

        match err {
            ResolveError::WindowsInteropOnly { name, found_at } => {
                assert_eq!(name, "claude");
                assert_eq!(found_at.len(), 1);
                assert!(found_at[0].ends_with("claude"));
            }
            other => panic!("expected WindowsInteropOnly, got {other:?}"),
        }
    }

    #[test]
    fn resolver_prefers_usable_hits_over_interop_hits() {
        let dir = tempfile::tempdir().unwrap();
        let candidate = dir.path().join("claude");
        write_file(&candidate);
        #[cfg(unix)]
        make_executable(&candidate);

        let path_list = OsString::from(dir.path());

        // The predicate marks nothing as interop, so the real candidate
        // must be used even though we are "on WSL".
        let resolved = resolve_with_interop_predicate(
            HostPlatform::Wsl,
            "claude",
            &path_list,
            dir.path(),
            |_platform, _path| false,
        )
        .expect("should resolve the usable candidate");
        assert!(resolved.path().ends_with("claude"));
    }

    // --- resolve_explicit_with ---------------------------------------------

    #[test]
    fn resolve_explicit_accepts_an_executable_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("harness");
        write_file(&path);
        #[cfg(unix)]
        make_executable(&path);

        let resolved = resolve_explicit_with(HostPlatform::Linux, &path).unwrap();
        assert_eq!(resolved.kind(), LaunchKind::Direct);
        assert!(resolved.path().is_absolute());
    }

    #[test]
    fn resolve_explicit_rejects_a_missing_path() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("nope");
        let err = resolve_explicit_with(HostPlatform::Linux, &missing).unwrap_err();
        assert!(matches!(err, ResolveError::NotFound { .. }));
    }

    #[cfg(unix)]
    #[test]
    fn resolve_explicit_rejects_a_non_executable_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("script.sh");
        write_file(&path);
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(&path).unwrap().permissions();
        perms.set_mode(0o644);
        std::fs::set_permissions(&path, perms).unwrap();

        let err = resolve_explicit_with(HostPlatform::Linux, &path).unwrap_err();
        assert!(matches!(err, ResolveError::NotExecutable { .. }));
    }

    #[test]
    fn resolve_explicit_does_not_filter_wsl_interop_paths() {
        // An explicit path is a deliberate user choice: even though it looks
        // like a Windows interop path, resolve_explicit must not reject it
        // the way resolve_with does — it should only warn (untestable here
        // without a tracing subscriber, so we just assert it still resolves).
        let dir = tempfile::tempdir().unwrap();
        let mnt = dir.path().join("mnt").join("c");
        std::fs::create_dir_all(&mnt).unwrap();
        let path = mnt.join("tool.exe");
        write_file(&path);
        #[cfg(unix)]
        make_executable(&path);

        let resolved = resolve_explicit_with(HostPlatform::Wsl, &path).unwrap();
        assert!(resolved.path().ends_with("tool.exe"));
    }
}
