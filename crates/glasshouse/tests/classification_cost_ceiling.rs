//! Capability map line 1436 — *"filter automatic candidates by maximum
//! marginal routing cost"* — exercised through the shipped binary.
//!
//! `glasshouse resources --no-harness --verbose` runs the real automatic
//! classification decision (`main.rs::automatic_classification_choice`, the
//! same function `glasshouse classify` calls) with no request in hand and
//! prints its explanation — the diagnostic
//! `tests/routing_economics.rs`'s own 1432/1435 tests already read the same
//! way. No network call is made on this path: the exclusion is decided
//! before any model would be asked, so a fake upstream is unnecessary here.
//!
//! Fixture shape follows `tests/classification_call.rs`: a real project
//! directory, real `UserConfig`, and a real `pricing.toml` written where
//! `PriceTable::load_from_dir` resolves it from — the same fixture shape
//! `tests/route_command.rs` uses for its own 1305/1306 tests.

use std::path::{Path, PathBuf};
use std::process::Command;

use clap::Parser;
use glasshouse::config::{ProviderConfig, RouterCostMicroUsd, RoutingModelChoice, UserConfig};
use glasshouse::provider::pricing::{ModelPrice, PRICING_FILE_NAME};
use glasshouse::routing::classify::CLASSIFICATION_PROMPT_CONTRACT;
use glasshouse::routing::disposable::estimated_classification_cost_micro_usd;
use glasshouse::routing::request::TASK_TEXT_CEILING_BYTES;
use glasshouse::{Cli, Runtime};

/// The variable every fixture provider's credential is read from —
/// `disposable_candidates` only builds a candidate for a provider whose
/// credential actually resolves.
const CREDENTIAL_VAR: &str = "GLASSHOUSE_TEST_ONLY_COST_CEILING_KEY";
const CREDENTIAL: &str = "sk-fabricated-test-value-not-a-real-credential";

// ---------------------------------------------------------------------------
// A project, and `glasshouse resources` run against it as a process.
// ---------------------------------------------------------------------------

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

    /// Add a provider naming `model` as free — never excluded by the price
    /// ceiling, whatever `pricing.toml` says about it.
    fn add_free_provider(&self, name: &str, model: &str) {
        let mut user = self.config();
        let mut provider = ProviderConfig::new("openai-compatible");
        provider.set_base_url(Some("http://127.0.0.1:1/v1".to_owned()));
        provider.set_credential_env(vec![CREDENTIAL_VAR.to_owned()]);
        provider.set_free_models(vec![model.to_owned()]);
        user.providers_mut().set(name, provider);
        self.save(user);
    }

    /// Add a provider naming `model` as metered — the only category the
    /// price ceiling can ever exclude.
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

    /// Write `pricing.toml` where `PriceTable::load_from_dir` resolves it
    /// from — this fixture's own `--config-dir`.
    fn plant_pricing(&self, contents: &str) {
        std::fs::write(self.base.join("config").join(PRICING_FILE_NAME), contents)
            .expect("write pricing.toml");
    }

    /// Run `glasshouse resources --no-harness --verbose`, exactly as
    /// `tests/routing_economics.rs`'s own classification-filter tests do.
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

/// Render exact micro-USD the same way `routing::disposable`'s own private
/// `format_micro_usd` does, so the assertions below pin the exact text
/// `classification_verdict` produces rather than a loose substring.
fn format_micro_usd(value: u64) -> String {
    let dollars = value / 1_000_000;
    let fraction = value % 1_000_000;
    format!("${dollars}.{fraction:06}")
}

// ---------------------------------------------------------------------------
// (a) An overpriced metered candidate is excluded; a cheaper one is chosen.
// ---------------------------------------------------------------------------

#[test]
fn an_overpriced_metered_candidate_is_excluded_and_a_cheaper_one_is_chosen() {
    let pricey_price = ModelPrice {
        input_per_million_usd: 1.0,
        output_per_million_usd: 1.0,
        cached_input_per_million_usd: None,
    };
    let cheap_price = ModelPrice {
        input_per_million_usd: 0.001,
        output_per_million_usd: 0.001,
        cached_input_per_million_usd: None,
    };
    let pricey_estimate = estimated_classification_cost_micro_usd(pricey_price);
    let cheap_estimate = estimated_classification_cost_micro_usd(cheap_price);
    assert!(
        cheap_estimate < pricey_estimate,
        "the fixture's two prices must actually differ in estimated cost: cheap={cheap_estimate} \
         pricey={pricey_estimate}"
    );
    let ceiling = cheap_estimate + (pricey_estimate - cheap_estimate) / 2;

    let fixture = Fixture::new();
    fixture.add_metered_provider("pricey-runner", "pricey-model");
    fixture.add_metered_provider("cheap-runner", "cheap-model");
    fixture.plant_pricing(&format!(
        "[[prices]]\nprovider = \"pricey-runner\"\nmodel = \"pricey-model\"\n\
         input_per_million_usd = {}\noutput_per_million_usd = {}\n\n\
         [[prices]]\nprovider = \"cheap-runner\"\nmodel = \"cheap-model\"\n\
         input_per_million_usd = {}\noutput_per_million_usd = {}\n",
        pricey_price.input_per_million_usd,
        pricey_price.output_per_million_usd,
        cheap_price.input_per_million_usd,
        cheap_price.output_per_million_usd,
    ));
    fixture.automatic_routing_model();
    fixture.set_max_marginal_cost(u32::try_from(ceiling).unwrap());

    let ran = fixture.resources_verbose();
    assert!(ran.status.success(), "{}", ran.stderr);

    assert!(
        ran.stdout
            .contains("would select    cheap-model on cheap-runner"),
        "the cheaper candidate must be chosen once the pricier one is excluded:\n{}",
        ran.stdout
    );
    let expected_exclusion = format!(
        "excluded candidate — pricey-model on pricey-runner: estimated classification cost {} \
         exceeds the {} price ceiling (map line 1436)",
        format_micro_usd(pricey_estimate),
        format_micro_usd(u64::from(u32::try_from(ceiling).unwrap())),
    );
    assert!(
        ran.stdout.contains(&expected_exclusion),
        "the explanation must name the estimate, the ceiling and line 1436:\n{}",
        ran.stdout
    );
}

// ---------------------------------------------------------------------------
// (b) The same pricier model is admitted under the default ceiling.
// ---------------------------------------------------------------------------

#[test]
fn the_default_ceiling_admits_a_model_a_stricter_one_would_exclude() {
    let pricey_price = ModelPrice {
        input_per_million_usd: 1.0,
        output_per_million_usd: 1.0,
        cached_input_per_million_usd: None,
    };
    let estimate = estimated_classification_cost_micro_usd(pricey_price);
    assert!(
        estimate < u64::from(glasshouse::config::RouterCostMicroUsd::DEFAULT.get()),
        "this fixture's price must fit under the default ceiling for the test to prove \
         anything: estimate={estimate}"
    );

    let fixture = Fixture::new();
    fixture.add_metered_provider("pricey-runner", "pricey-model");
    fixture.plant_pricing(&format!(
        "[[prices]]\nprovider = \"pricey-runner\"\nmodel = \"pricey-model\"\n\
         input_per_million_usd = {}\noutput_per_million_usd = {}\n",
        pricey_price.input_per_million_usd, pricey_price.output_per_million_usd,
    ));
    fixture.automatic_routing_model();
    // No `set_max_marginal_cost` call: the default (1000 micro-USD) applies.

    let ran = fixture.resources_verbose();
    assert!(ran.status.success(), "{}", ran.stderr);
    assert!(
        ran.stdout
            .contains("would select    pricey-model on pricey-runner"),
        "the default ceiling must admit a model whose estimate is under it:\n{}",
        ran.stdout
    );
    assert!(
        !ran.stdout.contains("excluded candidate"),
        "nothing should be excluded when the only candidate is under the default ceiling:\n{}",
        ran.stdout
    );
}

// ---------------------------------------------------------------------------
// (c) Free and unpriced-metered candidates are admitted, each with its own
// note — never excluded, and never confused with each other.
// ---------------------------------------------------------------------------

#[test]
fn free_and_unpriced_candidates_are_admitted_with_distinct_notes() {
    // No `pricing.toml` at all: nothing this test names has a known price.
    let free_fixture = Fixture::new();
    free_fixture.add_free_provider("free-runner", "free-model");
    free_fixture.automatic_routing_model();
    free_fixture.set_max_marginal_cost(0);

    let ran = free_fixture.resources_verbose();
    assert!(ran.status.success(), "{}", ran.stderr);
    assert!(
        ran.stdout
            .contains("would select    free-model on free-runner"),
        "a free candidate must never be excluded by the price ceiling, even at zero:\n{}",
        ran.stdout
    );
    assert!(
        ran.stdout
            .contains("free — the price ceiling does not apply (map line 1436)"),
        "the explanation must say the ceiling does not apply to a free candidate:\n{}",
        ran.stdout
    );

    let metered_fixture = Fixture::new();
    metered_fixture.add_metered_provider("unpriced-runner", "unpriced-model");
    metered_fixture.automatic_routing_model();

    let ran = metered_fixture.resources_verbose();
    assert!(ran.status.success(), "{}", ran.stderr);
    assert!(
        ran.stdout
            .contains("would select    unpriced-model on unpriced-runner"),
        "a metered candidate with no price entry must never be excluded:\n{}",
        ran.stdout
    );
    assert!(
        ran.stdout.contains(
            "unpriced: no entry in pricing.toml — the ceiling is inert; unpriced, not expensive \
             (map line 1436)"
        ),
        "the explanation must say the candidate is unpriced, never expensive:\n{}",
        ran.stdout
    );
}

// ---------------------------------------------------------------------------
// (d) A ceiling of zero admits only free candidates; with none, the job
// fails in exactly the words the other classification filters already use.
// ---------------------------------------------------------------------------

#[test]
fn a_zero_ceiling_admits_only_free_candidates() {
    let priced = ModelPrice {
        input_per_million_usd: 1.0,
        output_per_million_usd: 1.0,
        cached_input_per_million_usd: None,
    };

    // d1: a free candidate survives a zero ceiling even when `pricing.toml`
    // itself names a (non-zero) price for it — proof the `Cost::Free` arm
    // never falls through to the priced comparison at all.
    let with_free = Fixture::new();
    with_free.add_free_provider("free-runner", "free-model");
    with_free.add_metered_provider("pricey-runner", "pricey-model");
    with_free.plant_pricing(&format!(
        "[[prices]]\nprovider = \"free-runner\"\nmodel = \"free-model\"\n\
         input_per_million_usd = {}\noutput_per_million_usd = {}\n\n\
         [[prices]]\nprovider = \"pricey-runner\"\nmodel = \"pricey-model\"\n\
         input_per_million_usd = {}\noutput_per_million_usd = {}\n",
        priced.input_per_million_usd,
        priced.output_per_million_usd,
        priced.input_per_million_usd,
        priced.output_per_million_usd,
    ));
    with_free.automatic_routing_model();
    with_free.set_max_marginal_cost(0);

    let ran = with_free.resources_verbose();
    assert!(ran.status.success(), "{}", ran.stderr);
    assert!(
        ran.stdout
            .contains("would select    free-model on free-runner"),
        "a zero ceiling must still admit the free candidate:\n{}",
        ran.stdout
    );
    assert!(
        ran.stdout
            .contains("excluded candidate — pricey-model on pricey-runner"),
        "the priced metered candidate must be excluded at a zero ceiling:\n{}",
        ran.stdout
    );

    // d2: with no free candidate at all, a zero ceiling excludes the only
    // configured candidate and the job fails in the same words every other
    // classification-requirement exclusion already uses.
    let without_free = Fixture::new();
    without_free.add_metered_provider("pricey-runner", "pricey-model");
    without_free.plant_pricing(&format!(
        "[[prices]]\nprovider = \"pricey-runner\"\nmodel = \"pricey-model\"\n\
         input_per_million_usd = {}\noutput_per_million_usd = {}\n",
        priced.input_per_million_usd, priced.output_per_million_usd,
    ));
    without_free.automatic_routing_model();
    without_free.set_max_marginal_cost(0);

    let ran = without_free.resources_verbose();
    assert!(ran.status.success(), "{}", ran.stderr);
    assert!(
        ran.stdout.contains(
            "would select    nothing — every configured candidate was excluded by a \
             classification requirement"
        ),
        "with no free resource left, the failure must read exactly as 1427/1432/1435's own \
         all-excluded failure does:\n{}",
        ran.stdout
    );
}

// ---------------------------------------------------------------------------
// (e) A unit test pinning the estimate itself: a known price gives a known
// micro-USD figure, and the input token count is the ceiling's, not
// whatever the actual (absent, here) task text would have been.
// ---------------------------------------------------------------------------

#[test]
fn the_estimate_uses_the_task_text_ceiling_not_zero() {
    let price = ModelPrice {
        input_per_million_usd: 2.0,
        output_per_million_usd: 3.0,
        cached_input_per_million_usd: None,
    };

    let prompt_len = CLASSIFICATION_PROMPT_CONTRACT.len();
    let expected_tokens = (prompt_len + TASK_TEXT_CEILING_BYTES).div_ceil(4);
    let expected = (expected_tokens as f64 * price.input_per_million_usd
        + 64.0 * price.output_per_million_usd)
        .round() as u64;

    let actual = estimated_classification_cost_micro_usd(price);
    assert_eq!(
        actual, expected,
        "a known price must give a known micro-USD figure, computed from the task-text ceiling"
    );

    // If the input token count used zero bytes of task text instead of the
    // ceiling, the estimate would be smaller and distinct from `expected`.
    let tokens_with_zero_task_text = prompt_len.div_ceil(4);
    let estimate_with_zero_task_text = (tokens_with_zero_task_text as f64
        * price.input_per_million_usd
        + 64.0 * price.output_per_million_usd)
        .round() as u64;
    assert_ne!(
        actual, estimate_with_zero_task_text,
        "the estimate must use the task-text ceiling, not zero bytes of actual task text"
    );
}
