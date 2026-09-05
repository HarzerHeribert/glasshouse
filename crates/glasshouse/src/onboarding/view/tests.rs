use ratatui::Terminal;
use ratatui::backend::TestBackend;

use crate::config::{ProviderConfig, RoutingModelChoice, UserConfig};
use crate::integrations::{IntegrationId, IntegrationStatus};

use super::super::state::{IntegrationDetection, WizardState};
use super::*;

fn sample_state() -> WizardState {
    let detected = vec![
        IntegrationDetection {
            id: IntegrationId::ClaudeCode,
            status: IntegrationStatus::Configured,
            executable: Some("/usr/bin/claude".into()),
            version: Some("1.2.3".to_owned()),
        },
        IntegrationDetection {
            id: IntegrationId::Codex,
            status: IntegrationStatus::NotFound,
            executable: None,
            version: None,
        },
    ];
    WizardState::new(
        &detected,
        &UserConfig::default(),
        "glasshouse".to_owned(),
        "/home/user/glasshouse".into(),
        "0.1.0".to_owned(),
    )
}

fn render_at(state: &WizardState, width: u16, height: u16) {
    let backend = TestBackend::new(width, height);
    let mut terminal = Terminal::new(backend).expect("terminal");
    terminal
        .draw(|frame| render(state, frame))
        .expect("draw must not panic");
}

/// Every integration the wizard offers has a row on an 80x24 screen.
///
/// The catalogue grew from seven integrations to ten this session, which
/// moved this materially closer to its limit: ten rows plus two section
/// headers against the twenty-two the body gets. Not panicking is not the
/// same as being usable — Ratatui silently draws fewer rows when a list
/// outgrows its area, so an integration past the bottom edge would be one
/// the user can neither see nor toggle, with every other test green.
///
/// cmux is included here by giving it a detected executable, because the
/// wizard deliberately does not offer an undetected cmux at all (see
/// `build_rows`).
#[test]
fn every_offered_integration_has_a_row_at_80x24() {
    let state = advance_to_harnesses(all_detected_state());
    let screen = rendered_lines(&state, 80, 24);

    for &id in IntegrationId::ALL {
        let name = id.display_name();
        assert!(
            screen.iter().any(|line| line.contains(name)),
            "`{name}` has no visible row at 80x24; the catalogue has outgrown the \
             wizard's list"
        );
    }
}

/// Below 80x24 the list scrolls to follow the selection, so every row
/// stays reachable rather than being cut off at the bottom edge.
///
/// Twelve items into ten rows: this height genuinely truncates, which is
/// what makes the assertion mean something. Reverting the list to a
/// stateless `render_widget` fails this while leaving the test above
/// passing.
#[test]
fn a_short_terminal_still_reaches_every_integration() {
    let mut state = advance_to_harnesses(all_detected_state());

    for (step, &id) in IntegrationId::ALL.iter().enumerate() {
        let name = id.display_name();
        let screen = rendered_lines(&state, 80, 12);
        assert!(
            screen.iter().any(|line| line.contains(name)),
            "after {step} moves down, `{name}` is off a 80x12 screen and cannot be \
             reached"
        );
        state.handle_key(crossterm::event::KeyEvent::new(
            crossterm::event::KeyCode::Down,
            crossterm::event::KeyModifiers::NONE,
        ));
    }
}

/// A wizard offered every integration in the catalogue. cmux needs a
/// detected executable or the wizard will not offer it.
fn all_detected_state() -> WizardState {
    let detected: Vec<IntegrationDetection> = IntegrationId::ALL
        .iter()
        .map(|&id| IntegrationDetection {
            id,
            status: IntegrationStatus::NotFound,
            executable: (id == IntegrationId::Cmux).then(|| "/usr/bin/cmux".into()),
            version: None,
        })
        .collect();
    WizardState::new(
        &detected,
        &UserConfig::default(),
        "glasshouse".to_owned(),
        "/home/user/glasshouse".into(),
        "0.1.0".to_owned(),
    )
}

/// Draw `state` and read the screen back as lines of text.
fn rendered_lines(state: &WizardState, width: u16, height: u16) -> Vec<String> {
    let backend = TestBackend::new(width, height);
    let mut terminal = Terminal::new(backend).expect("terminal");
    terminal
        .draw(|frame| render(state, frame))
        .expect("draw must not panic");
    let buffer = terminal.backend().buffer().clone();
    (0..buffer.area.height)
        .map(|y| {
            (0..buffer.area.width)
                .map(|x| buffer[(x, y)].symbol())
                .collect::<String>()
        })
        .collect()
}

/// Move a fresh wizard to the harnesses step.
fn advance_to_harnesses(mut state: WizardState) -> WizardState {
    for _ in 0..4 {
        if matches!(state.step(), Step::Harnesses) {
            return state;
        }
        state.handle_key(crossterm::event::KeyEvent::new(
            crossterm::event::KeyCode::Enter,
            crossterm::event::KeyModifiers::NONE,
        ));
    }
    panic!("the wizard never reached the harnesses step");
}

/// Every [`Step`] the wizard has, drawn at 80x24.
///
/// Each stop asserts which step it actually reached. The previous version
/// of this test only *commented* that its last render was the Summary; it
/// was not — `Tab` does nothing in the provider template picker, so the
/// walk stopped one screen short and the Summary went unrendered here
/// while the comment said otherwise. A comment cannot fail, so the step is
/// asserted instead.
#[test]
fn every_step_renders_at_80x24_without_panicking() {
    let mut state = sample_state();
    assert_eq!(state.step(), Step::Welcome);
    render_at(&state, 80, 24);

    state.handle_key(crossterm::event::KeyEvent::new(
        crossterm::event::KeyCode::Enter,
        crossterm::event::KeyModifiers::NONE,
    ));
    assert_eq!(state.step(), Step::Harnesses);
    render_at(&state, 80, 24);

    state.handle_key(crossterm::event::KeyEvent::new(
        crossterm::event::KeyCode::Tab,
        crossterm::event::KeyModifiers::NONE,
    ));
    assert_eq!(state.step(), Step::Bypass);
    render_at(&state, 80, 24);

    state.handle_key(crossterm::event::KeyEvent::new(
        crossterm::event::KeyCode::Tab,
        crossterm::event::KeyModifiers::NONE,
    ));
    assert_eq!(state.step(), Step::Provider);
    render_at(&state, 80, 24); // Choice sub-mode

    state.handle_key(crossterm::event::KeyEvent::new(
        crossterm::event::KeyCode::Up,
        crossterm::event::KeyModifiers::NONE,
    ));
    state.handle_key(crossterm::event::KeyEvent::new(
        crossterm::event::KeyCode::Enter,
        crossterm::event::KeyModifiers::NONE,
    ));
    render_at(&state, 80, 24); // PickTemplate sub-mode

    // Back out of the picker, then continue: `Tab` is inert inside it.
    state.handle_key(crossterm::event::KeyEvent::new(
        crossterm::event::KeyCode::Esc,
        crossterm::event::KeyModifiers::NONE,
    ));
    state.handle_key(crossterm::event::KeyEvent::new(
        crossterm::event::KeyCode::Tab,
        crossterm::event::KeyModifiers::NONE,
    ));
    assert_eq!(state.step(), Step::Routing);
    render_at(&state, 80, 24); // Choice sub-mode

    state.handle_key(crossterm::event::KeyEvent::new(
        crossterm::event::KeyCode::Tab,
        crossterm::event::KeyModifiers::NONE,
    ));
    assert_eq!(state.step(), Step::Summary);
    render_at(&state, 80, 24);
}

/// Every sub-screen of the optional provider step, including the
/// base-URL text input, renders without panicking at every terminal
/// size this module already tests every other step at.
#[test]
fn every_provider_sub_screen_renders_without_panicking_at_every_size() {
    for (width, height) in [(80, 24), (20, 5), (300, 100), (0, 0)] {
        let mut state = advance_to_harnesses(sample_state());
        state.handle_key(crossterm::event::KeyEvent::new(
            crossterm::event::KeyCode::Tab,
            crossterm::event::KeyModifiers::NONE,
        ));
        assert_eq!(state.step(), Step::Bypass);
        state.handle_key(crossterm::event::KeyEvent::new(
            crossterm::event::KeyCode::Tab,
            crossterm::event::KeyModifiers::NONE,
        ));
        assert_eq!(state.step(), Step::Provider);
        render_at(&state, width, height); // Choice

        state.handle_key(crossterm::event::KeyEvent::new(
            crossterm::event::KeyCode::Up,
            crossterm::event::KeyModifiers::NONE,
        ));
        state.handle_key(crossterm::event::KeyEvent::new(
            crossterm::event::KeyCode::Enter,
            crossterm::event::KeyModifiers::NONE,
        ));
        render_at(&state, width, height); // PickTemplate

        // Move onto a generic template so the base-URL sub-mode is
        // reachable too.
        let generic_index = crate::provider::templates()
            .iter()
            .position(|p| crate::provider::GENERIC_TEMPLATE_NAMES.contains(&p.name.as_str()))
            .expect("a generic template exists");
        for _ in 0..generic_index {
            state.handle_key(crossterm::event::KeyEvent::new(
                crossterm::event::KeyCode::Down,
                crossterm::event::KeyModifiers::NONE,
            ));
        }
        state.handle_key(crossterm::event::KeyEvent::new(
            crossterm::event::KeyCode::Enter,
            crossterm::event::KeyModifiers::NONE,
        ));
        render_at(&state, width, height); // BaseUrlInput, empty

        for c in "https://gateway.example/v1".chars() {
            state.handle_key(crossterm::event::KeyEvent::new(
                crossterm::event::KeyCode::Char(c),
                crossterm::event::KeyModifiers::NONE,
            ));
        }
        render_at(&state, width, height); // BaseUrlInput, filled

        // An empty confirm surfaces the inline error state too.
        state.handle_key(crossterm::event::KeyEvent::new(
            crossterm::event::KeyCode::Backspace,
            crossterm::event::KeyModifiers::NONE,
        ));
    }
}

/// The optional bypass-acknowledgement step, toggled and untouched,
/// renders without panicking at every terminal size this module already
/// tests every other step at.
#[test]
fn every_bypass_row_renders_without_panicking_at_every_size() {
    for (width, height) in [(80, 24), (20, 5), (300, 100), (0, 0)] {
        let mut state = advance_to_harnesses(sample_state());
        state.handle_key(crossterm::event::KeyEvent::new(
            crossterm::event::KeyCode::Tab,
            crossterm::event::KeyModifiers::NONE,
        ));
        assert_eq!(state.step(), Step::Bypass);
        render_at(&state, width, height); // untouched, default declined

        state.handle_key(crossterm::event::KeyEvent::new(
            crossterm::event::KeyCode::Char(' '),
            crossterm::event::KeyModifiers::NONE,
        ));
        render_at(&state, width, height); // acknowledged
    }
}

#[test]
fn renders_without_panicking_at_a_tiny_size() {
    let state = sample_state();
    render_at(&state, 20, 5);
}

#[test]
fn renders_without_panicking_at_a_large_size() {
    let state = sample_state();
    render_at(&state, 300, 100);
}

#[test]
fn renders_without_panicking_with_the_path_input_open() {
    let mut state = sample_state();
    state.handle_key(crossterm::event::KeyEvent::new(
        crossterm::event::KeyCode::Enter,
        crossterm::event::KeyModifiers::NONE,
    ));
    // Move onto Codex (not detected) and open the path input.
    state.handle_key(crossterm::event::KeyEvent::new(
        crossterm::event::KeyCode::Down,
        crossterm::event::KeyModifiers::NONE,
    ));
    state.handle_key(crossterm::event::KeyEvent::new(
        crossterm::event::KeyCode::Enter,
        crossterm::event::KeyModifiers::NONE,
    ));
    assert!(state.path_input().is_some());
    render_at(&state, 80, 24);
    render_at(&state, 20, 5);
}

#[test]
fn zero_area_does_not_panic() {
    let state = sample_state();
    render_at(&state, 0, 0);
}

/// The three offers of the routing step, each highlighted in turn.
///
/// A reopen highlights whatever is recorded, and nothing is here, so the
/// screen opens on "Do later" — the Phase 2C line 4 default. Rendered at
/// 200 columns as well as 80 because every one of these labels wraps at
/// 80, and a wrapped label is a label a `contains` assertion can miss for
/// reasons that have nothing to do with the code (practice §17).
#[test]
fn every_routing_choice_renders_with_the_cursor_on_the_one_selected() {
    const LABELS: [&str; 3] = [
        "Do later — deterministic routing heuristics until configured",
        "Choose model — pin classification to one specific model",
        "Automatic — the cheapest sufficiently fast configured resource",
    ];

    let mut state = advance_to_routing(routing_state(&["my-router"], None));
    for (presses, selected) in LABELS.iter().enumerate() {
        for width in [80, 200] {
            let screen = rendered_lines(&state, width, 24);
            assert!(
                screen
                    .iter()
                    .any(|line| line.starts_with(&format!("> {selected}"))),
                "after {presses} moves up at {width} columns the cursor is not on \
                 `{selected}`:\n{}",
                screen.join("\n")
            );
            for other in LABELS.iter().filter(|label| *label != selected) {
                assert!(
                    screen
                        .iter()
                        .any(|line| line.starts_with(&format!("  {other}"))),
                    "`{other}` is not offered unselected at {width} columns:\n{}",
                    screen.join("\n")
                );
            }
        }
        state.handle_key(crossterm::event::KeyEvent::new(
            crossterm::event::KeyCode::Up,
            crossterm::event::KeyModifiers::NONE,
        ));
    }
}

/// "Choose model" needs two answers, and both screens render them: which
/// configured provider, then which model of that provider's.
#[test]
fn the_routing_provider_picker_and_the_model_field_render() {
    let mut state = advance_to_routing(routing_state(&["my-router"], None));
    state.handle_key(crossterm::event::KeyEvent::new(
        crossterm::event::KeyCode::Up,
        crossterm::event::KeyModifiers::NONE,
    ));
    state.handle_key(crossterm::event::KeyEvent::new(
        crossterm::event::KeyCode::Enter,
        crossterm::event::KeyModifiers::NONE,
    ));
    for width in [80, 200] {
        let screen = rendered_lines(&state, width, 24);
        assert!(
            screen
                .iter()
                .any(|line| line.contains("Configured providers")),
            "the provider picker has no heading at {width} columns:\n{}",
            screen.join("\n")
        );
        assert!(
            screen
                .iter()
                .any(|line| line.starts_with("> my-router") && line.contains("openrouter")),
            "the provider picker shows no cursor, name and template at {width} \
             columns:\n{}",
            screen.join("\n")
        );
    }

    state.handle_key(crossterm::event::KeyEvent::new(
        crossterm::event::KeyCode::Enter,
        crossterm::event::KeyModifiers::NONE,
    ));
    for c in "haiku-cheap".chars() {
        state.handle_key(crossterm::event::KeyEvent::new(
            crossterm::event::KeyCode::Char(c),
            crossterm::event::KeyModifiers::NONE,
        ));
    }
    for width in [80, 200] {
        let screen = rendered_lines(&state, width, 24);
        assert!(
            screen
                .iter()
                .any(|line| line.contains("Routing model to pin from `my-router`: haiku-cheap_")),
            "the model field does not name the provider, the buffer and the cursor at \
             {width} columns:\n{}",
            screen.join("\n")
        );
    }

    // An empty confirm is refused, and says so where the field is.
    for _ in 0.."haiku-cheap".len() {
        state.handle_key(crossterm::event::KeyEvent::new(
            crossterm::event::KeyCode::Backspace,
            crossterm::event::KeyModifiers::NONE,
        ));
    }
    state.handle_key(crossterm::event::KeyEvent::new(
        crossterm::event::KeyCode::Enter,
        crossterm::event::KeyModifiers::NONE,
    ));
    for width in [80, 200] {
        let screen = rendered_lines(&state, width, 24);
        assert!(
            screen
                .iter()
                .any(|line| line.contains("a model name is required to pin routing")),
            "an empty model name is refused silently at {width} columns:\n{}",
            screen.join("\n")
        );
    }
}

/// With no provider configured there is nothing to pin to, and the screen
/// says which prerequisite is missing rather than dropping the option.
///
/// The full parenthetical is only asserted at 200 columns: at 80 it wraps
/// mid-phrase, so the short assertion there is the single word that
/// cannot be split.
#[test]
fn choose_model_says_why_it_is_unavailable_with_no_provider_configured() {
    let mut state = advance_to_routing(routing_state(&[], None));

    let narrow = rendered_lines(&state, 80, 24);
    assert!(
        narrow
            .iter()
            .any(|line| line.contains("Choose model") && line.contains("unavailable")),
        "at 80 columns the Choose model row does not say it is unavailable:\n{}",
        narrow.join("\n")
    );
    let wide = rendered_lines(&state, 200, 24);
    assert!(
        wide.iter().any(|line| line.contains(
            "Choose model — pin classification to one specific model (unavailable: \
             needs a configured provider)"
        )),
        "at 200 columns the Choose model row does not name the missing \
         prerequisite:\n{}",
        wide.join("\n")
    );

    // Selecting it explains itself instead of reading as a dead key.
    state.handle_key(crossterm::event::KeyEvent::new(
        crossterm::event::KeyCode::Up,
        crossterm::event::KeyModifiers::NONE,
    ));
    state.handle_key(crossterm::event::KeyEvent::new(
        crossterm::event::KeyCode::Enter,
        crossterm::event::KeyModifiers::NONE,
    ));
    assert_eq!(state.step(), Step::Routing);
    assert!(
        rendered_lines(&state, 80, 24)
            .iter()
            .any(|line| line.contains("Choose model needs a configured provider")),
        "the refused press left no notice at 80 columns"
    );
    assert!(
        rendered_lines(&state, 200, 24)
            .iter()
            .any(|line| line.contains(
                "Choose model needs a configured provider, and none is configured yet. Go \
             back with Esc to add one, or pick Automatic or Do later."
            )),
        "the refused press left no complete notice at 200 columns"
    );
}

/// The degrade explanation from [`crate::config::RoutingFallback`] reaches
/// the screen verbatim, on the routing step and again on the Summary.
///
/// Verbatim is the whole point: this sentence is written once, in
/// configuration, so the wizard cannot invent a second account of the same
/// degrade. Asserting on the whole of it needs 200 columns — at 80 it
/// wraps across three rows, and the substring asserted there is chosen to
/// sit inside the first of them.
#[test]
fn a_pinned_model_whose_provider_vanished_explains_itself_on_both_screens() {
    let mut state = advance_to_routing(routing_state(
        &[],
        Some(RoutingModelChoice::Pinned {
            provider: "vanished".to_owned(),
            model: "haiku-cheap".to_owned(),
        }),
    ));

    assert_routing_degrade_is_visible(&state, "the routing step");

    state.handle_key(crossterm::event::KeyEvent::new(
        crossterm::event::KeyCode::Tab,
        crossterm::event::KeyModifiers::NONE,
    ));
    assert_eq!(state.step(), Step::Summary);
    assert_routing_degrade_is_visible(&state, "the Summary");
}

/// Both screens are checked the same way, so neither can drift into
/// paraphrasing while the other stays honest.
fn assert_routing_degrade_is_visible(state: &WizardState, screen_name: &str) {
    const WHOLE: &str = "routing model `haiku-cheap` names provider `vanished`, which is \
                         not configured; requests are classified by deterministic routing \
                         heuristics until that provider is configured again";

    let narrow = rendered_lines(state, 80, 24);
    assert!(
        narrow
            .iter()
            .any(|line| line.contains("names provider `vanished`, which is not configured")),
        "{screen_name} does not explain the vanished provider at 80 columns:\n{}",
        narrow.join("\n")
    );
    assert!(
        narrow
            .iter()
            .any(|line| line.contains("`haiku-cheap`") && line.contains("`vanished`")),
        "{screen_name} does not name the pinned model and its provider at 80 \
         columns:\n{}",
        narrow.join("\n")
    );

    let wide = rendered_lines(state, 200, 40);
    assert!(
        wide.iter().any(|line| line.contains(WHOLE)),
        "{screen_name} does not carry the degrade explanation verbatim at 200 \
         columns:\n{}",
        wide.join("\n")
    );
}

/// The Summary reports every recorded routing state, and no longer claims
/// that routing-model configuration is absent from this setup.
///
/// The `!contains` half is asserted at 200 columns as well as 80: a stale
/// sentence that is merely truncated off a narrow screen is still in the
/// build (practice §17).
#[test]
fn the_summary_reports_whichever_routing_model_is_recorded() {
    let cases = [
        (
            Vec::new(),
            None,
            "Routing model: none configured; deterministic routing heuristics classify \
             requests until one is, which is a working system rather than a gap.",
        ),
        (
            Vec::new(),
            Some(RoutingModelChoice::Deterministic),
            "Routing model: deterministic-only, on purpose — no model is asked, and \
             deterministic routing heuristics classify requests.",
        ),
        (
            Vec::new(),
            Some(RoutingModelChoice::Automatic),
            "Routing model: automatic — the resource is chosen at the moment a decision \
             is actually needed, not now.",
        ),
        (
            vec!["my-router"],
            Some(RoutingModelChoice::Pinned {
                provider: "my-router".to_owned(),
                model: "haiku-cheap".to_owned(),
            }),
            "Routing model: `haiku-cheap` from provider `my-router`.",
        ),
    ];

    for (providers, routing, expected) in cases {
        let mut state = advance_to_routing(routing_state(&providers, routing));
        state.handle_key(crossterm::event::KeyEvent::new(
            crossterm::event::KeyCode::Tab,
            crossterm::event::KeyModifiers::NONE,
        ));
        assert_eq!(state.step(), Step::Summary);

        let wide = rendered_lines(&state, 200, 40);
        assert!(
            wide.iter().any(|line| line.contains(expected)),
            "the Summary does not report `{expected}`:\n{}",
            wide.join("\n")
        );
        assert!(
            wide.iter()
                .any(|line| line.contains("The Glasshouse gateway is not part of this setup")),
            "the Summary stopped saying the gateway is still out of scope:\n{}",
            wide.join("\n")
        );
        for (width, height) in [(80, 24), (200, 40)] {
            let screen = rendered_lines(&state, width, height);
            assert!(
                !screen
                    .iter()
                    .any(|line| line.contains("routing-model configuration are not part")),
                "the Summary still claims routing-model configuration is out of scope, \
                 at {width}x{height}:\n{}",
                screen.join("\n")
            );
        }
    }
}

/// Every sub-screen of the optional routing step, including the
/// model-name text field and its inline error, renders without panicking
/// at every terminal size this module already tests every other step at.
#[test]
fn every_routing_sub_screen_renders_without_panicking_at_every_size() {
    for (width, height) in [(80, 24), (20, 5), (300, 100), (0, 0)] {
        // No provider: the Choice screen with the unavailable wording,
        // and the notice a refused "Choose model" leaves behind.
        let mut bare = advance_to_routing(routing_state(&[], None));
        render_at(&bare, width, height);
        bare.handle_key(crossterm::event::KeyEvent::new(
            crossterm::event::KeyCode::Up,
            crossterm::event::KeyModifiers::NONE,
        ));
        bare.handle_key(crossterm::event::KeyEvent::new(
            crossterm::event::KeyCode::Enter,
            crossterm::event::KeyModifiers::NONE,
        ));
        render_at(&bare, width, height); // Choice, with a notice

        // A pinned model whose provider is gone: the longest string any
        // of these screens can be asked to lay out.
        let degraded = advance_to_routing(routing_state(
            &[],
            Some(RoutingModelChoice::Pinned {
                provider: "vanished".to_owned(),
                model: "haiku-cheap".to_owned(),
            }),
        ));
        render_at(&degraded, width, height);

        let mut state = advance_to_routing(routing_state(&["my-router"], None));
        render_at(&state, width, height); // Choice
        state.handle_key(crossterm::event::KeyEvent::new(
            crossterm::event::KeyCode::Up,
            crossterm::event::KeyModifiers::NONE,
        ));
        state.handle_key(crossterm::event::KeyEvent::new(
            crossterm::event::KeyCode::Enter,
            crossterm::event::KeyModifiers::NONE,
        ));
        render_at(&state, width, height); // PickProvider

        state.handle_key(crossterm::event::KeyEvent::new(
            crossterm::event::KeyCode::Enter,
            crossterm::event::KeyModifiers::NONE,
        ));
        render_at(&state, width, height); // ModelInput, empty

        state.handle_key(crossterm::event::KeyEvent::new(
            crossterm::event::KeyCode::Enter,
            crossterm::event::KeyModifiers::NONE,
        ));
        render_at(&state, width, height); // ModelInput, refused and erroring

        for c in "haiku-cheap".chars() {
            state.handle_key(crossterm::event::KeyEvent::new(
                crossterm::event::KeyCode::Char(c),
                crossterm::event::KeyModifiers::NONE,
            ));
        }
        render_at(&state, width, height); // ModelInput, filled

        state.handle_key(crossterm::event::KeyEvent::new(
            crossterm::event::KeyCode::Enter,
            crossterm::event::KeyModifiers::NONE,
        ));
        render_at(&state, width, height); // Choice, with the pin recorded
    }
}

/// A wizard seeded with `providers` already in configuration and
/// `routing` already recorded, so the routing step's reopen behaviour and
/// its degrade can both be reached without pressing a key.
fn routing_state(providers: &[&str], routing: Option<RoutingModelChoice>) -> WizardState {
    let detected = vec![IntegrationDetection {
        id: IntegrationId::ClaudeCode,
        status: IntegrationStatus::Configured,
        executable: Some("/usr/bin/claude".into()),
        version: Some("1.2.3".to_owned()),
    }];
    let mut config = UserConfig::default();
    for name in providers {
        config
            .providers_mut()
            .set(*name, ProviderConfig::new("openrouter"));
    }
    config.routing_mut().set_model(routing);
    WizardState::new(
        &detected,
        &config,
        "glasshouse".to_owned(),
        "/home/user/glasshouse".into(),
        "0.1.0".to_owned(),
    )
}

/// Move a fresh wizard to the routing step without touching any earlier
/// one, which is the path a user who tabs through the optional steps
/// takes.
fn advance_to_routing(state: WizardState) -> WizardState {
    let mut state = advance_to_harnesses(state);
    for _ in 0..3 {
        state.handle_key(crossterm::event::KeyEvent::new(
            crossterm::event::KeyCode::Tab,
            crossterm::event::KeyModifiers::NONE,
        ));
    }
    assert_eq!(state.step(), Step::Routing);
    state
}

/// The Summary's genuine worst case, at the size it promises to fit.
///
/// Every integration in the catalogue detected with a realistic — that is,
/// long — executable path, a configured provider, and a pinned routing
/// model whose provider has vanished, so the four-row degrade explanation
/// is on screen too. This is not a hypothetical: running the shipped
/// binary on a machine with ten harnesses installed under a macOS
/// temporary directory dropped the entire `Routing model:` line and the
/// gateway note off the bottom, and nothing said so, because a wrapped
/// paragraph simply stops drawing.
///
/// The screen has **no rows to spare** in this state: it is the ruling
/// GH-SUMMARY-SCROLL implements. Nothing may be silently cut any more —
/// the last body row must say how much is still below, `End` must bring
/// the rest onto the screen, and the union of what both screens show must
/// still be everything.
fn worst_case_summary_state() -> WizardState {
    let long = "/private/var/folders/gc/y14vjq1j3wq6_gj1zt10t7j40000gn/T/agent-shims/\
                DC30465E-5CC0-4172-A1E8-F17DB285B969";
    let detected: Vec<IntegrationDetection> = IntegrationId::ALL
        .iter()
        .map(|&id| IntegrationDetection {
            id,
            status: IntegrationStatus::Configured,
            executable: Some(format!("{long}/{}", id.slug()).into()),
            version: None,
        })
        .collect();
    let mut config = UserConfig::default();
    config
        .providers_mut()
        .set("openrouter", ProviderConfig::new("openrouter"));
    config
        .routing_mut()
        .set_model(Some(RoutingModelChoice::Pinned {
            provider: "vanished-router".to_owned(),
            model: "gpt-5.6-luna".to_owned(),
        }));
    let mut state = WizardState::new(
        &detected,
        &config,
        "glasshouse".to_owned(),
        "/home/user/glasshouse".into(),
        "0.1.0".to_owned(),
    );
    let mut state = advance_to_routing({
        state.handle_key(crossterm::event::KeyEvent::new(
            crossterm::event::KeyCode::Tab,
            crossterm::event::KeyModifiers::NONE,
        ));
        state
    });
    state.handle_key(crossterm::event::KeyEvent::new(
        crossterm::event::KeyCode::Tab,
        crossterm::event::KeyModifiers::NONE,
    ));
    assert_eq!(state.step(), Step::Summary);
    state
}

#[test]
fn every_summary_section_survives_the_worst_case_at_80x24() {
    let mut state = worst_case_summary_state();

    let top = rendered_lines(&state, 80, 24);
    let top_text = top.join("\n");

    // Every integration is exactly one row, so eleven of them cost eleven
    // rows however long the machine's paths happen to be.
    for &id in IntegrationId::ALL {
        let name = id.display_name();
        assert_eq!(
            top.iter().filter(|line| line.contains(name)).count(),
            1,
            "{name} must occupy exactly one Summary row at 80 columns, got:\n{top_text}"
        );
    }

    // The last body row (index 22: title at 0, footer at 23) announces
    // that more follows rather than silently dropping it.
    let last_body_row = &top[22];
    assert!(
        last_body_row.contains('\u{2193}') && last_body_row.contains("more row"),
        "the last body row must announce the rows below, got:\n{last_body_row:?}"
    );

    state.handle_key(crossterm::event::KeyEvent::new(
        crossterm::event::KeyCode::End,
        crossterm::event::KeyModifiers::NONE,
    ));
    let bottom = rendered_lines(&state, 80, 24);
    let bottom_text = bottom.join("\n");

    // The first body row (index 1: title is index 0) now announces what
    // scrolled off above, and the routing lines and gateway note the top
    // screen could not fit are on screen.
    let first_body_row = &bottom[1];
    assert!(
        first_body_row.contains('\u{2191}') && first_body_row.contains("above"),
        "the first body row must announce the rows above once scrolled, got:\n{first_body_row:?}"
    );

    let union = format!("{top_text}\n{bottom_text}");
    for required in [
        "Routing model:",
        "gpt-5.6-luna",
        "vanished-router",
        "deterministic routing heuristics",
        "The Glasshouse gateway is not part of this setup yet.",
        "openrouter",
    ] {
        assert!(
            union.contains(required),
            "the Summary dropped {required:?} across both scroll positions:\n{union}"
        );
    }
}

/// A Summary that fits gets no scrolling machinery at all: same rendering
/// as before this packet, and the scroll keys are no-ops.
#[test]
fn a_fitting_summary_has_no_indicator_and_ignores_scroll_keys() {
    let mut state = advance_to_routing(sample_state());
    state.handle_key(crossterm::event::KeyEvent::new(
        crossterm::event::KeyCode::Tab,
        crossterm::event::KeyModifiers::NONE,
    ));
    assert_eq!(state.step(), Step::Summary);

    let before = rendered_lines(&state, 80, 40);
    assert!(
        !before
            .iter()
            .any(|line| line.contains('\u{2193}') || line.contains('\u{2191}')),
        "a fitting Summary must carry no scroll indicator, got:\n{}",
        before.join("\n")
    );
    let footer = &before[39];
    assert_eq!(footer.trim_end(), "Enter / Tab finish   Esc cancel");

    state.handle_key(crossterm::event::KeyEvent::new(
        crossterm::event::KeyCode::Down,
        crossterm::event::KeyModifiers::NONE,
    ));
    let after = rendered_lines(&state, 80, 40);
    assert_eq!(
        before, after,
        "Down on a fitting Summary must render nothing differently"
    );
}
