//! Independent tests of `glasshouse::routing`, against the capability lines
//! it claims to implement (Phase 9H lines 505-518, Phase 9I lines 527-540).
//!
//! These are deliberately not the author's own tests. `lead-route` wrote
//! `crates/glasshouse/src/routing/` and its unit tests; this file is the
//! other half — it reads the capability lines and the doc comments, not the
//! test suite already there, and tries to find the seam a policy's own
//! author is least likely to have covered: boundaries, order dependence,
//! identity, and the claims the doc comments make about the code.
//!
//! Every private constant this file relies on (`FAILURES_BEFORE_COOLDOWN`,
//! `BASE_COOLDOWN`, `MAX_COOLDOWN`) is copied from `src/routing/free.rs`
//! because the public API does not expose them; a change to those constants
//! in `src/` would need a matching change here, and that coupling is
//! unavoidable from outside the crate.

use std::time::{Duration, Instant};

use glasshouse::config::pairing::{NoObservations, PairingPreference};
use glasshouse::harness::pairing::PairingOverrides;
use glasshouse::routing::classify::{
    self, ClassificationSource, Complexity, Confidence, TaskClassification, WarmContextValue,
    WorkloadTier,
};
use glasshouse::routing::disposable::{
    DisposableCandidate, DisposableRouting, JobKind, MeteredUse, NoResource,
};
use glasshouse::routing::evidence::RouteCorrelations;
use glasshouse::routing::free::{
    FreePool, FreePreferences, FreeResource, FreeResourceKey, PoolReading, WorkloadOutcome,
};
use glasshouse::routing::interactive::{
    Assignment, FailureResponse, InteractiveRouting, ProviderFailure, SessionActivity,
};
use glasshouse::routing::{AssignedModel, Backend, Cost, CredentialId, ToolSemantics};
use glasshouse::secret::SecretRef;

// Copied from `src/routing/free.rs`; see module doc comment above.
const FAILURES_BEFORE_COOLDOWN: u32 = 2;
const BASE_COOLDOWN: Duration = Duration::from_secs(30);
const MAX_COOLDOWN: Duration = Duration::from_secs(15 * 60);

// --- shared builders ---

fn env_credential(provider: &str) -> CredentialId {
    CredentialId::new(
        provider,
        SecretRef::Environment {
            var: format!("{}_API_KEY", provider.to_uppercase()),
        },
    )
}

fn backend(provider: &str, model: &str) -> Backend {
    backend_full(
        provider,
        model,
        "anthropic-messages",
        ToolSemantics::Unverified,
        Cost::Metered,
    )
}

fn backend_full(
    provider: &str,
    model: &str,
    protocol: &str,
    tools: ToolSemantics,
    cost: Cost,
) -> Backend {
    Backend::new(
        provider,
        protocol,
        AssignedModel::named(model),
        env_credential(provider),
        cost,
        tools,
    )
}

fn session_on(provider: &str, model: &str) -> Assignment {
    Assignment::new("claude-code", backend(provider, model))
}

fn free_resource(provider: &str, model: &str) -> FreeResource {
    FreeResource::new(env_credential(provider), model)
}

// =====================================================================
// Boundaries
// =====================================================================
mod boundaries {
    use super::*;

    /// A cooldown set by the provider's own `retry_after` is a closed
    /// interval at its start: the instant it expires the resource is
    /// available again, not one tick later. Getting this off by one in
    /// either direction is exactly the kind of thing a policy's own author,
    /// testing with `now` and `now + retry`, would never notice.
    #[test]
    fn a_cooldown_ends_exactly_at_its_own_expiry_instant_not_before_or_after() {
        let now = Instant::now();
        let mut pool = FreePool::new();
        let it = free_resource("openrouter", "free-model");
        let retry = Duration::from_secs(30);

        for _ in 0..FAILURES_BEFORE_COOLDOWN {
            pool.observe(
                &it,
                WorkloadOutcome::RateLimited {
                    retry_after: Some(retry),
                },
                now,
            );
        }
        let expiry = now + retry;

        assert!(
            !pool.is_available(&it, expiry - Duration::from_nanos(1)),
            "one nanosecond before its own expiry, a cooldown must still be in force"
        );
        assert!(
            pool.is_available(&it, expiry),
            "at the exact expiry instant the cooldown must already be over — a resource that \
             stays cooled down one more tick is quietly worse than the provider's own answer"
        );
        assert!(
            pool.is_available(&it, expiry + Duration::from_nanos(1)),
            "one nanosecond after expiry the cooldown must obviously be over"
        );
    }

    /// Capability map line 1319 — *"treat provider-declared Retry-After or
    /// equivalent cooldown information as authoritative for temporary
    /// scheduling blocks."* **Authoritative means Glasshouse's own ceiling
    /// does not apply to it.**
    ///
    /// `MAX_COOLDOWN` is documented as *"the longest cooldown Glasshouse will
    /// impose **by itself**"*, and clamping a provider's stated wait down to
    /// it is not a ceiling on Glasshouse — it is overriding the provider,
    /// which is precisely what this line rules out. A provider that says
    /// "come back in an hour" and is returned to after fifteen minutes has
    /// not been treated as authoritative.
    #[test]
    fn a_providers_declared_wait_is_not_clamped_to_glasshouses_own_ceiling() {
        let now = Instant::now();
        let mut pool = FreePool::new();
        let it = free_resource("openrouter", "free-model");
        let declared = MAX_COOLDOWN * 4;

        // The premise, asserted before the behaviour (§17): this test is only
        // about anything at all if the provider's wait really is longer than
        // the ceiling Glasshouse would otherwise impose.
        assert!(
            declared > MAX_COOLDOWN,
            "sanity: this test exists to cover a declared wait that exceeds MAX_COOLDOWN"
        );

        pool.observe(
            &it,
            WorkloadOutcome::RateLimited {
                retry_after: Some(declared),
            },
            now,
        );

        assert_eq!(
            pool.health(&it).cooling_down_until(),
            Some(now + declared),
            "the provider's own wait must be recorded as stated, not shortened to Glasshouse's \
             own ceiling"
        );
        assert!(
            !pool.is_available(&it, now + MAX_COOLDOWN),
            "at Glasshouse's own ceiling the resource must still be blocked — this is the exact \
             instant a clamped cooldown would wrongly have released it back for scheduling"
        );
        assert!(
            pool.is_available(&it, now + declared),
            "at the instant the provider itself named, the resource must be schedulable again"
        );
    }

    /// Line 1319 again, at the other end: a declared wait is authoritative
    /// **from the first failure**, not from the second.
    ///
    /// `FAILURES_BEFORE_COOLDOWN` exists because *inventing* a cooldown from
    /// one ordinary `429` would empty a pool of healthy resources. That
    /// reasoning is about not knowing how long to wait — and it evaporates
    /// the moment the provider says how long. Continuing to schedule work
    /// against a resource that has just told us to hold for 60 seconds is the
    /// temporary scheduling block this line names, ignored.
    ///
    /// The `None` half is the control: it proves the block came from the
    /// *declaration* and not from some general "one failure now cools down"
    /// change, which would be a different and unasked-for policy.
    #[test]
    fn a_declared_wait_blocks_scheduling_on_the_first_failure_and_an_undeclared_one_does_not() {
        let now = Instant::now();
        let declared = Duration::from_secs(60);

        // A `const` block, not a plain `assert!`: the operands are both
        // compile-time constants, so this is a fact about the build and not
        // about this run, and `clippy::assertions_on_constants` correctly
        // refuses the runtime form. Written this way the check happens at
        // compile time and the sanity condition is kept rather than dropped.
        const {
            assert!(
                FAILURES_BEFORE_COOLDOWN > 1,
                "sanity: with a threshold of one there would be no distinction left to test"
            );
        }

        // Control: one failure the provider said nothing about does not cool
        // the resource down. Asserted on `health` rather than
        // `FreePool::is_available`, because the latter is the conjunction of
        // health and the credential's allowance, and a rate-limit marks the
        // allowance empty by a separate path — line 1324's own separation,
        // and the confounder this assertion has to avoid to mean anything.
        let mut invented = FreePool::new();
        let undeclared = free_resource("openrouter", "free-model");
        invented.observe(
            &undeclared,
            WorkloadOutcome::RateLimited { retry_after: None },
            now,
        );
        assert!(
            invented.health(&undeclared).is_available(now),
            "one rate-limit with no stated wait must still not cool a resource down — a single \
             429 on a shared free tier is ordinary"
        );
        assert_eq!(
            invented.health(&undeclared).cooling_down_until(),
            None,
            "and it must record no cooldown deadline at all, having been given none"
        );

        // The behaviour: the same single failure, with the provider's own
        // answer attached, blocks scheduling for exactly as long as it said.
        let mut stated = FreePool::new();
        let it = free_resource("openrouter", "free-model");
        stated.observe(
            &it,
            WorkloadOutcome::RateLimited {
                retry_after: Some(declared),
            },
            now,
        );
        assert!(
            !stated.health(&it).is_available(now),
            "a provider that stated how long to wait must be obeyed on the first failure, not \
             after a second one it never asked for"
        );
        assert!(
            !stated.is_available(&it, now + declared - Duration::from_nanos(1)),
            "the block must hold for the whole of the stated wait"
        );
        assert!(
            stated.is_available(&it, now + declared),
            "and end exactly when the provider said it would"
        );

        // The declaration is what makes the block *temporary*, which is the
        // adjective line 1319 uses. Without one, `observe` marks the
        // credential's pool empty with no reset instant, and nothing in
        // production ever clears it — so the undeclared resource is still
        // unschedulable at the same future instant the declared one has
        // already recovered at.
        assert!(
            !invented.is_available(&undeclared, now + declared),
            "a rate-limit with no stated wait leaves the credential's pool empty with no reset \
             instant — see this file's sibling note; it is the provider's own answer that gives \
             the block an end"
        );
    }

    /// Line 1319's *"scheduling blocks"*, at the one scheduling decision this
    /// crate actually makes with health: credential rotation.
    ///
    /// `FreePool::rotate_from` is what `gateway::session::observe_exchange`
    /// calls when a credential is rate-limited, and it is the production
    /// consumer that makes a cooldown a *block* rather than a stored number.
    /// A rotation that hands work to a credential the provider just told us
    /// to hold has not treated the declaration as authoritative, however
    /// faithfully the number was recorded.
    #[test]
    fn rotation_will_not_hand_work_to_a_credential_the_provider_told_us_to_hold() {
        let now = Instant::now();
        let mut pool = FreePool::new();
        let declared = Duration::from_secs(120);

        let first = env_credential("openrouter");
        let second = CredentialId::new(
            "openrouter",
            SecretRef::Environment {
                var: "OPENROUTER_SECOND_KEY".to_owned(),
            },
        );
        let model = "free-model";
        let candidates = vec![first.clone(), second.clone()];

        // The premise: with nothing observed, rotation away from `first`
        // finds `second`. Without this, the assertion below could pass
        // because rotation never finds anything at all.
        assert_eq!(
            pool.rotate_from(&first, &candidates, model, now),
            Some(second.clone()),
            "premise: an unobserved sibling credential is a valid rotation target"
        );

        // One rate-limit, with the provider's own wait, against the sibling.
        pool.observe(
            &FreeResource::new(second.clone(), model),
            WorkloadOutcome::RateLimited {
                retry_after: Some(declared),
            },
            now,
        );

        assert_eq!(
            pool.rotate_from(&first, &candidates, model, now),
            None,
            "the only sibling is inside a wait the provider itself declared, so there is nothing \
             to rotate to — scheduling into that wait is the block line 1319 forbids"
        );
        assert_eq!(
            pool.rotate_from(&first, &candidates, model, now + declared),
            Some(second),
            "and once the provider's own wait has elapsed the sibling is a target again"
        );
    }

    /// `FAILURES_BEFORE_COOLDOWN - 1` failures, then a success, then one more
    /// failure. If the counter did not really reset on success, this third
    /// failure would be the resource's second *recorded* failure and would
    /// trip the cooldown; if it did reset, the resource stays available.
    #[test]
    fn a_success_resets_the_consecutive_failure_counter_rather_than_only_the_cooldown() {
        let now = Instant::now();
        let mut pool = FreePool::new();
        let it = free_resource("openrouter", "free-model");

        for _ in 0..(FAILURES_BEFORE_COOLDOWN - 1) {
            pool.observe(&it, WorkloadOutcome::CapacityFailure, now);
        }
        assert!(
            pool.is_available(&it, now),
            "one failure short of the threshold must never cool a resource down"
        );

        pool.observe(&it, WorkloadOutcome::Served, now);
        pool.observe(&it, WorkloadOutcome::CapacityFailure, now);

        assert!(
            pool.is_available(&it, now),
            "a success must reset the *count* of consecutive failures, not just clear an \
             existing cooldown — otherwise a resource that failed, served once, then failed \
             again would be cooled down on its second-ever failure"
        );
        assert_eq!(
            pool.health(&it).consecutive_failures(),
            1,
            "after reset-then-one-failure the counter must read 1, not 2"
        );
    }

    /// Backoff doubles with each failure past the threshold but is bounded by
    /// `MAX_COOLDOWN`. This checks the cap is hit exactly, and that going
    /// further past it does not push the cooldown beyond the cap.
    #[test]
    fn backoff_is_capped_at_max_cooldown_and_does_not_grow_past_it() {
        let now = Instant::now();
        let mut pool = FreePool::new();
        let it = free_resource("openrouter", "free-model");

        // consecutive_failures = 2 + 8 = 10: steps = min(10 - 2, 8) = 8,
        // so 30s * 2^8 = 7680s, capped to MAX_COOLDOWN = 900s.
        for _ in 0..(FAILURES_BEFORE_COOLDOWN + 8) {
            pool.observe(&it, WorkloadOutcome::CapacityFailure, now);
        }
        assert_eq!(
            pool.health(&it).cooling_down_until(),
            Some(now + MAX_COOLDOWN),
            "ten consecutive failures with no provider-stated retry must land exactly at the cap"
        );

        // One more failure: uncapped backoff would be 15360s, but the cap
        // must hold the cooldown at exactly the same instant, not push it
        // further out.
        pool.observe(&it, WorkloadOutcome::CapacityFailure, now);
        assert_eq!(
            pool.health(&it).cooling_down_until(),
            Some(now + MAX_COOLDOWN),
            "a resource already at the cooldown cap must not be pushed further out by \
             additional failures — MAX_COOLDOWN is a ceiling, not a starting point for more math"
        );
        assert!(
            BASE_COOLDOWN < MAX_COOLDOWN,
            "sanity: the constants copied from src/routing/free.rs must still relate the way \
             this test assumes"
        );
    }

    /// A request pool's exhaustion is exact at the provider's own reset
    /// instant too: known-empty right up to it, unknown-again from it.
    #[test]
    fn a_request_pools_exhaustion_ends_exactly_at_the_providers_reset_instant() {
        let now = Instant::now();
        let mut pool = FreePool::new();
        let id = env_credential("openrouter");
        let window = Duration::from_secs(60);

        pool.record_pool(
            &id,
            &PoolReading {
                limit: Some(10),
                remaining: Some(0),
                resets_in: Some(window),
            },
            now,
        );
        let reset = now + window;

        assert!(
            pool.allowance(&id)
                .is_exhausted(reset - Duration::from_nanos(1))
        );
        assert!(
            !pool.allowance(&id).is_exhausted(reset),
            "at the provider's own reset instant, what is left becomes unknown again, not zero \
             — an exhausted pool must not be reported as exhausted one tick past when the \
             provider itself said it would refill"
        );
        assert!(
            !pool
                .allowance(&id)
                .is_exhausted(reset + Duration::from_nanos(1))
        );
    }
}

// =====================================================================
// Order dependence
// =====================================================================
mod order_dependence {
    use super::*;

    /// Line 513: same-model failover is preferred over a different-model
    /// migration candidate, whichever order the candidate slice is given in.
    #[test]
    fn failover_prefers_the_same_model_however_the_candidate_slice_is_ordered() {
        let routing = InteractiveRouting::new();
        let current = session_on("openrouter", "the-model");
        let other_model = backend("kilo", "a-different-model");
        let same_model = backend("nous", "the-model");

        let forward = routing.on_provider_failure(
            &current,
            ProviderFailure::Unreachable,
            &[other_model.clone(), same_model.clone()],
            PairingPreference::Strong,
            &PairingOverrides::default(),
            &NoObservations,
            &RouteCorrelations::default(),
        );
        let reversed = routing.on_provider_failure(
            &current,
            ProviderFailure::Unreachable,
            &[same_model.clone(), other_model.clone()],
            PairingPreference::Strong,
            &PairingOverrides::default(),
            &NoObservations,
            &RouteCorrelations::default(),
        );

        for (label, response) in [
            ("other-model-first", forward),
            ("same-model-first", reversed),
        ] {
            match response {
                FailureResponse::FailOver { to, .. } => {
                    assert_eq!(
                        to.backend().model(),
                        &AssignedModel::named("the-model"),
                        "{label}: a same-model candidate must win regardless of where it sits \
                         in the candidate slice"
                    );
                }
                other => panic!("{label}: expected a same-model failover, got {other:?}"),
            }
        }
    }

    /// Two equally valid same-model candidates: the policy takes the first
    /// one that survives, per its own documentation ("the user's own
    /// ordering wins"). Reversing the slice must reverse the answer — if it
    /// did not, the "first that survives" rule would be a lie and the
    /// function would actually be ranking on something else undocumented.
    #[test]
    fn between_two_equally_valid_same_model_candidates_the_earlier_one_in_the_slice_wins() {
        let routing = InteractiveRouting::new();
        let current = session_on("openrouter", "shared-model");
        let candidate_a = backend("kilo", "shared-model");
        let candidate_b = backend("nous", "shared-model");

        let a_first = routing.on_provider_failure(
            &current,
            ProviderFailure::Unreachable,
            &[candidate_a.clone(), candidate_b.clone()],
            PairingPreference::Strong,
            &PairingOverrides::default(),
            &NoObservations,
            &RouteCorrelations::default(),
        );
        match a_first {
            FailureResponse::FailOver { to, .. } => assert_eq!(to.provider(), "kilo"),
            other => panic!("expected a failover, got {other:?}"),
        }

        let b_first = routing.on_provider_failure(
            &current,
            ProviderFailure::Unreachable,
            &[candidate_b, candidate_a],
            PairingPreference::Strong,
            &PairingOverrides::default(),
            &NoObservations,
            &RouteCorrelations::default(),
        );
        match b_first {
            FailureResponse::FailOver { to, .. } => assert_eq!(
                to.provider(),
                "nous",
                "reversing the slice must reverse which equally-valid candidate is chosen — \
                 if it did not, the order-preserving claim in the doc comment would not hold"
            ),
            other => panic!("expected a failover, got {other:?}"),
        }
    }
}

// =====================================================================
// Identity
// =====================================================================
mod identity {
    use super::*;

    /// Two credentials that differ only in the store variant must not be
    /// treated as the same credential, even when every name they carry is
    /// identical.
    #[test]
    fn credentials_that_differ_only_in_store_variant_are_not_the_same_identity() {
        let env = CredentialId::new(
            "openrouter",
            SecretRef::Environment {
                var: "SHARED_NAME".to_owned(),
            },
        );
        let os = CredentialId::new(
            "openrouter",
            SecretRef::OsCredential {
                service: "openrouter".to_owned(),
                account: "SHARED_NAME".to_owned(),
            },
        );
        assert_ne!(
            env, os,
            "an environment-variable credential and an OS-keychain credential that happen to \
             share every name are still two different secrets and must not compare equal"
        );
    }

    /// A provider name that is a textual prefix of another provider's name
    /// must not be conflated with it when a failover excludes "the backend
    /// that just failed" — that exclusion compares provider, model and
    /// credential, and a substring match there would wrongly exclude (or
    /// wrongly include) a distinct provider.
    #[test]
    fn a_provider_name_that_prefixes_another_providers_name_is_a_distinct_provider() {
        let routing = InteractiveRouting::new();
        let current = Assignment::new(
            "claude-code",
            Backend::new(
                "kilo",
                "anthropic-messages",
                AssignedModel::named("the-model"),
                env_credential("kilo"),
                Cost::Metered,
                ToolSemantics::Unverified,
            ),
        );
        // "kilo2" has "kilo" as a strict prefix, and serves the same model.
        let prefixed_provider = Backend::new(
            "kilo2",
            "anthropic-messages",
            AssignedModel::named("the-model"),
            env_credential("kilo2"),
            Cost::Metered,
            ToolSemantics::Unverified,
        );

        let response = routing.on_provider_failure(
            &current,
            ProviderFailure::Unreachable,
            &[prefixed_provider],
            PairingPreference::Strong,
            &PairingOverrides::default(),
            &NoObservations,
            &RouteCorrelations::default(),
        );

        match response {
            FailureResponse::FailOver { to, .. } => assert_eq!(
                to.provider(),
                "kilo2",
                "a provider whose name merely starts with the failed provider's name is a real, \
                 distinct failover target and must be offered"
            ),
            other => panic!(
                "a same-model candidate on a distinctly-named provider must be a valid failover \
                 target, got {other:?}"
            ),
        }
    }

    /// A model name that is a textual prefix of another model name must be
    /// treated as a *different* model for the failover/migration split —
    /// exact identity, not a prefix match.
    #[test]
    fn a_model_name_that_prefixes_another_model_name_is_a_different_model() {
        let routing = InteractiveRouting::new();
        let current = session_on("openrouter", "gpt-4");
        let prefixed_model = backend("nous", "gpt-4-turbo");

        let response = routing.on_provider_failure(
            &current,
            ProviderFailure::Unreachable,
            &[prefixed_model],
            PairingPreference::Strong,
            &PairingOverrides::default(),
            &NoObservations,
            &RouteCorrelations::default(),
        );

        match response {
            FailureResponse::OfferMigration { to, .. } => {
                assert_eq!(to.backend().model(), &AssignedModel::named("gpt-4-turbo"));
            }
            other => panic!(
                "`gpt-4-turbo` is a different model from `gpt-4` even though one is a textual \
                 prefix of the other; a prefix-based comparison would wrongly treat this as a \
                 transparent failover instead of an offered migration, got {other:?}"
            ),
        }
    }

    /// `FreeResourceKey` matching (used by both disable-lists and pins) must
    /// require the whole model name, not a prefix of it.
    #[test]
    fn a_free_resource_key_does_not_match_on_a_model_name_prefix() {
        let banned = free_resource("openrouter", "model");
        let unrelated = free_resource("openrouter", "model-2");

        let prefs =
            FreePreferences::new().with_disabled(vec![FreeResourceKey::new("openrouter", "model")]);

        assert!(prefs.is_disabled(&banned));
        assert!(
            !prefs.is_disabled(&unrelated),
            "disabling `model` must not also disable `model-2` — a resource key match must be \
             exact, not a prefix match"
        );

        let arranged = prefs.arrange(&[banned, unrelated.clone()]);
        assert_eq!(
            arranged,
            vec![unrelated],
            "the only surviving candidate after disabling `model` must be `model-2`"
        );
    }
}

// =====================================================================
// The refusals, from the other side
// =====================================================================
mod refusals {
    use super::*;

    /// `Pin::None` must permit every provider, not accidentally none of
    /// them. Checked through `migrate`, the only public surface a caller
    /// can use to observe what a pin permits.
    #[test]
    fn pin_none_permits_a_migration_to_any_compatible_provider() {
        let routing = InteractiveRouting::new();
        assert_eq!(routing.pin(), &glasshouse::routing::interactive::Pin::None);
        let current = session_on("openrouter", "the-model");

        let migrated = routing
            .migrate(
                &current,
                backend("some-provider-nobody-configured-before", "another-model"),
                SessionActivity::Idle,
            )
            .expect(
                "Pin::None must not silently refuse a migration to an arbitrary compatible \
                 provider — that would make an unset pin behave like a pin to nothing",
            );
        assert_eq!(
            migrated.provider(),
            "some-provider-nobody-configured-before"
        );
    }

    /// `MeteredUse::for_automated_run` must read the *exact* variable name it
    /// documents, not just "whatever the closure happens to return". A
    /// closure that only answers a different variable name must leave the
    /// automated run withheld.
    #[test]
    fn for_automated_run_queries_its_own_named_variable_and_not_any_variable() {
        let use_ = MeteredUse::for_automated_run(|var| {
            if var == "SOME_OTHER_VARIABLE" {
                Some("1".to_owned())
            } else {
                None
            }
        });
        assert_eq!(
            use_,
            MeteredUse::Withheld,
            "a closure answering a different variable name must not opt an automated run in — \
             for_automated_run must ask for its own documented OPT_IN_VAR by name"
        );
    }

    /// Values nobody in the author's own test list thought of: whitespace
    /// variants, a numeric look-alike, and a value with the right prefix but
    /// extra trailing content.
    #[test]
    fn opt_in_values_the_authors_own_test_did_not_try_still_do_not_opt_in() {
        for value in ["1\n", "1\t", "01", "1.0", "11", "1 ", "\u{2460}"] {
            let use_ = MeteredUse::for_automated_run(|_| Some(value.to_owned()));
            assert_eq!(
                use_,
                MeteredUse::Withheld,
                "`{value:?}` must not be read as the opt-in — only the exact string `1` may spend \
                 real money on an automated run"
            );
        }
    }

    /// `NoResource::PinnedResourceUnavailable` must fire when the pinned
    /// model was never even offered as a candidate, not only when it was
    /// offered but is currently cooling down.
    #[test]
    fn a_pin_to_a_resource_absent_from_the_candidate_list_is_reported_as_unavailable_not_missing() {
        let routing = DisposableRouting::for_support_work(
            true,
            FreePreferences::new().with_pin(Some(FreeResourceKey::new(
                "openrouter",
                "never-configured-model",
            ))),
        );
        let err = routing
            .choose(
                JobKind::Classification,
                &[DisposableCandidate::new(
                    "openrouter",
                    "some-other-model",
                    env_credential("openrouter"),
                    Cost::Free,
                )],
                &FreePool::new(),
                Instant::now(),
                None,
            )
            .expect_err("a pin to a model nobody offered must not silently pick something else");

        assert!(
            matches!(err, NoResource::PinnedResourceUnavailable { .. }),
            "a pin to a resource absent from the candidate list must be reported the same way as \
             a pin to a resource that is present but cooling down — both are \"the pin cannot be \
             honoured\" — got {err:?} instead"
        );
    }
}

// =====================================================================
// The claims the doc comments make
// =====================================================================
mod doc_comment_claims {
    use super::*;

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

    /// `CacheLocality::between` claims to be "the one place the rule
    /// exists". Checked by scanning every other routing source file's
    /// production code for a second place that mints a `CacheLossReason`
    /// directly, which would mean a second copy of the rule had grown up
    /// beside `between`, computing the same verdict a different way.
    #[test]
    fn cache_locality_between_is_the_only_place_that_constructs_a_cache_loss_reason() {
        let interactive = production_code(include_str!("../src/routing/interactive.rs"));
        let free = production_code(include_str!("../src/routing/free.rs"));
        let disposable = production_code(include_str!("../src/routing/disposable.rs"));

        for (name, source) in [
            ("interactive.rs", &interactive),
            ("free.rs", &free),
            ("disposable.rs", &disposable),
        ] {
            assert!(
                !source.contains("CacheLossReason::"),
                "{name} constructs a `CacheLossReason` directly in production code, outside of \
                 `CacheLocality::between` — the doc comment's claim that between() is the one \
                 place the cache rule lives does not hold"
            );
        }
        // The claim holds: mod.rs is verified to construct it exactly where
        // documented, in `between`'s own body.
        let mod_source = production_code(include_str!("../src/routing/mod.rs"));
        assert!(
            mod_source.contains("CacheLossReason::ProviderChanged")
                && mod_source.contains("CacheLossReason::ModelChanged"),
            "sanity: mod.rs itself must still construct these somewhere, or this test would pass \
             vacuously"
        );
    }

    /// `FreePool::observe` claims to be "the only mutator of health in
    /// Glasshouse". Checked by scanning `free.rs`'s production code for any
    /// assignment into a `ResourceHealth` field from a function other than
    /// `ResourceHealth::observe`/`fail` (which `FreePool::observe` is the
    /// only caller of) or `FreePool::observe` itself.
    #[test]
    fn free_pool_observe_is_the_only_public_entry_point_that_changes_health() {
        let free = production_code(include_str!("../src/routing/free.rs"));

        // `pub fn` signatures on FreePool that are not `observe`, `new`, or a
        // read-only accessor (`allowance`, `health`, `is_available`,
        // `rotate_from`, `observed`, `record_pool`, `declare_token_priced`).
        // `record_pool` and `declare_token_priced` touch the *allowance*, a
        // separate field from health (line 528's whole point), so they are
        // not a second mutator of health.
        let health_mutating_names = [
            "fn set_health",
            "fn health_mut",
            "fn reset_health",
            "fn force_available",
        ];
        for forbidden in health_mutating_names {
            assert!(
                !free.contains(forbidden),
                "found `{forbidden}` in free.rs: a second way to change a resource's health has \
                 appeared, which the doc comment says cannot happen"
            );
        }

        // The behavioural half: observing directly proves work is the only
        // route in, by demonstrating that health for a resource nobody has
        // called `observe` on reads as untouched (the constructor default),
        // regardless of what else this pool has recorded for other
        // resources or allowances.
        let now = Instant::now();
        let mut pool = FreePool::new();
        pool.record_pool(
            &env_credential("openrouter"),
            &PoolReading {
                remaining: Some(0),
                ..PoolReading::default()
            },
            now,
        );
        let untouched = free_resource("openrouter", "a-model-nobody-observed");
        assert_eq!(
            pool.health(&untouched).consecutive_failures(),
            0,
            "recording an allowance reading for a credential must not, by itself, change the \
             health of any resource behind it — health changes only through observe()"
        );
    }

    /// `routing` claims that "nothing here opens a socket, resolves a
    /// credential, or reads the clock" — no policy can make a request. An
    /// independent, broader denylist than the author's own
    /// `no_routing_policy_can_make_a_request` test, which only checks four
    /// names.
    #[test]
    fn no_routing_source_file_names_anything_that_could_open_a_connection_or_read_the_clock() {
        let sources = [
            ("mod.rs", include_str!("../src/routing/mod.rs")),
            (
                "interactive.rs",
                include_str!("../src/routing/interactive.rs"),
            ),
            ("free.rs", include_str!("../src/routing/free.rs")),
            (
                "disposable.rs",
                include_str!("../src/routing/disposable.rs"),
            ),
        ];
        let forbidden = [
            "ureq",
            "reqwest",
            "hyper",
            "curl",
            "TcpStream",
            "TcpListener",
            "UdpSocket",
            "std::net",
            "std::process::Command",
            "Instant::now",
            "SystemTime::now",
        ];
        for (name, source) in sources {
            let code = production_code(source);
            for term in forbidden {
                assert!(
                    !code.contains(term),
                    "routing/{name} names `{term}` in production code: a routing policy that can \
                     open a connection, spawn a process, or read its own clock breaks the \
                     purity claim this module's header makes, and Phase 9I line 534 depends on \
                     policies never being able to probe on their own"
                );
            }
        }
    }
}

// =====================================================================
// The two policy classes on identical input
// =====================================================================
mod policy_divergence {
    use super::*;

    /// Phase 9I line 533's actual content, not just its type-level half: fed
    /// the same catalogue — a metered model this session is already on, and
    /// a free model marked and available — the interactive policy keeps its
    /// current backend and the disposable policy takes the free one.
    #[test]
    fn given_the_same_catalogue_the_two_policy_classes_answer_differently() {
        let now = Instant::now();

        // The interactive session is already assigned to the metered
        // backend; a free backend is available as an alternative.
        let metered_backend = backend_full(
            "openrouter",
            "primary-model",
            "anthropic-messages",
            ToolSemantics::Verified,
            Cost::Metered,
        );
        let free_backend = backend_full(
            "openrouter",
            "primary-free-model",
            "anthropic-messages",
            ToolSemantics::Verified,
            Cost::Free,
        );

        let interactive = InteractiveRouting::new();
        let current = Assignment::new("claude-code", metered_backend.clone());
        let turn = interactive.next_turn(&current, std::slice::from_ref(&free_backend));
        assert_eq!(
            turn.assignment().backend().cost(),
            Cost::Metered,
            "an interactive session on a normal turn must keep its metered backend even though \
             a free model is sitting right there in the alternatives it was shown"
        );
        assert_eq!(turn.assignment().backend().model(), metered_backend.model());

        // The disposable job is offered the same two options, marked the
        // same way.
        let disposable = DisposableRouting::for_support_work(false, FreePreferences::new());
        let choice = disposable
            .choose(
                JobKind::Classification,
                &[
                    DisposableCandidate::new(
                        "openrouter",
                        "primary-model",
                        env_credential("openrouter"),
                        Cost::Metered,
                    ),
                    DisposableCandidate::new(
                        "openrouter",
                        "primary-free-model",
                        env_credential("openrouter"),
                        Cost::Free,
                    ),
                ],
                &FreePool::new(),
                now,
                None,
            )
            .expect("a free candidate is configured and available");
        assert_eq!(
            choice.cost(),
            Cost::Free,
            "bounded support work, given the identical catalogue the interactive session just \
             ignored, must take the free model instead"
        );
        assert_eq!(choice.model(), "primary-free-model");

        assert_ne!(
            turn.assignment().backend().model(),
            &AssignedModel::named(choice.model()),
            "the two policy classes must not converge on the same model from the same input — \
             that is Phase 9I line 533's actual, behavioural content"
        );
    }
}

// =====================================================================
// Phase 33C: failure-domain diversity in failover ranking
// =====================================================================
mod failure_domain {
    use super::*;

    fn backend_with_credential(provider: &str, model: &str, var: &str) -> Backend {
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

    /// Acceptance test 1, from outside the crate: two same-model survivors,
    /// one sharing the failed backend's own provider (a different
    /// credential — the exact shape a provider with two configured keys
    /// produces, per `Upstream::failover_candidates`) and one on a
    /// genuinely different provider. The diverse one must win when nothing
    /// else distinguishes them, entering through the same
    /// `on_provider_failure` the gateway's accept loop calls.
    #[test]
    fn a_candidate_sharing_the_failed_backends_own_provider_loses_to_a_diverse_one() {
        let routing = InteractiveRouting::new();
        let current = session_on("openrouter", "the-model");
        let shared_domain =
            backend_with_credential("openrouter", "the-model", "OPENROUTER_API_KEY_2");
        let diverse_domain = backend("nous", "the-model");

        let response = routing.on_provider_failure(
            &current,
            ProviderFailure::Unreachable,
            &[shared_domain, diverse_domain],
            PairingPreference::Off,
            &PairingOverrides::default(),
            &NoObservations,
            &RouteCorrelations::default(),
        );

        match response {
            FailureResponse::FailOver { to, .. } => assert_eq!(
                to.provider(),
                "nous",
                "a candidate sharing the failed backend's own provider must never be preferred \
                 over a genuinely different one when nothing else distinguishes them"
            ),
            other => panic!("expected a same-model failover: {other:?}"),
        }
    }
}

/// Independent tests of `glasshouse::routing::classify` (Phase 35), against
/// its own doc-comment claims rather than `classify.rs`'s unit tests — the
/// same split this file's header describes for the two routing policies.
mod classification {
    use super::*;

    /// `classify_heuristically`'s doc comment claims it is "a pure function
    /// of `request_text`: same text in, same `TaskClassification` out,
    /// always". Checked from outside the module, on inputs the module's own
    /// unit tests do not use.
    #[test]
    fn classify_heuristically_is_deterministic() {
        for text in [
            "run the migration and fix the fallout",
            "what is a race condition?",
            "take a screenshot of the settings page",
            "",
            "asdkjfh",
        ] {
            assert_eq!(
                classify::classify_heuristically(text),
                classify::classify_heuristically(text),
                "classify_heuristically({text:?}) produced two different answers for the same \
                 input"
            );
        }
    }

    /// The module doc comment claims `classify_heuristically` "makes none"
    /// of the network calls `mod@super::super` forbids its two policy
    /// classes from making. Same scan idiom as
    /// `no_routing_policy_can_make_a_request` above, extended to this
    /// module.
    #[test]
    fn the_classifier_cannot_make_a_request_either() {
        let source = include_str!("../src/routing/classify.rs")
            .split("#[cfg(test)]")
            .next()
            .expect("split always yields at least one part")
            .lines()
            .filter(|line| !line.trim_start().starts_with("//"))
            .collect::<Vec<_>>()
            .join("\n");
        for forbidden in ["ureq", "TcpStream", "reqwest", "std::net"] {
            assert!(
                !source.contains(forbidden),
                "routing/classify.rs names `{forbidden}`: a classifier that can open a \
                 connection is not the lightweight, model-optional mechanism Phase 35 asks for"
            );
        }
    }

    /// An empty request is the sharpest boundary this heuristic has: no
    /// keyword list can match it, so it must land exactly where an
    /// unmatched, ambiguous request lands — `Confidence::Low`, escalated by
    /// the conservative accessor — never a panic and never a confident
    /// guess.
    #[test]
    fn an_empty_request_is_low_confidence_not_a_panic_or_a_confident_guess() {
        let c = classify::classify_heuristically("");
        assert_eq!(c.confidence(), Confidence::Low);
        assert_eq!(c.conservative_workload_tier(), WorkloadTier::Standard);
    }

    /// `classify`'s doc comment claims the caller's `model_output` is
    /// "never called for here" — it is a plain `Option` the function either
    /// returns as-is or ignores in favour of the heuristic. Proven by a
    /// model answer that actively disagrees with what the heuristic would
    /// say about the same text, so a bug that merged the two rather than
    /// picking one outright would be visible.
    #[test]
    fn classify_returns_the_model_answer_unmodified_even_when_it_disagrees_with_the_heuristic() {
        let text = "run cargo test and fix whatever fails";
        let heuristic_only = classify::classify_heuristically(text);
        assert_eq!(heuristic_only.workload_tier(), WorkloadTier::Heavy);

        let disagreeing_model_answer = TaskClassification::new(
            false,
            false,
            false,
            false,
            Complexity::Trivial,
            false,
            WorkloadTier::Leaf,
            true,
            WarmContextValue::PreferStrongerCold,
            Confidence::High,
            ClassificationSource::Model {
                label: "disagreeing-test-model".to_owned(),
            },
        );
        let result = classify::classify(text, Some(disagreeing_model_answer.clone()));
        assert_eq!(
            result, disagreeing_model_answer,
            "classify() must return the supplied model answer exactly, not a blend of it and \
             the heuristic"
        );
        assert_ne!(result.workload_tier(), heuristic_only.workload_tier());
    }
}

/// The one module in this file that goes through the shipped binary rather
/// than a library call. Every test above proves `classify`/`classify_heuristically`
/// behave correctly as functions; none of them proves anything calls those
/// functions in the binary a person actually runs. Per §35, a test written
/// against `classify_heuristically` directly would keep passing even if
/// `main.rs`'s `Classify` arm were deleted outright — nothing would call the
/// module and nothing would fail. This spawns the real executable
/// (`CARGO_BIN_EXE_glasshouse`, the same idiom `launch_overlay.rs` and
/// `pty_smoke.rs` already use) and reads its real stdout, so deleting that
/// arm's body is the one mutation these two tests exist to catch.
mod command_dispatch {
    use std::process::Command;

    fn glasshouse(workspace: &std::path::Path, data: &std::path::Path, args: &[&str]) -> String {
        let output = Command::new(env!("CARGO_BIN_EXE_glasshouse"))
            .arg("--scope")
            .arg(workspace)
            .arg("--data-dir")
            .arg(data)
            .arg("--config-dir")
            .arg(data)
            .args(args)
            .output()
            .expect("the glasshouse binary must be runnable");
        assert!(
            output.status.success(),
            "glasshouse {args:?} exited with {:?}\nstderr: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).expect("stdout must be UTF-8")
    }

    fn workspace_and_data() -> (tempfile::TempDir, tempfile::TempDir) {
        let workspace = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(workspace.path().join(".git")).unwrap();
        let data = tempfile::tempdir().unwrap();
        (workspace, data)
    }

    /// `glasshouse classify` on shell-shaped text must report heavy, no
    /// model, and shell execution needed — through real argument parsing and
    /// real command dispatch, not a function call this test wrote itself.
    #[test]
    fn glasshouse_classify_reaches_the_heuristic_through_the_real_command_dispatch() {
        let (workspace, data) = workspace_and_data();
        let stdout = glasshouse(
            workspace.path(),
            data.path(),
            &[
                "classify", "run", "cargo", "test", "and", "fix", "whatever", "fails",
            ],
        );

        assert!(
            stdout.contains("deterministic heuristics"),
            "no model is wired up in this build, so the source line must say so:\n{stdout}"
        );
        assert!(
            stdout.contains("shell execution         yes"),
            "a shell-shaped request must be reported as needing shell execution:\n{stdout}"
        );
        assert!(
            stdout.contains("workload tier           heavy"),
            "shell + code modification must reach heavy through the real dispatch:\n{stdout}"
        );
    }

    /// The other end of the scale, through the same real dispatch: a plain
    /// question reads as leaf tier and safe for a disposable model.
    #[test]
    fn glasshouse_classify_on_a_generic_question_is_leaf_tier_and_safe_for_disposable() {
        let (workspace, data) = workspace_and_data();
        let stdout = glasshouse(
            workspace.path(),
            data.path(),
            &["classify", "what", "is", "a", "mutex?"],
        );

        assert!(
            stdout.contains("workload tier           leaf"),
            "a plain question must reach leaf tier through the real dispatch:\n{stdout}"
        );
        assert!(
            stdout.contains("safe for disposable     yes"),
            "a plain question must be reported safe for a disposable model:\n{stdout}"
        );
    }
}

// =====================================================================
// Cadence availability — line 1546
// =====================================================================
//
// `provider_health` folds a provider-declared wait and Glasshouse's own
// invented backoff into one history-shaped term. These tests prove the two
// are now genuinely separable: a declared wait scores worse on the new
// `cadence_availability` term, and an invented cooldown — Glasshouse's own
// caution, deliberately probeable by real work (Phase 9I line 534) — does
// not score as a cadence claim at all, even while `provider_health` still
// reflects it.
mod cadence_availability_scoring {
    use super::*;
    use glasshouse::integrations::IntegrationId;
    use glasshouse::routing::session::{Destination, cadence_availability, provider_health};

    fn destination(provider: &str, model: &str) -> Destination {
        Destination::fresh(
            format!("{provider}-{model}"),
            IntegrationId::ClaudeCode,
            "default",
            backend(provider, model),
            None,
        )
    }

    /// Acceptance test 1: a resource put into a declared wait reports
    /// cadence-unavailable, and scores strictly worse than an untouched one.
    #[test]
    fn a_declared_wait_scores_strictly_worse_on_cadence_than_an_untouched_resource() {
        let now = Instant::now();
        let mut pool = FreePool::new();
        let waiting = free_resource("openrouter", "free-model");

        pool.observe(
            &waiting,
            WorkloadOutcome::RateLimited {
                retry_after: Some(Duration::from_secs(60)),
            },
            now,
        );

        let waiting_dest = destination("openrouter", "free-model");
        let untouched_dest = destination("openrouter", "other-model");

        let waiting_cadence = cadence_availability(&waiting_dest, &pool, now);
        let untouched_cadence = cadence_availability(&untouched_dest, &pool, now);

        assert_eq!(
            untouched_cadence.magnitude(),
            0.0,
            "an untouched resource must report cadence as inert, not a partial claim"
        );
        assert!(
            waiting_cadence.magnitude() < untouched_cadence.magnitude(),
            "a destination inside a declared wait must score strictly worse on cadence than one \
             that is not: {} vs {}",
            waiting_cadence.magnitude(),
            untouched_cadence.magnitude()
        );
    }

    /// Acceptance test 2 — the assertion that carries the line: a resource
    /// cooled down by invented backoff only (repeated failures, no stated
    /// wait) reports cadence-**available**, while `provider_health` still
    /// reflects the failures. Without this, the two terms are not proven
    /// separate — a term that merely renamed `provider_health` would fail
    /// this test and pass every other one.
    #[test]
    fn invented_backoff_alone_reports_cadence_available_while_provider_health_reflects_it() {
        let now = Instant::now();
        let mut pool = FreePool::new();
        let it = free_resource("openrouter", "free-model");

        for _ in 0..FAILURES_BEFORE_COOLDOWN {
            pool.observe(&it, WorkloadOutcome::CapacityFailure, now);
        }
        assert!(
            !pool.is_available(&it, now),
            "premise: repeated ordinary failures with no stated wait must cool the resource down"
        );

        let dest = destination("openrouter", "free-model");

        let cadence = cadence_availability(&dest, &pool, now);
        assert_eq!(
            cadence.magnitude(),
            0.0,
            "an invented cooldown is Glasshouse's own caution, not a provider cadence, and must \
             not score as one"
        );

        let health = provider_health(&dest, &pool, now);
        assert!(
            health.magnitude() < 0.0,
            "provider_health must still reflect the failures even though cadence reports \
             nothing: got {}",
            health.magnitude()
        );
    }

    /// Acceptance test 3: `Served` clears both a declared wait and the
    /// general health history it also clears.
    #[test]
    fn served_clears_both_a_declared_wait_and_general_health() {
        let now = Instant::now();
        let mut pool = FreePool::new();
        let it = free_resource("openrouter", "free-model");

        pool.observe(
            &it,
            WorkloadOutcome::RateLimited {
                retry_after: Some(Duration::from_secs(60)),
            },
            now,
        );
        assert!(
            pool.health(&it).declared_wait_remaining(now).is_some(),
            "premise: a declared wait must be in effect before Served can be shown to clear it"
        );

        pool.observe(&it, WorkloadOutcome::Served, now);

        let dest = destination("openrouter", "free-model");

        let cadence = cadence_availability(&dest, &pool, now);
        assert_eq!(
            cadence.magnitude(),
            0.0,
            "Served must clear a declared wait outright, not leave it to expire on its own"
        );

        let health = provider_health(&dest, &pool, now);
        assert_eq!(
            health.magnitude(),
            0.0,
            "Served must also clear the general health history the same outcome always clears"
        );
    }
}
