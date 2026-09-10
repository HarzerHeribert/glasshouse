//! Routing policy: which backend serves which work, and why.
//!
//! [`classify`] is a third, independent thing: not a policy that picks a
//! backend, but the lightweight, model-optional classification of a request
//! (Phase 35) that a future policy — Phase 34F/35B, neither built yet — would
//! read before picking one; nothing here consumes a
//! [`classify::TaskClassification`] today.
//!
//! Phase 9I line 533's two policy classes are separated **structurally**:
//! [`interactive::InteractiveRouting`] and [`disposable::DisposableRouting`]
//! have distinct result types with no conversion between them; neither
//! module names the other
//! (`tests::the_two_policy_classes_do_not_name_each_other` scans both); and
//! they decide differently on identical input — given a catalogue where a
//! free and a paid model both serve, disposable picks the free one and
//! interactive keeps the backend the session started on.
//!
//! Nothing here opens a socket, resolves a credential, or reads the clock:
//! every function is pure over values the caller supplies, including `now`
//! (never [`std::time::Instant::now`] called inside a policy) — a policy
//! that could probe would eventually spend the free requests Phase 9I line
//! 534 protects, and one reading its own clock could not be tested for a
//! cooldown boundary without waiting for one.
//!
//! [`CredentialId`] holds a `SecretRef`, never a value, because Phase 9I
//! lines 537/538 require quota state per credential, so a credential is a
//! map key — and `SecretRef` is the one shape in Glasshouse already safe to
//! write into a tracked configuration file.
// History: design-decisions.md, "Trims: routing module docs", routing/mod.rs module doc.

pub mod analysis;
pub mod burn;
pub mod capability;
pub mod classify;
pub mod disposable;
pub mod entitlement_policy;
pub mod evidence;
pub mod pressure;
pub mod request;
pub mod session;

// The destination vocabulary, the entitlement rules, the failure-domain
// model and the serving policies all live in the gateway crate now; the
// paths below keep resolving so nothing in Glasshouse has to say so.
pub use entitlement_policy::*;
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
    ///
    /// Comment lines go because this module's own documentation names both
    /// policy classes in one breath while explaining why they do not name
    /// each other.
    /// `routing/session` is a directory since Phase 59 (`GH-DECOMP-ROUTING-SESSION`);
    /// the boundary scans below read every production file of it, joined.
    fn session_source() -> String {
        [
            include_str!("session/mod.rs"),
            include_str!("session/discovery.rs"),
            include_str!("session/scoring.rs"),
            include_str!("session/reserve.rs"),
        ]
        .join("\n")
    }

    /// `routing/disposable` is a directory since Phase 59 (`GH-DECOMP-DISPOSABLE`);
    /// the boundary scans below read every production file of it, joined, the
    /// same way [`session_source`] does for `session/` above.
    fn disposable_source() -> String {
        [
            include_str!("disposable/mod.rs"),
            include_str!("disposable/candidates.rs"),
            include_str!("disposable/classification.rs"),
        ]
        .join("\n")
    }

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

    /// Phase 9I line 533's structural half: neither policy module can reach
    /// the other, so nothing can quietly become one router with a flag.
    #[test]
    fn the_two_policy_classes_do_not_name_each_other() {
        let interactive = production_code(include_str!(
            "../../../inference-gateway/src/routing/interactive/mod.rs"
        ));
        assert!(
            !interactive.contains("disposable"),
            "routing/interactive.rs names the disposable policy class: the two policy classes \
             Phase 9I line 533 requires to stay separate have started to share code"
        );
        let disposable = production_code(&disposable_source());
        assert!(
            !disposable.contains("interactive"),
            "routing/disposable.rs names the interactive policy class: the two policy classes \
             Phase 9I line 533 requires to stay separate have started to share code"
        );
    }

    /// Phase 9I line 533's third case, and the one the original scan could
    /// not have anticipated: [`session`] is a policy class too.
    ///
    /// It ranks destinations rather than backends, so it legitimately names
    /// `interactive` — a destination's current backend is an interactive
    /// concern and the two are layers, not peers. It must never name
    /// `disposable`. A session router that could reach the throwaway-job
    /// policy is one careless call site away from sending a person's live
    /// coding session wherever a classification job would have gone, which is
    /// the exact failure line 533 exists to prevent.
    #[test]
    fn the_session_router_cannot_reach_the_disposable_policy_class() {
        let session = production_code(&session_source());
        assert!(
            !session.contains("disposable"),
            "routing/session.rs names the disposable policy class: a router that chooses where a \
             person's live session goes has reached the policy for throwaway jobs"
        );
    }

    /// The session router's project-isolation guarantee, structurally.
    ///
    /// Map lines 1593 and 1594 make it *rank sessions*, which is the first
    /// routing policy in Glasshouse with a reason to want to look one up —
    /// and a policy that could enumerate sessions would be one query away
    /// from ranking another project's. It cannot: warmth arrives as a
    /// [`crate::config::pairing::WarmSession`] the caller read, and
    /// checkpoint quality as two booleans the caller read, exactly as
    /// continuity already arrives at `interactive`. This is the same move
    /// `ContinuitySource`'s own doc comment describes, kept honest by a scan
    /// rather than by a convention.
    #[test]
    fn the_session_router_cannot_look_a_session_or_a_checkpoint_up() {
        let session = production_code(&session_source());
        for forbidden in ["crate::session", "crate::checkpoint", "SessionStore"] {
            assert!(
                !session.contains(forbidden),
                "routing/session.rs names `{forbidden}`: a router that can enumerate sessions \
                 can enumerate another project's, and project scoping would become a habit \
                 rather than a structure"
            );
        }
    }

    /// Phase 9I line 534's structural half. A health checker that spent the
    /// quota it protects would need a way to make a request; there is none in
    /// this module, and that absence is the capability.
    #[test]
    fn no_routing_policy_can_make_a_request() {
        for (name, source) in [
            ("routing/mod.rs", include_str!("mod.rs")),
            (
                "routing/interactive.rs",
                include_str!("../../../inference-gateway/src/routing/interactive/mod.rs"),
            ),
            (
                "routing/free.rs",
                include_str!("../../../inference-gateway/src/routing/free.rs"),
            ),
            ("routing/disposable.rs", disposable_source().as_str()),
            ("routing/session.rs", session_source().as_str()),
        ] {
            let code = production_code(source);
            for forbidden in ["ureq", "TcpStream", "reqwest", "std::net"] {
                assert!(
                    !code.contains(forbidden),
                    "{name} names `{forbidden}`: a routing policy that can open a connection can \
                     spend the free requests Phase 9I line 534 exists to protect"
                );
            }
        }
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

    /// Anything nobody marked is metered. The fail-closed direction: a router
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
