//! The sanctioned production route for launching installed harnesses.
//!
//! [`HarnessLaunch`] ties together the two values every harness launch needs
//! — a resolved executable ([`ResolvedExecutable`]) and the active project
//! ([`Project`]) — and nothing else. It offers argument and child-environment
//! builders and `spawn`; it deliberately offers no way to set the working
//! directory or swap the program, because both are derived: the directory
//! from the project (via `TerminalCommand::for_harness`), the program from
//! the resolved executable (via
//! [`ResolvedExecutable::spawn_command`], which owns the Windows
//! `.cmd`/`.bat` interpreter wrapping so no caller ever reimplements it).
//!
//! # Honest scope
//!
//! This is structure within the sanctioned harness API, not a sandbox: the
//! underlying generic PTY APIs ([`TerminalCommand::new`] and
//! [`PtyProcess::spawn`]) stay public for genuinely generic terminal work,
//! and Rust cannot identify misuse of those. What this type guarantees is
//! that code which launches harnesses through it cannot get the working
//! directory, the launch-kind translation, or the environment semantics
//! wrong.
//!
//! Environment overrides preserve [`TerminalCommand`]'s exact
//! last-call-wins ordering: each key's operations are deduplicated at call
//! time so only its most recent `env` or `env_remove` is kept, and what
//! remains is applied in recorded order. Environment *values* are never
//! logged.

use std::ffi::OsString;

use anyhow::Context;

use crate::Project;
use std::path::Path;

use crate::platform::exec::{LaunchKind, ResolvedExecutable};
use crate::pty::{PtyOutput, PtyProcess, TerminalCommand, TerminalSize};

/// Why a particular executable cannot be launched in a particular directory,
/// or `None` when the combination is fine.
///
/// # The combination this exists for
///
/// `cmd.exe` cannot hold a UNC path as its working directory. Asked to, it
/// does not fail: it prints a notice, silently substitutes the Windows
/// directory, and carries on. So a `.cmd` harness — which is how npm installs
/// most of them — started in a project on a network share would come up
/// running in `C:\Windows` instead. That is not merely a broken session; it
/// breaks the project-isolation guarantee the whole product rests on, and it
/// does so quietly, which is the worst way for it to break.
///
/// Refusing with a diagnostic is strictly better than a session that looks
/// alive and is operating on the wrong directory.
///
/// # Why this is not `cfg`-gated
///
/// Like [`crate::platform::exec`]'s script-path conversion, this asks about
/// the *shape* of a path and the *kind* of an executable, not about the host.
/// Keeping it host-independent is what makes it testable somewhere other than
/// the platform where it matters — the same reasoning that would have caught
/// the verbatim-path defect before CI did.
fn unsupported_combination(kind: LaunchKind, cwd: &Path) -> Option<String> {
    if kind == LaunchKind::WindowsScript && is_unc_path(cwd) {
        return Some(format!(
            "cannot start a Windows `.cmd`/`.bat` harness in `{}`: that is a UNC network \
             path, and `cmd.exe` cannot use one as its working directory — it would \
             silently run in the Windows directory instead of this project. Map the \
             share to a drive letter and open the project from there, or use a harness \
             executable that is not a script shim.",
            cwd.display()
        ));
    }
    None
}

/// Whether `path` is a UNC path (`\\server\share\...`).
///
/// The verbatim UNC spelling `\\?\UNC\server\share` counts too. A verbatim
/// *drive* path (`\\?\C:\...`) does not, despite also starting with two
/// backslashes — it names a perfectly ordinary local directory, and refusing
/// it would reject the normal Windows case outright.
fn is_unc_path(path: &Path) -> bool {
    let text = path.to_string_lossy();
    if text.starts_with(r"\\?\UNC\") {
        return true;
    }
    text.starts_with(r"\\") && !text.starts_with(r"\\?\")
}

/// One recorded child-environment operation, in call order.
///
/// No `Debug`: the `Set` variant carries an environment value, so this type
/// must never be rendered wholesale — [`HarnessLaunch`]'s manual `Debug`
/// projects operations down to kind plus key name instead.
#[derive(Clone, PartialEq, Eq)]
enum EnvChange {
    /// Override `key` with `value` for the child process only.
    Set(OsString, OsString),
    /// Strip `key` from the child's inherited environment outright.
    Remove(OsString),
}

/// A prepared launch of an installed harness inside the active project.
///
/// Built with [`HarnessLaunch::new`], configured with `arg`/`args`,
/// `env`/`env_remove`, and started with [`HarnessLaunch::spawn`].
///
/// `Debug` is manual on purpose: arguments can carry session tokens and
/// environment values can carry API keys, so the rendering shows only
/// non-secret structure — executable path, project name, argument *count*,
/// and each environment operation's kind plus key name. Values are never
/// printed.
#[derive(Clone)]
pub struct HarnessLaunch<'a> {
    executable: ResolvedExecutable,
    project: &'a Project,
    args: Vec<OsString>,
    env_changes: Vec<EnvChange>,
    size: TerminalSize,
}

impl std::fmt::Debug for HarnessLaunch<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Secret-safe by construction: only counts, kinds, keys, paths, and
        // names are emitted — never `self.args` contents or any environment
        // value.
        let env_operations: Vec<(&str, &OsString)> = self
            .env_changes
            .iter()
            .map(|change| (change.kind(), change.key()))
            .collect();

        f.debug_struct("HarnessLaunch")
            .field("executable", &self.executable.path().display().to_string())
            .field("project", &self.project.name())
            .field("arg_count", &self.args.len())
            .field("env_operations", &env_operations)
            .field("size", &self.size)
            .finish()
    }
}

impl EnvChange {
    fn key(&self) -> &OsString {
        match self {
            EnvChange::Set(key, _) | EnvChange::Remove(key) => key,
        }
    }

    /// The operation kind, for redacted debug output.
    fn kind(&self) -> &'static str {
        match self {
            EnvChange::Set(..) => "set",
            EnvChange::Remove(_) => "remove",
        }
    }
}

impl<'a> HarnessLaunch<'a> {
    /// Prepare to launch `executable` inside `project`.
    ///
    /// The project is borrowed, not copied: launching is always tied to the
    /// runtime's active project, and the working directory the child gets is
    /// derived from this value at `spawn` time.
    pub fn new(executable: ResolvedExecutable, project: &'a Project) -> Self {
        Self {
            executable,
            project,
            args: Vec::new(),
            env_changes: Vec::new(),
            size: TerminalSize::default(),
        }
    }

    /// Append one argument to the harness invocation.
    pub fn arg(mut self, arg: impl Into<OsString>) -> Self {
        self.args.push(arg.into());
        self
    }

    /// Append several arguments to the harness invocation.
    pub fn args<I, S>(mut self, args: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<OsString>,
    {
        self.args.extend(args.into_iter().map(Into::into));
        self
    }

    /// Set an environment variable for the harness process only.
    ///
    /// A later call for the same key wins over any earlier `env` *or*
    /// `env_remove` for it: per-key changes are deduplicated at call time so
    /// only each key's final operation is kept, which preserves
    /// [`TerminalCommand`]'s last-call-wins rule (though not a literal
    /// recording of every operation).
    pub fn env(mut self, key: impl Into<OsString>, value: impl Into<OsString>) -> Self {
        let key = key.into();
        self.env_changes.retain(|change| change.key() != &key);
        self.env_changes.push(EnvChange::Set(key, value.into()));
        self
    }

    /// Remove an environment variable the harness would otherwise inherit.
    ///
    /// Same deduplication rule as [`HarnessLaunch::env`]: only each key's
    /// most recent operation survives.
    pub fn env_remove(mut self, key: impl Into<OsString>) -> Self {
        let key = key.into();
        self.env_changes.retain(|change| change.key() != &key);
        self.env_changes.push(EnvChange::Remove(key));
        self
    }

    /// Set the terminal size the harness starts with.
    ///
    /// Harness TUIs lay themselves out from the size they see at startup, so
    /// a session attached to a real terminal must pass that terminal's size
    /// here rather than let the child come up at the default 24x80 and be
    /// resized afterwards — the first frame would otherwise be drawn for the
    /// wrong geometry.
    ///
    /// Unlike the working directory, size is not derived from the project:
    /// it belongs to whatever surface is displaying the session, which the
    /// project knows nothing about. Defaults to [`TerminalSize::default`].
    pub fn size(mut self, size: TerminalSize) -> Self {
        self.size = size;
        self
    }

    /// Assemble the concrete [`TerminalCommand`] this launch describes.
    ///
    /// Split from [`HarnessLaunch::spawn`] so tests can inspect the assembled
    /// command without spawning anything. The order of operations here is the
    /// contract: translate through `spawn_command` (which owns the Windows
    /// `.cmd`/`.bat` interpreter wrapping and rejects unsafe arguments), then
    /// derive the working directory from the project through the crate's
    /// sanctioned seam, then apply the recorded environment operations **in
    /// recorded order** (each key appears at most once — its last call).
    ///
    /// Errors propagate from `spawn_command` unchanged: its messages name the
    /// argument *position* and offending *character*, never the value, so
    /// nothing secret leaks into diagnostics.
    fn build_command(&self) -> anyhow::Result<TerminalCommand> {
        let (program, translated_args) = self
            .executable
            .spawn_command(self.args.iter().cloned())
            .with_context(|| {
            format!(
                "could not assemble a safe command line for `{}`",
                self.executable.path().display()
            )
        })?;

        let mut command = TerminalCommand::for_harness(program, self.project)
            .args(translated_args)
            .size(self.size);

        // Refuse a combination that cannot work *before* a process exists,
        // rather than starting one that silently runs somewhere else.
        if let Some(reason) = unsupported_combination(self.executable.kind(), command.cwd()) {
            anyhow::bail!(reason);
        }

        // Replay in recorded order: this is what preserves last-call-wins
        // across mixed `env`/`env_remove` sequences. Values are never logged.
        for change in &self.env_changes {
            command = match change {
                EnvChange::Set(key, value) => command.env(key, value),
                EnvChange::Remove(key) => command.env_remove(key),
            };
        }
        Ok(command)
    }

    /// Open a pseudo-terminal and start the harness inside its project.
    pub fn spawn(&self) -> anyhow::Result<(PtyProcess, PtyOutput)> {
        let command = self.build_command()?;
        PtyProcess::spawn(command).with_context(|| {
            format!(
                "could not start `{}` in project `{}`",
                self.executable.path().display(),
                self.project.name()
            )
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::HostPlatform;
    use crate::platform::exec::{self, LaunchKind};
    use std::path::Path;

    /// A real, executable file resolved as a `Direct` launch — no spawning,
    /// just classification.
    fn direct_executable(path: &Path) -> ResolvedExecutable {
        std::fs::write(path, "#!/bin/sh\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = std::fs::metadata(path).unwrap().permissions();
            perms.set_mode(0o755);
            std::fs::set_permissions(path, perms).unwrap();
        }
        exec::resolve_explicit(path).expect("resolve")
    }

    fn project_at(name: &str) -> (tempfile::TempDir, Project) {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join(name);
        std::fs::create_dir_all(root.join(".git")).unwrap();
        let project = Project::discover(&root, None, false).unwrap();
        (tmp, project)
    }

    #[test]
    fn direct_arguments_are_translated_verbatim_into_the_terminal_command() {
        let tmp = tempfile::tempdir().unwrap();
        let executable = direct_executable(&tmp.path().join("fake-harness"));
        assert_eq!(executable.kind(), LaunchKind::Direct);
        let (_guard, project) = project_at("proj");

        let launch = HarnessLaunch::new(executable, &project)
            .arg("--resume")
            .args(["abc123", "--model=x"]);
        let command = launch.build_command().expect("safe arguments");

        // The program is the resolved executable itself (Direct: no
        // interpreter wrapping).
        assert!(
            command.program().ends_with("fake-harness"),
            "unexpected program: {:?}",
            command.program()
        );
        let args: Vec<_> = command.args_slice().to_vec();
        assert_eq!(args.len(), 3);
        assert_eq!(args[0], OsString::from("--resume"));
        assert_eq!(args[1], OsString::from("abc123"));
        assert_eq!(args[2], OsString::from("--model=x"));
        // The working directory came from the project, nowhere else.
        assert_eq!(command.cwd(), project.display_root());
    }

    #[test]
    fn env_operations_replay_in_order_so_the_last_call_wins() {
        let tmp = tempfile::tempdir().unwrap();
        let executable = direct_executable(&tmp.path().join("fake-harness"));
        let (_guard, project) = project_at("proj");

        let launch = HarnessLaunch::new(executable, &project)
            .env("GLASSHOUSE_TEST_KEY", "first")
            .env_remove("GLASSHOUSE_TEST_KEY")
            .env("GLASSHOUSE_TEST_KEY", "second")
            .env("OTHER", "kept")
            .env_remove("GONE");
        let command = launch.build_command().expect("safe arguments");

        // Last call wins per key: GLASSHOUSE_TEST_KEY ends as an override
        // with value `second`, GONE ends as a removal, OTHER is kept.
        let overrides: Vec<_> = command
            .env_overrides()
            .iter()
            .filter(|(k, _)| k == "GLASSHOUSE_TEST_KEY")
            .collect();
        assert_eq!(overrides.len(), 1);
        assert_eq!(overrides[0].1, OsString::from("second"));
        assert!(
            command.env_overrides().iter().all(|(k, _)| k != "GONE"),
            "a removal must not leave an override behind"
        );
        assert_eq!(command.env_removals(), &[OsString::from("GONE")]);
        assert!(
            command
                .env_overrides()
                .iter()
                .any(|(k, v)| k == "OTHER" && v == "kept")
        );
    }

    #[test]
    fn a_windows_script_executable_is_translated_through_the_interpreter() {
        // Cross-platform by design: `classify_launch_kind` keys off the
        // injected platform value, not the compile target, so this exercises
        // the exact translation production Windows launches go through
        // without needing Windows or touching global state (COMSPEC is only
        // read, never written).
        let tmp = tempfile::tempdir().unwrap();
        let script = tmp.path().join("fake-harness.cmd");
        std::fs::write(&script, "@echo off\r\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = std::fs::metadata(&script).unwrap().permissions();
            perms.set_mode(0o755);
            std::fs::set_permissions(&script, perms).unwrap();
        }
        let executable = exec::resolve_explicit_with(HostPlatform::Windows, &script)
            .expect("resolve explicit windows script");
        assert_eq!(executable.kind(), LaunchKind::WindowsScript);

        let (_guard, project) = project_at("proj");
        let launch = HarnessLaunch::new(executable, &project).arg("--flag=value");

        // And the assembled TerminalCommand carries the translated argv, not
        // the raw script path: /D /C <script> <args>, exactly as production
        // Windows launches go through cmd.exe.
        let command = launch.build_command().expect("safe arguments");
        let expected_interpreter = std::env::var_os("COMSPEC")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| std::path::PathBuf::from("cmd.exe"));
        assert_eq!(command.program(), expected_interpreter);
        assert_eq!(command.args_slice()[0], OsString::from("/D"));
        assert_eq!(command.args_slice()[1], OsString::from("/C"));
        assert!(
            command.args_slice()[2]
                .to_string_lossy()
                .ends_with("fake-harness.cmd"),
            "unexpected translated argv: {:?}",
            command.args_slice()
        );
        // The script path must reach cmd.exe in a form it can open. On
        // Windows the resolved path is canonical and therefore verbatim, and
        // cmd.exe rejects that outright — the failure this pins was a real
        // `windows-latest` red, not a hypothetical. Off Windows there is no
        // prefix to strip and the assertion simply holds.
        assert!(
            !command.args_slice()[2]
                .to_string_lossy()
                .starts_with(r"\\?\"),
            "cmd.exe cannot open a verbatim path: {:?}",
            command.args_slice()[2]
        );
        assert_eq!(command.args_slice()[3], OsString::from("--flag=value"));
    }

    #[test]
    fn debug_output_never_leaks_argument_or_environment_values() {
        let tmp = tempfile::tempdir().unwrap();
        let executable = direct_executable(&tmp.path().join("fake-harness"));
        let (_guard, project) = project_at("proj");

        let launch = HarnessLaunch::new(executable, &project)
            .arg("--api-key=sk-SUPER-SECRET-token-12345")
            .arg("SESSION-TOKEN-abcdef-value")
            .env("GLASSHOUSE_TEST_KEY", "hunter2-TOP-SECRET-env-value")
            .env_remove("GLASSHOUSE_REMOVED_KEY");

        // The unmistakable secret-shaped values must appear nowhere in the
        // debug rendering.
        let rendered = format!("{launch:?}");
        assert!(
            !rendered.contains("sk-SUPER-SECRET-token-12345"),
            "Debug leaked an argument value: {rendered}"
        );
        assert!(
            !rendered.contains("SESSION-TOKEN-abcdef-value"),
            "Debug leaked an argument value: {rendered}"
        );
        assert!(
            !rendered.contains("hunter2-TOP-SECRET-env-value"),
            "Debug leaked an environment value: {rendered}"
        );

        // Useful non-secret structure survives: type name, executable path,
        // project name, argument count, and env operation kind/key pairs.
        assert!(rendered.contains("HarnessLaunch"), "{rendered}");
        assert!(rendered.contains("fake-harness"), "{rendered}");
        assert!(rendered.contains("proj"), "{rendered}");
        assert!(rendered.contains("arg_count: 2"), "{rendered}");
        assert!(rendered.contains("GLASSHOUSE_TEST_KEY"), "{rendered}");
        assert!(rendered.contains("set"), "{rendered}");
        assert!(rendered.contains("GLASSHOUSE_REMOVED_KEY"), "{rendered}");
        assert!(rendered.contains("remove"), "{rendered}");
    }

    /// A `.cmd` harness in a UNC project must be refused, because `cmd.exe`
    /// would not fail there — it would substitute the Windows directory and
    /// run anyway, breaking project isolation silently.
    #[test]
    fn a_script_harness_in_a_unc_project_is_refused_with_a_diagnostic() {
        let reason = unsupported_combination(
            LaunchKind::WindowsScript,
            Path::new(r"\\fileserver\team\project"),
        )
        .expect("a script harness in a UNC directory must be refused");

        // The message has to say what is wrong *and* what to do about it;
        // "unsupported" alone would leave the user nowhere.
        assert!(reason.contains(r"\\fileserver\team\project"), "{reason}");
        assert!(reason.contains("UNC"), "{reason}");
        assert!(reason.contains("drive letter"), "{reason}");
    }

    /// The refusal must be narrow. A non-script harness has no `cmd.exe` in
    /// the picture, and an ordinary local directory is the normal case — if
    /// either were refused, the check would be worse than not having it.
    #[test]
    fn every_other_combination_is_allowed() {
        for (kind, cwd) in [
            (LaunchKind::Direct, r"\\fileserver\team\project"),
            (LaunchKind::WindowsScript, r"C:\Users\me\project"),
            (LaunchKind::WindowsScript, r"\\?\C:\Users\me\project"),
            (LaunchKind::Direct, "/home/me/project"),
            (LaunchKind::WindowsScript, "/home/me/project"),
        ] {
            assert!(
                unsupported_combination(kind, Path::new(cwd)).is_none(),
                "{kind:?} in {cwd} should be allowed"
            );
        }
    }

    #[test]
    fn unc_detection_covers_both_spellings_but_not_a_verbatim_drive() {
        assert!(is_unc_path(Path::new(r"\\server\share\p")));
        // The verbatim UNC spelling denotes the same thing.
        assert!(is_unc_path(Path::new(r"\\?\UNC\server\share\p")));
        // A verbatim *drive* path also starts with two backslashes and is an
        // ordinary local directory; refusing it would reject normal Windows.
        assert!(!is_unc_path(Path::new(r"\\?\C:\p")));
        assert!(!is_unc_path(Path::new(r"C:\p")));
        assert!(!is_unc_path(Path::new("/home/me/p")));
    }

    #[test]
    fn unsafe_windows_script_arguments_are_rejected_before_any_spawn() {
        let tmp = tempfile::tempdir().unwrap();
        let script = tmp.path().join("shim.cmd");
        std::fs::write(&script, "@echo off\r\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = std::fs::metadata(&script).unwrap().permissions();
            perms.set_mode(0o755);
            std::fs::set_permissions(&script, perms).unwrap();
        }
        let executable = exec::resolve_explicit_with(HostPlatform::Windows, &script).unwrap();

        let (_guard, project) = project_at("proj");

        // The BatBadBut shape: no CRT-quote trigger, but `&` chains a second
        // command under cmd.exe. Going through HarnessLaunch itself — not
        // `spawn_command` directly — assembly must fail, and `spawn` would
        // propagate the same error before any process exists. No spawning
        // happens in this test.
        let launch = HarnessLaunch::new(executable, &project).arg("--session=a&calc.exe");
        let err = launch.build_command().unwrap_err();

        // Secret-safe diagnostics: they name the position and offending
        // character, never the argument value.
        let message = format!("{err:#}");
        assert!(message.contains("cmd.exe"), "unexpected error: {message}");
        assert!(
            message.contains("argument 1"),
            "unexpected error: {message}"
        );
        assert!(
            !message.contains("a&calc"),
            "diagnostic leaked the argument value: {message}"
        );
    }
}
