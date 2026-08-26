//! The operating system's own credential store, and the labelled fallback
//! for when there isn't one (Phase 9E, lines 1 and 2).
//!
//! # Two stores, one trait, and a third that composes them
//!
//! - [`NativeSecretStore`] is the OS store itself. On macOS it is the
//!   Keychain, reached through `keyring`'s `apple-native` backend. On every
//!   other platform [`NativeSecretStore::detect`] answers
//!   [`Unavailable::UnsupportedPlatform`] and no instance can be built at
//!   all — see "Why only macOS" below.
//! - [`super::EnvironmentSecretStore`] is the cross-platform source that
//!   already existed, unchanged.
//! - [`PreferNativeSecretStore`] is what Glasshouse actually runs with: the
//!   native store first, the environment second, and a
//!   [`SecretStore::describe`] that says *which arrangement is in force* so
//!   a user never has to guess whether their key is in the Keychain or in a
//!   shell profile.
//!
//! # Why only macOS
//!
//! `keyring` 3.x resolves `keyring::default` to its **mock** store when no
//! backend feature is enabled for the target. The mock store accepts a
//! credential, hands it back within the same process, and persists nothing.
//! A build that linked it would report a working secure store and silently
//! lose every credential written to it — precisely the silent degradation
//! line 2 forbids. So the dependency itself is declared for
//! `cfg(target_os = "macos")` only (see `crates/glasshouse/Cargo.toml`), the
//! mock can never be linked, and the platforms whose stores this phase
//! cannot *prove* — Windows Credential Manager needs a real user session,
//! the Secret Service needs a session bus — say so out loud instead of
//! pretending.
//!
//! # A reference names a credential; the store decides where it lives
//!
//! [`SecretRef::Environment`] does not mean "this value must come from the
//! environment". It names a credential *by the variable a harness expects to
//! receive it in*, which is the only name Glasshouse has for a provider
//! credential anywhere. A store is free to answer that name from wherever it
//! keeps credentials, and that is exactly what line 1's "prefer the macOS
//! Keychain" is: the same reference, answered from the Keychain first. This
//! is why preferring the Keychain needed no change to
//! [`crate::profile::resolve`] or to the gateway — both already ask a store
//! rather than reading the environment themselves.
//!
//! [`SecretRef::OsCredential`] is the explicit form, for a credential filed
//! under a service and account that is not derived from a variable name. It
//! is still names only, and still safe to store in configuration and to
//! print.
//!
//! # What is never carried out of here
//!
//! `keyring::Error::BadEncoding` carries the **raw bytes** of whatever was
//! in the store, and `keyring::Error::Ambiguous` carries credential
//! handles. Neither is ever wrapped, formatted or re-raised:
//! `classify` reduces every `keyring::Error` to a fixed `&'static str`
//! chosen by variant alone, so no byte that came out of the store can reach
//! a message, a log or a `Debug`. See
//! `tests::a_store_error_never_carries_anything_the_store_returned`.

use super::{EnvironmentSecretStore, Secret, SecretRef, SecretStore};

/// The service name every Glasshouse-managed credential is filed under.
///
/// One fixed service, with the credential's own variable name as the
/// account, so the OS store mirrors the environment's namespace exactly:
/// one name, one value, and a user can find the item with
/// `security find-generic-password -s glasshouse -a <VARIABLE>`.
pub const SERVICE: &str = "glasshouse";

/// The account [`NativeSecretStore::detect`] probes with. Never written, so
/// probing it reads nothing and — on macOS, where reading a missing generic
/// password is answered from the keychain database without consulting an
/// item's access control list — prompts for nothing.
const PROBE_ACCOUNT: &str = "glasshouse-availability-probe";

/// The reference Glasshouse files the credential for `var` under.
///
/// A *name*, like every [`SecretRef`]: this function reads nothing and
/// resolves nothing. It exists so that configuration, the Settings overlay
/// and this module all derive the same service/account pair from the same
/// place rather than three of them agreeing by accident.
pub fn os_credential_for_variable(var: &str) -> SecretRef {
    SecretRef::OsCredential {
        service: SERVICE.to_owned(),
        account: var.to_owned(),
    }
}

/// Why the native store is not answering.
///
/// Two reasons, deliberately distinguished, because they call for different
/// things from the user: one is "this platform has no store Glasshouse can
/// use", which nothing the user does will change, and the other is "there is
/// one and it would not open", which unlocking a keychain might.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unavailable {
    /// No backend is compiled for this target. Phase 9E ships macOS only —
    /// see "Why only macOS".
    UnsupportedPlatform,
    /// A backend exists and refused a probe: a locked or missing keychain,
    /// or a session with no access to one.
    StoreUnreachable,
}

impl Unavailable {
    /// A short reason, for a diagnostic line.
    pub fn reason(self) -> &'static str {
        match self {
            Self::UnsupportedPlatform => "this platform has no secure store Glasshouse can use yet",
            Self::StoreUnreachable => "the native secure store could not be opened",
        }
    }
}

/// What [`NativeSecretStore::delete`] found.
///
/// Deleting a credential that is not there is **not** an error: it is
/// already the desired state, and the caller still deserves to know which
/// of the two happened, so this is reported rather than raised.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Deletion {
    /// There was an item, and it is gone now.
    Removed,
    /// There was nothing to remove.
    AlreadyAbsent,
}

/// Why a write to or a deletion from the native store did not happen.
///
/// Every variant carries **names and fixed text only**. The store's own
/// error is reduced to a `&'static str` by `classify` before it gets
/// anywhere near this type, so nothing that came out of the store can be
/// carried by one — see the module documentation.
#[derive(Debug, thiserror::Error)]
pub enum NativeStoreError {
    #[error("no native secure store is available: {}", .0.reason())]
    Unavailable(Unavailable),

    #[error(
        "the native secure store refused the credential filed as `{service}`/`{account}`: {reason}"
    )]
    Refused {
        service: String,
        account: String,
        /// Fixed text chosen by the store error's *variant*. Never the
        /// store's own message, and never any data it returned.
        reason: &'static str,
    },
}

/// The operating system's own credential store.
///
/// Only constructible through [`NativeSecretStore::detect`], and only on a
/// platform whose store this phase can prove, so holding one is itself the
/// evidence that a real secure store answered a probe.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NativeSecretStore {
    /// Deliberately unconstructible from outside: see the type's own doc.
    _probed: (),
}

impl NativeSecretStore {
    /// Probe the platform's store, and hand back a handle only if it
    /// answered.
    ///
    /// The probe reads an account that is never written, so the answer is
    /// "the store is reachable" and nothing else: no credential is read, and
    /// [`keyring::Error::NoEntry`] — the expected outcome — counts as
    /// success.
    pub fn detect() -> Result<Self, Unavailable> {
        backend::probe()?;
        Ok(Self { _probed: () })
    }

    /// Put `value` in the native store under `reference`, replacing whatever
    /// was there.
    ///
    /// `value` is a `&str` rather than a [`Secret`] because a [`Secret`] is
    /// what comes *out* of a store; the value going in is text the user just
    /// typed, and it is borrowed for exactly the length of this call.
    pub fn store(&self, reference: &SecretRef, value: &str) -> Result<(), NativeStoreError> {
        let (service, account) = entry_name(reference);
        backend::set(service, account, value).map_err(|reason| NativeStoreError::Refused {
            service: service.to_owned(),
            account: account.to_owned(),
            reason,
        })
    }

    /// Remove the credential `reference` names.
    ///
    /// Absent is [`Deletion::AlreadyAbsent`], not an error — see
    /// [`Deletion`].
    pub fn delete(&self, reference: &SecretRef) -> Result<Deletion, NativeStoreError> {
        let (service, account) = entry_name(reference);
        backend::delete(service, account).map_err(|reason| NativeStoreError::Refused {
            service: service.to_owned(),
            account: account.to_owned(),
            reason,
        })
    }
}

impl SecretStore for NativeSecretStore {
    fn resolve(&self, reference: &SecretRef) -> Option<Secret> {
        let (service, account) = entry_name(reference);
        backend::get(service, account).map(Secret)
    }

    /// Answered without the value ever entering this process.
    ///
    /// Unlike [`super::EnvironmentSecretStore::is_present`], which has
    /// `var_os`'s `Option` to read, the macOS Keychain has **no
    /// existence-only query**: `SecKeychainFindGenericPassword` returns the
    /// data. `keyring`'s `get_attributes` performs that same lookup for
    /// effect and returns the item's attributes — an empty map on macOS —
    /// so the store reads the item and Glasshouse still never receives its
    /// value. That is the closest this platform allows, and saying so is
    /// better than implying a check that does not exist.
    fn is_present(&self, reference: &SecretRef) -> bool {
        let (service, account) = entry_name(reference);
        backend::exists(service, account)
    }

    fn describe(&self) -> &'static str {
        backend::LABEL
    }
}

/// The store Glasshouse runs with: the native one first, the environment
/// second, and a description that says which of those two arrangements is
/// actually in force.
///
/// The fallback is **labelled, never silent**. [`SecretStore::describe`]
/// answers one of three fixed strings, and `glasshouse doctor` prints it, so
/// "is my key in the Keychain or in my shell profile" is a question a user
/// can read the answer to rather than infer.
#[derive(Debug)]
pub struct PreferNativeSecretStore {
    native: Result<NativeSecretStore, Unavailable>,
    environment: EnvironmentSecretStore,
}

/// [`SecretStore::describe`] with a native store answering first.
pub const NATIVE_FIRST_LABEL: &str = "the macOS Keychain, then the process environment";

/// [`SecretStore::describe`] on a platform with no store Glasshouse can use.
pub const UNSUPPORTED_PLATFORM_LABEL: &str =
    "the process environment (this platform has no secure store Glasshouse can use yet)";

/// [`SecretStore::describe`] when a store exists but would not open.
pub const STORE_UNREACHABLE_LABEL: &str =
    "the process environment (the native secure store could not be opened)";

impl Default for PreferNativeSecretStore {
    fn default() -> Self {
        Self::detect()
    }
}

impl PreferNativeSecretStore {
    /// Probe once, here, rather than on every resolution: a store that was
    /// reachable at startup is the fact a launch and a diagnostic both want,
    /// and probing per credential would turn one keychain round trip into
    /// one per variable per frame.
    pub fn detect() -> Self {
        Self {
            native: NativeSecretStore::detect(),
            environment: EnvironmentSecretStore::new(),
        }
    }

    /// The native store, when one answered — the handle
    /// [`NativeSecretStore::store`] and [`NativeSecretStore::delete`] need.
    pub fn native(&self) -> Result<&NativeSecretStore, Unavailable> {
        self.native.as_ref().map_err(|err| *err)
    }

    /// Which of this store's two sources answers `reference` right now, as
    /// a short label for a diagnostic. `None` when neither does.
    ///
    /// Per credential, unlike [`SecretStore::describe`], which describes the
    /// arrangement. A user with one key in the Keychain and another in a
    /// shell profile is told that, one line each.
    pub fn source_of(&self, reference: &SecretRef) -> Option<&'static str> {
        if let Ok(native) = &self.native
            && native.is_present(reference)
        {
            return Some(backend::LABEL);
        }
        if self.environment.is_present(reference) {
            return Some(self.environment.describe());
        }
        None
    }
}

impl SecretStore for PreferNativeSecretStore {
    /// The native store first, the environment second.
    ///
    /// "Prefer" is the whole of line 1: a credential the user stored in the
    /// Keychain wins over one that happens to be exported in the shell that
    /// launched Glasshouse, because the stored one is the one they chose
    /// deliberately.
    fn resolve(&self, reference: &SecretRef) -> Option<Secret> {
        if let Ok(native) = &self.native
            && let Some(secret) = native.resolve(reference)
        {
            return Some(secret);
        }
        self.environment.resolve(reference)
    }

    fn is_present(&self, reference: &SecretRef) -> bool {
        if let Ok(native) = &self.native
            && native.is_present(reference)
        {
            return true;
        }
        self.environment.is_present(reference)
    }

    fn describe(&self) -> &'static str {
        match self.native {
            Ok(_) => NATIVE_FIRST_LABEL,
            Err(Unavailable::UnsupportedPlatform) => UNSUPPORTED_PLATFORM_LABEL,
            Err(Unavailable::StoreUnreachable) => STORE_UNREACHABLE_LABEL,
        }
    }
}

/// The service and account a reference is filed under.
///
/// An [`SecretRef::Environment`] reference maps to the fixed [`SERVICE`]
/// with the variable's own name as the account — see the module
/// documentation on why a reference names a credential rather than a
/// location.
fn entry_name(reference: &SecretRef) -> (&str, &str) {
    match reference {
        SecretRef::Environment { var } => (SERVICE, var.as_str()),
        SecretRef::OsCredential { service, account } => (service.as_str(), account.as_str()),
    }
}

#[cfg(target_os = "macos")]
mod backend {
    //! The macOS Keychain, through `keyring`'s `apple-native` backend.

    pub const LABEL: &str = "the macOS Keychain";

    use super::{Deletion, PROBE_ACCOUNT, SERVICE, Unavailable};

    /// Turn off the Keychain's authorization dialogs for this process,
    /// once, before any Keychain call is made.
    ///
    /// # Why this is not optional
    ///
    /// `SecKeychainFindGenericPassword` decrypts the item, and decryption
    /// consults the item's access control list. For an item **this binary
    /// did not create** — one added with `security add-generic-password`,
    /// or written by a different build — the list does not name it, and the
    /// call blocks in `SecurityServer::ClientSession::decrypt` until a user
    /// answers a dialog. Found by running `glasshouse doctor` against
    /// exactly that item: it hung, and a stack sample put it in that call.
    ///
    /// A hang there is not a cosmetic problem. `doctor` is a
    /// non-interactive command that is piped into files, and the same read
    /// happens on the path that starts a session, where it would freeze the
    /// TUI with no dialog visible behind it. So interaction is disabled and
    /// the call fails cleanly instead: the store answers "no", resolution
    /// falls back to the environment, and
    /// [`super::PreferNativeSecretStore::describe`] says so. **Refusing
    /// plainly is the whole of line 2; blocking forever is not a fallback at
    /// all.**
    ///
    /// The cost, stated honestly: a credential a user filed by hand with the
    /// `security` CLI is not read, where before this it would have been read
    /// after a prompt. The supported way to put one where Glasshouse can
    /// read it is to store it *through* Glasshouse, which puts this binary
    /// on the item's access control list and needs no dialog thereafter.
    ///
    /// Declared here rather than by taking a dependency on
    /// `security-framework`: the framework is already linked by the one
    /// `keyring` pulls in, and the dependency for this batch was settled as
    /// `keyring` alone. `Boolean` is a `u8` and `OSStatus` an `i32`; the
    /// status is deliberately ignored, because a platform that refuses to
    /// turn interaction off is one whose reads will simply fail below.
    fn silence_authorization_dialogs() {
        #[link(name = "Security", kind = "framework")]
        unsafe extern "C" {
            fn SecKeychainSetUserInteractionAllowed(state: u8) -> i32;
        }

        static ONCE: std::sync::Once = std::sync::Once::new();
        ONCE.call_once(|| {
            // SAFETY: a C call taking one scalar and returning one, with no
            // pointers and no memory to own. It is process-global state,
            // which is why it is set exactly once, before the first
            // Keychain call any `NativeSecretStore` can make — nothing else
            // in Glasshouse touches the Keychain.
            unsafe {
                SecKeychainSetUserInteractionAllowed(0);
            }
        });
    }

    /// Every `keyring::Error` reduced to fixed text chosen by **variant
    /// alone**.
    ///
    /// `BadEncoding` carries the raw bytes the store returned and
    /// `Ambiguous` carries credential handles; neither the payloads nor the
    /// crate's own `Display` are used, so nothing that came out of the store
    /// can reach a Glasshouse message.
    fn classify(err: &keyring::Error) -> &'static str {
        match err {
            keyring::Error::PlatformFailure(_) => "the platform's secure storage failed",
            keyring::Error::NoStorageAccess(_) => {
                "the secure store would not grant access; it may be locked"
            }
            keyring::Error::NoEntry => "there is no such credential",
            keyring::Error::BadEncoding(_) => "the stored credential is not valid UTF-8",
            keyring::Error::TooLong(_, _) => "a name exceeded the store's length limit",
            keyring::Error::Invalid(_, _) => "a name is not valid for this store",
            keyring::Error::Ambiguous(_) => "more than one stored credential matches this name",
            _ => "the secure store reported an error",
        }
    }

    fn entry(service: &str, account: &str) -> Result<keyring::Entry, &'static str> {
        keyring::Entry::new(service, account).map_err(|err| classify(&err))
    }

    pub fn probe() -> Result<(), Unavailable> {
        // Every path into this module runs through `NativeSecretStore::detect`,
        // which runs through here, so this is the one place that has to
        // remember.
        silence_authorization_dialogs();
        let entry = keyring::Entry::new(SERVICE, PROBE_ACCOUNT)
            .map_err(|_| Unavailable::StoreUnreachable)?;
        match entry.get_attributes() {
            Ok(_) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(_) => Err(Unavailable::StoreUnreachable),
        }
    }

    pub fn get(service: &str, account: &str) -> Option<String> {
        entry(service, account).ok()?.get_password().ok()
    }

    /// The item is read for effect and its **attributes** are returned —
    /// an empty map on macOS — so the value never enters this process. See
    /// `NativeSecretStore::is_present`.
    pub fn exists(service: &str, account: &str) -> bool {
        entry(service, account)
            .ok()
            .is_some_and(|entry| entry.get_attributes().is_ok())
    }

    pub fn set(service: &str, account: &str, value: &str) -> Result<(), &'static str> {
        entry(service, account)?
            .set_password(value)
            .map_err(|err| classify(&err))
    }

    pub fn delete(service: &str, account: &str) -> Result<Deletion, &'static str> {
        match entry(service, account)?.delete_credential() {
            Ok(()) => Ok(Deletion::Removed),
            Err(keyring::Error::NoEntry) => Ok(Deletion::AlreadyAbsent),
            Err(err) => Err(classify(&err)),
        }
    }
}

#[cfg(not(target_os = "macos"))]
mod backend {
    //! No store this phase can prove. Nothing here pretends otherwise, and
    //! `keyring` is not even a dependency on these targets — see the module
    //! documentation's "Why only macOS".

    use super::{Deletion, Unavailable};

    pub const LABEL: &str = "no native secure store";

    pub fn probe() -> Result<(), Unavailable> {
        Err(Unavailable::UnsupportedPlatform)
    }

    /// Unreachable in practice: `probe` refuses, so no
    /// [`super::NativeSecretStore`] exists on this target to call it. Kept
    /// total rather than `unreachable!()` so that a future backend is added
    /// by writing one, not by removing a panic.
    pub fn get(_service: &str, _account: &str) -> Option<String> {
        None
    }

    pub fn exists(_service: &str, _account: &str) -> bool {
        false
    }

    pub fn set(_service: &str, _account: &str, _value: &str) -> Result<(), &'static str> {
        Err("this platform has no secure store Glasshouse can use yet")
    }

    pub fn delete(_service: &str, _account: &str) -> Result<Deletion, &'static str> {
        Err("this platform has no secure store Glasshouse can use yet")
    }
}

/// A store with no native half, for exercising the labelled-fallback path on
/// a machine that *does* have a Keychain.
///
/// `#[cfg(test)]` and `pub(crate)`: it does not exist in a release build and
/// it is not API, so the production boundary is exactly as narrow as it was.
/// Placed immediately above the test module, so the source scans below —
/// which read everything before the first `#[cfg(test)]` as production code
/// — still cover the whole of it.
#[cfg(test)]
impl PreferNativeSecretStore {
    pub(crate) fn without_native(reason: Unavailable) -> Self {
        Self {
            native: Err(reason),
            environment: EnvironmentSecretStore::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// This module's own source with its `#[cfg(test)]` block excluded and
    /// `//` comments stripped — the same idiom as `super`'s own
    /// `production_code`.
    fn production_code(source: &str) -> String {
        source
            .split("#[cfg(test)]")
            .next()
            .expect("split always yields at least one part")
            .lines()
            .filter(|line| !line.trim_start().starts_with("//"))
            .collect::<Vec<_>>()
            .join("\n")
    }

    // --- what this module must never do ----------------------------------

    /// The same guarantee `secret::nothing_in_this_module_writes_to_disk`
    /// makes for the parent, extended to the child that actually handles
    /// credential values: a module that never opens a file cannot leak one
    /// into a file.
    #[test]
    fn nothing_in_this_module_writes_to_disk() {
        let code = production_code(include_str!("native.rs"));
        for forbidden in ["std::fs", "fs::", "File::", "OpenOptions"] {
            assert!(
                !code.contains(forbidden),
                "secret/native.rs names `{forbidden}` in production code: no credential value \
                 may be written to disk by any path here"
            );
        }
    }

    /// A `Serialize` reachable from here would put a credential into
    /// whatever the serializer writes. The serialised *reference* shape
    /// lives in `crate::config`, which is the point: names are serialised
    /// there, values are handled here, and the two files do not overlap.
    #[test]
    fn nothing_in_this_module_is_serializable() {
        let code = production_code(include_str!("native.rs"));
        for forbidden in ["Serialize", "Deserialize", "serde"] {
            assert!(
                !code.contains(forbidden),
                "secret/native.rs names `{forbidden}` in production code: nothing that touches \
                 a credential value may be serialized"
            );
        }
    }

    /// The scans above are only worth having if they can fail.
    #[test]
    fn the_source_scans_would_catch_a_violation() {
        let writing = "fn save(v: &str) {\n    std::fs::write(\"k\", v).unwrap();\n}";
        assert!(production_code(writing).contains("std::fs"));
        let derived = "#[derive(Serialize)]\nstruct Leak(String);";
        assert!(production_code(derived).contains("Serialize"));
        // ... and neither fires on a doc comment that merely mentions one.
        let documented = "/// Nothing here uses `std::fs` or `Serialize`.\nfn f() {}";
        assert!(!production_code(documented).contains("std::fs"));
        assert!(!production_code(documented).contains("Serialize"));
        // ... nor on test code.
        let tested = "fn f() {}\n#[cfg(test)]\nmod tests { use std::fs; }";
        assert!(!production_code(tested).contains("std::fs"));
    }

    // --- a reference is still only names ---------------------------------

    #[test]
    fn an_os_credential_reference_is_two_names_and_is_safe_to_print() {
        let reference = os_credential_for_variable("OPENROUTER_API_KEY");
        assert_eq!(
            reference,
            SecretRef::OsCredential {
                service: SERVICE.to_owned(),
                account: "OPENROUTER_API_KEY".to_owned(),
            }
        );

        let rendered = format!("{reference:?}");
        assert!(rendered.contains("OPENROUTER_API_KEY"), "got {rendered}");
        assert!(rendered.contains(SERVICE), "got {rendered}");
    }

    /// The mapping the whole "prefer the Keychain" behaviour rests on: an
    /// `Environment` reference and the `OsCredential` reference derived from
    /// the same variable name address the *same* item, so a credential
    /// stored through Settings is the one `crate::profile::resolve` finds.
    #[test]
    fn an_environment_reference_and_its_os_credential_name_the_same_item() {
        let var = "GLASSHOUSE_NATIVE_TEST_ONLY_VAR";
        assert_eq!(
            entry_name(&SecretRef::Environment {
                var: var.to_owned()
            }),
            entry_name(&os_credential_for_variable(var)),
        );
    }

    // --- line 2: the fallback is labelled, never silent -------------------

    /// Acceptance 2. With no native store, resolution falls back to the
    /// environment and `describe` says so — asserted on the label itself,
    /// not on the mere fact that a value came back.
    #[test]
    fn with_no_native_store_resolution_falls_back_and_describe_says_which_answered() {
        const VAR: &str = "GLASSHOUSE_NATIVE_TEST_ONLY_FALLBACK_VAR";
        const VALUE: &str = "sk-fallback-test-0123456789abcdefghij";

        // SAFETY: `VAR` is unique to this test and removed again below.
        // This test has no early return between the two.
        unsafe {
            std::env::set_var(VAR, VALUE);
        }

        let store = PreferNativeSecretStore::without_native(Unavailable::UnsupportedPlatform);
        let reference = SecretRef::Environment {
            var: VAR.to_owned(),
        };

        let resolved = store.resolve(&reference).is_some();
        let present = store.is_present(&reference);
        let describe = store.describe();
        let source = store.source_of(&reference);
        let unreachable_label =
            PreferNativeSecretStore::without_native(Unavailable::StoreUnreachable).describe();

        unsafe {
            std::env::remove_var(VAR);
        }

        assert!(resolved, "the environment must still answer");
        assert!(present);
        assert_eq!(describe, UNSUPPORTED_PLATFORM_LABEL);
        assert!(
            describe.contains("process environment"),
            "the label must name the source that answered: {describe}"
        );
        assert_eq!(source, Some(EnvironmentSecretStore::new().describe()));

        // The two unavailable reasons are told apart, because one is
        // something a user can act on and the other is not.
        assert_eq!(unreachable_label, STORE_UNREACHABLE_LABEL);
        assert_ne!(unreachable_label, UNSUPPORTED_PLATFORM_LABEL);
    }

    /// An `OsCredential` reference against a store with no native half
    /// resolves to nothing rather than to some environment variable guessed
    /// from its account name.
    #[test]
    fn with_no_native_store_an_os_credential_reference_resolves_to_nothing() {
        const ACCOUNT: &str = "GLASSHOUSE_NATIVE_TEST_ONLY_ACCOUNT_VAR";

        // SAFETY: `ACCOUNT` is unique to this test and removed again below.
        unsafe {
            std::env::set_var(ACCOUNT, "sk-must-not-be-found-by-an-os-reference");
        }

        let store = PreferNativeSecretStore::without_native(Unavailable::UnsupportedPlatform);
        let resolved = store
            .resolve(&os_credential_for_variable(ACCOUNT))
            .is_some();
        let present = store.is_present(&os_credential_for_variable(ACCOUNT));

        unsafe {
            std::env::remove_var(ACCOUNT);
        }

        assert!(!resolved, "an OS reference must not be answered from env");
        assert!(!present);
    }

    #[test]
    fn every_unavailable_reason_has_text_and_they_differ() {
        assert_ne!(
            Unavailable::UnsupportedPlatform.reason(),
            Unavailable::StoreUnreachable.reason()
        );
        for reason in [
            Unavailable::UnsupportedPlatform,
            Unavailable::StoreUnreachable,
        ] {
            assert!(!reason.reason().is_empty());
        }
    }

    // --- nothing the store returns is ever carried out --------------------

    /// `keyring::Error::BadEncoding` carries the raw bytes the store
    /// returned. This proves the error type Glasshouse raises cannot carry
    /// them: it is built from a `&'static str` chosen by variant, so there
    /// is nowhere for a byte to go.
    #[test]
    fn a_store_error_never_carries_anything_the_store_returned() {
        const PLANTED: &str = "sk-planted-into-a-store-error-0123456789";

        let err = NativeStoreError::Refused {
            service: SERVICE.to_owned(),
            account: "OPENROUTER_API_KEY".to_owned(),
            reason: "the stored credential is not valid UTF-8",
        };
        let rendered = format!("{err} / {err:?}");
        assert!(!rendered.contains(PLANTED), "got {rendered}");
        assert!(rendered.contains("OPENROUTER_API_KEY"), "got {rendered}");

        let unavailable = NativeStoreError::Unavailable(Unavailable::StoreUnreachable);
        let rendered = format!("{unavailable} / {unavailable:?}");
        assert!(!rendered.contains(PLANTED), "got {rendered}");
        assert!(rendered.contains("could not be opened"), "got {rendered}");

        // The `reason` field is typed `&'static str`, so no value read out
        // of a store — which is never `'static` — can be put in it. This is
        // the structural half of the guarantee.
        let code = production_code(include_str!("native.rs"));
        assert!(
            code.contains("reason: &'static str"),
            "`NativeStoreError::Refused::reason` must stay `&'static str`: that type is what \
             makes it impossible to carry a value the store returned"
        );
    }

    /// Every store type's `Debug` is names only, with a credential planted
    /// in the environment the fallback reads from.
    #[test]
    fn no_store_debug_renders_a_credential() {
        const VAR: &str = "GLASSHOUSE_NATIVE_TEST_ONLY_DEBUG_VAR";
        const VALUE: &str = "sk-debug-render-test-abcdefghijklmnop";

        // SAFETY: `VAR` is unique to this test and removed again below.
        unsafe {
            std::env::set_var(VAR, VALUE);
        }

        let reference = SecretRef::Environment {
            var: VAR.to_owned(),
        };
        let rendered = format!(
            "{:?} {:?} {:?} {:?} {:?}",
            PreferNativeSecretStore::detect(),
            PreferNativeSecretStore::without_native(Unavailable::UnsupportedPlatform),
            reference,
            os_credential_for_variable(VAR),
            (Deletion::Removed, Deletion::AlreadyAbsent),
        );

        unsafe {
            std::env::remove_var(VAR);
        }

        assert!(
            !rendered.contains(VALUE),
            "a credential was rendered: {rendered}"
        );
        assert!(rendered.contains(VAR), "the NAME must survive: {rendered}");
    }

    // --- the macOS Keychain itself ---------------------------------------

    /// A unique account per process, so a leftover item from an earlier run
    /// can never be read by a later one — which on macOS would mean a
    /// different binary reading an item whose access control list does not
    /// name it, and therefore an interactive prompt in the middle of a test
    /// run.
    #[cfg(target_os = "macos")]
    fn test_account(suffix: &str) -> String {
        format!(
            "GLASSHOUSE_KEYCHAIN_TEST_ONLY_{suffix}_{}",
            std::process::id()
        )
    }

    /// Removes its Keychain item when it goes out of scope, however the
    /// test leaves — a passing assertion, a failing one, or a panic.
    ///
    /// Written after a mutation run left `glasshouse-availability-probe` in
    /// the developer's own login keychain: a test that deletes on its last
    /// line deletes nothing when an assertion above that line fires, and
    /// what it leaves behind is in a real user's real keychain.
    #[cfg(target_os = "macos")]
    struct KeychainItem {
        reference: SecretRef,
    }

    #[cfg(target_os = "macos")]
    impl KeychainItem {
        /// Store `value` under `reference` and take responsibility for
        /// removing it again.
        fn stored(store: &NativeSecretStore, reference: SecretRef, value: &str) -> Self {
            store.store(&reference, value).expect("store");
            Self { reference }
        }
    }

    #[cfg(target_os = "macos")]
    impl Drop for KeychainItem {
        fn drop(&mut self) {
            // Best effort by construction: this runs while a panic may be
            // unwinding, and a second panic from here would replace the
            // assertion message that explains the failure with an abort.
            if let Ok(store) = NativeSecretStore::detect() {
                let _ = store.delete(&self.reference);
            }
        }
    }

    /// Whether the `security` CLI can see the item — an independent witness
    /// that the credential really is in the login keychain, rather than
    /// `keyring` agreeing with itself.
    ///
    /// `-w` is deliberately NOT passed: that flag prints the password. This
    /// asks only whether the item exists.
    #[cfg(target_os = "macos")]
    fn security_cli_sees(account: &str) -> bool {
        std::process::Command::new("security")
            .args(["find-generic-password", "-s", SERVICE, "-a", account])
            .output()
            .map(|out| out.status.success())
            .unwrap_or(false)
    }

    /// **Acceptance 1, macOS-gated — and this is the only test in this
    /// batch that is.** Windows Credential Manager and the Secret Service
    /// are deliberately not implemented and deliberately not claimed; macOS
    /// is the platform whose store this machine can actually prove.
    ///
    /// Skipped, loudly, when the Keychain will not open — a CI runner with
    /// no login keychain is a real state, and a test that pretends to have
    /// proven something there would be worse than one that says it could
    /// not.
    #[cfg(target_os = "macos")]
    #[test]
    fn macos_only_a_credential_stored_in_the_keychain_resolves_and_deletes() {
        const VALUE: &str = "sk-keychain-round-trip-test-0123456789abcdef";

        let Ok(store) = NativeSecretStore::detect() else {
            eprintln!(
                "SKIPPED: the macOS Keychain would not open in this session, so this test \
                 proved nothing"
            );
            return;
        };

        let account = test_account("ROUNDTRIP");
        let reference = os_credential_for_variable(&account);

        let item = KeychainItem::stored(&store, reference.clone(), VALUE);
        assert!(
            security_cli_sees(&account),
            "the `security` CLI must see the item that was just stored"
        );

        assert!(store.is_present(&reference));
        let resolved = store.resolve(&reference).expect("resolve");
        assert!(
            resolved.expose() == VALUE,
            "the stored value must come back unchanged"
        );

        // The same credential, addressed the way `crate::profile::resolve`
        // addresses one: by the variable name a harness expects it in.
        let as_variable = SecretRef::Environment {
            var: account.clone(),
        };
        let via_variable = store.resolve(&as_variable).expect("resolve by variable");
        assert!(via_variable.expose() == VALUE);

        // Acceptance 3, first half.
        assert_eq!(store.delete(&reference).expect("delete"), Deletion::Removed);
        assert!(!store.is_present(&reference));
        assert!(store.resolve(&reference).is_none());
        assert!(
            !security_cli_sees(&account),
            "the `security` CLI must no longer see the item after deletion"
        );

        // Acceptance 3, second half: deleting what is already gone is the
        // desired state, reported rather than raised.
        assert_eq!(
            store
                .delete(&reference)
                .expect("a second delete is not an error"),
            Deletion::AlreadyAbsent
        );

        // The guard has nothing left to remove, which is the point: a third
        // delete on the way out is still not an error.
        drop(item);
    }

    /// Line 1: a credential in the Keychain beats one exported in the shell
    /// that launched Glasshouse, because the stored one was chosen
    /// deliberately. Both are planted at once so the test cannot pass by the
    /// environment simply being empty.
    #[cfg(target_os = "macos")]
    #[test]
    fn macos_only_the_keychain_is_preferred_over_the_environment() {
        const STORED: &str = "sk-from-the-keychain-0123456789abcdefghij";
        const EXPORTED: &str = "sk-from-the-environment-abcdefghij0123456";

        let Ok(native) = NativeSecretStore::detect() else {
            eprintln!("SKIPPED: the macOS Keychain would not open in this session");
            return;
        };

        let account = test_account("PREFERENCE");
        let reference = SecretRef::Environment {
            var: account.clone(),
        };
        let _item = KeychainItem::stored(&native, reference.clone(), STORED);

        // SAFETY: the variable name is unique to this process and is
        // removed below, before every assertion that could fail.
        unsafe {
            std::env::set_var(&account, EXPORTED);
        }

        let store = PreferNativeSecretStore::detect();
        let resolved = store.resolve(&reference).map(|s| s.expose().to_owned());
        let describe = store.describe();
        let source = store.source_of(&reference);

        unsafe {
            std::env::remove_var(&account);
        }

        assert!(resolved.as_deref() == Some(STORED), "the Keychain must win");
        assert_eq!(describe, NATIVE_FIRST_LABEL);
        assert_eq!(source, Some("the macOS Keychain"));
    }

    /// With the Keychain holding nothing for a name, the environment still
    /// answers it — the fallback half of the same store, on a machine where
    /// the native half is genuinely available.
    #[cfg(target_os = "macos")]
    #[test]
    fn macos_only_the_environment_answers_what_the_keychain_does_not_hold() {
        const EXPORTED: &str = "sk-only-in-the-environment-0123456789abcd";

        let account = test_account("ENVONLY");

        // SAFETY: the variable name is unique to this process and removed
        // below, before every assertion.
        unsafe {
            std::env::set_var(&account, EXPORTED);
        }

        let store = PreferNativeSecretStore::detect();
        let reference = SecretRef::Environment {
            var: account.clone(),
        };
        let resolved = store.resolve(&reference).map(|s| s.expose().to_owned());
        let source = store.source_of(&reference);

        unsafe {
            std::env::remove_var(&account);
        }

        assert!(resolved.as_deref() == Some(EXPORTED));
        assert_eq!(source, Some("process environment"));
    }

    /// **Acceptance 1, end to end and macOS-gated.** A credential stored in
    /// the Keychain resolves through a [`SecretStore`] and reaches a launch
    /// overlay's environment — the actual thing a user is trying to do, not
    /// just a round trip through this module.
    ///
    /// Goes through `crate::profile::resolve` unchanged: that function asks
    /// a store with a [`SecretRef::Environment`] reference and knows nothing
    /// about keychains, which is exactly the point of "a reference names a
    /// credential; the store decides where it lives".
    #[cfg(target_os = "macos")]
    #[test]
    fn macos_only_a_keychain_credential_reaches_a_launch_overlays_environment() {
        use crate::profile::{BackendResource, LaunchProfile, Resolution, resolve};

        const STORED: &str = "sk-into-a-launch-overlay-0123456789abcdef";

        let Ok(native) = NativeSecretStore::detect() else {
            eprintln!("SKIPPED: the macOS Keychain would not open in this session");
            return;
        };

        let account = test_account("OVERLAY");
        let reference = SecretRef::Environment {
            var: account.clone(),
        };
        let _item = KeychainItem::stored(&native, reference.clone(), STORED);

        // The mapping itself, witnessed from outside Glasshouse. Storing and
        // reading through the same `entry_name` would agree with each other
        // whatever that function did; the `security` CLI agrees with nothing
        // but the Keychain, so this is what pins an `Environment` reference
        // to the service and account a user is told to look under. Found by
        // mutation: redirecting the `Environment` arm to another account
        // left every other assertion in this test passing.
        assert!(
            security_cli_sees(&account),
            "an `Environment` reference must be filed under service `{SERVICE}` with the \
             variable's own name as the account"
        );

        let mut provider =
            crate::provider::template("openrouter").expect("a built-in template exists");
        provider.name = "keychain-router".to_owned();
        provider.credential_env = vec![account.clone()];

        let adapter = crate::harness::adapter_for(crate::integrations::IntegrationId::ClaudeCode)
            .expect("claude code has an adapter");
        let mut profile = LaunchProfile::native(crate::integrations::IntegrationId::ClaudeCode);
        profile.name = "keychain".to_owned();
        profile.backend = BackendResource::DirectProvider {
            provider: provider.name.clone(),
        };

        let secrets = PreferNativeSecretStore::detect();
        let outcome = resolve(
            &profile,
            &Resolution {
                adapter,
                acknowledged_bypass: false,
                provider: Some(&provider),
                secrets: &secrets,
            },
        );

        let overlay = outcome.expect("a direct-provider profile with a resolvable credential");
        let carried = overlay
            .env()
            .iter()
            .any(|(_, value)| value.to_string_lossy() == STORED);
        assert!(
            carried,
            "the credential stored in the Keychain never reached the overlay's environment"
        );

        // ... and nothing else about the overlay carries it in a form that
        // could be printed: the mechanism notes and args are names only.
        let described = format!("{:?} {:?}", overlay.args(), overlay.mechanisms());
        assert!(
            !described.contains(STORED),
            "a credential appeared outside the overlay's environment: {described}"
        );
    }
}
