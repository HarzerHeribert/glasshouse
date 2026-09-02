//! `config::tests`, part B: routing policy, memory/firewall, entitlement and model-facts tests.
//!

use super::*;

/// Phase 2C's whole job is to *record* the choice, so the thing worth
/// proving is that it survives the process that made it. Each of the
/// three answers the wizard offers goes to disk through the real `save`
/// and comes back through the real `load` — a `toml::to_string` in
/// isolation would pass even if `UserConfig`'s `[routing]` table were
/// never wired into the file that is actually written.
#[test]
fn every_routing_model_choice_survives_a_real_save_and_load() {
    fn round_trip(choice: Option<RoutingModelChoice>) -> UserConfig {
        let tmp = tempfile::tempdir().unwrap();
        let paths = RuntimePaths::new(tmp.path().join("data"), tmp.path().join("config"));

        let mut user = fully_populated_user_config();
        user.routing_mut().set_model(choice);
        user.save(&paths).unwrap();

        let loaded = UserConfig::load(&paths).unwrap();
        assert_eq!(
            loaded, user,
            "recording a routing model must not disturb anything else in the file"
        );
        loaded
    }

    assert_eq!(
        round_trip(Some(RoutingModelChoice::Automatic))
            .routing()
            .model(),
        Some(&RoutingModelChoice::Automatic)
    );

    let pinned = RoutingModelChoice::Pinned {
        provider: "openrouter".to_owned(),
        model: "gpt-5.6-luna".to_owned(),
    };
    assert_eq!(
        round_trip(Some(pinned.clone())).routing().model(),
        Some(&pinned)
    );

    assert_eq!(
        round_trip(Some(RoutingModelChoice::Deterministic))
            .routing()
            .model(),
        Some(&RoutingModelChoice::Deterministic)
    );

    // "Do later" must read back as *nothing recorded* rather than as an
    // explicit deterministic choice: the two resolve the same way but
    // say different, accurate things about what the user decided.
    let declined = round_trip(None);
    assert_eq!(declined.routing().model(), None);
    assert_eq!(
        EffectiveConfig::new(&declined, None)
            .routing_model_resolution()
            .value,
        RoutingModelResolution::Heuristics(RoutingFallback::NotConfigured)
    );
}

/// Phase 2C line 4: declining the routing step leaves no routing model
/// configured *and* the system keeps working. Both halves are asserted,
/// and the second is the one that matters — "nothing crashed" is not the
/// contract, "deterministic heuristics are what answer" is.
#[test]
fn declining_the_routing_step_writes_no_routing_table_and_still_resolves() {
    let tmp = tempfile::tempdir().unwrap();
    let paths = RuntimePaths::new(tmp.path().join("data"), tmp.path().join("config"));

    let mut user = fully_populated_user_config();
    user.routing_mut().set_model(None);
    user.save(&paths).unwrap();

    let text = std::fs::read_to_string(paths.user_config_file()).unwrap();
    assert!(
        !text.contains("routing"),
        "\"Do later\" must leave no `[routing]` table at all, not an empty one:\n{text}"
    );

    let loaded = UserConfig::load(&paths).unwrap();
    let effective = EffectiveConfig::new(&loaded, None);
    let resolution = effective.routing_model_resolution();
    assert_eq!(
        resolution.value,
        RoutingModelResolution::Heuristics(RoutingFallback::NotConfigured),
        "deterministic heuristics must be what answers, and must say they are \
         answering because nothing was ever configured"
    );
    assert_eq!(resolution.layer, Layer::Default);
    assert_eq!(
        effective.routing_model().value,
        RoutingModelChoice::Deterministic
    );
}

/// Phase 2C's behavioural contract: a configuration naming a routing
/// model whose provider has disappeared must degrade "and say so". It is
/// the one lookup in this module that refuses to return an error — a
/// routing model is an optimisation over a system that already works
/// without it, so a rotated key must not stop Glasshouse from starting.
#[test]
fn a_pinned_routing_model_whose_provider_is_gone_degrades_and_names_it() {
    let mut user = UserConfig::default();
    user.providers_mut()
        .set("openrouter", ProviderConfig::new("openrouter"));
    user.routing_mut()
        .set_model(Some(RoutingModelChoice::Pinned {
            provider: "retired-mirror".to_owned(),
            model: "gpt-5.6-luna".to_owned(),
        }));

    let effective = EffectiveConfig::new(&user, None);
    let resolution = effective.routing_model_resolution();
    assert_eq!(
        resolution.value,
        RoutingModelResolution::Heuristics(RoutingFallback::ProviderNotConfigured {
            provider: "retired-mirror".to_owned(),
            model: "gpt-5.6-luna".to_owned(),
        })
    );
    assert_eq!(
        resolution.layer,
        Layer::User,
        "the layer reported is where the CHOICE came from, not a claim about \
         where the degrade was decided"
    );

    // The degrade has to be *sayable*, and saying "your routing model is
    // unavailable" without naming which one is not saying it.
    let said = resolution.value.fallback().unwrap().to_string();
    assert!(said.contains("`retired-mirror`"), "{said}");
    assert!(said.contains("`gpt-5.6-luna`"), "{said}");
    assert!(said.contains("which is not configured"), "{said}");
    assert!(
        said.contains("deterministic routing heuristics"),
        "the message must say what is answering instead:\n{said}"
    );

    // The contrast that proves the degrade is a lookup and not a
    // blanket refusal: the same shape pinned to a provider that *is*
    // configured resolves to that model.
    user.routing_mut()
        .set_model(Some(RoutingModelChoice::Pinned {
            provider: "openrouter".to_owned(),
            model: "gpt-5.6-luna".to_owned(),
        }));
    assert_eq!(
        EffectiveConfig::new(&user, None)
            .routing_model_resolution()
            .value,
        RoutingModelResolution::Pinned {
            provider: "openrouter".to_owned(),
            model: "gpt-5.6-luna".to_owned(),
        }
    );
}

/// A routing-model choice grants nothing and attests to nothing, so it
/// layers by the ordinary rule rather than following
/// `bypass_acknowledged`'s user-layer-only exception. The first case is
/// the reason the stored field is an `Option` and not a plain enum: a
/// project saying "deterministic, on purpose" has to be able to override
/// a user-level `automatic`, which a collapsed shape could not express.
#[test]
fn a_routing_choice_layers_project_over_user_and_reports_the_deciding_layer() {
    let mut user = UserConfig::default();
    user.routing_mut()
        .set_model(Some(RoutingModelChoice::Automatic));

    let mut project = ProjectConfig::default();
    project
        .routing_mut()
        .set_model(Some(RoutingModelChoice::Deterministic));

    // Case 1: the project's explicit deterministic-only beats the user's
    // automatic, and the reason given is "chosen", not "never set".
    let effective = EffectiveConfig::new(&user, Some(&project));
    let chosen = effective.routing_model();
    assert_eq!(chosen.value, RoutingModelChoice::Deterministic);
    assert_eq!(chosen.layer, Layer::Project);
    let resolution = effective.routing_model_resolution();
    assert_eq!(
        resolution.value,
        RoutingModelResolution::Heuristics(RoutingFallback::DeterministicChosen)
    );
    assert_eq!(resolution.layer, Layer::Project);

    // Case 2: a project that has recorded nothing falls through to the
    // user layer rather than shadowing it with a default.
    let silent = ProjectConfig::default();
    let effective = EffectiveConfig::new(&user, Some(&silent));
    let chosen = effective.routing_model();
    assert_eq!(chosen.value, RoutingModelChoice::Automatic);
    assert_eq!(chosen.layer, Layer::User);
    let resolution = effective.routing_model_resolution();
    assert_eq!(resolution.value, RoutingModelResolution::Automatic);
    assert_eq!(resolution.layer, Layer::User);

    // Case 3: neither layer decided, so the default answers — and says
    // so with `NotConfigured`, not `DeterministicChosen`.
    let undecided = UserConfig::default();
    let effective = EffectiveConfig::new(&undecided, Some(&silent));
    let chosen = effective.routing_model();
    assert_eq!(chosen.value, RoutingModelChoice::Deterministic);
    assert_eq!(chosen.layer, Layer::Default);
    let resolution = effective.routing_model_resolution();
    assert_eq!(
        resolution.value,
        RoutingModelResolution::Heuristics(RoutingFallback::NotConfigured)
    );
    assert_eq!(resolution.layer, Layer::Default);
}

#[test]
fn memory_extraction_enabled_layers_project_over_user_over_default() {
    let user = UserConfig::default();
    let effective = EffectiveConfig::new(&user, None);
    assert_eq!(
        effective.memory_extraction_enabled(),
        Layered::new(true, Layer::Default),
        "nothing recorded anywhere must resolve to enabled"
    );

    let mut user = UserConfig::default();
    user.set_memory_extraction(Some(false));
    let effective = EffectiveConfig::new(&user, None);
    assert_eq!(
        effective.memory_extraction_enabled(),
        Layered::new(false, Layer::User)
    );

    let mut project = ProjectConfig::default();
    project.set_memory_extraction(Some(true));
    let effective = EffectiveConfig::new(&user, Some(&project));
    assert_eq!(
        effective.memory_extraction_enabled(),
        Layered::new(true, Layer::Project),
        "a project's explicit re-enable must win over the user's disable"
    );

    let silent_project = ProjectConfig::default();
    let effective = EffectiveConfig::new(&user, Some(&silent_project));
    assert_eq!(
        effective.memory_extraction_enabled(),
        Layered::new(false, Layer::User),
        "a project that recorded nothing must fall through to the user layer"
    );
}

/// `GH-LAUNCH-BRIEFING`'s opt-out — the ruling is opt-out, not opt-in, so
/// nothing recorded anywhere must resolve to `true`, and a project's
/// explicit choice must win over the user's.
#[test]
fn inject_memory_at_launch_layers_project_over_user_over_default() {
    let user = UserConfig::default();
    let effective = EffectiveConfig::new(&user, None);
    assert_eq!(
        effective.inject_memory_at_launch(),
        Layered::new(true, Layer::Default),
        "nothing recorded anywhere must resolve to enabled"
    );

    let mut user = UserConfig::default();
    user.memory_mut().set_inject_at_launch(Some(false));
    let effective = EffectiveConfig::new(&user, None);
    assert_eq!(
        effective.inject_memory_at_launch(),
        Layered::new(false, Layer::User)
    );

    let mut project = ProjectConfig::default();
    project.memory_mut().set_inject_at_launch(Some(true));
    let effective = EffectiveConfig::new(&user, Some(&project));
    assert_eq!(
        effective.inject_memory_at_launch(),
        Layered::new(true, Layer::Project),
        "a project's explicit re-enable must win over the user's disable"
    );

    let silent_project = ProjectConfig::default();
    let effective = EffectiveConfig::new(&user, Some(&silent_project));
    assert_eq!(
        effective.inject_memory_at_launch(),
        Layered::new(false, Layer::User),
        "a project that recorded nothing must fall through to the user layer"
    );
}

/// Map line 1089's consent: `None` unless named, project overrides user —
/// the same layering [`EffectiveConfig::memory_extraction_model`] uses
/// for the sibling knob.
#[test]
fn memory_rerank_model_layers_project_over_user_over_default() {
    let user = UserConfig::default();
    let effective = EffectiveConfig::new(&user, None);
    assert_eq!(
        effective.memory_rerank_model(),
        Layered::new(None, Layer::Default),
        "nobody who never configured a rerank model has one"
    );

    let mut user = UserConfig::default();
    user.memory_mut()
        .set_rerank_model(Some(ExtractionModelRef::new(
            "free-runner",
            "a-cheap-model",
        )));
    let effective = EffectiveConfig::new(&user, None);
    assert_eq!(
        effective.memory_rerank_model(),
        Layered::new(
            Some(ExtractionModelRef::new("free-runner", "a-cheap-model")),
            Layer::User
        )
    );

    let mut project = ProjectConfig::default();
    project
        .memory_mut()
        .set_rerank_model(Some(ExtractionModelRef::new(
            "named-runner",
            "another-model",
        )));
    let effective = EffectiveConfig::new(&user, Some(&project));
    assert_eq!(
        effective.memory_rerank_model(),
        Layered::new(
            Some(ExtractionModelRef::new("named-runner", "another-model")),
            Layer::Project
        ),
        "a project's own choice must win over the user's"
    );

    let silent_project = ProjectConfig::default();
    let effective = EffectiveConfig::new(&user, Some(&silent_project));
    assert_eq!(
        effective.memory_rerank_model(),
        Layered::new(
            Some(ExtractionModelRef::new("free-runner", "a-cheap-model")),
            Layer::User
        ),
        "a project that recorded nothing must fall through to the user layer"
    );
}

/// Map line 1094: off unless named, project overrides user.
#[test]
fn memory_retrieval_diagnostics_layers_project_over_user_over_default() {
    let user = UserConfig::default();
    let effective = EffectiveConfig::new(&user, None);
    assert_eq!(
        effective.memory_retrieval_diagnostics(),
        Layered::new(false, Layer::Default),
        "nothing recorded anywhere must resolve to off"
    );

    let mut user = UserConfig::default();
    user.memory_mut().set_retrieval_diagnostics(Some(true));
    let effective = EffectiveConfig::new(&user, None);
    assert_eq!(
        effective.memory_retrieval_diagnostics(),
        Layered::new(true, Layer::User)
    );

    let mut project = ProjectConfig::default();
    project.memory_mut().set_retrieval_diagnostics(Some(false));
    let effective = EffectiveConfig::new(&user, Some(&project));
    assert_eq!(
        effective.memory_retrieval_diagnostics(),
        Layered::new(false, Layer::Project),
        "a project's explicit off must win over the user's on"
    );

    let silent_project = ProjectConfig::default();
    let effective = EffectiveConfig::new(&user, Some(&silent_project));
    assert_eq!(
        effective.memory_retrieval_diagnostics(),
        Layered::new(true, Layer::User),
        "a project that recorded nothing must fall through to the user layer"
    );
}

/// Map line 1769: off unless named, project overrides user, independent
/// of [`EffectiveConfig::memory_retrieval_diagnostics`]'s own flag.
#[test]
fn memory_extraction_diagnostics_layers_project_over_user_over_default() {
    let user = UserConfig::default();
    let effective = EffectiveConfig::new(&user, None);
    assert_eq!(
        effective.memory_extraction_diagnostics(),
        Layered::new(false, Layer::Default),
        "nothing recorded anywhere must resolve to off"
    );

    let mut user = UserConfig::default();
    user.memory_mut().set_extraction_diagnostics(Some(true));
    let effective = EffectiveConfig::new(&user, None);
    assert_eq!(
        effective.memory_extraction_diagnostics(),
        Layered::new(true, Layer::User)
    );

    let mut project = ProjectConfig::default();
    project.memory_mut().set_extraction_diagnostics(Some(false));
    let effective = EffectiveConfig::new(&user, Some(&project));
    assert_eq!(
        effective.memory_extraction_diagnostics(),
        Layered::new(false, Layer::Project),
        "a project's explicit off must win over the user's on"
    );

    let silent_project = ProjectConfig::default();
    let effective = EffectiveConfig::new(&user, Some(&silent_project));
    assert_eq!(
        effective.memory_extraction_diagnostics(),
        Layered::new(true, Layer::User),
        "a project that recorded nothing must fall through to the user layer"
    );

    // Independent of the retrieval flag: turning extraction diagnostics
    // on must not turn retrieval diagnostics on too.
    assert_eq!(
        effective.memory_retrieval_diagnostics(),
        Layered::new(false, Layer::Default),
        "the two diagnostics knobs must not leak into each other"
    );
}

#[test]
fn context_firewall_reducer_layers_project_over_user_and_defaults_to_none() {
    let user = UserConfig::default();
    let effective = EffectiveConfig::new(&user, None);
    assert_eq!(
        effective.context_firewall_reducer(),
        Layered::new(None, Layer::Default),
        "nobody who never configured a reducer has one"
    );

    let mut user = UserConfig::default();
    user.context_firewall_mut()
        .set_reducer(Some("openrouter".to_owned()));
    let effective = EffectiveConfig::new(&user, None);
    assert_eq!(
        effective.context_firewall_reducer(),
        Layered::new(Some("openrouter".to_owned()), Layer::User)
    );

    let mut project = ProjectConfig::default();
    project
        .context_firewall_mut()
        .set_reducer(Some("a-project-entitlement".to_owned()));
    let effective = EffectiveConfig::new(&user, Some(&project));
    assert_eq!(
        effective.context_firewall_reducer(),
        Layered::new(Some("a-project-entitlement".to_owned()), Layer::Project),
        "a project's own reducer choice must win over the user's"
    );
}

#[test]
fn context_firewall_min_semantic_tokens_defaults_and_layers() {
    let user = UserConfig::default();
    let effective = EffectiveConfig::new(&user, None);
    assert_eq!(
        effective.context_firewall_min_semantic_tokens(),
        Layered::new(firewall::DEFAULT_MIN_SEMANTIC_TOKENS, Layer::Default)
    );

    let mut user = UserConfig::default();
    user.context_firewall_mut()
        .set_min_semantic_tokens(Some(500));
    let effective = EffectiveConfig::new(&user, None);
    assert_eq!(
        effective.context_firewall_min_semantic_tokens(),
        Layered::new(500, Layer::User)
    );
}

#[test]
fn context_firewall_aggressive_drops_uncertain_defaults_to_false() {
    let user = UserConfig::default();
    let effective = EffectiveConfig::new(&user, None);
    assert_eq!(
        effective.context_firewall_aggressive_drops_uncertain(),
        Layered::new(false, Layer::Default),
        "bias to inclusion is the default nobody had to ask for"
    );

    let mut user = UserConfig::default();
    user.context_firewall_mut()
        .set_aggressive_drops_uncertain(Some(true));
    let effective = EffectiveConfig::new(&user, None);
    assert_eq!(
        effective.context_firewall_aggressive_drops_uncertain(),
        Layered::new(true, Layer::User)
    );
}

#[test]
fn context_firewall_reducer_local_only_defaults_to_false() {
    let user = UserConfig::default();
    let effective = EffectiveConfig::new(&user, None);
    assert_eq!(
        effective.context_firewall_reducer_local_only(),
        Layered::new(false, Layer::Default)
    );

    let mut user = UserConfig::default();
    user.context_firewall_mut()
        .set_reducer_local_only(Some(true));
    let effective = EffectiveConfig::new(&user, None);
    assert_eq!(
        effective.context_firewall_reducer_local_only(),
        Layered::new(true, Layer::User)
    );
}

#[test]
fn automatic_checkpoint_enabled_layers_project_over_user_over_default() {
    let user = UserConfig::default();
    let effective = EffectiveConfig::new(&user, None);
    assert_eq!(
        effective.automatic_checkpoint_enabled(),
        Layered::new(true, Layer::Default),
        "nothing recorded anywhere must resolve to enabled"
    );

    let mut user = UserConfig::default();
    user.set_automatic_checkpoint(Some(false));
    let effective = EffectiveConfig::new(&user, None);
    assert_eq!(
        effective.automatic_checkpoint_enabled(),
        Layered::new(false, Layer::User)
    );

    let mut project = ProjectConfig::default();
    project.set_automatic_checkpoint(Some(true));
    let effective = EffectiveConfig::new(&user, Some(&project));
    assert_eq!(
        effective.automatic_checkpoint_enabled(),
        Layered::new(true, Layer::Project),
        "a project's explicit re-enable must win over the user's disable"
    );

    let silent_project = ProjectConfig::default();
    let effective = EffectiveConfig::new(&user, Some(&silent_project));
    assert_eq!(
        effective.automatic_checkpoint_enabled(),
        Layered::new(false, Layer::User),
        "a project that recorded nothing must fall through to the user layer"
    );
}

/// The independence half of the automatic-checkpoint switch:
/// [`EffectiveConfig::automatic_checkpoint_enabled`] must depend only on
/// its own field, never on [`UserConfig::memory_extraction`] or any other
/// automatic behaviour, and vice versa.
#[test]
fn automatic_checkpoint_and_memory_extraction_disable_independently() {
    for (checkpoint_off, memory_off) in [(false, false), (true, false), (false, true), (true, true)]
    {
        let mut user = UserConfig::default();
        user.set_automatic_checkpoint(Some(!checkpoint_off));
        user.set_memory_extraction(Some(!memory_off));

        let effective = EffectiveConfig::new(&user, None);

        assert_eq!(
            effective.automatic_checkpoint_enabled().value,
            !checkpoint_off,
            "checkpoint state must depend only on its own field, case {checkpoint_off} {memory_off}"
        );
        assert_eq!(
            effective.memory_extraction_enabled().value,
            !memory_off,
            "memory-extraction state must depend only on its own field, case {checkpoint_off} {memory_off}"
        );
    }
}

/// A pin is two names — a key into `ProviderTable` and a model name —
/// and never a credential, alongside
/// [`serialized_form_has_no_secret_capable_field`]'s structural guard on
/// the same shape. This is the behavioural half: a real key is planted
/// in the environment the pinned provider's `credential_env` points at,
/// so a serializer that resolved the pin to a usable credential — the
/// failure this test exists to catch — would have something to leak.
#[test]
fn a_pinned_routing_model_persists_names_and_never_a_credential_value() {
    const VAR: &str = "GLASSHOUSE_CONFIG_TEST_ONLY_ROUTING_PIN_VAR";
    const VALUE: &str = "sk-or-v1-routingpin0123456789abcdef0123456789abcdef01234567";

    let tmp = tempfile::tempdir().unwrap();
    let paths = RuntimePaths::new(tmp.path().join("data"), tmp.path().join("config"));

    let mut provider = ProviderConfig::new("openrouter");
    provider
        .set_credential_env(vec![VAR.to_owned()])
        .set_credential_store(Some(StoredCredentialRef::new("glasshouse", VAR)));

    let pinned = RoutingModelChoice::Pinned {
        provider: "openrouter".to_owned(),
        model: "gpt-5.6-luna".to_owned(),
    };
    let mut user = UserConfig::default();
    user.providers_mut().set("openrouter", provider);
    user.routing_mut().set_model(Some(pinned.clone()));

    // SAFETY: `VAR` is unique to this test and removed again below.
    unsafe {
        std::env::set_var(VAR, VALUE);
    }
    let saved = user.save(&paths);
    unsafe {
        std::env::remove_var(VAR);
    }
    saved.unwrap();

    let text = std::fs::read_to_string(paths.user_config_file()).unwrap();
    assert!(
        !text.contains(VALUE),
        "a credential value reached the configuration file:\n{text}"
    );
    assert!(
        !text.contains("sk-or-v1-"),
        "not even a prefix of a key belongs in a tracked configuration file:\n{text}"
    );

    // ... and the two names a pin is made of really are what got
    // written, so the assertion above is not passing on an empty file.
    assert!(text.contains("gpt-5.6-luna"), "{text}");
    assert!(text.contains("openrouter"), "{text}");
    assert!(text.contains("pinned"), "{text}");
    assert!(text.contains(VAR), "the NAME must be there:\n{text}");

    let loaded = UserConfig::load(&paths).unwrap();
    assert_eq!(loaded.routing().model(), Some(&pinned));
}

/// Every configuration file on disk today was written before this field
/// existed, so the missing `[routing]` table is the ordinary case, not an
/// edge one — the same treatment this module already gives unknown and
/// missing keys. Written by hand rather than saved, because a config this
/// build produced could never be missing a key this build knows about.
#[test]
fn a_configuration_written_before_routing_existed_loads_with_nothing_recorded() {
    let tmp = tempfile::tempdir().unwrap();
    let paths = RuntimePaths::new(tmp.path().join("data"), tmp.path().join("config"));
    std::fs::create_dir_all(paths.config_dir()).unwrap();
    std::fs::write(
        paths.user_config_file(),
        r#"
            version = 1

            [onboarding]
            completed = true
            completed_at_version = "0.1.0"

            [integrations.claude-code]
            enabled = true

            [providers.openrouter]
            template = "openrouter"
            credential_env = ["OPENROUTER_API_KEY"]
        "#,
    )
    .unwrap();

    let loaded = UserConfig::load(&paths).unwrap();
    assert!(loaded.onboarding().completed());
    assert_eq!(
        loaded.routing().model(),
        None,
        "an older file must load as \"never decided\", not as some invented choice"
    );

    let effective = EffectiveConfig::new(&loaded, None);
    let resolution = effective.routing_model_resolution();
    assert_eq!(
        resolution.value,
        RoutingModelResolution::Heuristics(RoutingFallback::NotConfigured)
    );
    assert_eq!(resolution.layer, Layer::Default);
}

/// Phase 2D routing preferences are exact, bounded, independently
/// layered values. A real save/load proves their serde wiring; mixed
/// layers prove one project override does not copy its siblings; invalid
/// TOML proves absurd scalar values cannot enter through hand editing.
#[test]
fn routing_policy_values_round_trip_layer_independently_and_reject_absurd_inputs() {
    let tmp = tempfile::tempdir().unwrap();
    let paths = RuntimePaths::new(tmp.path().join("data"), tmp.path().join("config"));

    let latency_user = RouterLatencyMs::try_from(1_500).unwrap();
    let cost_user = RouterCostMicroUsd::try_from(2_500).unwrap();
    let reserve_user = PremiumReservePercent::try_from(15).unwrap();
    let mut user = UserConfig::default();
    user.routing_mut()
        .set_max_router_latency(Some(latency_user))
        .set_max_marginal_cost(Some(cost_user))
        .set_prefer_free(Some(false))
        .set_premium_reserve(Some(reserve_user));
    user.save(&paths).unwrap();
    let loaded = UserConfig::load(&paths).unwrap();
    assert_eq!(loaded.routing(), user.routing());

    let latency_project = RouterLatencyMs::try_from(350).unwrap();
    let mut project = ProjectConfig::default();
    project
        .routing_mut()
        .set_max_router_latency(Some(latency_project))
        .set_prefer_free(Some(true));
    let effective = EffectiveConfig::new(&loaded, Some(&project));
    assert_eq!(
        effective.max_router_latency(),
        Layered::new(latency_project, Layer::Project)
    );
    assert_eq!(
        effective.max_router_cost(),
        Layered::new(cost_user, Layer::User)
    );
    assert_eq!(
        effective.prefer_free_routing(),
        Layered::new(true, Layer::Project)
    );
    assert_eq!(
        effective.premium_reserve(),
        Layered::new(reserve_user, Layer::User)
    );

    for invalid in [
        "max_router_latency_ms = 0",
        "max_router_latency_ms = 60001",
        "max_marginal_cost_micro_usd = 1000001",
        "premium_reserve_percent = 101",
    ] {
        let text = format!("version = 1\n[routing]\n{invalid}\n");
        assert!(
            toml::from_str::<UserConfig>(&text).is_err(),
            "absurd routing policy was accepted: {invalid}"
        );
    }
    assert_eq!(RouterLatencyMs::DEFAULT.get(), 2_000);
    assert_eq!(RouterCostMicroUsd::DEFAULT.get(), 1_000);
    assert_eq!(PremiumReservePercent::DEFAULT.get(), 20);
}

/// Capability map line 1270: capacity-band thresholds are user-
/// configurable, and a non-ascending set is refused at load time rather
/// than sorted into shape — the same fail-closed idiom
/// `routing_policy_values_round_trip_layer_independently_and_reject_absurd_inputs`
/// already proves for the single-field routing values, applied here to a
/// value validated across four fields at once.
#[test]
fn capacity_band_thresholds_round_trip_and_reject_a_non_monotonic_set() {
    let tmp = tempfile::tempdir().unwrap();
    let paths = RuntimePaths::new(tmp.path().join("data"), tmp.path().join("config"));

    let mut user = UserConfig::default();
    assert_eq!(user.routing().capacity_band_thresholds(), None);
    user.routing_mut().set_capacity_band_thresholds(Some(
        crate::provider::quota::CapacityBandThresholds::new(1, 10, 30, 60)
            .unwrap()
            .into(),
    ));
    user.save(&paths).unwrap();
    let loaded = UserConfig::load(&paths).unwrap();
    assert_eq!(
        loaded.routing().capacity_band_thresholds(),
        user.routing().capacity_band_thresholds()
    );

    let effective = EffectiveConfig::new(&loaded, None);
    let resolved = effective.capacity_band_thresholds();
    assert_eq!(resolved.layer, Layer::User);
    assert_eq!(resolved.value.reserve_percent(), 10);

    // §35-adjacent: prove the *loader* itself is the fail-closed gate,
    // not merely `CapacityBandThresholds::new` in isolation — this
    // parses through the exact path `UserConfig::load` uses.
    for invalid in [
        // reserve (50) above tight (30): not ascending.
        "[routing.capacity_band_thresholds]\nexhausted_percent = 2\nreserve_percent = 50\n\
         tight_percent = 30\nhealthy_percent = 70\n",
        // healthy_percent above 100.
        "[routing.capacity_band_thresholds]\nexhausted_percent = 2\nreserve_percent = 10\n\
         tight_percent = 30\nhealthy_percent = 150\n",
    ] {
        let text = format!("version = 1\n{invalid}");
        assert!(
            toml::from_str::<UserConfig>(&text).is_err(),
            "a non-monotonic set of capacity-band thresholds was accepted: {invalid}"
        );
    }

    // With nothing recorded, the domain default applies.
    let empty = UserConfig::default();
    let effective = EffectiveConfig::new(&empty, None);
    assert_eq!(
        effective.capacity_band_thresholds().value,
        crate::provider::quota::CapacityBandThresholds::DEFAULT
    );
    assert_eq!(effective.capacity_band_thresholds().layer, Layer::Default);
}

/// Capability map lines 1357/1358: routing score weights are
/// user-configurable, round-trip through the loader, resolve project
/// over user over [`crate::routing::session::ScoreWeights::default`] —
/// the same layering [`CapacityBandThresholdsConfig`]'s own test proves
/// — and a non-finite field is refused at load time rather than
/// substituted silently.
#[test]
fn score_weights_round_trip_layer_project_over_user_and_reject_non_finite_fields() {
    let tmp = tempfile::tempdir().unwrap();
    let paths = RuntimePaths::new(tmp.path().join("data"), tmp.path().join("config"));

    let mut user = UserConfig::default();
    assert_eq!(user.routing().score_weights(), None);
    let user_weights = crate::routing::session::ScoreWeights {
        quota_pressure_weight: 0.4,
        health_failure_penalty: -0.5,
        health_penalty_floor: -1.2,
        health_unavailable_penalty: -2.0,
    };
    user.routing_mut()
        .set_score_weights(Some(user_weights.into()));
    user.save(&paths).unwrap();
    let loaded = UserConfig::load(&paths).unwrap();
    assert_eq!(
        loaded.routing().score_weights(),
        user.routing().score_weights()
    );

    let effective = EffectiveConfig::new(&loaded, None);
    let resolved = effective.score_weights();
    assert_eq!(resolved.layer, Layer::User);
    assert_eq!(resolved.value, user_weights);

    let mut project = ProjectConfig::default();
    let project_weights = crate::routing::session::ScoreWeights {
        quota_pressure_weight: 0.1,
        ..user_weights
    };
    project
        .routing_mut()
        .set_score_weights(Some(project_weights.into()));
    let effective = EffectiveConfig::new(&loaded, Some(&project));
    let resolved = effective.score_weights();
    assert_eq!(resolved.layer, Layer::Project);
    assert_eq!(resolved.value, project_weights);

    // §35-adjacent: the loader itself is the fail-closed gate, not
    // merely a hypothetical caller of `ScoreWeights` in isolation — this
    // parses through the exact path `UserConfig::load` uses.
    for invalid in [
        "[routing.score_weights]\nquota_pressure_weight = nan\n\
         health_failure_penalty = -0.3\nhealth_penalty_floor = -0.9\n\
         health_unavailable_penalty = -1.5\n",
        "[routing.score_weights]\nquota_pressure_weight = 0.8\n\
         health_failure_penalty = -0.3\nhealth_penalty_floor = -0.9\n\
         health_unavailable_penalty = inf\n",
    ] {
        let text = format!("version = 1\n{invalid}");
        assert!(
            toml::from_str::<UserConfig>(&text).is_err(),
            "a non-finite score weight was accepted: {invalid}"
        );
    }

    // With nothing recorded, the domain default applies — today's
    // compile-time constants, unchanged.
    let empty = UserConfig::default();
    let effective = EffectiveConfig::new(&empty, None);
    assert_eq!(
        effective.score_weights().value,
        crate::routing::session::ScoreWeights::default()
    );
    assert_eq!(effective.score_weights().layer, Layer::Default);
}

/// Capability map line 1577: `[routing.reserve]` carries two policies,
/// they round-trip through the loader, they resolve **per field** with
/// the project layer first, and a layer that recorded neither leaves the
/// fail-closed `protect` default in place for both scopes.
#[test]
fn reserve_policies_round_trip_and_resolve_per_scope_with_protect_as_the_default() {
    use crate::routing::pressure::{ReservePolicy, ReserveScope};

    let tmp = tempfile::tempdir().unwrap();
    let paths = RuntimePaths::new(tmp.path().join("data"), tmp.path().join("config"));

    let mut user = UserConfig::default();
    assert_eq!(user.routing().reserve(), None);
    let mut reserve = ReservePoliciesConfig::default();
    reserve
        .set_interactive(Some(ReservePolicy::Spend))
        .set_background(Some(ReservePolicy::Protect));
    user.routing_mut().set_reserve(Some(reserve));
    user.save(&paths).unwrap();
    let loaded = UserConfig::load(&paths).unwrap();
    assert_eq!(loaded.routing().reserve(), user.routing().reserve());

    // The on-disk spelling is the enum's own, kebab-case.
    let text = std::fs::read_to_string(paths.user_config_file()).unwrap();
    assert!(text.contains("interactive = \"spend\""), "{text}");
    assert!(text.contains("background = \"protect\""), "{text}");

    let effective = EffectiveConfig::new(&loaded, None);
    let interactive = effective.reserve_policy(ReserveScope::Interactive);
    assert_eq!(
        (interactive.value, interactive.layer),
        (ReservePolicy::Spend, Layer::User)
    );
    let background = effective.reserve_policy(ReserveScope::Background);
    assert_eq!(
        (background.value, background.layer),
        (ReservePolicy::Protect, Layer::User)
    );

    // A project that records only the background policy wins that field
    // and leaves the interactive one to the user layer.
    let project: ProjectConfig =
        toml::from_str("version = 1\n\n[routing.reserve]\nbackground = \"spend\"\n").unwrap();
    let effective = EffectiveConfig::new(&loaded, Some(&project));
    let interactive = effective.reserve_policy(ReserveScope::Interactive);
    assert_eq!(
        (interactive.value, interactive.layer),
        (ReservePolicy::Spend, Layer::User)
    );
    let background = effective.reserve_policy(ReserveScope::Background);
    assert_eq!(
        (background.value, background.layer),
        (ReservePolicy::Spend, Layer::Project)
    );
    assert_eq!(
        effective.reserve_policies(),
        crate::routing::pressure::ReservePolicies {
            interactive: ReservePolicy::Spend,
            background: ReservePolicy::Spend,
        }
    );

    // Nothing recorded anywhere: protect, for both, from the default layer.
    let empty = UserConfig::default();
    let effective = EffectiveConfig::new(&empty, None);
    for scope in [ReserveScope::Interactive, ReserveScope::Background] {
        let resolved = effective.reserve_policy(scope);
        assert_eq!(
            (resolved.value, resolved.layer),
            (ReservePolicy::Protect, Layer::Default)
        );
    }

    // An unknown spelling is refused by the loader rather than defaulted.
    assert!(
        toml::from_str::<UserConfig>(
            "version = 1\n\n[routing.reserve]\ninteractive = \"exclude\"\n"
        )
        .is_err(),
        "an unknown reserve policy must be refused, not read as a default"
    );
}

/// Phase 56 lines 1946 and 1947: `[entitlements.<name>]` round-trips
/// through the loader with the routing types' own spellings, resolves
/// **by name** with the project layer replacing the user's entry whole,
/// supplies an unrestricted default for every harness's own sign-in that
/// nobody claimed, and refuses every unknown spelling rather than reading
/// it as "no rule".
#[test]
fn entitlements_round_trip_and_resolve_project_over_user_with_a_native_default() {
    use crate::profile::BackendResource;
    use crate::routing::classify::WorkloadTier;
    use crate::routing::disposable::JobKind;

    let tmp = tempfile::tempdir().unwrap();
    let paths = RuntimePaths::new(tmp.path().join("data"), tmp.path().join("config"));

    let mut user = UserConfig::default();
    assert!(user.entitlements().is_empty());
    let mut max = EntitlementConfig::default();
    max.set_kind(Some(EntitlementKind::Claude))
        .set_native_harness(Some(IntegrationId::ClaudeCode))
        .set_deny_tiers([WorkloadTier::Leaf])
        .set_allow_job_kinds([JobKind::MemoryExtraction]);
    user.entitlements_mut().set("max", max);
    let mut team = EntitlementConfig::default();
    team.set_kind(Some(EntitlementKind::ApiKey))
        .set_provider(Some("openrouter".to_owned()))
        .set_allow_harnesses([IntegrationId::Codex])
        .set_deny_harnesses([IntegrationId::ClaudeCode]);
    user.entitlements_mut().set("team-key", team);
    user.save(&paths).unwrap();
    let loaded = UserConfig::load(&paths).unwrap();
    assert_eq!(loaded.entitlements(), user.entitlements());

    // The on-disk spellings are the routing types' own.
    let text = std::fs::read_to_string(paths.user_config_file()).unwrap();
    for expected in [
        "[entitlements.max]",
        "kind = \"claude\"",
        "native_harness = \"claude-code\"",
        "deny_tiers = [\"leaf\"]",
        "allow_job_kinds = [\"memory extraction\"]",
        "[entitlements.team-key]",
        "kind = \"api-key\"",
        "provider = \"openrouter\"",
        "allow_harnesses = [\"codex\"]",
        "deny_harnesses = [\"claude-code\"]",
    ] {
        assert!(text.contains(expected), "missing `{expected}` in:\n{text}");
    }

    // The user layer alone: `max` is Claude Code's sign-in, every other
    // harness gets its unrestricted default, and the API key is found by
    // the provider it backs.
    let effective = EffectiveConfig::new(&loaded, None);
    let claude = effective
        .entitlement_for(IntegrationId::ClaudeCode, &BackendResource::Native)
        .unwrap()
        .expect("a harness's own sign-in always resolves to an entitlement");
    assert_eq!((claude.name(), claude.layer()), ("max", Layer::User));
    assert_eq!(claude.kind(), Some(EntitlementKind::Claude));
    assert!(!claude.rules().serves_tier(WorkloadTier::Leaf));
    assert!(claude.rules().serves_tier(WorkloadTier::Heavy));
    assert!(claude.rules().serves_job_kind(JobKind::MemoryExtraction));
    assert!(!claude.rules().serves_job_kind(JobKind::Classification));
    assert_eq!(claude.describe(), "Claude plan, Claude Code's own sign-in");

    let codex = effective
        .entitlement_for(IntegrationId::Codex, &BackendResource::Native)
        .unwrap()
        .unwrap();
    assert_eq!((codex.name(), codex.layer()), ("codex", Layer::Default));
    assert_eq!(codex.kind(), None);
    assert!(codex.rules().is_unrestricted());
    assert_eq!(codex.describe(), "Codex's own sign-in");

    let key = effective
        .entitlement_for(
            IntegrationId::Codex,
            &BackendResource::DirectProvider {
                provider: "openrouter".to_owned(),
            },
        )
        .unwrap()
        .unwrap();
    assert_eq!(key.name(), "team-key");
    assert!(key.rules().serves_harness(IntegrationId::Codex));
    assert!(!key.rules().serves_harness(IntegrationId::ClaudeCode));
    assert!(
        !key.rules().serves_harness(IntegrationId::Cursor),
        "a non-empty allow-list admits only what it names"
    );
    assert_eq!(key.describe(), "API key, behind provider `openrouter`");

    // No entry names this provider, and the gateway assigns its upstream
    // at session start: both are `None`, not a guess.
    assert_eq!(
        effective
            .entitlement_for(
                IntegrationId::ClaudeCode,
                &BackendResource::DirectProvider {
                    provider: "nobody-configured".to_owned(),
                },
            )
            .unwrap(),
        None
    );
    assert_eq!(
        effective
            .entitlement_for(
                IntegrationId::ClaudeCode,
                &BackendResource::GlasshouseGateway
            )
            .unwrap(),
        None
    );

    // A project entry of the same name replaces the user's whole: `max`
    // becomes Codex's sign-in with different rules, so Claude Code falls
    // back to its default and Codex is served by the project's `max`.
    let project: ProjectConfig = toml::from_str(
        "version = 1\n\n[entitlements.max]\nnative_harness = \"codex\"\n\
         allow_tiers = [\"heavy\", \"frontier\"]\n",
    )
    .unwrap();
    let effective = EffectiveConfig::new(&loaded, Some(&project));
    let claude = effective
        .entitlement_for(IntegrationId::ClaudeCode, &BackendResource::Native)
        .unwrap()
        .unwrap();
    assert_eq!(
        (claude.name(), claude.layer()),
        ("claude-code", Layer::Default)
    );
    let codex = effective
        .entitlement_for(IntegrationId::Codex, &BackendResource::Native)
        .unwrap()
        .unwrap();
    assert_eq!((codex.name(), codex.layer()), ("max", Layer::Project));
    assert_eq!(
        codex.kind(),
        None,
        "the project's entry replaced the kind too"
    );
    assert!(codex.rules().serves_tier(WorkloadTier::Heavy));
    assert!(!codex.rules().serves_tier(WorkloadTier::Leaf));

    // Every harness — and only a harness — has an entry.
    let all = effective.entitlements().unwrap();
    for id in IntegrationId::ALL {
        let has_default = all
            .iter()
            .any(|s| s.backing() == &EntitlementBacking::NativeHarness(*id));
        assert_eq!(
            has_default,
            id.kind() == crate::integrations::IntegrationKind::Harness,
            "{}",
            id.slug()
        );
    }

    // Unknown spellings are refused by the loader, never read as "no rule".
    for bad in [
        "[entitlements.x]\ndeny_tiers = [\"huge\"]\n",
        "[entitlements.x]\nallow_harnesses = [\"ollama\"]\n",
        "[entitlements.x]\nallow_harnesses = [\"Claude Code\"]\n",
        "[entitlements.x]\nkind = \"netflix\"\n",
        "[entitlements.x]\nallow_job_kinds = [\"laundry\"]\n",
    ] {
        assert!(
            toml::from_str::<UserConfig>(&format!("version = 1\n\n{bad}")).is_err(),
            "must be refused: {bad}"
        );
    }
}

/// The contradictions only the resolved set can show, each refused by
/// name rather than settled by picking one.
#[test]
fn contradictory_entitlement_tables_are_refused_by_name() {
    use crate::profile::BackendResource;

    let both: UserConfig = toml::from_str(
        "version = 1\n\n[entitlements.x]\nnative_harness = \"codex\"\nprovider = \"openrouter\"\n",
    )
    .unwrap();
    let err = EffectiveConfig::new(&both, None)
        .entitlements()
        .unwrap_err();
    assert!(matches!(err, EntitlementLookupError::TwoBackings { ref name } if name == "x"));

    let two_claim: UserConfig = toml::from_str(
        "version = 1\n\n[entitlements.a]\nnative_harness = \"codex\"\n\n\
         [entitlements.b]\nnative_harness = \"codex\"\n",
    )
    .unwrap();
    let err = EffectiveConfig::new(&two_claim, None)
        .entitlement_for(IntegrationId::Codex, &BackendResource::Native)
        .unwrap_err();
    assert!(
        matches!(
            &err,
            EntitlementLookupError::AmbiguousNativeHarness { harness: IntegrationId::Codex, names }
                if names == &["a".to_owned(), "b".to_owned()]
        ),
        "{err}"
    );
    // Claude Code is untouched by Codex's contradiction.
    assert!(
        EffectiveConfig::new(&two_claim, None)
            .entitlement_for(IntegrationId::ClaudeCode, &BackendResource::Native)
            .is_ok()
    );

    let two_providers: UserConfig = toml::from_str(
        "version = 1\n\n[entitlements.a]\nprovider = \"openrouter\"\n\n\
         [entitlements.b]\nprovider = \"openrouter\"\n",
    )
    .unwrap();
    let err = EffectiveConfig::new(&two_providers, None)
        .entitlement_for(
            IntegrationId::Codex,
            &BackendResource::DirectProvider {
                provider: "openrouter".to_owned(),
            },
        )
        .unwrap_err();
    assert!(
        matches!(&err, EntitlementLookupError::AmbiguousProvider { provider, .. } if provider == "openrouter"),
        "{err}"
    );

    let reserved: UserConfig =
        toml::from_str("version = 1\n\n[entitlements.codex]\nprovider = \"openrouter\"\n").unwrap();
    let err = EffectiveConfig::new(&reserved, None)
        .entitlements()
        .unwrap_err();
    assert!(
        matches!(&err, EntitlementLookupError::NameReservedForHarness { name, harness: IntegrationId::Codex } if name == "codex"),
        "{err}"
    );

    // An entry that names neither backing is listed and matches nothing.
    let unstated: UserConfig =
        toml::from_str("version = 1\n\n[entitlements.someday]\nkind = \"gemini\"\n").unwrap();
    let all = EffectiveConfig::new(&unstated, None)
        .entitlements()
        .unwrap();
    let someday = all.iter().find(|s| s.name() == "someday").unwrap();
    assert_eq!(someday.backing(), &EntitlementBacking::Unstated);
    assert_eq!(someday.describe(), "Gemini plan, no backing stated");
}

/// Every [`crate::routing::disposable::JobKind`] is listed in
/// [`JOB_KIND_SPELLINGS`] exactly once and round-trips through its
/// spelling — the run-time half of the guard `job_kind_ordinal`'s
/// exhaustive `match` provides at compile time.
#[test]
fn every_job_kind_spelling_round_trips() {
    for (index, kind) in JOB_KIND_SPELLINGS.into_iter().enumerate() {
        assert_eq!(job_kind_ordinal(kind), index, "{kind} is out of order");
        let configured = ConfiguredJobKind::parse(kind.as_str())
            .unwrap_or_else(|| panic!("`{}` must parse", kind.as_str()));
        assert_eq!(configured.kind(), kind);
        let json = serde_json::to_string(&configured).unwrap();
        let back: ConfiguredJobKind = serde_json::from_str(&json).unwrap();
        assert_eq!(back, configured);
    }
    assert_eq!(
        ConfiguredJobKind::parse("Classification"),
        None,
        "exact, not case-folded"
    );
    assert_eq!(
        ConfiguredHarness::parse("cmux"),
        None,
        "cmux is not a harness"
    );
    assert_eq!(
        ConfiguredHarness::parse("claude-code").map(|h| h.id()),
        Some(IntegrationId::ClaudeCode)
    );
}

/// Map line 1796, the spelling half. Every
/// [`crate::routing::classify::WorkloadTier`] is listed in
/// [`WORKLOAD_TIER_SPELLINGS`] exactly once and round-trips through
/// [`ConfiguredWorkloadTier`]'s parse and its serialised form — so the
/// config file's vocabulary is the tier type's own `as_str` and cannot
/// drift from it.
///
/// [`workload_tier_ordinal`]'s exhaustive `match` is the compile-time
/// half of the same guard; this is the run-time half that checks the
/// array and the match still agree.
#[test]
fn every_workload_tier_spelling_round_trips() {
    use crate::routing::classify::WorkloadTier;

    assert_eq!(
        WORKLOAD_TIER_SPELLINGS.len(),
        5,
        "a `WorkloadTier` variant was added or removed without updating this array"
    );
    for tier in WORKLOAD_TIER_SPELLINGS {
        assert_eq!(
            WORKLOAD_TIER_SPELLINGS[workload_tier_ordinal(tier)],
            tier,
            "`{tier}` is not at its own ordinal in WORKLOAD_TIER_SPELLINGS"
        );
        let configured = ConfiguredWorkloadTier::new(tier);
        assert_eq!(configured.as_str(), tier.as_str());
        assert_eq!(
            ConfiguredWorkloadTier::parse(tier.as_str()),
            Some(configured),
            "`{tier}` does not parse back from its own spelling"
        );
    }
    // The spellings are the tier type's, not a second vocabulary.
    assert_eq!(
        ConfiguredWorkloadTier::parse("heavy").map(ConfiguredWorkloadTier::tier),
        Some(WorkloadTier::Heavy)
    );
    // And nothing else parses — in particular nothing that would read as
    // a *lower* ceiling than the user wrote.
    for unknown in ["Heavy", "heavy ", "", "tier-3", "premium"] {
        assert_eq!(
            ConfiguredWorkloadTier::parse(unknown),
            None,
            "`{unknown}` must not parse as a workload tier"
        );
    }
}

/// Map line 1796, the fail-closed half — practice §68's family. A
/// misspelt ceiling must be a **load error**, never a silently absent
/// one: an absent ceiling is what the router reads as *not established*,
/// so a typo that read as absent would quietly widen the set of
/// destinations a task may go to and nothing anywhere would say so.
#[test]
fn an_unknown_model_ceiling_spelling_is_refused_at_load_rather_than_read_as_absent() {
    let good = "version = 1\n\n[providers.alpha]\ntemplate = \"openrouter\"\n\n\
                [providers.alpha.model_ceilings]\nsmall = \"leaf\"\n";
    let parsed: UserConfig = toml::from_str(good).expect("a known spelling must load");
    assert_eq!(
        parsed
            .providers()
            .get("alpha")
            .expect("the provider was configured")
            .ceiling_of("small"),
        Some(crate::routing::classify::WorkloadTier::Leaf)
    );

    let typo = "version = 1\n\n[providers.alpha]\ntemplate = \"openrouter\"\n\n\
                [providers.alpha.model_ceilings]\nsmall = \"lite\"\n";
    let err = toml::from_str::<UserConfig>(typo)
        .expect_err("an unknown workload tier must be refused, not read as no ceiling");
    let rendered = err.to_string();
    assert!(
        rendered.contains("lite") && rendered.contains("leaf"),
        "the refusal must name what was written and what is accepted:\n{rendered}"
    );
}

/// Map line 1796's lookup, and the three shapes of *not established*
/// that must never read as a low ceiling: an unnamed model, an
/// unconfigured provider, and a provider configured with no ceilings at
/// all. Layered project-over-user, exactly as
/// [`EffectiveConfig::model_cost`] is.
#[test]
fn model_ceiling_is_layered_and_absent_where_nobody_stated_one() {
    use crate::routing::classify::WorkloadTier;

    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("workspace");
    std::fs::create_dir_all(&root).unwrap();
    let project_root = test_project(&root);

    let mut user = UserConfig::default();
    let mut user_alpha = ProviderConfig::new("openrouter");
    user_alpha.set_model_ceilings(BTreeMap::from([
        (
            "small".to_owned(),
            ConfiguredWorkloadTier::new(WorkloadTier::Leaf),
        ),
        (
            "big".to_owned(),
            ConfiguredWorkloadTier::new(WorkloadTier::Frontier),
        ),
    ]));
    user.providers_mut().set("alpha", user_alpha);
    // A configured provider that states no ceiling at all.
    user.providers_mut()
        .set("beta", ProviderConfig::new("openrouter"));

    let effective = EffectiveConfig::new(&user, None);
    assert_eq!(
        effective.model_ceiling("alpha", "small"),
        Layered::new(Some(WorkloadTier::Leaf), Layer::User)
    );
    assert_eq!(
        effective.model_ceiling("alpha", "big"),
        Layered::new(Some(WorkloadTier::Frontier), Layer::User)
    );
    assert_eq!(
        effective.model_ceiling("alpha", "unnamed").value,
        None,
        "a model nobody named a ceiling for is not established, not capped"
    );
    assert_eq!(
        effective.model_ceiling("beta", "small").value,
        None,
        "a provider with no ceilings states nothing about any of its models"
    );
    assert_eq!(
        effective.model_ceiling("nowhere", "small"),
        Layered::new(None, Layer::Default),
        "a provider nobody configured is not a provider anybody capped"
    );

    // The project layer wins over the user layer, per provider, the same
    // way `model_cost` resolves beside it.
    let mut project = ProjectConfig::default();
    let mut project_alpha = ProviderConfig::new("openrouter");
    project_alpha.set_model_ceilings(BTreeMap::from([(
        "small".to_owned(),
        ConfiguredWorkloadTier::new(WorkloadTier::Standard),
    )]));
    project.providers_mut().set("alpha", project_alpha);
    let effective = EffectiveConfig::new(&user, Some(&project));
    assert_eq!(
        effective.model_ceiling("alpha", "small"),
        Layered::new(Some(WorkloadTier::Standard), Layer::Project)
    );
    assert_eq!(
        effective.model_ceiling("alpha", "big").value,
        None,
        "the project layer replaces the user's map for that provider rather than \
         merging into it — the same replace-not-merge rule `credential_env` follows"
    );
    drop(project_root);
}

// --- GH-CAPABILITY-FACTS: map lines 1517 and 1513 -----------------------

/// A missing `tool_calls` key must leave `declare_tool_calls`'s output
/// byte-identical to before the field existed — the census's mutation
/// (`upgrade-by-association`) is a missing key upgrading to
/// `Verified{true}`, and this is the test that must fail it.
#[test]
fn a_missing_tool_calls_key_leaves_the_templates_declaration_untouched() {
    let config = ProviderConfig::new("openrouter");
    let mut provider = config
        .to_provider("probe")
        .expect("a known template must resolve");
    let before = provider.clone();
    config.declare_tool_calls(&mut provider, Layer::User);

    assert_eq!(
        provider, before,
        "a `ProviderConfig` whose `tool_calls` is `None` must leave `declare_tool_calls`'s \
         output untouched"
    );
    for protocol in &provider.protocols {
        assert_eq!(
            protocol.tool_calls,
            crate::harness::Declared::Unverified,
            "the openrouter template's own tool_calls declaration must survive \
             untouched when nobody configured tool_calls"
        );
    }
}

/// `Some(false)` becomes `Declared::Verified { value: false, .. }` on
/// every protocol the provider serves, citing the layer and the exact
/// `[providers.<name>]` table the declaration came from.
#[test]
fn a_declared_tool_calls_false_becomes_verified_absent_with_a_layer_reason() {
    let mut config = ProviderConfig::new("openrouter");
    config.set_tool_calls(Some(false));
    let mut provider = config
        .to_provider("probe")
        .expect("a known template must resolve");
    config.declare_tool_calls(&mut provider, Layer::Project);

    assert!(
        !provider.protocols.is_empty(),
        "the openrouter template must declare at least one protocol for this to prove \
         anything"
    );
    for protocol in &provider.protocols {
        match protocol.tool_calls {
            crate::harness::Declared::Verified { value, evidence } => {
                assert!(!value, "a declared `Some(false)` must verify absent");
                assert!(
                    evidence.contains("project config") && evidence.contains("[providers]"),
                    "the evidence must name the layer and the [providers] table: {evidence:?}"
                );
            }
            crate::harness::Declared::Unverified => {
                panic!("a declared tool_calls value must not stay Unverified")
            }
        }
    }
}

/// `Some(true)` becomes `Declared::Verified { value: true, .. }` — the
/// same producer, the other declared value.
#[test]
fn a_declared_tool_calls_true_becomes_verified_present_with_a_layer_reason() {
    let mut config = ProviderConfig::new("openrouter");
    config.set_tool_calls(Some(true));
    let mut provider = config
        .to_provider("probe")
        .expect("a known template must resolve");
    config.declare_tool_calls(&mut provider, Layer::User);

    for protocol in &provider.protocols {
        assert_eq!(
            protocol.tool_calls,
            crate::harness::Declared::verified(
                true,
                declared_from_config(Layer::User, DeclaredIn::ProviderToolCalls)
            ),
            "a declared `Some(true)` must verify present, citing the user layer and the \
             [providers.probe] table"
        );
    }
}

/// `resource_facts_of`: an axis absent from a declared model's table
/// stays `Unverified` — a missing key must never upgrade to `Verified`,
/// the same rule `tool_calls` follows above.
#[test]
fn an_axis_absent_from_a_declared_models_table_stays_unverified() {
    let mut config = ProviderConfig::new("openrouter");
    config.set_model_facts(BTreeMap::from([(
        "small".to_owned(),
        ConfiguredModelFacts {
            shell_tool_use: Some(false),
            ..Default::default()
        },
    )]));

    let facts = config.resource_facts_of("small", Layer::User);
    assert_eq!(
        facts.shell_tool_use,
        crate::harness::Declared::verified(
            false,
            declared_from_config(Layer::User, DeclaredIn::ModelFacts)
        )
    );
    assert_eq!(
        facts.code_edit,
        crate::harness::Declared::Unverified,
        "an axis the user never set on a declared model must stay Unverified, not \
         upgrade because a sibling axis was declared"
    );
    assert_eq!(facts.browser_use, crate::harness::Declared::Unverified);
    assert_eq!(facts.large_context, crate::harness::Declared::Unverified);
    assert_eq!(
        facts.fast_cheap_analysis,
        crate::harness::Declared::Unverified
    );
    assert_eq!(
        facts.repository_review,
        crate::harness::Declared::Unverified
    );
    assert_eq!(facts.mcp, crate::harness::Declared::Unverified);
}

/// [`EffectiveConfig::model_facts`]: layered project-over-user exactly as
/// [`EffectiveConfig::model_cost`] and [`EffectiveConfig::model_ceiling`]
/// resolve beside it, and the three shapes of *not established* that
/// must never read as an established absence: an unnamed model, an
/// unconfigured provider, and a provider that declares no facts at all.
#[test]
fn model_facts_is_layered_and_unverified_where_nobody_declared_a_fact() {
    use crate::routing::capability::ResourceFacts;

    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("workspace");
    std::fs::create_dir_all(&root).unwrap();
    let project_root = test_project(&root);

    let mut user = UserConfig::default();
    let mut user_alpha = ProviderConfig::new("openrouter");
    user_alpha.set_model_facts(BTreeMap::from([(
        "small".to_owned(),
        ConfiguredModelFacts {
            shell_tool_use: Some(false),
            ..Default::default()
        },
    )]));
    user.providers_mut().set("alpha", user_alpha);
    // A configured provider that declares no facts at all.
    user.providers_mut()
        .set("beta", ProviderConfig::new("openrouter"));

    let effective = EffectiveConfig::new(&user, None);
    let small = effective.model_facts("alpha", "small");
    assert_eq!(small.layer, Layer::User);
    assert_eq!(
        small.value.shell_tool_use,
        crate::harness::Declared::verified(
            false,
            declared_from_config(Layer::User, DeclaredIn::ModelFacts)
        )
    );
    assert_eq!(
        small.value.code_edit,
        crate::harness::Declared::Unverified,
        "an undeclared axis on a declared model stays Unverified"
    );
    assert_eq!(
        effective.model_facts("alpha", "unnamed").value,
        ResourceFacts::UNVERIFIED,
        "a model nobody declared facts for is not established, not absent"
    );
    assert_eq!(
        effective.model_facts("beta", "small").value,
        ResourceFacts::UNVERIFIED,
        "a provider that declares no facts states nothing about any of its models"
    );
    assert_eq!(
        effective.model_facts("nowhere", "small"),
        Layered::new(ResourceFacts::UNVERIFIED, Layer::Default),
        "a provider nobody configured is not a provider anybody declared facts for"
    );

    // The project layer replaces the user's map for that provider,
    // exactly as `model_ceiling` resolves beside it.
    let mut project = ProjectConfig::default();
    let mut project_alpha = ProviderConfig::new("openrouter");
    project_alpha.set_model_facts(BTreeMap::from([(
        "small".to_owned(),
        ConfiguredModelFacts {
            shell_tool_use: Some(true),
            ..Default::default()
        },
    )]));
    project.providers_mut().set("alpha", project_alpha);
    let effective = EffectiveConfig::new(&user, Some(&project));
    let small = effective.model_facts("alpha", "small");
    assert_eq!(small.layer, Layer::Project);
    assert_eq!(
        small.value.shell_tool_use,
        crate::harness::Declared::verified(
            true,
            declared_from_config(Layer::Project, DeclaredIn::ModelFacts)
        )
    );
    drop(project_root);
}

/// [`EffectiveConfig::configured_provider`]: a project-layer `tool_calls`
/// declaration wins over a user-layer one for the same provider name —
/// the same project-over-user precedence
/// [`EffectiveConfig::model_cost`] and [`EffectiveConfig::model_facts`]
/// apply beside it.
#[test]
fn configured_provider_layers_tool_calls_project_over_user() {
    let mut user = UserConfig::default();
    let mut user_alpha = ProviderConfig::new("openrouter");
    user_alpha.set_tool_calls(Some(false));
    user.providers_mut().set("alpha", user_alpha);

    let mut project = ProjectConfig::default();
    let mut project_alpha = ProviderConfig::new("openrouter");
    project_alpha.set_tool_calls(Some(true));
    project.providers_mut().set("alpha", project_alpha);

    let effective = EffectiveConfig::new(&user, Some(&project));
    let resolved = effective
        .configured_provider("alpha")
        .expect("a configured provider must resolve");
    assert_eq!(resolved.layer, Layer::Project);
    for protocol in &resolved.value.protocols {
        match protocol.tool_calls {
            crate::harness::Declared::Verified { value, evidence } => {
                assert!(
                    value,
                    "the project layer's `tool_calls = true` must win over the user \
                     layer's `false`"
                );
                assert!(
                    evidence.contains("project"),
                    "the evidence must attribute the winning declaration to the \
                     project layer: {evidence:?}"
                );
            }
            crate::harness::Declared::Unverified => {
                panic!("the project layer's declared tool_calls must not read as Unverified")
            }
        }
    }
}
