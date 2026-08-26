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

use crate::routing::free::{FreePool, FreeResource, WorkloadOutcome};
use crate::routing::interactive::{
    Assignment, AssignmentChange, ChangeCause, FailureResponse, InteractiveRouting,
    MigrationRefusal, Pin, ProviderFailure, RoutingRecord, SessionActivity, StayReason,
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

    /// Fold in one finished exchange: update the resource's health, and, when
    /// it was a real provider failure, ask the policy what to do about it.
    ///
    /// This is the production feed for Phase 9H lines 512 to 517 and Phase 9I
    /// lines 529, 534, 535, 537 and 538. It is called once per connection,
    /// after the exchange is over.
    pub(super) fn observe_exchange(&self, upstream: &Upstream, exchange: &Exchange, now: Instant) {
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
        match state
            .policy
            .on_provider_failure(&current, failure, &candidates)
        {
            FailureResponse::FailOver { to, cache } => {
                if upstream.switch_to(to.backend().credential()) {
                    state.record.note(AssignmentChange {
                        from: current,
                        to: to.clone(),
                        cause: ChangeCause::Failover(failure),
                        cache,
                    });
                    state.assignment = Some(to);
                }
            }
            FailureResponse::OfferMigration { to, cache } => {
                // Phase 9H line 514: a material model change is not taken.
                // Said out loud, because an offer nobody hears is a decision
                // made by silence.
                tracing::info!(
                    harness = %current.harness(),
                    from = %current.label(),
                    offered = %to.label(),
                    cache = %cache,
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
                    // The provider's own `retry-after` is not read here: the
                    // gateway forwards headers without parsing them, and
                    // adding a parser to this path would make it a reader of
                    // the response it exists to pass through. Without one,
                    // the free pool's own bounded backoff applies.
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
