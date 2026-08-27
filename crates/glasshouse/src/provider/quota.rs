//! Phase 32A: one provider-independent model for how much usable capacity a
//! resource has left — [`CapacityState`].
//!
//! # Where this lives, and why it is not `crate::quota`
//!
//! A [`CapacityState`] is a derived view over a
//! [`crate::provider::registry::ResourceKind`], the same way
//! [`mod@crate::provider::registry`] is a derived view over
//! [`crate::provider::templates`]. Putting it beside the type it describes
//! keeps the whole quota story in one module tree and needs no new
//! top-level module registration. Moving it to `crate::quota` later is a
//! rename plus one line in `lib.rs`; nothing here depends on the path.
//!
//! # The hard constraint this type exists to honour
//!
//! Phase 32 established that the four quota *shapes* are not the same shape:
//! a subscription has a rolling window, a metered key has a balance, a free
//! pool has a request count, and local inference has neither. A model that
//! flattened them into one "percent remaining" number would satisfy the word
//! "unified" and break the requirement in the same motion.
//!
//! So [`CapacityState`] is **not** a percentage. It is a record of several
//! *independent* pools — tokens, requests, credits, a user's own monetary
//! ceiling — each of which is separately unknown, separately inapplicable, or
//! separately measured in **the provider's own units**. A normalized
//! percentage is something [`CapacityState::normalized`] *derives* on demand
//! and carries its own raw reading with it; it is never a field, so it can
//! never be what is left after the raw reading was thrown away.
//!
//! # Why "unknown" has four different answers
//!
//! The map's own rule is that Glasshouse must never invent exact token
//! balances for opaque subscriptions, and a model that reports a number it
//! cannot know is worse than one that says `unknown`. But "unknown" is doing
//! four different jobs, and collapsing them is how a later phase talks itself
//! into filling one in:
//!
//! - [`Capacity::Inapplicable`] — there is no such pool. A local inference
//!   server has no credit balance; asking is a category error, and a caller
//!   that sees this must not go looking for the number elsewhere.
//! - [`Capacity::ProviderOpaque`] — there is such a pool and the provider
//!   publishes no number for it. A first-party subscription's remaining
//!   tokens. **This one can never become a measurement**, and that is the
//!   map's rule expressed as a state rather than as a comment.
//! - [`Capacity::Unmeasured`] — there is such a pool, the provider does
//!   publish it, and nothing has read it. Every one of these is waiting on
//!   Phase 32B, which is where telemetry lives and which does not exist yet.
//! - [`Capacity::DelegatedUpstream`] — there is such a pool and it belongs to
//!   whichever upstream this resource is currently bound to, not to this
//!   resource. The Glasshouse gateway, and only the gateway.
//!
//! # What this module does not do
//!
//! It reads nothing. There is no HTTP client, no header parser, no clock and
//! no configuration reader here, and every [`Reading`] this module can build
//! must be handed its value, its observation time and its source by a caller.
//! That is Phase 32B's job. Consequently **every pool of every
//! [`CapacityState`] the shipped binary constructs today is one of the four
//! unknown states** — which is the honest answer, and is stated in the
//! evidence ledger rather than hidden behind a type that looks populated.

use std::collections::BTreeSet;

use crate::provider::registry::{Locality, QuotaModel, ResourceKind};

/// Whether a [`NativeAmount`]'s integer is a count of whole units or of
/// millionths of one.
///
/// Fixed point, never a float, for the reason
/// [`crate::config::RouterCostMicroUsd`] already gives: a human-editable
/// integer stays exact and policy comparisons stay exact. Tokens and requests
/// come in whole units; money and provider credits are fractional, and
/// millionths is the scale this project already uses for a US dollar.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum UnitScale {
    /// The integer counts whole units — tokens, requests.
    Whole,
    /// The integer counts millionths of one unit — microdollars, and any
    /// provider credit that is not a whole number.
    Millionths,
}

/// A quantity in **the provider's own unit**, kept as the provider stated it.
///
/// Capability map line 1217 asks that provider-native quota units be
/// preserved alongside any normalized percentage. The unit is a field of the
/// amount rather than a fact remembered somewhere else, so there is no
/// operation that can carry the number and drop the unit — including
/// normalization, which refuses outright when two amounts disagree about it
/// (see [`Pool::normalized`]).
///
/// `unit` is the provider's own word, not a Glasshouse enumeration:
/// `"credits"`, `"USD"`, `"tokens"`, `"requests"`, `"input tokens"`. A closed
/// enumeration here would force a translation step at exactly the boundary
/// where the map wants the native unit kept.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeAmount {
    value: i64,
    scale: UnitScale,
    unit: String,
}

impl NativeAmount {
    /// A count of whole native units — `1500` `"tokens"`.
    pub fn whole(value: i64, unit: impl Into<String>) -> Self {
        Self {
            value,
            scale: UnitScale::Whole,
            unit: unit.into(),
        }
    }

    /// A count of millionths of a native unit — `2_500_000` `"USD"` is
    /// two dollars fifty.
    pub fn millionths(value: i64, unit: impl Into<String>) -> Self {
        Self {
            value,
            scale: UnitScale::Millionths,
            unit: unit.into(),
        }
    }

    /// The integer, at whatever [`NativeAmount::scale`] says.
    pub fn value(&self) -> i64 {
        self.value
    }

    pub fn scale(&self) -> UnitScale {
        self.scale
    }

    /// The provider's own name for this unit.
    pub fn unit(&self) -> &str {
        &self.unit
    }

    /// Whether two amounts are stated in the same unit at the same scale.
    ///
    /// Two numbers that do not answer this cannot be divided into a
    /// percentage, and [`Pool::normalized`] refuses rather than guessing a
    /// conversion nobody established.
    pub fn commensurable_with(&self, other: &NativeAmount) -> bool {
        self.scale == other.scale && self.unit == other.unit
    }
}

/// Where a reading came from.
///
/// Carried so that a number can be argued with later: "the provider said so
/// in a response header" and "the user typed it into `config.toml`" are
/// different kinds of claim, and a router weighing a stale one against a
/// fresh one needs to know which it has.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReadingSource {
    /// A response header the provider sent, named verbatim.
    ResponseHeader(String),
    /// A dedicated endpoint the provider serves, named by URL or path.
    ProviderEndpoint(String),
    /// A harness reported it about its own first-party account.
    HarnessReport(String),
    /// The user configured it. The only source for a user-defined monetary
    /// ceiling — capability map line 1203.
    UserConfiguration,
}

/// One value that was actually read, with when and where from.
///
/// Telemetry ages, so an observation time is not optional: a credit balance
/// from three weeks ago looks exactly like one from three seconds ago, which
/// is the same failure [`crate::provider::cache::ModelCatalogue::fetched_at`]
/// exists to prevent for a model list. Unix seconds, matching
/// [`crate::provider::cache::now_unix_seconds`] — this module has no clock of
/// its own and never stamps a reading itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reading<T> {
    value: T,
    observed_at_unix: i64,
    source: ReadingSource,
}

impl<T> Reading<T> {
    pub fn new(value: T, observed_at_unix: i64, source: ReadingSource) -> Self {
        Self {
            value,
            observed_at_unix,
            source,
        }
    }

    pub fn value(&self) -> &T {
        &self.value
    }

    pub fn observed_at_unix(&self) -> i64 {
        self.observed_at_unix
    }

    pub fn source(&self) -> &ReadingSource {
        &self.source
    }
}

/// What is known about one quantity — see the module documentation for why
/// "unknown" is four states rather than one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Capacity<T> {
    /// This resource has no quantity of this kind at all.
    Inapplicable,
    /// It has one, and the provider publishes no number for it. Never
    /// becomes a [`Capacity::Measured`].
    ProviderOpaque,
    /// It has one, the provider publishes it, and nothing has read it yet.
    /// Phase 32B.
    Unmeasured,
    /// It has one, and it belongs to whichever upstream this resource is
    /// currently bound to.
    DelegatedUpstream,
    /// It was read.
    Measured(Reading<T>),
}

impl<T> Capacity<T> {
    /// The reading, if there is one. `None` for all four unknown states —
    /// which is the point: a caller cannot accidentally treat "opaque" as a
    /// zero or "inapplicable" as a full tank.
    pub fn reading(&self) -> Option<&Reading<T>> {
        match self {
            Capacity::Measured(reading) => Some(reading),
            _ => None,
        }
    }

    /// The value, if it was read.
    pub fn value(&self) -> Option<&T> {
        self.reading().map(Reading::value)
    }

    pub fn is_measured(&self) -> bool {
        matches!(self, Capacity::Measured(_))
    }

    /// Whether this quantity could ever be filled in by telemetry.
    ///
    /// False for [`Capacity::Inapplicable`] (there is nothing to read) and
    /// for [`Capacity::ProviderOpaque`] (the provider does not publish it) —
    /// the two states Phase 32B must leave alone. A telemetry pass that
    /// wrote a number into either would be inventing it.
    pub fn is_readable(&self) -> bool {
        matches!(
            self,
            Capacity::Unmeasured | Capacity::DelegatedUpstream | Capacity::Measured(_)
        )
    }

    /// A short, stable word for a diagnostic.
    pub fn as_str(&self) -> &'static str {
        match self {
            Capacity::Inapplicable => "not applicable",
            Capacity::ProviderOpaque => "opaque to the provider",
            Capacity::Unmeasured => "unmeasured",
            Capacity::DelegatedUpstream => "delegated to its assigned upstream",
            Capacity::Measured(_) => "measured",
        }
    }
}

/// One pool that can run down: what it holds when full, and what is left.
///
/// Both halves are separately unknown. A provider that publishes a remaining
/// count and no ceiling is common — `x-ratelimit-remaining` without
/// `x-ratelimit-limit` — and a model that demanded both would have to invent
/// one to record the other.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pool {
    limit: Capacity<NativeAmount>,
    remaining: Capacity<NativeAmount>,
}

impl Pool {
    /// A pool this resource does not have.
    pub fn inapplicable() -> Self {
        Self {
            limit: Capacity::Inapplicable,
            remaining: Capacity::Inapplicable,
        }
    }

    /// A pool the provider keeps but does not publish.
    pub fn opaque() -> Self {
        Self {
            limit: Capacity::ProviderOpaque,
            remaining: Capacity::ProviderOpaque,
        }
    }

    /// A pool the provider publishes and nothing has read — Phase 32B.
    pub fn unmeasured() -> Self {
        Self {
            limit: Capacity::Unmeasured,
            remaining: Capacity::Unmeasured,
        }
    }

    /// A pool belonging to whichever upstream this resource is bound to.
    pub fn delegated() -> Self {
        Self {
            limit: Capacity::DelegatedUpstream,
            remaining: Capacity::DelegatedUpstream,
        }
    }

    pub fn with_limit(mut self, limit: Capacity<NativeAmount>) -> Self {
        self.limit = limit;
        self
    }

    pub fn with_remaining(mut self, remaining: Capacity<NativeAmount>) -> Self {
        self.remaining = remaining;
        self
    }

    pub fn limit(&self) -> &Capacity<NativeAmount> {
        &self.limit
    }

    pub fn remaining(&self) -> &Capacity<NativeAmount> {
        &self.remaining
    }

    /// The percentage of this pool still usable, **carrying the raw readings
    /// it was computed from**.
    ///
    /// Capability map lines 1217 and 1218 together: the normalized score may
    /// exist, and it may not be what is left after the provider's own numbers
    /// were discarded. Returning a bare `u8` would make discarding them the
    /// default; [`NormalizedCapacity`] makes keeping them the only option the
    /// type offers.
    ///
    /// `None` unless both halves were read and both are stated in the same
    /// native unit at the same scale. A remaining count in `"requests"` over
    /// a ceiling in `"tokens"` is not a percentage, and no conversion between
    /// them was ever established.
    pub fn normalized(&self) -> Option<NormalizedCapacity> {
        let remaining = self.remaining.reading()?;
        let limit = self.limit.reading()?;
        if !remaining.value().commensurable_with(limit.value()) {
            return None;
        }
        if limit.value().value() <= 0 {
            return None;
        }
        let ratio = (remaining.value().value().clamp(0, limit.value().value()) as i128 * 100)
            / limit.value().value() as i128;
        Some(NormalizedCapacity {
            percent: ratio as u8,
            remaining: remaining.clone(),
            limit: limit.clone(),
        })
    }
}

/// A normalized capacity score that cannot be separated from its evidence.
///
/// There is no constructor that takes a bare percentage: the only way to get
/// one is [`Pool::normalized`], and it clones both provider-native readings
/// in. That is capability map line 1218 as a shape rather than as a rule
/// somebody has to remember — raw telemetry is not discarded *because* a
/// score was computed, because the score is made of it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NormalizedCapacity {
    percent: u8,
    remaining: Reading<NativeAmount>,
    limit: Reading<NativeAmount>,
}

impl NormalizedCapacity {
    /// Zero to one hundred.
    pub fn percent(&self) -> u8 {
        self.percent
    }

    /// What the provider said is left, in the provider's own unit.
    pub fn remaining(&self) -> &Reading<NativeAmount> {
        &self.remaining
    }

    /// What the provider said the pool holds when full, in the same unit.
    pub fn limit(&self) -> &Reading<NativeAmount> {
        &self.limit
    }

    /// The provider's own name for the unit both readings are stated in.
    pub fn native_unit(&self) -> &str {
        self.remaining.value().unit()
    }
}

/// Token budget, kept as up to four independent pools.
///
/// Capability map line 1205 asks that input-token budget be tracked
/// independently from output-token budget **when the provider exposes
/// separate limits**, which means the model must also be able to say that a
/// provider exposes one combined pool instead — otherwise "independently" is
/// unachievable in the common case and a caller has to guess which of two
/// fields the single number went into.
///
/// So a provider with one pool measures [`TokenBudget::combined`] and leaves
/// input and output [`Capacity::Inapplicable`]; a provider with separate
/// limits does the reverse. [`TokenBudget::cached_input`] is line 1206 and is
/// its own pool either way, because cache telemetry is a separate provider
/// feature from a token limit and is present or absent on its own.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TokenBudget {
    combined: Pool,
    input: Pool,
    output: Pool,
    cached_input: Pool,
}

impl TokenBudget {
    /// Every token pool in the same state — the shape a resource-level
    /// constructor needs, since none of the four is known before telemetry.
    pub fn uniform(state: Pool) -> Self {
        Self {
            combined: state.clone(),
            input: state.clone(),
            output: state.clone(),
            cached_input: state,
        }
    }

    pub fn with_combined(mut self, pool: Pool) -> Self {
        self.combined = pool;
        self
    }

    pub fn with_input(mut self, pool: Pool) -> Self {
        self.input = pool;
        self
    }

    pub fn with_output(mut self, pool: Pool) -> Self {
        self.output = pool;
        self
    }

    pub fn with_cached_input(mut self, pool: Pool) -> Self {
        self.cached_input = pool;
        self
    }

    /// One pool covering input and output together, for a provider that
    /// exposes no separate limits.
    pub fn combined(&self) -> &Pool {
        &self.combined
    }

    pub fn input(&self) -> &Pool {
        &self.input
    }

    pub fn output(&self) -> &Pool {
        &self.output
    }

    /// Cached-input usage — capability map line 1206.
    pub fn cached_input(&self) -> &Pool {
        &self.cached_input
    }
}

/// Whether a window slides or turns.
///
/// Capability map line 1212 asks for rolling-window capacity to be tracked
/// **separately from** fixed calendar-window capacity, which is a statement
/// about them coexisting: a subscription can have a five-hour rolling
/// allowance and a monthly cap at the same time, and a model with one window
/// field would have to pick one and lose the other. [`Windows`] therefore
/// holds one of each rather than a discriminant.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowShape {
    /// Capacity replenishes continuously across a trailing period.
    Rolling,
    /// Capacity resets at a fixed calendar boundary.
    Calendar,
}

/// One quota window: what it holds, when it started, when it resets.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowCapacity {
    shape: WindowShape,
    pool: Pool,
    started_at_unix: Capacity<i64>,
    resets_at_unix: Capacity<i64>,
}

impl WindowCapacity {
    /// A window whose pool and both timestamps are all in the same unknown
    /// state.
    pub fn uniform(shape: WindowShape, pool: Pool, unknown: Capacity<i64>) -> Self {
        Self {
            shape,
            pool,
            started_at_unix: unknown.clone(),
            resets_at_unix: unknown,
        }
    }

    pub fn with_started_at(mut self, started: Capacity<i64>) -> Self {
        self.started_at_unix = started;
        self
    }

    pub fn with_resets_at(mut self, resets: Capacity<i64>) -> Self {
        self.resets_at_unix = resets;
        self
    }

    pub fn with_pool(mut self, pool: Pool) -> Self {
        self.pool = pool;
        self
    }

    pub fn shape(&self) -> WindowShape {
        self.shape
    }

    pub fn pool(&self) -> &Pool {
        &self.pool
    }

    /// When the current window began — capability map line 1210.
    pub fn started_at_unix(&self) -> &Capacity<i64> {
        &self.started_at_unix
    }

    /// When the current window resets — capability map line 1211.
    pub fn resets_at_unix(&self) -> &Capacity<i64> {
        &self.resets_at_unix
    }
}

/// A resource's rolling window and its calendar window, tracked apart.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Windows {
    rolling: WindowCapacity,
    calendar: WindowCapacity,
}

impl Windows {
    /// Both windows in the same unknown state.
    pub fn uniform(pool: Pool, unknown: Capacity<i64>) -> Self {
        Self {
            rolling: WindowCapacity::uniform(WindowShape::Rolling, pool.clone(), unknown.clone()),
            calendar: WindowCapacity::uniform(WindowShape::Calendar, pool, unknown),
        }
    }

    pub fn with_rolling(mut self, window: WindowCapacity) -> Self {
        self.rolling = window;
        self
    }

    pub fn with_calendar(mut self, window: WindowCapacity) -> Self {
        self.calendar = window;
        self
    }

    pub fn rolling(&self) -> &WindowCapacity {
        &self.rolling
    }

    pub fn calendar(&self) -> &WindowCapacity {
        &self.calendar
    }
}

/// A request pool over a window longer than a minute.
///
/// Capability map line 1216 says "requests-per-day **or equivalent**", so the
/// window length is a field rather than a hardcoded day: a provider with a
/// weekly or per-hour pool is describable without a new variant, and a
/// caller reading `1000` never has to assume which period it covers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LongWindowRequests {
    limit: NativeAmount,
    window_seconds: i64,
}

impl LongWindowRequests {
    pub fn new(limit: NativeAmount, window_seconds: i64) -> Self {
        Self {
            limit,
            window_seconds,
        }
    }

    pub fn limit(&self) -> &NativeAmount {
        &self.limit
    }

    pub fn window_seconds(&self) -> i64 {
        self.window_seconds
    }
}

/// Rate ceilings, which bound throughput rather than exhausting.
///
/// Deliberately separate from [`CapacityState`]'s pools: hitting a
/// requests-per-minute ceiling makes a resource unusable for the next few
/// seconds, and running a credit balance to zero makes it unusable until
/// somebody pays. A model that put both in the same field would let a router
/// treat a momentary throttle as an exhausted account.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RateCeilings {
    requests_per_minute: Capacity<NativeAmount>,
    tokens_per_minute: Capacity<NativeAmount>,
    long_window_requests: Capacity<LongWindowRequests>,
    max_concurrent_requests: Capacity<NativeAmount>,
}

impl RateCeilings {
    /// Every ceiling in the same unknown state.
    pub fn uniform(amount: Capacity<NativeAmount>, long: Capacity<LongWindowRequests>) -> Self {
        Self {
            requests_per_minute: amount.clone(),
            tokens_per_minute: amount.clone(),
            long_window_requests: long,
            max_concurrent_requests: amount,
        }
    }

    pub fn with_requests_per_minute(mut self, value: Capacity<NativeAmount>) -> Self {
        self.requests_per_minute = value;
        self
    }

    pub fn with_tokens_per_minute(mut self, value: Capacity<NativeAmount>) -> Self {
        self.tokens_per_minute = value;
        self
    }

    pub fn with_long_window_requests(mut self, value: Capacity<LongWindowRequests>) -> Self {
        self.long_window_requests = value;
        self
    }

    pub fn with_max_concurrent_requests(mut self, value: Capacity<NativeAmount>) -> Self {
        self.max_concurrent_requests = value;
        self
    }

    /// Capability map line 1214.
    pub fn requests_per_minute(&self) -> &Capacity<NativeAmount> {
        &self.requests_per_minute
    }

    /// Capability map line 1215.
    pub fn tokens_per_minute(&self) -> &Capacity<NativeAmount> {
        &self.tokens_per_minute
    }

    /// Capability map line 1216.
    pub fn long_window_requests(&self) -> &Capacity<LongWindowRequests> {
        &self.long_window_requests
    }

    /// Capability map line 1213.
    pub fn max_concurrent_requests(&self) -> &Capacity<NativeAmount> {
        &self.max_concurrent_requests
    }
}

/// A unit whose exhaustion makes a resource unusable until it is topped up
/// or its window turns.
///
/// Not a rate ceiling: [`RateCeilings`] bounds how fast a resource may be
/// used, this names what running out of actually stops it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum LimitingUnit {
    /// Tokens — capability map line 1199.
    Tokens,
    /// Requests — capability map line 1200.
    Requests,
    /// Provider credits, which are the actual limiting unit on a metered
    /// account even when tokens are what is counted — capability map lines
    /// 1201 and 1208.
    Credits,
    /// A first-party subscription's allowance, whose limit the provider
    /// defines and does not publish — capability map line 1202.
    OpaqueProviderAllowance,
    /// A spending ceiling the user configured, which binds before the
    /// provider's own quota does — capability map lines 1203 and 1209.
    UserMonetaryBudget,
}

impl LimitingUnit {
    pub fn as_str(self) -> &'static str {
        match self {
            LimitingUnit::Tokens => "tokens",
            LimitingUnit::Requests => "requests",
            LimitingUnit::Credits => "credits",
            LimitingUnit::OpaqueProviderAllowance => "an opaque provider allowance",
            LimitingUnit::UserMonetaryBudget => "a user-configured monetary budget",
        }
    }
}

/// What can exhaust a resource, including the two answers that are not a set
/// of units.
///
/// "Nothing can" and "whatever limits something else" are real answers and
/// are not the empty set: an empty set of limiting units would read as
/// "nothing can exhaust it", which is true of local inference and false of
/// the gateway. Capability map line 1204 asks specifically that effectively
/// unlimited local inference be representable *separately from* remote
/// quota, and [`LimitingUnits::None`] is that separation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LimitingUnits {
    /// Nothing can exhaust this resource — a local inference server.
    None,
    /// Whatever limits the upstream this resource is currently bound to —
    /// the Glasshouse gateway, and only the gateway.
    Delegated,
    /// These units can. Never empty; use [`LimitingUnits::None`] for that.
    These(BTreeSet<LimitingUnit>),
}

impl LimitingUnits {
    /// The units named, or none for [`LimitingUnits::None`] and
    /// [`LimitingUnits::Delegated`] — both of which are answers about the
    /// resource rather than a list of units, and neither of which a caller
    /// may iterate as if it were an empty list.
    pub fn named(&self) -> Option<&BTreeSet<LimitingUnit>> {
        match self {
            LimitingUnits::These(units) => Some(units),
            _ => None,
        }
    }

    pub fn includes(&self, unit: LimitingUnit) -> bool {
        self.named().is_some_and(|units| units.contains(&unit))
    }
}

/// How much usable capacity a resource has left — capability map line 1198.
///
/// Provider-independent: nothing here names a provider, a harness or a
/// protocol. A [`CapacityState`] is built for a
/// [`crate::provider::registry::ResourceKind`] by
/// [`CapacityState::for_resource`], and every field it carries is a quantity
/// any provider could in principle expose.
///
/// # What it deliberately is not
///
/// It is not a number, and it has no `percent` field. See
/// [`Pool::normalized`] and [`NormalizedCapacity`] for why the normalized
/// score is derived and carries its own evidence.
///
/// It is also not a routability decision. Whether a resource with eight
/// percent of a credit balance left should be routed to is a policy question
/// with a threshold the user already configures
/// ([`crate::config::PremiumReservePercent`]); this type answers only what is
/// known about the capacity, and answers `unknown` far more often than not
/// until Phase 32B exists.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapacityState {
    model: QuotaModel,
    locality: Locality,
    limits: LimitingUnits,
    tokens: TokenBudget,
    requests: Pool,
    credits: Pool,
    user_budget: Pool,
    windows: Windows,
    rates: RateCeilings,
}

impl CapacityState {
    /// A first-party subscription: a rolling window whose limits the provider
    /// defines and does not publish — capability map line 1202.
    ///
    /// Every token and request pool is [`Capacity::ProviderOpaque`] rather
    /// than [`Capacity::Unmeasured`], and that is the map's own rule in code:
    /// Glasshouse must never invent exact token balances for opaque
    /// subscriptions, and [`Capacity::is_readable`] answers `false` for these
    /// so a later telemetry pass cannot fill one in by accident.
    ///
    /// The window's *reset time* is [`Capacity::Unmeasured`], not opaque —
    /// harnesses do print when a subscription window turns, so that one is a
    /// number Phase 32B can legitimately read.
    ///
    /// A credit balance is [`Pool::inapplicable`] — a flat-fee subscription
    /// has none — and so is a user monetary budget: line 1203 scopes those to
    /// metered APIs, and a spending ceiling on a subscription would bound
    /// nothing.
    pub fn opaque_subscription() -> Self {
        Self {
            model: QuotaModel::RollingWindowSubscription,
            locality: Locality::Remote,
            limits: LimitingUnits::These(BTreeSet::from([LimitingUnit::OpaqueProviderAllowance])),
            tokens: TokenBudget::uniform(Pool::opaque()),
            requests: Pool::opaque(),
            credits: Pool::inapplicable(),
            user_budget: Pool::inapplicable(),
            windows: Windows::uniform(Pool::opaque(), Capacity::Unmeasured),
            rates: RateCeilings::uniform(Capacity::Unmeasured, Capacity::Unmeasured),
        }
    }

    /// A metered account, where credits are what actually runs out —
    /// capability map lines 1201 and 1208.
    ///
    /// Tokens and requests are counted too and both may be published, so both
    /// pools are [`Capacity::Unmeasured`] rather than inapplicable; what is
    /// named as *limiting* is credits, because that is the unit whose
    /// exhaustion stops the account.
    pub fn metered_balance() -> Self {
        Self {
            model: QuotaModel::MeteredBalance,
            locality: Locality::Remote,
            limits: LimitingUnits::These(BTreeSet::from([LimitingUnit::Credits])),
            tokens: TokenBudget::uniform(Pool::unmeasured()),
            requests: Pool::unmeasured(),
            credits: Pool::unmeasured(),
            user_budget: Pool::unmeasured(),
            windows: Windows::uniform(Pool::unmeasured(), Capacity::Unmeasured),
            rates: RateCeilings::uniform(Capacity::Unmeasured, Capacity::Unmeasured),
        }
    }

    /// Local inference, which cannot run out — capability map line 1204.
    ///
    /// [`LimitingUnits::None`], and every pool, window and rate ceiling
    /// [`Capacity::Inapplicable`]. Not "unmeasured": there is nothing to
    /// measure, and a telemetry pass that treated an unmeasured local server
    /// as work to do would be looking for a number that does not exist.
    /// [`CapacityState::locality`] is [`Locality::Local`], which is the other
    /// half of the line's "separately from remote quota".
    pub fn unmetered_local() -> Self {
        Self {
            model: QuotaModel::Unmetered,
            locality: Locality::Local,
            limits: LimitingUnits::None,
            tokens: TokenBudget::uniform(Pool::inapplicable()),
            requests: Pool::inapplicable(),
            credits: Pool::inapplicable(),
            user_budget: Pool::inapplicable(),
            windows: Windows::uniform(Pool::inapplicable(), Capacity::Inapplicable),
            rates: RateCeilings::uniform(Capacity::Inapplicable, Capacity::Inapplicable),
        }
    }

    /// The Glasshouse gateway, whose capacity is whichever upstream it is
    /// currently bound to.
    ///
    /// Every quantity is [`Capacity::DelegatedUpstream`]. Answering
    /// [`LimitingUnits::None`] here would be the same mistake Phase 32
    /// refused when it declined to call the gateway `MeteredBalance`: a
    /// gateway bound to a metered upstream can absolutely run out, and one
    /// bound to Ollama cannot, so the honest answer names neither.
    pub fn delegated_to_upstream() -> Self {
        Self {
            model: QuotaModel::DelegatedToUpstream,
            locality: Locality::Local,
            limits: LimitingUnits::Delegated,
            tokens: TokenBudget::uniform(Pool::delegated()),
            requests: Pool::delegated(),
            credits: Pool::delegated(),
            user_budget: Pool::delegated(),
            windows: Windows::uniform(Pool::delegated(), Capacity::DelegatedUpstream),
            rates: RateCeilings::uniform(Capacity::DelegatedUpstream, Capacity::DelegatedUpstream),
        }
    }

    /// The capacity model for one kind of resource.
    ///
    /// **This is the production entry point.** `ResourceKind::quota` is
    /// implemented as `for_resource(self).model()`, so every direct-provider
    /// and gateway launch — `profile::apply_direct_provider` and
    /// `profile::apply_gateway`, which push the `"resource kind"` mechanism
    /// note Phase 32 wired — computes a [`CapacityState`] and reads its quota
    /// shape out of it. The launch path reads exactly that one projection;
    /// every pool below it is proven only by this module's own tests, which
    /// is recorded in the evidence ledger rather than implied.
    pub fn for_resource(kind: &ResourceKind) -> Self {
        match kind {
            ResourceKind::NativeSubscription { .. } => Self::opaque_subscription(),
            ResourceKind::DirectProvider {
                locality: Locality::Local,
                ..
            } => Self::unmetered_local(),
            ResourceKind::DirectProvider {
                locality: Locality::Remote,
                ..
            } => Self::metered_balance(),
            ResourceKind::GlasshouseGateway => Self::delegated_to_upstream(),
        }
    }

    /// Name a different set of limiting units — for a resource whose
    /// exhausting unit is established to be something other than its shape's
    /// default, such as a free pool that runs out of requests rather than
    /// credits.
    pub fn limited_by(mut self, limits: LimitingUnits) -> Self {
        self.limits = limits;
        self
    }

    pub fn with_tokens(mut self, tokens: TokenBudget) -> Self {
        self.tokens = tokens;
        self
    }

    pub fn with_requests(mut self, requests: Pool) -> Self {
        self.requests = requests;
        self
    }

    pub fn with_credits(mut self, credits: Pool) -> Self {
        self.credits = credits;
        self
    }

    pub fn with_user_budget(mut self, budget: Pool) -> Self {
        self.user_budget = budget;
        self
    }

    pub fn with_windows(mut self, windows: Windows) -> Self {
        self.windows = windows;
        self
    }

    pub fn with_rate_ceilings(mut self, rates: RateCeilings) -> Self {
        self.rates = rates;
        self
    }

    /// Which quota shape this resource has — Phase 32's
    /// [`QuotaModel`], projected out of the capacity model rather than
    /// computed beside it, so the two cannot disagree.
    pub fn model(&self) -> QuotaModel {
        self.model
    }

    /// Where this resource's compute runs. Carried here as well as on
    /// [`ResourceKind`] because line 1204's separation is a property of the
    /// capacity — "unlimited" means something different local and remote.
    pub fn locality(&self) -> Locality {
        self.locality
    }

    /// What can exhaust this resource.
    pub fn limiting_units(&self) -> &LimitingUnits {
        &self.limits
    }

    /// Token budget — capability map lines 1205 and 1206.
    pub fn tokens(&self) -> &TokenBudget {
        &self.tokens
    }

    /// Request count, independent of token consumption — capability map
    /// line 1207.
    pub fn requests(&self) -> &Pool {
        &self.requests
    }

    /// Provider credits, independent of raw tokens — capability map
    /// line 1208.
    pub fn credits(&self) -> &Pool {
        &self.credits
    }

    /// The user's own spending ceiling, independent of provider quota —
    /// capability map lines 1203 and 1209.
    ///
    /// Nothing populates this today: no configuration field records a
    /// spending ceiling, so there is no user-defined budget for a caller to
    /// read. See the evidence ledger.
    pub fn user_budget(&self) -> &Pool {
        &self.user_budget
    }

    /// Rolling and calendar windows, tracked apart — capability map lines
    /// 1210, 1211 and 1212.
    pub fn windows(&self) -> &Windows {
        &self.windows
    }

    /// Throughput ceilings — capability map lines 1213 to 1216.
    pub fn rate_ceilings(&self) -> &RateCeilings {
        &self.rates
    }

    /// Every exhaustible pool, in a fixed order, with the unit it belongs to.
    ///
    /// The order is stable so a diagnostic reads the same way twice; the
    /// labels are Glasshouse's own names for the pools, never the provider's
    /// units — those live on the [`NativeAmount`] inside each reading and are
    /// never flattened into this list.
    pub fn pools(&self) -> Vec<(&'static str, &Pool)> {
        vec![
            ("tokens", self.tokens.combined()),
            ("input tokens", self.tokens.input()),
            ("output tokens", self.tokens.output()),
            ("cached input tokens", self.tokens.cached_input()),
            ("requests", &self.requests),
            ("credits", &self.credits),
            ("user budget", &self.user_budget),
            ("rolling window", self.windows.rolling().pool()),
            ("calendar window", self.windows.calendar().pool()),
        ]
    }

    /// The tightest normalized score across every pool that has one, with the
    /// pool it came from.
    ///
    /// The *binding* pool, not an average: a resource with ninety percent of
    /// its tokens and two percent of its credits left has two percent of
    /// usable capacity, and averaging the two would report a resource that is
    /// about to stop working as comfortable. `None` when no pool was measured
    /// on both halves in one commensurable unit — which is every resource
    /// today, because nothing reads telemetry.
    ///
    /// Every raw reading survives this call untouched: it takes `&self`, and
    /// the value it returns carries its own evidence. Capability map
    /// line 1218.
    pub fn normalized(&self) -> Option<(&'static str, NormalizedCapacity)> {
        self.pools()
            .into_iter()
            .filter_map(|(label, pool)| pool.normalized().map(|score| (label, score)))
            .min_by_key(|(_, score)| score.percent())
    }
}

impl ResourceKind {
    /// This resource's capacity model — capability map line 1198.
    ///
    /// See [`CapacityState::for_resource`], which this delegates to and which
    /// documents what the production launch path actually reads out of it.
    pub fn capacity(&self) -> CapacityState {
        CapacityState::for_resource(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::integrations::IntegrationId;

    const OBSERVED: i64 = 1_756_000_000;

    fn header(name: &str) -> ReadingSource {
        ReadingSource::ResponseHeader(name.to_owned())
    }

    fn measured(value: i64, unit: &str, source: ReadingSource) -> Capacity<NativeAmount> {
        Capacity::Measured(Reading::new(
            NativeAmount::whole(value, unit),
            OBSERVED,
            source,
        ))
    }

    fn measured_usd(micro: i64, source: ReadingSource) -> Capacity<NativeAmount> {
        Capacity::Measured(Reading::new(
            NativeAmount::millionths(micro, "USD"),
            OBSERVED,
            source,
        ))
    }

    // --- line 1198: the model exists, is provider-independent, and is what
    // the quota shape is projected from ------------------------------------

    /// The production launch path calls `ResourceKind::quota`. If that were
    /// computed beside `CapacityState` instead of out of it, the two could
    /// disagree and this module would be a type nothing consults — which is
    /// exactly what practice §5 refuses. Deleting the delegation in
    /// `ResourceKind::quota` fails this.
    #[test]
    fn every_resource_kinds_quota_shape_is_projected_out_of_its_capacity_state() {
        for kind in crate::provider::registry::registry() {
            assert_eq!(
                kind.quota(),
                kind.capacity().model(),
                "{} disagrees with its own capacity state",
                kind.label()
            );
        }
    }

    /// Provider-independence, asserted against the type rather than claimed:
    /// two entirely different providers of the same shape produce byte-equal
    /// capacity states, so nothing provider-specific leaked into one.
    #[test]
    fn two_different_remote_providers_produce_the_same_capacity_model() {
        let openrouter = ResourceKind::from_direct_provider("openrouter").capacity();
        let nvidia = ResourceKind::from_direct_provider("nvidia").capacity();
        assert_eq!(openrouter, nvidia);
    }

    // --- lines 1199-1204: the six shapes are representable, and each is
    // distinguishable from the others rather than collapsing --------------

    #[test]
    fn a_token_limited_resource_is_representable() {
        let state = CapacityState::metered_balance()
            .limited_by(LimitingUnits::These(BTreeSet::from([LimitingUnit::Tokens])))
            .with_tokens(
                TokenBudget::uniform(Pool::unmeasured()).with_combined(
                    Pool::unmeasured()
                        .with_limit(measured(
                            1_000_000,
                            "tokens",
                            header("x-ratelimit-limit-tokens"),
                        ))
                        .with_remaining(measured(
                            250_000,
                            "tokens",
                            header("x-ratelimit-remaining-tokens"),
                        )),
                ),
            );
        assert!(state.limiting_units().includes(LimitingUnit::Tokens));
        assert_eq!(
            state
                .tokens()
                .combined()
                .remaining()
                .value()
                .unwrap()
                .value(),
            250_000
        );
    }

    #[test]
    fn a_request_limited_resource_is_representable() {
        let state = CapacityState::metered_balance()
            .limited_by(LimitingUnits::These(BTreeSet::from([
                LimitingUnit::Requests,
            ])))
            .with_requests(
                Pool::unmeasured()
                    .with_limit(measured(50, "requests", header("x-ratelimit-limit")))
                    .with_remaining(measured(3, "requests", header("x-ratelimit-remaining"))),
            );
        assert!(state.limiting_units().includes(LimitingUnit::Requests));
        assert_eq!(state.requests().remaining().value().unwrap().value(), 3);
        assert_eq!(
            state.requests().remaining().value().unwrap().unit(),
            "requests"
        );
    }

    #[test]
    fn a_credit_limited_resource_is_representable_and_is_what_a_metered_provider_is() {
        let state = ResourceKind::from_direct_provider("openrouter").capacity();
        assert!(state.limiting_units().includes(LimitingUnit::Credits));
        assert_eq!(state.model(), QuotaModel::MeteredBalance);
        // The balance itself is a number nobody has read — not a zero, and
        // not an invented figure.
        assert!(!state.credits().remaining().is_measured());
        assert!(state.credits().remaining().is_readable());
    }

    /// Capability map line 1202, and the map's own rule that Glasshouse must
    /// never invent exact token balances for opaque subscriptions. The
    /// subscription's pools are not merely unread — they are unreadable, so
    /// a telemetry pass has no state to transition them out of.
    #[test]
    fn an_opaque_subscription_is_representable_and_its_token_pools_can_never_be_read() {
        let state = ResourceKind::NativeSubscription {
            harness: IntegrationId::ClaudeCode,
        }
        .capacity();
        assert!(
            state
                .limiting_units()
                .includes(LimitingUnit::OpaqueProviderAllowance)
        );
        for (label, pool) in [
            ("combined", state.tokens().combined()),
            ("input", state.tokens().input()),
            ("output", state.tokens().output()),
            ("cached", state.tokens().cached_input()),
        ] {
            assert_eq!(*pool.remaining(), Capacity::ProviderOpaque, "{label}");
            assert!(!pool.remaining().is_readable(), "{label}");
        }
        // The reset time is a different question and is legitimately
        // readable: a harness prints when the window turns.
        assert!(state.windows().rolling().resets_at_unix().is_readable());
    }

    #[test]
    fn a_user_defined_monetary_budget_for_a_metered_api_is_representable() {
        let state = ResourceKind::from_direct_provider("openrouter")
            .capacity()
            .with_user_budget(
                Pool::unmeasured()
                    .with_limit(measured_usd(20_000_000, ReadingSource::UserConfiguration))
                    .with_remaining(measured_usd(4_000_000, ReadingSource::UserConfiguration)),
            );
        let remaining = state.user_budget().remaining().value().unwrap();
        assert_eq!(remaining.value(), 4_000_000);
        assert_eq!(remaining.scale(), UnitScale::Millionths);
        assert_eq!(remaining.unit(), "USD");
        assert_eq!(
            state.user_budget().remaining().reading().unwrap().source(),
            &ReadingSource::UserConfiguration
        );
    }

    /// Capability map line 1204. Local inference is not "unmeasured remote
    /// quota" and not "delegated": it is a third answer, and the two other
    /// unlimited-looking resources answer differently.
    #[test]
    fn local_inference_is_unlimited_in_a_way_no_remote_resource_can_be() {
        let ollama = ResourceKind::from_direct_provider("ollama").capacity();
        assert_eq!(*ollama.limiting_units(), LimitingUnits::None);
        assert_eq!(ollama.locality(), Locality::Local);
        for (label, pool) in ollama.pools() {
            assert_eq!(*pool.remaining(), Capacity::Inapplicable, "{label}");
            assert!(!pool.remaining().is_readable(), "{label}");
        }

        let remote = ResourceKind::from_direct_provider("openrouter").capacity();
        assert_ne!(*remote.limiting_units(), LimitingUnits::None);
        let gateway = ResourceKind::GlasshouseGateway.capacity();
        assert_eq!(*gateway.limiting_units(), LimitingUnits::Delegated);
        assert_ne!(*gateway.limiting_units(), LimitingUnits::None);
    }

    /// `LimitingUnits::None` and `LimitingUnits::Delegated` must not be
    /// readable as an empty list — a caller that iterated `named()` would
    /// treat the gateway as unlimited.
    #[test]
    fn neither_none_nor_delegated_can_be_iterated_as_an_empty_set_of_units() {
        assert!(LimitingUnits::None.named().is_none());
        assert!(LimitingUnits::Delegated.named().is_none());
        assert!(!LimitingUnits::Delegated.includes(LimitingUnit::Credits));
    }

    // --- lines 1205-1209: independence, one pool at a time ----------------

    #[test]
    fn input_and_output_token_budgets_are_tracked_independently() {
        let tokens = TokenBudget::uniform(Pool::unmeasured())
            .with_combined(Pool::inapplicable())
            .with_input(Pool::unmeasured().with_remaining(measured(
                800,
                "input tokens",
                header("anthropic-ratelimit-input-tokens-remaining"),
            )))
            .with_output(Pool::unmeasured().with_remaining(measured(
                120,
                "output tokens",
                header("anthropic-ratelimit-output-tokens-remaining"),
            )));
        assert_eq!(tokens.input().remaining().value().unwrap().value(), 800);
        assert_eq!(tokens.output().remaining().value().unwrap().value(), 120);
        assert_ne!(tokens.input().remaining(), tokens.output().remaining());
        // A provider that exposes separate limits exposes no combined one,
        // and the model says so rather than duplicating a number.
        assert_eq!(*tokens.combined().remaining(), Capacity::Inapplicable);
    }

    #[test]
    fn cached_input_usage_is_tracked_independently_of_input_tokens() {
        let tokens = TokenBudget::uniform(Pool::unmeasured())
            .with_input(Pool::unmeasured().with_remaining(measured(
                800,
                "input tokens",
                header("anthropic-ratelimit-input-tokens-remaining"),
            )))
            .with_cached_input(Pool::unmeasured().with_remaining(measured(
                5_000,
                "cached input tokens",
                header("anthropic-ratelimit-cache-read-input-tokens-remaining"),
            )));
        assert_eq!(
            tokens.cached_input().remaining().value().unwrap().value(),
            5_000
        );
        assert_ne!(
            tokens.cached_input().remaining(),
            tokens.input().remaining()
        );
    }

    #[test]
    fn request_count_and_token_consumption_can_constrain_one_resource_at_once() {
        let state = CapacityState::metered_balance()
            .limited_by(LimitingUnits::These(BTreeSet::from([
                LimitingUnit::Requests,
                LimitingUnit::Tokens,
            ])))
            .with_requests(Pool::unmeasured().with_remaining(measured(
                2,
                "requests",
                header("x-rl-req"),
            )))
            .with_tokens(TokenBudget::uniform(Pool::unmeasured()).with_combined(
                Pool::unmeasured().with_remaining(measured(90_000, "tokens", header("x-rl-tok"))),
            ));
        assert!(state.limiting_units().includes(LimitingUnit::Requests));
        assert!(state.limiting_units().includes(LimitingUnit::Tokens));
        assert_eq!(state.requests().remaining().value().unwrap().value(), 2);
        assert_eq!(
            state
                .tokens()
                .combined()
                .remaining()
                .value()
                .unwrap()
                .value(),
            90_000
        );
    }

    #[test]
    fn credits_are_tracked_independently_of_raw_tokens() {
        let state = CapacityState::metered_balance()
            .with_credits(Pool::unmeasured().with_remaining(measured_usd(
                1_250_000,
                ReadingSource::ProviderEndpoint("/api/v1/credits".to_owned()),
            )))
            .with_tokens(TokenBudget::uniform(Pool::unmeasured()).with_combined(
                Pool::unmeasured().with_remaining(measured(90_000, "tokens", header("x-rl-tok"))),
            ));
        let credits = state.credits().remaining().value().unwrap();
        let tokens = state.tokens().combined().remaining().value().unwrap();
        assert_eq!(credits.unit(), "USD");
        assert_eq!(tokens.unit(), "tokens");
        assert!(!credits.commensurable_with(tokens));
    }

    #[test]
    fn a_user_budget_is_tracked_separately_from_the_provider_quota_it_binds_before() {
        let state = CapacityState::metered_balance()
            .with_credits(Pool::unmeasured().with_remaining(measured_usd(
                40_000_000,
                ReadingSource::ProviderEndpoint("/api/v1/credits".to_owned()),
            )))
            .with_user_budget(
                Pool::unmeasured()
                    .with_remaining(measured_usd(2_000_000, ReadingSource::UserConfiguration)),
            );
        // Forty dollars of provider credit, two dollars of the user's own
        // ceiling. Neither number overwrote the other, and their sources
        // say which is whose.
        assert_eq!(
            state.credits().remaining().value().unwrap().value(),
            40_000_000
        );
        assert_eq!(
            state.user_budget().remaining().value().unwrap().value(),
            2_000_000
        );
        assert_ne!(
            state.credits().remaining().reading().unwrap().source(),
            state.user_budget().remaining().reading().unwrap().source()
        );
    }

    // --- lines 1210-1212: windows ----------------------------------------

    #[test]
    fn a_windows_start_and_reset_are_tracked_separately_and_either_may_be_unknown() {
        let window = WindowCapacity::uniform(
            WindowShape::Rolling,
            Pool::opaque(),
            Capacity::<i64>::Unmeasured,
        )
        .with_resets_at(Capacity::Measured(Reading::new(
            OBSERVED + 3_600,
            OBSERVED,
            ReadingSource::HarnessReport("session limit line".to_owned()),
        )));
        assert_eq!(*window.started_at_unix(), Capacity::Unmeasured);
        assert_eq!(*window.resets_at_unix().value().unwrap(), OBSERVED + 3_600);
    }

    #[test]
    fn a_rolling_window_and_a_calendar_window_are_tracked_at_the_same_time() {
        let windows = Windows::uniform(Pool::unmeasured(), Capacity::<i64>::Unmeasured)
            .with_rolling(
                WindowCapacity::uniform(
                    WindowShape::Rolling,
                    Pool::unmeasured(),
                    Capacity::<i64>::Unmeasured,
                )
                .with_resets_at(Capacity::Measured(Reading::new(
                    OBSERVED + 300,
                    OBSERVED,
                    header("x-ratelimit-reset"),
                ))),
            )
            .with_calendar(
                WindowCapacity::uniform(
                    WindowShape::Calendar,
                    Pool::unmeasured(),
                    Capacity::<i64>::Unmeasured,
                )
                .with_resets_at(Capacity::Measured(Reading::new(
                    OBSERVED + 2_600_000,
                    OBSERVED,
                    ReadingSource::ProviderEndpoint("/billing".to_owned()),
                ))),
            );
        assert_eq!(windows.rolling().shape(), WindowShape::Rolling);
        assert_eq!(windows.calendar().shape(), WindowShape::Calendar);
        assert_ne!(
            windows.rolling().resets_at_unix(),
            windows.calendar().resets_at_unix()
        );
    }

    // --- lines 1213-1216: rate ceilings -----------------------------------

    #[test]
    fn every_rate_ceiling_is_its_own_field_and_a_long_window_names_its_own_period() {
        let rates = RateCeilings::uniform(Capacity::Unmeasured, Capacity::Unmeasured)
            .with_requests_per_minute(measured(
                60,
                "requests",
                header("x-ratelimit-limit-requests"),
            ))
            .with_tokens_per_minute(measured(
                40_000,
                "tokens",
                header("x-ratelimit-limit-tokens"),
            ))
            .with_max_concurrent_requests(measured(4, "requests", header("x-concurrency-limit")))
            .with_long_window_requests(Capacity::Measured(Reading::new(
                LongWindowRequests::new(NativeAmount::whole(1_000, "requests"), 86_400),
                OBSERVED,
                header("x-ratelimit-limit-requests-day"),
            )));
        assert_eq!(rates.requests_per_minute().value().unwrap().value(), 60);
        assert_eq!(rates.tokens_per_minute().value().unwrap().value(), 40_000);
        assert_eq!(rates.max_concurrent_requests().value().unwrap().value(), 4);
        let long = rates.long_window_requests().value().unwrap();
        assert_eq!(long.limit().value(), 1_000);
        assert_eq!(long.window_seconds(), 86_400);
        // A per-minute request ceiling and a per-day one are different
        // numbers about different periods, and neither is the other.
        assert_ne!(
            rates.requests_per_minute().value().unwrap().value(),
            long.limit().value()
        );
    }

    // --- lines 1217-1218: normalization never costs the raw numbers -------

    #[test]
    fn a_normalized_score_carries_the_provider_native_readings_it_was_computed_from() {
        let pool = Pool::unmeasured()
            .with_limit(measured_usd(
                10_000_000,
                ReadingSource::ProviderEndpoint("/api/v1/credits".to_owned()),
            ))
            .with_remaining(measured_usd(
                2_500_000,
                ReadingSource::ProviderEndpoint("/api/v1/credits".to_owned()),
            ));
        let score = pool.normalized().expect("both halves were read");
        assert_eq!(score.percent(), 25);
        // The percentage did not replace anything: the provider's own unit,
        // scale, numbers, observation time and source are all still here.
        assert_eq!(score.native_unit(), "USD");
        assert_eq!(score.remaining().value().value(), 2_500_000);
        assert_eq!(score.remaining().value().scale(), UnitScale::Millionths);
        assert_eq!(score.limit().value().value(), 10_000_000);
        assert_eq!(score.remaining().observed_at_unix(), OBSERVED);
        assert!(matches!(
            score.remaining().source(),
            ReadingSource::ProviderEndpoint(_)
        ));
        // And the pool itself is untouched.
        assert_eq!(pool.remaining().value().unwrap().value(), 2_500_000);
    }

    /// A percentage over two different units is not a percentage. Preserving
    /// the native unit is what makes this detectable at all — a model that
    /// kept only numbers would have divided requests by tokens and reported
    /// a confident figure.
    #[test]
    fn two_incommensurable_readings_do_not_normalize_into_a_confident_number() {
        let mismatched_unit = Pool::unmeasured()
            .with_limit(measured(100, "tokens", header("x-limit")))
            .with_remaining(measured(25, "requests", header("x-remaining")));
        assert!(mismatched_unit.normalized().is_none());

        let mismatched_scale = Pool::unmeasured()
            .with_limit(measured(100, "USD", header("x-limit")))
            .with_remaining(measured_usd(25_000_000, header("x-remaining")));
        assert!(mismatched_scale.normalized().is_none());
    }

    #[test]
    fn the_binding_pool_is_what_a_resources_normalized_capacity_reports() {
        let state = CapacityState::metered_balance()
            .with_credits(
                Pool::unmeasured()
                    .with_limit(measured_usd(10_000_000, header("x-credit-limit")))
                    .with_remaining(measured_usd(200_000, header("x-credit-remaining"))),
            )
            .with_tokens(
                TokenBudget::uniform(Pool::unmeasured()).with_combined(
                    Pool::unmeasured()
                        .with_limit(measured(1_000, "tokens", header("x-token-limit")))
                        .with_remaining(measured(900, "tokens", header("x-token-remaining"))),
                ),
            );
        let (label, score) = state.normalized().expect("two pools were measured");
        // Two percent of credits, ninety percent of tokens: the resource has
        // two percent of usable capacity, and the answer says which pool.
        assert_eq!(label, "credits");
        assert_eq!(score.percent(), 2);
        assert_eq!(score.native_unit(), "USD");
    }

    #[test]
    fn a_resource_nothing_has_measured_reports_no_normalized_score_rather_than_zero() {
        for kind in crate::provider::registry::registry() {
            assert!(
                kind.capacity().normalized().is_none(),
                "{} invented a capacity score with no telemetry behind it",
                kind.label()
            );
        }
    }

    #[test]
    fn every_pool_a_capacity_state_carries_is_listed_by_pools() {
        let state = ResourceKind::from_direct_provider("openrouter").capacity();
        let labels: Vec<&str> = state.pools().into_iter().map(|(label, _)| label).collect();
        assert_eq!(
            labels,
            vec![
                "tokens",
                "input tokens",
                "output tokens",
                "cached input tokens",
                "requests",
                "credits",
                "user budget",
                "rolling window",
                "calendar window",
            ]
        );
    }
}
