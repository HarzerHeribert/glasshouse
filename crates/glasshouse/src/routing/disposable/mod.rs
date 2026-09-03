//! Routing for bounded internal jobs — the second policy class (Phase 9I).
//!
//! # What a disposable job is, and why it does not share a router
//!
//! A disposable job is a bounded, non-conversational request Glasshouse makes
//! for its own purposes: classifying a request before spending premium agent
//! capacity, extracting memories from a finished session, reranking search
//! results. Phase 9I line 530 names those three.
//!
//! Nothing about them resembles a live coding session. They have no
//! conversation prefix worth keeping warm, no tools, no user watching a
//! cursor, and no cost to being served by a different model than the last one
//! was. Line 533 therefore asks that they be routed by a **separate policy
//! class**, and the module header of [`mod@super`] lists the three
//! independent ways that separation is made structural here.
//!
//! The practical content of the separation is one sentence: this policy
//! **prefers free capacity and re-decides every time**, and the interactive
//! policy **keeps what it has and re-decides only after a real failure**.
//!
//! # Glasshouse's own test and evaluation runs
//!
//! Phase 9I line 539 — *"allow Glasshouse's own automated evaluation and test
//! runs to use configured zero-cost models, and never a metered resource
//! without an explicit opt-in"* — is an acceptance condition, not a
//! preference. A test run that silently spends the user's money is the worst
//! outcome this module can produce, and it is worse than a failing test.
//!
//! It is enforced by construction rather than by a check a caller might
//! forget: a routing policy is built with a [`MeteredUse`], the value that
//! Glasshouse's own runs are built with is [`MeteredUse::Withheld`], and a
//! [`DisposableChoice`] on a metered resource cannot be produced from a
//! policy holding it. There is no second door — [`DisposableChoice`]'s fields
//! are private and nothing else in the crate constructs one.

use std::collections::BTreeSet;
use std::time::Instant;

use super::free::{FreePool, FreePreferences};
use super::{
    Contribution, Cost, CredentialId, EligibleCandidate, HardConstraint, RoutingExplanation,
    UseReason, apply_hard_constraints,
};
use crate::provider::quota::{
    CapacityBand, ReserveDecision, ReserveDecisionInputs, evaluate_reserve_spend,
};
use crate::provider::registry::Locality;
use crate::provider::telemetry::RetainedPick;
use crate::routing::classify::{TaskClassification, WorkloadTier};
use crate::routing::evidence::MIN_SAMPLE_FOR_SUMMARY;
use crate::routing::pressure::{ReservePolicy, ReserveScope};

mod candidates;
mod classification;
#[cfg(test)]
mod tests;

use candidates::has_no_known_headroom;
pub use candidates::{CandidateCapacity, DisposableCandidate, JobKind, MeteredUse};
use classification::{
    CLASSIFICATION_PREFERENCE_WEIGHT, ClassificationVerdict, LATENCY_PREFERENCE_WEIGHT,
    REQUESTS_PER_MINUTE_DIMENSION, TimePricePreference, cheapest_priced_metered,
    classification_verdict, time_price_preference,
};
pub use classification::{
    CLASSIFICATION_RELIABILITY_FLOOR, CLASSIFICATION_RELIABILITY_MIN_OBSERVATIONS,
    ClassificationPolicy, estimated_classification_cost_micro_usd,
};

/// The resource one disposable job was routed to, and why.
///
/// **No public fields, and no conversion to or from
/// [`super::interactive::Assignment`].** That is the type-level half of
/// line 533: a caller holding one of these cannot turn it into the thing that
/// serves a live coding session, and vice versa.
#[derive(Debug, Clone, PartialEq)]
pub struct DisposableChoice {
    job: JobKind,
    provider: String,
    model: String,
    credential: CredentialId,
    cost: Cost,
    reason: UseReason,
    /// Map line 1554: the top candidate plus a concise explanation of the
    /// most important reasons it won — [`DisposableRouting::score`]'s
    /// output for the candidate that was actually chosen.
    ///
    /// **One exception: a retained pick (map lines 1441/1442,
    /// [`AutomaticClassificationDecision::Retained`]).** No ranking ran for
    /// it, so its explanation says exactly that — reused without
    /// re-ranking, and when it was originally chosen — rather than reading
    /// as `score`'s output for a comparison that never happened.
    explanation: RoutingExplanation,
}

impl DisposableChoice {
    pub fn job(&self) -> JobKind {
        self.job
    }

    pub fn provider(&self) -> &str {
        &self.provider
    }

    pub fn model(&self) -> &str {
        &self.model
    }

    pub fn credential(&self) -> &CredentialId {
        &self.credential
    }

    pub fn cost(&self) -> Cost {
        self.cost
    }

    /// Phase 9I line 540 — user preference, quota preservation, or fallback.
    pub fn reason(&self) -> UseReason {
        self.reason
    }

    /// Map line 1554: why this candidate won, as a named, inspectable list
    /// of contributions rather than an opaque number.
    pub fn explanation(&self) -> &RoutingExplanation {
        &self.explanation
    }

    /// A line a settings screen or a diagnostic can show. Names only.
    pub fn describe(&self) -> String {
        format!(
            "{} on {} — {}, used by {}\n{}",
            self.model,
            self.provider,
            self.cost.as_str(),
            self.reason,
            self.explanation.render()
        )
    }
}

/// Why no resource could be chosen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NoResource {
    /// Nothing was configured for this job at all.
    NothingConfigured,
    /// The user pinned a free resource and it cannot serve right now. A pin
    /// that silently fell back to something else would not be a pin.
    PinnedResourceUnavailable { provider: String, model: String },
    /// Every free resource is cooling down, disabled or out of allowance, and
    /// spending metered capacity is not permitted.
    ///
    /// This is line 539's refusal, and it is a **failure** on purpose: an
    /// automated Glasshouse run that cannot find a zero-cost model stops,
    /// rather than quietly buying one.
    NoFreeResourceAndMeteredWithheld { withheld: MeteredUse },
    /// Every free resource failed or was absent, metered spending is
    /// permitted in principle, but Phase 32F's protected-reserve policy
    /// denied every metered candidate that could otherwise have served —
    /// map line 1550. `reasons` names each denial, one per candidate.
    ProtectedReserveDenied { reasons: Vec<String> },
    /// Every configured candidate was excluded by a classification
    /// requirement before ranking ran — capability map lines 1427, 1432
    /// and 1435. `reasons` names each exclusion, one per candidate, so a
    /// caller that falls back to deterministic heuristics can say why no
    /// model was asked.
    ClassificationRequirementsExcludedAll { reasons: Vec<String> },
    /// Every configured candidate's entitlement has a rule that does not
    /// serve this job's kind — map line 1947's job-kind clause. `reasons`
    /// names each refusal: the candidate, the entitlement and the job kind,
    /// exactly as the session router's rejection names an entitlement and a
    /// harness. Distinct from
    /// [`Self::NoFreeResourceAndMeteredWithheld`] because "your rule forbids
    /// this" and "the pool is exhausted" call for different fixes, and
    /// reporting the first as the second would hide the user's own rule from
    /// them.
    EntitlementDeniesEveryCandidate { reasons: Vec<String> },
}

impl std::fmt::Display for NoResource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NothingConfigured => {
                f.write_str("no provider is configured for Glasshouse's own support work")
            }
            Self::PinnedResourceUnavailable { provider, model } => write!(
                f,
                "the pinned free resource `{model}` on `{provider}` cannot serve right now, and a \
                 pin is not a preference to fall back from"
            ),
            Self::NoFreeResourceAndMeteredWithheld { withheld } => write!(
                f,
                "no configured zero-cost model can serve this job, and {} — set {}=1 to allow \
                 one, which spends real money",
                withheld.describe(),
                MeteredUse::OPT_IN_VAR
            ),
            Self::ProtectedReserveDenied { reasons } => write!(
                f,
                "every free resource failed or was absent, and the protected-reserve policy \
                 denied every metered candidate: {}",
                reasons.join("; ")
            ),
            Self::ClassificationRequirementsExcludedAll { reasons } => write!(
                f,
                "every configured candidate was excluded by a classification requirement before \
                 ranking, so no model was asked: {}",
                reasons.join("; ")
            ),
            Self::EntitlementDeniesEveryCandidate { reasons } => write!(
                f,
                "every configured candidate's entitlement refuses this job kind: {}",
                reasons.join("; ")
            ),
        }
    }
}

/// The user's own override of protected-reserve protection — capability map
/// line 1290, *"allow the user to override reserve protection for a specific
/// task or session"*.
///
/// # Why this is a pair and not a boolean
///
/// [`crate::provider::quota::ReserveDecisionInputs::user_override`] is a
/// `bool`, and a `bool` is all a policy function should need. The scope
/// belongs one level up, here, because a boolean *setting* would be a
/// different capability from the one line 1290 asks for: set once, it would
/// spend protected reserve for every job in every session for ever, and no
/// reason string could say on whose behalf.
///
/// So the scope is part of the value. One half is the set of sessions the
/// user named; the other is the session this routing instance is deciding
/// for; and [`ReserveOverride::applies`] is true only where the two meet.
/// **There is deliberately no constructor meaning "everywhere"** — a user who
/// wants two sessions overridden names two sessions — and
/// [`ReserveOverride::default`] is the empty override that every caller
/// predating this line already gets.
///
/// # The task half of "task or session", which is not built
///
/// Only the session half exists, because only the session half has an
/// identifier on this path. A disposable job carries a [`JobKind`] —
/// `memory-extraction` or `classification` — which names a *class* of work
/// rather than one task, so a `JobKind`-scoped override would be a
/// category-wide switch wearing a scope's clothes: precisely the shape the
/// paragraph above refuses. Nothing in this build gives one disposable job an
/// identity its successor does not share, so there is nothing narrower to
/// name. The line is a disjunction and the session half is real; the task
/// half is recorded here as absent rather than approximated.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ReserveOverride {
    /// The sessions the user named, as
    /// [`crate::config::EffectiveConfig::reserve_override_sessions`] resolved
    /// them. A [`BTreeSet`] so the membership test does not depend on the
    /// order a configuration file happened to list them in.
    sessions: BTreeSet<String>,
    /// The session whose work this routing instance is deciding for, when the
    /// caller knows one.
    ///
    /// `None` is every caller that predates line 1290, and it can never
    /// match — which is what keeps this type's arrival a no-op for them.
    deciding_for: Option<String>,
}

impl ReserveOverride {
    /// No override at all: the reserve policy decides on its own signals.
    pub fn none() -> Self {
        Self::default()
    }

    /// The sessions the user named. Naming none is the same as [`Self::none`].
    pub fn for_sessions<S: Into<String>>(sessions: impl IntoIterator<Item = S>) -> Self {
        Self {
            sessions: sessions.into_iter().map(Into::into).collect(),
            deciding_for: None,
        }
    }

    /// Point this override at the session actually being decided for.
    ///
    /// Separate from [`Self::for_sessions`] because the two facts come from
    /// different places — the set from configuration, the subject from
    /// whichever caller is routing — and a single constructor taking both
    /// would invite a caller to pass the same value twice and prove nothing.
    #[must_use]
    pub fn deciding_for(mut self, session: impl Into<String>) -> Self {
        self.deciding_for = Some(session.into());
        self
    }

    /// Whether the user overrode reserve protection *for this decision*.
    ///
    /// False whenever the user named nothing, whenever the caller named no
    /// session, and — the case that matters — whenever the session being
    /// decided for is not one the user named.
    pub fn applies(&self) -> bool {
        self.deciding_for
            .as_deref()
            .is_some_and(|session| self.sessions.contains(session))
    }

    /// The session this override was granted for, when it applies — for the
    /// routing explanation, so a spend of protected reserve names the scope
    /// the user actually gave rather than only the fact of an override.
    pub fn granted_session(&self) -> Option<&str> {
        self.applies()
            .then_some(self.deciding_for.as_deref())
            .flatten()
    }
}

/// How long automatic classification's retained pick may be reused before a
/// fresh decision is required — map line 1442's "a short period", which
/// names no figure.
///
/// Tied to something that already exists rather than invented: one
/// requests-per-minute ceiling window, `crate::provider::telemetry::MINUTE_SECONDS`
/// — the same period this build already treats as the unit one rate-limit
/// reading governs (`provider::quota::CapacityState::remaining_capacity_score`'s
/// own requests-per-minute pairing). A pick should not outlive the window the
/// capacity reading that justified choosing it is itself scoped to.
pub const AUTOMATIC_CLASSIFICATION_STICKY_WINDOW_SECONDS: i64 =
    crate::provider::telemetry::MINUTE_SECONDS;

/// One outcome of [`DisposableRouting::choose_for_automatic_classification`]
/// — map lines 1441 and 1442.
#[derive(Debug, Clone, PartialEq)]
pub enum AutomaticClassificationDecision {
    /// The retained pick was reused: still inside the window, still naming a
    /// candidate this call was given, and still healthy. [`DisposableRouting::choose`]
    /// did not run. The [`DisposableChoice`] is built here, from the
    /// retained candidate, rather than handed to a caller as a bare
    /// [`RetainedPick`] — `DisposableChoice` has no public fields and no
    /// public constructor, so nothing outside this module could build one.
    Retained(DisposableChoice),
    /// A fresh decision was made — no usable retained pick, the window had
    /// elapsed, or the retained resource's health had turned against it.
    /// Carries both the ordinary [`DisposableChoice`] and the
    /// [`RetainedPick`] a caller should persist for the next call.
    Fresh(DisposableChoice, RetainedPick),
}

/// The routing policy for bounded internal jobs.
#[derive(Debug, Clone)]
pub struct DisposableRouting {
    metered: MeteredUse,
    /// The user's `prefer free resources` setting, from
    /// `crate::config::RoutingConfig::prefer_free`. It changes the *reason*
    /// reported for a free choice, never whether a free resource is
    /// preferred — this policy prefers free capacity for support work either
    /// way, which is line 530.
    prefer_free_setting: bool,
    preferences: FreePreferences,
    /// The user's scoped override of protected-reserve protection —
    /// capability map line 1290. [`ReserveOverride::none`] unless a caller
    /// said otherwise, so every construction that predates the line keeps
    /// exactly the behaviour it had.
    reserve_override: ReserveOverride,
    /// What the user's `[routing.reserve]` policy for **background** work
    /// makes of a reserve-band candidate — capability map line 1577's
    /// second half.
    ///
    /// # Why one policy and not both scopes' policies
    ///
    /// [`crate::routing::pressure::ReservePolicies`] carries two fields, and
    /// this router could have been handed the pair and selected from it.
    /// It is deliberately handed the **already-selected** value instead:
    /// `routing::tests::the_two_policy_classes_do_not_name_each_other` holds
    /// this module to never naming the other policy class, and a router that
    /// carried the other scope's policy would be holding a value it must
    /// never read. Selecting at the caller — `ReservePolicies::for_scope`,
    /// the one place the selection is made — makes the other scope's policy
    /// *unrepresentable* here rather than merely unread.
    ///
    /// [`ReservePolicy::Protect`] by default, which is the behaviour every
    /// caller predating line 1577 already had.
    reserve_policy: ReservePolicy,
    /// The classification-side requirements — capability map lines 1427
    /// and 1435 — consulted by [`Self::choose_for_automatic_classification`]
    /// alone. [`ClassificationPolicy::default`] applies nothing, so every
    /// construction that predates those lines keeps exactly the behaviour
    /// it had.
    classification_policy: ClassificationPolicy,
}

impl DisposableRouting {
    /// Ordinary support work: prefer free, fall back to metered when nothing
    /// free can serve.
    pub fn for_support_work(prefer_free_setting: bool, preferences: FreePreferences) -> Self {
        Self {
            metered: MeteredUse::Permitted,
            prefer_free_setting,
            preferences,
            reserve_override: ReserveOverride::none(),
            reserve_policy: ReservePolicy::default(),
            classification_policy: ClassificationPolicy::default(),
        }
    }

    /// Phase 9I line 539: the policy Glasshouse's own automated evaluation and
    /// test runs are built with.
    ///
    /// `metered` comes from [`MeteredUse::for_automated_run`], whose default
    /// is [`MeteredUse::Withheld`]. There is no constructor here that takes
    /// [`MeteredUse::Permitted`] for an automated run, so an automated run
    /// cannot be given ordinary support work's permission by accident.
    pub fn for_glasshouses_own_run(metered: MeteredUse, preferences: FreePreferences) -> Self {
        let metered = match metered {
            // An automated run may not silently inherit ordinary support
            // work's permission. Anything but a named opt-in is withheld.
            MeteredUse::Permitted => MeteredUse::Withheld,
            given => given,
        };
        Self {
            metered,
            prefer_free_setting: true,
            preferences,
            reserve_override: ReserveOverride::none(),
            reserve_policy: ReservePolicy::default(),
            classification_policy: ClassificationPolicy::default(),
        }
    }

    pub fn metered_use(&self) -> &MeteredUse {
        &self.metered
    }

    pub fn preferences(&self) -> &FreePreferences {
        &self.preferences
    }

    /// Carry the user's scoped reserve override — capability map line 1290.
    ///
    /// A builder rather than a constructor argument, and deliberately: both
    /// constructors above describe *what kind of work* this policy is for,
    /// which is a permanent property, while an override is a statement about
    /// one session that most callers will never make. Omitting it is
    /// [`ReserveOverride::none`], which is what every existing caller does.
    #[must_use]
    pub fn with_reserve_override(mut self, reserve_override: ReserveOverride) -> Self {
        self.reserve_override = reserve_override;
        self
    }

    /// The override this policy is carrying, for a caller that wants to
    /// report it.
    pub fn reserve_override(&self) -> &ReserveOverride {
        &self.reserve_override
    }

    /// Carry the user's reserve policy for background work — capability map
    /// line 1577.
    ///
    /// The value is expected to come from
    /// `EffectiveConfig::reserve_policies().for_scope(ReserveScope::Background)`,
    /// which is the one place in the build that selects a scope's policy.
    /// Omitting it is [`ReservePolicy::Protect`], the fail-closed default a
    /// spending protection must have and exactly what every caller that
    /// predates this line already got.
    ///
    /// A builder for [`Self::with_reserve_override`]'s reason: both
    /// constructors say what kind of work this policy is for, and how a
    /// person wants their reserve treated is a separate statement that most
    /// callers of a routing policy never make.
    #[must_use]
    pub fn with_reserve_policy(mut self, policy: ReservePolicy) -> Self {
        self.reserve_policy = policy;
        self
    }

    /// The background reserve policy this router is carrying.
    pub fn reserve_policy(&self) -> ReservePolicy {
        self.reserve_policy
    }

    /// Carry the user's classification requirements — capability map lines
    /// 1427 and 1435. A builder for [`Self::with_reserve_override`]'s
    /// reason: both constructors describe what kind of work this policy is
    /// for, and a latency ceiling or a privacy confinement is a statement
    /// about one job class that most callers never make.
    #[must_use]
    pub fn with_classification_policy(mut self, policy: ClassificationPolicy) -> Self {
        self.classification_policy = policy;
        self
    }

    pub fn classification_policy(&self) -> &ClassificationPolicy {
        &self.classification_policy
    }

    /// Choose a resource for one bounded job.
    ///
    /// # The order, and where each step comes from
    ///
    /// 1. **Hard constraints first, structurally** (map line 1553):
    ///    [`apply_hard_constraints`] removes any candidate this policy could
    ///    never use — a candidate whose entitlement's rules do not serve this
    ///    job's kind (map line 1947's third clause, refused as
    ///    [`HardConstraint::Entitlement`] by the entitlement's name; the
    ///    refusal rides the winner's explanation, and when nothing survives
    ///    the error names every one), and the metered candidates
    ///    [`MeteredUse`] withholds, named [`HardConstraint::UserConstraint`]
    ///    because line 568 calls a user's own opt-in rule exactly that. A
    ///    candidate that fails this is unrepresentable to the scorer below,
    ///    not merely given a large negative weight.
    /// 2. **Zero-headroom candidates are removed, not merely ranked last**
    ///    (map line 1434). A candidate whose [`CandidateCapacity`], carried on
    ///    [`DisposableCandidate::with_capacity`], is *known* to read zero
    ///    remaining headroom — no requests-per-minute (or other bound
    ///    dimension) capacity left — cannot serve, so it is dropped here,
    ///    before either the free loop or the metered-fallback loop below ever
    ///    sees it. An **absent** reading never eliminates: nothing being known
    ///    about a candidate is not the same claim as "this candidate is
    ///    exhausted", and turning "no telemetry" into "full" is the
    ///    fabrication this project refuses everywhere else — see
    ///    `tests/routing_disposable_tier.rs`'s
    ///    `an_absent_capacity_reading_never_eliminates_a_candidate`.
    ///    Removing rather than scoring low also means this step runs *before*
    ///    the free loop below walks the user's own order, so a candidate that
    ///    survives is never reordered by it — only ever removed outright.
    /// 3. **A pinned free resource wins outright** (line 536, 1552). If it
    ///    cannot serve, the job fails rather than silently going elsewhere —
    ///    a pin is a hard rule, never a scored preference, the same design
    ///    decision Phase 9J's `PairingPreference::Pin` already made.
    /// 4. **Free resources, in the user's own order**, skipping disabled ones
    ///    (line 536) and any whose health or allowance says it cannot serve
    ///    right now (lines 529, 535, 538). This is line 530's "prefer free
    ///    models for bounded Glasshouse support work", and line 531 falls out
    ///    of it: a model is in this list because the user marked it free, so
    ///    an explicitly configured free model such as a Nemotron variant
    ///    participates without this function knowing any model's name.
    /// 5. **A metered resource**, only when [`MeteredUse`] permits it
    ///    (line 539) *and* Phase 32F's protected-reserve policy allows
    ///    spending it (line 1550) — ranked by this policy's own `score`
    ///    when more than one survives the reserve gate.
    ///
    /// Every candidate this function actually reaches is scored by this
    /// policy's own `score` method (map line 1530), and the winner's
    /// [`RoutingExplanation`] travels home on
    /// [`DisposableChoice::explanation`] (line 1554). The free-tier winner is
    /// still the first available candidate in the user's own order, exactly
    /// as before this batch — every input this build can populate for a free
    /// candidate (cost, order position) is monotonic in that same order, and
    /// an absent capacity or reset reading contributes `0.0` for every
    /// candidate alike, so scoring never disagrees with it; see
    /// `tests::scoring_never_reorders_the_existing_free_selection`.
    ///
    /// `classification` is this job's Phase 35 [`TaskClassification`], when a
    /// caller has one — [`TaskClassification::conservative_workload_tier`]
    /// becomes the metered-fallback path's [`WorkloadTier`] (map line 1550's
    /// `tier` input), replacing the fixed [`WorkloadTier::Leaf`] this policy
    /// used before a classification existed to ask. `None` keeps that fixed
    /// [`WorkloadTier::Leaf`] behaviour exactly as it was: a caller with
    /// nothing to classify is not made to guess.
    pub fn choose(
        &self,
        job: JobKind,
        candidates: &[DisposableCandidate],
        pool: &FreePool,
        now: Instant,
        classification: Option<&TaskClassification>,
    ) -> Result<DisposableChoice, NoResource> {
        if candidates.is_empty() {
            return Err(NoResource::NothingConfigured);
        }

        // Map line 1947's job-kind clause, asked first and named: a
        // candidate whose entitlement's rules do not serve this job's kind
        // is never a candidate — not scored, not walked by either loop below
        // — exactly as the session router removes a destination whose
        // entitlement does not serve the harness. The refusal carries the
        // entitlement's name and the job kind, and it is *kept*: each one
        // travels on the winner's explanation, and when nothing survives at
        // all the error names every one rather than misreporting the pool as
        // merely exhausted.
        let (eligible, rejected) = apply_hard_constraints(candidates.to_vec(), |candidate| {
            if let Some(entitlement) = candidate.entitlement() {
                entitlement.job_constraint(job)?;
            }
            if candidate.cost().is_free() || self.metered.permits_metered() {
                Ok(())
            } else {
                Err(HardConstraint::UserConstraint)
            }
        });
        let entitlement_refusals: Vec<(String, String)> = rejected
            .iter()
            .filter_map(|(candidate, constraint)| match constraint {
                HardConstraint::Entitlement { .. } => Some((
                    format!("{} on {}", candidate.model(), candidate.provider()),
                    constraint
                        .reason()
                        .expect("an entitlement constraint always carries a reason"),
                )),
                _ => None,
            })
            .collect();
        if eligible.is_empty() && !entitlement_refusals.is_empty() {
            return Err(NoResource::EntitlementDeniesEveryCandidate {
                reasons: entitlement_refusals
                    .into_iter()
                    .map(|(candidate, reason)| format!("{candidate}: {reason}"))
                    .collect(),
            });
        }
        let refusal_notes: Vec<Contribution> = entitlement_refusals
            .iter()
            .map(|(candidate, reason)| {
                Contribution::new(
                    "entitlement rule",
                    0.0,
                    format!("{candidate} is not a candidate — {reason} (map line 1947)"),
                )
            })
            .collect();

        // Map line 1434, step 2 of the order documented above: a candidate
        // whose remaining capacity is *known* and reads zero headroom is
        // removed here, before it can be the free loop's first-available
        // pick or the metered loop's best-scored one. `has_no_known_headroom`
        // treats an absent reading as `false` on purpose — see its own doc.
        let eligible: Vec<EligibleCandidate<DisposableCandidate>> = eligible
            .into_iter()
            .filter(|candidate| !has_no_known_headroom(candidate.value()))
            .collect();

        let free: Vec<&EligibleCandidate<DisposableCandidate>> = eligible
            .iter()
            .filter(|candidate| candidate.value().cost().is_free())
            .collect();

        if let Some(pin) = self.preferences.pin() {
            let pinned = free
                .iter()
                .find(|candidate| candidate.value().key() == *pin)
                .filter(|candidate| pool.is_available(&candidate.value().as_free_resource(), now));
            return match pinned {
                Some(candidate) => {
                    let explanation = self.score_pinned(candidate);
                    Ok(self.finish(
                        job,
                        candidate.value(),
                        UseReason::UserPreference,
                        explanation,
                        &refusal_notes,
                    ))
                }
                None => Err(NoResource::PinnedResourceUnavailable {
                    provider: pin.provider.clone(),
                    model: pin.model.clone(),
                }),
            };
        }

        let arranged = self.preferences.arrange(
            &free
                .iter()
                .map(|c| c.value().as_free_resource())
                .collect::<Vec<_>>(),
        );
        let mut first_choice: Option<&DisposableCandidate> = None;
        for (position, resource) in arranged.iter().enumerate() {
            let Some(candidate) = free
                .iter()
                .find(|candidate| candidate.value().as_free_resource() == *resource)
            else {
                continue;
            };
            first_choice.get_or_insert(candidate.value());
            if pool.is_available(resource, now) {
                // The reason a free resource is the one being used — line 540.
                // "Fallback" outranks the others because it is the most
                // informative: it says the resource the user would have got
                // could not serve.
                let reason = if first_choice.is_some_and(|first| first != candidate.value()) {
                    UseReason::Fallback
                } else if self.prefer_free_setting {
                    UseReason::UserPreference
                } else {
                    // The disposable class does not spend metered capacity on
                    // throwaway work as a standing rule, whether or not a
                    // metered resource happens to be configured beside it.
                    UseReason::QuotaPreservation
                };
                let explanation = self.score(candidate, Some(position), arranged.len(), None);
                return Ok(self.finish(
                    job,
                    candidate.value(),
                    reason,
                    explanation,
                    &refusal_notes,
                ));
            }
        }

        // `eligible` already holds no metered candidate when `MeteredUse`
        // withholds them — the hard constraint above removed it, not this
        // loop — so an empty metered set here means either nothing metered
        // was ever configured or every one of them was withheld; both read
        // the same to a caller (line 539's refusal either way).
        //
        // Every free resource is gone or absent by this point, which answers
        // the *free* half of Phase 32F's "cheaper adequate resource" question
        // — reaching this line already proved there was none. The metered
        // half is the one this loop still has to ask, and
        // [`cheaper_adequate_resource_exists`] asks it.
        let metered: Vec<&EligibleCandidate<DisposableCandidate>> = eligible
            .iter()
            .filter(|candidate| !candidate.value().cost().is_free())
            .collect();
        let mut denied_reasons = Vec::new();
        let mut best: Option<(
            &EligibleCandidate<DisposableCandidate>,
            RoutingExplanation,
            f64,
        )> = None;
        for (index, candidate) in metered.iter().copied().enumerate() {
            let decision = evaluate_reserve_spend(ReserveDecisionInputs {
                band: candidate
                    .value()
                    .capacity
                    .band
                    .unwrap_or(CapacityBand::Plenty),
                tier: classification
                    .map(TaskClassification::conservative_workload_tier)
                    .unwrap_or(WorkloadTier::Leaf),
                cheaper_adequate_resource_exists: cheaper_adequate_resource_exists(&metered, index),
                // Line 1290, scoped: true only when the session this policy
                // was built for is one the user actually named. See
                // [`ReserveOverride`] for why the scope lives here and not
                // in the policy function.
                user_override: self.reserve_override.applies(),
                seconds_until_reset: candidate.value().capacity.seconds_until_reset,
                // Line 1294, and it stays a literal on purpose — nothing in
                // this build can observe that a task is nearly complete, and
                // [`ReserveDecisionInputs::task_nearly_complete`] records why
                // a proxy must not be invented for it.
                task_nearly_complete: false,
            });
            // Capability map line 1577, second half. The reserve policy the
            // user set for *background* work decides what happens to a
            // denial here: `Protect` (the default) leaves it standing, and
            // `Spend` removes it. It removes the **denial**, not the
            // pressure — `super::pressure`'s own ruling for the other scope,
            // kept identical here so one word does not mean two things — so
            // `score` below still renders the band and the reason this
            // candidate was denied on its own signals.
            //
            // Only a *denied* decision is affected. An allowed one is
            // allowed under either policy, and a policy that could turn an
            // allowance into a denial would be a second, unasked-for
            // protection wearing this line's name.
            let admitted = decision.is_allowed() || self.reserve_policy == ReservePolicy::Spend;
            if !admitted {
                denied_reasons.push(format!(
                    "{}: {}",
                    candidate.value().model(),
                    decision.reason()
                ));
                continue;
            }
            let explanation = self.score(candidate, None, 0, Some(&decision));
            let total = explanation.total();
            let is_better = match &best {
                None => true,
                Some((_, _, best_total)) => total > *best_total,
            };
            if is_better {
                best = Some((candidate, explanation, total));
            }
        }

        match best {
            Some((candidate, explanation, _)) => Ok(self.finish(
                job,
                candidate.value(),
                UseReason::Fallback,
                explanation,
                &refusal_notes,
            )),
            None if denied_reasons.is_empty() => {
                Err(NoResource::NoFreeResourceAndMeteredWithheld {
                    withheld: self.metered.clone(),
                })
            }
            None => Err(NoResource::ProtectedReserveDenied {
                reasons: denied_reasons,
            }),
        }
    }

    /// Attach every entitlement-refusal note to `explanation` and build the
    /// [`DisposableChoice`] — the sequence all three surviving arms of
    /// [`Self::choose`] repeated verbatim before this extraction.
    fn finish(
        &self,
        job: JobKind,
        candidate: &DisposableCandidate,
        reason: UseReason,
        mut explanation: RoutingExplanation,
        refusal_notes: &[Contribution],
    ) -> DisposableChoice {
        for note in refusal_notes.iter().cloned() {
            explanation.push(note);
        }
        self.choice(job, candidate, reason, explanation)
    }

    /// Map lines 1441 and 1442, for the one caller they name —
    /// `automatic_classification_choice`'s `glasshouse classify` decision —
    /// and deliberately not folded into [`Self::choose`] itself.
    ///
    /// # Why this is a second function rather than a flag on `choose`
    ///
    /// This module's own header states the separation as a design principle:
    /// the disposable policy "prefers free capacity and re-decides every
    /// time", against the interactive policy's "keeps what it has". Giving
    /// `choose` a retained pick to consult would blur that for every
    /// [`JobKind`] it serves — memory extraction, reranking, evaluation —
    /// none of which map lines 1441/1442 name. Stickiness here is scoped to
    /// automatic classification alone, so it stands beside `choose`, calling
    /// it unchanged, rather than reaching inside it.
    ///
    /// # Purity is preserved the same way `choose` preserves it
    ///
    /// `tests::no_routing_policy_can_make_a_request` (`routing/mod.rs`) holds
    /// this module to reading no telemetry itself. This function does not
    /// break that: `retained` is supplied by the caller, exactly as
    /// `candidates` and `pool` already are — nothing here opens a cache or a
    /// connection. The caller is expected to be
    /// `crate::provider::telemetry::RoutingStickyCache::load`, and to persist
    /// the returned pick with `RoutingStickyCache::store` on the
    /// [`AutomaticClassificationDecision::Fresh`] arm.
    ///
    /// # The honesty invariant (map line 1441)
    ///
    /// A retained pick is returned **only** when all three hold: it is still
    /// inside [`AUTOMATIC_CLASSIFICATION_STICKY_WINDOW_SECONDS`], it names a
    /// candidate still present in `candidates`, and that candidate is a free
    /// resource `pool` still reports available. A pick that fails any of
    /// these gets a fresh call to [`Self::choose`] instead — stickiness never
    /// outlives the healthiness it was predicated on. A **metered** retained
    /// pick always falls through to a fresh decision too: `pool` is the only
    /// health signal this build has for a free resource
    /// (`docs/product/evidence/phase-34c.md`'s 1433 entry: "the health pool
    /// reaches only free candidates"), so there is nothing honest to check a
    /// metered pick's continued health against, and inventing one would
    /// repeat the same fabrication line 1434's elimination step refuses.
    ///
    /// # The retained arm's explanation is not `score`'s output
    ///
    /// No ranking runs when a pick is retained, so the [`DisposableChoice`]
    /// built here carries a [`RoutingExplanation`] that says exactly that —
    /// reused without re-ranking, and its age — rather than a synthesised
    /// comparison that never happened. See [`DisposableChoice`]'s
    /// `explanation` field doc.
    pub fn choose_for_automatic_classification(
        &self,
        candidates: &[DisposableCandidate],
        pool: &FreePool,
        now: Instant,
        now_unix: i64,
        classification: Option<&TaskClassification>,
        retained: Option<RetainedPick>,
    ) -> Result<AutomaticClassificationDecision, NoResource> {
        // Capability map lines 1427, 1432 and 1435, before anything else:
        // the requirements a candidate must meet are applied here, once, to
        // the whole list — so a retained pick that no longer meets them is
        // simply not "still present" below and gets a fresh decision, the
        // same way a pick whose health turned is handled. `choose` itself
        // is untouched: these requirements are about *classification*, and
        // `choose` serves every `JobKind`.
        let mut admitted: Vec<(DisposableCandidate, Vec<Contribution>)> = Vec::new();
        let mut exclusions: Vec<Contribution> = Vec::new();
        for candidate in candidates {
            match classification_verdict(&self.classification_policy, candidate) {
                ClassificationVerdict::Admitted { notes } => {
                    admitted.push((candidate.clone(), notes));
                }
                ClassificationVerdict::Excluded { reason } => exclusions.push(Contribution::new(
                    "excluded candidate",
                    0.0,
                    format!(
                        "{} on {}: {reason}",
                        candidate.model(),
                        candidate.provider()
                    ),
                )),
            }
        }
        if admitted.is_empty() && !candidates.is_empty() {
            return Err(NoResource::ClassificationRequirementsExcludedAll {
                reasons: exclusions
                    .iter()
                    .map(|exclusion| exclusion.evidence().to_owned())
                    .collect(),
            });
        }

        // Capability map lines 1420, 1421, 1438 and 1419: among the
        // candidates the user has *not* placed in an explicit free-resource
        // order, the classification preferences decide the order `choose`'s
        // free loop walks. A stable sort on the preference total, so two
        // candidates nothing is known about keep the caller's order exactly
        // as before — and `FreePreferences::arrange` re-sorts by the user's
        // own order afterwards, so a ranked candidate is never moved by
        // this. The scoring invariant `choose` documents therefore still
        // holds: it consults no score to pick a free winner; the order it is
        // handed is what changed.
        //
        // `notes` — the same `Vec<Contribution>` `classification_verdict`
        // just built for this candidate — is summed in alongside
        // `classification_preferences` rather than left for the explanation
        // alone: every requirement note in it is a fixed `0.0` except the
        // 1419 *protected capacity* term, which is the one note in that list
        // ruled to carry a real magnitude (`design-decisions.md`, *"The
        // premium capacity a classifier protects"* — *"this line only
        // orders"*). Summing the whole `notes` list rather than picking that
        // term out by name costs nothing today (every sibling note is
        // `0.0`) and asks nothing of a future note beyond the same
        // convention.
        admitted.sort_by(|(left, left_notes), (right, right_notes)| {
            let of = |candidate: &DisposableCandidate, notes: &[Contribution]| {
                self.classification_preferences(candidate)
                    .iter()
                    .chain(notes)
                    .map(Contribution::magnitude)
                    .sum::<f64>()
            };
            of(right, right_notes)
                .partial_cmp(&of(left, left_notes))
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        let candidates: Vec<DisposableCandidate> = admitted
            .iter()
            .map(|(candidate, _)| candidate.clone())
            .collect();
        let candidates = candidates.as_slice();
        let notes_for = |choice: &DisposableChoice| -> Vec<Contribution> {
            admitted
                .iter()
                .find(|(candidate, _)| {
                    candidate.provider() == choice.provider() && candidate.model() == choice.model()
                })
                .map(|(_, notes)| notes.clone())
                .unwrap_or_default()
        };

        // Map line 1439, ahead of the retained-pick reuse below
        // (design-decisions.md, "Preferring a cheap metered classifier over
        // an unreliable free one", amended 2026-09-02): ask whether the free
        // candidate this policy would otherwise prefer — from this same
        // classification-admitted `candidates` list, so what it prefers has
        // already passed every other gate — is unreliable enough relative to
        // the cheapest admitted metered candidate's own measured latency,
        // and that metered candidate cheap enough, to switch. Evaluated
        // before the retained-pick check so that a *retained* free pick
        // whose inputs now fire this rule is overridden rather than
        // silently reused — map lines 1441/1442 are about health, not about
        // this preference, and a pick this rule would no longer make is not
        // "still healthy" in the sense that matters here.
        let time_price = self.time_price_seam(candidates, pool, now, classification);
        if let Some((contribution, Some(metered))) = &time_price {
            let mut explanation = RoutingExplanation::new();
            explanation.push(contribution.clone());
            let mut choice = self.choice(
                JobKind::Classification,
                metered,
                UseReason::Fallback,
                explanation,
            );
            for note in notes_for(&choice).into_iter().chain(exclusions.clone()) {
                choice.explanation.push(note);
            }
            let pick = RetainedPick {
                provider: choice.provider().to_owned(),
                model: choice.model().to_owned(),
                chosen_at_unix: now_unix,
            };
            return Ok(AutomaticClassificationDecision::Fresh(choice, pick));
        }
        // Inert (or never asked): carried forward and attached to whichever
        // choice — retained or freshly ranked — is made below, so the
        // explanation names the condition that kept this preference from
        // firing exactly as it would if it had.
        let inert_time_price_note = time_price.map(|(contribution, _)| contribution);

        if let Some(pick) = &retained {
            let age = now_unix.saturating_sub(pick.chosen_at_unix);
            let within_window = (0..AUTOMATIC_CLASSIFICATION_STICKY_WINDOW_SECONDS).contains(&age);
            let still_present = candidates.iter().find(|candidate| {
                candidate.provider() == pick.provider && candidate.model() == pick.model
            });
            let still_healthy = still_present.is_some_and(|candidate| {
                candidate.cost().is_free() && pool.is_available(&candidate.as_free_resource(), now)
            });
            if within_window && still_healthy {
                let candidate =
                    still_present.expect("still_healthy is true only when still_present is Some");
                let reason = if self.prefer_free_setting {
                    UseReason::UserPreference
                } else {
                    UseReason::QuotaPreservation
                };
                let mut explanation = RoutingExplanation::new();
                explanation.push(Contribution::new(
                    "retained pick",
                    0.0,
                    format!(
                        "reused without re-ranking: chosen {age}s ago, inside the \
                         {AUTOMATIC_CLASSIFICATION_STICKY_WINDOW_SECONDS}s sticky window (map \
                         line 1442); DisposableRouting::score did not run for this decision"
                    ),
                ));
                if let Some(contribution) = &inert_time_price_note {
                    explanation.push(contribution.clone());
                }
                let mut choice =
                    self.choice(JobKind::Classification, candidate, reason, explanation);
                for contribution in notes_for(&choice).into_iter().chain(exclusions) {
                    choice.explanation.push(contribution);
                }
                return Ok(AutomaticClassificationDecision::Retained(choice));
            }
        }

        let mut choice = self.choose(
            JobKind::Classification,
            candidates,
            pool,
            now,
            classification,
        )?;
        if let Some(contribution) = inert_time_price_note {
            choice.explanation.push(contribution);
        }
        for contribution in notes_for(&choice).into_iter().chain(exclusions) {
            choice.explanation.push(contribution);
        }
        let pick = RetainedPick {
            provider: choice.provider().to_owned(),
            model: choice.model().to_owned(),
            chosen_at_unix: now_unix,
        };
        Ok(AutomaticClassificationDecision::Fresh(choice, pick))
    }

    /// Map line 1439's seam, amended 2026-09-02: evaluate
    /// [`time_price_preference`] against whichever free candidate this
    /// policy would pick from `admitted_candidates` — the
    /// classification-admitted list
    /// [`Self::choose_for_automatic_classification`] already built, so what
    /// this preference prefers has passed every other gate — and the
    /// cheapest priced metered candidate [`cheapest_priced_metered`] finds
    /// in that same admitted list.
    ///
    /// `None` when there is nothing to ask: no free candidate would be
    /// picked at all (`Self::choose` on the admitted list errored, or its
    /// winner is metered already), or no metered candidate is priced and
    /// permitted.
    fn time_price_seam<'a>(
        &self,
        admitted_candidates: &'a [DisposableCandidate],
        pool: &FreePool,
        now: Instant,
        classification: Option<&TaskClassification>,
    ) -> Option<(Contribution, Option<&'a DisposableCandidate>)> {
        let natural = self
            .choose(
                JobKind::Classification,
                admitted_candidates,
                pool,
                now,
                classification,
            )
            .ok()?;
        if !natural.cost().is_free() {
            return None;
        }
        let free = admitted_candidates.iter().find(|candidate| {
            candidate.provider() == natural.provider() && candidate.model() == natural.model()
        })?;
        let metered = cheapest_priced_metered(admitted_candidates, &self.metered)?;
        match time_price_preference(&self.classification_policy, free, metered) {
            TimePricePreference::Fires(contribution) => Some((contribution, Some(metered))),
            TimePricePreference::Inert(contribution) => Some((contribution, None)),
        }
    }

    /// Score one eligible candidate — map line 1530: every input this policy
    /// can honestly evaluate today, named and signed, never blended into one
    /// opaque number (line 1553).
    ///
    /// `order_position`/`order_len` describe this candidate's place in the
    /// user's own free-resource order (`None`/`0` for a candidate the order
    /// does not rank — a metered fallback candidate, or a pinned one).
    /// `reserve` is `Some` only on the metered-fallback path, where Phase
    /// 32F's policy already ran as a hard gate before this function was even
    /// called (line 1550); what lands here is only its reason, for the
    /// record — an allow/deny gate is not itself a magnitude, so it is not
    /// double-counted as one.
    fn score(
        &self,
        candidate: &EligibleCandidate<DisposableCandidate>,
        order_position: Option<usize>,
        order_len: usize,
        reserve: Option<&ReserveDecision>,
    ) -> RoutingExplanation {
        let value = candidate.value();
        let mut explanation = RoutingExplanation::new();

        explanation.push(Contribution::new(
            "cost",
            if value.cost().is_free() { 1.0 } else { 0.0 },
            format!(
                "{} — line 530 prefers free capacity for disposable support work",
                value.cost().as_str()
            ),
        ));

        explanation.push(match (order_position, order_len) {
            (Some(position), len) if len > 0 => Contribution::new(
                "user free-resource order",
                (len - position) as f64 / len as f64 * 0.5,
                format!(
                    "position {} of {len} in the user's configured free-resource order (lines \
                     536, 1552)",
                    position + 1
                ),
            ),
            _ => Contribution::new(
                "user free-resource order",
                0.0,
                "not ranked by an explicit user order — no order is configured, or this is a \
                 metered fallback candidate the order does not cover"
                    .to_owned(),
            ),
        });

        explanation.push(match &value.capacity.remaining_capacity {
            Some(score) => Contribution::new(
                "normalized remaining capacity",
                score.routing_fraction(),
                format!(
                    "{} remaining, bound by {} (map line 1536)",
                    score.percent().render(),
                    score.dimension()
                ),
            ),
            None => Contribution::new(
                "normalized remaining capacity",
                0.0,
                "no capacity telemetry cached for this provider (map line 1536)".to_owned(),
            ),
        });

        explanation.push(match value.capacity.seconds_until_reset {
            Some(seconds) => {
                let boost = value
                    .capacity
                    .remaining_capacity
                    .as_ref()
                    .map(|score| score.effective(Some(seconds)) - score.routing_fraction())
                    .unwrap_or(0.0);
                Contribution::new(
                    "time until quota reset",
                    boost,
                    format!("resets in {seconds}s (map line 1549)"),
                )
            }
            None => Contribution::new(
                "time until quota reset",
                0.0,
                "no reset time known for this provider (map line 1549)".to_owned(),
            ),
        });

        for contribution in self.classification_preferences(value) {
            explanation.push(contribution);
        }

        // Capability map line 1539, beside classification latency's own
        // term rather than folded into `classification_preferences`: that
        // list is summed by `choose_for_automatic_classification` to order
        // the *free* candidates the user has not ranked (map lines 1420,
        // 1421, 1438), and this term must never join that sum — support-work
        // latency ranks the metered fallback and informs the winner's
        // explanation, exactly as design decisions for lines 1537/1538
        // record, and the free selection stays decided by the user's own
        // order and availability alone (`scoring_never_reorders_the_existing_free_selection`).
        explanation.push(
            match value
                .latency
                .as_ref()
                .and_then(|record| record.median_duration_ms)
            {
                Some(median) => {
                    let timed = value.latency.as_ref().map_or(0, |record| record.timed);
                    Contribution::new(
                        "expected latency",
                        LATENCY_PREFERENCE_WEIGHT / (1.0 + median as f64 / 1000.0),
                        format!(
                            "median {median}ms over {timed} timed support-work calls — lower \
                             is preferred, at classification latency's own weight (map line \
                             1539)"
                        ),
                    )
                }
                None => {
                    let timed = value.latency.as_ref().map_or(0, |record| record.timed);
                    Contribution::new(
                        "expected latency",
                        0.0,
                        format!(
                            "no latency figure yet ({timed} of {MIN_SAMPLE_FOR_SUMMARY} timed \
                             support-work calls) — this preference is inert (map line 1539)"
                        ),
                    )
                }
            },
        );

        if let (Some(session), true) = (self.reserve_override.granted_session(), reserve.is_some())
        {
            explanation.push(Contribution::new(
                "user reserve override",
                0.0,
                format!(
                    "the user overrode reserve protection for session {session}; protected \
                     reserve may be spent for this session's work and no other (map line 1290)"
                ),
            ));
        }

        if let Some(decision) = reserve {
            explanation.push(Contribution::new(
                "protected-reserve policy",
                0.0,
                format!(
                    "{} (map line 1550): {}",
                    if decision.is_allowed() {
                        "allowed"
                    } else {
                        "denied"
                    },
                    decision.reason()
                ),
            ));
            explanation.push(self.background_reserve_policy_note(value, decision));
        }

        explanation
    }

    /// What the user's background reserve policy did to the decision above —
    /// capability map line 1577, on the one path that can reach it.
    ///
    /// Always rendered beside a reserve decision rather than only when the
    /// policy changed the answer, so a reader of a support job's rationale
    /// can see *which* of the two configured policies was consulted. A line
    /// that appeared only on the overriding case would leave the ordinary
    /// case looking as though no scope had been chosen at all.
    ///
    /// The band is reported as it was read, and "unread" is said rather than
    /// filled in: [`Self::choose`]'s gate substitutes
    /// [`CapacityBand::Plenty`] for an absent reading because
    /// `evaluate_reserve_spend` needs a band, and printing that substitution
    /// here as though it were an observation is the fabrication this module
    /// refuses everywhere else.
    ///
    /// The denied-and-`Protect` arm is unreachable from [`Self::choose`],
    /// which drops such a candidate before scoring it. It is written out
    /// rather than collapsed into an `unreachable!` because the pair is
    /// total and a future caller that scores without gating should get a
    /// true sentence rather than a panic.
    fn background_reserve_policy_note(
        &self,
        value: &DisposableCandidate,
        decision: &ReserveDecision,
    ) -> Contribution {
        let band = match value.capacity.band {
            Some(band) => format!("in the {band} band"),
            None => "with no band reading".to_owned(),
        };
        let effect = match (decision.is_allowed(), self.reserve_policy) {
            (false, ReservePolicy::Spend) => format!(
                "admitted this candidate {band} anyway — the policy removes the denial, not the \
                 pressure, so the reason above still stands as the reading it was"
            ),
            (false, ReservePolicy::Protect) => {
                format!("leaves the denial above standing for this candidate {band}")
            }
            (true, _) => format!(
                "did not have to act: the reserve decision above allowed this candidate {band} \
                 on its own signals"
            ),
        };
        Contribution::new(
            format!("{} reserve policy", ReserveScope::Background),
            0.0,
            format!(
                "`{}` is the reserve policy configured for {} work, and it {effect} (map line \
                 1577)",
                self.reserve_policy,
                ReserveScope::Background,
            ),
        )
    }

    /// The four classification preferences — capability map lines 1421
    /// (latency), 1422 (structured-output reliability), 1420
    /// (requests-per-minute headroom) and 1438 (locality) — as named
    /// contributions, each inert and saying so when its quantity is not
    /// measured.
    ///
    /// One definition, two consumers: [`Self::score`] renders them on every
    /// explanation (and their magnitudes rank the metered-fallback path),
    /// and [`Self::choose_for_automatic_classification`] sums them to order
    /// the free candidates the user has not ranked. Splitting the two would
    /// let the explanation say one thing and the order do another.
    fn classification_preferences(&self, candidate: &DisposableCandidate) -> Vec<Contribution> {
        let mut out = Vec::with_capacity(4);

        let timed = candidate
            .classification
            .as_ref()
            .map_or(0, |record| record.timed);
        out.push(
            match candidate
                .classification
                .as_ref()
                .and_then(|record| record.median_duration_ms)
            {
                Some(median) => Contribution::new(
                    "classification latency",
                    CLASSIFICATION_PREFERENCE_WEIGHT / (1.0 + median as f64 / 1000.0),
                    format!(
                        "median {median}ms over {timed} timed classification calls — lower is \
                         preferred so routing does not make a person's turn feel slower than \
                         the harness alone (map line 1421)"
                    ),
                ),
                None => Contribution::new(
                    "classification latency",
                    0.0,
                    format!(
                        "no latency figure yet ({timed} of {MIN_SAMPLE_FOR_SUMMARY} timed \
                         classification calls) — this preference is inert (map line 1421)"
                    ),
                ),
            },
        );

        out.push(match candidate.classification.as_ref() {
            Some(record)
                if record.outcomes_recorded >= CLASSIFICATION_RELIABILITY_MIN_OBSERVATIONS =>
            {
                let fraction = record
                    .parsed_fraction()
                    .expect("outcomes_recorded is at least the minimum, so above zero");
                Contribution::new(
                    "structured-output reliability",
                    fraction * CLASSIFICATION_PREFERENCE_WEIGHT,
                    format!(
                        "{} of {} classification calls came back in the schema ({:.0}%) — more \
                         reliable is preferred (map line 1422)",
                        record.parsed,
                        record.outcomes_recorded,
                        fraction * 100.0
                    ),
                )
            }
            other => Contribution::new(
                "structured-output reliability",
                0.0,
                format!(
                    "no reliability figure yet ({} of {} outcome-carrying classification calls) \
                     — this preference is inert (map line 1422)",
                    other.map_or(0, |record| record.outcomes_recorded),
                    CLASSIFICATION_RELIABILITY_MIN_OBSERVATIONS
                ),
            ),
        });

        out.push(match &candidate.capacity.remaining_capacity {
            Some(score) if score.dimension() == REQUESTS_PER_MINUTE_DIMENSION => Contribution::new(
                "requests-per-minute headroom",
                score.routing_fraction() * CLASSIFICATION_PREFERENCE_WEIGHT,
                format!(
                    "{} of the per-minute request ceiling remains — more headroom is \
                         preferred so routing does not become the scheduler's bottleneck (map \
                         line 1420)",
                    score.percent().render()
                ),
            ),
            Some(score) => Contribution::new(
                "requests-per-minute headroom",
                0.0,
                format!(
                    "this candidate is bound by {} rather than by requests per minute — this \
                     preference is inert (map line 1420)",
                    score.dimension()
                ),
            ),
            None => Contribution::new(
                "requests-per-minute headroom",
                0.0,
                "no requests-per-minute reading for this provider — this preference is inert \
                 (map line 1420)"
                    .to_owned(),
            ),
        });

        out.push(match candidate.locality {
            Some(Locality::Local) => Contribution::new(
                "locality",
                CLASSIFICATION_PREFERENCE_WEIGHT,
                "local inference — preferred among candidates that met every requirement \
                 applied before ranking (map line 1438)"
                    .to_owned(),
            ),
            Some(Locality::Remote) => {
                Contribution::new("locality", 0.0, "remote (map line 1438)".to_owned())
            }
            None => Contribution::new(
                "locality",
                0.0,
                "locality not stated by the caller — this preference is inert (map line 1438)"
                    .to_owned(),
            ),
        });

        out
    }

    /// The pinned candidate's explanation: everything [`DisposableRouting::score`]
    /// says, plus the pin itself (map line 1552's other half). A pin is not
    /// scored as a magnitude — the same design decision Phase 9J's
    /// `PairingPreference::Pin` already made — it is reported as the reason
    /// ranking never ran at all.
    fn score_pinned(
        &self,
        candidate: &EligibleCandidate<DisposableCandidate>,
    ) -> RoutingExplanation {
        let mut explanation = self.score(candidate, None, 0, None);
        explanation.push(Contribution::new(
            "user pin",
            0.0,
            "the user pinned this exact free resource; pinning overrides ranking entirely (map \
             line 1552)"
                .to_owned(),
        ));
        explanation
    }

    fn choice(
        &self,
        job: JobKind,
        candidate: &DisposableCandidate,
        reason: UseReason,
        explanation: RoutingExplanation,
    ) -> DisposableChoice {
        DisposableChoice {
            job,
            provider: candidate.provider().to_owned(),
            model: candidate.model().to_owned(),
            credential: candidate.credential().clone(),
            cost: candidate.cost(),
            reason,
            explanation,
        }
    }
}

/// Whether a resource *other than* `metered[index]` could serve this job
/// without spending anybody's protected reserve — capability map line 1288's
/// input to [`evaluate_reserve_spend`], and the only way that line's own
/// branch is reachable from production.
///
/// # "Cheaper" is read here, not invented
///
/// The question sounds like it needs a price list, and Glasshouse has none:
/// [`Cost`] knows only free-or-metered and never compares two metered models
/// against each other. But
/// [`ReserveDecisionInputs::cheaper_adequate_resource_exists`] states its own
/// meaning, in the phase that owns the policy — *"whether a resource outside
/// the reserve band could adequately serve this task instead"*. So "cheaper"
/// is already denominated in the currency this policy protects, which is
/// reserve capacity rather than money, and [`CapacityBand`] is [`Ord`] with
/// [`CapacityBand::Exhausted`] lowest precisely so a policy can ask that as a
/// comparison. Reading that definition is the whole of this function.
///
/// # An unknown band is not a cheaper resource
///
/// Only a candidate whose band has actually been *read* counts. A metered
/// resource nothing has been observed about may be deep in its own protected
/// reserve for all Glasshouse knows, and denying a spend on the strength of
/// it would invent exactly the judgement this input exists to avoid.
/// [`CandidateCapacity::band`]'s own `None` already refuses to withhold a
/// resource by a band never observed; this is the same refusal pointed the
/// other way, at the resource being offered as an alternative.
///
/// # Free candidates are not consulted, and that is not an omission
///
/// Reaching [`DisposableRouting::choose`]'s metered loop has already proved
/// no free resource can serve: that loop returns on the first available one,
/// and [`FreePreferences::arrange`] has already dropped the ones the user
/// disabled. A resource that cannot serve now is not one that "could
/// adequately serve this task instead".
///
/// # What "adequately" leans on
///
/// This module has no per-candidate capability model, and does not acquire
/// one here: every eligible candidate is a model the user configured for this
/// provider that survived [`apply_hard_constraints`]. Treating those as
/// interchangeable for a bounded internal job is the assumption the free loop
/// above already ships — it returns the first *available* candidate in the
/// user's own order, never the most capable one — and this function inherits
/// it rather than introducing it.
fn cheaper_adequate_resource_exists(
    metered: &[&EligibleCandidate<DisposableCandidate>],
    index: usize,
) -> bool {
    metered
        .iter()
        .enumerate()
        .any(|(other, candidate)| other != index && candidate.value().is_outside_reserve())
}
