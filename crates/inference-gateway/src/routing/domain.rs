//! Two domains a failover candidate belongs to, and why they are two types —
//! Phase 33C line 1371.
//!
//! A quota domain **is** a [`super::CredentialId`]: two different
//! credentials are two different quota domains by that type's own
//! `PartialEq`, so nothing here wraps it in a second type that would only
//! compare the same way `CredentialId` already does.
//!
//! A failure domain is a different question: [`super::Backend`] carries no
//! base URL, so the only honest signal for "does this request land on the
//! same infrastructure" is the provider name — which answers "yes" with
//! certainty and "no" with **no certainty at all**, since a different
//! provider is only the absence of evidence that it is the same one.
//! [`FailureDomain`] is three states rather than a bool for exactly that
//! reason — line 1371's "represent... separately" and line 1378's "prevent
//! absent evidence from being interpreted as independence".
// History: design-decisions.md, "Trims: routing module docs", routing/domain.rs module doc.

use super::Backend;

/// What is known about whether two backends would go down together.
///
/// [`FailureDomain::Independent`] is the state this build never earns:
/// nothing here does the temporal correlation Phase 33C lines 1370 and 1373
/// would need to justify it (out of scope for this package — see the
/// dispatch packet), so [`FailureDomain::between`], the only function in
/// this crate that produces a `FailureDomain`, can only ever answer
/// [`FailureDomain::Shared`] or [`FailureDomain::Unknown`].
/// `tests::between_can_never_construct_independent` proves that directly
/// against `between`'s own body, not merely by inspecting its current call
/// sites.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailureDomain {
    /// The same provider. The one positive fact this build can establish —
    /// certain, not a guess, because it is the identity `between` compares.
    Shared,
    /// A different provider. Not evidence of independence — see line 1378 —
    /// only the absence of the one signal that would say "shared".
    Unknown,
    /// Never produced in this build. See this type's own doc comment.
    Independent,
}

impl FailureDomain {
    /// The one failure-domain signal this build can observe: same provider,
    /// or not.
    ///
    /// Never returns [`FailureDomain::Independent`] — there is nothing this
    /// function reads that could justify it. A candidate on a different
    /// provider is [`FailureDomain::Unknown`], not proven independent.
    pub fn between(current: &Backend, candidate: &Backend) -> Self {
        if current.provider() == candidate.provider() {
            Self::Shared
        } else {
            Self::Unknown
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Shared => "shared",
            Self::Unknown => "unknown",
            Self::Independent => "independent",
        }
    }
}

impl std::fmt::Display for FailureDomain {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.pad(self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::routing::{AssignedModel, Cost, CredentialId, ToolSemantics};
    use crate::secret::SecretRef;

    fn backend(provider: &str, model: &str, var: &str) -> Backend {
        Backend::new(
            provider,
            "anthropic-messages",
            AssignedModel::named(model),
            CredentialId::new(
                provider,
                SecretRef::Environment {
                    var: var.to_owned(),
                },
            ),
            Cost::Metered,
            ToolSemantics::Unverified,
        )
    }

    /// Line 1371, proven on the types themselves: two credentials of one
    /// provider are two quota domains (`CredentialId`'s own `PartialEq`) and
    /// one failure domain (`FailureDomain::Shared`), for the same pair of
    /// backends.
    #[test]
    fn two_credentials_of_one_provider_are_two_quota_domains_and_one_failure_domain() {
        let a = backend("openrouter", "the-model", "OPENROUTER_API_KEY");
        let b = backend("openrouter", "the-model", "OPENROUTER_API_KEY_2");

        assert_ne!(
            a.credential(),
            b.credential(),
            "two credentials of one provider must be two different quota domains"
        );
        assert_eq!(
            FailureDomain::between(&a, &b),
            FailureDomain::Shared,
            "one provider is one failure domain, whatever credential either backend uses"
        );
    }

    #[test]
    fn a_different_provider_is_an_unknown_failure_domain_not_a_shared_one() {
        let a = backend("openrouter", "the-model", "OPENROUTER_API_KEY");
        let b = backend("nous", "the-model", "NOUS_API_KEY");
        assert_eq!(FailureDomain::between(&a, &b), FailureDomain::Unknown);
    }

    /// Line 1378, proven structurally rather than only by example: the only
    /// function in this crate that can produce a `FailureDomain` never even
    /// names `Independent` in its own body, so no future edit inside it can
    /// start returning one without this test's text-match noticing before
    /// any behavioural test would. This is the direct target of the
    /// packet's `alter-boundary` mutation ("return `Independent` where you
    /// return `Unknown`").
    #[test]
    fn between_can_never_construct_independent() {
        let source = include_str!("domain.rs");
        let start = source
            .find("pub fn between(")
            .expect("FailureDomain::between must exist");
        let after_start = &source[start..];
        let next_fn_offset = after_start
            .find("\n    pub fn ")
            .expect("as_str follows between in this file");
        let body = &after_start[..next_fn_offset];
        assert!(
            !body.contains("Independent"),
            "FailureDomain::between must never mention Independent in its own body — there is no \
             evidence source for it in this build (line 1378): {body}"
        );
    }
}
