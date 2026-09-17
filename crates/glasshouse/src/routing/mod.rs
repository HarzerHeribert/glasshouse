//! What is left of `routing` after the 2026-09-16 ruling (design-decisions.md,
//! *Glasshouse never decides which model is used*): the entitlement policy
//! ([`entitlement_policy`], still consumed by `config`, `launch` and
//! `resources`) and the evidence ledger ([`evidence`], still the writer every
//! gateway exchange reports through and the reader `glasshouse cost` and the
//! shell's meters use). Everything that chose a destination, a disposable
//! resource, or a routing model — `session`, `disposable`, `burn`, `request`,
//! `classify`, `pressure`, `capability` — is deleted. [`analysis`] stays: it
//! caches published Artificial Analysis measurements for `pane`'s model
//! picker and decides nothing itself, so the ruling does not reach it.
//!
//! [`CredentialId`] holds a `SecretRef`, never a value, because a credential
//! is a map key — and `SecretRef` is the one shape in Glasshouse already
//! safe to write into a tracked configuration file.
// History: design-decisions.md, "Glasshouse never decides which model is used", GH-GLASSHOUSE-ROUTING-DELETE.

pub mod analysis;
pub mod entitlement_policy;
pub mod evidence;

// The destination vocabulary, the entitlement rules, the failure-domain
// model and the serving policies all live in the gateway crate now; the
// paths below keep resolving so nothing in Glasshouse has to say so.
pub use entitlement_policy::*;
pub use inference_gateway::routing::request::TaskClass;
pub use inference_gateway::routing::*;
pub use inference_gateway::routing::{domain, free, interactive, pairing, wire};

#[cfg(test)]
mod tests {
    use super::*;
    use inference_gateway::secret::SecretRef;

    /// A source file's production code: everything before the first
    /// `#[cfg(test)]`, with `//` comments stripped — the idiom
    /// `gateway/mod.rs`, `harness/mod.rs`, `main.rs`, `shim.rs` and
    /// `secret/mod.rs` each keep their own copy of.
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
            ToolSemantics::Verified,
        )
    }

    #[test]
    fn nothing_moved_preserves_the_cache() {
        let one = backend("openrouter", "z-ai/glm-4.5-air:free", "OPENROUTER_API_KEY");
        assert_eq!(CacheLocality::between(&one, &one), CacheLocality::Preserved);
        assert!(!CacheLocality::between(&one, &one).warrants_a_warning());
    }

    #[test]
    fn a_different_provider_or_model_loses_the_cache_certainly() {
        let from = backend("openrouter", "model-a", "OPENROUTER_API_KEY");

        let other_provider = backend("nous", "model-a", "NOUS_API_KEY");
        assert_eq!(
            CacheLocality::between(&from, &other_provider),
            CacheLocality::Lost(CacheLossReason::ProviderChanged)
        );

        let other_model = backend("openrouter", "model-b", "OPENROUTER_API_KEY");
        assert_eq!(
            CacheLocality::between(&from, &other_model),
            CacheLocality::Lost(CacheLossReason::ModelChanged)
        );

        let both = backend("nous", "model-b", "NOUS_API_KEY");
        assert_eq!(
            CacheLocality::between(&from, &both),
            CacheLocality::Lost(CacheLossReason::ProviderAndModelChanged)
        );
    }

    /// The case the map's word "likely" is about, and the one a rule written
    /// as "did the provider change" would miss entirely.
    #[test]
    fn rotating_a_credential_is_only_likely_to_lose_the_cache() {
        let from = backend("openrouter", "model-a", "OPENROUTER_API_KEY");
        let rotated = backend("openrouter", "model-a", "OPENROUTER_API_KEY_2");
        let locality = CacheLocality::between(&from, &rotated);
        assert_eq!(
            locality,
            CacheLocality::LikelyLost(CacheLossReason::CredentialChanged)
        );
        assert!(locality.warrants_a_warning());
        assert!(
            locality.to_string().contains("likely"),
            "a likelihood must be said as one, not asserted as a fact: {locality}"
        );
    }

    /// Two keys for the same router are two identities; the same variable
    /// name under two providers is also two identities.
    #[test]
    fn a_credential_identity_is_the_provider_and_the_reference_together() {
        let env = |var: &str| SecretRef::Environment {
            var: var.to_owned(),
        };
        assert_ne!(
            CredentialId::new("openrouter", env("OPENROUTER_API_KEY")),
            CredentialId::new("openrouter", env("OPENROUTER_API_KEY_2"))
        );
        assert_ne!(
            CredentialId::new("openrouter", env("SHARED")),
            CredentialId::new("nous", env("SHARED"))
        );
    }

    /// A label is a diagnostic, so it must carry names and nothing else.
    #[test]
    fn a_credential_label_is_two_names() {
        let id = CredentialId::new(
            "openrouter",
            SecretRef::Environment {
                var: "OPENROUTER_API_KEY".to_owned(),
            },
        );
        assert_eq!(id.label(), "openrouter/OPENROUTER_API_KEY");
    }

    /// Anything nobody marked is metered. The fail-closed direction: a caller
    /// that guessed "free" and was wrong spends the user's money.
    #[test]
    fn cost_has_no_third_state() {
        assert!(Cost::Free.is_free());
        assert!(!Cost::Metered.is_free());
    }

    /// A routing explanation is a plain ordered sum, and nothing here filters
    /// a contribution out for being zero or negative — the general surface
    /// must never itself become a hard rule.
    #[test]
    fn a_routing_explanation_sums_every_contribution_in_order() {
        let mut explanation = RoutingExplanation::new();
        explanation.push(Contribution::new("a", 1.0, "first"));
        explanation.push(Contribution::new("b", -0.25, "second"));
        explanation.push(Contribution::new("c", 0.0, "informational only"));

        assert_eq!(explanation.contributions().len(), 3);
        assert_eq!(explanation.contributions()[0].name(), "a");
        assert!((explanation.total() - 0.75).abs() < 1e-9);
        assert!(explanation.render().contains("informational only"));
    }

    /// The one function that can build an `EligibleCandidate`: candidates
    /// that fail `check` are rejected with a reason, and the rest come back
    /// wrapped, in the same order they went in.
    #[test]
    fn apply_hard_constraints_actually_filters_and_names_the_reason() {
        let candidates = vec![1, 2, 3, 4];
        let (eligible, rejected) = apply_hard_constraints(candidates, |n| {
            if *n % 2 == 0 {
                Ok(())
            } else {
                Err(HardConstraint::Protocol)
            }
        });

        assert_eq!(
            eligible
                .iter()
                .map(EligibleCandidate::value)
                .collect::<Vec<_>>(),
            vec![&2, &4]
        );
        assert_eq!(
            rejected,
            vec![(1, HardConstraint::Protocol), (3, HardConstraint::Protocol)]
        );
    }

    /// The structural half of design decision 2: nothing outside this module
    /// can construct an `EligibleCandidate` directly — its field is private
    /// and no `pub fn new`/`pub value` exists, so the only source is
    /// `apply_hard_constraints` actually running the check. A mutation that
    /// makes the field public or adds a bypass constructor is exactly what
    /// this guards.
    #[test]
    fn eligible_candidate_has_no_public_constructor_other_than_the_filter() {
        let code = production_code(include_str!("mod.rs"));
        assert!(
            !code.contains("pub value: T"),
            "EligibleCandidate's field must stay private, or a caller could build one without \
             passing through apply_hard_constraints"
        );
        assert!(
            !code.contains("impl<T> EligibleCandidate<T> {\n    pub fn new"),
            "a public constructor on EligibleCandidate would let a caller skip \
             apply_hard_constraints entirely"
        );
    }
}
