use super::*;
use std::io::Write;

/// The regression behind a real `windows-latest` failure: a resolved
/// `.cmd` path is canonical, canonical means verbatim on Windows, and
/// `cmd.exe` answers a verbatim path with "The system cannot find the
/// path specified" and exit 1. Every `.cmd`-shimmed harness — which is
/// how npm installs them — was unlaunchable.
///
/// Host-independent on purpose: `plain_script_path` transforms the path's
/// *spelling*, so this runs everywhere rather than only where the bug
/// reproduces.
#[test]
fn a_verbatim_script_path_is_converted_to_the_form_cmd_exe_accepts() {
    assert_eq!(
        plain_script_path(Path::new(r"\\?\C:\tools\claude.cmd")),
        PathBuf::from(r"C:\tools\claude.cmd")
    );
    assert_eq!(
        plain_script_path(Path::new(r"\\?\UNC\server\share\claude.cmd")),
        PathBuf::from(r"\\server\share\claude.cmd")
    );
}

/// Paths that were never verbatim must survive untouched — including
/// ordinary Unix paths, since this function is not host-gated.
#[test]
fn a_plain_script_path_is_left_alone() {
    for path in [
        r"C:\tools\claude.cmd",
        r"\\server\share\x.cmd",
        "/usr/bin/x",
    ] {
        assert_eq!(plain_script_path(Path::new(path)), PathBuf::from(path));
    }
}

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
