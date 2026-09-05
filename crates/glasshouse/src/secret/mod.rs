//! Where a provider credential comes from, and the only shape its value is
//! ever allowed to take (Phase 9E). Two types, deliberately not one: a
//! [`SecretRef`] **references** a source,
//! so config and diagnostics may hold one freely; a [`Secret`] is a
//! **value**, existing only between [`SecretStore::resolve`] and the child
//! process that needed it, with no `Display`/`Deref`/`Serialize` and a
//! [`Debug`](std::fmt::Debug) that prints [`REDACTED`] — not even a length,
//! which would narrow a key space. The only way out is [`Secret::expose`].
//! `resolve` reads the source when called, not when built, so a stored
//! `SecretRef` always reflects the environment now.
//!
//! This module writes and reads no file, reaches a launch through exactly
//! one call site ([`crate::profile::resolve`], which mints, moves and drops
//! one [`Secret`]; a harness adapter gets variable *names*, never a
//! `Secret`), and ships only the native OS-backed stores it can prove
//! ([`mod@native`]: Keychain, Credential Manager — no Secret Service
//! keyring, which can hang a launch on an unlock prompt). [`redact`] is
//! belt and braces for output not fully controlled, never a licence to
//! format a credential and clean up after. History: design-decisions.md,
//! "Trims: the remaining module docs, second packet", secret/mod.rs module doc.

use std::fmt;

pub mod native;

/// What stands in for a credential everywhere one might otherwise be
/// printed: the [`Debug`](std::fmt::Debug) rendering of a [`Secret`], and
/// [`redact`]'s replacement.
///
/// Fixed and value-independent on purpose. A marker that varied with the
/// value — a length, a first character, a hash — would leak exactly the
/// thing this module exists to withhold.
pub const REDACTED: &str = "[redacted]";

/// Where a credential's value comes from. **A reference, never a value.**
///
/// This type is what makes the secret boundary structural rather than a
/// habit: configuration, session records and diagnostics can all hold a
/// `SecretRef` freely, because holding one reveals nothing.
///
/// It is safe to store and safe to serialize — every field it has is a
/// *name*. It derives no serde impl here only because nothing in Glasshouse
/// stores one yet, and the on-disk shape of a stored reference is a
/// configuration-schema decision that belongs to the phase that first needs
/// it, not a guess made in advance.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SecretRef {
    /// Read from this environment variable at the moment of use.
    ///
    /// Naming a variable is not the same as naming a *location*: this is the
    /// only name Glasshouse has for a provider credential anywhere, and a
    /// store is free to answer it from wherever it keeps credentials. That
    /// is exactly what [`native::PreferNativeSecretStore`] does when it
    /// prefers the operating system's own store.
    Environment { var: String },
    /// Held in the operating system's own credential store, filed under
    /// this service and account.
    ///
    /// Still two names and nothing else, so this variant is as safe to store
    /// in configuration and to print as the other one. The value behind it
    /// is produced by [`native::NativeSecretStore`], at the moment of use,
    /// exactly like the environment's.
    OsCredential { service: String, account: String },
}

/// Resolves a [`SecretRef`] into a value that is deliberately awkward to
/// keep.
pub trait SecretStore {
    /// The value behind `reference`, or `None` when the source has nothing.
    fn resolve(&self, reference: &SecretRef) -> Option<Secret>;
    /// Whether `reference` currently has a value, without producing one.
    fn is_present(&self, reference: &SecretRef) -> bool;
    /// A short label naming this store, for diagnostics.
    fn describe(&self) -> &'static str;
}

/// A resolved credential value.
///
/// Everything about this type is arranged so that printing it by accident is
/// not possible: its `Debug` is manual and prints [`REDACTED`], and there is
/// no `Display`, no `Deref`, no `AsRef<str>` and no serde impl. The only way
/// out is [`Secret::expose`].
///
/// The inner `String` is private to this module and its descendants, so a
/// `Secret` can only be minted by a store that lives here — an outside crate
/// or module cannot construct one from arbitrary text and then claim the
/// protections of this type for it.
pub struct Secret(String);

impl Secret {
    /// Hand the value to something that genuinely needs it — a child
    /// process's environment, and essentially nothing else.
    ///
    /// Every call site is a place a credential leaves this module, so each
    /// one should be short-lived, obvious, and easy to count.
    pub fn expose(&self) -> &str {
        &self.0
    }
}

/// Prints [`REDACTED`] and nothing else.
///
/// Not derived, and not "the first four characters" either: a prefix, a
/// suffix or a length all narrow the space an attacker has to search, so
/// this rendering is identical for every value, including the empty one.
impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(REDACTED)
    }
}

/// The cross-platform [`SecretStore`]: values come from the process
/// environment, read at the moment of use.
///
/// It is not a secure store and does not claim to be one —
/// [`SecretStore::describe`] says exactly what it is, so a diagnostic can
/// tell a user where a credential came from without anyone having to infer
/// it. It is also the fallback half of
/// [`native::PreferNativeSecretStore`], which is what makes that fallback
/// labelled rather than silent.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct EnvironmentSecretStore;

impl EnvironmentSecretStore {
    pub fn new() -> Self {
        Self
    }
}

impl SecretStore for EnvironmentSecretStore {
    /// Reads with [`std::env::var_os`] and converts inside this method only.
    ///
    /// A variable holding bytes that are not valid UTF-8 resolves to `None`
    /// while [`SecretStore::is_present`] still reports it as set. That
    /// divergence is deliberate and honest: something *is* there, and this
    /// store cannot hand it over as a `&str`. Reporting it as absent would
    /// hide a misconfigured variable; lossy conversion would hand a harness
    /// a silently corrupted key.
    fn resolve(&self, reference: &SecretRef) -> Option<Secret> {
        match reference {
            SecretRef::Environment { var } => std::env::var_os(var)
                .and_then(|value| value.into_string().ok())
                .map(Secret),
            // Not this store's to answer. `None` rather than a guess at
            // some environment variable derived from the account name: this
            // store reads the environment, and a reference that names the
            // OS store is asking something it cannot answer.
            SecretRef::OsCredential { .. } => None,
        }
    }

    /// Never converts, never allocates a `String`, never builds a
    /// [`Secret`]: presence is answered from [`std::env::var_os`]'s
    /// `Option` alone, exactly as `integrations`' doctor report already
    /// answers it.
    fn is_present(&self, reference: &SecretRef) -> bool {
        match reference {
            SecretRef::Environment { var } => std::env::var_os(var).is_some(),
            SecretRef::OsCredential { .. } => false,
        }
    }

    fn describe(&self) -> &'static str {
        "process environment"
    }
}

/// The character class a token-shaped credential's tail is drawn from.
///
/// Deliberately broad — providers differ, and a class that is too narrow
/// stops a redaction halfway through a key, which is worse than not starting
/// it.
fn is_token_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_' || c == '-'
}

/// A `Bearer` token's class: anything that is not whitespace and not a
/// quote.
///
/// Broader than [`is_token_char`] because JWTs and base64 payloads carry
/// `.`, `+`, `/` and `=`. Quotes end the run so that a credential embedded
/// in JSON or a shell line is redacted without eating the punctuation around
/// it — a mangled diagnostic gets ignored, and an ignored diagnostic
/// protects nobody.
fn is_bearer_token_char(c: char) -> bool {
    !c.is_whitespace() && !matches!(c, '"' | '\'' | '`')
}

/// Prefixes that introduce a credential, with how many tail characters must
/// follow for a match to count.
///
/// Case-sensitive, and each minimum is set so ordinary prose cannot reach
/// it. `sk-or-v1-` needs no entry of its own: it is an `sk-` key whose tail
/// happens to start with `or-v1-`.
const CREDENTIAL_PREFIXES: &[(&str, usize)] = &[("sk-", 16), ("ghp_", 16), ("github_pat_", 16)];

/// The `Authorization` scheme whose token is redacted.
const BEARER_SCHEME: &str = "Bearer";

/// How long a `Bearer` token must be before it is treated as a credential.
const BEARER_MIN_TOKEN: usize = 20;

/// Replace anything that looks like a credential with a fixed marker.
///
/// Recognises `sk-` (including `sk-or-v1-` and `sk-ant-` forms) followed by
/// 16 or more characters of `[A-Za-z0-9_-]`, `ghp_` and `github_pat_` tokens
/// of the same shape, and `Bearer` followed by a token of 20 characters or
/// more. Prefixes are case-sensitive; the tail classes are deliberately
/// broad.
///
/// A prefix only counts at a token boundary, which is what keeps the
/// redactor from eating ordinary text: the `sk-` inside
/// `risk-assessment-and-mitigation` is preceded by `i`, so nothing there
/// matches. That restraint is the feature. An over-eager redactor makes
/// diagnostics useless, and a useless diagnostic gets switched off — taking
/// the protection with it.
///
/// For a `Bearer` match the scheme and the space after it are kept and only
/// the token is replaced, so `Authorization: Bearer [redacted]` still tells
/// a reader which header carried the credential.
pub fn redact(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    // The start of the text is a boundary; after that, only a character
    // outside the token class opens one.
    let mut at_boundary = true;

    while let Some(c) = rest.chars().next() {
        let matched = if at_boundary {
            credential_match(rest)
        } else {
            None
        };
        if let Some((keep, consume)) = matched {
            out.push_str(&rest[..keep]);
            out.push_str(REDACTED);
            rest = &rest[consume..];
            // Every match consumes a maximal run, so whatever follows is
            // outside the token class.
            at_boundary = true;
            continue;
        }
        out.push(c);
        at_boundary = !is_token_char(c);
        rest = &rest[c.len_utf8()..];
    }

    out
}

/// A credential starting at the front of `rest`, as `(keep, consume)`: the
/// first `keep` bytes are copied through unchanged and the bytes from `keep`
/// to `consume` are replaced by [`REDACTED`].
fn credential_match(rest: &str) -> Option<(usize, usize)> {
    for &(prefix, min_tail) in CREDENTIAL_PREFIXES {
        if let Some(tail) = rest.strip_prefix(prefix) {
            // Every character in the class is ASCII, so the count is also
            // the byte length.
            let tail_len = tail.chars().take_while(|&c| is_token_char(c)).count();
            if tail_len >= min_tail {
                return Some((0, prefix.len() + tail_len));
            }
        }
    }
    bearer_match(rest)
}

/// A `Bearer <token>` at the front of `rest`, keeping the scheme and the
/// separator and replacing only the token.
fn bearer_match(rest: &str) -> Option<(usize, usize)> {
    let after_scheme = rest.strip_prefix(BEARER_SCHEME)?;
    let gap = after_scheme
        .chars()
        .take_while(|&c| c == ' ' || c == '\t')
        .count();
    if gap == 0 {
        // `Bearer` as a word, or `Bearer\n`: not a credential on this line.
        return None;
    }

    let token = &after_scheme[gap..];
    let mut token_bytes = 0;
    let mut token_chars = 0;
    for c in token.chars() {
        if !is_bearer_token_char(c) {
            break;
        }
        token_bytes += c.len_utf8();
        token_chars += 1;
    }
    if token_chars < BEARER_MIN_TOKEN {
        return None;
    }

    let keep = BEARER_SCHEME.len() + gap;
    Some((keep, keep + token_bytes))
}

/// Building a [`Secret`] from a literal, for tests in other modules of this
/// crate.
///
/// [`Secret`]'s field is private to this module, which is what stops an
/// outside module from claiming this type's protections for arbitrary text —
/// but it also stops any test outside this module from implementing a
/// [`SecretStore`] at all, and [`crate::profile::resolve`]'s credential rules
/// have to be exercised against a store holding a known value. Setting a real
/// environment variable in a test would publish that value to every other
/// test in the process, which is a worse trade than this.
///
/// `#[cfg(test)]` and `pub(crate)`: it does not exist in a release build and
/// it is not API, so the production boundary is exactly as narrow as it was.
/// It sits here, immediately above the test module, so that the source scans
/// in this file and elsewhere — which read everything before the first
/// `#[cfg(test)]` as "production code" — still see the whole of it.
#[cfg(test)]
impl Secret {
    pub(crate) fn mint_for_test(value: &str) -> Self {
        Self(value.to_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::harness::Declared;

    /// This module's own source with its `#[cfg(test)]` block excluded and
    /// `//` comments stripped — the same idiom as
    /// `harness::resolving_a_launch_profile_touches_no_files`'s
    /// `production_code` helper, and `shim`'s copy of it.
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

    /// The declaration of the item named by `header`, brace to brace, taken
    /// from production code only.
    fn declaration_body<'a>(code: &'a str, header: &str) -> &'a str {
        let start = code
            .find(header)
            .unwrap_or_else(|| panic!("`{header}` is declared in this module's production code"))
            + header.len();
        let body = &code[start..];
        let end = body
            .find("\n}")
            .unwrap_or_else(|| panic!("`{header}`'s declaration ends"));
        &body[..end]
    }

    // --- a reference is a name, a secret is a value ---------------------

    /// Structural, not behavioural: a `SecretRef` cannot carry a value
    /// because it has nowhere to put one. Every field it declares is a
    /// plain `String` naming a source, so no future variant can quietly
    /// become the place a credential is stored.
    #[test]
    fn a_secret_ref_names_a_source_and_never_carries_a_value() {
        let code = production_code(include_str!("mod.rs"));
        let body = declaration_body(&code, "pub enum SecretRef {");

        let fields: Vec<String> = body
            .replace(['{', '}'], " ")
            .split(',')
            .map(|field| field.split_whitespace().collect::<Vec<_>>().join(" "))
            .filter(|field| field.contains(':'))
            .collect();
        assert!(
            !fields.is_empty(),
            "no field was found in SecretRef's declaration; the scan below would pass vacuously"
        );

        for field in &fields {
            let (name, ty) = field
                .split_once(':')
                .expect("filtered to fields containing a colon");
            let name = name
                .split_whitespace()
                .next_back()
                .expect("a field has a name");
            let ty = ty.trim();
            assert_eq!(
                ty, "String",
                "SecretRef::{name} is a `{ty}`: every field of a reference must be a plain \
                 name, so that holding one reveals nothing"
            );
            for banned in ["value", "secret", "token", "password", "credential"] {
                assert!(
                    !name.to_ascii_lowercase().contains(banned),
                    "SecretRef declares a field called `{name}`: this type names a source, \
                     never a value"
                );
            }
        }

        // ... and the reference really does render as the name it holds.
        let reference = SecretRef::Environment {
            var: "OPENROUTER_API_KEY".to_owned(),
        };
        assert!(format!("{reference:?}").contains("OPENROUTER_API_KEY"));
    }

    /// A length is a real leak: it narrows a key space. So the rendering is
    /// identical for every value, and no prefix or suffix of one — however
    /// short — survives into it.
    #[test]
    fn debug_on_a_secret_prints_a_fixed_marker_and_never_the_value() {
        const VALUE: &str = "ghp_qqqqwwwweeeerrrrttttyyyyuuuu9999";

        let rendered = format!("{:?}", Secret(VALUE.to_owned()));
        assert_eq!(rendered, REDACTED, "the marker must be fixed");

        for n in 1..=VALUE.len() {
            assert!(
                !rendered.contains(&VALUE[..n]),
                "the first {n} characters of the value survived into {rendered:?}"
            );
            assert!(
                !rendered.contains(&VALUE[VALUE.len() - n..]),
                "the last {n} characters of the value survived into {rendered:?}"
            );
        }

        assert!(
            !rendered.contains(&VALUE.len().to_string()),
            "the value's length appeared in {rendered:?}"
        );
        assert_eq!(
            format!("{:?}", Secret(String::new())),
            format!("{:?}", Secret("x".repeat(4096))),
            "an empty value and a 4096-character one must be indistinguishable in Debug output"
        );
    }

    /// The compile-fail guard this codebase can express: a source scan of
    /// production code, the same idiom as
    /// `harness::resolving_a_launch_profile_touches_no_files`.
    #[test]
    fn a_secret_has_no_display_no_deref_and_no_asref() {
        let code = production_code(include_str!("mod.rs"));
        for forbidden in ["Display", "Deref", "AsRef", "Borrow", "ToString"] {
            assert!(
                !code.contains(forbidden),
                "secret/mod.rs names `{forbidden}` in production code: a Secret must not be \
                 printable, dereferenceable or borrowable as a str, because every one of those \
                 is a way for a credential to reach output by accident. `expose` is the only \
                 way out."
            );
        }
    }

    /// A `Serialize` on a `Secret` would put a credential into whatever the
    /// serializer writes — a config file, an event payload, the database.
    #[test]
    fn a_secret_is_not_serializable() {
        let code = production_code(include_str!("mod.rs"));
        for forbidden in ["Serialize", "Deserialize", "serde"] {
            assert!(
                !code.contains(forbidden),
                "secret/mod.rs names `{forbidden}` in production code: a Secret must be \
                 neither Serialize nor Deserialize, and a derive on a neighbouring type is one \
                 careless edit away from being one on this one"
            );
        }
    }

    /// "A module that never opens a file cannot leak a credential into one"
    /// — stronger, and cheaper to keep true, than enumerating the files it
    /// must avoid.
    #[test]
    fn nothing_in_this_module_writes_to_disk() {
        let code = production_code(include_str!("mod.rs"));
        for forbidden in ["std::fs", "fs::", "File::", "OpenOptions"] {
            assert!(
                !code.contains(forbidden),
                "secret/mod.rs names `{forbidden}` in production code: no credential value may \
                 be written to disk by any path here"
            );
        }
    }

    /// The scans above are only worth having if they can fail.
    #[test]
    fn the_source_scans_would_catch_a_violation() {
        let violating = "impl std::ops::Deref for Secret {\n    type Target = str;\n}";
        assert!(production_code(violating).contains("Deref"));
        let writing = "fn save(s: &Secret) {\n    std::fs::write(\"k\", s.expose()).unwrap();\n}";
        assert!(production_code(writing).contains("std::fs"));
        // ... and neither fires on a doc comment that merely mentions one.
        let documented = "/// There is no `Deref`, and nothing here uses `std::fs`.\nfn f() {}";
        assert!(!production_code(documented).contains("Deref"));
        assert!(!production_code(documented).contains("std::fs"));
        // ... nor on test code.
        let tested = "fn f() {}\n#[cfg(test)]\nmod tests { use std::ops::Deref; }";
        assert!(!production_code(tested).contains("Deref"));
    }

    // --- the environment store -------------------------------------------

    /// Presence is answered from `var_os`'s `Option` alone. The behavioural
    /// half proves the answer is right; the source scan proves it was
    /// reached without ever building a value.
    #[test]
    fn is_present_reports_presence_without_resolving_a_value() {
        const VAR: &str = "GLASSHOUSE_SECRET_TEST_ONLY_PRESENCE_VAR";
        const VALUE: &str = "sk-presence-test-0123456789abcdef";

        let store = EnvironmentSecretStore::new();
        let reference = SecretRef::Environment {
            var: VAR.to_owned(),
        };

        // SAFETY: `VAR` is unique to this test and is removed again before
        // anything can panic, so no other test can observe it set.
        unsafe {
            std::env::set_var(VAR, VALUE);
        }
        let while_set = store.is_present(&reference);
        unsafe {
            std::env::remove_var(VAR);
        }
        let while_unset = store.is_present(&reference);

        assert!(while_set, "a set variable must be reported as present");
        assert!(!while_unset, "an unset variable must be reported as absent");

        let code = production_code(include_str!("mod.rs"));
        let impl_start = code
            .find("impl SecretStore for EnvironmentSecretStore")
            .expect("the environment store implements SecretStore");
        let body = declaration_body(&code[impl_start..], "fn is_present");
        let end = body.find("\n    }").expect("the method ends");
        let body = &body[..end];

        assert!(
            body.contains("var_os"),
            "is_present must answer from `var_os`, which never decodes a value: {body}"
        );
        // `SecretRef` is a reference; only the value type is forbidden here.
        let body = body.replace("SecretRef", "");
        for forbidden in [
            "Secret",
            "into_string",
            "to_string_lossy",
            "expose",
            "to_owned",
        ] {
            assert!(
                !body.contains(forbidden),
                "is_present names `{forbidden}`: checking presence must never produce a value"
            );
        }
    }

    #[test]
    fn resolve_returns_none_for_an_unset_variable() {
        let store = EnvironmentSecretStore::new();
        let reference = SecretRef::Environment {
            var: "GLASSHOUSE_SECRET_TEST_ONLY_NEVER_SET_VAR".to_owned(),
        };
        assert!(
            store.resolve(&reference).is_none(),
            "an unset variable must resolve to None, never to an empty Secret"
        );
        assert!(!store.is_present(&reference));
        assert_eq!(store.describe(), "process environment");
    }

    /// Resolution reads the source when it is called, not when the reference
    /// was built: the same reference answers differently as the environment
    /// changes, which is what lets configuration hold one indefinitely.
    ///
    /// The assertions deliberately compare without printing: even a
    /// fabricated value does not belong in test output, because that is the
    /// habit that puts a real one there.
    #[test]
    fn resolve_reads_the_value_from_the_named_variable_at_the_moment_of_use() {
        const VAR: &str = "GLASSHOUSE_SECRET_TEST_ONLY_RESOLVE_VAR";
        const FIRST: &str = "sk-first-fabricated-0123456789abcdef";
        const SECOND: &str = "sk-second-fabricated-fedcba9876543210";

        let store = EnvironmentSecretStore::new();
        let reference = SecretRef::Environment {
            var: VAR.to_owned(),
        };

        // SAFETY: `VAR` is unique to this test and is removed again before
        // anything can panic, so no other test can observe it set.
        unsafe {
            std::env::set_var(VAR, FIRST);
        }
        let first = store.resolve(&reference);
        unsafe {
            std::env::set_var(VAR, SECOND);
        }
        let second = store.resolve(&reference);
        unsafe {
            std::env::remove_var(VAR);
        }
        let after = store.resolve(&reference);

        assert!(
            first.as_ref().map(Secret::expose) == Some(FIRST),
            "resolve must return what the variable held at the moment of the call"
        );
        assert!(
            second.as_ref().map(Secret::expose) == Some(SECOND),
            "the same reference must see the variable's new value, not a cached one"
        );
        assert!(
            after.is_none(),
            "the same reference must resolve to None once the variable is gone"
        );
    }

    // --- redaction, the second line of defence ---------------------------

    #[test]
    fn redact_replaces_recognised_credential_shapes() {
        // Credentials a prefix identifies on sight: they are redacted
        // wherever they appear, on their own or inside a line.
        const PREFIXED: &[(&str, &str)] = &[
            ("sk-abcd1234efgh5678ijkl", "an OpenAI-style key"),
            (
                "sk-or-v1-4f6a2c8e0b1d3f5a7c9e1b3d5f7a9c1e3b5d7f9a1c3e5b7d9f1a3c5e7b9d",
                "an OpenRouter key",
            ),
            (
                "sk-ant-api03-QkFEQkVFRl9OT1RfQV9SRUFMX0tFWQ",
                "an Anthropic-style key",
            ),
            (
                "ghp_A1b2C3d4E5f6G7h8I9j0K1l2M3n4O5p6Q7r8",
                "a GitHub personal access token",
            ),
            (
                "github_pat_11ABCDEFG0aBcDeFgHiJkLmNoPqRsTuVwXyZ",
                "a fine-grained GitHub token",
            ),
        ];

        for (credential, what) in PREFIXED {
            let contexts = [
                (*credential).to_owned(),
                format!("OPENROUTER_API_KEY={credential}"),
                format!("Authorization: Bearer {credential}"),
                format!("harness stderr: request rejected (key {credential}), retrying"),
                format!("{{\"authorization\":\"Bearer {credential}\"}}"),
            ];
            for context in contexts {
                let redacted = redact(&context);
                assert!(
                    !redacted.contains(credential),
                    "{what} survived redaction of {context:?}: {redacted:?}"
                );
                assert!(
                    redacted.contains(REDACTED),
                    "{what} was removed from {context:?} without leaving a marker: {redacted:?}"
                );
            }
        }

        // An opaque token — a JWT, a session key — carries no prefix that
        // says "credential". It is recognised by the `Bearer` scheme in
        // front of it and by nothing else, deliberately: redacting every
        // long token on sight would eat git SHAs, base64 payloads and build
        // identifiers, which is the failure `redact_leaves_ordinary_text_alone`
        // exists to prevent.
        const OPAQUE: &str =
            "eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxIn0.dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
        for context in [
            format!("Bearer {OPAQUE}"),
            format!("Authorization: Bearer {OPAQUE}"),
            format!("{{\"authorization\":\"Bearer {OPAQUE}\"}}"),
            format!("curl -H 'Authorization: Bearer {OPAQUE}' https://example.test/v1/models"),
        ] {
            let redacted = redact(&context);
            assert!(
                !redacted.contains(OPAQUE),
                "a bearer token survived redaction of {context:?}: {redacted:?}"
            );
            assert!(redacted.contains(REDACTED), "{redacted:?}");
        }

        // Two credentials on one line: neither shadows the other.
        let both = redact("sk-abcd1234efgh5678ijkl and ghp_A1b2C3d4E5f6G7h8I9j0K1l2M3n4O5p6Q7r8");
        assert_eq!(both, format!("{REDACTED} and {REDACTED}"));

        // A bearer token is replaced, but the header it came in stays
        // readable — a diagnostic nobody can read is a diagnostic nobody
        // keeps.
        let header = redact("Authorization: Bearer eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxIn0.dBjft");
        assert_eq!(header, format!("Authorization: Bearer {REDACTED}"));
    }

    /// An over-eager redactor makes diagnostics useless, and a useless
    /// diagnostic gets switched off — taking the protection with it.
    #[test]
    fn redact_leaves_ordinary_text_alone() {
        const UNTOUCHED: &[&str] = &[
            "/Users/eneas/projects/glasshouse/crates/glasshouse/src/secret/mod.rs",
            "C:\\Users\\eneas\\AppData\\Local\\glasshouse\\state",
            "550e8400-e29b-41d4-a716-446655440000",
            "6a5df97c1b2d3e4f5a6b7c8d9e0f1a2b3c4d5e6f",
            "https://openrouter.ai/api/v1/models",
            // The trap: `risk-` contains `sk-` followed by plenty of tail.
            "the risk-assessment-and-mitigation-plan was approved",
            "sk-learn is not scikit-learn, and neither is a credential",
            "sk- is the prefix, and Bearer is the scheme",
            "Bearer token authentication is not configured for this provider",
            "github_pat_ was mentioned but no token followed",
            "OPENROUTER_API_KEY (set, value hidden)",
            "glasshouse doctor reported 548 passing tests and 0 failures",
            "",
        ];

        for text in UNTOUCHED {
            assert_eq!(
                redact(text),
                *text,
                "redact ate ordinary text: {text:?} became {:?}",
                redact(text)
            );
        }
    }

    // --- providers yield references, never values ------------------------

    #[test]
    fn a_provider_yields_one_secret_ref_per_credential_variable() {
        // The user's multiple-keys-per-router case: one provider, two keys,
        // held as a pool rather than as two provider instances.
        let pooled = crate::provider::Provider {
            name: "multi-key".to_owned(),
            protocols: Vec::new(),
            model_list_endpoint: Declared::Unverified,
            usage_telemetry: Declared::Unverified,
            credential_env: vec![
                "A_EXAMPLE_KEY".to_owned(),
                "A_EXAMPLE_KEY_BACKUP".to_owned(),
            ],
            headers: vec![],
        };
        assert_eq!(
            pooled.secret_refs(),
            vec![
                SecretRef::Environment {
                    var: "A_EXAMPLE_KEY".to_owned()
                },
                SecretRef::Environment {
                    var: "A_EXAMPLE_KEY_BACKUP".to_owned()
                },
            ],
            "each credential variable must yield its own reference, in the order declared"
        );

        // A built-in template with exactly one.
        let openrouter =
            crate::provider::template("openrouter").expect("openrouter is a built-in template");
        assert_eq!(
            openrouter.secret_refs(),
            vec![SecretRef::Environment {
                var: "OPENROUTER_API_KEY".to_owned()
            }]
        );

        // A provider with no credential variable yields no reference — never
        // a reference to an invented variable name.
        let ollama = crate::provider::template("ollama").expect("ollama is a built-in template");
        assert!(ollama.credential_env.is_empty());
        assert!(ollama.secret_refs().is_empty());
    }
}
