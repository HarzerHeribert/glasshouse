//! `config::tests`, part A: loading, hooks, launch-profile and provider/secret tests.
//!

use super::*;

/// A [`crate::secret::SecretStore`] holding exactly one credential — for
/// tests here that just need a direct-provider profile to resolve,
/// rather than exercising secret resolution itself (that belongs to
/// `crate::profile`'s own tests).
struct OneShotSecrets(&'static str, &'static str);
impl crate::secret::SecretStore for OneShotSecrets {
    fn resolve(&self, reference: &crate::secret::SecretRef) -> Option<crate::secret::Secret> {
        let crate::secret::SecretRef::Environment { var } = reference else {
            return None;
        };
        (var == self.0).then(|| crate::secret::Secret::mint_for_test(self.1))
    }

    fn is_present(&self, reference: &crate::secret::SecretRef) -> bool {
        self.resolve(reference).is_some()
    }

    fn describe(&self) -> &'static str {
        "one-shot test store"
    }
}
#[test]
fn missing_file_loads_as_default() {
    let tmp = tempfile::tempdir().unwrap();
    let paths = RuntimePaths::new(tmp.path().join("data"), tmp.path().join("config"));

    let config = UserConfig::load(&paths).unwrap();
    assert_eq!(config, UserConfig::default());
    assert!(!config.onboarding().completed());
    assert!(config.integrations().is_empty());
    // Loading must not have created anything.
    assert!(!paths.user_config_file().exists());
}

#[test]
fn round_trip_save_load_preserves_every_field() {
    let tmp = tempfile::tempdir().unwrap();
    let paths = RuntimePaths::new(tmp.path().join("data"), tmp.path().join("config"));

    let original = fully_populated_user_config();
    original.save(&paths).unwrap();

    let loaded = UserConfig::load(&paths).unwrap();
    assert_eq!(loaded, original);
    assert_eq!(loaded.onboarding().completed_at_version(), Some("0.1.0"));
    assert_eq!(
        loaded
            .integrations()
            .get(IntegrationId::ClaudeCode)
            .unwrap()
            .executable(),
        Some(Path::new("/opt/claude-code/bin/claude"))
    );
    assert_eq!(
        loaded.integrations().is_enabled(IntegrationId::Codex),
        Some(false)
    );
    assert_eq!(
        loaded
            .integrations()
            .get(IntegrationId::Hermes)
            .unwrap()
            .bypass_acknowledged(),
        Some(true)
    );
    let profile = loaded.profiles().get("fast").unwrap();
    assert_eq!(profile.harness_slug(), "claude-code");
    assert_eq!(profile.approval(), ProfileApproval::AutomaticReview);
    assert_eq!(loaded.memory_extraction(), Some(false));
}

#[test]
fn a_file_written_by_a_newer_version_loads_but_refuses_to_save() {
    let tmp = tempfile::tempdir().unwrap();
    let paths = RuntimePaths::new(tmp.path().join("data"), tmp.path().join("config"));
    std::fs::create_dir_all(paths.config_dir()).unwrap();
    std::fs::write(
        paths.user_config_file(),
        "version = 999\n\n[onboarding]\ncompleted = true\n",
    )
    .unwrap();

    let loaded = UserConfig::load(&paths).unwrap();
    assert_eq!(loaded.version(), 999);
    assert!(loaded.onboarding().completed());

    let err = loaded.save(&paths).unwrap_err();
    assert!(matches!(
        err,
        ConfigError::UnsupportedVersion {
            found: 999,
            supported: CURRENT_SCHEMA_VERSION,
            ..
        }
    ));
    let msg = err.to_string();
    assert!(msg.contains("newer version"), "{msg}");

    // The file on disk must be untouched by the failed save.
    let raw = std::fs::read_to_string(paths.user_config_file()).unwrap();
    assert!(raw.contains("999"));
}

#[test]
fn unknown_keys_and_fields_do_not_break_parsing() {
    let tmp = tempfile::tempdir().unwrap();
    let paths = RuntimePaths::new(tmp.path().join("data"), tmp.path().join("config"));
    std::fs::create_dir_all(paths.config_dir()).unwrap();
    std::fs::write(
        paths.user_config_file(),
        r#"
            version = 1
            some_future_top_level_key = "ignored"

            [onboarding]
            completed = true
            completed_at_version = "9.9.9"
            some_future_onboarding_field = 42

            [integrations.claude-code]
            enabled = true
            some_future_integration_field = true

            [integrations.a-future-harness-this-build-does-not-know]
            enabled = true
        "#,
    )
    .unwrap();

    let config = UserConfig::load(&paths).unwrap();
    assert!(config.onboarding().completed());
    assert_eq!(
        config.integrations().is_enabled(IntegrationId::ClaudeCode),
        Some(true)
    );
    // The unrecognized slug round-trips through the map even though no
    // `IntegrationId` variant names it.
    assert_eq!(
        config
            .integrations()
            .iter()
            .find(|(slug, _)| *slug == "a-future-harness-this-build-does-not-know")
            .map(|(_, cfg)| cfg.enabled()),
        Some(Some(true))
    );
}

#[test]
fn missing_version_field_defaults_to_current_schema_version() {
    let tmp = tempfile::tempdir().unwrap();
    let paths = RuntimePaths::new(tmp.path().join("data"), tmp.path().join("config"));
    std::fs::create_dir_all(paths.config_dir()).unwrap();
    std::fs::write(paths.user_config_file(), "[onboarding]\ncompleted = true\n").unwrap();

    let config = UserConfig::load(&paths).unwrap();
    assert_eq!(config.version(), CURRENT_SCHEMA_VERSION);
}

#[test]
fn malformed_toml_is_an_error_naming_the_path_and_does_not_overwrite() {
    let tmp = tempfile::tempdir().unwrap();
    let paths = RuntimePaths::new(tmp.path().join("data"), tmp.path().join("config"));
    std::fs::create_dir_all(paths.config_dir()).unwrap();
    let broken = "version = 1\n[onboarding\ncompleted = true\n";
    std::fs::write(paths.user_config_file(), broken).unwrap();

    let err = UserConfig::load(&paths).unwrap_err();
    assert!(matches!(err, ConfigError::Parse { .. }));
    let msg = err.to_string();
    assert!(
        msg.contains(&paths.user_config_file().display().to_string()),
        "{msg}"
    );

    // Nothing must have touched the file: same content, no temp files.
    let raw = std::fs::read_to_string(paths.user_config_file()).unwrap();
    assert_eq!(raw, broken);
    let entries: Vec<_> = std::fs::read_dir(paths.config_dir())
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    assert_eq!(entries, vec![std::ffi::OsString::from("config.toml")]);
}

#[test]
fn atomic_save_leaves_no_temporary_file_behind() {
    let tmp = tempfile::tempdir().unwrap();
    let paths = RuntimePaths::new(tmp.path().join("data"), tmp.path().join("config"));

    fully_populated_user_config().save(&paths).unwrap();

    let entries: Vec<_> = std::fs::read_dir(paths.config_dir())
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(entries, vec!["config.toml".to_owned()], "{entries:?}");
}

#[cfg(unix)]
#[test]
fn unix_permissions_are_owner_only() {
    use std::os::unix::fs::PermissionsExt;

    let tmp = tempfile::tempdir().unwrap();
    let paths = RuntimePaths::new(tmp.path().join("data"), tmp.path().join("config"));
    UserConfig::default().save(&paths).unwrap();

    let dir_mode = std::fs::metadata(paths.config_dir())
        .unwrap()
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(dir_mode, 0o700, "config dir mode was {dir_mode:o}");

    let file_mode = std::fs::metadata(paths.user_config_file())
        .unwrap()
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(file_mode, 0o600, "config file mode was {file_mode:o}");
}

#[test]
fn tri_state_enabled_distinguishes_never_asked_from_a_decision() {
    let mut config = UserConfig::default();
    assert_eq!(
        config.integrations().is_enabled(IntegrationId::ClaudeCode),
        None,
        "never asked"
    );
    assert!(
        config
            .integrations()
            .is_enabled_or_default(IntegrationId::ClaudeCode, true)
    );
    assert!(
        !config
            .integrations()
            .is_enabled_or_default(IntegrationId::ClaudeCode, false)
    );

    config
        .integrations_mut()
        .entry(IntegrationId::ClaudeCode)
        .set_enabled(false);
    assert_eq!(
        config.integrations().is_enabled(IntegrationId::ClaudeCode),
        Some(false),
        "explicitly declined"
    );
    assert!(
        !config
            .integrations()
            .is_enabled_or_default(IntegrationId::ClaudeCode, true)
    );

    config
        .integrations_mut()
        .entry(IntegrationId::ClaudeCode)
        .set_enabled(true);
    assert_eq!(
        config.integrations().is_enabled(IntegrationId::ClaudeCode),
        Some(true),
        "explicitly accepted"
    );
    assert!(
        config
            .integrations()
            .is_enabled_or_default(IntegrationId::ClaudeCode, false)
    );
}

/// Box 1800: cmux may be disabled even when it is detected. This module
/// has no concept of "detected" at all — that lives in `integrations::`,
/// which is exactly why an explicit decision here is immune to it: the
/// same generic tri-state `enabled` this file gives every integration
/// (see [`tri_state_enabled_distinguishes_never_asked_from_a_decision`])
/// applies to [`IntegrationId::Cmux`] with no special case, and
/// `onboarding::state::build_rows` reads this exact field to seed the
/// wizard's cmux row regardless of what live detection found.
#[test]
fn cmux_can_be_explicitly_disabled_and_the_decision_is_ordinary_configuration() {
    let mut config = UserConfig::default();
    assert_eq!(
        config.integrations().is_enabled(IntegrationId::Cmux),
        None,
        "never asked, whether or not cmux is present on this machine"
    );

    config
        .integrations_mut()
        .entry(IntegrationId::Cmux)
        .set_enabled(false);

    // Nothing in configuration ever consults "is cmux detected" — the
    // decision persists exactly like any other integration's, and a
    // caller must never fall back to treating detection as an override.
    assert_eq!(
        config.integrations().is_enabled(IntegrationId::Cmux),
        Some(false)
    );
    assert!(
        !config
            .integrations()
            .is_enabled_or_default(IntegrationId::Cmux, true),
        "an explicit disable must win over any default, including one a detector would suggest"
    );

    let tmp = tempfile::tempdir().unwrap();
    let paths = RuntimePaths::new(tmp.path().join("data"), tmp.path().join("config"));
    config.save(&paths).unwrap();
    let loaded = UserConfig::load(&paths).unwrap();
    assert_eq!(
        loaded.integrations().is_enabled(IntegrationId::Cmux),
        Some(false),
        "the disable survives a save and load, so a later run still honours it"
    );
}

#[test]
fn tri_state_project_hooks_consent_distinguishes_never_asked_from_a_decision() {
    let mut config = UserConfig::default();
    assert_eq!(
        config.integrations().get(IntegrationId::Codex),
        None,
        "never asked"
    );

    config
        .integrations_mut()
        .entry(IntegrationId::Codex)
        .set_project_hooks(false);
    assert_eq!(
        config
            .integrations()
            .get(IntegrationId::Codex)
            .unwrap()
            .project_hooks(),
        Some(false),
        "explicitly declined"
    );

    config
        .integrations_mut()
        .entry(IntegrationId::Codex)
        .set_project_hooks(true);
    assert_eq!(
        config
            .integrations()
            .get(IntegrationId::Codex)
            .unwrap()
            .project_hooks(),
        Some(true),
        "explicitly consented"
    );

    // Recording a decision about `enabled` must not silently record one
    // about `project_hooks` too — the whole reason this is a second
    // `Option<bool>` field rather than folded into `enabled`.
    let mut only_enabled = UserConfig::default();
    only_enabled
        .integrations_mut()
        .entry(IntegrationId::Codex)
        .set_enabled(true);
    assert_eq!(
        only_enabled
            .integrations()
            .get(IntegrationId::Codex)
            .unwrap()
            .project_hooks(),
        None
    );
}

#[test]
fn effective_config_defaults_project_hooks_consent_to_withheld() {
    // Absent consent must resolve to `false`, never `true` — a session
    // with no recorded decision must run without project-local hooks.
    let user = UserConfig::default();
    let effective = EffectiveConfig::new(&user, None);
    let consent = effective.project_hooks(IntegrationId::Codex);
    assert!(!consent.value);
    assert_eq!(consent.layer, Layer::Default);
}

#[test]
fn effective_config_project_hooks_consent_layers_like_enabled() {
    let mut user = UserConfig::default();
    user.integrations_mut()
        .entry(IntegrationId::Codex)
        .set_project_hooks(true);

    let mut project = ProjectConfig::default();
    project
        .integrations_mut()
        .entry(IntegrationId::Codex)
        .set_project_hooks(false);

    let effective = EffectiveConfig::new(&user, Some(&project));
    let consent = effective.project_hooks(IntegrationId::Codex);
    assert!(!consent.value, "the project layer withdraws consent");
    assert_eq!(consent.layer, Layer::Project);

    let effective_without_project = EffectiveConfig::new(&user, None);
    let consent = effective_without_project.project_hooks(IntegrationId::Codex);
    assert!(consent.value, "the user layer's consent still applies");
    assert_eq!(consent.layer, Layer::User);
}

#[test]
fn effective_config_defaults_bypass_acknowledgement_to_withheld() {
    let user = UserConfig::default();
    let effective = EffectiveConfig::new(&user, None);
    let acknowledged = effective.bypass_acknowledged(IntegrationId::Hermes);
    assert!(!acknowledged.value);
    assert_eq!(acknowledged.layer, Layer::Default);
}

/// Phase 9A: "Keep native-subscription profiles available even when
/// gateway providers are configured."
///
/// The Native profile is implied rather than stored, so no amount of
/// configuration in either layer can displace it. This is the test that
/// fails if someone ever "unifies" the lookup by moving Native into the
/// table alongside everything else.
/// Phase 9H line 518, the storage half: a pin recorded in configuration
/// reaches the launch profile that applies it, and survives a save and a
/// load.
///
/// **This test exists because a mutation survived without it.** Replacing
/// `to_launch_profile`'s `pin_gateway_backend` with a hard-coded `false`
/// broke nothing: the profile-side test that proves a pin turns failover
/// off builds its `LaunchProfile` by hand, so the one hop between stored
/// configuration and the value `apply_gateway` reads was uncovered.
#[test]
fn a_pin_recorded_in_configuration_reaches_the_launch_profile_and_round_trips() {
    let mut stored = ProfileConfig::new(IntegrationId::ClaudeCode);
    stored.set_backend(ProfileBackend::GlasshouseGateway);
    assert!(
        !stored.pin_gateway_backend(),
        "a profile nobody pinned is not pinned"
    );
    stored.set_pin_gateway_backend(true);

    let profile = stored
        .to_launch_profile("pinned")
        .expect("a known harness and backend");
    assert!(
        profile.pin_gateway_backend,
        "the stored pin must reach the value `apply_gateway` reads"
    );

    // And a file written before the field existed loads as not pinned,
    // which is the behaviour those files already had.
    let toml = toml::to_string(&stored).expect("serializable");
    assert!(toml.contains("pin_gateway_backend"), "{toml}");
    let reloaded: ProfileConfig = toml::from_str(&toml).expect("round-trips");
    assert!(reloaded.pin_gateway_backend());

    let legacy: ProfileConfig =
        toml::from_str("harness = \"claude-code\"").expect("a file without the field loads");
    assert!(!legacy.pin_gateway_backend());
    let legacy_toml = toml::to_string(&legacy).expect("serializable");
    assert!(
        !legacy_toml.contains("pin_gateway_backend"),
        "an unpinned profile writes exactly what it wrote before: {legacy_toml}"
    );
}

#[test]
fn a_configured_gateway_profile_never_displaces_the_native_one() {
    let mut user = UserConfig::default();
    let mut gateway = ProfileConfig::new(IntegrationId::ClaudeCode);
    gateway.set_backend(ProfileBackend::DirectProvider {
        provider: "openrouter".to_owned(),
    });
    user.profiles_mut().set("gateway", gateway);

    let mut project = ProjectConfig::default();
    let mut local = ProfileConfig::new(IntegrationId::Codex);
    local.set_backend(ProfileBackend::GlasshouseGateway);
    project.profiles_mut().set("local", local);

    let effective = EffectiveConfig::new(&user, Some(&project));

    let names = effective.profile_names();
    assert!(
        names
            .iter()
            .any(|n| n == crate::profile::NATIVE_PROFILE_NAME),
        "the native profile must survive every configured profile: {names:?}"
    );
    assert!(names.iter().any(|n| n == "gateway"), "{names:?}");
    assert!(names.iter().any(|n| n == "local"), "{names:?}");

    // And it still resolves for a harness that has a gateway profile of
    // its own configured — the case where a lookup that consulted the
    // table first would go wrong.
    let native = effective
        .launch_profile(
            crate::profile::NATIVE_PROFILE_NAME,
            IntegrationId::ClaudeCode,
        )
        .expect("the native profile is available for every harness");
    assert!(matches!(
        native.value.backend,
        crate::profile::BackendResource::Native
    ));
}

#[test]
fn a_project_layer_cannot_acknowledge_a_bypass() {
    // Unlike every other lookup on `EffectiveConfig`, a project-level
    // acknowledgement must have no effect at all: acknowledging a
    // blanket bypass is a statement by a person about a harness on their
    // own machine, and a repository cannot make that statement on behalf
    // of whoever cloned it.
    let mut project = ProjectConfig::default();
    project
        .integrations_mut()
        .entry(IntegrationId::Hermes)
        .set_bypass_acknowledged(true);

    let user = UserConfig::default();
    let effective = EffectiveConfig::new(&user, Some(&project));
    let acknowledged = effective.bypass_acknowledged(IntegrationId::Hermes);
    assert!(
        !acknowledged.value,
        "a project-level acknowledgement must not count"
    );
    assert_eq!(acknowledged.layer, Layer::Default);

    // The user layer's own acknowledgement still applies, and still only
    // for the harness it named.
    let mut user_with_ack = UserConfig::default();
    user_with_ack
        .integrations_mut()
        .entry(IntegrationId::Hermes)
        .set_bypass_acknowledged(true);
    let effective = EffectiveConfig::new(&user_with_ack, Some(&project));
    let acknowledged = effective.bypass_acknowledged(IntegrationId::Hermes);
    assert!(acknowledged.value);
    assert_eq!(acknowledged.layer, Layer::User);

    let other = effective.bypass_acknowledged(IntegrationId::Antigravity);
    assert!(
        !other.value,
        "acknowledging Hermes must not acknowledge Antigravity"
    );
    assert_eq!(other.layer, Layer::Default);
}

#[test]
fn project_config_layering_reports_the_correct_source_layer() {
    let mut user = UserConfig::default();
    user.integrations_mut()
        .entry(IntegrationId::ClaudeCode)
        .set_enabled(true);
    user.integrations_mut()
        .entry(IntegrationId::Codex)
        .set_enabled(true)
        .set_executable(Some(PathBuf::from("/usr/local/bin/codex")));

    let mut project = ProjectConfig::default();
    // Project explicitly disables what the user enabled: project wins.
    project
        .integrations_mut()
        .entry(IntegrationId::ClaudeCode)
        .set_enabled(false);

    let effective = EffectiveConfig::new(&user, Some(&project));

    // Case 1: project overrides user.
    let claude = effective.enabled(IntegrationId::ClaudeCode, true);
    assert!(!claude.value);
    assert_eq!(claude.layer, Layer::Project);

    // Case 2: only user has a decision.
    let codex = effective.enabled(IntegrationId::Codex, false);
    assert!(codex.value);
    assert_eq!(codex.layer, Layer::User);
    let codex_exe = effective.executable(IntegrationId::Codex).unwrap();
    assert_eq!(codex_exe.value, PathBuf::from("/usr/local/bin/codex"));
    assert_eq!(codex_exe.layer, Layer::User);

    // Case 3: neither layer has a decision, so the caller default wins.
    let ollama = effective.enabled(IntegrationId::Ollama, true);
    assert!(ollama.value);
    assert_eq!(ollama.layer, Layer::Default);
    assert!(effective.executable(IntegrationId::Ollama).is_none());
}

#[test]
fn effective_config_without_a_project_file_falls_back_to_user_then_default() {
    let mut user = UserConfig::default();
    user.integrations_mut()
        .entry(IntegrationId::ClaudeCode)
        .set_enabled(true);

    let effective = EffectiveConfig::new(&user, None);
    let claude = effective.enabled(IntegrationId::ClaudeCode, false);
    assert!(claude.value);
    assert_eq!(claude.layer, Layer::User);

    let codex = effective.enabled(IntegrationId::Codex, false);
    assert!(!codex.value);
    assert_eq!(codex.layer, Layer::Default);
}

#[test]
fn project_config_is_never_created_automatically() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("project");
    std::fs::create_dir_all(&root).unwrap();
    let project = test_project(&root);

    let loaded = load_project_config(&project).unwrap();
    assert!(loaded.is_none());
    assert!(!root.join(".glasshouse").exists());
}

#[test]
fn project_config_round_trips_and_requires_the_consent_named_call() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("project");
    std::fs::create_dir_all(&root).unwrap();
    let project = test_project(&root);

    let mut config = ProjectConfig::default();
    config
        .integrations_mut()
        .entry(IntegrationId::ClaudeCode)
        .set_enabled(true)
        .set_executable(Some(PathBuf::from("./vendored/claude")));

    write_project_config_with_consent(&project, &config).unwrap();

    assert!(root.join(".glasshouse/config.toml").is_file());
    let loaded = load_project_config(&project).unwrap().unwrap();
    assert_eq!(loaded, config);
}

// The relative path this module resolves (`.glasshouse/config.toml`) is a
// fixed constant, not caller-controlled input, so there is no untrusted
// string that could ever literally spell its way outside the project
// root. The one honest way to make `ProjectScope::resolve` actually
// reject it is the scenario its own doc comment names: a symlink planted
// at (or under) `.glasshouse` that resolves outside the root. A raw
// `root.join(".glasshouse/config.toml")` would happily write through
// such a symlink; going through the scope guard must not.
//
// Symlinks are POSIX-only in this test; `std::os::windows::fs::symlink_dir`
// requires a privilege this sandbox does not reliably have, and the
// `resolve` codepath under test is exercised identically on every
// platform (see `crate::project::scope`'s own cross-platform tests), so
// one platform is enough to prove this module wires it up correctly.
#[cfg(unix)]
#[test]
fn project_config_path_is_resolved_through_the_project_scope() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("project");
    let outside = tmp.path().join("outside");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::create_dir_all(&outside).unwrap();
    // `.glasshouse` itself is a symlink escaping the project root.
    std::os::unix::fs::symlink(&outside, root.join(".glasshouse")).unwrap();
    let project = test_project(&root);

    let err = load_project_config(&project).unwrap_err();
    assert!(matches!(err, ConfigError::Scope(_)), "{err:?}");

    let err = write_project_config_with_consent(&project, &ProjectConfig::default()).unwrap_err();
    assert!(matches!(err, ConfigError::Scope(_)), "{err:?}");
    // And critically: the write must not have gone through to the
    // symlink target either.
    assert!(!outside.join("config.toml").exists());
}

#[test]
fn project_executable_only_override_falls_through_to_user_enabled_decision() {
    let mut user = UserConfig::default();
    user.integrations_mut()
        .entry(IntegrationId::ClaudeCode)
        .set_enabled(true);

    let mut project = ProjectConfig::default();
    project
        .integrations_mut()
        .entry(IntegrationId::ClaudeCode)
        .set_executable(Some(PathBuf::from("/opt/bin/claude")));

    let effective = EffectiveConfig::new(&user, Some(&project));

    let enabled = effective.enabled(IntegrationId::ClaudeCode, true);
    assert!(enabled.value);
    assert_eq!(enabled.layer, Layer::User);

    let executable = effective.executable(IntegrationId::ClaudeCode).unwrap();
    assert_eq!(executable.value, PathBuf::from("/opt/bin/claude"));
    assert_eq!(executable.layer, Layer::Project);
}

#[test]
fn explicit_project_disable_still_wins_over_user_enable() {
    let mut user = UserConfig::default();
    user.integrations_mut()
        .entry(IntegrationId::ClaudeCode)
        .set_enabled(true);

    let mut project = ProjectConfig::default();
    project
        .integrations_mut()
        .entry(IntegrationId::ClaudeCode)
        .set_enabled(false);

    let effective = EffectiveConfig::new(&user, Some(&project));
    let enabled = effective.enabled(IntegrationId::ClaudeCode, true);
    assert!(!enabled.value);
    assert_eq!(enabled.layer, Layer::Project);
}

#[test]
fn enabled_key_parses_to_some_and_its_absence_parses_to_none() {
    let enabled_true: IntegrationConfig =
        toml::from_str("enabled = true\nexecutable = \"/x/y\"").unwrap();
    assert_eq!(enabled_true.enabled(), Some(true));

    let explicit_false: ProjectConfig = toml::from_str(
        r#"
            [integrations.claude-code]
            enabled = false
        "#,
    )
    .unwrap();
    assert_eq!(
        explicit_false
            .integrations()
            .is_enabled(IntegrationId::ClaudeCode),
        Some(false)
    );

    let omitted: ProjectConfig = toml::from_str(
        r#"
            [integrations.claude-code]
            executable = "/opt/bin/claude"
        "#,
    )
    .unwrap();
    assert_eq!(
        omitted
            .integrations()
            .get(IntegrationId::ClaudeCode)
            .unwrap()
            .enabled(),
        None
    );
    assert_eq!(
        omitted.integrations().is_enabled(IntegrationId::ClaudeCode),
        None,
        "an entry without a recorded decision is None, not Some(false)"
    );
}

#[test]
fn serializing_no_decision_omits_the_enabled_key() {
    let no_decision = IntegrationConfig {
        enabled: None,
        executable: Some(PathBuf::from("/opt/bin/claude")),
        project_hooks: None,
        bypass_acknowledged: None,
    };
    let toml_text = toml::to_string_pretty(&no_decision).unwrap();
    assert!(
        !toml_text.contains("enabled"),
        "no-decision entry must not serialize an `enabled` key:\n{toml_text}"
    );
    assert!(
        !toml_text.contains("project_hooks"),
        "no-decision entry must not serialize a `project_hooks` key:\n{toml_text}"
    );
    assert!(
        !toml_text.contains("bypass_acknowledged"),
        "no-decision entry must not serialize a `bypass_acknowledged` key:\n{toml_text}"
    );

    let explicit_false = IntegrationConfig {
        enabled: Some(false),
        executable: None,
        project_hooks: None,
        bypass_acknowledged: None,
    };
    let toml_text = toml::to_string_pretty(&explicit_false).unwrap();
    assert!(
        toml_text.contains("enabled = false"),
        "explicit disable must serialize `enabled = false`:\n{toml_text}"
    );
}

#[test]
fn enabled_or_returns_recorded_decision_or_supplied_default() {
    let decided = IntegrationConfig {
        enabled: Some(true),
        executable: None,
        project_hooks: None,
        bypass_acknowledged: None,
    };
    assert!(decided.enabled_or(false));

    let declined = IntegrationConfig {
        enabled: Some(false),
        executable: None,
        project_hooks: None,
        bypass_acknowledged: None,
    };
    assert!(!declined.enabled_or(true));

    let undecided = IntegrationConfig::default();
    assert!(undecided.enabled_or(true));
    assert!(!undecided.enabled_or(false));
}

// ---------------------------------------------------------------
// Launch profiles.
// ---------------------------------------------------------------

#[test]
fn the_native_profile_is_always_available_for_every_harness() {
    let user = UserConfig::default();
    let effective = EffectiveConfig::new(&user, None);

    assert!(
        effective
            .profile_names()
            .contains(&crate::profile::NATIVE_PROFILE_NAME.to_owned())
    );

    let resolved = effective
        .launch_profile(crate::profile::NATIVE_PROFILE_NAME, IntegrationId::Codex)
        .unwrap();
    assert_eq!(resolved.layer, Layer::Default);
    assert_eq!(resolved.value.harness, IntegrationId::Codex);
    assert_eq!(
        resolved.value.backend,
        crate::profile::BackendResource::Native
    );
}

/// Phase 2D: "disable is not delete" for launch profiles too — disabling
/// keeps every other field intact and is reversible without retyping.
/// Both halves are asserted.
#[test]
fn disabling_a_launch_profile_keeps_its_configuration_and_is_reversible() {
    let tmp = tempfile::tempdir().unwrap();
    let paths = RuntimePaths::new(tmp.path().join("data"), tmp.path().join("config"));

    assert!(
        ProfileConfig::new(IntegrationId::ClaudeCode).enabled(),
        "a freshly created profile is enabled by default"
    );

    let mut profile = ProfileConfig::new(IntegrationId::ClaudeCode);
    profile.set_model(Some("claude-opus".to_owned()));

    let mut user = UserConfig::default();
    let mut disabled = profile.clone();
    disabled.set_enabled(false);
    user.profiles_mut().set("fast", disabled);
    user.save(&paths).unwrap();

    let loaded = UserConfig::load(&paths).unwrap();
    let loaded_profile = loaded.profiles().get("fast").unwrap();
    assert!(!loaded_profile.enabled(), "the profile must be disabled");
    assert_eq!(
        loaded_profile.model(),
        Some("claude-opus"),
        "disabling must not touch the model"
    );
    assert_eq!(loaded_profile.harness_slug(), "claude-code");

    let mut re_enabled = loaded_profile.clone();
    re_enabled.set_enabled(true);
    let mut user = loaded;
    user.profiles_mut().set("fast", re_enabled);
    user.save(&paths).unwrap();
    let reloaded = UserConfig::load(&paths).unwrap();
    let reloaded_profile = reloaded.profiles().get("fast").unwrap();
    assert!(reloaded_profile.enabled());
    assert_eq!(
        reloaded_profile.model(),
        Some("claude-opus"),
        "re-enabling must not have required retyping the model"
    );
}

#[test]
fn an_unknown_profile_name_lists_the_known_names() {
    let user = UserConfig::default();
    let effective = EffectiveConfig::new(&user, None);

    let err = effective
        .launch_profile("does-not-exist", IntegrationId::ClaudeCode)
        .unwrap_err();
    match err {
        ProfileLookupError::Unknown { name, known } => {
            assert_eq!(name, "does-not-exist");
            assert!(known.contains(&crate::profile::NATIVE_PROFILE_NAME.to_owned()));
        }
        other => panic!("expected Unknown, got {other:?}"),
    }
}

#[test]
fn a_project_configured_profile_wins_over_a_user_configured_one_of_the_same_name() {
    let mut user = UserConfig::default();
    user.profiles_mut()
        .set("fast", ProfileConfig::new(IntegrationId::ClaudeCode));

    let mut project = ProjectConfig::default();
    let mut project_profile = ProfileConfig::new(IntegrationId::ClaudeCode);
    project_profile.set_approval(ProfileApproval::AutomaticReview);
    project.profiles_mut().set("fast", project_profile);

    let effective = EffectiveConfig::new(&user, Some(&project));
    let resolved = effective
        .launch_profile("fast", IntegrationId::ClaudeCode)
        .unwrap();
    assert_eq!(resolved.layer, Layer::Project);
    assert_eq!(
        resolved.value.approval,
        crate::profile::ApprovalSelection::AutomaticReview
    );

    let without_project = EffectiveConfig::new(&user, None);
    let resolved = without_project
        .launch_profile("fast", IntegrationId::ClaudeCode)
        .unwrap();
    assert_eq!(resolved.layer, Layer::User);
    assert_eq!(
        resolved.value.approval,
        crate::profile::ApprovalSelection::Default
    );
}

#[test]
fn a_profile_naming_a_different_harness_than_requested_is_refused() {
    let mut user = UserConfig::default();
    user.profiles_mut()
        .set("fast", ProfileConfig::new(IntegrationId::ClaudeCode));
    let effective = EffectiveConfig::new(&user, None);

    let err = effective
        .launch_profile("fast", IntegrationId::Codex)
        .unwrap_err();
    match err {
        ProfileLookupError::HarnessMismatch {
            name,
            profile_harness,
            requested_harness,
        } => {
            assert_eq!(name, "fast");
            assert_eq!(profile_harness, IntegrationId::ClaudeCode);
            assert_eq!(requested_harness, IntegrationId::Codex);
        }
        other => panic!("expected HarnessMismatch, got {other:?}"),
    }
}

#[test]
fn a_profile_naming_an_unknown_harness_slug_is_reported_rather_than_guessed() {
    let mut user = UserConfig::default();
    let mut profile = ProfileConfig::new(IntegrationId::ClaudeCode);
    profile.harness = "not-a-real-harness".to_owned();
    user.profiles_mut().set("broken", profile);
    let effective = EffectiveConfig::new(&user, None);

    let err = effective
        .launch_profile("broken", IntegrationId::ClaudeCode)
        .unwrap_err();
    assert!(matches!(
        err,
        ProfileLookupError::Invalid(ProfileConfigError::UnknownHarness { .. })
    ));
}

// ---------------------------------------------------------------
// Providers.
// ---------------------------------------------------------------

#[test]
fn a_configured_provider_may_override_a_template_base_url() {
    let mut user = UserConfig::default();
    let mut provider = ProviderConfig::new("openrouter");
    provider.set_base_url(Some("https://mirror.example.com/v1".to_owned()));
    user.providers_mut().set("my-openrouter", provider);

    let effective = EffectiveConfig::new(&user, None);
    let resolved = effective.configured_provider("my-openrouter").unwrap();
    assert_eq!(resolved.layer, Layer::User);

    let protocol = resolved
        .value
        .serves(crate::harness::WireProtocol::OpenAiChat)
        .expect("openrouter serves openai-chat");
    assert_eq!(protocol.base_url, "https://mirror.example.com/v1");

    // The unconfigured template still has its own base URL — the
    // override is per configured provider, not global to the template.
    let template = crate::provider::template("openrouter").unwrap();
    let template_protocol = template
        .serves(crate::harness::WireProtocol::OpenAiChat)
        .unwrap();
    assert_eq!(template_protocol.base_url, "https://openrouter.ai/api/v1");
}

#[test]
fn a_configured_provider_without_a_base_url_override_keeps_the_templates_own() {
    let mut user = UserConfig::default();
    user.providers_mut()
        .set("plain-openrouter", ProviderConfig::new("openrouter"));

    let effective = EffectiveConfig::new(&user, None);
    let resolved = effective.configured_provider("plain-openrouter").unwrap();
    let protocol = resolved
        .value
        .serves(crate::harness::WireProtocol::OpenAiChat)
        .unwrap();
    assert_eq!(protocol.base_url, "https://openrouter.ai/api/v1");
    // And the template's own default credential name is kept too, since
    // this configuration declared no override.
    assert_eq!(resolved.value.credential_env, vec!["OPENROUTER_API_KEY"]);
}

#[test]
fn a_provider_may_declare_several_credential_variable_names() {
    let mut user = UserConfig::default();
    let mut provider = ProviderConfig::new("openrouter");
    provider.set_credential_env(vec![
        "OPENROUTER_API_KEY".to_owned(),
        "OPENROUTER_API_KEY_BACKUP".to_owned(),
    ]);
    user.providers_mut().set("multi-key", provider);

    let effective = EffectiveConfig::new(&user, None);
    let resolved = effective.configured_provider("multi-key").unwrap();
    assert_eq!(
        resolved.value.credential_env,
        vec!["OPENROUTER_API_KEY", "OPENROUTER_API_KEY_BACKUP"]
    );
}

#[test]
fn a_provider_naming_an_unknown_template_is_reported_rather_than_guessed() {
    let mut user = UserConfig::default();
    user.providers_mut()
        .set("broken", ProviderConfig::new("not-a-real-template"));
    let effective = EffectiveConfig::new(&user, None);

    let err = effective.configured_provider("broken").unwrap_err();
    assert!(matches!(
        err,
        ProviderLookupError::Invalid(ProviderConfigError::UnknownTemplate { .. })
    ));
}

#[test]
fn an_unknown_provider_name_lists_the_known_names() {
    let mut user = UserConfig::default();
    user.providers_mut()
        .set("configured-one", ProviderConfig::new("openrouter"));
    let effective = EffectiveConfig::new(&user, None);

    let err = effective.configured_provider("does-not-exist").unwrap_err();
    match err {
        ProviderLookupError::Unknown { name, known } => {
            assert_eq!(name, "does-not-exist");
            assert_eq!(known, vec!["configured-one".to_owned()]);
        }
        other => panic!("expected Unknown, got {other:?}"),
    }
}

#[test]
fn a_project_configured_provider_wins_over_a_user_configured_one_of_the_same_name() {
    let mut user = UserConfig::default();
    user.providers_mut()
        .set("router", ProviderConfig::new("openrouter"));

    let mut project = ProjectConfig::default();
    let mut project_provider = ProviderConfig::new("openrouter");
    project_provider.set_base_url(Some("https://project-mirror.example.com/v1".to_owned()));
    project.providers_mut().set("router", project_provider);

    let effective = EffectiveConfig::new(&user, Some(&project));
    let resolved = effective.configured_provider("router").unwrap();
    assert_eq!(resolved.layer, Layer::Project);
    let protocol = resolved
        .value
        .serves(crate::harness::WireProtocol::OpenAiChat)
        .unwrap();
    assert_eq!(protocol.base_url, "https://project-mirror.example.com/v1");

    let without_project = EffectiveConfig::new(&user, None);
    let resolved = without_project.configured_provider("router").unwrap();
    assert_eq!(resolved.layer, Layer::User);
}

#[test]
fn provider_table_round_trips_through_save_load() {
    let tmp = tempfile::tempdir().unwrap();
    let paths = RuntimePaths::new(tmp.path().join("data"), tmp.path().join("config"));

    let mut user = UserConfig::default();
    let mut provider = ProviderConfig::new("zai");
    provider
        .set_base_url(Some("https://mirror.example.com/paas/v4".to_owned()))
        .set_credential_env(vec!["ZAI_API_KEY".to_owned(), "ZAI_API_KEY_2".to_owned()])
        .set_headers(vec![("X-Org-Id".to_owned(), "acme".to_owned())]);
    user.providers_mut().set("my-zai", provider);
    user.save(&paths).unwrap();

    let loaded = UserConfig::load(&paths).unwrap();
    assert_eq!(loaded, user);
    let loaded_provider = loaded.providers().get("my-zai").unwrap();
    assert_eq!(loaded_provider.template(), "zai");
    assert_eq!(
        loaded_provider.base_url(),
        Some("https://mirror.example.com/paas/v4")
    );
    assert_eq!(
        loaded_provider.credential_env(),
        &["ZAI_API_KEY".to_owned(), "ZAI_API_KEY_2".to_owned()]
    );
    assert_eq!(
        loaded_provider.headers(),
        &[("X-Org-Id".to_owned(), "acme".to_owned())]
    );
}

/// Phase 2D: "disable is not delete" — disabling a provider keeps every
/// other field intact and is reversible without retyping anything, and
/// the decision survives a save/load round trip. Both halves are
/// asserted: the disabled state, and that nothing else moved.
#[test]
fn disabling_a_provider_keeps_its_configuration_and_is_reversible() {
    let tmp = tempfile::tempdir().unwrap();
    let paths = RuntimePaths::new(tmp.path().join("data"), tmp.path().join("config"));

    let mut user = UserConfig::default();
    assert!(
        ProviderConfig::new("openrouter").enabled(),
        "a freshly created provider is enabled by default"
    );

    let mut provider = ProviderConfig::new("openrouter");
    provider
        .set_base_url(Some("https://mirror.example.com/v1".to_owned()))
        .set_credential_env(vec!["OPENROUTER_API_KEY".to_owned()]);
    user.providers_mut().set("my-router", provider.clone());

    // Disable: the rest of the configuration must not move.
    let mut disabled = provider.clone();
    disabled.set_enabled(false);
    user.providers_mut().set("my-router", disabled.clone());
    user.save(&paths).unwrap();

    let loaded = UserConfig::load(&paths).unwrap();
    let loaded_provider = loaded.providers().get("my-router").unwrap();
    assert!(!loaded_provider.enabled(), "the provider must be disabled");
    assert_eq!(
        loaded_provider.base_url(),
        Some("https://mirror.example.com/v1"),
        "disabling must not touch the base URL"
    );
    assert_eq!(
        loaded_provider.credential_env(),
        &["OPENROUTER_API_KEY".to_owned()],
        "disabling must not touch the credential variable names"
    );

    // Re-enable: reversible without retyping anything already configured.
    let mut re_enabled = loaded_provider.clone();
    re_enabled.set_enabled(true);
    let mut user = loaded;
    user.providers_mut().set("my-router", re_enabled);
    user.save(&paths).unwrap();
    let reloaded = UserConfig::load(&paths).unwrap();
    let reloaded_provider = reloaded.providers().get("my-router").unwrap();
    assert!(reloaded_provider.enabled());
    assert_eq!(
        reloaded_provider.base_url(),
        Some("https://mirror.example.com/v1"),
        "re-enabling must not have required retyping the base URL"
    );
}

/// A file written before `enabled` existed has no `enabled` key at all —
/// it must still load as enabled, not as a parse failure or a silent
/// disable.
#[test]
fn a_provider_with_no_enabled_key_loads_as_enabled() {
    let tmp = tempfile::tempdir().unwrap();
    let paths = RuntimePaths::new(tmp.path().join("data"), tmp.path().join("config"));
    std::fs::create_dir_all(paths.config_dir()).unwrap();
    std::fs::write(
        paths.user_config_file(),
        "version = 1\n[providers.legacy]\ntemplate = \"openrouter\"\n",
    )
    .unwrap();

    let loaded = UserConfig::load(&paths).unwrap();
    assert!(loaded.providers().get("legacy").unwrap().enabled());
}

#[test]
fn a_configured_base_url_override_is_what_reaches_a_launched_child_process() {
    // Line 423, all the way through: a base-URL override is not just a
    // config-layer value (`a_configured_provider_may_override_a_template_base_url`
    // already proves that) — it is what a real launch actually points
    // the harness at.
    let mut user = UserConfig::default();
    let mut provider_cfg = ProviderConfig::new("openrouter");
    provider_cfg.set_base_url(Some("https://mirror.example.com/api".to_owned()));
    user.providers_mut().set("my-openrouter", provider_cfg);

    let effective = EffectiveConfig::new(&user, None);
    let provider = effective
        .configured_provider("my-openrouter")
        .unwrap()
        .value;

    let adapter = crate::harness::adapter_for(IntegrationId::ClaudeCode).expect("a harness");
    let mut profile = crate::profile::LaunchProfile::native(IntegrationId::ClaudeCode);
    profile.backend = crate::profile::BackendResource::DirectProvider {
        provider: provider.name.clone(),
    };
    let secrets = OneShotSecrets("OPENROUTER_API_KEY", "sk-test-not-a-real-key");
    let resolution = crate::profile::Resolution {
        adapter,
        acknowledged_bypass: false,
        provider: Some(&provider),
        secrets: &secrets,
    };

    let overlay = crate::profile::resolve(&profile, &resolution)
        .expect("a configured openrouter provider now backs Claude Code via anthropic-messages");
    let base_url = overlay
        .env()
        .iter()
        .find(|(key, _)| key == std::ffi::OsStr::new("ANTHROPIC_BASE_URL"))
        .map(|(_, value)| value.to_string_lossy().into_owned())
        .expect("ANTHROPIC_BASE_URL must be set");
    assert_eq!(
        base_url, "https://mirror.example.com/api",
        "the configured override must reach the child, not openrouter's own default \
         (https://openrouter.ai/api)"
    );
}

/// Line 353, closed by a test: a `claude-code` profile backed by a
/// *configured* OpenRouter provider (no override at all) resolves, and
/// its `ANTHROPIC_BASE_URL` is the root OpenRouter now also serves
/// Anthropic Messages at — no `/v1`. That suffix is the exact mistake
/// the reference implementation (`~/projects/openrouter-clis/bin/claude-or`)
/// had to write a comment about: Claude Code appends `/v1/messages`
/// itself, so a base URL still carrying `/v1` would double it up.
#[test]
fn a_configured_openrouter_provider_backs_claude_code_at_the_v1_less_api_root() {
    let mut user = UserConfig::default();
    user.providers_mut()
        .set("openrouter-configured", ProviderConfig::new("openrouter"));

    let effective = EffectiveConfig::new(&user, None);
    let provider = effective
        .configured_provider("openrouter-configured")
        .unwrap()
        .value;

    let adapter = crate::harness::adapter_for(IntegrationId::ClaudeCode).expect("a harness");
    let mut profile = crate::profile::LaunchProfile::native(IntegrationId::ClaudeCode);
    profile.backend = crate::profile::BackendResource::DirectProvider {
        provider: provider.name.clone(),
    };
    let secrets = OneShotSecrets("OPENROUTER_API_KEY", "sk-test-not-a-real-key");
    let resolution = crate::profile::Resolution {
        adapter,
        acknowledged_bypass: false,
        provider: Some(&provider),
        secrets: &secrets,
    };

    let overlay = crate::profile::resolve(&profile, &resolution)
        .expect("claude-code + a configured openrouter provider must now resolve");
    let base_url = overlay
        .env()
        .iter()
        .find(|(key, _)| key == std::ffi::OsStr::new("ANTHROPIC_BASE_URL"))
        .map(|(_, value)| value.to_string_lossy().into_owned())
        .expect("ANTHROPIC_BASE_URL must be set");
    assert_eq!(base_url, "https://openrouter.ai/api");
    assert!(
        !base_url.ends_with("/v1"),
        "ANTHROPIC_BASE_URL must not carry a /v1 suffix: Claude Code appends \
         /v1/messages itself, so a URL of {base_url:?} would double it up"
    );
}

#[test]
fn a_configured_provider_may_declare_custom_headers_that_reach_the_provider() {
    let mut user = UserConfig::default();
    let mut provider_cfg = ProviderConfig::new("openrouter");
    provider_cfg.set_headers(vec![
        ("X-Org-Id".to_owned(), "acme".to_owned()),
        ("X-Trace".to_owned(), "on".to_owned()),
    ]);
    user.providers_mut().set("headered", provider_cfg);

    let effective = EffectiveConfig::new(&user, None);
    let provider = effective.configured_provider("headered").unwrap().value;
    assert_eq!(
        provider.headers,
        vec![
            ("X-Org-Id".to_owned(), "acme".to_owned()),
            ("X-Trace".to_owned(), "on".to_owned()),
        ]
    );
}

#[test]
fn a_header_name_with_an_unsafe_character_is_refused_and_named() {
    for (name, offending) in [("Bad:Name", ':'), ("Bad Name", ' ')] {
        let mut provider_cfg = ProviderConfig::new("openrouter");
        provider_cfg.set_headers(vec![(name.to_owned(), "value".to_owned())]);

        let err = provider_cfg
            .to_provider("headered")
            .expect_err("an unsafe header name must be refused");
        match &err {
            ProviderConfigError::InvalidHeaderName {
                header_name,
                offending: found,
                ..
            } => {
                assert_eq!(header_name, name);
                assert_eq!(*found, offending);
            }
            other => panic!("expected InvalidHeaderName for `{name}`, got {other:?}"),
        }
    }
}

#[test]
fn a_header_value_with_a_control_character_is_refused_and_named() {
    for (value, offending) in [("line-one\r\nline-two", '\r'), ("has\ttab", '\t')] {
        let mut provider_cfg = ProviderConfig::new("openrouter");
        provider_cfg.set_headers(vec![("X-Glasshouse".to_owned(), value.to_owned())]);

        let err = provider_cfg
            .to_provider("headered")
            .expect_err("a control character in a header value must be refused");
        match &err {
            ProviderConfigError::InvalidHeaderValue {
                header_name,
                offending: found,
                ..
            } => {
                assert_eq!(header_name, "X-Glasshouse");
                assert_eq!(*found, offending);
            }
            other => panic!("expected InvalidHeaderValue for {value:?}, got {other:?}"),
        }
    }
}

#[test]
fn a_header_carrying_crlf_is_refused_rather_than_escaped() {
    // The concrete injection this whole check exists to stop: a header
    // value containing a newline would otherwise let a configured
    // provider inject a second header of its own choosing into every
    // request Claude Code or Codex sends.
    let mut provider_cfg = ProviderConfig::new("openrouter");
    provider_cfg.set_headers(vec![(
        "X-Glasshouse".to_owned(),
        "legit\r\nX-Injected: evil".to_owned(),
    )]);

    let err = provider_cfg
        .to_provider("headered")
        .expect_err("a newline in a header value must be refused, never escaped");
    assert!(matches!(
        err,
        ProviderConfigError::InvalidHeaderValue { .. }
    ));
}

/// Structural guard, not a string search: enumerate every field this
/// module's config types can hold and assert none of them is
/// credential-shaped. If a future edit adds a field, this test forces a
/// conscious look rather than an accidental secret leaking into a
/// tracked `.glasshouse` file or the user config.
#[test]
fn serialized_form_has_no_secret_capable_field() {
    // `IntegrationConfig` has exactly these four fields.
    let cfg = IntegrationConfig {
        enabled: Some(true),
        executable: Some(PathBuf::from("/usr/bin/example")),
        project_hooks: Some(true),
        bypass_acknowledged: Some(true),
    };
    let value = toml::Value::try_from(&cfg).unwrap();
    let table = value.as_table().unwrap();
    let mut keys: Vec<&str> = table.keys().map(String::as_str).collect();
    keys.sort_unstable();
    assert_eq!(
        keys,
        vec![
            "bypass_acknowledged",
            "enabled",
            "executable",
            "project_hooks"
        ],
        "IntegrationConfig grew a field — confirm it cannot hold a credential \
         before widening this list"
    );

    // `ProfileConfig` — the other per-item shape this module stores —
    // likewise. `backend`'s `DirectProvider { provider }` payload is a
    // provider *name*, not a credential; there is still no field here
    // that could hold one.
    let mut profile_cfg = ProfileConfig::new(IntegrationId::ClaudeCode);
    profile_cfg
        .set_backend(ProfileBackend::DirectProvider {
            provider: "openrouter".to_owned(),
        })
        .set_model(Some("claude-opus".to_owned()))
        .set_expected_protocol(Some("anthropic-messages".to_owned()))
        .set_approval(ProfileApproval::Bypass)
        // Non-default, so the field actually appears below — see
        // `enabled_by_default`/`is_enabled_by_default`.
        .set_enabled(false);
    let profile_value = toml::Value::try_from(&profile_cfg).unwrap();
    let profile_table = profile_value.as_table().unwrap();
    let mut profile_keys: Vec<&str> = profile_table.keys().map(String::as_str).collect();
    profile_keys.sort_unstable();
    assert_eq!(
        profile_keys,
        vec![
            "approval",
            "backend",
            "enabled",
            "expected_protocol",
            "harness",
            "model"
        ],
        "ProfileConfig grew a field — confirm it cannot hold a credential before \
         widening this list"
    );

    // `RoutingModelChoice::Pinned` — the newest stored shape, and the
    // only variant of it that carries a payload. Both halves are NAMES:
    // `provider` is a key into `ProviderTable` and `model` is a model
    // name, exactly like `ProfileConfig`'s `backend`/`model` pair above.
    // Turning either into an actual credential stays `SecretStore`'s job.
    let pinned_routing = RoutingModelChoice::Pinned {
        provider: "openrouter".to_owned(),
        model: "gpt-5.6-luna".to_owned(),
    };
    let pinned_routing_value = toml::Value::try_from(&pinned_routing).unwrap();
    let pinned_routing_table = pinned_routing_value.as_table().unwrap();
    let mut pinned_routing_keys: Vec<&str> =
        pinned_routing_table.keys().map(String::as_str).collect();
    pinned_routing_keys.sort_unstable();
    assert_eq!(
        pinned_routing_keys,
        vec!["kind", "model", "provider"],
        "RoutingModelChoice::Pinned grew a field — confirm it cannot hold a credential \
         before widening this list"
    );

    // `ProviderConfig` — the shape that comes closest to a credential,
    // since it is the one a provider's key is configured through. Its
    // `credential_store` holds a `StoredCredentialRef`, which is two
    // NAMES; there is still no field here that could hold a value.
    let mut provider_cfg = ProviderConfig::new("openrouter");
    provider_cfg
        .set_base_url(Some("https://example.invalid".to_owned()))
        .set_credential_env(vec!["OPENROUTER_API_KEY".to_owned()])
        .set_credential_store(Some(StoredCredentialRef::new(
            "glasshouse",
            "OPENROUTER_API_KEY",
        )))
        .set_headers(vec![("X-Test".to_owned(), "1".to_owned())])
        .set_enabled(false)
        .set_free_models(vec!["nvidia/nemotron-nano-9b-v2:free".to_owned()])
        // A model NAME and a workload-tier spelling. Line 1796's field is
        // in this guard for the same reason `free_models` is: it is keyed
        // by the same identifier, and neither half can hold a value.
        .set_model_ceilings(BTreeMap::from([(
            "nvidia/nemotron-nano-9b-v2:free".to_owned(),
            ConfiguredWorkloadTier::new(crate::routing::classify::WorkloadTier::Leaf),
        )]));
    let provider_value = toml::Value::try_from(&provider_cfg).unwrap();
    let provider_table = provider_value.as_table().unwrap();
    let mut provider_keys: Vec<&str> = provider_table.keys().map(String::as_str).collect();
    provider_keys.sort_unstable();
    assert_eq!(
        provider_keys,
        vec![
            "base_url",
            "credential_env",
            "credential_store",
            "enabled",
            "free_models",
            "headers",
            "model_ceilings",
            "template"
        ],
        "ProviderConfig grew a field — confirm it cannot hold a credential before \
         widening this list"
    );
    // ... and the one field that names a secret store really does hold
    // only names.
    let stored = provider_table["credential_store"].as_table().unwrap();
    let mut stored_keys: Vec<&str> = stored.keys().map(String::as_str).collect();
    stored_keys.sort_unstable();
    assert_eq!(
        stored_keys,
        vec!["account", "service"],
        "StoredCredentialRef grew a field — a reference is a service and an account, \
         and nothing else"
    );

    // `UserConfig`'s top level, likewise.
    let user = fully_populated_user_config();
    let user_value = toml::Value::try_from(&user).unwrap();
    let mut user_keys: Vec<&str> = user_value
        .as_table()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    user_keys.sort_unstable();
    assert_eq!(
        user_keys,
        vec![
            "integrations",
            "memory_extraction",
            "onboarding",
            "profiles",
            "providers",
            "version"
        ]
    );

    // And the serialized TOML text itself contains none of the names a
    // secret field would plausibly carry, as a cheap extra check on top
    // of the structural one above.
    let serialized = toml::to_string_pretty(&user).unwrap();
    for forbidden in ["key", "token", "secret", "password", "credential"] {
        assert!(
            !serialized.to_lowercase().contains(forbidden),
            "serialized UserConfig unexpectedly contains `{forbidden}`:\n{serialized}"
        );
    }
}

/// Structural guard for [`ProviderConfig`] specifically, alongside
/// [`serialized_form_has_no_secret_capable_field`]'s coverage of the
/// rest of this module's config types.
///
/// `credential_env` holds environment variable *names* (e.g.
/// `"OPENROUTER_API_KEY"`), which legitimately contain words like "key"
/// as part of a name — that is exactly what the field is for. So unlike
/// the sibling test's broad word-scan (which only ever runs against a
/// fixture with no provider entries), what proves this type cannot hold
/// a secret *value* is structural: `credential_env`'s type is
/// `Vec<String>` of names, and this list pins that `ProviderConfig` has
/// no field beyond that, `base_url`, and `template` — nothing shaped to
/// carry an actual credential.
#[test]
fn no_provider_type_can_hold_a_credential_value() {
    let mut provider_cfg = ProviderConfig::new("openrouter");
    provider_cfg
        .set_base_url(Some("https://mirror.example.com/v1".to_owned()))
        .set_credential_env(vec!["OPENROUTER_API_KEY".to_owned()])
        // Set, so the field is actually serialized and this list pins
        // it: `skip_serializing_if = "Option::is_none"` means an unset
        // one would be invisible here and the guard would pass without
        // ever having seen it.
        .set_credential_store(Some(StoredCredentialRef::new(
            "glasshouse",
            "OPENROUTER_API_KEY",
        )))
        .set_headers(vec![("X-Org-Id".to_owned(), "acme".to_owned())])
        // Non-default, so the field actually appears below — see
        // `enabled_by_default`/`is_enabled_by_default`.
        .set_enabled(false);
    let provider_value = toml::Value::try_from(&provider_cfg).unwrap();
    let provider_table = provider_value.as_table().unwrap();
    let mut provider_keys: Vec<&str> = provider_table.keys().map(String::as_str).collect();
    provider_keys.sort_unstable();
    assert_eq!(
        provider_keys,
        vec![
            "base_url",
            "credential_env",
            "credential_store",
            "enabled",
            "headers",
            "template"
        ],
        "ProviderConfig grew a field — confirm it cannot hold a credential value \
         (as opposed to a variable name) before widening this list. `headers` holds \
         name/value pairs that are themselves configuration, never a credential — see \
         ProviderConfig::set_headers's own doc for why that is safe; \
         `credential_store` holds a service and an account NAME — see \
         StoredCredentialRef."
    );

    // `ProviderTable` itself adds nothing beyond the map: every entry it
    // can hold is one of the five fields just checked.
    let mut table = ProviderTable::default();
    table.set("mine", provider_cfg);
    let table_value = toml::Value::try_from(&table).unwrap();
    let entry = table_value.as_table().unwrap().get("mine").unwrap();
    let mut entry_keys: Vec<&str> = entry
        .as_table()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    entry_keys.sort_unstable();
    assert_eq!(
        entry_keys,
        vec![
            "base_url",
            "credential_env",
            "credential_store",
            "enabled",
            "headers",
            "template"
        ]
    );
}

/// Acceptance 4: a `SecretRef` naming an OS credential survives a real
/// save/load round trip through a configuration file, and the file's own
/// text carries no value.
///
/// A known credential is planted in the environment variable the
/// reference is named after *and* handed to the store, so a
/// serialisation that reached for either would be caught. Asserted with
/// `!contains`, never `assert_eq!` on the secret material — a failing
/// `assert_eq!` prints both sides.
#[test]
fn an_os_credential_reference_round_trips_through_configuration_without_its_value() {
    const VAR: &str = "GLASSHOUSE_CONFIG_TEST_ONLY_STORED_VAR";
    const VALUE: &str = "sk-config-round-trip-test-0123456789abcdef";

    let tmp = tempfile::tempdir().unwrap();
    let paths = RuntimePaths::new(tmp.path().join("data"), tmp.path().join("config"));

    let reference = crate::secret::native::os_credential_for_variable(VAR);
    let stored = StoredCredentialRef::from_secret_ref(&reference)
        .expect("an OsCredential reference has a stored shape");

    let mut provider = ProviderConfig::new("openrouter");
    provider
        .set_credential_env(vec![VAR.to_owned()])
        .set_credential_store(Some(stored.clone()));

    let mut user = UserConfig::default();
    user.providers_mut().set("stored", provider);

    // SAFETY: `VAR` is unique to this test and removed again below.
    // Planted so that a serializer which resolved the reference — the
    // failure this test exists to catch — would have something to leak.
    unsafe {
        std::env::set_var(VAR, VALUE);
    }
    let saved = user.save(&paths);
    unsafe {
        std::env::remove_var(VAR);
    }
    saved.unwrap();

    let path = paths.config_dir().join("config.toml");
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(
        !text.contains(VALUE),
        "a credential value reached the configuration file:\n{text}"
    );
    assert!(text.contains(VAR), "the NAME must be there:\n{text}");
    assert!(text.contains("glasshouse"), "got:\n{text}");

    let loaded = UserConfig::load(&paths).unwrap();
    let loaded_provider = loaded.providers().get("stored").unwrap();
    assert_eq!(loaded_provider.credential_store(), Some(&stored));
    assert_eq!(loaded_provider.credential_store().unwrap().account(), VAR);
    assert_eq!(
        loaded_provider.credential_store().unwrap().to_secret_ref(),
        reference,
        "the stored shape and the reference it came from must be the same thing"
    );

    // An environment reference has no stored shape — writing one here
    // would claim something about where a key lives that nobody
    // established.
    assert_eq!(
        StoredCredentialRef::from_secret_ref(&crate::secret::SecretRef::Environment {
            var: VAR.to_owned()
        }),
        None
    );
}

/// [`an_os_credential_reference_round_trips_through_configuration_without_its_value`]'s
/// sibling for the *project*-level file: box 1789 is specifically about
/// what a project may write into its own tracked `.glasshouse/config.toml`
/// — a file real repositories check in — so the guarantee needs its own
/// proof at [`write_project_config_with_consent`] rather than resting on
/// the user-file test alone.
///
/// "Wide", here, is comprehensiveness rather than a TUI viewport (§17's
/// truncation risk does not apply to a TOML file: nothing elides it) — a
/// project config populated across every component table this module
/// exposes (providers with headers and a credential store, profiles,
/// pairing corrections, a response profile, routing), so a leak in any
/// one of them would show up here rather than only in a narrow fixture.
#[test]
fn project_config_file_never_contains_a_planted_secret_value_across_every_table() {
    const VAR: &str = "GLASSHOUSE_PROJECT_CONFIG_TEST_ONLY_SECRET_VAR";
    const VALUE: &str = "sk-project-config-test-only-0123456789abcdef";

    let workspace = tempfile::tempdir().unwrap();
    let project = test_project(workspace.path());

    let mut provider = ProviderConfig::new("openrouter");
    provider
        .set_base_url(Some("https://example.invalid".to_owned()))
        .set_credential_env(vec![VAR.to_owned()])
        .set_credential_store(Some(StoredCredentialRef::new("glasshouse", VAR)))
        .set_headers(vec![("X-Test".to_owned(), "1".to_owned())])
        .set_free_models(vec!["nvidia/nemotron-nano-9b-v2:free".to_owned()]);

    let mut profile = ProfileConfig::new(IntegrationId::ClaudeCode);
    profile.set_backend(ProfileBackend::DirectProvider {
        provider: "wide".to_owned(),
    });

    let mut config = ProjectConfig::default();
    config.providers_mut().set("wide", provider);
    config.profiles_mut().set("wide", profile);
    config
        .integrations_mut()
        .entry(IntegrationId::Codex)
        .set_executable(Some(PathBuf::from("/opt/codex/bin/codex")));
    config
        .pairing_mut()
        .model_entry("gpt-5.6-luna")
        .set_developer(Some("openai".to_owned()));
    config
        .response_mut()
        .default_entry_mut()
        .set_preset(Some("audit".to_owned()));
    config
        .routing_mut()
        .set_model(Some(RoutingModelChoice::Pinned {
            provider: "wide".to_owned(),
            model: "gpt-5.6-luna".to_owned(),
        }));

    // SAFETY: `VAR` is unique to this test and removed again below.
    // Planted so that a serializer which resolved the reference would
    // have something to leak.
    unsafe {
        std::env::set_var(VAR, VALUE);
    }
    let written = write_project_config_with_consent(&project, &config);
    unsafe {
        std::env::remove_var(VAR);
    }
    written.unwrap();

    let path = project.root().join(PROJECT_CONFIG_RELATIVE_PATH);
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(
        !text.contains(VALUE),
        "a credential value reached the project configuration file:\n{text}"
    );
    assert!(text.contains(VAR), "the NAME must be there:\n{text}");

    // The same broad word-scan the user-file structural test runs,
    // applied to a project file that actually populates every table —
    // unlike that test's fixture, this one legitimately writes
    // `credential_env`/`credential_store` as *keys*, so the scan is
    // narrowed to lines that are not those two keys' own declarations.
    for line in text.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("credential_env")
            || trimmed.starts_with("credential_store")
            || trimmed.starts_with("service =")
            || trimmed.starts_with("account =")
        {
            continue;
        }
        for forbidden in ["token", "secret", "password"] {
            assert!(
                !line.to_lowercase().contains(forbidden),
                "project configuration file unexpectedly contains `{forbidden}` outside a \
                 credential reference's own keys: {line}"
            );
        }
    }
}
