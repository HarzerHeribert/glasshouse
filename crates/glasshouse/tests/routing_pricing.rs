//! Map lines 1305 and 1306 — *"treat unknown pricing as unknown instead of
//! assigning a fake zero cost"* and *"allow provider price metadata to be
//! updated independently from the router implementation"* — exercised
//! through the shipped surface: `SessionRouter::choose`'s public
//! `RoutingExplanation`, the same surface `tests/interactive_score_terms.rs`
//! already pins `expected marginal cost` on.
//!
//! `PriceTable::load_from_dir` (`crate::provider::pricing`) is the producer;
//! `SessionRouter::with_price_table` is how a caller attaches it, mirroring
//! `with_retry_after`'s own builder shape. Every test here loads from a real
//! temporary directory — never a global or an environment variable — the
//! same reason `firewall::store::RawStore::open` takes a plain path.

use std::fs;
use std::time::Instant;

use glasshouse::harness::pairing::PairingOverrides;
use glasshouse::integrations::IntegrationId;
use glasshouse::provider::pricing::PriceTable;
use glasshouse::routing::free::FreePool;
use glasshouse::routing::session::{
    Destination, RouterInputs, RoutingMoment, SessionRouter, TaskRequirements,
};
use glasshouse::routing::{AssignedModel, Backend, Cost, CredentialId, ToolSemantics};
use glasshouse::secret::SecretRef;

const ANTHROPIC: &str = "anthropic-messages";
const TERM: &str = "expected marginal cost";

fn backend_with_cost(provider: &str, model: &str, var: &str, cost: Cost) -> Backend {
    Backend::new(
        provider,
        ANTHROPIC,
        AssignedModel::named(model),
        CredentialId::new(
            provider,
            SecretRef::Environment {
                var: var.to_owned(),
            },
        ),
        cost,
        ToolSemantics::Verified,
    )
}

fn no_overrides() -> PairingOverrides {
    PairingOverrides::from_parts(
        "no configuration",
        std::collections::BTreeMap::new(),
        std::collections::BTreeMap::new(),
    )
}

struct Fixture {
    overrides: PairingOverrides,
    health: FreePool,
    now: Instant,
}

impl Fixture {
    fn new() -> Self {
        Self {
            overrides: no_overrides(),
            health: FreePool::new(),
            now: Instant::now(),
        }
    }

    fn inputs(&self) -> RouterInputs<'_> {
        RouterInputs {
            overrides: &self.overrides,
            health: &self.health,
            now: self.now,
            requirements: TaskRequirements::default(),
        }
    }
}

/// A fresh, uniquely named temp directory this test owns — never created on
/// disk until a test writes into it, so "the directory does not exist at
/// all" and "the directory exists with no file in it" both get exercised
/// across this file's tests.
fn temp_dir(label: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!(
        "glasshouse-routing-pricing-test-{label}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ))
}

fn write_pricing_toml(dir: &std::path::Path, contents: &str) {
    fs::create_dir_all(dir).expect("create temp config dir");
    fs::write(dir.join("pricing.toml"), contents).expect("write pricing.toml");
}

fn contribution_evidence(
    considered: &[(Destination, glasshouse::routing::RoutingExplanation)],
    id: &str,
) -> (f64, String) {
    let (_, explanation) = considered
        .iter()
        .find(|(destination, _)| destination.id() == id)
        .unwrap_or_else(|| panic!("`{id}` was scored"));
    let contribution = explanation
        .contributions()
        .iter()
        .find(|c| c.name() == TERM)
        .expect("every candidate must be scored for expected marginal cost");
    (contribution.magnitude(), contribution.evidence().to_owned())
}

// ---------------------------------------------------------------------------
// 1306 — a metadata file names a provider this build has no compiled
// knowledge of, and its price reaches the explanation.
// ---------------------------------------------------------------------------

#[test]
fn an_unrecognized_providers_price_reaches_the_explanation_with_no_recompilation() {
    let dir = temp_dir("unrecognized-provider");
    write_pricing_toml(
        &dir,
        r#"
        [[prices]]
        provider = "a-provider-this-binary-was-never-told-about"
        model = "a-model-nobody-compiled-in"
        input_per_million_usd = 3.0
        output_per_million_usd = 9.0
        "#,
    );
    let prices = PriceTable::load_from_dir(&dir);

    let fixture = Fixture::new();
    let destination = Destination::fresh(
        "fresh",
        IntegrationId::ClaudeCode,
        "default",
        backend_with_cost(
            "a-provider-this-binary-was-never-told-about",
            "a-model-nobody-compiled-in",
            "SOME_API_KEY",
            Cost::Metered,
        ),
        None,
    );

    let routed = SessionRouter::new()
        .with_price_table(prices)
        .choose(
            RoutingMoment::SessionStart,
            None,
            &[destination],
            &fixture.inputs(),
        )
        .expect("destination was offered");

    let (magnitude, evidence) = contribution_evidence(routed.considered(), "fresh");
    assert_ne!(
        magnitude, 0.0,
        "a metered destination must not score as free: {evidence}"
    );
    assert!(
        evidence.contains("known") && evidence.contains("3.00") && evidence.contains("9.00"),
        "the explanation must state the price read from the file, with no recompilation: \
         {evidence}"
    );
}

// ---------------------------------------------------------------------------
// 1306 — correcting a price in the file changes the explanation on the next
// read, with no other change.
// ---------------------------------------------------------------------------

#[test]
fn correcting_a_price_in_the_file_changes_the_explanation_on_the_next_read() {
    let dir = temp_dir("price-correction");
    let destination = || {
        Destination::fresh(
            "fresh",
            IntegrationId::ClaudeCode,
            "default",
            backend_with_cost(
                "openrouter",
                "some/model",
                "OPENROUTER_API_KEY",
                Cost::Metered,
            ),
            None,
        )
    };
    let fixture = Fixture::new();

    write_pricing_toml(
        &dir,
        r#"
        [[prices]]
        provider = "openrouter"
        model = "some/model"
        input_per_million_usd = 1.0
        output_per_million_usd = 2.0
        "#,
    );
    let before = PriceTable::load_from_dir(&dir);
    let routed_before = SessionRouter::new()
        .with_price_table(before)
        .choose(
            RoutingMoment::SessionStart,
            None,
            &[destination()],
            &fixture.inputs(),
        )
        .expect("destination was offered");
    let (_, evidence_before) = contribution_evidence(routed_before.considered(), "fresh");
    assert!(evidence_before.contains("1.00") && evidence_before.contains("2.00"));

    write_pricing_toml(
        &dir,
        r#"
        [[prices]]
        provider = "openrouter"
        model = "some/model"
        input_per_million_usd = 5.0
        output_per_million_usd = 20.0
        "#,
    );
    let after = PriceTable::load_from_dir(&dir);
    let routed_after = SessionRouter::new()
        .with_price_table(after)
        .choose(
            RoutingMoment::SessionStart,
            None,
            &[destination()],
            &fixture.inputs(),
        )
        .expect("destination was offered");
    let (magnitude_after, evidence_after) =
        contribution_evidence(routed_after.considered(), "fresh");

    assert!(
        evidence_after.contains("5.00") && evidence_after.contains("20.00"),
        "the corrected price must reach the explanation on the next read: {evidence_after}"
    );
    assert_ne!(
        evidence_before, evidence_after,
        "correcting the file must change the explanation"
    );
    // The magnitude does not move with the corrected price — no fictitious
    // precision, because no per-call token estimate exists at this call site
    // to turn either rate into an actual dollar figure. Only the evidence
    // text carries the correction.
    let (magnitude_before, _) = contribution_evidence(routed_before.considered(), "fresh");
    assert_eq!(magnitude_before, magnitude_after);
}

// ---------------------------------------------------------------------------
// 1305 — a metered destination with no metadata entry renders as unknown
// price, and is not treated as free.
// ---------------------------------------------------------------------------

#[test]
fn a_metered_destination_with_no_price_entry_renders_as_unknown_not_free() {
    let dir = temp_dir("unknown-metered");
    write_pricing_toml(
        &dir,
        r#"
        [[prices]]
        provider = "openrouter"
        model = "a-model-with-a-known-price"
        input_per_million_usd = 1.0
        output_per_million_usd = 2.0
        "#,
    );
    let prices = PriceTable::load_from_dir(&dir);

    let fixture = Fixture::new();
    let destination = Destination::fresh(
        "unpriced",
        IntegrationId::ClaudeCode,
        "default",
        backend_with_cost(
            "openrouter",
            "a-model-the-file-never-names",
            "OPENROUTER_API_KEY",
            Cost::Metered,
        ),
        None,
    );

    let routed = SessionRouter::new()
        .with_price_table(prices)
        .choose(
            RoutingMoment::SessionStart,
            None,
            &[destination],
            &fixture.inputs(),
        )
        .expect("destination was offered");

    let (magnitude, evidence) = contribution_evidence(routed.considered(), "unpriced");
    assert_ne!(
        magnitude, 0.0,
        "an unpriced metered destination must not score as free: {evidence}"
    );
    assert!(
        evidence.contains("unknown"),
        "the explanation must say the price is unknown: {evidence}"
    );
}

// ---------------------------------------------------------------------------
// 1305 — a free destination still renders as a known zero, distinct from an
// unknown-priced metered one.
// ---------------------------------------------------------------------------

#[test]
fn a_free_destination_is_a_known_zero_textually_distinct_from_an_unknown_price() {
    let dir = temp_dir("free-vs-unknown");
    // No file at all: both destinations below see an empty table.
    let prices = PriceTable::load_from_dir(&dir);

    let fixture = Fixture::new();
    let free = Destination::fresh(
        "free",
        IntegrationId::ClaudeCode,
        "default",
        backend_with_cost("openrouter", "m", "OPENROUTER_API_KEY", Cost::Free),
        None,
    );
    let unpriced_metered = Destination::fresh(
        "metered",
        IntegrationId::ClaudeCode,
        "default",
        backend_with_cost("openrouter", "m", "OPENROUTER_API_KEY", Cost::Metered),
        None,
    );

    let routed = SessionRouter::new()
        .with_price_table(prices)
        .choose(
            RoutingMoment::SessionStart,
            None,
            &[free, unpriced_metered],
            &fixture.inputs(),
        )
        .expect("destinations were offered");

    let (free_magnitude, free_evidence) = contribution_evidence(routed.considered(), "free");
    let (metered_magnitude, metered_evidence) =
        contribution_evidence(routed.considered(), "metered");

    assert_eq!(
        free_magnitude, 0.0,
        "a free destination must still read as a known zero: {free_evidence}"
    );
    assert_ne!(
        metered_magnitude, 0.0,
        "an unknown-priced metered destination must not collapse to the free branch's zero: \
         {metered_evidence}"
    );
    assert!(
        !free_evidence.contains("unknown"),
        "the free destination's evidence must not use the word this package reserves for an \
         unread price: {free_evidence}"
    );
    assert!(
        metered_evidence.contains("unknown"),
        "the metered destination's evidence must say its price is unknown: {metered_evidence}"
    );
    assert_ne!(
        free_evidence, metered_evidence,
        "a read zero and an unknown price must be textually distinguishable, not just \
         numerically: this is the package's whole point"
    );
}

// ---------------------------------------------------------------------------
// Fail-soft — a malformed file leaves routing working, price unknown.
// ---------------------------------------------------------------------------

#[test]
fn a_malformed_pricing_file_leaves_routing_working_with_price_unknown() {
    let dir = temp_dir("malformed");
    write_pricing_toml(&dir, "this is not [ valid toml at all");
    let prices = PriceTable::load_from_dir(&dir);
    assert_eq!(
        prices,
        PriceTable::empty(),
        "a malformed document must degrade to an empty table, never a partial one"
    );

    let fixture = Fixture::new();
    let destination = Destination::fresh(
        "fresh",
        IntegrationId::ClaudeCode,
        "default",
        backend_with_cost("openrouter", "m", "OPENROUTER_API_KEY", Cost::Metered),
        None,
    );

    let routed = SessionRouter::new()
        .with_price_table(prices)
        .choose(
            RoutingMoment::SessionStart,
            None,
            &[destination],
            &fixture.inputs(),
        )
        .expect("a malformed price file must not stop routing from working");

    let (magnitude, evidence) = contribution_evidence(routed.considered(), "fresh");
    assert_ne!(magnitude, 0.0);
    assert!(evidence.contains("unknown"), "{evidence}");
}

// ---------------------------------------------------------------------------
// Regression — no metadata file at all reproduces the pre-package
// explanation and ordering.
// ---------------------------------------------------------------------------

#[test]
fn with_no_price_table_attached_the_explanation_and_ranking_are_unchanged() {
    let fixture = Fixture::new();
    let free_dest = Destination::fresh(
        "free",
        IntegrationId::ClaudeCode,
        "default",
        backend_with_cost(
            "anthropic",
            "claude-opus-4",
            "ANTHROPIC_API_KEY",
            Cost::Free,
        ),
        None,
    );
    let metered_dest = Destination::fresh(
        "metered",
        IntegrationId::ClaudeCode,
        "default",
        backend_with_cost(
            "anthropic",
            "claude-opus-4",
            "ANTHROPIC_API_KEY",
            Cost::Metered,
        ),
        None,
    );

    // No `.with_price_table(...)` call at all — this is the exact router
    // construction every caller before this package used, and every caller
    // after it that never wires a file in.
    let routed = SessionRouter::new()
        .choose(
            RoutingMoment::SessionStart,
            None,
            &[metered_dest, free_dest],
            &fixture.inputs(),
        )
        .expect("destinations were offered");

    assert_eq!(
        routed.chosen().id(),
        "free",
        "the free destination must still win with no price table attached at all"
    );
    let (free_magnitude, _) = contribution_evidence(routed.considered(), "free");
    let (metered_magnitude, metered_evidence) =
        contribution_evidence(routed.considered(), "metered");
    assert_eq!(free_magnitude, 0.0);
    assert_ne!(metered_magnitude, 0.0);
    assert!(
        metered_evidence.contains("unknown"),
        "with no price table attached, a metered destination's price must read as unknown, \
         exactly as every destination did before this package: {metered_evidence}"
    );
}
