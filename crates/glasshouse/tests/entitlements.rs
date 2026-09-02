//! Phase 56 lines 1946, 1947 and 1954, renamed by Phase 56A line 1962 — an
//! entitlement as a routing resource with rules of its own, entered the way
//! production enters it.
//!
//! Two halves, for the reason `tests/subscription_pressure.rs` gives for its
//! own. The first goes through [`SessionRouter::choose`] with hand-built
//! destinations that differ **in the entitlement alone**, and shows the
//! entitlement constraint removing a candidate, naming itself, and refusing
//! to be outranked by a score. The second runs the shipped binary against a
//! `[entitlements.<name>]` table it wrote itself: nothing in half one can
//! fail on a build where `main.rs::routing_destinations` stops attaching the
//! entitlement, where the launch proceeds past a refused sole destination,
//! or where the announcement names the harness instead of the entitlement —
//! and those three are the whole of what this package wires. Practice §35.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::Instant;

use glasshouse::config::pairing::{WarmSession, WarmSessionState};
use glasshouse::harness::pairing::PairingOverrides;
use glasshouse::integrations::IntegrationId;
use glasshouse::routing::classify::WorkloadTier;
use glasshouse::routing::disposable::JobKind;
use glasshouse::routing::free::FreePool;
use glasshouse::routing::session::{
    Destination, OverrideRefusal, Routed, RouterInputs, RoutingMoment, RoutingOverride,
    SessionRouter, TaskRequirements,
};
use glasshouse::routing::{
    AssignedModel, Backend, Cost, CredentialId, Entitlement, EntitlementRefusal, EntitlementRules,
    HardConstraint, ToolSemantics,
};
use glasshouse::secret::SecretRef;

// ===========================================================================
// Half one — the rules, and the router through `SessionRouter::choose`.
// ===========================================================================

const PROTOCOL: &str = "anthropic-messages";
const HARNESS: IntegrationId = IntegrationId::ClaudeCode;

fn backend(provider: &str) -> Backend {
    Backend::new(
        provider,
        PROTOCOL,
        AssignedModel::named("the-same-model"),
        CredentialId::new(
            provider,
            SecretRef::Environment {
                var: format!("{}_KEY", provider.to_uppercase().replace('-', "_")),
            },
        ),
        Cost::Metered,
        ToolSemantics::Verified,
    )
}

/// A fresh destination that differs from its siblings in the entitlement
/// it carries and in nothing the router scores.
fn fresh(id: &str, entitlement: Option<Entitlement>) -> Destination {
    Destination::fresh(id, HARNESS, "profile", backend("the-same-provider"), None)
        .with_entitlement(entitlement)
}

/// A live, zero-idle existing session — the warmest destination this router
/// can be handed — carrying `entitlement`.
fn warm(id: &str, entitlement: Option<Entitlement>) -> Destination {
    Destination::existing(
        id,
        HARNESS,
        "profile",
        backend("the-same-provider"),
        WarmSession {
            state: WarmSessionState::Live,
            idle_seconds: 0,
        },
    )
    .with_entitlement(entitlement)
}

/// The unrestricted entry configuration supplies for a harness's own
/// sign-in when the user configured none.
fn own_sign_in() -> Entitlement {
    Entitlement::new(HARNESS.slug(), EntitlementRules::UNRESTRICTED)
}

/// A team's API key that must never serve Claude Code.
fn team_key() -> Entitlement {
    Entitlement::new(
        "team-key",
        EntitlementRules::UNRESTRICTED.deny_harnesses([HARNESS]),
    )
}

struct Fixture {
    overrides: PairingOverrides,
    health: FreePool,
}

impl Fixture {
    fn new() -> Self {
        Self {
            overrides: PairingOverrides::default(),
            health: FreePool::new(),
        }
    }

    fn inputs(&self, tier: Option<WorkloadTier>) -> RouterInputs<'_> {
        RouterInputs {
            overrides: &self.overrides,
            health: &self.health,
            now: Instant::now(),
            requirements: TaskRequirements {
                minimum_tier: tier,
                ..TaskRequirements::default()
            },
        }
    }

    fn choose(
        &self,
        router: &SessionRouter,
        destinations: &[Destination],
        tier: Option<WorkloadTier>,
    ) -> Routed {
        router
            .choose(
                RoutingMoment::SessionStart,
                None,
                destinations,
                &self.inputs(tier),
            )
            .expect("at least one destination is eligible in every test that calls this")
    }
}

/// The rejection recorded for `id`, if a hard constraint removed it.
fn rejection<'a>(routed: &'a Routed, id: &str) -> Option<&'a HardConstraint> {
    routed
        .rejected()
        .iter()
        .find(|(destination, _)| destination.id() == id)
        .map(|(_, constraint)| constraint)
}

fn refused_harness(entitlement: &str, harness: IntegrationId) -> HardConstraint {
    HardConstraint::Entitlement {
        entitlement: entitlement.to_owned(),
        refused: EntitlementRefusal::Harness(harness),
    }
}

fn refused_tier(entitlement: &str, tier: WorkloadTier) -> HardConstraint {
    HardConstraint::Entitlement {
        entitlement: entitlement.to_owned(),
        refused: EntitlementRefusal::Tier(tier),
    }
}

// --- line 1947: the rule -----------------------------------------------------

/// **Line 1947's resolution rule, on every axis.** Deny wins over allow; a
/// value on both lists is refused. The three axes share one resolution
/// function, and this holds each of them to it.
#[test]
fn deny_wins_over_allow_on_every_axis() {
    let rules = EntitlementRules::UNRESTRICTED
        .allow_harnesses([IntegrationId::ClaudeCode, IntegrationId::Codex])
        .deny_harnesses([IntegrationId::ClaudeCode])
        .allow_tiers([WorkloadTier::Leaf, WorkloadTier::Heavy])
        .deny_tiers([WorkloadTier::Heavy])
        .allow_job_kinds([JobKind::Classification, JobKind::Evaluation])
        .deny_job_kinds([JobKind::Evaluation]);

    assert!(
        !rules.serves_harness(IntegrationId::ClaudeCode),
        "a harness on both lists is denied"
    );
    assert!(rules.serves_harness(IntegrationId::Codex));
    assert!(
        !rules.serves_tier(WorkloadTier::Heavy),
        "a tier on both lists is denied"
    );
    assert!(rules.serves_tier(WorkloadTier::Leaf));
    assert!(
        !rules.serves_job_kind(JobKind::Evaluation),
        "a job kind on both lists is denied"
    );
    assert!(rules.serves_job_kind(JobKind::Classification));

    assert_eq!(
        rules.refusal(IntegrationId::ClaudeCode, Some(WorkloadTier::Leaf)),
        Some(EntitlementRefusal::Harness(IntegrationId::ClaudeCode)),
        "the harness half is asked first"
    );
    assert_eq!(
        rules.refusal(IntegrationId::Codex, Some(WorkloadTier::Heavy)),
        Some(EntitlementRefusal::Tier(WorkloadTier::Heavy))
    );
    assert_eq!(
        rules.refusal(IntegrationId::Codex, Some(WorkloadTier::Leaf)),
        None
    );
}

/// **Line 1947's two meanings of "absent".** An empty allow-list admits
/// everything not denied — which is what makes the default entry for a
/// harness's own sign-in change nothing — and a stated allow-list admits
/// only what it names.
#[test]
fn an_empty_allow_list_admits_everything_not_denied_and_a_stated_one_only_its_members() {
    assert!(EntitlementRules::UNRESTRICTED.is_unrestricted());
    for harness in IntegrationId::ALL {
        assert!(EntitlementRules::UNRESTRICTED.serves_harness(*harness));
    }
    for tier in [
        WorkloadTier::Deterministic,
        WorkloadTier::Leaf,
        WorkloadTier::Standard,
        WorkloadTier::Heavy,
        WorkloadTier::Frontier,
    ] {
        assert!(EntitlementRules::UNRESTRICTED.serves_tier(tier));
        assert_eq!(
            EntitlementRules::UNRESTRICTED.refusal(HARNESS, Some(tier)),
            None
        );
    }

    let deny_only = EntitlementRules::UNRESTRICTED.deny_tiers([WorkloadTier::Frontier]);
    assert!(!deny_only.is_unrestricted());
    assert!(
        deny_only.serves_tier(WorkloadTier::Heavy),
        "not denied, and no allow-list"
    );
    assert!(!deny_only.serves_tier(WorkloadTier::Frontier));

    let allow_only = EntitlementRules::UNRESTRICTED.allow_harnesses([IntegrationId::Codex]);
    assert!(allow_only.serves_harness(IntegrationId::Codex));
    assert!(
        !allow_only.serves_harness(IntegrationId::Cursor),
        "a stated allow-list admits only its members"
    );
}

// --- line 1954: the constraint ----------------------------------------------

/// **Line 1954 at the router.** Two fresh destinations identical in every
/// scored axis; one carries an entitlement whose rule denies this harness.
/// It is never a candidate — it is in `rejected`, not `considered` — the
/// constraint names the entitlement, and the rendered explanation carries
/// the sentence a person reads. Both orders, so the caller's tiebreaker
/// cannot be what decided it.
#[test]
fn an_entitlement_that_denies_the_harness_removes_the_destination_and_names_itself() {
    let fixture = Fixture::new();
    let router = SessionRouter::new();
    let team = fresh("team", Some(team_key()));
    let own = fresh("own", Some(own_sign_in()));

    for order in [
        vec![team.clone(), own.clone()],
        vec![own.clone(), team.clone()],
    ] {
        let routed = fixture.choose(&router, &order, None);
        assert_eq!(routed.chosen().id(), "own", "{}", routed.render_overview());
        assert!(
            routed.considered().iter().all(|(d, _)| d.id() != "team"),
            "a refused destination is never scored:\n{}",
            routed.render_overview()
        );
        assert_eq!(
            rejection(&routed, "team"),
            Some(&refused_harness("team-key", HARNESS)),
            "{}",
            routed.render_overview()
        );
        let rendered = routed.render_overview();
        assert!(
            rendered.contains(
                "hard entitlement constraint — entitlement `team-key` does not serve harness \
                 `claude-code`"
            ),
            "the explanation must name the entitlement and the harness:\n{rendered}"
        );
    }
}

/// **A hard constraint, not a price.** The warmest destination this router
/// knows — a live, zero-idle session — loses to a cold fresh one when its
/// entitlement's rule denies the harness. No score outranks the user's rule.
#[test]
fn the_entitlement_constraint_outranks_a_warm_session() {
    let fixture = Fixture::new();
    let router = SessionRouter::new();
    let warm_but_denied = warm("warm", Some(team_key()));
    let cold = fresh("cold", Some(own_sign_in()));

    let routed = fixture.choose(
        &router,
        &[warm_but_denied, cold],
        Some(WorkloadTier::Standard),
    );
    assert_eq!(routed.chosen().id(), "cold", "{}", routed.render_overview());
    assert_eq!(
        rejection(&routed, "warm"),
        Some(&refused_harness("team-key", HARNESS))
    );
}

/// **The tier half fires only against an established tier.** With no task
/// stated the tier is unknown, and a rule about tiers has nothing to compare
/// against — an allow-list of tiers does not refuse a launch that stated no
/// task, exactly as line 1516's ceiling gate is never raised against an
/// unknown ceiling. With a tier stated, deny and allow both bite, and the
/// constraint names the tier.
#[test]
fn a_tier_rule_fires_only_against_an_established_tier() {
    let fixture = Fixture::new();
    let router = SessionRouter::new();
    let no_heavy = Entitlement::new(
        "no-heavy",
        EntitlementRules::UNRESTRICTED.deny_tiers([WorkloadTier::Heavy]),
    );
    let leaf_only = Entitlement::new(
        "leaf-only",
        EntitlementRules::UNRESTRICTED.allow_tiers([WorkloadTier::Leaf]),
    );
    let set = vec![
        fresh("no-heavy", Some(no_heavy)),
        fresh("leaf-only", Some(leaf_only)),
        fresh("own", Some(own_sign_in())),
    ];

    let unknown = fixture.choose(&router, &set, None);
    assert!(
        unknown.rejected().is_empty(),
        "no tier is established, so no tier rule fires:\n{}",
        unknown.render_overview()
    );

    let heavy = fixture.choose(&router, &set, Some(WorkloadTier::Heavy));
    assert_eq!(
        rejection(&heavy, "no-heavy"),
        Some(&refused_tier("no-heavy", WorkloadTier::Heavy))
    );
    assert_eq!(
        rejection(&heavy, "leaf-only"),
        Some(&refused_tier("leaf-only", WorkloadTier::Heavy))
    );
    assert_eq!(heavy.chosen().id(), "own");
    assert!(
        heavy
            .render_overview()
            .contains("entitlement `no-heavy` does not serve the `heavy` tier"),
        "{}",
        heavy.render_overview()
    );

    let leaf = fixture.choose(&router, &set, Some(WorkloadTier::Leaf));
    assert!(
        leaf.rejected().is_empty(),
        "leaf work is admitted by both rules:\n{}",
        leaf.render_overview()
    );
}

/// **A destination with no entitlement is never refused by one.** `None`
/// is "no entry describes this resource" — a gateway-backed profile, a
/// provider nobody named — and nobody's rule can refuse what nobody's rule
/// describes.
#[test]
fn a_destination_with_no_entitlement_is_never_refused_by_one() {
    let fixture = Fixture::new();
    let router = SessionRouter::new();
    let routed = fixture.choose(
        &router,
        &[fresh("unnamed", None), fresh("team", Some(team_key()))],
        Some(WorkloadTier::Frontier),
    );
    assert_eq!(routed.chosen().id(), "unnamed");
    assert_eq!(rejection(&routed, "unnamed"), None);
    assert_eq!(
        rejection(&routed, "team"),
        Some(&refused_harness("team-key", HARNESS))
    );
}

/// **`refused` reports the gate `choose` ran, for the case `choose` cannot.**
/// One destination, refused, no current session to hold: `choose` answers
/// `None`, and the rejection would be lost with it. `refused` is the same
/// gate, and the launch path asks it before falling back — see half two.
#[test]
fn refused_reports_the_gate_when_choose_has_nowhere_to_go() {
    let fixture = Fixture::new();
    let router = SessionRouter::new();
    let only = vec![fresh("team", Some(team_key()))];
    let inputs = fixture.inputs(None);

    assert!(
        router
            .choose(RoutingMoment::SessionStart, None, &only, &inputs)
            .is_none(),
        "every destination refused and nothing to hold is `None`"
    );
    let refused = router.refused(&only, &inputs);
    assert_eq!(refused.len(), 1);
    assert_eq!(refused[0].0.id(), "team");
    assert_eq!(refused[0].1, refused_harness("team-key", HARNESS));

    // And the same gate admits what `choose` would have admitted.
    let admitted = vec![fresh("own", Some(own_sign_in()))];
    assert!(router.refused(&admitted, &inputs).is_empty());
}

/// **An override overrules a ranking, never a fact about what can serve.**
/// The user names the refused destination; the router keeps the eligible one
/// and says the override hit an entitlement constraint.
#[test]
fn an_override_naming_a_refused_destination_is_refused_by_the_entitlement() {
    let fixture = Fixture::new();
    let router = SessionRouter::with_override(RoutingOverride::to("team"));
    let routed = fixture.choose(
        &router,
        &[
            fresh("own", Some(own_sign_in())),
            fresh("team", Some(team_key())),
        ],
        None,
    );
    assert_eq!(routed.chosen().id(), "own");
    assert_eq!(
        routed.override_refused(),
        Some(&OverrideRefusal::Ineligible(
            "team".to_owned(),
            refused_harness("team-key", HARNESS)
        ))
    );
    assert!(
        routed
            .render_overview()
            .contains("which a hard entitlement constraint rejected"),
        "{}",
        routed.render_overview()
    );
}

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
        std::fs::write(
            config_dir.join("config.toml"),
            format!(
                "version = 1\n\n\
                 [integrations.claude-code]\nenabled = true\nexecutable = \"{escaped}\"\n\
                 {extra}"
            ),
        )
        .expect("write user config");

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

    fn stdout(&self, args: &[&str]) -> String {
        String::from_utf8_lossy(&self.glasshouse(args).stdout).into_owned()
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

/// The `rejected` section of a `glasshouse route` report, or an empty string
/// when nothing was rejected.
fn rejected_section(report: &str) -> &str {
    report
        .split_once("\nrejected\n")
        .map(|(_, rejected)| rejected)
        .unwrap_or("")
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

/// **Line 1954 on the reporting path.** `glasshouse route` ranks every
/// destination this project could use; the one whose entitlement denies the
/// harness is under `rejected`, with the entitlement named — and it decides
/// nothing.
#[test]
fn route_names_the_entitlement_that_refused_a_destination() {
    let binary = Binary::with_config(&format!("{PROFILES}{TEAM_KEY_DENIES_CLAUDE_CODE}"));
    let report = binary.stdout(&["route"]);
    let rejected = rejected_section(&report);
    assert!(
        rejected.contains("fresh:claude-code:alpha"),
        "the alpha profile is refused:\n{report}"
    );
    assert!(
        rejected.contains(
            "hard entitlement constraint — entitlement `team-key` does not serve harness \
             `claude-code`"
        ),
        "{report}"
    );
    assert!(
        !rejected.contains("fresh:claude-code:beta")
            && !rejected.contains("fresh:claude-code:native"),
        "only the destination the rule describes is refused:\n{report}"
    );
    assert!(
        binary.harness_invocations().is_empty(),
        "`route` starts nothing"
    );
}

/// **Line 1954's *announce which entitlement served*, and line 1946's
/// default.** A user who configured nothing is told the harness's own sign-in
/// serves the session, under the default entry named for the harness; a user
/// who configured an entry for that sign-in is told its name and its plan,
/// and the default is gone. A build whose announcement names the harness
/// instead of the entitlement fails the second half.
#[test]
fn the_native_default_and_a_configured_native_entitlement_are_announced_by_name() {
    // Automatic routing off: this test's contract is the NATIVE announcement,
    // and since map line 372 closed, an unpinned launch under automatic
    // routing (the default) may legitimately land on a provider profile
    // instead of the implied native one.
    let unconfigured = Binary::with_config(&format!("{PROFILES}\n[routing]\nautomatic = false\n"));
    let said = unconfigured.launch_ok(None);
    assert!(
        said.contains(
            "entitlement `claude-code` (Claude Code's own sign-in) will serve this session."
        ),
        "{said}"
    );

    let configured = Binary::with_config(&format!(
        "{PROFILES}{MAX_PLAN}\n[routing]\nautomatic = false\n"
    ));
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

/// **The announcement on the path that continues.** The second launch is
/// steered by the router into the session the first one started, and says
/// which entitlement that session is charged to — the same entry, resolved
/// by the same function.
#[test]
fn a_continued_session_announces_its_entitlement() {
    // MAX_PLAN alone, no provider profiles: continuation IS the router
    // steering the second launch, so automatic routing must stay on — and
    // since map line 372 closed, an unpinned launch under automatic routing
    // ranks every enabled profile, so the native entitlement this test
    // announces must be the only candidate for the first launch to be
    // native.
    let binary = Binary::with_config(MAX_PLAN);
    binary.launch_ok(None);
    let said = binary.launch_ok(None);
    assert!(said.contains("continuing session"), "{said}");
    assert!(
        said.contains(
            "entitlement `max` (Claude plan, Claude Code's own sign-in) will serve this session."
        ),
        "{said}"
    );
    assert_eq!(binary.harness_invocations().len(), 2);
}

/// **The launch the router never sees.** With routing off (`--no-routing`,
/// or `automatic = false` under `[routing]`) `routing_destinations` and
/// `choose` do not run, so the router's gate cannot refuse anything — and
/// line 1954 says *never*. The launch path asks the same
/// `EntitlementRules::refusal` once more, for the harness half a rule can
/// answer without a classification, and refuses by name.
#[test]
fn a_routing_off_launch_still_applies_the_harness_rule() {
    let binary = Binary::with_config(&format!("{PROFILES}{TEAM_KEY_DENIES_CLAUDE_CODE}"));
    let refused = binary.glasshouse(&[
        "launch",
        "claude-code",
        "--headless",
        "--no-routing",
        "--profile",
        "alpha",
    ]);
    let said = Binary::both_streams(&refused);
    assert!(!refused.status.success(), "{said}");
    assert!(
        said.contains("entitlement `team-key` does not serve harness `claude-code`"),
        "{said}"
    );
    assert!(binary.harness_invocations().is_empty());

    // And with routing off the admitted profile still launches, announced.
    let out = binary.glasshouse(&["launch", "claude-code", "--headless", "--no-routing"]);
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

/// **The router-side guard's own work: a tier refusal of a launch's sole
/// destination.** A launch that states heavy work under a profile whose
/// entitlement denies heavy work is refused by name before anything starts —
/// and only the router can refuse it, because the tier exists only once the
/// task is classified; the harness-half gate after the profile resolves has
/// no tier to read. The mutation this test exists to kill
/// (`launch-guard-removed`) survived every other test in this file, because
/// each of those launches was also refused by the harness half.
#[test]
fn a_launch_stating_heavy_work_is_refused_by_a_tier_rule_before_anything_starts() {
    const NO_HEAVY_WORK: &str = "\n\
         [entitlements.team-key]\nprovider = \"alpha-probe\"\n\
         deny_tiers = [\"heavy\", \"frontier\"]\n";
    let binary = Binary::with_config(&format!("{PROFILES}{NO_HEAVY_WORK}"));

    let refused = binary.glasshouse(&[
        "launch",
        "claude-code",
        "--headless",
        "--profile",
        "alpha",
        "--task",
        "run the whole test suite in the terminal and fix whatever breaks",
    ]);
    let said = Binary::both_streams(&refused);
    assert!(
        !refused.status.success(),
        "heavy work charged to an entitlement that denies heavy work must be refused:\n{said}"
    );
    assert!(
        said.contains("entitlement `team-key` does not serve the `heavy` tier"),
        "{said}"
    );
    assert!(
        binary.harness_invocations().is_empty(),
        "nothing may have been started: {:?}",
        binary.harness_invocations()
    );

    // The same profile, leaf-shaped work: admitted, and announced as an API
    // key behind its provider.
    let out = binary.glasshouse(&[
        "launch",
        "claude-code",
        "--headless",
        "--profile",
        "alpha",
        "--task",
        "what is a monad",
    ]);
    let said = Binary::both_streams(&out);
    assert!(out.status.success(), "{said}");
    assert!(
        said.contains(
            "entitlement `team-key` (behind provider `alpha-probe`) will serve this session."
        ),
        "{said}"
    );
    assert_eq!(binary.harness_invocations().len(), 1);
}

/// **The tier half, through the classification `--task` produces.** A rule
/// denying heavy work refuses the destination for a task the heuristics
/// classify as heavy, names the tier, and leaves a leaf-shaped task alone.
#[test]
fn a_tier_rule_reaches_the_route_report_through_the_task_classification() {
    const NO_HEAVY_WORK: &str = "\n\
         [entitlements.team-key]\nprovider = \"alpha-probe\"\n\
         deny_tiers = [\"heavy\", \"frontier\"]\n";
    let binary = Binary::with_config(&format!("{PROFILES}{NO_HEAVY_WORK}"));

    let heavy = binary.stdout(&[
        "route",
        "--task",
        "run the whole test suite in the terminal and fix whatever breaks",
    ]);
    let rejected = rejected_section(&heavy);
    assert!(
        rejected.contains("fresh:claude-code:alpha")
            && rejected.contains("entitlement `team-key` does not serve the `heavy` tier"),
        "{heavy}"
    );

    let leaf = binary.stdout(&["route", "--task", "what is a monad"]);
    assert!(
        !rejected_section(&leaf).contains("fresh:claude-code:alpha"),
        "leaf work is not heavy work:\n{leaf}"
    );

    // A session that already runs under `alpha` — started with no task, so
    // no tier rule could fire — is refused for heavy work exactly as the
    // fresh destination is: the rule is attached to recorded sessions too.
    binary.launch_ok(Some("alpha"));
    let heavy_again = binary.stdout(&[
        "route",
        "--task",
        "run the whole test suite in the terminal and fix whatever breaks",
    ]);
    let rejected = rejected_section(&heavy_again);
    assert!(
        rejected.contains("via alpha-probe (existing) — hard entitlement constraint — entitlement `team-key` does not serve the `heavy` tier"),
        "{heavy_again}"
    );
}
