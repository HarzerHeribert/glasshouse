//! The native secure store, from outside the crate (Phase 9E, lines 441 and
//! 442).
//!
//! # What this file exists to prove that the unit tests cannot
//!
//! Two things, and both of them are about the *build* rather than about a
//! function:
//!
//! 1. **The manifest can never enable `keyring` without a real backend.**
//!    `keyring` 3.x silently resolves to an in-process **mock** store when no
//!    backend feature is on for the target — a store that takes a provider
//!    credential, hands it back within the process, persists nothing, and
//!    reports itself as secure. That is not a build failure, so nothing but
//!    a check like this one would notice it.
//!    [`the_manifest_never_declares_keyring_without_a_real_backend`] reads
//!    `crates/glasshouse/Cargo.toml` and fails on any `keyring` line that is
//!    not paired with its own platform's backend feature.
//! 2. **A platform with no backend hands back no store at all**, and a
//!    platform with one never claims the reverse. Linux gained a backend on
//!    2026-09-05, so its assertion moved rather than went away: a headless
//!    cell — which every Linux CI cell is — must refuse with
//!    `StoreUnreachable` and an instruction, never with
//!    `UnsupportedPlatform`, and it must do it inside a bound, because this
//!    call is on the launch path.
//!
//! # Why the round-trip tests are gated rather than universal
//!
//! A test that runs the same assertions on every platform and is satisfied
//! by a mock on two of them proves nothing on those two. So the round trip
//! runs only where a real OS store answers, and every other target gets the
//! negative assertion instead — which is not a weaker test, it is the
//! required behaviour for that target.

use glasshouse::secret::native::{
    NativeSecretStore, PreferNativeSecretStore, SERVICE, STORE_UNREACHABLE_LABEL,
    UNSUPPORTED_PLATFORM_LABEL, Unavailable, os_credential_for_variable,
};
use glasshouse::secret::{SecretRef, SecretStore};

// Used only by the tests that need a store to answer, so gated exactly as
// they are: on a target with no backend these are dead imports, and `-D
// warnings` makes dead imports a hard error. Practice §18's rule — anything
// used only by a platform-gated item needs the same gate — reaches import
// lists too, and this file went red on the flipped build until it did.
#[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
use glasshouse::secret::EnvironmentSecretStore;
#[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
use glasshouse::secret::native::Deletion;

// --- the manifest may never link the mock --------------------------------

/// The backend feature each platform's `keyring` dependency must carry.
///
/// A target that appears in the manifest and not here is a failure rather
/// than a pass: adding a platform means proving it, and this list is where
/// that claim is recorded in a form a test can read.
const REQUIRED_BACKEND: &[(&str, &str)] =
    &[("macos", "apple-native"), ("windows", "windows-native")];

/// Every complaint this manifest has, or an empty list.
///
/// Written as a function over text rather than as assertions so that
/// [`the_manifest_scan_would_catch_a_violation`] can run the identical logic
/// over manifests that are wrong on purpose. A scan whose falsifiability is
/// not itself proven is a comment with a test's name.
fn manifest_complaints(manifest: &str) -> Vec<String> {
    let mut complaints = Vec::new();
    let mut seen: Vec<&str> = Vec::new();
    let mut section = "";

    for line in manifest.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') {
            section = trimmed;
            continue;
        }
        if trimmed.starts_with('#') || !trimmed.starts_with("keyring") {
            continue;
        }

        // The section header must name exactly one target OS, so that the
        // dependency cannot reach a platform whose backend nobody chose.
        let Some(os) = section
            .strip_prefix("[target.'cfg(target_os = \"")
            .and_then(|rest| rest.split('"').next())
        else {
            complaints.push(format!(
                "`keyring` is declared under `{section}`, which is not a single-target \
                 section: on any target it reaches without a backend feature, `keyring` \
                 resolves to its in-process mock store"
            ));
            continue;
        };

        let Some((_, backend)) = REQUIRED_BACKEND.iter().find(|(name, _)| *name == os) else {
            complaints.push(format!(
                "`keyring` is declared for `{os}`, which has no proven backend in this \
                 project: add it to REQUIRED_BACKEND only together with the evidence that \
                 its store was executed"
            ));
            continue;
        };

        if !trimmed.contains(&format!("\"{backend}\"")) {
            complaints.push(format!(
                "the `keyring` dependency for `{os}` does not enable `{backend}`: without a \
                 backend feature it links the mock store, which persists nothing and still \
                 describes itself as the operating system's store"
            ));
        }
        seen.push(os);
    }

    for (os, _) in REQUIRED_BACKEND {
        if !seen.contains(os) {
            complaints.push(format!(
                "no `keyring` dependency is declared for `{os}`, but this project claims \
                 that platform's native store"
            ));
        }
    }
    complaints
}

/// The mock hazard, checked against the manifest that decides it.
#[test]
fn the_manifest_never_declares_keyring_without_a_real_backend() {
    let complaints = manifest_complaints(include_str!("../Cargo.toml"));
    assert!(
        complaints.is_empty(),
        "crates/glasshouse/Cargo.toml would link a store that is not the platform's:\n- {}",
        complaints.join("\n- ")
    );
}

/// The scan above is worth having only if it can fail, and the three ways it
/// must fail are the three ways this project could actually get there.
#[test]
fn the_manifest_scan_would_catch_a_violation() {
    // Shared `[dependencies]`: reaches every target, backend for none.
    let shared = "[dependencies]\nkeyring = { workspace = true }\n";
    assert!(!manifest_complaints(shared).is_empty());

    // Per-target, but with the feature forgotten — the exact shape of a
    // "just enable it on Linux too" change.
    let featureless = "[target.'cfg(target_os = \"macos\")'.dependencies]\n\
                       keyring = { workspace = true }\n";
    assert!(!manifest_complaints(featureless).is_empty());

    // A platform nobody proved, with a plausible-looking feature.
    let unproven = "[target.'cfg(target_os = \"linux\")'.dependencies]\n\
                    keyring = { workspace = true, features = [\"sync-secret-service\"] }\n";
    assert!(!manifest_complaints(unproven).is_empty());

    // ... and it stays quiet on the shape that is correct.
    let correct = "[target.'cfg(target_os = \"macos\")'.dependencies]\n\
                   keyring = { workspace = true, features = [\"apple-native\"] }\n\
                   [target.'cfg(target_os = \"windows\")'.dependencies]\n\
                   keyring = { workspace = true, features = [\"windows-native\"] }\n";
    assert!(
        manifest_complaints(correct).is_empty(),
        "{:?}",
        manifest_complaints(correct)
    );
}

// --- the Linux dependency may never reach another target -----------------

/// Every complaint the two manifests have about the Secret Service
/// dependency, or an empty list.
///
/// A sibling of [`manifest_complaints`] and not an extension of it, because
/// the hazard is a different one. `keyring` can silently become a mock;
/// `dbus-secret-service` has no mock mode at all — the string `mock` does not
/// occur in its source. What it *can* do is send a provider credential over
/// the session bus in the clear, if the crypto feature that makes
/// `EncryptionType::Dh` exist is ever dropped, and reach a platform whose
/// Secret Service nobody proved.
///
/// The feature lives in the workspace manifest and the target section lives
/// in the crate manifest, so this reads both: a check of either alone would
/// pass while the other was wrong.
fn secret_service_complaints(crate_manifest: &str, workspace_manifest: &str) -> Vec<String> {
    let mut complaints = Vec::new();
    let mut section = "";
    let mut declared = false;

    for line in crate_manifest.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') {
            section = trimmed;
            continue;
        }
        if trimmed.starts_with('#') || !trimmed.starts_with("dbus-secret-service") {
            continue;
        }
        declared = true;
        if section != "[target.'cfg(target_os = \"linux\")'.dependencies]" {
            complaints.push(format!(
                "`dbus-secret-service` is declared under `{section}`: it links `libdbus-sys`, \
                 whose build script needs `libdbus-1-dev` on the build host, so it may reach \
                 no target but Linux"
            ));
        }
    }
    if !declared {
        complaints.push(
            "no `dbus-secret-service` dependency is declared, but this project claims a \
             Secret Service keyring on Linux"
                .to_owned(),
        );
    }

    for line in workspace_manifest.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('#') || !trimmed.starts_with("dbus-secret-service") {
            continue;
        }
        if !trimmed.contains("\"crypto-rust\"") && !trimmed.contains("\"crypto-openssl\"") {
            complaints.push(
                "the `dbus-secret-service` dependency enables no crypto feature: without one \
                 `EncryptionType::Dh` does not exist and the only session left is `Plain`, \
                 which carries a provider credential over the session bus unencrypted"
                    .to_owned(),
            );
        }
    }
    complaints
}

/// The Linux dependency, checked against the manifests that decide it.
#[test]
fn the_manifests_confine_the_secret_service_dependency_to_linux_and_keep_it_encrypted() {
    let complaints = secret_service_complaints(
        include_str!("../Cargo.toml"),
        include_str!("../../../Cargo.toml"),
    );
    assert!(
        complaints.is_empty(),
        "the Secret Service dependency is declared in a way this project cannot stand \
         behind:\n- {}",
        complaints.join("\n- ")
    );
}

/// The scan above is worth having only if it can fail, and these are the two
/// ways this project could actually get there.
#[test]
fn the_secret_service_manifest_scan_would_catch_a_violation() {
    let correct_crate = "[target.'cfg(target_os = \"linux\")'.dependencies]\n\
                         dbus-secret-service.workspace = true\n";
    let correct_workspace =
        "dbus-secret-service = { version = \"4.1\", features = [\"crypto-rust\"] }\n";

    // Shared `[dependencies]`: every build host now needs `libdbus-1-dev`.
    let shared = "[dependencies]\ndbus-secret-service.workspace = true\n";
    assert!(!secret_service_complaints(shared, correct_workspace).is_empty());

    // The crypto feature dropped, which leaves `EncryptionType::Plain`.
    let plain = "dbus-secret-service = { version = \"4.1\", default-features = false }\n";
    assert!(!secret_service_complaints(correct_crate, plain).is_empty());

    // Not declared at all, on a project that claims the Linux store.
    assert!(!secret_service_complaints("[dependencies]\n", correct_workspace).is_empty());

    // ... and it stays quiet on the shape that is correct.
    assert!(
        secret_service_complaints(correct_crate, correct_workspace).is_empty(),
        "{:?}",
        secret_service_complaints(correct_crate, correct_workspace)
    );
}

// --- the fallback is labelled on every platform ---------------------------

/// Whichever arrangement is in force, a caller can read which one it is.
///
/// Silent fallback is what makes the mock hazard dangerous, so this asserts
/// on the label rather than on a value coming back, and it asserts something
/// different — and true — on each kind of platform.
#[test]
fn the_store_says_which_of_its_sources_is_in_force() {
    let store = PreferNativeSecretStore::detect();
    let description = store.describe();

    match store.native() {
        Ok(native) => {
            assert!(
                description.starts_with(native.describe()),
                "the arrangement must begin by naming the store that answers first: \
                 `{description}` / `{}`",
                native.describe()
            );
            assert!(
                description.contains("process environment"),
                "the arrangement must also name the fallback: {description}"
            );
        }
        Err(Unavailable::UnsupportedPlatform) => {
            assert_eq!(description, UNSUPPORTED_PLATFORM_LABEL);
        }
        Err(Unavailable::StoreUnreachable(_)) => {
            assert_eq!(description, STORE_UNREACHABLE_LABEL);
        }
    }
}

/// Which refusal a platform is allowed to give, asserted where it can be
/// asserted about a value rather than about a `cfg!`.
///
/// On a platform with a backend, "no store Glasshouse can use" is simply
/// false, and a user told it would be told to stop looking for a fix that
/// exists. The only honest refusal here is that the store would not open,
/// which unlocking a keychain or logging in properly might change.
#[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
#[test]
fn on_a_platform_with_a_backend_the_only_honest_refusal_is_that_it_would_not_open() {
    assert_ne!(
        NativeSecretStore::detect(),
        Err(Unavailable::UnsupportedPlatform)
    );
}

/// On a target with no backend, nothing may hand back a native store.
///
/// **This is the Linux gate's real assertion**, and it is what a linked mock
/// would break: the mock's probe succeeds, so `detect` would return `Ok` and
/// `describe` would claim a secure store that persists nothing.
#[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
#[test]
fn on_a_platform_with_no_backend_no_native_store_can_be_built() {
    assert_eq!(
        NativeSecretStore::detect(),
        Err(Unavailable::UnsupportedPlatform)
    );

    let store = PreferNativeSecretStore::detect();
    assert_eq!(store.describe(), UNSUPPORTED_PLATFORM_LABEL);

    // The environment is still the whole answer here, and a reference that
    // names the OS store is answered by nothing at all.
    let reference = os_credential_for_variable("GLASSHOUSE_NO_BACKEND_TEST_ONLY");
    assert!(store.resolve(&reference).is_none());
    assert!(!store.is_present(&reference));
    assert_eq!(store.source_of(&reference), None);
}

/// **Requirement 1, and every Linux CI cell exercises it for free.** A
/// headless container has no session bus at all, so this is the honest
/// refusal on the platform that just gained a backend — and it must arrive
/// inside a bound, because `detect` sits on the launch path.
///
/// Written to pass on a Linux *desktop* too, where the store answers: the
/// claim is not "Linux has no keyring", it is "whichever of the two states
/// this host is in, Glasshouse says so quickly and truthfully". The refusal
/// this arm exists to prevent — a wait — would fail this test by timing out
/// the whole target rather than by an assertion, which is why the elapsed
/// time is asserted rather than assumed.
#[cfg(target_os = "linux")]
#[test]
fn on_linux_a_keyring_that_cannot_be_reached_refuses_quickly_and_says_what_to_do() {
    let started = std::time::Instant::now();
    let detected = NativeSecretStore::detect();
    let elapsed = started.elapsed();

    assert!(
        elapsed < std::time::Duration::from_secs(30),
        "`detect` is on the launch path and took {elapsed:?}: a Secret Service backend that \
         waits for an unlock prompt is the defect this arm exists to prevent"
    );
    assert_ne!(
        detected,
        Err(Unavailable::UnsupportedPlatform),
        "Linux has a backend now, so `UnsupportedPlatform` would tell a user to stop looking \
         for a fix that exists"
    );

    let store = PreferNativeSecretStore::detect();
    match detected {
        Ok(native) => {
            assert!(
                store.describe().starts_with(native.describe()),
                "the arrangement must name the store that answered first: `{}` / `{}`",
                store.describe(),
                native.describe()
            );
        }
        Err(refusal) => {
            assert_eq!(
                store.describe(),
                STORE_UNREACHABLE_LABEL,
                "with no keyring reachable the environment is the source in force, and the \
                 label is where a user reads that"
            );
            let reason = refusal.reason();
            assert!(
                reason.contains("; "),
                "a refusal must carry an instruction after the diagnosis, or a user is told \
                 what happened and not what to do: {reason}"
            );
        }
    }
}

// --- the store itself, where there is one ---------------------------------

/// Removes its stored item when it goes out of scope, however the test
/// leaves — a passing assertion, a failing one, or a panic. A test that
/// deletes on its last line deletes nothing when an assertion above that
/// line fires, and what it leaves behind is in a real user's real store.
#[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
struct StoredItem {
    reference: SecretRef,
}

#[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
impl Drop for StoredItem {
    fn drop(&mut self) {
        // Best effort by construction: this may run while a panic is
        // unwinding, and a second panic from here would replace the
        // assertion message that explains the failure with an abort.
        if let Ok(store) = NativeSecretStore::detect() {
            let _ = store.delete(&self.reference);
        }
    }
}

/// A name no other process in this test run uses, so a leftover item from an
/// earlier run can never be read by a later one.
#[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
fn test_account(suffix: &str) -> String {
    format!(
        "GLASSHOUSE_SECRET_NATIVE_TEST_ONLY_{suffix}_{}",
        std::process::id()
    )
}

/// **The required behaviour, in one test:** a secret written on a platform
/// is readable on that platform and **invisible to
/// [`EnvironmentSecretStore`]**.
///
/// The second half is the part worth writing down. `EnvironmentSecretStore`
/// is the fallback, and it is also the store every other part of Glasshouse
/// used before this phase; if storing a credential natively also made it
/// readable from the environment, the OS store would be a place to *copy*
/// secrets to rather than a place to keep them, and every process the user
/// launches would inherit one.
#[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
#[test]
fn a_credential_in_the_native_store_is_readable_there_and_invisible_to_the_environment() {
    const VALUE: &str = "sk-native-only-0123456789abcdefghijklmn";

    let native = match NativeSecretStore::detect() {
        Ok(native) => native,
        Err(refusal) => {
            eprintln!(
                "SKIPPED: the native secure store would not open in this session: {}",
                refusal.reason()
            );
            return;
        }
    };

    let account = test_account("INVISIBLE");
    let as_variable = SecretRef::Environment {
        var: account.clone(),
    };
    let as_credential = os_credential_for_variable(&account);

    native.store(&as_credential, VALUE).expect("store");
    let _item = StoredItem {
        reference: as_credential.clone(),
    };

    // Readable on this platform, by either name for the same credential.
    assert!(native.is_present(&as_credential));
    assert_eq!(
        native.resolve(&as_credential).expect("resolve").expose(),
        VALUE
    );
    assert_eq!(
        native.resolve(&as_variable).expect("resolve").expose(),
        VALUE
    );

    // Invisible to the environment, under either name, and no variable was
    // created anywhere along the way.
    let environment = EnvironmentSecretStore::new();
    assert!(environment.resolve(&as_variable).is_none());
    assert!(environment.resolve(&as_credential).is_none());
    assert!(!environment.is_present(&as_variable));
    assert!(!environment.is_present(&as_credential));
    assert!(
        std::env::var_os(&account).is_none(),
        "storing a credential natively must not export it to this process"
    );

    // And the composed store names the native half as the source, which is
    // what a user reads to know their key is not in a shell profile.
    let store = PreferNativeSecretStore::detect();
    assert_eq!(store.source_of(&as_variable), Some(native.describe()));
}

/// Deleting a credential that is not there is the desired state, reported
/// rather than raised — and it stays that way after a real deletion, which
/// is the case a "delete then delete again" bug would break.
#[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
#[test]
fn deleting_a_credential_that_is_not_there_is_success() {
    const VALUE: &str = "sk-delete-twice-abcdefghijklmnop01234567";

    let native = match NativeSecretStore::detect() {
        Ok(native) => native,
        Err(refusal) => {
            eprintln!(
                "SKIPPED: the native secure store would not open in this session: {}",
                refusal.reason()
            );
            return;
        }
    };

    let account = test_account("DELETE");
    let reference = os_credential_for_variable(&account);
    let guard = StoredItem {
        reference: reference.clone(),
    };

    // Never written: absent is not an error.
    assert_eq!(
        native.delete(&reference).expect("deleting nothing"),
        Deletion::AlreadyAbsent
    );

    native.store(&reference, VALUE).expect("store");
    assert_eq!(
        native.delete(&reference).expect("delete"),
        Deletion::Removed
    );
    assert_eq!(
        native.delete(&reference).expect("delete again"),
        Deletion::AlreadyAbsent
    );
    assert!(!native.is_present(&reference));

    drop(guard);
}

/// The service every Glasshouse credential is filed under is one fixed name,
/// so a user can find their own items and this project can say where to
/// look. Asserted from outside the crate because it is effectively public:
/// it appears in `security find-generic-password -s ...` and in Credential
/// Manager's target names.
#[test]
fn credentials_are_filed_under_one_fixed_service_name() {
    assert_eq!(SERVICE, "glasshouse");
    assert_eq!(
        os_credential_for_variable("OPENROUTER_API_KEY"),
        SecretRef::OsCredential {
            service: "glasshouse".to_owned(),
            account: "OPENROUTER_API_KEY".to_owned(),
        }
    );
}
