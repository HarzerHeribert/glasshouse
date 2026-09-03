use super::*;
use crate::session::{SessionId, SessionLifecycle, SessionPresentation, SessionRole};

fn record(id: &str, harness: &str) -> SessionRecord {
    SessionRecord {
        id: SessionId::new(id),
        project_id: "project".to_owned(),
        harness: harness.to_owned(),
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
    }
}

fn state_with(count: usize) -> ShellState {
    let sessions = (0..count)
        .map(|i| record(&format!("id-{i}"), "claude-code"))
        .collect();
    ShellState::new("glasshouse", "/projects/glasshouse", "0.1.0", sessions)
}

fn press(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

#[test]
fn the_active_project_root_is_always_available_to_the_view() {
    let state = state_with(1);
    assert_eq!(state.project_root(), Path::new("/projects/glasshouse"));
    assert_eq!(state.project_name(), "glasshouse");
}

#[test]
fn tab_moves_to_the_next_session_and_wraps() {
    let mut state = state_with(3);
    assert_eq!(state.selected_index(), 0);
    assert_eq!(state.handle_key(press(KeyCode::Tab)), Action::Redraw);
    assert_eq!(state.selected_index(), 1);
    state.handle_key(press(KeyCode::Tab));
    assert_eq!(state.selected_index(), 2);
    state.handle_key(press(KeyCode::Tab));
    assert_eq!(
        state.selected_index(),
        0,
        "the last entry wraps to the first"
    );
}

#[test]
fn shift_tab_moves_to_the_previous_session_and_wraps() {
    let mut state = state_with(3);
    assert_eq!(state.handle_key(press(KeyCode::BackTab)), Action::Redraw);
    assert_eq!(
        state.selected_index(),
        2,
        "the first entry wraps to the last"
    );
    state.handle_key(press(KeyCode::BackTab));
    assert_eq!(state.selected_index(), 1);
}

#[test]
fn arrow_keys_navigate_the_same_way_as_tab() {
    let mut state = state_with(3);
    state.handle_key(press(KeyCode::Right));
    assert_eq!(state.selected_index(), 1);
    state.handle_key(press(KeyCode::Left));
    assert_eq!(state.selected_index(), 0);
}

/// Navigation presents a different session; it must never look like
/// anything happened to the sessions themselves.
#[test]
fn navigating_changes_only_which_session_is_presented() {
    let mut state = state_with(3);
    let before: Vec<_> = state.sessions().to_vec();

    state.handle_key(press(KeyCode::Tab));
    state.handle_key(press(KeyCode::Tab));
    state.handle_key(press(KeyCode::BackTab));

    assert_eq!(
        state.sessions(),
        before.as_slice(),
        "no session was altered"
    );
    assert_eq!(state.selected_index(), 1);
}

/// With nothing to move between, the key must explain itself rather than
/// appear broken — but it must not move the selection.
#[test]
fn navigating_with_fewer_than_two_sessions_explains_itself() {
    for count in [0, 1] {
        let mut state = state_with(count);
        assert_eq!(state.handle_key(press(KeyCode::Tab)), Action::Redraw);
        assert_eq!(state.selected_index(), 0, "{count} sessions: nothing moved");
        let status = state.status().unwrap_or_else(|| {
            panic!("{count} sessions: the key must leave a note explaining itself")
        });
        assert!(!status.is_empty());
    }
    assert!(
        state_with(0).sessions().is_empty(),
        "the empty case must not invent a session"
    );
}

/// A note explains the key just pressed, so the next key clears it instead
/// of leaving stale text sitting under a new action.
#[test]
fn a_status_note_is_cleared_by_the_next_keystroke() {
    let mut state = state_with(1);
    state.handle_key(press(KeyCode::Tab));
    assert!(state.status().is_some());

    assert_eq!(
        state.handle_key(press(KeyCode::Char('z'))),
        Action::Redraw,
        "clearing a note is a visible change"
    );
    assert!(state.status().is_none(), "the note must not linger");

    assert_eq!(
        state.handle_key(press(KeyCode::Char('z'))),
        Action::None,
        "with no note to clear, an unknown key costs nothing"
    );
}

#[test]
fn an_empty_project_has_no_active_session_and_does_not_panic() {
    let mut state = state_with(0);
    assert!(state.active_session().is_none());
    state.handle_key(press(KeyCode::Tab));
    state.handle_key(press(KeyCode::BackTab));
    assert!(state.active_session().is_none());
}

#[test]
fn o_opens_the_session_overview_from_the_keyboard() {
    let mut state = state_with(2);
    assert_eq!(state.overlay(), None);
    assert_eq!(state.handle_key(press(KeyCode::Char('o'))), Action::Redraw);
    assert_eq!(state.overlay(), Some(Overlay::Overview));
    assert_eq!(
        state.handle_key(press(KeyCode::Char('o'))),
        Action::Redraw,
        "the same key closes it again"
    );
    assert_eq!(state.overlay(), None);
}

/// The capability that separates an overlay from a modal takeover: leaving
/// it returns to the session that was already active, and the session is
/// untouched.
#[test]
fn leaving_an_overlay_returns_to_the_active_session_without_ending_it() {
    let mut state = state_with(3);
    state.handle_key(press(KeyCode::Tab));
    let active = state.active_session().expect("a session").clone();

    state.handle_key(press(KeyCode::Char('o')));
    assert_eq!(state.overlay(), Some(Overlay::Overview));
    assert_eq!(
        state.active_session(),
        Some(&active),
        "opening an overlay must not change which session is active"
    );

    assert_eq!(state.handle_key(press(KeyCode::Esc)), Action::Redraw);
    assert_eq!(state.overlay(), None, "Escape leaves the overlay");
    assert_eq!(
        state.active_session(),
        Some(&active),
        "the same session is still presented, unchanged"
    );
    assert_eq!(state.sessions().len(), 3, "no session was closed");
}

/// Escape has to mean two things, and which one depends on whether an
/// overlay is open. Getting this backwards would make Escape close
/// Glasshouse from inside the overview.
#[test]
fn escape_leaves_an_overlay_first_and_only_then_leaves_glasshouse() {
    let mut state = state_with(2);
    state.handle_key(press(KeyCode::Char('o')));
    assert_eq!(state.handle_key(press(KeyCode::Esc)), Action::Redraw);
    assert_eq!(state.handle_key(press(KeyCode::Esc)), Action::Quit);
}

#[test]
fn quit_keys_are_q_escape_and_ctrl_c() {
    for key in [
        press(KeyCode::Char('q')),
        press(KeyCode::Esc),
        KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL),
    ] {
        let mut state = state_with(2);
        assert_eq!(state.handle_key(key), Action::Quit, "{key:?} should quit");
    }
}

#[test]
fn an_unrecognized_key_costs_nothing() {
    let mut state = state_with(2);
    assert_eq!(state.handle_key(press(KeyCode::Char('z'))), Action::None);
    // `Enter` is no longer unbound: it now enters session mode (see the
    // tests below). `F(1)` stands in as a key with genuinely nothing
    // bound to it in control mode.
    assert_eq!(state.handle_key(press(KeyCode::F(1))), Action::None);
}

/// A refresh reorders the list, because sessions sort by last activity.
/// Following the identifier rather than the index is what stops the user
/// being moved to a different session by a background update.
#[test]
fn a_refresh_keeps_the_same_session_presented_even_when_the_order_changes() {
    let mut state = ShellState::new(
        "p",
        "/p",
        "0.1.0",
        vec![record("a", "claude-code"), record("b", "codex")],
    );
    state.handle_key(press(KeyCode::Tab));
    assert_eq!(state.active_session().unwrap().id.as_str(), "b");

    // "b" is now first, as a fresh `list()` ordered by activity would give.
    state.refresh(vec![record("b", "codex"), record("a", "claude-code")]);
    assert_eq!(
        state.active_session().unwrap().id.as_str(),
        "b",
        "the presented session must follow its identifier, not its position"
    );
    assert_eq!(state.selected_index(), 0);
}

#[test]
fn a_refresh_that_removes_the_active_session_falls_back_to_the_first() {
    let mut state = ShellState::new(
        "p",
        "/p",
        "0.1.0",
        vec![record("a", "claude-code"), record("b", "codex")],
    );
    state.handle_key(press(KeyCode::Tab));
    state.refresh(vec![record("a", "claude-code")]);
    assert_eq!(state.active_session().unwrap().id.as_str(), "a");
}

/// Redrawing on every poll would make the interface flicker and burn CPU
/// on an idle project, so an unchanged list must report no change.
#[test]
fn a_refresh_with_identical_sessions_does_not_ask_for_a_redraw() {
    let mut state = state_with(2);
    let same: Vec<_> = state.sessions().to_vec();
    assert_eq!(state.refresh(same), Action::None);
}

// -----------------------------------------------------------------
// Session modes — `.agent-runtime/design-shell-session-modes.md`.
// -----------------------------------------------------------------

/// Invariant 1: in session mode, `q` reaches the harness and does not
/// quit Glasshouse.
#[test]
fn in_session_mode_q_reaches_the_harness_and_does_not_quit() {
    let mut state = state_with(1);
    assert_eq!(state.handle_key(press(KeyCode::Enter)), Action::Redraw);
    assert_eq!(state.mode(), Mode::Session);

    assert_eq!(
        state.handle_key(press(KeyCode::Char('q'))),
        Action::Forward(b"q".to_vec())
    );
    assert_eq!(
        state.mode(),
        Mode::Session,
        "q must not quit Glasshouse in session mode"
    );
}

/// Invariant 2: in session mode, Ctrl-C reaches the harness as a byte,
/// and does not quit. This is the entire reason `RawModeGuard` exists
/// rather than `TerminalGuard` — Ctrl-C belongs to the harness here.
#[test]
fn in_session_mode_ctrl_c_reaches_the_harness_as_a_byte_and_does_not_quit() {
    let mut state = state_with(1);
    state.handle_key(press(KeyCode::Enter));
    assert_eq!(state.mode(), Mode::Session);

    let ctrl_c = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
    assert_eq!(state.handle_key(ctrl_c), Action::Forward(vec![0x03]));
    assert_eq!(
        state.mode(),
        Mode::Session,
        "Ctrl-C belongs to the harness in session mode, not to Glasshouse"
    );
}

/// Invariant 3: `Ctrl-]` returns to control mode from session mode, and
/// is never forwarded — it is intercepted before `encode` ever runs.
#[test]
fn ctrl_bracket_returns_to_control_mode_and_is_never_forwarded() {
    let mut state = state_with(1);
    state.handle_key(press(KeyCode::Enter));
    assert_eq!(state.mode(), Mode::Session);

    let escape = KeyEvent::new(KeyCode::Char(']'), KeyModifiers::CONTROL);
    assert_eq!(
        state.handle_key(escape),
        Action::Redraw,
        "the escape chord must not be forwarded as an Action::Forward"
    );
    assert_eq!(state.mode(), Mode::Control);
}

/// Invariant 4: entering and leaving session mode never touches any
/// process. `ShellState` has no field for one — it holds only the
/// viewport text the run loop hands it — so this proves that guarantee
/// against a real running process rather than by inspection of the type
/// alone: same pid, still running, and its output (stood in for here by
/// the viewport text, since `ShellState` does not read a scrollback
/// itself) is untouched by the mode switch either way.
#[test]
fn entering_and_leaving_session_mode_never_touches_a_real_process() {
    let mut child = spawn_long_lived();
    let pid_before = child.id();
    assert!(
        child.try_wait().expect("try_wait").is_none(),
        "the process must start out running"
    );

    let mut state = state_with(1);
    let growing = ViewportGrid::new(1, 1, vec![("g".to_owned(), Style::default())], None);
    state.set_viewport_grid(growing.clone());
    state.handle_key(press(KeyCode::Enter));
    assert_eq!(state.mode(), Mode::Session);
    assert_eq!(state.viewport_grid(), &growing);

    let grown = ViewportGrid::new(1, 1, vec![("m".to_owned(), Style::default())], None);
    state.set_viewport_grid(grown.clone());
    let escape = KeyEvent::new(KeyCode::Char(']'), KeyModifiers::CONTROL);
    state.handle_key(escape);
    assert_eq!(state.mode(), Mode::Control);
    assert_eq!(state.viewport_grid(), &grown);

    assert_eq!(
        child.id(),
        pid_before,
        "the pid must be unaffected by a mode switch"
    );
    assert!(
        child.try_wait().expect("try_wait").is_none(),
        "the process must still be running after the mode switch"
    );

    child.kill().expect("kill");
    child.wait().expect("reap");
}

/// Invariant 5: with nothing focused, session mode cannot be entered —
/// there is nowhere to send the keys.
#[test]
fn session_mode_cannot_be_entered_with_nothing_focused() {
    let mut state = state_with(0);
    assert!(state.active_session().is_none());

    assert_eq!(state.handle_key(press(KeyCode::Enter)), Action::Redraw);
    assert_eq!(
        state.mode(),
        Mode::Control,
        "there is nowhere to send session-mode keystrokes"
    );
    assert!(
        state.status().is_some(),
        "the key must explain why nothing happened"
    );
}

/// Invariant 6: a session exiting while in session mode drops back to
/// control mode rather than leaving keystrokes going nowhere.
#[test]
fn a_session_exiting_in_session_mode_drops_back_to_control_mode() {
    let mut state = state_with(1);
    state.handle_key(press(KeyCode::Enter));
    assert_eq!(state.mode(), Mode::Session);

    assert_eq!(state.session_exited(), Action::Redraw);
    assert_eq!(state.mode(), Mode::Control);

    // Idempotent: already in control mode, so there is nothing left to do.
    assert_eq!(state.session_exited(), Action::None);
}

#[test]
fn session_mode_is_entered_with_enter_or_i() {
    for key in [press(KeyCode::Enter), press(KeyCode::Char('i'))] {
        let mut state = state_with(1);
        assert_eq!(state.handle_key(key), Action::Redraw);
        assert_eq!(
            state.mode(),
            Mode::Session,
            "{key:?} should enter session mode"
        );
    }
}

/// Session mode and an overlay must never coexist — see the design
/// note's "Overlays are control-mode only."
#[test]
fn entering_session_mode_closes_any_open_overlay() {
    let mut state = state_with(2);
    state.handle_key(press(KeyCode::Char('o')));
    assert_eq!(state.overlay(), Some(Overlay::Overview));

    state.handle_key(press(KeyCode::Enter));
    assert_eq!(state.mode(), Mode::Session);
    assert_eq!(
        state.overlay(),
        None,
        "session mode and an overlay must never coexist"
    );
}

#[test]
fn n_starts_a_new_session_from_control_mode() {
    let mut state = state_with(1);
    assert_eq!(
        state.handle_key(press(KeyCode::Char('n'))),
        Action::StartSession
    );
    assert_eq!(
        state.mode(),
        Mode::Control,
        "requesting a session does not itself change mode"
    );
}

#[test]
fn encode_translates_the_documented_keys_to_their_bytes() {
    assert_eq!(encode(press(KeyCode::Char('a'))), Some(b"a".to_vec()));
    assert_eq!(encode(press(KeyCode::Enter)), Some(vec![b'\r']));
    assert_eq!(encode(press(KeyCode::Backspace)), Some(vec![0x7f]));
    assert_eq!(encode(press(KeyCode::Tab)), Some(vec![b'\t']));
    assert_eq!(encode(press(KeyCode::Esc)), Some(vec![0x1b]));
    assert_eq!(encode(press(KeyCode::Up)), Some(b"\x1b[A".to_vec()));
    assert_eq!(encode(press(KeyCode::Down)), Some(b"\x1b[B".to_vec()));
    assert_eq!(encode(press(KeyCode::Right)), Some(b"\x1b[C".to_vec()));
    assert_eq!(encode(press(KeyCode::Left)), Some(b"\x1b[D".to_vec()));

    let ctrl_a = KeyEvent::new(KeyCode::Char('a'), KeyModifiers::CONTROL);
    assert_eq!(encode(ctrl_a), Some(vec![0x01]), "Ctrl-A is 0x01");
    let ctrl_z = KeyEvent::new(KeyCode::Char('z'), KeyModifiers::CONTROL);
    assert_eq!(encode(ctrl_z), Some(vec![0x1a]), "Ctrl-Z is 0x1a");

    // A multi-byte character must survive as UTF-8, not be truncated.
    assert_eq!(
        encode(press(KeyCode::Char('é'))),
        Some("é".as_bytes().to_vec())
    );
}

#[test]
fn encode_has_nothing_to_send_for_an_unmapped_key() {
    assert_eq!(encode(press(KeyCode::F(1))), None);
}

/// A real, long-lived child process for [`entering_and_leaving_session_mode_never_touches_a_real_process`].
/// Not a PTY session — nothing here is testing the harness launch seam,
/// only that `ShellState`'s own methods cannot reach a process at all.
fn spawn_long_lived() -> std::process::Child {
    if cfg!(windows) {
        std::process::Command::new("ping")
            .args(["-n", "5", "127.0.0.1"])
            .stdout(std::process::Stdio::null())
            .spawn()
            .expect("spawn ping")
    } else {
        std::process::Command::new("sleep")
            .arg("5")
            .spawn()
            .expect("spawn sleep")
    }
}

#[cfg(test)]
mod escape_chord_tests {
    use super::*;

    /// Both spellings of the escape chord must work, because a terminal sends
    /// the byte `0x1D` and the two platforms' parsers name it differently.
    ///
    /// Crossterm's Unix parser turns `0x1C..=0x1F` into `Char('4'..='7')` with
    /// CONTROL, so `Ctrl-]` arrives as `Ctrl-5`; Windows reads virtual key
    /// codes and gives `Char(']')`. Accepting only one leaves users of the
    /// other platform trapped in session mode.
    #[test]
    fn the_escape_chord_is_recognised_in_both_platform_spellings() {
        for code in [KeyCode::Char(']'), KeyCode::Char('5')] {
            assert!(
                is_session_escape(&KeyEvent::new(code, KeyModifiers::CONTROL)),
                "{code:?} with CONTROL must escape session mode"
            );
        }
    }

    /// The byte Crossterm's Unix parser produces for `Ctrl-]`, derived the same
    /// way Crossterm derives it, so this test fails if that mapping is ever
    /// misremembered.
    #[test]
    fn the_unix_spelling_matches_how_crossterm_decodes_the_byte() {
        const CTRL_RIGHT_BRACKET: u8 = 0x1D;
        let decoded = (CTRL_RIGHT_BRACKET - 0x1C + b'4') as char;
        assert_eq!(decoded, '5');
        assert!(is_session_escape(&KeyEvent::new(
            KeyCode::Char(decoded),
            KeyModifiers::CONTROL
        )));
    }

    /// Without CONTROL these are ordinary characters a harness must receive.
    #[test]
    fn the_bare_characters_are_not_an_escape() {
        for code in [KeyCode::Char(']'), KeyCode::Char('5')] {
            assert!(!is_session_escape(&KeyEvent::new(code, KeyModifiers::NONE)));
        }
    }

    /// The spelling the CI machine actually delivers, and the reason the
    /// Windows arm cannot be a list of characters.
    ///
    /// `'+'` is what a German physical layout puts on the key a US layout
    /// calls `']'`, and Crossterm reports the layout's character rather than
    /// the `0x1D` the console record carries. `'#'`, `'ü'` and `'$'` are the
    /// same key on three more layouts; none of them could have been guessed,
    /// and the rule has to hold for all of them.
    #[cfg(windows)]
    #[test]
    fn the_windows_spelling_is_whatever_the_layout_puts_on_that_key() {
        for code in [
            KeyCode::Char('+'),
            KeyCode::Char('#'),
            KeyCode::Char('ü'),
            KeyCode::Char('$'),
        ] {
            assert!(
                is_session_escape(&KeyEvent::new(code, KeyModifiers::CONTROL)),
                "{code:?} with CONTROL must escape session mode on Windows"
            );
        }
    }

    /// A bracket typed with `AltGr` is a bracket, not an escape.
    ///
    /// Windows reports `AltGr` as `CONTROL | ALT`, and on every layout where
    /// `']'` needs `AltGr` this is how a user types one into a harness. The
    /// chord itself never carries `ALT`.
    #[cfg(windows)]
    #[test]
    fn a_character_typed_with_altgr_is_not_an_escape() {
        for code in [KeyCode::Char(']'), KeyCode::Char('+'), KeyCode::Char('}')] {
            assert!(
                !is_session_escape(&KeyEvent::new(
                    code,
                    KeyModifiers::CONTROL | KeyModifiers::ALT
                )),
                "{code:?} with AltGr is a character the harness must receive"
            );
        }
    }

    /// The widened Windows rule must not eat the chords a harness lives on.
    ///
    /// `Ctrl-C` cancelling a turn is the one that would hurt, and it is the
    /// reason the rule tests for a non-alphanumeric character rather than for
    /// "not `']'`".
    #[cfg(windows)]
    #[test]
    fn a_control_letter_or_digit_still_reaches_the_harness() {
        for code in [
            KeyCode::Char('c'),
            KeyCode::Char('C'),
            KeyCode::Char('d'),
            KeyCode::Char('z'),
            KeyCode::Char('0'),
        ] {
            assert!(
                !is_session_escape(&KeyEvent::new(code, KeyModifiers::CONTROL)),
                "{code:?} with CONTROL belongs to the harness"
            );
        }
    }
}

#[cfg(test)]
mod native_input_tests {
    use super::*;
    use crate::session::{
        SessionId, SessionLifecycle, SessionPresentation, SessionRecord, SessionRole,
    };

    fn session_state() -> ShellState {
        let record = SessionRecord {
            id: SessionId::new("live"),
            project_id: "p".to_owned(),
            harness: "claude-code".to_owned(),
            native_session_id: None,
            role: SessionRole::Normal,
            lifecycle: SessionLifecycle::Running,
            presentation: SessionPresentation::Embedded,
            created_at: 0,
            last_activity_at: 0,
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
        let mut state = ShellState::new("p", "/p", "0.1.0", vec![record]);
        state.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert_eq!(state.mode(), Mode::Session);
        state
    }

    /// A harness's own slash commands must reach it verbatim. Glasshouse never
    /// interprets `/`, so `/compact` or `/model` is typed at the harness the
    /// same way it would be with no Glasshouse in between.
    #[test]
    fn a_slash_command_passes_straight_through_to_the_harness() {
        let mut state = session_state();
        let mut sent = Vec::new();
        for key in "/compact".chars() {
            match state.handle_key(KeyEvent::new(KeyCode::Char(key), KeyModifiers::NONE)) {
                Action::Forward(bytes) => sent.extend(bytes),
                other => panic!("`{key}` was intercepted as {other:?} instead of forwarded"),
            }
        }
        assert_eq!(String::from_utf8(sent).unwrap(), "/compact");
        assert_eq!(state.mode(), Mode::Session, "typing must not change mode");
    }

    /// There is no Glasshouse composer: input is forwarded key by key as the
    /// harness's own interface expects, so its editing, history, and completion
    /// all keep working. Every key Glasshouse binds in control mode must be
    /// forwarded here instead.
    #[test]
    fn keys_glasshouse_binds_elsewhere_belong_to_the_harness_in_session_mode() {
        for (code, expected) in [
            (KeyCode::Char('q'), vec![b'q']),
            (KeyCode::Char('n'), vec![b'n']),
            (KeyCode::Char('o'), vec![b'o']),
            (KeyCode::Char('i'), vec![b'i']),
            (KeyCode::Tab, vec![b'\t']),
            (KeyCode::Esc, vec![0x1b]),
            (KeyCode::Enter, vec![b'\r']),
            (KeyCode::Backspace, vec![0x7f]),
            (KeyCode::Up, b"\x1b[A".to_vec()),
        ] {
            let mut state = session_state();
            assert_eq!(
                state.handle_key(KeyEvent::new(code, KeyModifiers::NONE)),
                Action::Forward(expected),
                "{code:?} must reach the harness in session mode"
            );
            assert_eq!(state.mode(), Mode::Session);
        }
    }

    /// The escape captures input for Glasshouse only until the user hands it
    /// back, which is what "temporarily, without permanently stealing" means:
    /// the session is untouched and re-entering resumes forwarding.
    #[test]
    fn the_escape_captures_input_only_until_it_is_handed_back() {
        let mut state = session_state();
        let before = state.sessions().to_vec();

        state.handle_key(KeyEvent::new(KeyCode::Char(']'), KeyModifiers::CONTROL));
        assert_eq!(state.mode(), Mode::Control);
        assert_eq!(
            state.handle_key(KeyEvent::new(KeyCode::Char('o'), KeyModifiers::NONE)),
            Action::Redraw,
            "control mode's own bindings work again"
        );
        state.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));

        state.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert_eq!(
            state.mode(),
            Mode::Session,
            "input goes back to the harness"
        );
        assert_eq!(
            state.handle_key(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE)),
            Action::Forward(vec![b'q'])
        );
        assert_eq!(
            state.sessions(),
            before.as_slice(),
            "no session was touched"
        );
    }
}

#[cfg(test)]
mod settings_tests {
    use super::*;
    use crate::session::{SessionId, SessionLifecycle, SessionPresentation, SessionRole};

    fn record(id: &str) -> SessionRecord {
        SessionRecord {
            id: SessionId::new(id),
            project_id: "p".to_owned(),
            harness: "claude-code".to_owned(),
            native_session_id: None,
            role: SessionRole::Normal,
            lifecycle: SessionLifecycle::Running,
            presentation: SessionPresentation::Embedded,
            created_at: 0,
            last_activity_at: 0,
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
        }
    }

    fn state_with_a_session() -> ShellState {
        ShellState::new("p", "/p", "0.1.0", vec![record("only")])
    }

    fn press(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn sample_harness_rows() -> Vec<HarnessRow> {
        vec![
            HarnessRow {
                id: IntegrationId::ClaudeCode,
                detected: true,
                enabled: true,
                enabled_layer: Layer::User,
                executable: Some(PathBuf::from("/usr/bin/claude")),
                executable_layer: Some(Layer::User),
            },
            HarnessRow {
                id: IntegrationId::Codex,
                detected: false,
                enabled: false,
                enabled_layer: Layer::Default,
                executable: None,
                executable_layer: None,
            },
        ]
    }

    fn sample_integration_rows() -> Vec<IntegrationRow> {
        vec![IntegrationRow {
            id: IntegrationId::Ollama,
            detected: false,
            status: IntegrationStatus::NotFound,
        }]
    }

    fn sample_provider_rows() -> Vec<ProviderRow> {
        let mut config = ProviderConfig::new("openrouter");
        config.set_base_url(Some("https://mirror.example.com/v1".to_owned()));
        vec![ProviderRow::new("my-router", config, Layer::User)]
    }

    fn sample_profile_rows() -> Vec<ProfileRow> {
        vec![ProfileRow {
            name: "fast".to_owned(),
            config: ProfileConfig::new(IntegrationId::ClaudeCode),
            layer: Layer::User,
        }]
    }

    fn type_text(state: &mut ShellState, text: &str) {
        for c in text.chars() {
            state.handle_key(press(KeyCode::Char(c)));
        }
    }

    // Invariant 1 (design note): opening settings does not disturb the
    // session, and leaving it returns to the mode the user was in.
    #[test]
    fn opening_and_closing_settings_leaves_mode_and_session_untouched() {
        let mut state = state_with_a_session();
        let before = state.sessions().to_vec();

        assert_eq!(
            state.handle_key(press(KeyCode::Char('s'))),
            Action::OpenSettings
        );
        assert_eq!(
            state.overlay(),
            None,
            "opening settings needs data only the run loop can gather"
        );

        state.open_settings(
            sample_harness_rows(),
            sample_integration_rows(),
            Vec::new(),
            Vec::new(),
        );
        assert_eq!(state.overlay(), Some(Overlay::Settings));
        assert_eq!(state.mode(), Mode::Control);

        assert_eq!(state.handle_key(press(KeyCode::Esc)), Action::Redraw);
        assert_eq!(state.overlay(), None);
        assert_eq!(
            state.mode(),
            Mode::Control,
            "closing settings must not disturb the mode"
        );
        assert_eq!(
            state.sessions(),
            before.as_slice(),
            "no session was touched"
        );
    }

    /// `s` is an ordinary character to a harness in session mode: it must
    /// not open settings out from under it.
    #[test]
    fn s_is_forwarded_to_the_harness_in_session_mode_not_intercepted() {
        let mut state = state_with_a_session();
        state.handle_key(press(KeyCode::Enter));
        assert_eq!(state.mode(), Mode::Session);

        assert_eq!(
            state.handle_key(press(KeyCode::Char('s'))),
            Action::Forward(b"s".to_vec())
        );
        assert_eq!(state.overlay(), None);
    }

    #[test]
    fn tab_moves_between_sections_and_up_down_moves_within_one() {
        let mut state = state_with_a_session();
        state.open_settings(
            sample_harness_rows(),
            sample_integration_rows(),
            Vec::new(),
            Vec::new(),
        );
        assert_eq!(
            state.settings().unwrap().section(),
            SettingsSection::Harnesses
        );

        state.handle_key(press(KeyCode::Tab));
        assert_eq!(
            state.settings().unwrap().section(),
            SettingsSection::Integrations
        );
        state.handle_key(press(KeyCode::Left));
        assert_eq!(
            state.settings().unwrap().section(),
            SettingsSection::Harnesses
        );

        assert_eq!(state.settings().unwrap().selected_harness(), 0);
        state.handle_key(press(KeyCode::Down));
        assert_eq!(state.settings().unwrap().selected_harness(), 1);
        state.handle_key(press(KeyCode::Down));
        assert_eq!(
            state.settings().unwrap().selected_harness(),
            1,
            "selection clamps at the last row"
        );
        state.handle_key(press(KeyCode::Up));
        assert_eq!(state.settings().unwrap().selected_harness(), 0);
    }

    #[test]
    fn space_toggles_enabled_on_the_selected_harness_and_stages_the_user_layer() {
        let mut state = state_with_a_session();
        state.open_settings(sample_harness_rows(), Vec::new(), Vec::new(), Vec::new());
        let before = state.settings().unwrap().harnesses()[0].enabled;

        state.handle_key(press(KeyCode::Char(' ')));

        let row = &state.settings().unwrap().harnesses()[0];
        assert_eq!(row.enabled, !before);
        assert_eq!(row.enabled_layer, Layer::User);
        assert_eq!(state.settings_edits().len(), 1);
        assert_eq!(state.settings_edits()[0].id, IntegrationId::ClaudeCode);
        assert_eq!(state.settings_edits()[0].enabled, Some(!before));
    }

    /// "Esc in the editor cancels without changing anything."
    #[test]
    fn esc_in_the_path_editor_cancels_without_changing_anything() {
        let mut state = state_with_a_session();
        state.open_settings(sample_harness_rows(), Vec::new(), Vec::new(), Vec::new());
        let before = state.settings().unwrap().harnesses()[0].clone();

        state.handle_key(press(KeyCode::Enter));
        assert!(state.settings().unwrap().path_input().is_some());
        for c in "/bogus/path".chars() {
            state.handle_key(press(KeyCode::Char(c)));
        }

        assert_eq!(state.handle_key(press(KeyCode::Esc)), Action::Redraw);
        assert!(state.settings().unwrap().path_input().is_none());
        assert_eq!(
            state.settings().unwrap().harnesses()[0],
            before,
            "cancelling the editor must not change the row"
        );
        assert!(state.settings_edits().is_empty());
        assert_eq!(
            state.overlay(),
            Some(Overlay::Settings),
            "Esc in the editor only closes the editor, not all of settings"
        );
    }

    #[test]
    fn a_valid_explicit_path_is_recorded_with_the_user_layer() {
        let tmp = tempfile::tempdir().unwrap();
        let exe = tmp.path().join("my-claude");
        std::fs::write(&exe, "#!/bin/sh\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).unwrap();
        }

        let mut state = state_with_a_session();
        state.open_settings(sample_harness_rows(), Vec::new(), Vec::new(), Vec::new());
        state.handle_key(press(KeyCode::Enter));
        for c in exe.to_str().unwrap().chars() {
            state.handle_key(press(KeyCode::Char(c)));
        }
        state.handle_key(press(KeyCode::Enter));

        assert!(state.settings().unwrap().path_input().is_none());
        let row = &state.settings().unwrap().harnesses()[0];
        assert_eq!(
            std::fs::canonicalize(row.executable.as_ref().unwrap()).unwrap(),
            std::fs::canonicalize(&exe).unwrap()
        );
        assert_eq!(row.executable_layer, Some(Layer::User));
        assert_eq!(state.settings_edits().len(), 1);
    }

    #[test]
    fn an_invalid_explicit_path_surfaces_an_error_and_keeps_the_editor_open() {
        let mut state = state_with_a_session();
        state.open_settings(sample_harness_rows(), Vec::new(), Vec::new(), Vec::new());
        state.handle_key(press(KeyCode::Enter));
        for c in "/definitely/not/a/real/executable".chars() {
            state.handle_key(press(KeyCode::Char(c)));
        }
        assert_eq!(state.handle_key(press(KeyCode::Enter)), Action::Redraw);

        let input = state.settings().unwrap().path_input().expect("still open");
        assert!(input.error.is_some());
        assert!(state.settings_edits().is_empty());
    }

    #[test]
    fn lowercase_w_signals_an_immediate_user_level_save() {
        let mut state = state_with_a_session();
        state.open_settings(sample_harness_rows(), Vec::new(), Vec::new(), Vec::new());
        state.handle_key(press(KeyCode::Char(' ')));
        assert_eq!(
            state.handle_key(press(KeyCode::Char('w'))),
            Action::SaveUserSettings
        );
    }

    /// "It must first show a confirmation ... Only an explicit `y` (or
    /// Enter on the confirm) proceeds; `Esc`/`n` cancels."
    #[test]
    fn shift_w_requires_a_separate_explicit_confirmation() {
        let mut state = state_with_a_session();
        state.open_settings(sample_harness_rows(), Vec::new(), Vec::new(), Vec::new());

        assert_eq!(state.handle_key(press(KeyCode::Char('W'))), Action::Redraw);
        assert!(state.settings().unwrap().confirming_project_write());

        // An unrelated key does not proceed.
        assert_eq!(state.handle_key(press(KeyCode::Char('z'))), Action::None);
        assert!(state.settings().unwrap().confirming_project_write());

        assert_eq!(
            state.handle_key(press(KeyCode::Char('y'))),
            Action::SaveProjectSettings
        );
    }

    #[test]
    fn enter_on_the_confirmation_also_proceeds() {
        let mut state = state_with_a_session();
        state.open_settings(sample_harness_rows(), Vec::new(), Vec::new(), Vec::new());
        state.handle_key(press(KeyCode::Char('W')));
        assert_eq!(
            state.handle_key(press(KeyCode::Enter)),
            Action::SaveProjectSettings
        );
    }

    #[test]
    fn esc_or_n_cancels_the_confirmation_without_leaving_settings() {
        for cancel in [press(KeyCode::Esc), press(KeyCode::Char('n'))] {
            let mut state = state_with_a_session();
            state.open_settings(sample_harness_rows(), Vec::new(), Vec::new(), Vec::new());
            state.handle_key(press(KeyCode::Char('W')));

            assert_eq!(state.handle_key(cancel), Action::Redraw);
            assert!(!state.settings().unwrap().confirming_project_write());
            assert_eq!(state.overlay(), Some(Overlay::Settings));
        }
    }

    #[test]
    fn refresh_settings_clears_pending_edits_and_keeps_the_cursor() {
        let mut state = state_with_a_session();
        state.open_settings(sample_harness_rows(), Vec::new(), Vec::new(), Vec::new());
        state.handle_key(press(KeyCode::Down));
        state.handle_key(press(KeyCode::Char(' ')));
        assert!(!state.settings_edits().is_empty());
        assert_eq!(state.settings().unwrap().selected_harness(), 1);

        state.refresh_settings(sample_harness_rows(), Vec::new(), Vec::new(), Vec::new());
        assert!(state.settings_edits().is_empty());
        assert_eq!(
            state.settings().unwrap().selected_harness(),
            1,
            "the cursor position is preserved across a refresh"
        );
    }

    // -----------------------------------------------------------------
    // Phase 2D: Providers and Launch Profiles.
    // -----------------------------------------------------------------

    fn to_providers(mut state: ShellState) -> ShellState {
        state.handle_key(press(KeyCode::Tab));
        state.handle_key(press(KeyCode::Tab));
        assert_eq!(
            state.settings().unwrap().section(),
            SettingsSection::Providers
        );
        state
    }

    fn to_launch_profiles(mut state: ShellState) -> ShellState {
        state.handle_key(press(KeyCode::Tab));
        state.handle_key(press(KeyCode::Tab));
        state.handle_key(press(KeyCode::Tab));
        assert_eq!(
            state.settings().unwrap().section(),
            SettingsSection::LaunchProfiles
        );
        state
    }

    /// Acceptance 1: an empty Providers section must not panic on
    /// navigation or any of its keys.
    #[test]
    fn an_empty_providers_section_does_not_panic_on_navigation_or_actions() {
        let mut state = state_with_a_session();
        state.open_settings(Vec::new(), Vec::new(), Vec::new(), Vec::new());
        state = to_providers(state);
        assert!(state.settings().unwrap().providers().is_empty());

        for key in [
            KeyCode::Up,
            KeyCode::Down,
            KeyCode::Char(' '),
            KeyCode::Char('d'),
            KeyCode::Char('t'),
        ] {
            state.handle_key(press(key));
        }
        assert!(state.settings().unwrap().providers().is_empty());
    }

    /// Acceptance 2 (the staging half — see `shell::tests` for the
    /// save/reload half): adding a provider from a built-in template stages
    /// it at the user layer.
    #[test]
    fn adding_a_provider_from_a_template_stages_it_at_the_user_layer() {
        let mut state = state_with_a_session();
        state.open_settings(Vec::new(), Vec::new(), Vec::new(), Vec::new());
        state = to_providers(state);

        assert_eq!(state.handle_key(press(KeyCode::Char('a'))), Action::Redraw);
        assert!(state.settings().unwrap().provider_input().is_some());
        type_text(&mut state, "my-router");
        state.handle_key(press(KeyCode::Enter));
        assert!(
            state.settings().unwrap().provider_input().is_some(),
            "the wizard's second step (template) must still be open"
        );
        type_text(&mut state, "openrouter");
        state.handle_key(press(KeyCode::Enter));

        assert!(state.settings().unwrap().provider_input().is_none());
        let providers = state.settings().unwrap().providers();
        assert_eq!(providers.len(), 1);
        assert_eq!(providers[0].name, "my-router");
        assert_eq!(providers[0].config.template(), "openrouter");
        assert_eq!(providers[0].layer, Layer::User);

        let edits = state.settings_provider_edits();
        assert_eq!(edits.len(), 1);
        assert_eq!(edits[0].name, "my-router");
        assert!(edits[0].upsert.is_some());
    }

    /// An unknown template is refused with a message naming it, mirroring
    /// how an unknown harness is refused for a launch profile.
    #[test]
    fn adding_a_provider_with_an_unknown_template_is_refused_with_a_message_naming_it() {
        let mut state = state_with_a_session();
        state.open_settings(Vec::new(), Vec::new(), Vec::new(), Vec::new());
        state = to_providers(state);

        state.handle_key(press(KeyCode::Char('a')));
        type_text(&mut state, "my-router");
        state.handle_key(press(KeyCode::Enter));
        type_text(&mut state, "not-a-real-template");
        assert_eq!(state.handle_key(press(KeyCode::Enter)), Action::Redraw);

        let input = state
            .settings()
            .unwrap()
            .provider_input()
            .expect("still open on error");
        let error = input.error.expect("an error must be shown");
        assert!(
            error.contains("not-a-real-template"),
            "the error must name the template: {error}"
        );
        assert!(state.settings().unwrap().providers().is_empty());
    }

    /// Acceptance 3 (the staging half): editing a provider's base URL
    /// persists in the row and the staged edit.
    #[test]
    fn editing_a_providers_base_url_persists_in_the_row_and_the_staged_edit() {
        let mut state = state_with_a_session();
        state.open_settings(Vec::new(), Vec::new(), sample_provider_rows(), Vec::new());
        state = to_providers(state);

        let backspaces = {
            assert_eq!(state.handle_key(press(KeyCode::Char('e'))), Action::Redraw);
            let input = state
                .settings()
                .unwrap()
                .provider_input()
                .expect("base url editor open");
            assert_eq!(input.buffer, "https://mirror.example.com/v1");
            input.buffer.chars().count()
        };
        for _ in 0..backspaces {
            state.handle_key(press(KeyCode::Backspace));
        }
        type_text(&mut state, "https://new.example.com/v1");
        state.handle_key(press(KeyCode::Enter));

        let row = &state.settings().unwrap().providers()[0];
        assert_eq!(row.config.base_url(), Some("https://new.example.com/v1"));
        assert_eq!(row.layer, Layer::User);

        let edits = state.settings_provider_edits();
        assert_eq!(edits.len(), 1);
        assert_eq!(
            edits[0].upsert.as_ref().unwrap().base_url(),
            Some("https://new.example.com/v1")
        );
    }

    /// Acceptance 4: removing a provider removes it; disabling one keeps its
    /// configuration intact and reversible. Both halves are asserted.
    #[test]
    fn removing_a_provider_removes_it_and_disabling_keeps_it_reversible() {
        let mut state = state_with_a_session();
        state.open_settings(Vec::new(), Vec::new(), sample_provider_rows(), Vec::new());
        state = to_providers(state);

        // Disable half.
        let before = state.settings().unwrap().providers()[0].config.clone();
        assert!(before.enabled());
        state.handle_key(press(KeyCode::Char(' ')));
        let disabled = state.settings().unwrap().providers()[0].config.clone();
        assert!(!disabled.enabled(), "the provider must be disabled");
        assert_eq!(
            disabled.base_url(),
            before.base_url(),
            "disabling must not touch other fields"
        );
        // Reversible without retyping anything.
        state.handle_key(press(KeyCode::Char(' ')));
        let re_enabled = state.settings().unwrap().providers()[0].config.clone();
        assert!(re_enabled.enabled());
        assert_eq!(re_enabled.base_url(), before.base_url());

        // Remove half.
        assert_eq!(state.handle_key(press(KeyCode::Char('d'))), Action::Redraw);
        assert!(
            state.settings().unwrap().providers().is_empty(),
            "removing must actually remove the row"
        );
        let edits = state.settings_provider_edits();
        assert_eq!(edits.len(), 1);
        assert_eq!(edits[0].name, "my-router");
        assert!(
            edits[0].upsert.is_none(),
            "a removal edit carries no config to upsert"
        );
    }

    /// Acceptance 5 (the staging half): duplicating a launch profile
    /// produces an independent copy — editing the copy must not change the
    /// original.
    #[test]
    fn duplicating_a_profile_produces_an_independent_copy() {
        let mut state = state_with_a_session();
        state.open_settings(Vec::new(), Vec::new(), Vec::new(), sample_profile_rows());
        state = to_launch_profiles(state);

        assert_eq!(state.handle_key(press(KeyCode::Char('u'))), Action::Redraw);
        type_text(&mut state, "fast-copy");
        state.handle_key(press(KeyCode::Enter));

        assert_eq!(state.settings().unwrap().profiles().len(), 2);
        let copy_index = state
            .settings()
            .unwrap()
            .profiles()
            .iter()
            .position(|row| row.name == "fast-copy")
            .expect("the copy exists");
        assert_eq!(state.settings().unwrap().selected_profile(), copy_index);

        // Edit the copy's model.
        assert_eq!(state.handle_key(press(KeyCode::Char('e'))), Action::Redraw);
        type_text(&mut state, "claude-opus");
        state.handle_key(press(KeyCode::Enter));

        let original = state
            .settings()
            .unwrap()
            .profiles()
            .iter()
            .find(|row| row.name == "fast")
            .unwrap();
        let copy = state
            .settings()
            .unwrap()
            .profiles()
            .iter()
            .find(|row| row.name == "fast-copy")
            .unwrap();
        assert_eq!(
            original.config.model(),
            None,
            "editing the copy must not change the original"
        );
        assert_eq!(copy.config.model(), Some("claude-opus"));
    }

    /// Acceptance 6: creating a profile that names an unknown harness is
    /// refused with a message naming the harness.
    #[test]
    fn creating_a_profile_naming_an_unknown_harness_is_refused_with_a_message_naming_it() {
        let mut state = state_with_a_session();
        state.open_settings(Vec::new(), Vec::new(), Vec::new(), Vec::new());
        state = to_launch_profiles(state);

        state.handle_key(press(KeyCode::Char('a')));
        type_text(&mut state, "custom");
        state.handle_key(press(KeyCode::Enter));
        type_text(&mut state, "not-a-real-harness");
        assert_eq!(state.handle_key(press(KeyCode::Enter)), Action::Redraw);

        let input = state
            .settings()
            .unwrap()
            .profile_input()
            .expect("still open on error");
        let error = input.error.expect("an error must be shown");
        assert!(
            error.contains("not-a-real-harness"),
            "the error must name the harness: {error}"
        );
        assert!(
            state.settings().unwrap().profiles().is_empty(),
            "no profile must have been created"
        );
    }

    /// Found running the real binary: `cmux`, Ollama and llama.cpp are real
    /// integration slugs, but none is a launchable coding harness — naming
    /// one for a launch profile must be refused exactly like a truly unknown
    /// slug, not silently accepted because it happens to appear in
    /// `IntegrationId::ALL`.
    #[test]
    fn creating_a_profile_naming_a_non_harness_integration_is_also_refused() {
        for slug in ["cmux", "ollama", "llama-cpp"] {
            let mut state = state_with_a_session();
            state.open_settings(Vec::new(), Vec::new(), Vec::new(), Vec::new());
            state = to_launch_profiles(state);

            state.handle_key(press(KeyCode::Char('a')));
            type_text(&mut state, "custom");
            state.handle_key(press(KeyCode::Enter));
            type_text(&mut state, slug);
            state.handle_key(press(KeyCode::Enter));

            let input = state
                .settings()
                .unwrap()
                .profile_input()
                .unwrap_or_else(|| panic!("`{slug}` must be refused, not accepted"));
            assert!(input.error.is_some(), "`{slug}` must be refused");
            assert!(state.settings().unwrap().profiles().is_empty());
        }
    }

    /// Removing a launch profile stages its removal, matching
    /// [`removing_a_provider_removes_it_and_disabling_keeps_it_reversible`].
    #[test]
    fn removing_a_profile_stages_its_removal() {
        let mut state = state_with_a_session();
        state.open_settings(Vec::new(), Vec::new(), Vec::new(), sample_profile_rows());
        state = to_launch_profiles(state);

        assert_eq!(state.handle_key(press(KeyCode::Char('d'))), Action::Redraw);
        assert!(state.settings().unwrap().profiles().is_empty());
        let edits = state.settings_profile_edits();
        assert_eq!(edits.len(), 1);
        assert_eq!(edits[0].name, "fast");
        assert!(edits[0].upsert.is_none());
    }

    /// Disabling a launch profile is reversible without retyping, matching
    /// the provider half of acceptance 4.
    #[test]
    fn disabling_a_profile_keeps_it_reversible() {
        let mut state = state_with_a_session();
        state.open_settings(Vec::new(), Vec::new(), Vec::new(), sample_profile_rows());
        state = to_launch_profiles(state);

        assert!(state.settings().unwrap().profiles()[0].config.enabled());
        state.handle_key(press(KeyCode::Char(' ')));
        assert!(!state.settings().unwrap().profiles()[0].config.enabled());
        state.handle_key(press(KeyCode::Char(' ')));
        assert!(state.settings().unwrap().profiles()[0].config.enabled());
    }

    /// Acceptance 9: Line 5's check reports failure without disabling the
    /// provider. Uses a uniquely named, never-set variable rather than any
    /// built-in template's real one, so the test cannot pass by accident
    /// because of something set in the ambient environment.
    #[test]
    fn a_failed_reachability_check_reports_failure_without_disabling_the_provider() {
        const VAR: &str = "GLASSHOUSE_SETTINGS_TEST_ONLY_MISSING_CRED_VAR";
        // SAFETY: `VAR` is unique to this test and is not set by anything
        // else; removed again below regardless of how the test proceeds.
        unsafe {
            std::env::remove_var(VAR);
        }

        let mut config = ProviderConfig::new("openrouter");
        config.set_credential_env(vec![VAR.to_owned()]);
        let rows = vec![ProviderRow::new("unset-cred", config, Layer::User)];

        let mut state = state_with_a_session();
        state.open_settings(Vec::new(), Vec::new(), rows, Vec::new());
        state = to_providers(state);

        assert_eq!(state.handle_key(press(KeyCode::Char('t'))), Action::Redraw);
        let (name, outcome) = state
            .settings()
            .unwrap()
            .provider_test_result()
            .expect("a test ran");
        assert_eq!(name, "unset-cred");
        match outcome {
            ReachabilityCheck::Failed(reason) => assert!(
                reason.contains(VAR),
                "the failure must name the missing variable: {reason}"
            ),
            other => panic!("expected a failure, got {other:?}"),
        }
        assert!(
            state.settings().unwrap().providers()[0].config.enabled(),
            "a failed test must not disable the provider"
        );
    }

    /// A provider whose preconditions hold now produces a *request*, not a
    /// verdict. `t` hands the run loop something to do, the row says a
    /// request is running, and the intent names the exact URL that will be
    /// asked for.
    ///
    /// The `openrouter` template's model-list endpoint is verified, so the
    /// planned target is that endpoint rather than the bare base URL — one
    /// request that exercises the base URL, TLS, the credential and a real
    /// route.
    #[test]
    fn a_passing_precondition_check_plans_a_real_request_and_says_it_is_in_flight() {
        const VAR: &str = "GLASSHOUSE_SETTINGS_TEST_ONLY_PRESENT_CRED_VAR";
        // SAFETY: `VAR` is unique to this test and is removed again below.
        unsafe {
            std::env::set_var(VAR, "sk-fabricated-test-value-not-a-real-credential");
        }

        let mut config = ProviderConfig::new("openrouter");
        config.set_credential_env(vec![VAR.to_owned()]);
        let rows = vec![ProviderRow::new("set-cred", config, Layer::User)];

        let mut state = state_with_a_session();
        state.open_settings(Vec::new(), Vec::new(), rows, Vec::new());
        state = to_providers(state);
        let action = state.handle_key(press(KeyCode::Char('t')));

        unsafe {
            std::env::remove_var(VAR);
        }

        assert_eq!(
            action,
            Action::RunProviderProbe,
            "a passing precondition check must hand the run loop a request to make"
        );

        let (_, outcome) = state
            .settings()
            .unwrap()
            .provider_test_result()
            .expect("a test ran");
        match outcome {
            ReachabilityCheck::InFlight {
                protocol,
                base_url,
                endpoint,
            } => {
                assert_eq!(*protocol, "openai-chat");
                assert_eq!(*base_url, "https://openrouter.ai/api/v1");
                assert_eq!(*endpoint, "https://openrouter.ai/api/v1/models");
            }
            other => panic!("expected a request in flight, got {other:?}"),
        }

        assert_eq!(
            state.settings().unwrap().providers()[0].activity,
            Some(ProbeKind::Connectivity),
            "the row must say a request is running, or a busy interface looks frozen"
        );

        let intent = state
            .take_provider_probe_intent()
            .expect("the run loop is given exactly one request");
        assert_eq!(intent.provider, "set-cred");
        assert_eq!(intent.kind, ProbeKind::Connectivity);
        assert_eq!(intent.target, ProbeTarget::ModelList);
        assert_eq!(
            intent.secret_refs,
            vec![SecretRef::Environment {
                var: VAR.to_owned()
            }]
        );
        assert!(
            state.take_provider_probe_intent().is_none(),
            "an intent is taken, so one keystroke can only ever open one socket"
        );
    }

    /// **Acceptance test 6.** Phase 9D line 2 says "when the provider
    /// exposes model discovery", so a provider that does not must produce a
    /// plain sentence — not an error, and not a control that silently does
    /// nothing.
    ///
    /// Both negative states are asserted, because they are different facts
    /// and call for different next actions. `ollama`'s model list is
    /// `Unverified`: nobody has established it, and the sentence has to say
    /// so rather than claiming the service lacks one.
    #[test]
    fn a_provider_with_no_established_model_discovery_says_so_and_is_not_an_error() {
        let rows = vec![ProviderRow::new(
            "local",
            ProviderConfig::new("ollama"),
            Layer::User,
        )];
        let mut state = state_with_a_session();
        state.open_settings(Vec::new(), Vec::new(), rows, Vec::new());
        state = to_providers(state);

        let action = state.handle_key(press(KeyCode::Char('m')));
        assert_eq!(
            action,
            Action::Redraw,
            "no request may be planned for a provider with no established model list"
        );
        assert!(
            state.take_provider_probe_intent().is_none(),
            "nothing may be sent to a path nobody established"
        );

        let (name, refresh) = state
            .settings()
            .unwrap()
            .provider_models_result()
            .expect("the user must be told something");
        assert_eq!(name, "local");
        match refresh {
            ModelRefresh::NotOffered(reason) => {
                assert!(
                    reason.contains("has been established"),
                    "the sentence must say nobody established one, not that none exists: \
                     {reason}"
                );
                assert!(
                    reason.contains("local"),
                    "and it must name the provider: {reason}"
                );
            }
            other => panic!("expected a plain explanation, not an error: {other:?}"),
        }
        assert!(
            state.settings().unwrap().providers()[0].activity.is_none(),
            "nothing is in flight, so nothing may claim to be"
        );
    }

    /// A provider whose model list *is* established plans a refresh. The
    /// counterpart to the test above, so "not offered" cannot be passing
    /// because `m` does nothing at all.
    #[test]
    fn a_provider_with_an_established_model_list_plans_a_refresh_of_it() {
        let rows = vec![ProviderRow::new(
            "router",
            ProviderConfig::new("litellm"),
            Layer::User,
        )];
        let mut state = state_with_a_session();
        state.open_settings(Vec::new(), Vec::new(), rows, Vec::new());
        state = to_providers(state);

        assert_eq!(
            state.handle_key(press(KeyCode::Char('m'))),
            Action::RunProviderProbe
        );
        let intent = state.take_provider_probe_intent().expect("a request");
        assert_eq!(intent.kind, ProbeKind::ModelRefresh);
        assert_eq!(intent.target, ProbeTarget::ModelList);
        assert_eq!(probe_endpoint(&intent), "http://0.0.0.0:4000/models");
    }

    /// **Phase 9D line 1's own words: "before enabling it for routing".**
    ///
    /// Testing reports. It does not decide. A provider that was enabled stays
    /// enabled through a timeout, and a provider that was disabled stays
    /// disabled through a success — neither outcome touches the flag, and
    /// this asserts both directions because only checking one would let a
    /// "helpful" auto-disable through.
    #[test]
    fn a_connectivity_result_never_enables_or_disables_the_provider_it_is_about() {
        for (starts_enabled, outcome) in [
            (true, ProbeOutcome::TimedOut { waited_ms: 10_000 }),
            (true, ProbeOutcome::Rejected { status: 401 }),
            (
                true,
                ProbeOutcome::Unreachable {
                    reason: "the connection was refused".to_owned(),
                },
            ),
            (false, ProbeOutcome::Reached { status: 200 }),
        ] {
            let mut config = ProviderConfig::new("openrouter");
            config.set_enabled(starts_enabled);
            let rows = vec![ProviderRow::new("router", config, Layer::User)];
            let mut state = state_with_a_session();
            state.open_settings(Vec::new(), Vec::new(), rows, Vec::new());

            state.apply_provider_probe_result(ProviderProbeResult {
                provider: "router".to_owned(),
                notice: ProviderNotice::Reachability(ReachabilityCheck::Answered {
                    protocol: "openai-chat",
                    base_url: "https://openrouter.ai/api/v1".to_owned(),
                    endpoint: "https://openrouter.ai/api/v1/models".to_owned(),
                    outcome: outcome.clone(),
                }),
                catalogue: None,
            });

            assert_eq!(
                state.settings().unwrap().providers()[0].config.enabled(),
                starts_enabled,
                "a {outcome:?} result changed whether the provider was enabled; testing \
                 reports and the user decides"
            );
        }
    }

    /// **Acceptance test 4, at the state level.** A refresh replaces the
    /// cached list and moves the timestamp.
    #[test]
    fn a_manual_refresh_replaces_the_cached_list_and_moves_the_timestamp() {
        let rows = vec![
            ProviderRow::new("router", ProviderConfig::new("openrouter"), Layer::User).with_models(
                Some(ModelCatalogue::new(
                    "router",
                    "https://openrouter.ai/api/v1",
                    "https://openrouter.ai/api/v1/models",
                    1_000,
                    vec![
                        crate::provider::cache::ModelEntry::new("old/one"),
                        crate::provider::cache::ModelEntry::new("old/two"),
                    ],
                )),
            ),
        ];
        let mut state = state_with_a_session();
        state.open_settings(Vec::new(), Vec::new(), rows, Vec::new());

        let refreshed = ModelCatalogue::new(
            "router",
            "https://openrouter.ai/api/v1",
            "https://openrouter.ai/api/v1/models",
            2_000,
            vec![crate::provider::cache::ModelEntry::new("new/one")],
        );
        state.apply_provider_probe_result(ProviderProbeResult {
            provider: "router".to_owned(),
            notice: ProviderNotice::Models(ModelRefresh::Refreshed {
                count: 1,
                fetched_at: 2_000,
                endpoint: "https://openrouter.ai/api/v1/models".to_owned(),
            }),
            catalogue: Some(refreshed),
        });

        let row = &state.settings().unwrap().providers()[0];
        let models = row.models.as_ref().expect("a catalogue");
        assert_eq!(models.fetched_at(), 2_000, "the timestamp must move");
        assert_eq!(models.len(), 1);
        assert!(
            !models.models().iter().any(|m| m.id().starts_with("old/")),
            "a refresh replaces the list; it must never append to it"
        );
    }

    /// A request on the wire must be visible on the row, and must survive
    /// every keystroke that clears the banner beneath it.
    ///
    /// This is the state half of "a frozen screen and a slow screen look
    /// identical, and only one of them is acceptable". The banner is
    /// deliberately transient — that is what stops it shadowing a field
    /// editor — so if the in-flight marker lived there too, scrolling the
    /// list would make a running request invisible.
    #[test]
    fn an_in_flight_request_survives_the_keystrokes_that_clear_its_banner() {
        const VAR: &str = "GLASSHOUSE_SETTINGS_TEST_ONLY_INFLIGHT_CRED_VAR";
        // SAFETY: `VAR` is unique to this test and is removed again below.
        unsafe {
            std::env::set_var(VAR, "sk-fabricated-test-value-not-a-real-credential");
        }
        let mut config = ProviderConfig::new("openrouter");
        config.set_credential_env(vec![VAR.to_owned()]);
        let rows = vec![
            ProviderRow::new("first", config.clone(), Layer::User),
            ProviderRow::new("second", config, Layer::User),
        ];
        let mut state = state_with_a_session();
        state.open_settings(Vec::new(), Vec::new(), rows, Vec::new());
        state = to_providers(state);

        assert_eq!(
            state.handle_key(press(KeyCode::Char('t'))),
            Action::RunProviderProbe
        );
        unsafe {
            std::env::remove_var(VAR);
        }
        assert!(state.provider_probe_in_flight());

        // The interface still answers keys — this is the responsiveness
        // claim, made against the same state a real keystroke would reach.
        assert_eq!(state.handle_key(press(KeyCode::Down)), Action::Redraw);
        assert_eq!(state.settings().unwrap().selected_provider(), 1);
        assert!(
            state.settings().unwrap().provider_test_result().is_none(),
            "the banner clears on the next key, exactly as it always did"
        );
        assert!(
            state.provider_probe_in_flight(),
            "but the request is still running, and the interface must still say so"
        );
        assert_eq!(
            state.settings().unwrap().providers()[0].activity,
            Some(ProbeKind::Connectivity),
            "and it says so on the row the request is about, not on the selected one"
        );
    }

    /// A second press while one request is running must not open a second
    /// socket, and must say why rather than doing nothing — a key that
    /// silently does nothing is indistinguishable from a frozen screen.
    #[test]
    fn a_second_test_while_one_is_running_is_refused_out_loud() {
        const VAR: &str = "GLASSHOUSE_SETTINGS_TEST_ONLY_DOUBLE_PRESS_VAR";
        // SAFETY: `VAR` is unique to this test and is removed again below.
        unsafe {
            std::env::set_var(VAR, "sk-fabricated-test-value-not-a-real-credential");
        }
        let mut config = ProviderConfig::new("openrouter");
        config.set_credential_env(vec![VAR.to_owned()]);
        let rows = vec![ProviderRow::new("router", config, Layer::User)];
        let mut state = state_with_a_session();
        state.open_settings(Vec::new(), Vec::new(), rows, Vec::new());
        state = to_providers(state);

        assert_eq!(
            state.handle_key(press(KeyCode::Char('t'))),
            Action::RunProviderProbe
        );
        let _first = state.take_provider_probe_intent().expect("one request");

        let action = state.handle_key(press(KeyCode::Char('t')));
        unsafe {
            std::env::remove_var(VAR);
        }
        assert_eq!(action, Action::Redraw, "the second press plans nothing");
        assert!(
            state.take_provider_probe_intent().is_none(),
            "a second socket must not be opened while the first is outstanding"
        );
        match state.settings().unwrap().provider_test_result() {
            Some((_, ReachabilityCheck::Failed(reason))) => assert!(
                reason.contains("already running"),
                "the refusal must say why: {reason}"
            ),
            other => panic!("expected an out-loud refusal, got {other:?}"),
        }
    }

    /// **Acceptance test 7, at the state boundary.** A probe intent is names
    /// only. Asserted with `!contains` rather than `assert_eq!`, because a
    /// failing equality assertion on secret material prints both sides.
    #[test]
    fn a_probe_intent_and_its_debug_carry_names_only_and_never_a_credential() {
        const VAR: &str = "GLASSHOUSE_SETTINGS_TEST_ONLY_LEAK_CHECK_VAR";
        const VALUE: &str = "sk-planted-state-credential-9d";
        // SAFETY: `VAR` is unique to this test and is removed again below.
        unsafe {
            std::env::set_var(VAR, VALUE);
        }
        let mut config = ProviderConfig::new("openrouter");
        config.set_credential_env(vec![VAR.to_owned()]);
        let rows = vec![ProviderRow::new("router", config, Layer::User)];
        let mut state = state_with_a_session();
        state.open_settings(Vec::new(), Vec::new(), rows, Vec::new());
        state = to_providers(state);
        state.handle_key(press(KeyCode::Char('t')));
        let intent = state.take_provider_probe_intent().expect("a request");
        unsafe {
            std::env::remove_var(VAR);
        }

        for rendered in [
            format!("{intent:?}"),
            format!("{:?}", state.settings().unwrap().providers()),
            format!("{:?}", state.settings().unwrap().provider_test_result()),
        ] {
            assert!(
                !rendered.contains(VALUE),
                "a credential value reached a Debug rendering"
            );
        }
        assert!(
            format!("{intent:?}").contains(VAR),
            "the variable NAME is not a secret and is what makes the intent readable"
        );
    }

    /// A probe answering after Settings has been closed is dropped quietly
    /// rather than reopening anything or costing a frame.
    #[test]
    fn a_result_arriving_after_settings_closed_changes_nothing() {
        let mut state = state_with_a_session();
        assert_eq!(
            state.apply_provider_probe_result(ProviderProbeResult {
                provider: "router".to_owned(),
                notice: ProviderNotice::Models(ModelRefresh::Failed("late".to_owned())),
                catalogue: None,
            }),
            Action::None
        );
        assert!(state.settings().is_none());
    }

    /// Found by asking what happens to a request whose provider the user
    /// deletes while it is in flight: the row is gone, so there is nothing
    /// to clear, and the banner still reports what happened to the request
    /// they started.
    #[test]
    fn a_result_for_a_provider_that_has_since_been_deleted_still_reports() {
        let rows = vec![ProviderRow::new(
            "router",
            ProviderConfig::new("openrouter"),
            Layer::User,
        )];
        let mut state = state_with_a_session();
        state.open_settings(Vec::new(), Vec::new(), rows, Vec::new());

        assert_eq!(
            state.apply_provider_probe_result(ProviderProbeResult {
                provider: "already-deleted".to_owned(),
                notice: ProviderNotice::Models(ModelRefresh::Failed("gone".to_owned())),
                catalogue: None,
            }),
            Action::Redraw
        );
        assert!(
            state
                .settings()
                .unwrap()
                .provider_models_result()
                .is_some_and(|(name, _)| name == "already-deleted")
        );
    }

    /// Found running the real binary: a reachability-check result must not
    /// permanently shadow the bottom panel. Any other key clears it, and an
    /// active Launch-Profiles input takes priority over it even if it did
    /// not — see `SettingsState::handle_key`'s clearing line and
    /// `render_settings`'s panel-priority comment in `view.rs`.
    #[test]
    fn a_stale_reachability_result_does_not_shadow_a_later_profile_input() {
        let mut state = state_with_a_session();
        state.open_settings(
            Vec::new(),
            Vec::new(),
            sample_provider_rows(),
            sample_profile_rows(),
        );
        state = to_providers(state);
        state.handle_key(press(KeyCode::Char('t')));
        assert!(state.settings().unwrap().provider_test_result().is_some());

        // Move to Launch Profiles and open the "add" wizard.
        state.handle_key(press(KeyCode::Tab));
        assert_eq!(
            state.settings().unwrap().section(),
            SettingsSection::LaunchProfiles
        );
        assert!(
            state.settings().unwrap().provider_test_result().is_none(),
            "switching sections must clear the stale banner"
        );
        state.handle_key(press(KeyCode::Char('a')));
        assert!(state.settings().unwrap().profile_input().is_some());
        assert!(state.settings().unwrap().provider_test_result().is_none());
    }

    // --- storing and deleting a provider credential (Phase 9E) ----------

    /// A provider row with a credential variable, so `s` has a name to file
    /// a stored credential under.
    fn credential_provider_rows() -> Vec<ProviderRow> {
        let mut config = ProviderConfig::new("openrouter");
        config
            .set_base_url(Some("https://mirror.example.com/v1".to_owned()))
            .set_credential_env(vec!["MY_ROUTER_KEY".to_owned()]);
        vec![ProviderRow::new("my-router", config, Layer::User)]
    }

    fn settings_with_credential_provider() -> ShellState {
        let mut state = ShellState::new("glasshouse", "/work", "0.1.0", Vec::new());
        state.open_settings(
            Vec::new(),
            Vec::new(),
            credential_provider_rows(),
            Vec::new(),
        );
        to_providers(state)
    }

    /// **Acceptance 5 for this module.** A credential typed into Settings is
    /// masked on the way out to the renderer and redacted in every `Debug`
    /// that could reach a log or a panic message. Asserted with
    /// `!contains`, never `assert_eq!` on the secret material — a failing
    /// `assert_eq!` prints both sides.
    #[test]
    fn a_typed_credential_is_masked_for_the_view_and_redacted_in_every_debug() {
        const VALUE: &str = "sk-typed-into-settings-0123456789abcdef";

        let mut state = settings_with_credential_provider();
        state.handle_key(press(KeyCode::Char('s')));
        type_text(&mut state, VALUE);

        let view = state.settings().unwrap().provider_input().unwrap();
        assert!(
            !view.buffer.contains(VALUE),
            "the typed credential reached the view: {}",
            view.buffer
        );
        assert_eq!(
            view.buffer,
            "*".repeat(VALUE.chars().count()),
            "a credential field is masked character for character"
        );
        assert!(
            view.label.contains("my-router"),
            "the field must say which provider it is for: {}",
            view.label
        );

        let rendered = format!("{:?}", state.settings().unwrap());
        assert!(
            !rendered.contains(VALUE),
            "a credential reached a Debug rendering:\n{rendered}"
        );
        assert!(
            rendered.contains(crate::secret::REDACTED),
            "the buffer must render as the same marker `Secret` uses:\n{rendered}"
        );

        // ... and a field that is NOT a credential is still shown, so the
        // masking is the purpose's doing rather than a blanket rule that
        // would make every editor unusable.
        state.handle_key(press(KeyCode::Esc));
        state.handle_key(press(KeyCode::Char('e')));
        type_text(&mut state, "https://example.invalid/v1");
        let view = state.settings().unwrap().provider_input().unwrap();
        assert!(
            view.buffer.contains("example.invalid"),
            "got {}",
            view.buffer
        );
    }

    /// Enter on a credential field does not apply it here — writing to a
    /// keychain is I/O this module does not hold — it escalates, and the
    /// value leaves the overlay exactly once.
    #[test]
    fn enter_on_a_credential_field_hands_the_value_to_the_run_loop_once() {
        const VALUE: &str = "sk-handed-to-the-run-loop-0123456789abcd";

        let mut state = settings_with_credential_provider();
        state.handle_key(press(KeyCode::Char('s')));
        type_text(&mut state, VALUE);
        assert_eq!(
            state.handle_key(press(KeyCode::Enter)),
            Action::StoreProviderCredential
        );

        let taken = state.take_provider_credential_entry();
        assert_eq!(
            taken,
            Some(("my-router".to_owned(), VALUE.to_owned())),
            "the run loop gets the provider and the value"
        );
        // Taken means taken: a second call finds nothing, and the overlay
        // no longer holds the value.
        assert_eq!(state.take_provider_credential_entry(), None);
        assert!(state.settings().unwrap().provider_input().is_none());
        let rendered = format!("{:?}", state.settings().unwrap());
        assert!(!rendered.contains(VALUE), "got {rendered}");
    }

    /// An empty field is refused with a message rather than storing an empty
    /// credential, and Esc still leaves everything unchanged.
    #[test]
    fn an_empty_credential_field_is_refused_and_esc_changes_nothing() {
        let mut state = settings_with_credential_provider();
        state.handle_key(press(KeyCode::Char('s')));
        assert_eq!(state.handle_key(press(KeyCode::Enter)), Action::Redraw);

        let view = state.settings().unwrap().provider_input().unwrap();
        assert!(
            view.error.unwrap().contains("needs a value"),
            "{:?}",
            view.error
        );

        state.handle_key(press(KeyCode::Esc));
        assert!(state.settings().unwrap().provider_input().is_none());
        assert!(state.settings_provider_edits().is_empty());
    }

    /// A provider with no credential variable has no name to file a stored
    /// credential under, and is told so rather than having one invented.
    #[test]
    fn storing_a_credential_needs_a_credential_variable_name_first() {
        let mut state = ShellState::new("glasshouse", "/work", "0.1.0", Vec::new());
        state.open_settings(Vec::new(), Vec::new(), sample_provider_rows(), Vec::new());
        let mut state = to_providers(state);

        state.handle_key(press(KeyCode::Char('s')));
        let view = state.settings().unwrap().provider_input().unwrap();
        assert!(
            view.error.unwrap().contains("names no credential variable"),
            "{:?}",
            view.error
        );
    }

    /// **Acceptance 3, the configuration half.** Recording a stored
    /// credential stages the *reference* — two names — and clearing it
    /// removes exactly that, leaving every other field alone.
    #[test]
    fn recording_and_clearing_a_stored_credential_stages_only_the_reference() {
        let mut state = settings_with_credential_provider();

        let stored = StoredCredentialRef::new("glasshouse", "MY_ROUTER_KEY");
        state.record_provider_credential_stored("my-router", stored.clone());

        let row = &state.settings().unwrap().providers()[0];
        assert_eq!(row.config.credential_store(), Some(&stored));
        assert_eq!(
            row.config.base_url(),
            Some("https://mirror.example.com/v1"),
            "recording a stored credential must not disturb any other field"
        );
        let edits = state.settings_provider_edits();
        assert_eq!(edits.len(), 1);
        assert_eq!(
            edits[0].upsert.as_ref().unwrap().credential_store(),
            Some(&stored)
        );

        state.record_provider_credential_cleared("my-router");
        let row = &state.settings().unwrap().providers()[0];
        assert_eq!(row.config.credential_store(), None);
        assert_eq!(
            row.config.credential_env(),
            &["MY_ROUTER_KEY".to_owned()],
            "clearing the reference is not clearing the provider"
        );
        let edits = state.settings_provider_edits();
        assert_eq!(edits.len(), 1);
        assert_eq!(
            edits[0].upsert.as_ref().unwrap().credential_store(),
            None,
            "the staged edit must carry the cleared reference, or `w` would write it back"
        );
    }

    /// Deleting reaches the OS store, which no other edit in this overlay
    /// does, so it is confirmed first and Esc really does cancel.
    #[test]
    fn deleting_a_stored_credential_is_confirmed_first_and_esc_cancels() {
        let mut state = settings_with_credential_provider();
        state.record_provider_credential_stored(
            "my-router",
            StoredCredentialRef::new("glasshouse", "MY_ROUTER_KEY"),
        );

        state.handle_key(press(KeyCode::Char('x')));
        assert_eq!(
            state.settings().unwrap().confirming_credential_delete(),
            Some("my-router")
        );

        // An unrelated key is swallowed, exactly like the project-write
        // confirmation: never "any key dismisses".
        assert_eq!(state.handle_key(press(KeyCode::Char('z'))), Action::None);
        assert_eq!(
            state.settings().unwrap().confirming_credential_delete(),
            Some("my-router")
        );

        state.handle_key(press(KeyCode::Esc));
        assert_eq!(
            state.settings().unwrap().confirming_credential_delete(),
            None
        );
        assert!(
            state.settings().unwrap().providers()[0]
                .config
                .credential_store()
                .is_some(),
            "cancelling must change nothing"
        );

        // ... and confirming escalates to the run loop, which owns the I/O.
        state.handle_key(press(KeyCode::Char('x')));
        assert_eq!(
            state.handle_key(press(KeyCode::Char('y'))),
            Action::DeleteProviderCredential
        );
    }

    /// Every reference the selected provider's credential could be stored
    /// under, so a deletion cannot leave one of two copies behind.
    #[test]
    fn deletion_targets_both_the_recorded_reference_and_every_declared_variable() {
        let mut config = ProviderConfig::new("openrouter");
        config
            .set_credential_env(vec!["FIRST_KEY".to_owned(), "SECOND_KEY".to_owned()])
            .set_credential_store(Some(StoredCredentialRef::new("glasshouse", "FIRST_KEY")));
        let rows = vec![ProviderRow::new("pool", config, Layer::User)];

        let mut state = ShellState::new("glasshouse", "/work", "0.1.0", Vec::new());
        state.open_settings(Vec::new(), Vec::new(), rows, Vec::new());
        let state = to_providers(state);

        let (name, references) = state.selected_provider_stored_credentials().unwrap();
        assert_eq!(name, "pool");
        assert_eq!(
            references,
            vec![
                os_credential_for_variable("FIRST_KEY"),
                os_credential_for_variable("SECOND_KEY"),
            ],
            "the recorded reference is not duplicated, and no declared variable is skipped"
        );
    }

    /// Phase 2D's Routing tab offers all three model modes and stages each
    /// policy field independently. Invalid pins stay in the editor with an
    /// explanation instead of silently degrading before they are saved.
    #[test]
    fn routing_settings_validate_and_stage_every_policy_control() {
        fn replace_input(state: &mut ShellState, text: &str) {
            for _ in 0..80 {
                state.handle_key(press(KeyCode::Backspace));
            }
            type_text(state, text);
            state.handle_key(press(KeyCode::Enter));
        }

        let routing = RoutingRow::new(
            Layered::new(RoutingModelChoice::Deterministic, Layer::Default),
            Layered::new(RouterLatencyMs::DEFAULT, Layer::Default),
            Layered::new(RouterCostMicroUsd::DEFAULT, Layer::Default),
            Layered::new(true, Layer::Default),
            Layered::new(PremiumReservePercent::DEFAULT, Layer::Default),
            vec!["openrouter".to_owned()],
        );
        let mut state = ShellState::new("glasshouse", "/work", "0.1.0", Vec::new());
        state.open_settings_with_routing(
            Vec::new(),
            Vec::new(),
            sample_provider_rows(),
            Vec::new(),
            routing,
            MemoryRow::defaults(),
        );
        for _ in 0..4 {
            state.handle_key(press(KeyCode::Tab));
        }
        assert_eq!(
            state.settings().unwrap().section(),
            SettingsSection::Routing
        );

        state.handle_key(press(KeyCode::Char('m')));
        replace_input(&mut state, "missing:model");
        let error = state
            .settings()
            .unwrap()
            .routing_input()
            .and_then(|input| input.error)
            .unwrap_or_default();
        assert!(error.contains("not a configured provider"), "{error}");
        state.handle_key(press(KeyCode::Esc));

        state.handle_key(press(KeyCode::Char('m')));
        replace_input(&mut state, "automatic");
        assert_eq!(
            state.settings().unwrap().routing().model,
            RoutingModelChoice::Automatic
        );
        state.handle_key(press(KeyCode::Char('m')));
        replace_input(&mut state, "openrouter:openai/gpt-5-mini");
        assert!(matches!(
            &state.settings().unwrap().routing().model,
            RoutingModelChoice::Pinned { .. }
        ));
        state.handle_key(press(KeyCode::Char('m')));
        replace_input(&mut state, "deterministic");

        state.handle_key(press(KeyCode::Char('l')));
        replace_input(&mut state, "750");
        state.handle_key(press(KeyCode::Char('c')));
        replace_input(&mut state, "0.002500");
        state.handle_key(press(KeyCode::Char('f')));
        state.handle_key(press(KeyCode::Char('p')));
        replace_input(&mut state, "12");

        let edit = state.settings_routing_edit().expect("routing edit staged");
        assert_eq!(edit.model, Some(RoutingModelChoice::Deterministic));
        assert_eq!(edit.max_latency.unwrap().get(), 750);
        assert_eq!(edit.max_cost.unwrap().get(), 2_500);
        assert_eq!(edit.prefer_free, Some(false));
        assert_eq!(edit.premium_reserve.unwrap().get(), 12);
        let row = state.settings().unwrap().routing();
        assert_eq!(row.model_layer, Layer::User);
        assert_eq!(row.max_latency_layer, Layer::User);
        assert_eq!(row.max_cost_layer, Layer::User);
        assert_eq!(row.prefer_free_layer, Layer::User);
        assert_eq!(row.premium_reserve_layer, Layer::User);
    }
}

/// Phase 4's last three lines, at the layer that decides them: which session
/// the overview acts on, that acting on it leaves the presented session
/// alone, and that a session which is not running is refused out loud.
///
/// No processes here. What a byte does once it reaches a pseudo-terminal is
/// `tests/pty_smoke.rs`'s business; what this module has to get right is the
/// *aim* — and the aim is exactly what a test with a real process is worst
/// at proving, because a runtime with one session cannot tell "sent to the
/// session under the cursor" from "sent to whatever had focus".
#[cfg(test)]
mod overview_tests {
    use super::*;
    use crate::session::{SessionId, SessionLifecycle, SessionPresentation, SessionRole};

    fn record(id: &str, lifecycle: SessionLifecycle) -> SessionRecord {
        SessionRecord {
            id: SessionId::new(id),
            project_id: "p".to_owned(),
            harness: "claude-code".to_owned(),
            native_session_id: None,
            role: SessionRole::Normal,
            lifecycle,
            presentation: SessionPresentation::Embedded,
            created_at: 0,
            last_activity_at: 0,
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
        }
    }

    fn press(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn typed(state: &mut ShellState, text: &str) {
        for c in text.chars() {
            state.handle_key(press(KeyCode::Char(c)));
        }
    }

    /// Two live sessions, the overview open, and the cursor moved onto the
    /// one the shell is *not* presenting — the situation both of the first
    /// two capability lines are about.
    fn overview_on_the_other_session() -> ShellState {
        let mut state = ShellState::new(
            "p",
            "/p",
            "0.1.0",
            vec![
                record("presented", SessionLifecycle::Running),
                record("background", SessionLifecycle::Running),
            ],
        );
        state.handle_key(press(KeyCode::Char('o')));
        state.handle_key(press(KeyCode::Down));
        assert_eq!(
            state.overview_target().map(|r| r.id.as_str()),
            Some("background")
        );
        assert_eq!(
            state.active_session().map(|r| r.id.as_str()),
            Some("presented"),
            "moving the overview cursor must not move the session bar"
        );
        state
    }

    /// Line 1, first half: a line typed in the overview is aimed at the
    /// session under the cursor.
    #[test]
    fn a_line_sent_from_the_overview_is_aimed_at_the_session_under_the_cursor() {
        let mut state = overview_on_the_other_session();

        state.handle_key(press(KeyCode::Char('m')));
        typed(&mut state, "status");
        let action = state.handle_key(press(KeyCode::Enter));

        assert_eq!(
            action,
            Action::SendSessionText {
                id: SessionId::new("background"),
                text: "status".to_owned(),
            }
        );
    }

    /// Line 1, second half, and the half that is the whole point: **the
    /// presented session does not change**. A capability that delivered the
    /// text but pulled the user into the session receiving it would satisfy
    /// the word "sending" and none of the intent.
    #[test]
    fn sending_a_line_leaves_the_presented_session_exactly_where_it_was() {
        let mut state = overview_on_the_other_session();
        let before = state.active_session().map(|record| record.id.clone());
        let before_index = state.selected_index();

        state.handle_key(press(KeyCode::Char('m')));
        typed(&mut state, "hello");
        state.handle_key(press(KeyCode::Enter));

        assert_eq!(state.active_session().map(|r| r.id.clone()), before);
        assert_eq!(state.selected_index(), before_index);
        assert_eq!(
            state.mode(),
            Mode::Control,
            "sending text must not hand the keyboard to anything"
        );
    }

    /// Line 2: the interrupt is aimed at the cursor's session, and nothing
    /// else moves — including, deliberately, the session's own recorded
    /// state. Interrupting is not killing.
    #[test]
    fn an_interrupt_from_the_overview_targets_the_cursor_and_moves_nothing_else() {
        let mut state = overview_on_the_other_session();
        let before = state.sessions().to_vec();
        let presented = state.active_session().map(|record| record.id.clone());

        let action = state.handle_key(press(KeyCode::Char('c')));

        assert_eq!(
            action,
            Action::InterruptSession(SessionId::new("background"))
        );
        assert_eq!(state.active_session().map(|r| r.id.clone()), presented);
        assert_eq!(
            state.sessions(),
            before.as_slice(),
            "an interrupt must not move any session's lifecycle"
        );
    }

    /// `Ctrl-C` is still how a user leaves Glasshouse. The overview's own
    /// `c` must not have swallowed it.
    #[test]
    fn control_c_still_quits_while_the_overview_is_open() {
        let mut state = overview_on_the_other_session();
        assert_eq!(
            state.handle_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)),
            Action::Quit
        );
    }

    /// A refusal names the session and the state it is actually in, and
    /// produces no action at all — the acceptance condition for line 5 of the
    /// packet, and the rule this project already applies to the provider
    /// probes: a key that silently does nothing is indistinguishable from a
    /// frozen screen.
    #[test]
    fn acting_on_a_session_that_is_not_running_is_refused_by_name_and_changes_nothing() {
        for lifecycle in [
            SessionLifecycle::Stopped,
            SessionLifecycle::Failed,
            SessionLifecycle::Closed,
        ] {
            for key in [KeyCode::Char('c'), KeyCode::Char('m')] {
                let mut state = ShellState::new(
                    "p",
                    "/p",
                    "0.1.0",
                    vec![
                        record("alive", SessionLifecycle::Running),
                        record("finished", lifecycle),
                    ],
                );
                state.handle_key(press(KeyCode::Char('o')));
                state.handle_key(press(KeyCode::Down));
                let before = state.sessions().to_vec();

                let action = state.handle_key(press(key));

                assert_eq!(action, Action::Redraw, "{lifecycle:?} {key:?}");
                let status = state.status().unwrap_or_default().to_owned();
                assert!(
                    status.contains("finished"),
                    "the refusal must name the session; got {status:?}"
                );
                assert!(
                    status.contains(lifecycle.as_str()),
                    "the refusal must name the state; got {status:?}"
                );
                assert_eq!(
                    state.sessions(),
                    before.as_slice(),
                    "a refusal must change nothing"
                );
                assert!(
                    state.overview().and_then(OverviewState::entry).is_none(),
                    "a refused send must not leave a field open"
                );
            }
        }
    }

    /// The session can end while the line is being typed at it. Checking only
    /// when the field opened would send into a dead session and report
    /// success.
    #[test]
    fn a_session_that_dies_while_the_line_is_typed_is_refused_at_the_moment_of_sending() {
        let mut state = overview_on_the_other_session();
        state.handle_key(press(KeyCode::Char('m')));
        typed(&mut state, "too late");

        state.refresh(vec![
            record("presented", SessionLifecycle::Running),
            record("background", SessionLifecycle::Stopped),
        ]);

        let action = state.handle_key(press(KeyCode::Enter));
        assert_eq!(action, Action::Redraw, "nothing may be sent");
        let status = state.status().unwrap_or_default().to_owned();
        assert!(status.contains("background"), "got {status:?}");
        assert!(status.contains("stopped"), "got {status:?}");
    }

    /// Every key belongs to the field while one is open. Without this,
    /// typing a message containing `q` would quit Glasshouse — the same
    /// class of failure the session-mode split exists to prevent.
    #[test]
    fn the_send_field_owns_every_key_including_the_bindings() {
        let mut state = overview_on_the_other_session();
        state.handle_key(press(KeyCode::Char('m')));

        typed(&mut state, "quit now");
        assert_eq!(
            state.overview().and_then(OverviewState::entry),
            Some("quit now")
        );

        state.handle_key(press(KeyCode::Backspace));
        assert_eq!(
            state.overview().and_then(OverviewState::entry),
            Some("quit no")
        );

        let action = state.handle_key(press(KeyCode::Enter));
        assert_eq!(
            action,
            Action::SendSessionText {
                id: SessionId::new("background"),
                text: "quit no".to_owned(),
            }
        );
    }

    /// Escape cancels the field and returns to the overview rather than
    /// closing the overlay: leaving is one more Escape away, and a field that
    /// took the whole overlay with it would lose the row the user was aiming
    /// at.
    #[test]
    fn escape_cancels_the_field_and_keeps_the_overview_open() {
        let mut state = overview_on_the_other_session();
        state.handle_key(press(KeyCode::Char('m')));
        typed(&mut state, "never mind");

        assert_eq!(state.handle_key(press(KeyCode::Esc)), Action::Redraw);
        assert_eq!(state.overlay(), Some(Overlay::Overview));
        assert!(state.overview().and_then(OverviewState::entry).is_none());
    }

    /// An empty line is refused rather than sent as a bare carriage return —
    /// which a harness would read as a submitted empty prompt.
    #[test]
    fn an_empty_line_is_not_sent() {
        let mut state = overview_on_the_other_session();
        state.handle_key(press(KeyCode::Char('m')));

        assert_eq!(state.handle_key(press(KeyCode::Enter)), Action::Redraw);
        assert!(
            state.status().unwrap_or_default().contains("empty"),
            "got {:?}",
            state.status()
        );
    }

    /// Sessions are ordered by last activity, so any refresh can reorder
    /// them. A cursor held as a bare index would then be pointing at a
    /// different session — and the next `c` would interrupt the wrong
    /// process, silently.
    #[test]
    fn a_reorder_carries_the_overview_cursor_with_its_session() {
        let mut state = overview_on_the_other_session();

        state.refresh(vec![
            record("background", SessionLifecycle::Running),
            record("presented", SessionLifecycle::Running),
        ]);

        assert_eq!(
            state.overview_target().map(|r| r.id.as_str()),
            Some("background"),
            "the cursor must follow its session, not its row"
        );
        assert_eq!(
            state.active_session().map(|r| r.id.as_str()),
            Some("presented"),
            "and so must the bar's own selection"
        );
    }

    /// The cursor is a ring, like the session bar.
    #[test]
    fn the_overview_cursor_wraps_in_both_directions() {
        let mut state = overview_on_the_other_session();
        state.handle_key(press(KeyCode::Down));
        assert_eq!(
            state.overview_target().map(|r| r.id.as_str()),
            Some("presented")
        );
        state.handle_key(press(KeyCode::Up));
        assert_eq!(
            state.overview_target().map(|r| r.id.as_str()),
            Some("background")
        );
    }

    /// Closing and reopening the overview puts the cursor back on the session
    /// the bar is presenting, rather than resuming wherever it was left. A
    /// stale cursor is how an interrupt reaches a session the user has
    /// forgotten they pointed at.
    #[test]
    fn reopening_the_overview_starts_the_cursor_on_the_presented_session() {
        let mut state = overview_on_the_other_session();
        state.handle_key(press(KeyCode::Esc));
        assert_eq!(state.overlay(), None);

        state.handle_key(press(KeyCode::Char('o')));
        assert_eq!(
            state.overview_target().map(|r| r.id.as_str()),
            Some("presented")
        );
    }

    /// Tab still moves the session bar underneath the popup — the Overview is
    /// a passive overlay and stayed one.
    #[test]
    fn ordinary_navigation_still_works_underneath_the_overview() {
        let mut state = overview_on_the_other_session();
        state.handle_key(press(KeyCode::Tab));
        assert_eq!(
            state.active_session().map(|r| r.id.as_str()),
            Some("background")
        );
        assert_eq!(state.overlay(), Some(Overlay::Overview));
    }

    /// Line 3, at this layer: a headless session has no viewport, so there is
    /// nothing to enter. Letting the user in would put their keystrokes into
    /// whichever session held focus instead — the bar showing one session
    /// while another receives the typing.
    #[test]
    fn a_headless_session_cannot_be_entered() {
        let mut headless = record("hidden", SessionLifecycle::Running);
        headless.presentation = SessionPresentation::Headless;
        let mut state = ShellState::new("p", "/p", "0.1.0", vec![headless]);

        for key in [press(KeyCode::Enter), press(KeyCode::Char('i'))] {
            assert_eq!(state.handle_key(key), Action::Redraw);
            assert_eq!(
                state.mode(),
                Mode::Control,
                "a headless session must never take the keyboard"
            );
            let status = state.status().unwrap_or_default().to_owned();
            assert!(status.contains("hidden"), "got {status:?}");
            assert!(status.contains("headless"), "got {status:?}");
        }
    }

    /// `N` is the headless twin of `n`, and `n` still starts an ordinary one.
    #[test]
    fn shift_n_starts_a_headless_session_and_n_still_starts_an_embedded_one() {
        let mut state = ShellState::new("p", "/p", "0.1.0", vec![]);
        assert_eq!(
            state.handle_key(press(KeyCode::Char('n'))),
            Action::StartSession
        );
        assert_eq!(
            state.handle_key(KeyEvent::new(KeyCode::Char('N'), KeyModifiers::SHIFT)),
            Action::StartHeadlessSession
        );
    }

    /// An empty project answers both overview keys with a note rather than
    /// panicking on an index into nothing.
    #[test]
    fn an_empty_project_refuses_both_overview_actions() {
        let mut state = ShellState::new("p", "/p", "0.1.0", vec![]);
        state.handle_key(press(KeyCode::Char('o')));
        assert!(state.overview_target().is_none());

        for key in [
            press(KeyCode::Char('c')),
            press(KeyCode::Char('m')),
            press(KeyCode::Down),
        ] {
            assert_eq!(state.handle_key(key), Action::Redraw);
            assert!(state.status().is_some(), "{key:?} said nothing");
        }
    }

    /// Line 687: Enter brings the cursor's session into the viewport and
    /// hands it the keyboard — not the session the bar was already
    /// presenting, which is the entire reason `focus_overview_target` reads
    /// the cursor rather than reusing `active_session`.
    #[test]
    fn enter_focuses_the_cursors_session_not_the_presented_one() {
        let mut state = overview_on_the_other_session();

        let action = state.handle_key(press(KeyCode::Enter));

        assert_eq!(action, Action::Redraw);
        assert_eq!(
            state.active_session().map(|r| r.id.as_str()),
            Some("background"),
            "focus must move the viewport onto the cursor's session"
        );
        assert_eq!(
            state.mode(),
            Mode::Session,
            "focus must hand the keyboard to the focused session"
        );
        assert_eq!(
            state.overlay(),
            None,
            "focusing a session must close the overview it was opened from"
        );
    }

    /// A headless session is live, so `actionable_overview_target` alone
    /// would let it through — but it has no viewport to focus into, and the
    /// box names both adjectives. Refused by name, and nothing moves.
    #[test]
    fn enter_refuses_a_live_headless_session_by_name() {
        let mut state = ShellState::new(
            "p",
            "/p",
            "0.1.0",
            vec![SessionRecord {
                presentation: SessionPresentation::Headless,
                ..record("bg-headless", SessionLifecycle::Running)
            }],
        );
        state.handle_key(press(KeyCode::Char('o')));
        let before_mode = state.mode();
        let before_selected = state.selected_index();

        let action = state.handle_key(press(KeyCode::Enter));

        assert_eq!(action, Action::Redraw);
        let status = state.status().unwrap_or_default().to_owned();
        assert!(
            status.contains("bg-headless") && status.contains("headless"),
            "the refusal must name the session and why; got {status:?}"
        );
        assert_eq!(
            state.mode(),
            before_mode,
            "a refusal must not enter Session mode"
        );
        assert_eq!(state.selected_index(), before_selected);
        assert_eq!(
            state.overlay(),
            Some(Overlay::Overview),
            "a refusal must leave the overview open"
        );
    }

    /// Enter still refuses a stopped session, through the same liveness gate
    /// `c` and `m` already use — resume is `r`'s job, not Enter's.
    #[test]
    fn enter_refuses_a_stopped_session() {
        let mut state = ShellState::new(
            "p",
            "/p",
            "0.1.0",
            vec![
                record("alive", SessionLifecycle::Running),
                record("finished", SessionLifecycle::Stopped),
            ],
        );
        state.handle_key(press(KeyCode::Char('o')));
        state.handle_key(press(KeyCode::Down));

        let action = state.handle_key(press(KeyCode::Enter));

        assert_eq!(action, Action::Redraw);
        assert_eq!(state.mode(), Mode::Control);
        let status = state.status().unwrap_or_default().to_owned();
        assert!(status.contains("finished"), "got {status:?}");
    }

    /// Line 688: `r` resumes the session under the cursor — a *stopped* one,
    /// which is exactly what `actionable_overview_target`'s liveness gate
    /// would refuse, and the reason `resumable_overview_target` is its own
    /// gate rather than a reuse of that helper.
    #[test]
    fn r_resumes_a_stopped_session_with_a_native_identifier() {
        let mut state = ShellState::new(
            "p",
            "/p",
            "0.1.0",
            vec![SessionRecord {
                native_session_id: Some("native-42".to_owned()),
                ..record("finished", SessionLifecycle::Stopped)
            }],
        );
        state.handle_key(press(KeyCode::Char('o')));

        let action = state.handle_key(press(KeyCode::Char('r')));

        assert_eq!(action, Action::ResumeSession(SessionId::new("finished")));
    }

    /// A stopped session with no native identifier is `closed`, not
    /// `resumable` — `SessionRecord::disposition`'s own rule, so `r` must
    /// refuse it exactly as the STATE column already reports it.
    #[test]
    fn r_refuses_a_stopped_session_with_no_native_identifier() {
        let mut state = ShellState::new(
            "p",
            "/p",
            "0.1.0",
            vec![record("finished", SessionLifecycle::Stopped)],
        );
        state.handle_key(press(KeyCode::Char('o')));

        let action = state.handle_key(press(KeyCode::Char('r')));

        assert_eq!(action, Action::Redraw);
        let status = state.status().unwrap_or_default().to_owned();
        assert!(
            status.contains("finished") && status.contains("closed"),
            "got {status:?}"
        );
    }

    /// `r` must refuse a *live* session — the opposite mistake from every
    /// other overview action, and the one `actionable_overview_target` would
    /// make if resume reused it: that helper accepts exactly the sessions
    /// resume must refuse.
    #[test]
    fn r_refuses_a_live_session() {
        let mut state = overview_on_the_other_session();

        let action = state.handle_key(press(KeyCode::Char('r')));

        assert_eq!(action, Action::Redraw);
        let status = state.status().unwrap_or_default().to_owned();
        assert!(
            status.contains("background") && status.contains("running"),
            "got {status:?}"
        );
    }
}

/// Map line 1105: the project-knowledge overlay's cursor and detail popup,
/// exercised through [`ShellState::handle_key`] — the same production key
/// path a real terminal drives — rather than by poking `ProjectKnowledgeState`
/// fields directly.
#[cfg(test)]
mod project_knowledge_state_tests {
    use super::*;

    fn press(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn entry(text: &str) -> KnowledgeSection {
        KnowledgeSection {
            lines: vec![text.to_owned()],
            details: vec![MemoryDetail {
                rationale: Some(format!("why: {text}")),
                source_session: Some("sess_fixture".to_owned()),
                source_commit: Some("abc1234".to_owned()),
                lifecycle: "active".to_owned(),
            }],
            omitted: 0,
        }
    }

    fn opened_with_three_entries() -> ShellState {
        let mut state = ShellState::new("p", "/p", "0.1.0", Vec::new());
        state.handle_key(press(KeyCode::Char('k')));
        state.open_project_knowledge(
            entry("decision: first"),
            entry("constraint: second"),
            KnowledgeSection::default(),
            KnowledgeSection::default(),
            entry("todo: third"),
            None,
        );
        state
    }

    /// The cursor walks every section's entries in render order — decisions,
    /// constraints, features, failed attempts, todos — and wraps at both
    /// ends, the same ring [`ShellState::move_overview_cursor`] is.
    #[test]
    fn moving_the_knowledge_cursor_wraps_across_every_section() {
        let mut state = opened_with_three_entries();
        assert_eq!(state.project_knowledge().unwrap().cursor(), 0);

        state.handle_key(press(KeyCode::Down));
        assert_eq!(state.project_knowledge().unwrap().cursor(), 1);
        state.handle_key(press(KeyCode::Down));
        assert_eq!(state.project_knowledge().unwrap().cursor(), 2);
        state.handle_key(press(KeyCode::Down));
        assert_eq!(
            state.project_knowledge().unwrap().cursor(),
            0,
            "must wrap forward past the last entry"
        );

        state.handle_key(press(KeyCode::Up));
        assert_eq!(
            state.project_knowledge().unwrap().cursor(),
            2,
            "must wrap backward past the first entry"
        );
    }

    /// Enter opens the detail popup for whichever entry the cursor is on —
    /// not always the first one — and Esc returns to the list without
    /// closing the whole overlay.
    #[test]
    fn enter_opens_detail_for_the_entry_under_the_cursor_and_esc_returns_to_the_list() {
        let mut state = opened_with_three_entries();
        state.handle_key(press(KeyCode::Down));
        assert_eq!(state.project_knowledge().unwrap().cursor(), 1);

        state.handle_key(press(KeyCode::Enter));
        assert!(state.project_knowledge().unwrap().detail_open());
        let (text, detail) = state.project_knowledge().unwrap().selected().unwrap();
        assert_eq!(text, "constraint: second");
        assert_eq!(detail.rationale.as_deref(), Some("why: constraint: second"));

        state.handle_key(press(KeyCode::Esc));
        assert!(!state.project_knowledge().unwrap().detail_open());
        assert_eq!(
            state.overlay(),
            Some(Overlay::ProjectKnowledge),
            "Esc must close only the detail popup, not the whole overlay"
        );
    }

    /// A project-knowledge view with nothing recorded has nothing to
    /// select: Enter must not open an empty detail popup, and must say why
    /// rather than doing nothing silently.
    #[test]
    fn enter_refuses_when_nothing_is_selectable() {
        let mut state = ShellState::new("p", "/p", "0.1.0", Vec::new());
        state.handle_key(press(KeyCode::Char('k')));
        state.open_project_knowledge(
            KnowledgeSection::default(),
            KnowledgeSection::default(),
            KnowledgeSection::default(),
            KnowledgeSection::default(),
            KnowledgeSection::default(),
            None,
        );

        state.handle_key(press(KeyCode::Enter));

        assert!(!state.project_knowledge().unwrap().detail_open());
        assert!(state.status().unwrap_or_default().contains("nothing"));
    }
}

#[cfg(test)]
mod activity_tests {
    use super::*;
    use crate::events::{EventBus, GatewayFailure, ProcessExit};

    fn events_of(events: &[LifecycleEvent]) -> Vec<RecordedEvent> {
        let bus = EventBus::new();
        let session = SessionId::new("s-1");
        events
            .iter()
            .map(|event| bus.publish(&session, event.clone()))
            .collect()
    }

    /// One representative of every [`LifecycleEvent`] variant this crate
    /// defines.
    ///
    /// Held to that claim by `the_variant_list_covers_every_kind` rather than
    /// by a comment: a list that merely *said* it was complete would go on
    /// saying so after somebody added a variant, and the distinctness check
    /// below would then be proving less than it looks like it proves.
    fn one_of_each_variant() -> Vec<LifecycleEvent> {
        vec![
            LifecycleEvent::SessionStarted,
            LifecycleEvent::SessionResumed,
            LifecycleEvent::TurnStarted,
            LifecycleEvent::TurnEnded {
                outcome: TurnOutcome::Completed,
            },
            LifecycleEvent::WaitingForUser,
            LifecycleEvent::TextDelivered {
                origin: MessageOrigin::Machine,
                bytes: 41,
            },
            LifecycleEvent::InterruptDelivered {
                origin: MessageOrigin::Machine,
            },
            LifecycleEvent::ProcessExited {
                exit: ProcessExit::from_parts(0, None),
            },
            LifecycleEvent::OutputEnded,
            LifecycleEvent::GatewayUnhealthy {
                resource: "db".to_owned(),
                reason: GatewayFailure::Unreachable,
            },
            LifecycleEvent::GatewayBackendChanged {
                provider: "anthropic".to_owned(),
                model: "claude".to_owned(),
                cause: "failover".to_owned(),
            },
            LifecycleEvent::FileTouched {
                path: "crates/glasshouse/src/a.rs".to_owned(),
            },
        ]
    }

    #[test]
    fn an_empty_slice_notes_nothing() {
        let mut state = ShellState::new("p", "/p", "0.1.0", vec![]);
        assert_eq!(state.note_events(&[]), Action::None);
        assert!(state.activity().is_empty());
    }

    #[test]
    fn note_events_keeps_the_newest_and_is_bounded_at_activity_rows() {
        let mut state = ShellState::new("p", "/p", "0.1.0", vec![]);
        let variants: Vec<LifecycleEvent> = (0..ACTIVITY_ROWS + 3)
            .map(|n| LifecycleEvent::TextDelivered {
                origin: MessageOrigin::Machine,
                bytes: n,
            })
            .collect();
        let recorded = events_of(&variants);

        assert_eq!(state.note_events(&recorded), Action::Redraw);

        assert_eq!(state.activity().len(), ACTIVITY_ROWS);
        // Newest first: the last event published (largest `bytes`) leads.
        assert_eq!(
            state.activity()[0].event(),
            &LifecycleEvent::TextDelivered {
                origin: MessageOrigin::Machine,
                bytes: ACTIVITY_ROWS + 2,
            }
        );
        // Bounded: the oldest events (smallest `bytes`) were discarded.
        for kept in state.activity() {
            let LifecycleEvent::TextDelivered { bytes, .. } = kept.event() else {
                panic!("only TextDelivered events were published");
            };
            assert!(*bytes >= 3, "an old event survived: {bytes}");
        }
    }

    /// The list above really is every variant.
    ///
    /// Anchored on `LifecycleEvent::kind`, which is exhaustive at the
    /// compiler's insistence and is already pinned to the project database's
    /// own `CHECK` constraint. So adding a variant fails here until the list
    /// grows, which is what keeps the distinctness test below honest.
    #[test]
    fn the_variant_list_covers_every_kind() {
        let covered: std::collections::BTreeSet<&str> = one_of_each_variant()
            .iter()
            .map(LifecycleEvent::kind)
            .collect();
        let known: std::collections::BTreeSet<&str> =
            crate::database::LIFECYCLE_EVENT_KINDS.into_iter().collect();
        assert_eq!(
            covered, known,
            "the activity view's variant list has drifted from the event enum"
        );
    }

    /// Catches a `_` arm creeping back into `describe_event`: a collapsed
    /// variant would make two of these summaries equal.
    #[test]
    fn every_variant_renders_a_distinct_non_empty_summary() {
        let variants = one_of_each_variant();
        let mut summaries: Vec<String> = variants.iter().map(describe_event).collect();
        for summary in &summaries {
            assert!(!summary.is_empty());
        }
        let before = summaries.len();
        summaries.sort();
        summaries.dedup();
        assert_eq!(
            summaries.len(),
            before,
            "two variants rendered the same summary"
        );
    }

    #[test]
    fn a_completed_turn_reads_differently_from_a_failed_one() {
        let completed = describe_event(&LifecycleEvent::TurnEnded {
            outcome: TurnOutcome::Completed,
        });
        let failed = describe_event(&LifecycleEvent::TurnEnded {
            outcome: TurnOutcome::Failed,
        });
        assert_ne!(completed, failed);
    }

    #[test]
    fn machine_text_reads_differently_from_a_user_keystroke() {
        let machine = describe_event(&LifecycleEvent::TextDelivered {
            origin: MessageOrigin::Machine,
            bytes: 10,
        });
        let typed = describe_event(&LifecycleEvent::TextDelivered {
            origin: MessageOrigin::UserKeystroke,
            bytes: 10,
        });
        assert_ne!(machine, typed);
    }
}
