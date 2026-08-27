//! Phase 9J's pairing *prior*, lines 566–576 and Phase 49 line 1797.
//!
//! `docs/product/evidence/phase-9j.md` recorded, before this package, that
//! there were exactly two routing callers in the shipped binary and neither
//! ranked candidates. That is now half true: `DisposableRouting` still
//! cannot reach this function at all (`DisposableCandidate` carries no
//! harness — see that phase's own entry for why), but
//! `InteractiveRouting::on_provider_failure` now calls it for real, through
//! `routing::interactive::score_candidate`, to rank same-model failover
//! survivors — see this crate's own `routing::interactive` module tests for
//! that production path. What is still not wired through that caller is the
//! user's *configured* preference and pairing corrections (it scores every
//! candidate against `PairingPreference::Strong` and no corrections — see
//! `score_candidate`'s own doc comment for exactly why), so every test below
//! still enters through this policy function directly to exercise the full
//! range this one production caller does not yet reach.
//!
//! The `native_pairing_preference` tests are the one part of this package
//! with a real, if unwired, path: they load actual TOML through
//! `UserConfig::load` / `ProjectConfig`, the same as `tests/pairing.rs`.

use clap::Parser;

use glasshouse::config::pairing::{
    self, NoObservations, ObservationSource, ObservedEvidence, PairingPreference,
};
use glasshouse::config::{self, EffectiveConfig, ProjectConfig, UserConfig};
use glasshouse::harness::pairing::{EvidenceKey, PairingClass, PairingQuery, ServingRoute};
use glasshouse::harness::{Declared, WireProtocol};
use glasshouse::integrations::IntegrationId;
use glasshouse::routing::{
    AssignedModel, EligibleCandidate, HardConstraint, apply_hard_constraints,
};
use glasshouse::{Cli, Runtime, bootstrap};

fn query(harness: IntegrationId, model: &str) -> PairingQuery {
    PairingQuery {
        harness,
        model: AssignedModel::named(model),
        route: ServingRoute::default(),
        tool_calls: Declared::Unverified,
        provider_protocols: Vec::new(),
    }
}

fn no_overrides() -> glasshouse::harness::pairing::PairingOverrides {
    glasshouse::harness::pairing::PairingOverrides::default()
}

/// Wrap a classified pairing as the only public API allows: through the hard
/// constraint filter, with a check that always passes. This is the same
/// function a real candidate-scoring caller would have to call — there is no
/// other way to obtain an `EligibleCandidate`.
fn eligible(
    pairing: glasshouse::harness::pairing::Pairing,
) -> EligibleCandidate<glasshouse::harness::pairing::Pairing> {
    let (mut ok, rejected) = apply_hard_constraints(vec![pairing], |_| Ok(()));
    assert!(rejected.is_empty());
    ok.pop().expect("the one candidate passed the check")
}

fn key_for(harness: IntegrationId, model: &str) -> EvidenceKey {
    EvidenceKey::new(
        harness,
        "default",
        AssignedModel::named(model),
        ServingRoute::default(),
    )
}

struct FixedObservations(Vec<(EvidenceKey, ObservedEvidence)>);

impl ObservationSource for FixedObservations {
    fn observed(&self, key: &EvidenceKey) -> Option<ObservedEvidence> {
        self.0
            .iter()
            .find(|(candidate, _)| candidate == key)
            .map(|(_, observed)| *observed)
    }
}

fn good_observations(count: usize) -> ObservedEvidence {
    let mut observed = ObservedEvidence::none();
    observed.reliable_observation_count = count;
    observed.task_success_rate = Some(0.98);
    observed.usable_tool_call_rate = Some(0.97);
    observed.repair_rate = Some(0.02);
    observed.reliability = Some(0.99);
    observed
}

fn bad_observations(count: usize) -> ObservedEvidence {
    let mut observed = ObservedEvidence::none();
    observed.reliable_observation_count = count;
    observed.task_success_rate = Some(0.05);
    observed.usable_tool_call_rate = Some(0.1);
    observed.repair_rate = Some(0.9);
    observed.reliability = Some(0.08);
    observed
}

/// Design decision 1, the load-bearing property: the prior never excludes a
/// candidate. `Off` zeroes its magnitude, but the candidate still produces a
/// full explanation and a finite, comparable total — there is no code path
/// where any preference value makes a candidate simply disappear from
/// scoring.
#[test]
fn the_prior_is_never_a_filter_even_when_the_preference_is_off() {
    let pairing = glasshouse::harness::pairing::classify(
        &query(IntegrationId::ClaudeCode, "claude-fable-5"),
        &no_overrides(),
    );
    assert_eq!(pairing.class(), PairingClass::VendorNative);
    let key = key_for(IntegrationId::ClaudeCode, "claude-fable-5");

    let explanation = pairing::native_pairing_prior_contribution(
        &eligible(pairing),
        &key,
        PairingPreference::Off,
        &NoObservations,
    );

    assert!(!explanation.contributions().is_empty());
    assert!(explanation.total().is_finite());
    let prior = explanation
        .contributions()
        .iter()
        .find(|c| c.name() == "native-pairing prior")
        .expect("the prior contribution is present even when it is worth nothing");
    assert_eq!(prior.magnitude(), 0.0);
}

/// Design decision 4: the prior decays to exactly zero as reliable
/// observations accumulate, through the public entry point rather than the
/// internal decay function alone.
#[test]
fn the_prior_contribution_decays_to_zero_as_observations_accumulate() {
    let pairing = || {
        glasshouse::harness::pairing::classify(
            &query(IntegrationId::ClaudeCode, "claude-fable-5"),
            &no_overrides(),
        )
    };
    let key = key_for(IntegrationId::ClaudeCode, "claude-fable-5");

    let fresh = pairing::native_pairing_prior_contribution(
        &eligible(pairing()),
        &key,
        PairingPreference::Strong,
        &NoObservations,
    );
    let fresh_prior = fresh
        .contributions()
        .iter()
        .find(|c| c.name() == "native-pairing prior")
        .unwrap()
        .magnitude();
    assert!(fresh_prior > 0.0, "a fresh session gets a positive prior");

    let mut none = ObservedEvidence::none();
    none.reliable_observation_count = 200;
    let seasoned = FixedObservations(vec![(key.clone(), none)]);
    let decayed = pairing::native_pairing_prior_contribution(
        &eligible(pairing()),
        &key,
        PairingPreference::Strong,
        &seasoned,
    );
    let decayed_prior = decayed
        .contributions()
        .iter()
        .find(|c| c.name() == "native-pairing prior")
        .unwrap()
        .magnitude();
    assert_eq!(
        decayed_prior, 0.0,
        "at sufficient evidence the prior must contribute nothing measurable, not a floor"
    );
}

/// Design decision 5 / line 573: a cross-vendor pairing with good
/// observations must outrank a native pairing with bad ones, and it must do
/// so decisively rather than by a hair — this is a negative requirement, and
/// the packet asks for a mutation that would pass by accident to be named.
/// The mutation this test kills: deleting the `local observed evidence`
/// contribution entirely, which would leave both totals equal to their
/// (both-fresh) priors and this assertion failing.
#[test]
fn a_cross_vendor_pairing_with_good_evidence_outranks_a_native_pairing_with_bad_evidence() {
    let native = glasshouse::harness::pairing::classify(
        &query(IntegrationId::ClaudeCode, "claude-fable-5"),
        &no_overrides(),
    );
    assert_eq!(native.class(), PairingClass::VendorNative);
    let native_key = key_for(IntegrationId::ClaudeCode, "claude-fable-5");

    let mut cross_vendor_query = query(IntegrationId::ClaudeCode, "unlisted-model-v1");
    cross_vendor_query.route.protocol = Some(WireProtocol::AnthropicMessages);
    let cross_vendor = glasshouse::harness::pairing::classify(&cross_vendor_query, &no_overrides());
    assert_ne!(cross_vendor.class(), PairingClass::VendorNative);
    let cross_vendor_key = key_for(IntegrationId::ClaudeCode, "unlisted-model-v1");

    // 5, not 20: `FULL_DECAY_OBSERVATIONS` is 20, so 20 would leave the
    // native prior already fully decayed to zero on its own — a weaker test
    // that would pass even if the evidence-signal contribution were deleted
    // entirely. 5 is high enough for full evidence confidence
    // (`CONFIDENT_AT_OBSERVATIONS` is 5) while the prior is still worth 0.75
    // of its base magnitude, so this actually exercises evidence outranking
    // a *live* prior rather than a decayed one.
    let evidence = FixedObservations(vec![
        (native_key.clone(), bad_observations(5)),
        (cross_vendor_key.clone(), good_observations(5)),
    ]);

    let native_explanation = pairing::native_pairing_prior_contribution(
        &eligible(native),
        &native_key,
        PairingPreference::Strong,
        &evidence,
    );
    let cross_vendor_explanation = pairing::native_pairing_prior_contribution(
        &eligible(cross_vendor),
        &cross_vendor_key,
        PairingPreference::Strong,
        &evidence,
    );

    assert!(
        cross_vendor_explanation.total() > native_explanation.total(),
        "cross-vendor total {} did not outrank native total {}",
        cross_vendor_explanation.total(),
        native_explanation.total()
    );
}

/// Design decision 5 / line 574: a native pairing whose own project evidence
/// contradicts the prior must lose to a neutral candidate with no evidence at
/// all — not merely score lower than some other pairing, but actually fall
/// below a candidate the prior alone would have ranked beneath it.
#[test]
fn a_native_pairing_contradicted_by_evidence_loses_to_a_neutral_candidate() {
    let native = glasshouse::harness::pairing::classify(
        &query(IntegrationId::ClaudeCode, "claude-fable-5"),
        &no_overrides(),
    );
    let native_key = key_for(IntegrationId::ClaudeCode, "claude-fable-5");

    let mut neutral_query = query(IntegrationId::ClaudeCode, "unlisted-model-v1");
    neutral_query.route.protocol = Some(WireProtocol::AnthropicMessages);
    let neutral = glasshouse::harness::pairing::classify(&neutral_query, &no_overrides());
    let neutral_key = key_for(IntegrationId::ClaudeCode, "unlisted-model-v1");

    // 5 observations leave the native prior at 0.75 of its base magnitude
    // (see the comment in the cross-vendor test above) — a real positive
    // prior for the contradicting evidence to overcome, not one already
    // decayed away.
    let evidence = FixedObservations(vec![(native_key.clone(), bad_observations(5))]);

    let native_explanation = pairing::native_pairing_prior_contribution(
        &eligible(native),
        &native_key,
        PairingPreference::Strong,
        &evidence,
    );
    let neutral_explanation = pairing::native_pairing_prior_contribution(
        &eligible(neutral),
        &neutral_key,
        PairingPreference::Strong,
        &evidence,
    );

    assert_eq!(neutral_explanation.total(), 0.0);
    assert!(
        native_explanation.total() < neutral_explanation.total(),
        "contradicted native total {} did not fall below the neutral candidate's {}",
        native_explanation.total(),
        neutral_explanation.total()
    );
}

/// Design decision 7: `Pin` never reaches the additive scorer. This is
/// checked at the type level (`PairingPreference::strength` returns `None`
/// for `Pin`, and the scoring function takes the resulting `PriorStrength`,
/// not `PairingPreference`, so a pin cannot be passed in even by mistake) —
/// this test checks the one remaining runtime behaviour, which is what the
/// explanation says about a pinned session instead of scoring it.
#[test]
fn a_pinned_preference_is_explained_as_a_hard_rule_not_a_score() {
    let pairing = glasshouse::harness::pairing::classify(
        &query(IntegrationId::ClaudeCode, "claude-fable-5"),
        &no_overrides(),
    );
    let key = key_for(IntegrationId::ClaudeCode, "claude-fable-5");

    let explanation = pairing::native_pairing_prior_contribution(
        &eligible(pairing),
        &key,
        PairingPreference::Pin,
        &NoObservations,
    );

    assert!(
        explanation
            .contributions()
            .iter()
            .any(|c| c.name() == "native-pairing preference" && c.magnitude() == 0.0),
        "a pin must be named as a hard rule rather than scored: {:?}",
        explanation
    );
    assert!(
        !explanation
            .contributions()
            .iter()
            .any(|c| c.name() == "native-pairing prior"),
        "a pin must never produce a scored prior contribution"
    );
}

/// Design decision 2, structurally: a candidate that fails a hard constraint
/// never reaches the scorer at all, because `apply_hard_constraints` is the
/// only way to produce the type the scorer accepts.
#[test]
fn a_candidate_that_fails_a_hard_constraint_cannot_be_scored() {
    let incompatible = glasshouse::harness::pairing::classify(
        &{
            let mut q = query(IntegrationId::Codex, "claude-fable-5");
            q.route.protocol = Some(WireProtocol::AnthropicMessages);
            q.provider_protocols = vec![WireProtocol::AnthropicMessages];
            q
        },
        &no_overrides(),
    );
    assert_eq!(
        incompatible.protocol_fit(),
        glasshouse::harness::pairing::ProtocolFit::Incompatible
    );

    let (eligible, rejected) = apply_hard_constraints(vec![incompatible], |pairing| {
        if pairing.protocol_fit() == glasshouse::harness::pairing::ProtocolFit::Incompatible {
            Err(HardConstraint::Protocol)
        } else {
            Ok(())
        }
    });

    assert!(
        eligible.is_empty(),
        "an incompatible candidate must not become eligible"
    );
    assert_eq!(rejected.len(), 1);
    assert_eq!(rejected[0].1, HardConstraint::Protocol);
}

// --- The configuration half: real TOML, loaded the way the binary would. ---

fn new_workspace() -> tempfile::TempDir {
    let workspace = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(workspace.path().join(".git")).unwrap();
    workspace
}

fn runtime_for(workspace: &std::path::Path, data: &std::path::Path) -> Runtime {
    let cli = Cli::try_parse_from([
        "glasshouse",
        "--data-dir",
        data.to_str().unwrap(),
        "--config-dir",
        data.to_str().unwrap(),
    ])
    .unwrap();
    bootstrap(&cli, workspace).unwrap()
}

fn preference_for(user_toml: &str, project_toml: Option<&str>) -> (PairingPreference, String) {
    let workspace = new_workspace();
    let data = tempfile::tempdir().unwrap();
    std::fs::write(data.path().join("config.toml"), user_toml).unwrap();
    if let Some(project_toml) = project_toml {
        let dir = workspace.path().join(".glasshouse");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("config.toml"), project_toml).unwrap();
    }

    let runtime = runtime_for(workspace.path(), data.path());
    let user = UserConfig::load(runtime.paths()).unwrap();
    let project: Option<ProjectConfig> = config::load_project_config(runtime.project()).unwrap();
    let effective = EffectiveConfig::new(&user, project.as_ref());
    effective.native_pairing_preference()
}

/// Nothing configured: the default is `Strong`, not `Off` — line 566's
/// out-of-the-box behaviour.
#[test]
fn an_unconfigured_preference_defaults_to_strong() {
    let (preference, source) = preference_for("version = 1\n", None);
    assert_eq!(preference, PairingPreference::Strong);
    assert!(source.contains("default"));
}

/// Phase 49 line 1797: a person can set the preference in configuration,
/// with no vendor name anywhere in the setting.
#[test]
fn a_user_can_configure_each_of_the_four_preference_values() {
    for slug in ["strong", "weak", "off", "pin"] {
        let toml = format!("version = 1\n\n[pairing]\nnative_pairing_preference = \"{slug}\"\n");
        let (preference, source) = preference_for(&toml, None);
        assert_eq!(preference.slug(), slug);
        assert!(source.contains("user"));
    }
}

/// The project layer wins over the user layer, matching every other
/// `EffectiveConfig` lookup except `bypass_acknowledged`.
#[test]
fn a_project_preference_overrides_the_user_preference() {
    let user_toml = "version = 1\n\n[pairing]\nnative_pairing_preference = \"weak\"\n";
    let project_toml = "version = 1\n\n[pairing]\nnative_pairing_preference = \"off\"\n";
    let (preference, source) = preference_for(user_toml, Some(project_toml));
    assert_eq!(preference, PairingPreference::Off);
    assert!(source.contains("project"));
}

/// A spelling this build does not recognise degrades visibly rather than
/// refusing to load — the same rule `ModelBehaviourFit` and every other
/// pairing correction field already follows.
#[test]
fn an_unrecognised_preference_spelling_falls_back_to_the_default() {
    let toml = "version = 1\n\n[pairing]\nnative_pairing_preference = \"aggressive\"\n";
    let (preference, source) = preference_for(toml, None);
    assert_eq!(preference, PairingPreference::Strong);
    assert!(source.contains("default"));
}

/// Phase 49 line 1797, through the actual production caller: `report` is the
/// one function `main.rs`'s `pairing` arm calls, and it now prints the
/// configured preference — the one part of this package with a real, if
/// currently unread, caller in the shipped binary. Entering through `report`
/// rather than `EffectiveConfig::native_pairing_preference` directly is §35:
/// a test that only called the accessor would still pass against a build
/// where `report` never rendered it at all.
#[test]
fn the_report_the_binary_prints_names_the_configured_preference() {
    let workspace = new_workspace();
    let data = tempfile::tempdir().unwrap();
    std::fs::write(
        data.path().join("config.toml"),
        "version = 1\n\n[pairing]\nnative_pairing_preference = \"weak\"\n",
    )
    .unwrap();
    let runtime = runtime_for(workspace.path(), data.path());
    let user = UserConfig::load(runtime.paths()).unwrap();
    let project: Option<ProjectConfig> = config::load_project_config(runtime.project()).unwrap();
    let effective = EffectiveConfig::new(&user, project.as_ref());

    let report = config::pairing::report(&effective, None, None);
    assert!(
        report.contains("Native-pairing preference: weak (from the user configuration file)"),
        "the report the binary prints did not name the configured preference:\n{report}"
    );
}

/// A spelling this build cannot use is ignored **and said so** — the review
/// finding that this file's first version reported as "nothing configured".
///
/// Every other field in `config::pairing` degrades visibly: a bad `behaviour`
/// prints back as `behaviour=nonsense`. The preference line claimed the user's
/// file was empty while the misspelling sat in it, which is the one failure a
/// person debugging their own configuration cannot recover from — they are
/// told to add the thing they already added.
#[test]
fn an_unusable_preference_spelling_is_reported_rather_than_silently_defaulted() {
    let workspace = new_workspace();
    let data = tempfile::tempdir().unwrap();
    std::fs::write(
        data.path().join("config.toml"),
        "version = 1\n\n[pairing]\nnative_pairing_preference = \"week\"\n",
    )
    .unwrap();
    let runtime = runtime_for(workspace.path(), data.path());
    let user = UserConfig::load(runtime.paths()).unwrap();
    let project: Option<ProjectConfig> = config::load_project_config(runtime.project()).unwrap();
    let effective = EffectiveConfig::new(&user, project.as_ref());

    let report = config::pairing::report(&effective, None, None);

    // It still falls back, because ignoring is the settled rule ...
    assert!(
        report.contains("Native-pairing preference: strong"),
        "an unusable spelling should fall back to the default:\n{report}"
    );
    // ... but the user's own spelling has to appear, or the fallback is a lie.
    assert!(
        report.contains("`week`"),
        "the ignored spelling was swallowed instead of reported:\n{report}"
    );
    assert!(
        report.contains("the user configuration file set"),
        "the report did not say which layer the ignored value came from:\n{report}"
    );
    // And the bare "nothing configured" claim must not stand alone, because
    // something *was* configured.
    assert!(
        !report.contains("(from the default — nothing configured)"),
        "the report claimed nothing was configured while a value sat in the file:\n{report}"
    );
}
