//! Acceptance for map line 2455's platform appliers: a compiled `Profile`
//! becomes an OS sandbox on macOS, Linux and Windows. Each test names the
//! invariant of `docs/product/pane/sandbox-grants.md` it holds.
//!
//! **Nothing model-authored runs here — map line 2457.** Every process this
//! file spawns is `/bin/cat` with a fixed argv over a file this file wrote.
//! There is no generated code, no shell string built from a template, and no
//! input from anywhere but these tests.
//!
//! And no test asserts that a sandbox works by relying on the sandbox: the
//! two execution tests each prove the *same fixed argv* reaches the path
//! without confinement and fails with it. A sandbox that refused everything,
//! including the loader, would fail the unconfined half and be caught.

use pane::sandbox::profile::{Access, Profile};
use pane::sandbox::{linux, macos, windows};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static COUNTER: AtomicU64 = AtomicU64::new(0);

/// The Windows applier's source, for the one invariant that is a property of
/// the prose rather than of any call: the job object is a lifetime primitive
/// and its documentation has to say so first, because the map line's phrase
/// "Windows job objects" reads as though it were the grant mechanism.
const WINDOWS_SOURCE: &str = include_str!("../src/sandbox/windows.rs");

/// A throwaway project directory with a `.claude/` in it, removed when the
/// test finishes.
struct Fixture {
    root: PathBuf,
    outside: PathBuf,
}

impl Fixture {
    fn new(label: &str) -> Self {
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let stem = format!("pane-sandbox-apply-{}-{label}-{n}", std::process::id());
        let root = std::env::temp_dir().join(&stem);
        std::fs::create_dir_all(root.join(".claude")).unwrap();
        let outside = std::env::temp_dir().join(format!("{stem}-outside"));
        std::fs::create_dir_all(&outside).unwrap();
        Self { root, outside }
    }

    /// The profile for this fixture, compiled from `settings`.
    fn profile(&self, settings: Option<&str>) -> Profile {
        Profile::compile(&self.root, settings)
    }

    /// The root as `Profile` resolved it. Every assertion uses this rather
    /// than `self.root`: on macOS `temp_dir()` is `/var/folders/…` and its
    /// realpath is `/private/var/folders/…`, and a test comparing the two
    /// spellings would be testing the wrong thing.
    fn resolved(&self, profile: &Profile) -> PathBuf {
        profile.root().to_path_buf()
    }

    fn write(&self, path: &Path, contents: &str) -> PathBuf {
        std::fs::write(path, contents).unwrap();
        path.to_path_buf()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
        let _ = std::fs::remove_dir_all(&self.outside);
    }
}

/// A settings document granting the project root and one path outside it.
fn settings_for(root: &Path) -> String {
    let root = root.to_string_lossy().replace('\\', "/");
    format!(
        r#"{{"permissions":{{"allow":["Read({root}/**)","Edit({root}/src/**/*.rs)","Bash(cargo test*)"],"deny":["Read({root}/secrets/**)"]}}}}"#
    )
}

// --- macOS: the profile text -------------------------------------------

#[test]
fn the_macos_profile_denies_by_default_and_allows_only_the_grants() {
    let fixture = Fixture::new("default");
    let profile = fixture.profile(Some(&settings_for(&fixture.root)));
    let root = fixture.resolved(&profile);
    let text = macos::profile_text(&profile);

    // §3's shape, in the order a seatbelt profile is read.
    assert!(text.starts_with("(version 1)\n(deny default)\n"), "{text}");
    assert!(
        text.contains(&format!(
            "(allow file-read* (subpath \"{}\"))",
            root.display()
        )),
        "{text}"
    );
    assert!(
        text.contains(&format!(
            "(allow file-write* (subpath \"{}\"))",
            root.display()
        )),
        "{text}"
    );

    // The only paths granted are the project root and the fixed platform
    // machinery. Nothing user-shaped reaches the profile: `$HOME` is the
    // path §4.3 cares about most and it is absent even as a prefix.
    let home = std::env::var("HOME").unwrap();
    let user_paths: Vec<&str> = text
        .lines()
        .filter(|line| line.starts_with("(allow file-"))
        .filter(|line| line.contains(&home) && !line.contains(&root.to_string_lossy().to_string()))
        .collect();
    assert!(user_paths.is_empty(), "granted a home path: {user_paths:?}");

    // §2: a `Bash` pattern grants no file access. `Bash(cargo test*)` is in
    // the document above and must leave no trace here.
    assert!(!text.contains("cargo"), "{text}");
}

#[test]
fn the_macos_profile_denies_network_unconditionally() {
    let fixture = Fixture::new("network");
    // §4.1: no pattern names a host, a port or a protocol, so there is no
    // document that can put a network grant in. All three of these try.
    for settings in [
        None,
        Some(r#"{"permissions":{"allow":["WebFetch(domain:example.com)"]}}"#),
        Some(r#"{"permissions":{"allow":["WebFetch","WebSearch","Bash(curl*)"]}}"#),
    ] {
        let profile = fixture.profile(settings);
        let text = macos::profile_text(&profile);
        assert!(text.contains("(deny network*)"), "{settings:?}: {text}");
        assert!(!profile.grants_network(), "{settings:?}");
        assert!(!text.contains("(allow network"), "{settings:?}: {text}");
    }
}

#[test]
fn the_macos_profile_denies_writing_dot_claude_inside_the_project() {
    let fixture = Fixture::new("dotclaude");
    let profile = fixture.profile(Some(&settings_for(&fixture.root)));
    let root = fixture.resolved(&profile);
    let text = macos::profile_text(&profile);

    // §1.5: `.claude/` is inside the writable root, so a program that could
    // write it could widen the profile it was derived from.
    let deny = format!(
        "(deny file-write* (subpath \"{}/.claude\"))",
        root.display()
    );
    assert!(text.contains(&deny), "{text}");
    // The deny follows the root's write allow, because seatbelt takes the
    // last matching term and the reverse order would grant what this
    // refuses.
    let allow_at = text
        .find(&format!(
            "(allow file-write* (subpath \"{}\"))",
            root.display()
        ))
        .unwrap();
    assert!(text.find(&deny).unwrap() > allow_at, "{text}");
    // Reading it stays granted: `settings.json` is read before the sandbox
    // is entered.
    assert!(!text.contains("(deny file-read* (subpath"), "{text}");
    assert!(
        profile
            .check("read", Access::Read, &root.join(".claude/settings.json"))
            .is_ok()
    );
}

// --- macOS: the sandbox, actually applied ------------------------------

#[test]
fn a_sandboxed_process_cannot_read_outside_the_project_but_an_unsandboxed_one_can() {
    #[cfg(not(target_os = "macos"))]
    {
        eprintln!(
            "skipped: seatbelt is macOS-only; the Linux equivalent is \
             a_landlocked_process_cannot_read_outside_the_project_but_an_unsandboxed_one_can"
        );
    }
    #[cfg(target_os = "macos")]
    {
        use std::process::{Command, Stdio};

        let fixture = Fixture::new("exec");
        let profile = fixture.profile(Some(&settings_for(&fixture.root)));
        let root = fixture.resolved(&profile);
        let inside = fixture.write(&root.join("inside.txt"), "inside-secret\n");
        let outside = fixture.write(&fixture.outside.join("outside.txt"), "outside-secret\n");

        // A fixed argv over a file this test wrote. Nothing generated.
        let cat = |path: &Path, confined: bool| {
            let mut command = Command::new("/bin/cat");
            command
                .arg(path)
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped());
            if confined {
                macos::confine(&profile, &mut command).unwrap();
            }
            command.output().unwrap()
        };

        // The unconfined half. Without it a sandbox that refused the loader
        // itself — or a path that simply did not exist — would pass the
        // assertion below for the wrong reason.
        let free = cat(&outside, false);
        assert!(free.status.success(), "{free:?}");
        assert_eq!(String::from_utf8_lossy(&free.stdout), "outside-secret\n");

        // The same argv, confined.
        let confined = cat(&outside, true);
        assert!(!confined.status.success(), "{confined:?}");
        assert!(
            !String::from_utf8_lossy(&confined.stdout).contains("outside-secret"),
            "{confined:?}"
        );

        // And the sandbox is not simply refusing everything: the same
        // program reads the project through it.
        let granted = cat(&inside, true);
        assert!(granted.status.success(), "{granted:?}");
        assert_eq!(
            String::from_utf8_lossy(&granted.stdout),
            "inside-secret\n",
            "{granted:?}"
        );
    }
}

// --- every platform: the regime is reported ----------------------------

#[test]
fn the_applier_reports_its_enforcement_regime() {
    let fixture = Fixture::new("regime");
    let profile = fixture.profile(Some(&settings_for(&fixture.root)));

    // macOS. The count is the profile's own, and the sentence says the OS
    // layer is coarser than the pattern rather than implying it is not.
    let regime = macos::regime(&profile);
    assert_eq!(
        regime,
        macos::Regime::ProjectRootOnly {
            path_rules: profile.rule_count()
        }
    );
    assert!(profile.rule_count() > 0, "the fixture has path rules");
    assert!(regime.describe().contains("directory-granular"), "{regime}");
    assert!(regime.describe().contains("pre-call check"), "{regime}");

    // Linux. Every regime names what it does and does not enforce; the two
    // without a mount view say the network is still there.
    for coarse in [
        linux::Regime::LandlockOnly { abi: 3 },
        linux::Regime::Unconfined,
    ] {
        assert!(!coarse.removes_network(), "{coarse}");
    }
    for full in [
        linux::Regime::BubblewrapAndLandlock { abi: 4 },
        linux::Regime::BubblewrapOnly,
    ] {
        assert!(full.removes_network(), "{full}");
    }
    assert!(
        linux::Regime::BubblewrapOnly
            .describe()
            .contains("no Landlock"),
        "a coarser regime must say so"
    );
    assert!(
        linux::Regime::BubblewrapAndLandlock { abi: 3 }
            .describe()
            .contains("no glob"),
        "Landlock's missing glob must be stated"
    );

    // Windows. Only the AppContainer removes the network; the restricted
    // token has no bearing on sockets.
    assert!(windows::Regime::RestrictedTokenAndAppContainer.removes_network());
    assert!(!windows::Regime::RestrictedTokenOnly.removes_network());
    assert!(!windows::Regime::Unconfined.removes_network());

    #[cfg(target_os = "linux")]
    {
        // The host's own answer, whatever it is. Asserting a particular
        // regime here would be asserting the CI image's kernel.
        let live = linux::regime();
        assert!(!live.describe().is_empty(), "{live}");
    }
    #[cfg(not(target_os = "linux"))]
    eprintln!("skipped: linux::regime() probes the running kernel's Landlock ABI");
}

// --- every platform: nothing widens ------------------------------------

#[test]
fn no_runtime_input_can_widen_a_grant() {
    // (a) The profile decides, not the applier. A document whose `deny`
    // covers the project root produces no write grant anywhere.
    let fixture = Fixture::new("widen");
    let root_pattern = fixture.root.to_string_lossy().replace('\\', "/");
    let denied = fixture.profile(Some(&format!(
        r#"{{"permissions":{{"allow":["Write({root_pattern}/**)"],"deny":["Write({root_pattern}/**)"]}}}}"#
    )));
    let text = macos::profile_text(&denied);
    assert!(!text.contains("(allow file-write* (subpath"), "{text}");
    assert!(
        linux::landlock_rules(&denied).read_write.is_empty(),
        "{:?}",
        linux::landlock_rules(&denied)
    );
    assert!(
        windows::acl_grants(&denied).read_write.is_empty(),
        "{:?}",
        windows::acl_grants(&denied)
    );
    let argv = linux::bwrap_argv(&denied, "/bin/cat".as_ref(), &[]);
    assert!(
        !argv.iter().any(|arg| arg == "--bind"),
        "a denied root gets no read-write bind: {argv:?}"
    );

    // (b) A project directory name cannot close a profile term and be read
    // as more profile. This is the only place an attacker-shaped string
    // reaches the generated text at all.
    let evil = std::env::temp_dir().join(format!(
        "pane-sbx-{}-a\"b\\c) (allow file-write* (subpath \"/",
        std::process::id()
    ));
    std::fs::create_dir_all(&evil).unwrap();
    let injected = Profile::compile(&evil, None);
    let text = macos::profile_text(&injected);
    let _ = std::fs::remove_dir_all(&evil);
    // Counted per line, not per occurrence: the escaped directory name
    // contains the phrase too, which is exactly the point — it is inside a
    // string term instead of being one.
    assert_eq!(
        text.lines()
            .filter(|line| line.starts_with("(allow file-write* (subpath"))
            .count(),
        1,
        "the directory name opened a second write grant: {text}"
    );
    assert!(text.contains(r#"a\"b\\c"#), "not escaped: {text}");

    // (c) The text is a function of the profile and of nothing else: the
    // same profile renders the same bytes, and the argv a caller is about
    // to spawn never reaches the renderer — there is no parameter for it.
    let stable = fixture.profile(Some(&settings_for(&fixture.root)));
    assert_eq!(macos::profile_text(&stable), macos::profile_text(&stable));

    // (d) §4.1 has no off switch on any platform.
    assert!(macos::profile_text(&stable).contains("(deny network*)"));
    assert!(
        linux::bwrap_argv(&stable, "/bin/cat".as_ref(), &[])
            .iter()
            .any(|arg| arg == "--unshare-all")
    );
    assert!(!windows::acl_grants(&stable).internet_client);
}

// --- Linux -------------------------------------------------------------

#[test]
fn the_bwrap_view_unshares_everything_and_rebinds_the_project_over_a_read_only_root() {
    let fixture = Fixture::new("bwrap");
    let profile = fixture.profile(Some(&settings_for(&fixture.root)));
    let root = fixture.resolved(&profile);
    let argv: Vec<String> = linux::bwrap_argv(&profile, "/bin/cat".as_ref(), &["x".into()])
        .into_iter()
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect();

    assert_eq!(argv[0], "bwrap");
    assert!(argv.contains(&"--unshare-all".to_string()), "{argv:?}");
    assert!(argv.contains(&"--die-with-parent".to_string()), "{argv:?}");

    // Bind order is the policy. `/` read-only, then the project read-write
    // over it, then `.claude` read-only over that; bwrap applies binds in
    // argument order, so any reversal widens the result.
    let at = |flag: &str, path: &str| {
        argv.windows(3)
            .position(|w| w[0] == flag && w[1] == path && w[2] == path)
    };
    let slash = at("--ro-bind", "/").unwrap_or_else(|| panic!("{argv:?}"));
    let project = at("--bind", &root.to_string_lossy()).unwrap_or_else(|| panic!("{argv:?}"));
    let dot_claude = at("--ro-bind", &root.join(".claude").to_string_lossy())
        .unwrap_or_else(|| panic!("{argv:?}"));
    assert!(slash < project, "{argv:?}");
    assert!(project < dot_claude, "{argv:?}");

    // The program and its arguments come last, after `--`, so no path in
    // them can be read as a bwrap flag.
    assert_eq!(&argv[argv.len() - 3..], &["--", "/bin/cat", "x"]);
}

#[test]
fn the_landlock_ruleset_grants_the_project_and_nothing_else() {
    let fixture = Fixture::new("landlock");
    let profile = fixture.profile(Some(&settings_for(&fixture.root)));
    let root = fixture.resolved(&profile);
    let rules = linux::landlock_rules(&profile);

    assert_eq!(rules.read_write, vec![root.clone()], "{rules:?}");
    // No `.claude` rule: Landlock's rules are additive, so a read-only rule
    // beneath a read-write one removes nothing. The mount view carries §1.5
    // on this platform, and
    // `landlock_alone_does_not_enforce_the_dot_claude_carve_out_and_the_mount_view_does`
    // is what measured that.
    assert!(
        !rules.read_only.contains(&root.join(".claude")),
        "{rules:?}"
    );
    // Every other read grant is a system root. `$HOME` is not among them,
    // and §4.3's directories live under it.
    let home = PathBuf::from(std::env::var("HOME").unwrap());
    for path in &rules.read_only {
        assert!(
            path.starts_with(&root) || !path.starts_with(&home),
            "granted a home path: {path:?}"
        );
    }

    // A read grant is open, list and run — never a write and never a
    // `MAKE_*`. That is what makes the `.claude` carve-out a carve-out.
    assert_eq!(
        linux::access::READ,
        linux::access::EXECUTE | linux::access::READ_FILE | linux::access::READ_DIR
    );
    assert_eq!(linux::access::READ & linux::access::WRITE_FILE, 0);
    assert_eq!(linux::access::READ & linux::access::MAKE_REG, 0);
    // ABI 3's `TRUNCATE` is in the write grant: without it a write grant
    // has a hole in it.
    assert_ne!(linux::access::READ_WRITE & linux::access::TRUNCATE, 0);
}

#[test]
fn a_landlocked_process_cannot_read_outside_the_project_but_an_unsandboxed_one_can() {
    #[cfg(not(target_os = "linux"))]
    eprintln!("skipped: Landlock is a Linux kernel interface; this host is not Linux");
    #[cfg(target_os = "linux")]
    {
        use std::process::{Command, Stdio};

        if linux::landlock_abi() < 3 {
            eprintln!(
                "skipped: this kernel reports Landlock ABI {} and the specification asks for 3",
                linux::landlock_abi()
            );
            return;
        }
        let fixture = Fixture::new("landlock-exec");
        let profile = fixture.profile(Some(&settings_for(&fixture.root)));
        let root = fixture.resolved(&profile);
        let inside = fixture.write(&root.join("inside.txt"), "inside-secret\n");
        let outside = fixture.write(&fixture.outside.join("outside.txt"), "outside-secret\n");

        let cat = |path: &Path, confined: bool| {
            let mut command = Command::new("/bin/cat");
            command
                .arg(path)
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped());
            if confined {
                assert!(linux::confine(&profile, &mut command).unwrap());
            }
            command.output().unwrap()
        };

        let free = cat(&outside, false);
        assert!(free.status.success(), "{free:?}");
        assert_eq!(String::from_utf8_lossy(&free.stdout), "outside-secret\n");

        let confined = cat(&outside, true);
        assert!(!confined.status.success(), "{confined:?}");
        assert!(
            !String::from_utf8_lossy(&confined.stdout).contains("outside-secret"),
            "{confined:?}"
        );

        let granted = cat(&inside, true);
        assert!(granted.status.success(), "{granted:?}");
        assert_eq!(String::from_utf8_lossy(&granted.stdout), "inside-secret\n");
    }
}

// --- Windows -----------------------------------------------------------

#[test]
fn the_windows_acl_admits_the_capability_sid_to_the_project_and_nothing_else() {
    let fixture = Fixture::new("acl");
    let profile = fixture.profile(Some(&settings_for(&fixture.root)));
    let root = fixture.resolved(&profile);
    let grants = windows::acl_grants(&profile);

    assert_eq!(grants.read_write, vec![root.clone()], "{grants:?}");
    assert_eq!(grants.read_only, vec![root.join(".claude")], "{grants:?}");
    // §4.1: an AppContainer without `internetClient` has no network, and no
    // document can add the capability because no pattern names one.
    assert!(!grants.internet_client);

    // The container name is derived from the root alone, is stable across
    // calls, and fits `CreateAppContainerProfile`'s 64 UTF-16 limit however
    // long the project path is.
    let name = windows::container_name(&profile);
    assert_eq!(name, windows::container_name(&profile));
    assert!(name.len() <= 64, "{name}");
    assert!(name.starts_with("Glasshouse.Pane."), "{name}");
    let other = Profile::compile(root.join("elsewhere"), None);
    assert_ne!(name, windows::container_name(&other));

    #[cfg(not(target_os = "windows"))]
    eprintln!(
        "skipped: the restricted token, the AppContainer and the project ACL are Win32 calls; \
         no Windows cell exists for the pane job, so they are compile-verified only"
    );
}

#[test]
fn the_windows_job_object_is_documented_as_a_lifetime_primitive_and_not_a_sandbox() {
    // Requirement 4, and it is about the prose because the defect it guards
    // is a reader's: the map line says "Windows job objects" in a list of
    // sandboxes, and it is not one.
    let first = WINDOWS_SOURCE
        .lines()
        .find(|line| line.contains("job object"))
        .expect("the module documents the job object");
    assert!(
        first.contains("not a sandbox") && first.contains("grants nothing"),
        "the first sentence mentioning the job object must say it is not a sandbox: {first}"
    );
    assert!(
        WINDOWS_SOURCE.contains("lifetime"),
        "the job object's actual purpose must be named"
    );
}

#[test]
fn landlock_alone_does_not_enforce_the_dot_claude_carve_out_and_the_mount_view_does() {
    #[cfg(not(target_os = "linux"))]
    eprintln!("skipped: Landlock is a Linux kernel interface; this host is not Linux");
    #[cfg(target_os = "linux")]
    {
        use std::process::{Command, Stdio};

        if linux::landlock_abi() < 3 {
            eprintln!(
                "skipped: this kernel reports Landlock ABI {} and the specification asks for 3",
                linux::landlock_abi()
            );
            return;
        }
        // The measurement behind `landlock_rules`' central caveat, and the
        // reason this test asserts a limitation rather than a protection: a
        // Landlock ruleset's rules are ADDITIVE, so a read-only rule beneath
        // a read-write one removes nothing and `.claude/` stays writable
        // under Landlock alone. §1.5's OS-level enforcement on Linux is
        // therefore the mount view's read-only bind, and `Profile::check`
        // refuses the write in every regime.
        eprintln!("measured on Landlock ABI {}", linux::landlock_abi());
        let fixture = Fixture::new("landlock-claude");
        let profile = fixture.profile(Some(&settings_for(&fixture.root)));
        let root = fixture.resolved(&profile);
        let source = fixture.write(&root.join("inside.txt"), "inside-secret\n");
        let cp = ["/bin/cp", "/usr/bin/cp"]
            .into_iter()
            .find(|path| Path::new(path).exists())
            .expect("cp");

        let copy = |target: &Path| {
            let mut command = Command::new(cp);
            command
                .arg(&source)
                .arg(target)
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped());
            assert!(linux::confine(&profile, &mut command).unwrap());
            command.output().unwrap()
        };

        // The ruleset is applied and doing its job inside the project.
        let allowed = copy(&root.join("allowed.txt"));
        assert!(allowed.status.success(), "{allowed:?}");
        assert!(root.join("allowed.txt").exists(), "{allowed:?}");

        // And it does not carve `.claude` back out. If a future kernel makes
        // rules most-specific-wins, this assertion fails and the caveat in
        // `landlock_rules` is what needs rewriting.
        let claude = copy(&root.join(".claude/written.txt"));
        assert!(
            claude.status.success() && root.join(".claude/written.txt").exists(),
            "Landlock now enforces the carve-out; `landlock_rules` says it cannot: {claude:?}"
        );

        // The two layers that do refuse it. The mount view, by bind order:
        let argv: Vec<String> = linux::bwrap_argv(&profile, cp.as_ref(), &[])
            .into_iter()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();
        let claude_bind = root.join(".claude").to_string_lossy().into_owned();
        assert!(
            argv.windows(3)
                .any(|w| w[0] == "--ro-bind" && w[1] == claude_bind && w[2] == claude_bind),
            "{argv:?}"
        );
        // And pane's own pre-call check, in every regime:
        assert!(
            profile
                .check("write", Access::Write, &root.join(".claude/written.txt"))
                .is_err()
        );
    }
}
