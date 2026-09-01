//! Capability map lines 1475–1479 and 1482–1485 — Phase 34F, the 56A
//! ruling's work item 2 (`docs/product/design-decisions.md`, "Step 4's
//! fallback order"): a model's capability as configurable data, widening
//! `providers.<p>.model_ceilings` (already proven live by `tests/tier_ceiling.rs`)
//! to structured-output suitability, task-kind suitability, the
//! harness-model pairing class, evidence strength, and — line 1484's own
//! requirement — a benchmark-derived seed that can rank but never refuse.
//!
//! One test per box line, `tier_1479_...` proving the override beats the
//! seed, `tier_1485_...` proving two providers of the same nominal model
//! resolve independently, and the last test in this file the production-
//! caller assertion: it runs the shipped binary exactly as
//! `tests/tier_ceiling.rs` does, with a capability record and *no*
//! `model_ceilings` override, and shows the same rejection that file proves
//! for an override — the routing gate observably changing a decision
//! because of a configured capability record, not only a struct in a test.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use glasshouse::config::ProviderConfig;
use glasshouse::config::capability::{
    CapabilityProvenance, CapabilityQuery, CeilingResolution, ModelCapabilityRecord,
    TaskSuitability,
};
use glasshouse::harness::pairing::PairingClass;
use glasshouse::integrations::IntegrationId;
use glasshouse::routing::classify::WorkloadTier;

// --- 1475: configurable data, not hard-coded router logic ------------------

/// **Line 1475.** A capability record is data a provider entry holds and
/// hands back — round-tripped through TOML exactly as
/// `providers.alpha.model_ceilings` already is, and read back through
/// `ProviderConfig::model_capability` rather than any hard-coded table.
#[test]
fn line_1475_a_capability_record_round_trips_through_toml() {
    let toml_text = "template = \"openrouter\"\n\
         [model_capabilities.small]\n\
         ceiling = \"leaf\"\n\
         structured_output_suitable = true\n\
         task_suitability = \"support-only\"\n\
         provenance = \"user\"\n\
         pairing_class = \"vendor-native\"\n\
         evidence_strength = \"observed\"\n";

    let provider: ProviderConfig = toml::from_str(toml_text).expect("a well-formed record parses");
    let record = provider
        .model_capability("small")
        .expect("the record must be readable back off the provider, not just parsed and dropped");

    assert_eq!(record.ceiling(), Some(WorkloadTier::Leaf));
    assert!(record.structured_output_suitable());
    assert_eq!(record.task_suitability(), TaskSuitability::SupportOnly);
    assert_eq!(record.provenance(), CapabilityProvenance::User);
    assert_eq!(record.pairing_class(), Some(PairingClass::VendorNative));

    let rewritten = toml::to_string(&provider).expect("a round-tripped provider must re-serialize");
    let reparsed: ProviderConfig =
        toml::from_str(&rewritten).expect("the rewritten form must reparse");
    assert_eq!(
        reparsed.model_capability("small"),
        provider.model_capability("small"),
        "serialize -> parse must be lossless, or the config file drifts every time it is saved"
    );
}

/// **Line 1475's other half.** An unrecognised field inside one capability
/// record is refused at load, not silently dropped — the same
/// `deny_unknown_fields` idiom `EntitlementConfig` already uses, so a typo'd
/// key does not read as "this axis was never stated".
#[test]
fn line_1475_an_unknown_field_in_a_capability_record_is_refused() {
    let toml_text = "provenance = \"user\"\nfictional_field = 1\n";
    let parsed = toml::from_str::<ModelCapabilityRecord>(toml_text);
    assert!(
        parsed.is_err(),
        "an unknown field must fail to parse, not be silently ignored: {parsed:?}"
    );
}

// --- 1476/1477/1478/1483: the record's own facts ----------------------------

/// **Line 1476.** The initial ceiling a record states is exactly what comes
/// back — and, with no override or task-kind cap in play, exactly what
/// `resolve_ceiling` uses.
#[test]
fn line_1476_initial_ceiling_is_recorded_and_resolves_with_no_override() {
    let mut record = ModelCapabilityRecord::new(CapabilityProvenance::User);
    record.set_ceiling(Some(WorkloadTier::Standard));

    assert_eq!(record.ceiling(), Some(WorkloadTier::Standard));

    let mut provider = ProviderConfig::new("openrouter");
    provider.set_model_capabilities(std::collections::BTreeMap::from([("m".to_owned(), record)]));
    assert_eq!(
        provider.resolved_ceiling("m").hard_ceiling(),
        Some(WorkloadTier::Standard)
    );
}

/// **Line 1477.** Structured-output suitability is a plain recorded fact,
/// off by default (the fail-closed direction every other suitability-style
/// field in this config file takes — see `ProviderConfig::cost_of`'s own
/// doc for the same default direction on `free_models`).
#[test]
fn line_1477_structured_output_suitability_is_recorded() {
    let mut record = ModelCapabilityRecord::new(CapabilityProvenance::User);
    assert!(
        !record.structured_output_suitable(),
        "the default must be conservative — nothing has verified this yet"
    );
    record.set_structured_output_suitable(true);
    assert!(record.structured_output_suitable());
}

/// **Line 1478.** A model recorded as support-only never claims more than
/// `Leaf` — the tier support work (classification, extraction, reranking,
/// formatting) already occupies — even when its own stated ceiling claims
/// higher. A model recorded as core-engineering is uncapped.
#[test]
fn line_1478_support_only_task_suitability_caps_the_ceiling_at_leaf() {
    assert_eq!(
        TaskSuitability::SupportOnly.cap(Some(WorkloadTier::Frontier)),
        Some(WorkloadTier::Leaf),
        "a support-only model must never claim more than Leaf, however high its stated ceiling"
    );
    assert_eq!(
        TaskSuitability::SupportOnly.cap(Some(WorkloadTier::Deterministic)),
        Some(WorkloadTier::Deterministic),
        "capping is a ceiling, not a floor — a lower stated ceiling is left alone"
    );
    assert_eq!(
        TaskSuitability::SupportOnly.cap(None),
        None,
        "support-only with no stated ceiling stays unknown — it must never manufacture a Leaf \
         ceiling from nothing"
    );
    assert_eq!(
        TaskSuitability::CoreEngineering.cap(Some(WorkloadTier::Frontier)),
        Some(WorkloadTier::Frontier),
        "core-engineering applies no cap at all"
    );
}

/// **Line 1483.** The harness-model pairing class and the evidence strength
/// are stored alongside the rest of the calibration, not off in a separate
/// table a reader would have to cross-reference.
#[test]
fn line_1483_pairing_class_and_evidence_strength_are_recorded_together() {
    let mut record = ModelCapabilityRecord::new(CapabilityProvenance::User);
    assert_eq!(
        record.evidence_strength(),
        glasshouse::config::capability::EvidenceStrength::Asserted,
        "a record nobody has run yet is still `Asserted`, not silently `Observed`"
    );
    record.set_pairing_class(Some(PairingClass::ProtocolCompatible));
    record.set_evidence_strength(glasshouse::config::capability::EvidenceStrength::Observed);

    assert_eq!(
        record.pairing_class(),
        Some(PairingClass::ProtocolCompatible)
    );
    assert_eq!(
        record.evidence_strength(),
        glasshouse::config::capability::EvidenceStrength::Observed
    );
}

// --- 1479: precedence -------------------------------------------------------

/// **Line 1479.** `providers.<p>.model_ceilings`'s own override always beats
/// a capability record's initial ceiling — even one that claims a *higher*
/// tier, and even though nothing else in this package changed
/// `model_ceilings`'s own meaning.
#[test]
fn line_1479_the_override_beats_the_seeded_ceiling() {
    let mut record = ModelCapabilityRecord::new(CapabilityProvenance::User);
    record.set_ceiling(Some(WorkloadTier::Frontier));

    let mut provider = ProviderConfig::new("openrouter");
    provider.set_model_capabilities(std::collections::BTreeMap::from([("m".to_owned(), record)]));
    provider.set_model_ceilings(std::collections::BTreeMap::from([(
        "m".to_owned(),
        glasshouse::config::ConfiguredWorkloadTier::new(WorkloadTier::Leaf),
    )]));

    let resolution = provider.resolved_ceiling("m");
    assert_eq!(
        resolution,
        CeilingResolution::UserOverride(WorkloadTier::Leaf)
    );
    assert_eq!(
        resolution.hard_ceiling(),
        Some(WorkloadTier::Leaf),
        "the override (`leaf`) must win over the capability record's higher seed (`frontier`)"
    );
}

/// **Line 1479's other step.** With no override at all, a user-provenance
/// capability record's own ceiling is what resolves — the step the
/// precedence chain falls to next, not a step that gets skipped.
#[test]
fn line_1479_falls_to_the_capability_record_when_no_override_exists() {
    let mut record = ModelCapabilityRecord::new(CapabilityProvenance::User);
    record.set_ceiling(Some(WorkloadTier::Heavy));

    let mut provider = ProviderConfig::new("openrouter");
    provider.set_model_capabilities(std::collections::BTreeMap::from([("m".to_owned(), record)]));

    assert_eq!(
        provider.resolved_ceiling("m"),
        CeilingResolution::UserCapabilityRecord(WorkloadTier::Heavy)
    );
}

/// **Line 1479's floor.** With neither an override nor a capability record,
/// the ceiling is unknown — "nobody has said" is not "cannot", the same rule
/// `ProviderConfig::ceiling_of` already documents.
#[test]
fn line_1479_unknown_when_neither_is_configured() {
    let provider = ProviderConfig::new("openrouter");
    assert_eq!(provider.resolved_ceiling("m"), CeilingResolution::Unknown);
    assert_eq!(provider.resolved_ceiling("m").hard_ceiling(), None);
}

// --- 1482: isolation by harness, launch profile, and protocol --------------

/// **Line 1482.** A record scoped to one harness does not apply to a query
/// naming a different one, or to a query that states no harness at all —
/// isolation is enforced the moment the record narrows, not only when the
/// query happens to agree.
#[test]
fn line_1482_a_harness_scoped_record_does_not_leak_to_another_harness() {
    let mut record = ModelCapabilityRecord::new(CapabilityProvenance::User);
    record.set_harness(Some(IntegrationId::ClaudeCode));

    let same_harness = CapabilityQuery {
        harness: Some(IntegrationId::ClaudeCode),
        ..Default::default()
    };
    let different_harness = CapabilityQuery {
        harness: Some(IntegrationId::Codex),
        ..Default::default()
    };
    let unstated_harness = CapabilityQuery::default();

    assert!(record.applies_to(&same_harness));
    assert!(
        !record.applies_to(&different_harness),
        "a record calibrated for Claude Code must not apply to Codex"
    );
    assert!(
        !record.applies_to(&unstated_harness),
        "a query that does not know the harness must not silently inherit a harness-scoped \
         record — the isolation line 1482 asks for applies even to an incomplete query"
    );
}

/// **Line 1482, the live gate's own path.** `ProviderConfig::resolved_ceiling`
/// — the function `EffectiveConfig::model_ceiling` calls, and therefore the
/// shipped binary's own ceiling resolution — has no harness in hand: it
/// knows only a provider and a model. A harness-scoped record must be
/// **inert** there, not silently treated as general, or a calibration meant
/// for one harness would cap every other harness's launches through the
/// same provider and model — the live scoping leak this line exists to
/// close, not merely a property of `applies_to` nobody calls yet.
#[test]
fn line_1482_a_harness_scoped_record_is_inert_to_context_blind_resolution() {
    let mut scoped = ModelCapabilityRecord::new(CapabilityProvenance::User);
    scoped.set_harness(Some(IntegrationId::ClaudeCode));
    scoped.set_ceiling(Some(WorkloadTier::Leaf));

    let mut provider = ProviderConfig::new("openrouter");
    provider.set_model_capabilities(std::collections::BTreeMap::from([("m".to_owned(), scoped)]));

    assert_eq!(
        provider.resolved_ceiling("m"),
        CeilingResolution::Unknown,
        "a harness-scoped record must not be consumed by the context-blind path at all — \
         reading its ceiling anyway would leak a Claude-Code-only calibration onto every other \
         harness that shares this provider and model"
    );

    // The control: the identical record with no narrowing axis IS reachable
    // from the same context-blind path — proving the inertness above is
    // caused by the harness scope, not by some other difference.
    let mut unscoped = ModelCapabilityRecord::new(CapabilityProvenance::User);
    unscoped.set_ceiling(Some(WorkloadTier::Leaf));
    let mut control = ProviderConfig::new("openrouter");
    control.set_model_capabilities(std::collections::BTreeMap::from([(
        "m".to_owned(),
        unscoped,
    )]));
    assert_eq!(
        control.resolved_ceiling("m"),
        CeilingResolution::UserCapabilityRecord(WorkloadTier::Leaf),
        "the control must still resolve normally, or the assertion above proves nothing about \
         scoping specifically"
    );
}

/// **Line 1482's fallback.** A record that states no harness, profile, or
/// protocol at all applies generally — narrowing is opt-in, not the default,
/// or every existing `model_ceilings`-only project would need to start
/// naming a harness just to keep working.
#[test]
fn line_1482_an_unscoped_record_applies_to_any_query() {
    let record = ModelCapabilityRecord::new(CapabilityProvenance::User);
    assert!(record.applies_to(&CapabilityQuery::default()));
    assert!(record.applies_to(&CapabilityQuery {
        harness: Some(IntegrationId::Cursor),
        launch_profile: Some("anything"),
        protocol: Some(glasshouse::harness::WireProtocol::OpenAiChat),
    }));
}

/// **Line 1482, the launch-profile axis.** The same isolation applies to a
/// record scoped to one named launch profile.
#[test]
fn line_1482_a_profile_scoped_record_does_not_leak_to_another_profile() {
    let mut record = ModelCapabilityRecord::new(CapabilityProvenance::User);
    record.set_launch_profile(Some("careful".to_owned()));

    assert!(record.applies_to(&CapabilityQuery {
        launch_profile: Some("careful"),
        ..Default::default()
    }));
    assert!(!record.applies_to(&CapabilityQuery {
        launch_profile: Some("reckless"),
        ..Default::default()
    }));
}

// --- 1484: a benchmark seed ranks, and never refuses ------------------------

/// **Line 1484.** A benchmark-provenance record's ceiling is a prior for
/// ranking, and `hard_ceiling` never returns it — the property that keeps a
/// seed table from refusing a model the user never restricted.
#[test]
fn line_1484_a_benchmark_provenance_ceiling_never_becomes_a_hard_constraint() {
    let mut record = ModelCapabilityRecord::new(CapabilityProvenance::Benchmark);
    record.set_ceiling(Some(WorkloadTier::Leaf));

    let mut provider = ProviderConfig::new("openrouter");
    provider.set_model_capabilities(std::collections::BTreeMap::from([("m".to_owned(), record)]));

    let resolution = provider.resolved_ceiling("m");
    assert_eq!(
        resolution,
        CeilingResolution::Prior(Some(WorkloadTier::Leaf))
    );
    assert!(resolution.rested_on_prior());
    assert_eq!(
        resolution.hard_ceiling(),
        None,
        "a benchmark seed must never be able to refuse a candidate the user never restricted"
    );
    let explanation = resolution.explain();
    assert!(
        explanation.contains("not proof"),
        "the rendered explanation must say a prior is not proof of performance: {explanation}"
    );
}

/// **Line 1484's counterpart.** The identical ceiling, stated with
/// `provenance = "user"` instead, *does* reach a hard constraint — proving
/// the distinction is provenance, not the number.
#[test]
fn line_1484_the_same_ceiling_with_user_provenance_does_reach_a_hard_constraint() {
    let mut record = ModelCapabilityRecord::new(CapabilityProvenance::User);
    record.set_ceiling(Some(WorkloadTier::Leaf));

    let mut provider = ProviderConfig::new("openrouter");
    provider.set_model_capabilities(std::collections::BTreeMap::from([("m".to_owned(), record)]));

    assert_eq!(
        provider.resolved_ceiling("m").hard_ceiling(),
        Some(WorkloadTier::Leaf)
    );
}

// --- 1485: a local quantized model is a different backend ------------------

/// **Line 1485.** Two `[providers.<name>]` entries — standing for a hosted
/// backend and a local quantized backend — each holding a capability record
/// for a model of the *same name* resolve independently: the `backend` axis
/// line 1482 asks for is already the provider entry the record lives under,
/// with no extra plumbing needed to keep the two apart.
#[test]
fn line_1485_two_providers_with_the_same_model_name_resolve_independently() {
    let mut hosted_record = ModelCapabilityRecord::new(CapabilityProvenance::User);
    hosted_record.set_ceiling(Some(WorkloadTier::Frontier));
    let mut hosted = ProviderConfig::new("anthropic-compatible");
    hosted.set_model_capabilities(std::collections::BTreeMap::from([(
        "family-70b".to_owned(),
        hosted_record,
    )]));

    let mut local_record = ModelCapabilityRecord::new(CapabilityProvenance::User);
    local_record.set_ceiling(Some(WorkloadTier::Leaf));
    let mut local = ProviderConfig::new("openai-compatible");
    local.set_model_capabilities(std::collections::BTreeMap::from([(
        "family-70b".to_owned(),
        local_record,
    )]));

    let hosted_ceiling = hosted.resolved_ceiling("family-70b").hard_ceiling();
    let local_ceiling = local.resolved_ceiling("family-70b").hard_ceiling();

    assert_eq!(hosted_ceiling, Some(WorkloadTier::Frontier));
    assert_eq!(local_ceiling, Some(WorkloadTier::Leaf));
    assert_ne!(
        hosted_ceiling, local_ceiling,
        "the hosted and the local-quantized entries must not share a resolved capability just \
         because the model name is nominally the same"
    );
}

// --- Production-caller assertion: the shipped binary's routing gate --------

/// The provider credential variable — a name only, matching
/// `tests/tier_ceiling.rs`'s own fixture convention.
const CREDENTIAL_VAR: &str = "GLASSHOUSE_MODEL_CAPABILITY_TEST_KEY";

/// The same standard-tier repository task `tests/tier_ceiling.rs` uses, kept
/// identical on purpose: this file's binary test is the same experiment,
/// sourced from `model_capabilities` instead of `model_ceilings`, and a
/// mutation to the classifier should fail both files identically.
const STANDARD_REPO_TASK: &str = "refactor the launch profile handling in this project";

struct Fixture {
    _tmp: tempfile::TempDir,
    base: PathBuf,
    root: PathBuf,
}

impl Fixture {
    fn new(harnesses: &[&str], extra: &str) -> Self {
        let tmp = tempfile::tempdir().expect("tempdir");
        let base = tmp.path().to_path_buf();
        let root = base.join("workspace");
        std::fs::create_dir_all(root.join(".git")).expect("create project root");
        let root = std::fs::canonicalize(&root).expect("canonicalize project root");

        let bin_dir = base.join("bin");
        std::fs::create_dir_all(&bin_dir).expect("create bin dir");

        let mut config = String::from("version = 1\n\n");
        for harness in harnesses {
            let exe = install_fake_harness(&bin_dir, harness);
            let escaped = exe.display().to_string().replace('\\', "\\\\");
            config.push_str(&format!(
                "[integrations.{harness}]\nenabled = true\nexecutable = \"{escaped}\"\n\n"
            ));
        }
        config.push_str(extra);

        let config_dir = base.join("config");
        std::fs::create_dir_all(&config_dir).expect("create config dir");
        std::fs::write(config_dir.join("config.toml"), config).expect("write user config");

        Self {
            _tmp: tmp,
            base,
            root,
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
            .env(CREDENTIAL_VAR, "planted-opaque-model-capability-test-value")
            .env("PATH", self.base.join("empty-path"))
            .output()
            .expect("the glasshouse binary must be runnable")
    }

    fn route(&self, args: &[&str]) -> String {
        let output = self.glasshouse(args);
        assert!(
            output.status.success(),
            "`glasshouse {}` failed:\n{}{}",
            args.join(" "),
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );
        String::from_utf8_lossy(&output.stdout).into_owned()
    }
}

#[cfg(unix)]
fn install_fake_harness(bin_dir: &Path, harness: &str) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;

    let path = bin_dir.join(format!("fake-{harness}"));
    std::fs::write(&path, "#!/bin/sh\nexit 0\n").expect("write fake harness");
    let mut perms = std::fs::metadata(&path).unwrap().permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&path, perms).unwrap();
    path
}

#[cfg(windows)]
fn install_fake_harness(bin_dir: &Path, harness: &str) -> PathBuf {
    let path = bin_dir.join(format!("fake-{harness}.cmd"));
    std::fs::write(&path, "@echo off\r\nexit /b 0\r\n").expect("write fake harness");
    path
}

/// **The production-caller assertion.** With *no* `model_ceilings` table at
/// all — only a `model_capabilities` record, `provenance = "user"` — the
/// shipped binary still refuses the capped destination for a task above its
/// capability-record ceiling, exactly as `tests/tier_ceiling.rs` proves for
/// an override. The only path from this TOML table to that refusal is
/// `ProviderConfig::resolved_ceiling` -> `EffectiveConfig::model_ceiling` ->
/// `main.rs::destination_tier_ceiling` -> `Destination::with_tier_ceiling` ->
/// `session::hard_constraint` — every link production code, none of it
/// touched by this test.
#[test]
fn a_configured_capability_record_excludes_a_destination_below_the_required_tier_on_the_shipped_binary()
 {
    let fixture = Fixture::new(
        &["claude-code"],
        &format!(
            "[providers.alpha]\ntemplate = \"openrouter\"\n\
             credential_env = [\"{CREDENTIAL_VAR}\"]\n\n\
             [providers.alpha.model_capabilities.small]\n\
             ceiling = \"leaf\"\n\
             provenance = \"user\"\n\n\
             [profiles.capped]\nharness = \"claude-code\"\nmodel = \"small\"\n\
             expected_protocol = \"openai-chat\"\n\
             [profiles.capped.backend]\nkind = \"direct-provider\"\nprovider = \"alpha\"\n\n\
             [profiles.uncapped]\nharness = \"claude-code\"\nmodel = \"big\"\n\
             expected_protocol = \"openai-chat\"\n\
             [profiles.uncapped.backend]\nkind = \"direct-provider\"\nprovider = \"alpha\"\n"
        ),
    );
    let report = fixture.route(&["route", "--task", STANDARD_REPO_TASK]);

    let rejected = report
        .split_once("\nrejected\n")
        .unwrap_or_else(|| panic!("nothing was rejected at all:\n{report}"))
        .1;
    assert!(
        rejected.contains("fresh:claude-code:capped"),
        "the destination whose only established ceiling comes from a capability record must be \
         refused, not merely scored low:\n{report}"
    );
    assert!(
        rejected.contains("hard workload tier constraint"),
        "the refusal must name the workload-tier constraint:\n{report}"
    );
    assert!(
        !rejected.contains("fresh:claude-code:uncapped"),
        "the destination with no capability record at all must still be eligible — \"nobody has \
         said\" is not \"cannot\":\n{report}"
    );
}
