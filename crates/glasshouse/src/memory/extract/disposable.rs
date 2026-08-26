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
            outcome: routing.choose(job, candidates, &pool, Instant::now()),
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
