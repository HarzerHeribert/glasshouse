//! Capability map line 1439 — *"prefer a cheap metered model over an
//! unreliable free model when failed routing attempts would cost more time
//! than the price difference"* — `design-decisions.md`'s "Preferring a cheap
//! metered classifier over an unreliable free one", **amended 2026-09-02**.
//!
//! # The amendment, and why these fixtures compare two measured latencies
//!
//! The first version of this rule compared a free candidate's expected
//! wasted retry time against `[routing] max_router_latency` — the same
//! ceiling `classification_verdict`'s own 1435 gate excludes a candidate's
//! *raw* median on. Since expected wasted time is never more than that raw
//! median, the rule could only ever fire on a candidate 1435 (and, below the
//! 80% parse floor, 1432) had already excluded — an account of an exclusion,
//! never a preference that could change a choice. The orchestrator's ruling
//! withdrew that comparison. The amended rule compares the free candidate's
//! expected wasted time against **the metered candidate's own measured
//! median classification latency** instead — two independently-measured
//! times, neither one bounded by the other, so this rule can genuinely fire
//! on a candidate the router was about to choose. `max_router_latency` plays
//! no part here.
//!
//! # Two levels, matching `tests/routing_economics.rs`
//!
//! Tests (a)-(d) and (f) enter through
//! [`DisposableRouting::choose_for_automatic_classification`] directly, the
//! exact function `main.rs::automatic_classification_choice` calls, with
//! candidates built by hand — the same shape `tests/routing_economics.rs`
//! already uses for lines 1420-1438, this line's own siblings.
//!
//! Test (e) is the one claim only the shipped binary can prove — that a
//! *retained* pick from a previous process is overridden, not reused, once
//! its inputs newly fire the preference — and runs `glasshouse resources
//! --no-harness --verbose` twice against the same data/config directories so
//! `RoutingStickyCache` round-trips through disk between the two calls,
//! exactly as `tests/classification_call.rs`'s own stickiness test does for
//! map lines 1441/1442. The tightening between the two calls is done by
//! planting a newly-measured classification record for the *metered*
//! candidate (the amended rule's own comparison), not by touching any
//! config knob.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Instant;

use clap::Parser;
use glasshouse::config::{ProviderConfig, RouterCostMicroUsd, RoutingModelChoice, UserConfig};
use glasshouse::provider::pricing::{ModelPrice, PRICING_FILE_NAME};
use glasshouse::routing::disposable::{
    AutomaticClassificationDecision, CLASSIFICATION_RELIABILITY_MIN_OBSERVATIONS,
    ClassificationPolicy, DisposableCandidate, DisposableChoice, DisposableRouting, NoResource,
};
use glasshouse::routing::evidence::{
    CLASSIFICATION_PURPOSE, ClassificationRecord, EvidenceLedger, MIN_SAMPLE_FOR_SUMMARY,
    NewObservation, Outcome,
};
use glasshouse::routing::free::{FreePool, FreePreferences};
use glasshouse::routing::{Cost, CredentialId};
use glasshouse::secret::SecretRef;
use glasshouse::{Cli, Runtime};

// ===========================================================================
// Policy-level helpers — the same shapes `tests/routing_economics.rs` uses.
// ===========================================================================

fn credential(provider: &str) -> CredentialId {
    CredentialId::new(
        provider,
        SecretRef::Environment {
            var: format!("{}_API_KEY", provider.to_uppercase().replace('-', "_")),
        },
    )
}

fn free(provider: &str, model: &str) -> DisposableCandidate {
    DisposableCandidate::new(provider, model, credential(provider), Cost::Free)
}

fn metered(provider: &str, model: &str) -> DisposableCandidate {
    DisposableCandidate::new(provider, model, credential(provider), Cost::Metered)
}

/// A classification record as the ledger would hand it back: `parsed` of
/// `outcomes` calls in the schema, and a median latency only when one is
/// given.
fn record(
    provider: &str,
    model: &str,
    parsed: usize,
    outcomes: usize,
    median_ms: Option<i64>,
) -> ClassificationRecord {
    ClassificationRecord {
        provider: provider.to_owned(),
        model: model.to_owned(),
        outcomes_recorded: outcomes,
        parsed,
        timed: if median_ms.is_some() {
            MIN_SAMPLE_FOR_SUMMARY
        } else {
            0
        },
        median_duration_ms: median_ms,
    }
}

fn support_routing() -> DisposableRouting {
    DisposableRouting::for_support_work(true, FreePreferences::new())
}

fn policy(max_marginal_cost_micro_usd: Option<u32>) -> ClassificationPolicy {
    ClassificationPolicy::new().with_max_marginal_cost_micro_usd(max_marginal_cost_micro_usd)
}

/// One fresh automatic-classification decision — the production entry
/// point, with no retained pick and an empty health pool. Panics if a
/// retained pick were somehow reused, matching
/// `tests/routing_economics.rs`'s own `decide` exactly: none of these tests
/// supply one.
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

fn rendered(choice: &DisposableChoice) -> String {
    choice.explanation().render()
}

fn cheap_price() -> ModelPrice {
    ModelPrice {
        input_per_million_usd: 0.01,
        output_per_million_usd: 0.01,
        cached_input_per_million_usd: None,
    }
}

// ---------------------------------------------------------------------------
// (a) The free candidate's expected wasted time exceeds the metered
// candidate's own median latency, and the metered candidate is cheap
// enough: the metered candidate is chosen, and the explanation carries both
// figures.
// ---------------------------------------------------------------------------

#[test]
fn an_unreliable_free_candidate_is_passed_over_for_a_faster_cheap_metered_one() {
    let routing = support_routing().with_classification_policy(policy(Some(1_000_000)));

    // 10 outcomes, 8 parsed, median 900ms: expected wasted time is
    // (1 - 0.8) * 900 = 180ms.
    let unreliable = free("alpha", "alpha-model").with_classification_record(Some(record(
        "alpha",
        "alpha-model",
        8,
        10,
        Some(900),
    )));
    // The metered candidate's own median is 100ms — comfortably under the
    // free candidate's 180ms of expected wasted time.
    let faster_metered = metered("beta", "beta-model")
        .with_classification_record(Some(record("beta", "beta-model", 5, 5, Some(100))))
        .with_price(Some(cheap_price()));

    let choice = decide(&routing, &[unreliable, faster_metered]).expect("beta remains admitted");
    assert_eq!(
        choice.provider(),
        "beta",
        "the faster, cheap metered candidate must be preferred over the unreliable free one: {}",
        rendered(&choice)
    );
    assert_eq!(choice.cost(), Cost::Metered);

    let explanation = rendered(&choice);
    assert!(
        explanation.contains("free alpha-model expects 180ms of wasted retries per call"),
        "the explanation must name the wasted-time figure:\n{explanation}"
    );
    assert!(
        explanation.contains("over metered beta-model's own 100ms median classification latency"),
        "the explanation must compare against the metered candidate's own measured latency, not \
         a configured limit:\n{explanation}"
    );
    assert!(
        explanation.contains("metered beta-model at"),
        "the explanation must name the metered candidate's estimated cost:\n{explanation}"
    );
    assert!(
        explanation.contains("map line 1439"),
        "the explanation must cite the ruling line:\n{explanation}"
    );
}

// ---------------------------------------------------------------------------
// (b) The free candidate's expected wasted time is within the metered
// candidate's own (slower) median latency: the free candidate is kept, and
// the note says so.
// ---------------------------------------------------------------------------

#[test]
fn a_free_candidate_is_kept_when_its_wasted_time_is_within_the_metered_candidates_own_latency() {
    let routing = support_routing().with_classification_policy(policy(Some(1_000_000)));

    // Same free candidate as (a): 180ms of expected wasted time.
    let reliable_enough = free("alpha", "alpha-model").with_classification_record(Some(record(
        "alpha",
        "alpha-model",
        8,
        10,
        Some(900),
    )));
    // This time the metered candidate's own median is 500ms — slower than
    // the free candidate's 180ms of expected wasted time.
    let slower_metered = metered("beta", "beta-model")
        .with_classification_record(Some(record("beta", "beta-model", 5, 5, Some(500))))
        .with_price(Some(cheap_price()));

    let choice =
        decide(&routing, &[reliable_enough, slower_metered]).expect("alpha remains admitted");
    assert_eq!(
        choice.provider(),
        "alpha",
        "within the metered candidate's own latency, the free candidate must not be passed \
         over: {}",
        rendered(&choice)
    );

    let explanation = rendered(&choice);
    assert!(
        explanation.contains("free alpha-model expects 180ms of wasted retries per call"),
        "the explanation must name the wasted-time figure:\n{explanation}"
    );
    assert!(
        explanation.contains("within metered beta-model's own 500ms median classification latency"),
        "the note must say the wasted time is within the metered call's own latency:\n\
         {explanation}"
    );
}

// ---------------------------------------------------------------------------
// (c) No marginal-cost ceiling is configured: the policy's inert default is
// that no candidate is ever cheap enough, so the preference stays inert
// (even though the free candidate's wasted time does exceed the metered
// candidate's own latency) and the free candidate is kept.
// ---------------------------------------------------------------------------

#[test]
fn a_free_candidate_is_kept_when_no_marginal_cost_ceiling_is_configured() {
    let routing = support_routing().with_classification_policy(policy(None));

    // Same shape as (a): 180ms of expected wasted time, over the metered
    // candidate's own 100ms median — the latency half of the rule holds.
    let unreliable = free("alpha", "alpha-model").with_classification_record(Some(record(
        "alpha",
        "alpha-model",
        8,
        10,
        Some(900),
    )));
    let faster_metered = metered("beta", "beta-model")
        .with_classification_record(Some(record("beta", "beta-model", 5, 5, Some(100))))
        .with_price(Some(cheap_price()));

    let choice = decide(&routing, &[unreliable, faster_metered]).expect("alpha remains admitted");
    assert_eq!(
        choice.provider(),
        "alpha",
        "with no cost ceiling configured, no candidate is ever cheap enough, so the free \
         candidate must be kept: {}",
        rendered(&choice)
    );
    assert_eq!(choice.cost(), Cost::Free);

    let explanation = rendered(&choice);
    assert!(
        explanation.contains("free alpha-model expects 180ms of wasted retries per call")
            && explanation
                .contains("over metered beta-model's own 100ms median classification latency"),
        "the note must still name the unreliable comparison the preference was asked about:\n\
         {explanation}"
    );
    assert!(
        explanation.contains("no maximum marginal cost is configured"),
        "the note must say why the cost half never applied:\n{explanation}"
    );
    assert!(
        explanation.contains("map line 1439"),
        "the note must cite the ruling line:\n{explanation}"
    );
}

// ---------------------------------------------------------------------------
// (d) Below the sample floor: the preference is inert, and the note says
// unmeasured, never unreliable.
// ---------------------------------------------------------------------------

#[test]
fn a_free_candidate_below_the_sample_floor_is_treated_as_unmeasured_by_the_preference() {
    let routing = support_routing().with_classification_policy(policy(Some(1_000_000)));

    // 3 outcomes is below CLASSIFICATION_RELIABILITY_MIN_OBSERVATIONS (5) —
    // below the floor with no median at all, matching how the ledger itself
    // never attaches one below the floor.
    let unproven = free("alpha", "alpha-model").with_classification_record(Some(record(
        "alpha",
        "alpha-model",
        2,
        3,
        None,
    )));
    let admitted_metered = metered("beta", "beta-model")
        .with_classification_record(Some(record("beta", "beta-model", 5, 5, Some(50))))
        .with_price(Some(cheap_price()));

    let choice = decide(&routing, &[unproven, admitted_metered]).expect("alpha remains admitted");
    assert_eq!(
        choice.provider(),
        "alpha",
        "below the sample floor, the preference must never fire: {}",
        rendered(&choice)
    );

    let explanation = rendered(&choice);
    assert!(
        explanation.contains("unmeasured: 2 of 3 classification calls parsed"),
        "the note must say unmeasured, not unreliable:\n{explanation}"
    );
    assert!(
        explanation.contains(&format!(
            "fewer than the {CLASSIFICATION_RELIABILITY_MIN_OBSERVATIONS} needed"
        )),
        "the note must name the floor it fell under:\n{explanation}"
    );
}

// ---------------------------------------------------------------------------
// (f) Unit: the rule's own arithmetic. A parsed fraction of 1.0 wastes
// exactly zero time, so it can never exceed any metered candidate's median
// (zero or otherwise).
// ---------------------------------------------------------------------------

#[test]
fn a_parsed_fraction_of_one_wastes_zero_time() {
    let routing = support_routing().with_classification_policy(policy(Some(1_000_000)));

    let flawless = free("alpha", "alpha-model").with_classification_record(Some(record(
        "alpha",
        "alpha-model",
        10,
        10,
        Some(2000),
    )));
    // Even a very fast metered candidate cannot beat zero wasted time.
    let fast_metered = metered("beta", "beta-model")
        .with_classification_record(Some(record("beta", "beta-model", 5, 5, Some(1))))
        .with_price(Some(cheap_price()));

    let choice = decide(&routing, &[flawless, fast_metered]).expect("alpha remains admitted");
    assert_eq!(
        choice.provider(),
        "alpha",
        "a parsed fraction of 1.0 wastes no time, so the metered candidate must never be \
         preferred: {}",
        rendered(&choice)
    );

    let explanation = rendered(&choice);
    assert!(
        explanation.contains("free alpha-model expects 0ms of wasted retries per call"),
        "the arithmetic must compute exactly zero wasted time, not the raw median:\n{explanation}"
    );
}

// ===========================================================================
// (e) The shipped binary: a retained free pick is overridden, not reused,
// once the metered candidate's own record newly fires the preference.
// ===========================================================================

struct Fixture {
    _tmp: tempfile::TempDir,
    base: PathBuf,
    root: PathBuf,
    runtime: Runtime,
}

struct Ran {
    stdout: String,
    stderr: String,
    status: std::process::ExitStatus,
}

const CREDENTIAL_VAR: &str = "GLASSHOUSE_TEST_ONLY_TIME_PRICE_KEY";
const CREDENTIAL: &str = "sk-fabricated-test-value-not-a-real-credential";
const ROUTE: &str = "openai-chat";

impl Fixture {
    fn new() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path().to_path_buf();
        let root = base.join("workspace");
        std::fs::create_dir_all(root.join(".git")).unwrap();
        let root = std::fs::canonicalize(&root).unwrap();
        let runtime = bootstrap(&base, &root);
        Self {
            _tmp: tmp,
            base,
            root,
            runtime,
        }
    }

    fn config(&self) -> UserConfig {
        UserConfig::load(self.runtime.paths()).unwrap()
    }

    fn save(&self, user: UserConfig) {
        user.save(self.runtime.paths()).unwrap();
    }

    fn add_free_provider(&self, name: &str, model: &str) {
        let mut user = self.config();
        let mut provider = ProviderConfig::new("openai-compatible");
        provider.set_base_url(Some("http://127.0.0.1:1/v1".to_owned()));
        provider.set_credential_env(vec![CREDENTIAL_VAR.to_owned()]);
        provider.set_free_models(vec![model.to_owned()]);
        user.providers_mut().set(name, provider);
        self.save(user);
    }

    fn add_metered_provider(&self, name: &str, model: &str) {
        let mut user = self.config();
        let mut provider = ProviderConfig::new("openai-compatible");
        provider.set_base_url(Some("http://127.0.0.1:1/v1".to_owned()));
        provider.set_credential_env(vec![CREDENTIAL_VAR.to_owned()]);
        provider.set_metered_models(vec![model.to_owned()]);
        user.providers_mut().set(name, provider);
        self.save(user);
    }

    fn automatic_routing_model(&self) {
        let mut user = self.config();
        user.routing_mut()
            .set_model(Some(RoutingModelChoice::Automatic));
        self.save(user);
    }

    fn set_max_marginal_cost(&self, micro_usd: u32) {
        let mut user = self.config();
        user.routing_mut()
            .set_max_marginal_cost(Some(RouterCostMicroUsd::try_from(micro_usd).unwrap()));
        self.save(user);
    }

    fn plant_pricing(&self, contents: &str) {
        std::fs::write(self.base.join("config").join(PRICING_FILE_NAME), contents)
            .expect("write pricing.toml");
    }

    /// Plant one classification row as the producer would have written it,
    /// with an explicit duration — `dispatched_at`/`completed_at` are whole
    /// unix seconds, so a `duration_seconds` of `2` gives an exact 2000ms
    /// row every time, with no dependence on real wall-clock timing.
    fn plant_classification(
        &self,
        provider: &str,
        model: &str,
        outcome: Outcome,
        at: i64,
        duration_seconds: i64,
    ) {
        EvidenceLedger::open(&self.runtime)
            .unwrap()
            .record(
                NewObservation::new(provider, model)
                    .with_route(Some(ROUTE))
                    .with_purpose(Some(CLASSIFICATION_PURPOSE))
                    .with_timing(Some(at), Some(at + duration_seconds))
                    .with_tokens(Some(50), Some(50), None)
                    .with_outcome(outcome),
                at,
            )
            .unwrap();
    }

    fn resources_verbose(&self) -> Ran {
        let output = Command::new(env!("CARGO_BIN_EXE_glasshouse"))
            .env(CREDENTIAL_VAR, CREDENTIAL)
            .arg("--scope")
            .arg(&self.root)
            .arg("--data-dir")
            .arg(self.base.join("data"))
            .arg("--config-dir")
            .arg(self.base.join("config"))
            .arg("resources")
            .arg("--no-harness")
            .arg("--verbose")
            .output()
            .expect("the glasshouse binary must be runnable");
        Ran {
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
            status: output.status,
        }
    }
}

fn bootstrap(base: &Path, root: &Path) -> Runtime {
    let cli = Cli::try_parse_from([
        "glasshouse",
        "--data-dir",
        base.join("data").to_str().unwrap(),
        "--config-dir",
        base.join("config").to_str().unwrap(),
    ])
    .unwrap();
    glasshouse::bootstrap(&cli, root).unwrap()
}

#[test]
fn a_retained_free_pick_whose_inputs_now_fire_the_rule_is_overridden_and_not_reused() {
    let price = ModelPrice {
        input_per_million_usd: 0.001,
        output_per_million_usd: 0.001,
        cached_input_per_million_usd: None,
    };

    let fixture = Fixture::new();
    fixture.add_free_provider("alpha-runner", "alpha-model");
    fixture.add_metered_provider("beta-runner", "beta-model");
    fixture.plant_pricing(&format!(
        "[[prices]]\nprovider = \"beta-runner\"\nmodel = \"beta-model\"\n\
         input_per_million_usd = {}\noutput_per_million_usd = {}\n",
        price.input_per_million_usd, price.output_per_million_usd,
    ));
    fixture.automatic_routing_model();
    fixture.set_max_marginal_cost(1_000_000);

    // The free candidate's own record: 10 outcomes, 8 succeeded, each with
    // an exact 2000ms duration (the timing columns are whole unix seconds,
    // so every achievable duration is a multiple of 1000ms). Parsed
    // fraction 0.8, median 2000ms, expected wasted time (1 - 0.8) * 2000 =
    // 400ms. Planted once and never touched again — only the metered
    // candidate's own record changes between the two calls below.
    let now = glasshouse::provider::cache::now_unix_seconds();
    for i in 0..8 {
        fixture.plant_classification(
            "alpha-runner",
            "alpha-model",
            Outcome::Succeeded,
            now - 6000 + i * 10,
            2,
        );
    }
    for i in 0..2 {
        fixture.plant_classification(
            "alpha-runner",
            "alpha-model",
            Outcome::Failed,
            now - 6000 + 80 + i * 10,
            2,
        );
    }

    // Call 1: the metered candidate has no classification history at all —
    // the preference is inert (unmeasured), and the free candidate is
    // chosen and retained.
    let first = fixture.resources_verbose();
    assert!(first.status.success(), "{}", first.stderr);
    assert!(
        first
            .stdout
            .contains("would select    alpha-model on alpha-runner"),
        "with the metered candidate unmeasured, the retained pick must be the free candidate:\n\
         {}",
        first.stdout
    );

    // Plant the metered candidate's own record, inside the sticky window:
    // 5 outcomes, each with an exact 0ms duration (dispatched and completed
    // in the same second) — faster than any positive expected wasted time,
    // including the free candidate's 400ms.
    for i in 0..5 {
        fixture.plant_classification(
            "beta-runner",
            "beta-model",
            Outcome::Succeeded,
            now - 100 + i * 5,
            0,
        );
    }
    let second = fixture.resources_verbose();
    assert!(second.status.success(), "{}", second.stderr);
    assert!(
        second
            .stdout
            .contains("would select    beta-model on beta-runner"),
        "the metered candidate's newly-measured, faster latency must fire the preference and \
         choose it:\n{}",
        second.stdout
    );
    assert!(
        !second.stdout.contains("reused without re-ranking"),
        "the retained free pick must not be reused once the metered candidate's record fires \
         the preference:\n{}",
        second.stdout
    );
    assert!(
        second.stdout.contains("map line 1439"),
        "the explanation must cite the ruling line:\n{}",
        second.stdout
    );
}
