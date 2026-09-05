//! Phase 33C's last line (1366) and Phase 34B's last line (1419) —
//! `design-decisions.md`'s *"Provider cadence learned when no header states
//! it"* and *"The premium capacity a classifier protects"*.
//!
//! # 1366, in three layers
//!
//! [`glasshouse::routing::free::learned_window`] is a pure function over
//! evidence-ledger rows and is proven directly, in-process, against rows
//! written and read back through a real [`EvidenceLedger`] handle — the same
//! shape production writes and reads. [`cadence_availability`]'s provenance
//! phrase is proven directly against a hand-built [`FreePool`]. Only the
//! wiring rule itself — a stated window always wins, and the learner is
//! asked only when neither a window nor a reset was stated —
//! lives in `main.rs::observed_provider_health`, which is private, so that
//! one rule is proven through the shipped binary's `glasshouse route`.
//!
//! # 1419, through the exact function the launch path calls
//!
//! Every test enters through
//! [`DisposableRouting::choose_for_automatic_classification`] directly, the
//! same shape `tests/classification_time_price.rs` uses for its own
//! library-level cases (a)-(d): hand-built candidates, no process. The
//! `protected_capacity_price` a real launch computes from its own fresh
//! destination (`main.rs::launch_session`, private) is reproduced here as a
//! [`ClassificationPolicy`] value, which is exactly what the production
//! chain threads it into.

use std::path::PathBuf;
use std::process::Command;
use std::time::{Duration, Instant};

use clap::Parser;
use glasshouse::integrations::IntegrationId;
use glasshouse::provider::pricing::ModelPrice;
use glasshouse::provider::telemetry::{GatewayQuotaCache, RateLimitHeaders};
use glasshouse::routing::disposable::{
    AutomaticClassificationDecision, ClassificationPolicy, DisposableCandidate, DisposableChoice,
    DisposableRouting, NoResource,
};
use glasshouse::routing::evidence::{EvidenceLedger, FailureClass, NewObservation, Outcome};
use glasshouse::routing::free::{FreePool, FreePreferences, PoolReading, Window};
use glasshouse::routing::session::{Destination, cadence_availability};
use glasshouse::routing::{AssignedModel, Backend, Cost, CredentialId, ToolSemantics};
use glasshouse::secret::SecretRef;
use glasshouse::{Cli, Runtime};

fn credential(provider: &str) -> CredentialId {
    CredentialId::new(
        provider,
        SecretRef::Environment {
            var: format!("{}_KEY", provider.to_uppercase().replace('-', "_")),
        },
    )
}

// ===========================================================================
// A project with only a routing evidence ledger — no CLI, no config.
// ===========================================================================

struct LedgerFixture {
    _tmp: tempfile::TempDir,
    runtime: Runtime,
}

impl LedgerFixture {
    fn new() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path().to_path_buf();
        let root = base.join("workspace");
        std::fs::create_dir_all(root.join(".git")).unwrap();
        let root = std::fs::canonicalize(&root).unwrap();
        let cli = Cli::try_parse_from([
            "glasshouse",
            "--data-dir",
            base.join("data").to_str().unwrap(),
            "--config-dir",
            base.join("config").to_str().unwrap(),
        ])
        .unwrap();
        let runtime = glasshouse::bootstrap(&cli, &root).unwrap();
        Self { _tmp: tmp, runtime }
    }

    /// One `Throttle` row, as the gateway would have written it.
    fn plant_throttle(&self, provider: &str, at: i64) {
        EvidenceLedger::open(&self.runtime)
            .unwrap()
            .record(
                NewObservation::new(provider, "a-model")
                    .with_outcome(Outcome::Failed)
                    .with_failure_class(Some(FailureClass::Throttle)),
                at,
            )
            .unwrap();
    }

    /// Every row the learner would be handed — the same window
    /// `main.rs::observed_provider_health` reads.
    fn rows(&self, now_unix: i64) -> Vec<glasshouse::routing::evidence::RoutingObservation> {
        EvidenceLedger::open(&self.runtime)
            .unwrap()
            .observations_in_window(
                now_unix,
                glasshouse::routing::evidence::CLASSIFICATION_EVIDENCE_WINDOW_SECONDS,
            )
            .unwrap()
    }
}

// ---------------------------------------------------------------------------
// (1366-a) Six throttles, 300s apart: a learned window of 5 samples.
// ---------------------------------------------------------------------------

#[test]
fn six_throttles_300s_apart_learn_a_300s_window_from_5_samples() {
    let fixture = LedgerFixture::new();
    let base = 1_800_000_000_i64;
    for i in 0..6 {
        fixture.plant_throttle("alpha-provider", base + i * 300);
    }
    let now_unix = base + 6 * 300;
    let rows = fixture.rows(now_unix);

    let (window, last_throttle) =
        glasshouse::routing::free::learned_window(&rows, "alpha-provider", now_unix)
            .expect("six throttles clears the MIN_SAMPLE_FOR_SUMMARY floor");
    assert_eq!(
        window,
        Window::Learned {
            seconds: 300,
            sample: 5
        }
    );
    assert_eq!(last_throttle, base + 5 * 300);
}

// ---------------------------------------------------------------------------
// (1366-b) Four throttles: below the floor, no window — and the
// `learner-below-floor` mutation (the floor gate removed) is what this
// proves is watched.
// ---------------------------------------------------------------------------

#[test]
fn four_throttles_are_below_the_floor_and_learn_nothing() {
    let fixture = LedgerFixture::new();
    let base = 1_800_000_000_i64;
    for i in 0..4 {
        fixture.plant_throttle("alpha-provider", base + i * 300);
    }
    let now_unix = base + 4 * 300;
    let rows = fixture.rows(now_unix);

    assert!(
        glasshouse::routing::free::learned_window(&rows, "alpha-provider", now_unix).is_none(),
        "four throttles is one short of MIN_SAMPLE_FOR_SUMMARY (5) and must learn nothing"
    );
}

// ---------------------------------------------------------------------------
// (1366-c) Uneven intervals: the true median, not the last interval — kills
// the `median-is-last` mutation.
// ---------------------------------------------------------------------------

#[test]
fn uneven_intervals_use_the_true_median_not_the_last_interval() {
    let fixture = LedgerFixture::new();
    let base = 1_800_000_000_i64;
    // Six throttles => intervals 100, 100, 100, 100, 900. Sorted, the median
    // (index 2 of 5) is 100 — the last interval planted is 900, so a
    // `median-is-last` mutation reads 900 instead.
    let offsets = [0_i64, 100, 200, 300, 400, 1_300];
    for offset in offsets {
        fixture.plant_throttle("alpha-provider", base + offset);
    }
    let now_unix = base + 1_300;
    let rows = fixture.rows(now_unix);

    let (window, _) = glasshouse::routing::free::learned_window(&rows, "alpha-provider", now_unix)
        .expect("six throttles clears the floor");
    assert_eq!(
        window,
        Window::Learned {
            seconds: 100,
            sample: 5
        },
        "the median of [100, 100, 100, 100, 900] is 100, not the last interval planted"
    );
}

// ===========================================================================
// `cadence_availability`'s provenance phrase, against a hand-built pool.
// ===========================================================================

fn dest(id: &str) -> Destination {
    Destination::fresh(
        id,
        IntegrationId::ClaudeCode,
        "profile",
        Backend::new(
            format!("{id}-provider"),
            "anthropic-messages",
            AssignedModel::named("a-model"),
            credential(&format!("{id}-provider")),
            Cost::Free,
            ToolSemantics::Verified,
        ),
        None,
    )
}

#[test]
fn cadence_availability_names_a_stated_window() {
    let now = Instant::now();
    let mut pool = FreePool::new();
    let d = dest("alpha");
    pool.record_pool(
        d.backend().credential(),
        &PoolReading {
            limit: Some(100),
            remaining: Some(50),
            resets_in: None,
            window: Some(Window::Stated { seconds: 90 }),
        },
        now,
    );

    let contribution = cadence_availability(&d, &pool, now);
    assert!(
        contribution
            .evidence()
            .contains("window stated by the provider (90s)"),
        "{}",
        contribution.evidence()
    );
}

#[test]
fn cadence_availability_names_a_learned_window() {
    let now = Instant::now();
    let mut pool = FreePool::new();
    let d = dest("alpha");
    pool.record_pool(
        d.backend().credential(),
        &PoolReading {
            limit: Some(100),
            remaining: Some(50),
            resets_in: None,
            window: Some(Window::Learned {
                seconds: 300,
                sample: 5,
            }),
        },
        now,
    );

    let contribution = cadence_availability(&d, &pool, now);
    assert!(
        contribution
            .evidence()
            .contains("window learned from 5 throttles (300s)"),
        "{}",
        contribution.evidence()
    );
}

#[test]
fn cadence_availability_says_nothing_about_a_window_when_none_is_held() {
    let now = Instant::now();
    let pool = FreePool::new();
    let d = dest("alpha");

    let contribution = cadence_availability(&d, &pool, now);
    assert!(
        !contribution.evidence().contains("window"),
        "unchanged text when no window is held: {}",
        contribution.evidence()
    );
}

// ===========================================================================
// `Allowance::is_exhausted` against a learned window's own derived reset.
// ===========================================================================

#[test]
fn is_exhausted_holds_until_the_learned_windows_derived_reset_and_then_clears() {
    let now = Instant::now();
    let mut pool = FreePool::new();
    let id = credential("alpha-provider");
    pool.record_pool(
        &id,
        &PoolReading {
            limit: Some(10),
            remaining: Some(0),
            resets_in: Some(Duration::from_secs(300)),
            window: Some(Window::Learned {
                seconds: 300,
                sample: 5,
            }),
        },
        now,
    );

    assert!(pool.allowance(&id).is_exhausted(now));
    assert!(
        !pool
            .allowance(&id)
            .is_exhausted(now + Duration::from_secs(301)),
        "past last_throttle + window, what is left is unknown again, not zero"
    );
}

// ===========================================================================
// A stated window always wins, and the learner is asked only when neither a
// window nor a reset was stated — `main.rs::observed_provider_health`'s own
// wiring, private, so proven through the shipped binary.
// ===========================================================================

const ROUTE_CREDENTIAL_VAR: &str = "GLASSHOUSE_LAST_LINES_ROUTE_KEY";

struct RouteFixture {
    _tmp: tempfile::TempDir,
    base: PathBuf,
    root: PathBuf,
    runtime: Runtime,
}

impl RouteFixture {
    fn new() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path().to_path_buf();
        let root = base.join("workspace");
        std::fs::create_dir_all(root.join(".git")).unwrap();
        let root = std::fs::canonicalize(&root).unwrap();

        let bin_dir = base.join("bin");
        std::fs::create_dir_all(&bin_dir).unwrap();
        let harness_path = bin_dir.join("fake-claude-code");
        std::fs::write(&harness_path, "#!/bin/sh\nexit 0\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = std::fs::metadata(&harness_path).unwrap().permissions();
            perms.set_mode(0o755);
            std::fs::set_permissions(&harness_path, perms).unwrap();
        }
        let escaped = harness_path.display().to_string().replace('\\', "\\\\");

        let config_dir = base.join("config");
        std::fs::create_dir_all(&config_dir).unwrap();
        std::fs::write(
            config_dir.join("config.toml"),
            format!(
                "version = 1\n\n\
                 [integrations.claude-code]\nenabled = true\nexecutable = \"{escaped}\"\n\n\
                 [providers.route-probe]\ntemplate = \"openrouter\"\n\
                 credential_env = [\"{ROUTE_CREDENTIAL_VAR}\"]\n\n\
                 [profiles.direct]\nharness = \"claude-code\"\n\
                 expected_protocol = \"anthropic-messages\"\n\n\
                 [profiles.direct.backend]\nkind = \"direct-provider\"\n\
                 provider = \"route-probe\"\n"
            ),
        )
        .unwrap();

        let cli = Cli::try_parse_from([
            "glasshouse",
            "--data-dir",
            base.join("data").to_str().unwrap(),
            "--config-dir",
            config_dir.to_str().unwrap(),
        ])
        .unwrap();
        let runtime = glasshouse::bootstrap(&cli, &root).unwrap();

        Self {
            _tmp: tmp,
            base,
            root,
            runtime,
        }
    }

    fn data_dir(&self) -> PathBuf {
        self.base.join("data")
    }

    /// A request-pool reading with a real limit and remaining count, and
    /// `window_seconds` only when `stated_window` names one — the same
    /// header shape `tests/route_command.rs::plant_quota` plants, extended
    /// with `x-ratelimit-window`.
    fn plant_quota(&self, provider: &str, remaining: i64, limit: i64, stated_window: Option<i64>) {
        let cache = GatewayQuotaCache::at(self.data_dir().join("gateway-quota"));
        let limit_s = limit.to_string();
        let remaining_s = remaining.to_string();
        let window_s = stated_window.map(|seconds| seconds.to_string());
        let mut pairs: Vec<(&str, &str)> = vec![
            ("ratelimit-limit", limit_s.as_str()),
            ("ratelimit-remaining", remaining_s.as_str()),
        ];
        if let Some(window_s) = window_s.as_deref() {
            pairs.push(("x-ratelimit-window", window_s));
        }
        cache.store(provider, &RateLimitHeaders::read(pairs), now_unix());
        assert!(
            cache.load(provider).is_some(),
            "the planted quota reading for `{provider}` must be on disk and readable"
        );
    }

    fn plant_throttle(&self, provider: &str, at: i64) {
        EvidenceLedger::open(&self.runtime)
            .unwrap()
            .record(
                NewObservation::new(provider, "a-model")
                    .with_outcome(Outcome::Failed)
                    .with_failure_class(Some(FailureClass::Throttle)),
                at,
            )
            .unwrap();
    }

    fn route(&self) -> String {
        let output = Command::new(env!("CARGO_BIN_EXE_glasshouse"))
            .env(ROUTE_CREDENTIAL_VAR, "planted-opaque-route-value-not-real")
            .arg("--scope")
            .arg(&self.root)
            .arg("--data-dir")
            .arg(self.base.join("data"))
            .arg("--config-dir")
            .arg(self.base.join("config"))
            .arg("route")
            .output()
            .expect("the glasshouse binary must be runnable");
        String::from_utf8_lossy(&output.stdout).into_owned()
    }
}

fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("the system clock is after 1970")
        .as_secs() as i64
}

#[test]
fn route_names_a_window_learned_from_six_throttles() {
    let fixture = RouteFixture::new();
    fixture.plant_quota("route-probe", 40, 100, None);
    let base = now_unix() - 6 * 300;
    for i in 0..6 {
        fixture.plant_throttle("route-probe", base + i * 300);
    }

    let report = fixture.route();
    assert!(
        report.contains("window learned from 5 throttles (300s)"),
        "{report}"
    );
}

#[test]
fn a_stated_window_is_used_over_a_learned_one() {
    let fixture = RouteFixture::new();
    // A stated window this time, over the same six planted throttles a
    // learner would otherwise use.
    fixture.plant_quota("route-probe", 40, 100, Some(90));
    let base = now_unix() - 6 * 300;
    for i in 0..6 {
        fixture.plant_throttle("route-probe", base + i * 300);
    }

    let report = fixture.route();
    assert!(
        report.contains("window stated by the provider (90s)"),
        "{report}"
    );
    assert!(
        !report.contains("learned from"),
        "a stated window must never be replaced by a learned one: {report}"
    );
}

// ===========================================================================
// 1419 — the protected-capacity term, through the exact production function.
// ===========================================================================

fn free_candidate(provider: &str, model: &str) -> DisposableCandidate {
    DisposableCandidate::new(provider, model, credential(provider), Cost::Free)
}

fn metered_candidate(provider: &str, model: &str) -> DisposableCandidate {
    DisposableCandidate::new(provider, model, credential(provider), Cost::Metered)
}

fn support_routing_with(policy: ClassificationPolicy) -> DisposableRouting {
    DisposableRouting::for_support_work(true, FreePreferences::new())
        .with_classification_policy(policy)
}

/// One fresh automatic-classification decision — the production entry
/// point, with no retained pick and an empty health pool, matching
/// `tests/classification_time_price.rs`'s own `decide` exactly.
fn decide(
    routing: &DisposableRouting,
    candidates: &[DisposableCandidate],
) -> Result<DisposableChoice, NoResource> {
    match routing.choose_for_automatic_classification(
        candidates,
        &FreePool::new(),
        Instant::now(),
        1_800_000_000,
        None,
        None,
    )? {
        AutomaticClassificationDecision::Fresh(choice, _) => Ok(choice),
        AutomaticClassificationDecision::Retained(choice) => {
            panic!("no retained pick was supplied, yet one was reused: {choice:?}")
        }
    }
}

fn protected_price() -> ModelPrice {
    ModelPrice {
        input_per_million_usd: 1.0,
        output_per_million_usd: 1.0,
        cached_input_per_million_usd: None,
    }
}

fn scaled(price: ModelPrice, factor: f64) -> ModelPrice {
    ModelPrice {
        input_per_million_usd: price.input_per_million_usd * factor,
        output_per_million_usd: price.output_per_million_usd * factor,
        cached_input_per_million_usd: None,
    }
}

// ---------------------------------------------------------------------------
// (1419-a) A candidate at 1/50 of the protected price carries `+1.0` and
// wins over one at 1/2 — kills the `ratio-inverted` mutation: fed
// dear-first, so a tie under an inverted ratio (both candidates' magnitudes
// collapse to the same bound) would make the wrong one win by insertion
// order alone.
// ---------------------------------------------------------------------------

#[test]
fn a_materially_cheaper_candidate_carries_protected_capacity_and_wins() {
    let policy = ClassificationPolicy::new().with_protected_capacity_price(Some(protected_price()));
    let routing = support_routing_with(policy);

    let cheap = metered_candidate("cheap-runner", "cheap-model")
        .with_price(Some(scaled(protected_price(), 0.02)));
    let dear = metered_candidate("dear-runner", "dear-model")
        .with_price(Some(scaled(protected_price(), 0.5)));

    let choice = decide(&routing, &[dear, cheap]).expect("both candidates remain admitted");
    let explanation = choice.explanation().render();
    assert_eq!(
        choice.provider(),
        "cheap-runner",
        "the materially cheaper candidate must be preferred: {explanation}"
    );
    assert!(explanation.contains("protected capacity"), "{explanation}");
    assert!(
        explanation.contains("materially lower (map line 1419)")
            && !explanation.contains("not materially lower"),
        "{explanation}"
    );
}

// ---------------------------------------------------------------------------
// (1419-b) A pricier candidate is not excluded — it carries a negative note
// and is still chosen when it is the only candidate — kills the
// `term-excludes` mutation.
// ---------------------------------------------------------------------------

#[test]
fn a_pricier_candidate_is_not_excluded_and_carries_a_negative_note() {
    let policy = ClassificationPolicy::new().with_protected_capacity_price(Some(protected_price()));
    let routing = support_routing_with(policy);

    let dear = metered_candidate("dear-runner", "dear-model")
        .with_price(Some(scaled(protected_price(), 0.5)));

    let choice = decide(&routing, &[dear]).expect(
        "a candidate carrying a negative protected-capacity note must still be admitted, never \
         excluded (map line 1419 only orders)",
    );
    assert_eq!(choice.provider(), "dear-runner");
    let explanation = choice.explanation().render();
    assert!(
        explanation.contains("not materially lower (map line 1419)"),
        "{explanation}"
    );
    let note_line = explanation
        .lines()
        .find(|line| line.contains("protected capacity"))
        .unwrap_or_else(|| {
            panic!("the note must be printed in the winner's explanation: {explanation}")
        });
    assert!(
        note_line.trim_start().starts_with('-'),
        "the note's own magnitude must be negative, not merely its text: {note_line}"
    );
}

// ---------------------------------------------------------------------------
// (1419-c) No protected price at all: the term is inert and says why —
// kills the `unpriced-read-as-free` mutation (a `None` protected price
// scored `+1.0`).
// ---------------------------------------------------------------------------

#[test]
fn no_protected_price_leaves_the_term_inert() {
    let policy = ClassificationPolicy::new(); // no protected_capacity_price
    let routing = support_routing_with(policy);
    let candidate =
        metered_candidate("solo-runner", "solo-model").with_price(Some(protected_price()));

    let choice = decide(&routing, &[candidate]).unwrap();
    let explanation = choice.explanation().render();
    assert!(
        explanation.contains(
            "the launch's destinations are unpriced — nothing to compare against (map line 1419)"
        ),
        "{explanation}"
    );
    let note_line = explanation
        .lines()
        .find(|line| line.contains("protected capacity"))
        .unwrap_or_else(|| {
            panic!("the note must be printed in the winner's explanation: {explanation}")
        });
    assert!(
        note_line.trim_start().starts_with("+0.000"),
        "an unpriced-policy note is inert and must carry no magnitude, never a `+1.0` \
         \"materially lower\" score: {note_line}"
    );
}

// ---------------------------------------------------------------------------
// (1419-d) A free candidate protects everything by construction — the term
// is `0.0` with its own reason, distinct from the unpriced-metered case.
// ---------------------------------------------------------------------------

#[test]
fn a_free_candidate_protects_everything_it_is_asked_to() {
    let policy = ClassificationPolicy::new().with_protected_capacity_price(Some(protected_price()));
    let routing = support_routing_with(policy);
    let candidate = free_candidate("free-runner", "free-model");

    let choice = decide(&routing, &[candidate]).unwrap();
    let explanation = choice.explanation().render();
    assert!(
        explanation.contains("free — protects everything it is asked to (map line 1419)"),
        "{explanation}"
    );
}

// ---------------------------------------------------------------------------
// (1419-e) A metered candidate with no price of its own is also inert, and
// says so distinctly from the "no protected price" case above.
// ---------------------------------------------------------------------------

#[test]
fn an_unpriced_metered_candidate_is_inert_and_says_so() {
    let policy = ClassificationPolicy::new().with_protected_capacity_price(Some(protected_price()));
    let routing = support_routing_with(policy);
    let candidate = metered_candidate("unpriced-runner", "unpriced-model"); // no .with_price

    let choice = decide(&routing, &[candidate]).unwrap();
    let explanation = choice.explanation().render();
    assert!(
        explanation.contains("unpriced candidate (map line 1419)"),
        "{explanation}"
    );
}
