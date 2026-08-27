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

use std::time::Instant;

use super::free::{FreePool, FreePreferences, FreeResource, FreeResourceKey};
use super::{
    Contribution, Cost, CredentialId, EligibleCandidate, HardConstraint, RoutingExplanation,
    UseReason, apply_hard_constraints,
};
use crate::provider::quota::{
    CapacityBand, RemainingCapacityScore, ReserveDecision, ReserveDecisionInputs,
    evaluate_reserve_spend,
};
use crate::routing::classify::WorkloadTier;

/// The kind of bounded internal work a choice is being made for.
///
/// Carried so that a chosen resource can be recorded against the job that
/// used it — Phase 39's "record which resource performed important memory
/// extraction or classification for debugging" needs the pair, and a job
/// kind is a name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobKind {
    Classification,
    MemoryExtraction,
    Reranking,
    /// Glasshouse's own automated evaluation or test run.
    Evaluation,
}

impl JobKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Classification => "classification",
            Self::MemoryExtraction => "memory extraction",
            Self::Reranking => "reranking",
            Self::Evaluation => "evaluation",
        }
    }
}

impl std::fmt::Display for JobKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.pad(self.as_str())
    }
}

/// Whether this policy may spend metered capacity at all.
///
/// Three states, because two would not distinguish the two ways a policy can
/// be allowed to spend: ordinary support work may fall back to a metered
/// resource when no free one can serve, whereas Glasshouse's own runs may do
/// so only after somebody said so by name. Collapsing them would make
/// line 539's "explicit opt-in" indistinguishable from a default.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MeteredUse {
    /// Ordinary support work: a metered resource is a legitimate last resort.
    Permitted,
    /// Withheld. Nothing metered will be chosen, and a job with no free
    /// resource available fails instead.
    Withheld,
    /// Withheld by default, and then given. `by` names what gave it, so a
    /// later reader can find the switch that was thrown.
    OptedIn { by: &'static str },
}

impl MeteredUse {
    /// The environment variable an automated Glasshouse run opts in through.
    ///
    /// One name, spelled once. A second spelling is how "never without an
    /// explicit opt-in" becomes "unless you set the other one".
    pub const OPT_IN_VAR: &'static str = "GLASSHOUSE_ALLOW_METERED_MODELS";

    /// Read the opt-in for an automated run, defaulting to
    /// [`MeteredUse::Withheld`].
    ///
    /// `read` is injected rather than calling [`std::env::var`] here: this
    /// module is pure by rule (see [`mod@super`]), and a test that had to set
    /// a process-wide environment variable to check the default would be a
    /// test that raced every other test in the binary.
    ///
    /// Anything other than exactly `1` leaves it withheld. Not
    /// case-insensitive `true`, not "any non-empty value": the fail-closed
    /// direction, where a stray value spends nothing.
    pub fn for_automated_run(read: impl Fn(&str) -> Option<String>) -> Self {
        match read(Self::OPT_IN_VAR).as_deref() {
            Some("1") => Self::OptedIn {
                by: "GLASSHOUSE_ALLOW_METERED_MODELS=1",
            },
            _ => Self::Withheld,
        }
    }

    pub fn permits_metered(&self) -> bool {
        matches!(self, Self::Permitted | Self::OptedIn { .. })
    }

    pub fn describe(&self) -> String {
        match self {
            Self::Permitted => "metered resources may be used".to_owned(),
            Self::Withheld => "metered resources are withheld".to_owned(),
            Self::OptedIn { by } => format!("metered resources were opted in through {by}"),
        }
    }
}

/// What a caller may know about one candidate's live capacity, beyond the
/// static configuration [`DisposableCandidate`] itself carries.
///
/// Every field is `None` (or `Plenty`, capacity's most permissive band) until
/// a caller supplies a real reading — [`mod@super`]'s "every function is a
/// pure function of values the caller supplies" applies here too: this
/// module reads no telemetry itself, and none of it opens a connection to
/// get one (`tests::no_routing_policy_can_make_a_request` in `mod.rs` would
/// catch it if it tried). `main.rs` is the caller that has a real
/// [`crate::provider::telemetry::GatewayQuotaCache`] to read from, the same
/// one `glasshouse resources` already reads.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CandidateCapacity {
    /// Map line 1536: this candidate's own normalized remaining-capacity
    /// score, when real telemetry has been cached for its provider.
    remaining_capacity: Option<RemainingCapacityScore>,
    /// Map line 1549: seconds until this candidate's provider quota resets,
    /// when a real reading has stated one.
    seconds_until_reset: Option<i64>,
    /// This candidate's capacity band against the user's own thresholds —
    /// feeds Phase 32F's protected-reserve policy on the metered-fallback
    /// path (map line 1550). `None` (treated as [`CapacityBand::Plenty`],
    /// the least protective band) for a resource nothing has been read
    /// about: an unread resource is not withheld from support work by a
    /// band it has never been observed to cross.
    band: Option<CapacityBand>,
}

impl CandidateCapacity {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_remaining_capacity(mut self, score: Option<RemainingCapacityScore>) -> Self {
        self.remaining_capacity = score;
        self
    }

    pub fn with_seconds_until_reset(mut self, seconds: Option<i64>) -> Self {
        self.seconds_until_reset = seconds;
        self
    }

    pub fn with_band(mut self, band: Option<CapacityBand>) -> Self {
        self.band = band;
        self
    }
}

/// One resource a disposable job could be sent to.
///
/// Deliberately not a `super::Backend`: a backend carries a wire protocol and
/// tool semantics because an interactive session's harness depends on both,
/// and a disposable job has neither a harness nor tools. Sharing the type
/// would invite sharing the policy.
#[derive(Debug, Clone, PartialEq)]
pub struct DisposableCandidate {
    provider: String,
    model: String,
    credential: CredentialId,
    cost: Cost,
    /// Real capacity data the caller supplied for this candidate — see
    /// [`CandidateCapacity`]. Defaults to nothing known, which
    /// [`DisposableRouting::score`] renders as an honest `0.0` contribution
    /// naming the missing source, per this packet's design decision 3.
    capacity: CandidateCapacity,
}

impl DisposableCandidate {
    pub fn new(
        provider: impl Into<String>,
        model: impl Into<String>,
        credential: CredentialId,
        cost: Cost,
    ) -> Self {
        Self {
            provider: provider.into(),
            model: model.into(),
            credential,
            cost,
            capacity: CandidateCapacity::default(),
        }
    }

    /// Attach real capacity data a caller has read for this candidate — map
    /// lines 1536, 1549 and 1550.
    pub fn with_capacity(mut self, capacity: CandidateCapacity) -> Self {
        self.capacity = capacity;
        self
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

    fn as_free_resource(&self) -> FreeResource {
        FreeResource::new(self.credential.clone(), self.model.clone())
    }

    fn key(&self) -> FreeResourceKey {
        FreeResourceKey::new(self.provider.clone(), self.model.clone())
    }
}

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
        }
    }
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
}

impl DisposableRouting {
    /// Ordinary support work: prefer free, fall back to metered when nothing
    /// free can serve.
    pub fn for_support_work(prefer_free_setting: bool, preferences: FreePreferences) -> Self {
        Self {
            metered: MeteredUse::Permitted,
            prefer_free_setting,
            preferences,
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
        }
    }

    pub fn metered_use(&self) -> &MeteredUse {
        &self.metered
    }

    pub fn preferences(&self) -> &FreePreferences {
        &self.preferences
    }

    /// Choose a resource for one bounded job.
    ///
    /// # The order, and where each step comes from
    ///
    /// 1. **Hard constraints first, structurally** (map line 1553):
    ///    [`apply_hard_constraints`] removes any candidate this policy could
    ///    never use — today that is exactly the metered candidates
    ///    [`MeteredUse`] withholds, named [`HardConstraint::UserConstraint`]
    ///    because line 568 calls a user's own opt-in rule exactly that. A
    ///    candidate that fails this is unrepresentable to the scorer below,
    ///    not merely given a large negative weight.
    /// 2. **A pinned free resource wins outright** (line 536, 1552). If it
    ///    cannot serve, the job fails rather than silently going elsewhere —
    ///    a pin is a hard rule, never a scored preference, the same design
    ///    decision Phase 9J's `PairingPreference::Pin` already made.
    /// 3. **Free resources, in the user's own order**, skipping disabled ones
    ///    (line 536) and any whose health or allowance says it cannot serve
    ///    right now (lines 529, 535, 538). This is line 530's "prefer free
    ///    models for bounded Glasshouse support work", and line 531 falls out
    ///    of it: a model is in this list because the user marked it free, so
    ///    an explicitly configured free model such as a Nemotron variant
    ///    participates without this function knowing any model's name.
    /// 4. **A metered resource**, only when [`MeteredUse`] permits it
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
    pub fn choose(
        &self,
        job: JobKind,
        candidates: &[DisposableCandidate],
        pool: &FreePool,
        now: Instant,
    ) -> Result<DisposableChoice, NoResource> {
        if candidates.is_empty() {
            return Err(NoResource::NothingConfigured);
        }

        let (eligible, _rejected) = apply_hard_constraints(candidates.to_vec(), |candidate| {
            if candidate.cost().is_free() || self.metered.permits_metered() {
                Ok(())
            } else {
                Err(HardConstraint::UserConstraint)
            }
        });

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
                    Ok(self.choice(
                        job,
                        candidate.value(),
                        UseReason::UserPreference,
                        explanation,
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
                return Ok(self.choice(job, candidate.value(), reason, explanation));
            }
        }

        // `eligible` already holds no metered candidate when `MeteredUse`
        // withholds them — the hard constraint above removed it, not this
        // loop — so an empty metered set here means either nothing metered
        // was ever configured or every one of them was withheld; both read
        // the same to a caller (line 539's refusal either way).
        //
        // Every free resource is gone or absent by this point — Phase 32F's
        // own "cheaper adequate resource" question is answered `false` for
        // whichever metered candidate is considered next, because reaching
        // this line already proved there was none.
        let mut denied_reasons = Vec::new();
        let mut best: Option<(
            &EligibleCandidate<DisposableCandidate>,
            RoutingExplanation,
            f64,
        )> = None;
        for candidate in eligible.iter().filter(|c| !c.value().cost().is_free()) {
            let decision = evaluate_reserve_spend(ReserveDecisionInputs {
                band: candidate
                    .value()
                    .capacity
                    .band
                    .unwrap_or(CapacityBand::Plenty),
                tier: WorkloadTier::Leaf,
                cheaper_adequate_resource_exists: false,
                user_override: false,
                seconds_until_reset: candidate.value().capacity.seconds_until_reset,
                task_nearly_complete: false,
            });
            if !decision.is_allowed() {
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
            Some((candidate, explanation, _)) => {
                Ok(self.choice(job, candidate.value(), UseReason::Fallback, explanation))
            }
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
        }

        explanation
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::routing::free::WorkloadOutcome;
    use crate::secret::SecretRef;
    use std::time::Duration;

    fn credential(provider: &str) -> CredentialId {
        CredentialId::new(
            provider,
            SecretRef::Environment {
                var: format!("{}_API_KEY", provider.to_uppercase()),
            },
        )
    }

    fn free(provider: &str, model: &str) -> DisposableCandidate {
        DisposableCandidate::new(provider, model, credential(provider), Cost::Free)
    }

    fn metered(provider: &str, model: &str) -> DisposableCandidate {
        DisposableCandidate::new(provider, model, credential(provider), Cost::Metered)
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

    /// Line 533's type-level half, checked rather than asserted in a comment:
    /// a disposable choice offers no way to become an interactive assignment.
    #[test]
    fn a_disposable_choice_cannot_become_an_interactive_assignment() {
        let code = production_code(include_str!("disposable.rs"));
        for forbidden in ["Assignment", "InteractiveRouting", "TurnRouting"] {
            assert!(
                !code.contains(forbidden),
                "routing/disposable.rs names `{forbidden}`: the two policy classes Phase 9I \
                 line 533 requires to stay separate have started to share types"
            );
        }
    }

    /// Line 530, and line 531 with it: a user-marked free model is preferred
    /// for support work over a metered one.
    #[test]
    fn support_work_prefers_a_free_model_over_a_metered_one() {
        let routing = DisposableRouting::for_support_work(false, FreePreferences::new());
        let choice = routing
            .choose(
                JobKind::MemoryExtraction,
                &[
                    metered("openrouter", "an-expensive-model"),
                    free("openrouter", "nvidia/nemotron-nano-9b-v2:free"),
                ],
                &FreePool::new(),
                Instant::now(),
            )
            .expect("a free model is configured");

        assert_eq!(choice.model(), "nvidia/nemotron-nano-9b-v2:free");
        assert_eq!(choice.cost(), Cost::Free);
        assert_eq!(choice.reason(), UseReason::QuotaPreservation);
    }

    /// Line 539, the acceptance condition: an automated run finds no free
    /// resource and **fails** rather than buying one.
    #[test]
    fn glasshouses_own_run_refuses_a_metered_resource_without_an_opt_in() {
        let routing = DisposableRouting::for_glasshouses_own_run(
            MeteredUse::for_automated_run(|_| None),
            FreePreferences::new(),
        );
        let err = routing
            .choose(
                JobKind::Evaluation,
                &[metered("openrouter", "an-expensive-model")],
                &FreePool::new(),
                Instant::now(),
            )
            .expect_err("a test run must not spend the user's money");

        assert!(matches!(
            err,
            NoResource::NoFreeResourceAndMeteredWithheld { .. }
        ));
        assert!(err.to_string().contains(MeteredUse::OPT_IN_VAR));
    }

    /// And the opt-in works, so the capability is "never without an explicit
    /// opt-in" rather than "never".
    #[test]
    fn an_explicit_opt_in_lets_an_automated_run_use_a_metered_resource() {
        let routing = DisposableRouting::for_glasshouses_own_run(
            MeteredUse::for_automated_run(|var| {
                (var == MeteredUse::OPT_IN_VAR).then(|| "1".to_owned())
            }),
            FreePreferences::new(),
        );
        let choice = routing
            .choose(
                JobKind::Evaluation,
                &[metered("openrouter", "an-expensive-model")],
                &FreePool::new(),
                Instant::now(),
            )
            .expect("an explicit opt-in permits it");
        assert_eq!(choice.cost(), Cost::Metered);
    }

    /// The fail-closed reading of the opt-in: anything but `1` spends
    /// nothing.
    #[test]
    fn only_the_exact_opt_in_value_counts() {
        for value in ["", "0", "true", "yes", "TRUE", " 1"] {
            let use_ = MeteredUse::for_automated_run(|_| Some(value.to_owned()));
            assert_eq!(
                use_,
                MeteredUse::Withheld,
                "`{value}` must not be read as an opt-in"
            );
        }
    }

    /// An automated run cannot be handed ordinary support work's permission.
    #[test]
    fn an_automated_run_cannot_inherit_permitted() {
        let routing = DisposableRouting::for_glasshouses_own_run(
            MeteredUse::Permitted,
            FreePreferences::new(),
        );
        assert_eq!(routing.metered_use(), &MeteredUse::Withheld);
    }

    /// Line 540: the three reasons, produced by the policy that chose.
    #[test]
    fn a_choice_says_why_the_free_resource_is_the_one_being_used() {
        let now = Instant::now();

        let asked = DisposableRouting::for_support_work(true, FreePreferences::new())
            .choose(
                JobKind::Classification,
                &[free("openrouter", "a-free-model")],
                &FreePool::new(),
                now,
            )
            .expect("configured");
        assert_eq!(asked.reason(), UseReason::UserPreference);

        let mut pool = FreePool::new();
        let first = free("openrouter", "first-free-model");
        for _ in 0..2 {
            pool.observe(
                &FreeResource::new(first.credential().clone(), first.model()),
                WorkloadOutcome::CapacityFailure,
                now,
            );
        }
        let fell_back = DisposableRouting::for_support_work(true, FreePreferences::new())
            .choose(
                JobKind::Classification,
                &[first, free("openrouter", "second-free-model")],
                &pool,
                now,
            )
            .expect("the second free model can serve");
        assert_eq!(fell_back.model(), "second-free-model");
        assert_eq!(fell_back.reason(), UseReason::Fallback);
        assert!(fell_back.describe().contains("fallback"));
    }

    /// Line 536: a pin is not a preference to fall back from.
    #[test]
    fn a_pinned_free_resource_that_cannot_serve_fails_the_job() {
        let now = Instant::now();
        let pinned = free("openrouter", "the-pinned-model");
        let mut pool = FreePool::new();
        for _ in 0..2 {
            pool.observe(
                &FreeResource::new(pinned.credential().clone(), pinned.model()),
                WorkloadOutcome::RateLimited {
                    retry_after: Some(Duration::from_secs(300)),
                },
                now,
            );
        }

        let routing = DisposableRouting::for_support_work(
            true,
            FreePreferences::new()
                .with_pin(Some(FreeResourceKey::new("openrouter", "the-pinned-model"))),
        );
        let err = routing
            .choose(
                JobKind::Reranking,
                &[pinned, free("openrouter", "another-free-model")],
                &pool,
                now,
            )
            .expect_err("a pin does not fall back");
        assert!(matches!(err, NoResource::PinnedResourceUnavailable { .. }));
    }

    /// Line 536: a disabled resource is not chosen for any reason.
    #[test]
    fn a_disabled_free_resource_is_never_chosen() {
        let routing = DisposableRouting::for_support_work(
            true,
            FreePreferences::new()
                .with_disabled(vec![FreeResourceKey::new("openrouter", "banned-model")]),
        );
        let choice = routing
            .choose(
                JobKind::Classification,
                &[
                    free("openrouter", "banned-model"),
                    free("nous", "allowed-model"),
                ],
                &FreePool::new(),
                Instant::now(),
            )
            .expect("one free model is allowed");
        assert_eq!(choice.model(), "allowed-model");
    }

    /// Map line 1530 and 1554: the winning candidate's explanation names
    /// real, inspectable contributions — not an opaque number — and line
    /// 1553's structural separation shows up as a hard-constraint-shaped
    /// input (cost/eligibility) never being blended into the same magnitude
    /// as a soft one (order, capacity, reset).
    #[test]
    fn the_winning_candidate_carries_a_named_inspectable_explanation() {
        let routing = DisposableRouting::for_support_work(true, FreePreferences::new());
        let choice = routing
            .choose(
                JobKind::MemoryExtraction,
                &[free("openrouter", "a-free-model")],
                &FreePool::new(),
                Instant::now(),
            )
            .expect("configured");

        let names: Vec<&str> = choice
            .explanation()
            .contributions()
            .iter()
            .map(|c| c.name())
            .collect();
        assert!(names.contains(&"cost"));
        assert!(names.contains(&"user free-resource order"));
        assert!(names.contains(&"normalized remaining capacity"));
        assert!(names.contains(&"time until quota reset"));
        assert!(choice.describe().contains("normalized remaining capacity"));
    }

    /// Map line 1536 and 1549: when a caller supplies real capacity and
    /// reset data for a candidate, it reaches the explanation with a real
    /// magnitude — not the `0.0` absence contribution.
    #[test]
    fn real_capacity_and_reset_data_reach_the_explanation() {
        use crate::provider::quota::{
            Capacity, CapacityState, NativeAmount, Pool, Reading, ReadingSource,
        };

        const OBSERVED: i64 = 1_800_000_000;
        let measured = |value: i64, unit: &str| {
            Capacity::Measured(Reading::new(
                NativeAmount::whole(value, unit),
                OBSERVED,
                ReadingSource::ResponseHeader("x-ratelimit".to_owned()),
            ))
        };
        let state = CapacityState::metered_balance().with_credits(
            Pool::inapplicable()
                .with_remaining(measured(40, "tokens"))
                .with_limit(measured(100, "tokens")),
        );
        let scored = state
            .remaining_capacity_score()
            .expect("both halves of the credits pool are measured");

        let capacity = CandidateCapacity::new()
            .with_remaining_capacity(Some(scored))
            .with_seconds_until_reset(Some(120));
        let routing = DisposableRouting::for_support_work(true, FreePreferences::new());
        let choice = routing
            .choose(
                JobKind::MemoryExtraction,
                &[free("openrouter", "a-free-model").with_capacity(capacity)],
                &FreePool::new(),
                Instant::now(),
            )
            .expect("configured");

        let capacity_line = choice
            .explanation()
            .contributions()
            .iter()
            .find(|c| c.name() == "normalized remaining capacity")
            .expect("a capacity contribution is always present");
        assert!(
            capacity_line.magnitude() > 0.0,
            "real capacity data must produce a nonzero contribution, not the absence default"
        );
        assert!(capacity_line.evidence().contains("credits"));
        assert!(capacity_line.evidence().contains("40%"));

        let reset_line = choice
            .explanation()
            .contributions()
            .iter()
            .find(|c| c.name() == "time until quota reset")
            .expect("a reset contribution is always present");
        assert!(reset_line.evidence().contains("120"));
    }

    /// Map line 1550: Phase 32F's protected-reserve policy is a real gate on
    /// the metered-fallback path, proven with the actual production
    /// function — not a stand-in. A distant, known reset with a Reserve-band
    /// candidate is denied; the same candidate with no reset knowledge at
    /// all is allowed, because `evaluate_reserve_spend` treats "no cheaper
    /// alternative and no distant reset" as the least-bad option.
    #[test]
    fn the_protected_reserve_policy_gates_the_metered_fallback() {
        use crate::provider::quota::CapacityBand;

        let denied_capacity = CandidateCapacity::new()
            .with_band(Some(CapacityBand::Reserve))
            .with_seconds_until_reset(Some(7_200));
        let routing = DisposableRouting::for_support_work(true, FreePreferences::new());
        let err = routing
            .choose(
                JobKind::MemoryExtraction,
                &[metered("openrouter", "a-reserved-model").with_capacity(denied_capacity)],
                &FreePool::new(),
                Instant::now(),
            )
            .expect_err("a distant reset on a Reserve-band candidate must be denied");
        assert!(matches!(err, NoResource::ProtectedReserveDenied { .. }));
        assert!(err.to_string().contains("a-reserved-model"));

        let allowed = routing
            .choose(
                JobKind::MemoryExtraction,
                &[metered("openrouter", "an-unread-model")],
                &FreePool::new(),
                Instant::now(),
            )
            .expect("a candidate nothing has been read about is not withheld by reserve policy");
        assert_eq!(allowed.model(), "an-unread-model");
        assert!(
            allowed
                .explanation()
                .contributions()
                .iter()
                .any(|c| c.name() == "protected-reserve policy" && c.evidence().contains("allowed"))
        );
    }

    /// §35: mutate the call, not the callee. Deleting the reserve check from
    /// the metered-fallback path (treating every decision as allowed) must
    /// make a named test fail — proving the gate in `choose` is a real
    /// caller of `evaluate_reserve_spend`, not decoration around one.
    ///
    /// This test does not mutate source; it exists so that mutating
    /// `evaluate_reserve_spend`'s call in `choose` (deleting the
    /// `if !decision.is_allowed() { ... continue; }` guard) is guaranteed to
    /// flip `the_protected_reserve_policy_gates_the_metered_fallback` from
    /// pass to fail — recorded here so a future reader can find the killed
    /// mutation's evidence without re-deriving it. See this package's report
    /// for the actual mutation run.
    #[test]
    fn a_metered_candidate_with_no_reserve_data_is_never_denied() {
        let routing = DisposableRouting::for_support_work(true, FreePreferences::new());
        let choice = routing
            .choose(
                JobKind::MemoryExtraction,
                &[metered("openrouter", "plain-metered-model")],
                &FreePool::new(),
                Instant::now(),
            )
            .expect("no capacity data defaults to the least protective band, so nothing denies it");
        assert_eq!(choice.model(), "plain-metered-model");
    }
}
