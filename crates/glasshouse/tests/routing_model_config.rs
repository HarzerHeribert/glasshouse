//! Phase 34B/34C/34D — the routing-model role as a configuration surface.
//!
//! Every test here enters through a function the shipped binary actually
//! calls: `shell::save_user_settings_with_routing` (the Settings overlay's
//! `W` save, `shell/mod.rs:294`), `config::EffectiveConfig::routing_model`
//! (`shell::build_settings`'s own source for the Routing row,
//! `shell/mod.rs:1668`), `onboarding::WizardState::routing_selection` (what
//! `routing_step` renders during the real onboarding wizard, and what
//! `WizardState::new` seeds directly from a loaded `UserConfig`), and
//! `shell::view::render` (the exact frame the terminal draws).
//!
//! What is deliberately *not* covered: `routing::classify::TaskClassification`
//! and `EffectiveConfig::routing_model_resolution` have no caller outside
//! `#[cfg(test)]` anywhere in this crate — see `PACKET ERRORS` in this
//! package's report. A mechanism with no production caller does not get a
//! test that pretends it has one.

use std::path::PathBuf;

use clap::Parser;
use glasshouse::config::{self, EffectiveConfig, Layer, Layered, RoutingModelChoice, UserConfig};
use glasshouse::onboarding::WizardState;
use glasshouse::shell::{
    self, HarnessRow, IntegrationRow, ProfileRow, ProviderRow, RoutingRow, RoutingSettingsEdit,
    ShellState,
};
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

fn rendered_settings(state: &ShellState, width: u16, height: u16) -> String {
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

/// Phase 34B line 1413 — the routing-model role is a distinct config
/// surface, not folded into memory extraction or any other automatic
/// behaviour's field.
///
/// Proved by equality rather than by checking a couple of fields by hand:
/// `expected` is a clone of the pre-save config with only its routing field
/// touched, so if the save path ever entangled routing with anything else
/// in `UserConfig` — memory extraction, integrations, providers, profiles,
/// response, pairing — this assertion is where it would show up.
#[test]
fn routing_model_choice_is_a_distinct_config_surface_persisted_independently() {
    let workspace = new_workspace();
    let data = tempfile::tempdir().unwrap();
    let runtime = runtime_for(workspace.path(), data.path());

    let before = UserConfig::load(runtime.paths()).unwrap();
    let mut expected = before.clone();
    expected
        .routing_mut()
        .set_model(Some(RoutingModelChoice::Pinned {
            provider: "openrouter".to_owned(),
            model: "claude-tier".to_owned(),
        }));

    let edit = RoutingSettingsEdit {
        model: Some(RoutingModelChoice::Pinned {
            provider: "openrouter".to_owned(),
            model: "claude-tier".to_owned(),
        }),
        ..Default::default()
    };
    shell::save_user_settings_with_routing(&runtime, &[], &[], &[], Some(&edit)).unwrap();

    let after = UserConfig::load(runtime.paths()).unwrap();
    assert_eq!(
        after, expected,
        "setting the routing model must change nothing else in the saved file"
    );
}

/// Phase 34B lines 1414/1415/1416/1417 — a pinned routing model names *any*
/// configured provider and *any* model string: a remote paid resource, a
/// free-tier remote resource, a local one, or a specific model such as
/// GPT-5.6 Luna. `RoutingModelChoice::Pinned` carries two free `String`s, so
/// this proves the save/reload path never narrows, rewrites or rejects any
/// of them.
#[test]
fn pinned_routing_model_accepts_any_provider_and_model_string_including_gpt_5_6_luna() {
    let cases = [
        ("openrouter-remote-paid", "claude-frontier-tier"),
        ("groq-free-tier", "llama-3.1-8b-instant"),
        ("ollama-local", "qwen2.5-coder-local"),
        ("openrouter", "gpt-5.6-luna"),
    ];

    for (provider, model) in cases {
        let workspace = new_workspace();
        let data = tempfile::tempdir().unwrap();
        let runtime = runtime_for(workspace.path(), data.path());

        let edit = RoutingSettingsEdit {
            model: Some(RoutingModelChoice::Pinned {
                provider: provider.to_owned(),
                model: model.to_owned(),
            }),
            ..Default::default()
        };
        shell::save_user_settings_with_routing(&runtime, &[], &[], &[], Some(&edit)).unwrap();

        let user = UserConfig::load(runtime.paths()).unwrap();
        let effective = EffectiveConfig::new(&user, None);
        assert_eq!(
            effective.routing_model().value,
            RoutingModelChoice::Pinned {
                provider: provider.to_owned(),
                model: model.to_owned(),
            },
            "provider {provider:?} / model {model:?} must round-trip verbatim through the \
             real settings save path"
        );
    }
}

/// Phase 34B line 1418 — no vendor is hard-coded as a mandatory routing
/// dependency. With nothing ever configured, `shell::build_settings`'s own
/// source (`EffectiveConfig::routing_model`) resolves to
/// `RoutingModelChoice::Deterministic` at `Layer::Default` — not to any
/// specific model, vendor, or an error demanding one be set.
#[test]
fn fresh_configuration_resolves_to_deterministic_heuristics_with_no_vendor_hardcoded() {
    let workspace = new_workspace();
    let data = tempfile::tempdir().unwrap();
    let runtime = runtime_for(workspace.path(), data.path());

    let user = UserConfig::load(runtime.paths()).unwrap();
    let effective = EffectiveConfig::new(&user, None);
    let resolved = effective.routing_model();

    assert_eq!(resolved.value, RoutingModelChoice::Deterministic);
    assert_eq!(resolved.layer, Layer::Default);
}

/// Phase 34B line 1424 — deterministic heuristics remain the final fallback
/// when the pinned routing model is unavailable, specifically when its
/// provider has vanished from configuration.
///
/// Enters through `WizardState::routing_selection`, which is what
/// `routing_step` (rendered by the real onboarding wizard, `onboarding::run`
/// -> `view::render`) calls on every redraw — not a re-implementation of the
/// degrade logic. `WizardState::new` seeds `pending_routing` directly from
/// the loaded `UserConfig`, so no wizard navigation is needed to reach this
/// state; a config with a pin and zero configured providers already puts
/// the wizard there the moment it opens.
///
/// `RoutingSelectionView` is a private type (`onboarding::state` is not a
/// public module), so this asserts on its `Debug` text rather than naming
/// it — the same reason `main.rs`'s own tests source-scan text instead of
/// importing a private type.
#[test]
fn a_pinned_routing_models_vanished_provider_degrades_to_heuristics_in_the_onboarding_wizard() {
    let mut existing = UserConfig::default();
    existing
        .routing_mut()
        .set_model(Some(RoutingModelChoice::Pinned {
            provider: "vanished-provider".to_owned(),
            model: "some-pinned-model".to_owned(),
        }));
    // No providers configured at all, so the pin cannot resolve.

    let state = WizardState::new(
        &[],
        &existing,
        "proj".to_owned(),
        PathBuf::from("/work/proj"),
        "0.1.0".to_owned(),
    );

    let selection = format!("{:?}", state.routing_selection());
    assert!(
        selection.contains("PinnedUnavailable"),
        "a pin naming a provider that is no longer configured must degrade to \
         heuristics rather than silently keep claiming the pin: {selection}"
    );
    assert!(
        selection.contains("vanished-provider") && selection.contains("some-pinned-model"),
        "the degrade must say which provider and model it gave up on: {selection}"
    );
}

/// Phase 34C line 1443, the TUI-settings-screen reading only — see this
/// package's report for why the CLI (`glasshouse resources`) reading is
/// reported separately rather than folded into this test.
///
/// Enters through `shell::view::render`, the exact function `shell::run`'s
/// event loop draws every frame with, on a `ShellState` opened with the
/// production `RoutingRow` shape (`open_settings_with_routing`, what
/// `shell::run` calls after `build_settings`).
#[test]
fn the_settings_screen_shows_the_currently_selected_routing_model() {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    let routing = RoutingRow::new(
        Layered::new(
            RoutingModelChoice::Pinned {
                provider: "openrouter".to_owned(),
                model: "gpt-5.6-luna".to_owned(),
            },
            Layer::User,
        ),
        Layered::new(config::RouterLatencyMs::DEFAULT, Layer::Default),
        Layered::new(config::RouterCostMicroUsd::DEFAULT, Layer::Default),
        Layered::new(true, Layer::Default),
        Layered::new(config::PremiumReservePercent::DEFAULT, Layer::Default),
        vec!["openrouter".to_owned()],
    );

    let mut state = ShellState::new("p", "/work/p", "0.1.0", Vec::new());
    state.open_settings_with_routing(
        Vec::<HarnessRow>::new(),
        Vec::<IntegrationRow>::new(),
        Vec::<ProviderRow>::new(),
        Vec::<ProfileRow>::new(),
        routing,
    );

    // Harnesses -> Integrations -> Providers -> Launch Profiles -> Routing.
    for _ in 0..4 {
        state.handle_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
    }

    let text = rendered_settings(&state, 100, 30);
    assert!(
        text.contains("openrouter:gpt-5.6-luna"),
        "the Routing settings screen must show the selected routing model:\n{text}"
    );
}
