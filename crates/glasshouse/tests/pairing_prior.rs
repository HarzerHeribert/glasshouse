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
    // OpenCode (openai-chat) on an anthropic-only route: still incompatible
    // after T2b made openai-chat <-> openai-responses a translated pairing.
    let incompatible = glasshouse::harness::pairing::classify(
        &{
            let mut q = query(IntegrationId::OpenCode, "claude-fable-5");
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

// ---------------------------------------------------------------------------
// Lines 566 and 569, entered through `InteractiveRouting::start` — which is
// the function that reaches `best`, not the prior's own scorer (§35).
//
// Read `docs/product/evidence/phase-9j.md` and this package's report before
// reading these as evidence for the two boxes: `start` has **no production
// caller**, and the native-pairing prior is constant across every candidate
// set the shipped binary can build. What these prove is the policy — that the
// prior and a warm session are commensurable, that either can win, and that
// neither is a short circuit. What they do not prove is that any of it
// changes a decision the binary makes today.
// ---------------------------------------------------------------------------

use glasshouse::config::pairing::{
    ContinuitySource, NoWarmSessions, WarmSession, WarmSessionState,
    session_continuity_contribution,
};
use glasshouse::routing::interactive::{InteractiveRouting, SessionStartInputs};
use glasshouse::routing::{Backend, Cost, CredentialId, ToolSemantics};
use glasshouse::secret::SecretRef;

fn candidate(provider: &str, model: &str) -> Backend {
    Backend::new(
        provider,
        "anthropic-messages",
        AssignedModel::named(model),
        CredentialId::new(
            provider,
            SecretRef::Environment {
                var: format!("{}_API_KEY", provider.to_uppercase()),
            },
        ),
        Cost::Metered,
        ToolSemantics::Unverified,
    )
}

/// A warm session on exactly one provider, matched through the evidence key's
/// own route so that line 572's "the same nominal model served two ways is
/// not one body of evidence" holds for continuity too.
struct WarmOn(&'static str, WarmSession);

impl ContinuitySource for WarmOn {
    fn warm_session(&self, key: &EvidenceKey) -> Option<WarmSession> {
        (key.route().provider.as_deref() == Some(self.0)).then_some(self.1)
    }
}

fn live(idle_seconds: i64) -> WarmSession {
    WarmSession {
        state: WarmSessionState::Live,
        idle_seconds,
    }
}

fn resumable(idle_seconds: i64) -> WarmSession {
    WarmSession {
        state: WarmSessionState::Resumable,
        idle_seconds,
    }
}

fn start_with(
    preference: PairingPreference,
    candidates: &[Backend],
    evidence: &dyn ObservationSource,
    continuity: &dyn ContinuitySource,
) -> glasshouse::routing::interactive::SessionStart {
    InteractiveRouting::new()
        .start(
            "claude-code",
            "default",
            candidates,
            &SessionStartInputs {
                preference,
                overrides: &no_overrides(),
                evidence,
                continuity,
            },
        )
        .expect("a non-empty candidate set produces a start")
}

/// Line 569, the direction that makes the line non-trivial: a warm session
/// beats a native pairing that has nothing but its prior.
///
/// The vendor-native candidate is configured **first**, so order cannot
/// explain the result, and it holds a full-strength `Strong` prior with zero
/// observations — the most favourable case line 566 could ask for.
///
/// This candidate set has two different models for one harness, which is a
/// set the shipped binary cannot build (`Upstream::backends` carries no
/// model). It is the set line 569's sentence describes, and constructing it
/// by hand is the only way to exercise the comparison at all.
#[test]
fn a_live_warm_session_outweighs_a_full_strength_native_pairing_prior() {
    let candidates = [
        candidate("anthropic", "claude-fable-5"),
        candidate("openrouter", "unlisted-model-v1"),
    ];
    let start = start_with(
        PairingPreference::Strong,
        &candidates,
        &NoObservations,
        &WarmOn("openrouter", live(0)),
    );

    assert_eq!(
        start.assignment().provider(),
        "openrouter",
        "a fresh live warm session must be able to outweigh the prior, or line 569 is \
         unreachable by construction:\n{}",
        start.explanation().render()
    );
}

/// The same comparison, losing. Continuity decays, and once it has decayed
/// past the prior the native pairing wins again — which is what stops 569
/// from having quietly turned a soft prior into a warm-session trump card.
#[test]
fn a_stale_warm_session_no_longer_outweighs_the_native_pairing_prior() {
    let candidates = [
        candidate("anthropic", "claude-fable-5"),
        candidate("openrouter", "unlisted-model-v1"),
    ];
    let start = start_with(
        PairingPreference::Strong,
        &candidates,
        &NoObservations,
        // Six hours of the eight-hour relevance window: 1.5 * 0.25 = 0.375,
        // below the undecayed strong prior's 1.0.
        &WarmOn("openrouter", live(6 * 60 * 60)),
    );

    assert_eq!(
        start.assignment().provider(),
        "anthropic",
        "a warm session idle for most of its relevance window must lose to the prior:\n{}",
        start.explanation().render()
    );
}

/// A warm session past the relevance window is worth exactly zero, not a
/// floor — the same property `FULL_DECAY_OBSERVATIONS` gives the prior, and
/// the reason an equality assertion is possible here at all.
#[test]
fn a_warm_session_past_its_relevance_window_contributes_exactly_zero() {
    let key = key_for(IntegrationId::ClaudeCode, "claude-fable-5");
    let contribution =
        session_continuity_contribution(&key, &FixedWarm(vec![(key.clone(), live(9 * 60 * 60))]));
    assert_eq!(contribution.magnitude(), 0.0);
    assert_eq!(contribution.name(), "session continuity");
}

struct FixedWarm(Vec<(EvidenceKey, WarmSession)>);

impl ContinuitySource for FixedWarm {
    fn warm_session(&self, key: &EvidenceKey) -> Option<WarmSession> {
        self.0
            .iter()
            .find(|(candidate, _)| candidate == key)
            .map(|(_, warm)| *warm)
    }
}

/// Continuity is commensurable with the user's own preference, not just with
/// the default: a stopped-but-resumable session is worth less than a full
/// `Strong` prior and more than a `Weak` one. If the two signals were on
/// unrelated scales this test could not be written.
#[test]
fn a_resumable_warm_session_loses_to_a_strong_prior_and_beats_a_weak_one() {
    let candidates = [
        candidate("anthropic", "claude-fable-5"),
        candidate("openrouter", "unlisted-model-v1"),
    ];

    let strong = start_with(
        PairingPreference::Strong,
        &candidates,
        &NoObservations,
        &WarmOn("openrouter", resumable(0)),
    );
    assert_eq!(
        strong.assignment().provider(),
        "anthropic",
        "0.75 of resumable continuity must not beat a 1.0 strong prior:\n{}",
        strong.explanation().render()
    );

    let weak = start_with(
        PairingPreference::Weak,
        &candidates,
        &NoObservations,
        &WarmOn("openrouter", resumable(0)),
    );
    assert_eq!(
        weak.assignment().provider(),
        "openrouter",
        "0.75 of resumable continuity must beat a 0.4 weak prior, or the user's four \
         preference values do not interact with continuity at all:\n{}",
        weak.explanation().render()
    );
}

/// Neither signal is a trump card. Line 571 already says observed evidence
/// may outweigh the prior; this is the same requirement extended to
/// continuity, and it is what keeps line 574 true at this caller — measured
/// evidence still wins over both.
#[test]
fn measured_evidence_outranks_both_the_prior_and_a_fresh_warm_session() {
    let native = candidate("anthropic", "claude-fable-5");
    let warm_cross_vendor = candidate("openrouter", "unlisted-model-v1");
    let measured = candidate("bedrock", "unlisted-model-v1");

    let observations = FixedObservations(vec![(
        EvidenceKey::new(
            IntegrationId::ClaudeCode,
            "default",
            AssignedModel::named("unlisted-model-v1"),
            ServingRoute {
                provider: Some("bedrock".to_owned()),
                gateway: None,
                protocol: Some(WireProtocol::AnthropicMessages),
            },
        ),
        good_observations(5),
    )]);

    let start = start_with(
        PairingPreference::Strong,
        &[native, warm_cross_vendor, measured],
        &observations,
        &WarmOn("openrouter", live(0)),
    );

    assert_eq!(
        start.assignment().provider(),
        "bedrock",
        "a strong measured record must outrank both a full-strength native prior and a fresh \
         live warm session, or the prior and continuity have become rules:\n{}",
        start.explanation().render()
    );
}

/// Nothing fabricates "vendor-native". The prior is zero for a model no
/// declaration covers, and becomes positive only once somebody declares the
/// family — through the same `[pairing.harnesses.<slug>]` correction line 561
/// already ships.
#[test]
fn the_prior_at_session_start_comes_only_from_a_declared_native_family() {
    let candidates = [candidate("openrouter", "unlisted-model-v1")];

    let undeclared = start_with(
        PairingPreference::Strong,
        &candidates,
        &NoObservations,
        &NoWarmSessions,
    );
    let prior_of = |start: &glasshouse::routing::interactive::SessionStart| {
        start
            .explanation()
            .contributions()
            .iter()
            .find(|c| c.name() == "native-pairing prior")
            .expect("the prior is always named")
            .magnitude()
    };
    assert_eq!(
        prior_of(&undeclared),
        0.0,
        "a model nobody declared native must earn no prior"
    );

    let mut harnesses = std::collections::BTreeMap::new();
    harnesses.insert(
        IntegrationId::ClaudeCode.slug().to_owned(),
        glasshouse::harness::pairing::SupportCorrection {
            native_families: Some(vec!["unlisted".to_owned()]),
            supported_models: None,
        },
    );
    let mut models = std::collections::BTreeMap::new();
    models.insert(
        "unlisted-model-v1".to_owned(),
        glasshouse::harness::pairing::ModelCorrection {
            developer: Some(glasshouse::harness::pairing::ModelDeveloper::named(
                "anthropic",
            )),
            family: Some("unlisted".to_owned()),
            behaviour: None,
        },
    );
    let corrected = InteractiveRouting::new()
        .start(
            "claude-code",
            "default",
            &candidates,
            &SessionStartInputs {
                preference: PairingPreference::Strong,
                overrides: &glasshouse::harness::pairing::PairingOverrides::from_parts(
                    "the user configuration file",
                    models,
                    harnesses,
                ),
                evidence: &NoObservations,
                continuity: &NoWarmSessions,
            },
        )
        .expect("one candidate is not none");
    assert!(
        prior_of(&corrected) > 0.0,
        "a declared native family must earn the prior:\n{}",
        corrected.explanation().render()
    );
}

/// Line 575 at this caller, and the one security invariant a routing
/// explanation has: it reaches a diagnostic, so it must name the credential
/// only as `CredentialId` already names it — never a value.
#[test]
fn a_session_start_explanation_names_every_signal_and_no_credential_value() {
    let start = start_with(
        PairingPreference::Strong,
        &[candidate("anthropic", "claude-fable-5")],
        &NoObservations,
        &WarmOn("anthropic", live(60)),
    );
    let rendered = start.explanation().render();

    for expected in [
        "pairing class",
        "local evidence strength",
        "native-pairing prior",
        "session continuity",
    ] {
        assert!(
            rendered.contains(expected),
            "the explanation must name `{expected}`:\n{rendered}"
        );
    }
    assert!(
        rendered.contains("vendor-native"),
        "line 575's first term is the class itself:\n{rendered}"
    );
    assert!(
        !rendered.contains("ANTHROPIC_API_KEY"),
        "an explanation reaches a diagnostic and must not carry a credential reference into \
         it:\n{rendered}"
    );
}
