//! Map lines 1945 and 1955 — proof-only package `GH-DECOUPLE-PROOFS`.
//!
//! Both claims are already true in the shipped `EffectiveConfig::launch_profile`
//! resolver (`config::mod.rs:5070-5107`); this file pins them as tests, adding
//! no production behaviour. See `.agent-runtime/report-recon-56.md`'s "Cause 1"
//! section for the census this package proves.
//!
//! The destination-carrying half of 1945 (`routing_destinations` yielding one
//! fresh destination per profile) lives in `main.rs`'s own test module,
//! because `routing_destinations` is private to the binary crate.

use glasshouse::config::{EffectiveConfig, ProfileBackend, ProfileConfig, UserConfig};
use glasshouse::integrations::IntegrationId;

/// Map line 1945: a launch profile's `harness`, `backend` and `model` are
/// three independent fields, and `EffectiveConfig::launch_profile` resolves
/// each profile to its own values, never deriving one from another.
///
/// Mutation target: collapse `launch_profile` to return the native profile
/// for every name — this test fails because the direct-provider profile's
/// resolved backend and model would no longer be its own.
#[test]
fn line_1945_two_profiles_of_one_harness_resolve_independent_backend_and_model() {
    let harness = IntegrationId::ClaudeCode;

    let mut user = UserConfig::default();

    let mut native_profile = ProfileConfig::new(harness);
    native_profile.set_model(Some("claude-native-model".to_owned()));
    user.profiles_mut().set("alpha-native", native_profile);

    let mut direct_profile = ProfileConfig::new(harness);
    direct_profile.set_backend(ProfileBackend::DirectProvider {
        provider: "openrouter".to_owned(),
    });
    direct_profile.set_model(Some("some/other-model".to_owned()));
    user.profiles_mut().set("beta-direct", direct_profile);

    let effective = EffectiveConfig::new(&user, None);

    let alpha = effective.launch_profile("alpha-native", harness).unwrap();
    assert_eq!(alpha.value.harness, harness);
    assert_eq!(
        alpha.value.backend,
        glasshouse::profile::BackendResource::Native
    );
    assert_eq!(alpha.value.model.as_deref(), Some("claude-native-model"));

    let beta = effective.launch_profile("beta-direct", harness).unwrap();
    assert_eq!(beta.value.harness, harness);
    assert_eq!(
        beta.value.backend,
        glasshouse::profile::BackendResource::DirectProvider {
            provider: "openrouter".to_owned(),
        }
    );
    assert_eq!(beta.value.model.as_deref(), Some("some/other-model"));

    assert_ne!(
        alpha.value.backend, beta.value.backend,
        "two profiles of the same harness must not be forced onto the same backend"
    );
    assert_ne!(
        alpha.value.model, beta.value.model,
        "two profiles of the same harness must not be forced onto the same model"
    );
}

/// Map line 1955: an existing profile keeps its native pairing until the user
/// changes it — adding a second profile cannot mutate a previously
/// configured one, and a configured `native` entry can never shadow the
/// built-in native profile.
///
/// Two mutation targets, both under this one test:
/// - remove the `if name == NATIVE_PROFILE_NAME` short-circuit in
///   `launch_profile` → the configured `[profiles.native]` shadow wins and
///   the "native is unshadowable" assertion fails.
/// - break the name-keyed map lookup so a later insertion mutates an earlier
///   one → the "previously configured profile is unchanged" assertion fails.
#[test]
fn line_1955_native_is_unshadowable_and_existing_profiles_survive_new_ones() {
    let harness = IntegrationId::Codex;

    let mut user = UserConfig::default();

    let mut first = ProfileConfig::new(harness);
    first.set_model(Some("first-model".to_owned()));
    first.set_backend(ProfileBackend::DirectProvider {
        provider: "anthropic-direct".to_owned(),
    });
    user.profiles_mut().set("first", first);

    let effective_before = EffectiveConfig::new(&user, None);
    let resolved_before = effective_before
        .launch_profile("first", harness)
        .unwrap()
        .value;

    // Add a second, unrelated profile.
    let mut second = ProfileConfig::new(harness);
    second.set_model(Some("second-model".to_owned()));
    user.profiles_mut().set("second", second);

    // Add a config table literally named "native" with different fields —
    // the adversarial shadow case.
    let mut shadow = ProfileConfig::new(harness);
    shadow.set_backend(ProfileBackend::DirectProvider {
        provider: "shadow-provider".to_owned(),
    });
    shadow.set_model(Some("shadow-model".to_owned()));
    user.profiles_mut()
        .set(glasshouse::profile::NATIVE_PROFILE_NAME, shadow);

    let effective_after = EffectiveConfig::new(&user, None);

    let resolved_after = effective_after
        .launch_profile("first", harness)
        .unwrap()
        .value;
    assert_eq!(
        resolved_before, resolved_after,
        "adding new profiles must not change an existing profile's resolved fields"
    );

    let native = effective_after
        .launch_profile(glasshouse::profile::NATIVE_PROFILE_NAME, harness)
        .unwrap();
    assert_eq!(
        native.value,
        glasshouse::profile::LaunchProfile::native(harness),
        "a configured [profiles.native] table must never shadow the built-in native profile"
    );
}
