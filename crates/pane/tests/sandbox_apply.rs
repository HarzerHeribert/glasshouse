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

/// One filter inside a seatbelt term: `(subpath "/usr")` is
/// `("subpath", "/usr")`, `(global-name "com.apple.x")` is
/// `("global-name", "com.apple.x")`. Unquoted filters such as
/// `(target self)` carry no path and are not collected.
#[derive(Debug, PartialEq, Eq)]
struct Filter {
    term: String,
    form: String,
    value: String,
}

/// Every `(allow …)` / `(deny …)` line of a profile, split into the term it
/// names and the quoted filters it carries.
///
/// This is what makes the allow-set assertable *positively*. A substring
/// test for `$HOME` — which is what this file used to do — passes for
/// `(subpath "/Users")`, passes for `(literal "/")`, and never looks at a
/// non-path term such as `mach-lookup` at all, so a blanket grant of every
/// Mach service on the machine sat inside a green test file. Parsing the
/// terms means a new root, a new operation, or a new service has to be
/// declared below or the test fails.
fn parse(text: &str) -> (Vec<String>, Vec<Filter>) {
    let mut names = Vec::new();
    let mut filters = Vec::new();
    for line in text.lines() {
        let Some(rest) = line
            .strip_prefix("(allow ")
            .or_else(|| line.strip_prefix("(deny "))
        else {
            assert!(
                line.starts_with("(version ") || line.is_empty(),
                "a profile line that is neither a version nor an allow/deny term: {line}"
            );
            continue;
        };
        let term: String = rest
            .chars()
            .take_while(|c| !c.is_whitespace() && *c != ')')
            .collect();
        names.push(term.clone());

        // `(<form> "<value>")`, with the escaping `quote()` applies.
        let bytes: Vec<char> = rest.chars().collect();
        let mut i = 0;
        while i < bytes.len() {
            if bytes[i] != '(' {
                i += 1;
                continue;
            }
            let form: String = bytes[i + 1..]
                .iter()
                .take_while(|c| !c.is_whitespace() && **c != ')')
                .collect();
            let mut j = i + 1 + form.chars().count();
            while j < bytes.len() && bytes[j] != '"' && bytes[j] != ')' {
                j += 1;
            }
            if j >= bytes.len() || bytes[j] == ')' {
                i += 1;
                continue;
            }
            let mut value = String::new();
            j += 1;
            while j < bytes.len() && bytes[j] != '"' {
                if bytes[j] == '\\' {
                    j += 1;
                }
                value.push(bytes[j]);
                j += 1;
            }
            filters.push(Filter {
                term: term.clone(),
                form,
                value,
            });
            i = j + 1;
        }
    }
    (names, filters)
}

/// Every term the seatbelt profile is permitted to name. A term absent from
/// here is a test failure whether or not it carries a path, which is the
/// half the old substring filter had no way to see.
const EXPECTED_TERMS: &[&str] = &[
    "default",
    "file-read-metadata",
    "file-read*",
    "process-exec*",
    "process-fork",
    "signal",
    "sysctl-read",
    "file-write-data",
    "file-ioctl",
    "file-write*",
    "network*",
];

/// Every path the profile is permitted to name, other than the project root
/// and its `.claude`. Spelled out here rather than read from the applier's
/// own constants, so adding a root there fails this test instead of
/// travelling with it.
const EXPECTED_PATHS: &[&str] = &[
    "/",
    "/usr/bin",
    "/bin",
    "/usr/sbin",
    "/sbin",
    "/usr/local/bin",
    "/opt/homebrew/bin",
    "/usr",
    "/etc",
    "/private/etc",
    "/System",
    "/Library",
    "/opt/homebrew",
    "/private/var/db/dyld",
    "/private/var/db/timezone",
    "/dev/null",
    "/dev/zero",
    "/dev/random",
    "/dev/urandom",
    "/dev/tty",
    "/dev/stdin",
    "/dev/stdout",
    "/dev/stderr",
    "/dev/dtracehelper",
];

fn sorted(mut values: Vec<String>) -> Vec<String> {
    values.sort();
    values.dedup();
    values
}

#[test]
fn the_allow_set_is_exactly_the_declared_terms() {
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

    let (names, filters) = parse(&text);
    assert!(!filters.is_empty(), "the parser found no filters: {text}");

    // Positively, term by term: the set of operations this profile speaks
    // about is exactly the declared one. A new operation — `mach-lookup`,
    // `ipc-posix-shm`, `file-read-xattr` — fails here by construction.
    assert_eq!(
        sorted(names),
        sorted(EXPECTED_TERMS.iter().map(|t| t.to_string()).collect()),
        "{text}"
    );

    // Positively, path by path: the set of paths is exactly the declared
    // system machinery plus the project root and its `.claude`.
    let mut expected: Vec<String> = EXPECTED_PATHS.iter().map(|p| p.to_string()).collect();
    expected.push(root.to_string_lossy().into_owned());
    expected.push(root.join(".claude").to_string_lossy().into_owned());
    assert_eq!(
        sorted(filters.iter().map(|f| f.value.clone()).collect()),
        sorted(expected),
        "{text}"
    );

    // §4.3, and it holds however the two lists above are edited: no subtree
    // grant may be an ancestor of `$HOME`. `(subpath "/Users")` is what the
    // old `$HOME`-substring filter let through; `(literal "/")` is a single
    // directory entry and not a subtree, which is why the form matters.
    let home = PathBuf::from(std::env::var("HOME").unwrap());
    for filter in &filters {
        if filter.form == "subpath" {
            let granted = PathBuf::from(&filter.value);
            assert!(
                !home.starts_with(&granted) || granted.starts_with(&root),
                "a subtree grant contains $HOME: {filter:?}"
            );
        }
    }

    // §2: a `Bash` pattern grants no file access. `Bash(cargo test*)` is in
    // the document above and must leave no trace here.
    assert!(!text.contains("cargo"), "{text}");
}

#[test]
fn the_seatbelt_profile_names_every_mach_service_it_permits() {
    // §4.2: the Keychain is never grantable on any platform, and an
    // unfiltered `(allow mach-lookup)` grants it — `securityd` does the
    // keychain read on the caller's behalf, so the file rules never see it.
    // The measured base set is empty: no tool in scope needed a service.
    const EXPECTED_MACH_SERVICES: &[&str] = &[];

    let fixture = Fixture::new("mach");
    let profile = fixture.profile(Some(&settings_for(&fixture.root)));
    let text = macos::profile_text(&profile);

    // No blanket term, in any spelling: every `mach-lookup` line must carry
    // at least one `global-name` filter.
    for line in text.lines() {
        if line.contains("mach-lookup") {
            assert!(
                line.contains("(global-name \""),
                "a mach-lookup term with no global-name filter: {line}"
            );
        }
    }

    // And the services it does name are exactly the declared ones.
    let (_, filters) = parse(&text);
    let named: Vec<String> = filters
        .iter()
        .filter(|f| f.term == "mach-lookup")
        .map(|f| f.value.clone())
        .collect();
    assert_eq!(
        named,
        EXPECTED_MACH_SERVICES
            .iter()
            .map(|s| s.to_string())
            .collect::<Vec<_>>(),
        "{text}"
    );
    // Whatever that list grows to hold, §4.2 excludes the credential store.
    for service in &named {
        let lowered = service.to_lowercase();
        assert!(
            !lowered.contains("securityd") && !lowered.contains("securityserver"),
            "a keychain endpoint is never grantable: {service}"
        );
    }
}

#[test]
fn a_confined_process_cannot_reach_the_keychain() {
    #[cfg(not(target_os = "macos"))]
    eprintln!(
        "skipped: securityd and seatbelt are macOS; §4.2's other platforms are not this test"
    );
    #[cfg(target_os = "macos")]
    {
        use std::process::{Command, Stdio};

        // §4.2, demonstrated rather than read off the profile text. The
        // query names an item that does not exist, so no keychain ACL
        // prompt can fire and no stored secret is touched: what is being
        // watched is whether securityd answers *at all*.
        const ABSENT: &str = "__pane-sandbox-apply-nonexistent-item__";
        /// The answer only securityd can give: the search ran and the item
        /// is not there.
        const AUTHORITATIVE: &str = "could not be found in the keychain";
        /// The client-side failure of a process that has no Mach service to
        /// ask. `security` prints this *and then* prints its own
        /// item-not-found line, which is why the presence of the
        /// authoritative sentence alone proves nothing.
        const CLIENT_SIDE: &str = "were not valid";

        /// Whether securityd itself answered: the search was created and
        /// the item was not found, with no client-side failure alongside.
        fn asked_securityd(stderr: &str) -> bool {
            stderr.contains(AUTHORITATIVE) && !stderr.contains(CLIENT_SIDE)
        }

        let fixture = Fixture::new("keychain");
        let profile = fixture.profile(Some(&settings_for(&fixture.root)));

        let security = |args: &[&str], confined: bool| {
            let mut command = Command::new("/usr/bin/security");
            command
                .args(args)
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped());
            if confined {
                macos::confine(&profile, &mut command).unwrap();
            }
            let out = command.output().unwrap();
            (
                out.status.success(),
                String::from_utf8_lossy(&out.stdout).into_owned(),
                String::from_utf8_lossy(&out.stderr).into_owned(),
            )
        };

        // The unconfined halves, and they are controls rather than
        // assertions about the sandbox: they establish that securityd is
        // reachable and answering on this host at all. Where it is not — a
        // machine with no keychain in the session, or a macOS whose error
        // text has moved — the experiment cannot run, and the test says so
        // instead of passing quietly.
        let (listed, keychains, _) = security(&["list-keychains"], false);
        let (_, _, absent) = security(&["find-generic-password", "-s", ABSENT], false);
        if !listed || !keychains.contains("keychain") || !asked_securityd(&absent) {
            eprintln!(
                "skipped: securityd is not answering unconfined on this host, so the \
                 confined half would prove nothing: {keychains:?} {absent:?}"
            );
            return;
        }

        // The same two calls, confined. The keychain search list cannot be
        // read at all, and the search for the absent item never gets an
        // authoritative answer: it fails in the client, because there is no
        // Mach service to ask.
        let (listed, keychains, _) = security(&["list-keychains"], true);
        assert!(
            !listed,
            "the confined process listed the keychains: {keychains}"
        );
        assert!(
            !keychains.contains("keychain-db"),
            "the confined process read the keychain search list: {keychains}"
        );
        let (_, _, confined_absent) = security(&["find-generic-password", "-s", ABSENT], true);
        assert!(
            !asked_securityd(&confined_absent),
            "securityd answered a confined process authoritatively: {confined_absent}"
        );
        assert!(
            confined_absent.contains(CLIENT_SIDE),
            "the confined query failed for some reason other than the missing Mach service: \
             {confined_absent}"
        );
    }
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
fn the_reported_regime_matches_what_was_applied() {
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
    // §3: the coarseness is stated, not left to be discovered. Metadata is
    // readable filesystem-wide and `readlink(2)` is a metadata operation, so
    // a symlink's target is disclosed anywhere — and no Mach service is
    // reachable, which is §4.2's half of the same sentence.
    assert!(regime.describe().contains("metadata"), "{regime}");
    assert!(regime.describe().contains("symlink"), "{regime}");
    assert!(regime.describe().contains("no Mach service"), "{regime}");

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

        // What was APPLIED, not what is installed. Nothing in this package
        // spawns `bwrap` — `bwrap_argv` builds a value and the spawn path
        // that would run it is 61E's — so a mount-view regime here would
        // report an enforcement nobody installed, and `removes_network`
        // would answer `true` for a network nothing removed. That is §4.1
        // claimed as enforced while it is not, which is the specific failure
        // §3 exists to prevent.
        assert!(
            matches!(
                live,
                linux::Regime::LandlockOnly { .. } | linux::Regime::Unconfined
            ),
            "regime() reported a regime this package cannot apply: {live:?}"
        );
        assert!(
            !live.removes_network(),
            "nothing here removes the network: {live}"
        );
        let abi = linux::landlock_abi();
        assert_eq!(
            live,
            if abi >= 3 {
                linux::Regime::LandlockOnly { abi }
            } else {
                linux::Regime::Unconfined
            },
            "{live:?} at ABI {abi}"
        );
        // The host-capability question still has an answer; it has a
        // different name now, and it is allowed to be the wider one.
        assert!(!linux::available_regime().describe().is_empty());
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

/// Every path a Landlock ruleset is permitted to grant read on, in order,
/// declared here rather than read from the applier's own constant — so a
/// root added there fails this test instead of travelling with it.
///
/// `/proc`, `/sys` and `/dev` are deliberately absent. A rule beneath
/// `/proc` grants `READ_FILE` on `/proc/<pid>/environ` for every process of
/// the same user, and the regime this package can actually apply is Landlock
/// alone in the host's own PID namespace — so that is the harness's whole
/// environment, §4.2's credentials reached by a route no `permissions`
/// pattern names and no `deny` entry narrows.
const EXPECTED_LANDLOCK_READ_ONLY: &[&str] = &[
    "/usr",
    "/bin",
    "/sbin",
    "/lib",
    "/lib64",
    "/etc",
    "/opt",
    "/dev/null",
    "/dev/zero",
    "/dev/random",
    "/dev/urandom",
    "/dev/tty",
];

#[test]
fn the_landlock_ruleset_is_exactly_the_declared_paths() {
    let fixture = Fixture::new("landlock");
    let profile = fixture.profile(Some(&settings_for(&fixture.root)));
    let root = fixture.resolved(&profile);
    let rules = linux::landlock_rules(&profile);

    // Positively, path by path and in order: a new system root is a failure
    // by construction rather than by whether it happens to spell `$HOME`.
    assert_eq!(
        rules.read_only,
        EXPECTED_LANDLOCK_READ_ONLY
            .iter()
            .map(PathBuf::from)
            .collect::<Vec<_>>(),
        "{rules:?}"
    );
    assert_eq!(rules.read_write, vec![root.clone()], "{rules:?}");

    // The three that were here and are not. Named individually because the
    // equality above would also pass if all three were added and the
    // expectation edited to match — this is the clause that has to be read
    // and argued with instead.
    for tree in ["/proc", "/sys", "/dev"] {
        assert!(
            !rules.read_only.contains(&PathBuf::from(tree)),
            "{tree} is granted as a tree: {rules:?}"
        );
    }

    // No `.claude` rule: Landlock's rules are additive, so a read-only rule
    // beneath a read-write one removes nothing. The mount view carries §1.5
    // on this platform, and
    // `landlock_alone_does_not_enforce_the_dot_claude_carve_out_and_the_mount_view_does`
    // is what measured that.
    assert!(
        !rules.read_only.contains(&root.join(".claude")),
        "{rules:?}"
    );
    // §4.3: no granted subtree may contain `$HOME`. The old form of this
    // assertion asked whether a path *started with* `$HOME`, which is false
    // for every ancestor of it — `/home` and `/` would both have passed.
    let home = PathBuf::from(std::env::var("HOME").unwrap());
    for path in rules.read_only.iter().chain(rules.read_write.iter()) {
        assert!(
            !home.starts_with(path) || path.starts_with(&root),
            "a granted subtree contains $HOME: {path:?}"
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

    // The device grants are rules on files, and a rule on a file may carry
    // no directory-only right — the kernel refuses the entire ruleset if it
    // does, which fails every confined spawn. `confine` masks with this.
    assert_eq!(linux::access::FILE & linux::access::READ_DIR, 0);
    assert_eq!(
        linux::access::READ & linux::access::FILE,
        linux::access::EXECUTE | linux::access::READ_FILE
    );
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
fn the_project_acl_grant_is_documented_as_unverified_and_stays_uncalled() {
    // The one function in this crate that modifies a user's filesystem, on
    // the one platform no host here can execute. A previous revision built
    // the ACL from its own entry alone and wrote it protected, which takes
    // a developer's project directory away from them on the first call; the
    // repair is reasoning, so what guards it is that nothing calls it and
    // that its documentation says why.
    let lines: Vec<&str> = WINDOWS_SOURCE.lines().collect();
    let at = lines
        .iter()
        .position(|line| line.contains("pub fn grant_project_acl"))
        .expect("the function is gone; so is this test's subject");
    let preamble: Vec<&str> = lines[..at]
        .iter()
        .rev()
        .take_while(|line| line.trim_start().starts_with("///"))
        .copied()
        .collect();
    let first = preamble.last().expect("the function carries a doc comment");
    assert!(
        first.contains("Unverified") && first.contains("unwired"),
        "the first sentence must say it is unverified and unwired: {first}"
    );
    assert!(
        preamble.iter().any(|line| line.contains("Windows cell")),
        "the doc comment must name what would change that: {preamble:?}"
    );

    // And it is uncalled: the `pub use` re-export and the definition are the
    // only mentions, and no call expression exists anywhere in the crate.
    let called: Vec<&str> = WINDOWS_SOURCE
        .lines()
        .filter(|line| line.contains("grant_project_acl("))
        .filter(|line| !line.contains("pub fn "))
        .collect();
    assert!(
        called.is_empty(),
        "grant_project_acl has a caller: {called:?}"
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
