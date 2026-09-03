//! The inline test modules moved verbatim out of `session.rs` (Phase 59 line 2049).

use super::discovery::is_adequate;
use super::reserve::{RESET_BURN_HORIZON_SECONDS, RESET_PRESERVE_HORIZON_SECONDS, burn_urgency};
use super::scoring::harness_efficiency;
use super::*;

#[cfg(test)]
mod tool_evidence_tests {
    use super::*;
    use crate::routing::{AssignedModel, Cost, CredentialId};
    use crate::secret::SecretRef;

    fn backend(tools: ToolSemantics, evidence: Option<&'static str>) -> Backend {
        Backend::new(
            "anthropic",
            "anthropic-messages",
            AssignedModel::named("claude-opus-4"),
            CredentialId::new(
                "anthropic",
                SecretRef::Environment {
                    var: "TOOL_EVIDENCE_TEST_KEY".to_owned(),
                },
            ),
            Cost::Metered,
            tools,
        )
        .with_tools_evidence(evidence)
    }

    /// GH-TOOL-SEMANTICS-EVIDENCE's own inert-evidence guarantee: evidence
    /// is only meaningful beside `ToolSemantics::KnownAbsent`, so a `Backend`
    /// carrying it beside `Verified` (which no honest producer builds, but
    /// which `hard_constraint` must not be fooled by) is never rejected on
    /// tool semantics.
    #[test]
    fn verified_tools_with_stray_evidence_never_raises_a_tool_semantics_constraint() {
        let now = Instant::now();
        let overrides = pairing::PairingOverrides::default();
        let health = FreePool::new();
        let inputs = RouterInputs {
            overrides: &overrides,
            health: &health,
            now,
            requirements: TaskRequirements {
                needs_tool_calls: true,
                ..TaskRequirements::default()
            },
        };
        let destination = Destination::fresh(
            "verified-with-evidence",
            IntegrationId::ClaudeCode,
            "profile",
            backend(ToolSemantics::Verified, Some("stray evidence")),
            None,
        );

        let result = hard_constraint(&destination, &inputs, None, false);

        assert!(
            result.is_ok(),
            "evidence beside a Verified verdict must not raise a tool-semantics constraint: \
             {result:?}"
        );
    }
}

#[cfg(test)]
mod burn_urgency_tests {
    use super::{RESET_BURN_HORIZON_SECONDS, RESET_PRESERVE_HORIZON_SECONDS, burn_urgency};

    /// Line 1967's reset-boundary term: a reset within the burn horizon is
    /// urgent (+1.0), at or past the preserve horizon it is not (0.0) — and a
    /// reset ALREADY PAST (negative seconds, routine over the persisted,
    /// deliberately un-staled capacity cache) must score 0.0, never the max
    /// +1.0 an unguarded `<= horizon` gave it. The 2026-08-31 investigation
    /// swarm caught the unguarded form rewarding the *stalest* account over a
    /// fresh healthy one, inverting this line's own intent.
    #[test]
    fn a_reset_already_past_is_not_urgent() {
        assert_eq!(burn_urgency(4_800), 1.0, "1h20m — the user's burn example");
        assert_eq!(burn_urgency(RESET_BURN_HORIZON_SECONDS), 1.0);
        assert_eq!(burn_urgency(RESET_PRESERVE_HORIZON_SECONDS), 0.0);
        assert_eq!(burn_urgency(0), 0.0, "resetting now is already replenished");
        assert_eq!(burn_urgency(-1), 0.0, "one second past reset");
        assert_eq!(
            burn_urgency(-345_600),
            0.0,
            "days past — the stale-cache case"
        );
    }
}

#[cfg(test)]
mod harness_efficiency_tests {
    use super::*;
    use crate::routing::classify::{
        ClassificationSource, Complexity, Confidence, WarmContextValue,
    };
    use crate::routing::request::{AnswerProvenance, RouterAnswer};

    fn destination(id: &str, harness: IntegrationId) -> Destination {
        Destination::fresh(
            id,
            harness,
            "profile",
            Backend::new(
                "provider",
                "anthropic-messages",
                crate::routing::AssignedModel::named("a-model"),
                crate::routing::CredentialId::new(
                    "provider",
                    crate::secret::SecretRef::Environment {
                        var: "TEST_KEY".to_owned(),
                    },
                ),
                crate::routing::Cost::Metered,
                ToolSemantics::Verified,
            ),
            None,
        )
    }

    /// A stated task classified `Standard` at `Confidence::High` — not
    /// conservative, so `required_tier() == stated_tier()` and
    /// `task_class_bucket` reads the un-escalated bucket `"standard"`,
    /// [`RoutingTier::as_str`]'s own vocabulary for the row this summary
    /// looks up.
    fn standard_task() -> TaskRequirements {
        let classification = TaskClassification::new(
            false,
            true,
            false,
            false,
            Complexity::Moderate,
            false,
            WorkloadTier::Standard,
            false,
            WarmContextValue::PreferStrongerCold,
            Confidence::High,
            ClassificationSource::Model {
                label: "the-routing-model".to_owned(),
            },
        );
        RouterAnswer::new(
            classification,
            AnswerProvenance::Model {
                label: "the-routing-model".to_owned(),
            },
        )
        .requirements()
    }

    fn measured(
        harness: &str,
        task_class: &str,
        successful: i64,
        sample_size: i64,
    ) -> HarnessTierOutcome {
        HarnessTierOutcome {
            harness: harness.to_owned(),
            outcome: crate::evaluation::TierOutcome {
                bucket: task_class.to_owned(),
                undecided: 0,
                verdict: TierOutcomeVerdict::Measured {
                    successful,
                    failed: sample_size - successful,
                    sample_size,
                },
            },
        }
    }

    /// Map line 1952's own case: two harnesses in the candidate set, one
    /// with a clearly better recorded rate on the classified task's class —
    /// the better harness gets the positive term and names the harness, the
    /// class, the sample count and the rate.
    #[test]
    fn the_harness_with_the_better_recorded_rate_gets_the_positive_term() {
        let summary = HarnessEfficiencySummary::from_outcomes(&[
            measured("claude-code", "standard", 9, 10),
            measured("codex", "standard", 5, 10),
        ]);
        let requirements = standard_task();
        let candidates: BTreeSet<&str> = ["claude-code", "codex"].into_iter().collect();

        let better = destination("better", IntegrationId::ClaudeCode);
        let worse = destination("worse", IntegrationId::Codex);

        let better_term = harness_efficiency(&better, &summary, &requirements, &candidates);
        let worse_term = harness_efficiency(&worse, &summary, &requirements, &candidates);

        assert!(
            better_term.magnitude() > 0.0,
            "the better-observed harness must get a positive term: {better_term:?}"
        );
        assert!(
            worse_term.magnitude() < 0.0,
            "the worse-observed harness must get a negative term: {worse_term:?}"
        );
        assert!(
            better_term.magnitude() < 1.5,
            "must stay below warm affinity"
        );
        assert!(
            better_term.evidence().contains("claude-code")
                && better_term.evidence().contains("standard")
                && better_term.evidence().contains('9')
                && better_term.evidence().contains("10"),
            "the explanation must name the harness, the class, the sample count and the rate: {}",
            better_term.evidence()
        );
    }

    /// Below `MIN_SAMPLE_FOR_SUMMARY` for this destination's own harness and
    /// class: `0.0`, and the text says so, regardless of what other
    /// harnesses recorded.
    #[test]
    fn below_the_sample_gate_the_term_is_inert() {
        let summary = HarnessEfficiencySummary::from_outcomes(&[
            measured("claude-code", "standard", 2, 3),
            measured("codex", "standard", 5, 10),
        ]);
        let requirements = standard_task();
        let candidates: BTreeSet<&str> = ["claude-code", "codex"].into_iter().collect();
        let destination = destination("thin-evidence", IntegrationId::ClaudeCode);

        let term = harness_efficiency(&destination, &summary, &requirements, &candidates);
        assert_eq!(term.magnitude(), 0.0);
        assert!(term.evidence().contains("inert"), "{}", term.evidence());
    }

    /// An empty summary — the caller-with-no-ledger case — is inert exactly
    /// the same way, so a build that never wires a ledger in scores exactly
    /// as it did before this term existed.
    #[test]
    fn an_empty_summary_is_inert() {
        let requirements = standard_task();
        let candidates: BTreeSet<&str> = ["claude-code", "codex"].into_iter().collect();
        let destination = destination("no-ledger", IntegrationId::ClaudeCode);

        let term = harness_efficiency(
            &destination,
            &HarnessEfficiencySummary::empty(),
            &requirements,
            &candidates,
        );
        assert_eq!(term.magnitude(), 0.0);
    }

    /// No task classified: nothing to compare a rate within, whatever the
    /// summary holds.
    #[test]
    fn with_no_task_classified_the_term_is_inert() {
        let summary = HarnessEfficiencySummary::from_outcomes(&[
            measured("claude-code", "standard", 9, 10),
            measured("codex", "standard", 5, 10),
        ]);
        let candidates: BTreeSet<&str> = ["claude-code", "codex"].into_iter().collect();
        let destination = destination("no-task", IntegrationId::ClaudeCode);

        let term = harness_efficiency(
            &destination,
            &summary,
            &TaskRequirements::default(),
            &candidates,
        );
        assert_eq!(term.magnitude(), 0.0);
    }

    /// Map line 1952's own assignment clause: a candidate set already scoped
    /// to one harness — exactly what `launch_session` builds when the user
    /// named a harness — has no "other" harness to compare against, so the
    /// term is inert and the assigned harness is never moved off, whatever
    /// the ledger says about other harnesses entirely outside this set.
    #[test]
    fn a_single_harness_candidate_set_cannot_be_re_ranked_across_harnesses() {
        let summary = HarnessEfficiencySummary::from_outcomes(&[
            measured("claude-code", "standard", 9, 10),
            measured("codex", "standard", 1, 10),
        ]);
        let requirements = standard_task();
        // Only `claude-code` is offered — the user assigned it.
        let candidates: BTreeSet<&str> = ["claude-code"].into_iter().collect();
        let destination = destination("assigned", IntegrationId::ClaudeCode);

        let term = harness_efficiency(&destination, &summary, &requirements, &candidates);
        assert_eq!(
            term.magnitude(),
            0.0,
            "one harness offered is nothing to prefer among"
        );
    }
}

#[cfg(test)]
mod provider_health_tests {
    use super::*;
    use crate::routing::{AssignedModel, Cost, CredentialId};
    use crate::secret::SecretRef;

    fn destination(credential_var: &str) -> Destination {
        Destination::fresh(
            "dest-1",
            IntegrationId::ClaudeCode,
            "profile",
            Backend::new(
                "anthropic",
                "anthropic-messages",
                AssignedModel::named("claude-opus-4-1"),
                CredentialId::new(
                    "anthropic",
                    SecretRef::Environment {
                        var: credential_var.to_owned(),
                    },
                ),
                Cost::Metered,
                ToolSemantics::Verified,
            ),
            None,
        )
    }

    /// A pool whose only recorded fact is `failures` consecutive failures on
    /// `destination`'s resource, with no cooldown in effect — built through
    /// [`FreePool::adopt_observed`], the public entry point that states a
    /// failure count directly rather than deriving one from timed `observe`
    /// calls, so the test needs no assumption about `routing::free`'s
    /// cooldown length.
    fn health_with_failures(destination: &Destination, failures: u32) -> FreePool {
        let mut pool = FreePool::new();
        let resource = FreeResource::new(
            destination.backend().credential().clone(),
            destination.backend().model().label(),
        );
        pool.adopt_observed(&resource, failures, None, None, false);
        pool
    }

    /// Line 1353: keep an *additive* failure penalty, not a boolean one —
    /// two consecutive failures must price worse than one, and the additive
    /// climb must still be bounded at [`HEALTH_PENALTY_FLOOR`] rather than
    /// worsening without limit.
    #[test]
    fn the_failure_penalty_is_additive_and_bounded() {
        let now = Instant::now();
        let dest = destination("PROVIDER_HEALTH_TEST_KEY");

        let weights = ScoreWeights::default();
        let one = provider_health(&dest, &health_with_failures(&dest, 1), now, &weights);
        let two = provider_health(&dest, &health_with_failures(&dest, 2), now, &weights);
        assert!(
            two.magnitude() < one.magnitude(),
            "two consecutive failures ({}) must price worse than one ({}) — \
             an additive penalty, not a boolean",
            two.magnitude(),
            one.magnitude()
        );

        let many = provider_health(&dest, &health_with_failures(&dest, 50), now, &weights);
        assert_eq!(
            many.magnitude(),
            HEALTH_PENALTY_FLOOR,
            "the additive climb is bounded, never worsening without limit"
        );
    }
}

/// Map lines 1517 and 1518 — the two new `hard_constraint` exclusion arms —
/// driven through `SessionRouter::choose`, the real production path, per
/// `GH-CANDIDATE-GEN`'s acceptance tests.
#[cfg(test)]
mod hard_constraint_tests {
    use super::*;
    use crate::config::pairing::WarmSessionState;
    use crate::routing::free::CooldownCause;
    use crate::routing::{AssignedModel, Cost, CredentialId};
    use crate::secret::SecretRef;
    use std::time::Duration;

    fn anthropic_destination(id: &str, credential_var: &str) -> Destination {
        Destination::fresh(
            id,
            IntegrationId::ClaudeCode,
            "profile",
            Backend::new(
                "anthropic",
                "anthropic-messages",
                AssignedModel::named("claude-opus-4-1"),
                CredentialId::new(
                    "anthropic",
                    SecretRef::Environment {
                        var: credential_var.to_owned(),
                    },
                ),
                Cost::Metered,
                ToolSemantics::Verified,
            ),
            None,
        )
    }

    /// A gateway-backed candidate, built the same way `main.rs::destination_backend`
    /// builds one for `BackendResource::GlasshouseGateway` — the provider and
    /// credential name it, never a routing-level type, so this is what "gateway
    /// candidate" means at this layer.
    fn gateway_destination(id: &str) -> Destination {
        Destination::fresh(
            id,
            IntegrationId::ClaudeCode,
            "profile",
            Backend::new(
                "the Glasshouse gateway",
                "anthropic-messages",
                AssignedModel::named("claude-opus-4-1"),
                CredentialId::new(
                    "the Glasshouse gateway",
                    SecretRef::OsCredential {
                        service: "glasshouse-gateway".to_owned(),
                        account: "assigned when the session starts".to_owned(),
                    },
                ),
                Cost::Metered,
                ToolSemantics::Verified,
            ),
            None,
        )
    }

    fn inputs<'a>(
        overrides: &'a pairing::PairingOverrides,
        health: &'a FreePool,
        now: Instant,
        requirements: TaskRequirements,
    ) -> RouterInputs<'a> {
        RouterInputs {
            overrides,
            health,
            now,
            requirements,
        }
    }

    /// Line 1517. A gateway-backed candidate established to lack a required
    /// hard capability is excluded outright, never merely scored; an
    /// unverified axis on the surviving candidate still passes and is priced
    /// by `capability_fit` exactly as before this gate existed. The gateway
    /// candidate also stands as line 1513's capability-half production
    /// evidence: a fresh gateway-backed candidate is filtered by the same
    /// hard-constraint gate as every other backend.
    #[test]
    fn an_established_absent_hard_capability_excludes_and_an_unverified_one_passes() {
        let now = Instant::now();
        let overrides = pairing::PairingOverrides::default();
        let health = FreePool::new();

        let lacking = gateway_destination("gateway-no-shell").with_resource_facts(ResourceFacts {
            shell_tool_use: Declared::verified(false, "test evidence"),
            ..ResourceFacts::UNVERIFIED
        });
        let adequate = anthropic_destination("anthropic-unverified", "CAP_TEST_KEY");

        let requirements = TaskRequirements {
            hard_capabilities: vec![HardCapability::ShellExecution],
            ..TaskRequirements::default()
        };
        let router_inputs = inputs(&overrides, &health, now, requirements);

        let routed = SessionRouter::new()
            .choose(
                RoutingMoment::SessionStart,
                None,
                &[lacking.clone(), adequate.clone()],
                &router_inputs,
            )
            .expect("an adequate destination was offered");

        assert_eq!(
            routed.chosen().id(),
            "anthropic-unverified",
            "an established-absent capability must not win over an adequate destination"
        );
        assert_eq!(routed.rejected().len(), 1);
        assert_eq!(routed.rejected()[0].0.id(), "gateway-no-shell");
        assert_eq!(
            routed.rejected()[0].1,
            HardConstraint::Capability {
                axis: capability::CapabilityAxis::ShellToolUse,
                evidence: "test evidence",
            }
        );
        assert!(
            routed
                .considered()
                .iter()
                .any(|(d, _)| d.id() == "anthropic-unverified"),
            "an unverified axis must still be scored, not excluded"
        );
    }

    /// `GH-CONSTRAINT-REASONS` same-crate test: `is_adequate` must report the
    /// requirement that is actually established absent, not merely the
    /// first requirement in `hard_capabilities` — every other test in this
    /// module has only one requirement, so the two read the same there. This
    /// one names two, in an order where they would tell them apart.
    #[test]
    fn is_adequate_reports_the_failing_axis_not_merely_the_first_requirement() {
        let destination = gateway_destination("mixed-facts").with_resource_facts(ResourceFacts {
            code_edit: Declared::verified(true, "present evidence"),
            shell_tool_use: Declared::verified(false, "absent evidence"),
            ..ResourceFacts::UNVERIFIED
        });
        let requirements = TaskRequirements {
            hard_capabilities: vec![
                HardCapability::RepositoryAccess,
                HardCapability::ShellExecution,
            ],
            ..TaskRequirements::default()
        };

        assert_eq!(
            is_adequate(&destination, &requirements),
            Some((capability::CapabilityAxis::ShellToolUse, "absent evidence")),
            "the first requirement (repository access) is established present; only the \
             second (shell execution) is established absent, and that is the axis a caller \
             must be told about"
        );
    }

    /// Line 1518. A credential the provider refused is excluded, never merely
    /// priced worse.
    #[test]
    fn a_credential_the_provider_rejected_is_excluded() {
        let now = Instant::now();
        let overrides = pairing::PairingOverrides::default();
        let rejected_dest = anthropic_destination("rejected", "REJECTED_TEST_KEY");
        let healthy_dest = anthropic_destination("healthy", "HEALTHY_TEST_KEY");

        let mut health = FreePool::new();
        let resource = FreeResource::new(
            rejected_dest.backend().credential().clone(),
            rejected_dest.backend().model().label(),
        );
        health.adopt_observed(&resource, 0, None, None, true);

        let router_inputs = inputs(&overrides, &health, now, TaskRequirements::default());
        let routed = SessionRouter::new()
            .choose(
                RoutingMoment::SessionStart,
                None,
                &[rejected_dest.clone(), healthy_dest.clone()],
                &router_inputs,
            )
            .expect("a healthy destination was offered");

        assert_eq!(routed.chosen().id(), "healthy");
        assert_eq!(routed.rejected().len(), 1);
        assert_eq!(routed.rejected()[0].0.id(), "rejected");
        assert_eq!(
            routed.rejected()[0].1,
            HardConstraint::ProviderUnavailable {
                credential: rejected_dest.backend().credential().label(),
                cause: ProviderUnavailableCause::CredentialRejected,
            }
        );
        assert!(
            routed.rejected()[0]
                .1
                .reason()
                .expect("a provider-unavailable constraint always carries a reason")
                .contains("refused by its provider"),
            "the refusal reason must be a sentence a person can read"
        );
    }

    /// Line 1518. A cooldown the provider itself declared, still in force at
    /// `inputs.now`, is authoritative per line 1319 and excludes.
    #[test]
    fn a_declared_cooldown_still_in_force_is_excluded() {
        let now = Instant::now();
        let overrides = pairing::PairingOverrides::default();
        let cooling_dest = anthropic_destination("cooling", "DECLARED_TEST_KEY");
        let healthy_dest = anthropic_destination("healthy", "HEALTHY_TEST_KEY_2");

        let mut health = FreePool::new();
        let resource = FreeResource::new(
            cooling_dest.backend().credential().clone(),
            cooling_dest.backend().model().label(),
        );
        health.adopt_observed(
            &resource,
            1,
            Some(now + Duration::from_secs(120)),
            Some(CooldownCause::Declared),
            false,
        );

        let router_inputs = inputs(&overrides, &health, now, TaskRequirements::default());
        let routed = SessionRouter::new()
            .choose(
                RoutingMoment::SessionStart,
                None,
                &[cooling_dest.clone(), healthy_dest.clone()],
                &router_inputs,
            )
            .expect("a healthy destination was offered");

        assert_eq!(routed.chosen().id(), "healthy");
        assert_eq!(routed.rejected().len(), 1);
        assert_eq!(routed.rejected()[0].0.id(), "cooling");
        assert_eq!(
            routed.rejected()[0].1,
            HardConstraint::ProviderUnavailable {
                credential: cooling_dest.backend().credential().label(),
                cause: ProviderUnavailableCause::DeclaredCooldown,
            }
        );
    }

    /// Line 1518's own preservation clause. An *invented* cooldown — line 534's
    /// bounded backoff Glasshouse imposed on itself — is not authoritative,
    /// so it must never exclude and must keep pricing exactly as
    /// `provider_health` did before this gate existed.
    #[test]
    fn an_invented_cooldown_is_priced_softly_and_never_excludes() {
        let now = Instant::now();
        let overrides = pairing::PairingOverrides::default();
        let cooling_dest = anthropic_destination("cooling", "INVENTED_TEST_KEY");
        let other_dest = anthropic_destination("other", "OTHER_TEST_KEY");

        let mut health = FreePool::new();
        let resource = FreeResource::new(
            cooling_dest.backend().credential().clone(),
            cooling_dest.backend().model().label(),
        );
        health.adopt_observed(
            &resource,
            3,
            Some(now + Duration::from_secs(60)),
            Some(CooldownCause::Invented),
            false,
        );

        let router_inputs = inputs(&overrides, &health, now, TaskRequirements::default());
        let routed = SessionRouter::new()
            .choose(
                RoutingMoment::SessionStart,
                None,
                &[cooling_dest.clone(), other_dest.clone()],
                &router_inputs,
            )
            .expect("a destination was offered");

        assert!(
            routed.rejected().is_empty(),
            "an invented cooldown must not exclude — line 534 keeps it probeable by real work"
        );
        assert!(
            routed.considered().iter().any(|(d, _)| d.id() == "cooling"),
            "the cooling destination must still be scored, not excluded"
        );
    }

    /// The gate applies to an existing (warm) session exactly as it does to a
    /// fresh one — a session already running cannot serve either, if its
    /// provider has refused the credential.
    #[test]
    fn an_existing_warm_session_is_excluded_when_its_provider_is_unavailable() {
        let now = Instant::now();
        let overrides = pairing::PairingOverrides::default();
        let warm_backend = Backend::new(
            "anthropic",
            "anthropic-messages",
            AssignedModel::named("claude-opus-4-1"),
            CredentialId::new(
                "anthropic",
                SecretRef::Environment {
                    var: "WARM_REJECTED_KEY".to_owned(),
                },
            ),
            Cost::Metered,
            ToolSemantics::Verified,
        );
        let warm_dest = Destination::existing(
            "warm",
            IntegrationId::ClaudeCode,
            "profile",
            warm_backend,
            WarmSession {
                state: WarmSessionState::Live,
                idle_seconds: 0,
            },
        );
        let fresh_dest = anthropic_destination("fresh", "FRESH_KEY");

        let mut health = FreePool::new();
        let resource = FreeResource::new(
            warm_dest.backend().credential().clone(),
            warm_dest.backend().model().label(),
        );
        health.adopt_observed(&resource, 0, None, None, true);

        let router_inputs = inputs(&overrides, &health, now, TaskRequirements::default());
        let routed = SessionRouter::new()
            .choose(
                RoutingMoment::SessionStart,
                Some(&warm_dest),
                &[warm_dest.clone(), fresh_dest.clone()],
                &router_inputs,
            )
            .expect("a fresh destination was offered");

        assert_eq!(
            routed.chosen().id(),
            "fresh",
            "an existing session must not be favoured over the gate that excludes its unavailable provider"
        );
        assert_eq!(routed.rejected().len(), 1);
        assert_eq!(routed.rejected()[0].0.id(), "warm");
    }

    /// With no candidate either new arm would touch, the gate excludes
    /// nothing extra: both candidates are still scored, destination order is
    /// still the tiebreaker, and no "rejected" section renders — the ranking
    /// and explanation this package must not disturb.
    #[test]
    fn a_candidate_set_with_no_excluded_candidate_ranks_exactly_as_before_this_gate() {
        let now = Instant::now();
        let overrides = pairing::PairingOverrides::default();
        let health = FreePool::new();
        let first = anthropic_destination("first", "INERT_TEST_KEY_1");
        let second = anthropic_destination("second", "INERT_TEST_KEY_2");

        let router_inputs = inputs(&overrides, &health, now, TaskRequirements::default());
        let routed = SessionRouter::new()
            .choose(
                RoutingMoment::SessionStart,
                None,
                &[first.clone(), second.clone()],
                &router_inputs,
            )
            .expect("two destinations were offered");

        assert!(
            routed.rejected().is_empty(),
            "neither candidate should be excluded by the new gate arms"
        );
        assert_eq!(
            routed.considered().len(),
            2,
            "both candidates must still be scored"
        );
        assert_eq!(
            routed.chosen().id(),
            "first",
            "with every term tied, destination order is still the tiebreaker"
        );
        assert!(
            !routed.render_overview().contains("rejected"),
            "no rejected section renders when nothing is excluded"
        );
    }
}

/// Line 1302, driven through `SessionRouter::choose`, the real production
/// path — `GH-REQUEST-POOL-COST`'s acceptance tests.
#[cfg(test)]
mod request_pool_cost_tests {
    use super::*;
    use crate::routing::burn::ExhaustionForecast;
    use crate::routing::free::PoolReading;
    use crate::routing::{AssignedModel, Cost, CredentialId};
    use crate::secret::SecretRef;

    /// A fresh destination on its own provider (so its credential, and thus
    /// its `Allowance`, never collides with another destination's), free of
    /// charge — the money axis is not what these tests differ in.
    fn destination(id: &str) -> Destination {
        Destination::fresh(
            id,
            IntegrationId::ClaudeCode,
            "profile",
            Backend::new(
                format!("{id}-provider"),
                "anthropic-messages",
                AssignedModel::named("a-model"),
                CredentialId::new(
                    format!("{id}-provider"),
                    SecretRef::Environment {
                        var: format!("{}_KEY", id.to_uppercase()),
                    },
                ),
                Cost::Free,
                ToolSemantics::Verified,
            ),
            None,
        )
    }

    fn routed_with(health: &FreePool, destinations: &[Destination]) -> Routed {
        let overrides = pairing::PairingOverrides::default();
        let router_inputs = RouterInputs {
            overrides: &overrides,
            health,
            now: Instant::now(),
            requirements: TaskRequirements::default(),
        };
        SessionRouter::new()
            .choose(
                RoutingMoment::SessionStart,
                None,
                destinations,
                &router_inputs,
            )
            .expect("a non-empty candidate set is always routed")
    }

    fn term<'a>(routed: &'a Routed, destination: &str) -> &'a Contribution {
        routed
            .considered()
            .iter()
            .find(|(d, _)| d.id() == destination)
            .unwrap_or_else(|| panic!("`{destination}` was not ranked"))
            .1
            .contributions()
            .iter()
            .find(|c| c.name() == REQUEST_POOL_COST_TERM)
            .unwrap_or_else(|| panic!("`{destination}` carried no `{REQUEST_POOL_COST_TERM}` term"))
    }

    /// A pool spending fast (rate and remaining known, not forecast to
    /// exhaust well before its reset — here, no reset is known at all) scores
    /// lower than an otherwise identical token-priced destination, and names
    /// the pool, the count and the rate.
    #[test]
    fn a_request_pool_spending_fast_scores_lower_than_a_token_priced_twin() {
        let pool_dest = destination("pool");
        let token_dest = destination("token");

        let mut health = FreePool::new();
        health.record_pool(
            pool_dest.backend().credential(),
            &PoolReading {
                limit: Some(1_000),
                remaining: Some(40),
                resets_in: None,
                window: None,
            },
            Instant::now(),
        );
        health.declare_token_priced(token_dest.backend().credential());

        let pool_dest = pool_dest.with_burn_forecast(Some(ExhaustionForecast {
            requests_per_hour: 20.0,
            seconds_to_exhaustion: 7_200,
            survives_until_reset: None,
            seconds_until_reset: None,
            rows: 12,
        }));

        let routed = routed_with(&health, &[pool_dest.clone(), token_dest.clone()]);

        let pool_term = term(&routed, "pool");
        let token_term = term(&routed, "token");

        assert!(
            pool_term.magnitude() < 0.0,
            "a fast-spending pool must cost something: {pool_term:?}"
        );
        assert!(
            pool_term.magnitude() > REQUEST_POOL_COST_PENALTY,
            "bounded: {pool_term:?}"
        );
        assert!(
            pool_term.magnitude() > -1.5,
            "must stay below warm affinity: {pool_term:?}"
        );
        assert_eq!(token_term.magnitude(), 0.0);
        assert!(token_term.evidence().contains("inert"), "{token_term:?}");

        assert!(
            pool_term.evidence().contains("request pool"),
            "{}",
            pool_term.evidence()
        );
        assert!(
            pool_term.evidence().contains("40"),
            "must name the remaining count: {}",
            pool_term.evidence()
        );
        assert!(
            pool_term.evidence().contains("20.0"),
            "must name the rate: {}",
            pool_term.evidence()
        );

        assert_eq!(
            routed.chosen().id(),
            "token",
            "the request-pool destination is the only one this term prices, so it must \
             score lower:\n{}",
            routed.render_overview()
        );
    }

    /// The forecast term already prices a resource forecast to exhaust well
    /// before its reset — this term must read `0.0` and say the forecast term
    /// already prices it, and no destination may carry both magnitudes
    /// non-zero.
    #[test]
    fn inert_when_the_exhaustion_forecast_term_already_prices_the_resource() {
        let pool_dest = destination("early");

        let mut health = FreePool::new();
        health.record_pool(
            pool_dest.backend().credential(),
            &PoolReading {
                limit: Some(1_000),
                remaining: Some(40),
                resets_in: None,
                window: None,
            },
            Instant::now(),
        );

        let pool_dest = pool_dest.with_burn_forecast(Some(ExhaustionForecast {
            requests_per_hour: 40.0,
            seconds_to_exhaustion: 1_000,
            survives_until_reset: Some(false),
            seconds_until_reset: Some(3_000),
            rows: 60,
        }));

        let routed = routed_with(&health, std::slice::from_ref(&pool_dest));

        let pool_cost = term(&routed, "early");
        assert_eq!(pool_cost.magnitude(), 0.0);
        assert!(
            pool_cost.evidence().contains("already prices"),
            "{}",
            pool_cost.evidence()
        );

        let (_, explanation) = routed
            .considered()
            .iter()
            .find(|(d, _)| d.id() == "early")
            .expect("destination was ranked");
        let forecast_term = explanation
            .contributions()
            .iter()
            .find(|c| c.name() == "exhaustion forecast")
            .expect("the exhaustion forecast term is always pushed");
        assert_ne!(
            forecast_term.magnitude(),
            0.0,
            "the forecast term must be the one carrying the penalty here"
        );
        assert!(
            !(pool_cost.magnitude() != 0.0 && forecast_term.magnitude() != 0.0),
            "one forecast must never be priced by both terms at once"
        );
    }

    /// Token-priced, or nothing established about the pool: `0.0`, inert
    /// text, and a ranking byte-identical to what it was before this term
    /// existed.
    #[test]
    fn token_priced_or_unknown_is_inert_and_ranking_is_unchanged() {
        let a = destination("a");
        let b = destination("b");

        // Neither credential is ever touched: `FreePool::allowance` answers
        // `Allowance::unknown_pool()` for both, which has no remaining count
        // established.
        let health = FreePool::new();
        let routed = routed_with(&health, &[a.clone(), b.clone()]);

        let a_term = term(&routed, "a");
        assert_eq!(a_term.magnitude(), 0.0);
        assert!(a_term.evidence().contains("inert"), "{}", a_term.evidence());

        let total_a = routed
            .considered()
            .iter()
            .find(|(d, _)| d.id() == "a")
            .unwrap()
            .1
            .total();
        let total_b = routed
            .considered()
            .iter()
            .find(|(d, _)| d.id() == "b")
            .unwrap()
            .1
            .total();
        assert_eq!(
            total_a,
            total_b,
            "with nothing established on either, nothing separates them:\n{}",
            routed.render_overview()
        );

        // And an explicit token-priced declaration is inert the same way,
        // even with a burn forecast attached — a token budget is never asked
        // how many requests are left.
        let mut token_health = FreePool::new();
        token_health.declare_token_priced(a.backend().credential());
        let a_token_priced = a.clone().with_burn_forecast(Some(ExhaustionForecast {
            requests_per_hour: 20.0,
            seconds_to_exhaustion: 7_200,
            survives_until_reset: None,
            seconds_until_reset: None,
            rows: 12,
        }));
        let routed = routed_with(&token_health, &[a_token_priced]);
        let a_term = term(&routed, "a");
        assert_eq!(a_term.magnitude(), 0.0);
        assert!(a_term.evidence().contains("token"), "{}", a_term.evidence());
    }
}

#[cfg(test)]
mod pairing_prior_tests {
    use super::*;
    use crate::config::pairing::WarmSessionState;
    use crate::routing::{AssignedModel, Cost, CredentialId};
    use crate::secret::SecretRef;

    /// `claude-fable-5` under Claude Code is `PairingClass::VendorNative`
    /// (`crate::harness::pairing::tests::a_vendor_native_pairing_needs_the_family_and_the_developer`).
    /// `gpt-5.5` under Claude Code is not — attributed to a different vendor
    /// than Claude Code's own, so it never satisfies the family-and-developer
    /// check regardless of route (the comment on
    /// `crate::harness::pairing::tests::a_harness_speaking_anthropic_messages_on_a_chat_only_route_is_translated`).
    /// Both share the same wire protocol Claude Code itself speaks, so the
    /// only axis a fresh pair built from these two ever varies on is the one
    /// this package adds.
    const NATIVE_MODEL: &str = "claude-fable-5";
    const OTHER_MODEL: &str = "gpt-5.5";

    fn backend(model: &str, credential_var: &str) -> Backend {
        Backend::new(
            "anthropic",
            "anthropic-messages",
            AssignedModel::named(model),
            CredentialId::new(
                "anthropic",
                SecretRef::Environment {
                    var: credential_var.to_owned(),
                },
            ),
            Cost::Metered,
            ToolSemantics::Verified,
        )
    }

    fn fresh(id: &str, model: &str, credential_var: &str) -> Destination {
        Destination::fresh(
            id,
            IntegrationId::ClaudeCode,
            "profile",
            backend(model, credential_var),
            None,
        )
    }

    fn warm(id: &str, model: &str, credential_var: &str, idle_seconds: i64) -> Destination {
        Destination::existing(
            id,
            IntegrationId::ClaudeCode,
            "profile",
            backend(model, credential_var),
            WarmSession {
                state: WarmSessionState::Live,
                idle_seconds,
            },
        )
    }

    fn inputs<'a>(
        overrides: &'a pairing::PairingOverrides,
        health: &'a FreePool,
        now: Instant,
    ) -> RouterInputs<'a> {
        RouterInputs {
            overrides,
            health,
            now,
            requirements: TaskRequirements::default(),
        }
    }

    fn term(explanation: &RoutingExplanation) -> &Contribution {
        explanation
            .contributions()
            .iter()
            .find(|c| c.name() == "pairing prior")
            .expect("every scored destination's explanation must carry the pairing prior term")
    }

    /// 566, 1540: two fresh, cold, equally healthy destinations of one
    /// harness, differing only in `PairingClass`. Listed non-native-first so
    /// a stable tie-break cannot be mistaken for the term actually
    /// separating them — if [`PAIRING_PRIOR`] were zeroed, the first-listed
    /// candidate would win regardless, and this assertion would catch it.
    #[test]
    fn a_tied_pair_differing_only_in_vendor_native_class_is_won_by_the_native_one() {
        let now = Instant::now();
        let overrides = pairing::PairingOverrides::default();
        let health = FreePool::new();
        let inputs = inputs(&overrides, &health, now);

        let other = fresh("other", OTHER_MODEL, "PAIRING_PRIOR_TEST_A");
        let native = fresh("native", NATIVE_MODEL, "PAIRING_PRIOR_TEST_B");

        let routed = SessionRouter::new()
            .choose(RoutingMoment::SessionStart, None, &[other, native], &inputs)
            .expect("two eligible fresh destinations must produce a decision");

        assert!(
            routed.rejected().is_empty(),
            "no candidate is ever refused on this axis: {:?}",
            routed.rejected()
        );
        assert_eq!(
            routed.chosen().id(),
            "native",
            "the vendor-native pairing must win the tie"
        );

        let winner_term = term(routed.explanation());
        assert!(
            winner_term.magnitude() > 0.0,
            "the native pairing's prior must be positive: {}",
            winner_term.magnitude()
        );
        assert!(
            winner_term.evidence().contains("vendor-native")
                && winner_term.evidence().contains("starting assumption"),
            "the explanation must name the class and call it a starting assumption, not a \
             quality claim: {}",
            winner_term.evidence()
        );

        let (_, loser_explanation) = routed
            .considered()
            .iter()
            .find(|(d, _)| d.id() == "other")
            .expect("the non-native candidate must still be considered, never rejected");
        let loser_term = term(loser_explanation);
        assert_eq!(
            loser_term.magnitude(),
            0.0,
            "a non-native pairing contributes nothing"
        );
        assert!(
            loser_term
                .evidence()
                .contains("inert: not a vendor-native pairing"),
            "a non-native pairing's explanation must say so plainly: {}",
            loser_term.evidence()
        );
    }

    /// 569, killed directly rather than through a set that also prices
    /// bootstrap cost and a hot prompt cache (a fresh-vs-existing comparison
    /// would still choose the warm side even with a mutated, oversized
    /// prior, which would make that test a weak witness for this line —
    /// practice §41). This is the dedicated killer, isolating exactly the
    /// weight the packet names: [`PAIRING_PRIOR`] must stay strictly below
    /// the `warmth` facet's own ceiling — a live warm session at zero idle,
    /// worth `1.5` (this module's own header comment, and
    /// [`AffinityBreakdown::warmth`]) — never the full breakdown total,
    /// which other facets such as a hot prompt cache also add to.
    #[test]
    fn pairing_prior_stays_below_a_live_warm_sessions_own_warmth_facet() {
        let warm_dest = warm("warm", OTHER_MODEL, "PAIRING_PRIOR_TEST_C", 0);
        let breakdown = affinity_breakdown(&warm_dest, None, &TaskRequirements::default())
            .expect("an existing destination always has a breakdown");
        assert!(
            PAIRING_PRIOR < breakdown.warmth.magnitude(),
            "the pairing prior ({PAIRING_PRIOR}) must stay strictly below a live warm \
             session's own warmth facet ({}), or it could outrank one",
            breakdown.warmth.magnitude()
        );
    }

    /// 569's behavioural half: the same tied pair as the first test, except
    /// the non-native candidate is now a relevant warm existing session
    /// instead of a fresh one. The warm side must win.
    #[test]
    fn a_relevant_warm_session_outweighs_the_native_pairing_prior() {
        let now = Instant::now();
        let overrides = pairing::PairingOverrides::default();
        let health = FreePool::new();
        let inputs = inputs(&overrides, &health, now);

        let native = fresh("native", NATIVE_MODEL, "PAIRING_PRIOR_TEST_D");
        let warm_other = warm("other", OTHER_MODEL, "PAIRING_PRIOR_TEST_E", 0);

        let routed = SessionRouter::new()
            .choose(
                RoutingMoment::SessionStart,
                None,
                &[native, warm_other],
                &inputs,
            )
            .expect("two eligible candidates must produce a decision");

        assert!(routed.rejected().is_empty());
        assert_eq!(
            routed.chosen().id(),
            "other",
            "a relevant warm session must outweigh the native pairing's starting prior"
        );
    }

    /// 1923, 1541: the same tied pair, except the native candidate has
    /// accumulated at least [`PAIRING_PRIOR_EVIDENCE_THRESHOLD`] local
    /// observations. Its own `pairing prior` term must read `0.0` with text
    /// saying observed evidence replaced the starting prior — the direct
    /// killer for "remove the evidence decay (always apply the prior)": with
    /// that mutation this assertion reads [`PAIRING_PRIOR`], not `0.0`.
    #[test]
    fn accumulated_local_evidence_decays_the_prior_to_zero() {
        let now = Instant::now();
        let overrides = pairing::PairingOverrides::default();
        let health = FreePool::new();
        let inputs = inputs(&overrides, &health, now);

        let other = fresh("other", OTHER_MODEL, "PAIRING_PRIOR_TEST_F");
        let seasoned_native = fresh("native", NATIVE_MODEL, "PAIRING_PRIOR_TEST_G")
            .with_pairing_prior_evidence(PAIRING_PRIOR_EVIDENCE_THRESHOLD);

        let routed = SessionRouter::new()
            .choose(
                RoutingMoment::SessionStart,
                None,
                &[other, seasoned_native],
                &inputs,
            )
            .expect("two eligible fresh destinations must produce a decision");

        assert!(routed.rejected().is_empty());

        let (_, native_explanation) = routed
            .considered()
            .iter()
            .find(|(d, _)| d.id() == "native")
            .expect("the seasoned native candidate must still be considered");
        let decayed = term(native_explanation);
        assert_eq!(
            decayed.magnitude(),
            0.0,
            "accumulated local evidence must decay the prior to zero"
        );
        assert!(
            decayed
                .evidence()
                .contains("observed evidence has replaced the starting prior"),
            "the explanation must say evidence replaced the prior: {}",
            decayed.evidence()
        );
    }

    /// 1923's "user choice": a `RoutingOverride` naming the non-native
    /// destination wins even though the native one's prior would otherwise
    /// carry the tie (it is listed first too, so an unhonoured override
    /// would still pick it on both counts). The override is asserted here,
    /// never rebuilt from the prior's own logic.
    #[test]
    fn a_user_override_naming_the_non_native_destination_wins() {
        let now = Instant::now();
        let overrides = pairing::PairingOverrides::default();
        let health = FreePool::new();
        let inputs = inputs(&overrides, &health, now);

        let native = fresh("native", NATIVE_MODEL, "PAIRING_PRIOR_TEST_H");
        let other = fresh("other", OTHER_MODEL, "PAIRING_PRIOR_TEST_I");

        let routed = SessionRouter::with_override(RoutingOverride::to("other"))
            .choose(RoutingMoment::SessionStart, None, &[native, other], &inputs)
            .expect("an override naming an eligible destination must produce a decision");

        assert!(routed.rejected().is_empty());
        assert!(
            routed.override_refused().is_none(),
            "the override must be honoured: {:?}",
            routed.override_refused()
        );
        assert_eq!(
            routed.chosen().id(),
            "other",
            "the user's own override must win over the native pairing's prior"
        );
    }

    /// The map's own "ranks byte-for-byte" requirement: a candidate set with
    /// no vendor-native member gets a `pairing prior` term of exactly `0.0`
    /// on every candidate, so the total this term adds to is unchanged from
    /// what the ranking summed to before this package existed.
    #[test]
    fn a_set_with_no_vendor_native_member_adds_nothing_to_the_ranking() {
        let now = Instant::now();
        let overrides = pairing::PairingOverrides::default();
        let health = FreePool::new();
        let inputs = inputs(&overrides, &health, now);

        let a = fresh("a", OTHER_MODEL, "PAIRING_PRIOR_TEST_J");
        let b = fresh("b", OTHER_MODEL, "PAIRING_PRIOR_TEST_K");

        let routed = SessionRouter::new()
            .choose(RoutingMoment::SessionStart, None, &[a, b], &inputs)
            .expect("two eligible fresh destinations must produce a decision");

        assert!(routed.rejected().is_empty());
        for (destination, explanation) in routed.considered() {
            let t = term(explanation);
            assert_eq!(
                t.magnitude(),
                0.0,
                "`{}` is not vendor-native, so the term must contribute nothing to its total",
                destination.id()
            );
        }
    }
}
