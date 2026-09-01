//! The free pool: which zero-cost resources exist, what is left of each, and
//! which of them is currently able to serve.
//!
//! # Health comes from work, never from a probe
//!
//! Phase 9I line 534 asks Glasshouse to *"avoid consuming scarce free
//! requests on health probes when actual workload can provide health
//! signals"*. A health checker that burns the quota it is protecting is a
//! defect with a passing test, so this module is built so that one cannot be
//! written here: [`FreePool::observe`] is the **only** thing that changes a
//! resource's health, it takes a [`WorkloadOutcome`] that a real exchange
//! produced, and there is no client, no socket and no timer anywhere in this
//! file — `routing::tests::no_routing_policy_can_make_a_request` scans for
//! that.
//!
//! The production feed is the gateway's own request path: every exchange it
//! completes already knows the credential it used, the status the provider
//! returned and whether it reached the provider at all.
//!
//! # A request pool is not a token budget
//!
//! Phase 9I line 528 — *"track request-pool limits separately from
//! token-priced limits"*. [`Allowance`] has one variant for each and no
//! shared arithmetic, because the failure mode of collapsing them is
//! specific and quiet: a token budget decremented by one per request reads as
//! healthy for a very long time and then is not.
//!
//! What a request pool holds is what a **real response said** — a limit, a
//! remaining count and a reset instant, each `None` until a provider actually
//! stated it. Glasshouse defines no window of its own. A guessed window is
//! how a router talks itself into believing a pool has refilled.
//!
//! # Per credential, because two keys are two allowances
//!
//! Phase 9I lines 537 and 538. Allowance state is keyed by [`CredentialId`]
//! and health by credential **and** model, so exhausting one key says nothing
//! about the other key, and exhausting one model says nothing about the
//! others behind the same key. Keying either of these by provider is the
//! mistake the two lines exist to name; `crates/glasshouse/tests/` carries
//! the test that fails when it is made.

use std::time::{Duration, Instant};

use super::CredentialId;

/// How many consecutive rate-limit or capacity failures a resource is given
/// before Glasshouse puts it in a cooldown **of its own invention**.
///
/// Phase 9I line 535 says *"repeatedly"*, and one failure is not repeatedly.
/// A single 429 on a shared free tier is ordinary — another user's request
/// arrived first — and cooling a resource down for it would empty a pool of
/// perfectly good resources during the busiest minute of the day.
///
/// **This threshold governs only the invented cooldown.** A provider that
/// declared its own `Retry-After` is obeyed on the first failure — see
/// [`ResourceHealth::fail`] and capability map line 1319. The reason for
/// waiting for a second failure is that the first one does not tell
/// Glasshouse how long to wait; when the provider says how long, that reason
/// is gone.
const FAILURES_BEFORE_COOLDOWN: u32 = 2;

/// The first cooldown a resource gets when the provider did not say how long
/// to wait.
const BASE_COOLDOWN: Duration = Duration::from_secs(30);

/// The longest cooldown Glasshouse will impose by itself.
///
/// A bound rather than unbounded doubling: a free resource that has been
/// failing all morning may be serving again now, and the only way to find
/// out — under line 534 — is to let real work try it.
const MAX_COOLDOWN: Duration = Duration::from_secs(15 * 60);

/// What a provider is limiting, for one credential.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Allowance {
    /// A pool of requests. Every field is `None` until a real response
    /// stated it — Glasshouse invents neither a limit nor a window.
    RequestPool {
        limit: Option<u32>,
        remaining: Option<u32>,
        resets_at: Option<Instant>,
    },
    /// Priced per token. There is no pool to count down, and asking this
    /// variant "how many requests are left" has no answer, which is the
    /// point of it being a separate variant rather than a limit of zero.
    TokenPriced,
}

impl Allowance {
    /// An untouched request pool: known to be a pool, with nothing yet
    /// established about its size.
    pub const fn unknown_pool() -> Self {
        Self::RequestPool {
            limit: None,
            remaining: None,
            resets_at: None,
        }
    }

    /// Whether this allowance is a request pool at all.
    pub fn is_request_pool(&self) -> bool {
        matches!(self, Self::RequestPool { .. })
    }

    /// Whether the pool is known to be empty right now.
    ///
    /// `false` for [`Allowance::TokenPriced`] — not because a token-priced
    /// resource is always affordable, but because "how many requests are
    /// left" is not a question it answers, and answering it anyway is the
    /// conflation line 528 forbids.
    pub fn is_exhausted(&self, now: Instant) -> bool {
        match self {
            Self::TokenPriced => false,
            Self::RequestPool {
                remaining,
                resets_at,
                ..
            } => {
                if resets_at.is_some_and(|reset| reset <= now) {
                    // The provider's own reset instant has passed. What is
                    // left is unknown again, not zero.
                    return false;
                }
                matches!(remaining, Some(0))
            }
        }
    }

    /// Fold in what one real response stated about the pool.
    fn record(&mut self, reading: &PoolReading, now: Instant) {
        let Self::RequestPool {
            limit,
            remaining,
            resets_at,
        } = self
        else {
            return;
        };
        if let Some(stated) = reading.limit {
            *limit = Some(stated);
        }
        if let Some(stated) = reading.remaining {
            *remaining = Some(stated);
        }
        if let Some(after) = reading.resets_in {
            *resets_at = Some(now + after);
        }
    }
}

/// What one real response said about the request pool behind the credential
/// that made it.
///
/// Every field is optional because every field is optional in reality:
/// providers state some of these headers, some of the time. A field nobody
/// stated stays unknown rather than becoming a default.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PoolReading {
    pub limit: Option<u32>,
    pub remaining: Option<u32>,
    pub resets_in: Option<Duration>,
}

/// What one real unit of work told us about a resource.
///
/// Deliberately not an HTTP status. A status is the transport's word and
/// carries cases routing must not conflate — a `400` is the harness's own
/// malformed request and says nothing about the provider's health, while a
/// `401` is about the credential and not about the model. The caller that
/// holds the exchange translates once, and the reasoning lives at that call
/// site rather than being re-derived in every policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkloadOutcome {
    /// The provider served it.
    Served,
    /// The provider refused for rate or quota reasons, for **this
    /// credential**. `retry_after` is the provider's own answer when it gave
    /// one, and it is preferred over any cooldown Glasshouse would invent.
    RateLimited { retry_after: Option<Duration> },
    /// The provider refused the credential itself — expired, revoked, wrong.
    /// A different failure from being out of requests, and it must not be
    /// mistaken for one: waiting does not fix it.
    CredentialRejected,
    /// The provider could not serve: capacity, an upstream error, or nothing
    /// listening at all.
    CapacityFailure,
}

impl WorkloadOutcome {
    /// Whether this outcome is one of the two Phase 9I line 535 names —
    /// "rate-limit or capacity failures".
    ///
    /// Public because the caller that translates an exchange needs the same
    /// classification for its own diagnostics, and two spellings of "which
    /// failures count" is how the two drift apart.
    pub fn counts_toward_cooldown(self) -> bool {
        matches!(self, Self::RateLimited { .. } | Self::CapacityFailure)
    }
}

/// One free resource: a model, behind a credential.
///
/// Health is keyed by both. Phase 9I line 529 asks for *"per-model free-tier
/// health ... when a router exposes multiple free models"*, and line 538 asks
/// for quota state per credential; a router that shared one health entry
/// across a provider's models would take every model out of service because
/// one of them was busy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FreeResource {
    credential: CredentialId,
    model: String,
}

impl FreeResource {
    pub fn new(credential: CredentialId, model: impl Into<String>) -> Self {
        Self {
            credential,
            model: model.into(),
        }
    }

    pub fn credential(&self) -> &CredentialId {
        &self.credential
    }

    pub fn model(&self) -> &str {
        &self.model
    }

    pub fn provider(&self) -> &str {
        self.credential.provider()
    }

    /// A diagnostic name: two names and a model, never a value.
    pub fn label(&self) -> String {
        format!("{} ({})", self.model, self.credential.label())
    }
}

/// Which kind of cooldown [`ResourceHealth`] is carrying, when it is
/// carrying one at all — named for what each is, not for the code path that
/// produced it: a provider's own cadence, or Glasshouse's own caution while
/// it works out whether a resource has recovered.
///
/// `ResourceHealth::fail` already knows this distinction at the moment it
/// writes `cooling_down_until` — the `Some(declared)` arm versus the `None`
/// arm's `ResourceHealth::backoff` — and until now nothing retained it past
/// that call, which is the gap capability map line 1546 names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CooldownCause {
    /// The provider itself stated the wait — authoritative per capability
    /// map line 1319, applied immediately and unclamped.
    Declared,
    /// Glasshouse's own bounded backoff, imposed only after
    /// `FAILURES_BEFORE_COOLDOWN` ordinary failures that stated no wait. Not
    /// a cadence claim: Phase 9I line 534 deliberately keeps this probeable
    /// by real work rather than trusted as a fact about the provider.
    Invented,
}

/// What is currently known about one [`FreeResource`], learned entirely from
/// work that was going to happen anyway.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResourceHealth {
    consecutive_failures: u32,
    cooling_down_until: Option<Instant>,
    /// Which kind of cooldown `cooling_down_until` is, or `None` when either
    /// nothing is cooling down or the cause was never established (adopted
    /// from another process's persisted reading — see
    /// [`FreePool::adopt_observed`], which cannot carry this without a new
    /// persisted column). `None` here reports as inert everywhere this is
    /// read, the same honest-unknown stance the rest of this module takes.
    cooldown_cause: Option<CooldownCause>,
    /// Set when a provider refused the credential itself. Not a cooldown:
    /// waiting does not fix a revoked key, so it is reported rather than
    /// slept off.
    credential_rejected: bool,
}

impl ResourceHealth {
    fn new() -> Self {
        Self {
            consecutive_failures: 0,
            cooling_down_until: None,
            cooldown_cause: None,
            credential_rejected: false,
        }
    }

    /// Whether this resource may be chosen right now.
    pub fn is_available(&self, now: Instant) -> bool {
        if self.credential_rejected {
            return false;
        }
        match self.cooling_down_until {
            Some(until) => until <= now,
            None => true,
        }
    }

    pub fn cooling_down_until(&self) -> Option<Instant> {
        self.cooling_down_until
    }

    /// How long is left on a wait this resource's own provider declared, if
    /// it is inside one right now — capability map line 1546.
    ///
    /// `None` covers every case that is not that one: no cooldown at all, an
    /// invented cooldown ([`CooldownCause::Invented`]), a declared one that
    /// has already expired, and a cause never established at all (adopted
    /// health — see [`FreePool::adopt_observed`]). All four are the same
    /// answer to *"is a provider cadence in effect"* — no — and this reader
    /// does not distinguish them further, the same way [`Self::is_available`]
    /// does not split its own `false` by cause.
    pub fn declared_wait_remaining(&self, now: Instant) -> Option<Duration> {
        if !matches!(self.cooldown_cause, Some(CooldownCause::Declared)) {
            return None;
        }
        self.cooling_down_until
            .filter(|&until| until > now)
            .map(|until| until - now)
    }

    pub fn consecutive_failures(&self) -> u32 {
        self.consecutive_failures
    }

    pub fn credential_was_rejected(&self) -> bool {
        self.credential_rejected
    }

    /// Fold in one real outcome.
    ///
    /// The cooldown length is the provider's own `retry_after` when it gave
    /// one — it knows and we do not, and capability map line 1319 makes that
    /// answer authoritative rather than advisory — and otherwise a bounded
    /// doubling from [`BASE_COOLDOWN`], so a resource failing repeatedly is
    /// tried less often without ever being written off. [`Self::fail`] has
    /// the difference between the two, which is more than a length.
    fn observe(&mut self, outcome: WorkloadOutcome, now: Instant) {
        match outcome {
            WorkloadOutcome::Served => {
                // A resource that served is healthy, whatever it did before.
                // This is the other half of line 534: recovery is learned
                // from work too, so nothing has to probe to find out that a
                // cooldown can end early.
                self.consecutive_failures = 0;
                self.cooling_down_until = None;
                self.cooldown_cause = None;
                self.credential_rejected = false;
            }
            WorkloadOutcome::CredentialRejected => {
                self.credential_rejected = true;
            }
            WorkloadOutcome::RateLimited { retry_after } => {
                self.fail(retry_after, now);
            }
            WorkloadOutcome::CapacityFailure => {
                self.fail(None, now);
            }
        }
    }

    /// One rate-limit or capacity failure — the two outcomes Phase 9I line
    /// 535 names — and the cooldown that follows.
    ///
    /// **A cooldown a provider declared and one Glasshouse invented are not
    /// the same kind of fact.** Capability map line 1319 makes the provider's
    /// own answer *authoritative* for a temporary scheduling block, not
    /// merely preferred, so the two take different paths here:
    ///
    /// - **A declared `retry_after` applies as given, and immediately.**
    ///   [`FAILURES_BEFORE_COOLDOWN`] exists because *inventing* a cooldown
    ///   out of one ordinary `429` would empty a pool of perfectly good
    ///   resources; nothing is invented when the provider stated the wait
    ///   itself, and scheduling work against a resource that just told us to
    ///   hold is exactly the block line 1319 forbids. [`MAX_COOLDOWN`] does
    ///   not apply either — it bounds what Glasshouse imposes *by itself*
    ///   (see its own doc), never what a provider declared. Clamping a stated
    ///   one-hour wait down to fifteen minutes is overriding the provider,
    ///   which is the whole of what this line rules out.
    /// - **Without one, the bounded doubling from [`BASE_COOLDOWN`] applies**,
    ///   and only once there have been [`FAILURES_BEFORE_COOLDOWN`] of them.
    ///   [`Self::backoff`] applies [`MAX_COOLDOWN`] itself, so the ceiling on
    ///   the invented path is unchanged by the split.
    ///
    /// A declared wait that is *shorter* than a cooldown already in place
    /// shortens it, for the same reason: authoritative means authoritative in
    /// both directions, and it is the same rule that lets
    /// [`WorkloadOutcome::Served`] clear a cooldown outright.
    fn fail(&mut self, retry_after: Option<Duration>, now: Instant) {
        self.consecutive_failures = self.consecutive_failures.saturating_add(1);
        match retry_after {
            Some(declared) => {
                self.cooling_down_until = Some(now + declared);
                self.cooldown_cause = Some(CooldownCause::Declared);
            }
            None => {
                if self.consecutive_failures >= FAILURES_BEFORE_COOLDOWN {
                    self.cooling_down_until = Some(now + self.backoff());
                    self.cooldown_cause = Some(CooldownCause::Invented);
                }
            }
        }
    }

    /// [`BASE_COOLDOWN`] doubled once per failure past the threshold,
    /// capped by [`MAX_COOLDOWN`].
    fn backoff(&self) -> Duration {
        let steps = self
            .consecutive_failures
            .saturating_sub(FAILURES_BEFORE_COOLDOWN)
            .min(8);
        BASE_COOLDOWN
            .saturating_mul(1u32 << steps)
            .min(MAX_COOLDOWN)
    }
}

/// The free pool's live state: what each credential's allowance looks like,
/// and how each model behind each credential has been behaving.
///
/// A plain value with no interior mutability. The gateway wraps it in a lock
/// because several connection threads observe into it; a policy that owned
/// its own lock would decide the sharing strategy for every caller.
#[derive(Debug, Clone, Default)]
pub struct FreePool {
    /// Per credential — line 538.
    allowances: Vec<(CredentialId, Allowance)>,
    /// Per credential *and* model — line 529.
    health: Vec<(FreeResource, ResourceHealth)>,
}

impl FreePool {
    pub fn new() -> Self {
        Self::default()
    }

    /// Fold in one real unit of work.
    ///
    /// **The only mutator of health that *learns* anything in Glasshouse.**
    /// See this module's header for why that is the capability rather than an
    /// implementation detail. [`FreePool::adopt_observed`] is the one other
    /// way health changes, and it deliberately learns nothing: it carries a
    /// state some other process already learned this way.
    pub fn observe(&mut self, resource: &FreeResource, outcome: WorkloadOutcome, now: Instant) {
        self.health_entry(resource).observe(outcome, now);

        // A rate-limit answer is also a statement about the pool behind this
        // credential, so it lands on the allowance too — again from real
        // work, and again per credential rather than per provider.
        if let WorkloadOutcome::RateLimited { retry_after } = outcome {
            let reading = PoolReading {
                remaining: Some(0),
                resets_in: retry_after,
                ..PoolReading::default()
            };
            self.record_pool(resource.credential(), &reading, now);
        }
    }

    /// Adopt a health state **another process** already observed about
    /// `resource` — capability map line 1599's bridge, and the only entry
    /// point that does not learn.
    ///
    /// # Why this is not [`FreePool::observe`]
    ///
    /// `observe` takes one outcome and derives the rest: it counts the
    /// failure, and it computes the cooldown itself from `BASE_COOLDOWN` or
    /// the provider's stated `retry_after`. There is no outcome here to
    /// derive anything from. A caller holding a persisted reading knows the
    /// failure count and the deadline as *facts already established*, and
    /// replaying them through `observe` would manufacture a cooldown length
    /// this pool invented rather than the one the gateway actually granted.
    ///
    /// # `cooling_down_until` is the caller's conversion, and that is
    /// deliberate
    ///
    /// [`Instant`] has no epoch, so a deadline that crossed a process
    /// boundary as a wall-clock second can only be placed on this process's
    /// monotonic clock by something holding **both clocks read at the same
    /// moment**. This pool holds neither.
    /// [`crate::provider::telemetry::GatewayHealthReading::cooling_down_until`]
    /// is that conversion and states the rule this method depends on: a
    /// deadline that has already elapsed arrives as `None` — *not cooling
    /// down* — never as an `Instant` in the past manufactured for the sake of
    /// carrying a value.
    ///
    /// Last write wins, exactly like `observe`: a resource this is called for
    /// twice holds what the second call said.
    ///
    /// # `cooldown_cause` is not carried across this bridge
    ///
    /// `GatewayHealthReading` — what actually crosses the process boundary —
    /// persists no distinction between a declared and an invented cooldown,
    /// and adding one is a schema decision outside this package's scope. An
    /// adopted `cooling_down_until` is therefore recorded with its cause
    /// unknown, which [`ResourceHealth::declared_wait_remaining`] reports as
    /// inert rather than as a guess in either direction.
    pub fn adopt_observed(
        &mut self,
        resource: &FreeResource,
        consecutive_failures: u32,
        cooling_down_until: Option<Instant>,
        credential_rejected: bool,
    ) {
        let health = self.health_entry(resource);
        health.consecutive_failures = consecutive_failures;
        health.cooling_down_until = cooling_down_until;
        health.cooldown_cause = None;
        health.credential_rejected = credential_rejected;
    }

    /// Fold in what a real response stated about a credential's request pool.
    ///
    /// Separate from [`FreePool::observe`] because the two carry different
    /// facts: an outcome says whether the work happened, a reading says what
    /// the provider claims is left. A response can carry both, one, or
    /// neither.
    pub fn record_pool(&mut self, credential: &CredentialId, reading: &PoolReading, now: Instant) {
        self.allowance_entry(credential).record(reading, now);
    }

    /// Declare that this credential is priced per token rather than pooled.
    ///
    /// Explicit rather than inferred: line 528's whole content is that the
    /// two are not the same shape, and a pool that defaulted every unknown
    /// credential to "pooled" would be inferring one from silence.
    pub fn declare_token_priced(&mut self, credential: &CredentialId) {
        match self.allowances.iter_mut().find(|(id, _)| id == credential) {
            Some((_, allowance)) => *allowance = Allowance::TokenPriced,
            None => self
                .allowances
                .push((credential.clone(), Allowance::TokenPriced)),
        }
    }

    /// This credential's allowance, or an untouched request pool.
    pub fn allowance(&self, credential: &CredentialId) -> Allowance {
        self.allowances
            .iter()
            .find(|(id, _)| id == credential)
            .map(|(_, allowance)| allowance.clone())
            .unwrap_or_else(Allowance::unknown_pool)
    }

    /// This resource's health, or a resource nothing has been observed about.
    pub fn health(&self, resource: &FreeResource) -> ResourceHealth {
        self.health
            .iter()
            .find(|(key, _)| key == resource)
            .map(|(_, health)| health.clone())
            .unwrap_or_else(ResourceHealth::new)
    }

    /// Whether this resource can be chosen right now: not cooling down, its
    /// credential not rejected, and its credential's pool not known-empty.
    pub fn is_available(&self, resource: &FreeResource, now: Instant) -> bool {
        self.health(resource).is_available(now)
            && !self.allowance(resource.credential()).is_exhausted(now)
    }

    /// Phase 9I line 537 — the next credential to try for `model` after
    /// `exhausted` could not serve it.
    ///
    /// `pool` is this provider's configured credentials, in the user's own
    /// order. The rule is a plain one and its content is the second half of
    /// the line: **`exhausted` is removed and nothing else is**, so one key's
    /// exhaustion is that key's limit and never the provider's. A rotation
    /// that concluded "this provider is out" from one key's `429` is the
    /// defect the line names.
    ///
    /// `None` means every configured credential for this provider is
    /// currently unable to serve this model — which is a fact about the
    /// credentials, and the caller may still have other providers.
    pub fn rotate_from(
        &self,
        exhausted: &CredentialId,
        pool: &[CredentialId],
        model: &str,
        now: Instant,
    ) -> Option<CredentialId> {
        pool.iter()
            .filter(|candidate| *candidate != exhausted)
            .find(|candidate| {
                self.is_available(&FreeResource::new((*candidate).clone(), model), now)
            })
            .cloned()
    }

    /// Every resource that has been observed, in a stable order, for a
    /// settings or diagnostic view.
    pub fn observed(&self) -> Vec<(FreeResource, ResourceHealth)> {
        let mut observed = self.health.clone();
        observed.sort_by_key(|(resource, _)| resource.label());
        observed
    }

    fn health_entry(&mut self, resource: &FreeResource) -> &mut ResourceHealth {
        if let Some(index) = self.health.iter().position(|(key, _)| key == resource) {
            return &mut self.health[index].1;
        }
        self.health.push((resource.clone(), ResourceHealth::new()));
        let last = self.health.len() - 1;
        &mut self.health[last].1
    }

    fn allowance_entry(&mut self, credential: &CredentialId) -> &mut Allowance {
        if let Some(index) = self.allowances.iter().position(|(id, _)| id == credential) {
            return &mut self.allowances[index].1;
        }
        self.allowances
            .push((credential.clone(), Allowance::unknown_pool()));
        let last = self.allowances.len() - 1;
        &mut self.allowances[last].1
    }
}

/// A free resource by name, as the user's settings record it.
///
/// Two names, storable in a tracked configuration file for the same reason
/// [`crate::config::RoutingModelChoice::Pinned`] already is. The credential
/// is deliberately **not** part of this: a user ordering free resources is
/// expressing a preference about models, not about which of their own keys
/// serves one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FreeResourceKey {
    pub provider: String,
    pub model: String,
}

impl FreeResourceKey {
    pub fn new(provider: impl Into<String>, model: impl Into<String>) -> Self {
        Self {
            provider: provider.into(),
            model: model.into(),
        }
    }

    fn matches(&self, resource: &FreeResource) -> bool {
        self.provider == resource.provider() && self.model == resource.model()
    }
}

/// The user's own say over the free pool — Phase 9I line 536: *"allow the
/// user to order, disable, or pin free resources from settings"*.
///
/// All three are the user's, and all three beat any ordering Glasshouse would
/// have chosen. A pin is stronger than an order and stronger than a
/// preference; a disabled resource is not chosen for any reason at all.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FreePreferences {
    order: Vec<FreeResourceKey>,
    disabled: Vec<FreeResourceKey>,
    pinned: Option<FreeResourceKey>,
}

impl FreePreferences {
    pub fn new() -> Self {
        Self::default()
    }

    /// The user's preferred order. Anything not named keeps the order it was
    /// configured in, after everything that was named.
    pub fn with_order(mut self, order: Vec<FreeResourceKey>) -> Self {
        self.order = order;
        self
    }

    pub fn with_disabled(mut self, disabled: Vec<FreeResourceKey>) -> Self {
        self.disabled = disabled;
        self
    }

    pub fn with_pin(mut self, pinned: Option<FreeResourceKey>) -> Self {
        self.pinned = pinned;
        self
    }

    pub fn order(&self) -> &[FreeResourceKey] {
        &self.order
    }

    pub fn disabled(&self) -> &[FreeResourceKey] {
        &self.disabled
    }

    pub fn pin(&self) -> Option<&FreeResourceKey> {
        self.pinned.as_ref()
    }

    pub fn is_disabled(&self, resource: &FreeResource) -> bool {
        self.disabled.iter().any(|key| key.matches(resource))
    }

    pub fn is_pinned(&self, resource: &FreeResource) -> bool {
        self.pinned
            .as_ref()
            .is_some_and(|key| key.matches(resource))
    }

    /// `candidates` with disabled resources removed and the rest in the
    /// user's order.
    ///
    /// A pin does not filter here; it is applied by the policy that chooses,
    /// so that "pinned but currently unavailable" can be reported as exactly
    /// that instead of silently looking like an empty pool.
    pub fn arrange(&self, candidates: &[FreeResource]) -> Vec<FreeResource> {
        let mut kept: Vec<FreeResource> = candidates
            .iter()
            .filter(|resource| !self.is_disabled(resource))
            .cloned()
            .collect();
        kept.sort_by_key(|resource| {
            self.order
                .iter()
                .position(|key| key.matches(resource))
                .unwrap_or(usize::MAX)
        });
        kept
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::routing::CredentialId;
    use crate::secret::SecretRef;

    fn credential(provider: &str, var: &str) -> CredentialId {
        CredentialId::new(
            provider,
            SecretRef::Environment {
                var: var.to_owned(),
            },
        )
    }

    fn resource(provider: &str, var: &str, model: &str) -> FreeResource {
        FreeResource::new(credential(provider, var), model)
    }

    /// Line 535 says "repeatedly". One 429 on a shared free tier is another
    /// user's request having arrived first.
    #[test]
    fn one_failure_is_not_a_cooldown_and_two_are() {
        let now = Instant::now();
        let mut pool = FreePool::new();
        let it = resource("openrouter", "OPENROUTER_API_KEY", "free-model");

        pool.observe(&it, WorkloadOutcome::CapacityFailure, now);
        assert!(
            pool.health(&it).is_available(now),
            "a single capacity failure must not cool a resource down"
        );

        pool.observe(&it, WorkloadOutcome::CapacityFailure, now);
        assert!(
            !pool.health(&it).is_available(now),
            "two consecutive failures must cool it down"
        );
    }

    /// The provider knows how long to wait and Glasshouse does not.
    #[test]
    fn a_providers_own_retry_after_sets_the_cooldown() {
        let now = Instant::now();
        let mut pool = FreePool::new();
        let it = resource("openrouter", "OPENROUTER_API_KEY", "free-model");
        let retry = Duration::from_secs(90);

        for _ in 0..FAILURES_BEFORE_COOLDOWN {
            pool.observe(
                &it,
                WorkloadOutcome::RateLimited {
                    retry_after: Some(retry),
                },
                now,
            );
        }

        assert_eq!(pool.health(&it).cooling_down_until(), Some(now + retry));
        assert!(!pool.is_available(&it, now));
        assert!(pool.is_available(&it, now + retry));
    }

    /// Recovery is learned from work too — the other half of line 534.
    #[test]
    fn work_that_succeeds_ends_a_cooldown_without_a_probe() {
        let now = Instant::now();
        let mut pool = FreePool::new();
        let it = resource("openrouter", "OPENROUTER_API_KEY", "free-model");

        for _ in 0..FAILURES_BEFORE_COOLDOWN {
            pool.observe(&it, WorkloadOutcome::CapacityFailure, now);
        }
        assert!(!pool.is_available(&it, now));

        pool.observe(&it, WorkloadOutcome::Served, now);
        assert!(pool.is_available(&it, now));
        assert_eq!(pool.health(&it).consecutive_failures(), 0);
    }

    /// Line 529: one busy model must not take its siblings out of service.
    #[test]
    fn health_is_per_model_behind_one_credential() {
        let now = Instant::now();
        let mut pool = FreePool::new();
        let busy = resource("openrouter", "OPENROUTER_API_KEY", "busy-model");
        let quiet = resource("openrouter", "OPENROUTER_API_KEY", "quiet-model");

        for _ in 0..FAILURES_BEFORE_COOLDOWN {
            pool.observe(&busy, WorkloadOutcome::CapacityFailure, now);
        }

        assert!(!pool.is_available(&busy, now));
        assert!(
            pool.is_available(&quiet, now),
            "a second free model behind the same key must keep its own health"
        );
    }

    /// Lines 537 and 538, in one sentence: one key's exhaustion is that key's
    /// limit.
    #[test]
    fn exhausting_one_key_leaves_the_other_key_of_the_same_router_alone() {
        let now = Instant::now();
        let mut pool = FreePool::new();
        let first = credential("openrouter", "OPENROUTER_API_KEY");
        let second = credential("openrouter", "OPENROUTER_API_KEY_2");
        let model = "free-model";

        for _ in 0..FAILURES_BEFORE_COOLDOWN {
            pool.observe(
                &FreeResource::new(first.clone(), model),
                WorkloadOutcome::RateLimited { retry_after: None },
                now,
            );
        }

        assert!(!pool.is_available(&FreeResource::new(first.clone(), model), now));
        assert!(
            pool.is_available(&FreeResource::new(second.clone(), model), now),
            "the provider's other key has its own allowance and was never used"
        );
        assert_eq!(
            pool.rotate_from(&first, &[first.clone(), second.clone()], model, now),
            Some(second),
            "rotation must move to the provider's other credential, not give up on the provider"
        );
    }

    /// A revoked key is not a busy one: waiting does not fix it, so it is not
    /// slept off.
    #[test]
    fn a_rejected_credential_is_not_a_cooldown() {
        let now = Instant::now();
        let mut pool = FreePool::new();
        let it = resource("openrouter", "OPENROUTER_API_KEY", "free-model");

        pool.observe(&it, WorkloadOutcome::CredentialRejected, now);
        let health = pool.health(&it);
        assert!(health.credential_was_rejected());
        assert_eq!(health.cooling_down_until(), None);
        assert!(!pool.is_available(&it, now + Duration::from_secs(3600)));
    }

    /// Line 528: the two limit shapes do not share arithmetic.
    #[test]
    fn a_token_priced_allowance_is_never_asked_how_many_requests_are_left() {
        let now = Instant::now();
        let mut pool = FreePool::new();
        let id = credential("nous", "NOUS_API_KEY");

        pool.declare_token_priced(&id);
        assert!(!pool.allowance(&id).is_request_pool());
        assert!(!pool.allowance(&id).is_exhausted(now));

        // And the same reading that would empty a pool does not touch it.
        pool.record_pool(
            &id,
            &PoolReading {
                remaining: Some(0),
                ..PoolReading::default()
            },
            now,
        );
        assert_eq!(pool.allowance(&id), Allowance::TokenPriced);
    }

    #[test]
    fn a_request_pool_records_what_a_real_response_stated() {
        let now = Instant::now();
        let mut pool = FreePool::new();
        let id = credential("openrouter", "OPENROUTER_API_KEY");

        assert_eq!(pool.allowance(&id), Allowance::unknown_pool());

        pool.record_pool(
            &id,
            &PoolReading {
                limit: Some(50),
                remaining: Some(0),
                resets_in: Some(Duration::from_secs(60)),
            },
            now,
        );

        assert!(pool.allowance(&id).is_exhausted(now));
        assert!(
            !pool
                .allowance(&id)
                .is_exhausted(now + Duration::from_secs(61)),
            "past the provider's own reset instant, what is left is unknown again, not zero"
        );
    }

    /// Line 536, the ordering half.
    #[test]
    fn the_users_order_wins_and_a_disabled_resource_is_not_offered() {
        let a = resource("openrouter", "OPENROUTER_API_KEY", "a-model");
        let b = resource("openrouter", "OPENROUTER_API_KEY", "b-model");
        let c = resource("nous", "NOUS_API_KEY", "c-model");

        let prefs = FreePreferences::new()
            .with_order(vec![
                FreeResourceKey::new("nous", "c-model"),
                FreeResourceKey::new("openrouter", "b-model"),
            ])
            .with_disabled(vec![FreeResourceKey::new("openrouter", "a-model")]);

        let arranged = prefs.arrange(&[a.clone(), b.clone(), c.clone()]);
        assert_eq!(arranged, vec![c, b]);
        assert!(prefs.is_disabled(&a));
    }
}
