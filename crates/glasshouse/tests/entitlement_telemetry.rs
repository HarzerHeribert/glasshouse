//! Capability map line 1965 — per-entitlement telemetry: remaining
//! capacity, time until reset, recent throttling and the models an
//! entitlement can serve, each read from the telemetry the provider
//! actually exposes and each carrying its scope — per-account where the
//! reading is keyed by this account's own credential, provider-wide where
//! every entitlement of the provider shares it, and *unknown* spelled out
//! where nothing exists (never full, never empty, never a fabricated
//! number).
//!
//! Two halves, `tests/entitlement_pool.rs`'s own split. Half one enters
//! through the public resolver exactly as `main.rs::status_report` enters
//! it — `EffectiveConfig::configured_entitlements_with_telemetry` over a
//! fixture-fed `GatewayQuotaCache`, `ModelCache` and a real project
//! ledger's rows. Half two runs the shipped binary's `glasshouse status`
//! over the same plants, because nothing in half one can fail on a build
//! where the status line stops rendering the facets (practice §35).
//!
//! The scope discipline under test, stated once: the gateway's quota cache
//! is keyed by provider — the gateway's write is settled — so two
//! entitlements of one provider must show the *same* capacity and reset
//! reading, marked `provider-wide`; the ledger's throttle rows carry the
//! serving credential's label in `quota_context`, so the throttle facet is
//! the one reading that can honestly narrow to `this account`.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use clap::Parser;

use glasshouse::Runtime;
use glasshouse::config::{
    EffectiveConfig, EntitlementModels, EntitlementTelemetry, TelemetryScope, UserConfig,
};
use glasshouse::provider::cache::{ModelCache, ModelCatalogue, ModelEntry};
use glasshouse::provider::quota::{CapacityBand, CapacityBandThresholds};
use glasshouse::provider::telemetry::{GatewayQuotaCache, RateLimitHeaders};
use glasshouse::routing::evidence::{
    CLASSIFICATION_EVIDENCE_WINDOW_SECONDS, EvidenceLedger, FailureClass, NewObservation, Outcome,
};

const VAR_A: &str = "GLASSHOUSE_ENT_TELEM_KEY_A";
const VAR_B: &str = "GLASSHOUSE_ENT_TELEM_KEY_B";

/// The `quota_context` labels the gateway would stamp for these two
/// credentials — `CredentialId::label`'s `provider/var` shape.
const LABEL_A: &str = "alpha-probe/GLASSHOUSE_ENT_TELEM_KEY_A";

/// Two entitlements of ONE provider — the sharing the scope words are
/// about — plus a configured native sign-in, which has no provider
/// telemetry at all and whose models are the harness's own decision.
fn pool_config() -> String {
    format!(
        "[providers.alpha-probe]\ntemplate = \"openrouter\"\n\
         credential_env = [\"{VAR_A}\", \"{VAR_B}\"]\n\n\
         [entitlements.claude-a]\nkind = \"claude\"\nvendor = \"claude\"\n\
         provider = \"alpha-probe\"\ncredential = {{ env = \"{VAR_A}\" }}\n\n\
         [entitlements.claude-b]\nkind = \"claude\"\nvendor = \"claude\"\n\
         provider = \"alpha-probe\"\ncredential = {{ env = \"{VAR_B}\" }}\n\n\
         [entitlements.claude-native]\nnative_harness = \"claude-code\"\n"
    )
}

fn user_config() -> UserConfig {
    toml::from_str(&format!("version = 1\n\n{}", pool_config())).expect("the fixture parses")
}

/// AnyRouter's real header shape with both halves stated: 240 of 300 left
/// (80% — `Plenty` under the default thresholds) and a reset 600 seconds
/// after the observation.
fn planted_headers() -> RateLimitHeaders {
    RateLimitHeaders::read(vec![
        ("ratelimit-limit", "300"),
        ("ratelimit-remaining", "240"),
        ("ratelimit-reset", "600"),
    ])
}

// ===========================================================================
// Half one — the resolver, entered as `main.rs::status_report` enters it.
// ===========================================================================

/// **The scope rule for capacity and reset.** The cache is keyed by
/// provider, so both entitlements of `alpha-probe` get the *same* reading —
/// value-equal, never each their own — and it is marked provider-wide.
#[test]
fn two_entitlements_of_one_provider_share_the_same_provider_wide_capacity_reading() {
    let tmp = tempfile::tempdir().unwrap();
    let quota = GatewayQuotaCache::at(tmp.path().join("gateway-quota"));
    let now = 1_800_000_000_i64;
    quota.store("alpha-probe", &planted_headers(), now - 30);

    let user = user_config();
    let effective = EffectiveConfig::new(&user, None);
    let telemetry = EntitlementTelemetry::new(now).with_gateway_quota(&quota);
    let entries = effective
        .configured_entitlements_with_telemetry(&telemetry)
        .expect("the pool resolves");
    assert_eq!(entries.len(), 3, "{entries:?}");
    let a = &entries[0];
    let b = &entries[1];
    assert_eq!(a.name(), "claude-a");
    assert_eq!(b.name(), "claude-b");

    let score = a
        .remaining_capacity()
        .expect("a cached reading with both halves yields a real score");
    assert_eq!(
        score.percent().exact(),
        Some(80),
        "240 of 300 is an exact 80%, never an estimate: {score:?}"
    );
    assert_eq!(
        score.band(&CapacityBandThresholds::DEFAULT),
        CapacityBand::Plenty
    );
    assert_eq!(
        a.remaining_capacity(),
        b.remaining_capacity(),
        "one provider, one reading — the two accounts share it verbatim"
    );
    assert_eq!(a.seconds_until_reset(), Some(570));
    assert_eq!(
        a.seconds_until_reset(),
        b.seconds_until_reset(),
        "the reset is the provider's window, shared like the capacity"
    );
    assert_eq!(a.capacity_scope(), Some(TelemetryScope::ProviderWide));
    assert_eq!(
        b.capacity_scope(),
        Some(TelemetryScope::ProviderWide),
        "a shared reading must say it is shared"
    );

    // The native sign-in has no provider telemetry: unknown, never a number.
    let native = &entries[2];
    assert_eq!(native.name(), "claude-native");
    assert!(native.remaining_capacity().is_none());
    assert!(native.seconds_until_reset().is_none());
    assert!(native.capacity_scope().is_none());
}

/// **Unknown is the answer, not a gap.** With no sources at all every facet
/// stays `None`; with sources in hand but nothing cached, capacity and
/// models stay `None` while the throttle facet — whose resolver actually
/// looked at the (empty) rows — honestly reads zero observed at provider
/// scope.
#[test]
fn an_entitlement_with_no_telemetry_stays_unknown_on_every_facet() {
    let user = user_config();
    let effective = EffectiveConfig::new(&user, None);
    let now = 1_800_000_000_i64;

    let blind = effective
        .configured_entitlements_with_telemetry(&EntitlementTelemetry::new(now))
        .expect("the pool resolves");
    for entry in blind.iter().filter(|e| e.name() != "claude-native") {
        assert!(entry.remaining_capacity().is_none(), "{entry:?}");
        assert!(entry.seconds_until_reset().is_none());
        assert!(entry.capacity_scope().is_none());
        assert!(
            entry.throttling().is_none(),
            "no rows were consulted, so 'none observed' may not be claimed: {entry:?}"
        );
        assert!(entry.models().is_none());
    }

    let tmp = tempfile::tempdir().unwrap();
    let quota = GatewayQuotaCache::at(tmp.path().join("gateway-quota"));
    let models = ModelCache::at(tmp.path().join("providers"));
    let telemetry = EntitlementTelemetry::new(now)
        .with_gateway_quota(&quota)
        .with_model_catalogues(&models)
        .with_observations(&[]);
    let entries = effective
        .configured_entitlements_with_telemetry(&telemetry)
        .expect("the pool resolves");
    let a = &entries[0];
    assert!(
        a.remaining_capacity().is_none(),
        "empty cache reads unknown"
    );
    assert!(a.seconds_until_reset().is_none());
    assert!(a.models().is_none(), "no catalogue was ever fetched");
    let throttling = a.throttling().expect("the rows were consulted");
    assert_eq!(throttling.throttled(), 0);
    assert_eq!(
        throttling.scope(),
        TelemetryScope::ProviderWide,
        "zero rows cannot be narrowed to an account"
    );
}

/// **The models facet is the provider's own declaration and nothing else.**
/// A `Provider(X)`-backed entry reads the fetched catalogue; the native
/// sign-in is `HarnessDecided` even with catalogues sitting right there —
/// Glasshouse does not know a plan's models and never invents a list.
#[test]
fn the_models_facet_reads_the_declared_catalogue_and_never_invents_one_for_native() {
    let tmp = tempfile::tempdir().unwrap();
    let models = ModelCache::at(tmp.path().join("providers"));
    models
        .store(&ModelCatalogue::new(
            "alpha-probe",
            "https://alpha-probe.example/api/v1",
            "https://alpha-probe.example/api/v1/models",
            1_800_000_000,
            vec![ModelEntry::new("alpha-m1"), ModelEntry::new("alpha-m2")],
        ))
        .expect("the catalogue stores");

    let user = user_config();
    let effective = EffectiveConfig::new(&user, None);
    let telemetry = EntitlementTelemetry::new(1_800_000_000).with_model_catalogues(&models);
    let entries = effective
        .configured_entitlements_with_telemetry(&telemetry)
        .expect("the pool resolves");

    let a = &entries[0];
    assert_eq!(
        a.models(),
        Some(&EntitlementModels::Declared {
            models: vec!["alpha-m1".to_owned(), "alpha-m2".to_owned()],
            scope: TelemetryScope::ProviderWide,
        }),
        "the provider's own declared list, marked as the provider's"
    );

    let native = entries
        .iter()
        .find(|entry| entry.name() == "claude-native")
        .expect("the native entry resolves");
    assert_eq!(
        native.models(),
        Some(&EntitlementModels::HarnessDecided),
        "a native sign-in's models are the harness's decision — no list, ever: {native:?}"
    );
}

// ===========================================================================
// A real project ledger for the throttle facet, and the shipped binary.
// ===========================================================================

/// A bootstrapped project inside `base`, sharing `base`'s data and config
/// roots — `tests/rate_limit_scope.rs`'s own idiom.
struct Fixture {
    base: PathBuf,
    root: PathBuf,
    runtime: Runtime,
}

impl Fixture {
    fn new(base: &Path) -> Self {
        let root = base.join("workspace");
        std::fs::create_dir_all(root.join(".git")).unwrap();
        let root = std::fs::canonicalize(&root).unwrap();

        let config_dir = base.join("config");
        std::fs::create_dir_all(&config_dir).unwrap();
        std::fs::write(
            config_dir.join("config.toml"),
            format!("version = 1\n\n{}", pool_config()),
        )
        .unwrap();

        let cli = glasshouse::Cli::try_parse_from([
            "glasshouse",
            "--data-dir",
            base.join("data").to_str().unwrap(),
            "--config-dir",
            config_dir.to_str().unwrap(),
        ])
        .unwrap();
        let runtime = glasshouse::bootstrap(&cli, &root).unwrap();
        Fixture {
            base: base.to_path_buf(),
            root,
            runtime,
        }
    }

    fn ledger(&self) -> EvidenceLedger {
        EvidenceLedger::open(&self.runtime).unwrap()
    }

    /// The shipped binary, pointed at this project.
    fn glasshouse(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_glasshouse"))
            .arg("--scope")
            .arg(&self.root)
            .arg("--data-dir")
            .arg(self.base.join("data"))
            .arg("--config-dir")
            .arg(self.base.join("config"))
            .args(args)
            .env("PATH", self.base.join("empty-path"))
            .output()
            .expect("the glasshouse binary must be runnable")
    }
}

fn throttle(provider: &str, account: Option<&str>, at: i64) -> NewObservation {
    NewObservation::new(provider, "some-model")
        .with_route(Some("anthropic-messages"))
        .with_harness(Some("claude-code"))
        .with_quota_context(account)
        .with_timing(Some(at), Some(at + 5))
        .with_outcome(Outcome::Failed)
        .with_failure_class(Some(FailureClass::Throttle))
}

/// **The throttle facet narrows by the credential label, and only this
/// provider's rows are read.** Two throttles under `claude-a`'s own label,
/// none under `claude-b`'s, and three on an unrelated provider that must
/// not leak into either count.
#[test]
fn the_throttle_facet_narrows_to_the_account_and_reads_only_this_providers_rows() {
    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path());
    let ledger = fixture.ledger();
    let now = 1_800_000_000_i64;
    for i in 0..2 {
        ledger
            .record(
                throttle("alpha-probe", Some(LABEL_A), now - 3_600 + i * 300),
                now - 3_600 + i * 300 + 5,
            )
            .unwrap();
    }
    for i in 0..3 {
        ledger
            .record(
                throttle(
                    "other-probe",
                    Some("other-probe/OTHER_KEY"),
                    now - 1_800 + i * 300,
                ),
                now - 1_800 + i * 300 + 5,
            )
            .unwrap();
    }

    let rows = ledger
        .observations_in_window(now, CLASSIFICATION_EVIDENCE_WINDOW_SECONDS)
        .expect("the window reads");
    let user = user_config();
    let effective = EffectiveConfig::new(&user, None);
    let telemetry = EntitlementTelemetry::new(now).with_observations(&rows);
    let entries = effective
        .configured_entitlements_with_telemetry(&telemetry)
        .expect("the pool resolves");

    let a = entries[0].throttling().expect("the rows were consulted");
    assert_eq!(
        a.throttled(),
        2,
        "claude-a's own rows — not other-probe's three, not five in total"
    );
    assert_eq!(a.scope(), TelemetryScope::PerAccount);

    let b = entries[1].throttling().expect("the rows were consulted");
    assert_eq!(
        b.throttled(),
        0,
        "every alpha-probe throttle names claude-a's credential, so claude-b's own count is zero"
    );
    assert_eq!(b.scope(), TelemetryScope::PerAccount);

    // The native sign-in has no provider whose rows could be read.
    let native = entries
        .iter()
        .find(|entry| entry.name() == "claude-native")
        .unwrap();
    assert!(native.throttling().is_none());
}

/// **A context-less throttle row widens the reading to provider scope.**
/// One row nothing attributes to an account makes the honest reading the
/// provider's total, shared by both entitlements — and both say so. The
/// total is still *this provider's*: the other provider's context-less
/// throttles must not inflate it, which is the assertion the per-account
/// narrowing above cannot make (its label already embeds the provider).
#[test]
fn a_contextless_throttle_widens_both_accounts_readings_to_provider_scope() {
    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path());
    let ledger = fixture.ledger();
    let now = 1_800_000_000_i64;
    ledger
        .record(
            throttle("alpha-probe", Some(LABEL_A), now - 3_600),
            now - 3_595,
        )
        .unwrap();
    ledger
        .record(throttle("alpha-probe", None, now - 3_000), now - 2_995)
        .unwrap();
    // Another provider's context-less throttles: never alpha-probe's.
    for i in 0..3 {
        ledger
            .record(
                throttle("other-probe", None, now - 2_000 + i * 100),
                now - 1_995 + i * 100,
            )
            .unwrap();
    }

    let rows = ledger
        .observations_in_window(now, CLASSIFICATION_EVIDENCE_WINDOW_SECONDS)
        .expect("the window reads");
    let user = user_config();
    let effective = EffectiveConfig::new(&user, None);
    let telemetry = EntitlementTelemetry::new(now).with_observations(&rows);
    let entries = effective
        .configured_entitlements_with_telemetry(&telemetry)
        .expect("the pool resolves");

    for entry in entries.iter().filter(|e| e.name() != "claude-native") {
        let reading = entry.throttling().expect("the rows were consulted");
        assert_eq!(
            reading.throttled(),
            2,
            "{}: the provider's total",
            entry.name()
        );
        assert_eq!(
            reading.scope(),
            TelemetryScope::ProviderWide,
            "{}: an unattributable throttle cannot be subtracted from an account",
            entry.name()
        );
    }
}

// ===========================================================================
// Half two — the shipped binary's status line (practice §35).
// ===========================================================================

fn both_streams(output: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

/// The status line for one entitlement, by its backticked name.
fn facet_line(status: &str, name: &str) -> String {
    status
        .lines()
        .find(|line| line.trim_start().starts_with(&format!("`{name}`")))
        .unwrap_or_else(|| panic!("no status line for `{name}` in:\n{status}"))
        .to_owned()
}

/// **Line 1965 through the shipped binary.** A fixture-fed cache, a
/// declared catalogue and real ledger rows; two entitlements of one
/// provider; every facet rendered with its scope, and the two accounts'
/// shared readings *identical* — a build that hands them different
/// provider-wide readings fails here.
#[test]
fn status_shows_all_four_facets_with_their_scope_through_the_shipped_binary() {
    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path());

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;

    // The gateway-quota cache, at the exact directory `GatewayQuotaCache::new`
    // resolves from the binary's `--data-dir`.
    GatewayQuotaCache::at(tmp.path().join("data").join("gateway-quota")).store(
        "alpha-probe",
        &planted_headers(),
        now,
    );
    ModelCache::at(tmp.path().join("data").join("providers"))
        .store(&ModelCatalogue::new(
            "alpha-probe",
            "https://alpha-probe.example/api/v1",
            "https://alpha-probe.example/api/v1/models",
            now,
            vec![ModelEntry::new("alpha-m1"), ModelEntry::new("alpha-m2")],
        ))
        .expect("the catalogue stores");
    let ledger = fixture.ledger();
    for i in 0..2 {
        ledger
            .record(
                throttle("alpha-probe", Some(LABEL_A), now - 3_600 + i * 300),
                now - 3_600 + i * 300 + 5,
            )
            .unwrap();
    }

    let out = fixture.glasshouse(&["status"]);
    let said = both_streams(&out);
    assert!(out.status.success(), "{said}");
    assert!(said.contains("Entitlements 3 configured"), "{said}");

    let line_a = facet_line(&said, "claude-a");
    let line_b = facet_line(&said, "claude-b");
    let line_native = facet_line(&said, "claude-native");

    // Capacity and reset: real readings, banded, marked provider-wide.
    assert!(
        line_a.contains("capacity: plenty (provider-wide)"),
        "80% of the provider's window is the plenty band, and the reading is shared:\n{line_a}"
    );
    assert!(
        line_a.contains("reset: in ") && line_a.contains("s (provider-wide)"),
        "the reset is rendered with its scope:\n{line_a}"
    );

    // The sharing itself: strip the names and the two lines' facets must be
    // byte-identical on capacity and reset — same reading, not two.
    let facets_a = line_a.split("capacity:").nth(1).unwrap();
    let facets_b = line_b.split("capacity:").nth(1).unwrap();
    let capacity_and_reset = |facets: &str| {
        facets
            .split("· throttling:")
            .next()
            .expect("the facet line carries a throttling facet")
            .to_owned()
    };
    assert_eq!(
        capacity_and_reset(facets_a),
        capacity_and_reset(facets_b),
        "two entitlements of one provider share one provider-wide reading:\n{line_a}\n{line_b}"
    );

    // Throttling: narrowed per account — the two accounts differ honestly.
    assert!(
        line_a.contains("throttling: 2 recent (this account)"),
        "claude-a's own two throttles, said to be its own:\n{line_a}"
    );
    assert!(
        line_b.contains("throttling: none observed (this account)"),
        "every throttle names claude-a, so claude-b's own count is zero:\n{line_b}"
    );

    // Models: the provider's declared list for the backed entries; the
    // harness's own decision for the native sign-in — never a list.
    assert!(
        line_a.contains("models: alpha-m1, alpha-m2 (provider-wide)"),
        "the provider's own declared models, marked shared:\n{line_a}"
    );
    assert!(
        line_native.contains("models: the harness decides"),
        "a native sign-in's models are the harness's decision:\n{line_native}"
    );
    assert!(
        !line_native.contains("alpha-m1"),
        "no declared list may leak onto a native sign-in:\n{line_native}"
    );

    // And the native entry's other facets are unknown, spelled out.
    assert!(
        line_native.contains("capacity: unknown")
            && line_native.contains("reset: unknown")
            && line_native.contains("throttling: unknown"),
        "a native sign-in has no provider telemetry — unknown, never a number:\n{line_native}"
    );
}

/// **Nothing measured renders `unknown`, never a number.** No cache, no
/// catalogue, an empty ledger: capacity, reset and models say `unknown`;
/// throttling — whose resolver did look, at zero rows — says none observed
/// at provider scope.
#[test]
fn status_spells_unknown_for_an_entitlement_nothing_measured() {
    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path());

    let out = fixture.glasshouse(&["status"]);
    let said = both_streams(&out);
    assert!(out.status.success(), "{said}");

    let line_a = facet_line(&said, "claude-a");
    assert!(
        line_a.contains("capacity: unknown")
            && line_a.contains("reset: unknown")
            && line_a.contains("models: unknown"),
        "unknown is a rendered word:\n{line_a}"
    );
    assert!(
        line_a.contains("throttling: none observed (provider-wide)"),
        "an empty ledger was still consulted, and zero cannot be narrowed to an account:\n{line_a}"
    );
    assert!(
        !line_a.contains('%'),
        "no percentage may appear for an entitlement nothing measured:\n{line_a}"
    );
}

#[test]
fn entitlement_json_exposes_sorted_catalogues_without_credentials() {
    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path());
    ModelCache::at(tmp.path().join("data/providers"))
        .store(&ModelCatalogue::new(
            "alpha-probe",
            "https://alpha-probe.example/api/v1",
            "https://alpha-probe.example/api/v1/models",
            glasshouse::provider::cache::now_unix_seconds(),
            vec![ModelEntry::new("z-model"), ModelEntry::new("a-model")],
        ))
        .unwrap();
    let output = fixture.glasshouse(&["entitlements", "--json"]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let body: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(body["version"], 1);
    assert_eq!(body["accounts"][0]["account"], "claude-a");
    assert_eq!(body["accounts"][1]["account"], "claude-b");
    assert_eq!(
        body["accounts"][0]["models"],
        serde_json::json!(["a-model", "z-model"])
    );
    assert_eq!(body["accounts"][2]["scope"], "harness-decides");
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(!text.contains(VAR_A) && !text.contains(VAR_B) && !text.contains("credential"));
}

#[test]
fn entitlement_refresh_populates_missing_catalogues_through_the_binary() {
    use std::io::{BufRead, BufReader, Write};
    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path());
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let config_path = fixture.base.join("config/config.toml");
    let config = std::fs::read_to_string(&config_path).unwrap().replace(
        "[providers.alpha-probe]",
        &format!("[providers.alpha-probe]\nbase_url = \"http://{address}/v1\""),
    );
    std::fs::write(&config_path, config).unwrap();
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut reader = BufReader::new(stream.try_clone().unwrap());
        let mut line = String::new();
        reader.read_line(&mut line).unwrap();
        assert!(line.starts_with("GET /v1/models "));
        loop {
            line.clear();
            reader.read_line(&mut line).unwrap();
            if line == "\r\n" {
                break;
            }
        }
        let body = r#"{"data":[{"id":"fresh-model"}]}"#;
        write!(
            stream,
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        )
        .unwrap();
    });
    let output = fixture.glasshouse(&["entitlements", "--json", "--refresh"]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    server.join().unwrap();
    let body: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        body["accounts"][0]["models"],
        serde_json::json!(["fresh-model"])
    );
    assert_eq!(
        body["accounts"][1]["models"],
        serde_json::json!(["fresh-model"])
    );
}
