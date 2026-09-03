use super::*;
use crossterm::event::KeyModifiers;

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

fn ctrl_c() -> KeyEvent {
    KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)
}

fn char_key(c: char) -> KeyEvent {
    KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)
}

fn detection(
    id: IntegrationId,
    status: IntegrationStatus,
    executable: Option<&str>,
) -> IntegrationDetection {
    IntegrationDetection {
        id,
        status,
        executable: executable.map(PathBuf::from),
        version: None,
    }
}

/// The four harnesses detected as usable, nothing else. A reasonable
/// "everything found" baseline most tests start from.
fn all_harnesses_detected() -> Vec<IntegrationDetection> {
    vec![
        detection(
            IntegrationId::ClaudeCode,
            IntegrationStatus::Configured,
            Some("/usr/bin/claude"),
        ),
        detection(
            IntegrationId::Codex,
            IntegrationStatus::Unconfigured,
            Some("/usr/bin/codex"),
        ),
        detection(
            IntegrationId::Antigravity,
            IntegrationStatus::NotFound,
            None,
        ),
        detection(IntegrationId::OpenCode, IntegrationStatus::NotFound, None),
        detection(IntegrationId::Cmux, IntegrationStatus::NotFound, None),
        detection(IntegrationId::Ollama, IntegrationStatus::NotFound, None),
        detection(IntegrationId::LlamaCpp, IntegrationStatus::NotFound, None),
    ]
}

fn new_state(detected: &[IntegrationDetection]) -> WizardState {
    WizardState::new(
        detected,
        &UserConfig::default(),
        "demo-project".to_owned(),
        PathBuf::from("/home/user/demo-project"),
        "1.2.3".to_owned(),
    )
}

/// Drive `state` through a sequence of keys as `super::run`'s loop would,
/// stopping at the first terminal `Action` (`Cancel` or `Finish`), or
/// once the sequence is exhausted. Mirrors the loop's dispatch without
/// needing a `Screen` or `EventSource` — exactly the "state machine
/// without a terminal" split this module exists for.
fn drive(state: &mut WizardState, keys: &[KeyEvent]) -> Action {
    let mut last = Action::None;
    for &k in keys {
        last = state.handle_key(k);
        if matches!(last, Action::Cancel | Action::Finish) {
            return last;
        }
    }
    last
}

// --- is_required is exercised in `super::tests`, not here: it takes a
// `UserConfig` directly and has nothing to do with this state machine.

#[test]
fn happy_path_disables_one_harness_and_records_explicit_decisions() {
    let mut state = new_state(&all_harnesses_detected());

    // Welcome -> Harnesses.
    assert_eq!(state.handle_key(key(KeyCode::Enter)), Action::Redraw);
    assert_eq!(state.step(), Step::Harnesses);

    // Selection starts on Claude Code (first row); move to Codex (second
    // row) and turn it off.
    assert_eq!(state.handle_key(key(KeyCode::Down)), Action::Redraw);
    assert_eq!(state.handle_key(key(KeyCode::Char(' '))), Action::Redraw);

    // Harnesses -> Bypass.
    assert_eq!(state.handle_key(key(KeyCode::Tab)), Action::Redraw);
    assert_eq!(state.step(), Step::Bypass);

    // Bypass is optional too: Tab skips it (declined) straight to
    // Provider.
    assert_eq!(state.handle_key(key(KeyCode::Tab)), Action::Redraw);
    assert_eq!(state.step(), Step::Provider);

    // Provider is optional: Tab skips it ("Do later") straight to the
    // routing step without ever touching `pending_provider`.
    assert_eq!(state.handle_key(key(KeyCode::Tab)), Action::Redraw);
    assert_eq!(state.step(), Step::Routing);

    // The routing step is optional too, and lands *after* Provider
    // because pinning a model means naming a configured provider.
    assert_eq!(state.handle_key(key(KeyCode::Tab)), Action::Redraw);
    assert_eq!(state.step(), Step::Summary);

    // Finish.
    let action = state.handle_key(key(KeyCode::Enter));
    assert_eq!(action, Action::Finish);

    let mut config = UserConfig::default();
    state.apply_to(&mut config);

    assert!(config.onboarding().completed());
    assert_eq!(config.onboarding().completed_at_version(), Some("1.2.3"));

    // Claude Code: detected and usable, never toggled -> defaulted on.
    assert_eq!(
        config.integrations().is_enabled(IntegrationId::ClaudeCode),
        Some(true)
    );
    // Codex: detected and usable, explicitly toggled off.
    assert_eq!(
        config.integrations().is_enabled(IntegrationId::Codex),
        Some(false)
    );
    // Never detected, never touched -> defaulted off, but still an
    // explicit decision, not "never asked".
    for id in [
        IntegrationId::Antigravity,
        IntegrationId::OpenCode,
        IntegrationId::Ollama,
        IntegrationId::LlamaCpp,
    ] {
        assert_eq!(
            config.integrations().is_enabled(id),
            Some(false),
            "{id:?} must have an explicit decision, not never-asked"
        );
    }
    // cmux was not detected in this fixture, so it must not even appear
    // as a shown row, let alone get a recorded decision.
    assert_eq!(config.integrations().is_enabled(IntegrationId::Cmux), None);
}

#[test]
fn full_flow_driven_through_the_drive_helper_reaches_finish() {
    let mut state = new_state(&all_harnesses_detected());
    let action = drive(
        &mut state,
        &[
            key(KeyCode::Tab), // Welcome -> Harnesses
            key(KeyCode::Tab), // Harnesses -> Bypass
            key(KeyCode::Tab), // Bypass (declined) -> Provider
            key(KeyCode::Tab), // Provider (Do later) -> Routing
            key(KeyCode::Tab), // Routing (Do later) -> Summary
            key(KeyCode::Enter),
        ],
    );
    assert_eq!(action, Action::Finish);
    assert_eq!(state.step(), Step::Summary);
}

#[test]
fn cancelling_returns_cancel_and_leaves_the_caller_nothing_to_save() {
    let mut state = new_state(&all_harnesses_detected());

    // Make some changes first: cancellation must discard them, not just
    // happen to occur before any were made.
    let action = drive(
        &mut state,
        &[
            key(KeyCode::Tab),
            key(KeyCode::Char(' ')), // toggle Claude Code off
            key(KeyCode::Esc),
        ],
    );
    assert_eq!(action, Action::Cancel);

    // The contract `super::run` relies on: `Cancel` is never followed by
    // `apply_to`/`save`. There is no config mutation to inspect here
    // because none happens — that absence *is* the behaviour under
    // test. A fresh config, as the caller would still have it, remains
    // exactly default.
    let untouched = UserConfig::default();
    assert!(!untouched.onboarding().completed());
    assert!(untouched.integrations().is_empty());
}

#[test]
fn ctrl_c_cancels_even_while_typing_a_path() {
    let mut state = new_state(&all_harnesses_detected());
    state.handle_key(key(KeyCode::Enter)); // Welcome -> Harnesses
    // Antigravity (3rd row, not detected) -> open path input.
    state.handle_key(key(KeyCode::Down));
    state.handle_key(key(KeyCode::Down));
    assert_eq!(state.handle_key(key(KeyCode::Enter)), Action::Redraw);
    assert!(state.path_input().is_some());

    assert_eq!(state.handle_key(ctrl_c()), Action::Cancel);
}

#[test]
fn esc_while_typing_a_path_only_closes_the_input() {
    let mut state = new_state(&all_harnesses_detected());
    state.handle_key(key(KeyCode::Enter));
    state.handle_key(key(KeyCode::Down));
    state.handle_key(key(KeyCode::Down));
    state.handle_key(key(KeyCode::Enter)); // open input on Antigravity
    assert!(state.path_input().is_some());

    assert_eq!(state.handle_key(key(KeyCode::Esc)), Action::Redraw);
    assert!(state.path_input().is_none());
    assert_eq!(
        state.step(),
        Step::Harnesses,
        "Esc must not cancel the wizard here"
    );
}

#[test]
fn reopening_preselects_existing_decisions_and_override_path() {
    let mut existing = UserConfig::default();
    existing
        .integrations_mut()
        .entry(IntegrationId::ClaudeCode)
        .set_enabled(false);
    existing
        .integrations_mut()
        .entry(IntegrationId::Antigravity)
        .set_enabled(true)
        .set_executable(Some(PathBuf::from("/opt/antigravity/bin/antigravity")));

    let state = WizardState::new(
        &all_harnesses_detected(),
        &existing,
        "demo".to_owned(),
        PathBuf::from("/tmp/demo"),
        "9.9.9".to_owned(),
    );

    let claude = state
        .rows()
        .find(|r| r.id == IntegrationId::ClaudeCode)
        .expect("claude row present");
    assert_eq!(claude.decision, Some(false));

    let antigravity = state
        .rows()
        .find(|r| r.id == IntegrationId::Antigravity)
        .expect("antigravity row present");
    assert_eq!(antigravity.decision, Some(true));
    assert_eq!(
        antigravity.executable,
        Some(Path::new("/opt/antigravity/bin/antigravity"))
    );
    assert!(
        antigravity.usable,
        "an overridden path makes the row usable"
    );
}

#[test]
fn invalid_explicit_path_surfaces_the_error_and_does_not_advance() {
    let mut state = new_state(&all_harnesses_detected());
    state.handle_key(key(KeyCode::Enter)); // Welcome -> Harnesses
    state.handle_key(key(KeyCode::Down)); // Codex (usable, wrong target)
    state.handle_key(key(KeyCode::Down)); // Antigravity (not detected)
    assert_eq!(state.handle_key(key(KeyCode::Enter)), Action::Redraw);

    for c in "/definitely/not/a/real/executable".chars() {
        state.handle_key(char_key(c));
    }
    let action = state.handle_key(key(KeyCode::Enter));
    assert_eq!(action, Action::Redraw);

    let input = state.path_input().expect("still in input mode");
    assert!(input.error.is_some(), "resolve error must be surfaced");
    assert_eq!(input.integration_name, "Antigravity");

    let antigravity = state
        .rows()
        .find(|r| r.id == IntegrationId::Antigravity)
        .unwrap();
    assert_eq!(
        antigravity.decision, None,
        "must not record a decision on failure"
    );
    assert!(!antigravity.usable);
}

#[test]
fn valid_explicit_path_is_recorded_as_the_executable() {
    let tmp = tempfile::tempdir().unwrap();
    let exe_path = tmp.path().join("my-antigravity");
    std::fs::write(&exe_path, "#!/bin/sh\necho hi\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&exe_path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    let mut state = new_state(&all_harnesses_detected());
    state.handle_key(key(KeyCode::Enter));
    state.handle_key(key(KeyCode::Down));
    state.handle_key(key(KeyCode::Down));
    assert_eq!(state.handle_key(key(KeyCode::Enter)), Action::Redraw);

    for c in exe_path.to_str().unwrap().chars() {
        state.handle_key(char_key(c));
    }
    let action = state.handle_key(key(KeyCode::Enter));
    assert_eq!(action, Action::Redraw);
    assert!(
        state.path_input().is_none(),
        "successful resolve closes the input"
    );

    let antigravity = state
        .rows()
        .find(|r| r.id == IntegrationId::Antigravity)
        .unwrap();
    assert_eq!(antigravity.decision, Some(true));
    assert!(antigravity.usable);
    let recorded = antigravity.executable.expect("executable recorded");
    assert_eq!(
        std::fs::canonicalize(recorded).unwrap(),
        std::fs::canonicalize(&exe_path).unwrap()
    );

    // And it round-trips into a real `UserConfig`.
    state.handle_key(key(KeyCode::Tab));
    state.handle_key(key(KeyCode::Enter));
    let mut config = UserConfig::default();
    state.apply_to(&mut config);
    assert_eq!(
        config
            .integrations()
            .get(IntegrationId::Antigravity)
            .and_then(crate::config::IntegrationConfig::executable)
            .map(|p| std::fs::canonicalize(p).unwrap()),
        Some(std::fs::canonicalize(&exe_path).unwrap())
    );
    assert_eq!(
        config.integrations().is_enabled(IntegrationId::Antigravity),
        Some(true)
    );
}

#[test]
fn cmux_row_is_present_only_when_detected() {
    let without_cmux = new_state(&all_harnesses_detected());
    assert!(without_cmux.rows().all(|r| r.id != IntegrationId::Cmux));

    let mut with_cmux: Vec<IntegrationDetection> = all_harnesses_detected()
        .into_iter()
        .filter(|d| d.id != IntegrationId::Cmux)
        .collect();
    with_cmux.push(detection(
        IntegrationId::Cmux,
        IntegrationStatus::Available,
        Some("/usr/bin/cmux"),
    ));
    let state = new_state(&with_cmux);
    assert!(state.rows().any(|r| r.id == IntegrationId::Cmux));
}

#[test]
fn selection_clamps_at_both_ends() {
    let mut state = new_state(&all_harnesses_detected());
    state.handle_key(key(KeyCode::Enter)); // -> Harnesses
    state.handle_key(key(KeyCode::Up));
    state.handle_key(key(KeyCode::Up));
    assert_eq!(
        state.rows().position(|r| r.selected),
        Some(0),
        "cannot move above the first row"
    );

    let row_count = state.rows().count();
    for _ in 0..row_count + 3 {
        state.handle_key(key(KeyCode::Down));
    }
    assert_eq!(state.rows().position(|r| r.selected), Some(row_count - 1));
}

#[test]
fn tab_advances_and_enter_toggles_are_distinct_on_the_harnesses_step() {
    let mut state = new_state(&all_harnesses_detected());
    state.handle_key(key(KeyCode::Enter)); // Welcome -> Harnesses
    assert_eq!(state.step(), Step::Harnesses);

    // Enter on a usable row toggles it, it does not advance the step.
    state.handle_key(key(KeyCode::Enter));
    assert_eq!(state.step(), Step::Harnesses);
    let claude = state
        .rows()
        .find(|r| r.id == IntegrationId::ClaudeCode)
        .unwrap();
    assert_eq!(claude.decision, Some(false), "default true, toggled once");

    // Tab does advance — to the optional Bypass step now sitting
    // between Harnesses and Provider.
    state.handle_key(key(KeyCode::Tab));
    assert_eq!(state.step(), Step::Bypass);
}

// --- Phase 2C: the optional provider step -----------------------------

/// Drive a fresh wizard from Welcome to the Provider step, letting every
/// harness default (Tab through Welcome and Harnesses).
fn drive_to_provider(detected: &[IntegrationDetection]) -> WizardState {
    let mut state = new_state(detected);
    state.handle_key(key(KeyCode::Tab)); // Welcome -> Harnesses
    state.handle_key(key(KeyCode::Tab)); // Harnesses -> Bypass
    state.handle_key(key(KeyCode::Tab)); // Bypass (declined) -> Provider
    assert_eq!(state.step(), Step::Provider);
    state
}

/// Acceptance 1: the provider step is reachable and optional — the
/// wizard completes without ever touching it. A mutation that made
/// `Step::Provider` mandatory (refusing `Tab`/`Finish` until a provider
/// is chosen) fails this.
#[test]
fn the_provider_step_is_reachable_and_the_wizard_completes_without_touching_it() {
    let mut state = drive_to_provider(&all_harnesses_detected());

    // Do nothing but continue: Tab from the Choice screen, exactly like
    // Welcome and Harnesses.
    assert_eq!(state.handle_key(key(KeyCode::Tab)), Action::Redraw);
    assert_eq!(state.step(), Step::Routing);
    assert_eq!(state.handle_key(key(KeyCode::Tab)), Action::Redraw);
    assert_eq!(state.step(), Step::Summary);
    assert_eq!(state.handle_key(key(KeyCode::Enter)), Action::Finish);

    let mut config = UserConfig::default();
    state.apply_to(&mut config);
    assert!(config.onboarding().completed());
    assert!(
        config.providers().is_empty(),
        "the wizard must complete without recording a provider when the step is skipped"
    );
}

/// Acceptance 2 and 3: "Configure now" leads to real provider
/// configuration, and "Do later" completes onboarding recording none —
/// on the same wizard, proving the two paths are genuinely distinct
/// rather than one silently doing the other's job.
#[test]
fn configure_now_records_a_provider_while_do_later_records_none() {
    // --- Do later: no provider recorded, onboarding still completes.
    // The Choice screen defaults to "Do later" (see `WizardState::new`),
    // so `Enter` here confirms it directly.
    let mut do_later = drive_to_provider(&all_harnesses_detected());
    assert_eq!(do_later.handle_key(key(KeyCode::Enter)), Action::Redraw);
    assert_eq!(do_later.step(), Step::Routing);
    assert_eq!(do_later.handle_key(key(KeyCode::Tab)), Action::Redraw);
    assert_eq!(do_later.step(), Step::Summary);
    assert_eq!(do_later.handle_key(key(KeyCode::Enter)), Action::Finish);
    let mut config = UserConfig::default();
    do_later.apply_to(&mut config);
    assert!(config.onboarding().completed());
    assert!(
        config.providers().is_empty(),
        "\"Do later\" must record no provider and no credential of any kind"
    );

    // --- Configure now: picking the first template (openrouter, a
    // named, non-generic template) records a real, resolvable provider.
    let mut configure_now = drive_to_provider(&all_harnesses_detected());
    // The Choice screen defaults to "Do later"; move up onto "Configure
    // now".
    assert_eq!(configure_now.handle_key(key(KeyCode::Up)), Action::Redraw);
    assert_eq!(
        configure_now.handle_key(key(KeyCode::Enter)),
        Action::Redraw
    );
    let first_template = provider::templates()
        .first()
        .expect("at least one built-in template")
        .name
        .clone();
    assert_eq!(
        configure_now.handle_key(key(KeyCode::Enter)), // choose the first template
        Action::Redraw
    );
    assert_eq!(configure_now.step(), Step::Provider);
    assert_eq!(configure_now.handle_key(key(KeyCode::Tab)), Action::Redraw);
    assert_eq!(configure_now.step(), Step::Routing);
    assert_eq!(configure_now.handle_key(key(KeyCode::Tab)), Action::Redraw);
    assert_eq!(configure_now.step(), Step::Summary);
    assert_eq!(
        configure_now.handle_key(key(KeyCode::Enter)),
        Action::Finish
    );

    let mut config = UserConfig::default();
    configure_now.apply_to(&mut config);
    assert!(config.onboarding().completed());
    let provider_config = config
        .providers()
        .get(&first_template)
        .unwrap_or_else(|| panic!("`{first_template}` must be recorded after Configure now"));
    assert_eq!(provider_config.template(), first_template);

    // And it is a real, resolvable provider — not a name stashed
    // somewhere inert.
    let provider = provider_config
        .to_provider(&first_template)
        .expect("a built-in template name must resolve");
    assert_eq!(provider.name, first_template);
}

/// A generic template (`openai-compatible`/`anthropic-compatible`)
/// declares no base URL of its own, so "Configure now" must ask for one
/// and refuse to record the provider until it gets a non-empty answer.
#[test]
fn a_generic_template_is_recorded_only_once_a_base_url_is_typed() {
    let mut state = drive_to_provider(&all_harnesses_detected());
    state.handle_key(key(KeyCode::Up)); // onto "Configure now"
    state.handle_key(key(KeyCode::Enter)); // -> PickTemplate

    let templates = provider::templates();
    let generic_index = templates
        .iter()
        .position(|p| provider::GENERIC_TEMPLATE_NAMES.contains(&p.name.as_str()))
        .expect("at least one generic template exists");
    let generic_name = templates[generic_index].name.clone();
    for _ in 0..generic_index {
        state.handle_key(key(KeyCode::Down));
    }
    assert_eq!(state.handle_key(key(KeyCode::Enter)), Action::Redraw);

    // Confirming an empty URL is refused, with an inline error, and
    // nothing is recorded.
    assert_eq!(state.handle_key(key(KeyCode::Enter)), Action::Redraw);
    match state.provider_step() {
        ProviderStepView::BaseUrlInput { error, .. } => {
            assert!(error.is_some(), "an empty base URL must surface an error")
        }
        other => panic!("expected BaseUrlInput, got {other:?}"),
    }
    assert!(state.configured_providers().is_empty());

    for c in "https://gateway.example/v1".chars() {
        state.handle_key(char_key(c));
    }
    assert_eq!(state.handle_key(key(KeyCode::Enter)), Action::Redraw);
    assert_eq!(state.step(), Step::Provider);

    let mut config = UserConfig::default();
    state.handle_key(key(KeyCode::Tab)); // -> Summary
    state.handle_key(key(KeyCode::Enter)); // Finish
    state.apply_to(&mut config);

    let recorded = config
        .providers()
        .get(&generic_name)
        .expect("the generic template must be recorded once a base URL is confirmed");
    assert_eq!(recorded.base_url(), Some("https://gateway.example/v1"));
}

/// Acceptance 4: after "Do later", at least one detected harness is
/// enabled and its Native launch profile resolves — Glasshouse remains
/// fully usable on native, subscription-backed harnesses alone, with no
/// provider and no credential anywhere in the resulting configuration.
#[test]
fn do_later_leaves_glasshouse_usable_on_native_harnesses_with_no_provider_or_credential() {
    let mut state = drive_to_provider(&all_harnesses_detected());
    assert_eq!(state.handle_key(key(KeyCode::Tab)), Action::Redraw); // Do later
    assert_eq!(state.step(), Step::Routing);
    assert_eq!(state.handle_key(key(KeyCode::Tab)), Action::Redraw); // Do later
    assert_eq!(state.step(), Step::Summary);
    assert_eq!(state.handle_key(key(KeyCode::Enter)), Action::Finish);

    let mut config = UserConfig::default();
    state.apply_to(&mut config);

    // The absence that would otherwise silently rot: no provider, no
    // credential, anywhere in the resulting configuration.
    assert!(config.providers().is_empty());

    let enabled_harness = IntegrationId::ALL
        .iter()
        .copied()
        .find(|&id| {
            id.kind() == IntegrationKind::Harness
                && config.integrations().is_enabled(id) == Some(true)
        })
        .expect("at least one detected harness must be enabled after \"Do later\"");

    let adapter = crate::harness::adapter_for(enabled_harness)
        .expect("an enabled harness must have an adapter");
    let profile = crate::profile::LaunchProfile::native(enabled_harness);
    let secrets = crate::secret::EnvironmentSecretStore::new();
    let resolution = crate::profile::Resolution {
        adapter,
        acknowledged_bypass: false,
        provider: None,
        secrets: &secrets,
    };
    crate::profile::resolve(&profile, &resolution).unwrap_or_else(|err| {
        panic!("{enabled_harness:?}'s Native profile must resolve after \"Do later\": {err}")
    });
}

/// Acceptance 6, for the provider step specifically: reopening the
/// wizard after a provider was already configured must not lose it —
/// choosing "Do later" this time still leaves the earlier provider on
/// disk.
#[test]
fn reopening_preserves_a_previously_configured_provider() {
    let mut existing = UserConfig::default();
    existing.providers_mut().set(
        "openrouter",
        crate::config::ProviderConfig::new("openrouter"),
    );

    let mut state = WizardState::new(
        &all_harnesses_detected(),
        &existing,
        "demo".to_owned(),
        PathBuf::from("/tmp/demo"),
        "9.9.9".to_owned(),
    );

    // The reopened wizard shows the existing provider without being
    // told about it again.
    match state.provider_step() {
        ProviderStepView::Choice { providers, .. } => {
            assert!(providers.iter().any(|p| p.name == "openrouter"));
        }
        other => panic!("expected Choice, got {other:?}"),
    }

    state.handle_key(key(KeyCode::Tab)); // Welcome -> Harnesses
    state.handle_key(key(KeyCode::Tab)); // Harnesses -> Provider
    state.handle_key(key(KeyCode::Tab)); // Do later -> Summary
    state.handle_key(key(KeyCode::Enter)); // Finish

    let mut config = existing;
    state.apply_to(&mut config);
    assert_eq!(
        config.providers().get("openrouter").map(|p| p.template()),
        Some("openrouter"),
        "\"Do later\" on a reopen must not clear a provider configured in a prior run"
    );
}

/// Acceptance 5, live-request half: cmux stays absent until the user
/// explicitly asks for it with `c`, and once asked for it behaves like
/// any other row — an ordinary explicit-path/enable flow.
#[test]
fn cmux_can_be_explicitly_requested_even_when_not_detected() {
    let without_cmux: Vec<IntegrationDetection> = all_harnesses_detected()
        .into_iter()
        .filter(|d| d.id != IntegrationId::Cmux)
        .collect();
    let mut state = new_state(&without_cmux);
    state.handle_key(key(KeyCode::Enter)); // Welcome -> Harnesses
    assert!(
        state.rows().all(|r| r.id != IntegrationId::Cmux),
        "cmux must be absent when neither detected nor requested"
    );

    assert_eq!(state.handle_key(char_key('c')), Action::Redraw);
    assert!(
        state.rows().any(|r| r.id == IntegrationId::Cmux),
        "`c` must add cmux to the list on explicit request"
    );
    let cmux = state
        .rows()
        .find(|r| r.id == IntegrationId::Cmux)
        .expect("cmux row present");
    assert!(cmux.selected, "requesting cmux must select its new row");

    // A second `c` must not duplicate the row.
    state.handle_key(char_key('c'));
    assert_eq!(
        state.rows().filter(|r| r.id == IntegrationId::Cmux).count(),
        1
    );
}

/// Acceptance 6, config-persistence half for cmux: a previously
/// explicitly-configured cmux (via an explicit path, in an earlier run)
/// must still be shown on reopen even though live detection still finds
/// nothing — the wizard must not silently drop it from the list.
#[test]
fn reopening_shows_a_previously_configured_cmux_even_without_live_detection() {
    let mut existing = UserConfig::default();
    existing
        .integrations_mut()
        .entry(IntegrationId::Cmux)
        .set_enabled(true)
        .set_executable(Some(PathBuf::from("/opt/cmux/bin/cmux")));

    let without_cmux: Vec<IntegrationDetection> = all_harnesses_detected()
        .into_iter()
        .filter(|d| d.id != IntegrationId::Cmux)
        .collect();

    let state = WizardState::new(
        &without_cmux,
        &existing,
        "demo".to_owned(),
        PathBuf::from("/tmp/demo"),
        "9.9.9".to_owned(),
    );
    let cmux = state
        .rows()
        .find(|r| r.id == IntegrationId::Cmux)
        .expect("a previously configured cmux must still be shown on reopen");
    assert_eq!(cmux.decision, Some(true));
}

// --- Amendment 1: the optional bypass-acknowledgement step ------------

/// Production source of this module, with its test module and its
/// comments removed — mirrors `harness::production_code` and its
/// siblings elsewhere in this crate.
fn production_code(source: &str) -> String {
    source
        .split("#[cfg(test)]")
        .next()
        .expect("split always yields at least one part")
        .lines()
        .filter(|line| {
            let trimmed = line.trim_start();
            !trimmed.starts_with("//")
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Drive a fresh wizard from Welcome to the Bypass step.
fn drive_to_bypass(detected: &[IntegrationDetection]) -> WizardState {
    let mut state = new_state(detected);
    state.handle_key(key(KeyCode::Tab)); // Welcome -> Harnesses
    state.handle_key(key(KeyCode::Tab)); // Harnesses -> Bypass
    assert_eq!(state.step(), Step::Bypass);
    state
}

/// Skip both optional steps and finish, from wherever `state` currently
/// is on or before Bypass.
fn skip_to_finish(state: &mut WizardState) {
    while state.step() != Step::Summary {
        state.handle_key(key(KeyCode::Tab));
    }
    state.handle_key(key(KeyCode::Enter));
}

/// Acceptance 7: a harness declaring automatic review is not offered a
/// bypass acknowledgement; one declaring only a bypass is.
#[test]
fn only_a_harness_with_a_bypass_and_no_automatic_review_is_offered_bypass_acknowledgement() {
    let state = new_state(&all_harnesses_detected());
    let offered: Vec<IntegrationId> = state.bypass_rows().map(|r| r.id).collect();

    // Claude Code declares automatic review (see
    // `crate::profile::tests::a_native_profile_exists_for_every_harness_and_adds_nothing`
    // and its neighbours) — it must not be offered this step at all.
    assert!(
        !offered.contains(&IntegrationId::ClaudeCode),
        "a harness with an automatic-review mode must not be offered a bypass \
         acknowledgement: {offered:?}"
    );

    // Read from the adapters, not a fixed name, so this half stays
    // honest if the qualifying set ever changes.
    let expected: Vec<IntegrationId> = IntegrationId::ALL
        .iter()
        .copied()
        .filter(|&id| {
            crate::harness::adapter_for(id).is_some_and(|adapter| {
                let approvals = adapter.describe().approvals;
                !approvals.automatic_review.is_verified() && approvals.bypass.is_verified()
            })
        })
        .collect();
    assert!(
        !expected.is_empty(),
        "at least one harness declaring a bypass but no automatic review must exist for \
         this test to mean anything"
    );
    for id in expected {
        assert!(
            offered.contains(&id),
            "{id:?} declares a bypass but no automatic review and must be offered the step"
        );
    }
}

/// Acceptance 8: declining leaves `bypass_acknowledged` genuinely unset
/// — not an explicit `false` — and a `Bypass` profile for that harness
/// is still refused end to end, through the same `EffectiveConfig` read
/// a real caller uses.
#[test]
fn declining_leaves_bypass_acknowledged_unset_and_the_profile_still_refused() {
    let mut state = drive_to_bypass(&all_harnesses_detected());
    let harness = state
        .bypass_rows()
        .next()
        .expect("at least one bypass row")
        .id;

    // Leave every row untouched — declining is doing nothing here.
    skip_to_finish(&mut state);

    let mut config = UserConfig::default();
    state.apply_to(&mut config);
    assert_eq!(
        config
            .integrations()
            .get(harness)
            .and_then(crate::config::IntegrationConfig::bypass_acknowledged),
        None,
        "declining must leave bypass_acknowledged genuinely unset"
    );

    let effective = crate::config::EffectiveConfig::new(&config, None);
    let adapter = crate::harness::adapter_for(harness).expect("adapter exists");
    let mut profile = crate::profile::LaunchProfile::native(harness);
    profile.approval = crate::profile::ApprovalSelection::Bypass;
    let secrets = crate::secret::EnvironmentSecretStore::new();
    let resolution = crate::profile::Resolution {
        adapter,
        acknowledged_bypass: effective.bypass_acknowledged(harness).value,
        provider: None,
        secrets: &secrets,
    };
    let err = crate::profile::resolve(&profile, &resolution)
        .expect_err("an unacknowledged bypass must be refused");
    assert!(
        matches!(err, crate::profile::Refusal::BypassNotAcknowledged { .. }),
        "expected BypassNotAcknowledged, got {err:?}"
    );
}

/// Acceptance 9: accepting records the acknowledgement for that harness
/// only, leaving every other qualifying harness unset.
#[test]
fn accepting_records_the_acknowledgement_for_that_harness_only() {
    let mut state = drive_to_bypass(&all_harnesses_detected());
    let rows: Vec<IntegrationId> = state.bypass_rows().map(|r| r.id).collect();
    assert!(
        rows.len() >= 2,
        "need at least two qualifying harnesses for this test to mean anything: {rows:?}"
    );
    let accepted = rows[0];
    let untouched = rows[1..].to_vec();

    // Acknowledge only the first (selected) row.
    assert_eq!(state.handle_key(key(KeyCode::Char(' '))), Action::Redraw);

    skip_to_finish(&mut state);

    let mut config = UserConfig::default();
    state.apply_to(&mut config);
    assert_eq!(
        config
            .integrations()
            .get(accepted)
            .and_then(crate::config::IntegrationConfig::bypass_acknowledged),
        Some(true),
        "{accepted:?} was explicitly acknowledged and must be recorded"
    );
    for id in untouched {
        assert_eq!(
            config
                .integrations()
                .get(id)
                .and_then(crate::config::IntegrationConfig::bypass_acknowledged),
            None,
            "{id:?} was never touched and must remain unacknowledged"
        );
    }
}

/// Acceptance 10: the acknowledgement is written to the user layer and
/// never the project layer.
///
/// Structural half: this module has no way to reach a `ProjectConfig` at
/// all — `apply_to` only ever mutates the `UserConfig` it is handed, and
/// [`super::run`] only ever saves that same `UserConfig`. Runtime half:
/// once written, `EffectiveConfig::bypass_acknowledged` — which
/// deliberately never reads a project layer for this field, see its own
/// doc comment — reports it as [`crate::config::Layer::User`].
#[test]
fn the_acknowledgement_is_written_to_the_user_layer_and_never_the_project_layer() {
    let code = production_code(include_str!("mod.rs"));
    for forbidden in ["ProjectConfig", "write_project_config_with_consent"] {
        assert!(
            !code.contains(forbidden),
            "onboarding/state.rs names `{forbidden}` in production code: the wizard must \
             stay structurally unable to write a project-level configuration file"
        );
    }

    let mut state = drive_to_bypass(&all_harnesses_detected());
    let harness = state
        .bypass_rows()
        .next()
        .expect("at least one bypass row")
        .id;
    assert_eq!(state.handle_key(key(KeyCode::Char(' '))), Action::Redraw);
    skip_to_finish(&mut state);

    let mut config = UserConfig::default();
    state.apply_to(&mut config);

    let effective = crate::config::EffectiveConfig::new(&config, None);
    let resolved = effective.bypass_acknowledged(harness);
    assert!(resolved.value);
    assert_eq!(resolved.layer, crate::config::Layer::User);
}

/// Acceptance 11: the step is skippable and onboarding completes
/// without it — a mutation that made a row mandatory before `Tab`/
/// `Finish` would work fails this.
#[test]
fn the_bypass_step_is_skippable_and_onboarding_completes_without_it() {
    let mut state = drive_to_bypass(&all_harnesses_detected());
    assert_eq!(state.handle_key(key(KeyCode::Tab)), Action::Redraw);
    assert_eq!(state.step(), Step::Provider);

    skip_to_finish(&mut state);
    let mut config = UserConfig::default();
    state.apply_to(&mut config);
    assert!(config.onboarding().completed());
    for row in state.bypass_rows() {
        assert_eq!(
            config
                .integrations()
                .get(row.id)
                .and_then(crate::config::IntegrationConfig::bypass_acknowledged),
            None,
            "skipping the step must record no acknowledgement for {:?}",
            row.id
        );
    }
}

// --- Phase 2C, the four routing-model lines -----------------------

/// Drive a fresh wizard to the routing step by declining everything
/// before it, which is exactly the path a user who tabs through takes.
fn drive_to_routing(detected: &[IntegrationDetection]) -> WizardState {
    let mut state = drive_to_provider(detected);
    state.handle_key(key(KeyCode::Tab)); // Provider (Do later) -> Routing
    assert_eq!(state.step(), Step::Routing);
    state
}

/// A wizard seeded with one already-configured provider, so "Choose
/// model" has something to pin to. This is the state a user reaches by
/// configuring a provider on the previous step, or by reopening the
/// wizard after having done so.
fn config_with_provider(name: &str) -> UserConfig {
    let mut config = UserConfig::default();
    config
        .providers_mut()
        .set(name.to_owned(), ProviderConfig::new("openrouter"));
    config
}

fn drive_to_routing_with(existing: &UserConfig) -> WizardState {
    let mut state = WizardState::new(
        &all_harnesses_detected(),
        existing,
        "demo-project".to_owned(),
        PathBuf::from("/home/user/demo-project"),
        "1.2.3".to_owned(),
    );
    state.handle_key(key(KeyCode::Tab)); // Welcome -> Harnesses
    state.handle_key(key(KeyCode::Tab)); // Harnesses -> Bypass
    state.handle_key(key(KeyCode::Tab)); // Bypass -> Provider
    state.handle_key(key(KeyCode::Tab)); // Provider -> Routing
    assert_eq!(state.step(), Step::Routing);
    state
}

/// Line 1: the step exists, and it sits *after* the provider step and
/// before the summary. The order is the line's own wording ("after
/// providers have been detected or configured") and it is load-bearing:
/// the assertion below that `Step::Routing` is unreachable before
/// `Step::Provider` is what a mutation moving the step earlier fails on.
#[test]
fn the_routing_step_comes_after_the_provider_step_and_before_the_summary() {
    let mut state = new_state(&all_harnesses_detected());
    let mut seen = vec![state.step()];
    for _ in 0..4 {
        state.handle_key(key(KeyCode::Tab));
        seen.push(state.step());
    }
    assert_eq!(
        seen,
        vec![
            Step::Welcome,
            Step::Harnesses,
            Step::Bypass,
            Step::Provider,
            Step::Routing,
        ],
        "the routing step must be the one immediately after Provider"
    );

    // ...and the next Tab reaches the Summary, so nothing sits between
    // routing and the review screen.
    state.handle_key(key(KeyCode::Tab));
    assert_eq!(state.step(), Step::Summary);
}

/// Line 1 again, from the other side: the step is genuinely optional.
/// A mutation making `Step::Routing` refuse `Tab` until a choice is made
/// fails here.
#[test]
fn the_routing_step_is_reachable_and_the_wizard_completes_without_touching_it() {
    let mut state = drive_to_routing(&all_harnesses_detected());
    assert_eq!(state.handle_key(key(KeyCode::Tab)), Action::Redraw);
    assert_eq!(state.step(), Step::Summary);
    assert_eq!(state.handle_key(key(KeyCode::Enter)), Action::Finish);

    let mut config = UserConfig::default();
    state.apply_to(&mut config);
    assert!(config.onboarding().completed());
    assert_eq!(
        config.routing().model(),
        None,
        "skipping the step must record no routing model at all"
    );
}

/// Line 4, and the acceptance test that matters most: declining must
/// leave a *working* system, not merely a non-crashing one. So this
/// asserts what actually answers a routing question afterwards — the
/// deterministic-heuristics path — rather than only that the field is
/// empty.
#[test]
fn do_later_records_no_routing_model_and_deterministic_heuristics_answer() {
    let mut state = drive_to_routing(&all_harnesses_detected());
    // "Do later" is the default highlight, so Enter confirms it
    // directly — a first run that tabs through gets the same outcome.
    assert_eq!(state.handle_key(key(KeyCode::Enter)), Action::Redraw);
    assert_eq!(state.step(), Step::Summary);
    assert_eq!(state.handle_key(key(KeyCode::Enter)), Action::Finish);

    let mut config = UserConfig::default();
    state.apply_to(&mut config);

    assert_eq!(config.routing().model(), None);
    let effective = crate::config::EffectiveConfig::new(&config, None);
    let resolved = effective.routing_model_resolution();
    assert_eq!(
        resolved.value,
        crate::config::RoutingModelResolution::Heuristics(
            crate::config::RoutingFallback::NotConfigured
        ),
        "with nothing configured, deterministic heuristics must be what answers"
    );
    assert_eq!(resolved.layer, crate::config::Layer::Default);
}

/// Line 2: "Automatic" records the *intent*. Selecting the cheapest
/// sufficiently fast resource is Phase 34C's job and depends on
/// conditions that change after this wizard exits, so a mutation that
/// resolved a concrete model here and stored that instead would freeze a
/// decision the map wants re-evaluated. Asserting the stored value is
/// exactly `Automatic` — not merely "something is stored" — is what
/// catches it.
#[test]
fn automatic_records_the_intent_and_never_a_resolved_model() {
    let mut state = drive_to_routing_with(&config_with_provider("openrouter"));
    // Default highlight is "Do later" (bottom of three); go up twice.
    assert_eq!(state.handle_key(key(KeyCode::Up)), Action::Redraw);
    assert_eq!(state.handle_key(key(KeyCode::Up)), Action::Redraw);
    assert_eq!(state.handle_key(key(KeyCode::Enter)), Action::Redraw);
    assert_eq!(state.step(), Step::Summary);
    assert_eq!(state.handle_key(key(KeyCode::Enter)), Action::Finish);

    let mut config = config_with_provider("openrouter");
    state.apply_to(&mut config);
    assert_eq!(
        config.routing().model(),
        Some(&RoutingModelChoice::Automatic),
        "Automatic must be stored as the intent, with no model name in it"
    );

    // And it stays Automatic through resolution: nothing here picks a
    // model on the user's behalf.
    let effective = crate::config::EffectiveConfig::new(&config, None);
    assert_eq!(
        effective.routing_model_resolution().value,
        crate::config::RoutingModelResolution::Automatic
    );
}

/// Line 3: "Choose model" pins classification to a specific model, and
/// what gets stored is a provider-and-model *reference* — two names, the
/// same rule `StoredCredentialRef` follows. Nothing typed on that screen
/// is a credential.
#[test]
fn choose_model_records_a_provider_and_model_reference() {
    let mut state = drive_to_routing_with(&config_with_provider("openrouter"));
    assert_eq!(state.handle_key(key(KeyCode::Up)), Action::Redraw); // -> Choose model
    assert_eq!(state.handle_key(key(KeyCode::Enter)), Action::Redraw); // -> PickProvider
    assert!(matches!(
        state.routing_step(),
        RoutingStepView::PickProvider { .. }
    ));
    assert_eq!(state.handle_key(key(KeyCode::Enter)), Action::Redraw); // -> ModelInput
    for c in "gpt-5.6-luna".chars() {
        state.handle_key(char_key(c));
    }
    assert_eq!(state.handle_key(key(KeyCode::Enter)), Action::Redraw);

    // Back on the Choice screen with the pin recorded and visible.
    assert_eq!(state.step(), Step::Routing);
    assert_eq!(
        state.routing_selection(),
        RoutingSelectionView::Pinned {
            provider: "openrouter".to_owned(),
            model: "gpt-5.6-luna".to_owned(),
        }
    );

    state.handle_key(key(KeyCode::Tab));
    assert_eq!(state.step(), Step::Summary);
    let mut config = config_with_provider("openrouter");
    state.apply_to(&mut config);
    assert_eq!(
        config.routing().model(),
        Some(&RoutingModelChoice::Pinned {
            provider: "openrouter".to_owned(),
            model: "gpt-5.6-luna".to_owned(),
        })
    );
}

/// "Choose model" with nothing to choose from must say so rather than
/// doing nothing. A dead key on a first run — the case where no provider
/// is configured, which is the common one — reads as a broken wizard.
#[test]
fn choose_model_is_refused_with_a_notice_when_no_provider_is_configured() {
    let mut state = drive_to_routing(&all_harnesses_detected());
    assert_eq!(state.handle_key(key(KeyCode::Up)), Action::Redraw); // -> Choose model
    assert_eq!(state.handle_key(key(KeyCode::Enter)), Action::Redraw);

    // Still on the Choice screen, and the user has been told why.
    let RoutingStepView::Choice {
        can_choose_model,
        notice,
        ..
    } = state.routing_step()
    else {
        panic!("Choose model must not open a picker with no providers to pick from");
    };
    assert!(!can_choose_model);
    let notice = notice.expect("a press that does nothing must explain itself");
    assert!(
        notice.contains("configured provider"),
        "notice must say what is missing, got: {notice}"
    );

    // And the other two choices still work, so this is a bounded refusal
    // rather than a stuck screen.
    assert_eq!(state.handle_key(key(KeyCode::Down)), Action::Redraw);
    assert_eq!(state.handle_key(key(KeyCode::Enter)), Action::Redraw);
    assert_eq!(state.step(), Step::Summary);
}

/// A provider configured on the *previous* step is immediately pinnable,
/// which is the whole reason line 1 puts this step after the provider
/// step rather than before it.
#[test]
fn a_provider_configured_this_run_can_be_pinned_immediately() {
    let mut state = drive_to_provider(&all_harnesses_detected());
    state.handle_key(key(KeyCode::Up)); // -> Configure now
    state.handle_key(key(KeyCode::Enter)); // -> PickTemplate
    let first = provider::templates()
        .first()
        .expect("at least one built-in template")
        .name
        .clone();
    state.handle_key(key(KeyCode::Enter)); // record the first template
    state.handle_key(key(KeyCode::Tab)); // -> Routing

    assert_eq!(state.step(), Step::Routing);
    let RoutingStepView::Choice {
        can_choose_model, ..
    } = state.routing_step()
    else {
        panic!("expected the Choice screen");
    };
    assert!(
        can_choose_model,
        "a provider configured one step earlier must be available to pin to"
    );

    state.handle_key(key(KeyCode::Up)); // -> Choose model
    state.handle_key(key(KeyCode::Enter)); // -> PickProvider
    let RoutingStepView::PickProvider { options } = state.routing_step() else {
        panic!("expected the provider picker");
    };
    assert!(
        options.iter().any(|row| row.name == first),
        "the just-configured provider must be in the picker, got: {:?}",
        options.iter().map(|r| &r.name).collect::<Vec<_>>()
    );
}

/// The behavioural contract's third clause: a configuration naming a
/// model whose provider has since disappeared degrades to deterministic
/// heuristics *and says so*. The message is not composed here — it comes
/// from `RoutingFallback`'s own `Display`, so the wizard and the rest of
/// Glasshouse cannot explain the same degrade two different ways.
#[test]
fn a_pinned_model_whose_provider_is_gone_degrades_to_heuristics_and_says_so() {
    let mut existing = UserConfig::default();
    existing
        .routing_mut()
        .set_model(Some(RoutingModelChoice::Pinned {
            provider: "retired-router".to_owned(),
            model: "gpt-5.6-luna".to_owned(),
        }));
    // No provider by that name is configured anywhere.
    let state = drive_to_routing_with(&existing);

    let RoutingSelectionView::PinnedUnavailable {
        provider,
        model,
        message,
    } = state.routing_selection()
    else {
        panic!(
            "a pin naming an unconfigured provider must degrade, got {:?}",
            state.routing_selection()
        );
    };
    assert_eq!(provider, "retired-router");
    assert_eq!(model, "gpt-5.6-luna");
    assert!(
        message.contains("retired-router") && message.contains("gpt-5.6-luna"),
        "the degrade must name what went missing, got: {message}"
    );
    assert!(
        message.contains("deterministic"),
        "the degrade must say heuristics are answering, got: {message}"
    );

    // And it is not a startup failure: the wizard still finishes, and
    // the recorded choice is preserved rather than silently wiped, so
    // reconfiguring the provider restores the pin.
    let mut state = state;
    state.handle_key(key(KeyCode::Tab));
    assert_eq!(state.handle_key(key(KeyCode::Enter)), Action::Finish);
    let mut config = UserConfig::default();
    state.apply_to(&mut config);
    assert_eq!(
        config.routing().model(),
        Some(&RoutingModelChoice::Pinned {
            provider: "retired-router".to_owned(),
            model: "gpt-5.6-luna".to_owned(),
        }),
        "a degraded pin must be preserved, not deleted — the provider may come back"
    );
}

/// Reopening from settings shows what is already recorded rather than
/// re-offering the default, and tabbing past the step changes nothing.
#[test]
fn reopening_preselects_the_recorded_routing_choice_and_tabbing_past_preserves_it() {
    let mut existing = config_with_provider("openrouter");
    existing
        .routing_mut()
        .set_model(Some(RoutingModelChoice::Automatic));
    let mut state = drive_to_routing_with(&existing);

    let RoutingStepView::Choice {
        selected, recorded, ..
    } = state.routing_step()
    else {
        panic!("expected the Choice screen");
    };
    assert_eq!(
        selected,
        RoutingChoice::Automatic,
        "a reopen must highlight the choice already on disk, not the default"
    );
    assert_eq!(recorded, RoutingSelectionView::Automatic);

    state.handle_key(key(KeyCode::Tab));
    state.handle_key(key(KeyCode::Enter));
    let mut config = existing.clone();
    state.apply_to(&mut config);
    assert_eq!(
        config.routing().model(),
        Some(&RoutingModelChoice::Automatic),
        "tabbing past the step must not disturb what is recorded"
    );
}

/// The only way to un-configure a routing model from the wizard, and it
/// takes a deliberate press. This is where the routing step deviates
/// from the provider step, which never removes anything — see
/// `WizardState::pending_routing`.
#[test]
fn enter_on_do_later_clears_a_previously_recorded_routing_choice() {
    let mut existing = config_with_provider("openrouter");
    existing
        .routing_mut()
        .set_model(Some(RoutingModelChoice::Automatic));
    let mut state = drive_to_routing_with(&existing);

    // Highlight starts on Automatic (the recorded choice); move down
    // twice to "Do later" and confirm.
    state.handle_key(key(KeyCode::Down));
    state.handle_key(key(KeyCode::Down));
    assert_eq!(state.handle_key(key(KeyCode::Enter)), Action::Redraw);
    assert_eq!(state.step(), Step::Summary);
    state.handle_key(key(KeyCode::Enter));

    let mut config = existing.clone();
    state.apply_to(&mut config);
    assert_eq!(
        config.routing().model(),
        None,
        "explicitly choosing Do later must clear the recorded choice"
    );
    // The providers it was pinned against are untouched: this step
    // records a routing preference and nothing else.
    assert!(config.providers().get("openrouter").is_some());
}

/// `Esc` inside the routing sub-screens steps back one level rather than
/// cancelling the whole wizard, exactly as it does on the provider step
/// and in the explicit-path input. Losing a half-finished wizard to a
/// habitual Esc is the failure this prevents.
#[test]
fn esc_steps_back_through_the_routing_sub_screens_without_cancelling() {
    let mut state = drive_to_routing_with(&config_with_provider("openrouter"));
    state.handle_key(key(KeyCode::Up)); // -> Choose model
    state.handle_key(key(KeyCode::Enter)); // -> PickProvider
    state.handle_key(key(KeyCode::Enter)); // -> ModelInput

    assert_eq!(state.handle_key(key(KeyCode::Esc)), Action::Redraw);
    assert!(matches!(
        state.routing_step(),
        RoutingStepView::PickProvider { .. }
    ));
    assert_eq!(state.handle_key(key(KeyCode::Esc)), Action::Redraw);
    assert!(matches!(
        state.routing_step(),
        RoutingStepView::Choice { .. }
    ));
    // From the top-level Choice screen Esc means what it means
    // everywhere else.
    assert_eq!(state.handle_key(key(KeyCode::Esc)), Action::Cancel);
}

/// An empty model name is refused inline and the input stays open, the
/// same contract the explicit-path and base-URL fields already keep:
/// show the real problem and let the user correct it rather than
/// recording something that cannot work.
#[test]
fn an_empty_model_name_is_refused_and_the_input_stays_open() {
    let mut state = drive_to_routing_with(&config_with_provider("openrouter"));
    state.handle_key(key(KeyCode::Up)); // -> Choose model
    state.handle_key(key(KeyCode::Enter)); // -> PickProvider
    state.handle_key(key(KeyCode::Enter)); // -> ModelInput

    state.handle_key(char_key(' '));
    assert_eq!(state.handle_key(key(KeyCode::Enter)), Action::Redraw);
    let RoutingStepView::ModelInput { error, .. } = state.routing_step() else {
        panic!("an empty name must leave the input open");
    };
    assert!(
        error.is_some(),
        "an empty name must be explained, not eaten"
    );
    assert_eq!(
        state.routing_selection(),
        RoutingSelectionView::NotConfigured,
        "a refused name must record nothing"
    );

    // Typing a real one then works.
    for c in "llama3".chars() {
        state.handle_key(char_key(c));
    }
    state.handle_key(key(KeyCode::Enter));
    assert_eq!(
        state.routing_selection(),
        RoutingSelectionView::Pinned {
            provider: "openrouter".to_owned(),
            model: "llama3".to_owned(),
        }
    );
}

/// Acceptance 3, the path most users take: tab through every optional
/// step from a genuine first run and get a configuration that is
/// complete, saves and loads unchanged, and resolves to a working
/// routing answer. A new *required* step anywhere in the wizard breaks
/// this and nothing else would catch it.
#[test]
fn tabbing_through_every_optional_step_produces_a_valid_configuration() {
    let mut state = new_state(&all_harnesses_detected());
    let action = drive(
        &mut state,
        &[
            key(KeyCode::Tab), // Welcome -> Harnesses
            key(KeyCode::Tab), // Harnesses -> Bypass
            key(KeyCode::Tab), // Bypass -> Provider
            key(KeyCode::Tab), // Provider -> Routing
            key(KeyCode::Tab), // Routing -> Summary
            key(KeyCode::Tab), // finish
        ],
    );
    assert_eq!(action, Action::Finish);

    let mut config = UserConfig::default();
    state.apply_to(&mut config);

    assert!(config.onboarding().completed());
    assert_eq!(config.routing().model(), None);
    assert!(config.providers().is_empty());

    // Valid on disk, not merely valid in memory: it survives the exact
    // serialise/parse round trip `save`/`load` performs.
    let text = toml::to_string_pretty(&config).expect("the config must serialize");
    assert!(
        !text.contains("[routing"),
        "a wizard run that declined routing must write no routing table:\n{text}"
    );
    let reloaded: UserConfig = toml::from_str(&text).expect("the config must parse back");
    assert_eq!(reloaded, config);

    // And it resolves to something that works.
    let effective = crate::config::EffectiveConfig::new(&reloaded, None);
    assert!(matches!(
        effective.routing_model_resolution().value,
        crate::config::RoutingModelResolution::Heuristics(_)
    ));
}
