use super::*;
use ratatui::style::{Color, Modifier};

/// Colours, bold/inverse, and cursor position must all survive the walk
/// from a `vt100::Screen` into a [`ViewportGrid`] — the design decision's
/// invariant that "colours, cursor position and line wrapping survive a
/// round trip through the emulator into Ratatui cells."
#[test]
fn colours_bold_inverse_and_cursor_position_survive_the_conversion() {
    let mut parser = vt100::Parser::new(3, 10, 0);
    // Bold, indexed red-on-blue "hi", then inverse-video "x".
    parser.process(b"\x1b[1;31;44mhi\x1b[0m\x1b[7mx\x1b[0m");
    parser.process(b"\x1b[2;3H"); // move to row 2, col 3 (1-based)

    let grid = build_viewport_grid(parser.screen());

    let (text, style) = grid.cell(0, 0).expect("cell (0,0) exists");
    assert_eq!(text, "h");
    assert_eq!(
        style.fg,
        Some(Color::Indexed(1)),
        "fg 31 is ANSI red, index 1"
    );
    assert_eq!(
        style.bg,
        Some(Color::Indexed(4)),
        "bg 44 is ANSI blue, index 4"
    );
    assert!(style.add_modifier.contains(Modifier::BOLD));

    let (_, inverse_style) = grid.cell(0, 2).expect("cell (0,2) exists");
    assert!(inverse_style.add_modifier.contains(Modifier::REVERSED));

    let (_, default_style) = grid.cell(1, 0).expect("cell (1,0) exists");
    assert_eq!(
        default_style.fg, None,
        "an untouched cell's colour must inherit, not be forced to a literal colour"
    );

    assert_eq!(
        grid.cursor(),
        Some((1, 2)),
        "vt100 reports zero-based; row 2 col 3 one-based is (1, 2)"
    );
}

/// A hidden cursor (`ESC[?25l`) must not be drawn back in.
#[test]
fn a_hidden_cursor_is_not_shown() {
    let mut parser = vt100::Parser::new(2, 5, 0);
    parser.process(b"\x1b[?25l");
    let grid = build_viewport_grid(parser.screen());
    assert_eq!(grid.cursor(), None);
}

/// Text that overruns a row must wrap onto the next one exactly as
/// `vt100` lays it out — the grid is a direct walk of the screen, so this
/// is really a proof that the walk visits cells in the right order.
#[test]
fn line_wrapping_is_preserved_in_the_grid() {
    let mut parser = vt100::Parser::new(2, 5, 0);
    parser.process(b"abcdefghij"); // 10 characters into a 5-wide screen
    assert!(
        parser.screen().row_wrapped(0),
        "the first row must have wrapped for this test to mean anything"
    );

    let grid = build_viewport_grid(parser.screen());
    let row = |r: u16| -> String {
        (0..5u16)
            .map(|c| grid.cell(r, c).expect("cell exists").0.clone())
            .collect()
    };
    assert_eq!(row(0), "abcde");
    assert_eq!(row(1), "fghij");
}

/// A screen with nothing drawn on it yet is still a valid, non-empty
/// grid — every cell is blank, not absent — which is what lets the view
/// tell "no live session" apart from "a live session with a blank
/// screen".
#[test]
fn a_fresh_screen_converts_to_a_full_grid_of_blank_cells() {
    let parser = vt100::Parser::new(4, 10, 0);
    let grid = build_viewport_grid(parser.screen());
    assert!(!grid.is_empty());
    assert_eq!(grid.rows(), 4);
    assert_eq!(grid.cols(), 10);
    assert_eq!(grid.cell(0, 0).unwrap().0, "");
    assert_eq!(grid.cursor(), Some((0, 0)));
}

/// Phase 2D: Settings' save/reload behaviour for Providers and Launch
/// Profiles, exercised through a real [`Runtime`] and real files — the
/// staging half (which row/edit changes when a key is pressed) is
/// `shell::state`'s own tests; this is the write half.
#[cfg(test)]
mod settings_persistence_tests {
    use super::*;
    use crate::config::{Layer as ConfigLayer, ProfileConfig, ProviderConfig};

    /// Bootstrap a `Runtime` over fresh, isolated data/config/workspace
    /// directories, matching `integrations::tests::bootstrapped_runtime`.
    fn bootstrapped_runtime() -> (tempfile::TempDir, tempfile::TempDir, crate::Runtime) {
        use clap::Parser;

        let data = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(workspace.path().join(".git")).unwrap();

        let cli = crate::Cli::try_parse_from([
            "glasshouse",
            "--data-dir",
            data.path().to_str().unwrap(),
            "--config-dir",
            data.path().to_str().unwrap(),
        ])
        .unwrap();
        let runtime = crate::bootstrap(&cli, workspace.path()).unwrap();
        (data, workspace, runtime)
    }

    /// Acceptance 2: adding a provider from a built-in template persists it
    /// to the user layer, and it survives a reload.
    #[test]
    fn adding_a_provider_from_a_template_persists_to_the_user_layer_and_survives_reload() {
        let (_data, _workspace, runtime) = bootstrapped_runtime();

        let edit = ProviderSettingsEdit {
            name: "my-router".to_owned(),
            upsert: Some(ProviderConfig::new("openrouter")),
        };
        save_user_settings(&runtime, &[], &[edit], &[]).expect("save must succeed");

        // Reload from disk — a fresh read, not the in-memory value just
        // written — to prove this is a persistence test and not a tautology.
        let (harnesses, integrations, providers, profiles, _, _) =
            build_settings(&runtime).expect("settings must rebuild after the save");
        let _ = (harnesses, integrations, profiles);
        assert_eq!(providers.len(), 1);
        assert_eq!(providers[0].name, "my-router");
        assert_eq!(providers[0].config.template(), "openrouter");
        assert_eq!(providers[0].layer, ConfigLayer::User);

        // And directly against `UserConfig`, independent of `build_settings`.
        let reloaded = UserConfig::load(runtime.paths()).unwrap();
        assert_eq!(
            reloaded.providers().get("my-router").unwrap().template(),
            "openrouter"
        );
    }

    /// Acceptance 3: editing a provider's base URL persists, and the edited
    /// value is what a launch would actually use — proven by resolving the
    /// saved configuration into a real `Provider` and reading its protocol's
    /// base URL, the exact value `crate::launch` would send a harness to.
    #[test]
    fn an_edited_base_url_persists_and_is_what_a_launch_would_use() {
        let (_data, _workspace, runtime) = bootstrapped_runtime();

        let mut config = ProviderConfig::new("openrouter");
        config.set_base_url(Some("https://mirror.example.com/v1".to_owned()));
        let edit = ProviderSettingsEdit {
            name: "my-router".to_owned(),
            upsert: Some(config),
        };
        save_user_settings(&runtime, &[], &[edit], &[]).expect("save must succeed");

        let reloaded = UserConfig::load(runtime.paths()).unwrap();
        let effective = config::EffectiveConfig::new(&reloaded, None);
        let resolved = effective
            .configured_provider("my-router")
            .expect("the provider must resolve");
        let openai_chat = resolved
            .value
            .protocols
            .iter()
            .find(|p| p.protocol == crate::harness::WireProtocol::OpenAiChat)
            .expect("openrouter serves openai-chat");
        assert_eq!(
            openai_chat.base_url, "https://mirror.example.com/v1",
            "the edited base URL must be exactly what a launch would use"
        );
    }

    /// Acceptance 4 (the full write path): disabling a provider through
    /// `save_user_settings` persists the disabled state and every other
    /// field, and re-enabling needs no retyping.
    #[test]
    fn disabling_a_provider_through_the_save_path_persists_and_is_reversible() {
        let (_data, _workspace, runtime) = bootstrapped_runtime();

        let mut config = ProviderConfig::new("openrouter");
        config.set_base_url(Some("https://mirror.example.com/v1".to_owned()));
        config.set_enabled(false);
        let edit = ProviderSettingsEdit {
            name: "my-router".to_owned(),
            upsert: Some(config),
        };
        save_user_settings(&runtime, &[], &[edit], &[]).unwrap();

        let reloaded = UserConfig::load(runtime.paths()).unwrap();
        let provider = reloaded.providers().get("my-router").unwrap();
        assert!(!provider.enabled());
        assert_eq!(
            provider.base_url(),
            Some("https://mirror.example.com/v1"),
            "disabling must not touch other fields"
        );

        let mut re_enabled = provider.clone();
        re_enabled.set_enabled(true);
        let edit = ProviderSettingsEdit {
            name: "my-router".to_owned(),
            upsert: Some(re_enabled),
        };
        save_user_settings(&runtime, &[], &[edit], &[]).unwrap();
        let reloaded_again = UserConfig::load(runtime.paths()).unwrap();
        let provider_again = reloaded_again.providers().get("my-router").unwrap();
        assert!(provider_again.enabled());
        assert_eq!(
            provider_again.base_url(),
            Some("https://mirror.example.com/v1")
        );
    }

    /// Removing a provider through the save path actually removes the
    /// entry — the other half of acceptance 4.
    #[test]
    fn removing_a_provider_through_the_save_path_removes_it() {
        let (_data, _workspace, runtime) = bootstrapped_runtime();

        let edit = ProviderSettingsEdit {
            name: "my-router".to_owned(),
            upsert: Some(ProviderConfig::new("openrouter")),
        };
        save_user_settings(&runtime, &[], &[edit], &[]).unwrap();
        assert!(
            UserConfig::load(runtime.paths())
                .unwrap()
                .providers()
                .get("my-router")
                .is_some()
        );

        let removal = ProviderSettingsEdit {
            name: "my-router".to_owned(),
            upsert: None,
        };
        save_user_settings(&runtime, &[], &[removal], &[]).unwrap();
        assert!(
            UserConfig::load(runtime.paths())
                .unwrap()
                .providers()
                .get("my-router")
                .is_none()
        );
    }

    /// Acceptance 5 (the full write path): a duplicated launch profile is an
    /// independent entry once saved — editing the copy's stored
    /// configuration must never touch the original's file record.
    #[test]
    fn a_duplicated_profile_persists_as_an_independent_entry() {
        let (_data, _workspace, runtime) = bootstrapped_runtime();

        let original = ProfileConfig::new(crate::integrations::IntegrationId::ClaudeCode);
        save_user_settings(
            &runtime,
            &[],
            &[],
            &[ProfileSettingsEdit {
                name: "fast".to_owned(),
                upsert: Some(original),
            }],
        )
        .unwrap();

        let mut copy = ProfileConfig::new(crate::integrations::IntegrationId::ClaudeCode);
        copy.set_model(Some("claude-opus".to_owned()));
        save_user_settings(
            &runtime,
            &[],
            &[],
            &[ProfileSettingsEdit {
                name: "fast-copy".to_owned(),
                upsert: Some(copy),
            }],
        )
        .unwrap();

        let reloaded = UserConfig::load(runtime.paths()).unwrap();
        assert_eq!(reloaded.profiles().get("fast").unwrap().model(), None);
        assert_eq!(
            reloaded.profiles().get("fast-copy").unwrap().model(),
            Some("claude-opus")
        );
    }

    /// Acceptance 8: `save_user_settings` never touches the project root,
    /// and only `save_project_settings` — reached only after the Settings
    /// overlay's own explicit `W` confirmation (see `state`'s
    /// `shift_w_requires_a_separate_explicit_confirmation`) — writes
    /// `.glasshouse/config.toml`. This is the write half of that guarantee;
    /// the confirmation-gating half is `state`'s.
    #[test]
    fn saving_user_settings_never_creates_a_project_config_file() {
        let (_data, workspace, runtime) = bootstrapped_runtime();
        let project_config_path = workspace.path().join(".glasshouse").join("config.toml");

        let edit = ProviderSettingsEdit {
            name: "my-router".to_owned(),
            upsert: Some(ProviderConfig::new("openrouter")),
        };
        save_user_settings(&runtime, &[], &[edit], &[]).unwrap();

        assert!(
            !project_config_path.exists(),
            "a user-layer save must never create the project config file"
        );
    }

    /// The other side of acceptance 8: `save_project_settings` does write
    /// exactly `<project root>/.glasshouse/config.toml`, and the provider
    /// edit lands in the project layer, not the user layer.
    #[test]
    fn saving_project_settings_writes_the_project_layer_only() {
        let (_data, workspace, runtime) = bootstrapped_runtime();

        let edit = ProviderSettingsEdit {
            name: "my-router".to_owned(),
            upsert: Some(ProviderConfig::new("openrouter")),
        };
        let path = save_project_settings(&runtime, &[], &[edit], &[]).unwrap();

        assert!(path.exists());
        // Canonicalize before comparing: on macOS `TempDir` paths run through
        // `/tmp`, a symlink to `/private/tmp`, and the runtime's own scope
        // resolution follows it — a portability quirk of the test fixture,
        // not of `save_project_settings` itself.
        assert_eq!(
            std::fs::canonicalize(&path).unwrap(),
            std::fs::canonicalize(workspace.path().join(".glasshouse").join("config.toml"))
                .unwrap()
        );

        let project_config = config::load_project_config(runtime.project())
            .unwrap()
            .expect("the project config file must now exist");
        assert_eq!(
            project_config
                .providers()
                .get("my-router")
                .unwrap()
                .template(),
            "openrouter"
        );
        assert!(
            UserConfig::load(runtime.paths())
                .unwrap()
                .providers()
                .get("my-router")
                .is_none(),
            "a project-layer save must not also write the user layer"
        );
    }

    /// `build_settings` must read a disabled provider or profile back
    /// without panicking or dropping the disabled state — the same rows the
    /// Settings overlay renders.
    // --- Phase 9D: the network never touches the drawing thread ----------
    use crate::provider::cache::{ModelCatalogue, ModelEntry};
    use crate::provider::discovery::{ProbeTarget, ProbeTimeouts};
    use crate::provider::fixture::FixtureProvider;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    /// Short enough that a hanging endpoint is bounded inside a test run,
    /// and far longer than a loopback round trip.
    fn quick_timeouts() -> ProbeTimeouts {
        ProbeTimeouts {
            connect: std::time::Duration::from_millis(500),
            response: std::time::Duration::from_millis(400),
            total: std::time::Duration::from_millis(900),
        }
    }

    fn press(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    /// A shell with Settings open on one provider pointed at `base_url`,
    /// with a credential in the environment so the preconditions pass.
    fn settings_open_on(base_url: &str, var: &str) -> ShellState {
        let mut config = ProviderConfig::new("openrouter");
        config.set_base_url(Some(base_url.to_owned()));
        config.set_credential_env(vec![var.to_owned()]);
        let rows = vec![ProviderRow::new("router", config, ConfigLayer::User)];

        let mut state = ShellState::new("glasshouse", "/work", crate::VERSION, Vec::new());
        state.open_settings(Vec::new(), Vec::new(), rows, Vec::new());
        // Tab to the Providers section: Harnesses, Integrations, Providers.
        state.handle_key(press(KeyCode::Tab));
        state.handle_key(press(KeyCode::Tab));
        state
    }

    /// **Acceptance test 3, through the production spawn path, and the
    /// single most important test in this batch.**
    ///
    /// The fixture accepts the connection and then never writes a byte and
    /// never closes — a wedged provider, not a refused one. A refused
    /// connection is the easy case and proves almost nothing: it comes back
    /// in microseconds whether or not anyone remembered a timeout.
    ///
    /// Two things are asserted, and both matter:
    ///
    /// 1. **The interface stayed alive.** While the request is outstanding
    ///    the main thread — the one that in production reads keys and draws
    ///    frames — keeps handling keystrokes and rendering, and it does so
    ///    many times. Under the bug this batch exists to prevent, the very
    ///    first of those would have blocked until the timeout expired.
    /// 2. **The request came back bounded**, reported as a timeout rather
    ///    than as an unreachable host, because "your network is slow" and
    ///    "your URL is wrong" are different problems.
    #[test]
    fn a_provider_that_accepts_and_never_answers_never_blocks_the_drawing_thread() {
        const VAR: &str = "GLASSHOUSE_SHELL_TEST_ONLY_HANGING_PROBE_VAR";
        // SAFETY: `VAR` is unique to this test and removed again below.
        unsafe {
            std::env::set_var(VAR, "sk-fabricated-test-value-not-a-real-credential");
        }
        let (_data, _workspace, runtime) = bootstrapped_runtime();
        let fixture = FixtureProvider::hanging();
        let mut state = settings_open_on(&fixture.base_url(), VAR);

        assert_eq!(
            state.handle_key(press(KeyCode::Char('t'))),
            Action::RunProviderProbe
        );

        let (results, inbox) = std::sync::mpsc::channel();
        let (wake, wake_inbox) = std::sync::mpsc::channel();
        let started = std::time::Instant::now();
        spawn_provider_probe(&runtime, &mut state, &results, &wake, quick_timeouts());
        unsafe {
            std::env::remove_var(VAR);
        }

        // The loop the run loop would be running. Every iteration is work
        // the drawing thread does *while the request is outstanding*; under
        // the bug, iteration one would not have returned.
        let mut frames = 0usize;
        let mut answer = None;
        while started.elapsed() < std::time::Duration::from_secs(5) {
            assert!(
                state.provider_probe_in_flight() || answer.is_some(),
                "a request that has not come back must still be reported as in flight"
            );
            // A real keystroke, answered while the socket is open.
            state.handle_key(press(if frames.is_multiple_of(2) {
                KeyCode::Down
            } else {
                KeyCode::Up
            }));
            let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(100, 30))
                .expect("a test terminal");
            terminal
                .draw(|frame| view::render(&state, frame))
                .expect("a frame is drawn while the request is outstanding");
            frames += 1;

            if let Ok(result) = inbox.try_recv() {
                answer = Some(result);
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        let elapsed = started.elapsed();

        let answer = answer.expect("the probe must come back, bounded by its own timeout");
        assert!(
            elapsed < std::time::Duration::from_secs(5),
            "the probe must be bounded by its timeout, not by the peer; took {elapsed:?}"
        );
        assert!(
            frames > 5,
            "the interface must have kept drawing while the request was outstanding; \
             it managed {frames} frames in {elapsed:?}"
        );
        assert_eq!(
            fixture.connections(),
            1,
            "the probe must really have connected — a refused connection would prove \
             nothing about a stall"
        );

        match &answer.notice {
            ProviderNotice::Reachability(ReachabilityCheck::Answered { outcome, .. }) => assert!(
                matches!(
                    outcome,
                    crate::provider::discovery::ProbeOutcome::TimedOut { .. }
                ),
                "a stall must be reported as a timeout, not as an unreachable host: {outcome:?}"
            ),
            other => panic!("expected a connectivity answer, got {other:?}"),
        }

        // And the answer reaches the state, clearing the in-flight marker.
        assert_eq!(state.apply_provider_probe_result(answer), Action::Redraw);
        assert!(!state.provider_probe_in_flight());

        // The worker nudged the event loop, so the answer is drawn when it
        // lands rather than at the next tick.
        assert!(
            wake_inbox.try_recv().is_ok(),
            "a finished probe must wake the interface"
        );
    }

    /// **Acceptance test 5.** Starting with a cached catalogue issues no
    /// network request at all.
    ///
    /// Asserted on the fixture seeing **zero connections**, not on elapsed
    /// time. A timing assertion would pass on a fast machine no matter what
    /// the code did; a connection counter cannot.
    #[test]
    fn opening_settings_with_a_cached_catalogue_opens_no_connection_at_all() {
        let (_data, _workspace, runtime) = bootstrapped_runtime();
        let fixture = FixtureProvider::answering(
            "HTTP/1.1 200 OK",
            "",
            r#"{"data":[{"id":"should/never/be/fetched"}]}"#,
        );

        let mut config = ProviderConfig::new("openrouter");
        config.set_base_url(Some(fixture.base_url()));
        save_user_settings(
            &runtime,
            &[],
            &[ProviderSettingsEdit {
                name: "router".to_owned(),
                upsert: Some(config),
            }],
            &[],
        )
        .expect("the provider is configured");

        // A catalogue already on disk, as a previous run's refresh would
        // have left it.
        let cache = ModelCache::new(runtime.paths());
        cache
            .store(&ModelCatalogue::new(
                "router",
                fixture.base_url(),
                format!("{}/models", fixture.base_url()),
                1_787_336_476,
                vec![ModelEntry::new("cached/one"), ModelEntry::new("cached/two")],
            ))
            .expect("the cache is written");

        let (harnesses, integrations, providers, profiles, _, _) =
            build_settings(&runtime).expect("settings open");

        assert_eq!(
            fixture.connections(),
            0,
            "opening Settings made a network request; Phase 9D line 3 exists to stop \
             Glasshouse querying a remote catalogue on every start"
        );

        let row = providers
            .iter()
            .find(|row| row.name == "router")
            .expect("the row");
        let models = row.models.as_ref().expect("the cached catalogue is loaded");
        assert_eq!(models.len(), 2);
        assert_eq!(models.fetched_at(), 1_787_336_476);
        assert_eq!(models.models()[0].id(), "cached/one");
        assert!(
            !models.models().iter().any(|m| m.id().contains("never")),
            "the list must be the cached one, not one the fixture served"
        );

        // Rendering it opens nothing either — a renderer that fetched
        // lazily would be the same bug wearing a different hat.
        let mut state = ShellState::new("glasshouse", "/work", crate::VERSION, Vec::new());
        state.open_settings(harnesses, integrations, providers, profiles);
        state.handle_key(press(KeyCode::Tab));
        state.handle_key(press(KeyCode::Tab));
        let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(200, 40))
            .expect("a test terminal");
        terminal
            .draw(|frame| view::render(&state, frame))
            .expect("a frame");
        assert_eq!(
            fixture.connections(),
            0,
            "drawing a cached model list must not fetch one"
        );
    }

    /// A provider with no cache is simply a provider with no models. It does
    /// **not** become a fetch — the counterpart to the test above, so "zero
    /// connections" cannot be passing because nothing was configured.
    #[test]
    fn a_provider_with_no_cached_catalogue_fetches_nothing_on_open_either() {
        let (_data, _workspace, runtime) = bootstrapped_runtime();
        let fixture = FixtureProvider::answering("HTTP/1.1 200 OK", "", r#"{"data":[]}"#);

        let mut config = ProviderConfig::new("openrouter");
        config.set_base_url(Some(fixture.base_url()));
        save_user_settings(
            &runtime,
            &[],
            &[ProviderSettingsEdit {
                name: "router".to_owned(),
                upsert: Some(config),
            }],
            &[],
        )
        .expect("configured");

        let (_, _, providers, _, _, _) = build_settings(&runtime).expect("settings open");
        assert_eq!(fixture.connections(), 0);
        assert!(
            providers[0].models.is_none(),
            "no cache means no models, never an implicit fetch"
        );
    }

    /// **Acceptance test 4, end to end.** A manual refresh fetches, replaces
    /// the cache on disk, moves the timestamp, and survives a reopen — which
    /// is what "cached" has to mean.
    #[test]
    fn a_manual_refresh_writes_the_catalogue_to_disk_and_a_reopen_finds_it() {
        const VAR: &str = "GLASSHOUSE_SHELL_TEST_ONLY_REFRESH_VAR";
        // SAFETY: `VAR` is unique to this test and removed again below.
        unsafe {
            std::env::set_var(VAR, "sk-fabricated-test-value-not-a-real-credential");
        }
        let (_data, _workspace, runtime) = bootstrapped_runtime();
        let fixture = FixtureProvider::answering(
            "HTTP/1.1 200 OK",
            "",
            r#"{"data":[{"id":"vendor/a"},{"id":"vendor/b"},{"id":"vendor/c"}]}"#,
        );

        let mut config = ProviderConfig::new("openrouter");
        config.set_base_url(Some(fixture.base_url()));
        config.set_credential_env(vec![VAR.to_owned()]);
        save_user_settings(
            &runtime,
            &[],
            &[ProviderSettingsEdit {
                name: "router".to_owned(),
                upsert: Some(config),
            }],
            &[],
        )
        .expect("configured");

        // A stale catalogue, so this proves a replacement rather than a
        // first write.
        let cache = ModelCache::new(runtime.paths());
        cache
            .store(&ModelCatalogue::new(
                "router",
                fixture.base_url(),
                format!("{}/models", fixture.base_url()),
                1_000,
                vec![ModelEntry::new("stale/one")],
            ))
            .expect("stale cache written");

        let (harnesses, integrations, providers, profiles, _, _) =
            build_settings(&runtime).expect("settings open");
        let mut state = ShellState::new("glasshouse", "/work", crate::VERSION, Vec::new());
        state.open_settings(harnesses, integrations, providers, profiles);
        state.handle_key(press(KeyCode::Tab));
        state.handle_key(press(KeyCode::Tab));

        assert_eq!(
            state.handle_key(press(KeyCode::Char('m'))),
            Action::RunProviderProbe
        );
        let (results, inbox) = std::sync::mpsc::channel();
        let (wake, _wake_inbox) = std::sync::mpsc::channel();
        spawn_provider_probe(&runtime, &mut state, &results, &wake, quick_timeouts());
        unsafe {
            std::env::remove_var(VAR);
        }

        let result = inbox
            .recv_timeout(std::time::Duration::from_secs(5))
            .expect("the refresh must come back");
        assert_eq!(fixture.requests().len(), 1, "exactly one request, no other");
        assert_eq!(
            fixture.requests()[0].target,
            "/models",
            "the model list, at the path the provider's own base URL names"
        );

        let fetched_at = match &result.notice {
            ProviderNotice::Models(ModelRefresh::Refreshed {
                count, fetched_at, ..
            }) => {
                assert_eq!(*count, 3);
                *fetched_at
            }
            other => panic!("expected a refreshed catalogue, got {other:?}"),
        };
        assert!(
            fetched_at > 1_000,
            "the timestamp must move forward on a refresh, or a stale list looks fresh"
        );
        state.apply_provider_probe_result(result);

        // On disk, and found by a completely fresh read — the thing that
        // makes the next start silent.
        let (_, _, reopened, _, _, _) = build_settings(&runtime).expect("settings reopen");
        let models = reopened[0].models.as_ref().expect("a cached catalogue");
        assert_eq!(models.len(), 3);
        assert_eq!(models.fetched_at(), fetched_at);
        assert!(
            !models.models().iter().any(|m| m.id() == "stale/one"),
            "a refresh replaces the cached list; it must never append to it"
        );
        assert_eq!(
            fixture.requests().len(),
            1,
            "and reopening Settings must not have fetched again"
        );
    }

    /// **Acceptance test 2, end to end.** A provider answering `401` is
    /// reported as reachable-but-rejected.
    #[test]
    fn a_provider_answering_401_is_reported_as_reachable_but_rejected() {
        const VAR: &str = "GLASSHOUSE_SHELL_TEST_ONLY_REJECTED_VAR";
        // SAFETY: `VAR` is unique to this test and removed again below.
        unsafe {
            std::env::set_var(VAR, "sk-fabricated-test-value-not-a-real-credential");
        }
        let (_data, _workspace, runtime) = bootstrapped_runtime();
        let fixture = FixtureProvider::answering(
            "HTTP/1.1 401 Unauthorized",
            "",
            r#"{"error":{"message":"Authentication parameter not received in Header"}}"#,
        );
        let mut state = settings_open_on(&fixture.base_url(), VAR);
        state.handle_key(press(KeyCode::Char('t')));

        let (results, inbox) = std::sync::mpsc::channel();
        let (wake, _wake_inbox) = std::sync::mpsc::channel();
        spawn_provider_probe(&runtime, &mut state, &results, &wake, quick_timeouts());
        unsafe {
            std::env::remove_var(VAR);
        }

        let result = inbox
            .recv_timeout(std::time::Duration::from_secs(5))
            .expect("the probe comes back");
        match &result.notice {
            ProviderNotice::Reachability(ReachabilityCheck::Answered { outcome, .. }) => {
                assert_eq!(
                    outcome,
                    &crate::provider::discovery::ProbeOutcome::Rejected { status: 401 }
                );
            }
            other => panic!("expected a rejection, got {other:?}"),
        }
        state.apply_provider_probe_result(result);

        let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(200, 40))
            .expect("a test terminal");
        terminal
            .draw(|frame| view::render(&state, frame))
            .expect("a frame");
        let text: String = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(ratatui::buffer::Cell::symbol)
            .collect();
        assert!(
            text.contains("reachable, but it did not accept the credential"),
            "the user must be told which of the two problems they have: {text}"
        );
    }

    /// The run loop probes with the production timeouts and nothing else.
    ///
    /// A source scan, in the same idiom as `secret`'s own. The parameter that
    /// makes the tests above fast is also a parameter someone could quietly
    /// widen at the one call site that matters, and that call site is not
    /// otherwise reachable from a test without a real terminal.
    #[test]
    fn the_run_loop_probes_with_the_default_timeouts() {
        assert!(
            run_loop_passes_the_default_timeouts(include_str!("../mod.rs")),
            "the run loop must pass the default timeouts, not values of its own"
        );
    }

    /// Whether the run loop's own call to `spawn_provider_probe` passes
    /// [`discovery::ProbeTimeouts::default`].
    ///
    /// # Scanned by lines, deliberately
    ///
    /// The first version of this searched for the literal
    /// `"spawn_provider_probe(\n"`. That is a **multi-line literal**, and on a
    /// checkout where Git converts line endings the source
    /// [`include_str!`] hands back contains `\r\n`, so the search finds
    /// nothing and the scan fails by *panicking* rather than by asserting.
    /// Windows CI went red on exactly that, for a test that has nothing to do
    /// with platforms — the second time this repository has paid for the same
    /// mistake, which is why the practice file has a section about it.
    ///
    /// [`str::lines`] strips the carriage return, so this is CRLF-agnostic by
    /// construction rather than by remembering. See
    /// `the_scan_finds_the_call_whatever_the_line_endings_are`, which proves
    /// it against a CRLF copy of this very file — an LF checkout never
    /// exercises the broken path, so without that control the fix would be
    /// untested precisely where it was needed.
    fn run_loop_passes_the_default_timeouts(source: &str) -> bool {
        let production = source.split("#[cfg(test)]").next().unwrap_or(source);
        let lines: Vec<&str> = production.lines().collect();
        let call = lines.iter().position(|line| {
            let trimmed = line.trim();
            // The call site, not the `fn spawn_provider_probe(` definition
            // line and not a single-line test call.
            trimmed == "spawn_provider_probe("
        });
        let Some(call) = call else { return false };
        lines
            .iter()
            .skip(call)
            .take(12)
            .any(|line| line.contains("discovery::ProbeTimeouts::default()"))
    }

    /// The control that keeps the scan above honest.
    ///
    /// Both sides are built from a **normalised** base rather than from
    /// whatever `include_str!` happened to produce, because an assertion whose
    /// input varies with the checkout is a flake generator that will find the
    /// environment you did not test on.
    #[test]
    fn the_scan_finds_the_call_whatever_the_line_endings_are() {
        let normalised = include_str!("../mod.rs").replace("\r\n", "\n");
        let crlf = normalised.replace('\n', "\r\n");
        assert!(
            run_loop_passes_the_default_timeouts(&normalised),
            "the scan must find the call in an LF checkout"
        );
        assert!(
            run_loop_passes_the_default_timeouts(&crlf),
            "the scan must find the call in a CRLF checkout — this is the assertion \
             Windows CI failed on"
        );
        // And it must be capable of saying no, or the two above prove nothing.
        assert!(
            !run_loop_passes_the_default_timeouts("fn main() {}\n"),
            "a source with no such call must not report that the call is correct"
        );
    }

    /// A credential reaches the provider's `authorization` header and no
    /// other surface the run loop touches — including the cache file it
    /// writes, which is a new place on disk for one to end up.
    ///
    /// `!contains`, never `assert_eq!`, on the raw bytes.
    #[test]
    fn a_planted_credential_reaches_the_header_and_not_the_cache_file() {
        const VAR: &str = "GLASSHOUSE_SHELL_TEST_ONLY_LEAK_VAR";
        const VALUE: &str = "sk-planted-run-loop-credential-9d";
        // SAFETY: `VAR` is unique to this test and removed again below.
        unsafe {
            std::env::set_var(VAR, VALUE);
        }
        let (_data, _workspace, runtime) = bootstrapped_runtime();
        let fixture =
            FixtureProvider::answering("HTTP/1.1 200 OK", "", r#"{"data":[{"id":"vendor/a"}]}"#);

        let mut config = ProviderConfig::new("openrouter");
        config.set_base_url(Some(fixture.base_url()));
        config.set_credential_env(vec![VAR.to_owned()]);
        let rows = vec![ProviderRow::new("router", config, ConfigLayer::User)];
        let mut state = ShellState::new("glasshouse", "/work", crate::VERSION, Vec::new());
        state.open_settings(Vec::new(), Vec::new(), rows, Vec::new());
        state.handle_key(press(KeyCode::Tab));
        state.handle_key(press(KeyCode::Tab));
        state.handle_key(press(KeyCode::Char('m')));

        let (results, inbox) = std::sync::mpsc::channel();
        let (wake, _wake_inbox) = std::sync::mpsc::channel();
        spawn_provider_probe(&runtime, &mut state, &results, &wake, quick_timeouts());
        let result = inbox
            .recv_timeout(std::time::Duration::from_secs(5))
            .expect("the refresh comes back");
        // Removed only once the worker has finished with it: the credential
        // is resolved on that thread, at the moment of use, so unsetting the
        // variable any earlier is a race with the code under test rather
        // than a tidy-up.
        unsafe {
            std::env::remove_var(VAR);
        }

        // It really was sent — otherwise every `!contains` below would pass
        // for the wrong reason.
        let sent = fixture.requests();
        assert_eq!(
            sent[0].header("authorization"),
            Some(format!("Bearer {VALUE}").as_str())
        );

        assert!(!format!("{result:?}").contains(VALUE), "a probe result");
        state.apply_provider_probe_result(result);
        assert!(
            !format!("{:?}", state.settings().unwrap().providers()).contains(VALUE),
            "a provider row"
        );

        // The cache file on disk, byte for byte.
        let path = ModelCache::new(runtime.paths()).path_for("router");
        let bytes = std::fs::read(&path).expect("the refresh wrote a cache file");
        assert!(
            !bytes.is_empty(),
            "and it is not empty, so this checks something"
        );
        assert!(
            !String::from_utf8_lossy(&bytes).contains(VALUE),
            "a credential reached the cache file at {}",
            path.display()
        );

        // And the whole rendered screen.
        let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(400, 60))
            .expect("a test terminal");
        terminal
            .draw(|frame| view::render(&state, frame))
            .expect("a frame");
        let text: String = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(ratatui::buffer::Cell::symbol)
            .collect();
        assert!(!text.contains(VALUE), "a credential was rendered on screen");
    }

    /// A probe whose target is the bare base URL appends no path.
    ///
    /// The `ollama` template's model list is `Unverified`, so a connectivity
    /// test of it asks the base URL itself rather than guessing `/models` —
    /// and the fixture is what proves no path was invented.
    #[test]
    fn a_provider_with_no_established_model_list_is_probed_at_its_base_url() {
        let (_data, _workspace, runtime) = bootstrapped_runtime();
        let fixture = FixtureProvider::answering("HTTP/1.1 200 OK", "", "ok");

        let mut config = ProviderConfig::new("ollama");
        config.set_base_url(Some(format!("{}/v1", fixture.base_url())));
        let rows = vec![ProviderRow::new("local", config, ConfigLayer::User)];
        let mut state = ShellState::new("glasshouse", "/work", crate::VERSION, Vec::new());
        state.open_settings(Vec::new(), Vec::new(), rows, Vec::new());
        state.handle_key(press(KeyCode::Tab));
        state.handle_key(press(KeyCode::Tab));
        state.handle_key(press(KeyCode::Char('t')));

        let (results, inbox) = std::sync::mpsc::channel();
        let (wake, _wake_inbox) = std::sync::mpsc::channel();
        spawn_provider_probe(&runtime, &mut state, &results, &wake, quick_timeouts());
        inbox
            .recv_timeout(std::time::Duration::from_secs(5))
            .expect("the probe comes back");

        assert_eq!(
            fixture.requests()[0].target,
            "/v1",
            "a provider with no established model list must be asked for its base URL, \
             never a path nobody read from its documentation"
        );
    }

    /// `ProbeTarget` is chosen from the provider's own declaration, and the
    /// two templates that bracket the choice are asserted by name.
    #[test]
    fn the_probe_target_follows_whether_a_model_list_was_established() {
        const VAR: &str = "GLASSHOUSE_SHELL_TEST_ONLY_TARGET_MATRIX_VAR";
        // SAFETY: `VAR` is unique to this test and removed again below. It
        // exists because the preconditions are checked before a target is
        // chosen, so a template whose credential variable is unset would
        // never reach the line under test.
        unsafe {
            std::env::set_var(VAR, "sk-fabricated-test-value-not-a-real-credential");
        }
        for (template, expected) in [
            ("openrouter", ProbeTarget::ModelList),
            ("litellm", ProbeTarget::ModelList),
            ("ollama", ProbeTarget::BaseUrl),
            ("nvidia", ProbeTarget::BaseUrl),
        ] {
            let mut config = ProviderConfig::new(template);
            config.set_base_url(Some("http://127.0.0.1:1/v1".to_owned()));
            config.set_credential_env(vec![VAR.to_owned()]);
            let rows = vec![ProviderRow::new("p", config, ConfigLayer::User)];
            let mut state = ShellState::new("glasshouse", "/work", crate::VERSION, Vec::new());
            state.open_settings(Vec::new(), Vec::new(), rows, Vec::new());
            state.handle_key(press(KeyCode::Tab));
            state.handle_key(press(KeyCode::Tab));
            state.handle_key(press(KeyCode::Char('t')));
            let intent = state
                .take_provider_probe_intent()
                .unwrap_or_else(|| panic!("{template} must plan a probe"));
            assert_eq!(intent.target, expected, "for the {template} template");
        }
        unsafe {
            std::env::remove_var(VAR);
        }
    }

    #[test]
    fn build_settings_reflects_a_disabled_provider_and_profile() {
        let (_data, _workspace, runtime) = bootstrapped_runtime();

        let mut provider = ProviderConfig::new("openrouter");
        provider.set_enabled(false);
        let mut profile = ProfileConfig::new(crate::integrations::IntegrationId::ClaudeCode);
        profile.set_enabled(false);

        save_user_settings(
            &runtime,
            &[],
            &[ProviderSettingsEdit {
                name: "my-router".to_owned(),
                upsert: Some(provider),
            }],
            &[ProfileSettingsEdit {
                name: "fast".to_owned(),
                upsert: Some(profile),
            }],
        )
        .unwrap();

        let (_, _, providers, profiles, _, _) = build_settings(&runtime).unwrap();
        assert!(!providers[0].config.enabled());
        assert!(!profiles[0].config.enabled());
    }
}

/// Phase 41: the project overview reads real binding memory and real
/// unresolved todos through [`build_project_overview_memory`] — the
/// production function `Action::OpenProjectOverview`'s handler calls, not a
/// helper that re-implements the query. `MemoryStore::binding` and
/// `memory::snapshot::snapshot` had no other production caller before this
/// (only `tests/memory_authority.rs` and `tests/memory_snapshot.rs`
/// exercised them), so the overview is what makes them reachable at all.
#[cfg(test)]
mod project_overview_tests {
    use super::*;
    use crate::memory::{MemoryAuthority, MemoryKind, NewMemory, ProjectMemory};

    /// Bootstrap a `Runtime` over fresh, isolated data/config/workspace
    /// directories, matching `settings_persistence_tests::bootstrapped_runtime`.
    fn bootstrapped_runtime() -> (tempfile::TempDir, tempfile::TempDir, crate::Runtime) {
        use clap::Parser;

        let data = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(workspace.path().join(".git")).unwrap();

        let cli = crate::Cli::try_parse_from([
            "glasshouse",
            "--data-dir",
            data.path().to_str().unwrap(),
            "--config-dir",
            data.path().to_str().unwrap(),
        ])
        .unwrap();
        let runtime = crate::bootstrap(&cli, workspace.path()).unwrap();
        (data, workspace, runtime)
    }

    /// A project with no memory at all gets empty, honest sections — not an
    /// error. `ProjectMemory::open` creates the database on first use, so
    /// "no memory yet" and "could not read memory" must not collapse into
    /// the same outcome.
    #[test]
    fn a_project_with_no_memory_yet_reports_empty_sections_not_an_error() {
        let (_data, _workspace, runtime) = bootstrapped_runtime();

        let memory = build_project_overview_memory(&runtime).expect("must not fail");
        assert!(memory.decisions.is_empty());
        assert!(memory.todos.is_empty());
        assert_eq!(memory.todos_omitted, 0);
    }

    /// A recorded, active constraint and decision both come back from the
    /// real `MemoryStore::binding` call, and a memory with no authority
    /// classification (`None`, never presented as a rule) does not.
    #[test]
    fn active_decisions_and_constraints_are_read_through_the_real_binding_query() {
        let (_data, _workspace, runtime) = bootstrapped_runtime();
        let memory = ProjectMemory::open(&runtime).expect("open");
        let store = memory.store();

        store
            .record(
                NewMemory::new(MemoryKind::Constraint, "the local gate must run alone")
                    .with_authority(Some(MemoryAuthority::Constraint)),
            )
            .unwrap();
        store
            .record(
                NewMemory::new(MemoryKind::Decision, "sonnet closes phase 41")
                    .with_authority(Some(MemoryAuthority::Decision)),
            )
            .unwrap();
        // Never classified, so `binding()` must not return it — see
        // `MemoryStore::binding`'s own doc comment.
        store
            .record(NewMemory::new(
                MemoryKind::Finding,
                "an unclassified finding",
            ))
            .unwrap();

        let overview = build_project_overview_memory(&runtime).expect("must not fail");
        assert_eq!(overview.decisions.len(), 2);
        assert!(
            overview
                .decisions
                .iter()
                .any(|line| line.contains("the local gate must run alone"))
        );
        assert!(
            overview
                .decisions
                .iter()
                .any(|line| line.contains("sonnet closes phase 41"))
        );
        assert!(
            overview
                .decisions
                .iter()
                .all(|line| !line.contains("an unclassified finding"))
        );
    }

    /// A resolved todo is queryable but must never be presented as open work
    /// — `MemoryStatus::is_open_work`'s own contract, proven here through the
    /// same `snapshot` call the overview uses.
    #[test]
    fn only_unresolved_todos_are_shown() {
        let (_data, _workspace, runtime) = bootstrapped_runtime();
        let memory = ProjectMemory::open(&runtime).expect("open");
        let store = memory.store();

        store
            .record(NewMemory::new(MemoryKind::Todo, "wire the shell into main"))
            .unwrap();
        let resolved = store
            .record(NewMemory::new(MemoryKind::Todo, "already done"))
            .unwrap();
        store
            .set_status(&resolved.id, crate::memory::MemoryStatus::Resolved)
            .unwrap();

        let overview = build_project_overview_memory(&runtime).expect("must not fail");
        assert_eq!(overview.todos.len(), 1);
        assert!(overview.todos[0].contains("wire the shell into main"));
        assert!(
            overview
                .todos
                .iter()
                .all(|line| !line.contains("already done"))
        );
    }

    /// The `p` key opens the overlay through the real run-loop action, and
    /// the overlay carries the memory the run loop read — not a
    /// hand-constructed fixture.
    #[test]
    fn opening_the_project_overview_shows_real_memory() {
        let (_data, _workspace, runtime) = bootstrapped_runtime();
        let memory = ProjectMemory::open(&runtime).expect("open");
        memory
            .store()
            .record(
                NewMemory::new(MemoryKind::Constraint, "never run ci-local beside cargo")
                    .with_authority(Some(MemoryAuthority::Constraint)),
            )
            .unwrap();

        let mut state = state::ShellState::new(
            "glasshouse",
            runtime.project().display_root(),
            "test",
            Vec::new(),
        );
        assert_eq!(
            state.handle_key(crossterm::event::KeyEvent::from(
                crossterm::event::KeyCode::Char('p')
            )),
            state::Action::OpenProjectOverview
        );

        let built = build_project_overview_memory(&runtime).expect("must not fail");
        state.open_project_overview(
            built.decisions,
            built.todos,
            built.todos_omitted,
            Vec::new(),
            String::new(),
            None,
        );

        assert_eq!(state.overlay(), Some(state::Overlay::ProjectOverview));
        let overview = state.project_overview().expect("open");
        assert!(
            overview
                .decisions()
                .iter()
                .any(|line| line.contains("never run ci-local beside cargo"))
        );
    }
}

/// Phase 41 lines 1657-1660 and 1663: [`resource_capacity_line`]'s honesty
/// rules, each proven directly against a hand-built
/// [`crate::provider::quota::CapacityState`] — the same construction
/// technique `provider::quota`'s own tests use, entirely through public
/// constructors, so every case is fast and needs no runtime or on-disk
/// config — plus one test that goes through
/// [`build_project_overview_capacity`]'s real config-file and
/// gateway-quota-cache reads, so the formatter is proven reachable from a
/// real configured provider and not only from a hand-built fixture
/// (practice §35).
#[cfg(test)]
mod project_overview_capacity_tests {
    use super::*;
    use crate::provider::quota::{
        Capacity, CapacityBandThresholds, CapacityState, NativeAmount, Pool, Reading,
        ReadingSource, WindowCapacity, WindowShape, Windows,
    };

    const NOW: i64 = 1_800_000_000;

    /// A `requests` pool whose remaining and limit both came from a
    /// provider's own response header — [`ReadingSource::ResponseHeader`],
    /// which is the only [`crate::provider::quota::TelemetryClass::Authoritative`]
    /// producer, so [`crate::provider::quota::Percentage::exact`] answers
    /// `Some` for it.
    fn measured_requests_pool(remaining: i64, limit: i64) -> Pool {
        Pool::unmeasured()
            .with_limit(Capacity::Measured(Reading::new(
                NativeAmount::whole(limit, "requests"),
                NOW,
                ReadingSource::ResponseHeader("x-ratelimit-limit-requests".to_owned()),
            )))
            .with_remaining(Capacity::Measured(Reading::new(
                NativeAmount::whole(remaining, "requests"),
                NOW,
                ReadingSource::ResponseHeader("x-ratelimit-remaining-requests".to_owned()),
            )))
    }

    /// The same pool, with `remaining` inferred rather than read from the
    /// provider — [`ReadingSource::InferredEstimate`], so the combined
    /// percentage can never be [`crate::provider::quota::Percentage::Exact`].
    fn estimated_requests_pool(remaining: i64, limit: i64) -> Pool {
        Pool::unmeasured()
            .with_limit(Capacity::Measured(Reading::new(
                NativeAmount::whole(limit, "requests"),
                NOW,
                ReadingSource::ResponseHeader("x-ratelimit-limit-requests".to_owned()),
            )))
            .with_remaining(Capacity::Measured(Reading::new(
                NativeAmount::whole(remaining, "requests"),
                NOW,
                ReadingSource::InferredEstimate("recent usage".to_owned()),
            )))
    }

    fn with_reset(state: CapacityState, seconds_from_now: i64) -> CapacityState {
        let windows = Windows::uniform(Pool::unmeasured(), Capacity::Unmeasured).with_rolling(
            WindowCapacity::uniform(
                WindowShape::Rolling,
                Pool::unmeasured(),
                Capacity::Unmeasured,
            )
            .with_resets_at(Capacity::Measured(Reading::new(
                NOW + seconds_from_now,
                NOW,
                ReadingSource::ResponseHeader("x-ratelimit-reset-requests".to_owned()),
            ))),
        );
        state.with_windows(windows)
    }

    /// **Line 1283's killer.** A surfaced forecast is an *estimate*, and the
    /// exact words are the capability.
    ///
    /// This asserts the hedges positively and the promise words negatively,
    /// because those are two different failures: dropping `about` weakens
    /// the hedge, and adding `will` replaces it with a commitment. Either
    /// one is the mutation this test exists to catch.
    #[test]
    fn a_surfaced_forecast_is_hedged_and_never_promises() {
        let state = CapacityState::metered_balance().with_requests(measured_requests_pool(12, 100));
        let forecast = crate::routing::burn::ExhaustionForecast {
            requests_per_hour: 30.0,
            seconds_to_exhaustion: 5_400,
            survives_until_reset: Some(false),
            seconds_until_reset: Some(28_800),
            rows: 42,
        };
        let line = resource_capacity_line(
            "openrouter (remote)",
            &state,
            &CapacityBandThresholds::DEFAULT,
            20,
            NOW,
            Some(forecast),
        );

        assert!(
            line.contains("estimated to last about 1.5h at the current rate"),
            "the forecast must be surfaced as an estimate: {line}"
        );
        assert!(
            line.contains("may not reach its reset at the current rate"),
            "the verdict must be hedged and must name its assumption: {line}"
        );
        assert!(
            line.contains("over 42 observations"),
            "a reader must be able to see how much history the estimate rests on: {line}"
        );

        for promise in [
            "will last",
            "will run out",
            "will not reach",
            "guaranteed",
            "certainly",
            "exhausts at",
        ] {
            assert!(
                !line.contains(promise),
                "a forecast must never promise; found `{promise}` in: {line}"
            );
        }
    }

    /// The inert case, and it is the one that keeps this build honest: with
    /// no forecast the line is **byte-identical** to what it was before
    /// Phase 32E. Asserted as an exact string rather than an absence of
    /// words, because an absence assertion cannot catch a stray separator.
    #[test]
    fn a_resource_with_no_forecast_prints_exactly_what_it_printed_before() {
        let state = CapacityState::metered_balance().with_requests(measured_requests_pool(82, 100));
        let line = resource_capacity_line(
            "openrouter (remote)",
            &state,
            &CapacityBandThresholds::DEFAULT,
            20,
            NOW,
            None,
        );
        assert_eq!(line, "  openrouter (remote)  plenty 82% [measured]");
        assert_eq!(
            crate::shell::forecast_note(None),
            "",
            "no forecast contributes no characters at all"
        );
    }

    /// Map lines 1658 and 1659: a measured reading renders its band and the
    /// literal word `"measured"`.
    #[test]
    fn a_measured_reading_renders_its_band_and_says_measured() {
        let state = CapacityState::metered_balance().with_requests(measured_requests_pool(82, 100));
        let line = resource_capacity_line(
            "openrouter (remote)",
            &state,
            &CapacityBandThresholds::DEFAULT,
            20,
            NOW,
            None,
        );
        assert!(line.contains("82%"), "{line}");
        assert!(line.contains("[measured]"), "{line}");
        assert!(line.contains("plenty"), "{line}");
    }

    /// Map line 1659: the same resource with only an estimated reading
    /// renders the estimate labelled as one, and the two renderings differ —
    /// the mutation this test kills is `remove-validation` dropping the
    /// measured/estimated label.
    #[test]
    fn a_measured_and_an_estimated_reading_of_the_same_resource_render_differently() {
        let thresholds = CapacityBandThresholds::DEFAULT;
        let measured =
            CapacityState::metered_balance().with_requests(measured_requests_pool(82, 100));
        let estimated =
            CapacityState::metered_balance().with_requests(estimated_requests_pool(82, 100));

        let measured_line =
            resource_capacity_line("openrouter (remote)", &measured, &thresholds, 20, NOW, None);
        let estimated_line = resource_capacity_line(
            "openrouter (remote)",
            &estimated,
            &thresholds,
            20,
            NOW,
            None,
        );

        assert!(measured_line.contains("[measured]"), "{measured_line}");
        assert!(estimated_line.contains("[estimated]"), "{estimated_line}");
        assert_ne!(measured_line, estimated_line);
    }

    /// Map lines 1658 and 1659: a resource with no telemetry at all renders
    /// `"unknown"` and no number anywhere — the mutation this test kills is
    /// `accept-stale-state` rendering an unknown capacity as a number.
    #[test]
    fn no_telemetry_renders_unknown_with_no_number_at_all() {
        let state = CapacityState::metered_balance();
        let line = resource_capacity_line(
            "openrouter (remote)",
            &state,
            &CapacityBandThresholds::DEFAULT,
            20,
            NOW,
            None,
        );
        assert!(line.contains("unknown"), "{line}");
        assert!(
            !line.chars().any(|c| c.is_ascii_digit()),
            "must show no number at all: {line}"
        );
    }

    /// Map line 1660: a constrained resource — one whose reset time is
    /// actually known — renders it.
    #[test]
    fn a_constrained_resource_with_a_known_reset_shows_it() {
        let state = with_reset(
            CapacityState::metered_balance().with_requests(measured_requests_pool(82, 100)),
            3600,
        );
        let line = resource_capacity_line(
            "openrouter (remote)",
            &state,
            &CapacityBandThresholds::DEFAULT,
            20,
            NOW,
            None,
        );
        assert!(line.contains("reset in 3600s"), "{line}");
    }

    /// Map line 1660: the same resource with no reset ever read renders
    /// none — the two renderings the acceptance test asks to differ.
    #[test]
    fn an_unconstrained_resource_shows_no_reset() {
        let state = CapacityState::metered_balance().with_requests(measured_requests_pool(82, 100));
        let line = resource_capacity_line(
            "openrouter (remote)",
            &state,
            &CapacityBandThresholds::DEFAULT,
            20,
            NOW,
            None,
        );
        assert!(!line.contains("reset"), "{line}");
    }

    /// Map line 1663: a reserve that currently gates routing — this
    /// resource's band, folded with its reserve percentage, has crossed into
    /// [`crate::provider::quota::CapacityBand::Reserve`], the exact boundary
    /// `crate::provider::quota::evaluate_reserve_spend` itself stops
    /// trivially allowing at — appears.
    #[test]
    fn a_reserve_that_currently_gates_routing_appears() {
        // 10% is below `CapacityBandThresholds::DEFAULT`'s 15% reserve
        // boundary, so the band is `Reserve` and the policy actually runs.
        let state = CapacityState::metered_balance().with_requests(measured_requests_pool(10, 100));
        let line = resource_capacity_line(
            "openrouter (remote)",
            &state,
            &CapacityBandThresholds::DEFAULT,
            20,
            NOW,
            None,
        );
        assert!(line.contains("protected reserve 20%"), "{line}");
        assert!(line.contains("limiting routing"), "{line}");
    }

    /// Map line 1663: a reserve that has influenced nothing — this
    /// resource's band is well above `Reserve` — does not appear. The
    /// mutation this test kills is `invert-condition` showing a reserve that
    /// influenced nothing.
    #[test]
    fn a_reserve_that_influences_nothing_does_not_appear() {
        let state = CapacityState::metered_balance().with_requests(measured_requests_pool(80, 100));
        let line = resource_capacity_line(
            "openrouter (remote)",
            &state,
            &CapacityBandThresholds::DEFAULT,
            20,
            NOW,
            None,
        );
        assert!(!line.contains("reserve"), "{line}");
    }

    /// Bootstrap a `Runtime` over fresh, isolated data/config/workspace
    /// directories, matching `project_overview_tests::bootstrapped_runtime`.
    fn bootstrapped_runtime() -> (tempfile::TempDir, tempfile::TempDir, crate::Runtime) {
        use clap::Parser;

        let data = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(workspace.path().join(".git")).unwrap();

        let cli = crate::Cli::try_parse_from([
            "glasshouse",
            "--data-dir",
            data.path().to_str().unwrap(),
            "--config-dir",
            data.path().to_str().unwrap(),
        ])
        .unwrap();
        let runtime = crate::bootstrap(&cli, workspace.path()).unwrap();
        (data, workspace, runtime)
    }

    /// A project with no configured provider shows no resource lines rather
    /// than the full, unconfigured `provider::registry` catalog — the
    /// behavioral contract's "configured resources", read through
    /// [`EffectiveConfig::provider_names`].
    #[test]
    fn no_configured_providers_yields_no_resource_lines() {
        let (_data, _workspace, runtime) = bootstrapped_runtime();
        let lines = build_project_overview_capacity(&runtime);
        assert!(lines.is_empty(), "{lines:?}");
    }

    /// [`build_project_overview_capacity`] reached through its real callers
    /// — a real configured provider on disk, and a real planted
    /// [`crate::provider::telemetry::GatewayQuotaCache`] reading, the same
    /// on-disk bridge `main.rs::resources_report` and
    /// `main.rs::disposable_candidate_capacity` already read — not a
    /// hand-built [`crate::provider::quota::CapacityState`] a test
    /// constructed itself (practice §35).
    #[test]
    fn build_project_overview_capacity_reads_a_real_configured_provider_and_a_real_planted_reading()
    {
        let (_data, _workspace, runtime) = bootstrapped_runtime();

        let mut user = UserConfig::load(runtime.paths()).unwrap();
        let provider = crate::config::ProviderConfig::new("openai-compatible");
        user.providers_mut()
            .set("overview-capacity-test-provider", provider);
        user.save(runtime.paths()).unwrap();

        let now_unix = crate::provider::cache::now_unix_seconds();
        crate::provider::telemetry::GatewayQuotaCache::new(runtime.paths()).store(
            "overview-capacity-test-provider",
            &crate::provider::telemetry::RateLimitHeaders::read(vec![
                ("x-ratelimit-limit-requests", "100"),
                ("x-ratelimit-remaining-requests", "82"),
            ]),
            now_unix,
        );

        let lines = build_project_overview_capacity(&runtime);
        assert_eq!(lines.len(), 1, "{lines:?}");
        assert!(
            lines[0].contains("overview-capacity-test-provider"),
            "{lines:?}"
        );
        assert!(lines[0].contains("82%"), "{lines:?}");
        assert!(lines[0].contains("[measured]"), "{lines:?}");
    }

    /// Line 1276's production caller. `task_class_request_rates` names a
    /// class the moment it has at least one live row
    /// (`routing::burn::task_class_rates_name_only_the_classes_that_have_rows`
    /// plants six and gets a rate back) — there is no
    /// `MIN_ROWS_FOR_BURN_RATE` gate on this reader, unlike
    /// [`crate::routing::burn::burn_rate`]. So this plants a class with rows
    /// and a class with none, real timestamps through the real ledger
    /// (practice §35), and asserts the populated class's line and the
    /// missing class's absence.
    #[test]
    fn a_class_with_recent_rows_prints_a_hedged_line_and_an_absent_class_prints_nothing() {
        use crate::routing::evidence::{EvidenceLedger, NewObservation};
        use crate::routing::request::TaskClass;

        let (_data, _workspace, runtime) = bootstrapped_runtime();

        let mut user = UserConfig::load(runtime.paths()).unwrap();
        let provider = crate::config::ProviderConfig::new("openai-compatible");
        user.providers_mut()
            .set("overview-capacity-test-provider", provider);
        user.save(runtime.paths()).unwrap();

        let now_unix = crate::provider::cache::now_unix_seconds();
        let ledger = EvidenceLedger::open(&runtime).unwrap();
        for i in 0..12 {
            ledger
                .record(
                    NewObservation::new("glasshouse", "session-router")
                        .with_harness(Some("claude-code"))
                        .with_task_class(Some(TaskClass::Question)),
                    now_unix - 3600 + i * 60,
                )
                .unwrap();
        }

        let lines = build_project_overview_capacity(&runtime);
        let class_line = lines
            .iter()
            .find(|line| line.contains("requests by task class"))
            .unwrap_or_else(|| panic!("no task-class line in {lines:?}"));
        assert!(class_line.contains("recent"), "{class_line}");
        assert!(class_line.contains("estimated"), "{class_line}");
        assert!(class_line.contains("question"), "{class_line}");
        assert!(class_line.contains("/h"), "{class_line}");
        assert!(
            !class_line.contains("code modification"),
            "a class with no rows must not appear: {class_line}"
        );
    }

    /// The orchestrator's follow-up decision: `task_class_request_rates`
    /// itself has no row-count floor, so a class with too few rows to be a
    /// meaningful moving average must be gated in this module, at the same
    /// `MIN_ROWS_FOR_BURN_RATE` the per-resource burn rate line already
    /// enforces. Three rows of one class, twelve of another — only the
    /// twelve-row class may print.
    #[test]
    fn a_class_below_the_minimum_row_count_does_not_print_even_though_the_reader_would_name_it() {
        use crate::routing::evidence::{EvidenceLedger, NewObservation};
        use crate::routing::request::TaskClass;

        let (_data, _workspace, runtime) = bootstrapped_runtime();

        let mut user = UserConfig::load(runtime.paths()).unwrap();
        let provider = crate::config::ProviderConfig::new("openai-compatible");
        user.providers_mut()
            .set("overview-capacity-test-provider", provider);
        user.save(runtime.paths()).unwrap();

        let now_unix = crate::provider::cache::now_unix_seconds();
        let ledger = EvidenceLedger::open(&runtime).unwrap();
        // 3 rows is below `MIN_ROWS_FOR_BURN_RATE` (8); 12 is above it.
        for i in 0..3 {
            ledger
                .record(
                    NewObservation::new("glasshouse", "session-router")
                        .with_harness(Some("claude-code"))
                        .with_task_class(Some(TaskClass::Investigation)),
                    now_unix - 3600 + i * 60,
                )
                .unwrap();
        }
        for i in 0..12 {
            ledger
                .record(
                    NewObservation::new("glasshouse", "session-router")
                        .with_harness(Some("claude-code"))
                        .with_task_class(Some(TaskClass::CodeModification)),
                    now_unix - 3600 + i * 60,
                )
                .unwrap();
        }

        let lines = build_project_overview_capacity(&runtime);
        let class_line = lines
            .iter()
            .find(|line| line.contains("requests by task class"))
            .unwrap_or_else(|| panic!("no task-class line in {lines:?}"));
        assert!(class_line.contains("code modification"), "{class_line}");
        assert!(
            !class_line.contains("investigation"),
            "a class with fewer than MIN_ROWS_FOR_BURN_RATE rows must not print, \
             even though the reader itself would have named it: {class_line}"
        );
    }

    /// The other half of line 1276's contract: an empty ledger prints
    /// **exactly** what the overview printed before this line's call was
    /// wired in — byte-identical, not merely "no task-class words".
    #[test]
    fn an_empty_ledger_prints_the_capacity_overview_byte_identical_to_before() {
        let (_data, _workspace, runtime) = bootstrapped_runtime();

        let mut user = UserConfig::load(runtime.paths()).unwrap();
        let provider = crate::config::ProviderConfig::new("openai-compatible");
        user.providers_mut()
            .set("overview-capacity-test-provider", provider);
        user.save(runtime.paths()).unwrap();

        let now_unix = crate::provider::cache::now_unix_seconds();
        crate::provider::telemetry::GatewayQuotaCache::new(runtime.paths()).store(
            "overview-capacity-test-provider",
            &crate::provider::telemetry::RateLimitHeaders::read(vec![
                ("x-ratelimit-limit-requests", "100"),
                ("x-ratelimit-remaining-requests", "82"),
            ]),
            now_unix,
        );

        let lines = build_project_overview_capacity(&runtime);
        assert_eq!(lines.len(), 1, "{lines:?}");
        assert!(
            lines[0].contains("overview-capacity-test-provider"),
            "{lines:?}"
        );
        assert!(lines[0].contains("82%"), "{lines:?}");
        assert!(lines[0].contains("[measured]"), "{lines:?}");
        assert!(
            !lines[0].contains("task class"),
            "no ledger rows means no task-class line, unchanged from before: {lines:?}"
        );
    }

    /// The boundary itself: exactly `MIN_ROWS_FOR_BURN_RATE` live rows is
    /// enough for the class to print. Pins the low edge of the audit's
    /// `>=` → `>` off-by-one mutation (`docs/product/evidence/phase-32e.md`,
    /// *1276, audit note 2026-09-02*).
    #[test]
    fn a_class_with_exactly_the_minimum_row_count_prints() {
        use crate::routing::evidence::{EvidenceLedger, NewObservation};
        use crate::routing::request::TaskClass;

        let (_data, _workspace, runtime) = bootstrapped_runtime();

        let mut user = UserConfig::load(runtime.paths()).unwrap();
        let provider = crate::config::ProviderConfig::new("openai-compatible");
        user.providers_mut()
            .set("overview-capacity-test-provider", provider);
        user.save(runtime.paths()).unwrap();

        let now_unix = crate::provider::cache::now_unix_seconds();
        let ledger = EvidenceLedger::open(&runtime).unwrap();
        for i in 0..crate::routing::burn::MIN_ROWS_FOR_BURN_RATE {
            ledger
                .record(
                    NewObservation::new("glasshouse", "session-router")
                        .with_harness(Some("claude-code"))
                        .with_task_class(Some(TaskClass::CodeModification)),
                    now_unix - 3600 + (i as i64) * 60,
                )
                .unwrap();
        }

        let lines = build_project_overview_capacity(&runtime);
        let class_line = lines
            .iter()
            .find(|line| line.contains("requests by task class"))
            .unwrap_or_else(|| panic!("no task-class line in {lines:?}"));
        assert!(
            class_line.contains("code modification"),
            "exactly MIN_ROWS_FOR_BURN_RATE rows must be enough to print: {class_line}"
        );
    }

    /// The other edge: one row short of `MIN_ROWS_FOR_BURN_RATE` and the
    /// class must not print. Pins the high edge of the same off-by-one.
    #[test]
    fn a_class_with_one_fewer_than_the_minimum_row_count_does_not_print() {
        use crate::routing::evidence::{EvidenceLedger, NewObservation};
        use crate::routing::request::TaskClass;

        let (_data, _workspace, runtime) = bootstrapped_runtime();

        let mut user = UserConfig::load(runtime.paths()).unwrap();
        let provider = crate::config::ProviderConfig::new("openai-compatible");
        user.providers_mut()
            .set("overview-capacity-test-provider", provider);
        user.save(runtime.paths()).unwrap();

        let now_unix = crate::provider::cache::now_unix_seconds();
        let ledger = EvidenceLedger::open(&runtime).unwrap();
        for i in 0..(crate::routing::burn::MIN_ROWS_FOR_BURN_RATE - 1) {
            ledger
                .record(
                    NewObservation::new("glasshouse", "session-router")
                        .with_harness(Some("claude-code"))
                        .with_task_class(Some(TaskClass::CodeModification)),
                    now_unix - 3600 + (i as i64) * 60,
                )
                .unwrap();
        }

        let lines = build_project_overview_capacity(&runtime);
        let has_class_line = lines
            .iter()
            .any(|line| line.contains("requests by task class"));
        assert!(
            !has_class_line,
            "MIN_ROWS_FOR_BURN_RATE - 1 rows must not be enough to print: {lines:?}"
        );
    }

    /// **Line 1275.** A class whose rows all carry token counts, and clear
    /// the same floor as the request figure, prints a token-per-hour
    /// figure beside its request rate.
    #[test]
    fn a_class_with_token_carrying_rows_prints_a_token_figure() {
        use crate::routing::evidence::{EvidenceLedger, NewObservation};
        use crate::routing::request::TaskClass;

        let (_data, _workspace, runtime) = bootstrapped_runtime();

        let mut user = UserConfig::load(runtime.paths()).unwrap();
        let provider = crate::config::ProviderConfig::new("openai-compatible");
        user.providers_mut()
            .set("overview-capacity-test-provider", provider);
        user.save(runtime.paths()).unwrap();

        let now_unix = crate::provider::cache::now_unix_seconds();
        let ledger = EvidenceLedger::open(&runtime).unwrap();
        for i in 0..12 {
            ledger
                .record(
                    NewObservation::new("glasshouse", "session-router")
                        .with_harness(Some("claude-code"))
                        .with_task_class(Some(TaskClass::CodeModification))
                        .with_tokens(Some(100), Some(50), None),
                    now_unix - 3600 + i * 60,
                )
                .unwrap();
        }

        let lines = build_project_overview_capacity(&runtime);
        let class_line = lines
            .iter()
            .find(|line| line.contains("requests by task class"))
            .unwrap_or_else(|| panic!("no task-class line in {lines:?}"));
        assert!(class_line.contains("tok/h"), "{class_line}");
        assert!(!class_line.contains("tokens not counted"), "{class_line}");
    }

    /// The other half: a class seeded without any token count clears the
    /// request floor but shows the words, never a fabricated `0 tok/h`.
    #[test]
    fn a_class_seeded_without_tokens_shows_tokens_not_counted() {
        use crate::routing::evidence::{EvidenceLedger, NewObservation};
        use crate::routing::request::TaskClass;

        let (_data, _workspace, runtime) = bootstrapped_runtime();

        let mut user = UserConfig::load(runtime.paths()).unwrap();
        let provider = crate::config::ProviderConfig::new("openai-compatible");
        user.providers_mut()
            .set("overview-capacity-test-provider", provider);
        user.save(runtime.paths()).unwrap();

        let now_unix = crate::provider::cache::now_unix_seconds();
        let ledger = EvidenceLedger::open(&runtime).unwrap();
        for i in 0..12 {
            ledger
                .record(
                    NewObservation::new("glasshouse", "session-router")
                        .with_harness(Some("claude-code"))
                        .with_task_class(Some(TaskClass::CodeModification)),
                    now_unix - 3600 + i * 60,
                )
                .unwrap();
        }

        let lines = build_project_overview_capacity(&runtime);
        let class_line = lines
            .iter()
            .find(|line| line.contains("requests by task class"))
            .unwrap_or_else(|| panic!("no task-class line in {lines:?}"));
        assert!(class_line.contains("tokens not counted"), "{class_line}");
        assert!(!class_line.contains("tok/h"), "{class_line}");
    }

    /// The token floor's low edge: exactly `MIN_ROWS_FOR_BURN_RATE`
    /// token-carrying rows is enough to print the figure.
    #[test]
    fn a_class_with_exactly_the_minimum_token_row_count_prints_the_token_figure() {
        use crate::routing::evidence::{EvidenceLedger, NewObservation};
        use crate::routing::request::TaskClass;

        let (_data, _workspace, runtime) = bootstrapped_runtime();

        let mut user = UserConfig::load(runtime.paths()).unwrap();
        let provider = crate::config::ProviderConfig::new("openai-compatible");
        user.providers_mut()
            .set("overview-capacity-test-provider", provider);
        user.save(runtime.paths()).unwrap();

        let now_unix = crate::provider::cache::now_unix_seconds();
        let ledger = EvidenceLedger::open(&runtime).unwrap();
        for i in 0..crate::routing::burn::MIN_ROWS_FOR_BURN_RATE {
            ledger
                .record(
                    NewObservation::new("glasshouse", "session-router")
                        .with_harness(Some("claude-code"))
                        .with_task_class(Some(TaskClass::CodeModification))
                        .with_tokens(Some(100), Some(50), None),
                    now_unix - 3600 + (i as i64) * 60,
                )
                .unwrap();
        }

        let lines = build_project_overview_capacity(&runtime);
        let class_line = lines
            .iter()
            .find(|line| line.contains("requests by task class"))
            .unwrap_or_else(|| panic!("no task-class line in {lines:?}"));
        assert!(
            class_line.contains("tok/h"),
            "exactly MIN_ROWS_FOR_BURN_RATE token rows must be enough to print: {class_line}"
        );
    }

    /// The token floor's high edge: one row short of `MIN_ROWS_FOR_BURN_RATE`
    /// token-carrying rows shows the words instead, even though the request
    /// figure — seeded above its own floor with untokened rows filling the
    /// rest — still prints.
    #[test]
    fn a_class_with_one_fewer_than_the_minimum_token_row_count_shows_tokens_not_counted() {
        use crate::routing::evidence::{EvidenceLedger, NewObservation};
        use crate::routing::request::TaskClass;

        let (_data, _workspace, runtime) = bootstrapped_runtime();

        let mut user = UserConfig::load(runtime.paths()).unwrap();
        let provider = crate::config::ProviderConfig::new("openai-compatible");
        user.providers_mut()
            .set("overview-capacity-test-provider", provider);
        user.save(runtime.paths()).unwrap();

        let now_unix = crate::provider::cache::now_unix_seconds();
        let ledger = EvidenceLedger::open(&runtime).unwrap();
        let token_rows = crate::routing::burn::MIN_ROWS_FOR_BURN_RATE - 1;
        for i in 0..token_rows {
            ledger
                .record(
                    NewObservation::new("glasshouse", "session-router")
                        .with_harness(Some("claude-code"))
                        .with_task_class(Some(TaskClass::CodeModification))
                        .with_tokens(Some(100), Some(50), None),
                    now_unix - 3600 + (i as i64) * 60,
                )
                .unwrap();
        }
        // Fill the request figure's own floor with untokened rows so the
        // request rate prints regardless of the token boundary this test
        // pins.
        for i in token_rows..(token_rows + crate::routing::burn::MIN_ROWS_FOR_BURN_RATE) {
            ledger
                .record(
                    NewObservation::new("glasshouse", "session-router")
                        .with_harness(Some("claude-code"))
                        .with_task_class(Some(TaskClass::CodeModification)),
                    now_unix - 3600 + (i as i64) * 60,
                )
                .unwrap();
        }

        let lines = build_project_overview_capacity(&runtime);
        let class_line = lines
            .iter()
            .find(|line| line.contains("requests by task class"))
            .unwrap_or_else(|| panic!("no task-class line in {lines:?}"));
        assert!(
            class_line.contains("tokens not counted"),
            "MIN_ROWS_FOR_BURN_RATE - 1 token rows must not be enough to print a figure: \
             {class_line}"
        );
        assert!(!class_line.contains("tok/h"), "{class_line}");
    }
}

/// Map line 1661: [`build_project_overview_routing`] reached through its
/// real callers — a real routing choice on disk, resolved against real
/// configured providers, and a real [`crate::routing::evidence::EvidenceLedger`]
/// with planted observations, not a hand-built `RoutingSummary` a test
/// constructed itself (practice §35).
#[cfg(test)]
mod project_overview_routing_tests {
    use super::*;
    use crate::config::{RoutingModelChoice, UserConfig};
    use crate::routing::evidence::{
        EvidenceLedger, MIN_SAMPLE_FOR_SUMMARY, NewObservation, Outcome,
    };

    /// Same bootstrap `project_overview_capacity_tests` uses — an isolated,
    /// real on-disk project database, not a fixture that reimplements the
    /// query.
    fn bootstrapped_runtime() -> (tempfile::TempDir, tempfile::TempDir, crate::Runtime) {
        use clap::Parser;

        let data = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(workspace.path().join(".git")).unwrap();

        let cli = crate::Cli::try_parse_from([
            "glasshouse",
            "--data-dir",
            data.path().to_str().unwrap(),
            "--config-dir",
            data.path().to_str().unwrap(),
        ])
        .unwrap();
        let runtime = crate::bootstrap(&cli, workspace.path()).unwrap();
        (data, workspace, runtime)
    }

    fn pin_routing_model(runtime: &crate::Runtime, provider: &str, model: &str) {
        let mut user = UserConfig::load(runtime.paths()).unwrap();
        user.providers_mut().set(
            provider,
            crate::config::ProviderConfig::new("openai-compatible"),
        );
        user.routing_mut()
            .set_model(Some(RoutingModelChoice::Pinned {
                provider: provider.to_owned(),
                model: model.to_owned(),
            }));
        user.save(runtime.paths()).unwrap();
    }

    fn plant_observations(
        runtime: &crate::Runtime,
        provider: &str,
        model: &str,
        count: usize,
        duration_seconds: i64,
    ) {
        let ledger = EvidenceLedger::open(runtime).unwrap();
        // Real recent timestamps, not a tiny epoch offset: the production
        // function this test exercises windows against the real clock
        // (`crate::provider::cache::now_unix_seconds`), so an observation
        // timestamped near the Unix epoch would fall outside that window and
        // be silently excluded — every observation here must actually be
        // "recent" for the same reason the line it feeds is called that.
        let base = crate::provider::cache::now_unix_seconds() - (count as i64 * 10) - 100;
        for i in 0..count {
            let at = base + i as i64 * 10;
            let new = NewObservation::new(provider, model)
                .with_route(Some("anthropic-messages"))
                .with_harness(Some("claude-code"))
                .with_timing(Some(at), Some(at + duration_seconds))
                .with_outcome(Outcome::Succeeded);
            ledger.record(new, at).unwrap();
        }
    }

    /// No routing model configured: the default is deterministic, which
    /// names no single model — the line says so rather than showing a
    /// project-wide average attributed to a name that did not earn it
    /// (ruling 3).
    #[test]
    fn no_pinned_routing_model_reports_not_applicable() {
        let (_data, _workspace, runtime) = bootstrapped_runtime();
        let line = build_project_overview_routing(&runtime);
        assert!(line.contains("deterministic heuristics"), "{line}");
        assert!(line.contains("not applicable"), "{line}");
    }

    /// [`RoutingModelChoice::Automatic`] names no single model either —
    /// Phase 34C's dynamic choice has no production caller yet.
    #[test]
    fn automatic_routing_reports_not_applicable_latency() {
        let (_data, _workspace, runtime) = bootstrapped_runtime();
        let mut user = UserConfig::load(runtime.paths()).unwrap();
        user.routing_mut()
            .set_model(Some(RoutingModelChoice::Automatic));
        user.save(runtime.paths()).unwrap();

        let line = build_project_overview_routing(&runtime);
        assert!(line.contains("automatic"), "{line}");
        assert!(line.contains("not applicable"), "{line}");
    }

    /// Acceptance test 1: with enough observations for the selected model,
    /// the overview shows a real latency figure.
    #[test]
    fn a_pinned_model_with_enough_observations_shows_a_real_latency_figure() {
        let (_data, _workspace, runtime) = bootstrapped_runtime();
        pin_routing_model(&runtime, "anyrouter", "claude-opus-4-1");
        plant_observations(
            &runtime,
            "anyrouter",
            "claude-opus-4-1",
            MIN_SAMPLE_FOR_SUMMARY,
            2,
        );

        let line = build_project_overview_routing(&runtime);
        assert!(line.contains("anyrouter:claude-opus-4-1"), "{line}");
        assert!(line.contains("median 2000ms"), "{line}");
        assert!(!line.contains("unknown"), "{line}");
    }

    /// Acceptance test 2 / ruling 1: below the minimum sample, the line must
    /// say `unknown` rather than rendering `0ms` — the mutation this proof
    /// exists to kill is rendering the unknown case as a real zero.
    #[test]
    fn a_pinned_model_below_the_minimum_sample_shows_unknown_never_zero() {
        let (_data, _workspace, runtime) = bootstrapped_runtime();
        pin_routing_model(&runtime, "anyrouter", "claude-opus-4-1");
        plant_observations(
            &runtime,
            "anyrouter",
            "claude-opus-4-1",
            MIN_SAMPLE_FOR_SUMMARY - 1,
            2,
        );

        let line = build_project_overview_routing(&runtime);
        assert!(line.contains("unknown"), "{line}");
        assert!(!line.contains("0ms"), "{line}");
        assert!(!line.contains("0 ms"), "{line}");
    }

    /// Acceptance test 4, the empty-ledger half: a pinned model nothing has
    /// ever recorded an observation for degrades to the same honest
    /// `unknown`, never a panic or a blocked overview.
    #[test]
    fn a_pinned_model_with_an_empty_ledger_shows_unknown() {
        let (_data, _workspace, runtime) = bootstrapped_runtime();
        pin_routing_model(&runtime, "anyrouter", "claude-opus-4-1");

        let line = build_project_overview_routing(&runtime);
        assert!(line.contains("anyrouter:claude-opus-4-1"), "{line}");
        assert!(line.contains("unknown"), "{line}");
    }

    /// Acceptance test 3 / ruling 3: latency is attributed to the *selected*
    /// model, never a second, differently-performing model — the mutation
    /// this proof exists to kill is querying the ledger without the model
    /// filter.
    #[test]
    fn latency_is_attributed_to_the_selected_model_not_a_second_ones() {
        let (_data, _workspace, runtime) = bootstrapped_runtime();
        pin_routing_model(&runtime, "anyrouter", "fast-model");
        plant_observations(
            &runtime,
            "anyrouter",
            "fast-model",
            MIN_SAMPLE_FOR_SUMMARY,
            2,
        );
        plant_observations(
            &runtime,
            "anyrouter",
            "slow-model",
            MIN_SAMPLE_FOR_SUMMARY,
            900,
        );

        let line = build_project_overview_routing(&runtime);
        assert!(line.contains("anyrouter:fast-model"), "{line}");
        assert!(line.contains("median 2000ms"), "{line}");
        assert!(
            !line.contains("900000"),
            "the unselected model's latency must not leak into the selected model's line: {line}"
        );
    }

    /// A `Pinned` choice naming a provider that has since been removed from
    /// configuration must not read as "selected" — the resolution degrades
    /// to heuristics and says so, and there is no identity left to query for
    /// latency (§36: the caller must exercise the policy for what it is
    /// actually being asked, not a resolution that no longer holds).
    #[test]
    fn a_pinned_model_naming_a_vanished_provider_degrades_to_heuristics() {
        let (_data, _workspace, runtime) = bootstrapped_runtime();
        let mut user = UserConfig::load(runtime.paths()).unwrap();
        user.routing_mut()
            .set_model(Some(RoutingModelChoice::Pinned {
                provider: "vanished".to_owned(),
                model: "m".to_owned(),
            }));
        user.save(runtime.paths()).unwrap();
        // "vanished" is never added to `user.providers_mut()`.

        let line = build_project_overview_routing(&runtime);
        assert!(line.contains("deterministic heuristics"), "{line}");
        assert!(line.contains("no longer configured"), "{line}");
        assert!(line.contains("not applicable"), "{line}");
        assert!(!line.contains("vanished:m"), "{line}");
    }
}

/// Phase 25: the project-knowledge view reads every kind of durable project
/// memory through [`build_project_knowledge_memory`] — the production
/// function `Action::OpenProjectKnowledge`'s handler calls, not a helper
/// that re-implements the query (practice §35). Map lines 1098-1107.
#[cfg(test)]
mod project_knowledge_tests {
    use super::*;
    use crate::memory::{MemoryKind, MemoryStatus, NewMemory, ProjectMemory};

    /// Same bootstrap `project_overview_tests` uses — an isolated, real
    /// on-disk project database, not a fixture that reimplements the query.
    fn bootstrapped_runtime() -> (tempfile::TempDir, tempfile::TempDir, crate::Runtime) {
        use clap::Parser;

        let data = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(workspace.path().join(".git")).unwrap();

        let cli = crate::Cli::try_parse_from([
            "glasshouse",
            "--data-dir",
            data.path().to_str().unwrap(),
            "--config-dir",
            data.path().to_str().unwrap(),
        ])
        .unwrap();
        let runtime = crate::bootstrap(&cli, workspace.path()).unwrap();
        (data, workspace, runtime)
    }

    /// A project with no memory at all gets five empty, honest sections —
    /// not an error (map line 1098's empty-state half).
    #[test]
    fn a_project_with_no_knowledge_yet_reports_empty_sections_not_an_error() {
        let (_data, _workspace, runtime) = bootstrapped_runtime();

        let memory = build_project_knowledge_memory(&runtime).expect("must not fail");
        for section in [
            &memory.decisions,
            &memory.constraints,
            &memory.features,
            &memory.failed_attempts,
            &memory.todos,
        ] {
            assert!(section.lines.is_empty());
            assert_eq!(section.omitted, 0);
        }
    }

    /// Map line 1100, and acceptance test 3: a superseded decision is
    /// history, not active knowledge, so it must not appear in the active
    /// decisions section — only the memory that replaced it does.
    #[test]
    fn a_superseded_decision_does_not_appear_among_active_decisions() {
        let (_data, _workspace, runtime) = bootstrapped_runtime();
        let memory = ProjectMemory::open(&runtime).expect("open");
        let store = memory.store();

        let old = store
            .record(NewMemory::new(
                MemoryKind::Decision,
                "ship the old approach",
            ))
            .unwrap();
        let new = store
            .record(NewMemory::new(
                MemoryKind::Decision,
                "ship the replacement approach",
            ))
            .unwrap();
        store.supersede(&old.id, &new.id).unwrap();

        let built = build_project_knowledge_memory(&runtime).expect("must not fail");
        assert!(
            built
                .decisions
                .lines
                .iter()
                .any(|line| line.contains("ship the replacement approach"))
        );
        assert!(
            built
                .decisions
                .lines
                .iter()
                .all(|line| !line.contains("ship the old approach"))
        );
    }

    /// Map lines 1101 and 1102: known constraints and implemented-or-planned
    /// features are filtered to current knowledge the same way decisions
    /// are — a superseded record of either kind does not reach its section.
    #[test]
    fn constraints_and_features_are_filtered_to_current_the_same_way_decisions_are() {
        let (_data, _workspace, runtime) = bootstrapped_runtime();
        let memory = ProjectMemory::open(&runtime).expect("open");
        let store = memory.store();

        store
            .record(NewMemory::new(
                MemoryKind::Constraint,
                "the local gate must run alone",
            ))
            .unwrap();
        let old_constraint = store
            .record(NewMemory::new(MemoryKind::Constraint, "an old constraint"))
            .unwrap();
        let new_constraint = store
            .record(NewMemory::new(MemoryKind::Constraint, "its replacement"))
            .unwrap();
        store
            .supersede(&old_constraint.id, &new_constraint.id)
            .unwrap();

        store
            .record(NewMemory::new(MemoryKind::Feature, "the knowledge view"))
            .unwrap();
        let old_feature = store
            .record(NewMemory::new(MemoryKind::Feature, "an old feature plan"))
            .unwrap();
        let new_feature = store
            .record(NewMemory::new(MemoryKind::Feature, "the revised plan"))
            .unwrap();
        store.supersede(&old_feature.id, &new_feature.id).unwrap();

        let built = build_project_knowledge_memory(&runtime).expect("must not fail");
        assert_eq!(built.constraints.lines.len(), 2);
        assert!(
            built
                .constraints
                .lines
                .iter()
                .all(|line| !line.contains("an old constraint"))
        );
        assert_eq!(built.features.lines.len(), 2);
        assert!(
            built
                .features
                .lines
                .iter()
                .all(|line| !line.contains("an old feature plan"))
        );
    }

    /// Map line 1104, and acceptance test 3's other half: a resolved todo is
    /// queryable but must never be presented as unresolved work.
    #[test]
    fn a_resolved_todo_does_not_appear_among_unresolved_todos() {
        let (_data, _workspace, runtime) = bootstrapped_runtime();
        let memory = ProjectMemory::open(&runtime).expect("open");
        let store = memory.store();

        store
            .record(NewMemory::new(MemoryKind::Todo, "wire the knowledge view"))
            .unwrap();
        let resolved = store
            .record(NewMemory::new(MemoryKind::Todo, "already done"))
            .unwrap();
        store
            .set_status(&resolved.id, MemoryStatus::Resolved)
            .unwrap();

        let built = build_project_knowledge_memory(&runtime).expect("must not fail");
        assert_eq!(built.todos.lines.len(), 1);
        assert!(built.todos.lines[0].contains("wire the knowledge view"));
        assert!(
            built
                .todos
                .lines
                .iter()
                .all(|line| !line.contains("already done"))
        );
    }

    /// Map line 1104 turns on `MemoryStatus::is_open_work`, not
    /// `is_current`: a todo under review is not `Active`, but it is still
    /// open work and must still count as unresolved. This is what would
    /// distinguish the two predicates if `knowledge_section`'s todos call
    /// were quietly narrowed to `is_current`.
    #[test]
    fn a_todo_marked_needs_review_still_counts_as_unresolved() {
        let (_data, _workspace, runtime) = bootstrapped_runtime();
        let memory = ProjectMemory::open(&runtime).expect("open");
        let store = memory.store();

        let todo = store
            .record(NewMemory::new(MemoryKind::Todo, "revisit after the audit"))
            .unwrap();
        store
            .set_status(&todo.id, MemoryStatus::NeedsReview)
            .unwrap();

        let built = build_project_knowledge_memory(&runtime).expect("must not fail");
        assert!(
            built
                .todos
                .lines
                .iter()
                .any(|line| line.contains("revisit after the audit"))
        );
    }

    /// Map lines 1103 and 1106: failed approaches are shown regardless of
    /// status — including one a newer memory has superseded — and the
    /// superseded one names its successor while the current one stays
    /// silent about supersession, since it has none.
    #[test]
    fn failed_approaches_are_shown_regardless_of_status_and_name_their_successor() {
        let (_data, _workspace, runtime) = bootstrapped_runtime();
        let memory = ProjectMemory::open(&runtime).expect("open");
        let store = memory.store();

        let old = store
            .record(NewMemory::new(
                MemoryKind::FailedAttempt,
                "tried a global lock, it deadlocked",
            ))
            .unwrap();
        let new = store
            .record(NewMemory::new(
                MemoryKind::FailedAttempt,
                "tried per-project locks instead, still fails under load",
            ))
            .unwrap();
        store.supersede(&old.id, &new.id).unwrap();

        let built = build_project_knowledge_memory(&runtime).expect("must not fail");
        assert_eq!(built.failed_attempts.lines.len(), 2);

        let old_line = built
            .failed_attempts
            .lines
            .iter()
            .find(|line| line.contains("tried a global lock"))
            .expect("the superseded failed attempt is still shown");
        assert!(
            old_line.contains(&format!("superseded by {}", new.id)),
            "must name its successor: {old_line}"
        );

        let new_line = built
            .failed_attempts
            .lines
            .iter()
            .find(|line| line.contains("tried per-project locks"))
            .expect("the current failed attempt is shown");
        assert!(
            !new_line.contains("superseded by"),
            "has no successor, so must say nothing: {new_line}"
        );
    }

    /// The `k` key opens the overlay through the real run-loop action, and
    /// the overlay carries the memory the run loop read — not a
    /// hand-constructed fixture.
    #[test]
    fn opening_the_project_knowledge_view_shows_real_memory() {
        let (_data, _workspace, runtime) = bootstrapped_runtime();
        let memory = ProjectMemory::open(&runtime).expect("open");
        memory
            .store()
            .record(NewMemory::new(
                MemoryKind::Constraint,
                "never run ci-local beside cargo",
            ))
            .unwrap();

        let mut state = state::ShellState::new(
            "glasshouse",
            runtime.project().display_root(),
            "test",
            Vec::new(),
        );
        assert_eq!(
            state.handle_key(crossterm::event::KeyEvent::from(
                crossterm::event::KeyCode::Char('k')
            )),
            state::Action::OpenProjectKnowledge
        );

        let built = build_project_knowledge_memory(&runtime).expect("must not fail");
        state.open_project_knowledge(
            built.decisions,
            built.constraints,
            built.features,
            built.failed_attempts,
            built.todos,
            None,
        );

        assert_eq!(state.overlay(), Some(state::Overlay::ProjectKnowledge));
        let knowledge = state.project_knowledge().expect("open");
        assert!(
            knowledge
                .constraints()
                .lines
                .iter()
                .any(|line| line.contains("never run ci-local beside cargo"))
        );
    }

    /// Map line 1105: [`knowledge_detail`] carries the real rationale,
    /// source session and source commit a memory was recorded with —
    /// through [`build_project_knowledge_memory`], the production function,
    /// not a hand-built fixture.
    #[test]
    fn build_project_knowledge_memory_carries_real_provenance_for_the_detail_view() {
        use crate::memory::DecisionProvenance;

        let (_data, _workspace, runtime) = bootstrapped_runtime();
        let memory = ProjectMemory::open(&runtime).expect("open");
        memory
            .store()
            .record(
                NewMemory::new(MemoryKind::Decision, "adopt the drill-down view")
                    .with_source_session(Some("sess_01AAAAAAAAAAAAAAAAAAAAAAAA"))
                    .with_source_commit(Some("d34db33f"))
                    .with_provenance(DecisionProvenance {
                        rationale: Some("answers one question at a time".to_owned()),
                        ..Default::default()
                    }),
            )
            .unwrap();

        let built = build_project_knowledge_memory(&runtime).expect("must not fail");
        assert_eq!(built.decisions.lines.len(), 1);
        assert_eq!(built.decisions.details.len(), 1);
        let detail = &built.decisions.details[0];
        assert_eq!(
            detail.rationale.as_deref(),
            Some("answers one question at a time")
        );
        assert_eq!(
            detail.source_session.as_deref(),
            Some("sess_01AAAAAAAAAAAAAAAAAAAAAAAA")
        );
        assert_eq!(detail.source_commit.as_deref(), Some("d34db33f"));
        assert_eq!(detail.lifecycle, "active");
    }

    /// Map line 1105's honesty half, at the query layer: a memory recorded
    /// with no rationale, no source session and no source commit produces a
    /// [`MemoryDetail`] with `None` in each of those fields — never an
    /// empty string standing in for "not recorded".
    #[test]
    fn build_project_knowledge_memory_leaves_unrecorded_provenance_as_none() {
        let (_data, _workspace, runtime) = bootstrapped_runtime();
        let memory = ProjectMemory::open(&runtime).expect("open");
        memory
            .store()
            .record(NewMemory::new(
                MemoryKind::Todo,
                "wire the knowledge view into main",
            ))
            .unwrap();

        let built = build_project_knowledge_memory(&runtime).expect("must not fail");
        assert_eq!(built.todos.details.len(), 1);
        let detail = &built.todos.details[0];
        assert_eq!(detail.rationale, None);
        assert_eq!(detail.source_session, None);
        assert_eq!(detail.source_commit, None);
        assert_eq!(detail.lifecycle, MemoryStatus::Active.to_string());
    }

    /// Map lines 1880 and 1881's positive half: a supersession relationship
    /// recorded through the real store (`MemoryStore::supersede`, not a
    /// hand-built `MemoryRecord`) reaches `knowledge_line` and is said in
    /// words — the successor is named, not drawn as an edge.
    #[test]
    fn a_supersession_recorded_through_the_real_store_is_named_in_the_knowledge_line() {
        let (_data, _workspace, runtime) = bootstrapped_runtime();
        let memory = ProjectMemory::open(&runtime).expect("open");
        let store = memory.store();

        let old = store
            .record(NewMemory::new(
                MemoryKind::Constraint,
                "the old relationship shape",
            ))
            .unwrap();
        let new = store
            .record(NewMemory::new(
                MemoryKind::Constraint,
                "the replacement relationship shape",
            ))
            .unwrap();
        store.supersede(&old.id, &new.id).unwrap();

        let record = store
            .get(&old.id)
            .expect("read back")
            .expect("still present");
        assert_eq!(record.superseded_by.as_ref(), Some(&new.id));

        let line = knowledge_line(&record);
        assert!(line.contains("superseded by"), "{line}");
        assert!(line.contains(&new.id.to_string()), "{line}");
    }
}

/// Map line 234: the project-memory view reads every kind of durable
/// project memory, at every status, through [`build_project_memory_view`] —
/// the production function `Action::OpenProjectMemory`'s handler calls, not
/// a helper that re-implements the query (practice §35).
#[cfg(test)]
mod project_memory_tests {
    use super::*;
    use crate::memory::{MemoryKind, MemoryStatus, NewMemory, ProjectMemory};

    /// Same bootstrap `project_knowledge_tests` uses — an isolated, real
    /// on-disk project database, not a fixture that reimplements the query.
    fn bootstrapped_runtime() -> (tempfile::TempDir, tempfile::TempDir, crate::Runtime) {
        use clap::Parser;

        let data = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(workspace.path().join(".git")).unwrap();

        let cli = crate::Cli::try_parse_from([
            "glasshouse",
            "--data-dir",
            data.path().to_str().unwrap(),
            "--config-dir",
            data.path().to_str().unwrap(),
        ])
        .unwrap();
        let runtime = crate::bootstrap(&cli, workspace.path()).unwrap();
        (data, workspace, runtime)
    }

    /// A project with no memory at all gets one empty, honest section — not
    /// an error.
    #[test]
    fn a_project_with_no_memory_yet_reports_an_empty_section_not_an_error() {
        let (_data, _workspace, runtime) = bootstrapped_runtime();

        let memory = build_project_memory_view(&runtime).expect("must not fail");
        assert!(memory.lines.is_empty());
        assert_eq!(memory.omitted, 0);
    }

    /// The whole point of this view next to `ProjectKnowledge`: a `Finding`
    /// record has no section in `build_project_knowledge_memory` at all, but
    /// it must appear here.
    #[test]
    fn a_finding_record_appears_in_the_project_memory_view() {
        let (_data, _workspace, runtime) = bootstrapped_runtime();
        let memory = ProjectMemory::open(&runtime).expect("open");
        memory
            .store()
            .record(NewMemory::new(
                MemoryKind::Finding,
                "the local gate must run alone",
            ))
            .unwrap();

        let built = build_project_memory_view(&runtime).expect("must not fail");
        assert!(
            built
                .lines
                .iter()
                .any(|line| line.contains("the local gate must run alone")),
            "{:?}",
            built.lines
        );

        let knowledge = build_project_knowledge_memory(&runtime).expect("must not fail");
        for section in [
            &knowledge.decisions,
            &knowledge.constraints,
            &knowledge.features,
            &knowledge.failed_attempts,
            &knowledge.todos,
        ] {
            assert!(
                section
                    .lines
                    .iter()
                    .all(|line| !line.contains("the local gate must run alone")),
                "a Finding must not reach any ProjectKnowledge section: {:?}",
                section.lines
            );
        }
    }

    /// Unlike `build_project_knowledge_memory`'s five sections, this view is
    /// not filtered by status: a superseded decision — invisible to the
    /// active-decisions section — is still shown here, with its status said
    /// on the line rather than implied by which section it is in.
    #[test]
    fn a_superseded_record_still_appears_here_with_its_status_on_the_line() {
        let (_data, _workspace, runtime) = bootstrapped_runtime();
        let memory = ProjectMemory::open(&runtime).expect("open");
        let store = memory.store();

        let old = store
            .record(NewMemory::new(
                MemoryKind::Decision,
                "ship the old approach",
            ))
            .unwrap();
        let new = store
            .record(NewMemory::new(
                MemoryKind::Decision,
                "ship the replacement approach",
            ))
            .unwrap();
        store.supersede(&old.id, &new.id).unwrap();

        let built = build_project_memory_view(&runtime).expect("must not fail");
        let old_line = built
            .lines
            .iter()
            .find(|line| line.contains("ship the old approach"))
            .expect("the superseded decision must still be shown");
        assert!(
            old_line.contains(&format!("[{}]", MemoryStatus::Superseded)),
            "its status must be said on the line: {old_line}"
        );
    }

    /// The `M` key opens the overlay through the real run-loop action, and
    /// the overlay carries the memory the run loop read — not a
    /// hand-constructed fixture.
    #[test]
    fn opening_the_project_memory_view_shows_real_memory() {
        let (_data, _workspace, runtime) = bootstrapped_runtime();
        let memory = ProjectMemory::open(&runtime).expect("open");
        memory
            .store()
            .record(NewMemory::new(MemoryKind::Finding, "placeholder"))
            .unwrap();

        let mut state = state::ShellState::new(
            "glasshouse",
            runtime.project().display_root(),
            "test",
            Vec::new(),
        );
        assert_eq!(
            state.handle_key(crossterm::event::KeyEvent::from(
                crossterm::event::KeyCode::Char('M')
            )),
            state::Action::OpenProjectMemory
        );

        let built = build_project_memory_view(&runtime).expect("must not fail");
        state.open_project_memory(built, None);

        assert_eq!(state.overlay(), Some(state::Overlay::ProjectMemory));
        let shown = state.project_memory().expect("open");
        assert!(
            shown
                .memory()
                .lines
                .iter()
                .any(|line| line.contains("placeholder"))
        );
    }
}

/// Phase 47 lines 1762 and 1764: the route-evidence table reads real
/// recorded routing observations through [`build_route_evidence_table`] —
/// the production function `Action::OpenRouteEvidence`'s handler calls, not
/// a helper that re-implements
/// `routing::evidence::EvidenceLedger::observed_identities` (practice §35),
/// the one method that can answer which identities exist at all
/// (practice §71).
#[cfg(test)]
mod route_evidence_tests {
    use super::*;
    use crate::routing::evidence::{EvidenceLedger, NewObservation};

    /// Same bootstrap `project_overview_tests` and `project_knowledge_tests`
    /// use — an isolated, real on-disk project database.
    fn bootstrapped_runtime() -> (tempfile::TempDir, tempfile::TempDir, crate::Runtime) {
        use clap::Parser;

        let data = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(workspace.path().join(".git")).unwrap();

        let cli = crate::Cli::try_parse_from([
            "glasshouse",
            "--data-dir",
            data.path().to_str().unwrap(),
            "--config-dir",
            data.path().to_str().unwrap(),
        ])
        .unwrap();
        let runtime = crate::bootstrap(&cli, workspace.path()).unwrap();
        (data, workspace, runtime)
    }

    /// A project with no routing evidence at all gets an honest, empty
    /// table — not an error. `EvidenceLedger::open` creates the database on
    /// first use, so "no evidence yet" and "could not read the ledger" must
    /// not collapse into the same outcome, the same rule
    /// `a_project_with_no_memory_yet_reports_empty_sections_not_an_error`
    /// proves for the project overview.
    #[test]
    fn a_project_with_no_routing_evidence_yet_reports_an_empty_table_not_an_error() {
        let (_data, _workspace, runtime) = bootstrapped_runtime();
        let rows = build_route_evidence_table(&runtime).expect("must not fail");
        assert!(rows.is_empty());
    }

    /// Real recorded observations, through the production `EvidenceLedger`,
    /// come back as distinct rows with their real sample counts — not a
    /// fixture standing in for the ledger. Acceptance test 4.
    #[test]
    fn two_recorded_identities_come_back_as_two_rows_with_real_sample_counts() {
        let (_data, _workspace, runtime) = bootstrapped_runtime();
        let ledger = EvidenceLedger::open(&runtime).expect("open");
        // `build_route_evidence_table` windows against the real wall clock
        // (`ROUTE_EVIDENCE_WINDOW_SECONDS`), so observations here must be
        // recorded near real "now" — a small fixed epoch like `1_000` would
        // fall outside every window this function ever queries.
        let now = crate::provider::cache::now_unix_seconds();
        ledger
            .record(
                NewObservation::new("anyrouter", "claude-opus-4-1")
                    .with_route(Some("anthropic-messages")),
                now - 20,
            )
            .unwrap();
        ledger
            .record(
                NewObservation::new("anyrouter", "claude-opus-4-1")
                    .with_route(Some("anthropic-messages")),
                now - 10,
            )
            .unwrap();
        ledger
            .record(NewObservation::new("openai-router", "gpt-5"), now)
            .unwrap();

        let rows = build_route_evidence_table(&runtime).expect("must not fail");
        assert_eq!(rows.len(), 2);
        let anyrouter = rows
            .iter()
            .find(|row| row.provider == "anyrouter")
            .expect("anyrouter row");
        let openai = rows
            .iter()
            .find(|row| row.provider == "openai-router")
            .expect("openai row");
        assert_eq!(anyrouter.sample_count, 2);
        assert_eq!(openai.sample_count, 1);
        assert_ne!(
            anyrouter.sample_count, openai.sample_count,
            "two identities with different counts must render differently"
        );
    }

    /// Line 1764: a row recorded with no context state — the honest default
    /// every real production row has today, since
    /// `NewObservation::with_context_state` has zero non-test callers (see
    /// `routing::evidence`'s own module header) — comes back labelled
    /// `"unknown"`, never blank and never upgraded to a measurement.
    #[test]
    fn a_row_with_no_recorded_context_state_reads_unknown() {
        let (_data, _workspace, runtime) = bootstrapped_runtime();
        let ledger = EvidenceLedger::open(&runtime).expect("open");
        let now = crate::provider::cache::now_unix_seconds();
        ledger
            .record(NewObservation::new("anyrouter", "m"), now)
            .unwrap();

        let rows = build_route_evidence_table(&runtime).expect("must not fail");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].context_state, "unknown");
    }

    /// The `r` key opens the overlay through the real run-loop action, and
    /// the overlay carries the rows the run loop actually read — not a
    /// hand-constructed fixture (practice §35).
    #[test]
    fn opening_the_route_evidence_table_shows_real_recorded_observations() {
        let (_data, _workspace, runtime) = bootstrapped_runtime();
        let ledger = EvidenceLedger::open(&runtime).expect("open");
        let now = crate::provider::cache::now_unix_seconds();
        ledger
            .record(NewObservation::new("anyrouter", "claude-opus-4-1"), now)
            .unwrap();

        let mut state = state::ShellState::new(
            "glasshouse",
            runtime.project().display_root(),
            "test",
            Vec::new(),
        );
        assert_eq!(
            state.handle_key(crossterm::event::KeyEvent::from(
                crossterm::event::KeyCode::Char('r')
            )),
            state::Action::OpenRouteEvidence
        );

        let rows = build_route_evidence_table(&runtime).expect("must not fail");
        state.open_route_evidence(rows, None);

        assert_eq!(state.overlay(), Some(state::Overlay::RouteEvidence));
        let evidence = state.route_evidence().expect("open");
        assert!(
            evidence
                .rows()
                .iter()
                .any(|row| row.provider == "anyrouter")
        );
    }
}

/// The routing-decisions view reads real recorded decisions through
/// [`build_route_decision_table`] — the production function
/// `Action::OpenRouteDecisions`'s handler calls.
///
/// These write through `crate::evaluation`'s own store rather than
/// hand-building a [`RouteDecisionRow`], for practice §35's reason. What they
/// do **not** claim is that a decision ever reaches that store from the
/// shipped binary — that is `tests/disposable_route_sink.rs`, which drives a
/// real `glasshouse hook`, and it is a separate proof on purpose.
#[cfg(test)]
mod route_decision_tests {
    use super::*;
    use crate::evaluation::{
        EvaluationKind, EvaluationObservations, NewObservation, RetrievalScope,
    };

    /// The same bootstrap `route_evidence_tests` uses — an isolated, real
    /// on-disk project database.
    fn bootstrapped_runtime() -> (tempfile::TempDir, tempfile::TempDir, crate::Runtime) {
        use clap::Parser;

        let data = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(workspace.path().join(".git")).unwrap();

        let cli = crate::Cli::try_parse_from([
            "glasshouse",
            "--data-dir",
            data.path().to_str().unwrap(),
            "--config-dir",
            data.path().to_str().unwrap(),
        ])
        .unwrap();
        let runtime = crate::bootstrap(&cli, workspace.path()).unwrap();
        (data, workspace, runtime)
    }

    /// A project that has recorded nothing gets an empty table and not an
    /// error — the state every fresh installation is in.
    #[test]
    fn a_project_with_no_recorded_decision_reads_an_empty_table() {
        let (_data, _workspace, runtime) = bootstrapped_runtime();
        let rows = build_route_decision_table(&runtime).expect("must not fail");
        assert!(rows.is_empty(), "{rows:#?}");
    }

    /// Every stored column reaches the row, and nothing else is invented.
    #[test]
    fn a_recorded_decision_reaches_the_table_with_its_rationale_intact() {
        let (_data, _workspace, runtime) = bootstrapped_runtime();
        let rationale = "a-free-model on a-provider — free, used by user preference\n  \
                         +1.000  cost — free";
        {
            let ledger = EvaluationObservations::open(&runtime).unwrap();
            ledger
                .record(
                    NewObservation::new(EvaluationKind::DisposableRouteDecided)
                        .with_subject("memory extraction")
                        .with_session_id("session-abc")
                        .with_detail(rationale),
                    1_000,
                )
                .unwrap();
        }

        let rows = build_route_decision_table(&runtime).expect("must not fail");
        assert_eq!(rows.len(), 1, "{rows:#?}");
        assert_eq!(rows[0].job, "memory extraction");
        assert_eq!(rows[0].session_id.as_deref(), Some("session-abc"));
        assert_eq!(rows[0].rationale.as_deref(), Some(rationale));
        assert_eq!(rows[0].observed_at_unix, 1_000);
    }

    /// **The narrowing, which is the whole reason `recent_of_kind` exists.**
    ///
    /// The evaluation ledger is shared. A project whose memory has been
    /// searched recently has memory-retrieval rows newer than any routing
    /// decision, so a view built on the unkeyed `recent` listing would show
    /// an empty table while the decision sat two rows down — and would show
    /// it *only* on projects that use memory, which is the worst possible
    /// place for the bug to be.
    #[test]
    fn a_newer_retrieval_of_another_kind_does_not_displace_a_recorded_decision() {
        let (_data, _workspace, runtime) = bootstrapped_runtime();
        {
            let ledger = EvaluationObservations::open(&runtime).unwrap();
            ledger
                .record(
                    NewObservation::new(EvaluationKind::DisposableRouteDecided)
                        .with_subject("memory extraction")
                        .with_detail("the decision"),
                    1_000,
                )
                .unwrap();
            // Newer, and more of them than the view will show.
            for n in 0..(ROUTE_DECISION_ROW_LIMIT * 2) {
                ledger
                    .record(
                        NewObservation::new(EvaluationKind::MemoryRetrieved)
                            .with_subject(RetrievalScope::Current.as_str())
                            .with_memory_id(format!("memory-{n}")),
                        2_000 + n as i64,
                    )
                    .unwrap();
            }
        }

        let rows = build_route_decision_table(&runtime).expect("must not fail");
        assert_eq!(
            rows.len(),
            1,
            "the decision must survive newer rows of another kind, and no retrieval may be \
             drawn as one: {rows:#?}"
        );
        assert_eq!(rows[0].rationale.as_deref(), Some("the decision"));
    }

    /// Newest first, and bounded — a reader looking at this view wants the
    /// last decision, and the bound is what keeps a long-lived project's
    /// view from being a transcript.
    ///
    /// # The fixture size is a literal, deliberately (practice §80, case 6)
    ///
    /// Written first as `ROUTE_DECISION_ROW_LIMIT + 5` recorded rows and
    /// `assert_eq!(rows.len(), ROUTE_DECISION_ROW_LIMIT)`, which is a test
    /// that **rescales with the constant it is watching**: raise the limit to
    /// 30 and the fixture dutifully records 35, the builder returns 30, and
    /// the assertion passes against a view that is no longer bounded at all.
    /// `SEEDED` is therefore a fixed number the mutation cannot move, and the
    /// bracket is `rows.len() < SEEDED` — false exactly when the limit stops
    /// being a limit.
    #[test]
    fn the_table_is_newest_first_and_bounded() {
        /// Comfortably more than the shipped limit, and independent of it.
        const SEEDED: usize = 15;

        let (_data, _workspace, runtime) = bootstrapped_runtime();
        {
            let ledger = EvaluationObservations::open(&runtime).unwrap();
            for n in 0..SEEDED {
                ledger
                    .record(
                        NewObservation::new(EvaluationKind::DisposableRouteDecided)
                            .with_subject("memory extraction")
                            .with_detail(format!("decision {n}")),
                        1_000 + n as i64,
                    )
                    .unwrap();
            }
        }

        let rows = build_route_decision_table(&runtime).expect("must not fail");
        assert!(
            rows.len() < SEEDED,
            "{SEEDED} decisions were recorded and the view returned all of them, so nothing is \
             bounding it: {rows:#?}"
        );
        assert_eq!(
            rows.len(),
            ROUTE_DECISION_ROW_LIMIT,
            "the bound must be the shipped one: {rows:#?}"
        );
        assert_eq!(
            rows[0].rationale.as_deref(),
            Some(format!("decision {}", SEEDED - 1).as_str()),
            "newest first, whatever the bound is: {rows:#?}"
        );
    }

    /// The run loop's own arm, not just the builder: pressing `d` opens the
    /// overlay carrying what the builder read.
    ///
    /// This is the in-crate half of the dispatch proof and it is deliberately
    /// weaker than `tests/disposable_route_sink.rs`'s, which types the key at
    /// a real terminal — the arm itself lives inside `run`'s event loop and
    /// no in-crate test can reach it (the finding `tests/tui_harness.rs`
    /// exists for).
    #[test]
    fn opening_the_routing_decisions_view_shows_real_recorded_decisions() {
        let (_data, _workspace, runtime) = bootstrapped_runtime();
        {
            let ledger = EvaluationObservations::open(&runtime).unwrap();
            ledger
                .record(
                    NewObservation::new(EvaluationKind::DisposableRouteDecided)
                        .with_subject("memory extraction")
                        .with_detail("chose a-free-model"),
                    1_000,
                )
                .unwrap();
        }

        let mut state = ShellState::new("p", "/p", "0.1.0", Vec::new());
        assert_eq!(
            state.handle_key(crossterm::event::KeyEvent::new(
                crossterm::event::KeyCode::Char('d'),
                crossterm::event::KeyModifiers::NONE,
            )),
            state::Action::OpenRouteDecisions
        );

        let rows = build_route_decision_table(&runtime).expect("must not fail");
        state.open_route_decisions(rows, None);

        assert_eq!(state.overlay(), Some(state::Overlay::RouteDecisions));
        let shown = state.route_decisions().expect("open");
        assert!(
            shown
                .rows()
                .iter()
                .any(|row| row.rationale.as_deref() == Some("chose a-free-model")),
            "{:#?}",
            shown.rows()
        );
    }
}

/// Phase 47 line 1765: the route-health view reads real gateway telemetry
/// through [`build_route_health_table`] — the production function
/// `Action::OpenRouteHealth`'s handler calls.
///
/// These write through the *production* cache writers
/// (`GatewayHealthCache::store` and `GatewayQuotaCache::store`, the same two
/// calls `gateway::mod`'s accept loop makes) and read back through the
/// production builder, rather than hand-building a `RouteHealthRow`. A test
/// that constructed the row itself would leave the builder deletable without
/// anything noticing, which is practice §35's exact failure.
#[cfg(test)]
mod route_health_tests {
    use super::*;
    use crate::provider::telemetry::{
        GatewayHealthCache, GatewayHealthReading, GatewayQuotaCache, RateLimitHeaders,
    };

    /// The same bootstrap `route_evidence_tests` uses. `data_dir` is where
    /// both telemetry caches live, so pointing it at a temporary directory is
    /// what keeps these tests from reading the developer's own installation.
    fn bootstrapped_runtime() -> (tempfile::TempDir, tempfile::TempDir, crate::Runtime) {
        use clap::Parser;

        let data = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(workspace.path().join(".git")).unwrap();

        let cli = crate::Cli::try_parse_from([
            "glasshouse",
            "--data-dir",
            data.path().to_str().unwrap(),
            "--config-dir",
            data.path().to_str().unwrap(),
        ])
        .unwrap();
        let runtime = crate::bootstrap(&cli, workspace.path()).unwrap();
        (data, workspace, runtime)
    }

    fn reading(
        model: &str,
        consecutive_failures: u32,
        cooling_down_until_unix: Option<i64>,
        credential_rejected: bool,
    ) -> GatewayHealthReading {
        GatewayHealthReading {
            credential_label: "anyrouter/ANYROUTER_API_KEY".to_owned(),
            model: model.to_owned(),
            consecutive_failures,
            cooling_down_until_unix,
            cooldown_cause: None,
            credential_rejected,
        }
    }

    /// A fresh installation has observed nothing, and that is a complete
    /// answer rather than an error — the caches' own fail-soft contract,
    /// which is also why this builder returns no `Result`.
    #[test]
    fn an_installation_with_no_gateway_telemetry_yields_no_rows() {
        let (_data, _workspace, runtime) = bootstrapped_runtime();
        assert!(build_route_health_table(&runtime).is_empty());
    }

    /// The five concepts come back as five *separate* fields, read through
    /// the production caches. The fixture is deliberately one where they
    /// disagree — no failures, yet unavailable, and paced — because that is
    /// the case a single collapsed status word cannot represent.
    #[test]
    fn the_five_concepts_survive_the_process_boundary_as_separate_fields() {
        let (_data, _workspace, runtime) = bootstrapped_runtime();
        let now = crate::provider::cache::now_unix_seconds();

        GatewayHealthCache::new(runtime.paths()).store(
            "anyrouter",
            &[reading("claude-opus-4-1", 0, Some(now + 300), true)],
            now,
        );
        GatewayQuotaCache::new(runtime.paths()).store(
            "anyrouter",
            &RateLimitHeaders::read([
                ("ratelimit-limit", "300"),
                ("ratelimit-remaining", "12"),
                ("ratelimit-reset", "1800"),
            ]),
            now,
        );

        let rows = build_route_health_table(&runtime);
        assert_eq!(rows.len(), 1);
        let row = &rows[0];

        // 1. route health — a streak and a flag, both preserved.
        assert_eq!(row.consecutive_failures, 0);
        assert!(row.credential_rejected);
        // 2. immediate availability — the producer's own answer, and it
        //    disagrees with the zero failure streak above.
        assert!(
            !row.available_now,
            "a refused credential is unavailable even with no failure streak"
        );
        // 3. cadence — Glasshouse's own pacing and the provider's window,
        //    two different facts kept apart.
        assert_eq!(row.cooling_down_until_unix, Some(now + 300));
        assert_eq!(row.stated_limit, Some(300));
        assert_eq!(row.stated_window_seconds, None);
        // 4. quota reset — the provider's own clock, a different instant
        //    from the cooldown above.
        assert_eq!(row.quota_resets_at_unix, Some(now + 1_800));
        assert_ne!(
            row.quota_resets_at_unix, row.cooling_down_until_unix,
            "the provider's reset and Glasshouse's cooldown are two clocks"
        );
        // 5. failure-domain evidence — one observed resource, so nothing is
        //    known to share its domain, and never `independent`.
        assert_eq!(row.failure_domain, "unknown");
        assert_eq!(row.failure_domain_peers, 0);
    }

    /// A provider with nothing stated leaves the three provider-sourced
    /// concepts `None` — the shape the view turns into `unknown`. A default
    /// of zero here is the defect this assertion exists to catch, because it
    /// would reach the screen as a measurement.
    #[test]
    fn a_provider_that_stated_no_headers_leaves_every_stated_field_none() {
        let (_data, _workspace, runtime) = bootstrapped_runtime();
        let now = crate::provider::cache::now_unix_seconds();
        GatewayHealthCache::new(runtime.paths()).store(
            "openrouter",
            &[reading("some-free-model", 2, None, false)],
            now,
        );

        let rows = build_route_health_table(&runtime);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].stated_limit, None);
        assert_eq!(rows[0].stated_window_seconds, None);
        assert_eq!(rows[0].quota_resets_at_unix, None);
        assert_eq!(rows[0].cooling_down_until_unix, None);
        // Route health is still real: the streak crossed the boundary.
        assert_eq!(rows[0].consecutive_failures, 2);
    }

    /// Failure-domain evidence is about a *pair*, and the only signal this
    /// build has is the provider. Two resources behind one provider are
    /// `shared`; each is `unknown` with respect to the other provider, and
    /// nothing anywhere is ever `independent`.
    #[test]
    fn two_resources_on_one_provider_are_shared_and_never_independent() {
        let (_data, _workspace, runtime) = bootstrapped_runtime();
        let now = crate::provider::cache::now_unix_seconds();
        let health = GatewayHealthCache::new(runtime.paths());
        health.store(
            "anyrouter",
            &[
                reading("model-a", 0, None, false),
                reading("model-b", 1, None, false),
            ],
            now,
        );
        health.store("openrouter", &[reading("model-c", 0, None, false)], now);

        let rows = build_route_health_table(&runtime);
        assert_eq!(rows.len(), 3);
        for row in &rows {
            assert_ne!(
                row.failure_domain, "independent",
                "nothing in this build can establish independence"
            );
        }
        let anyrouter: Vec<_> = rows.iter().filter(|r| r.provider == "anyrouter").collect();
        assert_eq!(anyrouter.len(), 2);
        for row in &anyrouter {
            assert_eq!(row.failure_domain, "shared");
            assert_eq!(row.failure_domain_peers, 1);
        }
        let lone = rows
            .iter()
            .find(|r| r.provider == "openrouter")
            .expect("openrouter row");
        assert_eq!(lone.failure_domain, "unknown");
        assert_eq!(lone.failure_domain_peers, 0);
    }

    /// The `h` key reaches this builder through the real run-loop action, and
    /// the overlay carries the rows the builder actually read — not a
    /// hand-constructed fixture (practice §35).
    #[test]
    fn opening_the_route_health_view_shows_real_gateway_telemetry() {
        let (_data, _workspace, runtime) = bootstrapped_runtime();
        let now = crate::provider::cache::now_unix_seconds();
        GatewayHealthCache::new(runtime.paths()).store(
            "anyrouter",
            &[reading("claude-opus-4-1", 3, None, false)],
            now,
        );

        let mut state = state::ShellState::new(
            "glasshouse",
            runtime.project().display_root(),
            "test",
            Vec::new(),
        );
        assert_eq!(
            state.handle_key(crossterm::event::KeyEvent::from(
                crossterm::event::KeyCode::Char('h')
            )),
            state::Action::OpenRouteHealth
        );

        state.open_route_health(build_route_health_table(&runtime));

        assert_eq!(state.overlay(), Some(state::Overlay::RouteHealth));
        let health = state.route_health().expect("open");
        let row = health
            .rows()
            .iter()
            .find(|row| row.provider == "anyrouter")
            .expect("the observed resource must reach the overlay");
        assert_eq!(row.consecutive_failures, 3);
    }

    /// The isolation invariant, asserted rather than assumed: this builder
    /// opens **no project database at all**. It reads two provider-keyed
    /// cache directories under the installation's data directory, so there is
    /// no project predicate for it to get wrong — and a future edit that
    /// started reading project rows here would have to delete this test.
    #[test]
    fn the_builder_reads_no_project_scoped_store() {
        let source = include_str!("../mod.rs");
        let start = source
            .find("fn build_route_health_table(")
            .expect("the function must exist");
        // Ended at the next item at column zero, read with `str::lines` so a
        // CRLF checkout cannot defeat it (practice §14).
        let body: String = source[start..]
            .lines()
            .skip(1)
            .take_while(|line| !line.starts_with('}'))
            .collect::<Vec<_>>()
            .join("\n");
        for forbidden in [
            "MemoryStore",
            "EvidenceLedger",
            "ProjectSessions",
            "EventLog",
            "project_state_dir",
            "Connection",
        ] {
            assert!(
                !body.contains(forbidden),
                "build_route_health_table must not reach a project-scoped store, \
                 but names `{forbidden}`:\n{body}"
            );
        }
        assert!(
            body.contains("data_dir") || body.contains("GatewayHealthCache::new"),
            "the builder must read the installation-wide telemetry caches:\n{body}"
        );
    }
}

/// Phase 9A line 368's shell half: `start_session` — the TUI's `n` key — must
/// record the same six facts `main.rs::launch_session` does, not `-` for
/// every one of them.
///
/// These call [`start_session`] itself, the production function, against a
/// real [`SessionRuntime`] and a fake installed harness — the same shape
/// `tests/events_lifecycle.rs` already uses to drive `SessionRuntime` outside
/// a real terminal. A test that resolved the six facts by hand instead would
/// prove nothing about whether `start_session` actually calls the code that
/// resolves them.
#[cfg(test)]
mod native_session_facts_tests {
    use super::*;

    /// A [`Runtime`] whose config directory already names one installed,
    /// harmless harness — a shell script that exits immediately, exactly like
    /// `tests/session_model.rs`'s fake `claude-code`.
    fn runtime_with_fake_claude_code() -> (tempfile::TempDir, tempfile::TempDir, crate::Runtime) {
        use clap::Parser;

        let data = tempfile::tempdir().expect("tempdir");
        let workspace = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir_all(workspace.path().join(".git")).expect("create .git");
        let workspace_root =
            std::fs::canonicalize(workspace.path()).expect("canonicalize workspace root");

        let bin_dir = data.path().join("bin");
        std::fs::create_dir_all(&bin_dir).expect("create bin dir");
        let harness = install_fake_claude_code(&bin_dir);
        let escaped = harness.display().to_string().replace('\\', "\\\\");
        std::fs::write(
            data.path().join("config.toml"),
            format!(
                "version = 1\n\n\
                 [integrations.claude-code]\nenabled = true\nexecutable = \"{escaped}\"\n"
            ),
        )
        .expect("write user config");

        let cli = crate::Cli::try_parse_from([
            "glasshouse",
            "--data-dir",
            data.path().to_str().unwrap(),
            "--config-dir",
            data.path().to_str().unwrap(),
        ])
        .unwrap();
        let runtime = crate::bootstrap(&cli, &workspace_root).unwrap();
        (data, workspace, runtime)
    }

    #[cfg(unix)]
    fn install_fake_claude_code(bin_dir: &std::path::Path) -> std::path::PathBuf {
        use std::os::unix::fs::PermissionsExt;

        let path = bin_dir.join("fake-claude");
        std::fs::write(&path, "#!/bin/sh\nexit 0\n").expect("write fake harness");
        let mut perms = std::fs::metadata(&path).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&path, perms).unwrap();
        path
    }

    #[cfg(windows)]
    fn install_fake_claude_code(bin_dir: &std::path::Path) -> std::path::PathBuf {
        let path = bin_dir.join("fake-claude.cmd");
        std::fs::write(&path, "@echo off\r\nexit /b 0\r\n").expect("write fake harness");
        path
    }

    #[test]
    fn starting_a_session_from_the_shell_records_all_six_facts() {
        let (_data, _workspace, runtime) = runtime_with_fake_claude_code();
        let sessions = ProjectSessions::open(&runtime).expect("open project sessions");
        let mut live = SessionRuntime::new();
        let mut index_snapshots = HashMap::new();

        start_session(
            &runtime,
            &mut live,
            &sessions,
            SessionPresentation::Embedded,
            TerminalSize::new(24, 80),
            &mut index_snapshots,
        )
        .expect("starting a session from the shell must succeed");

        let records = sessions.store().list().expect("list sessions");
        assert_eq!(records.len(), 1, "exactly one session must be recorded");
        let record = &records[0];

        // Line 368's own words: "the resolved harness, backend resource,
        // model, protocol, pairing class, and response profile" — six facts,
        // and this used to record none of them.
        assert_eq!(
            record.launch_profile.as_deref(),
            Some(crate::profile::NATIVE_PROFILE_NAME),
            "the implied Native profile is still a profile, and must be named"
        );
        assert_eq!(
            record.backend_resource.as_deref(),
            Some("native"),
            "record: {record:?}"
        );
        assert!(record.model.is_some(), "record: {record:?}");
        assert!(record.pairing_class.is_some(), "record: {record:?}");
        assert!(record.protocol.is_some(), "record: {record:?}");
        assert!(record.response_profile.is_some(), "record: {record:?}");
        assert!(record.response_mechanism.is_some(), "record: {record:?}");
    }
}

/// Map line 1973's scrub, through [`start_session`] itself — the production
/// function, not `tests/entitlement_shell_scrub.rs`'s seam one layer below
/// it — proving the scrub this package adds is actually wired in here, not
/// merely declared beside it. Same shape as `native_session_facts_tests`
/// above: a real `SessionRuntime`, a fake installed harness, no terminal.
#[cfg(test)]
mod shell_entitlement_scrub_tests {
    use super::*;

    const VAR_B: &str = "GLASSHOUSE_SHELL_SCRUB_UNIT_TEST_ONLY_B";
    const VALUE_B: &str = "fake-shell-scrub-unit-b-0123456789abcdef";

    /// Like `native_session_facts_tests::runtime_with_fake_claude_code`, but
    /// the installed harness dumps its own environment instead of exiting
    /// silently, and the user config also configures one provider-backed
    /// entitlement — `claude-b`, carrying an environment-shaped credential
    /// that no native launch may serve.
    fn runtime_with_env_dumping_harness_and_an_entitlement() -> (
        tempfile::TempDir,
        tempfile::TempDir,
        crate::Runtime,
        std::path::PathBuf,
    ) {
        use clap::Parser;

        let data = tempfile::tempdir().expect("tempdir");
        let workspace = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir_all(workspace.path().join(".git")).expect("create .git");
        let workspace_root =
            std::fs::canonicalize(workspace.path()).expect("canonicalize workspace root");

        let bin_dir = data.path().join("bin");
        std::fs::create_dir_all(&bin_dir).expect("create bin dir");
        let env_log = data.path().join("env.log");
        let harness = install_env_dumping_harness(&bin_dir, &env_log);
        let escaped = harness.display().to_string().replace('\\', "\\\\");
        std::fs::write(
            data.path().join("config.toml"),
            format!(
                "version = 1\n\n\
                 [integrations.claude-code]\nenabled = true\nexecutable = \"{escaped}\"\n\n\
                 [entitlements.claude-b]\nvendor = \"claude\"\nprovider = \"beta-probe\"\n\
                 credential = {{ env = \"{VAR_B}\" }}\n"
            ),
        )
        .expect("write user config");

        let cli = crate::Cli::try_parse_from([
            "glasshouse",
            "--data-dir",
            data.path().to_str().unwrap(),
            "--config-dir",
            data.path().to_str().unwrap(),
        ])
        .unwrap();
        let runtime = crate::bootstrap(&cli, &workspace_root).unwrap();
        (data, workspace, runtime, env_log)
    }

    #[cfg(unix)]
    fn install_env_dumping_harness(
        bin_dir: &std::path::Path,
        env_log: &std::path::Path,
    ) -> std::path::PathBuf {
        use std::os::unix::fs::PermissionsExt;

        let path = bin_dir.join("fake-claude-env-dump");
        // `export -p` is a shell builtin, so the scrubbed environment this
        // fixture spawns under cannot break it.
        std::fs::write(
            &path,
            format!("#!/bin/sh\nexport -p > '{}'\nexit 0\n", env_log.display()),
        )
        .expect("write fake harness");
        let mut perms = std::fs::metadata(&path).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&path, perms).unwrap();
        path
    }

    #[cfg(windows)]
    fn install_env_dumping_harness(
        bin_dir: &std::path::Path,
        env_log: &std::path::Path,
    ) -> std::path::PathBuf {
        let path = bin_dir.join("fake-claude-env-dump.cmd");
        std::fs::write(
            &path,
            format!(
                "@echo off\r\nset > \"{}\"\r\nexit /b 0\r\n",
                env_log.display()
            ),
        )
        .expect("write fake harness");
        path
    }

    /// A configured entitlement's environment-shaped credential must not
    /// reach a session it does not serve. `start_session`'s launch is
    /// always Native, served only by the harness's own default sign-in
    /// (which carries no credential of its own) — so `claude-b`'s variable,
    /// inherited from this test process, must be scrubbed before the child
    /// spawns. This is the mutation `foreign_entitlement_credential_vars`
    /// returning an empty list, or the `env_remove` loop being dropped here,
    /// would both survive if this test did not exist.
    #[test]
    fn a_shell_started_native_session_does_not_carry_a_configured_entitlements_variable() {
        let (_data, _workspace, runtime, env_log) =
            runtime_with_env_dumping_harness_and_an_entitlement();
        let sessions = ProjectSessions::open(&runtime).expect("open project sessions");
        let mut live = SessionRuntime::new();
        let mut index_snapshots = HashMap::new();

        // SAFETY: unique to this test and removed before it can panic.
        unsafe {
            std::env::set_var(VAR_B, VALUE_B);
        }
        let start_result = start_session(
            &runtime,
            &mut live,
            &sessions,
            SessionPresentation::Embedded,
            TerminalSize::new(24, 80),
            &mut index_snapshots,
        );
        start_result.expect("starting a session from the shell must succeed");

        let id = sessions
            .store()
            .list()
            .expect("list sessions")
            .into_iter()
            .next()
            .expect("one session was recorded")
            .id;

        // `answer_terminal_queries` is in the loop because it is in the
        // production tick this test is standing in for — `shell::run` calls
        // it beside `poll_exits` a few hundred lines above. On Windows it is
        // not a nicety: ConPTY sends `ESC[6n` on the pty's own output while
        // bringing the pseudo-console up and does not let the child start
        // until something replies, and Glasshouse is the terminal for an
        // embedded session, so nothing else can. Without this call the fake
        // harness has not run a single line by the deadline, and this test
        // fails on the Windows ARM64 CI VM with "the fake harness never
        // exited" — measured, not assumed. Same reason as
        // `tests/events_lifecycle.rs`'s `drive`.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        loop {
            live.answer_terminal_queries();
            if live
                .poll_exits()
                .into_iter()
                .any(|(exited, _)| exited == id)
            {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "the fake harness never exited"
            );
            std::thread::sleep(std::time::Duration::from_millis(25));
        }
        unsafe {
            std::env::remove_var(VAR_B);
        }

        let child_env =
            std::fs::read_to_string(&env_log).expect("the fake harness dumped its environment");
        assert!(
            !child_env.contains(VAR_B) && !child_env.contains(VALUE_B),
            "a configured entitlement's credential reached a native session it does not \
             serve:\n{child_env}"
        );
    }
}
