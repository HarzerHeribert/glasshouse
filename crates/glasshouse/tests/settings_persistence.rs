//! Real-filesystem coverage for the Settings overlay's save paths.
//!
//! `shell::state`'s own tests prove the keymap and the in-memory model;
//! these prove the two functions that actually touch disk —
//! `shell::save_user_settings` and `shell::save_project_settings` — against a
//! real project directory and a real user config directory, per the six
//! invariants in `GLASSHOUSE_DESIGN_DECISIONS.md`'s "Settings" section.

use clap::Parser;

use glasshouse::config;
use glasshouse::integrations::IntegrationId;
use glasshouse::shell::{self, SettingsEdit};
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
                let _ = shell::save_project_settings(&runtime, &state.settings_edits());
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

    let path = shell::save_project_settings(&runtime, &edits).expect("save must succeed");
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
    shell::save_user_settings(&runtime, &edits).expect("save must succeed");

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
    shell::save_user_settings(&runtime, &edits).expect("save must succeed");

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
