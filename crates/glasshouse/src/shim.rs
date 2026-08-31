//! `glasshouse shim` — a tiny executable that `exec`s `glasshouse run`.
//!
//! A shim is the entire mechanism for putting a launch profile on a user's
//! `PATH`: one file, written to a directory the user names, whose only job
//! is to `exec` `glasshouse run <harness> --profile <name>` with its own
//! arguments forwarded. It carries no credential, no base URL, and no copy
//! of the profile itself — only a harness name, a profile name, and this
//! executable's own absolute path, none of which needs to stay secret.
//!
//! Deleting the file is the entire removal story: nothing else is ever
//! written on its behalf (see [`generate`]'s doc), and Glasshouse never
//! touches a shell startup file to make the shim reachable on `PATH` — see
//! `generating_a_shim_never_touches_a_shell_startup_file` below.
//!
//! Content is chosen from the injected [`HostPlatform`], never
//! `#[cfg(windows)]`, so both the `.cmd` and the `#!/bin/sh` spellings are
//! exercised by tests on every runner regardless of which one is actually
//! executing them — the same seam [`crate::platform::exec`] already uses for
//! the same reason.

use std::path::{Path, PathBuf};

use crate::platform::HostPlatform;

/// Why generating a shim failed.
#[derive(Debug, thiserror::Error)]
pub enum ShimError {
    /// A file already sits at the destination and `--force` was not given.
    #[error("`{path}` already exists; pass --force to overwrite it")]
    AlreadyExists { path: PathBuf },

    /// A name would be interpolated into the generated script, and it
    /// carries a character that a shell or `cmd.exe` would act on.
    ///
    /// `name` is formatted with `{name:?}` (escaped, quoted) rather than
    /// `{name}` (raw) — the rejected value is exactly the one that failed a
    /// character check, so it may itself contain a newline or other control
    /// character, and echoing that verbatim into a log would let the
    /// refused input inject a fake line into it.
    #[error(
        "refusing to generate a shim for {field} {name:?}: it contains `{offending}`, which a \
         shell would interpret rather than pass through. Names may use letters, digits, `-`, \
         `_` and `.` only"
    )]
    UnsafeName {
        field: &'static str,
        name: String,
        offending: char,
    },

    /// `--name` (the on-disk write target) failed a check beyond the
    /// character allow-list [`UnsafeName`](ShimError::UnsafeName) applies —
    /// see `check_file_name` in this module. `reason` is always a fixed
    /// string, never built from the rejected value, so there is nothing here
    /// for an attacker-controlled name to inject.
    #[error("refusing to generate a shim for {field}: {reason}")]
    InvalidName {
        field: &'static str,
        reason: &'static str,
    },

    /// The file (or its permissions, on Unix) could not be written.
    #[error("could not write `{path}`: {source}")]
    Write {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

/// Everything one generated shim needs, and nothing else.
///
/// No field here may ever hold a credential, a base URL, or the profile's
/// own configuration — only its *name*. Resolving what the profile actually
/// means happens later, inside `glasshouse run`, against whatever
/// configuration is live at the time the shim is executed.
#[derive(Debug, Clone, Copy)]
pub struct ShimRequest<'a> {
    /// The harness identifier the shim opens, e.g. `claude-code`.
    pub harness: &'a str,
    /// The launch profile name the shim resolves the session through.
    pub profile: &'a str,
    /// This Glasshouse executable's own path, embedded so the shim can
    /// `exec` back into it without relying on `PATH`.
    pub glasshouse_exe: &'a Path,
    /// The user-chosen directory the shim is written into. Never anywhere
    /// else — see [`generate`]'s doc.
    pub dir: &'a Path,
    /// File name for the shim, when the caller chooses one explicitly.
    /// `None` defaults to the harness name (`.cmd` appended on Windows).
    pub name: Option<&'a str>,
    /// Overwrite a file already at the destination.
    pub force: bool,
}

/// Refuse a name that a shell or `cmd.exe` would act on rather than pass
/// through.
///
/// The generated shim interpolates the harness and profile names into a
/// script. A profile name is user-chosen, so it is untrusted input reaching a
/// command line — and this codebase already answers that class of problem by
/// **refusing** rather than escaping: `platform::exec` rejects `cmd.exe`
/// metacharacters in harness arguments instead of trying to quote them.
///
/// Refusing is the better answer here for the same reason it was there. A
/// general shell-escaper has to be right about two different shells forever,
/// while an allow-list is right by construction — and a launch profile is a
/// short identifier, a TOML table key, so nothing legitimate is lost.
fn check_name(field: &'static str, name: &str) -> Result<(), ShimError> {
    if let Some(offending) = name
        .chars()
        .find(|c| !(c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.')))
    {
        return Err(ShimError::UnsafeName {
            field,
            name: name.to_owned(),
            offending,
        });
    }
    Ok(())
}

/// Refuse a `--name` that would make [`generate`]'s write land somewhere
/// other than `request.dir.join(<name>)`.
///
/// `--name` becomes the on-disk write target, not a script interpolation
/// like `harness` or `profile` — the hazard is worse, not smaller: an
/// absolute path (or, on Windows, a drive- or UNC-qualified one) makes
/// `request.dir.join(file_name)` discard `dir` entirely, and a path
/// separator lets `name` climb out of `dir` via `..`. [`check_name`]'s
/// character allow-list already refuses every `/`, `\` and `:` — refusing by
/// character set rather than trying to parse a path, so this closes both a
/// Unix absolute path and every Windows spelling (`C:\x`, `\\?\x`, bare
/// `C:x`) the same way. Two values pass that allow-list unchanged and still
/// need a name of their own: the empty string, and a bare `..`, which today
/// is inert (it resolves to `dir`'s own parent, an existing directory, so
/// `std::fs::write` fails with "is a directory" rather than writing
/// anywhere) but is refused explicitly so that stays true regardless of how
/// the path is computed later.
fn check_file_name(name: &str) -> Result<(), ShimError> {
    if name.is_empty() {
        return Err(ShimError::InvalidName {
            field: "name",
            reason: "may not be empty",
        });
    }
    if name == ".." {
        return Err(ShimError::InvalidName {
            field: "name",
            reason: "may not be `..`",
        });
    }
    check_name("name", name)
}

/// The file name a shim gets when the caller does not choose one.
fn default_file_name(platform: HostPlatform, harness: &str) -> String {
    if platform.is_windows() {
        format!("{harness}.cmd")
    } else {
        harness.to_owned()
    }
}

/// The shim's file contents for `platform`.
///
/// Both branches do exactly one thing: forward every argument the shim
/// itself receives into `glasshouse run <harness> --profile <profile> --
/// <forwarded args>`. Neither ever names an adapter-specific flag — the `--`
/// separator is what keeps this a pure pass-through rather than a duplicate
/// of whatever the harness's own adapter already knows how to build.
fn render(platform: HostPlatform, harness: &str, profile: &str, glasshouse_exe: &Path) -> String {
    let exe = glasshouse_exe.display();
    if platform.is_windows() {
        format!("@echo off\r\n\"{exe}\" run \"{harness}\" --profile \"{profile}\" -- %*\r\n")
    } else {
        format!("#!/bin/sh\nexec \"{exe}\" run \"{harness}\" --profile \"{profile}\" -- \"$@\"\n")
    }
}

/// Write one shim file for `request`, refusing to overwrite an existing one
/// unless `request.force` is set.
///
/// This is the only thing in this module that touches the filesystem, and it
/// writes exactly one file, exactly at `request.dir.join(<name>)` — never a
/// parent directory it creates on the user's behalf, never any location
/// outside `request.dir`, and never a shell startup file. Deleting that one
/// file afterward leaves nothing else behind.
pub fn generate(platform: HostPlatform, request: &ShimRequest<'_>) -> Result<PathBuf, ShimError> {
    // Before any path is computed or any byte written: these names are
    // interpolated into a script, so an unsafe one is refused outright.
    check_name("harness", request.harness)?;
    check_name("profile", request.profile)?;
    // `request.name`, when given, becomes the write target itself — see
    // `check_file_name`'s doc for why it needs more than the interpolation
    // check above.
    if let Some(name) = request.name {
        check_file_name(name)?;
    }

    let file_name = request
        .name
        .map(str::to_owned)
        .unwrap_or_else(|| default_file_name(platform, request.harness));
    let path = request.dir.join(file_name);

    if path.exists() && !request.force {
        return Err(ShimError::AlreadyExists { path });
    }

    let contents = render(
        platform,
        request.harness,
        request.profile,
        request.glasshouse_exe,
    );
    std::fs::write(&path, contents).map_err(|source| ShimError::Write {
        path: path.clone(),
        source,
    })?;

    // A mode bit is only meaningful on a real Unix filesystem, so this stays
    // `#[cfg(unix)]` rather than keyed off the injected `platform`: unlike
    // the content shape above (which is a product decision, testable
    // anywhere), whether `chmod` exists at all is a fact about the host, and
    // `platform.is_unix()` here is only ever true in production when the
    // host genuinely is one.
    #[cfg(unix)]
    if platform.is_unix() {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(&path)
            .map_err(|source| ShimError::Write {
                path: path.clone(),
                source,
            })?
            .permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&path, perms).map_err(|source| ShimError::Write {
            path: path.clone(),
            source,
        })?;
    }

    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// This module's own source, with its `#[cfg(test)]` block (and this
    /// helper's own string literals) excluded, and `//` comments stripped —
    /// the same idiom as
    /// `harness::resolving_a_launch_profile_touches_no_files`'s
    /// `production_code` helper.
    fn production_code(source: &str) -> String {
        source
            .split("#[cfg(test)]")
            .next()
            .expect("split always yields at least one part")
            .lines()
            .filter(|line| !line.trim_start().starts_with("//"))
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn request<'a>(
        harness: &'a str,
        profile: &'a str,
        exe: &'a Path,
        dir: &'a Path,
        name: Option<&'a str>,
        force: bool,
    ) -> ShimRequest<'a> {
        ShimRequest {
            harness,
            profile,
            glasshouse_exe: exe,
            dir,
            name,
            force,
        }
    }

    /// A profile name is user-chosen and lands inside a generated script, so
    /// it is untrusted input reaching a command line. This codebase refuses
    /// that class of input rather than escaping it — the same answer
    /// `platform::exec` gives for `cmd.exe` metacharacters in harness
    /// arguments.
    #[test]
    fn a_shell_unsafe_name_is_refused_before_any_file_is_written() {
        let tmp = tempfile::tempdir().unwrap();
        let exe = std::path::Path::new("/usr/local/bin/glasshouse");

        // Each of these would break out of the double quotes the shim
        // interpolates the name into, or be acted on by cmd.exe.
        for hostile in [
            "evil\"; rm -rf /; echo \"",
            "$(whoami)",
            "`id`",
            "a&calc.exe",
            "a|b",
            "a b",
            "a\nb",
            "a>out",
        ] {
            let request = ShimRequest {
                harness: "claude-code",
                profile: hostile,
                glasshouse_exe: exe,
                dir: tmp.path(),
                name: Some("shim-under-test"),
                force: true,
            };
            let err = generate(HostPlatform::Linux, &request).unwrap_err();
            assert!(
                matches!(err, ShimError::UnsafeName { .. }),
                "profile name {hostile:?} should be refused, got: {err}"
            );
        }

        // And nothing was written while refusing — the check runs before the
        // path is even computed.
        assert!(
            !tmp.path().join("shim-under-test").exists(),
            "a refused shim must leave no file behind"
        );

        // The ordinary case still works, so the allow-list is not so narrow
        // that it refuses real profile names.
        for good in ["native", "claude-openrouter", "gateway_2", "v1.2"] {
            let request = ShimRequest {
                harness: "claude-code",
                profile: good,
                glasshouse_exe: exe,
                dir: tmp.path(),
                name: Some("shim-ok"),
                force: true,
            };
            generate(HostPlatform::Linux, &request)
                .unwrap_or_else(|err| panic!("{good} should be accepted: {err}"));
        }
    }

    #[test]
    fn a_generated_shim_contains_no_secret_and_no_url() {
        let tmp = tempfile::tempdir().unwrap();
        let exe = PathBuf::from("/opt/glasshouse/bin/glasshouse");
        let req = request("claude-code", "gateway", &exe, tmp.path(), None, false);

        let path = generate(HostPlatform::Linux, &req).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();

        assert!(
            !text.to_ascii_lowercase().contains("http"),
            "shim names a URL scheme: {text}"
        );
        for key_shaped in ["sk-", "Bearer ", "api_key", "Authorization", "://"] {
            assert!(
                !text.contains(key_shaped),
                "shim contains a key-shaped string `{key_shaped}`: {text}"
            );
        }
    }

    #[test]
    fn a_generated_shim_calls_glasshouse_run() {
        let tmp = tempfile::tempdir().unwrap();
        let exe = PathBuf::from("/opt/glasshouse/bin/glasshouse");
        let req = request("claude-code", "fast", &exe, tmp.path(), None, false);

        let path = generate(HostPlatform::Linux, &req).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();

        assert!(
            text.contains("glasshouse"),
            "shim never names glasshouse: {text}"
        );
        assert!(text.contains(" run "), "shim never calls `run`: {text}");
        // No adapter-specific argument is duplicated here — the shim only
        // ever forwards its own arguments verbatim.
        for adapter_flag in [
            "--resume",
            "--model",
            "--base-url",
            "--api-key",
            "--permission-mode",
            "--dangerously-skip-permissions",
        ] {
            assert!(
                !text.contains(adapter_flag),
                "shim duplicates an adapter argument `{adapter_flag}`: {text}"
            );
        }
    }

    #[test]
    fn a_shim_is_written_only_inside_the_user_selected_directory() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("tools");
        std::fs::create_dir_all(&dir).unwrap();
        let elsewhere = tmp.path().join("elsewhere");
        std::fs::create_dir_all(&elsewhere).unwrap();

        let exe = PathBuf::from("/opt/glasshouse/bin/glasshouse");
        let req = request("codex", "native", &exe, &dir, None, false);
        let path = generate(HostPlatform::Linux, &req).unwrap();

        assert_eq!(path.parent(), Some(dir.as_path()));
        assert!(
            std::fs::read_dir(&elsewhere).unwrap().next().is_none(),
            "generating a shim wrote something outside --dir"
        );
    }

    #[test]
    fn a_windows_shim_is_a_cmd_file_and_a_unix_shim_is_a_shell_script() {
        // Both branches are exercised on every runner: rendering keys off
        // the injected platform, never the compile target.
        let tmp = tempfile::tempdir().unwrap();
        let exe = PathBuf::from("/opt/glasshouse/bin/glasshouse");

        let unix_dir = tmp.path().join("unix");
        std::fs::create_dir_all(&unix_dir).unwrap();
        let unix_req = request("codex", "native", &exe, &unix_dir, None, false);
        let unix_path = generate(HostPlatform::Linux, &unix_req).unwrap();
        assert_eq!(unix_path.extension(), None);
        let unix_contents = std::fs::read_to_string(&unix_path).unwrap();
        assert!(unix_contents.starts_with("#!/bin/sh"));
        assert!(unix_contents.contains("exec "));
        assert!(!unix_contents.contains("%*"));

        let windows_dir = tmp.path().join("windows");
        std::fs::create_dir_all(&windows_dir).unwrap();
        let windows_req = request("codex", "native", &exe, &windows_dir, None, false);
        let windows_path = generate(HostPlatform::Windows, &windows_req).unwrap();
        assert_eq!(
            windows_path.extension().and_then(|s| s.to_str()),
            Some("cmd")
        );
        let windows_contents = std::fs::read_to_string(&windows_path).unwrap();
        assert!(windows_contents.contains("%*"));
        assert!(!windows_contents.starts_with("#!/bin/sh"));
    }

    #[test]
    fn generating_a_shim_never_touches_a_shell_startup_file() {
        let code = production_code(include_str!("shim.rs"));
        // Each forbidden marker is the *quoted* filename, as it would appear
        // if production code ever built a path to open or write one — not a
        // bare word, so this cannot false-positive on `request.profile` (a
        // launch profile's *name*, an entirely different thing from a shell
        // startup file) or any other legitimate identifier.
        for forbidden in [
            "\".zshrc\"",
            "\".bashrc\"",
            "\".bash_profile\"",
            "\".profile\"",
            "\"config.fish\"",
            "PowerShell_profile",
            "PATH=",
        ] {
            assert!(
                !code.contains(forbidden),
                "shim.rs names `{forbidden}` in production code: generating a shim must never \
                 read or write a shell startup file"
            );
        }
    }

    #[test]
    fn deleting_a_generated_shim_leaves_nothing_behind() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("shims");
        std::fs::create_dir_all(&dir).unwrap();
        let exe = PathBuf::from("/opt/glasshouse/bin/glasshouse");

        let req = request("claude-code", "fast", &exe, &dir, None, false);
        let path = generate(HostPlatform::Linux, &req).unwrap();
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);

        std::fs::remove_file(&path).unwrap();

        assert_eq!(
            std::fs::read_dir(&dir).unwrap().count(),
            0,
            "deleting the generated shim must leave nothing behind"
        );
    }

    #[test]
    fn an_existing_file_is_not_overwritten_without_force() {
        let tmp = tempfile::tempdir().unwrap();
        let existing = tmp.path().join("claude-code");
        std::fs::write(&existing, "PRE-EXISTING").unwrap();

        let exe = PathBuf::from("/opt/glasshouse/bin/glasshouse");
        let req = request("claude-code", "fast", &exe, tmp.path(), None, false);
        let err = generate(HostPlatform::Linux, &req).unwrap_err();
        assert!(matches!(err, ShimError::AlreadyExists { .. }));
        assert_eq!(
            std::fs::read_to_string(&existing).unwrap(),
            "PRE-EXISTING",
            "a refused write must not touch the existing file"
        );

        let forced = request("claude-code", "fast", &exe, tmp.path(), None, true);
        let path = generate(HostPlatform::Linux, &forced).unwrap();
        assert_eq!(path, existing);
        assert_ne!(std::fs::read_to_string(&path).unwrap(), "PRE-EXISTING");
    }

    /// `--name` becomes the write target, not a script interpolation, so a
    /// hostile value here is a path-traversal hazard: an absolute path, a
    /// `..` component, or a separator can make `request.dir.join(name)` land
    /// outside `request.dir` entirely (finding GH-BREAK-CLI-SURFACE #1). This
    /// table is what `scripts/mutate.sh` kills when the `check_file_name`
    /// call on `request.name` is removed from `generate`.
    #[test]
    fn a_hostile_name_is_refused_and_writes_nothing_anywhere() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("dir");
        std::fs::create_dir_all(&dir).unwrap();
        // A directory outside `dir` an absolute `--name` could point into —
        // never `/tmp` directly, so this test cannot escape into the real
        // filesystem.
        let elsewhere = tmp.path().join("elsewhere");
        std::fs::create_dir_all(&elsewhere).unwrap();
        let absolute_target = elsewhere.join("poc");

        let exe = PathBuf::from("/opt/glasshouse/bin/glasshouse");

        let hostile_names: Vec<String> = vec![
            absolute_target.to_str().unwrap().to_owned(), // absolute path
            "../x".to_owned(),                            // multi-level traversal
            "a/b".to_owned(),                             // Unix separator
            "a\\b".to_owned(),                            // Windows separator
            "C:\\x".to_owned(),                           // Windows drive-qualified
            "..".to_owned(),                              // bare parent component
            String::new(),                                // empty
        ];

        for name in &hostile_names {
            let req = ShimRequest {
                harness: "claude-code",
                profile: "fast",
                glasshouse_exe: &exe,
                dir: &dir,
                name: Some(name.as_str()),
                force: true,
            };
            let err = generate(HostPlatform::Linux, &req).unwrap_err();
            assert!(
                matches!(
                    err,
                    ShimError::UnsafeName { .. } | ShimError::InvalidName { .. }
                ),
                "name {name:?} should be refused, got: {err}"
            );
        }

        assert_eq!(
            std::fs::read_dir(&dir).unwrap().count(),
            0,
            "a refused --name must leave the target directory untouched"
        );
        assert!(
            !absolute_target.exists(),
            "a refused absolute --name must not write outside --dir"
        );
        assert_eq!(
            std::fs::read_dir(&elsewhere).unwrap().count(),
            0,
            "a refused absolute --name must not write outside --dir"
        );
        // `../x`'s would-be target: one level above `dir`, i.e. directly
        // inside `tmp`.
        assert!(
            !tmp.path().join("x").exists(),
            "a refused `../x` --name must not escape --dir"
        );
    }

    /// The control case for the table above: an ordinary `--name` is
    /// unaffected by the new check and still writes exactly one file at
    /// `dir.join(name)`, as it did before this fix.
    #[test]
    fn an_ordinary_name_still_writes_exactly_at_dir_join_name() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("dir");
        std::fs::create_dir_all(&dir).unwrap();
        let exe = PathBuf::from("/opt/glasshouse/bin/glasshouse");

        let req = request("claude-code", "fast", &exe, &dir, Some("my-shim.sh"), false);
        let path = generate(HostPlatform::Linux, &req).unwrap();

        assert_eq!(path, dir.join("my-shim.sh"));
        assert!(path.exists());
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);
    }
}
