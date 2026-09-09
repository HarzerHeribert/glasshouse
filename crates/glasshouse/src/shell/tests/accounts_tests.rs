//! Connecting an account: the one journey a new install cannot skip.
//!
//! Every test here answers the same question from a different surface — *can
//! somebody who has never read `glasshouse --help` find the way in?* — because
//! the defect being fixed was never that a capability was missing. Providers
//! could be added and subscriptions could be connected; neither said so
//! anywhere a person looks.

use super::*;

use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::layout::Rect;

use crossterm::event::{KeyEvent, KeyModifiers};

use crate::cli::SubscriptionProvider;
use crate::shell::state::{Action, BrokerState, SettingsRows, SubscriptionRow};
use crate::shell::view::render_recording;
use crate::subscription::{Account, Summary};

fn press(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

/// One frame, and everything it made clickable.
fn recorded(state: &ShellState, width: u16, height: u16) -> (String, Vec<Hotspot>) {
    let mut terminal = Terminal::new(TestBackend::new(width, height)).expect("terminal");
    let mut sink = Vec::new();
    terminal
        .draw(|frame| render_recording(state, frame, &mut sink))
        .expect("draw must not panic");
    let buffer = terminal.backend().buffer();
    let text = (0..buffer.area.height)
        .map(|y| {
            (0..buffer.area.width)
                .map(|x| buffer[(x, y)].symbol())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n");
    (text, sink)
}

/// The hotspot whose *painted cells* spell `needle`.
///
/// Read back off the frame rather than off the pill list, so a hotspot
/// recorded somewhere other than where the text was drawn does not count — the
/// invariant `hotspot`'s module doc rests on.
fn clickable(state: &ShellState, width: u16, height: u16, needle: &str) -> Option<Hotspot> {
    let (text, spots) = recorded(state, width, height);
    let rows: Vec<&str> = text.lines().collect();
    spots.into_iter().find(|spot| {
        let rect = spot.rect();
        rows[usize::from(rect.y)]
            .chars()
            .skip(usize::from(rect.x))
            .take(usize::from(rect.width))
            .collect::<String>()
            .contains(needle)
    })
}

fn account(entitlement: &str, present: bool) -> SubscriptionRow {
    SubscriptionRow::from_account(&Account {
        entitlement: entitlement.to_owned(),
        provider: SubscriptionProvider::Anthropic,
        credential_present: present,
    })
}

fn settings_open(rows: SettingsRows) -> ShellState {
    let mut state = ShellState::new("glasshouse", "/work/glasshouse", "0.1.0", Vec::new());
    state.open_settings_rows(rows);
    state
}

fn tab_to(state: &mut ShellState, target: SettingsSection) {
    for _ in 0..SettingsSection::ORDER.len() {
        if state.settings().unwrap().section() == target {
            return;
        }
        state.handle_key(press(KeyCode::Tab));
    }
    panic!("Tab never reached {target:?}");
}

/// **The packet's first acceptance, and the user's actual complaint.**
///
/// A terminal exactly eighty by twenty-four, nothing configured, and the
/// question is whether the interface contains a *visible, pressable* way to
/// connect an account. Before this it did not — subscriptions had a CLI
/// command and no surface at all, and the Providers section's `a` was one of
/// ten undocumented letters.
///
/// Asserted on both surfaces a person could be standing on: the control-mode
/// footer they see first, and the Subscriptions section they arrive at.
#[test]
fn a_fresh_install_can_see_and_press_a_way_to_connect_an_account_at_eighty_by_twenty_four() {
    let mut state = ShellState::new("glasshouse", "/work/glasshouse", "0.1.0", Vec::new());
    state.set_accounts(Summary {
        configured: 0,
        connected: 0,
        broker_adopted: false,
        read: true,
    });

    let (text, _) = recorded(&state, 80, 24);
    assert!(
        text.contains("c connect"),
        "control mode must offer connecting an account at 80x24:\n{text}"
    );
    let spot = clickable(&state, 80, 24, "c connect")
        .expect("the connect affordance must be pressable, not merely printed");
    assert_eq!(
        spot.keys(),
        [press(KeyCode::Char('c'))],
        "clicking it must be the key a user could have typed instead"
    );

    // And in Settings, where the account actually lives.
    let mut settings = settings_open(SettingsRows {
        subscriptions: vec![account("claude-max", false)],
        broker: BrokerState { adopted: true },
        ..SettingsRows::default()
    });
    tab_to(&mut settings, SettingsSection::Subscriptions);
    let (text, _) = recorded(&settings, 80, 24);
    assert!(
        text.contains("c connect"),
        "the Subscriptions section must offer connecting at 80x24:\n{text}"
    );
    assert!(
        clickable(&settings, 80, 24, "c connect").is_some(),
        "and it must be pressable"
    );
}

/// **The packet's second acceptance.** The section exists, Tab reaches it, and
/// what it renders is the act plus the exact command that performs it.
#[test]
fn the_subscriptions_section_is_reachable_by_tab_and_renders_its_connect_action() {
    assert!(
        SettingsSection::ORDER.contains(&SettingsSection::Subscriptions),
        "a section outside ORDER is unreachable by Tab and invisible in the strip"
    );

    let mut state = settings_open(SettingsRows {
        subscriptions: vec![account("claude-max", false)],
        broker: BrokerState { adopted: true },
        ..SettingsRows::default()
    });
    tab_to(&mut state, SettingsSection::Subscriptions);

    // 140: the strip spells the long labels out only where all seven fit.
    let (text, _) = recorded(&state, 140, 30);
    assert!(text.contains("Subscriptions"), "the tab must show:\n{text}");
    assert!(
        text.contains("claude-max"),
        "the account must be listed:\n{text}"
    );
    assert!(
        text.contains("not connected"),
        "and its measured state named:\n{text}"
    );
    assert!(
        text.contains("glasshouse subscriptions login anthropic --entitlement claude-max"),
        "the command that connects THIS account must be spelled out, with no \
         placeholder to guess at:\n{text}"
    );
}

/// A connected account offers the act that is left, never the one already
/// done.
#[test]
fn a_connected_account_offers_disconnect_and_never_calls_its_credential_valid() {
    let mut state = settings_open(SettingsRows {
        subscriptions: vec![account("claude-max", true)],
        broker: BrokerState { adopted: true },
        ..SettingsRows::default()
    });
    tab_to(&mut state, SettingsSection::Subscriptions);

    let (text, _) = recorded(&state, 120, 30);
    assert!(text.contains("credential present"), "got:\n{text}");
    assert!(
        text.contains("glasshouse subscriptions logout anthropic --entitlement claude-max"),
        "got:\n{text}"
    );
    let row = text
        .lines()
        .find(|line| line.contains("claude-max") && line.contains("credential"))
        .expect("the account's own row");
    assert!(
        !row.contains("valid") && !row.contains('\u{2713}'),
        "presence is a directory entry, and the row must not upgrade it to a \
         verdict: `{row}`"
    );
    assert!(
        text.contains("Presence is not validity"),
        "and the section must say so out loud:\n{text}"
    );
    assert!(
        clickable(&state, 120, 30, "x disconnect").is_some(),
        "the pill must follow the row's state and be pressable:\n{text}"
    );
}

/// Pressing the pill's key spells the command out in the bottom panel, with
/// the sentence saying why Glasshouse is not running it.
///
/// This is the behaviour that makes "we cannot host the browser flow" an
/// answer rather than an excuse: the user is handed the exact string.
#[test]
fn pressing_connect_spells_the_command_out_rather_than_running_anything() {
    let mut state = settings_open(SettingsRows {
        subscriptions: vec![account("claude-max", false)],
        broker: BrokerState { adopted: true },
        ..SettingsRows::default()
    });
    tab_to(&mut state, SettingsSection::Subscriptions);
    state.handle_key(press(KeyCode::Char('c')));

    assert_eq!(
        state.account_notice(),
        Some("glasshouse subscriptions login anthropic --entitlement claude-max"),
    );
    let (text, _) = recorded(&state, 120, 30);
    assert!(
        text.contains("Run this in another terminal"),
        "the panel must say where the command goes:\n{text}"
    );
}

/// An unadopted broker is the step *before* the login, and the section says so
/// instead of offering a command that would be refused.
#[test]
fn an_unadopted_broker_names_the_step_that_actually_comes_first() {
    let mut state = settings_open(SettingsRows {
        subscriptions: vec![account("claude-max", false)],
        broker: BrokerState { adopted: false },
        ..SettingsRows::default()
    });
    tab_to(&mut state, SettingsSection::Subscriptions);

    let (text, _) = recorded(&state, 120, 30);
    assert!(
        text.contains("adopt-binary"),
        "the first missing step must be the one named:\n{text}"
    );
}

/// **The packet's third acceptance.** Ranking the bar removed nothing: every
/// action is still reachable by its own key and still records a hotspot.
///
/// The key half matters as much as the click half. A demotion that quietly
/// unbound `d decisions` would look identical on screen to one that did not,
/// and the pill is defined as *the key a user could have typed instead* — so
/// this replays each pill's keys through `ShellState` and refuses an
/// `Action::None`.
#[test]
fn every_control_action_survives_the_ranking_by_key_and_by_click() {
    use crate::session::{
        SessionId, SessionLifecycle, SessionPresentation, SessionRecord, SessionRole,
    };

    let session = |id: &str| SessionRecord {
        id: SessionId::new(id),
        project_id: "project".to_owned(),
        harness: "claude-code".to_owned(),
        native_session_id: None,
        role: SessionRole::Normal,
        lifecycle: SessionLifecycle::Running,
        presentation: SessionPresentation::Embedded,
        created_at: 1_000,
        last_activity_at: 1_000,
        launch_profile: None,
        backend_resource: None,
        model: None,
        pairing_class: None,
        protocol: None,
        response_profile: None,
        response_mechanism: None,
        display_name: None,
        purpose: None,
        source_session_id: None,
        observed_compactions: None,
        presentation_ref: None,
        last_seen_commit: None,
        entitlement: None,
    };
    let sample = || {
        ShellState::new(
            "glasshouse",
            "/work/glasshouse",
            "0.1.0",
            vec![session("aaaaaaaaaaaa1"), session("bbbbbbbbbbbb2")],
        )
    };

    for pill in hotspot::control_pills(crate::shell::appearance::Theme::default()) {
        let text = pill.text();
        // Clickable: the pill's cells are in the footer's hotspot sink.
        let state = sample();
        let spot = clickable(&state, 80, 24, &text)
            .unwrap_or_else(|| panic!("`{text}` is drawn nowhere, or records no hotspot"));
        assert!(
            !spot.keys().is_empty(),
            "`{text}` is drawn as a button, so pressing it must do something"
        );

        // Reachable by key: replaying exactly those presses changes something.
        let mut state = sample();
        let mut last = Action::None;
        for key in spot.keys() {
            last = state.handle_key(*key);
        }
        assert_ne!(
            last,
            Action::None,
            "`{text}` is advertised in the bar, so its key must still be bound"
        );
    }
}

/// **The demotion is visible, not merely declared.**
///
/// The whole answer to *"5 million buttons"* is that three of them now look
/// different from the other thirteen. Asserted against the frame's styles
/// rather than its text, because the text is identical by design — a
/// subordinate pill keeps its width so the click target does not move. Without
/// this, removing `Pill::subordinate`'s effect entirely would pass every other
/// test in this file.
#[test]
fn the_demoted_actions_are_drawn_differently_from_the_primary_ones() {
    use ratatui::style::Modifier;

    let state = ShellState::new("glasshouse", "/work/glasshouse", "0.1.0", Vec::new());
    let mut terminal = Terminal::new(TestBackend::new(80, 24)).expect("terminal");
    let mut sink = Vec::new();
    terminal
        .draw(|frame| render_recording(&state, frame, &mut sink))
        .expect("draw");
    let buffer = terminal.backend().buffer().clone();

    // The mnemonic cell of a pill, read off the frame it was painted into.
    let style_of = |needle: &str| {
        let spot = sink
            .iter()
            .find(|spot| {
                let rect = spot.rect();
                (rect.x..rect.right())
                    .map(|x| buffer[(x, rect.y)].symbol())
                    .collect::<String>()
                    .contains(needle)
            })
            .unwrap_or_else(|| panic!("`{needle}` was drawn nowhere"));
        let rect = spot.rect();
        // Cell 2 is the mnemonic: cap, marker, then the mnemonic itself.
        buffer[(rect.x + 2, rect.y)].style()
    };

    let primary = style_of("n new");
    let demoted = style_of("d decisions");
    assert_ne!(
        primary.fg, demoted.fg,
        "a primary action and a demoted one must not be the same colour, or the \
         ranking exists only in the source"
    );
    assert!(
        primary.add_modifier.contains(Modifier::BOLD),
        "the primary run keeps the accent's weight"
    );
    assert!(
        !demoted.add_modifier.contains(Modifier::BOLD),
        "and the demoted run gives it up"
    );
}

/// **A band too short for two tiers keeps the demoted actions.**
///
/// The ranking splits the bar's rows between a primary run and a demoted one,
/// and a six-row terminal affords exactly one row of chrome. Reserving that
/// row for the three primaries would put `q quit` and everything after it on
/// no screen and in no hotspot — which is the 168-column clip the wrapped bar
/// was built to end, arriving through the new feature. The fallback draws one
/// flat row instead, and this is the test that watches it: a mutation dropping
/// the demoted run in that branch SURVIVED the whole `shell::view` suite
/// before this existed.
#[test]
fn a_band_too_short_for_two_tiers_still_draws_the_demoted_actions() {
    let state = ShellState::new("glasshouse", "/work/glasshouse", "0.1.0", Vec::new());
    let (text, _) = recorded(&state, 80, 6);
    assert!(
        text.contains("n new"),
        "the primary run leads what fits:\n{text}"
    );
    assert!(
        text.contains("q quit"),
        "and the demoted run is not dropped to make room for it:\n{text}"
    );
    assert!(
        clickable(&state, 80, 6, "q quit").is_some(),
        "a demoted action drawn on a short band must still be pressable"
    );
}

/// **The packet's fourth acceptance.** A first run — no sessions, no accounts
/// — names the next step, and does it on the panel that fills the screen
/// rather than in a footer hint.
#[test]
fn a_first_run_frame_names_the_next_step() {
    let mut state = ShellState::new("glasshouse", "/work/glasshouse", "0.1.0", Vec::new());
    state.set_accounts(Summary {
        configured: 0,
        connected: 0,
        broker_adopted: false,
        read: true,
    });
    let (text, _) = recorded(&state, 100, 30);
    assert!(
        text.contains("Next step: connect an account"),
        "an empty install must be told what to do first:\n{text}"
    );
}

/// The other half, and the reason [`Summary`] carries a `read` flag: a summary
/// nothing has read must claim nothing.
///
/// The mutation this kills is dropping the flag and testing `configured == 0`.
/// Every fixture in this crate that never calls `set_accounts` would then
/// render "connect an account" as though it had been checked, on projects that
/// have three accounts connected.
#[test]
fn an_unread_account_summary_puts_no_claim_on_the_screen() {
    let state = ShellState::new("glasshouse", "/work/glasshouse", "0.1.0", Vec::new());
    let (text, _) = recorded(&state, 100, 30);
    assert!(
        !text.contains("Next step:"),
        "nothing read the configuration, so nothing may be asserted about it:\n{text}"
    );

    let mut connected = ShellState::new("glasshouse", "/work/glasshouse", "0.1.0", Vec::new());
    connected.set_accounts(Summary {
        configured: 3,
        connected: 3,
        broker_adopted: true,
        read: true,
    });
    let (text, _) = recorded(&connected, 100, 30);
    assert!(
        !text.contains("Next step:"),
        "a user with accounts connected is not at step one:\n{text}"
    );
}

/// The Providers section's ten letters are drawn as buttons, `a add provider`
/// first — the half of the user's sentence about providers.
#[test]
fn the_providers_section_draws_its_actions_with_add_first() {
    let mut state = settings_open(SettingsRows::default());
    tab_to(&mut state, SettingsSection::Providers);

    let (text, _) = recorded(&state, 120, 30);
    assert!(
        text.contains("a add provider"),
        "the section's own first act must be a button:\n{text}"
    );
    let spot = clickable(&state, 120, 30, "a add provider").expect("and it must be pressable");
    assert_eq!(spot.keys(), [press(KeyCode::Char('a'))]);

    // Pressing it opens the wizard the letter always opened — the click is the
    // same door, not a parallel one.
    for key in spot.keys() {
        state.handle_key(*key);
    }
    assert!(
        state.settings().unwrap().provider_input().is_some(),
        "clicking `add provider` must open the same editor `a` does"
    );
}

/// Every section that has an act says what it is for, in one line, without
/// naming a key the section does not bind.
#[test]
fn every_section_carries_a_tip_that_is_about_that_section() {
    for section in SettingsSection::ORDER {
        let mut state = settings_open(SettingsRows {
            subscriptions: vec![account("claude-max", false)],
            broker: BrokerState { adopted: true },
            ..SettingsRows::default()
        });
        tab_to(&mut state, section);
        let tip = section_tip(state.settings().unwrap());
        assert!(
            tip.len() > 30,
            "{section:?}'s tip says nothing useful: `{tip}`"
        );
        let (text, _) = recorded(&state, 160, 30);
        let head: String = tip.chars().take(40).collect();
        assert!(
            text.contains(&head),
            "{section:?}'s tip must be on screen:\n{text}"
        );
    }
}

/// The overlay must survive every terminal a user can produce, including the
/// ones where the action band cannot fit at all.
#[test]
fn the_subscriptions_section_renders_at_absurd_sizes() {
    let mut state = settings_open(SettingsRows {
        subscriptions: vec![account("claude-max", true), account("chatgpt-pro", false)],
        broker: BrokerState { adopted: true },
        ..SettingsRows::default()
    });
    tab_to(&mut state, SettingsSection::Subscriptions);
    state.handle_key(press(KeyCode::Char('c')));
    for (w, h) in [(1, 1), (1, 40), (40, 1), (3, 3), (80, 24), (300, 80)] {
        recorded(&state, w, h);
    }
}

/// The band is reserved and painted from the same walk, so the section's list
/// starts where the band ends. A band that over-reserved would eat rows the
/// account list needs; one that under-reserved would paint pills over them.
#[test]
fn the_action_band_reserves_exactly_the_rows_it_paints() {
    let mut state = settings_open(SettingsRows::default());
    tab_to(&mut state, SettingsSection::Providers);
    let settings = state.settings().unwrap();
    for width in [40u16, 80, 100, 160] {
        let area = Rect::new(0, 0, width, 30);
        let rows = band_rows(settings, area);
        assert!(rows >= 1, "the tip always has a row");
        assert!(
            rows <= area.height / 2,
            "the band may never take more than half the list's room"
        );
    }
}

/// A settings frame is not allowed to leak a hotspot from the bands under it.
///
/// `render_recording` clears the sink when an overlay is open, and the action
/// band is drawn after that clear; this is the test that would fail if the
/// band were ever moved before it, which would leave the control-mode footer's
/// pills clickable through the popup drawn over them.
#[test]
fn only_the_overlays_own_pills_are_clickable_while_settings_is_open() {
    let mut state = settings_open(SettingsRows::default());
    tab_to(&mut state, SettingsSection::Providers);
    assert!(
        clickable(&state, 120, 30, "q quit").is_none(),
        "the footer's pills are under the popup and must not be pressable"
    );
    assert!(
        clickable(&state, 120, 30, "a add provider").is_some(),
        "the overlay's own pills must be"
    );
}

/// **A section the strip cannot fit is panned to, never dropped in silence.**
///
/// Seven sections do not fit an eighty-column terminal at any spelling — the
/// popup's inner width is seventy and the short labels need seventy-nine — and
/// the plain `Paragraph` this replaced drew `Memory` nowhere, with no marker
/// and nothing saying it existed. Asserted for every section at every width a
/// terminal plausibly is: whichever one has the cursor is on screen, and the
/// strip says when it moved past others to get there.
#[test]
fn the_settings_tab_strip_never_hides_the_section_that_has_the_cursor() {
    for width in [60u16, 80, 100, 120, 160] {
        for section in SettingsSection::ORDER {
            let mut state = settings_open(SettingsRows::default());
            tab_to(&mut state, section);
            let (text, _) = recorded(&state, width, 30);
            let strip = text.lines().nth(4).unwrap_or_default();
            let expected = if strip.contains(section.label()) {
                section.label()
            } else {
                section.short_label()
            };
            assert!(
                strip.contains(expected),
                "at {width} columns the strip does not show {section:?}, which has \
                 the cursor: `{strip}`"
            );
            let shown = SettingsSection::ORDER
                .iter()
                .filter(|other| {
                    strip.contains(other.label()) || strip.contains(other.short_label())
                })
                .count();
            if shown < SettingsSection::ORDER.len() {
                assert!(
                    strip.contains('\u{2039}') || strip.contains('\u{203a}'),
                    "at {width} columns {} of 7 sections are drawn and the strip does \
                     not say the rest exist: `{strip}`",
                    shown
                );
            }
        }
    }
}

/// Clicking a tab is the Tab presses a user would have made, not a second way
/// to move the cursor.
#[test]
fn clicking_a_settings_tab_is_the_tab_presses_that_reach_it() {
    let mut state = settings_open(SettingsRows::default());
    let spot = clickable(&state, 160, 30, "Routing").expect("the Routing tab must be pressable");
    assert!(
        spot.keys().iter().all(|key| key.code == KeyCode::Tab),
        "reaching a later section is forward Tabs, exactly as typed"
    );
    for key in spot.keys() {
        state.handle_key(*key);
    }
    assert_eq!(
        state.settings().unwrap().section(),
        SettingsSection::Routing
    );
}

/// Not an assertion — a printer. `cargo test -- --nocapture --ignored
/// shell::view::settings_actions::tests::print_frames` shows the real frames
/// this package changed, at the size the user's terminal actually is.
#[test]
#[ignore = "prints frames for a human to read"]
fn print_frames() {
    let mut state = ShellState::new("glasshouse", "/work/glasshouse", "0.1.0", Vec::new());
    state.set_accounts(Summary {
        configured: 0,
        connected: 0,
        broker_adopted: false,
        read: true,
    });
    let (text, _) = recorded(&state, 80, 24);
    println!("=== control mode, first run, 80x24 ===\n{text}");

    let mut settings = settings_open(SettingsRows {
        subscriptions: vec![account("claude-max", true), account("chatgpt-pro", false)],
        broker: BrokerState { adopted: true },
        ..SettingsRows::default()
    });
    tab_to(&mut settings, SettingsSection::Subscriptions);
    let (text, _) = recorded(&settings, 80, 24);
    println!("=== settings / Subscriptions, 80x24 ===\n{text}");

    let mut empty = settings_open(SettingsRows::default());
    tab_to(&mut empty, SettingsSection::Subscriptions);
    let (text, _) = recorded(&empty, 100, 30);
    println!("=== settings / Subscriptions, nothing configured, 100x30 ===\n{text}");

    let mut providers = settings_open(SettingsRows::default());
    tab_to(&mut providers, SettingsSection::Providers);
    let (text, _) = recorded(&providers, 100, 30);
    println!("=== settings / Providers, 100x30 ===\n{text}");
}
