//! The one production caller of [`crate::routing::disposable::DisposableRouting`]
//! (Phase 9I lines 530, 531, 532, 540), and — since GH-ROUTED-EXTRACTION-CLIENT
//! — the thing that then **calls the resource it chose**.
//!
//! # What this is
//!
//! [`RoutedModel`] is an [`ExtractionModel`] that asks the disposable routing
//! policy which resource should perform this bounded support job —
//! preferring a free one (line 530), letting an explicitly configured free
//! model such as a Nemotron variant participate by name (line 531), and
//! reporting why the resource it landed on is the one in use (line 540) —
//! and then builds nothing itself and sends nothing itself. Its caller hands
//! it a [`ConfiguredModel`] for the chosen resource, and it makes the call
//! through that.
//!
//! # Why the client arrives from outside
//!
//! Turning a [`DisposableChoice`] into a client needs the provider table,
//! the layering rule that resolves a provider's configuration, and a
//! [`crate::secret::SecretStore`]. All three live in `main.rs`, which is the
//! only place in this build that reads a user's configuration, and none of
//! them may live here: `crate::routing` is pure by rule and this module is
//! the seam directly above it. So [`RoutedModel::with_client`] takes a
//! client the caller resolved for the choice this type already made, the
//! same shape `main.rs::context_firewall_reducer_model` and
//! `main.rs::classification_model` already use one layer over.
//!
//! # The consent boundary is the caller's, and it did not move
//!
//! A [`RoutedModel`] with **no** client behaves exactly as this type did
//! before it could call anything: it chooses, it says what it chose, and
//! [`ExtractionModel::complete`] fails with [`ModelError::Unavailable`]. That
//! is not a leftover — it is the state every user who has not configured
//! `[memory] extraction_model` is still in, and `main.rs`'s own
//! `configured_extraction_model` documents why: *a free-model list is a
//! statement about cost; it is not a request that a hook running inside a
//! coding session start making outbound requests.*
//!
//! What changed is what happens **after** that consent. The configured
//! extraction model used to bypass the router entirely, so the policy chose
//! only when nothing would be called and the model that was called had never
//! been routed. Now the configured model is one candidate among the user's
//! own free ones and `DisposableRouting::choose` ranks them all, which is
//! what makes line 530's *prefer free models when quality is sufficient*
//! true on the path that actually spends something.
//!
//! # What the call teaches, and where it goes
//!
//! [`WorkloadOutcome`] is [`crate::routing::free::FreePool`]'s only teacher
//! (line 534), and until this batch nothing on this path produced one. The
//! translation from one finished exchange to one outcome happens **here**,
//! once, exactly as [`WorkloadOutcome`]'s own documentation requires — and
//! the result goes to the observer the caller supplied, which is what makes
//! it durable for the next short-lived process that dispatches an extraction.

use std::time::Instant;

use crate::routing::classify::classify_heuristically;
use crate::routing::disposable::{
    DisposableCandidate, DisposableChoice, DisposableRouting, JobKind, NoResource,
};
use crate::routing::free::{FreePool, FreeResource, WorkloadOutcome};

use super::model::ConfiguredModel;
use super::{ExtractionModel, ModelError, ModelReply, Prompt};

/// Where one finished exchange's [`WorkloadOutcome`] goes.
///
/// A boxed closure rather than a channel or a returned value because the
/// model is **moved into the extraction thread and dropped there**
/// (`main.rs::run_extraction`), so there is nothing left for a caller to read
/// afterwards. `Send + Sync` for the same reason [`ExtractionModel`] is.
type ObserveOutcome = Box<dyn Fn(&FreeResource, WorkloadOutcome) + Send + Sync>;

/// Routes a disposable job through [`DisposableRouting`] and performs it
/// through the client its caller resolved for the resource that won.
///
/// The choice is made once, at construction — [`ExtractionModel::describe`]
/// must be cheap and is called on every run including one with an empty
/// chunk, so the routing decision cannot be deferred into it.
pub struct RoutedModel {
    outcome: Result<DisposableChoice, NoResource>,
    /// The client for the chosen resource. [`None`] is *no client was
    /// supplied* — the unconsented case, where this type still chooses,
    /// still explains, and still calls nothing. [`Some`]`(`[`Err`]`)` is
    /// *a client was owed and could not be built*, which is a different fact
    /// and says so in words.
    client: Option<Result<ConfiguredModel, String>>,
    /// [`crate::routing::CredentialId::label`] for the chosen resource —
    /// a provider and a reference **name**, never a value. Travels onto
    /// [`super::ModelCall::credential_label`] so the ledger row can say
    /// which allowance paid for the call.
    credential_label: Option<String>,
    /// The resource health is keyed by, when there is a call to learn from.
    resource: Option<FreeResource>,
    observe: Option<ObserveOutcome>,
}

impl RoutedModel {
    /// Ask `routing` which of `candidates` should perform `job`, against the
    /// health `pool` the caller has.
    ///
    /// `pool` is the durable half of Phase 9I line 534. Health is learned
    /// from real request outcomes, and the processes that dispatch an
    /// extraction — `glasshouse hook` and `glasshouse memory commit` — are
    /// short-lived and never see each other's, so an empty pool here would
    /// mean every free candidate is treated as available on every dispatch
    /// and `choose`'s health filter could never exclude anything (map line
    /// 1433, practice §36). `main.rs::observed_health_of` reads the persisted
    /// readings back; [`FreePool::new`] is still the honest argument for a
    /// caller that genuinely has no history.
    pub fn new(
        job: JobKind,
        candidates: &[DisposableCandidate],
        routing: &DisposableRouting,
        pool: &FreePool,
    ) -> Self {
        Self::from_outcome(routing.choose(job, candidates, pool, Instant::now(), None))
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
    /// premium reserve only at `WorkloadTier::Heavy` (tier 3) or above. A transcript of hard
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
    ///
    /// It keeps [`FreePool::new`] for the same reason it keeps the fixed
    /// tier: it has no caller, so there is no process whose persisted health
    /// this could honestly be reading.
    pub fn new_for_request(
        job: JobKind,
        request_text: &str,
        candidates: &[DisposableCandidate],
        routing: &DisposableRouting,
    ) -> Self {
        let classification = classify_heuristically(request_text);
        let pool = FreePool::new();
        Self::from_outcome(routing.choose(
            job,
            candidates,
            &pool,
            Instant::now(),
            Some(&classification),
        ))
    }

    fn from_outcome(outcome: Result<DisposableChoice, NoResource>) -> Self {
        Self {
            outcome,
            client: None,
            credential_label: None,
            resource: None,
            observe: None,
        }
    }

    /// What the policy decided, so a caller can resolve a client for exactly
    /// that resource rather than re-deriving the decision.
    ///
    /// Asking `choose` a second time would produce a different [`Instant`]
    /// and could produce a different answer, which is the same reason
    /// `main.rs` renders the ledger's rationale from `describe()` rather than
    /// routing again for it.
    pub fn choice(&self) -> Result<&DisposableChoice, &NoResource> {
        self.outcome.as_ref()
    }

    /// Supply the client for the resource [`Self::choice`] named, or the one
    /// sentence saying why it could not be built.
    ///
    /// `credential_label` is [`crate::routing::CredentialId::label`] — two
    /// names and never a value, safe to persist and render for the reason
    /// that method's own doc gives. It is the **only** thing about the
    /// credential this type holds; the value itself is resolved by the
    /// caller, handed to [`ConfiguredModel`], and read by that type in
    /// exactly one place, the `authorization` header it builds.
    pub fn with_client(
        mut self,
        client: Result<ConfiguredModel, String>,
        credential_label: impl Into<String>,
    ) -> Self {
        if let Ok(choice) = &self.outcome {
            self.resource = Some(FreeResource::new(
                choice.credential().clone(),
                choice.model(),
            ));
        }
        self.credential_label = Some(credential_label.into());
        self.client = Some(client);
        self
    }

    /// Where this run's [`WorkloadOutcome`] goes when a call is actually
    /// made.
    ///
    /// Nothing is reported for a run that made no call: a choice is not an
    /// exchange, and a pool that learned from decisions rather than from work
    /// would be inventing the health line 534 says must be observed.
    pub fn observing(
        mut self,
        observe: impl Fn(&FreeResource, WorkloadOutcome) + Send + Sync + 'static,
    ) -> Self {
        self.observe = Some(Box::new(observe));
        self
    }

    /// The client to call, or the [`ModelError`] that stands in for it.
    ///
    /// Three no-call states, kept apart because they are three different
    /// facts and the caller renders them differently:
    /// no resource was chosen, no client was supplied, and a client was owed
    /// and could not be built.
    fn callable(&self) -> Result<&ConfiguredModel, ModelError> {
        match &self.client {
            Some(Ok(client)) => Ok(client),
            Some(Err(_)) | None => Err(ModelError::Unavailable),
        }
    }
}

/// What one finished extraction exchange said about the resource that served
/// it — [`WorkloadOutcome`]'s *"the caller that holds the exchange translates
/// once"*, and this is that caller.
///
/// [`ModelError`] is deliberately not an HTTP status, so the one status whose
/// routing meaning cannot be recovered from the variant alone —
/// `429`, which is [`ModelError::Failed`] like a dozen unrelated transport
/// problems — is recovered from the phrase `crate::memory::extract::model`
/// names for it. That constant exists for this comparison and for no other
/// reason.
///
/// `retry_after` is always [`None`]: `ConfiguredModel` reads no response
/// headers, so a provider that declared its own wait is not heard here and
/// the bounded invented backoff applies instead. That understates a declared
/// cadence and never overstates one, which is the safe direction.
fn workload_outcome(result: &Result<ModelReply, ModelError>) -> WorkloadOutcome {
    match result {
        Ok(_) => WorkloadOutcome::Served,
        // 401/403 — the credential itself, not the model. Waiting does not
        // fix it, and calling it a capacity failure would have this resource
        // quietly retried on a bounded backoff forever.
        Err(ModelError::Refused) => WorkloadOutcome::CredentialRejected,
        Err(ModelError::Failed { phrase }) if *phrase == super::model::RATE_LIMITED => {
            WorkloadOutcome::RateLimited { retry_after: None }
        }
        Err(ModelError::Unavailable | ModelError::TimedOut | ModelError::Failed { .. }) => {
            WorkloadOutcome::CapacityFailure
        }
    }
}

impl ExtractionModel for RoutedModel {
    fn describe(&self) -> String {
        match (&self.outcome, &self.client) {
            (Ok(choice), Some(Ok(client))) => {
                format!("{} — asked as {}", choice.describe(), client.describe())
            }
            (Ok(choice), Some(Err(reason))) => {
                format!("{} — no model was called: {reason}", choice.describe())
            }
            (Ok(choice), None) => format!("{} — no model was called", choice.describe()),
            (Err(reason), _) => format!(
                "none configured (Phase 39 supplies the provider): {reason} — no model was \
                 called"
            ),
        }
    }

    fn complete(&self, prompt: &Prompt) -> Result<String, ModelError> {
        self.complete_observed(prompt)
            .map(|answered| answered.reply)
    }

    fn complete_observed(&self, prompt: &Prompt) -> Result<ModelReply, ModelError> {
        let client = self.callable()?;
        let mut result = client.complete_observed(prompt);

        // The credential's *label* onto the call, so the ledger row can say
        // which allowance paid for it (`quota_context`, the column
        // `gateway::session` already records a credential label in). The
        // value stays where `ConfiguredModel` put it — one header — and is
        // not reachable from here at all.
        if let Ok(reply) = &mut result
            && let Some(call) = &mut reply.call
        {
            call.credential_label = self.credential_label.clone();
        }

        // Line 534's write side, on the only path in this build that produces
        // a real outcome for a disposable job. Reported before the result is
        // handed back so that a caller which drops the reply still taught the
        // pool what the call cost it.
        if let (Some(resource), Some(observe)) = (&self.resource, &self.observe) {
            observe(resource, workload_outcome(&result));
        }

        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::routing::free::FreePreferences;
    use crate::routing::{Cost, CredentialId, UseReason};
    use crate::secret::SecretRef;
    use std::sync::{Arc, Mutex};

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
    /// metered one, and with no client supplied the model never answers.
    #[test]
    fn a_configured_free_model_is_preferred_and_no_model_is_called() {
        let routing = DisposableRouting::for_support_work(true, FreePreferences::new());
        let candidates = [
            metered("openrouter", "an-expensive-model"),
            free("openrouter", "nvidia/nemotron-nano-9b-v2:free"),
        ];
        let model = RoutedModel::new(
            JobKind::MemoryExtraction,
            &candidates,
            &routing,
            &FreePool::new(),
        );

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
        let model = RoutedModel::new(
            JobKind::Classification,
            &candidates,
            &routing,
            &FreePool::new(),
        );
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
        let model = RoutedModel::new(JobKind::MemoryExtraction, &[], &routing, &FreePool::new());
        assert!(model.describe().contains("no model was called"));
        assert!(matches!(
            model.complete(&test_prompt()),
            Err(ModelError::Unavailable)
        ));
    }

    /// A chosen resource whose client could not be built says which resource
    /// it was and why there is no call — not the same sentence as "nothing
    /// was configured", because the fixes differ.
    #[test]
    fn a_chosen_resource_with_no_usable_client_says_which_and_why() {
        let routing = DisposableRouting::for_support_work(true, FreePreferences::new());
        let candidates = [free("openrouter", "a-free-model")];
        let model = RoutedModel::new(
            JobKind::MemoryExtraction,
            &candidates,
            &routing,
            &FreePool::new(),
        )
        .with_client(
            Err("`openrouter` has no base URL configured".to_owned()),
            "openrouter/OPENROUTER_API_KEY",
        );

        let described = model.describe();
        assert!(described.contains("a-free-model"), "{described}");
        assert!(
            described.contains("has no base URL configured"),
            "{described}"
        );
        assert!(matches!(
            model.complete(&test_prompt()),
            Err(ModelError::Unavailable)
        ));
    }

    /// The observer is only fed by a real call. A run that chose a resource
    /// and never dialled it must teach the pool nothing — otherwise health
    /// would be learned from decisions rather than from work, which is
    /// exactly what line 534 forbids.
    #[test]
    fn a_run_that_calls_nothing_reports_no_outcome() {
        let seen: Arc<Mutex<Vec<WorkloadOutcome>>> = Arc::new(Mutex::new(Vec::new()));
        let recorder = Arc::clone(&seen);
        let routing = DisposableRouting::for_support_work(true, FreePreferences::new());
        let candidates = [free("openrouter", "a-free-model")];
        let model = RoutedModel::new(
            JobKind::MemoryExtraction,
            &candidates,
            &routing,
            &FreePool::new(),
        )
        .observing(move |_, outcome| recorder.lock().unwrap().push(outcome));

        assert!(model.complete(&test_prompt()).is_err());
        assert!(seen.lock().unwrap().is_empty());
    }

    /// The four translations `FreePool` is taught with, from the errors this
    /// build's own transport actually produces.
    #[test]
    fn an_exchange_translates_to_exactly_one_workload_outcome() {
        assert_eq!(
            workload_outcome(&Ok(ModelReply::uncalled("an answer"))),
            WorkloadOutcome::Served
        );
        assert_eq!(
            workload_outcome(&Err(ModelError::Refused)),
            WorkloadOutcome::CredentialRejected
        );
        assert_eq!(
            workload_outcome(&Err(ModelError::Failed {
                phrase: super::super::model::RATE_LIMITED
            })),
            WorkloadOutcome::RateLimited { retry_after: None }
        );
        assert_eq!(
            workload_outcome(&Err(ModelError::TimedOut)),
            WorkloadOutcome::CapacityFailure
        );
        assert_eq!(
            workload_outcome(&Err(ModelError::Failed {
                phrase: "the extraction model's reply was not JSON"
            })),
            WorkloadOutcome::CapacityFailure
        );
    }
}
