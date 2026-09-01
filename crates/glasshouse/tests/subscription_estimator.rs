//! Capability map lines 1244/1245/1246/1250/1251/1254 — the subscription
//! headroom estimator: a per-account band derived entirely on read from the
//! evidence ledger's own rows, the caller's own reset reading, and this
//! project's own session history. No new table, no migration, no persisted
//! estimator state — today's history IS the ledger's own rows in window.
//!
//! Three levels, the same split `tests/entitlement_telemetry.rs` uses. Most
//! tests call [`estimate_subscription_headroom`] directly over rows the
//! ledger actually recorded — a pure function, so nothing here needs the
//! resolver or the binary to prove the band logic. Two tests go through the
//! resolver ([`EffectiveConfig::configured_entitlements_with_telemetry`]) to
//! prove the wiring: that the resolver populates the facet at all, and that
//! it never displaces an authoritative reading headers actually gave. One
//! goes through the shipped binary, because nothing at the first two levels
//! can fail on a build where `glasshouse entitlements` stops rendering the
//! facet (practice §35).

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use clap::Parser;

use glasshouse::Runtime;
use glasshouse::config::{EffectiveConfig, EntitlementTelemetry, UserConfig};
use glasshouse::provider::quota::Confidence;
use glasshouse::provider::telemetry::{GatewayQuotaCache, RateLimitHeaders};
use glasshouse::routing::evidence::{
    CLASSIFICATION_EVIDENCE_WINDOW_SECONDS, EvidenceLedger, FailureClass, HeadroomBand,
    HeadroomBasis, LongWindowPressure, NewObservation, Outcome, ResetBasis,
    estimate_subscription_headroom,
};
use glasshouse::session::{NewSession, ProjectSessions};

const PROVIDER: &str = "sub-est-probe";
const VAR_A: &str = "GLASSHOUSE_SUB_EST_KEY_A";
const VAR_B: &str = "GLASSHOUSE_SUB_EST_KEY_B";
const LABEL_A: &str = "sub-est-probe/GLASSHOUSE_SUB_EST_KEY_A";
const LABEL_B: &str = "sub-est-probe/GLASSHOUSE_SUB_EST_KEY_B";

fn pool_config() -> String {
    format!(
        "[providers.{PROVIDER}]\ntemplate = \"openrouter\"\n\
         credential_env = [\"{VAR_A}\", \"{VAR_B}\"]\n\n\
         [entitlements.acct-a]\nkind = \"claude\"\nvendor = \"claude\"\n\
         provider = \"{PROVIDER}\"\ncredential = {{ env = \"{VAR_A}\" }}\n\n\
         [entitlements.acct-b]\nkind = \"claude\"\nvendor = \"claude\"\n\
         provider = \"{PROVIDER}\"\ncredential = {{ env = \"{VAR_B}\" }}\n"
    )
}

fn user_config() -> UserConfig {
    toml::from_str(&format!("version = 1\n\n{}", pool_config())).expect("the fixture parses")
}

/// A bootstrapped project inside `base` — `tests/entitlement_telemetry.rs`'s
/// own idiom.
struct Fixture {
    base: PathBuf,
    root: PathBuf,
    runtime: Runtime,
}

impl Fixture {
    fn new(base: &Path) -> Self {
        Self::with_pool_config(base, &pool_config())
    }

    /// Map lines 1252/1255: a fixture over a caller-supplied
    /// `[providers]`/`[entitlements]` block rather than the fixed two-account
    /// `pool_config()`, so a test can plant an override or a disable switch
    /// on one account without disturbing the other.
    fn with_pool_config(base: &Path, pool_config: &str) -> Self {
        let root = base.join("workspace");
        std::fs::create_dir_all(root.join(".git")).unwrap();
        let root = std::fs::canonicalize(&root).unwrap();

        let config_dir = base.join("config");
        std::fs::create_dir_all(&config_dir).unwrap();
        std::fs::write(
            config_dir.join("config.toml"),
            format!("version = 1\n\n{pool_config}"),
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

    fn runtime(&self) -> &Runtime {
        &self.runtime
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

fn accepted(account: Option<&str>, at: i64) -> NewObservation {
    NewObservation::new(PROVIDER, "some-model")
        .with_route(Some("anthropic-messages"))
        .with_harness(Some("claude-code"))
        .with_quota_context(account)
        .with_timing(Some(at), Some(at + 5))
        .with_outcome(Outcome::Succeeded)
}

fn accepted_with_tokens(account: Option<&str>, at: i64, input: i64, output: i64) -> NewObservation {
    accepted(account, at).with_tokens(Some(input), Some(output), None)
}

fn throttle(account: Option<&str>, at: i64) -> NewObservation {
    NewObservation::new(PROVIDER, "some-model")
        .with_route(Some("anthropic-messages"))
        .with_harness(Some("claude-code"))
        .with_quota_context(account)
        .with_timing(Some(at), Some(at + 5))
        .with_outcome(Outcome::Failed)
        .with_failure_class(Some(FailureClass::Throttle))
}

// ===========================================================================
// The pure function, over rows the ledger actually recorded.
// ===========================================================================

/// **Map line 1244.** A provider that publishes no numeric budget still
/// yields an estimate from accepted-request counts alone — no token row is
/// ever planted here.
#[test]
fn test_1244_opaque_limit_account_gets_an_estimate_without_token_rows() {
    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path());
    let ledger = fixture.ledger();
    let now = 1_800_000_000_i64;
    for i in 0..3 {
        ledger
            .record(
                accepted(Some(LABEL_A), now - 300 + i * 60),
                now - 295 + i * 60,
            )
            .unwrap();
    }
    let rows = ledger
        .observations_in_window(now, CLASSIFICATION_EVIDENCE_WINDOW_SECONDS)
        .unwrap();

    let estimate = estimate_subscription_headroom(&rows, PROVIDER, Some(LABEL_A), now, None, None)
        .expect("accepted rows are real evidence even with no token budget ever published");

    assert_eq!(
        estimate.basis,
        HeadroomBasis::RequestActivity,
        "no row carried a token count: {estimate:?}"
    );
    assert_eq!(estimate.band, HeadroomBand::Ample);
    assert_eq!(estimate.confidence, Confidence::Low);
}

/// **Map line 1245, throttle recency.** A throttle inside the recency
/// horizon with no reset in sight reads as still-live pressure — the
/// worst band this estimator states.
#[test]
fn test_1245_a_recent_throttle_with_no_reset_in_sight_reads_exhausted() {
    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path());
    let ledger = fixture.ledger();
    let now = 1_800_000_000_i64;
    ledger
        .record(throttle(Some(LABEL_A), now - 60), now - 55)
        .unwrap();
    let rows = ledger
        .observations_in_window(now, CLASSIFICATION_EVIDENCE_WINDOW_SECONDS)
        .unwrap();

    let estimate = estimate_subscription_headroom(&rows, PROVIDER, Some(LABEL_A), now, None, None)
        .expect("a throttle is real evidence");
    assert_eq!(estimate.band, HeadroomBand::Exhausted, "{estimate:?}");
}

/// **Map line 1245, reset behavior softening a throttle.** The same recent
/// throttle, but the quota cache says the window is about to roll over —
/// the estimator reads that as relief and reports `Low` rather than
/// `Exhausted`.
#[test]
fn test_1245_a_recent_throttle_softened_by_an_imminent_reset_reads_low() {
    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path());
    let ledger = fixture.ledger();
    let now = 1_800_000_000_i64;
    ledger
        .record(throttle(Some(LABEL_A), now - 60), now - 55)
        .unwrap();
    let rows = ledger
        .observations_in_window(now, CLASSIFICATION_EVIDENCE_WINDOW_SECONDS)
        .unwrap();

    let estimate =
        estimate_subscription_headroom(&rows, PROVIDER, Some(LABEL_A), now, Some(120), None)
            .expect("a throttle is real evidence");
    assert_eq!(estimate.band, HeadroomBand::Low, "{estimate:?}");
}

/// **Map line 1245, historical sessions.** No ledger rows at all, no reset
/// reading — only this project's own count of sessions it has charged to
/// the account. That alone is real evidence of an account in ordinary use.
#[test]
fn test_1245_session_history_alone_reads_ample() {
    let now = 1_800_000_000_i64;
    let estimate = estimate_subscription_headroom(&[], PROVIDER, Some(LABEL_A), now, None, Some(4))
        .expect("session history is real evidence");
    assert_eq!(estimate.band, HeadroomBand::Ample, "{estimate:?}");
}

/// **Map line 1245, reset behavior alone.** A reset reading with nothing
/// else behind it is real evidence the account is quota-bound, and none at
/// all that it is under pressure — the estimator's own honest middle.
#[test]
fn test_1245_a_reset_reading_alone_with_no_activity_reads_moderate() {
    let now = 1_800_000_000_i64;
    let estimate =
        estimate_subscription_headroom(&[], PROVIDER, Some(LABEL_A), now, Some(3_000), None)
            .expect("a reset reading is real evidence");
    assert_eq!(estimate.band, HeadroomBand::Moderate, "{estimate:?}");
}

/// **Map line 1246.** Every informative row of the provider names its own
/// account, so the estimate narrows — and the moment one row does not, the
/// whole estimate widens to provider scope rather than silently dropping
/// the unattributable row.
#[test]
fn test_1246_keying_narrows_to_the_account_and_a_contextless_row_widens_it() {
    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path());
    let ledger = fixture.ledger();
    let now = 1_800_000_000_i64;
    ledger
        .record(accepted(Some(LABEL_A), now - 300), now - 295)
        .unwrap();
    let rows = ledger
        .observations_in_window(now, CLASSIFICATION_EVIDENCE_WINDOW_SECONDS)
        .unwrap();
    let narrowed = estimate_subscription_headroom(&rows, PROVIDER, Some(LABEL_A), now, None, None)
        .expect("accepted rows are real evidence");
    assert!(
        narrowed.account_narrowed,
        "every row named its own account: {narrowed:?}"
    );

    // One more row, naming no account at all.
    ledger.record(accepted(None, now - 200), now - 195).unwrap();
    let rows = ledger
        .observations_in_window(now, CLASSIFICATION_EVIDENCE_WINDOW_SECONDS)
        .unwrap();
    let widened = estimate_subscription_headroom(&rows, PROVIDER, Some(LABEL_A), now, None, None)
        .expect("accepted rows are still real evidence");
    assert!(
        !widened.account_narrowed,
        "one contextless row makes the honest reading provider-wide: {widened:?}"
    );
}

/// **Map line 1250.** The estimate carries a [`HeadroomBand`] and a
/// [`Confidence`], never a percentage — there is no field on the type that
/// could render one, so this pins the rendered [`Debug`] form as a proxy for
/// "no percent sign anywhere in this value."
#[test]
fn test_1250_the_estimate_never_carries_a_percentage() {
    let now = 1_800_000_000_i64;
    let estimate =
        estimate_subscription_headroom(&[], PROVIDER, Some(LABEL_A), now, Some(100), None)
            .expect("a reset reading is real evidence");
    let rendered = format!("{estimate:?}");
    assert!(
        !rendered.contains('%'),
        "no percentage may appear in an estimate: {rendered}"
    );
}

/// **Map line 1251.** Rows that carry a token count change the estimate's
/// [`HeadroomBasis`] label — never a figure. Two windows with very
/// different token sums, and no ceiling to divide either by, must still
/// agree on the band that accepted-request activity alone would already
/// produce.
#[test]
fn test_1251_token_rows_change_the_basis_label_never_a_figure() {
    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path());
    let ledger = fixture.ledger();
    let now = 1_800_000_000_i64;
    ledger
        .record(
            accepted_with_tokens(Some(LABEL_A), now - 300, 88_123, 4_321),
            now - 295,
        )
        .unwrap();
    let rows = ledger
        .observations_in_window(now, CLASSIFICATION_EVIDENCE_WINDOW_SECONDS)
        .unwrap();

    let estimate = estimate_subscription_headroom(&rows, PROVIDER, Some(LABEL_A), now, None, None)
        .expect("accepted rows are real evidence");
    assert_eq!(estimate.basis, HeadroomBasis::TokenUsage, "{estimate:?}");
    assert_eq!(
        estimate.band,
        HeadroomBand::Ample,
        "the same accepted-activity band a token-free window would read: {estimate:?}"
    );
    let rendered = format!("{estimate:?}");
    assert!(
        !rendered.contains("88123") && !rendered.contains("4321") && !rendered.contains("92444"),
        "no planted token figure may appear in the estimate itself: {rendered}"
    );
}

/// **Map line 1254, and REQUIRED BEHAVIOR's "adding one unrelated account's
/// rows changes the first account's estimate by nothing."** The flagship
/// never-mix test: `acct-a` is throttled seconds ago, `acct-b` served
/// cleanly, and each account's estimate must see only its own rows out of
/// one shared row set — the exact shape the mutation `(a) the never-mix
/// narrowing (an unrelated account's rows leak into the sum)` would break.
#[test]
fn test_1254_two_accounts_estimates_never_mix() {
    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path());
    let ledger = fixture.ledger();
    let now = 1_800_000_000_i64;
    for i in 0..3 {
        ledger
            .record(throttle(Some(LABEL_A), now - 60 - i * 5), now - 55 - i * 5)
            .unwrap();
    }
    for i in 0..3 {
        ledger
            .record(
                accepted(Some(LABEL_B), now - 300 + i * 30),
                now - 295 + i * 30,
            )
            .unwrap();
    }
    let rows = ledger
        .observations_in_window(now, CLASSIFICATION_EVIDENCE_WINDOW_SECONDS)
        .unwrap();

    let a = estimate_subscription_headroom(&rows, PROVIDER, Some(LABEL_A), now, None, None)
        .expect("acct-a's own throttles are real evidence");
    let b = estimate_subscription_headroom(&rows, PROVIDER, Some(LABEL_B), now, None, None)
        .expect("acct-b's own accepted rows are real evidence");

    assert_eq!(
        a.band,
        HeadroomBand::Exhausted,
        "acct-a's own recent throttles, undiluted by acct-b's clean rows: {a:?}"
    );
    assert_eq!(
        b.band,
        HeadroomBand::Ample,
        "acct-b's own clean rows, unpenalised by acct-a's throttles: {b:?}"
    );
    assert!(a.account_narrowed && b.account_narrowed);
}

/// **REQUIRED BEHAVIOR: an account with zero rows reads UNKNOWN, never zero
/// and never full.** No ledger row, no session count, no reset reading —
/// genuinely nothing to estimate from.
#[test]
fn required_behavior_zero_evidence_reads_unknown() {
    let now = 1_800_000_000_i64;
    let estimate = estimate_subscription_headroom(&[], PROVIDER, Some(LABEL_A), now, None, None);
    assert!(
        estimate.is_none(),
        "nothing was observed, so the honest answer is unknown, not a band: {estimate:?}"
    );
}

/// **Map line 1248.** Two throttle→success recoveries in window, with no
/// stated reset reading, learn a fallback window — and the same rows, with a
/// real `Some(_)` reading supplied, leave that reading's own basis untouched
/// rather than recomputing anything from the recoveries at all.
#[test]
fn test_1248_two_or_more_recoveries_learn_a_window_and_never_displace_a_real_reading() {
    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path());
    let ledger = fixture.ledger();
    let now = 1_800_000_000_i64;
    // Two throttle→success recoveries, ~300s apart each.
    ledger
        .record(throttle(Some(LABEL_A), now - 1_000), now - 995)
        .unwrap();
    ledger
        .record(accepted(Some(LABEL_A), now - 700), now - 695)
        .unwrap();
    ledger
        .record(throttle(Some(LABEL_A), now - 500), now - 495)
        .unwrap();
    ledger
        .record(accepted(Some(LABEL_A), now - 200), now - 195)
        .unwrap();
    let rows = ledger
        .observations_in_window(now, CLASSIFICATION_EVIDENCE_WINDOW_SECONDS)
        .unwrap();

    let learned = estimate_subscription_headroom(&rows, PROVIDER, Some(LABEL_A), now, None, None)
        .expect("accepted and throttle rows are real evidence");
    assert_eq!(
        learned.reset_basis,
        ResetBasis::Learned,
        "two recoveries clear the anecdote guard: {learned:?}"
    );

    let stated =
        estimate_subscription_headroom(&rows, PROVIDER, Some(LABEL_A), now, Some(42), None)
            .expect("a stated reading is real evidence");
    assert_eq!(
        stated.reset_basis,
        ResetBasis::Stated,
        "a real seconds_until_reset reading is never recomputed from recoveries: {stated:?}"
    );
}

/// **Map line 1248, the anecdote rule.** One throttle→success recovery is
/// not enough to learn a fallback window — the honest answer stays
/// [`ResetBasis::Unknown`], not a window inferred from a single coincidence.
#[test]
fn test_1248_one_recovery_is_an_anecdote_and_learns_nothing() {
    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path());
    let ledger = fixture.ledger();
    let now = 1_800_000_000_i64;
    ledger
        .record(throttle(Some(LABEL_A), now - 1_000), now - 995)
        .unwrap();
    ledger
        .record(accepted(Some(LABEL_A), now - 700), now - 695)
        .unwrap();
    let rows = ledger
        .observations_in_window(now, CLASSIFICATION_EVIDENCE_WINDOW_SECONDS)
        .unwrap();

    let estimate = estimate_subscription_headroom(&rows, PROVIDER, Some(LABEL_A), now, None, None)
        .expect("accepted rows are real evidence");
    assert_eq!(
        estimate.reset_basis,
        ResetBasis::Unknown,
        "one recovery is an anecdote, not a learned window: {estimate:?}"
    );
}

/// **Map line 1249.** A throttle inside the short horizon and a second one
/// only inside the longer horizon are reported as different pressure: the
/// band still reflects the recent one, and [`LongWindowPressure::Present`]
/// reports the older one separately rather than folding it into the same
/// signal or dropping it.
#[test]
fn test_1249_a_long_only_throttle_is_reported_separately_from_short_window_pressure() {
    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path());
    let ledger = fixture.ledger();
    let now = 1_800_000_000_i64;
    // Inside the short horizon.
    ledger
        .record(throttle(Some(LABEL_A), now - 60), now - 55)
        .unwrap();
    // Outside the short horizon, inside the longer one — two days ago.
    ledger
        .record(
            throttle(Some(LABEL_A), now - 2 * 24 * 3_600),
            now - 2 * 24 * 3_600 + 5,
        )
        .unwrap();
    let rows = ledger
        .observations_in_window(now, CLASSIFICATION_EVIDENCE_WINDOW_SECONDS)
        .unwrap();

    let estimate = estimate_subscription_headroom(&rows, PROVIDER, Some(LABEL_A), now, None, None)
        .expect("throttle rows are real evidence");
    assert_eq!(
        estimate.band,
        HeadroomBand::Exhausted,
        "the recent throttle still drives the band: {estimate:?}"
    );
    assert_eq!(
        estimate.long_window_pressure,
        LongWindowPressure::Present,
        "the two-day-old throttle is reported as its own, separate fact: {estimate:?}"
    );
}

/// **Map line 1249, "when evidence allows."** A single throttle an hour old
/// is real evidence of short-window pressure, but it says nothing at all
/// about whether pressure exists further back — the rows never reached that
/// far. The honest answer is [`LongWindowPressure::Undistinguished`], not a
/// guessed [`LongWindowPressure::NoPressure`].
#[test]
fn test_1249_thin_evidence_renders_undistinguished_not_a_guessed_bucket() {
    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path());
    let ledger = fixture.ledger();
    let now = 1_800_000_000_i64;
    ledger
        .record(throttle(Some(LABEL_A), now - 60), now - 55)
        .unwrap();
    let rows = ledger
        .observations_in_window(now, CLASSIFICATION_EVIDENCE_WINDOW_SECONDS)
        .unwrap();

    let estimate = estimate_subscription_headroom(&rows, PROVIDER, Some(LABEL_A), now, None, None)
        .expect("a throttle is real evidence");
    assert_eq!(
        estimate.long_window_pressure,
        LongWindowPressure::Undistinguished,
        "nothing in `scoped` reaches the long horizon, so absence cannot be claimed: {estimate:?}"
    );
}

// ===========================================================================
// The resolver — proving the wiring, not the band logic again.
// ===========================================================================

/// **REQUIRED BEHAVIOR: with headers DID give per-account data, authoritative
/// readings always beat estimates** — and the estimate still populates
/// alongside a provider-wide reading, exactly the "resolver populates the
/// per-account capacity facet from the estimator where the provider-wide
/// reading is all headers gave" the packet asks for. This is
/// `(b) authoritative-beats-estimate inverted`'s mutation target: inverting
/// the resolver's `capacity_scope != PerAccount` guard would make the
/// estimate vanish here, where a provider-wide capacity reading already
/// exists.
#[test]
fn required_behavior_authoritative_capacity_reading_is_never_displaced() {
    let tmp = tempfile::tempdir().unwrap();
    let quota = GatewayQuotaCache::at(tmp.path().join("gateway-quota"));
    let now = 1_800_000_000_i64;
    // 240 of 300 left — a real, exact, provider-wide reading.
    quota.store(
        PROVIDER,
        &RateLimitHeaders::read(vec![
            ("ratelimit-limit", "300"),
            ("ratelimit-remaining", "240"),
            ("ratelimit-reset", "600"),
        ]),
        now - 30,
    );

    let user = user_config();
    let effective = EffectiveConfig::new(&user, None);
    let telemetry = EntitlementTelemetry::new(now).with_gateway_quota(&quota);
    let entries = effective
        .configured_entitlements_with_telemetry(&telemetry)
        .expect("the pool resolves");
    let a = entries
        .iter()
        .find(|entry| entry.name() == "acct-a")
        .unwrap();

    // The authoritative reading headers actually gave, untouched.
    assert!(
        a.remaining_capacity().is_some(),
        "the planted headers must still resolve to a real reading: {a:?}"
    );
    assert_eq!(
        a.remaining_capacity().unwrap().percent().exact(),
        Some(80),
        "240 of 300 is an exact 80%, the provider's own word: {a:?}"
    );

    // The estimate populates alongside it — no observations were even
    // supplied here, so the only signal it has is the very reset reading
    // the authoritative facet above also carries: real evidence the
    // account is quota-bound, and none at all that it is under pressure,
    // which is exactly `HeadroomBand::Moderate`. A guard that skipped this
    // call whenever an authoritative reading exists would leave this
    // `None` instead — the mutation this test kills.
    let estimate = a
        .headroom_estimate()
        .expect("the reset reading alone is real evidence, even beside an authoritative facet");
    assert_eq!(estimate.band, HeadroomBand::Moderate, "{a:?}");
}

/// The other half of the same guard: with real ledger evidence in hand
/// *and* a provider-wide authoritative reading, the resolver populates
/// **both** facets — the estimate does not wait for the authoritative
/// reading to be absent, because today's gateway cache can never narrow to
/// one account and a caller reading only `remaining_capacity` would see
/// nothing about *this* account at all.
#[test]
fn required_behavior_the_estimate_populates_beside_a_provider_wide_reading() {
    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path());
    let quota = GatewayQuotaCache::at(tmp.path().join("data").join("gateway-quota"));
    let now = 1_800_000_000_i64;
    quota.store(
        PROVIDER,
        &RateLimitHeaders::read(vec![
            ("ratelimit-limit", "300"),
            ("ratelimit-remaining", "240"),
            ("ratelimit-reset", "600"),
        ]),
        now - 30,
    );
    let ledger = fixture.ledger();
    ledger
        .record(accepted(Some(LABEL_A), now - 300), now - 295)
        .unwrap();
    let rows = ledger
        .observations_in_window(now, CLASSIFICATION_EVIDENCE_WINDOW_SECONDS)
        .unwrap();

    let user = user_config();
    let effective = EffectiveConfig::new(&user, None);
    let telemetry = EntitlementTelemetry::new(now)
        .with_gateway_quota(&quota)
        .with_observations(&rows);
    let entries = effective
        .configured_entitlements_with_telemetry(&telemetry)
        .expect("the pool resolves");
    let a = entries
        .iter()
        .find(|entry| entry.name() == "acct-a")
        .unwrap();

    assert!(
        a.remaining_capacity().is_some(),
        "the authoritative reading is still there: {a:?}"
    );
    let estimate = a
        .headroom_estimate()
        .expect("real ledger rows are real evidence, alongside the provider-wide reading");
    assert_eq!(estimate.band, HeadroomBand::Ample, "{estimate:?}");
}

// ===========================================================================
// The shipped binary — practice §35.
// ===========================================================================

fn both_streams(output: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

/// The `glasshouse entitlements` view renders one entry as `` `name`
/// (describe) `` followed by its facets on the next line.
fn facets_line(view: &str, name: &str) -> String {
    let mut lines = view.lines();
    while let Some(line) = lines.next() {
        if line.trim_start().starts_with(&format!("`{name}`")) {
            return lines
                .next()
                .unwrap_or_else(|| panic!("no facet line after `{name}` in:\n{view}"))
                .to_owned();
        }
    }
    panic!("no entry for `{name}` in:\n{view}");
}

/// **REQUIRED BEHAVIOR, flagship: an opaque-limit account's estimate carries
/// no exact token figure anywhere it renders.** `acct-a` has no gateway-quota
/// reading at all (an opaque provider, headers give nothing), real ledger
/// rows carrying a distinctive planted token count, and a real session row
/// this project's own history recorded — map line 1245's "historical
/// sessions" input, read through `sessions.entitlement` rather than faked.
/// The rendered pool view must show a band, a confidence and a basis, and
/// must never show the planted token figure anywhere near it.
#[test]
fn required_behavior_opaque_account_never_renders_an_exact_token_figure_through_the_binary() {
    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path());

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;

    let ledger = fixture.ledger();
    ledger
        .record(
            accepted_with_tokens(Some(LABEL_A), now - 300, 60_222, 7_111),
            now - 295,
        )
        .unwrap();

    let sessions = ProjectSessions::open(fixture.runtime()).unwrap();
    sessions
        .store()
        .create(NewSession::embedded("claude-code").with_entitlement(Some("acct-a".to_owned())))
        .unwrap();

    let out = fixture.glasshouse(&["entitlements"]);
    let said = both_streams(&out);
    assert!(out.status.success(), "{said}");

    let facets = facets_line(&said, "acct-a");
    assert!(
        facets.contains("capacity: unknown"),
        "no gateway-quota reading was ever planted — an opaque provider: {facets}"
    );
    assert!(
        facets.contains("headroom estimate: ~ample"),
        "accepted rows and a served session are real evidence of ordinary use: {facets}"
    );
    assert!(
        facets.contains("token usage"),
        "the scoped row carried a token count, so the basis says so: {facets}"
    );
    assert!(
        facets.contains("low confidence"),
        "every headroom estimate is Confidence::Low today: {facets}"
    );
    assert!(
        !facets.contains("60222") && !facets.contains("7111") && !facets.contains("67333"),
        "no planted token figure may appear anywhere the estimate renders: {facets}"
    );
}

/// **Map line 1252.** `headroom_override` is authoritative over the derived
/// band at the one consumer: a recent throttle with no reset in sight would
/// derive `exhausted` (map line 1245's own worst reading) if nothing
/// overrode it, but the account's stated override reads `ample` instead —
/// and renders in its own distinct vocabulary, never the derived estimate's
/// confidence-and-basis phrasing, so the substitution is never silent.
#[test]
fn test_1252_a_user_override_displaces_a_wrong_derived_band_at_the_consumer() {
    let tmp = tempfile::tempdir().unwrap();
    let pool = format!(
        "[providers.{PROVIDER}]\ntemplate = \"openrouter\"\n\
         credential_env = [\"{VAR_A}\"]\n\n\
         [entitlements.acct-a]\nkind = \"claude\"\nvendor = \"claude\"\n\
         provider = \"{PROVIDER}\"\ncredential = {{ env = \"{VAR_A}\" }}\n\
         headroom_override = \"ample\"\n"
    );
    let fixture = Fixture::with_pool_config(tmp.path(), &pool);
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    let ledger = fixture.ledger();
    ledger
        .record(throttle(Some(LABEL_A), now - 60), now - 55)
        .unwrap();

    let out = fixture.glasshouse(&["entitlements"]);
    let said = both_streams(&out);
    assert!(out.status.success(), "{said}");

    let facets = facets_line(&said, "acct-a");
    assert!(
        facets.contains("headroom estimate: ~ample (your reading"),
        "the override must displace the wrong derived band: {facets}"
    );
    assert!(
        !facets.contains("exhausted"),
        "the derived (and wrong) band must not leak through beside the override: {facets}"
    );
}

/// **Map line 1255.** A disabled entitlement renders `headroom estimate:
/// unknown` — never zero, never a band with a "disabled" label — while an
/// enabled entitlement in the *same* config still renders its own estimate,
/// which is why this is per-entitlement rather than global. Disabling
/// touches nothing else: `capacity` and `throttling` render exactly as they
/// would without the switch.
#[test]
fn test_1255_a_disabled_entitlement_renders_unknown_while_an_enabled_one_beside_it_still_estimates()
{
    let tmp = tempfile::tempdir().unwrap();
    let pool = format!(
        "[providers.{PROVIDER}]\ntemplate = \"openrouter\"\n\
         credential_env = [\"{VAR_A}\", \"{VAR_B}\"]\n\n\
         [entitlements.acct-a]\nkind = \"claude\"\nvendor = \"claude\"\n\
         provider = \"{PROVIDER}\"\ncredential = {{ env = \"{VAR_A}\" }}\n\
         disable_headroom_estimate = true\n\n\
         [entitlements.acct-b]\nkind = \"claude\"\nvendor = \"claude\"\n\
         provider = \"{PROVIDER}\"\ncredential = {{ env = \"{VAR_B}\" }}\n"
    );
    let fixture = Fixture::with_pool_config(tmp.path(), &pool);
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    let ledger = fixture.ledger();
    ledger
        .record(accepted(Some(LABEL_A), now - 300), now - 295)
        .unwrap();
    ledger
        .record(accepted(Some(LABEL_B), now - 300), now - 295)
        .unwrap();

    let out = fixture.glasshouse(&["entitlements"]);
    let said = both_streams(&out);
    assert!(out.status.success(), "{said}");

    let disabled = facets_line(&said, "acct-a");
    assert!(
        disabled.contains("headroom estimate: unknown"),
        "disabled means absent, never zero and never a band: {disabled}"
    );
    assert!(
        disabled.contains("capacity: unknown"),
        "disabling must not disturb the capacity facet beside it: {disabled}"
    );
    assert!(
        disabled.contains("throttling: none observed"),
        "disabling must not disturb the throttling facet beside it: {disabled}"
    );

    let enabled = facets_line(&said, "acct-b");
    assert!(
        enabled.contains("headroom estimate: ~ample"),
        "an enabled entitlement in the same config still estimates: {enabled}"
    );
}

/// **Regression, REQUIRED BEHAVIOR.** With no new config set and no learned
/// window derivable, the rendered line is byte-identical to what `4f0c1cf`
/// shipped: one accepted row is real evidence (`~ample`), but it is alone,
/// so no reset is learned, no long-window throttle exists, and no
/// override/disable config is set — every facet this package adds stays
/// silent. Pins the four honesty facets as additive, never as a change to
/// the shipped surface.
#[test]
fn regression_no_new_signal_renders_byte_identical_to_batch_74() {
    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path());
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    let ledger = fixture.ledger();
    ledger
        .record(accepted(Some(LABEL_A), now - 300), now - 295)
        .unwrap();

    let out = fixture.glasshouse(&["entitlements"]);
    let said = both_streams(&out);
    assert!(out.status.success(), "{said}");

    let facets = facets_line(&said, "acct-a");
    assert_eq!(
        facets.trim(),
        "capacity: unknown · reset: unknown · throttling: none observed (provider-wide) · \
         models: unknown · headroom estimate: ~ample (this account, low confidence, request activity)",
        "no facet this package adds may change the rendered line when nothing new applies: {facets}"
    );
}
