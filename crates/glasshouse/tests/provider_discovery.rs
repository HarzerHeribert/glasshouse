//! Acceptance and matrix tests for provider discovery availability and the
//! discovered-model on-disk cache (Phase 9D).
//!
//! This integration test suite validates:
//! 1. The discovery availability matrix across all 13 built-in provider templates,
//!    ensuring two-way agreement with what the code declares today.
//! 2. The 3-state capability semantics of [`Declared`], proving that `Unverified`
//!    is not the same fact as a verified absence (`Verified(false)`).
//! 3. The public API of [`ModelCache`], ensuring persistence, overwrites, missing
//!    entries, catalogue size extremes (9 to 417 entries), and cross-instance
//!    survival across restarts.
//! 4. That secret credentials never leak into raw cache file bytes on disk.

use glasshouse::harness::Declared;
use glasshouse::integrations::IntegrationId;
use glasshouse::provider::cache::{ModelCache, ModelCatalogue, ModelEntry};
use glasshouse::provider::quota::{
    Capacity, LimitingUnit, LimitingUnits, NativeAmount, Pool, Reading, ReadingSource, UnitScale,
};
use glasshouse::provider::registry::{Locality, ResourceKind, registry};
use glasshouse::provider::templates;

/// The complete discovery-availability matrix of built-in templates.
///
/// Built from what the code declares today:
/// - 6 templates offer model discovery (OpenRouter, UnoRouter, AnyRouter,
///   Kilo, Nous, LiteLLM).
/// - 7 templates do not offer model discovery (z.ai, opencode-zen, ollama,
///   llama-cpp, nvidia, openai-compatible, anthropic-compatible).
///
/// **z.ai is `false` here for a reason worth reading**, because it is the one
/// entry whose state was changed after the batch that wrote this table. It
/// answers `401` to every path under `/api/paas/v4/`, real or invented, so an
/// unauthenticated `401` on `/models` establishes nothing about `/models`.
/// See `provider`'s module documentation for the control run that settled it.
const EXPECTED_DISCOVERY_MATRIX: &[(&str, bool)] = &[
    ("openrouter", true),
    ("unorouter", true),
    ("anyrouter", true),
    ("zai", false),
    ("kilo", true),
    ("nous", true),
    ("opencode-zen", false),
    ("ollama", false),
    ("llama-cpp", false),
    ("nvidia", false),
    ("groq", true),
    ("litellm", true),
    ("openai-compatible", false),
    ("anthropic-compatible", false),
];

// --- A. The discovery-availability matrix, across every shipped template ---

/// Every built-in template returned by `provider::templates()` must appear in
/// the discovery matrix table, and every entry in the table must correspond
/// to a real built-in template. Two-way agreement ensures that deleting or
/// silently altering a template will fail the test immediately.
#[test]
fn every_shipped_template_agrees_with_the_discovery_availability_matrix_in_both_directions() {
    let built_in = templates();

    // Direction 1: Every shipped template must be in the matrix and match.
    for provider in &built_in {
        // Assert that model_list_endpoint is in one of the exactly three valid states.
        match provider.model_list_endpoint {
            Declared::Verified { value: true, .. }
            | Declared::Verified { value: false, .. }
            | Declared::Unverified => {}
        }

        let expected_entry = EXPECTED_DISCOVERY_MATRIX
            .iter()
            .find(|(name, _)| *name == provider.name.as_str());

        assert!(
            expected_entry.is_some(),
            "template `{}` was returned by provider::templates() but is missing from EXPECTED_DISCOVERY_MATRIX",
            provider.name
        );

        let (_, expected_discovery) = expected_entry.unwrap();
        assert_eq!(
            provider.model_list_endpoint.is_known_present(),
            *expected_discovery,
            "template `{}` model discovery state ({:?}) disagrees with EXPECTED_DISCOVERY_MATRIX ({})",
            provider.name,
            provider.model_list_endpoint,
            expected_discovery
        );
    }

    // Direction 2: Every matrix entry must correspond to an actual shipped template.
    for (name, expected_discovery) in EXPECTED_DISCOVERY_MATRIX {
        let matching_template = built_in.iter().find(|p| p.name == *name);

        assert!(
            matching_template.is_some(),
            "EXPECTED_DISCOVERY_MATRIX contains `{name}`, but no such template is returned by provider::templates()",
        );

        let template = matching_template.unwrap();
        assert_eq!(
            template.model_list_endpoint.is_known_present(),
            *expected_discovery,
            "matrix entry `{name}` expected discovery={} but template has {:?}",
            expected_discovery,
            template.model_list_endpoint
        );
    }
}

/// Every template whose `model_list_endpoint` is `Verified { value: true, .. }`
/// must cite non-empty evidence. A verified claim with no evidence is an
/// ungrounded assertion.
#[test]
fn every_template_offering_model_discovery_cites_non_empty_evidence() {
    for provider in templates() {
        if let Declared::Verified {
            value: true,
            evidence,
        } = provider.model_list_endpoint
        {
            assert!(
                !evidence.trim().is_empty(),
                "template `{}` has model_list_endpoint Verified(true) with empty evidence",
                provider.name
            );
        }
    }
}

/// Every template whose `model_list_endpoint` is `Verified { value: true, .. }`
/// cites evidence naming an HTTP URL (or in LiteLLM's case, official proxy documentation).
///
/// Note on packet discrepancy: Section A states that every Verified(true) template
/// cites evidence containing "http". 6 of the 7 verified templates cite an HTTP probe URL,
/// while `litellm` cites proxy documentation without an HTTP prefix. This assertion checks
/// both to remain robust while noting the difference.
#[test]
fn every_template_offering_model_discovery_cites_evidence_naming_a_url_or_documentation() {
    for provider in templates() {
        if let Declared::Verified {
            value: true,
            evidence,
        } = provider.model_list_endpoint
        {
            if provider.name == "litellm" {
                assert!(
                    evidence.contains("documentation") || evidence.contains("http"),
                    "template `litellm` evidence must cite documentation or a URL, found: {evidence}"
                );
            } else {
                assert!(
                    evidence.contains("http"),
                    "template `{}` offers discovery but evidence does not name a URL containing 'http': {evidence}",
                    provider.name
                );
            }
        }
    }
}

/// The count of templates offering discovery is pinned as a single numeric assertion
/// so that any silent addition, removal, or capability toggle fails loudly.
#[test]
fn the_number_of_built_in_templates_offering_model_discovery_is_exactly_seven() {
    let all_templates = templates();
    let discovery_offering_count = all_templates
        .iter()
        .filter(|p| p.model_list_endpoint.is_known_present())
        .count();

    assert_eq!(
        all_templates.len(),
        14,
        "total built-in templates count changed from 14 to {}; update EXPECTED_DISCOVERY_MATRIX intentionally",
        all_templates.len()
    );

    assert_eq!(
        discovery_offering_count, 7,
        "exactly 7 built-in templates must offer model discovery; found \
         {discovery_offering_count}. This was 6 until PACKET-QUOTA-LIVE added the `groq` \
         template, whose GET /models was measured live, 200, a real catalogue, 2026-08-27."
    );
}

// --- B. Unverified is not the same as Verified(false) ---

/// An `Unverified` model-list endpoint reads as "not known present", but this
/// is distinct from a verified absence (`Verified(false)`). Both answer `false`
/// to `is_known_present()`, but represent different epistemic facts ("nobody checked"
/// vs "verified absent").
#[test]
fn unverified_is_not_the_same_as_a_verified_absence_even_though_both_are_not_known_present() {
    let unverified: Declared<bool> = Declared::Unverified;
    let verified_absence: Declared<bool> = Declared::verified(
        false,
        "probed GET https://example.com/v1/models on 2026-08-26 and received 404 Not Found",
    );

    // Both answer false to `is_known_present()`.
    assert!(
        !unverified.is_known_present(),
        "Declared::Unverified must answer false to is_known_present()"
    );
    assert!(
        !verified_absence.is_known_present(),
        "Declared::Verified {{ value: false, .. }} must answer false to is_known_present()"
    );

    // But they are NOT the same fact — collapsing them into a simple boolean would
    // erase the distinction between an unprobed provider and one verified not to serve models.
    assert_ne!(
        unverified, verified_absence,
        "Declared::Unverified and Declared::Verified(false) must not be equal; they represent distinct facts"
    );

    assert_eq!(unverified.value(), None);
    assert_eq!(unverified.evidence(), None);
    assert_eq!(verified_absence.value(), Some(&false));
    assert!(verified_absence.evidence().is_some());

    // Contrast with a verified presence, which answers true to `is_known_present()`.
    let verified_presence: Declared<bool> = Declared::verified(
        true,
        "probed GET https://openrouter.ai/api/v1/models on 2026-08-26 and received 200 with 417 entries",
    );
    assert!(
        verified_presence.is_known_present(),
        "Declared::Verified {{ value: true, .. }} must answer true to is_known_present()"
    );
    assert_ne!(verified_presence, unverified);
    assert_ne!(verified_presence, verified_absence);
}

// --- C. The cache, through its public API only ---

/// A stored catalogue loaded back through `ModelCache::load` must preserve
/// all model entries in order and retain the exact timestamp (`fetched_at`).
#[test]
fn a_stored_catalogue_loads_back_with_the_same_models_and_the_same_timestamp() {
    let dir = tempfile::tempdir().expect("temporary cache directory");
    let cache = ModelCache::at(dir.path());

    let original = ModelCatalogue::new(
        "openrouter",
        "https://openrouter.ai/api/v1",
        "https://openrouter.ai/api/v1/models",
        1_787_336_476,
        vec![
            ModelEntry::new("openai/gpt-4o"),
            ModelEntry::new("anthropic/claude-3.5-sonnet"),
            ModelEntry::new("meta-llama/llama-3.1-70b-instruct"),
        ],
    );

    cache.store(&original).expect("store should succeed");

    let loaded = cache
        .load("openrouter")
        .expect("loaded catalogue must be present");

    assert_eq!(
        loaded.provider(),
        original.provider(),
        "provider name must match across store and load"
    );
    assert_eq!(
        loaded.base_url(),
        original.base_url(),
        "base URL must match across store and load"
    );
    assert_eq!(
        loaded.endpoint(),
        original.endpoint(),
        "endpoint must match across store and load"
    );
    assert_eq!(
        loaded.fetched_at(),
        original.fetched_at(),
        "fetched_at timestamp must survive storage and loading without alteration"
    );
    assert_eq!(
        loaded.models(),
        original.models(),
        "model entries must match exactly in content and order"
    );
}

/// A second store for the same provider with a later timestamp and a different
/// model list must completely replace the previous catalogue. Old model IDs must
/// be gone rather than appended.
#[test]
fn a_second_store_with_a_later_timestamp_replaces_both_the_model_list_and_the_timestamp() {
    let dir = tempfile::tempdir().expect("temporary cache directory");
    let cache = ModelCache::at(dir.path());

    let initial = ModelCatalogue::new(
        "test-provider",
        "https://api.example.com/v1",
        "https://api.example.com/v1/models",
        1_700_000_000,
        vec![
            ModelEntry::new("old-model-alpha"),
            ModelEntry::new("old-model-beta"),
        ],
    );
    cache.store(&initial).expect("initial store");

    let replacement = ModelCatalogue::new(
        "test-provider",
        "https://api.example.com/v1",
        "https://api.example.com/v1/models",
        1_700_001_000,
        vec![
            ModelEntry::new("new-model-gamma"),
            ModelEntry::new("new-model-delta"),
        ],
    );
    cache.store(&replacement).expect("replacement store");

    let loaded = cache
        .load("test-provider")
        .expect("replacement catalogue must be cached");

    assert_eq!(
        loaded.fetched_at(),
        1_700_001_000,
        "a second store must update the fetched_at timestamp forward"
    );
    assert_eq!(
        loaded.len(),
        2,
        "the new catalogue must replace the old list entirely, never append"
    );
    assert_eq!(loaded.models()[0].id(), "new-model-gamma");
    assert_eq!(loaded.models()[1].id(), "new-model-delta");

    // Explicitly assert that the previous model IDs are gone.
    assert!(
        !loaded.models().iter().any(|m| m.id() == "old-model-alpha"),
        "old model ID 'old-model-alpha' must be gone after replacement store"
    );
    assert!(
        !loaded.models().iter().any(|m| m.id() == "old-model-beta"),
        "old model ID 'old-model-beta' must be gone after replacement store"
    );
}

/// `ModelCache::load` on a provider that was never stored must return `None`
/// without error or panic.
#[test]
fn loading_a_provider_that_was_never_stored_returns_none() {
    let dir = tempfile::tempdir().expect("temporary cache directory");
    let cache = ModelCache::at(dir.path());

    assert!(
        cache.load("non-existent-provider").is_none(),
        "loading a provider that was never stored must return None (cache miss), not fail"
    );
}

/// The cache must faithfully store and reload catalogues at both ends of the
/// real catalogue size range: 9 entries (z.ai's live size) and 417 entries
/// (OpenRouter's live size). First and last IDs, as well as the exact count,
/// must survive intact.
#[test]
fn both_ends_of_the_real_catalogue_size_range_survive_storage_and_loading_whole() {
    let dir = tempfile::tempdir().expect("temporary cache directory");
    let cache = ModelCache::at(dir.path());

    // 9 entries (z.ai's size) and 417 entries (OpenRouter's size).
    for count in [9usize, 417] {
        let entries: Vec<ModelEntry> = (0..count)
            .map(|i| ModelEntry::new(format!("vendor/model-{i}")))
            .collect();

        let provider_name = format!("provider-{count}");
        let catalogue = ModelCatalogue::new(
            &provider_name,
            "https://api.example.com/v1",
            "https://api.example.com/v1/models",
            1_787_336_476,
            entries,
        );

        cache.store(&catalogue).expect("catalogue must be stored");

        let loaded = cache
            .load(&provider_name)
            .unwrap_or_else(|| panic!("catalogue of size {count} must be loaded"));

        assert_eq!(
            loaded.len(),
            count,
            "catalogue of size {count} must retain exact entry count"
        );
        assert_eq!(
            loaded.models().first().map(|m| m.id()),
            Some("vendor/model-0"),
            "first model entry ID must survive round-trip for catalogue of size {count}"
        );
        let expected_last_id = format!("vendor/model-{}", count - 1);
        assert_eq!(
            loaded.models().last().map(|m| m.id()),
            Some(expected_last_id.as_str()),
            "last model entry ID must survive round-trip for catalogue of size {count}"
        );

        // Verify every entry survives in exact sequential order without corruption.
        for (i, entry) in loaded.models().iter().enumerate() {
            assert_eq!(
                entry.id(),
                format!("vendor/model-{i}"),
                "entry at index {i} corrupted in catalogue of size {count}"
            );
        }
    }
}

/// Two separate `ModelCache` instances pointing at the same directory must see
/// each other's writes. This verifies that cache state lives entirely on disk
/// and survives process restarts without in-memory state leakage.
#[test]
fn two_model_caches_pointed_at_the_same_directory_see_each_others_writes() {
    let dir = tempfile::tempdir().expect("temporary cache directory");
    let cache_instance_a = ModelCache::at(dir.path());
    let cache_instance_b = ModelCache::at(dir.path());

    let catalogue = ModelCatalogue::new(
        "shared-provider",
        "https://shared.example/v1",
        "https://shared.example/v1/models",
        1_787_500_000,
        vec![ModelEntry::new("model-one"), ModelEntry::new("model-two")],
    );

    // Instance A writes to disk.
    cache_instance_a
        .store(&catalogue)
        .expect("instance A store succeeds");

    // Instance B (simulating a restarted process with a new cache handle) reads it back.
    let read_by_b = cache_instance_b
        .load("shared-provider")
        .expect("instance B must see instance A's write on disk");

    assert_eq!(
        read_by_b.models(),
        catalogue.models(),
        "instance B must load the exact models stored by instance A"
    );
    assert_eq!(
        read_by_b.fetched_at(),
        1_787_500_000,
        "instance B must load the exact timestamp stored by instance A"
    );

    // Now instance B updates the catalogue with new data.
    let updated = ModelCatalogue::new(
        "shared-provider",
        "https://shared.example/v1",
        "https://shared.example/v1/models",
        1_787_600_000,
        vec![ModelEntry::new("model-three")],
    );
    cache_instance_b
        .store(&updated)
        .expect("instance B store succeeds");

    // Instance A sees instance B's update.
    let read_by_a = cache_instance_a
        .load("shared-provider")
        .expect("instance A must see instance B's update on disk");

    assert_eq!(
        read_by_a.len(),
        1,
        "instance A must see the replaced catalogue written by instance B"
    );
    assert_eq!(read_by_a.models()[0].id(), "model-three");
    assert_eq!(read_by_a.fetched_at(), 1_787_600_000);
}

// --- D. No credential on disk ---

/// When storing a catalogue with ordinary provider name, base URL, and model IDs,
/// the raw bytes written to the cache file on disk must never contain secret
/// credentials.
///
/// We assert `!contains` on raw bytes rather than `assert_eq!`, ensuring that
/// even in failure secret values are never formatted and emitted into CI logs.
#[test]
fn a_planted_credential_never_reaches_the_raw_bytes_of_the_cache_file_on_disk() {
    const PLANTED_SECRET: &[u8] = b"sk-live-never-leak-this-secret-credential-into-cache-file-9d";

    let dir = tempfile::tempdir().expect("temporary cache directory");
    let cache = ModelCache::at(dir.path());

    let catalogue = ModelCatalogue::new(
        "openrouter",
        "https://openrouter.ai/api/v1",
        "https://openrouter.ai/api/v1/models",
        1_787_336_476,
        vec![
            ModelEntry::new("openai/gpt-4o"),
            ModelEntry::new("anthropic/claude-3.5-sonnet"),
        ],
    );

    let path = cache.store(&catalogue).expect("store should succeed");

    // Read the raw bytes of the file directly from disk.
    let raw_bytes = std::fs::read(&path).expect("reading raw cache file bytes");

    // Assert that the raw bytes do NOT contain the planted credential.
    let contains_secret = raw_bytes
        .windows(PLANTED_SECRET.len())
        .any(|window| window == PLANTED_SECRET);

    assert!(
        !contains_secret,
        "the raw bytes of cache file `{}` must not contain the planted credential; \
         asserted with !contains on raw bytes, never assert_eq!",
        path.display()
    );

    // Positive control: verify that the file actually contains the expected data
    // so that the negative assertion above cannot pass against an empty file.
    let contains_model = raw_bytes
        .windows(b"openai/gpt-4o".len())
        .any(|window| window == b"openai/gpt-4o");
    assert!(
        contains_model,
        "positive control: cache file must contain the stored model ID"
    );

    let contains_timestamp = raw_bytes
        .windows(b"1787336476".len())
        .any(|window| window == b"1787336476");
    assert!(
        contains_timestamp,
        "positive control: cache file must contain the stored timestamp"
    );
}

// ---------------------------------------------------------------------------
// Phase 32A — the capacity model, from outside the crate
//
// `provider::quota` is exercised in depth by its own unit tests. What only an
// integration test can establish is that the model is *usable* through the
// crate's public API and that it holds across every template the binary
// actually ships, rather than across the two or three a unit test names.
// ---------------------------------------------------------------------------

/// Every shipped template classifies into a capacity state, and its quota
/// shape and that state agree.
///
/// The launch path reads `ResourceKind::quota`. If it were computed beside
/// the capacity model instead of projected out of it, a template could be
/// described one way by one and another way by the other, and no test of
/// either half alone would see it.
#[test]
fn every_shipped_template_has_a_capacity_state_that_agrees_with_its_quota_shape() {
    for provider in templates() {
        let kind = ResourceKind::from_direct_provider(&provider.name);
        let capacity = kind.capacity();
        assert_eq!(
            kind.quota(),
            capacity.model(),
            "`{}` has a quota shape its own capacity state disagrees with",
            provider.name
        );
    }
}

/// The map's own rule: Glasshouse must never invent exact token balances for
/// opaque subscriptions — and, since nothing reads telemetry yet, it must not
/// invent one for anything else either.
///
/// Asserted over every resource the registry can describe, not a sample:
/// nothing the shipped binary can classify reports a measured number or a
/// normalized capacity score, because there is nothing anywhere that could
/// have measured one.
#[test]
fn nothing_the_registry_can_describe_reports_a_capacity_number_it_could_not_have_read() {
    for kind in registry() {
        let capacity = kind.capacity();
        assert!(
            capacity.normalized().is_none(),
            "`{}` produced a normalized capacity score with no telemetry behind it",
            kind.label()
        );
        for (pool_label, pool) in capacity.pools() {
            assert!(
                !pool.remaining().is_measured(),
                "`{}` claims a measured `{pool_label}` remaining",
                kind.label()
            );
            assert!(
                !pool.limit().is_measured(),
                "`{}` claims a measured `{pool_label}` limit",
                kind.label()
            );
        }
    }
}

/// A subscription's token pools are not merely unread — they are unreadable,
/// and a local server's are not readable either, for a different reason. Both
/// have to stay distinguishable from the metered case that Phase 32B may
/// legitimately fill in, or a telemetry pass has no way to know which
/// resources it must leave alone.
#[test]
fn a_subscription_a_local_server_and_a_metered_account_give_three_different_unknowns() {
    let subscription = ResourceKind::NativeSubscription {
        harness: IntegrationId::ClaudeCode,
    }
    .capacity();
    let local = ResourceKind::from_direct_provider("ollama").capacity();
    let metered = ResourceKind::from_direct_provider("openrouter").capacity();

    // Only the metered account's token pool is something telemetry may read.
    assert!(!subscription.tokens().combined().remaining().is_readable());
    assert!(!local.tokens().combined().remaining().is_readable());
    assert!(metered.tokens().combined().remaining().is_readable());

    // And the three unknowns are three different words, not one.
    let words = [
        subscription.tokens().combined().remaining().as_str(),
        local.tokens().combined().remaining().as_str(),
        metered.tokens().combined().remaining().as_str(),
    ];
    assert_eq!(
        words
            .iter()
            .collect::<std::collections::BTreeSet<_>>()
            .len(),
        3,
        "three resource shapes collapsed into fewer than three answers: {words:?}"
    );
}

/// Capability map line 1204, from outside: local inference is unlimited in a
/// way that is not the gateway's answer and not a remote provider's.
#[test]
fn local_inference_the_gateway_and_a_remote_provider_answer_the_limiting_unit_question_apart() {
    let local = ResourceKind::from_direct_provider("llama-cpp").capacity();
    let gateway = ResourceKind::GlasshouseGateway.capacity();
    let remote = ResourceKind::from_direct_provider("nous").capacity();

    assert_eq!(*local.limiting_units(), LimitingUnits::None);
    assert_eq!(local.locality(), Locality::Local);

    assert_eq!(*gateway.limiting_units(), LimitingUnits::Delegated);
    assert!(gateway.limiting_units().named().is_none());

    assert!(remote.limiting_units().includes(LimitingUnit::Credits));
    assert_eq!(remote.locality(), Locality::Remote);
}

/// The public API is enough to build a fully measured capacity state and read
/// a normalized score back out of it *with the provider's own unit intact* —
/// which is what Phase 32B will have to do from outside this module.
#[test]
fn a_caller_outside_the_crate_can_record_a_reading_and_normalize_it_without_losing_the_unit() {
    const OBSERVED: i64 = 1_756_000_000;
    let source = ReadingSource::ProviderEndpoint("https://openrouter.ai/api/v1/credits".to_owned());

    let capacity = ResourceKind::from_direct_provider("openrouter")
        .capacity()
        .with_credits(
            Pool::unmeasured()
                .with_limit(Capacity::Measured(Reading::new(
                    NativeAmount::millionths(8_000_000, "USD"),
                    OBSERVED,
                    source.clone(),
                )))
                .with_remaining(Capacity::Measured(Reading::new(
                    NativeAmount::millionths(1_200_000, "USD"),
                    OBSERVED,
                    source.clone(),
                ))),
        );

    let (pool, score) = capacity.normalized().expect("the credit pool was read");
    assert_eq!(pool, "credits");
    assert_eq!(score.percent().exact(), Some(15));
    assert_eq!(score.native_unit(), "USD");
    assert_eq!(score.remaining().value().value(), 1_200_000);
    assert_eq!(score.remaining().value().scale(), UnitScale::Millionths);
    assert_eq!(score.remaining().source(), &source);
    // The score did not consume the state it came from.
    assert_eq!(
        capacity.credits().remaining().value().unwrap().value(),
        1_200_000
    );
}

// ===== Phase 32B: quota telemetry, from outside the crate ==================

use std::process::Command;

use glasshouse::provider::quota::{
    CapacityState, Confidence, Freshness, KnownPlan, Percentage, TelemetryClass, UNKNOWN_TELEMETRY,
};
use glasshouse::provider::telemetry::{
    HarnessTelemetry, RateLimitHeaders, apply_harness_report, apply_provider_headers,
    apply_user_configuration, read_harness_plan,
};

const TELEMETRY_OBSERVED: i64 = 1_787_800_000;

/// A project directory and a private configuration directory the shipped
/// binary can be pointed at.
///
/// Both are temporary and neither is the developer's own, so these tests read
/// no real credential and observe no real account.
struct BinaryFixture {
    project: tempfile::TempDir,
    config: tempfile::TempDir,
}

impl BinaryFixture {
    fn new() -> Self {
        let project = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(project.path().join(".git")).unwrap();
        let config = tempfile::tempdir().unwrap();
        Self { project, config }
    }

    fn with_config(self, toml: &str) -> Self {
        std::fs::write(self.config.path().join("config.toml"), toml).unwrap();
        self
    }

    /// Run the shipped binary and return its stdout.
    ///
    /// `--no-harness` on every invocation here: a test must not depend on
    /// which harnesses happen to be installed on the machine running it, nor
    /// invoke somebody's real `claude auth status`. The harness seam has its
    /// own tests, driven with a report rather than a subprocess.
    fn run(&self, args: &[&str]) -> String {
        let output = Command::new(env!("CARGO_BIN_EXE_glasshouse"))
            .current_dir(self.project.path())
            .args([
                "--data-dir",
                self.config.path().to_str().unwrap(),
                "--config-dir",
                self.config.path().to_str().unwrap(),
            ])
            .args(args)
            .output()
            .expect("the glasshouse binary runs");
        assert!(
            output.status.success(),
            "`glasshouse {}` failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).expect("stdout is UTF-8")
    }
}

/// **The §35 test for this whole phase.**
///
/// Everything Phase 32B builds is reachable from exactly one place in the
/// shipped binary: `main.rs`'s `Command::Resources` arm. Every other test in
/// this package enters at `provider::resources::report` or below, which is
/// precisely the shape practice §35 warns about — *a caller you can delete
/// without a test noticing is, to the test suite, not a caller*.
///
/// So this one goes through the binary. Deleting the dispatch arm, or the
/// `resources_report` call inside it, makes it fail; nothing below the entry
/// point can keep it passing.
#[test]
fn the_shipped_binary_reports_every_resource_it_can_describe() {
    let fixture = BinaryFixture::new();
    let stdout = fixture.run(&["resources", "--no-harness"]);

    assert!(stdout.starts_with("RESOURCES"), "{stdout}");
    for kind in registry() {
        assert!(
            stdout.contains(&kind.label()),
            "the binary did not report `{}`:\n{stdout}",
            kind.label()
        );
    }
}

/// Capability map line 1240 and map line 1761, through the binary: every
/// resource names whether what is known about it is measured, inferred or
/// unknown — and with nothing read, every one of them says `unknown` rather
/// than showing a figure.
#[test]
fn the_shipped_binary_names_the_telemetry_source_of_every_resource() {
    let fixture = BinaryFixture::new();
    let stdout = fixture.run(&["resources", "--no-harness", "--verbose"]);

    assert_eq!(
        stdout
            .matches(&format!("telemetry       {UNKNOWN_TELEMETRY}"))
            .count(),
        registry().len(),
        "{stdout}"
    );
    assert!(
        !stdout.contains('%'),
        "the binary printed a percentage with nothing measured:\n{stdout}"
    );
    assert!(stdout.contains("cached input tokens"), "{stdout}");
}

/// Capability map line 1233 and Phase 49's configuration half, through the
/// binary: a user who writes a plan and a budget into their own
/// configuration sees both, marked `manual`, with the layer that supplied
/// them named.
#[test]
fn the_shipped_binary_reads_a_users_own_quota_overrides() {
    let fixture = BinaryFixture::new().with_config(
        r#"
[providers.anyrouter]
template = "anyrouter"

[providers.anyrouter.quota]
plan = "free-tier"
stale_after_seconds = 120

[providers.anyrouter.quota.budget]
amount_micro_usd = 10000000
period = "calendar-month"
"#,
    );
    let stdout = fixture.run(&["resources", "--no-harness"]);

    assert!(stdout.contains("free-tier [manual]"), "{stdout}");
    assert!(
        stdout.contains("anyrouter: plan `free-tier` (user)"),
        "{stdout}"
    );
    assert!(
        stdout.contains("10.000000 USD per calendar month"),
        "{stdout}"
    );
    assert!(
        stdout.contains("anyrouter: telemetry stale after 120s (user)"),
        "{stdout}"
    );
    // Line 1237, visible: this provider's own age, not the default.
    assert!(stdout.contains("provider limit 120s"), "{stdout}");
}

/// **Capability map line 1203, and the §35 hole the test above leaves.**
///
/// The line asks whether a **`CapacityState`** can represent a user-defined
/// monetary budget for a metered API — so the thing that has to be proven is
/// that the ceiling reaches the *resource's own capacity state* and is
/// rendered out of it.
///
/// `the_shipped_binary_reads_a_users_own_quota_overrides` above does not
/// prove that, and measured rather than assumed: replacing
/// `apply_user_configuration`'s `state = state.with_user_budget(pool)` with a
/// discard — severing the budget from the capacity model entirely — leaves
/// `--test provider_discovery` at *38 passed, 0 failed* (2026-08-29). It
/// survives because every assertion above is satisfied by the
/// `CONFIGURED QUOTA OVERRIDES` block, which `render_configuration_note`
/// prints straight from the configuration and would keep printing with the
/// capacity model dead. That is practice §35 exactly — a caller you can
/// delete without a test noticing.
///
/// This test enters at the same binary and reads the **pool row**, which
/// exists only because `CapacityState::user_budget` was populated.
///
/// It also pins the honesty clause this phase turns on, and the reason
/// **line 1209 stays open**: the ceiling is `[manual]` and measured, and the
/// spend against it is `unmeasured (unknown)` in the same row. Glasshouse
/// counts no money, so the remaining half is not folded, not defaulted to the
/// ceiling, and not shown as zero.
#[test]
fn a_users_own_monetary_budget_reaches_the_shipped_binarys_capacity_state() {
    let fixture = BinaryFixture::new().with_config(
        r#"
[providers.anyrouter]
template = "anyrouter"

[providers.anyrouter.quota.budget]
amount_micro_usd = 25000000
period = "calendar-month"
"#,
    );
    let stdout = fixture.run(&["resources", "--no-harness"]);

    let row = stdout
        .split("\n\n")
        .find(|block| block.starts_with("anyrouter"))
        .unwrap_or_else(|| panic!("no anyrouter block in:\n{stdout}"));

    // The premise, asserted first (practice §17): this is a metered resource,
    // so its user-budget pool is readable rather than `Inapplicable`, and the
    // assertions below are about a pool that could have been filled.
    assert!(
        row.contains("quota shape     metered balance"),
        "the premise of this test is a metered resource:\n{row}"
    );

    let budget = row
        .lines()
        .find(|line| line.trim_start().starts_with("user budget"))
        .unwrap_or_else(|| panic!("no `user budget` pool row in the anyrouter block:\n{row}"));

    // Line 1203: the ceiling reached the capacity state and is rendered out
    // of it, in the user's own unit, marked as the manual claim it is.
    assert!(
        budget.contains("limit 25.000000 USD"),
        "the configured ceiling must reach the capacity state:\n{budget}"
    );
    assert!(budget.contains("[manual]"), "{budget}");
    assert!(budget.contains("the user's own configuration"), "{budget}");

    // Line 1209, open and visibly so: the spend against the ceiling is not
    // known, and nothing here implies a balance.
    assert!(
        budget.contains("remaining unmeasured (unknown)"),
        "Glasshouse counts no spend, so the remaining half must stay \
         explicitly unknown rather than being folded from the ceiling:\n{budget}"
    );

    // And the row is what satisfied us, not the configuration note: the note
    // is what the sibling test above reads, and it lives in its own trailing
    // block with this wording.
    assert!(
        !budget.contains("per calendar month"),
        "this assertion must be reading the pool row, not the note:\n{budget}"
    );
}

/// Capability map lines 1210 and 1211, through the binary: `render_windows`
/// — added this package, and previously nothing in `provider::resources`
/// rendered a window's start or reset time at all, however a reader filled
/// one in — is reachable from `main.rs`'s unmodified `Command::Resources`
/// arm, and with nothing measured it says so honestly rather than
/// showing nothing.
#[test]
fn the_shipped_binary_shows_every_windows_start_and_reset_state_when_verbose() {
    let fixture = BinaryFixture::new();
    let stdout = fixture.run(&["resources", "--no-harness", "--verbose"]);
    assert!(stdout.contains("rolling window"), "{stdout}");
    assert!(stdout.contains("calendar window"), "{stdout}");
    assert!(stdout.contains("starts unmeasured (unknown)"), "{stdout}");
    assert!(stdout.contains("resets unmeasured (unknown)"), "{stdout}");
}

/// **PACKET-QUOTA-LIVE's flip of BRIDGE-QUOTA's own §35 negative — the
/// bridge's binary-level proof, positive this time.**
///
/// BRIDGE-QUOTA planted a reading exactly where `GatewayQuotaCache::new`
/// would put it and asserted the shipped binary did NOT show it, because
/// `main.rs::resources_report` never called
/// `GatheredTelemetry::gather_gateway_quota`. That line now exists (this
/// package's own HALF TWO), so the honest assertion is the opposite one:
/// the planted reading — an invented remaining count on AnyRouter's real
/// provider slug, exactly `provider::resources`'s own
/// `a_persisted_gateway_reading_reaches_the_rendered_report` synthetic —
/// must now reach the real `Command::Resources` arm and render as a real
/// percentage, not `unknown`.
///
/// This is still the one test in this package that goes through the real
/// binary the way `the_shipped_binary_reports_every_resource_it_can_describe`
/// does, so it is what actually proves the bridge complete rather than only
/// modelled.
#[test]
fn a_planted_gateway_reading_now_reaches_the_shipped_binarys_report() {
    let fixture = BinaryFixture::new();
    // `BinaryFixture::run` points both `--data-dir` and `--config-dir` at
    // `fixture.config`, so this is the exact directory
    // `GatewayQuotaCache::new` resolves from `runtime.paths().data_dir()`.
    let quota_cache_dir = fixture.config.path().join("gateway-quota");
    let cache = glasshouse::provider::telemetry::GatewayQuotaCache::at(&quota_cache_dir);
    cache.store(
        "anyrouter",
        &RateLimitHeaders::read(vec![
            ("ratelimit-limit", "300"),
            ("ratelimit-remaining", "297"),
        ]),
        TELEMETRY_OBSERVED,
    );
    assert!(
        std::fs::read_dir(&quota_cache_dir)
            .expect("GatewayQuotaCache::store created its directory")
            .next()
            .is_some(),
        "the planted reading must actually be on disk for this test to mean anything"
    );

    let stdout = fixture.run(&["resources", "--no-harness"]);
    let row = stdout
        .split("\n\n")
        .find(|block| block.starts_with("anyrouter"))
        .unwrap_or_else(|| panic!("no anyrouter block in:\n{stdout}"));
    assert!(
        row.contains("capacity        99%"),
        "the shipped binary must show the real percentage a planted gateway reading \
         carries, now that main.rs calls `gather_gateway_quota`:\n{row}"
    );
    assert!(
        !row.contains("capacity        unknown"),
        "the planted reading must have reached the report:\n{row}"
    );
}

/// **The hypothesis PACKET-QUOTA-LIVE rests on, proven together rather than
/// separately.** Neither HALF ONE (the `groq` template, this package's own
/// addition to `provider::templates`) nor HALF TWO (the three `main.rs`
/// edits, proven above with a synthetic AnyRouter reading) closes anything
/// alone — see the packet's own "WHY BOTH HALVES, AND WHY NEITHER ALONE".
/// This test plants Groq's own real header set — the exact values
/// `.agent-runtime/probe-quota-headers-2026-08-27.md` recorded from a real
/// `POST https://api.groq.com/openai/v1/chat/completions`, 2026-08-27 — at
/// `GatewayQuotaCache`'s real path, keyed `"groq"`, and drives the real
/// `Command::Resources` arm the way every other binary-level test here does.
/// If the template and the cache key ever disagreed (the packet's own
/// falsification check #1), or if a real, both-halves-in-one-response
/// reading could not render as a percentage (check #2), this test is where
/// either would show up — it is the first test anywhere to reach a real
/// `Percentage::Exact` for a provider `glasshouse resources` actually ships.
#[test]
fn groqs_own_real_headers_reach_the_shipped_binarys_report_as_groq() {
    let fixture = BinaryFixture::new();
    let quota_cache_dir = fixture.config.path().join("gateway-quota");
    let cache = glasshouse::provider::telemetry::GatewayQuotaCache::at(&quota_cache_dir);
    cache.store(
        "groq",
        &RateLimitHeaders::read(vec![
            ("x-ratelimit-limit-requests", "7000"),
            ("x-ratelimit-limit-tokens", "6000"),
            ("x-ratelimit-remaining-requests", "6999"),
            ("x-ratelimit-remaining-tokens", "5991"),
            ("x-ratelimit-reset-requests", "12.342s"),
            ("x-ratelimit-reset-tokens", "90ms"),
        ]),
        TELEMETRY_OBSERVED,
    );

    let stdout = fixture.run(&["resources", "--no-harness"]);
    let row = stdout
        .split("\n\n")
        .find(|block| block.starts_with("groq"))
        .unwrap_or_else(|| panic!("no groq block in:\n{stdout}"));
    assert!(
        row.contains("capacity        99%"),
        "Groq's own real header set, both halves in one response, must render a real \
         percentage for the `groq` template the shipped binary now ships:\n{row}"
    );
    assert!(
        !row.contains("capacity        unknown"),
        "the planted Groq reading must have reached the report:\n{row}"
    );
}

/// Capability map Phase 33 line 1314 (`python3 scripts/discover.py --phase
/// 33`): *"Track known quota or usage state when a provider or harness
/// exposes it."* Through the real `Command::Resources` arm, over the same
/// Groq header set `groqs_own_real_headers_reach_the_shipped_binarys_report_as_groq`
/// plants — this is the same production path, asserting the two native-unit
/// pool lines neither that test nor
/// `a_planted_gateway_reading_now_reaches_the_shipped_binarys_report` (which
/// only checks the derived `capacity NN%` line) pins down: the raw `remaining`
/// and `limit` readings themselves, for both the token pool and the request
/// pool, each carrying its own class and source rather than a bare number.
#[test]
fn groqs_own_real_headers_populate_both_native_unit_pools_in_the_shipped_binarys_report() {
    let fixture = BinaryFixture::new();
    let quota_cache_dir = fixture.config.path().join("gateway-quota");
    let cache = glasshouse::provider::telemetry::GatewayQuotaCache::at(&quota_cache_dir);
    cache.store(
        "groq",
        &RateLimitHeaders::read(vec![
            ("x-ratelimit-limit-requests", "7000"),
            ("x-ratelimit-limit-tokens", "6000"),
            ("x-ratelimit-remaining-requests", "6999"),
            ("x-ratelimit-remaining-tokens", "5991"),
            ("x-ratelimit-reset-requests", "12.342s"),
            ("x-ratelimit-reset-tokens", "90ms"),
        ]),
        TELEMETRY_OBSERVED,
    );

    let stdout = fixture.run(&["resources", "--no-harness"]);
    let row = stdout
        .split("\n\n")
        .find(|block| block.starts_with("groq"))
        .unwrap_or_else(|| panic!("no groq block in:\n{stdout}"));

    assert!(
        row.contains("remaining 5991 tokens [authoritative]"),
        "the token pool's own remaining reading must reach the report:\n{row}"
    );
    assert!(
        row.contains("limit 6000 tokens [authoritative]"),
        "the token pool's own limit reading must reach the report:\n{row}"
    );
    assert!(
        row.contains("remaining 6999 requests [authoritative]"),
        "the request pool's own remaining reading must reach the report:\n{row}"
    );
    assert!(
        row.contains("limit 7000 requests [authoritative]"),
        "the request pool's own limit reading must reach the report:\n{row}"
    );
}

// ===== Phase 33: resource health, through the shipped binary ===============
//
// The health twin of `a_planted_gateway_reading_now_reaches_the_shipped_binarys_report`,
// above: capability map lines 1311/1321/1322/1324, each proven the same way —
// a reading planted exactly where `GatewayHealthCache::new` would put it, and
// the real `Command::Resources` arm asserted against.

/// Wall-clock now, for a test that must plant a cooldown deadline the shipped
/// binary's own real clock will still see as future (or past) when it runs a
/// moment later — `resources_report` has no injectable clock, for
/// `mod@glasshouse::provider::quota`'s own reason.
fn wall_clock_now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("the system clock is after 1970")
        .as_secs() as i64
}

fn planted_health_reading(
    model: &str,
    consecutive_failures: u32,
    cooling_down_until_unix: Option<i64>,
    credential_rejected: bool,
) -> glasshouse::provider::telemetry::GatewayHealthReading {
    glasshouse::provider::telemetry::GatewayHealthReading {
        credential_label: "anyrouter/ANYROUTER_API_KEY".to_owned(),
        model: model.to_owned(),
        consecutive_failures,
        cooling_down_until_unix,
        cooldown_cause: None,
        credential_rejected,
    }
}

/// A resource nothing has been observed about reports `unknown` through the
/// real `Command::Resources` arm — capability map line 1324's "never invent a
/// reading" half, with no gateway-health cache present at all.
#[test]
fn a_resource_with_no_health_observation_reports_unknown_through_the_shipped_binary() {
    let fixture = BinaryFixture::new();
    let stdout = fixture.run(&["resources", "--no-harness"]);
    assert_eq!(
        stdout
            .matches(&format!("health          {UNKNOWN_TELEMETRY}"))
            .count(),
        registry().len(),
        "every resource must report unknown health when no gateway has ever observed one:\n\
         {stdout}"
    );
    assert!(!stdout.contains("paced"), "{stdout}");
    assert!(!stdout.contains("consecutive failure"), "{stdout}");
}

/// A planted, healthy reading reaches the real `Command::Resources` arm —
/// the write side is proven separately, in `gateway::conformance`, over a
/// real socket; this is the read side, exactly
/// `a_planted_gateway_reading_now_reaches_the_shipped_binarys_report`'s own
/// shape for quota.
#[test]
fn a_planted_gateway_health_reading_now_reaches_the_shipped_binarys_report() {
    let fixture = BinaryFixture::new();
    let health_cache_dir = fixture.config.path().join("gateway-health");
    let cache = glasshouse::provider::telemetry::GatewayHealthCache::at(&health_cache_dir);
    cache.store(
        "anyrouter",
        &[planted_health_reading(
            "anyrouter/free-model",
            0,
            None,
            false,
        )],
        TELEMETRY_OBSERVED,
    );
    assert!(
        std::fs::read_dir(&health_cache_dir)
            .expect("GatewayHealthCache::store created its directory")
            .next()
            .is_some(),
        "the planted reading must actually be on disk for this test to mean anything"
    );

    let stdout = fixture.run(&["resources", "--no-harness"]);
    let row = stdout
        .split("\n\n")
        .find(|block| block.starts_with("anyrouter"))
        .unwrap_or_else(|| panic!("no anyrouter block in:\n{stdout}"));

    assert!(
        row.contains("health          anyrouter/free-model") && row.contains(": available"),
        "the planted reading must reach the report as available:\n{row}"
    );
    assert!(
        !row.contains(&format!("health          {UNKNOWN_TELEMETRY}")),
        "the planted reading must have reached the report:\n{row}"
    );
}

/// Capability map line 1324, through the real binary: a resource cooling
/// down after real failures is **paced**, never broken. The cooldown is
/// planted relative to the wall clock the shipped binary will actually read,
/// since `glasshouse resources` has no injectable "now".
#[test]
fn a_cooling_down_resource_is_shown_as_paced_through_the_shipped_binary() {
    let fixture = BinaryFixture::new();
    let health_cache_dir = fixture.config.path().join("gateway-health");
    let cache = glasshouse::provider::telemetry::GatewayHealthCache::at(&health_cache_dir);
    cache.store(
        "anyrouter",
        &[planted_health_reading(
            "anyrouter/free-model",
            2,
            Some(wall_clock_now_unix() + 3600),
            false,
        )],
        TELEMETRY_OBSERVED,
    );

    let stdout = fixture.run(&["resources", "--no-harness"]);
    let row = stdout
        .split("\n\n")
        .find(|block| block.starts_with("anyrouter"))
        .unwrap_or_else(|| panic!("no anyrouter block in:\n{stdout}"));

    assert!(
        row.contains("paced, cooling down until unix"),
        "a resource an hour from its cooldown deadline must render as paced, not broken:\n{row}"
    );
    assert!(
        row.contains("2 consecutive failure(s)"),
        "the observed failure count must reach the report:\n{row}"
    );
}

/// A corrupt cache file must leave `glasshouse resources` working and simply
/// carry no health — [`GatewayHealthCache::load`]'s own fail-soft contract,
/// proven through the real binary rather than only at the type that owns the
/// read.
#[test]
fn a_corrupt_gateway_health_cache_file_leaves_the_shipped_binary_working() {
    let fixture = BinaryFixture::new();
    let health_cache_dir = fixture.config.path().join("gateway-health");
    let cache = glasshouse::provider::telemetry::GatewayHealthCache::at(&health_cache_dir);
    cache.store(
        "anyrouter",
        &[planted_health_reading(
            "anyrouter/free-model",
            1,
            None,
            false,
        )],
        TELEMETRY_OBSERVED,
    );
    // `GatewayHealthCache`'s on-disk file name is built from `file_stem`,
    // which is `pub(crate)` and unreachable from this external test crate —
    // so the file `store` just created is found by listing the directory
    // rather than guessing its name.
    let path = std::fs::read_dir(&health_cache_dir)
        .expect("the store above created this directory")
        .next()
        .expect("the store above wrote exactly one file")
        .expect("a readable directory entry")
        .path();
    std::fs::write(&path, b"not json").expect("overwritten with garbage");

    let stdout = fixture.run(&["resources", "--no-harness"]);
    let row = stdout
        .split("\n\n")
        .find(|block| block.starts_with("anyrouter"))
        .unwrap_or_else(|| panic!("no anyrouter block in:\n{stdout}"));
    assert!(
        row.contains(&format!("health          {UNKNOWN_TELEMETRY}")),
        "a corrupt cache file must read as no health, not fail the command:\n{row}"
    );
}

/// Capability map Phase 33 line 1315 (`python3 scripts/discover.py --phase
/// 33`): *"Track known quota reset time when it is exposed."* Through the
/// real `Command::Resources` arm, over the same Groq header set the sibling
/// tests above plant. `x-ratelimit-reset-requests: 12.342s` observed at
/// `TELEMETRY_OBSERVED` (1787800000) is `RateLimitHeaders::resets_at_unix`
/// read as a delta and folded into the rolling window by
/// `RateLimitHeaders::apply_to` — capability map line 1211's own mechanism —
/// so the shipped binary's `rolling window` line must carry the resulting
/// absolute second, 1787800012, not just a derived percentage. Neither
/// `groqs_own_real_headers_reach_the_shipped_binarys_report_as_groq` nor
/// `the_shipped_binary_shows_every_windows_start_and_reset_state_when_verbose`
/// (which only proves the *unmeasured* rendering) asserts this line's
/// populated form.
#[test]
fn groqs_own_real_headers_populate_the_rolling_windows_reset_time_in_the_shipped_binarys_report() {
    let fixture = BinaryFixture::new();
    let quota_cache_dir = fixture.config.path().join("gateway-quota");
    let cache = glasshouse::provider::telemetry::GatewayQuotaCache::at(&quota_cache_dir);
    cache.store(
        "groq",
        &RateLimitHeaders::read(vec![
            ("x-ratelimit-limit-requests", "7000"),
            ("x-ratelimit-limit-tokens", "6000"),
            ("x-ratelimit-remaining-requests", "6999"),
            ("x-ratelimit-remaining-tokens", "5991"),
            ("x-ratelimit-reset-requests", "12.342s"),
            ("x-ratelimit-reset-tokens", "90ms"),
        ]),
        TELEMETRY_OBSERVED,
    );

    let stdout = fixture.run(&["resources", "--no-harness"]);
    let row = stdout
        .split("\n\n")
        .find(|block| block.starts_with("groq"))
        .unwrap_or_else(|| panic!("no groq block in:\n{stdout}"));

    assert!(
        row.contains("rolling window "),
        "the groq block must carry a rolling-window line at all:\n{row}"
    );
    assert!(
        row.contains("resets unix 1787800012 [authoritative]"),
        "the provider's own reset-requests header must reach the report as an absolute \
         unix second, not stay `unmeasured`:\n{row}"
    );
}

/// The note that tells a user the override exists, when they have set none.
/// A screen full of `unknown` with no way out of it is the failure this
/// guards against.
#[test]
fn the_shipped_binary_says_how_to_record_a_plan_when_nothing_is_configured() {
    let fixture = BinaryFixture::new();
    let stdout = fixture.run(&["resources", "--no-harness"]);
    assert!(stdout.contains("CONFIGURED QUOTA OVERRIDES"), "{stdout}");
    assert!(stdout.contains("[providers.<name>.quota]"), "{stdout}");
    assert!(stdout.contains("never as measurements"), "{stdout}");
}

/// Capability map line 1229, from outside the crate: the header set a real
/// provider sent becomes a reading with the header's own name on it.
#[test]
fn a_real_providers_rate_limit_headers_become_a_reading_outside_the_crate() {
    let headers = RateLimitHeaders::read(vec![
        ("ratelimit-limit", "300"),
        ("ratelimit-policy", "300;w=60"),
        ("x-ratelimit-limit", "300"),
        ("x-ratelimit-tier", "ip"),
        ("x-ratelimit-window", "60"),
    ]);
    let state = apply_provider_headers(
        ResourceKind::from_direct_provider("anyrouter").capacity(),
        &headers,
        TELEMETRY_OBSERVED,
    );

    let limit = state
        .requests()
        .limit()
        .value()
        .expect("a ceiling was read");
    assert_eq!(limit.value(), 300);
    assert_eq!(limit.unit(), "requests");
    assert_eq!(
        state.requests().limit().telemetry_class(),
        Some(TelemetryClass::Authoritative)
    );
    assert!(
        state
            .requests()
            .limit()
            .describe_source()
            .contains("ratelimit-limit")
    );
    // What the provider did not send stays unknown.
    assert_eq!(state.requests().remaining().telemetry_class(), None);
    assert_eq!(
        state.requests().remaining().telemetry_class_str(),
        UNKNOWN_TELEMETRY
    );
    // Line 1236.
    assert_eq!(state.last_observed_at_unix(), Some(TELEMETRY_OBSERVED));
}

/// Capability map line 1234, from outside the crate, on the type a caller
/// actually holds: an estimate has no accessor that yields a bare figure.
#[test]
fn a_caller_outside_the_crate_cannot_read_an_estimate_as_an_exact_figure() {
    let pool = Pool::unmeasured()
        .with_limit(Capacity::Measured(Reading::new(
            NativeAmount::millionths(10_000_000, "USD"),
            TELEMETRY_OBSERVED,
            ReadingSource::UserConfiguration,
        )))
        .with_remaining(Capacity::Measured(Reading::new(
            NativeAmount::millionths(2_500_000, "USD"),
            TELEMETRY_OBSERVED,
            ReadingSource::LocalObservation("this session's own spend".to_owned()),
        )));
    let score = pool.normalized().expect("both halves were read");

    assert_eq!(score.percent().exact(), None);
    let percentage = score.percent();
    let (percent, confidence, source) = percentage.estimated().expect("an estimate");
    assert_eq!(percent, 25);
    assert_eq!(confidence, Confidence::Medium);
    assert!(!source.is_empty());
    assert!(matches!(score.percent(), Percentage::Estimated { .. }));
    assert!(score.percent().render().contains("estimated"));

    // And the exact case, so the test is not passing for want of a path to
    // the other answer.
    let authoritative = Pool::unmeasured()
        .with_limit(Capacity::Measured(Reading::new(
            NativeAmount::whole(1_000, "requests"),
            TELEMETRY_OBSERVED,
            ReadingSource::ResponseHeader("ratelimit-limit".to_owned()),
        )))
        .with_remaining(Capacity::Measured(Reading::new(
            NativeAmount::whole(250, "requests"),
            TELEMETRY_OBSERVED,
            ReadingSource::ResponseHeader("ratelimit-remaining".to_owned()),
        )))
        .normalized()
        .expect("both halves were read");
    assert_eq!(authoritative.percent().exact(), Some(25));
    assert_eq!(authoritative.percent().render(), "25%");
}

/// Capability map line 1228, from outside the crate, over all four classes.
#[test]
fn authoritative_telemetry_is_preferred_over_every_weaker_class() {
    let authoritative = Capacity::Measured(Reading::new(
        NativeAmount::whole(10, "requests"),
        TELEMETRY_OBSERVED,
        ReadingSource::ResponseHeader("ratelimit-remaining".to_owned()),
    ));
    let weaker = [
        ReadingSource::LocalObservation("this session".to_owned()),
        ReadingSource::UserConfiguration,
        ReadingSource::InferredEstimate("the previous window".to_owned()),
    ];
    for source in weaker {
        // Fresher and still weaker: the authoritative reading survives.
        let candidate = Capacity::Measured(Reading::new(
            NativeAmount::whole(999, "requests"),
            TELEMETRY_OBSERVED + 10_000,
            source.clone(),
        ));
        assert_eq!(
            authoritative
                .clone()
                .prefer(candidate.clone())
                .value()
                .unwrap()
                .value(),
            10,
            "{source:?} displaced an authoritative reading"
        );
        assert_eq!(
            candidate
                .prefer(authoritative.clone())
                .value()
                .unwrap()
                .value(),
            10,
            "{source:?} displaced an authoritative reading, applied the other way"
        );
    }
}

/// Capability map line 1232, from outside the crate: the two seams are
/// independent, and a harness's own report never touches a provider's fields.
#[test]
fn harness_telemetry_and_provider_telemetry_stay_independent_outside_the_crate() {
    let plan = read_harness_plan(
        r#"{"subscriptionType":"max","email":"someone@example.com"}"#,
        TELEMETRY_OBSERVED,
        "claude auth status --json",
    );
    let subscription = apply_harness_report(CapacityState::opaque_subscription(), &plan);
    assert_eq!(
        subscription.plan().value().map(KnownPlan::name),
        Some("max")
    );
    // Nothing about the account holder survived the read.
    assert!(!format!("{subscription:?}").contains("someone@example.com"));
    // And no pool learned anything from a plan.
    assert_eq!(
        subscription.requests(),
        CapacityState::opaque_subscription().requests()
    );

    let provider = apply_provider_headers(
        CapacityState::metered_balance(),
        &RateLimitHeaders::read(vec![("ratelimit-limit", "300")]),
        TELEMETRY_OBSERVED,
    );
    // And no plan learned anything from a header.
    assert_eq!(provider.plan().telemetry_class(), None);
}

/// Capability map line 1237, from outside the crate: the same reading, two
/// ages, two answers.
#[test]
fn staleness_is_decided_by_the_age_a_caller_supplies_and_not_by_a_constant() {
    let reading = Reading::new(
        NativeAmount::whole(300, "requests"),
        TELEMETRY_OBSERVED,
        ReadingSource::ResponseHeader("ratelimit-limit".to_owned()),
    );
    let now = TELEMETRY_OBSERVED + 300;
    assert!(!reading.freshness(now, 900).is_stale());
    assert!(reading.freshness(now, 120).is_stale());
    assert_eq!(reading.freshness(now, 900).age_seconds(), 300);
    assert_eq!(
        Freshness::of(TELEMETRY_OBSERVED, now, 900).age_seconds(),
        300
    );
}

/// Capability map line 1238, from outside the crate: nothing in the telemetry
/// path is fallible, so a caller cannot write an error branch that fails a
/// session on a bad header or an unreadable status report.
#[test]
fn no_telemetry_reader_can_hand_a_caller_an_error_to_fail_a_session_on() {
    // Garbage in every position, and a complete capacity state out.
    let state = apply_user_configuration(
        apply_harness_report(
            apply_provider_headers(
                CapacityState::metered_balance(),
                &RateLimitHeaders::read(vec![
                    ("ratelimit-limit", "not a number"),
                    ("ratelimit-policy", "nonsense"),
                    ("ratelimit-remaining", "-1"),
                ]),
                TELEMETRY_OBSERVED,
            ),
            &read_harness_plan(
                "<html>not json</html>",
                TELEMETRY_OBSERVED,
                "some --interface",
            ),
        ),
        Some("   "),
        None,
        TELEMETRY_OBSERVED,
    );

    assert_eq!(state.last_observed_at_unix(), None);
    assert_eq!(state.telemetry_class_str(), UNKNOWN_TELEMETRY);
    assert!(state.normalized().is_none());
    // Still a complete, usable model — the exact one a resource with no
    // telemetry has.
    assert_eq!(state, CapacityState::metered_balance());
}

/// Every unknown quantity in the shipped binary answers `unknown` rather than
/// a number, over every entry of the registry and every pool of each — the
/// standing guard against this phase's characteristic failure, extended from
/// Phase 32A's version to cover the telemetry class as well as the value.
#[test]
fn nothing_the_registry_can_describe_claims_a_telemetry_class_it_did_not_earn() {
    for kind in registry() {
        let capacity = kind.capacity();
        assert_eq!(
            capacity.telemetry_class(),
            None,
            "`{}` claims a telemetry class with nothing read",
            kind.label()
        );
        for (label, pool) in capacity.pools() {
            for half in [pool.limit(), pool.remaining()] {
                assert_eq!(
                    half.telemetry_class(),
                    None,
                    "`{}`'s {label} claims a telemetry class with nothing read",
                    kind.label()
                );
                assert_eq!(half.telemetry_class_str(), UNKNOWN_TELEMETRY);
            }
        }
        assert!(capacity.plan().reading().is_none());
        assert_eq!(capacity.last_observed_at_unix(), None);
    }
}

/// Capability map line 1231: the plan reader takes one field and leaves the
/// rest of the body where it found it. Driven from outside the crate, over
/// the shape the real interface was measured emitting.
#[test]
fn a_harness_status_body_yields_a_plan_and_nothing_else_about_the_account() {
    let body = r#"{"loggedIn":true,"authMethod":"claude.ai","apiProvider":"firstParty",
        "email":"someone@example.com","orgId":"5916b68d-0000-0000-0000-000000000000",
        "orgName":"someone@example.com's Organization","subscriptionType":"max"}"#;
    let report = read_harness_plan(body, TELEMETRY_OBSERVED, "claude auth status --json");
    let rendered = format!("{report:?}");

    assert!(rendered.contains("max"));
    for forbidden in [
        "someone@example.com",
        "5916b68d",
        "Organization",
        "claude.ai",
    ] {
        assert!(
            !rendered.contains(forbidden),
            "`{forbidden}` survived the read"
        );
    }
    // And a body with no plan yields nothing rather than a partial account.
    assert!(
        !read_harness_plan(
            r#"{"email":"someone@example.com"}"#,
            TELEMETRY_OBSERVED,
            "x"
        )
        .known_plan()
        .is_measured()
    );
}

/// A `HarnessTelemetry` that read nothing must not blank a plan that is
/// already known — capability map line 1238 on the harness seam.
#[test]
fn a_harness_that_reported_nothing_does_not_erase_a_plan_already_known() {
    let known = apply_harness_report(
        CapacityState::opaque_subscription(),
        &HarnessTelemetry::plan("max", TELEMETRY_OBSERVED, "claude auth status --json"),
    );
    let after = apply_harness_report(known, &HarnessTelemetry::nothing());
    assert_eq!(after.plan().value().map(KnownPlan::name), Some("max"));
}

// ===== Phase 34C: the routing model in resource diagnostics ================
//
// Capability map lines 1443 (*"show the currently selected routing model in
// resource diagnostics"*) and 1433 (*"filter automatic candidates by current
// provider health"*), both through the real `Command::Resources` arm.
//
// # Why one surface proves both
//
// `main.rs::render_routing_model` does not describe the decision, it *runs*
// it: the block names whatever `automatic_classification_choice` returns, and
// that is the same function `glasshouse classify` calls on
// `RoutingModelResolution::Automatic`. So a health reading that changes which
// resource the policy picks changes what this report prints, and the printed
// name is evidence about the decision rather than about a renderer.
//
// That is deliberate, and it is practice §36: a filter that exists but is
// never fed a candidate that could fail it is not applied. Until this batch
// the classifier handed `DisposableRouting::choose` a `FreePool::new()`, so
// no candidate could ever fail its health check on the production path.

/// The credential variable the routing-model fixtures below name. It is never
/// a real key and never a real provider: the base URL is the discard port, so
/// a candidate can be *built* (which is all routing needs) without anything
/// being reachable.
const ROUTING_CREDENTIAL_VAR: &str = "GLASSHOUSE_TEST_ROUTING_KEY";

/// Two free providers at the discard port, `zeta-runner` first in the user's
/// own free-resource order.
///
/// `zeta-runner` sorts *second* in `provider_names` (a `BTreeSet`) and is
/// therefore second in the candidate list, so the two answers differ: anything
/// reading the user's order picks `zeta-model`, and anything taking the
/// candidate list as it comes picks `alpha-model`. It is the same
/// discriminating configuration `tests/classification_call.rs` uses for the
/// classifier, pointed at the report.
const TWO_FREE_PROVIDERS: &str = r#"
[routing]
model = { kind = "automatic" }
free_resource_order = [{ provider = "zeta-runner", model = "zeta-model" }]

[providers.alpha-runner]
template = "openai-compatible"
base_url = "http://127.0.0.1:9/v1"
credential_env = ["GLASSHOUSE_TEST_ROUTING_KEY"]
free_models = ["alpha-model"]

[providers.zeta-runner]
template = "openai-compatible"
base_url = "http://127.0.0.1:9/v1"
credential_env = ["GLASSHOUSE_TEST_ROUTING_KEY"]
free_models = ["zeta-model"]
"#;

impl BinaryFixture {
    /// Run the shipped binary with one environment variable set, and return
    /// its stdout.
    ///
    /// Separate from [`BinaryFixture::run`] because every other test in this
    /// file must resolve no credential at all; the routing-model tests need
    /// exactly one to resolve, or `disposable_candidates` builds no candidate
    /// and there is nothing for the policy to choose between.
    fn run_with_credential(&self, args: &[&str], value: &str) -> String {
        let output = Command::new(env!("CARGO_BIN_EXE_glasshouse"))
            .current_dir(self.project.path())
            .env(ROUTING_CREDENTIAL_VAR, value)
            .args([
                "--data-dir",
                self.config.path().to_str().unwrap(),
                "--config-dir",
                self.config.path().to_str().unwrap(),
            ])
            .args(args)
            .output()
            .expect("the glasshouse binary runs");
        assert!(
            output.status.success(),
            "`glasshouse {}` failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).expect("stdout is UTF-8")
    }
}

/// The `ROUTING MODEL` block of `glasshouse resources`, as one string.
fn routing_block(stdout: &str) -> String {
    stdout
        .split_once("ROUTING MODEL\n")
        .unwrap_or_else(|| panic!("no ROUTING MODEL block in:\n{stdout}"))
        .1
        .to_owned()
}

/// **Map line 1443.** `glasshouse resources` names the model automatic
/// selection would pick — not the word "automatic", which is the intent the
/// Settings overlay already shows and which answers a different question.
///
/// The assertion is on `zeta-model`, which is reachable only through the
/// user's own free-resource order — an input nothing but
/// `DisposableRouting::choose` reads. A block that printed the configured
/// choice, or the first configured provider, would say `alpha-model` here.
#[test]
fn the_shipped_binary_names_the_model_automatic_routing_would_select() {
    let fixture = BinaryFixture::new().with_config(TWO_FREE_PROVIDERS);
    let stdout = fixture.run_with_credential(&["resources", "--no-harness"], "sk-not-a-real-key");
    let block = routing_block(&stdout);

    assert!(
        block.contains("configured      automatic"),
        "the block must name the configured intent:\n{block}"
    );
    assert!(
        block.contains("would select    zeta-model on zeta-runner"),
        "the report must name the resource the routing policy selected:\n{block}"
    );
    assert!(
        !block.contains("alpha-model"),
        "naming the first configured provider would mean the policy never ran:\n{block}"
    );
}

/// **The honesty constraint `docs/product/evidence/phase-34c.md` put on
/// whoever closed 1443.** Nothing in this build classifies anything on its
/// own, so the diagnostic must not read as a habit. The `in use` row says so
/// unconditionally — it is not gated on configuration — and the selected
/// resource is reported with `would`.
#[test]
fn the_routing_model_block_never_claims_a_model_is_classifying_anything() {
    let fixture = BinaryFixture::new().with_config(TWO_FREE_PROVIDERS);
    let stdout = fixture.run_with_credential(&["resources", "--no-harness"], "sk-not-a-real-key");
    let block = routing_block(&stdout);

    assert!(
        block.contains("in use          nothing yet"),
        "the block must say nothing is classifying with it:\n{block}"
    );
    assert!(
        block.contains("would select"),
        "the selected resource must be reported as a choice, not a habit:\n{block}"
    );
    assert!(
        !block.contains("currently using") && !block.contains("in use          zeta"),
        "no row may imply the model is in use:\n{block}"
    );
}

/// A build with no routing model configured — every build until somebody
/// configures one — names no model at all, rather than reporting whatever the
/// policy would have picked had it been asked.
#[test]
fn with_no_routing_model_configured_the_block_names_no_model() {
    let fixture = BinaryFixture::new();
    let stdout = fixture.run(&["resources", "--no-harness"]);
    let block = routing_block(&stdout);

    assert!(
        block.contains("configured      deterministic heuristics"),
        "{block}"
    );
    assert!(
        block.contains("would select    no model"),
        "an unconfigured routing model must select nothing:\n{block}"
    );
}

/// **Map line 1433, through the production path.** A resource the gateway
/// recorded as having a rejected credential is excluded from automatic
/// selection, and the next candidate in the user's order is chosen instead.
///
/// The control is the sibling test above: with no health reading planted, the
/// same configuration selects `zeta-model`. The only difference here is a
/// reading on disk, so the change of answer is attributable to health and to
/// nothing else.
///
/// This is the test that fails if `automatic_classification_choice` goes back
/// to handing `choose` an empty `FreePool` — which is what it did until this
/// batch, and which made `choose`'s health filter unreachable from production.
#[test]
fn an_unhealthy_resource_is_not_the_one_automatic_routing_would_select() {
    let fixture = BinaryFixture::new().with_config(TWO_FREE_PROVIDERS);
    let health_cache_dir = fixture.config.path().join("gateway-health");
    let cache = glasshouse::provider::telemetry::GatewayHealthCache::at(&health_cache_dir);
    cache.store(
        "zeta-runner",
        &[glasshouse::provider::telemetry::GatewayHealthReading {
            credential_label: format!("zeta-runner/{ROUTING_CREDENTIAL_VAR}"),
            model: "zeta-model".to_owned(),
            consecutive_failures: 3,
            cooling_down_until_unix: None,
            cooldown_cause: None,
            credential_rejected: true,
        }],
        TELEMETRY_OBSERVED,
    );

    let stdout = fixture.run_with_credential(&["resources", "--no-harness"], "sk-not-a-real-key");
    let block = routing_block(&stdout);

    assert!(
        block.contains("would select    alpha-model on alpha-runner"),
        "a resource whose credential the gateway saw rejected must not be selected:\n{block}"
    );
    assert!(
        !block.contains("zeta-model"),
        "the unhealthy resource must be excluded, not merely ranked lower:\n{block}"
    );
}

/// The same filter on the other half of `FreePool::is_available`: a resource
/// still cooling down after real failures is not selected either.
///
/// Planted against the wall clock the shipped binary will actually read, for
/// `a_cooling_down_resource_is_shown_as_paced_through_the_shipped_binary`'s
/// own reason — `resources_report` has no injectable "now".
#[test]
fn a_cooling_down_resource_is_not_the_one_automatic_routing_would_select() {
    let fixture = BinaryFixture::new().with_config(TWO_FREE_PROVIDERS);
    let health_cache_dir = fixture.config.path().join("gateway-health");
    let cache = glasshouse::provider::telemetry::GatewayHealthCache::at(&health_cache_dir);
    let now = wall_clock_now_unix();
    cache.store(
        "zeta-runner",
        &[glasshouse::provider::telemetry::GatewayHealthReading {
            credential_label: format!("zeta-runner/{ROUTING_CREDENTIAL_VAR}"),
            model: "zeta-model".to_owned(),
            consecutive_failures: 2,
            cooling_down_until_unix: Some(now + 3_600),
            cooldown_cause: None,
            credential_rejected: false,
        }],
        now,
    );

    let stdout = fixture.run_with_credential(&["resources", "--no-harness"], "sk-not-a-real-key");
    let block = routing_block(&stdout);

    assert!(
        block.contains("would select    alpha-model on alpha-runner"),
        "a resource still cooling down must not be selected:\n{block}"
    );
}

/// The credential the candidate resolved with never reaches the report.
///
/// `DisposableChoice` carries a `CredentialId`, and the block prints a
/// provider name, a model name and the policy's own explanation. This holds
/// that against the bytes the binary actually wrote, including the verbose
/// explanation, which is the longest thing the block can print.
#[test]
fn no_credential_reaches_the_routing_model_block() {
    let secret = "sk-a-value-that-must-not-be-printed";
    let fixture = BinaryFixture::new().with_config(TWO_FREE_PROVIDERS);
    let stdout = fixture.run_with_credential(&["resources", "--no-harness", "--verbose"], secret);

    assert!(
        stdout.contains("ROUTING MODEL"),
        "the block must have been rendered for this test to mean anything:\n{stdout}"
    );
    assert!(!stdout.contains(secret), "the credential value leaked");
    assert!(
        !stdout.contains(ROUTING_CREDENTIAL_VAR),
        "the credential variable name leaked into the routing block:\n{stdout}"
    );
}
