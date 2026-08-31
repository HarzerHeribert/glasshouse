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
use glasshouse::routing::evidence::{EvidenceLedger, FailureClass, NewObservation, Outcome};
use glasshouse::routing::free::FreePool;
use glasshouse::routing::session::{
    CheckpointQuality, Destination, EntitlementPoolView, Routed, RouterInputs, RoutingMoment,
    RoutingOverride, SessionRouter, TaskRequirements, entitlement_capacity,
    entitlement_model_availability, entitlement_reset_boundary, entitlement_throttling,
};
use glasshouse::routing::{
    AssignedModel, Backend, Cost, CredentialId, Entitlement, EntitlementModelsFacet,
    EntitlementRefusal, EntitlementRules, HardConstraint, ToolSemantics,
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
        let harness = install_fake_harness(&bin_dir, &env_log);
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

#[cfg(unix)]
fn install_fake_harness(bin_dir: &Path, env_log: &Path) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;

    let path = bin_dir.join("fake-claude-code");
    // `export -p` is a shell builtin, so the empty PATH the fixture
    // launches under cannot break it — `tests/entitlement_pool.rs`'s idiom.
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
    let path = bin_dir.join("fake-claude-code.cmd");
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
