//! Phase 33A — the project-local routing evidence ledger.
//!
//! An append-oriented record of what actually happened on a routed turn
//! (line 1329), stored in `routing_observations` (`crate::database` migration
//! 11), plus rolling summaries computed **on read** from those raw rows
//! (line 1335) rather than replacing them. Every summary carries its own
//! source, window, sample size, freshness and confidence (line 1339, and see
//! [`AggregateReading`]) and stays [`None`] — "unknown" — when the sample is
//! too small to support a routing decision (line 1340), never a wide error
//! bar around a guess.
//!
//! # What a gateway exchange can actually supply, and what it cannot
//!
//! [`crate::gateway::session::SessionRouting`] is this ledger's one production
//! producer this round (see [`EvidenceLedger::record`]'s callers in
//! `crate::gateway`). It sees far less of a turn than a naive reading of line
//! 1331 suggests, and the honest limits are load-bearing for which boxes this
//! package can close:
//!
//! - **`provider`, `model`, `harness`, `quota_context`, `route`: available**,
//!   but only once a launch profile has called
//!   [`crate::gateway::session::SessionRouting::bind`]. Before that, the
//!   gateway forwards bytes for a session nothing has claimed yet, and
//!   recording a provider/model pair for it would be inventing an identity
//!   the exchange does not have. `route` is the wire protocol slug
//!   (`crate::gateway::ingress::Exchange::protocol`, private to that module),
//!   not a full [`crate::harness::pairing::ServingRoute`] — the gateway module may not
//!   name `crate::harness` at all (see its own header), so a routing
//!   observation cannot carry more identity than the ingress already exposes.
//! - **`dispatched_at`: an approximation, not the true instant.** The real
//!   moment a request left for the provider lives inside
//!   `crate::gateway::ingress::forward`, which is outside this round's
//!   partition (`gateway/ingress.rs` is not in this package's `YOURS` list).
//!   What this producer stamps instead is the instant the accept loop handed
//!   the connection to `ingress::serve` — earlier than the true dispatch by
//!   however long it takes to read the request head and stream its body to
//!   the provider, which is not bounded for a coding session's full context
//!   window. Recorded as an honest upper-bound proxy, not silently corrected.
//! - **`completed_at`: accurate.** Stamped the instant `ingress::serve`
//!   returns, which is genuinely when the exchange finished — every byte of
//!   the response has been relayed and the connection is closing.
//! - **`first_byte_at`: accurate, and the one timing column this producer
//!   added after this module's own header was first written.** Stamped the
//!   instant `crate::gateway::ingress::forward` sees the provider's status
//!   and headers arrive — before a byte of the body is read, so this is a
//!   clock reading rather than a step toward the parse this module is
//!   forbidden. `None` on every exchange that never reached a provider at
//!   all, and on the transport-failure case where one was dialled but never
//!   answered.
//! - **`first_token_at`, `first_tool_call_at`: not supplied, at all, by this
//!   producer.** Not merely unavailable to this round's partition —
//!   structurally unavailable to the ingress design itself.
//!   `crate::gateway::ingress`'s own module documentation is explicit that
//!   `crate::gateway::ingress::Exchange` (private to that module) is
//!   "structurally incapable of carrying a body," because a pass-through
//!   gateway that parsed response bytes to find the first real token would be
//!   a parser of the payload it exists to be unable to read. Line 1332's
//!   warning against treating "whitespace padding, transport keepalives, or
//!   reasoning-only deltas" as the first generated token is consequently moot
//!   for this producer: it never attempts to find one, so it cannot get it
//!   wrong, and it leaves the column `NULL` rather than fabricate a value.
//!   **These two boxes stay open.** A component that reads the response
//!   stream's own framing (the harness adapter, or a body-aware layer this
//!   project has not built) is what would have to supply them.
//! - **`tool_rounds`, `repairs`: not supplied.** The gateway serves one HTTP
//!   request per connection (`crate::gateway::ingress::serve`'s own "why one
//!   request per connection") and has no notion of a *turn* spanning several
//!   of them; a harness may make several exchanges for what a user
//!   experiences as one turn, and only something above the gateway — the
//!   harness, or the session it belongs to — can count rounds across that
//!   boundary. A *repair* is a concept nothing in this tree holds at all.
//! - **`retries`: `0`, and it is a count, not a default.** The gateway
//!   forwards each request exactly once — `crate::gateway::ingress::forward`
//!   calls `Agent::run` once, and `ureq` 3 performs no transparent retry —
//!   so every gateway row says so. A harness's own retries are separate
//!   connections and separate rows.
//! - **`failovers`: supplied.** Whether *this* exchange's outcome moved the
//!   session to another backend is decided by
//!   `crate::gateway::session::SessionRouting::observe_exchange` in the same
//!   connection thread, before the row is written, so the row can carry it:
//!   `1` for a `ChangeCause::Failover`, else `0`. A credential rotation
//!   within one provider is deliberately **not** a failover here — Phase 9I
//!   line 537 keeps the two apart, and so does this column.
//! - **`failure_class`: supplied, from framing alone.** Capability map line
//!   1364's nine-way vocabulary, [`FailureClass`], decided by
//!   `crate::gateway::session`'s `failure_class` from the status, the
//!   rate-limit headers, the byte count and how the stream ended — never
//!   from a byte of the body. `None` on a served exchange.
//! - **`input_tokens`, `output_tokens`, `cached_input_tokens`,
//!   `cost_micro_usd`: not supplied *by this producer*.** Same reason as the
//!   timing columns above: reading them means parsing a response body this
//!   module is forbidden to parse. See the second producer below, which is
//!   not a gateway and is not forbidden it.
//! - **`outcome`: a coarse proxy, not the user-visible outcome line 1334
//!   asks for.** This producer only records an observation when an exchange
//!   actually reached the provider (`Forwarded` or `Unreachable` — the same
//!   filter `crate::gateway::session::classify` already applies for Phase 9H
//!   and 9I), and maps a `2xx`/`3xx` forwarded status to
//!   [`Outcome::Succeeded`] and anything else reaching the provider to
//!   [`Outcome::Failed`]. That is a transport-level fact, not a statement
//!   about whether the turn actually helped the user — a `200` whose body
//!   describes a model error looks identical to this producer, because the
//!   body is exactly what it cannot read. Recorded because it is a real,
//!   non-fabricated signal and the schema's own `outcome` vocabulary
//!   includes it; the gap to a genuine user-visible verdict is named here
//!   rather than papered over.
//! - **`context_state`: always `unknown`** from this producer. The gateway
//!   has no cache-state signal of its own; the schema's `NOT NULL DEFAULT
//!   'unknown'` is exactly what makes that the honest default rather than a
//!   guess.
//!
//! # The second producer, and why it can read what the gateway cannot
//!
//! `crate::memory::extract` supplies the token columns the gateway leaves
//! `NULL`, and it is allowed to for a reason that does not weaken the rule
//! above. The gateway **relays** somebody else's request: the response body
//! is a byte stream `crate::gateway::ingress` is designed never to parse,
//! and that is unchanged. Memory extraction is the **disposable** path,
//! where Glasshouse builds the request itself and already deserializes the
//! whole reply document to find the assistant message in it — so `usage` is
//! a sibling key of something already parsed, not a new capability to read
//! payloads.
//!
//! What that producer supplies, through
//! [`crate::memory::extract::ModelCall::observation`]: `provider`, `model`,
//! `route` (the wire protocol slug, the same spelling the gateway uses), and
//! `input_tokens`, `output_tokens`, `cached_input_tokens` **when the
//! provider reported them**. What it leaves `NULL`, deliberately: every
//! timing column, `outcome`, the four turn counters, `purpose`, and
//! `cost_micro_usd` — see that type's own documentation for why filling a
//! column with the nearest available number is worse than leaving it empty.
//!
//! # [`ObservationSource`] for `crate::config::pairing`
//!
//! [`EvidenceLedger`] implements [`ObservationSource`], replacing
//! `NoObservations` — design decision 6. One honest gap in that
//! implementation: [`crate::harness::pairing::EvidenceKey`] is a four-part
//! identity that includes a launch profile name, and this ledger's schema has
//! nowhere to put one — the gateway that produces these rows does not see a
//! launch profile either, only a harness slug and a bound assignment (see
//! above). [`ObservedEvidenceSource::observed`] matches on harness, model and
//! route and **ignores launch profile**, which means observations from two
//! launch profiles that otherwise share a harness, model and route are
//! folded together. Recorded rather than hidden: the alternative was
//! inventing a launch-profile column no producer can fill, which is the same
//! mistake line 1333 exists to prevent for cost.

use std::sync::Mutex;

use rusqlite::{Connection, OptionalExtension, Row, params};

use crate::config::pairing::{ObservationSource, ObservedEvidence};
use crate::database::PROJECT_ID_KEY;
use crate::harness::pairing::EvidenceKey;
use crate::provider::quota::{Confidence, Freshness, Reading, ReadingSource};

/// How many reliable observations a bucket needs before
/// [`EvidenceLedger::summarize`] answers anything but "unknown" for it —
/// capability map line 1340: *"too small a sample yields unknown, not a wide
/// error bar."*
///
/// Five, matching `crate::config::pairing::CONFIDENT_AT_OBSERVATIONS` — not
/// because the two numbers must agree, but because both answer the same
/// underlying question ("how many local observations before this project
/// trusts them at all") and picking a different number here with no evidence
/// either way would be exactly the kind of unearned precision line 1234
/// exists to forbid on the quota side.
pub const MIN_SAMPLE_FOR_SUMMARY: usize = 5;

/// What `routing_observations.purpose` records for a routing-model
/// classification call — `main.rs`'s `glasshouse classify` producer writes
/// it, and [`EvidenceLedger::classification_record`] reads it back.
///
/// Spelled once, here, because two spellings of one word would silently
/// split the only producer from the only reader: `purpose` is a `TEXT`
/// column with no `CHECK` (migration 11), so nothing in the schema would
/// notice.
pub const CLASSIFICATION_PURPOSE: &str = "classification";

/// How far back [`EvidenceLedger::classification_record`] and the routing
/// economics readers look — seven days, the same window the shell's
/// route-evidence view already uses, so a routing model's record and the
/// route table beside it agree on what "recent" means.
pub const CLASSIFICATION_EVIDENCE_WINDOW_SECONDS: i64 = 7 * 24 * 60 * 60;

/// The fraction of task spend above which [`RoutingOverhead::exceeds`] says
/// so — capability map line 1466's *"non-trivial fraction of the resources
/// it is intended to save"*.
///
/// One in ten. A classifier exists to keep premium capacity for the work
/// that needs it; once it is spending a tenth as many tokens as that work
/// itself, the most it could possibly save is of the same order as what it
/// costs, and a person should be told to look at it.
pub const ROUTING_OVERHEAD_WARNING_FRACTION: f64 = 0.10;

/// Whether the response the harness ultimately saw succeeded, from this
/// producer's point of view — capability map line 1334's "final user-visible
/// outcome," with the honest caveat this module's own doc comment gives:
/// **a gateway exchange only ever supplies a transport-level proxy for this,
/// never the harness's actual verdict.**
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Succeeded,
    Failed,
    Cancelled,
    Unknown,
}

impl Outcome {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
            Self::Unknown => "unknown",
        }
    }

    fn from_stored(value: &str) -> Option<Self> {
        match value {
            "succeeded" => Some(Self::Succeeded),
            "failed" => Some(Self::Failed),
            "cancelled" => Some(Self::Cancelled),
            "unknown" => Some(Self::Unknown),
            _ => None,
        }
    }
}

/// Whether the context this turn ran in was known to be warm, known to be
/// cold, or not established either way — capability map line 1337: *"do not
/// average away cache effects."*
///
/// `Unknown` is a real, storable answer rather than the absence of a row —
/// the schema's `context_state` column is `NOT NULL DEFAULT 'unknown'` for
/// exactly this reason, and this type has no fourth, "not recorded" state to
/// keep that true by construction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ContextState {
    Warm,
    Cold,
    #[default]
    Unknown,
}

impl ContextState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Warm => "warm",
            Self::Cold => "cold",
            Self::Unknown => "unknown",
        }
    }

    fn from_stored(value: &str) -> Option<Self> {
        match value {
            "warm" => Some(Self::Warm),
            "cold" => Some(Self::Cold),
            "unknown" => Some(Self::Unknown),
            _ => None,
        }
    }
}

/// How well a stored [`ObservedCost::micro_usd`] is actually known —
/// capability map line 1333's "explicit confidence label," made unforgeable
/// by migration 11's own `CHECK` pairing the two columns.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CostConfidence {
    Exact,
    Estimated,
    Unknown,
}

impl CostConfidence {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Exact => "exact",
            Self::Estimated => "estimated",
            Self::Unknown => "unknown",
        }
    }

    fn from_stored(value: &str) -> Option<Self> {
        match value {
            "exact" => Some(Self::Exact),
            "estimated" => Some(Self::Estimated),
            "unknown" => Some(Self::Unknown),
            _ => None,
        }
    }
}

/// A monetary reading with its required confidence label attached — the two
/// always travel together, so there is no way to construct one without the
/// other. See migration 11's `CHECK (cost_micro_usd IS NULL OR
/// cost_confidence IS NOT NULL)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ObservedCost {
    pub micro_usd: i64,
    pub confidence: CostConfidence,
}

/// What kind of failure one exchange was, judged from the status line, the
/// headers, byte counts and timing alone — capability map line 1364's
/// vocabulary, and lines 1316 and 1365's separation: a rate-limit response is
/// counted apart from a transport or model failure, and cadence throttling
/// apart from a spent long-window quota.
///
/// `None` on a [`RoutingObservation`] means the exchange completed and no
/// failure was seen — a served turn — **or** that the row was written before
/// `routing_observations.failure_class` existed (`crate::database` migration
/// 18). The two are not told apart, exactly as every other nullable column on
/// this row treats a pre-migration `NULL`; [`FailureClassCounts`] keeps such
/// rows out of *served* by reading [`Outcome`] beside this.
///
/// # Stored as text with no SQL `CHECK`
///
/// The column carries no `CHECK`, for the reason
/// `crate::database::EVALUATION_KINDS` gives: a vocabulary that will grow must
/// not cost a table rebuild per value. The vocabulary lives here —
/// [`FailureClass::ALL`], [`FailureClass::as_str`], `from_stored` — and
/// `crate::database::FAILURE_CLASSES` is pinned against it by a test.
///
/// # What decides each value, and what is never read to decide it
///
/// The one place a value is chosen is `crate::gateway::session`'s
/// `failure_class`, beside `classify`. Every rule there is over a status
/// code, a rate-limit header the relay already reads in order to forward it,
/// a byte count the relay already keeps in order to relay the body, or how
/// the stream ended as its own framing said it would. **No rule reads a byte
/// of the body**: a `200` whose body describes a model error is [`None`]
/// here, because the body is exactly what the relay cannot read — the same
/// caveat [`Outcome`] already carries. The design ruling is recorded in
/// `docs/product/design-decisions.md` under *"Phase 33: framing is not
/// content"*.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FailureClass {
    /// `429` from a per-window cadence limit: the provider asked for a pause
    /// and its own headers say the window reopens soon, or say nothing.
    Throttle,
    /// The account or the long window is spent: `402`, or a `429` whose
    /// headers say nothing remains until a reset far enough out to be a quota
    /// rather than a cadence — see `crate::gateway::session`'s
    /// `EXHAUSTED_QUOTA_HORIZON_SECONDS`.
    ExhaustedQuota,
    /// The provider answered `5xx`.
    Upstream5xx,
    /// The provider did not answer in time.
    Timeout,
    /// The provider answered, and then its response stream ended before its
    /// own framing said it would — short of a declared length, or before the
    /// terminating chunk.
    StreamAbort,
    /// The provider answered a success status and a body was permitted, and
    /// zero bytes of one arrived.
    EmptyCompletion,
    /// `401` or `403`: the credential, not the provider.
    CredentialFailure,
    /// Any other `4xx`: the request, not the provider.
    RequestIncompatibility,
    /// The provider could not be reached, for a reason this vocabulary does
    /// not name — a refused connection, an unresolvable host, a TLS failure.
    Unknown,
}

impl FailureClass {
    /// Every class, in the order capability map line 1364 lists them.
    pub const ALL: [FailureClass; 9] = [
        Self::Throttle,
        Self::ExhaustedQuota,
        Self::Upstream5xx,
        Self::Timeout,
        Self::StreamAbort,
        Self::EmptyCompletion,
        Self::CredentialFailure,
        Self::RequestIncompatibility,
        Self::Unknown,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Throttle => "throttle",
            Self::ExhaustedQuota => "exhausted_quota",
            Self::Upstream5xx => "upstream_5xx",
            Self::Timeout => "timeout",
            Self::StreamAbort => "stream_abort",
            Self::EmptyCompletion => "empty_completion",
            Self::CredentialFailure => "credential_failure",
            Self::RequestIncompatibility => "request_incompatibility",
            Self::Unknown => "unknown",
        }
    }

    pub fn from_stored(value: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|class| class.as_str() == value)
    }

    /// Whether this class says something about the **provider's health**, as
    /// distinct from its cadence limit, its account's quota, the credential,
    /// or the request — capability map line 1365's third figure.
    ///
    /// A throttle and a spent quota are pacing facts about a window; a
    /// credential failure is about a key; a request incompatibility is about
    /// what the harness sent. None of those says the provider is unwell.
    /// Everything else does: it answered `5xx`, took too long, cut its own
    /// stream, produced nothing, or could not be reached at all.
    pub fn is_provider_health(self) -> bool {
        match self {
            Self::Upstream5xx
            | Self::Timeout
            | Self::StreamAbort
            | Self::EmptyCompletion
            | Self::Unknown => true,
            Self::Throttle
            | Self::ExhaustedQuota
            | Self::CredentialFailure
            | Self::RequestIncompatibility => false,
        }
    }

    fn index(self) -> usize {
        Self::ALL
            .iter()
            .position(|class| *class == self)
            .expect("every class is in ALL")
    }
}

impl std::fmt::Display for FailureClass {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.pad(self.as_str())
    }
}

/// How many exchanges in one window fell into each [`FailureClass`], beside
/// the denominator they are out of — capability map line 1316's count of
/// rate-limit responses *separately from* transport or model failures, and
/// line 1365's three figures, which this type refuses to add together: there
/// is no `failures()` total here on purpose.
///
/// Counts, not rates, so unlike [`RoutingSummary`]'s aggregates they are not
/// withheld below [`MIN_SAMPLE_FOR_SUMMARY`]: two throttles out of two
/// exchanges is a true statement about two exchanges, and it is the
/// denominator printed beside it that keeps a reader from mistaking it for a
/// rate.
///
/// # Which rows count
///
/// A row is folded in only when it recorded an [`Outcome`] at all — the
/// gateway producer always does; `crate::memory::extract`'s rows never do and
/// are not gateway exchanges, so they are neither served nor failed here. A
/// row with a class is counted under it. A row with no class and a
/// [`Outcome::Succeeded`] is *served*. A row with no class and any other
/// outcome is *unclassified*: written before migration 18, or by a producer
/// that recorded a verdict without a kind — counted in the denominator so it
/// is not silently absent, and never mistaken for served.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct FailureClassCounts {
    served: usize,
    unclassified: usize,
    by_class: [usize; FailureClass::ALL.len()],
}

impl FailureClassCounts {
    /// Fold one row in — see the type's own doc for which rows count.
    pub fn record(&mut self, outcome: Option<Outcome>, class: Option<FailureClass>) {
        match (outcome, class) {
            (None, _) => {}
            (Some(_), Some(class)) => self.by_class[class.index()] += 1,
            (Some(Outcome::Succeeded), None) => self.served += 1,
            (Some(_), None) => self.unclassified += 1,
        }
    }

    /// Every exchange these counts are out of — the denominator.
    pub fn observed(&self) -> usize {
        self.served + self.unclassified + self.by_class.iter().sum::<usize>()
    }

    /// Whether anything at all was folded in.
    pub fn is_empty(&self) -> bool {
        self.observed() == 0
    }

    /// Exchanges that completed with no failure seen.
    pub fn served(&self) -> usize {
        self.served
    }

    /// Exchanges that recorded a non-success outcome and no class — see the
    /// type's own doc.
    pub fn unclassified(&self) -> usize {
        self.unclassified
    }

    pub fn count(&self, class: FailureClass) -> usize {
        self.by_class[class.index()]
    }

    /// Line 1365's first figure: temporary cadence throttling.
    pub fn cadence_throttled(&self) -> usize {
        self.count(FailureClass::Throttle)
    }

    /// Line 1365's second figure: an exhausted long-window quota.
    pub fn exhausted_quota(&self) -> usize {
        self.count(FailureClass::ExhaustedQuota)
    }

    /// Line 1365's third figure: the provider itself failing — every class
    /// [`FailureClass::is_provider_health`] says yes to, and none it says no
    /// to.
    pub fn provider_health_failures(&self) -> usize {
        FailureClass::ALL
            .into_iter()
            .filter(|class| class.is_provider_health())
            .map(|class| self.count(class))
            .sum()
    }
}

/// What one producer has to say about one measurable turn — capability map
/// lines 1330 to 1334, before it is stored.
///
/// Every field beyond `provider` and `model` is optional, for the reason this
/// module's own header gives at length: most producers, this round's gateway
/// included, can supply only a subset, and `None` here is what becomes `NULL`
/// in the ledger — "the build that wrote this row recorded nothing here,"
/// never a zero.
#[derive(Debug, Clone, PartialEq)]
pub struct NewObservation {
    pub provider: String,
    pub model: String,
    pub route: Option<String>,
    pub quota_context: Option<String>,
    pub harness: Option<String>,
    pub purpose: Option<String>,

    pub dispatched_at_unix: Option<i64>,
    pub first_byte_at_unix: Option<i64>,
    pub first_token_at_unix: Option<i64>,
    pub first_tool_call_at_unix: Option<i64>,
    pub completed_at_unix: Option<i64>,

    pub input_tokens: Option<i64>,
    pub output_tokens: Option<i64>,
    pub cached_input_tokens: Option<i64>,
    pub cost: Option<ObservedCost>,

    pub tool_rounds: Option<i64>,
    pub retries: Option<i64>,
    pub repairs: Option<i64>,
    pub failovers: Option<i64>,
    pub outcome: Option<Outcome>,
    /// What kind of failure this was, when it was one — see [`FailureClass`].
    pub failure_class: Option<FailureClass>,

    pub context_state: ContextState,
}

impl NewObservation {
    /// A bare observation naming only what every row must: which provider,
    /// and which model. Everything else starts absent.
    pub fn new(provider: impl Into<String>, model: impl Into<String>) -> Self {
        Self {
            provider: provider.into(),
            model: model.into(),
            route: None,
            quota_context: None,
            harness: None,
            purpose: None,
            dispatched_at_unix: None,
            first_byte_at_unix: None,
            first_token_at_unix: None,
            first_tool_call_at_unix: None,
            completed_at_unix: None,
            input_tokens: None,
            output_tokens: None,
            cached_input_tokens: None,
            cost: None,
            tool_rounds: None,
            retries: None,
            repairs: None,
            failovers: None,
            outcome: None,
            failure_class: None,
            context_state: ContextState::Unknown,
        }
    }

    pub fn with_route(mut self, route: Option<impl Into<String>>) -> Self {
        self.route = route.map(Into::into);
        self
    }

    pub fn with_quota_context(mut self, quota_context: Option<impl Into<String>>) -> Self {
        self.quota_context = quota_context.map(Into::into);
        self
    }

    pub fn with_harness(mut self, harness: Option<impl Into<String>>) -> Self {
        self.harness = harness.map(Into::into);
        self
    }

    /// What this call was *for* — the axis that separates what routing spent
    /// from what the user's own work spent.
    ///
    /// `routing_observations.purpose` is a `TEXT` column with no `CHECK`, so
    /// the vocabulary is the producers' to agree on rather than the schema's
    /// to enforce; today there is exactly one producer that sets it, `main.rs`'s
    /// `glasshouse classify`, writing `"classification"`.
    ///
    /// # Why the existing producer keeps writing `NULL`
    ///
    /// [`crate::memory::extract::ModelCall::observation`] does not call this, and must
    /// not be made to: its own doc comment records that every column it could
    /// plausibly fill with a nearby value stays unwritten, and extraction's
    /// rows are already on disk with `purpose` `NULL`. Back-filling them from
    /// a builder added later would make "this build recorded nothing here"
    /// indistinguishable from "this build recorded a purpose", which is the
    /// one thing the nullable columns on this type exist to keep apart.
    pub fn with_purpose(mut self, purpose: Option<impl Into<String>>) -> Self {
        self.purpose = purpose.map(Into::into);
        self
    }

    pub fn with_timing(
        mut self,
        dispatched_at_unix: Option<i64>,
        completed_at_unix: Option<i64>,
    ) -> Self {
        self.dispatched_at_unix = dispatched_at_unix;
        self.completed_at_unix = completed_at_unix;
        self
    }

    /// Line 1331's one timing column [`Self::with_timing`] does not carry:
    /// the instant the first response byte arrived, supplied only by the one
    /// producer that can honestly observe it — the gateway relay, mid-exchange,
    /// before its own body-parsing prohibition would apply. A separate
    /// builder rather than a third parameter on [`Self::with_timing`], so
    /// every other producer's existing two-argument call is untouched by a
    /// column only one producer can ever supply. `None` becomes `NULL`,
    /// exactly like every other absent column on this type.
    pub fn with_first_byte_at(mut self, first_byte_at_unix: Option<i64>) -> Self {
        self.first_byte_at_unix = first_byte_at_unix;
        self
    }

    /// The token counts a provider reported for this turn.
    ///
    /// Three `Option`s rather than a struct, matching [`Self::with_timing`]
    /// next door: a producer that read one field and not the others passes
    /// [`None`] for the rest, and [`None`] becomes `NULL` — *this build
    /// recorded nothing here* — exactly as it does for every other optional
    /// column on this type. **A producer that did not read a count must
    /// never pass `Some(0)` for it**: the columns are nullable so that
    /// "unreported" and "zero" stay two different facts, and a consumer
    /// cannot recover the difference once it is lost.
    ///
    /// `cost_micro_usd` is deliberately not part of this. A cost needs
    /// per-model pricing, migration 11 `CHECK`s it against a
    /// `cost_confidence` label for that reason, and tokens are a thing a
    /// provider reports while a price is a thing somebody would have to
    /// supply.
    pub fn with_tokens(
        mut self,
        input_tokens: Option<i64>,
        output_tokens: Option<i64>,
        cached_input_tokens: Option<i64>,
    ) -> Self {
        self.input_tokens = input_tokens;
        self.output_tokens = output_tokens;
        self.cached_input_tokens = cached_input_tokens;
        self
    }

    pub fn with_outcome(mut self, outcome: Outcome) -> Self {
        self.outcome = Some(outcome);
        self
    }

    /// What kind of failure this exchange was — capability map line 1364.
    /// `None` is a served exchange, and it stays `None` rather than becoming
    /// a class that means "nothing": a row with no failure has no kind of
    /// failure to name.
    pub fn with_failure_class(mut self, failure_class: Option<FailureClass>) -> Self {
        self.failure_class = failure_class;
        self
    }

    /// How many times this exchange's own outcome moved the session to
    /// another backend — capability map line 1334's `failovers`, the one of
    /// its four counters a gateway exchange can honestly supply, because the
    /// failover it caused is decided in the same connection thread before
    /// its row is written (`crate::gateway::session::SessionRouting::observe_exchange`).
    ///
    /// A `u32` here and an `i64` in the ledger, so a negative count cannot be
    /// built even though the column's `CHECK` would refuse it anyway. `None`
    /// is "this producer did not count", as for every other nullable column.
    pub fn with_failovers(mut self, failovers: Option<u32>) -> Self {
        self.failovers = failovers.map(i64::from);
        self
    }

    /// How many times the request was re-sent in place before this outcome —
    /// line 1334's `retries`. The gateway forwards each request exactly once
    /// (`ureq` 3 has no transparent retry, and `crate::gateway::ingress::forward`
    /// calls `Agent::run` once), so its producer writes `Some(0)`: a count it
    /// took, not a count it declined to take.
    pub fn with_retries(mut self, retries: Option<u32>) -> Self {
        self.retries = retries.map(i64::from);
        self
    }

    pub fn with_context_state(mut self, context_state: ContextState) -> Self {
        self.context_state = context_state;
        self
    }
}

/// One observation exactly as it came out of `routing_observations` — the raw
/// row line 1335 requires to stay available beside any aggregate computed
/// from it.
#[derive(Debug, Clone, PartialEq)]
pub struct RoutingObservation {
    pub seq: i64,
    pub project_id: String,
    pub observed_at_unix: i64,

    pub provider: String,
    pub model: String,
    pub route: Option<String>,
    pub quota_context: Option<String>,
    pub harness: Option<String>,
    pub purpose: Option<String>,

    pub dispatched_at_unix: Option<i64>,
    pub first_byte_at_unix: Option<i64>,
    pub first_token_at_unix: Option<i64>,
    pub first_tool_call_at_unix: Option<i64>,
    pub completed_at_unix: Option<i64>,

    pub input_tokens: Option<i64>,
    pub output_tokens: Option<i64>,
    pub cached_input_tokens: Option<i64>,
    pub cost: Option<ObservedCost>,

    pub tool_rounds: Option<i64>,
    pub retries: Option<i64>,
    pub repairs: Option<i64>,
    pub failovers: Option<i64>,
    pub outcome: Option<Outcome>,
    /// `None` for a served exchange, and for every row written before
    /// migration 18 — see [`FailureClass`].
    pub failure_class: Option<FailureClass>,

    pub context_state: ContextState,
}

impl RoutingObservation {
    /// The wall-clock duration of this exchange, when both ends were
    /// recorded — the closest this ledger comes to a latency figure given
    /// line 1332's own honest gap (see this module's header).
    pub fn duration_ms(&self) -> Option<i64> {
        let dispatched = self.dispatched_at_unix?;
        let completed = self.completed_at_unix?;
        if completed < dispatched {
            return None;
        }
        (completed - dispatched).checked_mul(1000)
    }
}

/// Everything that can go wrong reading or writing the evidence ledger.
#[derive(Debug, thiserror::Error)]
pub enum EvidenceLedgerError {
    #[error("the project database has no project identifier bound")]
    UnboundDatabase,
    #[error("routing observation {seq} stored an unrecognized {column} value `{value}`")]
    UnknownValue {
        seq: i64,
        column: &'static str,
        value: String,
    },
    #[error(
        "an observed routing identity grouped by (provider, model, route, context_state) stored an unrecognized {column} value `{value}`"
    )]
    UnknownAggregateValue { column: &'static str, value: String },
    #[error("could not {action} in the routing evidence ledger")]
    Sql {
        action: &'static str,
        #[source]
        source: rusqlite::Error,
    },
}

fn sql_err(action: &'static str) -> impl Fn(rusqlite::Error) -> EvidenceLedgerError {
    move |source| EvidenceLedgerError::Sql { action, source }
}

/// One aggregate figure computed from raw [`RoutingObservation`] rows —
/// design decision 2: *every aggregate carries source, window, sample size,
/// freshness and confidence, never a bare number.*
///
/// Wraps [`crate::provider::quota::Reading`] rather than reinventing its
/// value/observed-at/source shape — the precedent design decision 2 names —
/// and adds exactly the two things `Reading` does not carry on its own:
/// how many raw rows went into it, and the time span they were drawn from.
#[derive(Debug, Clone, PartialEq)]
pub struct AggregateReading<T> {
    reading: Reading<T>,
    sample_count: usize,
    window_start_unix: i64,
    window_end_unix: i64,
}

impl<T> AggregateReading<T> {
    fn new(
        value: T,
        window_start_unix: i64,
        window_end_unix: i64,
        sample_count: usize,
        source: ReadingSource,
    ) -> Self {
        Self {
            reading: Reading::new(value, window_end_unix, source),
            sample_count,
            window_start_unix,
            window_end_unix,
        }
    }

    pub fn value(&self) -> &T {
        self.reading.value()
    }

    pub fn source(&self) -> &ReadingSource {
        self.reading.source()
    }

    /// How many raw observations this figure was computed from — always at
    /// least [`MIN_SAMPLE_FOR_SUMMARY`], because nothing below that count is
    /// ever wrapped in one; see [`RoutingSummary`]'s own `Option` fields.
    pub fn sample_count(&self) -> usize {
        self.sample_count
    }

    /// The observation window this figure was drawn from, as
    /// `(earliest_unix, latest_unix)`.
    pub fn window(&self) -> (i64, i64) {
        (self.window_start_unix, self.window_end_unix)
    }

    /// Whether this figure has aged past `stale_after_seconds`, measured from
    /// its most recent contributing observation.
    pub fn freshness(&self, now_unix: i64, stale_after_seconds: i64) -> Freshness {
        self.reading.freshness(now_unix, stale_after_seconds)
    }

    /// How much this figure is worth relying on.
    ///
    /// Always [`Confidence::Medium`]: every aggregate this ledger produces is
    /// Glasshouse's own count of its own gateway activity —
    /// [`ReadingSource::LocalObservation`]'s own class, matching
    /// `TelemetryClass::Observed.confidence()` — never the provider's own
    /// word, and never a derived estimate either.
    pub fn confidence(&self) -> Confidence {
        Confidence::Medium
    }
}

/// Rolling summaries for one `(provider, model, route)` identity, within one
/// [`ContextState`] bucket — capability map line 1337's separation kept all
/// the way through the aggregate, never blended back together.
///
/// Every field is `None` — "unknown" — below [`MIN_SAMPLE_FOR_SUMMARY`], per
/// line 1340; `crate::config::pairing::evidence_signal`'s own convention is
/// that an absent field contributes nothing to a routing decision, which is
/// exactly the composition this type is built to support.
#[derive(Debug, Clone, PartialEq)]
pub struct RoutingSummary {
    pub provider: String,
    pub model: String,
    pub route: Option<String>,
    pub context_state: ContextState,

    /// Median exchange duration, in milliseconds — capability map line 1339's
    /// "median."
    pub median_duration_ms: Option<AggregateReading<i64>>,
    /// 95th-percentile exchange duration, in milliseconds — line 1339's
    /// "tail latency."
    pub tail_duration_ms: Option<AggregateReading<i64>>,
    /// Exponentially-weighted moving average of exchange duration, in
    /// milliseconds — line 1339's "exponentially weighted averages."
    pub ewma_duration_ms: Option<AggregateReading<f64>>,
    /// Fraction of observations with a known outcome that were
    /// [`Outcome::Failed`] — line 1339's "failure rates."
    pub failure_rate: Option<AggregateReading<f64>>,
    /// How many of this identity's exchanges in the window fell into each
    /// [`FailureClass`], with their denominator — lines 1316 and 1365. Counts
    /// rather than rates, so **not** withheld below [`MIN_SAMPLE_FOR_SUMMARY`]
    /// like the four aggregates above; see [`FailureClassCounts`]' own doc.
    pub failure_classes: FailureClassCounts,
}

/// How much weight [`ewma`] gives the most recent observation.
///
/// A third, chosen so that roughly the last five observations dominate the
/// average — matching [`MIN_SAMPLE_FOR_SUMMARY`] rather than an unrelated
/// number, so "how many observations before this project trusts a figure"
/// and "how many observations that figure actually weighs" tell a consistent
/// story.
const EWMA_ALPHA: f64 = 1.0 / (MIN_SAMPLE_FOR_SUMMARY as f64);

fn median(mut values: Vec<i64>) -> i64 {
    values.sort_unstable();
    values[values.len() / 2]
}

fn p95(mut values: Vec<i64>) -> i64 {
    values.sort_unstable();
    let index = ((values.len() - 1) * 95) / 100;
    values[index]
}

/// The oldest-first EWMA of `values`, seeded with the first observation.
fn ewma(values: &[i64]) -> f64 {
    let mut iter = values.iter();
    let Some(&first) = iter.next() else {
        return 0.0;
    };
    let mut acc = first as f64;
    for &value in iter {
        acc = EWMA_ALPHA * value as f64 + (1.0 - EWMA_ALPHA) * acc;
    }
    acc
}

/// The identity a group of observations is read back by — `provider`,
/// `model`, `route` and `harness`, matching migration 11's own index and
/// capability map line 1338's "materially different" set. Bundled into one
/// type so [`EvidenceLedger::recent`] and [`EvidenceLedger::summarize`] stay
/// under this crate's argument-count lint rather than each taking four
/// separate identity parameters beside their own.
#[derive(Debug, Clone, Copy)]
pub struct ObservationQuery<'a> {
    pub provider: &'a str,
    pub model: &'a str,
    /// `None` matches rows recorded with no route, not "any route."
    pub route: Option<&'a str>,
    /// `None` matches rows recorded with no harness, not "any harness."
    pub harness: Option<&'a str>,
}

/// One `(provider, model, route)` identity that actually has rows in
/// `routing_observations` within a queried window, grouped further by
/// [`ContextState`] — capability map line 1762's route-evidence table and
/// line 1764's "which of warm, cold or unknown," and the missing link batch
/// 42 found and this package builds (practice §71): [`EvidenceLedger::recent`]
/// and [`EvidenceLedger::summarize`] both require the caller to already name
/// an identity via [`ObservationQuery`]; neither, nor anything else on this
/// ledger before [`EvidenceLedger::observed_identities`], can answer "which
/// identities exist at all."
///
/// `context_state` is part of the group, not a value chosen or averaged
/// across it — the same separation [`RoutingSummary`] keeps for the same
/// reason (line 1337) — so an identity that genuinely has both warm and
/// unknown rows gets one row per state here rather than one row picking a
/// winner.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObservedIdentity {
    pub provider: String,
    pub model: String,
    /// `None` means these rows were recorded with no route, matching
    /// [`ObservationQuery::route`]'s own convention.
    pub route: Option<String>,
    pub context_state: ContextState,
    sample_count: usize,
    window_start_unix: i64,
    window_end_unix: i64,
}

impl ObservedIdentity {
    /// How many raw `routing_observations` rows this identity was counted
    /// from, within the queried window — a real `COUNT(*)` over recorded
    /// rows, never an estimate and never rounded up to look confident.
    pub fn sample_count(&self) -> usize {
        self.sample_count
    }

    /// The observation window this count was drawn from, as
    /// `(earliest_unix, latest_unix)` — the same shape
    /// [`AggregateReading::window`] returns, for the same reason: a count
    /// with no window attached invites reading it as "ever," which it is
    /// not.
    pub fn window(&self) -> (i64, i64) {
        (self.window_start_unix, self.window_end_unix)
    }
}

/// Request and token consumption for one `(purpose, harness_recorded)`
/// group, within a queried window — capability map line 1464's "measure
/// routing-model token and request consumption separately from coding-agent
/// consumption," and the absent aggregate
/// [`EvidenceLedger::consumption_by_purpose`] builds: every other reader on
/// this ledger requires the caller to already name an identity, and nothing
/// before this grouped by the columns that answer *what a call was for* and
/// *whether a harness was relaying it*.
///
/// `purpose` alone is not enough to separate coding-agent consumption from
/// everything else: `purpose` is `None` for every row no producer has
/// stamped, and today that is **both** every gateway relay exchange (line
/// 1464's own "coding-agent consumption", `crate::gateway::session`, which
/// always calls [`NewObservation::with_harness`]) **and** every
/// memory-extraction call (`crate::memory::extract::ModelCall::observation`,
/// which never does) — see [`NewObservation::with_purpose`]'s doc comment
/// for why extraction's rows are not back-filled with one. `harness_recorded`
/// is what tells those two `NULL`-purpose producers apart: `true` only when
/// every row in the group named a harness, which today means gateway rows
/// and gateway rows alone.
///
/// `sample_count` is a real `COUNT(*)`, always defined. The three token
/// fields are not: each is `None` when every row in the group left that
/// column `NULL`, which is a different fact from `Some(0)` and must stay
/// one — the hazard this whole aggregate exists to avoid rendering as a
/// number. A group that mixes counted and uncounted rows sums only what was
/// counted, exactly as [`NewObservation::with_tokens`] asks every producer to
/// leave absent counts absent rather than zeroed.
#[derive(Debug, Clone, PartialEq)]
pub struct PurposeConsumption {
    pub purpose: Option<String>,
    pub harness_recorded: bool,
    pub sample_count: usize,
    pub input_tokens: Option<i64>,
    pub output_tokens: Option<i64>,
    pub cached_input_tokens: Option<i64>,
    /// How many rows in this group carried a `first_byte_at` — a real
    /// `COUNT(first_byte_at)`, always defined and honestly `0` when none did.
    /// Line 1331's gateway producer is the only writer that can ever supply
    /// this column, so today it is nonzero only for the coding-agent group.
    pub first_byte_sample_count: usize,
    /// The mean time to first byte, in milliseconds, over exactly the rows
    /// counted in [`Self::first_byte_sample_count`] — `None` when that count
    /// is `0`, never a fabricated duration for a group nothing timed.
    pub mean_time_to_first_byte_ms: Option<f64>,
}

/// What this project's ledger holds about one `(provider, model)` **as a
/// routing-model classifier** — capability map lines 1422/1432 (does it
/// come back in the schema?) and 1421/1435 (how long does it take?) — read
/// from the [`CLASSIFICATION_PURPOSE`] rows alone.
///
/// Two counts and one median, each carrying its own denominator:
///
/// - `outcomes_recorded` is the number of rows that carry a parse outcome
///   at all — [`Outcome::Succeeded`] or [`Outcome::Failed`] — and `parsed`
///   is how many of those succeeded. A row with no outcome (written by a
///   build before the producer recorded one) counts in neither: it is not
///   evidence of reliability in either direction.
/// - `timed` is how many rows carry a duration, and `median_duration_ms`
///   is their median **only** once there are at least
///   [`MIN_SAMPLE_FOR_SUMMARY`] of them — the same floor every other figure
///   on this ledger sits behind. Below it the field is `None`, which a
///   consumer must read as *unmeasured*, never as fast.
///
/// **Resolution is one second.** `dispatched_at` and `completed_at` are
/// whole Unix seconds (this module's header, on line 1332's gap), so every
/// duration here is a multiple of 1000ms, and a ceiling compared against
/// this median is honest only to the second.
///
/// Not split by [`ContextState`]: a classification call is a fresh prompt
/// every time with nothing warm to keep, and its producer records
/// [`ContextState::Unknown`] on every row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClassificationRecord {
    pub provider: String,
    pub model: String,
    /// Rows carrying [`Outcome::Succeeded`] or [`Outcome::Failed`].
    pub outcomes_recorded: usize,
    /// Of those, the rows whose reply parsed as a classification.
    pub parsed: usize,
    /// Rows carrying a duration at all.
    pub timed: usize,
    /// The median of those durations, once there are enough to trust.
    pub median_duration_ms: Option<i64>,
}

impl ClassificationRecord {
    /// The share of outcome-carrying rows that parsed, or `None` when no
    /// row carries an outcome — a ratio over a zero denominator is not a
    /// reliability of `0`.
    pub fn parsed_fraction(&self) -> Option<f64> {
        (self.outcomes_recorded > 0).then(|| self.parsed as f64 / self.outcomes_recorded as f64)
    }
}

/// Routing-model spend set against everything else — capability map line
/// 1465 — as one pure reading over
/// [`EvidenceLedger::consumption_by_purpose`]'s groups, so the arithmetic is
/// testable without a database and is rendered with its denominators rather
/// than as a bare ratio.
///
/// "Spend" is **tokens**, input plus output as the provider reported them,
/// because that is the only currency this ledger holds: `cost_micro_usd`
/// has no producer in this build (see [`NewObservation::with_tokens`]).
/// Cached input tokens are left out of the sum — providers disagree on
/// whether they are already inside `input_tokens`, and a sum that might
/// double-count is worse than one that names what it omits.
///
/// A `None` token figure means *no row in that side carried a count*, the
/// same convention [`PurposeConsumption`] keeps; a side that mixes counted
/// and uncounted rows sums only what was counted. [`Self::fraction`] is
/// `None` whenever either side is uncounted or the task side is zero, and
/// [`Self::exceeds`] never fires on an unmeasured comparison.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoutingOverhead {
    /// Rows whose `purpose` is [`CLASSIFICATION_PURPOSE`].
    pub classification_requests: usize,
    pub classification_tokens: Option<i64>,
    /// Every other row the ledger holds in the window — gateway exchanges,
    /// memory extraction, anything a later producer stamps with another
    /// purpose.
    pub task_requests: usize,
    pub task_tokens: Option<i64>,
}

impl RoutingOverhead {
    pub fn from_consumption(groups: &[PurposeConsumption]) -> Self {
        let mut overhead = Self {
            classification_requests: 0,
            classification_tokens: None,
            task_requests: 0,
            task_tokens: None,
        };
        for group in groups {
            let tokens = match (group.input_tokens, group.output_tokens) {
                (None, None) => None,
                (input, output) => Some(input.unwrap_or(0) + output.unwrap_or(0)),
            };
            let (requests, total) = if group.purpose.as_deref() == Some(CLASSIFICATION_PURPOSE) {
                (
                    &mut overhead.classification_requests,
                    &mut overhead.classification_tokens,
                )
            } else {
                (&mut overhead.task_requests, &mut overhead.task_tokens)
            };
            *requests += group.sample_count;
            if let Some(tokens) = tokens {
                *total = Some(total.unwrap_or(0) + tokens);
            }
        }
        overhead
    }

    /// Classification tokens as a fraction of task tokens, when both sides
    /// were counted and the task side is not zero.
    pub fn fraction(&self) -> Option<f64> {
        let classification = self.classification_tokens?;
        let task = self.task_tokens?;
        (task > 0).then(|| classification as f64 / task as f64)
    }

    /// Capability map line 1466: whether routing's own spend has crossed
    /// `threshold` of the task spend it exists to protect. `false` whenever
    /// [`Self::fraction`] is `None` — an unmeasured comparison is not a
    /// warning.
    pub fn exceeds(&self, threshold: f64) -> bool {
        self.fraction().is_some_and(|fraction| fraction > threshold)
    }
}

/// An open project database plus the routing observations inside it.
///
/// Owns its connection behind a [`Mutex`] rather than borrowing one, unlike
/// [`crate::memory::MemoryStore`] and
/// [`crate::checkpoint::store::CheckpointStore`]: this ledger's production
/// writer (`crate::gateway::session::SessionRouting`) is called from a fresh
/// thread per connection (`crate::gateway::mod::accept_loop`'s own "giving
/// each connection a thread"), so the store this module hands the gateway
/// must be safe to hold behind one shared `Arc` and written from many threads
/// at once. A single [`Connection`] behind a [`Mutex`] is the same answer
/// [`crate::gateway::session::SessionRouting`]'s own `State` gives for the
/// same reason.
pub struct EvidenceLedger {
    conn: Mutex<Connection>,
    project_id: String,
}

impl std::fmt::Debug for EvidenceLedger {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EvidenceLedger")
            .field("project_id", &self.project_id)
            .finish_non_exhaustive()
    }
}

impl EvidenceLedger {
    /// Open the active project's database and read its binding.
    ///
    /// The path comes from `runtime` and nowhere else — the same door
    /// [`crate::memory::ProjectMemory::open`] uses, so every check
    /// `crate::database::open` performs (the symlink refusal, the read-only
    /// refusal, the project-identity check, the migrations) applies here too.
    /// This is also the whole of this ledger's contribution to line 1343's
    /// "keep the evidence ledger physically project-scoped": there is no
    /// second constructor that accepts a path, a project id, or another
    /// project's already-open connection, so nothing built on this type can
    /// name another project's file.
    pub fn open(runtime: &crate::Runtime) -> anyhow::Result<Self> {
        let conn = crate::database::open(runtime)?;
        let project_id: Option<String> = conn
            .query_row(
                "SELECT value FROM project_metadata WHERE key = ?1",
                [PROJECT_ID_KEY],
                |row| row.get(0),
            )
            .optional()
            .map_err(sql_err("read the project identifier"))?;
        Ok(Self {
            project_id: project_id.ok_or(EvidenceLedgerError::UnboundDatabase)?,
            conn: Mutex::new(conn),
        })
    }

    pub fn project_id(&self) -> &str {
        &self.project_id
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Connection> {
        self.conn
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Append one observation. There is no corresponding `update` — see this
    /// module's own header and migration 11's own doc comment for why: line
    /// 1329's "append-oriented" is a property of this store's method list,
    /// not merely of the schema underneath it.
    ///
    /// Returns the row's `seq`.
    pub fn record(
        &self,
        new: NewObservation,
        observed_at_unix: i64,
    ) -> Result<i64, EvidenceLedgerError> {
        let (cost_micro_usd, cost_confidence) = match new.cost {
            Some(cost) => (Some(cost.micro_usd), Some(cost.confidence.as_str())),
            None => (None, None),
        };
        let conn = self.lock();
        conn.execute(
            "INSERT INTO routing_observations (
                project_id, observed_at,
                provider, model, route, quota_context, harness, purpose,
                dispatched_at, first_byte_at, first_token_at, first_tool_call_at, completed_at,
                input_tokens, output_tokens, cached_input_tokens,
                cost_micro_usd, cost_confidence,
                tool_rounds, retries, repairs, failovers, outcome,
                context_state, failure_class
            ) VALUES (
                ?1, ?2,
                ?3, ?4, ?5, ?6, ?7, ?8,
                ?9, ?10, ?11, ?12, ?13,
                ?14, ?15, ?16,
                ?17, ?18,
                ?19, ?20, ?21, ?22, ?23,
                ?24, ?25
            )",
            params![
                self.project_id,
                observed_at_unix,
                new.provider,
                new.model,
                new.route,
                new.quota_context,
                new.harness,
                new.purpose,
                new.dispatched_at_unix,
                new.first_byte_at_unix,
                new.first_token_at_unix,
                new.first_tool_call_at_unix,
                new.completed_at_unix,
                new.input_tokens,
                new.output_tokens,
                new.cached_input_tokens,
                cost_micro_usd,
                cost_confidence,
                new.tool_rounds,
                new.retries,
                new.repairs,
                new.failovers,
                new.outcome.map(Outcome::as_str),
                new.context_state.as_str(),
                new.failure_class.map(FailureClass::as_str),
            ],
        )
        .map_err(sql_err("record a routing observation"))?;
        Ok(conn.last_insert_rowid())
    }

    /// The most recent observations for one `(provider, model, route)`
    /// identity, newest first — the raw rows line 1335 requires to remain
    /// available beside [`Self::summarize`]'s aggregates, and the read
    /// `routing_observations_by_route_time` (migration 11's own index)
    /// exists to serve.
    ///
    /// `route` and `harness` match exactly, including `None`, which is
    /// deliberate: a route or harness recorded as unknown is a different fact
    /// from any named one, and this read must not conflate them.
    pub fn recent(
        &self,
        query: ObservationQuery<'_>,
        limit: usize,
    ) -> Result<Vec<RoutingObservation>, EvidenceLedgerError> {
        let conn = self.lock();
        let mut statement = conn
            .prepare(
                "SELECT * FROM routing_observations
                 WHERE provider = ?1 AND model = ?2
                   AND route IS ?3 AND harness IS ?4
                 ORDER BY observed_at DESC
                 LIMIT ?5",
            )
            .map_err(sql_err("read routing observations"))?;
        let rows = statement
            .query_map(
                params![
                    query.provider,
                    query.model,
                    query.route,
                    query.harness,
                    limit as i64
                ],
                row_to_observation,
            )
            .map_err(sql_err("read routing observations"))?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row.map_err(sql_err("read a routing observation"))??);
        }
        Ok(out)
    }

    /// Rolling summaries for one `(provider, model, route, harness)`
    /// identity, within one [`ContextState`] bucket, computed from every
    /// observation newer than `now_unix - window_seconds` — capability map
    /// line 1341's decay: nothing older than the window contributes to the
    /// aggregate, but nothing is deleted from the table to make that true. A
    /// raw row outside the window is still readable through [`Self::recent`]
    /// for as long as it exists.
    pub fn summarize(
        &self,
        query: ObservationQuery<'_>,
        context_state: ContextState,
        now_unix: i64,
        window_seconds: i64,
    ) -> Result<RoutingSummary, EvidenceLedgerError> {
        let earliest = now_unix.saturating_sub(window_seconds);
        let observations = {
            let conn = self.lock();
            let mut statement = conn
                .prepare(
                    "SELECT * FROM routing_observations
                     WHERE provider = ?1 AND model = ?2
                       AND route IS ?3 AND harness IS ?4
                       AND context_state = ?5
                       AND observed_at >= ?6 AND observed_at <= ?7
                     ORDER BY observed_at ASC",
                )
                .map_err(sql_err("read routing observations"))?;
            let rows = statement
                .query_map(
                    params![
                        query.provider,
                        query.model,
                        query.route,
                        query.harness,
                        context_state.as_str(),
                        earliest,
                        now_unix
                    ],
                    row_to_observation,
                )
                .map_err(sql_err("read routing observations"))?;
            let mut observations = Vec::new();
            for row in rows {
                observations.push(row.map_err(sql_err("read a routing observation"))??);
            }
            observations
        };

        Ok(RoutingSummary {
            provider: query.provider.to_owned(),
            model: query.model.to_owned(),
            route: query.route.map(str::to_owned),
            context_state,
            median_duration_ms: duration_aggregate(
                &observations,
                median,
                "median gateway exchange duration",
            ),
            tail_duration_ms: duration_aggregate(
                &observations,
                p95,
                "p95 gateway exchange duration",
            ),
            ewma_duration_ms: ewma_duration_aggregate(&observations),
            failure_rate: failure_rate_aggregate(&observations),
            failure_classes: failure_class_counts(&observations),
        })
    }

    /// Every provider's [`FailureClassCounts`] over the window ending at
    /// `now_unix` — capability map lines 1316 and 1365's reader, at the grain
    /// `glasshouse resources` renders: one entry per provider, across every
    /// model, route, harness and context state it was observed under.
    ///
    /// Per provider rather than per [`ObservationQuery`] identity because
    /// the question these two lines ask — *is this provider throttling me,
    /// out of quota, or unwell?* — is about the resource, and
    /// `crate::provider::resources` keys its health rendering by provider
    /// name exactly as [`crate::provider::telemetry::GatewayHealthCache`]
    /// does. Blending across context states is harmless here because these
    /// are counts of failures, not the latency figures line 1337 forbids
    /// averaging across a cache boundary.
    ///
    /// One `GROUP BY` rather than a row-by-row read: the ledger may hold a
    /// long session's every exchange, and a report should not pull each of
    /// them into memory to count nine buckets.
    pub fn failure_classes_by_provider(
        &self,
        now_unix: i64,
        window_seconds: i64,
    ) -> Result<std::collections::BTreeMap<String, FailureClassCounts>, EvidenceLedgerError> {
        let earliest = now_unix.saturating_sub(window_seconds);
        let conn = self.lock();
        let mut statement = conn
            .prepare(
                "SELECT provider, outcome, failure_class, COUNT(*) AS n
                 FROM routing_observations
                 WHERE observed_at >= ?1 AND observed_at <= ?2
                 GROUP BY provider, outcome, failure_class",
            )
            .map_err(sql_err("count routing failures by class"))?;
        let rows = statement
            .query_map(params![earliest, now_unix], |row| {
                let provider: String = row.get("provider")?;
                let outcome: Option<String> = row.get("outcome")?;
                let class: Option<String> = row.get("failure_class")?;
                let n: i64 = row.get("n")?;
                Ok((provider, outcome, class, n))
            })
            .map_err(sql_err("count routing failures by class"))?;

        let mut out: std::collections::BTreeMap<String, FailureClassCounts> = Default::default();
        for row in rows {
            let (provider, outcome, class, n) =
                row.map_err(sql_err("count routing failures by class"))?;
            // A stored value this build does not recognise is reported, not
            // guessed at — the same refusal `row_to_observation` makes. A
            // grouped row has no single `seq` to name, so `-1` says so.
            let outcome = match outcome {
                None => None,
                Some(text) => Some(Outcome::from_stored(&text).ok_or_else(|| {
                    EvidenceLedgerError::UnknownValue {
                        seq: -1,
                        column: "outcome",
                        value: text,
                    }
                })?),
            };
            let class = match class {
                None => None,
                Some(text) => Some(FailureClass::from_stored(&text).ok_or_else(|| {
                    EvidenceLedgerError::UnknownValue {
                        seq: -1,
                        column: "failure_class",
                        value: text,
                    }
                })?),
            };
            let counts = out.entry(provider).or_default();
            for _ in 0..n.max(0) {
                counts.record(outcome, class);
            }
        }
        Ok(out)
    }

    /// [`Self::summarize`] for whichever `(route, harness, context_state)`
    /// this `(provider, model)` was most recently observed under — additive,
    /// because a caller that only knows a routing selection's provider and
    /// model from configuration (never its route, harness or context-state
    /// bucket) cannot build the [`ObservationQuery`] [`Self::summarize`]
    /// requires, the same gap [`Self::observed_identities`] closed for
    /// listing rather than summarizing (practice §71). This picks the single
    /// most recently active identity for the pair and summarizes exactly
    /// that one — never blended across context states, matching every other
    /// summary this ledger returns.
    ///
    /// `Ok(None)` means no observation exists for this `(provider, model)` at
    /// all, within the window. That is a different fact from
    /// [`RoutingSummary`]'s own `None` fields (observed, but below
    /// [`MIN_SAMPLE_FOR_SUMMARY`]) — a caller that only wants "is there a
    /// figure to show" can treat both the same way, but one that wants to say
    /// *why* there is not should keep them apart.
    pub fn summarize_latest_for_model(
        &self,
        provider: &str,
        model: &str,
        now_unix: i64,
        window_seconds: i64,
    ) -> Result<Option<RoutingSummary>, EvidenceLedgerError> {
        let earliest = now_unix.saturating_sub(window_seconds);
        let identity = {
            let conn = self.lock();
            conn.query_row(
                "SELECT route, harness, context_state
                 FROM routing_observations
                 WHERE project_id = ?1 AND provider = ?2 AND model = ?3
                   AND observed_at >= ?4 AND observed_at <= ?5
                 ORDER BY observed_at DESC
                 LIMIT 1",
                params![self.project_id, provider, model, earliest, now_unix],
                |row| {
                    let route: Option<String> = row.get(0)?;
                    let harness: Option<String> = row.get(1)?;
                    let context_state: String = row.get(2)?;
                    Ok((route, harness, context_state))
                },
            )
            .optional()
            .map_err(sql_err(
                "find the most recently observed identity for a model",
            ))?
        };
        let Some((route, harness, context_text)) = identity else {
            return Ok(None);
        };
        let Some(context_state) = ContextState::from_stored(&context_text) else {
            return Err(EvidenceLedgerError::UnknownAggregateValue {
                column: "context_state",
                value: context_text,
            });
        };
        let query = ObservationQuery {
            provider,
            model,
            route: route.as_deref(),
            harness: harness.as_deref(),
        };
        Ok(Some(self.summarize(
            query,
            context_state,
            now_unix,
            window_seconds,
        )?))
    }

    /// The distinct `(provider, model, route, context_state)` identities
    /// this project has actually recorded within the last `window_seconds`,
    /// most recently active first — capability map lines 1762 and 1764, and
    /// the enumeration link batch 42 found missing (practice §71):
    /// [`Self::recent`] and [`Self::summarize`] both require the caller to
    /// already name an identity; this is the one method on this ledger that
    /// answers which identities exist at all.
    ///
    /// A `SELECT DISTINCT`, expressed as a `GROUP BY` with its own count and
    /// window — over columns `routing_observations` already has. No schema
    /// change, and [`Self::record`], [`Self::recent`], [`Self::summarize`]
    /// and [`ObservationQuery`] are all untouched. Bounded by `limit`, the
    /// same shape [`Self::recent`] takes: an unbounded listing over a
    /// growing table is a defect waiting for a busy project.
    ///
    /// Scoped to this ledger's own `project_id`, like every write this
    /// ledger makes — belt-and-suspenders alongside the physical per-project
    /// database file [`Self::open`] already guarantees, because this method,
    /// unlike [`Self::recent`] and [`Self::summarize`], reads across every
    /// identity in the table rather than one already-named one.
    pub fn observed_identities(
        &self,
        now_unix: i64,
        window_seconds: i64,
        limit: usize,
    ) -> Result<Vec<ObservedIdentity>, EvidenceLedgerError> {
        let earliest = now_unix.saturating_sub(window_seconds);
        let conn = self.lock();
        let mut statement = conn
            .prepare(
                "SELECT provider, model, route, context_state,
                        COUNT(*) AS sample_count,
                        MIN(observed_at) AS window_start,
                        MAX(observed_at) AS window_end
                 FROM routing_observations
                 WHERE project_id = ?1 AND observed_at >= ?2 AND observed_at <= ?3
                 GROUP BY provider, model, route, context_state
                 ORDER BY window_end DESC, provider ASC, model ASC, route ASC, context_state ASC
                 LIMIT ?4",
            )
            .map_err(sql_err("read observed routing identities"))?;
        let rows = statement
            .query_map(
                params![self.project_id, earliest, now_unix, limit as i64],
                row_to_identity,
            )
            .map_err(sql_err("read observed routing identities"))?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row.map_err(sql_err("read an observed routing identity"))??);
        }
        Ok(out)
    }

    /// [`PurposeConsumption`] for every `(purpose, harness_recorded)` group
    /// this ledger holds a row for, within one window — capability map line
    /// 1464, and the aggregate this module's own header says nothing
    /// computes yet.
    ///
    /// Grouped by `purpose` first, so a routing model's own spend (`purpose
    /// = "classification"` today) never gets folded into anyone else's
    /// total; and, within the `NULL`-purpose rows every other producer
    /// leaves, split again by whether a harness was recorded, because that
    /// is what actually separates coding-agent consumption
    /// (`crate::gateway::session` always names a harness) from every other
    /// `NULL`-purpose producer (`crate::memory::extract` never does) — a
    /// distinction `purpose` alone cannot make. See [`PurposeConsumption`]'s
    /// own doc comment for why grouping on `purpose` alone would still fold
    /// two different producers together.
    ///
    /// `SUM(input_tokens)`, and its two siblings, are what SQLite's own
    /// aggregate already does correctly: it skips `NULL` inputs and answers
    /// `NULL` only when a group carried none at all, never `0` for an absent
    /// count. The row reader reads that straight into the `Option<i64>`
    /// [`PurposeConsumption`] declares, with no manual accumulate-and-default
    /// in between for a mutation to weaken.
    ///
    /// `first_byte_sample_count` is a genuine `COUNT(first_byte_at)`, so it
    /// is honestly `0` — not absent — for a group nothing timed.
    /// `mean_time_to_first_byte_ms` is computed only across rows carrying
    /// **both** `first_byte_at` and `dispatched_at`, and is `NULL` (`None`)
    /// exactly when that count is `0` — SQLite's `AVG` over an empty set is
    /// already `NULL`, so there is no manual zero-guard here either.
    ///
    /// Scoped to this ledger's own `project_id`, like [`Self::observed_identities`]
    /// next door and for the same belt-and-suspenders reason: this reads
    /// across every row in the table rather than one already-named identity.
    pub fn consumption_by_purpose(
        &self,
        now_unix: i64,
        window_seconds: i64,
    ) -> Result<Vec<PurposeConsumption>, EvidenceLedgerError> {
        let earliest = now_unix.saturating_sub(window_seconds);
        let conn = self.lock();
        let mut statement = conn
            .prepare(
                "SELECT purpose,
                        (harness IS NOT NULL) AS harness_recorded,
                        COUNT(*) AS sample_count,
                        SUM(input_tokens) AS input_tokens,
                        SUM(output_tokens) AS output_tokens,
                        SUM(cached_input_tokens) AS cached_input_tokens,
                        COUNT(first_byte_at) AS first_byte_sample_count,
                        AVG(
                            CASE
                                WHEN first_byte_at IS NOT NULL AND dispatched_at IS NOT NULL
                                THEN CAST(first_byte_at - dispatched_at AS REAL) * 1000
                            END
                        ) AS mean_time_to_first_byte_ms
                 FROM routing_observations
                 WHERE project_id = ?1 AND observed_at >= ?2 AND observed_at <= ?3
                 GROUP BY purpose, harness_recorded
                 ORDER BY purpose IS NULL, purpose ASC, harness_recorded DESC",
            )
            .map_err(sql_err("read routing consumption by purpose"))?;
        let rows = statement
            .query_map(
                params![self.project_id, earliest, now_unix],
                row_to_purpose_consumption,
            )
            .map_err(sql_err("read routing consumption by purpose"))?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row.map_err(sql_err("read one purpose's routing consumption"))?);
        }
        Ok(out)
    }

    /// [`ClassificationRecord`] for one `(provider, model)` over the last
    /// `window_seconds` — the reader for capability map lines 1422/1432 and
    /// 1421/1435, and the one that makes those quantities *measured* for
    /// `crate::routing::disposable`'s classification filters.
    ///
    /// Reads only rows whose `purpose` is [`CLASSIFICATION_PURPOSE`]: a
    /// model's gateway exchanges or extraction calls say nothing about how
    /// it behaves as a classifier, and folding them in would let a model
    /// that relays fine but never returns the schema look reliable.
    ///
    /// Scoped to this ledger's own `project_id`, like every read here that
    /// is not already keyed by a full identity.
    pub fn classification_record(
        &self,
        provider: &str,
        model: &str,
        now_unix: i64,
        window_seconds: i64,
    ) -> Result<ClassificationRecord, EvidenceLedgerError> {
        let earliest = now_unix.saturating_sub(window_seconds);
        let observations = {
            let conn = self.lock();
            let mut statement = conn
                .prepare(
                    "SELECT * FROM routing_observations
                     WHERE project_id = ?1 AND provider = ?2 AND model = ?3
                       AND purpose = ?4
                       AND observed_at >= ?5 AND observed_at <= ?6
                     ORDER BY observed_at ASC",
                )
                .map_err(sql_err("read classification observations"))?;
            let rows = statement
                .query_map(
                    params![
                        self.project_id,
                        provider,
                        model,
                        CLASSIFICATION_PURPOSE,
                        earliest,
                        now_unix
                    ],
                    row_to_observation,
                )
                .map_err(sql_err("read classification observations"))?;
            let mut observations = Vec::new();
            for row in rows {
                observations.push(row.map_err(sql_err("read a classification observation"))??);
            }
            observations
        };

        let outcomes_recorded = observations
            .iter()
            .filter(|o| matches!(o.outcome, Some(Outcome::Succeeded) | Some(Outcome::Failed)))
            .count();
        let parsed = observations
            .iter()
            .filter(|o| o.outcome == Some(Outcome::Succeeded))
            .count();
        let durations: Vec<i64> = observations
            .iter()
            .filter_map(RoutingObservation::duration_ms)
            .collect();
        let timed = durations.len();
        let median_duration_ms = (timed >= MIN_SAMPLE_FOR_SUMMARY).then(|| median(durations));

        Ok(ClassificationRecord {
            provider: provider.to_owned(),
            model: model.to_owned(),
            outcomes_recorded,
            parsed,
            timed,
            median_duration_ms,
        })
    }
}

fn duration_aggregate(
    observations: &[RoutingObservation],
    reduce: fn(Vec<i64>) -> i64,
    what: &'static str,
) -> Option<AggregateReading<i64>> {
    let durations: Vec<i64> = observations
        .iter()
        .filter_map(RoutingObservation::duration_ms)
        .collect();
    if durations.len() < MIN_SAMPLE_FOR_SUMMARY {
        return None;
    }
    let window_start = observations
        .iter()
        .filter(|o| o.duration_ms().is_some())
        .map(|o| o.observed_at_unix)
        .min()?;
    let window_end = observations
        .iter()
        .filter(|o| o.duration_ms().is_some())
        .map(|o| o.observed_at_unix)
        .max()?;
    let sample_count = durations.len();
    Some(AggregateReading::new(
        reduce(durations),
        window_start,
        window_end,
        sample_count,
        ReadingSource::LocalObservation(what.to_owned()),
    ))
}

fn ewma_duration_aggregate(observations: &[RoutingObservation]) -> Option<AggregateReading<f64>> {
    let with_duration: Vec<&RoutingObservation> = observations
        .iter()
        .filter(|o| o.duration_ms().is_some())
        .collect();
    if with_duration.len() < MIN_SAMPLE_FOR_SUMMARY {
        return None;
    }
    let durations: Vec<i64> = with_duration
        .iter()
        .filter_map(|o| o.duration_ms())
        .collect();
    let window_start = with_duration.first()?.observed_at_unix;
    let window_end = with_duration.last()?.observed_at_unix;
    Some(AggregateReading::new(
        ewma(&durations),
        window_start,
        window_end,
        durations.len(),
        ReadingSource::LocalObservation(
            "exponentially weighted gateway exchange duration".to_owned(),
        ),
    ))
}

fn failure_rate_aggregate(observations: &[RoutingObservation]) -> Option<AggregateReading<f64>> {
    let with_outcome: Vec<&RoutingObservation> = observations
        .iter()
        .filter(|o| matches!(o.outcome, Some(Outcome::Succeeded) | Some(Outcome::Failed)))
        .collect();
    if with_outcome.len() < MIN_SAMPLE_FOR_SUMMARY {
        return None;
    }
    let failed = with_outcome
        .iter()
        .filter(|o| matches!(o.outcome, Some(Outcome::Failed)))
        .count();
    let window_start = with_outcome.first()?.observed_at_unix;
    let window_end = with_outcome.last()?.observed_at_unix;
    Some(AggregateReading::new(
        failed as f64 / with_outcome.len() as f64,
        window_start,
        window_end,
        with_outcome.len(),
        ReadingSource::LocalObservation("gateway exchange failure count".to_owned()),
    ))
}

fn failure_class_counts(observations: &[RoutingObservation]) -> FailureClassCounts {
    let mut counts = FailureClassCounts::default();
    for observation in observations {
        counts.record(observation.outcome, observation.failure_class);
    }
    counts
}

fn row_to_observation(
    row: &Row<'_>,
) -> rusqlite::Result<Result<RoutingObservation, EvidenceLedgerError>> {
    let seq: i64 = row.get("seq")?;

    let outcome_text: Option<String> = row.get("outcome")?;
    let outcome = match outcome_text {
        None => None,
        Some(text) => match Outcome::from_stored(&text) {
            Some(outcome) => Some(outcome),
            None => {
                return Ok(Err(EvidenceLedgerError::UnknownValue {
                    seq,
                    column: "outcome",
                    value: text,
                }));
            }
        },
    };

    let failure_class_text: Option<String> = row.get("failure_class")?;
    let failure_class = match failure_class_text {
        None => None,
        Some(text) => match FailureClass::from_stored(&text) {
            Some(class) => Some(class),
            None => {
                return Ok(Err(EvidenceLedgerError::UnknownValue {
                    seq,
                    column: "failure_class",
                    value: text,
                }));
            }
        },
    };

    let context_text: String = row.get("context_state")?;
    let Some(context_state) = ContextState::from_stored(&context_text) else {
        return Ok(Err(EvidenceLedgerError::UnknownValue {
            seq,
            column: "context_state",
            value: context_text,
        }));
    };

    let cost_micro_usd: Option<i64> = row.get("cost_micro_usd")?;
    let cost_confidence_text: Option<String> = row.get("cost_confidence")?;
    let cost = match (cost_micro_usd, cost_confidence_text) {
        (None, _) => None,
        (Some(micro_usd), Some(text)) => match CostConfidence::from_stored(&text) {
            Some(confidence) => Some(ObservedCost {
                micro_usd,
                confidence,
            }),
            None => {
                return Ok(Err(EvidenceLedgerError::UnknownValue {
                    seq,
                    column: "cost_confidence",
                    value: text,
                }));
            }
        },
        // Migration 11's own `CHECK` refuses this combination on the way in;
        // reaching it means a row was written by something that bypassed the
        // schema, and this reader reports it rather than guessing a
        // confidence nobody stated.
        (Some(_), None) => {
            return Ok(Err(EvidenceLedgerError::UnknownValue {
                seq,
                column: "cost_confidence",
                value: "absent".to_owned(),
            }));
        }
    };

    Ok(Ok(RoutingObservation {
        seq,
        project_id: row.get("project_id")?,
        observed_at_unix: row.get("observed_at")?,
        provider: row.get("provider")?,
        model: row.get("model")?,
        route: row.get("route")?,
        quota_context: row.get("quota_context")?,
        harness: row.get("harness")?,
        purpose: row.get("purpose")?,
        dispatched_at_unix: row.get("dispatched_at")?,
        first_byte_at_unix: row.get("first_byte_at")?,
        first_token_at_unix: row.get("first_token_at")?,
        first_tool_call_at_unix: row.get("first_tool_call_at")?,
        completed_at_unix: row.get("completed_at")?,
        input_tokens: row.get("input_tokens")?,
        output_tokens: row.get("output_tokens")?,
        cached_input_tokens: row.get("cached_input_tokens")?,
        cost,
        tool_rounds: row.get("tool_rounds")?,
        retries: row.get("retries")?,
        repairs: row.get("repairs")?,
        failovers: row.get("failovers")?,
        outcome,
        failure_class,
        context_state,
    }))
}

/// No enum on this row to fall through on, unlike [`row_to_identity`] next
/// door — `purpose` is a free-form nullable `TEXT` with no vocabulary this
/// module enforces, so there is no unrecognized value to reject, and a plain
/// [`rusqlite::Result`] is honest about that.
fn row_to_purpose_consumption(row: &Row<'_>) -> rusqlite::Result<PurposeConsumption> {
    let sample_count: i64 = row.get("sample_count")?;
    let first_byte_sample_count: i64 = row.get("first_byte_sample_count")?;
    Ok(PurposeConsumption {
        purpose: row.get("purpose")?,
        harness_recorded: row.get("harness_recorded")?,
        sample_count: sample_count as usize,
        input_tokens: row.get("input_tokens")?,
        output_tokens: row.get("output_tokens")?,
        cached_input_tokens: row.get("cached_input_tokens")?,
        first_byte_sample_count: first_byte_sample_count as usize,
        mean_time_to_first_byte_ms: row.get("mean_time_to_first_byte_ms")?,
    })
}

fn row_to_identity(
    row: &Row<'_>,
) -> rusqlite::Result<Result<ObservedIdentity, EvidenceLedgerError>> {
    let context_text: String = row.get("context_state")?;
    let Some(context_state) = ContextState::from_stored(&context_text) else {
        return Ok(Err(EvidenceLedgerError::UnknownAggregateValue {
            column: "context_state",
            value: context_text,
        }));
    };
    let sample_count: i64 = row.get("sample_count")?;
    Ok(Ok(ObservedIdentity {
        provider: row.get("provider")?,
        model: row.get("model")?,
        route: row.get("route")?,
        context_state,
        sample_count: sample_count as usize,
        window_start_unix: row.get("window_start")?,
        window_end_unix: row.get("window_end")?,
    }))
}

/// How old an aggregate's most recent contributing observation may be before
/// [`ObservedEvidenceSource`] stops trusting it at full strength — map line
/// 1548's "stale windows count for less." This is distinct from the window
/// [`ObservedEvidenceSource::new`] is given: a row can sit comfortably inside
/// a wide `summarize` window (`crate::routing::interactive`'s own
/// `FAILOVER_EVIDENCE_WINDOW_SECONDS` is seven days) and still be the only
/// thing behind an aggregate that has not moved in days — the window decides
/// what is read at all, this decides how much the read result is trusted.
///
/// Provisional, like [`STALE_OBSERVATION_DISCOUNT`]: a day is long enough
/// that a routing decision inside the same working session trusts it fully,
/// and short enough that "stale" and "within the seven-day evidence window"
/// stay two different words rather than one.
const EVIDENCE_STALE_AFTER_SECONDS: i64 = 24 * 60 * 60;

/// How much a stale aggregate's effective sample count is discounted before
/// [`crate::config::pairing::evidence_signal`] — and, through
/// [`ObservedEvidence::reliable_observation_count`], the native-pairing
/// prior's own decay — ever sees it.
///
/// A fraction, never zero: line 1548 asks stale evidence to count for *less*,
/// not to vanish, and reducing all the way to zero would silently reproduce
/// the "no evidence at all" case this module already represents honestly
/// (an absent [`ObservedEvidence`], not a zeroed-out one — see
/// [`ObservedEvidenceSource::observed`]'s own empty-count fallback).
/// Provisional, tuned against nothing but being large enough to prove
/// against float rounding at [`MIN_SAMPLE_FOR_SUMMARY`]'s own boundary in a
/// test.
const STALE_OBSERVATION_DISCOUNT: f64 = 0.5;

/// [`ObservationSource`] for [`crate::config::pairing`]'s pairing prior —
/// design decision 6, replacing `NoObservations` with a real implementation
/// backed by this ledger.
///
/// A thin wrapper rather than `impl ObservationSource for EvidenceLedger`
/// directly, so the window this evidence is drawn from and the minimum
/// sample it requires are visible at the call site that constructs one,
/// rather than buried as constants only this module can see.
pub struct ObservedEvidenceSource<'a> {
    ledger: &'a EvidenceLedger,
    now_unix: i64,
    window_seconds: i64,
}

impl<'a> ObservedEvidenceSource<'a> {
    pub fn new(ledger: &'a EvidenceLedger, now_unix: i64, window_seconds: i64) -> Self {
        Self {
            ledger,
            now_unix,
            window_seconds,
        }
    }
}

impl ObservationSource for ObservedEvidenceSource<'_> {
    /// See this module's own header for the one gap in this match: `key`'s
    /// launch profile is not part of the query, because nothing this ledger
    /// stores carries one.
    ///
    /// `key.route().provider` is `None` for a first-party, non-gateway
    /// route — this ledger's one producer never records an observation for
    /// one of those (see this module's header), so there is nothing to look
    /// up and this answers `None` rather than guessing a provider.
    fn observed(&self, key: &EvidenceKey) -> Option<ObservedEvidence> {
        let provider = key.route().provider.as_deref()?;
        let route = key.route().protocol.map(|protocol| protocol.slug());
        let query = ObservationQuery {
            provider,
            model: key.model().label(),
            route,
            harness: Some(key.harness().slug()),
        };
        let summary = self
            .ledger
            .summarize(
                query,
                ContextState::Unknown,
                self.now_unix,
                self.window_seconds,
            )
            .ok()?;

        let task_success_rate = summary
            .failure_rate
            .as_ref()
            .map(|reading| 1.0 - reading.value());
        // Line 1548: a stale aggregate contributes less than a fresh one at
        // the same sample count, never a fabricated number — `task_success_rate`
        // above is untouched, only how many observations the rest of this
        // struct claims to stand on. See `EVIDENCE_STALE_AFTER_SECONDS` and
        // `STALE_OBSERVATION_DISCOUNT` for why these two numbers.
        let reliable_observation_count = summary
            .failure_rate
            .as_ref()
            .map(|reading| {
                let raw = reading.sample_count();
                match reading.freshness(self.now_unix, EVIDENCE_STALE_AFTER_SECONDS) {
                    Freshness::Fresh { .. } => raw,
                    Freshness::Stale { .. } => ((raw as f64) * STALE_OBSERVATION_DISCOUNT) as usize,
                }
            })
            .unwrap_or(0);

        if reliable_observation_count == 0 {
            return None;
        }

        Some(ObservedEvidence {
            reliable_observation_count,
            task_success_rate,
            // Not supplied by this ledger's one producer today — see this
            // module's own header. `None` rather than a guess.
            usable_tool_call_rate: None,
            repair_rate: None,
            // Requires `first_byte_at`, which this ledger's gateway producer
            // never records (see this module's header) — there is no honest
            // ratio to compute.
            effective_ttfc_ratio: None,
            reliability: None,
            user_override_signal: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Cli, Runtime};
    use clap::Parser;
    use std::path::Path;

    struct Fixture {
        runtime: Runtime,
    }

    impl Fixture {
        fn new(base: &Path, name: &str) -> Self {
            let root = base.join("workspace").join(name);
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
            let runtime = crate::bootstrap(&cli, &root).unwrap();
            Self { runtime }
        }

        fn ledger(&self) -> EvidenceLedger {
            EvidenceLedger::open(&self.runtime).unwrap()
        }
    }

    fn observation(provider: &str, model: &str) -> NewObservation {
        NewObservation::new(provider, model)
            .with_route(Some("anthropic-messages"))
            .with_harness(Some("claude-code"))
    }

    #[test]
    fn a_recorded_observation_reads_back_with_every_field_it_was_given() {
        let tmp = tempfile::tempdir().unwrap();
        let fixture = Fixture::new(tmp.path(), "alpha");
        let ledger = fixture.ledger();

        let new = observation("anyrouter", "claude-opus-4-1")
            .with_timing(Some(1_000), Some(1_002))
            .with_outcome(Outcome::Succeeded)
            .with_context_state(ContextState::Warm);
        let seq = ledger.record(new, 1_002).unwrap();
        assert!(seq > 0);

        let rows = ledger
            .recent(
                ObservationQuery {
                    provider: "anyrouter",
                    model: "claude-opus-4-1",
                    route: Some("anthropic-messages"),
                    harness: Some("claude-code"),
                },
                10,
            )
            .unwrap();
        assert_eq!(rows.len(), 1);
        let row = &rows[0];
        assert_eq!(row.provider, "anyrouter");
        assert_eq!(row.model, "claude-opus-4-1");
        assert_eq!(row.route.as_deref(), Some("anthropic-messages"));
        assert_eq!(row.harness.as_deref(), Some("claude-code"));
        assert_eq!(row.dispatched_at_unix, Some(1_000));
        assert_eq!(row.completed_at_unix, Some(1_002));
        assert_eq!(row.duration_ms(), Some(2_000));
        assert_eq!(row.outcome, Some(Outcome::Succeeded));
        assert_eq!(row.context_state, ContextState::Warm);
        assert_eq!(
            row.first_byte_at_unix, None,
            "this producer never supplies it"
        );
        assert_eq!(
            row.failure_class, None,
            "a served row has no kind of failure"
        );
        assert_eq!(row.failovers, None, "this test's producer did not count");
        assert_eq!(row.retries, None);
    }

    /// Migration 18's column and line 1334's two counters the gateway can
    /// supply, through the real schema and back — including the value the
    /// `outcome` `CHECK` two columns over would never have allowed a
    /// vocabulary to grow into.
    #[test]
    fn a_failure_class_and_the_two_counters_round_trip() {
        let tmp = tempfile::tempdir().unwrap();
        let fixture = Fixture::new(tmp.path(), "alpha");
        let ledger = fixture.ledger();

        for (i, class) in FailureClass::ALL.iter().enumerate() {
            ledger
                .record(
                    observation("anyrouter", "claude-opus-4-1")
                        .with_outcome(Outcome::Failed)
                        .with_failure_class(Some(*class))
                        .with_failovers(Some(u32::from(i == 0)))
                        .with_retries(Some(0)),
                    1_000 + i as i64,
                )
                .unwrap();
        }

        let mut rows = ledger
            .recent(
                ObservationQuery {
                    provider: "anyrouter",
                    model: "claude-opus-4-1",
                    route: Some("anthropic-messages"),
                    harness: Some("claude-code"),
                },
                20,
            )
            .unwrap();
        rows.sort_by_key(|row| row.seq);
        assert_eq!(rows.len(), FailureClass::ALL.len());
        for (row, class) in rows.iter().zip(FailureClass::ALL) {
            assert_eq!(row.failure_class, Some(class));
            assert_eq!(row.retries, Some(0));
        }
        assert_eq!(rows[0].failovers, Some(1));
        assert!(rows[1..].iter().all(|row| row.failovers == Some(0)));
    }

    /// Which rows count, per [`FailureClassCounts`]' own doc: a row with no
    /// outcome is nobody's exchange; a class is counted under itself; a
    /// success with no class is served; anything else with no class is
    /// unclassified — and line 1365's third figure excludes the two classes
    /// that say nothing about the provider's health.
    #[test]
    fn failure_class_counts_keep_served_unclassified_and_each_class_apart() {
        let mut counts = FailureClassCounts::default();
        assert!(counts.is_empty());

        counts.record(None, None);
        counts.record(None, Some(FailureClass::Throttle));
        assert!(
            counts.is_empty(),
            "rows without an outcome are not exchanges"
        );

        counts.record(Some(Outcome::Succeeded), None);
        counts.record(Some(Outcome::Failed), None);
        counts.record(Some(Outcome::Unknown), None);
        counts.record(Some(Outcome::Failed), Some(FailureClass::Throttle));
        counts.record(Some(Outcome::Failed), Some(FailureClass::Throttle));
        counts.record(Some(Outcome::Failed), Some(FailureClass::ExhaustedQuota));
        counts.record(Some(Outcome::Failed), Some(FailureClass::Upstream5xx));
        counts.record(Some(Outcome::Failed), Some(FailureClass::StreamAbort));
        counts.record(Some(Outcome::Failed), Some(FailureClass::CredentialFailure));
        counts.record(
            Some(Outcome::Failed),
            Some(FailureClass::RequestIncompatibility),
        );

        assert_eq!(counts.served(), 1);
        assert_eq!(counts.unclassified(), 2);
        assert_eq!(counts.cadence_throttled(), 2);
        assert_eq!(counts.exhausted_quota(), 1);
        assert_eq!(
            counts.provider_health_failures(),
            2,
            "upstream 5xx and stream abort; never the credential or the request"
        );
        assert_eq!(counts.count(FailureClass::CredentialFailure), 1);
        assert_eq!(counts.observed(), 10);
    }

    /// [`EvidenceLedger::summarize`] carries the counts for its identity, and
    /// — being counts, not rates — does not withhold them below
    /// [`MIN_SAMPLE_FOR_SUMMARY`] the way it withholds `failure_rate`.
    #[test]
    fn summarize_counts_failure_classes_even_below_the_sample_floor() {
        let tmp = tempfile::tempdir().unwrap();
        let fixture = Fixture::new(tmp.path(), "alpha");
        let ledger = fixture.ledger();
        for at in [1_000, 1_001] {
            ledger
                .record(
                    observation("anyrouter", "claude-opus-4-1")
                        .with_outcome(Outcome::Failed)
                        .with_failure_class(Some(FailureClass::Throttle)),
                    at,
                )
                .unwrap();
        }
        let summary = ledger
            .summarize(
                ObservationQuery {
                    provider: "anyrouter",
                    model: "claude-opus-4-1",
                    route: Some("anthropic-messages"),
                    harness: Some("claude-code"),
                },
                ContextState::Unknown,
                1_100,
                1_000,
            )
            .unwrap();
        assert!(summary.failure_rate.is_none(), "two is below the floor");
        assert_eq!(summary.failure_classes.cadence_throttled(), 2);
        assert_eq!(summary.failure_classes.observed(), 2);
    }

    /// [`EvidenceLedger::failure_classes_by_provider`] counts every model,
    /// route and harness of a provider together, within the window only, and
    /// leaves an outcome-less row (the extraction producer's shape) out.
    #[test]
    fn failure_classes_by_provider_counts_across_identities_within_the_window() {
        let tmp = tempfile::tempdir().unwrap();
        let fixture = Fixture::new(tmp.path(), "alpha");
        let ledger = fixture.ledger();
        let now = 10_000;

        ledger
            .record(
                observation("anyrouter", "claude-opus-4-1")
                    .with_outcome(Outcome::Failed)
                    .with_failure_class(Some(FailureClass::Throttle)),
                now - 10,
            )
            .unwrap();
        ledger
            .record(
                NewObservation::new("anyrouter", "claude-sonnet-4-5")
                    .with_outcome(Outcome::Failed)
                    .with_failure_class(Some(FailureClass::Upstream5xx)),
                now - 20,
            )
            .unwrap();
        ledger
            .record(
                observation("anyrouter", "claude-opus-4-1").with_outcome(Outcome::Succeeded),
                now - 30,
            )
            .unwrap();
        // No outcome: not an exchange, not counted.
        ledger
            .record(
                NewObservation::new("anyrouter", "claude-opus-4-1"),
                now - 40,
            )
            .unwrap();
        // Outside the window.
        ledger
            .record(
                observation("anyrouter", "claude-opus-4-1")
                    .with_outcome(Outcome::Failed)
                    .with_failure_class(Some(FailureClass::ExhaustedQuota)),
                now - 1_001,
            )
            .unwrap();
        // Another provider entirely.
        ledger
            .record(
                observation("groq", "llama")
                    .with_outcome(Outcome::Failed)
                    .with_failure_class(Some(FailureClass::CredentialFailure)),
                now - 5,
            )
            .unwrap();

        let by_provider = ledger.failure_classes_by_provider(now, 1_000).unwrap();
        assert_eq!(by_provider.len(), 2, "{by_provider:?}");
        let anyrouter = &by_provider["anyrouter"];
        assert_eq!(anyrouter.observed(), 3);
        assert_eq!(anyrouter.cadence_throttled(), 1);
        assert_eq!(anyrouter.provider_health_failures(), 1);
        assert_eq!(anyrouter.served(), 1);
        assert_eq!(
            anyrouter.exhausted_quota(),
            0,
            "yesterday's row is outside the window"
        );
        let groq = &by_provider["groq"];
        assert_eq!(groq.count(FailureClass::CredentialFailure), 1);
        assert_eq!(groq.observed(), 1);
    }

    /// The ledger's own append-oriented promise, proven rather than assumed:
    /// there is no way to reach a second, differently-timestamped copy of one
    /// observation through this store's own API.
    #[test]
    fn there_is_no_way_to_edit_a_recorded_observation() {
        let tmp = tempfile::tempdir().unwrap();
        let fixture = Fixture::new(tmp.path(), "alpha");
        let ledger = fixture.ledger();
        ledger.record(observation("anyrouter", "m"), 1_000).unwrap();
        ledger.record(observation("anyrouter", "m"), 1_001).unwrap();

        let rows = ledger
            .recent(
                ObservationQuery {
                    provider: "anyrouter",
                    model: "m",
                    route: Some("anthropic-messages"),
                    harness: Some("claude-code"),
                },
                10,
            )
            .unwrap();
        assert_eq!(
            rows.len(),
            2,
            "two records must produce two rows, never one edited in place"
        );
    }

    /// Capability map line 1343's structural half: nothing built on this
    /// ledger can read another project's observations, because each project
    /// has a physically separate database file.
    #[test]
    fn a_ledger_never_sees_another_projects_observations() {
        let tmp = tempfile::tempdir().unwrap();
        let alpha = Fixture::new(tmp.path(), "alpha");
        let beta = Fixture::new(tmp.path(), "beta");

        alpha
            .ledger()
            .record(observation("anyrouter", "m"), 1_000)
            .unwrap();

        let beta_rows = beta
            .ledger()
            .recent(
                ObservationQuery {
                    provider: "anyrouter",
                    model: "m",
                    route: Some("anthropic-messages"),
                    harness: Some("claude-code"),
                },
                10,
            )
            .unwrap();
        assert!(
            beta_rows.is_empty(),
            "a sibling project's database must never contain another project's observation"
        );
    }

    /// Capability map line 1340: below the minimum sample, every aggregate is
    /// `None` rather than a number computed from too little evidence.
    #[test]
    fn a_summary_below_the_minimum_sample_is_unknown() {
        let tmp = tempfile::tempdir().unwrap();
        let fixture = Fixture::new(tmp.path(), "alpha");
        let ledger = fixture.ledger();

        for i in 0..(MIN_SAMPLE_FOR_SUMMARY - 1) {
            let at = 1_000 + i as i64;
            let new = observation("anyrouter", "m")
                .with_timing(Some(at), Some(at + 1))
                .with_outcome(Outcome::Succeeded);
            ledger.record(new, at).unwrap();
        }

        let summary = ledger
            .summarize(
                ObservationQuery {
                    provider: "anyrouter",
                    model: "m",
                    route: Some("anthropic-messages"),
                    harness: Some("claude-code"),
                },
                ContextState::Unknown,
                10_000,
                100_000,
            )
            .unwrap();
        assert!(summary.median_duration_ms.is_none());
        assert!(summary.failure_rate.is_none());
    }

    #[test]
    fn a_summary_at_the_minimum_sample_is_a_real_number() {
        let tmp = tempfile::tempdir().unwrap();
        let fixture = Fixture::new(tmp.path(), "alpha");
        let ledger = fixture.ledger();

        for i in 0..MIN_SAMPLE_FOR_SUMMARY {
            let at = 1_000 + i as i64 * 10;
            let new = observation("anyrouter", "m")
                .with_timing(Some(at), Some(at + 2))
                .with_outcome(Outcome::Succeeded);
            ledger.record(new, at).unwrap();
        }

        let summary = ledger
            .summarize(
                ObservationQuery {
                    provider: "anyrouter",
                    model: "m",
                    route: Some("anthropic-messages"),
                    harness: Some("claude-code"),
                },
                ContextState::Unknown,
                10_000,
                100_000,
            )
            .unwrap();
        let median = summary
            .median_duration_ms
            .expect("five samples must produce a reading");
        assert_eq!(*median.value(), 2_000);
        assert_eq!(median.sample_count(), MIN_SAMPLE_FOR_SUMMARY);
        assert_eq!(median.confidence(), Confidence::Medium);
        let failure_rate = summary
            .failure_rate
            .expect("five outcomes must produce a reading");
        assert_eq!(*failure_rate.value(), 0.0);
    }

    /// Capability map line 1341: an observation older than the summary's
    /// window is excluded from the aggregate, but stays readable raw — decay
    /// without deletion.
    #[test]
    fn an_observation_outside_the_window_is_excluded_from_the_summary_but_not_deleted() {
        let tmp = tempfile::tempdir().unwrap();
        let fixture = Fixture::new(tmp.path(), "alpha");
        let ledger = fixture.ledger();

        // One very old observation, then enough recent ones to clear the
        // minimum sample on their own.
        let old = observation("anyrouter", "m")
            .with_timing(Some(0), Some(1))
            .with_outcome(Outcome::Failed);
        ledger.record(old, 0).unwrap();
        for i in 0..MIN_SAMPLE_FOR_SUMMARY {
            let at = 100_000 + i as i64;
            let new = observation("anyrouter", "m")
                .with_timing(Some(at), Some(at + 1))
                .with_outcome(Outcome::Succeeded);
            ledger.record(new, at).unwrap();
        }

        let query = ObservationQuery {
            provider: "anyrouter",
            model: "m",
            route: Some("anthropic-messages"),
            harness: Some("claude-code"),
        };

        let raw = ledger.recent(query, 100).unwrap();
        assert_eq!(
            raw.len(),
            MIN_SAMPLE_FOR_SUMMARY + 1,
            "the old row must still be readable raw"
        );

        let summary = ledger
            .summarize(
                query,
                ContextState::Unknown,
                100_000 + MIN_SAMPLE_FOR_SUMMARY as i64,
                1_000,
            )
            .unwrap();
        let failure_rate = summary
            .failure_rate
            .expect("the recent, in-window observations alone must clear the minimum sample");
        assert_eq!(
            *failure_rate.value(),
            0.0,
            "the old failed observation is outside the window and must not pull the rate down"
        );
    }

    /// Capability map line 1337: rows in different context-state buckets are
    /// never blended into one summary.
    #[test]
    fn warm_and_cold_observations_never_share_one_summary() {
        let tmp = tempfile::tempdir().unwrap();
        let fixture = Fixture::new(tmp.path(), "alpha");
        let ledger = fixture.ledger();

        for i in 0..MIN_SAMPLE_FOR_SUMMARY {
            let at = 1_000 + i as i64;
            let new = observation("anyrouter", "m")
                .with_timing(Some(at), Some(at + 1))
                .with_outcome(Outcome::Failed)
                .with_context_state(ContextState::Cold);
            ledger.record(new, at).unwrap();
        }
        for i in 0..MIN_SAMPLE_FOR_SUMMARY {
            let at = 2_000 + i as i64;
            let new = observation("anyrouter", "m")
                .with_timing(Some(at), Some(at + 1))
                .with_outcome(Outcome::Succeeded)
                .with_context_state(ContextState::Warm);
            ledger.record(new, at).unwrap();
        }

        let query = ObservationQuery {
            provider: "anyrouter",
            model: "m",
            route: Some("anthropic-messages"),
            harness: Some("claude-code"),
        };
        let cold = ledger
            .summarize(query, ContextState::Cold, 10_000, 100_000)
            .unwrap();
        let warm = ledger
            .summarize(query, ContextState::Warm, 10_000, 100_000)
            .unwrap();
        assert_eq!(*cold.failure_rate.unwrap().value(), 1.0);
        assert_eq!(*warm.failure_rate.unwrap().value(), 0.0);
    }

    /// A raw insert that pairs `cost_micro_usd` with no `cost_confidence`
    /// cannot happen through this store's own `record` — [`NewObservation`]
    /// has no way to construct that combination, since [`ObservedCost`]
    /// always carries both.
    #[test]
    fn a_cost_recorded_through_this_store_always_carries_a_confidence() {
        let tmp = tempfile::tempdir().unwrap();
        let fixture = Fixture::new(tmp.path(), "alpha");
        let ledger = fixture.ledger();
        let mut new = observation("anyrouter", "m");
        new.cost = Some(ObservedCost {
            micro_usd: 500,
            confidence: CostConfidence::Estimated,
        });
        ledger.record(new, 1_000).unwrap();

        let rows = ledger
            .recent(
                ObservationQuery {
                    provider: "anyrouter",
                    model: "m",
                    route: Some("anthropic-messages"),
                    harness: Some("claude-code"),
                },
                10,
            )
            .unwrap();
        let cost = rows[0].cost.expect("the cost must round-trip");
        assert_eq!(cost.micro_usd, 500);
        assert_eq!(cost.confidence, CostConfidence::Estimated);
    }

    /// Capability map line 1342: token volume, request count and spend are
    /// resource telemetry, never evidence of quality. A summary computed from
    /// two batches that differ only in `input_tokens`/`output_tokens`/`cost`
    /// must be byte-for-byte identical — if a later change folded token
    /// volume into a quality aggregate, this test would be the one to notice.
    #[test]
    fn no_aggregate_changes_when_only_token_volume_or_cost_changes() {
        let tmp = tempfile::tempdir().unwrap();
        let cheap = Fixture::new(tmp.path(), "cheap");
        let expensive = Fixture::new(tmp.path(), "expensive");
        let cheap_ledger = cheap.ledger();
        let expensive_ledger = expensive.ledger();

        for i in 0..MIN_SAMPLE_FOR_SUMMARY {
            let at = 1_000 + i as i64;
            let mut small = observation("anyrouter", "m")
                .with_timing(Some(at), Some(at + 1))
                .with_outcome(Outcome::Succeeded);
            small.input_tokens = Some(10);
            small.output_tokens = Some(10);
            cheap_ledger.record(small, at).unwrap();

            let mut large = observation("anyrouter", "m")
                .with_timing(Some(at), Some(at + 1))
                .with_outcome(Outcome::Succeeded);
            large.input_tokens = Some(200_000);
            large.output_tokens = Some(50_000);
            large.cost = Some(ObservedCost {
                micro_usd: 9_000_000,
                confidence: CostConfidence::Exact,
            });
            expensive_ledger.record(large, at).unwrap();
        }

        let query = ObservationQuery {
            provider: "anyrouter",
            model: "m",
            route: Some("anthropic-messages"),
            harness: Some("claude-code"),
        };
        let cheap_summary = cheap_ledger
            .summarize(query, ContextState::Unknown, 10_000, 100_000)
            .unwrap();
        let expensive_summary = expensive_ledger
            .summarize(query, ContextState::Unknown, 10_000, 100_000)
            .unwrap();

        assert_eq!(
            cheap_summary.failure_rate.map(|r| *r.value()),
            expensive_summary.failure_rate.map(|r| *r.value())
        );
        assert_eq!(
            cheap_summary.median_duration_ms.map(|r| *r.value()),
            expensive_summary.median_duration_ms.map(|r| *r.value())
        );
    }

    /// [`ObservationSource`] end to end: a real [`EvidenceKey`] resolves
    /// through [`ObservedEvidenceSource`] to the same failure rate
    /// [`EvidenceLedger::summarize`] computes directly.
    #[test]
    fn observed_evidence_source_answers_from_the_same_ledger_summarize_reads() {
        use crate::harness::WireProtocol;
        use crate::harness::pairing::ServingRoute;
        use crate::integrations::IntegrationId;
        use crate::routing::AssignedModel;

        let tmp = tempfile::tempdir().unwrap();
        let fixture = Fixture::new(tmp.path(), "alpha");
        let ledger = fixture.ledger();

        for i in 0..MIN_SAMPLE_FOR_SUMMARY {
            let at = 1_000 + i as i64;
            let new = observation("anyrouter", "claude-opus-4-1")
                .with_timing(Some(at), Some(at + 1))
                .with_outcome(Outcome::Succeeded);
            ledger.record(new, at).unwrap();
        }

        let key = EvidenceKey::new(
            IntegrationId::ClaudeCode,
            "default",
            AssignedModel::named("claude-opus-4-1"),
            ServingRoute {
                provider: Some("anyrouter".to_owned()),
                gateway: None,
                protocol: Some(WireProtocol::AnthropicMessages),
            },
        );
        let source = ObservedEvidenceSource::new(&ledger, 10_000, 100_000);
        let observed = source
            .observed(&key)
            .expect("five successes must produce evidence");
        assert_eq!(observed.reliable_observation_count, MIN_SAMPLE_FOR_SUMMARY);
        assert_eq!(observed.task_success_rate, Some(1.0));
        assert_eq!(observed.usable_tool_call_rate, None);
    }

    /// A route this ledger never recorded anything for (no `provider` in the
    /// key) must answer `None`, not a fabricated zero.
    #[test]
    fn observed_evidence_source_answers_none_for_a_first_party_route() {
        use crate::harness::pairing::ServingRoute;
        use crate::integrations::IntegrationId;
        use crate::routing::AssignedModel;

        let tmp = tempfile::tempdir().unwrap();
        let fixture = Fixture::new(tmp.path(), "alpha");
        let ledger = fixture.ledger();

        let key = EvidenceKey::new(
            IntegrationId::ClaudeCode,
            "default",
            AssignedModel::named("claude-opus-4-1"),
            ServingRoute {
                provider: None,
                gateway: None,
                protocol: None,
            },
        );
        let source = ObservedEvidenceSource::new(&ledger, 10_000, 100_000);
        assert!(source.observed(&key).is_none());
    }

    /// Acceptance test 1: two recorded identities come back as exactly two
    /// distinct identities, with their real sample counts — the enumeration
    /// [`EvidenceLedger::recent`] and [`EvidenceLedger::summarize`] cannot
    /// answer (practice §71).
    #[test]
    fn observed_identities_returns_the_distinct_identities_actually_recorded() {
        let tmp = tempfile::tempdir().unwrap();
        let fixture = Fixture::new(tmp.path(), "alpha");
        let ledger = fixture.ledger();

        ledger.record(observation("anyrouter", "m"), 1_000).unwrap();
        ledger.record(observation("anyrouter", "m"), 1_001).unwrap();
        ledger
            .record(NewObservation::new("openai-router", "gpt-5"), 1_002)
            .unwrap();

        let identities = ledger.observed_identities(10_000, 100_000, 50).unwrap();
        let mut pairs: Vec<(String, String)> = identities
            .iter()
            .map(|i| (i.provider.clone(), i.model.clone()))
            .collect();
        pairs.sort();
        assert_eq!(
            pairs,
            vec![
                ("anyrouter".to_owned(), "m".to_owned()),
                ("openai-router".to_owned(), "gpt-5".to_owned()),
            ]
        );

        let anyrouter = identities
            .iter()
            .find(|i| i.provider == "anyrouter")
            .expect("anyrouter identity");
        let openai = identities
            .iter()
            .find(|i| i.provider == "openai-router")
            .expect("openai identity");
        assert_eq!(anyrouter.sample_count(), 2);
        assert_eq!(openai.sample_count(), 1);
        assert_ne!(
            anyrouter.sample_count(),
            openai.sample_count(),
            "two identities with different counts must be distinguishable"
        );
    }

    /// Acceptance test 2: bounded, the same shape [`EvidenceLedger::recent`]
    /// takes.
    #[test]
    fn observed_identities_is_bounded_by_the_limit() {
        let tmp = tempfile::tempdir().unwrap();
        let fixture = Fixture::new(tmp.path(), "alpha");
        let ledger = fixture.ledger();
        for i in 0..5 {
            ledger
                .record(NewObservation::new(format!("provider-{i}"), "m"), 1_000 + i)
                .unwrap();
        }

        let identities = ledger.observed_identities(10_000, 100_000, 3).unwrap();
        assert_eq!(identities.len(), 3, "at most the limit must come back");
    }

    /// Acceptance test 3, structural half: physical per-project database
    /// separation, the same guarantee
    /// [`a_ledger_never_sees_another_projects_observations`] proves for
    /// [`EvidenceLedger::recent`].
    #[test]
    fn observed_identities_never_sees_another_projects_observations() {
        let tmp = tempfile::tempdir().unwrap();
        let alpha = Fixture::new(tmp.path(), "alpha");
        let beta = Fixture::new(tmp.path(), "beta");
        alpha
            .ledger()
            .record(observation("anyrouter", "m"), 1_000)
            .unwrap();

        let beta_identities = beta
            .ledger()
            .observed_identities(10_000, 100_000, 50)
            .unwrap();
        assert!(
            beta_identities.is_empty(),
            "a sibling project's database must never contain another project's identity"
        );
    }

    /// Acceptance test 3, defensive half — and why this ledger's own
    /// `WHERE project_id = ?1` cannot be demonstrated to fail by a mutation
    /// that removes it: a row tagged with a foreign `project_id` can never
    /// even be inserted into this database. Migration 11's own
    /// `routing_observations_reject_foreign_project_insert` trigger refuses
    /// it at the SQL layer, before [`EvidenceLedger::observed_identities`] or
    /// [`EvidenceLedger::record`] ever runs — a stronger guarantee than this
    /// method's own filter, and the reason
    /// [`observed_identities_never_sees_another_projects_observations`]
    /// above is this project's only *reachable* isolation test for this
    /// method, exactly as it already is for [`EvidenceLedger::recent`] and
    /// [`EvidenceLedger::summarize`], neither of which filters by
    /// `project_id` in SQL at all.
    #[test]
    fn a_foreign_project_id_row_cannot_even_be_inserted_into_this_database() {
        let tmp = tempfile::tempdir().unwrap();
        let fixture = Fixture::new(tmp.path(), "alpha");
        let ledger = fixture.ledger();

        let conn = ledger.lock();
        let err = conn
            .execute(
                "INSERT INTO routing_observations
                    (project_id, observed_at, provider, model, context_state)
                 VALUES ('someone-elses-project', 1_001, 'anyrouter', 'm', 'unknown')",
                [],
            )
            .expect_err("the schema's own trigger must refuse a foreign project_id");
        assert!(err.to_string().contains("different project"), "got: {err}");
    }

    /// The window and sample count both reflect real recorded timestamps —
    /// not a placeholder — and rows outside the queried window are excluded
    /// from both, the same decay-without-deletion contract
    /// [`an_observation_outside_the_window_is_excluded_from_the_summary_but_not_deleted`]
    /// proves for [`EvidenceLedger::summarize`].
    #[test]
    fn observed_identities_reports_the_real_window_and_excludes_rows_outside_it() {
        let tmp = tempfile::tempdir().unwrap();
        let fixture = Fixture::new(tmp.path(), "alpha");
        let ledger = fixture.ledger();
        ledger.record(observation("anyrouter", "old"), 0).unwrap();
        ledger
            .record(observation("anyrouter", "m"), 100_000)
            .unwrap();
        ledger
            .record(observation("anyrouter", "m"), 100_050)
            .unwrap();

        let identities = ledger.observed_identities(100_050, 1_000, 50).unwrap();
        let models: Vec<&str> = identities.iter().map(|i| i.model.as_str()).collect();
        assert_eq!(
            models,
            vec!["m"],
            "the row outside the window must not appear at all"
        );
        let m = identities.iter().find(|i| i.model == "m").unwrap();
        assert_eq!(m.sample_count(), 2);
        assert_eq!(m.window(), (100_000, 100_050));
    }

    /// Capability map line 1764, at the enumeration layer: rows in different
    /// [`ContextState`] buckets are never blended into one identity — the
    /// same separation [`warm_and_cold_observations_never_share_one_summary`]
    /// proves for [`EvidenceLedger::summarize`].
    #[test]
    fn observed_identities_keeps_different_context_states_as_separate_rows() {
        let tmp = tempfile::tempdir().unwrap();
        let fixture = Fixture::new(tmp.path(), "alpha");
        let ledger = fixture.ledger();
        ledger
            .record(
                observation("anyrouter", "m").with_context_state(ContextState::Warm),
                1_000,
            )
            .unwrap();
        ledger
            .record(
                observation("anyrouter", "m").with_context_state(ContextState::Unknown),
                1_001,
            )
            .unwrap();

        let identities = ledger.observed_identities(10_000, 100_000, 50).unwrap();
        assert_eq!(
            identities.len(),
            2,
            "warm and unknown must not be blended into one row"
        );
        assert!(
            identities
                .iter()
                .any(|i| i.context_state == ContextState::Warm)
        );
        assert!(
            identities
                .iter()
                .any(|i| i.context_state == ContextState::Unknown)
        );
    }

    /// Capability map line 1661's own gap: a caller that only knows a
    /// provider and model from configuration must still get a real
    /// aggregate, without naming the route/harness/context-state
    /// [`EvidenceLedger::summarize`] requires.
    #[test]
    fn summarize_latest_for_model_finds_the_real_identity_and_summarizes_it() {
        let tmp = tempfile::tempdir().unwrap();
        let fixture = Fixture::new(tmp.path(), "alpha");
        let ledger = fixture.ledger();

        for i in 0..MIN_SAMPLE_FOR_SUMMARY {
            let at = 1_000 + i as i64 * 10;
            let new = observation("anyrouter", "claude-opus-4-1")
                .with_timing(Some(at), Some(at + 2))
                .with_outcome(Outcome::Succeeded);
            ledger.record(new, at).unwrap();
        }

        let summary = ledger
            .summarize_latest_for_model("anyrouter", "claude-opus-4-1", 10_000, 100_000)
            .unwrap()
            .expect("an observed model must produce a summary");
        let median = summary
            .median_duration_ms
            .expect("five samples must produce a reading");
        assert_eq!(*median.value(), 2_000);
        assert_eq!(summary.provider, "anyrouter");
        assert_eq!(summary.model, "claude-opus-4-1");
    }

    /// A model nothing has ever recorded gets `Ok(None)`, distinct from a
    /// [`RoutingSummary`] whose fields are all `None` below the minimum
    /// sample — [`a_summary_below_the_minimum_sample_is_unknown`] proves the
    /// latter.
    #[test]
    fn summarize_latest_for_model_is_none_when_nothing_was_ever_observed() {
        let tmp = tempfile::tempdir().unwrap();
        let fixture = Fixture::new(tmp.path(), "alpha");
        let ledger = fixture.ledger();

        let summary = ledger
            .summarize_latest_for_model("anyrouter", "claude-opus-4-1", 10_000, 100_000)
            .unwrap();
        assert!(summary.is_none());
    }

    /// Ruling 3: attributed to the named model, never a blend with a
    /// differently-performing sibling.
    #[test]
    fn summarize_latest_for_model_never_blends_a_second_models_observations_in() {
        let tmp = tempfile::tempdir().unwrap();
        let fixture = Fixture::new(tmp.path(), "alpha");
        let ledger = fixture.ledger();

        for i in 0..MIN_SAMPLE_FOR_SUMMARY {
            let at = 1_000 + i as i64 * 10;
            ledger
                .record(
                    observation("anyrouter", "cheap-model").with_timing(Some(at), Some(at + 2)),
                    at,
                )
                .unwrap();
        }
        for i in 0..MIN_SAMPLE_FOR_SUMMARY {
            let at = 2_000 + i as i64 * 10;
            ledger
                .record(
                    observation("anyrouter", "slow-model").with_timing(Some(at), Some(at + 500)),
                    at,
                )
                .unwrap();
        }

        let cheap = ledger
            .summarize_latest_for_model("anyrouter", "cheap-model", 10_000, 100_000)
            .unwrap()
            .expect("cheap-model was observed");
        let slow = ledger
            .summarize_latest_for_model("anyrouter", "slow-model", 10_000, 100_000)
            .unwrap()
            .expect("slow-model was observed");
        assert_eq!(*cheap.median_duration_ms.unwrap().value(), 2_000);
        assert_eq!(*slow.median_duration_ms.unwrap().value(), 500_000);
    }

    /// Picks the most recently active `(route, harness, context_state)`
    /// bucket rather than the first one it finds — observations recorded
    /// under a different route earlier must not win over a more recent one
    /// under the route this project actually uses now.
    #[test]
    fn summarize_latest_for_model_uses_the_most_recent_identitys_own_route_and_harness() {
        let tmp = tempfile::tempdir().unwrap();
        let fixture = Fixture::new(tmp.path(), "alpha");
        let ledger = fixture.ledger();

        for i in 0..MIN_SAMPLE_FOR_SUMMARY {
            let at = 1_000 + i as i64 * 10;
            ledger
                .record(
                    NewObservation::new("anyrouter", "m")
                        .with_route(Some("old-route"))
                        .with_harness(Some("old-harness"))
                        .with_timing(Some(at), Some(at + 2)),
                    at,
                )
                .unwrap();
        }
        for i in 0..MIN_SAMPLE_FOR_SUMMARY {
            let at = 5_000 + i as i64 * 10;
            ledger
                .record(
                    NewObservation::new("anyrouter", "m")
                        .with_route(Some("new-route"))
                        .with_harness(Some("new-harness"))
                        .with_timing(Some(at), Some(at + 900)),
                    at,
                )
                .unwrap();
        }

        let summary = ledger
            .summarize_latest_for_model("anyrouter", "m", 10_000, 100_000)
            .unwrap()
            .expect("m was observed");
        assert_eq!(summary.route.as_deref(), Some("new-route"));
        assert_eq!(
            *summary.median_duration_ms.unwrap().value(),
            900_000,
            "the most recently active identity's own observations must be summarized, \
             not the older route's"
        );
    }

    /// The identity-discovery step must itself filter by `model`: two models
    /// sharing a provider, observed at the exact same timestamps so they tie
    /// on `observed_at`, must never let one model's route leak into the
    /// other's summary — the mutation this proof exists to kill drops
    /// `AND model = ?3` from that lookup's own `WHERE` clause. Batch
    /// overview-latency's own mutation run found this SURVIVED against every
    /// test that gave both models the same route and harness (§80: a
    /// SURVIVED that means "the fixture never varied the thing the mutation
    /// touches" reads exactly like one that means "nothing watches this").
    #[test]
    fn summarize_latest_for_model_never_lets_a_tied_second_models_route_leak_in() {
        let tmp = tempfile::tempdir().unwrap();
        let fixture = Fixture::new(tmp.path(), "alpha");
        let ledger = fixture.ledger();

        for i in 0..MIN_SAMPLE_FOR_SUMMARY {
            let at = 1_000 + i as i64 * 10;
            ledger
                .record(
                    NewObservation::new("anyrouter", "target-model")
                        .with_route(Some("route-a"))
                        .with_harness(Some("harness-a"))
                        .with_timing(Some(at), Some(at + 2)),
                    at,
                )
                .unwrap();
            ledger
                .record(
                    NewObservation::new("anyrouter", "other-model")
                        .with_route(Some("route-b"))
                        .with_harness(Some("harness-b"))
                        .with_timing(Some(at), Some(at + 900)),
                    at,
                )
                .unwrap();
        }

        let summary = ledger
            .summarize_latest_for_model("anyrouter", "target-model", 10_000, 100_000)
            .unwrap()
            .expect("target-model was observed");
        assert_eq!(summary.route.as_deref(), Some("route-a"));
        assert_eq!(*summary.median_duration_ms.unwrap().value(), 2_000);
    }
}
