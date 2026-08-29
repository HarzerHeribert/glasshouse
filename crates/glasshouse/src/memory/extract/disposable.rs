//! The one production caller of [`crate::routing::disposable::DisposableRouting`]
//! (Phase 9I lines 530, 531, 532, 540).
//!
//! # What this is, and what it is not
//!
//! [`RoutedNoModel`] is an [`ExtractionModel`] that asks the disposable
//! routing policy which resource should perform this bounded support job —
//! preferring a free one (line 530), letting an explicitly configured free
//! model such as a Nemotron variant participate by name (line 531), and
//! reporting why the resource it landed on is the one in use (line 540) —
//! **and then calls no model at all.**
//!
//! Phase 39's disposable-job provider interface does not exist yet, so there
//! is nothing on the other end of the chosen resource's name to send a
//! request to. [`RoutedNoModel::complete`] always fails with
//! [`ModelError::Unavailable`], and [`RoutedNoModel::describe`] always says
//! so in words, in both branches, matching the two existing precedents this
//! project already uses for the same fact:
//! `glasshouse memory extract`'s `ReplyFromFile` and
//! `main.rs`'s `NoExtractionModel`.
//!
//! This closes lines 530, 531 and 540 — the policy is proven
//! (`crate::routing::disposable`'s own tests) and now has a real caller
//! choosing over real candidates built from the user's own configuration.
//! It does **not** close line 532: a full harness launch profile is backed
//! by `crate::profile::gateway_upstream`, a completely different path this
//! type has no connection to.

use std::time::Instant;

use crate::routing::classify::classify_heuristically;
use crate::routing::disposable::{
    DisposableCandidate, DisposableChoice, DisposableRouting, JobKind, NoResource,
};
use crate::routing::free::FreePool;

use super::{ExtractionModel, ModelError, Prompt};

/// Routes a disposable job through [`DisposableRouting`], reports the
/// outcome, and calls nothing.
///
/// The choice is made once, at construction — [`ExtractionModel::describe`]
/// must be cheap and is called on every run including one with an empty
/// chunk, so the routing decision cannot be deferred into it.
pub struct RoutedNoModel {
    outcome: Result<DisposableChoice, NoResource>,
}

impl RoutedNoModel {
    /// Ask `routing` which of `candidates` should perform `job`.
    ///
    /// There is no live [`FreePool`] to consult here: health is learned from
    /// real request outcomes (Phase 9I line 534), and this caller never makes
    /// one. An empty pool means every free candidate is treated as available,
    /// which is correct for a caller that has no health history to offer.
    pub fn new(
        job: JobKind,
        candidates: &[DisposableCandidate],
        routing: &DisposableRouting,
    ) -> Self {
        let pool = FreePool::new();
        Self {
            outcome: routing.choose(job, candidates, &pool, Instant::now(), None),
        }
    }

    /// Same as [`Self::new`], but with GH-CLASSIFY-CALLER's fifth link: a
    /// real [`crate::routing::classify::TaskClassification`] of
    /// `request_text` reaches the metered-fallback path's `tier` input (map
    /// line 1550) instead of the fixed
    /// [`crate::routing::classify::WorkloadTier::Leaf`] [`Self::new`] still
    /// passes.
    ///
    /// `classify_heuristically`, not [`crate::routing::classify::classify`]:
    /// this caller has no model answer to prefer, the same "no cheap model is
    /// available" case Phase 35's own production caller (`glasshouse
    /// classify`) is already built for.
    ///
    /// **Not called by `main.rs`, and `JobKind::MemoryExtraction` must not
    /// call it.** Two things block it, and only the first is about ordering.
    ///
    /// *Ordering:* `disposable_extraction_model` builds and calls its model
    /// closure before `run_extraction_after_turn` reads this session's events
    /// or builds its chunk, so no text exists at the point the routing
    /// decision is made. That part is fixable by reordering `main.rs`.
    ///
    /// *Semantics — the blocking one:* reordering would hand this constructor
    /// the **chunk**, which is a transcript of a finished turn, not a request.
    /// [`crate::routing::classify::classify_heuristically`] is documented as
    /// classifying *a request*, and the tier it yields feeds
    /// `evaluate_reserve_spend`, whose distant-reset branch spends protected
    /// premium reserve only for `WorkloadTier::Heavy`. A transcript of hard
    /// debugging work is full of the keywords that produce `Heavy` — so
    /// wiring the chunk here would let a *cheap* extraction job spend the
    /// reserve because the conversation it is summarising happened to be
    /// demanding. The tier would vary with conversation topic rather than
    /// with this job's own demand, which is the opposite of what the gate is
    /// protecting.
    ///
    /// This constructor is therefore correct and ready for a `JobKind` that
    /// carries a real user request — `Classification`, `Reranking`,
    /// `Evaluation` — none of which has a production caller today.
    /// `MemoryExtraction`, the only one that does, is disposable by design and
    /// keeps [`Self::new`]'s fixed `WorkloadTier::Leaf`.
    pub fn new_for_request(
        job: JobKind,
        request_text: &str,
        candidates: &[DisposableCandidate],
        routing: &DisposableRouting,
    ) -> Self {
        let classification = classify_heuristically(request_text);
        let pool = FreePool::new();
        Self {
            outcome: routing.choose(
                job,
                candidates,
                &pool,
                Instant::now(),
                Some(&classification),
            ),
        }
    }
}

impl ExtractionModel for RoutedNoModel {
    fn describe(&self) -> String {
        match &self.outcome {
            Ok(choice) => format!("{} — no model was called", choice.describe()),
            Err(reason) => format!(
                "none configured (Phase 39 supplies the provider): {reason} — no model was \
                 called"
            ),
        }
    }

    fn complete(&self, _prompt: &Prompt) -> Result<String, ModelError> {
        Err(ModelError::Unavailable)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::routing::free::FreePreferences;
    use crate::routing::{Cost, CredentialId, UseReason};
    use crate::secret::SecretRef;

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

    /// Line 530 and 531, through the real seam: a configured free model —
    /// named the way a Nemotron variant would be — is preferred over a
    /// metered one, and the model never answers.
    #[test]
    fn a_configured_free_model_is_preferred_and_no_model_is_called() {
        let routing = DisposableRouting::for_support_work(true, FreePreferences::new());
        let candidates = [
            metered("openrouter", "an-expensive-model"),
            free("openrouter", "nvidia/nemotron-nano-9b-v2:free"),
        ];
        let model = RoutedNoModel::new(JobKind::MemoryExtraction, &candidates, &routing);

        let described = model.describe();
        assert!(described.contains("nvidia/nemotron-nano-9b-v2:free"));
        assert!(described.contains("no model was called"));

        assert!(matches!(
            model.complete(&test_prompt()),
            Err(ModelError::Unavailable)
        ));
    }

    fn test_prompt() -> Prompt {
        let chunk = crate::memory::extract::chunk::SessionChunk::build(
            "session",
            None::<String>,
            std::iter::empty(),
            crate::memory::extract::chunk::ChunkLimits::default(),
        );
        Prompt::build(&chunk, &[])
    }

    /// Line 540: the reason the policy chose is visible in the description,
    /// not reconstructed here.
    #[test]
    fn the_description_names_the_reason_the_policy_gave() {
        let routing = DisposableRouting::for_support_work(true, FreePreferences::new());
        let candidates = [free("openrouter", "a-free-model")];
        let model = RoutedNoModel::new(JobKind::Classification, &candidates, &routing);
        assert!(
            model
                .describe()
                .contains(&UseReason::UserPreference.to_string())
        );
    }

    /// No candidates at all — still says plainly that no model was called,
    /// rather than looking like a successful, silent no-op.
    #[test]
    fn nothing_configured_still_says_no_model_was_called() {
        let routing = DisposableRouting::for_support_work(true, FreePreferences::new());
        let model = RoutedNoModel::new(JobKind::MemoryExtraction, &[], &routing);
        assert!(model.describe().contains("no model was called"));
        assert!(matches!(
            model.complete(&test_prompt()),
            Err(ModelError::Unavailable)
        ));
    }
}
