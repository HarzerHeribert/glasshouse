//! `config::tests`, part B: the `[routing]`-refusal test, memory/firewall,
//! entitlement and model-facts tests.
//!

use super::*;

/// design-decisions.md, 2026-09-16, *Glasshouse never decides which model is
/// used*: a config file that still carries a `[routing]` table is refused by
/// name at load, rather than silently dropped — a build that dropped it
/// quietly would leave a user believing a preference they wrote (a pinned
/// model, a reserve threshold) was still in effect. Checked against both
/// `UserConfig::load` and `load_project_config`, since both route through
/// the same `parse_toml`.
#[test]
fn a_routing_table_is_refused_by_name_naming_the_ruling() {
    let tmp = tempfile::tempdir().unwrap();
    let paths = RuntimePaths::new(tmp.path().join("data"), tmp.path().join("config"));
    std::fs::create_dir_all(paths.user_config_file().parent().unwrap()).unwrap();
    std::fs::write(
        paths.user_config_file(),
        "version = 1\n[routing]\nmax_router_latency_ms = 2000\n",
    )
    .unwrap();

    let err = UserConfig::load(&paths).unwrap_err().to_string();
    assert!(err.contains("[routing]"), "{err}");
    assert!(err.contains("2026-09-16"), "{err}");
    assert!(
        err.contains("Glasshouse never decides which model is used"),
        "{err}"
    );

    let project_root = tmp.path().join("project");
    std::fs::create_dir_all(&project_root).unwrap();
    let project = test_project(&project_root);
    let project_config_path = project_config_path(&project).unwrap();
    std::fs::create_dir_all(project_config_path.parent().unwrap()).unwrap();
    std::fs::write(
        &project_config_path,
        "version = 1\n[routing]\nprefer_free = true\n",
    )
    .unwrap();
    let project_err = load_project_config(&project).unwrap_err().to_string();
    assert!(project_err.contains("[routing]"), "{project_err}");
    assert!(project_err.contains("2026-09-16"), "{project_err}");
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

/// Phase 56 lines 1946 and 1947: `[entitlements.<name>]` round-trips
/// through the loader with the routing types' own spellings, resolves
/// **by name** with the project layer replacing the user's entry whole,
/// supplies an unrestricted default for every harness's own sign-in that
/// nobody claimed, and refuses every unknown spelling rather than reading
/// it as "no rule".
#[test]
fn entitlements_round_trip_and_resolve_project_over_user_with_a_native_default() {
    use crate::config::JobKind;
    use crate::config::WorkloadTier;
    use crate::profile::BackendResource;

    let tmp = tempfile::tempdir().unwrap();
    let paths = RuntimePaths::new(tmp.path().join("data"), tmp.path().join("config"));

    // The accounts are the gateway's since the 2026-09-11 ruling; the two
    // `[entitlements.<name>]` tables below are policy about them.
    let gateway = crate::config::GatewayCatalogue::from_toml(
        "[accounts.max]\nkind = \"claude\"\n\n\
         [accounts.team-key]\nkind = \"api-key\"\nprovider = \"openrouter\"\n",
    )
    .expect("the gateway's catalogue parses");

    let mut user = UserConfig::default();
    assert!(user.entitlements().is_empty());
    let mut max = EntitlementConfig::default();
    max.set_native_harness(Some(IntegrationId::ClaudeCode))
        .set_deny_tiers([WorkloadTier::Leaf])
        .set_allow_job_kinds([JobKind::MemoryExtraction]);
    user.entitlements_mut().set("max", max);
    let mut team = EntitlementConfig::default();
    team.set_allow_harnesses([IntegrationId::Codex])
        .set_deny_harnesses([IntegrationId::ClaudeCode]);
    user.entitlements_mut().set("team-key", team);
    user.save(&paths).unwrap();
    let loaded = UserConfig::load(&paths).unwrap();
    assert_eq!(loaded.entitlements(), user.entitlements());

    // The on-disk spellings are the routing types' own — and not one of the
    // five account keys is among them.
    let text = std::fs::read_to_string(paths.user_config_file()).unwrap();
    for expected in [
        "[entitlements.max]",
        "native_harness = \"claude-code\"",
        "deny_tiers = [\"leaf\"]",
        "allow_job_kinds = [\"memory extraction\"]",
        "[entitlements.team-key]",
        "allow_harnesses = [\"codex\"]",
        "deny_harnesses = [\"claude-code\"]",
    ] {
        assert!(text.contains(expected), "missing `{expected}` in:\n{text}");
    }
    for gateways in [
        "kind =",
        "vendor =",
        "credential =",
        "provider =",
        "subscription_broker =",
    ] {
        assert!(
            !text.contains(gateways),
            "`{gateways}` is the gateway's and must not be written here:\n{text}"
        );
    }

    // The user layer alone: `max` is Claude Code's sign-in, every other
    // harness gets its unrestricted default, and the API key is found by
    // the provider it backs.
    let effective = EffectiveConfig::with_gateway(&loaded, None, &gateway);
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
    let effective = EffectiveConfig::with_gateway(&loaded, Some(&project), &gateway);
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
        Some(EntitlementKind::Claude),
        "a project overlay replaces the policy whole and the account not at all: \
         the plan is the gateway's"
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
///
/// Half of each contradiction is now in the gateway's catalogue — the
/// provider an account is behind is the gateway's since the 2026-09-11
/// ruling — and the other half in Glasshouse's overlay, which is precisely
/// why these can only be seen once the two are resolved together.
#[test]
fn contradictory_entitlement_tables_are_refused_by_name() {
    use crate::profile::BackendResource;

    let gateway = |text: &str| {
        crate::config::GatewayCatalogue::from_toml(text).expect("the gateway catalogue parses")
    };
    let user = |text: &str| toml::from_str::<UserConfig>(text).expect("the overlay parses");

    let both_gw = gateway("[accounts.x]\nprovider = \"openrouter\"\n");
    let both = user("version = 1\n\n[entitlements.x]\nnative_harness = \"codex\"\n");
    let err = EffectiveConfig::with_gateway(&both, None, &both_gw)
        .entitlements()
        .unwrap_err();
    assert!(matches!(err, EntitlementLookupError::TwoBackings { ref name } if name == "x"));

    let empty_gw = gateway("");
    let two_claim = user(
        "version = 1\n\n[entitlements.a]\nnative_harness = \"codex\"\n\n\
         [entitlements.b]\nnative_harness = \"codex\"\n",
    );
    let err = EffectiveConfig::with_gateway(&two_claim, None, &empty_gw)
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
        EffectiveConfig::with_gateway(&two_claim, None, &empty_gw)
            .entitlement_for(IntegrationId::ClaudeCode, &BackendResource::Native)
            .is_ok()
    );

    let two_providers = gateway(
        "[accounts.a]\nprovider = \"openrouter\"\n\n[accounts.b]\nprovider = \"openrouter\"\n",
    );
    let none = UserConfig::default();
    let err = EffectiveConfig::with_gateway(&none, None, &two_providers)
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

    let reserved = gateway("[accounts.codex]\nprovider = \"openrouter\"\n");
    let err = EffectiveConfig::with_gateway(&none, None, &reserved)
        .entitlements()
        .unwrap_err();
    assert!(
        matches!(&err, EntitlementLookupError::NameReservedForHarness { name, harness: IntegrationId::Codex } if name == "codex"),
        "{err}"
    );

    // An account that names neither backing is listed and matches nothing.
    let unstated = gateway("[accounts.someday]\nkind = \"gemini\"\n");
    let all = EffectiveConfig::with_gateway(&none, None, &unstated)
        .entitlements()
        .unwrap();
    let someday = all.iter().find(|s| s.name() == "someday").unwrap();
    assert_eq!(someday.backing(), &EntitlementBacking::Unstated);
    assert_eq!(someday.describe(), "Gemini plan, no backing stated");

    // And an overlay with no account behind it, and no harness sign-in of
    // its own, is refused by name — the 2026-09-11 ruling's own refusal.
    let orphan = user("version = 1\n\n[entitlements.nothing]\ndeny_tiers = [\"leaf\"]\n");
    let err = EffectiveConfig::with_gateway(&orphan, None, &empty_gw)
        .entitlements()
        .unwrap_err();
    assert!(
        matches!(&err, EntitlementLookupError::UnknownAccount { name, .. } if name == "nothing"),
        "{err}"
    );
}

/// Every [`crate::config::JobKind`] is listed in
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
/// [`crate::config::WorkloadTier`] is listed in
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
    use crate::config::WorkloadTier;

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
        Some(crate::config::WorkloadTier::Leaf)
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
    use crate::config::WorkloadTier;

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
    use crate::config::ResourceFacts;

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
