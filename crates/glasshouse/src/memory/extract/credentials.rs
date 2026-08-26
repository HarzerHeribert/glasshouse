//! The one control that stops a secret reaching a model or a memory row.
//!
//! # Why this module exists at all, and why here
//!
//! `the_project_database_schema_has_nowhere_to_put_a_credential` pins every
//! column of the project database and proves no column's *purpose* is a
//! credential. Its own doc comment then says what it cannot prove:
//! `memories.subject` and `memories.body` are free text, and free text holds
//! anything. The worker that added migration 4 declined to certify otherwise
//! and was right to. **The control therefore belongs to the producer, and
//! this module is it.**
//!
//! # Two directions, deliberately asymmetric
//!
//! **Into the model, text is scrubbed.** A session that happened to print a
//! key still contains everything else the project learned that hour, and
//! throwing the hour away to punish one line would lose far more than it
//! protects. [`scrub`] removes the credential and keeps the session.
//!
//! **Out of the model, a memory is refused whole.** [`screen`] is
//! fail-closed: a memory whose text trips the recognizer is not stored at
//! all, not stored redacted. The asymmetry is the point. A redacted secret
//! sitting in a durable row still carries everything around it — which host,
//! which account, which variable, which project — and
//! [`crate::secret::redact`]'s own documentation records the failure this
//! project already had: it "removes credential-shaped runs and says nothing
//! about the text around them", and a captured line once had its credential
//! redacted and a planted prompt body verbatim. One lost memory costs one
//! memory. A durable row that names a live credential's neighbourhood costs
//! something else.
//!
//! # What is recognized, stated as a closed list
//!
//! Nothing here is a general secret detector, and pretending otherwise would
//! be worse than the honest list:
//!
//! 1. Everything [`crate::secret::redact`] recognizes — `sk-`, `ghp_`,
//!    `github_pat_` tokens and `Bearer` tokens. That function is already
//!    tested in its own module, and reusing it means the two recognizers
//!    cannot drift apart.
//! 2. A **secret assignment**: one of [`ASSIGNMENT_KEYWORDS`] followed by
//!    `=` or `:` and a value that is credential-shaped —
//!    [`MIN_ASSIGNED_VALUE`] characters or more, no whitespace, and carrying
//!    both a letter and a digit.
//!
//! # Why there is no entropy rule
//!
//! The obvious next step — refuse any long high-entropy run — refuses
//! `source_commit`. A Git SHA is forty characters of hex, and storing it is
//! a Phase 20 requirement. An entropy rule that admitted SHAs would admit
//! most keys too, so the rule would be decoration. The letter-and-digit
//! requirement on an *assigned* value does the same work without the
//! collateral damage, because a SHA does not follow the word `password`.
//!
//! # Why the error carries no text
//!
//! [`CredentialFound`] names the shape and nothing else. An error that
//! quoted the offending line would move the credential from the row into the
//! log, which is the same leak wearing a different hat —
//! `crate::gateway`'s eight fixed phrases exist for exactly this reason.
//! `a_credential_refusal_never_repeats_the_credential` holds it to that.

use std::fmt;

/// Replaces a line that carried a secret assignment.
///
/// A marker rather than deletion, so the model can see that something was
/// removed and does not silently reason about a transcript with a hole in
/// it. Fixed text: it names no key, no value and no variable.
pub const REMOVED_LINE: &str = "[glasshouse removed a line that assigned a credential]";

/// How long an assigned value must be before it counts as a credential.
///
/// Sixteen, matching the tail length `crate::secret`'s own prefix table
/// requires. Ordinary prose reaches it only when it has no spaces in it,
/// which prose does not.
pub const MIN_ASSIGNED_VALUE: usize = 16;

/// Names that introduce a credential when something is assigned to them.
///
/// Compound where a bare word would be ambiguous. `key` is deliberately
/// **absent**: a memory body may legitimately read `key: memory belongs to
/// the project`, and a recognizer that refused that would refuse real
/// knowledge. `api_key`, `secret_key` and `private_key` carry the same
/// meaning without the ambiguity.
///
/// Matched case-insensitively, and only at a boundary — the character before
/// a match must not be alphanumeric — so `ANTHROPIC_AUTH_TOKEN` matches
/// `auth_token` while `subtoken` does not match `token`.
pub const ASSIGNMENT_KEYWORDS: &[&str] = &[
    "access_key",
    "access_token",
    "api_key",
    "apikey",
    "auth_key",
    "auth_token",
    "authorization",
    "client_secret",
    "credential",
    "passphrase",
    "passwd",
    "password",
    "private_key",
    "refresh_token",
    "secret",
    "secret_key",
    "session_token",
    "signing_key",
    "token",
];

/// What kind of credential material was recognized.
///
/// Carries no value, no key name and no surrounding text — see the module
/// documentation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CredentialShape {
    /// A token [`crate::secret::redact`] recognizes: `sk-`, `ghp_`,
    /// `github_pat_`, or a `Bearer` token.
    KnownToken,
    /// A credential-shaped value assigned to one of
    /// [`ASSIGNMENT_KEYWORDS`].
    Assignment,
}

impl CredentialShape {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::KnownToken => "a recognized credential token",
            Self::Assignment => "a credential assigned to a secret-shaped name",
        }
    }
}

impl fmt::Display for CredentialShape {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.pad(self.as_str())
    }
}

/// Credential material was found in text that must not carry any.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error(
    "refusing to store this memory: its {field} contains {shape}. Extraction \
     never stores credential material, and this text is dropped rather than \
     redacted"
)]
pub struct CredentialFound {
    /// Which part of the memory tripped the recognizer. A fixed
    /// `&'static str`, never text from the memory itself.
    pub field: &'static str,
    pub shape: CredentialShape,
}

/// Text with every recognized credential removed, and a count of how many.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Scrubbed {
    text: String,
    removals: usize,
}

impl Scrubbed {
    pub fn text(&self) -> &str {
        &self.text
    }

    /// How many lines or tokens were removed. Never silent: the extractor
    /// reports this so a transcript that lost content says so.
    pub fn removals(&self) -> usize {
        self.removals
    }
}

/// Remove every recognized credential from `text`, keeping the rest.
///
/// Used on the way *in*, so a session that printed a key can still be
/// extracted from. A whole line goes when it assigns a credential — keeping
/// the key name and dropping only the value would preserve the half that
/// says which credential it was — and everything else goes through
/// [`crate::secret::redact`], which keeps `Bearer` and the header around it
/// so the remaining text still reads.
pub fn scrub(text: &str) -> Scrubbed {
    let mut removals = 0;
    let mut lines: Vec<String> = Vec::new();

    for line in text.lines() {
        if assignment_in(line).is_some() {
            removals += 1;
            lines.push(REMOVED_LINE.to_owned());
            continue;
        }
        let redacted = crate::secret::redact(line);
        if redacted != line {
            removals += 1;
        }
        lines.push(redacted);
    }

    Scrubbed {
        text: lines.join("\n"),
        removals,
    }
}

/// Refuse `text` outright if it carries recognized credential material.
///
/// Used on the way *out*, where redaction is not enough. `field` names the
/// part of the memory being checked and appears in the error; it must be a
/// fixed string, never text under inspection.
///
/// Defined in terms of the same two recognizers [`scrub`] uses, so the pair
/// cannot drift: anything scrubbing would have removed is something
/// screening refuses.
pub fn screen(field: &'static str, text: &str) -> Result<(), CredentialFound> {
    for line in text.lines() {
        if assignment_in(line).is_some() {
            return Err(CredentialFound {
                field,
                shape: CredentialShape::Assignment,
            });
        }
        if crate::secret::redact(line) != line {
            return Err(CredentialFound {
                field,
                shape: CredentialShape::KnownToken,
            });
        }
    }
    Ok(())
}

/// The byte offset of a secret assignment in `line`, if it has one.
///
/// Returns a position rather than a bool so the unit tests can assert *which*
/// assignment matched rather than only that one did.
fn assignment_in(line: &str) -> Option<usize> {
    let lower = line.to_ascii_lowercase();

    for keyword in ASSIGNMENT_KEYWORDS {
        let mut from = 0;
        while let Some(offset) = lower[from..].find(keyword) {
            let at = from + offset;
            from = at + 1;

            // A match must start at a boundary, so `subtoken` is not
            // `token`. `_`, `-` and `.` are boundaries, so
            // `ANTHROPIC_AUTH_TOKEN` is `auth_token`.
            if lower[..at]
                .chars()
                .next_back()
                .is_some_and(|c| c.is_ascii_alphanumeric())
            {
                continue;
            }

            let after = &line[at + keyword.len()..];
            if let Some(value) = assigned_value(after)
                && is_credential_shaped(value)
            {
                return Some(at);
            }
        }
    }
    None
}

/// The value assigned after a keyword, if this really is an assignment.
///
/// Accepts `=`, `:` and `=>` with optional whitespace and one optional
/// opening quote, which covers shell, JSON, TOML, YAML and Rust literals
/// alike. Anything else — a bare word, a sentence, a keyword used as prose —
/// yields `None`.
fn assigned_value(after: &str) -> Option<&str> {
    let after = after.trim_start_matches([' ', '\t', '"', '\'']);
    let after = after.strip_prefix(['=', ':'])?;
    let after = after.strip_prefix('>').unwrap_or(after);
    let after = after.trim_start_matches([' ', '\t', '"', '\'', '`']);

    let end = after
        .find(|c: char| c.is_whitespace() || matches!(c, '"' | '\'' | '`' | ',' | ';'))
        .unwrap_or(after.len());
    Some(&after[..end])
}

/// Whether an assigned value looks like a credential rather than a word.
///
/// Long, unbroken, and carrying both a letter and a digit. The last
/// condition is what keeps `secret: memory-belongs-to-the-project` — a real
/// sentence this project would want to remember — out of the recognizer,
/// while `secret: hunter2xyzabcdefghij` stays in.
fn is_credential_shaped(value: &str) -> bool {
    value.chars().count() >= MIN_ASSIGNED_VALUE
        && value.chars().any(|c| c.is_ascii_alphabetic())
        && value.chars().any(|c| c.is_ascii_digit())
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '+' | '/' | '.' | '='))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A value with the shape of a real key, built here rather than pasted,
    /// so nothing in this repository is a credential someone could try.
    const PLANTED: &str = "hunter2xyzabcdefghijklmn";

    #[test]
    fn an_assignment_of_a_credential_shaped_value_is_recognized() {
        for line in [
            "API_KEY=hunter2xyzabcdefghijklmn",
            "  password: hunter2xyzabcdefghijklmn",
            "ANTHROPIC_AUTH_TOKEN=hunter2xyzabcdefghijklmn",
            "\"client_secret\": \"hunter2xyzabcdefghijklmn\"",
            "secret_key => hunter2xyzabcdefghijklmn",
        ] {
            assert!(
                assignment_in(line).is_some(),
                "should have recognized an assignment in {line:?}"
            );
        }
    }

    #[test]
    fn prose_that_merely_mentions_a_secret_is_not_an_assignment() {
        for line in [
            // No digit: a sentence, not a key.
            "secret: memory-belongs-to-the-project",
            // Too short.
            "token: abc123",
            // A word, with spaces.
            "password: correct horse battery staple",
            // Not an assignment at all.
            "The auth token is stored in the Keychain, never in memory.",
            // `subtoken` is not `token`.
            "subtoken=hunter2xyzabcdefghijklmn",
            // A commit SHA is not a credential, and Phase 20 stores one.
            "source_commit: a938fcc1d2e3b4a5968778695a4b3c2d1e0f9a8b",
        ] {
            assert!(
                assignment_in(line).is_none(),
                "should not have recognized an assignment in {line:?}"
            );
        }
    }

    #[test]
    fn scrub_removes_the_whole_assignment_line_and_keeps_the_rest() {
        let text = format!(
            "the gateway holds the credential\nAPI_KEY={PLANTED}\nand the harness never sees it"
        );
        let scrubbed = scrub(&text);

        assert!(!scrubbed.text().contains(PLANTED));
        assert!(!scrubbed.text().contains("API_KEY"));
        assert!(scrubbed.text().contains("the gateway holds the credential"));
        assert!(scrubbed.text().contains("and the harness never sees it"));
        assert_eq!(scrubbed.removals(), 1);
    }

    #[test]
    fn scrub_defers_to_the_secret_modules_own_recognizer() {
        let text = "Authorization: Bearer abcdefghijklmnopqrstuvwxyz012345";
        let scrubbed = scrub(text);

        assert!(!scrubbed.text().contains("abcdefghijklmnopqrstuvwxyz012345"));
        assert_eq!(scrubbed.removals(), 1);
    }

    #[test]
    fn scrub_leaves_ordinary_project_knowledge_untouched() {
        let text = "The local gateway holds the provider credential so the harness never \
                    receives one. Verified against commit a938fcc.";
        assert_eq!(scrub(text).text(), text);
        assert_eq!(scrub(text).removals(), 0);
    }

    #[test]
    fn screen_refuses_rather_than_redacting() {
        let body = format!("set API_KEY={PLANTED} before launching");
        let err = screen("body", &body).unwrap_err();
        assert_eq!(err.shape, CredentialShape::Assignment);
        assert_eq!(err.field, "body");
    }

    /// The refusal is an error a caller will log. If it quoted the line it
    /// refused, the credential would simply move from the database into the
    /// log — the leak this module exists to prevent, in a different place.
    #[test]
    fn a_credential_refusal_never_repeats_the_credential() {
        let body = format!("API_KEY={PLANTED}");
        let err = screen("body", &body).unwrap_err();

        let rendered = format!("{err}");
        let debugged = format!("{err:?}");
        assert!(!rendered.contains(PLANTED), "Display leaked: {rendered}");
        assert!(!debugged.contains(PLANTED), "Debug leaked: {debugged}");
        assert!(!rendered.contains("API_KEY"));
        assert!(!debugged.contains("API_KEY"));
    }

    #[test]
    fn screen_admits_a_memory_that_only_talks_about_credentials() {
        let body = "Credentials resolve from the macOS Keychain first and the process \
                    environment second; a provider key is never written into the project \
                    database.";
        assert!(screen("body", body).is_ok());
    }

    /// The two directions must agree. Anything `scrub` would have removed is
    /// something `screen` refuses, and that is a property of the pair rather
    /// than of either one — so it is asserted, not assumed.
    #[test]
    fn anything_scrub_removes_is_something_screen_refuses() {
        for text in [
            "API_KEY=hunter2xyzabcdefghijklmn",
            "Authorization: Bearer abcdefghijklmnopqrstuvwxyz012345",
            "sk-abcdefghijklmnopqrstuvwxyz",
            "ghp_abcdefghijklmnopqrstuvwxyz",
        ] {
            assert_eq!(scrub(text).removals(), 1, "scrub missed {text:?}");
            assert!(screen("body", text).is_err(), "screen admitted {text:?}");
        }
    }
}
