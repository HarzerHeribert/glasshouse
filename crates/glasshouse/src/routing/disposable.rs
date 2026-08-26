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
use super::{Cost, CredentialId, UseReason};

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

/// One resource a disposable job could be sent to.
///
/// Deliberately not a `super::Backend`: a backend carries a wire protocol and
/// tool semantics because an interactive session's harness depends on both,
/// and a disposable job has neither a harness nor tools. Sharing the type
/// would invite sharing the policy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DisposableCandidate {
    provider: String,
    model: String,
    credential: CredentialId,
    cost: Cost,
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
        }
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
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DisposableChoice {
    job: JobKind,
    provider: String,
    model: String,
    credential: CredentialId,
    cost: Cost,
    reason: UseReason,
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

    /// A line a settings screen or a diagnostic can show. Names only.
    pub fn describe(&self) -> String {
        format!(
            "{} on {} — {}, used by {}",
            self.model,
            self.provider,
            self.cost.as_str(),
            self.reason
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
    /// 1. **A pinned free resource wins outright** (line 536). If it cannot
    ///    serve, the job fails rather than silently going elsewhere.
    /// 2. **Free resources, in the user's own order**, skipping disabled ones
    ///    (line 536) and any whose health or allowance says it cannot serve
    ///    right now (lines 529, 535, 538). This is line 530's "prefer free
    ///    models for bounded Glasshouse support work", and line 531 falls out
    ///    of it: a model is in this list because the user marked it free, so
    ///    an explicitly configured free model such as a Nemotron variant
    ///    participates without this function knowing any model's name.
    /// 3. **A metered resource**, only if [`MeteredUse`] permits it
    ///    (line 539).
    ///
    /// Nothing here ranks on quality. Line 530's "when quality is sufficient"
    /// is the user's own marking and ordering, not a score this phase
    /// invents; Phase 9J is where measured pairing evidence arrives, and this
    /// is the function that will read it.
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

        let free: Vec<&DisposableCandidate> = candidates
            .iter()
            .filter(|candidate| candidate.cost().is_free())
            .collect();

        if let Some(pin) = self.preferences.pin() {
            let pinned = free
                .iter()
                .find(|candidate| candidate.key() == *pin)
                .filter(|candidate| pool.is_available(&candidate.as_free_resource(), now));
            return match pinned {
                Some(candidate) => Ok(self.choice(job, candidate, UseReason::UserPreference)),
                None => Err(NoResource::PinnedResourceUnavailable {
                    provider: pin.provider.clone(),
                    model: pin.model.clone(),
                }),
            };
        }

        let arranged = self.preferences.arrange(
            &free
                .iter()
                .map(|c| c.as_free_resource())
                .collect::<Vec<_>>(),
        );
        let mut first_choice: Option<&DisposableCandidate> = None;
        for resource in &arranged {
            let Some(candidate) = free
                .iter()
                .find(|candidate| candidate.as_free_resource() == *resource)
            else {
                continue;
            };
            first_choice.get_or_insert(candidate);
            if pool.is_available(resource, now) {
                // The reason a free resource is the one being used — line 540.
                // "Fallback" outranks the others because it is the most
                // informative: it says the resource the user would have got
                // could not serve.
                let reason = if first_choice.is_some_and(|first| first != *candidate) {
                    UseReason::Fallback
                } else if self.prefer_free_setting {
                    UseReason::UserPreference
                } else {
                    // The disposable class does not spend metered capacity on
                    // throwaway work as a standing rule, whether or not a
                    // metered resource happens to be configured beside it.
                    UseReason::QuotaPreservation
                };
                return Ok(self.choice(job, candidate, reason));
            }
        }

        if !self.metered.permits_metered() {
            return Err(NoResource::NoFreeResourceAndMeteredWithheld {
                withheld: self.metered.clone(),
            });
        }

        candidates
            .iter()
            .find(|candidate| !candidate.cost().is_free())
            .map(|candidate| self.choice(job, candidate, UseReason::Fallback))
            .ok_or(NoResource::NoFreeResourceAndMeteredWithheld {
                withheld: self.metered.clone(),
            })
    }

    fn choice(
        &self,
        job: JobKind,
        candidate: &DisposableCandidate,
        reason: UseReason,
    ) -> DisposableChoice {
        DisposableChoice {
            job,
            provider: candidate.provider().to_owned(),
            model: candidate.model().to_owned(),
            credential: candidate.credential().clone(),
            cost: candidate.cost(),
            reason,
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
}
