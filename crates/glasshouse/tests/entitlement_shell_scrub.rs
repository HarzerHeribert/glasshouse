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
        let harness_path = install_fake_harness(&bin_dir, &env_log);
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
        let mut launch = HarnessLaunch::new(self.executable.clone(), &self.project);
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

#[cfg(unix)]
fn install_fake_harness(bin_dir: &Path, env_log: &Path) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;

    let path = bin_dir.join("fake-shell-harness");
    // `export -p` is a shell builtin, so the scrubbed environment this
    // fixture spawns under cannot break it.
    std::fs::write(
        &path,
        format!("#!/bin/sh\nexport -p > '{}'\nexit 0\n", env_log.display()),
    )
    .expect("write fake harness");
    let mut perms = std::fs::metadata(&path).unwrap().permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&path, perms).unwrap();
    path
}

#[cfg(windows)]
fn install_fake_harness(bin_dir: &Path, env_log: &Path) -> PathBuf {
    let path = bin_dir.join("fake-shell-harness.cmd");
    std::fs::write(
        &path,
        format!(
            "@echo off\r\nset > \"{}\"\r\nexit /b 0\r\n",
            env_log.display()
        ),
    )
    .expect("write fake harness");
    path
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
