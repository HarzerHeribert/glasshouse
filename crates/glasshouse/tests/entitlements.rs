//! Phase 56 lines 1946, 1947 and 1954, renamed by Phase 56A line 1962 — an
//! entitlement as a routing resource with rules of its own, entered the way
//! production enters it: the shipped binary against a `[entitlements.<name>]`
//! table it wrote itself.
//!
//! This file used to have a second, unit-level half through
//! `SessionRouter::choose`, hand-building destinations that differed in the
//! entitlement alone — deleted with the router (design-decisions.md,
//! 2026-09-16, "Glasshouse never decides which model is used"), along with
//! the two tests below that drove the deleted `glasshouse route`/`--task`
//! surface. What is left is `launch`'s own application of
//! `EntitlementRules::refusal`, which needed no router to begin with.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use clap::Parser as _;

// Splitting a pre-2026-09-11 fixture into Glasshouse's `config.toml` and the
// gateway's `gateway.toml` — see the included file for what moves and why.
include!("fixtures/gateway_split.rs");

// ===========================================================================
// Half two — the shipped binary, reading `[entitlements.<name>]`.
//
// The fixture is `tests/subscription_pressure.rs`'s, reproduced rather than
// shared because integration tests are separate crates; the fake harness and
// the argv log are the same mechanism for the same reasons that file gives.
// ===========================================================================

const CREDENTIAL_VAR: &str = "GLASSHOUSE_ENTITLEMENT_TEST_KEY";

/// Two direct-provider launch profiles for Claude Code, on two providers.
const PROFILES: &str = "\n\
     [providers.alpha-probe]\ntemplate = \"openrouter\"\n\
     credential_env = [\"GLASSHOUSE_ENTITLEMENT_TEST_KEY\"]\n\n\
     [providers.beta-probe]\ntemplate = \"openrouter\"\n\
     credential_env = [\"GLASSHOUSE_ENTITLEMENT_TEST_KEY\"]\n\n\
     [profiles.alpha]\nharness = \"claude-code\"\n\
     expected_protocol = \"anthropic-messages\"\n\n\
     [profiles.alpha.backend]\nkind = \"direct-provider\"\n\
     provider = \"alpha-probe\"\n\n\
     [profiles.beta]\nharness = \"claude-code\"\n\
     expected_protocol = \"anthropic-messages\"\n\n\
     [profiles.beta.backend]\nkind = \"direct-provider\"\n\
     provider = \"beta-probe\"\n";

/// The team's API key behind `alpha-probe`, which must never serve Claude Code.
const TEAM_KEY_DENIES_CLAUDE_CODE: &str = "\n\
     [entitlements.team-key]\nkind = \"api-key\"\nprovider = \"alpha-probe\"\n\
     deny_harnesses = [\"claude-code\"]\n";

/// A configured entry for Claude Code's own sign-in, replacing the default.
const MAX_PLAN: &str = "\n\
     [entitlements.max]\nkind = \"claude\"\nnative_harness = \"claude-code\"\n";

/// The env var the shared fixture script reads its argv-log destination
/// from, set per spawn by [`Binary::glasshouse`] rather than baked into the
/// script bytes — see [`shared_fixture`]'s doc for why.
const ARGV_LOG_VAR: &str = "GLASSHOUSE_TEST_ARGV_LOG";

struct Binary {
    _tmp: tempfile::TempDir,
    base: PathBuf,
    root: PathBuf,
    argv_log: PathBuf,
}

impl Binary {
    fn with_config(extra: &str) -> Self {
        let tmp = tempfile::tempdir().expect("tempdir");
        let base = tmp.path().to_path_buf();
        let root = base.join("workspace");
        std::fs::create_dir_all(root.join(".git")).expect("create project root");
        let root = std::fs::canonicalize(&root).expect("canonicalize project root");

        let bin_dir = base.join("bin");
        std::fs::create_dir_all(&bin_dir).expect("create bin dir");
        let argv_log = base.join("argv.log");
        let harness = install_fake_harness(&bin_dir);
        let escaped = harness.display().to_string().replace('\\', "\\\\");

        let config_dir = base.join("config");
        std::fs::create_dir_all(&config_dir).expect("create config dir");
        // The accounts are the gateway's since the 2026-09-11 ruling; this
        // fixture writes both halves so the binary reads what it used to.
        let (config_text, gateway_text) = split_gateway_state(&format!(
            "version = 1\n\n\
                 [integrations.claude-code]\nenabled = true\nexecutable = \"{escaped}\"\n\
                 {extra}"
        ));
        std::fs::write(config_dir.join("config.toml"), config_text).expect("write user config");
        std::fs::write(config_dir.join("gateway.toml"), gateway_text).expect("write user config");

        Self {
            _tmp: tmp,
            base,
            root,
            argv_log,
        }
    }

    fn glasshouse(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_glasshouse"))
            .arg("--scope")
            .arg(&self.root)
            .arg("--data-dir")
            .arg(self.base.join("data"))
            .arg("--config-dir")
            .arg(self.base.join("config"))
            .args(args)
            .env(CREDENTIAL_VAR, "planted-opaque-entitlement-value-56")
            .env(ARGV_LOG_VAR, &self.argv_log)
            .env("PATH", self.base.join("empty-path"))
            .output()
            .expect("the glasshouse binary must be runnable")
    }

    fn both_streams(output: &Output) -> String {
        format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )
    }

    fn harness_invocations(&self) -> Vec<String> {
        match std::fs::read_to_string(&self.argv_log) {
            Ok(log) => log.lines().map(str::to_owned).collect(),
            Err(_) => Vec::new(),
        }
    }

    /// Launch headless under `profile` (or the implied Native one) and hand
    /// back what the binary said on both streams, asserting success.
    fn launch_ok(&self, profile: Option<&str>) -> String {
        let mut args = vec!["launch", "claude-code", "--headless"];
        if let Some(profile) = profile {
            args.extend(["--profile", profile]);
        }
        let out = self.glasshouse(&args);
        let said = Self::both_streams(&out);
        assert!(out.status.success(), "the launch must succeed:\n{said}");
        said
    }

    /// The one recorded session — `glasshouse launch` no longer ranks this
    /// project's sessions against a new one (design-decisions.md, 2026-09-16,
    /// "Glasshouse never decides which model is used"), so a continuation
    /// this fixture wants to observe has to name the session by id through
    /// `--to` rather than rely on a bare launch picking it automatically.
    fn only_session_id(&self) -> String {
        let cli = glasshouse::Cli::try_parse_from([
            "glasshouse",
            "--data-dir",
            self.base.join("data").to_str().unwrap(),
            "--config-dir",
            self.base.join("config").to_str().unwrap(),
        ])
        .unwrap();
        let runtime = glasshouse::bootstrap(&cli, &self.root).unwrap();
        let conn = rusqlite::Connection::open(runtime.database_path()).unwrap();
        let mut statement = conn.prepare("SELECT id FROM sessions").unwrap();
        let ids: Vec<String> = statement
            .query_map([], |row| row.get(0))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        assert_eq!(
            ids.len(),
            1,
            "expected exactly one recorded session: {ids:?}"
        );
        ids.into_iter().next().unwrap()
    }
}

/// Write the shared fixture executable once per test binary instead of once
/// per test, so macOS Gatekeeper (`syspolicyd`/XProtect) validates it once
/// per run instead of once per test — see the project memory
/// `gatekeeper-scans-make-pty-fixtures-flaky` and GH-FIXTURE-REUSE /
/// GH-ARGV-LOG-HOIST. The argv-log destination used to be interpolated into
/// the script bytes, which made every call's content distinct; it is now
/// read from `ARGV_LOG_VAR` at spawn time (set by [`Binary::glasshouse`]),
/// so the script bytes are constant and every call below collapses onto the
/// one file the first caller writes.
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
    shared_fixture(
        "fake-claude-code",
        &format!("#!/bin/sh\nprintf '%s\\n' \"$*\" >> \"${ARGV_LOG_VAR}\"\nexit 0\n"),
    )
}

#[cfg(windows)]
fn install_fake_harness(_bin_dir: &Path) -> PathBuf {
    shared_fixture(
        "fake-claude-code.cmd",
        &format!("@echo off\r\necho %*>>\"%{ARGV_LOG_VAR}%\"\r\nexit /b 0\r\n"),
    )
}

#[cfg(test)]
mod shared_fixture_proof {
    use super::{Binary, MAX_PLAN, install_fake_harness};

    /// `ARGV_LOG_VAR` is read only by the byte-for-byte fixture check in this
    /// module, which is `#[cfg(unix)]` because the shared fixture is a
    /// `#!/bin/sh` script there and a `.cmd` file on Windows. Gated to the
    /// same cfg as its only user rather than silenced with an `allow`.
    #[cfg(unix)]
    use super::ARGV_LOG_VAR;

    /// **The once-per-binary proof, through the real caller.** Every test in
    /// this file that spawns the harness goes through [`Binary::with_config`],
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

    /// **Bytes constant.** The shared fixture's bytes read the argv-log
    /// destination from `ARGV_LOG_VAR` rather than embedding a per-test
    /// path, so the script text is the same regardless of which tempdir
    /// asked for it.
    #[cfg(unix)]
    #[test]
    fn the_shared_fixture_reads_its_log_path_from_the_env_var_not_the_script() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = install_fake_harness(tmp.path());
        let content = std::fs::read_to_string(&path).expect("read shared fixture");
        assert_eq!(
            content,
            format!("#!/bin/sh\nprintf '%s\\n' \"$*\" >> \"${ARGV_LOG_VAR}\"\nexit 0\n"),
            "the shared fixture's bytes must read the log destination from the env var, \
             not have a path baked in"
        );
    }

    /// **End-to-end, through the real caller.** The env var the fixture
    /// reads is exactly the one [`Binary::glasshouse`] sets per spawn —
    /// proven by actually launching the shipped binary and reading the argv
    /// log back, not by inspecting the script text alone.
    #[test]
    fn a_real_launch_through_the_shared_fixture_writes_its_argv_to_the_requested_log() {
        let binary = Binary::with_config(MAX_PLAN);
        let said = binary.launch_ok(None);
        assert_eq!(
            binary.harness_invocations().len(),
            1,
            "the shared, env-driven fixture must still log exactly one invocation \
             into this fixture's own argv log:\n{said}"
        );
    }
}

/// **Line 1954's *never charge*, through the acting path.** A launch under a
/// profile whose entitlement's rule denies this harness is refused **by
/// name**, before anything exists: no process, no session. The sibling
/// profile on a provider no entry names launches, and is told no rule
/// applies. A build where `routing_destinations` stops attaching the
/// entitlement, or where the launch keeps falling back past a refused sole
/// destination, fails here; nothing in half one can keep it passing.
#[test]
fn a_launch_whose_entitlement_denies_the_harness_is_refused_by_name_and_starts_nothing() {
    let binary = Binary::with_config(&format!("{PROFILES}{TEAM_KEY_DENIES_CLAUDE_CODE}"));

    let refused = binary.glasshouse(&["launch", "claude-code", "--headless", "--profile", "alpha"]);
    let said = Binary::both_streams(&refused);
    assert!(
        !refused.status.success(),
        "a launch charged to an entitlement whose rule denies the harness must be refused:\n{said}"
    );
    assert!(
        said.contains("entitlement `team-key` does not serve harness `claude-code`"),
        "the refusal names the entitlement and the harness:\n{said}"
    );
    assert!(
        said.contains("[entitlements.team-key]"),
        "the refusal says where the rule lives:\n{said}"
    );
    assert!(
        binary.harness_invocations().is_empty(),
        "nothing may have been started: {:?}",
        binary.harness_invocations()
    );

    // The same harness on a provider no entry describes: no rule, and the
    // launch says so rather than naming an entitlement nobody configured.
    let said = binary.launch_ok(Some("beta"));
    assert!(
        said.contains("no `[entitlements]` entry names provider `beta-probe`"),
        "{said}"
    );
    assert_eq!(binary.harness_invocations().len(), 1);
}

/// **Line 1954's *announce which entitlement served*, and line 1946's
/// default.** A user who configured nothing is told the harness's own sign-in
/// serves the session, under the default entry named for the harness; a user
/// who configured an entry for that sign-in is told its name and its plan,
/// and the default is gone. A build whose announcement names the harness
/// instead of the entitlement fails the second half.
#[test]
fn the_native_default_and_a_configured_native_entitlement_are_announced_by_name() {
    // `glasshouse launch` with no destination flag opens the native entry
    // unconditionally now (design-decisions.md, 2026-09-16, "Glasshouse
    // never decides which model is used") — no automatic-routing toggle
    // left to turn off.
    let unconfigured = Binary::with_config(PROFILES);
    let said = unconfigured.launch_ok(None);
    assert!(
        said.contains(
            "entitlement `claude-code` (Claude Code's own sign-in) will serve this session."
        ),
        "{said}"
    );

    let configured = Binary::with_config(&format!("{PROFILES}{MAX_PLAN}"));
    let said = configured.launch_ok(None);
    assert!(
        said.contains(
            "entitlement `max` (Claude plan, Claude Code's own sign-in) will serve this session."
        ),
        "{said}"
    );
    assert!(
        !said.contains("entitlement `claude-code`"),
        "the configured entry replaces the default rather than joining it:\n{said}"
    );
}

/// **The announcement on the path that continues.** A second launch naming
/// the first one's session by `--to` continues it, and says which
/// entitlement that session is charged to — the same entry, resolved by the
/// same function.
#[test]
fn a_continued_session_announces_its_entitlement() {
    let binary = Binary::with_config(MAX_PLAN);
    binary.launch_ok(None);
    let id = binary.only_session_id();
    let out = binary.glasshouse(&["launch", "claude-code", "--headless", "--to", &id]);
    let said = Binary::both_streams(&out);
    assert!(
        out.status.success(),
        "the continuation must launch:\n{said}"
    );
    assert!(said.contains("continuing session"), "{said}");
    assert!(
        said.contains(
            "entitlement `max` (Claude plan, Claude Code's own sign-in) will serve this session."
        ),
        "{said}"
    );
    assert_eq!(binary.harness_invocations().len(), 2);
}

/// **The launch path applies the harness rule.** `glasshouse launch` no
/// longer ranks destinations at all (design-decisions.md, 2026-09-16,
/// "Glasshouse never decides which model is used"), so there is no router
/// gate left to ask — the launch path applies `EntitlementRules::refusal`
/// directly, for the harness half a rule can answer without a
/// classification, and refuses by name.
#[test]
fn a_routing_off_launch_still_applies_the_harness_rule() {
    let binary = Binary::with_config(&format!("{PROFILES}{TEAM_KEY_DENIES_CLAUDE_CODE}"));
    let refused = binary.glasshouse(&["launch", "claude-code", "--headless", "--profile", "alpha"]);
    let said = Binary::both_streams(&refused);
    assert!(!refused.status.success(), "{said}");
    assert!(
        said.contains("entitlement `team-key` does not serve harness `claude-code`"),
        "{said}"
    );
    assert!(binary.harness_invocations().is_empty());

    // And the admitted profile still launches, announced.
    let out = binary.glasshouse(&["launch", "claude-code", "--headless"]);
    let said = Binary::both_streams(&out);
    assert!(out.status.success(), "{said}");
    assert!(
        said.contains(
            "entitlement `claude-code` (Claude Code's own sign-in) will serve this session."
        ),
        "{said}"
    );
    assert_eq!(binary.harness_invocations().len(), 1);
}
