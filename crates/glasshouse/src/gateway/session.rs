//! What backend one live gateway-backed session is on, and what moves it —
//! Phase 9H, wired to the request path that actually serves the session.
//!
//! # This is the caller, not another mechanism
//!
//! [`mod@crate::routing::interactive`] decides; this decides nothing. Every
//! rule about stickiness, failover, migration, pins and cache locality lives
//! there as a pure function of values, and this module's whole job is to hold
//! the state those functions need, hand them real observations, and apply
//! what they answer to the [`Upstream`] the gateway is forwarding through.
//!
//! The observations are real. `SessionRouting::observe_exchange` takes an
//! `Exchange` that a connection thread has just finished — the same value
//! the gateway already logs — so the health of a free resource and the
//! decision to fail over both come from work that was going to happen anyway.
//! Phase 9I line 534 asks for exactly that, and the shape of this module is
//! the reason there is no probe to write: there is nowhere here to make a
//! request from.
//!
//! # One lock, taken briefly
//!
//! A connection thread calls `SessionRouting::observe_exchange` after its
//! exchange is finished and its socket is closed, so the lock is never held
//! across I/O. The `Upstream` it may then switch is moved by a single atomic
//! store, and every connection thread reads its serving backend once at the
//! top of its own exchange — so a failover can never split one request
//! between two providers.

use std::sync::Mutex;
use std::time::Instant;

// `PairingOverrides` comes from `crate::config::pairing`'s own `pub use`, not
// from `crate::harness::pairing` directly — this module must never name
// `crate::harness` at all, see the module documentation above.
use crate::config::pairing::{
    NoObservations, ObservationSource, PairingOverrides, PairingPreference,
};
use crate::provider::telemetry::RateLimitHeaders;
use crate::routing::evidence::{
    EvidenceLedger, NewObservation, ObservedEvidenceSource, Outcome as RoutingOutcome,
};
use crate::routing::free::{FreePool, FreeResource, WorkloadOutcome};
use crate::routing::interactive::{
    Assignment, AssignmentChange, ChangeCause, FAILOVER_EVIDENCE_WINDOW_SECONDS, FailureResponse,
    InteractiveRouting, MigrationRefusal, Pin, ProviderFailure, RoutingRecord, SessionActivity,
    StayReason,
};
use crate::routing::{AssignedModel, Backend, CacheLocality};

use super::ingress::{Exchange, Outcome};
use super::upstream::Upstream;

/// Everything one gateway knows about which backend is serving it.
#[derive(Debug, Default)]
pub struct SessionRouting {
    state: Mutex<State>,
}

#[derive(Debug, Default)]
struct State {
    policy: InteractiveRouting,
    /// `None` until a launch profile binds one. A gateway can be bound and
    /// serving before any harness has been pointed at it, and claiming an
    /// assignment then would be recording a decision nobody made.
    assignment: Option<Assignment>,
    record: RoutingRecord,
    free: FreePool,
    /// The most recent rate-limit headers a forwarded response carried, and
    /// when — capability map line 1229's gateway half. `None` until at least
    /// one exchange has produced one. Not `Exchange`'s business — see
    /// `ingress`'s own doc comment on why that type stays incapable of
    /// carrying a header value; this is a second, separate observation.
    quota: Option<(RateLimitHeaders, i64)>,
    /// Phase 9J line 576: the user's configured native-pairing preference and
    /// corrections, as `crate::profile`'s gateway path resolved them. Held
    /// here rather than on `policy`, because `Self::pin_to_serving_provider`
    /// and `Self::unpin` replace `policy` wholesale, and a resolved
    /// preference must survive that replacement — see
    /// `Self::set_pairing_preference`. Defaults match
    /// `EffectiveConfig::native_pairing_preference`'s own out-of-the-box
    /// answer, so a gateway nothing has called `set_pairing_preference` on
    /// yet — every test double, and any future caller that forgets — scores
    /// exactly as `on_provider_failure` always has.
    pairing_preference: PairingPreference,
    pairing_overrides: PairingOverrides,
}

/// What one finished exchange said about the backend that served it.
///
/// Three separable facts, because they have different consequences: the
/// resource's health (Phase 9I line 529), whether the credential itself was
/// refused (line 537), and whether the **provider** failed in the sense
/// Phase 9H line 512 means.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Observation {
    workload: WorkloadOutcome,
    failure: Option<ProviderFailure>,
}

impl SessionRouting {
    pub fn new() -> Self {
        Self::default()
    }

    /// Phase 9H lines 505, 506 and 507: record which provider, model and
    /// credential are serving this harness's session, now that it is starting.
    ///
    /// Called by `crate::profile`'s gateway path, which is the only place
    /// that knows all three of the harness, the protocol chosen for it, and
    /// the model the launch profile named. The gateway itself knows none of
    /// them — it knows where to forward bytes.
    ///
    /// The assignment names the **serving** backend, which is the first
    /// configured one. Nothing here chooses; the choice was made when the
    /// upstream was built, and this records it.
    pub fn bind(&self, harness: &str, protocol: &str, model: AssignedModel, upstream: &Upstream) {
        let Some(backend) = upstream.serving().as_routing_backend(protocol, &model) else {
            // The serving backend has no route for the protocol the profile
            // resolved to. `apply_gateway` refuses that case before a child
            // exists, so reaching here means the two disagreed; recording an
            // assignment that names a route which does not exist would be
            // worse than recording none.
            return;
        };
        let mut state = self.lock();
        state.assignment = Some(state.policy.assign(harness, backend));
    }

    /// Phase 9J line 576, called beside [`Self::bind`]: record the
    /// native-pairing preference and corrections `crate::profile`'s gateway
    /// path resolved from configuration for this session, so
    /// `Self::observe_exchange`'s failover scores candidates against what
    /// the user actually configured instead of the out-of-the-box default
    /// [`InteractiveRouting::on_provider_failure`] used before this method
    /// existed.
    ///
    /// Kept separate from [`Self::bind`] rather than folded into it: `bind`
    /// answers only when a backend was actually resolved (its early `return`
    /// above), while a pairing preference is known whether or not that
    /// lookup succeeds, and a caller that skipped `bind` for a real reason
    /// should not lose its pairing configuration as a side effect.
    ///
    /// `preference_slug` is [`PairingPreference::slug`]'s own spelling, not
    /// the type itself — `crate::profile`, the only caller, may not import
    /// `crate::config` (see that module's own documentation), so it resolves
    /// the value and hands over the spelling. An unrecognised spelling
    /// degrades to [`PairingPreference::Strong`], the same out-of-the-box
    /// default `EffectiveConfig::native_pairing_preference` itself falls back
    /// to — this method never refuses a launch over a configuration value it
    /// cannot parse.
    pub fn set_pairing_preference(&self, preference_slug: &str, overrides: PairingOverrides) {
        let preference =
            PairingPreference::from_slug(preference_slug).unwrap_or(PairingPreference::Strong);
        let mut state = self.lock();
        state.pairing_preference = preference;
        state.pairing_overrides = overrides;
    }

    /// The backend serving this session, once one has been bound.
    pub fn assignment(&self) -> Option<Assignment> {
        self.lock().assignment.clone()
    }

    /// Phase 9H line 518: pin this session to the provider now serving it and
    /// turn automatic failover off.
    ///
    /// Returns the provider it pinned to, or `None` when nothing is bound
    /// yet — a pin before an assignment would name nothing.
    pub fn pin_to_serving_provider(&self) -> Option<String> {
        let mut state = self.lock();
        let provider = state.assignment.as_ref()?.provider().to_owned();
        state.policy = InteractiveRouting::pinned_to(provider.clone());
        Some(provider)
    }

    /// Lift a pin.
    pub fn unpin(&self) {
        let mut state = self.lock();
        state.policy = InteractiveRouting::new();
    }

    /// Whether, and where, this session is pinned.
    pub fn pin(&self) -> Pin {
        self.lock().policy.pin().clone()
    }

    /// Phase 9H line 511: move this session to another backend, explicitly,
    /// at a task boundary.
    ///
    /// The refusals are the policy's, not this module's. What happens here is
    /// only the consequence: the upstream is switched and the change is
    /// recorded with its cache locality.
    pub fn migrate(
        &self,
        to: &Backend,
        activity: SessionActivity,
        upstream: &Upstream,
    ) -> Result<Assignment, MigrationRefusal> {
        let mut state = self.lock();
        let Some(current) = state.assignment.clone() else {
            return Err(MigrationRefusal::MidTurn);
        };
        let migrated = state.policy.migrate(&current, to.clone(), activity)?;
        if !upstream.switch_to(migrated.backend().credential()) {
            // The caller offered a backend this gateway does not hold. A
            // refusal rather than a silent no-op: the session would otherwise
            // believe it had moved.
            return Err(MigrationRefusal::Incompatible(
                crate::routing::interactive::Incompatibility::Protocol {
                    provider: migrated.provider().to_owned(),
                    speaks: migrated.protocol().to_owned(),
                    needed: current.protocol().to_owned(),
                },
            ));
        }
        let cache = CacheLocality::between(current.backend(), migrated.backend());
        state.record.note(AssignmentChange {
            from: current,
            to: migrated.clone(),
            cause: ChangeCause::Migration,
            cache,
        });
        state.assignment = Some(migrated.clone());
        Ok(migrated)
    }

    /// Phase 9H line 515: every change of the backend serving this session,
    /// in order.
    pub fn changes(&self) -> Vec<AssignmentChange> {
        self.lock().record.entries().to_vec()
    }

    /// What has been learned about each free resource, from real work only.
    pub fn free_pool(&self) -> FreePool {
        self.lock().free.clone()
    }

    /// Every resource's health this gateway has observed for `provider`, as
    /// [`crate::provider::telemetry::GatewayHealthReading`]s ready to cross
    /// the process boundary — capability map lines 1311/1321/1322/1324's
    /// gateway half, called once per exchange from the accept loop,
    /// symmetric with [`Self::observe_quota_headers`] rather than folded into
    /// [`Self::observe_exchange`] itself.
    ///
    /// `now` and `now_unix` name the same instant in the two clocks this
    /// crosses between: `now` is what [`FreePool::observe`] measured
    /// `cooling_down_until` against, and `now_unix` is the wall-clock second
    /// the caller is about to persist this snapshot under. The remaining
    /// duration between `cooling_down_until` and `now` is added to
    /// `now_unix` — never `cooling_down_until` compared against `now_unix`
    /// directly, which would mix an [`std::time::Instant`], with no fixed
    /// epoch, into unix-second arithmetic.
    pub(super) fn health_readings_for(
        &self,
        provider: &str,
        now: Instant,
        now_unix: i64,
    ) -> Vec<crate::provider::telemetry::GatewayHealthReading> {
        self.free_pool()
            .observed()
            .into_iter()
            .filter(|(resource, _)| resource.provider() == provider)
            .map(
                |(resource, health)| crate::provider::telemetry::GatewayHealthReading {
                    credential_label: resource.credential().label(),
                    model: resource.model().to_owned(),
                    consecutive_failures: health.consecutive_failures(),
                    cooling_down_until_unix: health.cooling_down_until().map(|until| {
                        now_unix + until.saturating_duration_since(now).as_secs() as i64
                    }),
                    credential_rejected: health.credential_was_rejected(),
                },
            )
            .collect()
    }

    /// Record what a forwarded response's headers said — capability map line
    /// 1229's gateway half, called once per exchange from the accept loop.
    ///
    /// A no-op when `headers` is empty, which is the ordinary case: most
    /// exchanges forward a response that carries no rate-limit header this
    /// reader understands, and a no-op leaves whatever the last real reading
    /// was in place rather than clearing it.
    pub(super) fn observe_quota_headers(&self, headers: RateLimitHeaders, observed_at_unix: i64) {
        if headers.is_empty() {
            return;
        }
        self.lock().quota = Some((headers, observed_at_unix));
    }

    /// The most recent rate-limit headers observed, and when — capability map
    /// line 1229's gateway half, read by [`super::Gateway::quota_headers`].
    pub fn quota_headers(&self) -> Option<(RateLimitHeaders, i64)> {
        self.lock().quota.clone()
    }

    /// Phase 33A's one production producer this round: turn one finished
    /// exchange into a [`crate::routing::evidence::RoutingObservation`], when
    /// there is enough to say. See `crate::routing::evidence`'s own module
    /// documentation for exactly which fields this can and cannot supply and
    /// why — this method is simply where that honest subset gets built.
    ///
    /// Two conditions must both hold before anything is recorded, mirroring
    /// [`classify`]'s own filter:
    ///
    /// - the exchange must have reached the provider (`Forwarded` or
    ///   `Unreachable` — the same two outcomes [`Self::observe_exchange`]
    ///   treats as saying something about the backend), because nothing else
    ///   is a measurable turn;
    /// - `assignment` must be `Some`, because a provider/model identity
    ///   recorded for an unbound session would be invented rather than
    ///   observed.
    ///
    /// `assignment` is a snapshot the caller took **at dispatch**, not a
    /// read of whatever [`Self::bind`] or a failover has since made current.
    /// A connection thread only reaches this call after the exchange is
    /// already on the wire, and reading `self.lock().assignment` at that
    /// point would attribute the exchange to a bind or re-bind that happened
    /// *during* it rather than the one that actually served it — the
    /// defect this parameter exists to close. So its absence means "there
    /// was no assignment when this exchange was served", not "there is
    /// none now".
    ///
    /// `dispatched_at_unix` and `completed_at_unix` come from the accept
    /// loop, the only place in this partition with a timestamp on both sides
    /// of `ingress::serve`.
    pub(super) fn record_routing_observation(
        &self,
        ledger: &EvidenceLedger,
        exchange: &Exchange,
        dispatched_at_unix: i64,
        completed_at_unix: i64,
        assignment: Option<Assignment>,
    ) {
        let outcome = match &exchange.outcome {
            Outcome::Forwarded {
                upstream_status, ..
            } => Some(if (200..400).contains(upstream_status) {
                RoutingOutcome::Succeeded
            } else {
                RoutingOutcome::Failed
            }),
            Outcome::Unreachable { .. } => Some(RoutingOutcome::Failed),
            Outcome::Unauthenticated
            | Outcome::Declined
            | Outcome::Unrouted
            | Outcome::ClientGone
            | Outcome::Idle => None,
        };
        let Some(outcome) = outcome else {
            return;
        };

        let Some(assignment) = assignment else {
            return;
        };

        let new = NewObservation::new(
            exchange.provider.clone(),
            assignment.backend().model().label().to_owned(),
        )
        .with_route(exchange.protocol.clone())
        .with_harness(Some(assignment.harness().to_owned()))
        .with_quota_context(Some(assignment.backend().credential().label()))
        .with_timing(Some(dispatched_at_unix), Some(completed_at_unix))
        .with_outcome(outcome);

        // Best-effort, exactly like `observe_quota_headers`'s own write to
        // `GatewayQuotaCache`: the accept loop cannot fail a real session's
        // exchange over a full disk or a locked database.
        if let Err(err) = ledger.record(new, completed_at_unix) {
            tracing::debug!(
                error = %err,
                "could not record a routing observation"
            );
        }
    }

    /// Fold in one finished exchange: update the resource's health, and, when
    /// it was a real provider failure, ask the policy what to do about it.
    ///
    /// This is the production feed for Phase 9H lines 512 to 517 and Phase 9I
    /// lines 529, 534, 535, 537 and 538. It is called once per connection,
    /// after the exchange is over.
    ///
    /// `ledger` and `now_unix` feed Phase 9J's native-pairing prior and Phase
    /// 33A's local evidence into the one ranking decision this build makes
    /// (`InteractiveRouting::on_provider_failure`) — the same
    /// [`EvidenceLedger`] and completion timestamp
    /// [`Self::record_routing_observation`] is given, so a failover reads the
    /// very observations this gateway's own accept loop wrote. `None`
    /// reproduces this policy's pre-batch-46 behaviour exactly (see
    /// [`crate::routing::interactive::InteractiveRouting::on_provider_failure`]'s
    /// own doc): with nothing to weigh, every survivor ties and the first one
    /// found wins, the same as before this package.
    pub(super) fn observe_exchange(
        &self,
        upstream: &Upstream,
        exchange: &Exchange,
        now: Instant,
        ledger: Option<&EvidenceLedger>,
        now_unix: i64,
    ) {
        let Some(observation) = classify(exchange) else {
            // Nothing reached the provider — an unauthenticated caller, a
            // malformed head, a target belonging to no protocol. Recording
            // health for a request the provider never saw would be inventing
            // a signal.
            return;
        };

        let mut state = self.lock();
        let Some(current) = state.assignment.clone() else {
            return;
        };
        let credential = current.backend().credential().clone();
        let model = model_key(current.backend().model());

        // Phase 9I lines 529 and 538: health per credential and model, from
        // real workload.
        state.free.observe(
            &FreeResource::new(credential.clone(), model.clone()),
            observation.workload,
            now,
        );

        // Phase 9I line 537: a credential that was refused or is out of
        // requests is *that credential's* problem. Try this provider's other
        // keys before concluding anything about the provider.
        if matches!(
            observation.workload,
            WorkloadOutcome::CredentialRejected | WorkloadOutcome::RateLimited { .. }
        ) {
            let siblings = upstream.credentials_of(current.provider());
            if let Some(next) = state.free.rotate_from(&credential, &siblings, &model, now)
                && let Some(backend) =
                    upstream.backend_for(&next, current.protocol(), current.backend().model())
                && upstream.switch_to(&next)
            {
                let to = Assignment::new(current.harness(), backend);
                let cache = CacheLocality::between(current.backend(), to.backend());
                state.record.note(AssignmentChange {
                    from: current,
                    to: to.clone(),
                    cause: ChangeCause::CredentialRotation,
                    cache,
                });
                state.assignment = Some(to);
                return;
            }
        }

        let Some(failure) = observation.failure else {
            return;
        };

        let candidates =
            upstream.failover_candidates(current.protocol(), current.backend().model());

        // Phase 9J and Phase 33A's one production consumer. `NoObservations`
        // when no durable ledger was ever handed to this gateway — the same
        // additive shape `record_routing_observation`'s own `ledger: &
        // EvidenceLedger` parameter follows, so a build with no telemetry
        // configured behaves exactly as it did before this package.
        let no_observations = NoObservations;
        let observed_source;
        let evidence: &dyn ObservationSource = match ledger {
            Some(ledger) => {
                observed_source =
                    ObservedEvidenceSource::new(ledger, now_unix, FAILOVER_EVIDENCE_WINDOW_SECONDS);
                &observed_source
            }
            None => &no_observations,
        };

        match state.policy.on_provider_failure(
            &current,
            failure,
            &candidates,
            state.pairing_preference,
            &state.pairing_overrides,
            evidence,
        ) {
            FailureResponse::FailOver {
                to,
                cache,
                explanation,
            } => {
                if upstream.switch_to(to.backend().credential()) {
                    tracing::debug!(
                        harness = %current.harness(),
                        from = %current.label(),
                        to = %to.label(),
                        explanation = %explanation.render(),
                        "the native-pairing prior and local evidence behind a Glasshouse gateway \
                         failover"
                    );
                    state.record.note(AssignmentChange {
                        from: current,
                        to: to.clone(),
                        cause: ChangeCause::Failover(failure),
                        cache,
                    });
                    state.assignment = Some(to);
                }
            }
            FailureResponse::OfferMigration {
                to,
                cache,
                explanation,
            } => {
                // Phase 9H line 514: a material model change is not taken.
                // Said out loud, because an offer nobody hears is a decision
                // made by silence.
                tracing::info!(
                    harness = %current.harness(),
                    from = %current.label(),
                    offered = %to.label(),
                    cache = %cache,
                    explanation = %explanation.render(),
                    "a Glasshouse gateway backend failed and the only compatible replacement \
                     serves a different model, which is a migration rather than a failover"
                );
            }
            FailureResponse::Stay { reason } => {
                let detail = match &reason {
                    StayReason::SessionPinned { provider } => {
                        format!("the session is pinned to `{provider}`")
                    }
                    StayReason::NoCompatibleBackend { rejected } => {
                        if rejected.is_empty() {
                            "no other backend is configured".to_owned()
                        } else {
                            rejected
                                .iter()
                                .map(|why| why.to_string())
                                .collect::<Vec<_>>()
                                .join("; ")
                        }
                    }
                };
                tracing::info!(
                    harness = %current.harness(),
                    backend = %current.label(),
                    failure = %failure.describe(),
                    detail,
                    "a Glasshouse gateway backend failed and the session stayed where it was"
                );
            }
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        // A poisoned lock is treated as ownership rather than as a reason to
        // give up — the same decision `shutdown`'s bounded retry made, and for
        // the same reason: refusing to route because another thread panicked
        // would turn one failure into every session's failure.
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

/// A stable key for the model an assignment names, including the honest case
/// where the harness chose it.
///
/// [`AssignedModel::HarnessDefault`]'s label is used as the key rather than an
/// empty string: health has to be tracked for it like any other resource, and
/// an empty key would silently share one entry with anything else that had
/// none.
fn model_key(model: &AssignedModel) -> String {
    model.label().to_owned()
}

/// What one exchange says, or `None` when it never reached the provider.
///
/// The one place an HTTP status becomes a routing fact. Phase 9H line 512
/// wants a *real provider failure*, and three of these numbers are not one:
///
/// - `401` and `403` are about the credential, and Phase 9I line 537 answers
///   them by rotating keys within the provider;
/// - any other `4xx` is the harness's own request being wrong, and the
///   provider that answered it is healthy — sending the same malformed
///   request somewhere else would fail there too.
fn classify(exchange: &Exchange) -> Option<Observation> {
    match &exchange.outcome {
        Outcome::Forwarded {
            upstream_status, ..
        } => Some(match *upstream_status {
            // `402` is here because a real run put it here. Claude Code was
            // driven through this gateway to OpenRouter on 2026-08-26 for a
            // model OpenRouter itself lists as `:free`, and the answer was
            // `402 Insufficient credits — this account never purchased
            // credits`. That is not a provider outage and it is not a
            // malformed request: it is this **account's** key being unable to
            // pay, which is the same class of fact as a revoked one. Another
            // key on another account would serve. So it rotates like `401`
            // and `403` rather than failing the provider over, and waiting
            // does not fix it — which is why `CredentialRejected` is not a
            // cooldown.
            401..=403 => Observation {
                workload: WorkloadOutcome::CredentialRejected,
                failure: None,
            },
            429 => Observation {
                workload: WorkloadOutcome::RateLimited {
                    // Left `None` here, not because headers are unreadable —
                    // `ingress::forward` now reads this allowlist for
                    // capability map line 1229's gateway half — but because
                    // wiring `retry-after` into a routing decision is Phase
                    // 9H/9I's own scope and outside this package's
                    // partition. Without one, the free pool's own bounded
                    // backoff applies.
                    retry_after: None,
                },
                failure: ProviderFailure::from_status(429),
            },
            status @ 500..=599 => Observation {
                workload: WorkloadOutcome::CapacityFailure,
                failure: ProviderFailure::from_status(status),
            },
            // Everything else: the provider answered, so it is healthy, and
            // whether that answer is a *provider failure* is
            // `ProviderFailure::from_status`'s question and not a second
            // reading of the same number written here. Hard-coding `None`
            // was the first version, and a mutation of `from_status` proved
            // it: widening that function to call a `400` a provider failure
            // changed nothing, because this arm had already decided. Two
            // copies of one rule is exactly the shape that lets them drift.
            status => Observation {
                workload: WorkloadOutcome::Served,
                failure: ProviderFailure::from_status(status),
            },
        }),
        Outcome::Unreachable { .. } => Some(Observation {
            workload: WorkloadOutcome::CapacityFailure,
            failure: Some(ProviderFailure::Unreachable),
        }),
        // None of these reached the provider, so none of them says anything
        // about it.
        Outcome::Unauthenticated
        | Outcome::Declined
        | Outcome::Unrouted
        | Outcome::ClientGone
        | Outcome::Idle => None,
    }
}

/// Map line 1735: whether one finished exchange says the gateway's own
/// upstream failed, separately from anything about the harness process that
/// sent the request — and separately from [`classify`]'s question, which is
/// "does this session need to move" rather than "is the resource itself
/// unhealthy". Called on every exchange, whether or not a session is bound —
/// a gateway can be serving before any harness has been pointed at it, and
/// the resource can still be unreachable.
///
/// Only [`Outcome::Unreachable`] qualifies. A `Forwarded` exchange reached
/// the provider and got an answer — even a `4xx` or `5xx` one — which is the
/// provider or the request being wrong, not the gateway failing to reach it;
/// mapping that to a gateway failure would be exactly the invented signal the
/// packet forbids ("a `Forwarded` exchange that merely returned an
/// application-level error the gateway passed through is not a gateway
/// failure"). Every other outcome (`Unauthenticated`, `Declined`,
/// `Unrouted`, `ClientGone`, `Idle`) never reached the provider at all, for
/// reasons that have nothing to do with the provider's health.
///
/// [`crate::events::GatewayFailure::TimedOut`] and
/// [`crate::events::GatewayFailure::Rejected`] are never produced here:
/// `ingress::Outcome` has no production path that distinguishes either from
/// a plain `Unreachable` today — `Outcome::Unreachable`'s own `detail` is a
/// diagnostic phrase for `tracing`, not a second, finer-grained outcome, and
/// re-deriving one by matching on that text would be reading `ingress`'s
/// output more closely than `ingress`'s own module documentation allows this
/// directory to. `ingress.rs` is this package's `FORBIDDEN FILES`.
pub(super) fn gateway_failure(exchange: &Exchange) -> Option<crate::events::GatewayFailure> {
    match &exchange.outcome {
        Outcome::Unreachable { .. } => Some(crate::events::GatewayFailure::Unreachable),
        Outcome::Forwarded { .. }
        | Outcome::Unauthenticated
        | Outcome::Declined
        | Outcome::Unrouted
        | Outcome::ClientGone
        | Outcome::Idle => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::upstream::{Route, UpstreamBackend};
    use crate::routing::evidence::NewObservation;
    use crate::routing::interactive::RoutingBenefit;
    use crate::routing::{Cost, CredentialId};
    use crate::secret::{Secret, SecretRef};
    use crate::{Cli, Runtime};
    use clap::Parser;

    /// A real project database plus an [`EvidenceLedger`] opened on it — the
    /// same [`crate::bootstrap`] door every other store's own tests use, so a
    /// read here is proven against the real schema rather than a stand-in.
    fn ledger_fixture(base: &std::path::Path) -> EvidenceLedger {
        let root = base.join("workspace").join("proj");
        std::fs::create_dir_all(root.join(".git")).unwrap();
        let root = std::fs::canonicalize(&root).unwrap();
        let cli = Cli::try_parse_from([
            "glasshouse",
            "--data-dir",
            base.join("data").to_str().unwrap(),
            "--config-dir",
            base.join("config").to_str().unwrap(),
        ])
        .unwrap();
        let runtime: Runtime = crate::bootstrap(&cli, &root).unwrap();
        EvidenceLedger::open(&runtime).unwrap()
    }

    fn upstream_backend(name: &str) -> UpstreamBackend {
        UpstreamBackend::new(
            name.to_owned(),
            vec![Route::new(
                "anthropic-messages".to_owned(),
                &["/messages"],
                "http://127.0.0.1:1",
            )],
            Secret::mint_for_test("test-secret"),
            CredentialId::new(
                name,
                SecretRef::Environment {
                    var: format!("{}_API_KEY", name.to_uppercase()),
                },
            ),
            Cost::Metered,
        )
        .expect("a loopback http URL is absolute and this credential is header-safe")
    }

    fn unreachable_exchange(provider: &str) -> Exchange {
        Exchange {
            outcome: Outcome::Unreachable {
                detail: "connection refused",
            },
            status: 502,
            provider: provider.to_owned(),
            protocol: Some("anthropic-messages".to_owned()),
            host: String::new(),
        }
    }

    /// A second credential for `provider`, so a test can put two backends on
    /// one provider — Phase 9I line 537's rotation candidate.
    fn upstream_backend_with_credential(provider: &str, var: &str) -> UpstreamBackend {
        UpstreamBackend::new(
            provider.to_owned(),
            vec![Route::new(
                "anthropic-messages".to_owned(),
                &["/messages"],
                "http://127.0.0.1:1",
            )],
            Secret::mint_for_test("test-secret"),
            CredentialId::new(
                provider,
                SecretRef::Environment {
                    var: var.to_owned(),
                },
            ),
            Cost::Metered,
        )
        .expect("a loopback http URL is absolute and this credential is header-safe")
    }

    fn rate_limited_exchange(provider: &str) -> Exchange {
        Exchange {
            outcome: Outcome::Forwarded {
                upstream_status: 429,
                bytes: 0,
            },
            status: 429,
            provider: provider.to_owned(),
            protocol: Some("anthropic-messages".to_owned()),
            host: String::new(),
        }
    }

    /// The §36 proof for this package's own wiring, not
    /// `InteractiveRouting::on_provider_failure`'s (see
    /// `routing::interactive::tests` for that one): this drives a **real**
    /// [`EvidenceLedger`] through [`SessionRouting::observe_exchange`] itself,
    /// the function `gateway/mod.rs`'s accept loop actually calls, rather than
    /// through the pure policy function directly. Mutating this method's
    /// `Some(ledger) => ...` arm back to always using `NoObservations` fails
    /// this test, because it is the only one that supplies a ledger here at
    /// all.
    #[test]
    fn observe_exchange_ranks_a_real_failover_by_the_ledger_it_was_given() {
        let tmp = tempfile::tempdir().unwrap();
        let ledger = ledger_fixture(tmp.path());
        let now_unix = 1_800_000_000_i64;

        for _ in 0..5 {
            ledger
                .record(
                    NewObservation::new("poor-evidence", "the-routed-model")
                        .with_route(Some("anthropic-messages"))
                        .with_harness(Some("claude-code"))
                        .with_outcome(RoutingOutcome::Failed),
                    now_unix,
                )
                .unwrap();
            ledger
                .record(
                    NewObservation::new("good-evidence", "the-routed-model")
                        .with_route(Some("anthropic-messages"))
                        .with_harness(Some("claude-code"))
                        .with_outcome(RoutingOutcome::Succeeded),
                    now_unix,
                )
                .unwrap();
        }

        let upstream = Upstream::with_failover(vec![
            upstream_backend("first"),
            upstream_backend("poor-evidence"),
            upstream_backend("good-evidence"),
        ])
        .expect("three backends is not none");

        let routing = SessionRouting::new();
        routing.bind(
            "claude-code",
            "anthropic-messages",
            AssignedModel::named("the-routed-model"),
            &upstream,
        );
        assert_eq!(
            routing.assignment().map(|a| a.provider().to_owned()),
            Some("first".to_owned())
        );

        routing.observe_exchange(
            &upstream,
            &unreachable_exchange("first"),
            Instant::now(),
            Some(&ledger),
            now_unix,
        );

        assert_eq!(
            routing.assignment().map(|a| a.provider().to_owned()),
            Some("good-evidence".to_owned()),
            "the candidate with strong recorded evidence must win the real failover, not \
             `poor-evidence`, which is configured first among the two survivors"
        );
    }

    /// The same failover with no ledger at all reproduces the pre-batch-46
    /// behaviour: the first compatible survivor in configuration order wins.
    #[test]
    fn observe_exchange_falls_back_to_configuration_order_with_no_ledger() {
        let upstream = Upstream::with_failover(vec![
            upstream_backend("first"),
            upstream_backend("second"),
            upstream_backend("third"),
        ])
        .expect("three backends is not none");

        let routing = SessionRouting::new();
        routing.bind(
            "claude-code",
            "anthropic-messages",
            AssignedModel::named("the-routed-model"),
            &upstream,
        );

        routing.observe_exchange(
            &upstream,
            &unreachable_exchange("first"),
            Instant::now(),
            None,
            0,
        );

        assert_eq!(
            routing.assignment().map(|a| a.provider().to_owned()),
            Some("second".to_owned())
        );
    }

    /// The rendered explanation `Self::observe_exchange` logs for a real
    /// failover, captured the way `gateway::ingress::tests::recorded` reads
    /// `Exchange::record`'s own log line — through the exact `tracing` call
    /// site the accept loop's own build would emit from, not a value handed
    /// back for a test to inspect.
    fn failover_explanation_log(preference_slug: &str) -> String {
        use std::sync::{Arc, Mutex};

        #[derive(Clone)]
        struct Capture(Arc<Mutex<Vec<u8>>>);

        impl std::io::Write for Capture {
            fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
                self.0
                    .lock()
                    .expect("no test panics while holding this")
                    .extend_from_slice(buf);
                Ok(buf.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }

        impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for Capture {
            type Writer = Capture;
            fn make_writer(&'a self) -> Capture {
                self.clone()
            }
        }

        // A vendor-native pairing for `claude-code` — the same fixture
        // `routing::interactive::tests::a_failover_explanation_names_the_pairing_class_it_scored`
        // uses — so the native-pairing prior actually has a nonzero
        // magnitude to vary under `Strong` and zero it under `Off`.
        let upstream = Upstream::with_failover(vec![
            upstream_backend("openrouter"),
            upstream_backend("nous"),
        ])
        .expect("two backends is not none");
        let routing = SessionRouting::new();
        routing.bind(
            "claude-code",
            "anthropic-messages",
            AssignedModel::named("claude-fable-5"),
            &upstream,
        );
        routing.set_pairing_preference(preference_slug, PairingOverrides::default());

        let sink = Arc::new(Mutex::new(Vec::new()));
        let subscriber = tracing_subscriber::fmt()
            .with_writer(Capture(Arc::clone(&sink)))
            .with_max_level(tracing::Level::DEBUG)
            .without_time()
            .finish();
        tracing::subscriber::with_default(subscriber, || {
            routing.observe_exchange(
                &upstream,
                &unreachable_exchange("openrouter"),
                Instant::now(),
                None,
                0,
            );
        });

        let captured = sink
            .lock()
            .expect("no test panics while holding this")
            .clone();
        String::from_utf8_lossy(&captured).into_owned()
    }

    /// Phase 9J line 576's own proof for this package's wiring: what
    /// `Self::set_pairing_preference` was given reaches
    /// `Self::observe_exchange`'s real failover, not a hardcoded
    /// `PairingPreference::Strong`. If the call inside `observe_exchange`
    /// still passed a literal `PairingPreference::Strong` instead of
    /// `state.pairing_preference`, both log lines below would read
    /// `+1.000  native-pairing prior` and this test would fail on the second
    /// assertion.
    #[test]
    fn observe_exchange_scores_a_real_failover_against_the_configured_preference() {
        let strong = failover_explanation_log("strong");
        let off = failover_explanation_log("off");

        assert!(
            strong.contains("+1.000  native-pairing prior"),
            "a Strong preference on a real vendor-native pairing must log a full-magnitude \
             prior: {strong}"
        );
        assert!(
            off.contains("+0.000  native-pairing prior"),
            "an Off preference must log a zeroed prior for the very same pairing: {off}"
        );
    }

    /// Acceptance test 4, through the real production caller (§35/§36): a
    /// single `429` on one credential rotates this session to the same
    /// provider's other credential — Phase 9I line 537's existing behaviour
    /// — and the recorded change must say honestly that this bought a
    /// different queue onto the same upstream, never "independent failure
    /// handling", per line 1372's inference ban.
    #[test]
    fn observe_exchange_records_a_credential_rotation_as_a_different_queue_not_independent_failure_handling()
     {
        let upstream = Upstream::with_failover(vec![
            upstream_backend_with_credential("openrouter", "OPENROUTER_API_KEY"),
            upstream_backend_with_credential("openrouter", "OPENROUTER_API_KEY_2"),
        ])
        .expect("two backends is not none");

        let routing = SessionRouting::new();
        routing.bind(
            "claude-code",
            "anthropic-messages",
            AssignedModel::named("the-routed-model"),
            &upstream,
        );

        routing.observe_exchange(
            &upstream,
            &rate_limited_exchange("openrouter"),
            Instant::now(),
            None,
            0,
        );

        let changes = routing.changes();
        let entry = changes.last().expect("a rotation must have been recorded");
        assert_eq!(entry.cause, ChangeCause::CredentialRotation);

        let benefit = entry.benefit();
        assert_eq!(
            benefit,
            RoutingBenefit::DifferentQueueSameUpstream,
            "a same-provider credential rotation must record a different queue onto the same \
             upstream, not {benefit:?}, which is what implying resilience was gained would look \
             like"
        );
        let rendered = benefit.as_str();
        assert!(
            rendered.contains("different queue onto the same upstream"),
            "a same-provider credential rotation must record the honest reason rather than \
             implying resilience was gained: {rendered}"
        );
        assert_ne!(
            benefit,
            RoutingBenefit::UnconfirmedFailureDomainChange,
            "a credential rotation must never be recorded as an (even unconfirmed) failure-domain \
             change — the failure domain did not move"
        );
    }

    // --- capability map lines 1311/1321/1322/1324: the health snapshot ----

    /// A provider [`Self::health_readings_for`] was never asked about, and a
    /// provider it was asked about but that never served an exchange, both
    /// come back empty — never a fabricated entry for a resource nothing was
    /// observed about.
    #[test]
    fn health_readings_for_an_unobserved_provider_is_empty() {
        let routing = SessionRouting::new();
        assert_eq!(
            routing.health_readings_for("anyrouter", Instant::now(), 1_800_000_000),
            Vec::new()
        );
    }

    /// Two consecutive rate-limit failures — `routing::free`'s own
    /// `FAILURES_BEFORE_COOLDOWN` threshold, exercised through the real
    /// production caller [`Self::observe_exchange`] rather than
    /// `routing::free::ResourceHealth` directly — must reach
    /// [`Self::health_readings_for`] as a cooldown converted to an absolute
    /// unix second, and must never leak into a different provider's
    /// snapshot.
    #[test]
    fn health_readings_for_reports_a_real_cooldown_as_an_absolute_unix_deadline_and_only_for_its_own_provider()
     {
        let upstream = Upstream::with_failover(vec![upstream_backend("openrouter")])
            .expect("one backend is not none");
        let routing = SessionRouting::new();
        routing.bind(
            "claude-code",
            "anthropic-messages",
            AssignedModel::named("the-routed-model"),
            &upstream,
        );

        let now = Instant::now();
        let now_unix = 1_800_000_000_i64;
        routing.observe_exchange(
            &upstream,
            &rate_limited_exchange("openrouter"),
            now,
            None,
            0,
        );
        routing.observe_exchange(
            &upstream,
            &rate_limited_exchange("openrouter"),
            now,
            None,
            0,
        );

        let readings = routing.health_readings_for("openrouter", now, now_unix);
        let reading = readings
            .iter()
            .find(|r| r.model == "the-routed-model")
            .expect("the bound model must have a health reading after two real failures");
        assert_eq!(reading.consecutive_failures, 2);
        assert!(!reading.credential_rejected);
        let until = reading
            .cooling_down_until_unix
            .expect("two consecutive rate-limit failures must trigger a cooldown");
        assert!(
            until > now_unix,
            "a fresh cooldown must read as a deadline still in the future: {until} vs {now_unix}"
        );

        assert_eq!(
            routing.health_readings_for("a-different-provider", now, now_unix),
            Vec::new(),
            "a provider's own snapshot must never include another provider's resource"
        );
    }

    /// A resource that served after failing is healthy again — Phase 9I line
    /// 534's recovery-from-work half — and [`Self::health_readings_for`]
    /// reports that as no cooldown at all, not as a deadline already in the
    /// past.
    #[test]
    fn health_readings_for_clears_a_cooldown_once_the_resource_serves_again() {
        let upstream = Upstream::with_failover(vec![upstream_backend("openrouter")])
            .expect("one backend is not none");
        let routing = SessionRouting::new();
        routing.bind(
            "claude-code",
            "anthropic-messages",
            AssignedModel::named("the-routed-model"),
            &upstream,
        );

        let now = Instant::now();
        routing.observe_exchange(
            &upstream,
            &rate_limited_exchange("openrouter"),
            now,
            None,
            0,
        );
        routing.observe_exchange(
            &upstream,
            &rate_limited_exchange("openrouter"),
            now,
            None,
            0,
        );
        routing.observe_exchange(
            &upstream,
            &Exchange {
                outcome: Outcome::Forwarded {
                    upstream_status: 200,
                    bytes: 0,
                },
                status: 200,
                provider: "openrouter".to_owned(),
                protocol: Some("anthropic-messages".to_owned()),
                host: String::new(),
            },
            now,
            None,
            0,
        );

        let readings = routing.health_readings_for("openrouter", now, 1_800_000_000);
        let reading = readings
            .iter()
            .find(|r| r.model == "the-routed-model")
            .expect("the bound model must still have a health reading");
        assert_eq!(reading.consecutive_failures, 0);
        assert_eq!(reading.cooling_down_until_unix, None);
    }
}
