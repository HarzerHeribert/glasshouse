//! Map line 1973's scrub, at the two shell-started launch sites
//! `shell::mod::start_session` and `shell::mod::resume_session` build their
//! own `HarnessLaunch` for — the gap `phase-56.md`'s 1973 entry recorded as
//! the reason this package exists.
//!
//! Neither function is reachable from an integration test: both are private
//! to the shell module, and driving the interactive TUI loop headlessly is
//! not available here. So this exercises the exact seam both now call before
//! `HarnessLaunch::spawn` — `EffectiveConfig::entitlement_for`,
//! `EffectiveConfig::foreign_entitlement_credential_vars`, and
//! `HarnessLaunch::env_remove` — applied to a real `HarnessLaunch` and
//! observed through the child's own environment dump, the real leak path
//! (practice §35), rather than through the shell's private functions.
//!
//! Fixture credentials are obviously fake strings; the only artifact a test
//! here can read — the child's environment dump — is asserted not to contain
//! the one that must not be there.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use glasshouse::config::{EffectiveConfig, UserConfig};
use glasshouse::integrations::IntegrationId;
use glasshouse::launch::HarnessLaunch;
use glasshouse::platform::exec::{self, ResolvedExecutable};
use glasshouse::profile::BackendResource;
use glasshouse::project::Project;
use glasshouse::session::{SessionId, SessionPresentation, SessionRuntime};

/// Two accounts of one vendor, each backed by its own provider name and
/// carrying its own environment-shaped credential — the same shape
/// `entitlement_pool.rs`'s own fixtures use.
fn two_accounts(var_a: &str, var_b: &str) -> UserConfig {
    toml::from_str(&format!(
        "version = 1\n\n\
         [entitlements.claude-a]\nvendor = \"claude\"\nprovider = \"alpha-probe\"\n\
         credential = {{ env = \"{var_a}\" }}\n\n\
         [entitlements.claude-b]\nvendor = \"claude\"\nprovider = \"beta-probe\"\n\
         credential = {{ env = \"{var_b}\" }}\n"
    ))
    .expect("two provider-backed accounts parse")
}

/// The env var the shared fixture script reads its environment-dump
/// destination from, set per spawn by [`Fixture::scrubbed_launch`] rather
/// than baked into the script bytes — see [`shared_fixture`]'s doc for why.
const ENV_LOG_VAR: &str = "GLASSHOUSE_TEST_ENV_LOG";

struct Fixture {
    _tmp: tempfile::TempDir,
    project: Project,
    executable: ResolvedExecutable,
    env_log: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path().join("workspace");
        std::fs::create_dir_all(root.join(".git")).expect("create project root");
        let root = std::fs::canonicalize(&root).expect("canonicalize project root");
        let project = Project::discover(&root, None, false).expect("discover project");

        let bin_dir = tmp.path().join("bin");
        std::fs::create_dir_all(&bin_dir).expect("create bin dir");
        let env_log = tmp.path().join("env.log");
        let harness_path = install_fake_harness(&bin_dir);
        let executable = exec::resolve_explicit(&harness_path).expect("resolve fake harness");

        Self {
            _tmp: tmp,
            project,
            executable,
            env_log,
        }
    }

    /// The scrubbed launch a shell launch site now builds: the fake harness,
    /// with `vars` removed exactly as `foreign_entitlement_credential_vars`
    /// names them, before `HarnessLaunch::spawn`.
    fn scrubbed_launch(&self, vars: Vec<String>) -> HarnessLaunch<'_> {
        let mut launch = HarnessLaunch::new(self.executable.clone(), &self.project)
            .env(ENV_LOG_VAR, self.env_log.clone());
        for var in vars {
            launch = launch.env_remove(var);
        }
        launch
    }

    /// Spawn `launch` through `SessionRuntime::start` — the same call both
    /// shell launch sites make on `live` — and wait for the fake harness to
    /// exit and dump its own environment.
    fn run(&self, launch: &HarnessLaunch<'_>) -> String {
        let mut live = SessionRuntime::new();
        let id = SessionId::new("test-entitlement-shell-scrub");
        live.start(id.clone(), SessionPresentation::Headless, launch)
            .expect("the fake harness must start");

        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            if live
                .poll_exits()
                .into_iter()
                .any(|(exited, _)| exited == id)
            {
                break;
            }
            assert!(Instant::now() < deadline, "the fake harness never exited");
            std::thread::sleep(Duration::from_millis(25));
        }

        std::fs::read_to_string(&self.env_log).expect("the fake harness dumped its environment")
    }
}

/// Write the shared fixture executable once per test binary instead of once
/// per test, so macOS Gatekeeper (`syspolicyd`/XProtect) validates it once
/// per run instead of once per test — see the project memory
/// `gatekeeper-scans-make-pty-fixtures-flaky` and GH-FIXTURE-REUSE /
/// GH-ARGV-LOG-HOIST. The env-dump destination used to be interpolated into
/// the script bytes, which made every call's content distinct; it is now
/// read from `ENV_LOG_VAR` at spawn time (set by
/// [`Fixture::scrubbed_launch`] with [`HarnessLaunch::env`], never
/// process-globally), so the script bytes are constant and every call below
/// collapses onto the one file the first caller writes.
///
/// Sharing is keyed by content, never by the caller's requested name, so a
/// name never causes two distinct fixtures to collide, and a repeated name
/// with the same bytes never causes a second write. Race-free the way
/// `provider/cache.rs::write_json_atomically` is: one process-wide mutex
/// serialises the check-and-write, and the write itself lands in a
/// same-directory temporary name before an atomic rename.
fn shared_fixture(unique_name: &str, contents: &str) -> PathBuf {
    use std::collections::HashMap;
    use std::hash::{Hash, Hasher};
    use std::sync::{Mutex, OnceLock};

    static DIR: OnceLock<tempfile::TempDir> = OnceLock::new();
    static CACHE: OnceLock<Mutex<HashMap<String, PathBuf>>> = OnceLock::new();

    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    let mut guard = cache.lock().expect("shared fixture cache poisoned");
    if let Some(path) = guard.get(contents) {
        return path.clone();
    }

    let dir = DIR.get_or_init(|| tempfile::tempdir().expect("shared fixture dir"));
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    contents.hash(&mut hasher);
    let digest = format!("{:016x}", hasher.finish());
    let named = Path::new(unique_name);
    let stem = named
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or(unique_name);
    let filename = match named.extension().and_then(|e| e.to_str()) {
        Some(ext) => format!("{stem}-{digest}.{ext}"),
        None => format!("{stem}-{digest}"),
    };
    let path = dir.path().join(&filename);
    let temporary = dir.path().join(format!("{filename}.writing"));
    std::fs::write(&temporary, contents).expect("write shared fixture");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(&temporary).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&temporary, perms).unwrap();
    }
    std::fs::rename(&temporary, &path).expect("rename shared fixture into place");
    guard.insert(contents.to_string(), path.clone());
    path
}

#[cfg(unix)]
fn install_fake_harness(_bin_dir: &Path) -> PathBuf {
    // `export -p` is a shell builtin, so the scrubbed environment this
    // fixture spawns under cannot break it.
    shared_fixture(
        "fake-shell-harness",
        &format!("#!/bin/sh\nexport -p > \"${ENV_LOG_VAR}\"\nexit 0\n"),
    )
}

#[cfg(windows)]
fn install_fake_harness(_bin_dir: &Path) -> PathBuf {
    shared_fixture(
        "fake-shell-harness.cmd",
        &format!("@echo off\r\nset > \"%{ENV_LOG_VAR}%\"\r\nexit /b 0\r\n"),
    )
}

#[cfg(test)]
mod shared_fixture_proof {
    use super::{ENV_LOG_VAR, Fixture, install_fake_harness};

    /// **The once-per-binary proof, through the real caller.** Every test in
    /// this file that spawns the harness goes through [`Fixture::new`],
    /// which unconditionally calls `install_fake_harness` — so two
    /// independent per-test tempdirs asking for it, the ordinary shape this
    /// binary runs under, must collapse to one file rather than each writing
    /// its own.
    #[test]
    fn two_tempdirs_installing_the_fake_harness_get_one_shared_file() {
        let tmp_a = tempfile::tempdir().expect("tempdir a");
        let tmp_b = tempfile::tempdir().expect("tempdir b");
        let a = install_fake_harness(tmp_a.path());
        let meta_before = std::fs::metadata(&a).expect("fixture exists after first install");

        let b = install_fake_harness(tmp_b.path());
        assert_eq!(
            a, b,
            "two different tempdirs installing the fixture must share one file"
        );
        assert!(
            !a.starts_with(tmp_a.path()) && !a.starts_with(tmp_b.path()),
            "the shared file must live in the per-binary fixture dir, not either \
             test's own tempdir: {a:?}"
        );

        let meta_after = std::fs::metadata(&b).expect("fixture exists after second install");
        assert_eq!(
            meta_before.modified().unwrap(),
            meta_after.modified().unwrap(),
            "a second install of the same fixture must not rewrite the file"
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            assert_eq!(
                meta_before.ino(),
                meta_after.ino(),
                "a second install of the same fixture must return the same inode, \
                 not a second copy"
            );
        }
    }

    /// **Bytes constant.** The shared fixture's bytes read the env-dump
    /// destination from `ENV_LOG_VAR` rather than embedding a per-test path,
    /// so the script text is the same regardless of which tempdir asked for
    /// it.
    #[cfg(unix)]
    #[test]
    fn the_shared_fixture_reads_its_log_path_from_the_env_var_not_the_script() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = install_fake_harness(tmp.path());
        let content = std::fs::read_to_string(&path).expect("read shared fixture");
        assert_eq!(
            content,
            format!("#!/bin/sh\nexport -p > \"${ENV_LOG_VAR}\"\nexit 0\n"),
            "the shared fixture's bytes must read the log destination from the env var, \
             not have a path baked in"
        );
    }

    /// **End-to-end, through the real caller.** The env var the fixture
    /// reads is exactly the one [`Fixture::scrubbed_launch`] sets per spawn
    /// via [`super::HarnessLaunch::env`] — proven by actually running the
    /// launch and reading the child's environment dump back, not by
    /// inspecting the script text alone.
    #[test]
    fn a_real_launch_through_the_shared_fixture_dumps_its_env_to_the_requested_log() {
        let fixture = Fixture::new();
        let launch = fixture.scrubbed_launch(Vec::new());
        let child_env = fixture.run(&launch);
        assert!(
            !child_env.is_empty(),
            "the shared, env-driven fixture must still dump this fixture's own \
             child environment into its own env log"
        );
    }
}

/// **Under entitlement A, the child sees A's variable and never B's.** The
/// scrub both shell launch sites now apply, exercised the way they call it:
/// `entitlement_for` resolves the serving account from the launch's backend,
/// then `foreign_entitlement_credential_vars` names every OTHER
/// entitlement's environment-shaped credential for `env_remove` before the
/// launch spawns.
#[test]
fn a_shell_started_launch_under_one_entitlement_never_carries_the_other_accounts_variable() {
    const VAR_A: &str = "GLASSHOUSE_SHELL_SCRUB_TEST_ONLY_A";
    const VAR_B: &str = "GLASSHOUSE_SHELL_SCRUB_TEST_ONLY_B";
    const VALUE_A: &str = "fake-shell-scrub-a-0123456789abcdef";
    const VALUE_B: &str = "fake-shell-scrub-b-fedcba9876543210";

    let user = two_accounts(VAR_A, VAR_B);
    let effective = EffectiveConfig::new(&user, None);
    let entitlement = effective
        .entitlement_for(
            IntegrationId::ClaudeCode,
            &BackendResource::DirectProvider {
                provider: "alpha-probe".to_owned(),
            },
        )
        .expect("one entitlement names alpha-probe, which is not a contradiction");
    assert_eq!(entitlement.as_ref().map(|e| e.name()), Some("claude-a"));
    let scrub =
        effective.foreign_entitlement_credential_vars(entitlement.as_ref().map(|e| e.name()));
    assert_eq!(
        scrub,
        vec![VAR_B.to_owned()],
        "only the other account's variable is named for removal"
    );

    let fixture = Fixture::new();
    let launch = fixture.scrubbed_launch(scrub);

    // SAFETY: both variables are unique to this test and removed before it
    // can panic, so no other test can observe them set.
    unsafe {
        std::env::set_var(VAR_A, VALUE_A);
        std::env::set_var(VAR_B, VALUE_B);
    }
    let child_env = fixture.run(&launch);
    unsafe {
        std::env::remove_var(VAR_A);
        std::env::remove_var(VAR_B);
    }

    assert!(
        child_env.contains(VALUE_A),
        "the serving account's credential must reach its own launch:\n{child_env}"
    );
    assert!(
        !child_env.contains(VAR_B) && !child_env.contains(VALUE_B),
        "the other account's variable was inherited into a session it does not serve:\n{child_env}"
    );
}

/// **The scrub's other direction.** A launch no entitlement serves — here, a
/// backend no configured entitlement's `provider` names — carries *neither*
/// account's variable: a session charged to no account has no business
/// holding any account's key.
#[test]
fn a_shell_started_launch_with_no_serving_entitlement_carries_neither_accounts_variable() {
    const VAR_A: &str = "GLASSHOUSE_SHELL_SCRUB_TEST_ONLY_NONE_A";
    const VAR_B: &str = "GLASSHOUSE_SHELL_SCRUB_TEST_ONLY_NONE_B";
    const VALUE_A: &str = "fake-shell-scrub-none-a-0123456789abcdef";
    const VALUE_B: &str = "fake-shell-scrub-none-b-fedcba9876543210";

    let user = two_accounts(VAR_A, VAR_B);
    let effective = EffectiveConfig::new(&user, None);
    let entitlement = effective
        .entitlement_for(
            IntegrationId::ClaudeCode,
            &BackendResource::DirectProvider {
                provider: "unclaimed-probe".to_owned(),
            },
        )
        .expect("no entitlement names unclaimed-probe, which is not a contradiction");
    assert!(
        entitlement.is_none(),
        "no configured entitlement should claim this backend"
    );
    let scrub =
        effective.foreign_entitlement_credential_vars(entitlement.as_ref().map(|e| e.name()));
    assert_eq!(
        scrub,
        vec![VAR_A.to_owned(), VAR_B.to_owned()],
        "with no serving entitlement, every account's variable is named for removal"
    );

    let fixture = Fixture::new();
    let launch = fixture.scrubbed_launch(scrub);

    // SAFETY: see the sibling test above.
    unsafe {
        std::env::set_var(VAR_A, VALUE_A);
        std::env::set_var(VAR_B, VALUE_B);
    }
    let child_env = fixture.run(&launch);
    unsafe {
        std::env::remove_var(VAR_A);
        std::env::remove_var(VAR_B);
    }

    assert!(
        !child_env.contains(VAR_A)
            && !child_env.contains(VALUE_A)
            && !child_env.contains(VAR_B)
            && !child_env.contains(VALUE_B),
        "a session no entitlement serves inherited a configured account's credential:\n{child_env}"
    );
}
