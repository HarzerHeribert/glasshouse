//! Real-filesystem coverage for the Settings overlay's save paths.
//!
//! `shell::state`'s own tests prove the keymap and the in-memory model;
//! these prove the two functions that actually touch disk —
//! `shell::save_user_settings` and `shell::save_project_settings` — against a
//! real project directory and a real user config directory, per the six
//! invariants in `docs/product/design-decisions.md`'s "Settings" section.

use clap::Parser;

use glasshouse::config;
use glasshouse::integrations::IntegrationId;
use glasshouse::shell::{self, MemorySettingsEdit, RoutingSettingsEdit, SettingsEdit};
use glasshouse::{Cli, Runtime, bootstrap};

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

fn new_workspace() -> tempfile::TempDir {
    let workspace = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(workspace.path().join(".git")).unwrap();
    workspace
}

/// Invariant: "Cancelling a project-level write creates no file and no
/// directory."
///
/// This drives the keys and then acts on what the state machine returned,
/// exactly as `shell::run`'s event loop does — which is the only way the
/// assertion means anything. An earlier version of this test asserted that an
/// untouched workspace was untouched without ever invoking the cancel path;
/// mutating `W` to save immediately, with no confirmation at all, left it
/// green.
#[test]
fn cancelling_a_project_level_save_creates_no_file_and_no_directory() {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use glasshouse::shell::{Action, HarnessRow, ShellState};

    for cancel in [KeyCode::Esc, KeyCode::Char('n')] {
        let workspace = new_workspace();
        let data = tempfile::tempdir().unwrap();
        let runtime = runtime_for(workspace.path(), data.path());
        let root = runtime.project().display_root();

        let mut state = ShellState::new("p", &root, "0.1.0", Vec::new());
        state.open_settings(
            vec![HarnessRow {
                id: IntegrationId::ClaudeCode,
                detected: true,
                enabled: false,
                enabled_layer: config::Layer::Default,
                executable: None,
                executable_layer: None,
            }],
            Vec::new(),
            Vec::new(),
            Vec::new(),
        );

        // Stage a real edit, so there is genuinely something a save could write,
        // then ask for a project-level save and change your mind.
        //
        // Every action is acted on, exactly as the run loop does — not just the
        // last one. The first version of this test kept only the answer to the
        // cancel key and threw away the answer to `W`, which is precisely where
        // a missing confirmation would save. It passed under that mutation.
        let mut asked_to_write = false;
        for key in [
            KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE),
            KeyEvent::new(KeyCode::Char('W'), KeyModifiers::SHIFT),
            KeyEvent::new(cancel, KeyModifiers::NONE),
        ] {
            if state.handle_key(key) == Action::SaveProjectSettings {
                asked_to_write = true;
                let _ = shell::save_project_settings(&runtime, &state.settings_edits(), &[], &[]);
            }
        }

        assert!(
            !asked_to_write,
            "cancelling with {cancel:?} still asked for a project write"
        );
        assert!(
            !root.join(".glasshouse").exists(),
            "cancelling with {cancel:?} left `.glasshouse` behind in the repository"
        );
    }
}

/// Invariant: "Confirming creates exactly that one file, and it parses
/// back."
#[test]
fn confirming_a_project_level_save_creates_exactly_one_file_that_parses_back() {
    let workspace = new_workspace();
    let data = tempfile::tempdir().unwrap();
    let runtime = runtime_for(workspace.path(), data.path());

    let edits = vec![SettingsEdit {
        id: IntegrationId::ClaudeCode,
        enabled: Some(true),
        executable: Some(Some(std::path::PathBuf::from("/opt/bin/claude"))),
    }];

    let path = shell::save_project_settings(&runtime, &edits, &[], &[]).expect("save must succeed");
    // Compared against the runtime's own (canonicalized) display root rather
    // than `workspace.path()` directly: on macOS `/tmp` is itself a symlink
    // to `/private/tmp`, and `Project::discover` canonicalizes the root —
    // the same reason `integrations::doctor_report` prints `display_root`
    // rather than the raw input path.
    assert_eq!(
        path,
        runtime
            .project()
            .display_root()
            .join(".glasshouse")
            .join("config.toml")
    );
    assert!(path.is_file());

    let entries: Vec<_> = std::fs::read_dir(runtime.project().display_root().join(".glasshouse"))
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    assert_eq!(
        entries,
        vec![std::ffi::OsString::from("config.toml")],
        "exactly one file, no leftover temp file"
    );

    let loaded = config::load_project_config(runtime.project())
        .unwrap()
        .expect("the file must parse back");
    assert_eq!(
        loaded.integrations().is_enabled(IntegrationId::ClaudeCode),
        Some(true)
    );
    assert_eq!(
        loaded
            .integrations()
            .get(IntegrationId::ClaudeCode)
            .unwrap()
            .executable(),
        Some(std::path::Path::new("/opt/bin/claude"))
    );
}

/// Invariant: "A user-level edit never writes into the project root."
#[test]
fn a_user_level_save_never_writes_inside_the_project_root() {
    let workspace = new_workspace();
    let data = tempfile::tempdir().unwrap();
    let runtime = runtime_for(workspace.path(), data.path());

    let edits = vec![SettingsEdit {
        id: IntegrationId::Codex,
        enabled: Some(false),
        executable: None,
    }];
    shell::save_user_settings(&runtime, &edits, &[], &[]).expect("save must succeed");

    assert!(
        !workspace.path().join(".glasshouse").exists(),
        "a user-level save must never touch the project root"
    );

    let loaded = glasshouse::config::UserConfig::load(runtime.paths()).unwrap();
    assert_eq!(
        loaded.integrations().is_enabled(IntegrationId::Codex),
        Some(false)
    );
}

/// A save only ever applies the fields an edit actually named, so a harness
/// the user never touched keeps whatever the project layer already said
/// about it instead of being silently overwritten by a user-level write.
#[test]
fn a_save_only_touches_the_fields_an_edit_actually_named() {
    let workspace = new_workspace();
    let data = tempfile::tempdir().unwrap();
    let runtime = runtime_for(workspace.path(), data.path());

    let mut existing = glasshouse::config::UserConfig::load(runtime.paths()).unwrap();
    existing
        .integrations_mut()
        .entry(IntegrationId::Codex)
        .set_enabled(true)
        .set_executable(Some(std::path::PathBuf::from("/usr/local/bin/codex")));
    existing.save(runtime.paths()).unwrap();

    // Only toggling `enabled`; the executable override must survive.
    let edits = vec![SettingsEdit {
        id: IntegrationId::Codex,
        enabled: Some(false),
        executable: None,
    }];
    shell::save_user_settings(&runtime, &edits, &[], &[]).expect("save must succeed");

    let loaded = glasshouse::config::UserConfig::load(runtime.paths()).unwrap();
    assert_eq!(
        loaded.integrations().is_enabled(IntegrationId::Codex),
        Some(false)
    );
    assert_eq!(
        loaded
            .integrations()
            .get(IntegrationId::Codex)
            .unwrap()
            .executable(),
        Some(std::path::Path::new("/usr/local/bin/codex")),
        "an untouched field must not be clobbered by an unrelated edit"
    );
}

/// Routing follows the same two explicit save paths as every other Settings
/// section, and its per-field edit shape must not promote untouched values.
#[test]
fn routing_edits_persist_to_the_chosen_layer_without_clobbering_siblings() {
    use glasshouse::config::{
        EffectiveConfig, Layer, PremiumReservePercent, RouterCostMicroUsd, RouterLatencyMs,
        RoutingModelChoice,
    };

    let workspace = new_workspace();
    let data = tempfile::tempdir().unwrap();
    let runtime = runtime_for(workspace.path(), data.path());

    let mut user = config::UserConfig::load(runtime.paths()).unwrap();
    user.routing_mut()
        .set_model(Some(RoutingModelChoice::Automatic))
        .set_max_router_latency(Some(RouterLatencyMs::try_from(1_500).unwrap()));
    user.save(runtime.paths()).unwrap();

    let user_edit = RoutingSettingsEdit {
        max_cost: Some(RouterCostMicroUsd::try_from(2_500).unwrap()),
        prefer_free: Some(false),
        ..RoutingSettingsEdit::default()
    };
    shell::save_user_settings_with_routing(&runtime, &[], &[], &[], Some(&user_edit), None)
        .unwrap();
    assert!(
        !runtime
            .project()
            .display_root()
            .join(".glasshouse")
            .exists(),
        "a user routing save wrote into the project"
    );
    let user = config::UserConfig::load(runtime.paths()).unwrap();
    assert_eq!(user.routing().model(), Some(&RoutingModelChoice::Automatic));
    assert_eq!(user.routing().max_router_latency().unwrap().get(), 1_500);
    assert_eq!(user.routing().max_marginal_cost().unwrap().get(), 2_500);
    assert_eq!(user.routing().prefer_free(), Some(false));

    let project_edit = RoutingSettingsEdit {
        max_latency: Some(RouterLatencyMs::try_from(350).unwrap()),
        premium_reserve: Some(PremiumReservePercent::try_from(12).unwrap()),
        ..RoutingSettingsEdit::default()
    };
    let path = shell::save_project_settings_with_routing(
        &runtime,
        &[],
        &[],
        &[],
        Some(&project_edit),
        None,
    )
    .unwrap();
    assert!(path.is_file());
    let project = config::load_project_config(runtime.project())
        .unwrap()
        .expect("project routing config");
    let effective = EffectiveConfig::new(&user, Some(&project));
    assert_eq!(effective.max_router_latency().layer, Layer::Project);
    assert_eq!(effective.max_router_latency().value.get(), 350);
    assert_eq!(effective.max_router_cost().layer, Layer::User);
    assert_eq!(effective.max_router_cost().value.get(), 2_500);
    assert_eq!(effective.prefer_free_routing().layer, Layer::User);
    assert!(!effective.prefer_free_routing().value);
    assert_eq!(effective.premium_reserve().layer, Layer::Project);
    assert_eq!(effective.premium_reserve().value.get(), 12);
}

/// Memory follows the same two explicit save paths as every other Settings
/// section, and its single-field edit must not touch routing config it never
/// named — the sibling half of the "only the named field" guarantee
/// `a_save_only_touches_the_fields_an_edit_actually_named` already proves for
/// harnesses.
#[test]
fn memory_edit_persists_to_the_chosen_layer_without_clobbering_sibling_routing_fields() {
    use glasshouse::config::{EffectiveConfig, Layer, Layered, RoutingModelChoice};

    let workspace = new_workspace();
    let data = tempfile::tempdir().unwrap();
    let runtime = runtime_for(workspace.path(), data.path());

    let mut user = config::UserConfig::load(runtime.paths()).unwrap();
    user.routing_mut()
        .set_model(Some(RoutingModelChoice::Automatic));
    user.save(runtime.paths()).unwrap();

    // Premise, per §17: before the edit, this layer has never decided
    // memory_extraction — or a later assertion that it became `Some(false)`
    // proves nothing about the edit.
    let before = config::UserConfig::load(runtime.paths()).unwrap();
    assert_eq!(before.memory_extraction(), None);

    let user_edit = MemorySettingsEdit {
        memory_extraction: Some(false),
    };
    shell::save_user_settings_with_routing(&runtime, &[], &[], &[], None, Some(&user_edit))
        .unwrap();
    assert!(
        !runtime
            .project()
            .display_root()
            .join(".glasshouse")
            .exists(),
        "a user memory save wrote into the project"
    );
    let user = config::UserConfig::load(runtime.paths()).unwrap();
    assert_eq!(user.memory_extraction(), Some(false));
    assert_eq!(
        user.routing().model(),
        Some(&RoutingModelChoice::Automatic),
        "a memory-only edit must not clobber the routing model it never named"
    );

    let project_edit = MemorySettingsEdit {
        memory_extraction: Some(true),
    };
    let path = shell::save_project_settings_with_routing(
        &runtime,
        &[],
        &[],
        &[],
        None,
        Some(&project_edit),
    )
    .unwrap();
    assert!(path.is_file());
    let project = config::load_project_config(runtime.project())
        .unwrap()
        .expect("project memory config");
    let effective = EffectiveConfig::new(&user, Some(&project));
    assert_eq!(
        effective.memory_extraction_enabled(),
        Layered::new(true, Layer::Project),
        "a project's explicit re-enable must win over the user's disable"
    );
}

// -----------------------------------------------------------------------
// Phase 9I: free resources in configuration and in Settings.
// -----------------------------------------------------------------------

/// Acceptance 1 — a model marked free-tier survives a save and a load.
#[test]
fn a_model_marked_free_tier_survives_a_save_and_a_load() {
    let workspace = new_workspace();
    let data = tempfile::tempdir().unwrap();
    let runtime = runtime_for(workspace.path(), data.path());

    let mut user = config::UserConfig::load(runtime.paths()).unwrap();
    let mut provider = config::ProviderConfig::new("openrouter");
    provider.set_free_models(vec!["nvidia/nemotron-nano-9b-v2:free".to_owned()]);
    user.providers_mut().set("openrouter", provider);
    user.save(runtime.paths()).unwrap();

    let loaded = config::UserConfig::load(runtime.paths()).unwrap();
    let loaded_provider = loaded.providers().get("openrouter").unwrap();
    assert_eq!(
        loaded_provider.free_models(),
        &["nvidia/nemotron-nano-9b-v2:free".to_owned()]
    );
    assert_eq!(
        loaded_provider.cost_of("nvidia/nemotron-nano-9b-v2:free"),
        glasshouse::routing::Cost::Free
    );
}

/// Acceptance 2 — a model nobody marked answers `Cost::Metered`, and no
/// `:free` suffix or any other spelling changes that. The fail-closed
/// direction: a router that guessed "free" and was wrong spends the user's
/// money.
#[test]
fn an_unmarked_model_is_metered_and_a_free_looking_name_changes_nothing() {
    let provider = config::ProviderConfig::new("openrouter");
    for model in [
        "nvidia/nemotron-nano-9b-v2:free",
        "z-ai/glm-4.5-air:free",
        "plain-metered-model",
        "",
    ] {
        assert_eq!(
            provider.cost_of(model),
            glasshouse::routing::Cost::Metered,
            "`{model}` must be metered until the user marks it, whatever its own name suggests"
        );
    }
}

/// Acceptance 3 — a configuration file written without the new fields loads
/// with empty preferences and no error.
#[test]
fn a_config_file_written_before_free_resources_existed_loads_with_empty_preferences() {
    let workspace = new_workspace();
    let data = tempfile::tempdir().unwrap();
    let runtime = runtime_for(workspace.path(), data.path());

    std::fs::write(
        runtime.paths().user_config_file(),
        "[providers.openrouter]\ntemplate = \"openrouter\"\ncredential_env = [\"OPENROUTER_API_KEY\"]\n",
    )
    .unwrap();

    let loaded = config::UserConfig::load(runtime.paths()).expect("must load without error");
    let provider = loaded.providers().get("openrouter").unwrap();
    assert!(provider.free_models().is_empty());
    assert_eq!(
        provider.cost_of("anything"),
        glasshouse::routing::Cost::Metered
    );

    let preferences = loaded.routing().free_preferences();
    assert!(preferences.order().is_empty());
    assert!(preferences.disabled().is_empty());
    assert_eq!(preferences.pin(), None);
}

/// A config file predating the Memory section — same file as Acceptance 3 —
/// loads without error, with the memory-extraction setting unset at this
/// layer (`None`) and resolving through `EffectiveConfig` to its documented
/// default: enabled, at `Layer::Default`.
#[test]
fn a_config_file_written_before_the_memory_section_existed_loads_with_the_default() {
    use glasshouse::config::{EffectiveConfig, Layer, Layered};

    let workspace = new_workspace();
    let data = tempfile::tempdir().unwrap();
    let runtime = runtime_for(workspace.path(), data.path());

    std::fs::write(
        runtime.paths().user_config_file(),
        "[providers.openrouter]\ntemplate = \"openrouter\"\ncredential_env = [\"OPENROUTER_API_KEY\"]\n",
    )
    .unwrap();

    let loaded = config::UserConfig::load(runtime.paths()).expect("must load without error");
    assert_eq!(loaded.memory_extraction(), None);

    let effective = EffectiveConfig::new(&loaded, None);
    assert_eq!(
        effective.memory_extraction_enabled(),
        Layered::new(true, Layer::Default),
        "nothing recorded anywhere must resolve to enabled"
    );
}

/// Acceptance 4 — the user's order, disabled list and pin round-trip, and a
/// pin naming a provider that is no longer configured degrades visibly
/// rather than failing, through the frozen routing policy —
/// `RoutingModelChoice::resolve`'s own reasoning, applied to a free-resource
/// pin.
#[test]
fn free_resource_order_disabled_and_pin_round_trip_and_a_stale_pin_degrades_visibly() {
    use glasshouse::config::FreeResourceRef;
    use glasshouse::routing::disposable::{
        DisposableCandidate, DisposableRouting, JobKind, NoResource,
    };
    use glasshouse::routing::free::FreePool;
    use glasshouse::routing::{Cost, CredentialId};
    use glasshouse::secret::SecretRef;

    let workspace = new_workspace();
    let data = tempfile::tempdir().unwrap();
    let runtime = runtime_for(workspace.path(), data.path());

    let mut user = config::UserConfig::load(runtime.paths()).unwrap();
    user.routing_mut()
        .set_free_resource_order(Some(vec![
            FreeResourceRef::new("nous", "c-model"),
            FreeResourceRef::new("openrouter", "b-model"),
        ]))
        .set_free_resource_disabled(Some(vec![FreeResourceRef::new("openrouter", "a-model")]))
        .set_free_resource_pin(Some(FreeResourceRef::new(
            "vanished-provider",
            "gone-model",
        )));
    user.save(runtime.paths()).unwrap();

    let loaded = config::UserConfig::load(runtime.paths()).unwrap();
    assert_eq!(
        loaded.routing().free_resource_order().unwrap(),
        &[
            FreeResourceRef::new("nous", "c-model"),
            FreeResourceRef::new("openrouter", "b-model"),
        ]
    );
    assert_eq!(
        loaded.routing().free_resource_disabled().unwrap(),
        &[FreeResourceRef::new("openrouter", "a-model")]
    );
    assert_eq!(
        loaded.routing().free_resource_pin(),
        Some(&FreeResourceRef::new("vanished-provider", "gone-model"))
    );

    // The stale pin was never validated against configured providers at
    // load time — loading did not fail — and the frozen routing policy is
    // where it degrades, visibly, rather than silently substituting another
    // free resource.
    let preferences = loaded.routing().free_preferences();
    let routing = DisposableRouting::for_support_work(true, preferences);
    let candidate = DisposableCandidate::new(
        "openrouter",
        "b-model",
        CredentialId::new(
            "openrouter",
            SecretRef::Environment {
                var: "OPENROUTER_API_KEY".to_owned(),
            },
        ),
        Cost::Free,
    );
    let err = routing
        .choose(
            JobKind::Classification,
            &[candidate],
            &FreePool::new(),
            std::time::Instant::now(),
            None,
        )
        .expect_err(
            "a pin naming a provider nobody configured must not silently substitute another \
             resource",
        );
    assert!(matches!(err, NoResource::PinnedResourceUnavailable { .. }));
}

/// GH-PROFILE-ENABLED acceptance test 2: a disabled launch profile is still
/// **listed**, so a person can find it and turn it back on.
///
/// This is the constraint that decided where the enabled filter went. The
/// obvious fix for "a disabled profile is still a routing candidate" is to
/// filter inside `EffectiveConfig::profile_names`, and it is wrong: that
/// accessor means *every configured profile name*, this screen is what a
/// person uses to re-enable one, and a profile filtered out of the list
/// cannot be re-enabled from anywhere. `disable is not delete` — the rule
/// `ProfileConfig::enabled`'s own doc names — needs both halves, and the
/// routing half is worthless if it takes this one with it.
///
/// The Settings list is the **only** surface that enumerates launch
/// profiles: `shell::build_settings` merges the two profile tables itself
/// rather than calling `profile_names` (its own comment says why — the
/// implied Native profile has no `ProfileConfig` to show), and `glasshouse
/// doctor` does not list profiles at all.
///
/// # Rendered twice, at two widths
///
/// Practice §17: the row is a fixed-column format and `enabled`/`disabled` is
/// its **last** column, so a narrow viewport clips exactly the word under
/// test. 100 columns is what a person sees; 200 is where nothing can be
/// truncated. A build that rendered the label correctly and a build that
/// dropped the row entirely differ at both widths, but only the wide one can
/// tell "shows disabled" from "shows nothing after the approval column".
#[test]
fn a_disabled_launch_profile_is_still_listed_in_settings_so_it_can_be_re_enabled() {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use glasshouse::config::{Layer, ProfileConfig};
    use glasshouse::integrations::IntegrationId;
    use glasshouse::shell::{HarnessRow, ProfileRow, ProviderRow, ShellState};

    let mut parked = ProfileConfig::new(IntegrationId::ClaudeCode);
    parked.set_enabled(false);
    let mut running = ProfileConfig::new(IntegrationId::ClaudeCode);
    running.set_enabled(true);

    let mut state = ShellState::new("p", "/work/p", "0.1.0", Vec::new());
    state.open_settings(
        Vec::<HarnessRow>::new(),
        Vec::new(),
        Vec::<ProviderRow>::new(),
        vec![
            ProfileRow {
                name: "parked-one".to_owned(),
                config: parked,
                layer: Layer::User,
            },
            ProfileRow {
                name: "running-one".to_owned(),
                config: running,
                layer: Layer::User,
            },
        ],
    );
    // Harnesses, Integrations, Providers, Launch Profiles.
    for _ in 0..3 {
        state.handle_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
    }

    for width in [100, 200] {
        let text = rendered_settings(&state, width, 30);
        assert!(
            text.contains("parked-one"),
            "a disabled profile must still be listed at {width} columns, or there is nowhere \
             to re-enable it from:\n{text}"
        );
        assert!(
            text.contains("running-one"),
            "and so must its enabled sibling, so the assertion above is about listing rather \
             than about this screen rendering at all:\n{text}"
        );
    }

    // The word itself, only where it cannot be truncated away.
    let wide = rendered_settings(&state, 200, 30);
    assert!(
        wide.contains("disabled"),
        "and it must be shown *as* disabled — a row listed with no state is a profile a \
         person cannot tell is off:\n{wide}"
    );
}

fn rendered_settings(state: &glasshouse::shell::ShellState, width: u16, height: u16) -> String {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    let backend = TestBackend::new(width, height);
    let mut terminal = Terminal::new(backend).expect("terminal");
    terminal
        .draw(|frame| glasshouse::shell::view::render(state, frame))
        .expect("draw must not panic");
    let buffer = terminal.backend().buffer();
    (0..buffer.area.height)
        .map(|y| {
            (0..buffer.area.width)
                .map(|x| buffer[(x, y)].symbol())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Acceptance 5 — the Routing settings screen renders the reason for the
/// resource in use, for each of the three `UseReason` values, using that
/// type's own words — there is exactly one spelling of those three phrases
/// and it is `UseReason::Display`'s.
#[test]
fn routing_settings_render_the_disposable_choice_reason_in_the_types_own_words() {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use glasshouse::routing::disposable::{DisposableCandidate, DisposableRouting, JobKind};
    use glasshouse::routing::free::{FreePool, FreePreferences, FreeResource, WorkloadOutcome};
    use glasshouse::routing::{Cost, CredentialId, UseReason};
    use glasshouse::secret::SecretRef;
    use glasshouse::shell::{HarnessRow, ProfileRow, ProviderRow, ShellState};

    fn credential(provider: &str) -> CredentialId {
        CredentialId::new(
            provider,
            SecretRef::Environment {
                var: format!("{}_API_KEY", provider.to_uppercase()),
            },
        )
    }

    for reason in [
        UseReason::UserPreference,
        UseReason::QuotaPreservation,
        UseReason::Fallback,
    ] {
        let (routing, candidates, pool) = match reason {
            UseReason::UserPreference => (
                DisposableRouting::for_support_work(true, FreePreferences::new()),
                vec![DisposableCandidate::new(
                    "openrouter",
                    "a-free-model",
                    credential("openrouter"),
                    Cost::Free,
                )],
                FreePool::new(),
            ),
            UseReason::QuotaPreservation => (
                DisposableRouting::for_support_work(false, FreePreferences::new()),
                vec![DisposableCandidate::new(
                    "openrouter",
                    "a-free-model",
                    credential("openrouter"),
                    Cost::Free,
                )],
                FreePool::new(),
            ),
            UseReason::Fallback => {
                let mut pool = FreePool::new();
                let first_credential = credential("openrouter");
                for _ in 0..2 {
                    pool.observe(
                        &FreeResource::new(first_credential.clone(), "first-model"),
                        WorkloadOutcome::CapacityFailure,
                        std::time::Instant::now(),
                    );
                }
                (
                    DisposableRouting::for_support_work(true, FreePreferences::new()),
                    vec![
                        DisposableCandidate::new(
                            "openrouter",
                            "first-model",
                            first_credential,
                            Cost::Free,
                        ),
                        DisposableCandidate::new(
                            "openrouter",
                            "second-model",
                            credential("openrouter"),
                            Cost::Free,
                        ),
                    ],
                    pool,
                )
            }
        };

        let choice = routing
            .choose(
                JobKind::Classification,
                &candidates,
                &pool,
                std::time::Instant::now(),
                None,
            )
            .expect("configured");
        assert_eq!(choice.reason(), reason);

        let mut state = ShellState::new("p", "/work/p", "0.1.0", Vec::new());
        state.open_settings(
            Vec::<HarnessRow>::new(),
            Vec::new(),
            Vec::<ProviderRow>::new(),
            Vec::<ProfileRow>::new(),
        );
        state.record_disposable_choice(choice);
        // Harnesses, Integrations, Providers, Launch Profiles, Routing.
        for _ in 0..4 {
            state.handle_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
        }

        let text = rendered_settings(&state, 100, 30);
        assert!(
            text.contains(reason.as_str()),
            "the Routing screen must show `{}` in `UseReason`'s own words:\n{text}",
            reason.as_str()
        );
    }
}

/// The Memory section shows the current automatic-memory-extraction setting
/// and its layer, using the same `layer_label` treatment as every other
/// section, and an edit both flips the value and promotes its layer to
/// `(user)`. The "not available in this build" placeholder Phase 2D line 190
/// leaves behind must be gone.
#[test]
fn the_memory_extraction_setting_renders_its_value_and_layer_and_an_edit_changes_both() {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use glasshouse::shell::{HarnessRow, ProfileRow, ProviderRow, ShellState};

    let mut state = ShellState::new("p", "/work/p", "0.1.0", Vec::new());
    state.open_settings(
        Vec::<HarnessRow>::new(),
        Vec::new(),
        Vec::<ProviderRow>::new(),
        Vec::<ProfileRow>::new(),
    );
    // Harnesses -> Integrations -> Providers -> Launch Profiles -> Routing -> Memory.
    for _ in 0..5 {
        state.handle_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
    }

    // Premise, per §17: the row starts enabled at `Layer::Default` — or a
    // later assertion that a toggle changed it to "no"/`(user)` proves
    // nothing about the toggle.
    let text = rendered_settings(&state, 100, 30);
    assert!(text.contains("Memory"), "{text}");
    assert!(text.contains("yes (default)"), "{text}");
    assert!(
        !text.contains("not available in this build"),
        "the placeholder text must be gone:\n{text}"
    );

    state.handle_key(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE));
    let toggled = rendered_settings(&state, 100, 30);
    assert!(toggled.contains("no (user)"), "{toggled}");
}

/// Acceptance 6 — a credential value planted in the environment never
/// appears in any Providers or Routing render, at a realistic width **and**
/// at 400 columns, while exercising the two editors this batch adds: a
/// provider's free-model markings and the Routing section's free-resource
/// preferences.
#[test]
fn no_credential_value_leaks_through_the_free_resource_editors() {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use glasshouse::config::Layer;
    use glasshouse::shell::{HarnessRow, ProfileRow, ProviderRow, ShellState};

    const VAR: &str = "GLASSHOUSE_FREE_RESOURCE_TEST_ONLY_SECRET_VAR";
    const SECRET_VALUE: &str = "sk-free-resource-test-totally-real-looking-secret-xyz123";

    // SAFETY: `VAR` is unique to this test and removed again below.
    unsafe {
        std::env::set_var(VAR, SECRET_VALUE);
    }

    let mut provider_config = config::ProviderConfig::new("openrouter");
    provider_config.set_credential_env(vec![VAR.to_owned()]);
    provider_config.set_free_models(vec!["nvidia/nemotron-nano-9b-v2:free".to_owned()]);
    let providers = vec![ProviderRow::new(
        "secret-test",
        provider_config,
        Layer::User,
    )];

    let mut state = ShellState::new("p", "/work/p", "0.1.0", Vec::new());
    state.open_settings(
        Vec::<HarnessRow>::new(),
        Vec::new(),
        providers,
        Vec::<ProfileRow>::new(),
    );

    let press = |state: &mut ShellState, code: KeyCode| {
        state.handle_key(KeyEvent::new(code, KeyModifiers::NONE));
    };

    let mut screens = Vec::new();

    // Providers: the free-model editor open on the planted provider.
    press(&mut state, KeyCode::Tab);
    press(&mut state, KeyCode::Tab);
    screens.push(rendered_settings(&state, 100, 30));
    screens.push(rendered_settings(&state, 400, 60));
    press(&mut state, KeyCode::Char('f'));
    for c in "another-free-model".chars() {
        press(&mut state, KeyCode::Char(c));
    }
    screens.push(rendered_settings(&state, 100, 30));
    screens.push(rendered_settings(&state, 400, 60));
    press(&mut state, KeyCode::Enter);
    screens.push(rendered_settings(&state, 100, 30));
    screens.push(rendered_settings(&state, 400, 60));

    // Routing: the order, disabled and pin editors, each typed and confirmed.
    press(&mut state, KeyCode::Tab);
    press(&mut state, KeyCode::Tab);
    for (key, typed) in [
        (
            KeyCode::Char('o'),
            "openrouter:a-free-model,nous:b-free-model",
        ),
        (KeyCode::Char('d'), "openrouter:c-free-model"),
        (KeyCode::Char('n'), "openrouter:a-free-model"),
    ] {
        press(&mut state, key);
        for c in typed.chars() {
            press(&mut state, KeyCode::Char(c));
        }
        screens.push(rendered_settings(&state, 100, 30));
        screens.push(rendered_settings(&state, 400, 60));
        press(&mut state, KeyCode::Enter);
        screens.push(rendered_settings(&state, 100, 30));
        screens.push(rendered_settings(&state, 400, 60));
    }

    // SAFETY: matches the `set_var` above.
    unsafe {
        std::env::remove_var(VAR);
    }

    for screen in &screens {
        assert!(
            !screen.contains(SECRET_VALUE),
            "the planted credential's value leaked into a render:\n{screen}"
        );
    }
    // The variable's NAME must still be shown — the redaction is of the
    // value, never of the reference to it.
    assert!(
        screens.iter().any(|screen| screen.contains(VAR)),
        "the credential variable name must still be shown somewhere"
    );
}
