//! The operating system's own credential store, and the labelled fallback
//! for when there isn't one (Phase 9E, lines 1 and 2).
//!
//! # Two stores, one trait, and a third that composes them
//!
//! - [`NativeSecretStore`] is the OS store itself. On macOS it is the
//!   Keychain, reached through `keyring`'s `apple-native` backend; on Windows
//!   it is Credential Manager, through `windows-native`. On every other
//!   platform [`NativeSecretStore::detect`] answers
//!   [`Unavailable::UnsupportedPlatform`] and no instance can be built at
//!   all — see "Which platforms, and why not the third" below.
//! - [`super::EnvironmentSecretStore`] is the cross-platform source that
//!   already existed, unchanged.
//! - [`PreferNativeSecretStore`] is what Glasshouse actually runs with: the
//!   native store first, the environment second, and a
//!   [`SecretStore::describe`] that says *which arrangement is in force* so
//!   a user never has to guess whether their key is in the Keychain or in a
//!   shell profile.
//!
//! # The mock hazard, which shapes everything below
//!
//! `keyring` 3.x resolves `keyring::default` to its **mock** store when no
//! backend feature is enabled for the target. The mock store accepts a
//! credential, hands it back within the same process, and persists nothing.
//! A build that linked it would report a working secure store and silently
//! lose every credential written to it — precisely the silent degradation
//! line 2 forbids. The failure mode is not a build error; it is a build that
//! looks like it works.
//!
//! Three things keep it out, and none of them is a comment:
//!
//! 1. The dependency is declared **per target, always with a backend
//!    feature** (see `crates/glasshouse/Cargo.toml`), so a platform with no
//!    proven store does not link `keyring` at all.
//! 2. This module never calls `keyring::Entry::new`, which reads a
//!    process-global builder that `keyring::set_default_credential_builder`
//!    can replace with anything. It names the platform's own builder —
//!    `keyring::macos` or `keyring::windows` — and those module paths
//!    **only exist when that platform's backend feature is on**. Enabling
//!    `keyring` without a backend therefore stops compiling rather than
//!    quietly resolving to the mock.
//! 3. `tests::the_store_is_built_by_the_platforms_own_backend_and_never_the_mock`
//!    downcasts the credential this module builds and fails if it is a
//!    `keyring::mock::MockCredential`, and
//!    `tests/secret_native.rs` fails if the manifest ever declares `keyring`
//!    without a backend feature or outside a per-target section.
//!
//! # Which platforms, and why not the third
//!
//! macOS and Windows are here. **The Secret Service is deliberately not**,
//! and the reason is no longer only that a headless runner cannot prove it.
//! keyring 3.6.3's Secret Service backend reaches the bus through
//! `SecretService::connect`, leaving `dbus-secret-service`'s prompt timeout
//! unset; that crate then defaults an unanswered unlock prompt to
//! `ONE_YEAR_SECONDS` and blocks the calling thread for it. A locked
//! collection is not an error there — `keyring`'s `map_matching_items`
//! unlocks locked items before reading them, and `get_collection` unlocks a
//! locked collection before writing — so a Linux desktop with a locked
//! keyring would **hang a launch**, which is the same defect
//! `silence_authorization_dialogs` exists to prevent on macOS with no
//! equivalent available. Worse, a probe cannot see it coming: probing an
//! account that was never written matches no item, so it returns before
//! anything needs unlocking and reports the store as healthy. Until a
//! backend can refuse a prompt rather than wait for one, "prefer a Secret
//! Service-compatible keyring **when available**" cannot be honoured, and
//! saying so is the honest answer.
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
/// one name, one value, and a user can find the item themselves —
/// `security find-generic-password -s glasshouse -a <VARIABLE>` on macOS,
/// or `cmdkey /list` on Windows, where `keyring` files a generic credential
/// whose target is `<VARIABLE>.glasshouse`.
pub const SERVICE: &str = "glasshouse";

/// The account [`NativeSecretStore::detect`] probes with. Never written, so
/// probing it reads nothing and prompts for nothing: on macOS a missing
/// generic password is answered from the keychain database without
/// consulting an item's access control list, and on Windows `CredReadW`
/// answers `ERROR_NOT_FOUND` from the user's own credential set with no UI
/// at any point.
///
/// Gated to the platforms that have a backend. Only the `keyring` `backend`
/// module reads it, so on every other target it is dead code, and `-D
/// warnings` makes dead code a hard error: this constant took Linux, Windows
/// and lint red while macOS stayed green. **Anything here that only a
/// platform-gated backend uses needs the same gate as that backend** — which
/// is why this gate has to be widened in lockstep with the backend's own,
/// and not one line later.
#[cfg(any(target_os = "macos", target_os = "windows"))]
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
    /// No backend is compiled for this target — see "Which platforms, and
    /// why not the third".
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
/// platform whose store this project can prove, so holding one is itself the
/// evidence that a real secure store answered a probe. Never the mock: see
/// the module documentation's "The mock hazard".
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
    /// `keyring::Error::NoEntry` — the expected outcome — counts as success.
    ///
    /// That name is deliberately not a link. `keyring` is a macOS-only
    /// dependency (see `crates/glasshouse/Cargo.toml`), so an intra-doc link
    /// to it resolves on macOS and fails the rustdoc gate on Linux and
    /// Windows. **Anything naming a platform-gated dependency in a doc
    /// comment on an ungated item has the same problem** — the sibling rule to
    /// the one `PROBE_ACCOUNT` earned about platform-gated code.
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
    /// `var_os`'s `Option` to read, **neither native store has an
    /// existence-only query**, so both are asked for the item's
    /// *attributes* and the answer is whether that succeeded:
    ///
    /// - macOS: `SecKeychainFindGenericPassword` returns the data.
    ///   `keyring`'s `get_attributes` performs that same lookup for effect
    ///   and returns the item's attributes — an empty map there.
    /// - Windows: `CredReadW` returns the whole credential, blob included.
    ///   `keyring` copies out the comment, target alias and user name, then
    ///   zeroes the blob and frees the credential, so what it hands back is
    ///   three names.
    ///
    /// In both cases the store reads the item and Glasshouse still never
    /// receives its value. That is the closest these platforms allow, and
    /// saying so is better than implying a check that does not exist.
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
///
/// Platform-dependent, because naming the store is the whole point of the
/// label: a Windows user told "the macOS Keychain" would be told a lie in
/// the one place this phase exists to tell the truth. It is defined next to
/// the backend's own [`SecretStore::describe`] label so the two cannot
/// drift — `tests::the_native_first_label_names_this_platforms_store`
/// asserts that it starts with it.
pub const NATIVE_FIRST_LABEL: &str = backend::NATIVE_FIRST_LABEL;

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

#[cfg(any(target_os = "macos", target_os = "windows"))]
mod backend {
    //! The operating system's own credential store, through `keyring`.
    //!
    //! **One module for two platforms, because the plumbing really is the
    //! same.** `keyring` presents the macOS Keychain and Windows Credential
    //! Manager behind one `CredentialApi`, and every difference between them
    //! that Glasshouse cares about is a `const` or a one-function detail —
    //! the label, and whether the platform can raise a dialog. Writing the
    //! two out separately would duplicate `classify`, which is the piece
    //! that must not diverge: it is the only thing standing between a
    //! `keyring::Error` payload and a Glasshouse message.

    use super::{Deletion, PROBE_ACCOUNT, SERVICE, Unavailable};

    /// [`super::SecretStore::describe`] for the native store alone.
    #[cfg(target_os = "macos")]
    pub const LABEL: &str = "the macOS Keychain";
    /// [`super::PreferNativeSecretStore`]'s whole arrangement, native first.
    #[cfg(target_os = "macos")]
    pub const NATIVE_FIRST_LABEL: &str = "the macOS Keychain, then the process environment";

    /// [`super::SecretStore::describe`] for the native store alone.
    #[cfg(target_os = "windows")]
    pub const LABEL: &str = "Windows Credential Manager";
    /// [`super::PreferNativeSecretStore`]'s whole arrangement, native first.
    #[cfg(target_os = "windows")]
    pub const NATIVE_FIRST_LABEL: &str = "Windows Credential Manager, then the process environment";

    /// The platform's own credential builder, named by the path that only
    /// exists when that platform's backend feature is enabled.
    ///
    /// **This import is the compile-time half of the mock guard.**
    /// `keyring::macos` and `keyring::windows` are declared
    /// `#[cfg(all(target_os = .., feature = ..))]` inside `keyring`, so
    /// declaring the dependency without its backend feature does not
    /// silently fall through to `keyring::mock` here — it fails to resolve
    /// this path and the build stops. Going through `keyring::Entry::new`
    /// instead would read a process-global builder that
    /// `keyring::set_default_credential_builder` can replace at run time,
    /// which is a second way to end up on the mock and one no manifest rule
    /// can catch. See the module documentation.
    #[cfg(target_os = "macos")]
    use keyring::macos::default_credential_builder as platform_credential_builder;
    #[cfg(target_os = "windows")]
    use keyring::windows::default_credential_builder as platform_credential_builder;

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
    #[cfg(target_os = "macos")]
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

    /// Nothing to silence: Credential Manager cannot raise a dialog.
    ///
    /// `CredReadW`, `CredWriteW` and `CredDeleteW` are local RPC calls into
    /// LSA against the **calling user's own** credential set. They present
    /// no UI, wait for no user, and either answer or return an error code:
    /// a session with no credential store answers `ERROR_NO_SUCH_LOGON_SESSION`
    /// immediately, which `probe` turns into
    /// [`super::Unavailable::StoreUnreachable`]. There is therefore no
    /// Windows analogue of the macOS hang, and no call to make here.
    ///
    /// Kept as a same-named no-op rather than dropped, so `probe` has one
    /// shape on every platform and a future backend that *can* prompt —
    /// the Secret Service is exactly that, which is why it is not here —
    /// has an obvious place it must refuse from.
    #[cfg(target_os = "windows")]
    fn silence_authorization_dialogs() {}

    /// Every `keyring::Error` reduced to fixed text chosen by **variant
    /// alone**.
    ///
    /// `BadEncoding` carries the raw bytes the store returned and
    /// `Ambiguous` carries credential handles; neither the payloads nor the
    /// crate's own `Display` are used, so nothing that came out of the store
    /// can reach a Glasshouse message. This is shared by both platforms on
    /// purpose: it is the choke point the whole "no secret reaches a
    /// message" property rests on, and two copies of it would be two chances
    /// for one of them to start formatting an error.
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

    /// One entry, built by the platform's backend and by nothing else.
    ///
    /// See `platform_credential_builder`: this is the only place in
    /// Glasshouse that constructs a `keyring::Entry`, and it does so from a
    /// named platform builder rather than the global default.
    ///
    /// `pub(super)` so that
    /// `tests::the_store_is_built_by_the_platforms_own_backend_and_never_the_mock`
    /// can downcast what it builds, rather than a test-only wrapper here.
    /// A `cfg(test)` item anywhere above the test module would truncate
    /// `production_code`, which splits this file on the first occurrence of
    /// that attribute, and would quietly shrink what the source scans below
    /// cover — including this whole backend.
    pub(super) fn entry(service: &str, account: &str) -> Result<keyring::Entry, &'static str> {
        let credential = platform_credential_builder()
            .build(None, service, account)
            .map_err(|err| classify(&err))?;
        Ok(keyring::Entry::new_with_credential(credential))
    }

    pub fn probe() -> Result<(), Unavailable> {
        // Every path into this module runs through `NativeSecretStore::detect`,
        // which runs through here, so this is the one place that has to
        // remember.
        silence_authorization_dialogs();
        let entry = entry(SERVICE, PROBE_ACCOUNT).map_err(|_| Unavailable::StoreUnreachable)?;
        match entry.get_attributes() {
            // `NoEntry` is the expected answer and the successful one: the
            // store was reached and had nothing filed under a name nothing
            // ever writes. Anything else means it did not answer.
            Ok(_) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(_) => Err(Unavailable::StoreUnreachable),
        }
    }

    pub fn get(service: &str, account: &str) -> Option<String> {
        entry(service, account).ok()?.get_password().ok()
    }

    /// The item is read for effect and its **attributes** are returned — an
    /// empty map on macOS, three names on Windows — so the value never
    /// enters this process. See `NativeSecretStore::is_present`.
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

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
mod backend {
    //! No store this project can prove. Nothing here pretends otherwise, and
    //! `keyring` is not even a dependency on these targets — see the module
    //! documentation's "Which platforms, and why not the third".

    use super::{Deletion, Unavailable};

    pub const LABEL: &str = "no native secure store";

    /// Unreachable through [`super::PreferNativeSecretStore::describe`],
    /// whose native-first arm needs a `Result::Ok` that `probe` never
    /// produces on this target — but the arm still has to compile, so the
    /// constant still has to exist. Worded so that if it ever *did* appear
    /// it would read as the defect it would be.
    pub const NATIVE_FIRST_LABEL: &str = "no native secure store, then the process environment";

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

    // --- the backend that is linked is the platform's own ----------------

    /// **The mock guard, at run time.** `keyring` 3.x resolves
    /// `keyring::default` to an in-process mock store whenever no backend
    /// feature is enabled, and a mock accepts a credential, hands it back
    /// inside the process, persists nothing, and reports itself as a working
    /// secure store. This asks the backend to build the credential it would
    /// use and refuses to accept a mock one.
    ///
    /// Not redundant with the compile-time guard. `platform_credential_builder`
    /// makes a *missing feature* a build error; this makes a *wrong
    /// credential* a test failure, and the two catch different mistakes —
    /// the second would also catch a future path that went back through
    /// `keyring::Entry::new`, whose builder anything in the process can
    /// replace.
    ///
    /// Deliberately does not need the store to open: what is linked is a
    /// property of the build, not of the session, so this runs and means the
    /// same thing on a runner with no login keychain.
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    #[test]
    fn the_store_is_built_by_the_platforms_own_backend_and_never_the_mock() {
        let entry = backend::entry(SERVICE, PROBE_ACCOUNT).expect("the backend must name an entry");
        let credential = entry.get_credential();

        assert!(
            credential
                .downcast_ref::<keyring::mock::MockCredential>()
                .is_none(),
            "this build's secure store is `keyring`'s in-process mock: it would take a \
             provider credential, hand it back within the process, persist nothing, and \
             still describe itself as the operating system's store"
        );

        #[cfg(target_os = "macos")]
        assert!(
            credential
                .downcast_ref::<keyring::macos::MacCredential>()
                .is_some(),
            "the credential must be the Keychain's own"
        );
        #[cfg(target_os = "windows")]
        assert!(
            credential
                .downcast_ref::<keyring::windows::WinCredential>()
                .is_some(),
            "the credential must be Credential Manager's own"
        );
    }

    /// The arrangement label must name *this* platform's store. A Windows
    /// user told "the macOS Keychain" would be misinformed in the one place
    /// this line exists to inform them, and the two constants live in
    /// different places, so this is what stops them drifting apart.
    #[test]
    fn the_native_first_label_names_this_platforms_store() {
        assert!(
            NATIVE_FIRST_LABEL.starts_with(backend::LABEL),
            "`{NATIVE_FIRST_LABEL}` must begin by naming this platform's store, \
             `{}`",
            backend::LABEL
        );
        assert!(
            NATIVE_FIRST_LABEL.contains("process environment"),
            "the label must also name the fallback: {NATIVE_FIRST_LABEL}"
        );
        // All three arrangements are told apart, because a user reads this
        // string to find out where their key actually came from.
        for other in [UNSUPPORTED_PLATFORM_LABEL, STORE_UNREACHABLE_LABEL] {
            assert_ne!(NATIVE_FIRST_LABEL, other);
        }
    }

    /// `detect` offers the native store on exactly the platforms with a
    /// backend, and refuses with the *unchangeable* reason everywhere else.
    ///
    /// The negative half is the one that matters: on a target with no
    /// backend a successful `detect` would mean something was linked that
    /// this project has not proven, which is how a mock would first show up
    /// at run time.
    #[test]
    fn detect_offers_a_native_store_on_exactly_the_platforms_with_a_backend() {
        let detected = NativeSecretStore::detect();

        #[cfg(any(target_os = "macos", target_os = "windows"))]
        assert!(
            !matches!(detected, Err(Unavailable::UnsupportedPlatform)),
            "this platform has a backend, so the only honest refusal is \
             `StoreUnreachable`"
        );

        #[cfg(not(any(target_os = "macos", target_os = "windows")))]
        assert_eq!(
            detected,
            Err(Unavailable::UnsupportedPlatform),
            "no backend is compiled for this target, so nothing may hand back a store"
        );
    }

    // --- the platform's own store -----------------------------------------

    /// A unique account per process, so a leftover item from an earlier run
    /// can never be read by a later one — which on macOS would mean a
    /// different binary reading an item whose access control list does not
    /// name it, and therefore an interactive prompt in the middle of a test
    /// run.
    #[cfg(any(target_os = "macos", target_os = "windows"))]
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
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    struct KeychainItem {
        reference: SecretRef,
    }

    #[cfg(any(target_os = "macos", target_os = "windows"))]
    impl KeychainItem {
        /// Store `value` under `reference` and take responsibility for
        /// removing it again.
        fn stored(store: &NativeSecretStore, reference: SecretRef, value: &str) -> Self {
            store.store(&reference, value).expect("store");
            Self { reference }
        }
    }

    #[cfg(any(target_os = "macos", target_os = "windows"))]
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

    /// Whether the platform's own CLI can see the item — an independent
    /// witness that the credential really is in the OS store, rather than
    /// `keyring` agreeing with itself.
    ///
    /// **Neither form asks for the value.** `security`'s `-w` prints the
    /// password and is deliberately not passed; `cmdkey` has no flag that
    /// prints one at all. Both are asked only whether the item exists, and
    /// neither one's output is ever put into an assertion message.
    ///
    /// `security find-generic-password` reports absence through its exit
    /// status. `cmdkey /list` exits 0 whether or not anything matched and
    /// prints `* NONE *` when nothing did, so on Windows the **output** is
    /// the witness, matched against the target name `keyring` composes —
    /// the account, a dot, then the service — which is also the string a
    /// user sees in Credential Manager.
    #[cfg(target_os = "macos")]
    fn os_cli_sees(account: &str) -> bool {
        std::process::Command::new("security")
            .args(["find-generic-password", "-s", SERVICE, "-a", account])
            .output()
            .map(|out| out.status.success())
            .unwrap_or(false)
    }

    #[cfg(target_os = "windows")]
    fn os_cli_sees(account: &str) -> bool {
        let target = format!("{account}.{SERVICE}");
        std::process::Command::new("cmdkey")
            .arg("/list")
            .output()
            .map(|out| String::from_utf8_lossy(&out.stdout).contains(&target))
            .unwrap_or(false)
    }

    /// **Acceptance 1, gated to the two platforms that have a backend.**
    /// The Secret Service is deliberately not implemented and deliberately
    /// not claimed; this runs where a real store answers, and there is no
    /// arm of it a mock could satisfy, because on every other target the
    /// dependency is not linked at all.
    ///
    /// Skipped, loudly, when the store will not open — a CI runner with no
    /// login keychain, or a Windows session with no credential store, is a
    /// real state, and a test that pretended to have proven something there
    /// would be worse than one that says it could not.
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    #[test]
    fn a_credential_stored_in_the_native_store_resolves_and_deletes() {
        const VALUE: &str = "sk-keychain-round-trip-test-0123456789abcdef";

        let Ok(store) = NativeSecretStore::detect() else {
            eprintln!(
                "SKIPPED: the native secure store would not open in this session, so this \
                 test proved nothing"
            );
            return;
        };

        let account = test_account("ROUNDTRIP");
        let reference = os_credential_for_variable(&account);

        let item = KeychainItem::stored(&store, reference.clone(), VALUE);
        assert!(
            os_cli_sees(&account),
            "the platform's own credential CLI must see the item that was just stored"
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
            !os_cli_sees(&account),
            "the platform's own credential CLI must no longer see the item after deletion"
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

    /// Lines 1 and 441: a credential in the OS store beats one exported in
    /// the shell that launched Glasshouse, because the stored one was
    /// chosen deliberately. Both are planted at once so the test cannot
    /// pass by the environment simply being empty.
    ///
    /// The label is asserted against `backend::LABEL` rather than a
    /// literal, so this one test says "prefer the macOS Keychain" on macOS
    /// and "prefer Windows Credential Manager" on Windows without either
    /// claim being written twice.
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    #[test]
    fn the_native_store_is_preferred_over_the_environment() {
        const STORED: &str = "sk-from-the-keychain-0123456789abcdefghij";
        const EXPORTED: &str = "sk-from-the-environment-abcdefghij0123456";

        let Ok(native) = NativeSecretStore::detect() else {
            eprintln!("SKIPPED: the native secure store would not open in this session");
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

        assert!(
            resolved.as_deref() == Some(STORED),
            "the native store must win"
        );
        assert_eq!(describe, NATIVE_FIRST_LABEL);
        assert_eq!(source, Some(backend::LABEL));
    }

    /// With the native store holding nothing for a name, the environment
    /// still answers it — the fallback half of the same store, on a machine
    /// where the native half is genuinely available.
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    #[test]
    fn the_environment_answers_what_the_native_store_does_not_hold() {
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

    /// **Acceptance 1, end to end.** A credential stored in the OS store
    /// resolves through a [`SecretStore`] and reaches a launch overlay's
    /// environment — the actual thing a user is trying to do, not just a
    /// round trip through this module.
    ///
    /// Goes through `crate::profile::resolve` unchanged: that function asks
    /// a store with a [`SecretRef::Environment`] reference and knows nothing
    /// about keychains, which is exactly the point of "a reference names a
    /// credential; the store decides where it lives".
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    #[test]
    fn a_native_store_credential_reaches_a_launch_overlays_environment() {
        use crate::profile::{BackendResource, LaunchProfile, Resolution, resolve};

        const STORED: &str = "sk-into-a-launch-overlay-0123456789abcdef";

        let Ok(native) = NativeSecretStore::detect() else {
            eprintln!("SKIPPED: the native secure store would not open in this session");
            return;
        };

        let account = test_account("OVERLAY");
        let reference = SecretRef::Environment {
            var: account.clone(),
        };
        let _item = KeychainItem::stored(&native, reference.clone(), STORED);

        // The mapping itself, witnessed from outside Glasshouse. Storing and
        // reading through the same `entry_name` would agree with each other
        // whatever that function did; the platform's own CLI agrees with
        // nothing but the platform's store, so this is what pins an
        // `Environment` reference to the service and account a user is told
        // to look under. Found by mutation: redirecting the `Environment`
        // arm to another account left every other assertion in this test
        // passing.
        assert!(
            os_cli_sees(&account),
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
            "the credential stored in the native store never reached the overlay's environment"
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
