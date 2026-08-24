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
    /// arguments, unchanged: no shell is involved in spawning a `Direct`
    /// executable, so there is nothing to validate. For
    /// [`LaunchKind::WindowsScript`] the program becomes the command
    /// interpreter (`%COMSPEC%`, falling back to `cmd.exe`) and the
    /// arguments become `["/D", "/C", <script path>, ...args]`, which is how
    /// `.cmd`/`.bat` files are actually launched.
    ///
    /// # Why this can fail
    ///
    /// `cmd.exe /C` is a shell invocation, and portable-pty's
    /// [`CommandBuilder`](https://docs.rs/portable-pty/latest/portable_pty/struct.CommandBuilder.html)
    /// only applies the standard CRT `ArgvQuote` quoting rules (space, tab,
    /// newline, VT, `"`) when it builds the process command line —
    /// `cmd.exe` does not parse with those rules. An argument like
    /// `--session=a&calc.exe` contains no CRT-quote trigger, reaches `cmd`
    /// verbatim, and `&` starts a second command (this is the BatBadBut /
    /// CVE-2024-24576 shape). `%`, `^`, `|`, `<`, `>`, and backtick are
    /// equally unhandled.
    ///
    /// Escaping correctly through `cmd.exe` is notoriously unreliable, and
    /// Glasshouse does not need to try: harness arguments are flags, paths,
    /// model names, and session ids — prompts are typed into the PTY, never
    /// passed as argv. So instead of a clever escaper, every argument (and
    /// the script path itself) is validated up front and rejected outright
    /// if it contains a cmd.exe metacharacter. Once validated, no
    /// metacharacter can trigger command-chaining, so passing the script
    /// path and arguments as separate argv entries — letting
    /// `CommandBuilder` do its normal CRT quoting — is safe.
    ///
    /// This branch is deliberately not `#[cfg(windows)]`-gated: the classic
    /// deployment story for one of these launchers is npm-installed CLI
    /// shims, and this logic needs to be exercised by tests on any host, not
    /// just when actually compiled for Windows. What *is* platform-specific
    /// is which files ever get classified as [`LaunchKind::WindowsScript`]
    /// in the first place — see [`classify_launch_kind`].
    pub fn spawn_command<I, S>(&self, args: I) -> Result<(PathBuf, Vec<OsString>), LaunchError>
    where
        I: IntoIterator<Item = S>,
        S: Into<OsString>,
    {
        match self.kind {
            // No shell parses a Direct executable's argv -- the OS execs the
            // resolved path with the arguments passed through verbatim -- so
            // there is no command-chaining risk here to validate against.
            LaunchKind::Direct => Ok((
                self.path.clone(),
                args.into_iter().map(Into::into).collect(),
            )),
            LaunchKind::WindowsScript => {
                validate_cmd_argument(self.path.as_os_str(), ArgumentPosition::ScriptPath)?;
                let args: Vec<OsString> = args.into_iter().map(Into::into).collect();
                for (index, arg) in args.iter().enumerate() {
                    validate_cmd_argument(arg, ArgumentPosition::Argument(index))?;
                }

                let interpreter = std::env::var_os("COMSPEC")
                    .map(PathBuf::from)
                    .unwrap_or_else(|| PathBuf::from("cmd.exe"));
                let mut full_args = Vec::with_capacity(3 + args.len());
                full_args.push(OsString::from("/D"));
                // `/D`: disable `HKCU\...\Command Processor\AutoRun` so a
                // user's autorun script does not execute on every harness
                // launch. No `/S`: that flag only changes quoting behavior
                // for the single quoted-command-line form, which is exactly
                // the form we are deliberately not building -- we pass the
                // script path and arguments as separate argv entries
                // instead, so `/S` would just be cargo-culted.
                full_args.push(OsString::from("/C"));
                full_args.push(self.path.clone().into_os_string());
                full_args.extend(args);
                Ok((interpreter, full_args))
            }
        }
    }
}

/// Where in a [`LaunchKind::WindowsScript`] invocation an unsafe
/// `cmd.exe` metacharacter was found.
///
/// Kept separate from the offending value itself: harness arguments can
/// carry secrets (session ids, tokens), so [`LaunchError`]'s message reports
/// *where* the problem is and *which character* triggered it, never the
/// full argument text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArgumentPosition {
    /// The resolved `.cmd`/`.bat` script path itself.
    ScriptPath,
    /// A caller-supplied argument, by its zero-based index in the argument
    /// list passed to [`ResolvedExecutable::spawn_command`].
    Argument(usize),
}

impl std::fmt::Display for ArgumentPosition {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ArgumentPosition::ScriptPath => write!(f, "the script path"),
            ArgumentPosition::Argument(index) => write!(f, "argument {}", index + 1),
        }
    }
}

/// Why a [`LaunchKind::WindowsScript`] command could not be safely
/// assembled.
#[derive(Debug, thiserror::Error)]
pub enum LaunchError {
    /// `cmd.exe /C` is a shell invocation with no reliable escaping story
    /// (see [`ResolvedExecutable::spawn_command`]), so an argument
    /// containing one of its metacharacters is rejected outright rather than
    /// escaped.
    #[error(
        "{position} contains the character {character:?}, which cmd.exe treats specially and \
         cannot be passed through `cmd.exe /C` safely; configure a direct executable path for \
         this harness instead of relying on its `.cmd`/`.bat` shim"
    )]
    UnsafeCmdArgument {
        position: ArgumentPosition,
        character: char,
    },
}

/// Characters that make `value` unsafe to hand to `cmd.exe /C` as a bare
/// argv entry: cmd metacharacters (`& | < > ^ % ! "` and backtick, which
/// PowerShell-flavored shims sometimes pass through) plus CR/LF/NUL, none of
/// which CRT `ArgvQuote` quoting (see [`ResolvedExecutable::spawn_command`])
/// ever escapes.
const CMD_UNSAFE_CHARACTERS: &[char] = &[
    '&', '|', '<', '>', '^', '%', '!', '"', '`', '\r', '\n', '\0',
];

/// Reject `value` if it contains any [`CMD_UNSAFE_CHARACTERS`].
///
/// `to_string_lossy` is used rather than requiring valid UTF-8: on Windows
/// `OsString` is always representable as UTF-16 and lossy conversion never
/// alters an ASCII metacharacter, so this cannot miss a real one.
fn validate_cmd_argument(value: &OsStr, position: ArgumentPosition) -> Result<(), LaunchError> {
    match value
        .to_string_lossy()
        .chars()
        .find(|c| CMD_UNSAFE_CHARACTERS.contains(c))
    {
        Some(character) => Err(LaunchError::UnsafeCmdArgument {
            position,
            character,
        }),
        None => Ok(()),
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
        // `which_in_all` reports the raw PATH hit with no symlink
        // resolution, so an ordinary-looking Linux path that is actually a
        // symlink into `/mnt/<drive>` (e.g. an npm shim symlinked to a
        // Windows-side install) would pass the interop filter unresolved.
        // Canonicalize *before* classifying so the filter runs on the real
        // target, not the symlink's apparent location. `unwrap_or` falls
        // back to the raw candidate when canonicalization fails (e.g. a
        // dangling symlink) rather than dropping it from consideration.
        let candidate = std::fs::canonicalize(&candidate).unwrap_or(candidate);
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

    // Already canonicalized above, before classification -- no need to do
    // it again here.
    let kind = classify_launch_kind(platform, &chosen);
    Ok(ResolvedExecutable { path: chosen, kind })
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
/// The signal is the well-known WSL drive-mount convention,
/// `/mnt/<single-letter>/...` (e.g. `/mnt/c/Users/...`), which is where WSL
/// exposes the Windows filesystem and where the Windows-appended `PATH`
/// entries point. This is checked case-insensitively on the drive letter,
/// matching how Windows drive letters work.
///
/// An earlier version of this function also treated any `.exe` extension as
/// a secondary Windows-side signal, even outside `/mnt`. That rule was
/// removed: `which` does not append `PATHEXT` on Linux, so it could only
/// ever fire when the caller literally asked for a name ending in `.exe` —
/// in which case the hit is almost always under `/mnt/<letter>` anyway and
/// this rule alone already catches it — while it would wrongly reject a
/// genuinely Linux-built cross-compiled `.exe` artifact (e.g. a MinGW build)
/// sitting on `PATH`. Real WSL-interop coverage instead comes from
/// canonicalizing each candidate before classification (see
/// `resolve_with_interop_predicate`), which catches the much more common
/// case this drive-mount check alone would miss: a Linux-looking symlink
/// that actually resolves into `/mnt/<drive>`.
///
/// Returns `false` outright for every platform other than
/// [`HostPlatform::Wsl`]: `/mnt/c` has no special meaning on real Linux,
/// macOS, or native Windows itself.
pub(crate) fn is_windows_interop_path(platform: HostPlatform, path: &Path) -> bool {
    if platform != HostPlatform::Wsl {
        return false;
    }
    is_under_windows_drive_mount(path)
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

/// Whether the current user can execute `path`.
///
/// On Unix this asks the kernel the actual question via `access(2)` with
/// `X_OK` -- the same check [`which`] uses when it walks `PATH` -- rather
/// than inspecting the permission bits directly. `mode & 0o111 != 0` only
/// asks "does *anyone* have an execute bit set", which can be true for a
/// path this process cannot actually execute (e.g. a root-owned `0o700`
/// file readable-but-not-executable by the current user): that combination
/// would pass a mode-bit check here and then fail at spawn time with a
/// confusing `EACCES`, instead of the clear [`ResolveError::NotExecutable`]
/// this function exists to produce. On non-Unix, executability is governed
/// by extension and ACLs rather than a permission bit, so existence as a
/// regular file is what actually matters for spawning it.
fn is_executable(path: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::ffi::CString;
        use std::os::unix::ffi::OsStrExt;

        let Ok(c_path) = CString::new(path.as_os_str().as_bytes()) else {
            // A path with an interior NUL can never be valid; report it as
            // not executable rather than panicking on the CString::new
            // failure.
            return false;
        };
        // SAFETY: `c_path` is a valid, NUL-terminated C string that outlives
        // this call, and `access` neither retains the pointer nor mutates
        // through it.
        unsafe { libc::access(c_path.as_ptr(), libc::X_OK) == 0 }
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

    /// File name a PATH search for `stem` can actually find on this platform.
    ///
    /// Windows decides executability by extension via `PATHEXT`, so a file
    /// named plainly `claude` is not a candidate there no matter how the
    /// permissions look. Searching for the bare stem still finds it, because
    /// that is exactly what `PATHEXT` expansion is for.
    fn executable_name(stem: &str) -> String {
        if cfg!(windows) {
            format!("{stem}.exe")
        } else {
            stem.to_owned()
        }
    }

    /// Create a findable fake executable named after `stem` inside `dir`.
    fn write_executable(dir: &Path, stem: &str) -> PathBuf {
        let path = dir.join(executable_name(stem));
        write_file(&path);
        #[cfg(unix)]
        make_executable(&path);
        path
    }

    // --- spawn_command ---------------------------------------------------

    #[test]
    fn spawn_command_direct_passes_through_unchanged() {
        let resolved = ResolvedExecutable {
            path: PathBuf::from("/usr/bin/claude"),
            kind: LaunchKind::Direct,
        };
        let (program, args) = resolved.spawn_command(["--resume", "abc"]).unwrap();
        assert_eq!(program, PathBuf::from("/usr/bin/claude"));
        assert_eq!(
            args,
            vec![OsString::from("--resume"), OsString::from("abc")]
        );
    }

    #[test]
    fn spawn_command_direct_passes_through_metacharacters_unchanged() {
        // No shell parses Direct argv, so there is nothing to reject here —
        // this is the control case proving validation is specific to
        // WindowsScript, not blanket-applied.
        let resolved = ResolvedExecutable {
            path: PathBuf::from("/usr/bin/claude"),
            kind: LaunchKind::Direct,
        };
        let (_program, args) = resolved.spawn_command(["--session=a&calc.exe"]).unwrap();
        assert_eq!(args, vec![OsString::from("--session=a&calc.exe")]);
    }

    #[test]
    fn spawn_command_windows_script_wraps_through_the_interpreter() {
        let script = PathBuf::from(r"C:\Users\me\AppData\Roaming\npm\claude.cmd");
        let resolved = ResolvedExecutable {
            path: script.clone(),
            kind: LaunchKind::WindowsScript,
        };
        let (program, args) = resolved.spawn_command(["--resume", "abc"]).unwrap();

        let expected_interpreter = std::env::var_os("COMSPEC")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("cmd.exe"));
        assert_eq!(program, expected_interpreter);
        // `/D` before `/C`, then the script path, then args in order — exactly.
        assert_eq!(
            args,
            vec![
                OsString::from("/D"),
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
        let (_program, args) = resolved.spawn_command(Vec::<OsString>::new()).unwrap();
        assert_eq!(args[0], OsString::from("/D"));
        assert_eq!(args[1], OsString::from("/C"));
        assert_eq!(args[2], OsString::from(r"C:\tools\codex.bat"));
        assert_eq!(args.len(), 3);
    }

    #[test]
    fn spawn_command_windows_script_rejects_each_cmd_metacharacter() {
        for character in CMD_UNSAFE_CHARACTERS {
            let resolved = ResolvedExecutable {
                path: PathBuf::from(r"C:\tools\codex.bat"),
                kind: LaunchKind::WindowsScript,
            };
            let bad_arg = format!("--session=a{character}calc");
            let err = resolved.spawn_command([bad_arg]).unwrap_err();
            match err {
                LaunchError::UnsafeCmdArgument {
                    position,
                    character: found,
                } => {
                    assert_eq!(position, ArgumentPosition::Argument(0));
                    assert_eq!(found, *character, "wrong character reported");
                }
            }
        }
    }

    #[test]
    fn spawn_command_windows_script_rejects_a_metacharacter_in_the_script_path() {
        let resolved = ResolvedExecutable {
            path: PathBuf::from(r"C:\tools\codex&calc.bat"),
            kind: LaunchKind::WindowsScript,
        };
        let err = resolved.spawn_command(Vec::<OsString>::new()).unwrap_err();
        match err {
            LaunchError::UnsafeCmdArgument { position, .. } => {
                assert_eq!(position, ArgumentPosition::ScriptPath);
            }
        }
    }

    #[test]
    fn spawn_command_windows_script_reports_the_offending_argument_position() {
        let resolved = ResolvedExecutable {
            path: PathBuf::from(r"C:\tools\codex.bat"),
            kind: LaunchKind::WindowsScript,
        };
        let err = resolved
            .spawn_command(["--fine", "--also-fine", "--bad=x|y"])
            .unwrap_err();
        match err {
            LaunchError::UnsafeCmdArgument { position, .. } => {
                assert_eq!(position, ArgumentPosition::Argument(2));
            }
        }
    }

    #[test]
    fn spawn_command_windows_script_accepts_normal_flags_paths_and_uuids() {
        let resolved = ResolvedExecutable {
            path: PathBuf::from(r"C:\Users\me\AppData\Roaming\npm\claude.cmd"),
            kind: LaunchKind::WindowsScript,
        };
        let (_program, args) = resolved
            .spawn_command([
                "--resume",
                "550e8400-e29b-41d4-a716-446655440000",
                "--base-url=https://api.example.com/v1",
                r"C:\Users\me\project",
                "--model=claude-3.7-sonnet",
            ])
            .expect("ordinary flag/path/uuid arguments must be accepted");
        assert_eq!(args.len(), 3 + 5); // /D /C <script> + 5 args
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
        let candidate = write_executable(dir.path(), "claude");

        let path_list = OsString::from(dir.path());
        // The resolver canonicalizes every candidate before classifying it, so
        // the predicate has to compare against the canonical form. On macOS a
        // temporary directory lives under `/var`, which is a symlink to
        // `/private/var`, so the raw and canonical paths genuinely differ.
        let marked = std::fs::canonicalize(&candidate).unwrap();

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
                assert!(found_at[0].ends_with(executable_name("claude")));
            }
            other => panic!("expected WindowsInteropOnly, got {other:?}"),
        }
    }

    #[test]
    fn resolver_prefers_usable_hits_over_interop_hits() {
        let dir = tempfile::tempdir().unwrap();
        write_executable(dir.path(), "claude");

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
        assert!(resolved.path().ends_with(executable_name("claude")));
    }

    // Unix only: the case this guards is a WSL PATH entry symlinked into the
    // Windows filesystem, and creating a symlink on a Windows CI runner needs
    // privileges it does not have.
    #[cfg(unix)]
    #[test]
    fn resolver_classifies_symlinks_by_their_canonicalized_target_not_the_raw_hit() {
        // Regression test for the ordering bug in Finding 2: `which_in_all`
        // reports the raw PATH hit with no symlink resolution, so
        // classifying on the raw path would miss a symlink whose *target*
        // is Windows-interop-shaped -- exactly the
        // `/usr/local/bin/claude -> /mnt/c/.../claude.exe` case the module
        // docs describe. This builds a real symlink on disk so the
        // resolver's actual `std::fs::canonicalize` call is exercised, not
        // a stand-in.
        let dir = tempfile::tempdir().unwrap();

        // The real target. Its path does not itself start with `/mnt/<x>`
        // (tests cannot rely on a writable real `/mnt/c`), but that is not
        // the point of this test: the predicate below identifies it by
        // exact canonicalized identity, so what matters is only *which*
        // path — raw symlink or resolved target — reaches the predicate.
        let target_dir = dir.path().join("windows-side").join("c-drive-mount");
        std::fs::create_dir_all(&target_dir).unwrap();
        let target = write_executable(&target_dir, "claude");

        // A separate "Linux-looking" bin directory holding only a symlink
        // to that target — this is what `which_in_all` actually reports as
        // the raw candidate.
        let bin = dir.path().join("usr-local-bin");
        std::fs::create_dir_all(&bin).unwrap();
        let symlink_path = bin.join("claude");
        std::os::unix::fs::symlink(&target, &symlink_path).unwrap();

        let path_list = OsString::from(&bin);
        let canonical_target = std::fs::canonicalize(&target).unwrap();

        // This predicate matches only the canonicalized target, never the
        // raw symlink path — so the resolver can only report it as interop
        // if it canonicalizes each candidate *before* classifying it.
        let err = resolve_with_interop_predicate(
            HostPlatform::Wsl,
            "claude",
            &path_list,
            dir.path(),
            move |_platform, p| p == canonical_target,
        )
        .unwrap_err();

        assert!(matches!(err, ResolveError::WindowsInteropOnly { .. }));
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

    // --- is_executable -----------------------------------------------------

    #[cfg(unix)]
    #[test]
    fn is_executable_rejects_a_non_executable_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("data.txt");
        write_file(&path);
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(&path).unwrap().permissions();
        perms.set_mode(0o644);
        std::fs::set_permissions(&path, perms).unwrap();

        assert!(!is_executable(&path));
    }

    #[cfg(unix)]
    #[test]
    fn is_executable_accepts_an_executable_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("run.sh");
        write_file(&path);
        make_executable(&path);

        assert!(is_executable(&path));
    }
}
