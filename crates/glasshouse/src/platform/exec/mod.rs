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
    /// in the first place — see `classify_launch_kind`.
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
                // `cmd.exe` cannot open a verbatim (`\\?\`) path: it prints
                // "The system cannot find the path specified" and exits 1.
                // Resolving an executable canonicalizes it, and on Windows
                // canonicalization produces exactly that form, so the path
                // has to be converted back before it becomes a `cmd.exe`
                // argument. Without this, no harness installed as a `.cmd`
                // shim can start at all -- which is how npm installs most of
                // them.
                //
                // This mirrors what `Project::display_root` already does for
                // the working directory. Both are the same rule: a verbatim
                // path is fine as an identity, and unusable at the boundary
                // where a process is actually started.
                let script = plain_script_path(&self.path);
                validate_cmd_argument(script.as_os_str(), ArgumentPosition::ScriptPath)?;
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
                full_args.push(script.into_os_string());
                full_args.extend(args);
                Ok((interpreter, full_args))
            }
        }
    }
}

/// Convert a resolved path into the form `cmd.exe` can actually open.
///
/// `\\?\C:\dir\x.cmd` becomes `C:\dir\x.cmd`, and the UNC spelling
/// `\\?\UNC\server\share\x.cmd` becomes `\\server\share\x.cmd`. Anything
/// else is returned unchanged.
///
/// Deliberately **not** `#[cfg(windows)]`-gated, unlike
/// [`crate::platform::paths::strip_verbatim_prefix`], and the difference is
/// the point. That function asks "how does *this host* spell paths?", which
/// is a question about the running platform. This one asks "what will
/// `cmd.exe` accept?", which is a property of the command line being built
/// and is the same wherever it is built. Keeping it host-independent is what
/// lets the translation be tested off Windows instead of being taken on
/// trust until CI says otherwise — which is exactly how the missing
/// conversion went unnoticed.
fn plain_script_path(path: &Path) -> PathBuf {
    let text = path.to_string_lossy();
    if let Some(rest) = text.strip_prefix(r"\\?\UNC\") {
        return PathBuf::from(format!(r"\\{rest}"));
    }
    if let Some(rest) = text.strip_prefix(r"\\?\") {
        return PathBuf::from(rest);
    }
    path.to_path_buf()
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
mod tests;
