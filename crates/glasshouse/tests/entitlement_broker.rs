//! Phase 56A step 3, capability map lines 1953 and 1966–1969 — the broker:
//! the pool enters the candidate set, and the score chooses.
//!
//! Two halves, `tests/entitlements.rs`'s own split and for its reason. Half
//! one goes through [`SessionRouter::choose`] with hand-built destinations
//! whose entitlements differ in exactly the facet under test, and shows the
//! five-factor score choosing by score (never rotation), the reset-boundary
//! rule reproducing the user's two examples verbatim, stickiness riding on
//! the affinity term's weight, distribution spreading independent fresh
//! choices, and the model axis refusing by name. Half two runs the shipped
//! binary — `glasshouse route` and `glasshouse launch` — against a
//! `[entitlements]` pool it wrote itself, because nothing in half one can
//! fail on a build where `main.rs::routing_destinations` stops widening the
//! candidate set along the entitlement axis, stops attaching the 56A-2
//! facets, or where the launch stops binding the harness process to the
//! pool's chosen account (practice §35).

use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::Instant;

use clap::Parser;

use glasshouse::Runtime;
use glasshouse::config::pairing::{WarmSession, WarmSessionState};
use glasshouse::harness::pairing::PairingOverrides;
use glasshouse::integrations::IntegrationId;
use glasshouse::provider::quota::{CapacityBand, CapacityBandThresholds};
use glasshouse::provider::telemetry::{GatewayQuotaCache, RateLimitHeaders};
use glasshouse::routing::classify::WorkloadTier;
use glasshouse::routing::disposable::JobKind;
use glasshouse::routing::evidence::{
    CostConfidence, EvidenceLedger, FailureClass, NewObservation, ObservationQuery, Outcome,
};
use glasshouse::routing::free::FreePool;
use glasshouse::routing::session::{
    CheckpointQuality, Destination, EntitlementPoolView, FallbackReason, FallbackStep, Routed,
    RouterInputs, RoutingMoment, RoutingOverride, SessionRouter, TaskRequirements,
    entitlement_capacity, entitlement_fallback, entitlement_model_availability,
    entitlement_reset_boundary, entitlement_throttling,
};
use glasshouse::routing::{
    AssignedModel, Backend, Cost, CredentialId, Entitlement, EntitlementModelsFacet,
    EntitlementRefusal, EntitlementRules, EntitlementSource, EntitlementSpendFacet,
    EntitlementThrottleFacet, HardConstraint, ToolSemantics,
};
use glasshouse::secret::SecretRef;

// ===========================================================================
// Half one — the score, through `SessionRouter::choose`.
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

/// An unrestricted, configured entitlement with only its capacity facets
/// attached — the shape most of the score tests vary.
fn entitled(name: &str, band: Option<CapacityBand>, reset: Option<i64>) -> Entitlement {
    Entitlement::new(name, EntitlementRules::UNRESTRICTED).with_capacity(band, reset)
}

/// A fresh destination differing from its siblings in its entitlement and
/// nothing else the router scores.
fn fresh(id: &str, entitlement: Entitlement) -> Destination {
    Destination::fresh(id, HARNESS, "profile", backend("the-same-provider"), None)
        .with_entitlement(Some(entitlement))
}

/// The same, booting from a good checkpoint (line 1594's reduced bootstrap).
fn fresh_with_checkpoint(id: &str, entitlement: Entitlement) -> Destination {
    Destination::fresh(
        id,
        HARNESS,
        "profile",
        backend("the-same-provider"),
        Some(CheckpointQuality::new(true, true)),
    )
    .with_entitlement(Some(entitlement))
}

/// A live, zero-idle existing session — the warmest destination this router
/// can be handed — on `entitlement`.
fn warm(id: &str, entitlement: Entitlement) -> Destination {
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
    .with_entitlement(Some(entitlement))
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

    fn inputs(&self) -> RouterInputs<'_> {
        RouterInputs {
            overrides: &self.overrides,
            health: &self.health,
            now: Instant::now(),
            requirements: TaskRequirements::default(),
        }
    }

    fn choose(&self, router: &SessionRouter, destinations: &[Destination]) -> Routed {
        router
            .choose(
                RoutingMoment::SessionStart,
                None,
                destinations,
                &self.inputs(),
            )
            .expect("at least one destination is eligible in every test that calls this")
    }
}

/// **Line 1966: chosen by score, never rotation.** Three entitlements at
/// three bands, handed to the router worst-first so the caller's-order
/// tiebreaker cannot produce the right answer by accident; repeated
/// identical choices must return the best band every single time, in
/// band order — a build that rotates through the pool, or that collapses
/// the capacity factor to a constant, fails here on the first repeat.
#[test]
fn three_live_entitlements_are_chosen_by_score_order_and_never_rotation() {
    let fixture = Fixture::new();
    let router = SessionRouter::new();
    let set = vec![
        fresh("e3", entitled("acct-3", Some(CapacityBand::Tight), None)),
        fresh("e2", entitled("acct-2", Some(CapacityBand::Healthy), None)),
        fresh("e1", entitled("acct-1", Some(CapacityBand::Plenty), None)),
    ];

    for repeat in 0..8 {
        let routed = fixture.choose(&router, &set);
        assert_eq!(
            routed.chosen().id(),
            "e1",
            "repeat {repeat}: the plenty-band entitlement wins by score, every time:\n{}",
            routed.render_overview()
        );
        let order: Vec<&str> = routed
            .considered()
            .iter()
            .map(|(destination, _)| destination.id())
            .collect();
        assert_eq!(
            order,
            ["e1", "e2", "e3"],
            "the ranking is band order, not offer order and not rotation"
        );
    }
}

/// **Line 1967, the user's two examples verbatim.** A at 12% capacity —
/// the reserve band under the default thresholds — resetting in 1h20m
/// (4800s) beside B at 61% (healthy) resetting in 4 days (345600s): burn A,
/// its remainder would otherwise expire. The same A resetting in 4 days:
/// preserve A, route B.
#[test]
fn the_reset_boundary_burns_an_expiring_remainder_and_preserves_a_distant_one() {
    let thresholds = CapacityBandThresholds::DEFAULT;
    let band_a = thresholds.band_for_percent(12);
    let band_b = thresholds.band_for_percent(61);
    assert_eq!(band_a, CapacityBand::Reserve, "12% is the reserve band");
    assert_eq!(band_b, CapacityBand::Healthy, "61% is the healthy band");

    let fixture = Fixture::new();
    let router = SessionRouter::new();

    // Example 1: A resets in 1h20m — burn A.
    let burning = vec![
        fresh("a", entitled("claude-a", Some(band_a), Some(4_800))),
        fresh("b", entitled("claude-b", Some(band_b), Some(345_600))),
    ];
    let routed = fixture.choose(&router, &burning);
    assert_eq!(
        routed.chosen().id(),
        "a",
        "A's remainder would otherwise expire, so A is burned:\n{}",
        routed.render_overview()
    );
    assert!(
        routed.render_overview().contains("burned aggressively"),
        "the burn is said as the user's rule:\n{}",
        routed.render_overview()
    );

    // Example 2: the same A resets in 4 days — preserve A, route B.
    let preserving = vec![
        fresh("a", entitled("claude-a", Some(band_a), Some(345_600))),
        fresh("b", entitled("claude-b", Some(band_b), Some(345_600))),
    ];
    let routed = fixture.choose(&router, &preserving);
    assert_eq!(
        routed.chosen().id(),
        "b",
        "a low remainder with a far reset is preserved:\n{}",
        routed.render_overview()
    );
    assert!(
        routed.render_overview().contains("preserved"),
        "the preserve half is said too:\n{}",
        routed.render_overview()
    );
}

/// **Line 1968's stickiness, as the affinity term's weight.** A live
/// zero-idle session on a tight entitlement keeps the work against a fresh
/// sibling whose entitlement is two bands ahead and which would boot from a
/// good checkpoint — because the warm context is priced by `session
/// affinity`, and the explanation says stickiness is that term's weight
/// rather than a second mechanism. A build that zeroes the affinity term
/// hops to the sibling and fails here.
#[test]
fn a_warm_session_stays_on_its_entitlement_against_a_better_scoring_sibling() {
    let fixture = Fixture::new();
    let router = SessionRouter::new();
    let set = vec![
        fresh_with_checkpoint(
            "sibling",
            entitled("claude-b", Some(CapacityBand::Plenty), None),
        ),
        warm(
            "warm-on-a",
            entitled("claude-a", Some(CapacityBand::Tight), None),
        ),
    ];

    let routed = fixture.choose(&router, &set);
    assert_eq!(
        routed.chosen().id(),
        "warm-on-a",
        "the session's context outweighs the sibling's capacity lead:\n{}",
        routed.render_overview()
    );
    let rendered = routed.render_overview();
    assert!(
        rendered.contains("entitlement stickiness") && rendered.contains("not a second mechanism"),
        "the explanation says stickiness is the affinity term's weight:\n{rendered}"
    );
}

/// **Line 1968's forced move.** The same warm session moves when its
/// entitlement's capacity band reads exhausted: an account with nothing
/// left cannot serve the next turn, and that is the one band reading sized
/// to outweigh warmth plus the sibling's cold bootstrap.
#[test]
fn a_warm_session_moves_when_its_entitlements_band_reads_exhausted() {
    let fixture = Fixture::new();
    let router = SessionRouter::new();
    let set = vec![
        warm(
            "warm-on-a",
            entitled("claude-a", Some(CapacityBand::Exhausted), None),
        ),
        fresh(
            "sibling",
            entitled("claude-b", Some(CapacityBand::Plenty), None),
        ),
    ];

    let routed = fixture.choose(&router, &set);
    assert_eq!(
        routed.chosen().id(),
        "sibling",
        "an exhausted account cannot serve the next turn:\n{}",
        routed.render_overview()
    );
    assert!(
        routed.render_overview().contains("reads exhausted"),
        "{}",
        routed.render_overview()
    );
}

/// **Line 1968's distribution.** Two simultaneous fresh choices with one
/// entitlement slightly ahead — one band step — must not both land on it:
/// the second choice sees the first's live session in its candidate set,
/// and the in-flight load term hands the second worker to the sibling.
#[test]
fn independent_fresh_choices_spread_across_the_pool() {
    let fixture = Fixture::new();
    let router = SessionRouter::new();

    // Choice 1: nothing live anywhere; the slightly-ahead account wins.
    let first = vec![
        fresh(
            "on-a",
            entitled("claude-a", Some(CapacityBand::Plenty), None),
        ),
        fresh(
            "on-b",
            entitled("claude-b", Some(CapacityBand::Healthy), None),
        ),
    ];
    let routed = fixture.choose(&router, &first);
    assert_eq!(routed.chosen().id(), "on-a", "{}", routed.render_overview());

    // Choice 2: the first choice's session is now live on claude-a and in
    // the set. A second independent fresh choice goes to the sibling.
    let second = vec![
        warm(
            "first-workers-session",
            entitled("claude-a", Some(CapacityBand::Plenty), None),
        ),
        fresh(
            "on-a",
            entitled("claude-a", Some(CapacityBand::Plenty), None),
        ),
        fresh(
            "on-b",
            entitled("claude-b", Some(CapacityBand::Healthy), None),
        ),
    ];
    let routed = fixture.choose(
        &SessionRouter::with_override(RoutingOverride::fresh()),
        &second,
    );
    assert_eq!(
        routed.chosen().id(),
        "on-b",
        "the second fresh choice must not pile onto the marginally-ahead account:\n{}",
        routed.render_overview()
    );
    assert!(
        routed
            .render_overview()
            .contains("spreads to a sibling account"),
        "{}",
        routed.render_overview()
    );
}

/// **Line 1953's model half.** A candidate whose entitlement declares its
/// models and does not declare this destination's is refused by name — in
/// `rejected`, never scored — while a harness-decided facet and an unknown
/// one constrain nothing.
#[test]
fn a_model_the_entitlement_cannot_serve_is_refused_by_name() {
    let fixture = Fixture::new();
    let router = SessionRouter::new();
    let limited = Entitlement::new("limited", EntitlementRules::UNRESTRICTED).with_models(Some(
        EntitlementModelsFacet::Declared(vec!["another-model".to_owned()]),
    ));
    let native = Entitlement::new("native-plan", EntitlementRules::UNRESTRICTED)
        .with_models(Some(EntitlementModelsFacet::HarnessDecided));
    let unknown = Entitlement::new("unknown-models", EntitlementRules::UNRESTRICTED);
    let set = vec![
        fresh("on-limited", limited),
        fresh("on-native", native),
        fresh("on-unknown", unknown),
    ];

    let routed = fixture.choose(&router, &set);
    let rejection = routed
        .rejected()
        .iter()
        .find(|(destination, _)| destination.id() == "on-limited")
        .map(|(_, constraint)| constraint);
    assert_eq!(
        rejection,
        Some(&HardConstraint::Entitlement {
            entitlement: "limited".to_owned(),
            refused: EntitlementRefusal::Model("the-same-model".to_owned()),
        }),
        "{}",
        routed.render_overview()
    );
    assert!(
        routed
            .render_overview()
            .contains("entitlement `limited` does not serve the `the-same-model` model"),
        "the refusal names the entitlement and the model:\n{}",
        routed.render_overview()
    );
    assert!(
        routed
            .considered()
            .iter()
            .all(|(destination, _)| destination.id() != "on-limited"),
        "a refused candidate is never scored"
    );
    assert!(
        routed
            .rejected()
            .iter()
            .all(|(destination, _)| destination.id() == "on-limited"),
        "harness-decided and unknown facets constrain nothing:\n{}",
        routed.render_overview()
    );
}

/// **Line 1966's unknown discipline, per factor.** Every pool term
/// contributes exactly nothing when its facet is unknown, and its evidence
/// says so — never a guessed number — and every pool term is inert, saying
/// so, when the candidate set carries fewer than two configured
/// entitlements.
#[test]
fn unknown_facets_contribute_nothing_and_a_pool_of_one_is_inert() {
    // A live pool (two configured entitlements), no facet read anywhere.
    let unknown_a = fresh(
        "a",
        Entitlement::new("claude-a", EntitlementRules::UNRESTRICTED),
    );
    let unknown_b = fresh(
        "b",
        Entitlement::new("claude-b", EntitlementRules::UNRESTRICTED),
    );
    let pool = EntitlementPoolView::of(&[unknown_a.clone(), unknown_b.clone()]);
    for (term, expected) in [
        (entitlement_capacity(&unknown_a, &pool), "unknown"),
        (entitlement_reset_boundary(&unknown_a, &pool), "unknown"),
        (entitlement_throttling(&unknown_a, &pool), "unknown"),
        (entitlement_model_availability(&unknown_a, &pool), "unknown"),
    ] {
        assert_eq!(term.magnitude(), 0.0, "{}", term.evidence());
        assert!(
            term.evidence().contains(expected),
            "an unknown facet must say it is unknown: {}",
            term.evidence()
        );
    }

    // A pool of one configured entitlement: inert even with a facet read.
    let alone = fresh(
        "a",
        entitled("claude-a", Some(CapacityBand::Plenty), Some(60)),
    );
    let pool = EntitlementPoolView::of(std::slice::from_ref(&alone));
    assert!(!pool.offers_a_choice());
    for term in [
        entitlement_capacity(&alone, &pool),
        entitlement_reset_boundary(&alone, &pool),
        entitlement_throttling(&alone, &pool),
        entitlement_model_availability(&alone, &pool),
    ] {
        assert_eq!(term.magnitude(), 0.0, "{}", term.evidence());
        assert!(
            term.evidence().starts_with("inert"),
            "a pool of one offers no choice: {}",
            term.evidence()
        );
    }
}

/// **The preservation clause.** With a single configured entitlement in the
/// set, every candidate's total is exactly what it is with no entitlement
/// attached at all: the pool terms appear in the explanation and weigh
/// nothing, so a user with zero or one entitlement sees today's ranking
/// unchanged.
#[test]
fn a_single_configured_entitlement_leaves_every_total_unchanged() {
    let fixture = Fixture::new();
    let router = SessionRouter::new();

    let with_entitlement = vec![
        fresh(
            "first",
            entitled("claude-a", Some(CapacityBand::Plenty), Some(60)),
        ),
        Destination::fresh(
            "second",
            HARNESS,
            "profile",
            backend("the-same-provider"),
            None,
        ),
    ];
    let without = vec![
        Destination::fresh(
            "first",
            HARNESS,
            "profile",
            backend("the-same-provider"),
            None,
        ),
        Destination::fresh(
            "second",
            HARNESS,
            "profile",
            backend("the-same-provider"),
            None,
        ),
    ];

    let routed_with = fixture.choose(&router, &with_entitlement);
    let routed_without = fixture.choose(&router, &without);
    for ((destination, explanation), (_, baseline)) in routed_with
        .considered()
        .iter()
        .zip(routed_without.considered().iter())
    {
        assert_eq!(
            explanation.total(),
            baseline.total(),
            "`{}`: a pool of one must not move any total",
            destination.id()
        );
    }
}

// ===========================================================================
// Half two — the shipped binary: the axis in `glasshouse route`, the reset
// boundary through planted telemetry, and the launch bound to the pool's
// chosen account.
// ===========================================================================

const VAR_A: &str = "GLASSHOUSE_BROKER_KEY_A";
const VAR_B: &str = "GLASSHOUSE_BROKER_KEY_B";
const VALUE_A: &str = "planted-broker-value-a-56";
const VALUE_B: &str = "planted-broker-value-b-56";

/// The credential label the gateway would stamp for `VAR_A` —
/// `CredentialId::label`'s `provider/var` shape, `tests/entitlement_telemetry.rs`'s
/// own idiom.
const LABEL_A: &str = "alpha-probe/GLASSHOUSE_BROKER_KEY_A";

/// One provider, two accounts — the pool — plus the launch plumbing the
/// fake harness needs.
fn pool_config() -> String {
    format!(
        "[providers.alpha-probe]\ntemplate = \"openrouter\"\n\
         credential_env = [\"{VAR_A}\", \"{VAR_B}\"]\n\n\
         [profiles.alpha]\nharness = \"claude-code\"\n\
         expected_protocol = \"anthropic-messages\"\n\n\
         [profiles.alpha.backend]\nkind = \"direct-provider\"\n\
         provider = \"alpha-probe\"\n\n\
         [entitlements.claude-a]\nkind = \"claude\"\nvendor = \"claude\"\n\
         provider = \"alpha-probe\"\ncredential = {{ env = \"{VAR_A}\" }}\n\n\
         [entitlements.claude-b]\nkind = \"claude\"\nvendor = \"claude\"\n\
         provider = \"alpha-probe\"\ncredential = {{ env = \"{VAR_B}\" }}\n"
    )
}

/// Two providers, one account each — the cross-provider pool the
/// reset-boundary examples run on.
fn two_provider_config() -> String {
    format!(
        "[providers.prov-a]\ntemplate = \"openrouter\"\n\
         credential_env = [\"{VAR_A}\"]\n\n\
         [providers.prov-b]\ntemplate = \"openrouter\"\n\
         credential_env = [\"{VAR_B}\"]\n\n\
         [profiles.alpha]\nharness = \"claude-code\"\n\
         expected_protocol = \"anthropic-messages\"\n\n\
         [profiles.alpha.backend]\nkind = \"direct-provider\"\nprovider = \"prov-a\"\n\n\
         [profiles.beta]\nharness = \"claude-code\"\n\
         expected_protocol = \"anthropic-messages\"\n\n\
         [profiles.beta.backend]\nkind = \"direct-provider\"\nprovider = \"prov-b\"\n\n\
         [entitlements.acct-a]\nprovider = \"prov-a\"\ncredential = {{ env = \"{VAR_A}\" }}\n\n\
         [entitlements.acct-b]\nprovider = \"prov-b\"\ncredential = {{ env = \"{VAR_B}\" }}\n"
    )
}

/// The env var the shared fixture script reads its environment-dump
/// destination from, set per spawn by [`Binary::glasshouse`] rather than
/// baked into the script bytes — see [`shared_fixture`]'s doc for why.
const ENV_LOG_VAR: &str = "GLASSHOUSE_TEST_ENV_LOG";

struct Binary {
    _tmp: tempfile::TempDir,
    base: PathBuf,
    root: PathBuf,
    env_log: PathBuf,
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
        let env_log = base.join("env.log");
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
            env_log,
        }
    }

    /// A bootstrapped runtime over this fixture's own directories, for
    /// planting evidence-ledger rows. Opened, used and dropped by the
    /// caller before the binary runs.
    fn runtime(&self) -> Runtime {
        let cli = glasshouse::Cli::try_parse_from([
            "glasshouse",
            "--data-dir",
            self.base.join("data").to_str().unwrap(),
            "--config-dir",
            self.base.join("config").to_str().unwrap(),
        ])
        .unwrap();
        glasshouse::bootstrap(&cli, &self.root).unwrap()
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
            .env(VAR_A, VALUE_A)
            .env(VAR_B, VALUE_B)
            .env(ENV_LOG_VAR, &self.env_log)
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

    fn child_env(&self) -> String {
        std::fs::read_to_string(&self.env_log).expect("the fake harness dumped its environment")
    }
}

/// Write the shared fixture executable once per test binary instead of once
/// per test, so macOS Gatekeeper (`syspolicyd`/XProtect) validates it once
/// per run instead of once per test — see the project memory
/// `gatekeeper-scans-make-pty-fixtures-flaky` and GH-FIXTURE-REUSE /
/// GH-ARGV-LOG-HOIST. The env-dump destination used to be interpolated into
/// the script bytes, which made every call's content distinct; it is now
/// read from `ENV_LOG_VAR` at spawn time (set by [`Binary::glasshouse`]), so
/// the script bytes are constant and every call below collapses onto the one
/// file the first caller writes.
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
    // `export -p` is a shell builtin, so the empty PATH the fixture
    // launches under cannot break it — `tests/entitlement_pool.rs`'s idiom.
    shared_fixture(
        "fake-claude-code",
        &format!("#!/bin/sh\nexport -p > \"${ENV_LOG_VAR}\"\nexit 0\n"),
    )
}

#[cfg(windows)]
fn install_fake_harness(_bin_dir: &Path) -> PathBuf {
    shared_fixture(
        "fake-claude-code.cmd",
        &format!("@echo off\r\nset > \"%{ENV_LOG_VAR}%\"\r\nexit /b 0\r\n"),
    )
}

#[cfg(test)]
mod shared_fixture_proof {
    use super::{Binary, ENV_LOG_VAR, install_fake_harness, pool_config};

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
    /// reads is exactly the one [`Binary::glasshouse`] sets per spawn —
    /// proven by actually launching the shipped binary and reading the
    /// child's environment dump back, not by inspecting the script text
    /// alone.
    #[test]
    fn a_real_launch_through_the_shared_fixture_dumps_its_env_to_the_requested_log() {
        let binary = Binary::with_config(&pool_config());
        let out = binary.glasshouse(&["launch", "claude-code", "--headless", "--profile", "alpha"]);
        let said = Binary::both_streams(&out);
        assert!(out.status.success(), "launch must succeed:\n{said}");
        let child_env = binary.child_env();
        assert!(
            !child_env.is_empty(),
            "the shared, env-driven fixture must still dump this fixture's own \
             child environment into its own env log"
        );
    }
}

/// AnyRouter's real header shape: `remaining` of `limit` left, resetting
/// `reset` seconds after the observation.
fn headers(limit: &str, remaining: &str, reset: &str) -> RateLimitHeaders {
    RateLimitHeaders::read(vec![
        ("ratelimit-limit", limit),
        ("ratelimit-remaining", remaining),
        ("ratelimit-reset", reset),
    ])
}

fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}

/// **Line 1953 through the shipped binary.** One profile on a provider two
/// accounts back: `glasshouse route` ranks the same harness and profile
/// once per account, each candidate named `fresh:<harness>:<profile>@<name>`,
/// and every one of the five factors appears as a named term with `unknown`
/// spelled out where nothing measured. A build whose `routing_destinations`
/// drops the second allowed entitlement from the candidates fails here.
#[test]
fn route_ranks_the_same_profile_across_every_account_of_the_pool() {
    let binary = Binary::with_config(&pool_config());
    let out = binary.glasshouse(&["route"]);
    let said = Binary::both_streams(&out);
    assert!(out.status.success(), "{said}");

    assert!(
        said.contains("fresh:claude-code:alpha@claude-a"),
        "one candidate per account — claude-a's is missing:\n{said}"
    );
    assert!(
        said.contains("fresh:claude-code:alpha@claude-b"),
        "one candidate per account — claude-b's is missing:\n{said}"
    );
    for term in [
        "entitlement capacity",
        "reset boundary",
        "entitlement throttling",
        "entitlement model availability",
    ] {
        assert!(said.contains(term), "the `{term}` factor is named:\n{said}");
    }
    assert!(
        said.contains("remaining capacity is unknown"),
        "nothing was measured, and the capacity factor says unknown rather than a number:\n{said}"
    );
}

/// **Line 1967 through the shipped binary.** Two providers with one account
/// each, telemetry planted the way the gateway writes it: the tight account
/// resetting in 1h20m is burned; the same account resetting in 4 days is
/// preserved and the work routes to the healthy sibling.
#[test]
fn route_burns_a_tight_remainder_about_to_expire_and_preserves_a_distant_one() {
    // Example 1: prov-a at 25% (tight) resetting in 4800s — burn it.
    let burning = Binary::with_config(&two_provider_config());
    let now = now_unix();
    let quota = GatewayQuotaCache::at(burning.base.join("data").join("gateway-quota"));
    quota.store("prov-a", &headers("300", "75", "4800"), now);
    quota.store("prov-b", &headers("300", "165", "345600"), now);
    let out = burning.glasshouse(&["route"]);
    let said = Binary::both_streams(&out);
    assert!(out.status.success(), "{said}");
    assert!(
        said.contains("destination  fresh:claude-code:alpha on"),
        "the expiring remainder is burned — the tight account wins:\n{said}"
    );
    assert!(said.contains("burned aggressively"), "{said}");

    // Example 2: the same account resetting in 4 days — preserve it.
    let preserving = Binary::with_config(&two_provider_config());
    let now = now_unix();
    let quota = GatewayQuotaCache::at(preserving.base.join("data").join("gateway-quota"));
    quota.store("prov-a", &headers("300", "75", "345600"), now);
    quota.store("prov-b", &headers("300", "165", "345600"), now);
    let out = preserving.glasshouse(&["route"]);
    let said = Binary::both_streams(&out);
    assert!(out.status.success(), "{said}");
    assert!(
        said.contains("destination  fresh:claude-code:beta on"),
        "a low remainder with a far reset is preserved:\n{said}"
    );
    assert!(said.contains("preserved"), "{said}");
}

/// **Line 1969's routing half, through the acting path.** A launch under a
/// profile that pins no account is served by the pool's chosen candidate:
/// with claude-a's account throttled (56A-2's account-scoped narrowing),
/// the broker chooses claude-b, the announcement names it, and the harness
/// process is bound to it — claude-b's credential value stands in the
/// harness's own credential variable, claude-b's reference variable rides
/// along, and claude-a's variable and value appear nowhere in the child.
#[test]
fn a_launch_on_a_pooled_provider_is_bound_to_the_chosen_account() {
    let binary = Binary::with_config(&pool_config());

    // Two recent throttles against claude-a's own credential label — the
    // account-scoped reading that separates the two accounts of one
    // provider, planted as the gateway records it.
    {
        let runtime = binary.runtime();
        let ledger = EvidenceLedger::open(&runtime).unwrap();
        let now = now_unix();
        for age in [600, 300] {
            ledger
                .record(
                    NewObservation::new("alpha-probe", "some-model")
                        .with_route(Some("anthropic-messages"))
                        .with_harness(Some("claude-code"))
                        .with_quota_context(Some(LABEL_A))
                        .with_timing(Some(now - age), Some(now - age + 5))
                        .with_outcome(Outcome::Failed)
                        .with_failure_class(Some(FailureClass::Throttle)),
                    now - age + 5,
                )
                .unwrap();
        }
    }

    let out = binary.glasshouse(&["launch", "claude-code", "--headless", "--profile", "alpha"]);
    let said = Binary::both_streams(&out);
    assert!(out.status.success(), "the launch must succeed:\n{said}");
    assert!(
        said.contains("entitlement `claude-b`"),
        "the announcement names the pool's choice, not the first account:\n{said}"
    );
    assert!(
        !said.contains("entitlement `claude-a` ("),
        "the throttled account is not the one announced as serving:\n{said}"
    );

    let child = binary.child_env();
    assert!(
        child.contains(VAR_B) && child.contains(VALUE_B),
        "the serving account's own variable reaches the child:\n{child}"
    );
    assert!(
        !child.contains(VAR_A),
        "the sibling account's variable is scrubbed from the child:\n{child}"
    );
    assert!(
        !child.contains(VALUE_A),
        "the sibling account's value appears nowhere in the child:\n{child}"
    );
    assert!(
        child.contains(&format!("ANTHROPIC_AUTH_TOKEN={VALUE_B}"))
            || child.contains(&format!("ANTHROPIC_AUTH_TOKEN='{VALUE_B}'"))
            || child.contains(&format!("ANTHROPIC_AUTH_TOKEN=\"{VALUE_B}\"")),
        "the harness's own credential variable carries the CHOSEN account's value — the \
         process is bound to the pool's choice:\n{child}"
    );
}

// ===========================================================================
// Half three — Phase 56A step 4, capability map line 1972: the durable link
// (migration 22's `sessions.entitlement`) and the `glasshouse entitlements`
// view that makes it answerable.
//
// The writer's proof runs through the shipped binary for practice §35's
// reason: nothing that builds a `NewSession` by hand can fail on a build
// where `launch_session` stops filling the column, and that call site is the
// only thing the map line is about.
// ===========================================================================

/// A third account nothing has ever measured — the pool's untelemetered
/// entry, whose telemetry facets must every one read `unknown`.
///
/// A harness's own sign-in, which is `tests/entitlement_telemetry.rs`'s idiom
/// for this case and the honest one: it has no provider, so no quota cache
/// row and no `quota_context`-keyed throttle row can exist for it, and it
/// needs no credential reference of its own — which matters here, because map
/// line 1973 refuses two entries that name one credential and the loader says
/// so by name.
fn pool_config_with_an_unmeasured_account() -> String {
    format!(
        "{}\n[entitlements.spare]\nnative_harness = \"codex\"\n",
        pool_config()
    )
}

/// **The writer, through the acting path — line 1972's durable half.** The
/// broker chooses `claude-b` (claude-a's account is throttled), the launch
/// announces it, and the session record the launch wrote carries that same
/// name. The announcement and the column agree because they are the same
/// binding read twice, which is the property that stops the view from
/// lying about what served.
///
/// The `backend_resource` assertion is the point of the whole column: both
/// accounts are `direct-provider:alpha-probe`, so the coarse slug that
/// column has always held cannot tell them apart, and the new one can.
#[test]
fn the_session_record_names_the_entitlement_that_served_it() {
    let binary = Binary::with_config(&pool_config());

    {
        let runtime = binary.runtime();
        let ledger = EvidenceLedger::open(&runtime).unwrap();
        let now = now_unix();
        for age in [600, 300] {
            ledger
                .record(
                    NewObservation::new("alpha-probe", "some-model")
                        .with_route(Some("anthropic-messages"))
                        .with_harness(Some("claude-code"))
                        .with_quota_context(Some(LABEL_A))
                        .with_timing(Some(now - age), Some(now - age + 5))
                        .with_outcome(Outcome::Failed)
                        .with_failure_class(Some(FailureClass::Throttle)),
                    now - age + 5,
                )
                .unwrap();
        }
    }

    let out = binary.glasshouse(&["launch", "claude-code", "--headless", "--profile", "alpha"]);
    let said = Binary::both_streams(&out);
    assert!(out.status.success(), "the launch must succeed:\n{said}");
    assert!(
        said.contains("entitlement `claude-b`"),
        "the announcement names the pool's choice:\n{said}"
    );

    let runtime = binary.runtime();
    let sessions = glasshouse::session::ProjectSessions::open(&runtime).unwrap();
    let records = sessions.store().list().unwrap();
    assert_eq!(records.len(), 1, "one launch, one record: {records:#?}");
    let record = &records[0];
    assert_eq!(
        record.entitlement.as_deref(),
        Some("claude-b"),
        "the record carries the account that actually served, and it is the one \
         the launch announced:\n{said}"
    );
    assert_eq!(
        record.backend_resource.as_deref(),
        Some("direct-provider:alpha-probe"),
        "both accounts slug to this one value — which is exactly why the \
         entitlement column had to exist"
    );
}

/// **Two accounts of one vendor are two values where `backend_resource` is
/// one.** The case the coarse slug is structurally unable to express, stated
/// directly against the store: same harness, same resource, two accounts,
/// and the records differ on exactly one field.
///
/// Deliberately not a launch: this is a claim about what the *column* can
/// hold, and a launch would only be able to exercise one account per run.
#[test]
fn two_accounts_of_one_vendor_are_two_values_where_backend_resource_is_one() {
    use glasshouse::session::{NewSession, ProjectSessions};

    let binary = Binary::with_config(&pool_config());
    let runtime = binary.runtime();
    let sessions = ProjectSessions::open(&runtime).unwrap();
    let store = sessions.store();

    let first = store
        .create(
            NewSession::embedded("claude-code")
                .with_backend_resource(Some("direct-provider:alpha-probe".to_owned()))
                .with_entitlement(Some("claude-a".to_owned())),
        )
        .unwrap();
    let second = store
        .create(
            NewSession::embedded("claude-code")
                .with_backend_resource(Some("direct-provider:alpha-probe".to_owned()))
                .with_entitlement(Some("claude-b".to_owned())),
        )
        .unwrap();

    let read_first = store.get(&first.id).unwrap().unwrap();
    let read_second = store.get(&second.id).unwrap().unwrap();

    assert_eq!(
        read_first.backend_resource, read_second.backend_resource,
        "the coarse slug cannot tell the two accounts apart"
    );
    assert_eq!(read_first.entitlement.as_deref(), Some("claude-a"));
    assert_eq!(read_second.entitlement.as_deref(), Some("claude-b"));
    assert_ne!(
        read_first.entitlement, read_second.entitlement,
        "the entitlement column is what separates them"
    );
}

/// **The view names every configured entitlement, and an account nothing
/// measured reads `unknown`.** Three entries; the gateway's quota cache
/// holds a reading for `alpha-probe` only, so `spare` has no capacity, no
/// reset and no throttle reading of its own — and it must still appear, with
/// `unknown` spelled out on each. Never full, never empty, never a number:
/// 56A step 2's Cluster E discipline, now on the view line 1972 asks for.
#[test]
fn the_view_names_every_entitlement_and_spells_unknown_for_one_nothing_measured() {
    let binary = Binary::with_config(&pool_config_with_an_unmeasured_account());

    {
        let runtime = binary.runtime();
        let quota = GatewayQuotaCache::new(runtime.paths());
        let now = now_unix();
        quota.store(
            "alpha-probe",
            &RateLimitHeaders::read(vec![
                ("ratelimit-limit", "300"),
                ("ratelimit-remaining", "240"),
                ("ratelimit-reset", "600"),
            ]),
            now - 30,
        );
    }

    let out = binary.glasshouse(&["entitlements"]);
    let view = Binary::both_streams(&out);
    assert!(out.status.success(), "the view must render:\n{view}");

    for name in ["claude-a", "claude-b", "spare"] {
        assert!(
            view.contains(&format!("`{name}`")),
            "every configured entitlement appears, `{name}` included:\n{view}"
        );
    }

    let spare = view
        .lines()
        .skip_while(|line| !line.starts_with("`spare`"))
        .take(3)
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        spare.contains("capacity: unknown"),
        "an account nothing measured has no capacity — never full, never \
         empty, never a number:\n{spare}"
    );
    assert!(spare.contains("reset: unknown"), "and no reset:\n{spare}");
    assert!(
        spare.contains("throttling: unknown"),
        "and no throttle history — `unknown`, not `none observed`, which only \
         a resolver that actually looked may say:\n{spare}"
    );
    assert!(
        spare.contains("served: nothing recorded"),
        "the sessions table WAS read and holds no row for this account, which \
         is a measured zero rather than an unknown:\n{spare}"
    );

    // The measured account is the contrast: the same view, with a reading.
    let measured = view
        .lines()
        .skip_while(|line| !line.starts_with("`claude-a`"))
        .take(3)
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        !measured.contains("capacity: unknown"),
        "the account the cache does describe reads its band, not unknown:\n{measured}"
    );

    assert!(
        !view.contains(VALUE_A) && !view.contains(VALUE_B),
        "an entitlement is named, never its secret — no credential value may \
         reach this view:\n{view}"
    );
}

/// **The view answers *what it served*, from migration 22's column.** A
/// launch is recorded against `claude-b`, and the view attributes it to that
/// account and to no other. This is the facet the coarse `backend_resource`
/// could never have produced, and it is the reason the column exists.
#[test]
fn the_view_reports_what_each_entitlement_served() {
    let binary = Binary::with_config(&pool_config());

    {
        let runtime = binary.runtime();
        let ledger = EvidenceLedger::open(&runtime).unwrap();
        let now = now_unix();
        for age in [600, 300] {
            ledger
                .record(
                    NewObservation::new("alpha-probe", "some-model")
                        .with_route(Some("anthropic-messages"))
                        .with_harness(Some("claude-code"))
                        .with_quota_context(Some(LABEL_A))
                        .with_timing(Some(now - age), Some(now - age + 5))
                        .with_outcome(Outcome::Failed)
                        .with_failure_class(Some(FailureClass::Throttle)),
                    now - age + 5,
                )
                .unwrap();
        }
    }

    let launched =
        binary.glasshouse(&["launch", "claude-code", "--headless", "--profile", "alpha"]);
    assert!(
        launched.status.success(),
        "the launch must succeed:\n{}",
        Binary::both_streams(&launched)
    );

    let out = binary.glasshouse(&["entitlements"]);
    let view = Binary::both_streams(&out);
    assert!(out.status.success(), "the view must render:\n{view}");

    let served = view
        .lines()
        .skip_while(|line| !line.starts_with("`claude-b`"))
        .take(3)
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        served.contains("served: 1 session —"),
        "the account that served is credited with the session:\n{view}"
    );

    let idle = view
        .lines()
        .skip_while(|line| !line.starts_with("`claude-a`"))
        .take(3)
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        idle.contains("served: nothing recorded"),
        "and the account that did not serve is credited with nothing:\n{view}"
    );
}

/// **A session charged to an entry the configuration no longer describes is
/// still reported.** Recorded history does not vanish when a person edits a
/// file, and a view that silently dropped those rows would under-report what
/// the pool has served — while still, correctly, not inventing a row for an
/// account that is no longer configured.
#[test]
fn the_view_still_reports_sessions_charged_to_an_entry_no_longer_configured() {
    use glasshouse::session::{NewSession, ProjectSessions};

    let binary = Binary::with_config(&pool_config());
    {
        let runtime = binary.runtime();
        let sessions = ProjectSessions::open(&runtime).unwrap();
        sessions
            .store()
            .create(
                NewSession::embedded("claude-code")
                    .with_entitlement(Some("retired-account".to_owned())),
            )
            .unwrap();
    }

    let out = binary.glasshouse(&["entitlements"]);
    let view = Binary::both_streams(&out);
    assert!(out.status.success(), "the view must render:\n{view}");
    assert!(
        view.contains("Also served, by entries no longer configured: `retired-account`"),
        "the recorded row is reported rather than dropped:\n{view}"
    );
    let configured_rows: Vec<&str> = view.lines().filter(|line| line.starts_with('`')).collect();
    assert_eq!(
        configured_rows.len(),
        2,
        "the two configured entries get rows and the retired one does not — it \
         is reported as history, never promoted back into the pool:\n{view}"
    );
}

// ===========================================================================
// Half four — Phase 56A step 5, capability map lines 1970, 1971 and 1974:
// the tier-preserving fallback across the pool, the user's per-entitlement
// rules the broker may never exceed, and the end-to-end cover.
//
// The order itself (`entitlement_fallback`) is exercised directly, the way
// this file already exercises the five score terms directly — it is one
// pure function over a ranked list, and every arm of the ruling's order is
// reachable that way. Beside it, and load-bearing for practice §35, two
// tests drive the whole of `SessionRouter::choose`: nothing above them can
// fail on a build where `choose` stops calling the reselection at all.
// ===========================================================================

/// An account with a backing, a capacity band and a throttle count — the
/// three facts map line 1970's order and its trigger read.
fn account(
    name: &str,
    source: EntitlementSource,
    band: Option<CapacityBand>,
    throttles: usize,
) -> Entitlement {
    Entitlement::new(name, EntitlementRules::UNRESTRICTED)
        .with_source(source)
        .with_capacity(band, None)
        .with_throttling(Some(EntitlementThrottleFacet::new(throttles, true)))
}

/// A fresh destination on a **named** model other than the one `fresh`
/// carries — the "different model" half of the narrowing proof.
fn fresh_on_model(id: &str, model: &str, entitlement: Entitlement) -> Destination {
    Destination::fresh(
        id,
        HARNESS,
        "profile",
        Backend::new(
            "the-same-provider",
            PROTOCOL,
            AssignedModel::named(model),
            CredentialId::new(
                "the-same-provider",
                SecretRef::Environment {
                    var: "THE_SAME_PROVIDER_KEY".to_owned(),
                },
            ),
            Cost::Metered,
            ToolSemantics::Verified,
        ),
        None,
    )
    .with_entitlement(Some(entitlement))
}

/// [`fresh_on_model`], with a user-assigned capability tier attached —
/// Phase 34F's axis, plugged into the fallback exactly the way
/// `main.rs::routing_destinations` plugs it in for the shipped binary: one
/// value, attached at construction, that `same_capability_tier` later
/// compares.
fn fresh_on_model_with_tier(
    id: &str,
    model: &str,
    tier: WorkloadTier,
    entitlement: Entitlement,
) -> Destination {
    fresh_on_model(id, model, entitlement).with_capability_tier(Some(tier))
}

fn ranked(destinations: &[Destination]) -> Vec<&Destination> {
    destinations.iter().collect()
}

/// **Line 1970's order, the packet's own acceptance test.** An exhausted
/// subscription is the ranking's winner; a same-model second subscription
/// and a same-model API-credit account are both available and healthy, and
/// the **API one is ranked above the subscription on purpose** — so a build
/// that simply took the best-scoring healthy candidate, rather than walking
/// `FallbackStep::ORDER`, chooses the API account and fails here. Remove the
/// subscription and the API account is taken, which is the ruling's own
/// completion of the order: *"If subscription model of capability is not
/// available switch to api one - if available."*
#[test]
fn the_fallback_order_prefers_a_subscription_and_takes_api_credits_only_when_it_must() {
    let exhausted = fresh(
        "d-a",
        account(
            "claude-a",
            EntitlementSource::Subscription,
            Some(CapacityBand::Exhausted),
            0,
        ),
    );
    let api = fresh(
        "d-api",
        account(
            "openrouter",
            EntitlementSource::ApiCredits,
            Some(CapacityBand::Plenty),
            0,
        ),
    );
    let subscription = fresh(
        "d-b",
        account(
            "claude-b",
            EntitlementSource::Subscription,
            Some(CapacityBand::Healthy),
            0,
        ),
    );

    let all = vec![exhausted.clone(), api.clone(), subscription.clone()];
    let pool = EntitlementPoolView::of(&all);
    let (index, record) = entitlement_fallback(&ranked(&all), 0, &pool)
        .expect("an exhausted account with a healthy same-model sibling falls back");
    assert_eq!(
        index, 2,
        "the subscription is taken even though the API account ranks above it"
    );
    assert_eq!(record.from(), "claude-a");
    assert_eq!(record.to(), "claude-b");
    assert_eq!(record.from_destination(), "d-a");
    assert_eq!(record.to_destination(), "d-b");
    assert_eq!(record.reason(), FallbackReason::Exhausted);
    assert_eq!(record.step(), FallbackStep::SubscriptionSameModel);

    // Remove the subscription: the API-credit account is step three, and now
    // it is the one that matches.
    let without = vec![exhausted.clone(), api.clone()];
    let pool = EntitlementPoolView::of(&without);
    let (index, record) = entitlement_fallback(&ranked(&without), 0, &pool)
        .expect("with no subscription left, API credits serve the same model");
    assert_eq!(index, 1);
    assert_eq!(record.to(), "openrouter");
    assert_eq!(record.step(), FallbackStep::ApiCreditsSameModel);
}

/// **Line 1970's tier-preserving constraint — the narrowing half.** The same
/// exhausted subscription, with a healthy sibling subscription that serves a
/// **different** model, and neither destination carries an attached
/// capability tier — the plain state every destination the router sees
/// arrives in until something assigns one. `same_capability_tier` answers
/// *unknown* for two unattached values, exactly as it does for two attached
/// values nobody has ranked the same. Unknown does not widen the order:
/// there is **no fallback at all**, which is the ruling's own direction —
/// *"You can't put a fable 5 task and switch it to a nemotron v3"*, and a
/// fallback that silently downgrades *"is worse than a refusal, because the
/// work continues and looks fine"*. See
/// `a_shared_user_assigned_capability_tier_reaches_the_fallbacks_tier_step`
/// beside this test for the positive case, now that Phase 34F's axis has
/// landed.
#[test]
fn an_unknown_capability_tier_never_widens_the_fallback() {
    let exhausted = fresh(
        "d-a",
        account(
            "claude-a",
            EntitlementSource::Subscription,
            Some(CapacityBand::Exhausted),
            0,
        ),
    );
    let other_model = fresh_on_model(
        "d-c",
        "a-different-model",
        account(
            "claude-c",
            EntitlementSource::Subscription,
            Some(CapacityBand::Plenty),
            0,
        ),
    );
    let all = vec![exhausted, other_model];
    let pool = EntitlementPoolView::of(&all);
    assert!(
        entitlement_fallback(&ranked(&all), 0, &pool).is_none(),
        "a model no axis has ranked beside this one is not established to be the same tier, \
         and an unestablished tier is not a fallback"
    );
}

/// **Line 1970's tier-preserving constraint — the positive half, now that
/// Phase 34F's axis has landed.** The same exhausted subscription, with a
/// healthy sibling subscription that serves a genuinely **different** model
/// the user has assigned the **same** capability tier.
/// `same_capability_tier` now answers `Same` for the two attached values, and
/// `FallbackStep::SubscriptionSameTier` — present in `FallbackStep::ORDER`
/// since batch 70 but unreachable until this axis existed — fires.
#[test]
fn a_shared_user_assigned_capability_tier_reaches_the_fallbacks_tier_step() {
    let exhausted = fresh_on_model_with_tier(
        "d-a",
        "the-same-model",
        WorkloadTier::Standard,
        account(
            "claude-a",
            EntitlementSource::Subscription,
            Some(CapacityBand::Exhausted),
            0,
        ),
    );
    let same_tier_other_model = fresh_on_model_with_tier(
        "d-c",
        "a-different-model",
        WorkloadTier::Standard,
        account(
            "claude-c",
            EntitlementSource::Subscription,
            Some(CapacityBand::Plenty),
            0,
        ),
    );

    let all = vec![exhausted, same_tier_other_model];
    let pool = EntitlementPoolView::of(&all);
    let (index, record) = entitlement_fallback(&ranked(&all), 0, &pool).expect(
        "two models the user assigned the same capability tier must reach the fallback's tier \
         step",
    );
    assert_eq!(index, 1);
    assert_eq!(record.from(), "claude-a");
    assert_eq!(record.to(), "claude-c");
    assert_eq!(
        record.step(),
        FallbackStep::SubscriptionSameTier,
        "the step must name the tier match, not merely find a candidate"
    );
}

/// **A fallback never lands somewhere in the same state.** The first two
/// siblings are themselves exhausted and throttled; only the third is
/// healthy, and it is the one taken. A build that took the first candidate
/// matching the step's backing would move the work onto an account with
/// nothing left.
#[test]
fn a_fallback_never_lands_on_an_account_in_the_same_state() {
    let all = vec![
        fresh(
            "d-a",
            account(
                "claude-a",
                EntitlementSource::Subscription,
                Some(CapacityBand::Exhausted),
                0,
            ),
        ),
        fresh(
            "d-b",
            account(
                "claude-b",
                EntitlementSource::Subscription,
                Some(CapacityBand::Exhausted),
                0,
            ),
        ),
        fresh(
            "d-c",
            account(
                "claude-c",
                EntitlementSource::Subscription,
                Some(CapacityBand::Plenty),
                2,
            ),
        ),
        fresh(
            "d-d",
            account(
                "claude-d",
                EntitlementSource::Subscription,
                Some(CapacityBand::Tight),
                0,
            ),
        ),
    ];
    let pool = EntitlementPoolView::of(&all);
    let (index, record) =
        entitlement_fallback(&ranked(&all), 0, &pool).expect("one healthy sibling remains");
    assert_eq!(index, 3, "the exhausted and the throttled are both skipped");
    assert_eq!(record.to(), "claude-d");
}

/// **An entitlement that names no backing is never a fallback target.** Its
/// own documentation is *listed, never matched, never charged*, and an order
/// over subscriptions and API credits has no step it belongs to — so a pool
/// whose only healthy account is unstated produces no fallback rather than
/// charging one Glasshouse cannot say who pays for.
#[test]
fn an_entitlement_with_no_backing_stated_is_never_a_fallback_target() {
    let all = vec![
        fresh(
            "d-a",
            account(
                "claude-a",
                EntitlementSource::Subscription,
                Some(CapacityBand::Exhausted),
                0,
            ),
        ),
        fresh(
            "d-x",
            account(
                "someday",
                EntitlementSource::Unstated,
                Some(CapacityBand::Plenty),
                0,
            ),
        ),
    ];
    let pool = EntitlementPoolView::of(&all);
    assert!(entitlement_fallback(&ranked(&all), 0, &pool).is_none());
}

/// **The untriggered case, and the pool of one.** A healthy winner produces
/// no fallback, and neither does a set with a single configured entitlement
/// however bad its state — the same preservation gate every pool term
/// checks. Zero fallbacks is `None`, never an empty record.
#[test]
fn an_untriggered_selection_and_a_pool_of_one_never_fall_back() {
    let healthy = vec![
        fresh(
            "d-a",
            account(
                "claude-a",
                EntitlementSource::Subscription,
                Some(CapacityBand::Plenty),
                0,
            ),
        ),
        fresh(
            "d-b",
            account(
                "claude-b",
                EntitlementSource::Subscription,
                Some(CapacityBand::Healthy),
                0,
            ),
        ),
    ];
    let pool = EntitlementPoolView::of(&healthy);
    assert!(entitlement_fallback(&ranked(&healthy), 0, &pool).is_none());

    let alone = vec![fresh(
        "d-a",
        account(
            "claude-a",
            EntitlementSource::Subscription,
            Some(CapacityBand::Exhausted),
            0,
        ),
    )];
    let pool = EntitlementPoolView::of(&alone);
    assert!(entitlement_fallback(&ranked(&alone), 0, &pool).is_none());
}

/// **Line 1970 through `SessionRouter::choose` — practice §35's caller.**
/// `claude-a` is in the `plenty` band and was throttled once, `claude-b` is
/// in the `tight` band and was not: the score prefers `claude-a`
/// (+0.3 − 0.2 against −0.15), so the ranking's own winner is a throttled
/// account, and the fallback then moves the work to its healthy sibling.
///
/// The assertion on `considered()` is what makes this a *post-ranking*
/// reselection rather than a filter: `claude-a` is still the top of the
/// ranking, with its score and its evidence intact — design decision 1,
/// *additive, never a filter*. A build where `choose` stops calling the
/// reselection fails here and nowhere above.
#[test]
fn a_throttled_winner_is_fallen_back_from_through_choose() {
    let fixture = Fixture::new();
    let destinations = vec![
        fresh(
            "d-a",
            account(
                "claude-a",
                EntitlementSource::Subscription,
                Some(CapacityBand::Plenty),
                1,
            ),
        ),
        fresh(
            "d-b",
            account(
                "claude-b",
                EntitlementSource::Subscription,
                Some(CapacityBand::Tight),
                0,
            ),
        ),
    ];
    let routed = fixture.choose(&SessionRouter::new(), &destinations);

    assert_eq!(
        routed.considered()[0].0.id(),
        "d-a",
        "the ranking is untouched — the throttled account still scores best"
    );
    assert_eq!(
        routed.chosen().id(),
        "d-b",
        "and the work still goes to the healthy account"
    );
    let fallback = routed
        .fallback()
        .expect("the decision records the fallback it made");
    assert_eq!(fallback.from(), "claude-a");
    assert_eq!(fallback.to(), "claude-b");
    assert_eq!(fallback.reason(), FallbackReason::Throttled);
    assert_eq!(fallback.step(), FallbackStep::SubscriptionSameModel);

    let report = routed.render();
    assert!(
        report.contains("fallback     entitlement `claude-a` is throttled"),
        "a person reads the fallback as a heading, with both accounts and the reason:\n{report}"
    );
    assert!(
        report.contains("another subscription serving the same model"),
        "and which step of line 1970's order matched:\n{report}"
    );
}

/// **Zero fallbacks leaves zero records.** Two healthy accounts: the
/// decision is the one this router made before line 1970 existed, and
/// nothing anywhere says "fallback".
#[test]
fn zero_fallbacks_leave_no_record_and_say_nothing() {
    let fixture = Fixture::new();
    let destinations = vec![
        fresh(
            "d-a",
            account(
                "claude-a",
                EntitlementSource::Subscription,
                Some(CapacityBand::Plenty),
                0,
            ),
        ),
        fresh(
            "d-b",
            account(
                "claude-b",
                EntitlementSource::Subscription,
                Some(CapacityBand::Tight),
                0,
            ),
        ),
    ];
    let routed = fixture.choose(&SessionRouter::new(), &destinations);
    assert_eq!(routed.chosen().id(), "d-a");
    assert!(routed.fallback().is_none());
    assert!(
        !routed.render_overview().contains("fallback"),
        "{}",
        routed.render_overview()
    );
}

/// **An account the user named exactly is never moved.** `--to
/// d-a` pins the account itself, and an override *"may overrule a ranking
/// and not a fact about what can serve"* — this is neither, it is Glasshouse
/// preferring one admissible account over another, which the person has
/// already done. The throttle is still in their explanation.
#[test]
fn an_exact_account_override_is_never_moved_by_the_fallback() {
    let fixture = Fixture::new();
    let destinations = vec![
        fresh(
            "d-b",
            account(
                "claude-b",
                EntitlementSource::Subscription,
                Some(CapacityBand::Plenty),
                0,
            ),
        ),
        fresh(
            "d-a",
            account(
                "claude-a",
                EntitlementSource::Subscription,
                Some(CapacityBand::Tight),
                2,
            ),
        ),
    ];
    let routed = fixture.choose(
        &SessionRouter::with_override(RoutingOverride::to("d-a")),
        &destinations,
    );
    assert_eq!(routed.chosen().id(), "d-a");
    assert!(
        routed.fallback().is_none(),
        "the person chose this account; the fallback does not overrule them"
    );
    assert!(
        routed
            .render()
            .contains("2 recent throttles recorded against `claude-a`"),
        "and they are told what they chose:\n{}",
        routed.render()
    );
}

/// **1974(d): the reset boundary, and selection returning with the
/// capacity.** The same pool read twice: while `claude-a` reads exhausted
/// the work is on `claude-b`, and once the window has turned and `claude-a`
/// reads `plenty` again the selection returns to it with no fallback left to
/// record. The second half is the one that matters — a build that remembered
/// the fallback, or that kept steering away from a recovered account, fails
/// on it.
#[test]
fn capacity_returning_at_the_reset_boundary_brings_the_account_back() {
    let fixture = Fixture::new();
    let router = SessionRouter::new();

    let before = vec![
        fresh(
            "d-a",
            account(
                "claude-a",
                EntitlementSource::Subscription,
                Some(CapacityBand::Exhausted),
                0,
            ),
        ),
        fresh(
            "d-b",
            account(
                "claude-b",
                EntitlementSource::Subscription,
                Some(CapacityBand::Healthy),
                0,
            ),
        ),
    ];
    let routed = fixture.choose(&router, &before);
    assert_eq!(
        routed.chosen().id(),
        "d-b",
        "an exhausted account does not serve the next turn"
    );

    let after = vec![
        fresh(
            "d-a",
            account(
                "claude-a",
                EntitlementSource::Subscription,
                Some(CapacityBand::Plenty),
                0,
            ),
        ),
        fresh(
            "d-b",
            account(
                "claude-b",
                EntitlementSource::Subscription,
                Some(CapacityBand::Healthy),
                0,
            ),
        ),
    ];
    let routed = fixture.choose(&router, &after);
    assert_eq!(
        routed.chosen().id(),
        "d-a",
        "the allowance reset, and the account is selectable again"
    );
    assert!(routed.fallback().is_none());
}

// ---------------------------------------------------------------------------
// Line 1971 — the user's per-entitlement rules, and that no amount of
// fallback pressure gets past one.
//
// Every one of these is the same shape, and the shape is the point: the
// ranking's winner is throttled, so line 1970 *wants* to move the work, and
// the only account it could move to is one a rule removed. The fallback
// reselects over `Routed::considered()` — the already-gated list — so there
// is no path from the reselection to a refused candidate at all. *An
// exhausted pool does not license exceeding a rule*, and it does not do so
// structurally rather than by a second check that could be forgotten.
// ---------------------------------------------------------------------------

/// The two-account set every rule test below drives: `claude-a` throttled
/// and in the plenty band so the ranking picks it and line 1970 wants to
/// move the work off it, and `claude-b` — the only place it could move to —
/// healthy but carrying `rules`.
fn under_fallback_pressure(rules: EntitlementRules) -> Vec<Destination> {
    vec![
        fresh(
            "d-a",
            account(
                "claude-a",
                EntitlementSource::Subscription,
                Some(CapacityBand::Plenty),
                1,
            ),
        ),
        fresh(
            "d-b",
            Entitlement::new("claude-b", rules)
                .with_source(EntitlementSource::Subscription)
                .with_capacity(Some(CapacityBand::Tight), None)
                .with_throttling(Some(EntitlementThrottleFacet::new(0, true))),
        ),
    ]
}

/// What the gate refused, and by which rule.
fn rule_refusal(routed: &Routed) -> Option<(String, EntitlementRefusal)> {
    routed
        .rejected()
        .iter()
        .find_map(|(_, constraint)| match constraint {
            HardConstraint::Entitlement {
                entitlement,
                refused,
            } => Some((entitlement.clone(), refused.clone())),
            _ => None,
        })
}

/// **The harness rule holds under fallback pressure.** `claude-b` does not
/// serve this harness, so the gate removed it before the ranking existed —
/// the work stays on the throttled account rather than being charged to one
/// the user's rule forbids, and the rejection names the entitlement.
#[test]
fn a_harness_rule_holds_under_fallback_pressure() {
    let fixture = Fixture::new();
    let destinations =
        under_fallback_pressure(EntitlementRules::UNRESTRICTED.deny_harnesses([HARNESS]));
    let routed = fixture.choose(&SessionRouter::new(), &destinations);

    assert_eq!(routed.chosen().id(), "d-a");
    assert!(
        routed.fallback().is_none(),
        "an exhausted or throttled account does not license exceeding a rule"
    );
    assert_eq!(
        rule_refusal(&routed),
        Some(("claude-b".to_owned(), EntitlementRefusal::Harness(HARNESS))),
        "and the refusal names the entitlement and the axis:\n{}",
        routed.render_overview()
    );
}

/// **The tier rule holds under fallback pressure.** The same set with a
/// stated task tier `claude-b`'s rule denies.
#[test]
fn a_tier_rule_holds_under_fallback_pressure() {
    let fixture = Fixture::new();
    let destinations =
        under_fallback_pressure(EntitlementRules::UNRESTRICTED.deny_tiers([WorkloadTier::Heavy]));
    let inputs = RouterInputs {
        requirements: TaskRequirements {
            minimum_tier: Some(WorkloadTier::Heavy),
            ..TaskRequirements::default()
        },
        ..fixture.inputs()
    };
    let routed = SessionRouter::new()
        .choose(RoutingMoment::SessionStart, None, &destinations, &inputs)
        .expect("the throttled account is still eligible");

    assert_eq!(routed.chosen().id(), "d-a");
    assert!(routed.fallback().is_none());
    assert_eq!(
        rule_refusal(&routed),
        Some((
            "claude-b".to_owned(),
            EntitlementRefusal::Tier(WorkloadTier::Heavy)
        )),
        "{}",
        routed.render_overview()
    );
}

/// **The spend ceiling holds under fallback pressure.** `claude-b` is
/// healthy and would be step one of the order, and it is over the ceiling
/// the user wrote for it — so the gate removed it and the work stays where
/// it is. The refusal carries both numbers, because *"over its ceiling"* is
/// only inspectable next to which ceiling and how much was seen.
#[test]
fn a_spend_ceiling_holds_under_fallback_pressure() {
    let fixture = Fixture::new();
    let mut destinations = under_fallback_pressure(
        EntitlementRules::UNRESTRICTED.with_spend_ceiling_tokens(Some(1_000)),
    );
    destinations[1] = fresh(
        "d-b",
        Entitlement::new(
            "claude-b",
            EntitlementRules::UNRESTRICTED.with_spend_ceiling_tokens(Some(1_000)),
        )
        .with_source(EntitlementSource::Subscription)
        .with_capacity(Some(CapacityBand::Tight), None)
        .with_throttling(Some(EntitlementThrottleFacet::new(0, true)))
        .with_spend(Some(EntitlementSpendFacet::new(1_200, true))),
    );
    let routed = fixture.choose(&SessionRouter::new(), &destinations);

    assert_eq!(routed.chosen().id(), "d-a");
    assert!(routed.fallback().is_none());
    assert_eq!(
        rule_refusal(&routed),
        Some((
            "claude-b".to_owned(),
            EntitlementRefusal::SpendCeiling {
                ceiling_tokens: 1_000,
                observed_tokens: 1_200,
            }
        ))
    );
    assert!(
        routed
            .render_overview()
            .contains("its spend ceiling of 1000 tokens is reached (1200 observed)"),
        "the person reads the rule they wrote:\n{}",
        routed.render_overview()
    );
}

/// **A spend ceiling refuses only against an established reading.** The same
/// ceiling with nothing measured admits the account and it becomes the
/// fallback's target — *"nobody has said"* is not *"cannot"*, and the
/// alternative would refuse every account forever on a build whose ledger is
/// empty. The direction the rule does guarantee is the other one, which the
/// test above pins.
#[test]
fn a_spend_ceiling_whose_spend_nothing_measured_refuses_nothing() {
    let fixture = Fixture::new();
    let destinations = under_fallback_pressure(
        EntitlementRules::UNRESTRICTED.with_spend_ceiling_tokens(Some(1_000)),
    );
    let routed = fixture.choose(&SessionRouter::new(), &destinations);

    assert!(routed.rejected().is_empty(), "{}", routed.render_overview());
    assert_eq!(
        routed.considered()[0].0.id(),
        "d-a",
        "the throttled account is still the ranking's winner"
    );
    assert_eq!(routed.chosen().id(), "d-b");
    assert_eq!(
        routed.fallback().map(|f| f.to().to_owned()),
        Some("claude-b".to_owned())
    );
}

/// **The job-kind axis, which no session router asks and one router does.**
/// A session has no job kind (`EntitlementRefusal`'s own documentation), so
/// line 1971's third axis is enforced where a job kind exists:
/// `Entitlement::job_constraint`, the question
/// `disposable::DisposableRouting` asks of every candidate that carries an
/// entitlement. It refuses by the same named constraint the other three
/// axes do.
#[test]
fn a_job_kind_rule_refuses_by_name_on_the_router_that_has_a_job_kind() {
    let entitlement = Entitlement::new(
        "claude-b",
        EntitlementRules::UNRESTRICTED.deny_job_kinds([JobKind::MemoryExtraction]),
    );
    assert_eq!(
        entitlement.job_constraint(JobKind::MemoryExtraction),
        Err(HardConstraint::Entitlement {
            entitlement: "claude-b".to_owned(),
            refused: EntitlementRefusal::JobKind(JobKind::MemoryExtraction),
        })
    );
    assert_eq!(entitlement.job_constraint(JobKind::Evaluation), Ok(()));
}

// ---------------------------------------------------------------------------
// The backing becomes routing-significant, and the kind still is not — the
// ruling's work item 1, and the invariant it was careful not to break.
// ---------------------------------------------------------------------------

/// **Map line 1970's work item 1.** `ResolvedEntitlement::to_routing` used
/// to render the backing as a human string and carry nothing the router
/// could branch on; now it carries the discriminant, derived from the
/// loader-enforced backing.
///
/// The second half is the REQUIRED BEHAVIOR of this package, asserted rather
/// than assumed: `EntitlementKind` stays **routing-insignificant**. Two
/// entries identical but for their `kind` — one of them lying outright,
/// calling a harness's own sign-in an `api-key` — produce the *same*
/// `routing::Entitlement`, source included. A wrong `kind` still
/// misdescribes an entitlement and still never misroutes one.
#[test]
fn the_backing_becomes_the_routing_source_and_the_kind_stays_routing_insignificant() {
    use glasshouse::config::{EntitlementConfig, EntitlementKind, Layer};

    let routing_of = |config: &EntitlementConfig| {
        config
            .to_resolved("an-account", Layer::User)
            .expect("the entry resolves")
            .to_routing()
    };

    let mut native = EntitlementConfig::default();
    native.set_native_harness(Some(IntegrationId::ClaudeCode));
    assert_eq!(
        routing_of(&native).source(),
        EntitlementSource::Subscription,
        "a harness's own sign-in authenticates through the harness — that is a subscription"
    );

    let mut api = EntitlementConfig::default();
    api.set_provider(Some("alpha-probe".to_owned()));
    assert_eq!(
        routing_of(&api).source(),
        EntitlementSource::ApiCredits,
        "a `[providers.<name>]` backing carries a credential of its own — that is an API key"
    );

    assert_eq!(
        routing_of(&EntitlementConfig::default()).source(),
        EntitlementSource::Unstated,
        "an entry naming neither is listed, never matched, never charged"
    );

    let mut mislabelled = native.clone();
    mislabelled.set_kind(Some(EntitlementKind::ApiKey));
    let mut labelled = native.clone();
    labelled.set_kind(Some(EntitlementKind::Claude));
    assert_eq!(
        routing_of(&mislabelled),
        routing_of(&labelled),
        "the router's value does not depend on `kind` — including when `kind` is wrong"
    );
    assert_eq!(
        routing_of(&mislabelled),
        routing_of(&native),
        "and stating no kind at all is the same value again"
    );
}

/// **Line 1971's fourth axis reaches the rules value from the user's own
/// table.** `spend_ceiling_tokens` is accepted by the loader (the table is
/// `deny_unknown_fields`, so an unrecognised key is a parse error rather
/// than a silently ignored ceiling), round-trips, and becomes the one thing
/// the gate reads.
#[test]
fn a_spend_ceiling_round_trips_from_the_entitlements_table_into_the_rules() {
    use glasshouse::config::EntitlementConfig;

    let parsed: EntitlementConfig = toml::from_str(
        "provider = \"alpha-probe\"\nspend_ceiling_tokens = 250000\ndeny_harnesses = [\"codex\"]\n",
    )
    .expect("the ceiling is a recognised key on an entitlement entry");
    assert_eq!(parsed.spend_ceiling_tokens(), Some(250_000));
    assert_eq!(parsed.rules().spend_ceiling_tokens(), Some(250_000));

    let written = toml::to_string(&parsed).expect("serialises");
    assert!(
        written.contains("spend_ceiling_tokens = 250000"),
        "the ceiling survives a write:\n{written}"
    );

    let stated_none: EntitlementConfig =
        toml::from_str("provider = \"alpha-probe\"\n").expect("parses");
    assert_eq!(
        stated_none.rules().spend_ceiling_tokens(),
        None,
        "no ceiling stated is `None` and never a zero"
    );
}

// ---------------------------------------------------------------------------
// Line 1974 through the shipped binary — practice §35 again, and the reason
// this file already keeps a binary half: nothing above here can fail on a
// build where the ledger's rows never reach the router, where
// `to_routing` stops carrying the backing discriminant, or where the launch
// path stops building the pool at all.
// ---------------------------------------------------------------------------

/// Two providers, one account each, **both serving one named model** — so
/// the fallback's same-model step has two candidates it can establish are
/// the same model, which a harness-picked default across two profiles is
/// not.
fn two_provider_config_on_one_model() -> String {
    let config = two_provider_config()
        .replace(
            "[profiles.alpha]\nharness = \"claude-code\"\n",
            "[profiles.alpha]\nharness = \"claude-code\"\nmodel = \"shared-model\"\n",
        )
        .replace(
            "[profiles.beta]\nharness = \"claude-code\"\n",
            "[profiles.beta]\nharness = \"claude-code\"\nmodel = \"shared-model\"\n",
        );
    // The two profiles must genuinely state one model: without it the
    // fallback's same-model step has nothing it can establish, and a silently
    // no-op `replace` would turn this fixture into a different test.
    assert_eq!(
        config.matches("model = \"shared-model\"").count(),
        2,
        "both profiles state the shared model:\n{config}"
    );
    config
}

/// **Line 1970 end to end.** `acct-a` has plenty of quota and one recorded
/// throttle, `acct-b` is tight — so the five-factor score ranks `acct-a`
/// first and the ranking's own winner is a throttled account. `glasshouse
/// route` shows the ranking unchanged, the work moved to `acct-b`, and the
/// reason and the step of line 1970's order named in one line a person
/// reads.
#[test]
fn route_falls_back_from_a_throttled_winner_and_names_the_reason() {
    let binary = Binary::with_config(&two_provider_config_on_one_model());
    let now = now_unix();
    let quota = GatewayQuotaCache::at(binary.base.join("data").join("gateway-quota"));
    // prov-a nearly untouched (plenty), prov-b at 25% with a distant reset
    // (tight, and preserved rather than burned).
    quota.store("prov-a", &headers("300", "290", "345600"), now);
    quota.store("prov-b", &headers("300", "75", "345600"), now);
    {
        let runtime = binary.runtime();
        let ledger = EvidenceLedger::open(&runtime).unwrap();
        ledger
            .record(
                NewObservation::new("prov-a", "shared-model")
                    .with_route(Some("anthropic-messages"))
                    .with_harness(Some("claude-code"))
                    .with_quota_context(Some(format!("prov-a/{VAR_A}")))
                    .with_timing(Some(now - 300), Some(now - 295))
                    .with_outcome(Outcome::Failed)
                    .with_failure_class(Some(FailureClass::Throttle)),
                now - 295,
            )
            .unwrap();
    }

    let out = binary.glasshouse(&["route"]);
    let said = Binary::both_streams(&out);
    assert!(out.status.success(), "{said}");
    assert!(
        said.contains("destination  fresh:claude-code:beta"),
        "the work moved to the healthy account:\n{said}"
    );
    assert!(
        said.contains("fallback     entitlement `acct-a` is throttled"),
        "the fallback is named with the account it left and why:\n{said}"
    );
    assert!(
        said.contains("the work moved to `acct-b`"),
        "and with the account it went to:\n{said}"
    );
    assert!(
        said.contains("an API-credit account serving the same model"),
        "and with the step of line 1970's order that matched:\n{said}"
    );
}

/// **Line 1970's durable record, on the path that acts.** `glasshouse
/// route` renders a fallback and records nothing (the test above); the same
/// fallback on a real launch writes ONE evidence-ledger row whose `purpose`
/// is the trigger and whose `quota_context` is the account the work left —
/// read back here through the same purpose-bucketed aggregation the
/// overhead report uses, so a row written under no purpose (or the wrong
/// one) fails this test rather than hiding in the unstamped bucket.
#[test]
fn a_launch_that_falls_back_records_the_fallback_with_its_reason() {
    let binary = Binary::with_config(&two_provider_config_on_one_model());
    let now = now_unix();
    let quota = GatewayQuotaCache::at(binary.base.join("data").join("gateway-quota"));
    quota.store("prov-a", &headers("300", "290", "345600"), now);
    quota.store("prov-b", &headers("300", "75", "345600"), now);
    {
        let runtime = binary.runtime();
        let ledger = EvidenceLedger::open(&runtime).unwrap();
        ledger
            .record(
                NewObservation::new("prov-a", "shared-model")
                    .with_route(Some("anthropic-messages"))
                    .with_harness(Some("claude-code"))
                    .with_quota_context(Some(format!("prov-a/{VAR_A}")))
                    .with_timing(Some(now - 300), Some(now - 295))
                    .with_outcome(Outcome::Failed)
                    .with_failure_class(Some(FailureClass::Throttle)),
                now - 295,
            )
            .unwrap();
    }

    let out = binary.glasshouse(&["launch", "claude-code", "--headless"]);
    let said = Binary::both_streams(&out);
    assert!(out.status.success(), "{said}");
    assert!(
        said.contains("is throttled"),
        "the launch says the fallback out loud before acting on it:\n{said}"
    );

    // Read back through the same purpose-bucketed aggregation the overhead
    // report renders (a fallback row carries no outcome, so the
    // outcome-carrying readers rightly never see it): a row written under no
    // purpose — or the wrong one — lands in another bucket and fails here.
    let runtime = binary.runtime();
    let ledger = EvidenceLedger::open(&runtime).unwrap();
    let groups = ledger.consumption_by_purpose(now_unix() + 1, 3600).unwrap();
    let overhead = glasshouse::routing::evidence::RoutingOverhead::from_consumption(&groups);
    assert_eq!(
        overhead.entitlement_fallback_requests, 1,
        "exactly one fallback happened, so exactly one row records it:\n{groups:?}\nSAID:\n{said}"
    );
    let fallback_group = groups
        .iter()
        .find(|group| {
            group.purpose.as_deref()
                == Some(glasshouse::routing::evidence::ENTITLEMENT_FALLBACK_THROTTLED_PURPOSE)
        })
        .expect("the throttled-fallback purpose group exists");
    assert_eq!(
        fallback_group.sample_count, 1,
        "and its purpose is the trigger, not the exhausted twin:\n{groups:?}"
    );
}

/// **Line 1307 on the same fallback path.** The row the test above reads
/// back also carries an estimated cost, computed at the moment of the
/// decision (`estimated_cost` in `routing/session.rs`) and written through
/// `record_entitlement_fallback`'s `with_cost(cost)` — priced, not the free
/// zero, so the estimate this asserts is a real comparison point against
/// actual usage, matching what the box promises.
#[test]
fn a_launch_that_falls_back_records_the_chosen_destinations_estimated_cost() {
    let binary = Binary::with_config(&two_provider_config_on_one_model());

    // A priced entry for the destination the fallback lands on, so the
    // estimate is a real number rather than the free-model zero.
    std::fs::write(
        binary.base.join("config").join("pricing.toml"),
        "[[prices]]\nprovider = \"prov-b\"\nmodel = \"shared-model\"\n\
         input_per_million_usd = 3.0\noutput_per_million_usd = 9.0\n",
    )
    .expect("write pricing.toml");

    let now = now_unix();
    let quota = GatewayQuotaCache::at(binary.base.join("data").join("gateway-quota"));
    quota.store("prov-a", &headers("300", "290", "345600"), now);
    quota.store("prov-b", &headers("300", "75", "345600"), now);
    {
        let runtime = binary.runtime();

        // A checkpoint, so `latest_checkpoint_tokens` is `Some` and the
        // fresh destination's estimated input size is known — otherwise
        // `estimated_cost` has a price but no size and returns `None`.
        let checkpoint = glasshouse::checkpoint::Checkpoint {
            session: glasshouse::session::SessionId::new("cost-recorded-session"),
            harness: "claude-code".to_owned(),
            reason: glasshouse::checkpoint::CheckpointReason::Manual,
            created_at: now,
            git: None,
            working_tree: None,
            handoff: glasshouse::checkpoint::Handoff {
                objective: "close out line 1307".to_owned(),
                implementation_state: "fallback recorded, cost not yet observed".to_owned(),
                decisions: Vec::new(),
                memory: Vec::new(),
                failed_approaches: Vec::new(),
                files: Vec::new(),
                test_state: None,
                next_actions: Vec::new(),
            },
            trimmed: false,
        };
        glasshouse::checkpoint::ProjectCheckpoints::open(&runtime)
            .unwrap()
            .store()
            .save(checkpoint)
            .unwrap();

        let ledger = EvidenceLedger::open(&runtime).unwrap();
        ledger
            .record(
                NewObservation::new("prov-a", "shared-model")
                    .with_route(Some("anthropic-messages"))
                    .with_harness(Some("claude-code"))
                    .with_quota_context(Some(format!("prov-a/{VAR_A}")))
                    .with_timing(Some(now - 300), Some(now - 295))
                    .with_outcome(Outcome::Failed)
                    .with_failure_class(Some(FailureClass::Throttle)),
                now - 295,
            )
            .unwrap();
    }

    let out = binary.glasshouse(&["launch", "claude-code", "--headless"]);
    let said = Binary::both_streams(&out);
    assert!(out.status.success(), "{said}");

    let runtime = binary.runtime();
    let ledger = EvidenceLedger::open(&runtime).unwrap();
    let rows = ledger
        .recent(
            ObservationQuery {
                provider: "prov-b",
                model: "shared-model",
                route: None,
                harness: Some("claude-code"),
            },
            10,
        )
        .unwrap();
    let observation = rows
        .first()
        .unwrap_or_else(|| panic!("the fallback row exists:\n{said}"));
    let cost = observation
        .cost
        .unwrap_or_else(|| panic!("the fallback row carries an estimated cost:\n{said}"));
    assert!(
        cost.micro_usd > 0,
        "a priced destination's estimate is nonzero, not the free-model zero:\n{cost:?}"
    );
    assert_eq!(
        cost.confidence,
        CostConfidence::Estimated,
        "every cost this function produces is an estimate, never an actual:\n{cost:?}"
    );
}

/// **Line 1971 end to end, and the whole spend chain in one run.** The
/// ledger holds 1,200 tokens against `claude-a`'s own credential label; the
/// user's `[entitlements.claude-a]` states a 1,000-token ceiling. The
/// resolver reads the rows, `to_routing` carries the reading, the gate
/// refuses the account **by name and with both numbers**, and the work goes
/// to the sibling — which states the same ceiling and whose spend nothing
/// measured, so it is admitted.
///
/// This is the producer→caller→propagation→consumer path in one assertion:
/// a build that broke any link renders `fresh:claude-code:alpha@claude-a`
/// as an ordinary candidate instead of a refusal.
#[test]
fn route_refuses_an_account_over_the_spend_ceiling_the_user_wrote() {
    let config = format!("{}\n[entitlements.claude-a.__placeholder]\n", pool_config())
        .replace(
            "[entitlements.claude-a]\nkind = \"claude\"",
            "[entitlements.claude-a]\nspend_ceiling_tokens = 1000\nkind = \"claude\"",
        )
        .replace(
            "[entitlements.claude-b]\nkind = \"claude\"",
            "[entitlements.claude-b]\nspend_ceiling_tokens = 1000\nkind = \"claude\"",
        )
        .replace("\n[entitlements.claude-a.__placeholder]\n", "");
    let binary = Binary::with_config(&config);
    let now = now_unix();
    {
        let runtime = binary.runtime();
        let ledger = EvidenceLedger::open(&runtime).unwrap();
        ledger
            .record(
                NewObservation::new("alpha-probe", "some-model")
                    .with_route(Some("anthropic-messages"))
                    .with_harness(Some("claude-code"))
                    .with_quota_context(Some(LABEL_A))
                    .with_timing(Some(now - 300), Some(now - 295))
                    .with_tokens(Some(800), Some(400), None)
                    .with_outcome(Outcome::Succeeded),
                now - 295,
            )
            .unwrap();
    }

    let out = binary.glasshouse(&["route"]);
    let said = Binary::both_streams(&out);
    assert!(out.status.success(), "{said}");
    assert!(
        said.contains("its spend ceiling of 1000 tokens is reached (1200 observed)"),
        "the account over the ceiling the user wrote is refused, with both numbers:\n{said}"
    );
    assert!(
        said.contains(
            "fresh:claude-code:alpha@claude-a on claude-code via alpha-probe (fresh) \
                       — hard entitlement constraint"
        ),
        "and the refusal names the candidate it removed:\n{said}"
    );
    assert!(
        said.contains("destination  fresh:claude-code:alpha@claude-b"),
        "the sibling states the same ceiling and nothing measured its spend, so it serves:\n\
         {said}"
    );
}
