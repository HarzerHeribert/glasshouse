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

use super::free::{FreePool, FreePreferences, FreeResource, FreeResourceKey};
use super::{
    Contribution, Cost, CredentialId, EligibleCandidate, HardConstraint, RoutingExplanation,
    UseReason, apply_hard_constraints,
};
use crate::provider::quota::{
    CapacityBand, RemainingCapacityScore, ReserveDecision, ReserveDecisionInputs,
    evaluate_reserve_spend,
};
use crate::provider::telemetry::RetainedPick;
use crate::routing::classify::{TaskClassification, WorkloadTier};

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

    /// Whether a *read* band puts this resource outside its protected
    /// reserve, so that spending it costs nobody's reserve — the predicate
    /// [`cheaper_adequate_resource_exists`] is built from.
    ///
    /// `None` is **not** outside the reserve here. That is deliberately the
    /// opposite of [`DisposableRouting::choose`]'s own
    /// `unwrap_or(CapacityBand::Plenty)` one field away, and both are the same
    /// rule applied to the two different questions being asked: an unread
    /// resource is never *withheld* by a band nobody observed, and it is never
    /// *offered* as the reason to withhold another one either.
    fn is_outside_reserve(&self) -> bool {
        self.capacity
            .band
            .is_some_and(|band| band > CapacityBand::Reserve)
    }

    fn as_free_resource(&self) -> FreeResource {
        FreeResource::new(self.credential.clone(), self.model.clone())
    }

    fn key(&self) -> FreeResourceKey {
        FreeResourceKey::new(self.provider.clone(), self.model.clone())
    }
}

/// Map line 1434: whether `candidate` is known to have no headroom left on
/// whichever dimension `crate::provider::quota::CapacityState::remaining_capacity_score`
/// found tightest — the reading `candidate.capacity.remaining_capacity`
/// carries, which may be bound by requests-per-minute or another dimension
/// depending on what that call found.
///
/// `false` whenever nothing is known (`None`): an unread candidate is not a
/// candidate known to be exhausted, and eliminating on absence would turn "we
/// have no telemetry" into "this provider is full" — precisely what
/// [`CandidateCapacity`]'s own doc comment already refuses for the *scoring*
/// path, and this is the same rule applied to elimination.
fn has_no_known_headroom(candidate: &DisposableCandidate) -> bool {
    candidate
        .capacity
        .remaining_capacity
        .as_ref()
        .is_some_and(|score| score.fraction() <= 0.0)
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
    /// did not run.
    Retained(RetainedPick),
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

        let (eligible, _rejected) = apply_hard_constraints(candidates.to_vec(), |candidate| {
            if candidate.cost().is_free() || self.metered.permits_metered() {
                Ok(())
            } else {
                Err(HardConstraint::UserConstraint)
            }
        });

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
    /// the returned pick with `RoutingStickyCache::store`, but neither call
    /// happens in this crate today — see this package's report for the exact
    /// `main.rs` insertion point that would make it the production path.
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
    pub fn choose_for_automatic_classification(
        &self,
        candidates: &[DisposableCandidate],
        pool: &FreePool,
        now: Instant,
        now_unix: i64,
        classification: Option<&TaskClassification>,
        retained: Option<RetainedPick>,
    ) -> Result<AutomaticClassificationDecision, NoResource> {
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
                return Ok(AutomaticClassificationDecision::Retained(pick.clone()));
            }
        }

        let choice = self.choose(
            JobKind::Classification,
            candidates,
            pool,
            now,
            classification,
        )?;
        let pick = RetainedPick {
            provider: choice.provider().to_owned(),
            model: choice.model().to_owned(),
            chosen_at_unix: now_unix,
        };
        Ok(AutomaticClassificationDecision::Fresh(choice, pick))
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
                None,
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
                None,
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
                None,
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
                None,
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
                None,
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
                None,
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
                None,
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
                None,
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
                None,
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
                None,
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
                None,
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
                None,
            )
            .expect("no capacity data defaults to the least protective band, so nothing denies it");
        assert_eq!(choice.model(), "plain-metered-model");
    }

    /// GH-RESERVE-RESET, map lines 1291 and 1292: reset distance, not the
    /// band alone, decides the outcome. `the_protected_reserve_policy_gates_the_metered_fallback`
    /// only ever drives a distant reset, so nothing in the suite watched the
    /// imminent branch of `evaluate_reserve_spend` before this test — the
    /// orchestrator proved that gap by mutating `reset_urgency`'s distant arm
    /// from `0.0` to `1.0` and watching 37 tests, including that one, stay
    /// green.
    ///
    /// Both candidates share the same Reserve band, the same model identity
    /// and the same everything else `evaluate_reserve_spend` reads; only
    /// `seconds_until_reset` moves from [`RESET_IMMINENT_SECONDS`] to
    /// [`RESET_DISTANT_SECONDS`] (referenced by name per §17's premise
    /// discipline, not copied as a literal, so a change to either constant
    /// moves this test with it).
    #[test]
    fn reset_distance_alone_flips_the_protected_reserve_decision() {
        use crate::provider::quota::{CapacityBand, RESET_DISTANT_SECONDS, RESET_IMMINENT_SECONDS};

        let base = CandidateCapacity::new().with_band(Some(CapacityBand::Reserve));
        let imminent_capacity = base
            .clone()
            .with_seconds_until_reset(Some(RESET_IMMINENT_SECONDS));
        let distant_capacity = base
            .clone()
            .with_seconds_until_reset(Some(RESET_DISTANT_SECONDS));

        // Assert the premise (§17): the two inputs actually differ, and the
        // only thing they differ in is `seconds_until_reset` — strip that one
        // field back out of each and they become equal, so the band (and
        // every other field `evaluate_reserve_spend` could read) never moved.
        assert_ne!(
            imminent_capacity, distant_capacity,
            "the two capacities must actually differ for this test to prove anything"
        );
        assert_eq!(
            imminent_capacity.clone().with_seconds_until_reset(None),
            distant_capacity.clone().with_seconds_until_reset(None),
            "band and every other field besides seconds_until_reset must be identical"
        );

        let routing = DisposableRouting::for_support_work(true, FreePreferences::new());

        let allowed = routing
            .choose(
                JobKind::MemoryExtraction,
                &[metered("openrouter", "same-reserved-model").with_capacity(imminent_capacity)],
                &FreePool::new(),
                Instant::now(),
                None,
            )
            .expect(
                "a reset within RESET_IMMINENT_SECONDS permits spending the reserve (line 1291)",
            );
        assert_eq!(allowed.model(), "same-reserved-model");

        let denied = routing
            .choose(
                JobKind::MemoryExtraction,
                &[metered("openrouter", "same-reserved-model").with_capacity(distant_capacity)],
                &FreePool::new(),
                Instant::now(),
                None,
            )
            .expect_err(
                "a reset at RESET_DISTANT_SECONDS denies the same Reserve-band candidate (line 1292)",
            );
        assert!(matches!(denied, NoResource::ProtectedReserveDenied { .. }));
    }

    /// Two metered candidates whose *bands* differ, built so that the one in
    /// the protected reserve would otherwise **win** — the fixture the two
    /// tests below share.
    ///
    /// # Why the reserved candidate is the better-scoring one, and why that
    /// is not a contrived configuration
    ///
    /// `score` never reads `band`; the magnitude it reads is
    /// `remaining_capacity`. The two are genuinely independent inputs,
    /// because a band is a percentage compared against **that provider's own
    /// thresholds** — `EffectiveConfig::reserve_percent(provider)` — and
    /// `phase-32d`/`phase-32f` already ruled that a user may widen one
    /// provider's reserve past the global `Tight` boundary as a legitimate
    /// policy. So "60% left, and that is inside the reserve its owner asked
    /// for" beside "30% left, and that is plenty by its owner's thresholds"
    /// is exactly the configuration those rulings describe, not an invention
    /// of this test.
    ///
    /// It matters because it is what makes the tests non-vacuous: without
    /// the reserve gate the higher-scoring reserved candidate is chosen, so
    /// any test that saw the *other* one chosen has watched the gate act and
    /// not the scorer.
    fn reserved_and_unreserved_pair(
        reserved_reset: Option<i64>,
    ) -> (DisposableCandidate, DisposableCandidate) {
        use crate::provider::quota::{
            Capacity, CapacityBand, CapacityState, NativeAmount, Pool, Reading, ReadingSource,
        };

        const OBSERVED: i64 = 1_800_000_000;
        let percent_remaining = |value: i64| {
            let measured = |amount: i64| {
                Capacity::Measured(Reading::new(
                    NativeAmount::whole(amount, "tokens"),
                    OBSERVED,
                    ReadingSource::ResponseHeader("x-ratelimit".to_owned()),
                ))
            };
            CapacityState::metered_balance()
                .with_credits(
                    Pool::inapplicable()
                        .with_remaining(measured(value))
                        .with_limit(measured(100)),
                )
                .remaining_capacity_score()
                .expect("both halves of the credits pool are measured")
        };

        let reserved = metered("openrouter", "a-reserved-model").with_capacity(
            CandidateCapacity::new()
                .with_band(Some(CapacityBand::Reserve))
                .with_remaining_capacity(Some(percent_remaining(60)))
                .with_seconds_until_reset(reserved_reset),
        );
        let unreserved = metered("anyrouter", "a-plentiful-model").with_capacity(
            CandidateCapacity::new()
                .with_band(Some(CapacityBand::Plenty))
                .with_remaining_capacity(Some(percent_remaining(30))),
        );
        (reserved, unreserved)
    }

    /// Capability map line 1288 — *"avoid spending protected reserve on
    /// low-tier work while cheaper adequate resources exist"* — at the one
    /// production caller of `evaluate_reserve_spend`.
    ///
    /// The whole line lives in one input, and that input was a hardcoded
    /// `false` until this package: with it, the policy's
    /// cheaper-alternative branch is unreachable from production, and the
    /// line is not a missing mechanism but an unfed one.
    ///
    /// **Premise first (§17), and it is the same candidate both times.** A
    /// Reserve-band candidate *alone* is allowed — nothing cheaper exists, so
    /// spending the reserve is the least-bad option — and is chosen. Put an
    /// unreserved candidate beside it and the reserved one is refused, so the
    /// unreserved one is chosen instead, although it scores strictly lower.
    /// Only the presence of the sibling moved.
    ///
    /// Deleting this test loses the only proof that
    /// `cheaper_adequate_resource_exists` carries a real value; restoring the
    /// constant must fail it.
    #[test]
    fn line_1288_an_unreserved_sibling_denies_the_reserve_to_low_tier_work() {
        let routing = DisposableRouting::for_support_work(true, FreePreferences::new());
        let (reserved, unreserved) = reserved_and_unreserved_pair(None);

        let alone = routing
            .choose(
                JobKind::MemoryExtraction,
                std::slice::from_ref(&reserved),
                &FreePool::new(),
                Instant::now(),
                None,
            )
            .expect("with nothing cheaper available, spending the reserve is the least-bad option");
        assert_eq!(
            alone.model(),
            "a-reserved-model",
            "the premise: this candidate is chosen when it is the only one"
        );

        let beside_a_cheaper_one = routing
            .choose(
                JobKind::MemoryExtraction,
                &[reserved.clone(), unreserved.clone()],
                &FreePool::new(),
                Instant::now(),
                None,
            )
            .expect("the unreserved candidate can serve, so the job is not refused");
        assert_eq!(
            beside_a_cheaper_one.model(),
            "a-plentiful-model",
            "a resource outside its protected reserve is adequate and cheaper in the currency \
             this policy protects, so leaf-tier work must not spend the reserve (line 1288)"
        );
        assert!(
            beside_a_cheaper_one
                .explanation()
                .contributions()
                .iter()
                .any(|c| c.name() == "protected-reserve policy" && c.evidence().contains("allowed")),
            "the chosen candidate still records the reserve decision that let it through"
        );

        // The order of the candidate list must not decide this: the same two
        // resources the other way round answer the same.
        let reversed = routing
            .choose(
                JobKind::MemoryExtraction,
                &[unreserved, reserved],
                &FreePool::new(),
                Instant::now(),
                None,
            )
            .expect("the unreserved candidate can serve, whichever end of the list it is on");
        assert_eq!(reversed.model(), "a-plentiful-model");
    }

    /// The two ways a sibling is **not** a cheaper adequate resource, which
    /// are the two ways the change for line 1288 could have made the policy
    /// refuse work it should do.
    ///
    /// - **Equally reserved.** Two candidates both inside their protected
    ///   reserve are not alternatives to each other. If they were, a user
    ///   whose every metered resource is in its reserve band would get
    ///   `ProtectedReserveDenied` for all of them instead of the least-bad
    ///   spend `evaluate_reserve_spend`'s tail is written to allow. A
    ///   `>=` where the predicate says `>` produces exactly that, and this
    ///   test is what catches it.
    /// - **Unread.** A resource nothing has been observed about may be deep
    ///   in its own reserve; withholding a spend on the strength of it would
    ///   invent the judgement the input exists to avoid. `None` is not
    ///   "outside the reserve" — deliberately the opposite of `choose`'s own
    ///   `unwrap_or(CapacityBand::Plenty)` for the candidate *being judged*,
    ///   and both are the same refusal to let an unobserved band decide
    ///   anything.
    #[test]
    fn an_equally_reserved_or_unread_sibling_is_not_a_cheaper_alternative() {
        use crate::provider::quota::CapacityBand;

        let routing = DisposableRouting::for_support_work(true, FreePreferences::new());
        let (reserved, _) = reserved_and_unreserved_pair(None);
        let also_reserved = metered("anyrouter", "another-reserved-model")
            .with_capacity(CandidateCapacity::new().with_band(Some(CapacityBand::Reserve)));
        let unread = metered("anyrouter", "an-unread-model");

        assert_eq!(
            routing
                .choose(
                    JobKind::MemoryExtraction,
                    &[reserved.clone(), also_reserved],
                    &FreePool::new(),
                    Instant::now(),
                    None,
                )
                .expect(
                    "when every metered resource is inside its reserve, spending one is still \
                     the least-bad option — they are not alternatives to each other"
                )
                .model(),
            "a-reserved-model",
            "the better-scoring reserved candidate is chosen, and neither denies the other"
        );

        assert_eq!(
            routing
                .choose(
                    JobKind::MemoryExtraction,
                    &[reserved, unread],
                    &FreePool::new(),
                    Instant::now(),
                    None,
                )
                .expect("a candidate nothing has been read about denies nothing")
                .model(),
            "a-reserved-model",
            "an unobserved band is not evidence that a cheaper adequate resource exists"
        );
    }

    /// Capability map line 1291 — *"allow reserve policy to become more
    /// permissive shortly before a known quota reset"* — which needs line
    /// 1288's input to be observable at all.
    ///
    /// `phase-32f.md` recorded exactly why this line stayed open after its
    /// own mechanism was built and tested: `evaluate_reserve_spend`'s tail
    /// denies only when `cheaper_adequate_resource_exists`, and otherwise
    /// falls through to `Allow`, so with that input nailed to `false` the
    /// imminent-reset branch's `Allow` and the default `Allow` were **the
    /// same decision** and "more permissive" could not be seen. Disabling
    /// the branch outright changed nothing; the orchestrator ran that
    /// mutation and it SURVIVED.
    ///
    /// With a real sibling the two decisions come apart. Same pair of
    /// candidates, same bands, same scores: at a reset the policy calls
    /// imminent the reserved candidate is spent, and at a reset it does not
    /// the cheaper one is taken instead. **Reset distance is the only field
    /// that moves**, asserted below rather than claimed.
    ///
    /// The far case is deliberately *between* [`RESET_IMMINENT_SECONDS`] and
    /// [`RESET_DISTANT_SECONDS`], so that the denial comes from line 1288's
    /// branch and not from the distant-reset branch
    /// `reset_distance_alone_flips_the_protected_reserve_decision` already
    /// covers — two different lines, kept apart.
    #[test]
    fn line_1291_an_imminent_reset_makes_the_policy_spend_a_reserve_it_would_otherwise_keep() {
        use crate::provider::quota::{RESET_DISTANT_SECONDS, RESET_IMMINENT_SECONDS};

        let (imminent, unreserved) = reserved_and_unreserved_pair(Some(RESET_IMMINENT_SECONDS));
        let midway = (RESET_IMMINENT_SECONDS + RESET_DISTANT_SECONDS) / 2;
        let (not_imminent, _) = reserved_and_unreserved_pair(Some(midway));

        // Assert the premise (§17): the reserved candidate's two forms differ
        // in `seconds_until_reset` and in nothing else — strip that one field
        // from both and they are the same candidate, so band, capacity score
        // and identity have provably not moved.
        assert_ne!(imminent, not_imminent);
        assert_eq!(
            imminent
                .clone()
                .with_capacity(imminent.capacity.clone().with_seconds_until_reset(None)),
            not_imminent
                .clone()
                .with_capacity(not_imminent.capacity.clone().with_seconds_until_reset(None)),
            "only seconds_until_reset may differ between the two reserved candidates"
        );
        assert!(
            midway > RESET_IMMINENT_SECONDS && midway < RESET_DISTANT_SECONDS,
            "the far case must fall short of the distant-reset branch, so that this test is \
             about line 1291 and not about line 1292"
        );

        let routing = DisposableRouting::for_support_work(true, FreePreferences::new());

        let spent = routing
            .choose(
                JobKind::MemoryExtraction,
                &[imminent, unreserved.clone()],
                &FreePool::new(),
                Instant::now(),
                None,
            )
            .expect("configured");
        assert_eq!(
            spent.model(),
            "a-reserved-model",
            "conserving buys little when the quota is about to reset, so the policy becomes \
             permissive and spends the reserve (line 1291)"
        );

        let kept = routing
            .choose(
                JobKind::MemoryExtraction,
                &[not_imminent, unreserved],
                &FreePool::new(),
                Instant::now(),
                None,
            )
            .expect("configured");
        assert_eq!(
            kept.model(),
            "a-plentiful-model",
            "with the same cheaper sibling and a reset that is not imminent, the reserve is \
             kept — which is what makes the case above 'more permissive' rather than 'always \
             permissive'"
        );
    }

    /// GH-CLASSIFY-CALLER, the fifth link: a real [`TaskClassification`]
    /// reaching `choose`'s metered-fallback path must change the outcome, not
    /// merely be accepted and ignored. Reuses the exact Reserve-band,
    /// distant-reset candidate `the_protected_reserve_policy_gates_the_metered_fallback`
    /// denies at the fixed [`WorkloadTier::Leaf`] this policy used before a
    /// classification existed to ask — the same candidate, the same band, the
    /// same reset, only the classification differs, so any change in the
    /// outcome is attributable to `classification` alone.
    ///
    /// `classify_heuristically`'s two production examples from Phase 35's own
    /// evidence: "what is a mutex" (leaf, confidence medium, no escalation)
    /// and "run cargo test and fix whatever fails" (heavy, confidence
    /// medium) — line 2307/2317 of `provider::quota::evaluate_reserve_spend`
    /// denies every tier but heavy once a reset is distant, so this is the
    /// exact boundary a policy stuck on `WorkloadTier::Leaf` could never
    /// cross.
    #[test]
    fn a_real_classification_changes_the_metered_fallback_outcome_at_the_same_call_site() {
        use crate::provider::quota::CapacityBand;
        use crate::routing::classify::classify_heuristically;

        let reserve_capacity = || {
            CandidateCapacity::new()
                .with_band(Some(CapacityBand::Reserve))
                .with_seconds_until_reset(Some(7_200))
        };
        let routing = DisposableRouting::for_support_work(true, FreePreferences::new());

        let trivial = classify_heuristically("what is a mutex");
        assert_eq!(trivial.conservative_workload_tier(), WorkloadTier::Leaf);
        let denied = routing
            .choose(
                JobKind::MemoryExtraction,
                &[metered("openrouter", "a-reserved-model").with_capacity(reserve_capacity())],
                &FreePool::new(),
                Instant::now(),
                Some(&trivial),
            )
            .expect_err("a leaf-tier classification must not justify spending the reserve");
        assert!(matches!(denied, NoResource::ProtectedReserveDenied { .. }));

        let demanding = classify_heuristically("run cargo test and fix whatever fails");
        assert_eq!(demanding.conservative_workload_tier(), WorkloadTier::Heavy);
        let allowed = routing
            .choose(
                JobKind::MemoryExtraction,
                &[metered("openrouter", "a-reserved-model").with_capacity(reserve_capacity())],
                &FreePool::new(),
                Instant::now(),
                Some(&demanding),
            )
            .expect("a heavy-tier classification justifies spending the reserve (line 1290)");
        assert_eq!(allowed.model(), "a-reserved-model");
    }
}
