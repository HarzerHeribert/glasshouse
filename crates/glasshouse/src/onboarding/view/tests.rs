use ratatui::Terminal;
use ratatui::backend::TestBackend;

use crate::config::{ProviderConfig, UserConfig};
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

/// The Summary's genuine worst case, at the size it promises to fit.
///
/// Every integration in the catalogue detected with a realistic — that is,
/// long — executable path and a configured provider, so every row's own
/// length is on screen. This is not a hypothetical: running the shipped
/// binary on a machine with ten harnesses installed under a macOS
/// temporary directory dropped rows off the bottom, and nothing said so,
/// because a wrapped paragraph simply stops drawing.
///
/// GH-SUMMARY-SCROLL's ruling — nothing may be silently cut, the last body
/// row must say how much is still below, `End` must bring the rest onto the
/// screen, and the union of what both screens show must still be everything
/// — is exercised at a height tight enough to force it: since the routing
/// deletion (design-decisions, 2026-09-16) removed the routing-model line
/// that used to make 80x24 the exact tight boundary, eleven real integration
/// rows plus one provider and the gateway note now fit at 80x24 with rows to
/// spare, so this state is rendered at a shorter screen in the test itself.
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
    let state = WizardState::new(
        &detected,
        &config,
        "glasshouse".to_owned(),
        "/home/user/glasshouse".into(),
        "0.1.0".to_owned(),
    );
    let mut state = advance_to_harnesses(state);
    for _ in 0..3 {
        state.handle_key(crossterm::event::KeyEvent::new(
            crossterm::event::KeyCode::Tab,
            crossterm::event::KeyModifiers::NONE,
        ));
    }
    assert_eq!(state.step(), Step::Summary);
    state
}

#[test]
fn every_summary_section_survives_the_worst_case_at_80x18() {
    let mut state = worst_case_summary_state();

    let top = rendered_lines(&state, 80, 18);
    let top_text = top.join("\n");

    // Every integration is exactly one row, so eleven of them cost eleven
    // rows however long the machine's paths happen to be, and all eleven
    // are still ahead of the point this screen height cuts off.
    for &id in IntegrationId::ALL {
        let name = id.display_name();
        assert_eq!(
            top.iter().filter(|line| line.contains(name)).count(),
            1,
            "{name} must occupy exactly one Summary row at 80 columns, got:\n{top_text}"
        );
    }

    // The last body row (index 16: title at 0, footer at 17) announces
    // that more follows rather than silently dropping it.
    let last_body_row = &top[16];
    assert!(
        last_body_row.contains('\u{2193}') && last_body_row.contains("more row"),
        "the last body row must announce the rows below, got:\n{last_body_row:?}"
    );

    state.handle_key(crossterm::event::KeyEvent::new(
        crossterm::event::KeyCode::End,
        crossterm::event::KeyModifiers::NONE,
    ));
    let bottom = rendered_lines(&state, 80, 18);
    let bottom_text = bottom.join("\n");

    // The first body row (index 1: title is index 0) now announces what
    // scrolled off above, and the providers and gateway note the top
    // screen could not fit are on screen.
    let first_body_row = &bottom[1];
    assert!(
        first_body_row.contains('\u{2191}') && first_body_row.contains("above"),
        "the first body row must announce the rows above once scrolled, got:\n{first_body_row:?}"
    );

    let union = format!("{top_text}\n{bottom_text}");
    for required in [
        "Providers",
        "openrouter",
        "The Glasshouse gateway is not part of this setup yet.",
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
    let mut state = advance_to_harnesses(sample_state());
    for _ in 0..3 {
        state.handle_key(crossterm::event::KeyEvent::new(
            crossterm::event::KeyCode::Tab,
            crossterm::event::KeyModifiers::NONE,
        ));
    }
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
