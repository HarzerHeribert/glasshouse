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

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

// `PairingOverrides` comes from `crate::config::pairing`'s own `pub use`, not
// from `crate::harness::pairing` directly — this module must never name
// `crate::harness` at all, see the module documentation above.
use crate::config::pairing::{
    NoObservations, ObservationSource, PairingOverrides, PairingPreference,
};
use crate::provider::telemetry::RateLimitHeaders;
use crate::routing::evidence::{
    EvidenceLedger, FailureClass, HARNESS_TURN_PURPOSE, NewObservation, ObservedEvidenceSource,
    Outcome as RoutingOutcome, RouteCorrelations,
};
use crate::routing::free::{FreePool, FreeResource, WorkloadOutcome};
use crate::routing::interactive::{
    Assignment, AssignmentChange, ChangeCause, FAILOVER_EVIDENCE_WINDOW_SECONDS, FailureResponse,
    InteractiveRouting, MigrationRefusal, Pin, ProviderFailure, RoutingRecord, SessionActivity,
    StayReason,
};
use crate::routing::request::TaskClass;
use crate::routing::{AssignedModel, Backend, CacheLocality};

use super::ingress::{Exchange, Framing, Outcome, StreamEnd, TRANSPORT_TIMEOUT_DETAIL};
use super::upstream::Upstream;

/// Everything one gateway knows about which backend is serving it.
#[derive(Debug, Default)]
pub struct SessionRouting {
    state: Mutex<State>,
}

/// Told what the failure-domain term did to one failover's ranking, once per
/// failover the gateway takes — capability map line 1851's write side.
///
/// **A sink rather than a ledger handle**, exactly like
/// [`super::DegradeSink`] one module up and for practice §65's reason: the
/// gateway holds this for its whole life, and an open SQLite connection held
/// for the life of a session is free on the developer's machine and billed
/// on Windows. The sink's own body opens, writes and drops a handle at the
/// one moment a failover has actually been decided, which is a small
/// minority of exchanges and none of the ones that move nothing.
///
/// It also keeps this module incapable of reaching a database: nothing here
/// knows a project, a path or a `crate::Runtime`, and the only thing it can
/// do with a prevention is hand it to whoever asked for it.
pub type FailoverPreventionSink =
    Arc<dyn Fn(&crate::routing::interactive::FailureDomainEffect) + Send + Sync>;

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
    /// The Glasshouse session this gateway serves — `crate::database`
    /// migration 24's `routing_observations.session_id`. `None` until a
    /// launch tells it (see [`SessionRouting::serve_session`]), and a
    /// gateway nothing has told is a gateway serving no session: its rows
    /// say so with `NULL` rather than an invented id.
    ///
    /// A plain `String`, never `crate::session::SessionId`: this module may
    /// not name `crate::session` at all (see this file's own module
    /// documentation and `gateway::tests::
    /// the_gateway_imports_none_of_the_modules_that_would_make_it_a_harness`),
    /// so the id crosses into this state as its string and nothing else —
    /// [`SessionRouting::serve_session`]'s own doc says why.
    session_id: Option<String>,
    /// The task class the launch that started this gateway was routed as —
    /// capability map line 1301 and `crate::database` migration 23's
    /// `routing_observations.task_class`, this producer's missing join
    /// (`GH-TASK-CLASS-COST-JOIN`, `docs/product/evidence/phase-32g.md`'s
    /// Censused 2026-09-02 entry). `None` until a launch tells it (see
    /// [`SessionRouting::serve_task_class`]), the same honest absence
    /// [`Self::session_id`] carries for a gateway nothing has told.
    task_class: Option<TaskClass>,
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

/// What observing one exchange did to the session's assignment — returned
/// by [`SessionRouting::observe_exchange`] so the same connection thread can
/// write it onto the exchange's own evidence row, capability map line 1334's
/// `failovers`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ExchangeEffect {
    /// The assignment stands.
    Unchanged,
    /// Another of the same provider's credentials took over — Phase 9I line
    /// 537. **Not a failover**: the provider serving the session did not
    /// change, and the routing record keeps the two apart
    /// (`ChangeCause::CredentialRotation` beside `ChangeCause::Failover`),
    /// so this column does too.
    RotatedCredential,
    /// The session moved to another backend — Phase 9H line 512.
    FailedOver,
}

impl ExchangeEffect {
    /// Line 1334's `failovers` for the exchange this describes: `1` when it
    /// caused one, else `0`. A count rather than a flag because the column
    /// is one, and because a later producer that spans several exchanges may
    /// have more than one to report.
    pub(super) fn failovers(self) -> u32 {
        match self {
            Self::FailedOver => 1,
            Self::Unchanged | Self::RotatedCredential => 0,
        }
    }
}

/// What the accept loop knows about one finished exchange that [`Exchange`]
/// itself does not carry — handed to
/// [`SessionRouting::record_routing_observation`] as one value, because every
/// field is a fact about the same exchange and only the connection thread
/// that served it holds all of them at once.
pub(super) struct ExchangeReading<'a> {
    /// This response's own rate-limit headers, exactly as `ingress::serve`
    /// returned them. Read here for one purpose — capability map lines 1364
    /// and 1365's distinction between a cadence throttle and a spent quota,
    /// in [`failure_class`] — and for nothing else. This is not the
    /// narrowing [`stated_retry_after`] performs for a *routing decision*
    /// being undone: what is written from these headers is a class name,
    /// never a header value, and nothing here changes where a session is
    /// routed.
    pub(super) quota: &'a RateLimitHeaders,
    /// The instant the accept loop handed the connection to `ingress::serve`
    /// — an honest upper-bound proxy for dispatch, see
    /// `crate::routing::evidence`'s own header.
    pub(super) dispatched_at_unix: i64,
    /// The instant `ingress::serve` returned.
    pub(super) completed_at_unix: i64,
    /// The assignment as of dispatch — see the method's own doc for why this
    /// is a snapshot rather than a fresh read.
    pub(super) assignment: Option<Assignment>,
    /// What observing this same exchange did to the assignment —
    /// [`SessionRouting::observe_exchange`]'s own return, so the row can say
    /// whether *this* exchange caused a failover.
    pub(super) effect: ExchangeEffect,
}

impl SessionRouting {
    pub fn new() -> Self {
        Self::default()
    }

    /// Phase 9H lines 505, 506 and 507: record which provider, model and
    /// credential are serving this harness's session, now that it is starting.
    ///
    /// Called by `crate::profile`'s gateway path, which is the only place
    /// that knows all three of the harness, the protocol resolved for it,
    /// and the model the launch profile named. The gateway itself knows
    /// none of them — it knows where to forward bytes.
    ///
    /// `protocol` is the **served** protocol, not necessarily the one the
    /// harness itself speaks: `crate::profile::apply_gateway` (Phase 56,
    /// GH-GATEWAY-TRANSLATE-LAUNCH) hands this the pair table's `to` slug
    /// for a translated launch, so a served-but-not-native pairing binds a
    /// real route the same way a native one always has.
    ///
    /// The assignment names the **serving** backend, which is the first
    /// configured one. Nothing here chooses; the choice was made when the
    /// upstream was built, and this records it.
    pub fn bind(&self, harness: &str, protocol: &str, model: AssignedModel, upstream: &Upstream) {
        let Some(backend) = upstream.serving().as_routing_backend(protocol, &model) else {
            // The serving backend has no route for `protocol`. `apply_gateway`
            // refuses both the unserved and the table-refused case before a
            // child exists, so reaching here means the caller and this
            // backend disagreed about what the serving backend carries;
            // recording an assignment that names a route which does not
            // exist would be worse than recording none.
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

    /// Capability map line 2019, and `crate::database` migration 24: record
    /// which Glasshouse session this gateway is serving, so that every row
    /// this type's own `record_routing_observation` writes can name it.
    ///
    /// [`Self::set_pairing_preference`]'s shape, and separate from
    /// [`Self::bind`] for its reason: `bind` answers only when a backend
    /// actually resolved, while the session this gateway serves is known
    /// whether or not that lookup succeeded.
    ///
    /// # Told by the launch, not learned from the wire
    ///
    /// `main.rs`'s two launch doors call this — `launch_session` after
    /// `store.create` returns the record and before the harness is spawned,
    /// and `resolve_resume_overlay` for the record being resumed. A gateway
    /// is started once per launched session, so there is exactly one answer
    /// per gateway and nothing here ever changes it from the wire. In
    /// particular this is **not** derived from a request: the relay reads no
    /// body by construction (`super::ingress`'s own
    /// `an_exchange_has_nowhere_to_put_a_body`), and the harness's
    /// `metadata.user_id` names an account, not a Glasshouse session — see
    /// `docs/product/design-decisions.md`, *A session identity on the
    /// routing evidence rows*.
    ///
    /// `session_id` is a plain `&str`, not `crate::session::SessionId`: this
    /// module is structurally unable to see the session model at all (this
    /// file's own module documentation, and `gateway::tests::
    /// the_gateway_imports_none_of_the_modules_that_would_make_it_a_harness`
    /// enforces it with a source scan), so the id crosses this boundary as
    /// its string and nothing else — the caller's own typed id, narrowed at
    /// the one call.
    pub fn serve_session(&self, session_id: &str) {
        self.lock().session_id = Some(session_id.to_owned());
    }

    /// Capability map line 1301, and `crate::database` migration 23: record
    /// which task class the launch that started this gateway was routed as,
    /// so every row `record_routing_observation` writes can join to
    /// [`crate::routing::burn::output_tokens_by_class`]'s reader the way
    /// `record_routing_latency`'s own row already does.
    ///
    /// [`Self::serve_session`]'s shape exactly: `None` for a launch with no
    /// routing decision — `main.rs::launch_session` passes nothing when
    /// routing was off or no task was classified — and a gateway nothing has
    /// told stamps `NULL`, never an invented class.
    pub fn serve_task_class(&self, task_class: Option<TaskClass>) {
        self.lock().task_class = task_class;
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
                    cooldown_cause: health.cooldown_cause(),
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
    /// of `ingress::serve`. `exchange.first_byte_at` comes from inside that
    /// call instead — `ingress::forward` is the only place that ever sees the
    /// provider's response arrive — and is `None` on every exchange that
    /// never reached a provider, exactly like every other honest absence this
    /// method reads off `exchange` rather than invents.
    ///
    /// # What the row's `outcome` means now that a `failure_class` sits beside it
    ///
    /// `outcome` still answers *did the turn succeed*, at the transport level
    /// this producer can see. Before framing was observed, a `2xx` was
    /// always [`RoutingOutcome::Succeeded`]; a `2xx` whose stream was cut
    /// short, or whose body was permitted and never came, is now
    /// [`RoutingOutcome::Failed`] with the class that says why. The
    /// invariant this keeps is simple and a test holds it: a row carries a
    /// failure class exactly when its outcome is not a success.
    ///
    /// `retries` is written as `0` on every row — a count, not a default:
    /// `ingress::forward` calls `Agent::run` once and `ureq` performs no
    /// transparent retry. `tool_rounds` and `repairs` stay `NULL`; see the
    /// ledger's own header for why nothing at this layer can count them.
    pub(super) fn record_routing_observation(
        &self,
        ledger: &EvidenceLedger,
        exchange: &Exchange,
        reading: ExchangeReading<'_>,
    ) {
        let failure_class = failure_class(exchange, reading.quota);
        let outcome = match &exchange.outcome {
            Outcome::Forwarded {
                upstream_status, ..
            } => Some(
                if (200..400).contains(upstream_status) && failure_class.is_none() {
                    RoutingOutcome::Succeeded
                } else {
                    RoutingOutcome::Failed
                },
            ),
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

        let Some(assignment) = reading.assignment else {
            return;
        };

        // Migration 24's three. The session is what this gateway was told it
        // serves — `None`, and so `NULL`, for a gateway nothing told; the
        // other two are the decoded request's own facts, carried on the
        // exchange from `super::translate::serve` and `None` on every
        // relayed exchange, whose body this gateway never reads.
        let session_id = self.lock().session_id.clone();
        // Map line 1301's missing join: `task_class` has been migration 23's
        // column since Phase 34C, and this producer is the first to stamp it
        // on a gateway-served row — `record_routing_latency`'s own row is
        // the only other writer and is unaffected by this one.
        let task_class = self.lock().task_class;

        let new = NewObservation::new(
            exchange.provider.clone(),
            assignment.backend().model().label().to_owned(),
        )
        .with_route(exchange.protocol.clone())
        .with_harness(Some(assignment.harness().to_owned()))
        .with_purpose(Some(HARNESS_TURN_PURPOSE))
        .with_quota_context(Some(assignment.backend().credential().label()))
        .with_timing(
            Some(reading.dispatched_at_unix),
            Some(reading.completed_at_unix),
        )
        .with_first_byte_at(exchange.first_byte_at)
        // Line 1331/1332's pair: `translate::serve` derives both from the
        // canonical events it already had to decode, and `None` on a
        // relayed exchange (this method's own caller never gives it one),
        // exactly like `first_byte_at` above.
        .with_first_token_at(exchange.first_token_at)
        .with_first_tool_call_at(exchange.first_tool_call_at)
        .with_outcome(outcome)
        .with_failure_class(failure_class)
        .with_failovers(Some(reading.effect.failovers()))
        .with_retries(Some(0))
        .with_session_id(session_id)
        .with_task_class(task_class)
        .with_effort_level(exchange.effort)
        .with_turn_shape(exchange.turn_shape)
        // Phase 56: a translated exchange has a parsed response, so its
        // usage is exact where the provider stated it. A relayed exchange
        // carries `None` here and writes the same NULLs it always did.
        .with_tokens(
            exchange
                .tokens
                .and_then(|tokens| i64::try_from(tokens.input).ok()),
            exchange
                .tokens
                .and_then(|tokens| i64::try_from(tokens.output).ok()),
            exchange
                .tokens
                .and_then(|tokens| tokens.cached)
                .and_then(|cached| i64::try_from(cached).ok()),
        );

        // Best-effort, exactly like `observe_quota_headers`'s own write to
        // `GatewayQuotaCache`: the accept loop cannot fail a real session's
        // exchange over a full disk or a locked database.
        if let Err(err) = ledger.record(new, reading.completed_at_unix) {
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
    ///
    /// `stated_retry_after` is what **the provider itself said** about how
    /// long to wait, read off this same response's headers by
    /// [`stated_retry_after`] — capability map line 1319. `None` means the
    /// provider said nothing, and it must stay `None` all the way down: the
    /// free pool's own bounded backoff is what applies then, and a wait
    /// nobody stated is not a fact to record.
    ///
    /// Returns what this exchange did to the assignment, so the accept loop
    /// can write it onto the exchange's own evidence row — capability map
    /// line 1334's `failovers`. Every early return is
    /// [`ExchangeEffect::Unchanged`]: an exchange that said nothing moved
    /// nothing.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn observe_exchange(
        &self,
        upstream: &Upstream,
        exchange: &Exchange,
        now: Instant,
        ledger: Option<&EvidenceLedger>,
        now_unix: i64,
        stated_retry_after: Option<Duration>,
        prevention: Option<&FailoverPreventionSink>,
    ) -> ExchangeEffect {
        let Some(observation) = classify(exchange, stated_retry_after) else {
            // Nothing reached the provider — an unauthenticated caller, a
            // malformed head, a target belonging to no protocol. Recording
            // health for a request the provider never saw would be inventing
            // a signal.
            return ExchangeEffect::Unchanged;
        };

        let mut state = self.lock();
        let Some(current) = state.assignment.clone() else {
            return ExchangeEffect::Unchanged;
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
                return ExchangeEffect::RotatedCredential;
            }
        }

        let Some(failure) = observation.failure else {
            return ExchangeEffect::Unchanged;
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
        // Phase 33C lines 1370–1376's one production consumer, beside the
        // evidence source and under the same `None` rule: no ledger, no
        // correlation, and the ranking is exactly what it was before that
        // package. Read here, at the moment of a real provider failure, and
        // not kept warm across the exchanges that move nothing.
        let correlations = match ledger {
            Some(ledger) => ledger
                .route_correlations(now_unix, FAILOVER_EVIDENCE_WINDOW_SECONDS)
                .unwrap_or_else(|err| {
                    tracing::debug!(
                        error = %err,
                        "could not read route correlations; this failover is ranked without them"
                    );
                    RouteCorrelations::default()
                }),
            None => RouteCorrelations::default(),
        };

        match state.policy.on_provider_failure(
            &current,
            failure,
            &candidates,
            state.pairing_preference,
            &state.pairing_overrides,
            evidence,
            &correlations,
        ) {
            FailureResponse::FailOver {
                to,
                cache,
                explanation,
                domain_effect,
            } => {
                if upstream.switch_to(to.backend().credential()) {
                    // Capability map line 1851, at the one moment a failover
                    // is real: `domain_effect` is the comparison
                    // `on_provider_failure` made between its own ranking and
                    // the same ranking without the failure-domain term. It
                    // is reported here rather than at the `OfferMigration`
                    // arm below, because that arm offers a move nobody takes
                    // and counting it would put it in the denominator of how
                    // often a *failover* was steered.
                    //
                    // Inside the `switch_to` guard on purpose: an upstream
                    // that refused the switch produced no failover, and a row
                    // for it would count a move that did not happen.
                    if let Some(sink) = prevention {
                        sink(&domain_effect);
                    }
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
                    ExchangeEffect::FailedOver
                } else {
                    ExchangeEffect::Unchanged
                }
            }
            FailureResponse::OfferMigration {
                to,
                cache,
                explanation,
                domain_effect: _,
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
                ExchangeEffect::Unchanged
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
                ExchangeEffect::Unchanged
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

/// The wait **the provider itself declared** on one response, as a duration —
/// capability map line 1319's producer end, narrowed to the one fact the
/// decision is allowed to carry.
///
/// # Why the whole [`RateLimitHeaders`] does not travel further than this
///
/// A `Retry-After` is a duration and nothing else. The rest of that value —
/// limits, remaining counts, reset instants, the header names it was read
/// from — is capacity telemetry with its own destination
/// ([`SessionRouting::observe_quota_headers`] and the on-disk quota cache),
/// and a scheduling block has no business seeing it. So this is where the
/// narrowing happens, once, at the boundary between the two.
///
/// # `None` stays `None`
///
/// The provider saying nothing is not the same fact as the provider saying
/// zero, and neither is a reason to invent a number: with no stated wait,
/// [`crate::routing::free::ResourceHealth::fail`]'s own bounded backoff is
/// what applies, after the failures it requires. `RateLimitHeaders` has
/// already dropped anything that was not a non-negative integer (see
/// [`RateLimitHeaders::read`]), and [`u64::try_from`] is the second, local
/// refusal rather than a clamp — a negative wait is a header this code does
/// not understand, not a zero-second one.
pub(super) fn stated_retry_after(headers: &RateLimitHeaders) -> Option<Duration> {
    headers
        .retry_after_seconds()
        .and_then(|seconds| u64::try_from(seconds).ok())
        .map(Duration::from_secs)
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
///
/// `stated_retry_after` is the wait the provider declared on **this**
/// response, and it is used by exactly one arm — the `429`. A `Retry-After`
/// on any other status is not folded in anywhere, because
/// [`crate::routing::free::WorkloadOutcome`] keeps a rate-limit refusal, a
/// credential rejection and a transport failure apart on purpose, and only
/// the first of the three is a *temporary scheduling block* capability map
/// line 1319 speaks about. Widening it would blur exactly the distinction
/// that type exists to hold.
fn classify(exchange: &Exchange, stated_retry_after: Option<Duration>) -> Option<Observation> {
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
                    // Capability map line 1319. The provider's own answer,
                    // read off this very response by `ingress::forward` and
                    // carried here by the accept loop — *authoritative* for a
                    // temporary scheduling block, which is why
                    // `routing::free::ResourceHealth::fail` applies a stated
                    // wait immediately and unclamped while an invented one
                    // still has to earn `FAILURES_BEFORE_COOLDOWN`.
                    //
                    // `None` when the provider stated nothing, and it stays
                    // `None`: the free pool's own bounded backoff is the
                    // honest fallback, and a wait nobody declared is not one
                    // to invent here.
                    retry_after: stated_retry_after,
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

/// How far out a `429`'s own reset must be before the refusal is read as a
/// spent long-window quota rather than a cadence limit — capability map line
/// 1365's boundary between the two, and [`failure_class`]'s one constant.
///
/// Five minutes. Every per-minute cadence limit this project has read off a
/// real host (`crate::provider::telemetry`'s AnyRouter and Groq fixtures:
/// `w=60`, per-minute request and token pools) reopens within a minute, and
/// a `Retry-After` on one is seconds to a couple of minutes. A window that
/// reopens in hours, or at midnight, is a quota. Five minutes sits between
/// the two with room on both sides. A constant rather than a configuration
/// because nothing measured yet says a user needs to move it; the day one
/// does, this is the one number to lift.
pub(super) const EXHAUSTED_QUOTA_HORIZON_SECONDS: i64 = 300;

/// What kind of failure one exchange was — capability map line 1364's
/// nine-way vocabulary, decided here and nowhere else, from the status line,
/// the rate-limit headers, the byte count and how the stream ended. `None`
/// for a served exchange, and for every exchange that never reached the
/// provider, on the same reasoning as [`classify`]: nothing can be said about
/// a provider that never saw the request.
///
/// # Every rule, and what it reads
///
/// - `401`, `403` → [`FailureClass::CredentialFailure`]; `402` →
///   [`FailureClass::ExhaustedQuota`] — the account cannot pay, the
///   `phase-9h` live finding [`classify`] records above.
/// - `429` → [`FailureClass::Throttle`], **unless** the response's own
///   headers say nothing remains (`remaining = 0`) and the window reopens at
///   or beyond [`EXHAUSTED_QUOTA_HORIZON_SECONDS`] from when the response
///   arrived — then [`FailureClass::ExhaustedQuota`]. The reopening instant
///   is the provider's reset field when it sent one, else its
///   `Retry-After`; with neither, or with anything remaining, a `429` is a
///   throttle. Line 1365: cadence throttling stays apart from a spent
///   window, and the distinction is *read*, never guessed.
/// - any other `4xx` → [`FailureClass::RequestIncompatibility`]; `5xx` →
///   [`FailureClass::Upstream5xx`].
/// - `2xx`/`3xx` are decided by framing alone: a stream that ended short of
///   its declared length or before its terminating chunk →
///   [`FailureClass::StreamAbort`]; a body that was permitted and never
///   came, zero bytes and a clean end → [`FailureClass::EmptyCompletion`];
///   otherwise served, `None`.
/// - a transport failure → [`FailureClass::Timeout`] when
///   `ingress::transport_detail` said so, else [`FailureClass::Unknown`]
///   with the detail still on the exchange's own log line.
///
/// # What is never read
///
/// No byte of the body. A `200` whose body describes a model error is
/// served here, and the ledger's own header says so; the harness that
/// received the body is the thing that can read it.
pub(super) fn failure_class(exchange: &Exchange, quota: &RateLimitHeaders) -> Option<FailureClass> {
    match &exchange.outcome {
        Outcome::Forwarded {
            upstream_status, ..
        } => match *upstream_status {
            401 | 403 => Some(FailureClass::CredentialFailure),
            402 => Some(FailureClass::ExhaustedQuota),
            429 => Some(if quota_is_exhausted(quota, exchange.first_byte_at) {
                FailureClass::ExhaustedQuota
            } else {
                FailureClass::Throttle
            }),
            400..=499 => Some(FailureClass::RequestIncompatibility),
            500..=599 => Some(FailureClass::Upstream5xx),
            _ => match exchange.framing {
                Some(Framing {
                    ended: StreamEnd::Truncated | StreamEnd::Aborted,
                    ..
                }) => Some(FailureClass::StreamAbort),
                Some(Framing {
                    relayed: Some(0),
                    ended: StreamEnd::Complete,
                    ..
                }) => Some(FailureClass::EmptyCompletion),
                _ => None,
            },
        },
        Outcome::Unreachable { detail } => Some(if *detail == TRANSPORT_TIMEOUT_DETAIL {
            FailureClass::Timeout
        } else {
            FailureClass::Unknown
        }),
        Outcome::Unauthenticated
        | Outcome::Declined
        | Outcome::Unrouted
        | Outcome::ClientGone
        | Outcome::Idle => None,
    }
}

/// [`failure_class`]'s `429` rule: nothing remains, and the window reopens
/// no sooner than [`EXHAUSTED_QUOTA_HORIZON_SECONDS`] after the response was
/// observed. `observed_at_unix` is the exchange's `first_byte_at`, which is
/// set on every forwarded exchange; a reset field with no observation
/// instant to anchor it is read as the delta the IETF field specifies.
fn quota_is_exhausted(quota: &RateLimitHeaders, observed_at_unix: Option<i64>) -> bool {
    if quota.remaining() != Some(0) {
        return false;
    }
    let reopens_in = match observed_at_unix {
        Some(observed) => quota
            .resets_at_unix(observed)
            .map(|at| at.saturating_sub(observed)),
        None => quota.reset(),
    }
    .or_else(|| quota.retry_after_seconds());
    reopens_in.is_some_and(|seconds| seconds >= EXHAUSTED_QUOTA_HORIZON_SECONDS)
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
    use crate::routing::free::CooldownCause;
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
            // No response ever arrived — this outcome exists precisely
            // because the provider could not be reached at all.
            first_byte_at: None,
            first_token_at: None,
            first_tool_call_at: None,
            framing: None,
            tokens: None,
            effort: None,
            turn_shape: None,
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
        forwarded_exchange(provider, 429, Some(0), Some(0), StreamEnd::Complete)
    }

    /// A forwarded exchange with the framing `ingress::forward` would have
    /// recorded for it: what was declared, what arrived, and how it ended.
    fn forwarded_exchange(
        provider: &str,
        status: u16,
        declared: Option<u64>,
        relayed: Option<u64>,
        ended: StreamEnd,
    ) -> Exchange {
        Exchange {
            outcome: Outcome::Forwarded {
                upstream_status: status,
                bytes: relayed.unwrap_or(0),
            },
            status,
            provider: provider.to_owned(),
            protocol: Some("anthropic-messages".to_owned()),
            host: String::new(),
            first_byte_at: Some(1_700_000_000),
            first_token_at: None,
            first_tool_call_at: None,
            framing: Some(Framing {
                declared,
                relayed,
                ended,
            }),
            tokens: None,
            effort: None,
            turn_shape: None,
        }
    }

    fn no_headers() -> RateLimitHeaders {
        RateLimitHeaders::default()
    }

    /// The mapping from what the relay observed to line 1364's vocabulary,
    /// one case per class and the served case beside them — every input a
    /// status, a header, a count or a way of ending, and not one a byte of
    /// a body.
    #[test]
    fn every_failure_class_is_decided_from_status_headers_and_framing_alone() {
        let served = forwarded_exchange("p", 200, Some(512), Some(512), StreamEnd::Complete);
        assert_eq!(failure_class(&served, &no_headers()), None);

        let chunked_served = forwarded_exchange("p", 200, None, Some(9_000), StreamEnd::Complete);
        assert_eq!(failure_class(&chunked_served, &no_headers()), None);

        for (status, expected) in [
            (401, FailureClass::CredentialFailure),
            (403, FailureClass::CredentialFailure),
            (402, FailureClass::ExhaustedQuota),
            (429, FailureClass::Throttle),
            (400, FailureClass::RequestIncompatibility),
            (404, FailureClass::RequestIncompatibility),
            (413, FailureClass::RequestIncompatibility),
            (500, FailureClass::Upstream5xx),
            (503, FailureClass::Upstream5xx),
            (529, FailureClass::Upstream5xx),
        ] {
            let exchange = forwarded_exchange("p", status, Some(40), Some(40), StreamEnd::Complete);
            assert_eq!(
                failure_class(&exchange, &no_headers()),
                Some(expected),
                "status {status}"
            );
        }

        let truncated = forwarded_exchange("p", 200, Some(1000), Some(100), StreamEnd::Truncated);
        assert_eq!(
            failure_class(&truncated, &no_headers()),
            Some(FailureClass::StreamAbort)
        );
        let aborted = forwarded_exchange("p", 200, None, Some(100), StreamEnd::Aborted);
        assert_eq!(
            failure_class(&aborted, &no_headers()),
            Some(FailureClass::StreamAbort)
        );
        // A stream cut at zero bytes is an abort, not an empty completion:
        // the framing said more was coming.
        let cut_at_zero = forwarded_exchange("p", 200, Some(1000), Some(0), StreamEnd::Truncated);
        assert_eq!(
            failure_class(&cut_at_zero, &no_headers()),
            Some(FailureClass::StreamAbort)
        );

        let empty = forwarded_exchange("p", 200, Some(0), Some(0), StreamEnd::Complete);
        assert_eq!(
            failure_class(&empty, &no_headers()),
            Some(FailureClass::EmptyCompletion)
        );
        // No body was *permitted* — a `204`, or a `HEAD` — so nothing is
        // missing from it.
        let no_body_permitted = forwarded_exchange("p", 204, None, None, StreamEnd::Complete);
        assert_eq!(failure_class(&no_body_permitted, &no_headers()), None);

        let timed_out = Exchange {
            outcome: Outcome::Unreachable {
                detail: TRANSPORT_TIMEOUT_DETAIL,
            },
            ..unreachable_exchange("p")
        };
        assert_eq!(
            failure_class(&timed_out, &no_headers()),
            Some(FailureClass::Timeout)
        );
        assert_eq!(
            failure_class(&unreachable_exchange("p"), &no_headers()),
            Some(FailureClass::Unknown)
        );

        // Never reached the provider: nothing to classify, the same filter
        // `classify` applies.
        for outcome in [
            Outcome::Unauthenticated,
            Outcome::Declined,
            Outcome::Unrouted,
            Outcome::ClientGone,
            Outcome::Idle,
        ] {
            let exchange = Exchange {
                outcome,
                ..unreachable_exchange("p")
            };
            assert_eq!(failure_class(&exchange, &no_headers()), None);
        }
    }

    /// Line 1365's boundary, read off the headers rather than guessed: a
    /// `429` is a spent quota only when nothing remains **and** the window
    /// reopens at or beyond the horizon; anything else about it is a
    /// throttle.
    #[test]
    fn a_429_is_exhausted_quota_only_when_nothing_remains_until_a_reset_beyond_the_horizon() {
        let exchange = rate_limited_exchange("p");
        let horizon = EXHAUSTED_QUOTA_HORIZON_SECONDS.to_string();
        let just_under = (EXHAUSTED_QUOTA_HORIZON_SECONDS - 1).to_string();

        let cases: [(&[(&str, &str)], FailureClass); 8] = [
            (&[("retry-after", "2")], FailureClass::Throttle),
            (
                &[
                    ("x-ratelimit-remaining", "0"),
                    ("x-ratelimit-reset", "3600"),
                ],
                FailureClass::ExhaustedQuota,
            ),
            (
                &[
                    ("x-ratelimit-remaining", "0"),
                    ("x-ratelimit-reset", &horizon),
                ],
                FailureClass::ExhaustedQuota,
            ),
            (
                &[
                    ("x-ratelimit-remaining", "0"),
                    ("x-ratelimit-reset", &just_under),
                ],
                FailureClass::Throttle,
            ),
            (
                &[
                    ("x-ratelimit-remaining", "5"),
                    ("x-ratelimit-reset", "3600"),
                ],
                FailureClass::Throttle,
            ),
            (
                &[("x-ratelimit-remaining", "0"), ("retry-after", "3600")],
                FailureClass::ExhaustedQuota,
            ),
            (&[("x-ratelimit-remaining", "0")], FailureClass::Throttle),
            (
                // An absolute reset an hour past the response's own arrival.
                &[
                    ("ratelimit-remaining", "0"),
                    ("ratelimit-reset", "1700003600"),
                ],
                FailureClass::ExhaustedQuota,
            ),
        ];
        for (headers, expected) in cases {
            let quota = RateLimitHeaders::read(headers.iter().copied());
            assert_eq!(
                failure_class(&exchange, &quota),
                Some(expected),
                "headers {headers:?}"
            );
        }
    }

    /// The row's `outcome` and its `failure_class` agree by construction:
    /// a class exactly when the outcome is not a success. Driven through the
    /// real writer against a real ledger, and read back through the public
    /// reader.
    #[test]
    fn record_routing_observation_writes_the_class_the_failover_count_and_zero_retries() {
        use crate::routing::evidence::ObservationQuery;

        let tmp = tempfile::tempdir().unwrap();
        let ledger = ledger_fixture(tmp.path());
        let upstream = Upstream::with_failover(vec![upstream_backend("openrouter")])
            .expect("one backend is not none");
        let routing = SessionRouting::new();
        routing.bind(
            "claude-code",
            "anthropic-messages",
            AssignedModel::named("the-routed-model"),
            &upstream,
        );
        let assignment = routing.assignment();
        let exhausted = RateLimitHeaders::read(vec![
            ("x-ratelimit-remaining", "0"),
            ("x-ratelimit-reset", "7200"),
        ]);

        let cases = [
            (
                rate_limited_exchange("openrouter"),
                &exhausted,
                ExchangeEffect::Unchanged,
            ),
            (
                forwarded_exchange("openrouter", 200, Some(64), Some(64), StreamEnd::Complete),
                &no_headers(),
                ExchangeEffect::Unchanged,
            ),
            (
                forwarded_exchange(
                    "openrouter",
                    200,
                    Some(1000),
                    Some(100),
                    StreamEnd::Truncated,
                ),
                &no_headers(),
                ExchangeEffect::FailedOver,
            ),
        ];
        for (i, (exchange, quota, effect)) in cases.iter().enumerate() {
            routing.record_routing_observation(
                &ledger,
                exchange,
                ExchangeReading {
                    quota,
                    dispatched_at_unix: 1_700_000_000 + i as i64,
                    completed_at_unix: 1_700_000_001 + i as i64,
                    assignment: assignment.clone(),
                    effect: *effect,
                },
            );
        }

        let mut rows = ledger
            .recent(
                ObservationQuery {
                    provider: "openrouter",
                    model: "the-routed-model",
                    route: Some("anthropic-messages"),
                    harness: Some("claude-code"),
                },
                10,
            )
            .unwrap();
        rows.sort_by_key(|row| row.dispatched_at_unix);
        assert_eq!(rows.len(), 3);

        assert_eq!(rows[0].failure_class, Some(FailureClass::ExhaustedQuota));
        assert_eq!(rows[0].outcome, Some(RoutingOutcome::Failed));
        assert_eq!(rows[1].failure_class, None);
        assert_eq!(rows[1].outcome, Some(RoutingOutcome::Succeeded));
        assert_eq!(rows[2].failure_class, Some(FailureClass::StreamAbort));
        assert_eq!(
            rows[2].outcome,
            Some(RoutingOutcome::Failed),
            "a 2xx whose stream was cut is not a success at the transport level either"
        );
        assert_eq!(rows[2].failovers, Some(1));
        for row in &rows[..2] {
            assert_eq!(row.failovers, Some(0));
        }
        for row in &rows {
            assert_eq!(row.retries, Some(0), "the gateway forwards exactly once");
            assert_eq!(row.tool_rounds, None);
            assert_eq!(row.repairs, None);
            assert_eq!(
                row.failure_class.is_some(),
                row.outcome != Some(RoutingOutcome::Succeeded),
                "a class exactly when the outcome is not a success: {row:?}"
            );
        }
    }

    /// What `observe_exchange` says it did is what it did: a real failover
    /// answers `FailedOver`, a rotation within the provider answers
    /// `RotatedCredential` and is not counted as a failover, and an exchange
    /// with nowhere to go answers `Unchanged`.
    #[test]
    fn observe_exchange_reports_what_it_did_to_the_assignment() {
        let three = Upstream::with_failover(vec![
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
            &three,
        );
        let effect = routing.observe_exchange(
            &three,
            &unreachable_exchange("first"),
            Instant::now(),
            None,
            0,
            None,
            None,
        );
        assert_eq!(effect, ExchangeEffect::FailedOver);
        assert_eq!(effect.failovers(), 1);

        let alone = Upstream::with_failover(vec![upstream_backend("only")])
            .expect("one backend is not none");
        let routing = SessionRouting::new();
        routing.bind(
            "claude-code",
            "anthropic-messages",
            AssignedModel::named("the-routed-model"),
            &alone,
        );
        let effect = routing.observe_exchange(
            &alone,
            &unreachable_exchange("only"),
            Instant::now(),
            None,
            0,
            None,
            None,
        );
        assert_eq!(effect, ExchangeEffect::Unchanged);
        assert_eq!(effect.failovers(), 0);

        let two_keys = Upstream::with_failover(vec![
            upstream_backend_with_credential("openrouter", "OPENROUTER_KEY_A"),
            upstream_backend_with_credential("openrouter", "OPENROUTER_KEY_B"),
        ])
        .expect("two backends is not none");
        let routing = SessionRouting::new();
        routing.bind(
            "claude-code",
            "anthropic-messages",
            AssignedModel::named("the-routed-model"),
            &two_keys,
        );
        let effect = routing.observe_exchange(
            &two_keys,
            &forwarded_exchange("openrouter", 401, Some(0), Some(0), StreamEnd::Complete),
            Instant::now(),
            None,
            0,
            None,
            None,
        );
        assert_eq!(effect, ExchangeEffect::RotatedCredential);
        assert_eq!(
            effect.failovers(),
            0,
            "a rotation within one provider is not a failover"
        );
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
            None,
            None,
        );

        assert_eq!(
            routing.assignment().map(|a| a.provider().to_owned()),
            Some("good-evidence".to_owned()),
            "the candidate with strong recorded evidence must win the real failover, not \
             `poor-evidence`, which is configured first among the two survivors"
        );
    }

    /// Phase 33C lines 1370–1376 and 1852, at the production caller (§35):
    /// a **real** [`EvidenceLedger`] whose rows show `correlated` answering
    /// `5xx` at the same moments `first` did, and `independent` serving
    /// through those same moments, driven through
    /// [`SessionRouting::observe_exchange`] itself. Both survivors carry the
    /// same failure rate (five failed, five served each), the same harness,
    /// model and route, and `correlated` is configured first — so nothing
    /// but the correlation term can move the winner, and the sink must be
    /// told that it did.
    ///
    /// Mutating this method's `&correlations` back to
    /// `&RouteCorrelations::default()` fails this test, because it is the
    /// only one that supplies correlated rows here at all.
    #[test]
    fn observe_exchange_steers_a_real_failover_off_a_route_the_ledger_shows_failing_with_it() {
        use crate::routing::evidence::RouteIdentity;

        let tmp = tempfile::tempdir().unwrap();
        let ledger = ledger_fixture(tmp.path());
        let now_unix = 1_800_000_000_i64;
        let row = |provider: &str, outcome: RoutingOutcome, class: Option<FailureClass>| {
            NewObservation::new(provider, "the-routed-model")
                .with_route(Some("anthropic-messages"))
                .with_harness(Some("claude-code"))
                .with_outcome(outcome)
                .with_failure_class(class)
        };

        for i in 0..5 {
            // The failed backend's own 5xx, and what the two survivors were
            // doing ten seconds later: one failing the same way, one serving.
            let failed_at = now_unix - 3_600 + i * 120;
            ledger
                .record(
                    row(
                        "first",
                        RoutingOutcome::Failed,
                        Some(FailureClass::Upstream5xx),
                    )
                    .with_timing(Some(failed_at), Some(failed_at + 5)),
                    failed_at + 5,
                )
                .unwrap();
            ledger
                .record(
                    row(
                        "correlated",
                        RoutingOutcome::Failed,
                        Some(FailureClass::Upstream5xx),
                    )
                    .with_timing(Some(failed_at + 10), Some(failed_at + 15)),
                    failed_at + 15,
                )
                .unwrap();
            ledger
                .record(
                    row("independent", RoutingOutcome::Succeeded, None)
                        .with_timing(Some(failed_at + 10), Some(failed_at + 15)),
                    failed_at + 15,
                )
                .unwrap();
            // Balance the two survivors' own records so the local-evidence
            // term ties: `correlated` served, and `independent` failed, at
            // moments nothing else was observed.
            let alone_at = now_unix - 7_200 + i * 120;
            ledger
                .record(
                    row("correlated", RoutingOutcome::Succeeded, None)
                        .with_timing(Some(alone_at), Some(alone_at + 5)),
                    alone_at + 5,
                )
                .unwrap();
            let alone_at = now_unix - 10_800 + i * 120;
            ledger
                .record(
                    row(
                        "independent",
                        RoutingOutcome::Failed,
                        Some(FailureClass::Upstream5xx),
                    )
                    .with_timing(Some(alone_at), Some(alone_at + 5)),
                    alone_at + 5,
                )
                .unwrap();
        }

        let seen: std::sync::Arc<std::sync::Mutex<Vec<Option<RouteIdentity>>>> = Default::default();
        let sink_seen = std::sync::Arc::clone(&seen);
        let sink: FailoverPreventionSink = std::sync::Arc::new(move |effect| {
            sink_seen
                .lock()
                .unwrap()
                .push(effect.correlation_displaced().cloned());
        });

        let upstream = Upstream::with_failover(vec![
            upstream_backend("first"),
            upstream_backend("correlated"),
            upstream_backend("independent"),
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
            Some(&ledger),
            now_unix,
            None,
            Some(&sink),
        );

        assert_eq!(
            routing.assignment().map(|a| a.provider().to_owned()),
            Some("independent".to_owned()),
            "a route the ledger shows failing at the same moments as the failed backend must \
             lose the failover to one it shows serving through them, even though it is \
             configured first"
        );
        assert_eq!(
            seen.lock().unwrap().as_slice(),
            &[Some(RouteIdentity::new("correlated", "the-routed-model"))],
            "line 1852: the sink is told which nominally different route the correlation \
             steered this failover off"
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
            None,
            None,
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
        use std::sync::{Arc, Mutex, Once};

        // `tracing`'s callsite `Interest` cache is a *process-global* static,
        // not a per-thread one: the first time the failover-explanation log
        // line anywhere in this test binary fires, if no subscriber has ever
        // been registered yet, `tracing_core` permanently caches
        // `Interest::never()` for that callsite (an empty dispatcher list
        // folds to "never" — see `tracing_core::callsite::rebuild_callsite_interest`).
        // Another test in this module (e.g. the real-failover assertions
        // near this one) can win that race on its own thread before this
        // helper's `with_default` subscriber ever registers, which is why
        // the capture comes back empty roughly one run in five under
        // `cargo test`'s default thread pool. A `with_default` scope cannot
        // fix this by itself — thread-local scoping only decides who
        // *receives* an event once interest says to emit one. Registering a
        // permanent, sufficiently-verbose global default once, before this
        // helper ever calls into production code, keeps the dispatcher list
        // non-empty for the rest of the process: any later rebuild
        // (including the one triggered by installing this helper's own
        // `with_default` subscriber below) recomputes interest against a
        // live dispatcher instead of an empty list, so the callsite can
        // never get stuck at `never` again.
        static ENSURE_GLOBAL_DISPATCH: Once = Once::new();
        ENSURE_GLOBAL_DISPATCH.call_once(|| {
            let _ = tracing::subscriber::set_global_default(
                tracing_subscriber::fmt()
                    .with_writer(std::io::sink)
                    .with_max_level(tracing::Level::TRACE)
                    .finish(),
            );
        });

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
                None,
                None,
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
            None,
            None,
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
            None,
            None,
        );
        routing.observe_exchange(
            &upstream,
            &rate_limited_exchange("openrouter"),
            now,
            None,
            0,
            None,
            None,
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

    /// Capability map line 1546's write side: [`Self::health_readings_for`]
    /// must carry the *cause* `ResourceHealth::fail` already recorded on the
    /// same value it reads `cooling_down_until` from, not merely the
    /// deadline. This is the exact gap the line's hold ruling
    /// (`docs/product/evidence/phase-35b.md`) named: the mechanism existed
    /// and this call site silently dropped it.
    #[test]
    fn health_readings_for_carries_the_cooldown_cause_the_pool_already_recorded() {
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
        // A stated `retry_after` applies immediately, on the first failure —
        // REQUIRED BEHAVIOR of `ResourceHealth::fail`, unchanged here.
        routing.observe_exchange(
            &upstream,
            &rate_limited_exchange("openrouter"),
            now,
            None,
            0,
            Some(Duration::from_secs(60)),
            None,
        );

        let readings = routing.health_readings_for("openrouter", now, now_unix);
        let reading = readings
            .iter()
            .find(|r| r.model == "the-routed-model")
            .expect("a declared wait must produce a health reading");
        assert_eq!(
            reading.cooldown_cause,
            Some(CooldownCause::Declared),
            "a provider-declared wait must cross as a recorded Declared cause, never dropped to \
             None"
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
            None,
            None,
        );
        routing.observe_exchange(
            &upstream,
            &rate_limited_exchange("openrouter"),
            now,
            None,
            0,
            None,
            None,
        );
        routing.observe_exchange(
            &upstream,
            &forwarded_exchange("openrouter", 200, Some(0), Some(0), StreamEnd::Complete),
            now,
            None,
            0,
            None,
            None,
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
