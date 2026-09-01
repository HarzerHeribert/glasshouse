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

/// The fallback bucket for a `routing_observations.harness` column that is
/// `NULL`, or for a `sessions.harness` join that found no row — the same
/// convention `crate::evaluation::UNKNOWN_COST_CLASS` gives the tier and
/// pairing-class readers, spelled once here so
/// [`EvidenceLedger::request_stats_by_harness`] and
/// [`crate::evaluation::EvaluationObservations::outcomes_by_tier_and_harness`]
/// cannot drift into two different words for the same absence.
pub const UNKNOWN_HARNESS: &str = "unknown";

/// What `routing_observations.purpose` records for a routing-model
/// classification call — `main.rs`'s `glasshouse classify` producer writes
/// it, and [`EvidenceLedger::classification_record`] reads it back.
///
/// Spelled once, here, because two spellings of one word would silently
/// split the only producer from the only reader: `purpose` is a `TEXT`
/// column with no `CHECK` (migration 11), so nothing in the schema would
/// notice.
pub const CLASSIFICATION_PURPOSE: &str = "classification";

/// What `routing_observations.purpose` records for a memory-extraction call
/// — `main.rs`'s `record_extraction_observation` producer writes it, and
/// [`RoutingOverhead`] reads it back as its own bucket.
///
/// **Rows written before this constant existed carry `NULL` and stay that
/// way.** [`NewObservation::with_purpose`]'s own doc comment records why:
/// back-filling them would make *"this build recorded nothing here"*
/// indistinguishable from *"this build recorded a purpose"*. So the stamp
/// applies from now on, an unstamped row is counted as unstamped, and
/// nothing is ever re-labelled — which is what makes capability map line
/// 1832's separation honest rather than retroactive.
pub const EXTRACTION_PURPOSE: &str = "memory-extraction";

/// What `routing_observations.purpose` records for map line 1849's
/// decision-latency row — `main.rs`'s `record_routing_latency` producer
/// writes it.
///
/// Spelled here rather than only at that producer so [`RoutingOverhead`] can
/// read the same word: a second spelling would silently split the only
/// producer from the only reader, exactly as [`CLASSIFICATION_PURPOSE`]'s
/// own doc says.
///
/// **A row under this purpose is not a model call.** It records the wall
/// clock a routing decision took, and carries no tokens at all — which is
/// why it is its own bucket rather than folded into
/// [`CLASSIFICATION_PURPOSE`]'s, where it would inflate a count of model
/// requests with rows no model ever served.
pub const ROUTING_LATENCY_PURPOSE: &str = "routing-latency";

/// What `routing_observations.purpose` records when the session router
/// **escalated** the tier a decision prefers — capability map line 1566,
/// written by `main.rs`'s `record_tier_movement` on the launch path (the
/// path that acts; `glasshouse route` reports and records nothing).
///
/// Spelled here beside [`CLASSIFICATION_PURPOSE`] for its reason, and read
/// back by [`RoutingOverhead`] into its own bucket: a movement row is not a
/// model call and carries no tokens, so it must be neither counted as one
/// nor left to the unstamped bucket as though no producer had named it.
///
/// **The row records that a movement happened and its direction, and
/// nothing else.** The tiers it moved between and the destination it landed
/// on have no column, and adding one is a migration this producer's package
/// may not make; it writes the same `glasshouse`/`session-router` identity
/// [`ROUTING_LATENCY_PURPOSE`]'s producer writes, so it can never blend into
/// a real model's latency summary.
pub const TIER_ESCALATION_PURPOSE: &str = "tier-escalation";

/// [`TIER_ESCALATION_PURPOSE`]'s other direction — line 1566 asks for both,
/// and a reader counting one must not have to subtract the other.
pub const TIER_DOWNGRADE_PURPOSE: &str = "tier-downgrade";

/// Capability map line 1970: one ledger row per pool fallback the launch
/// path acted on, under this purpose or
/// [`ENTITLEMENT_FALLBACK_THROTTLED_PURPOSE`], so a later evaluation can
/// count how often the broker left an account and why. `quota_context`
/// carries the account the work **left**, while `provider` and `model` are
/// the chosen destination's; the purpose column is what keeps these rows
/// out of any model's own summary. A decision that made no fallback writes
/// nothing — "the broker stayed put" is the row's absence, exactly as a
/// held tier is.
pub const ENTITLEMENT_FALLBACK_EXHAUSTED_PURPOSE: &str = "entitlement-fallback-exhausted";

/// [`ENTITLEMENT_FALLBACK_EXHAUSTED_PURPOSE`]'s other trigger.
pub const ENTITLEMENT_FALLBACK_THROTTLED_PURPOSE: &str = "entitlement-fallback-throttled";

/// Capability map line 1987: one row per tool result the context firewall
/// deterministically reduced — mirroring
/// [`ENTITLEMENT_FALLBACK_EXHAUSTED_PURPOSE`]'s own shape, this purpose or
/// [`CONTEXT_FIREWALL_BYPASS_PURPOSE`] beside it. `quota_context` carries
/// the tool name (`crate::firewall`'s own categorical label, the same role
/// [`ENTITLEMENT_FALLBACK_EXHAUSTED_PURPOSE`]'s doc comment gives the
/// account it records), and `harness` carries the adapter's own harness
/// slug — today always `claude-code`, since [`crate::firewall::adapter`] is
/// that harness's own. **This purpose's rows never set
/// `input_tokens`/`output_tokens`**: those columns are documented as a
/// provider's own reported count ([`NewObservation::with_tokens`]), and
/// this build's raw/forwarded figures are `chars/4` estimates, never a
/// provider's word — writing them there would make "recorded nothing"
/// indistinguishable from "the provider reported this", the exact
/// confusion [`NewObservation::with_tokens`]'s own doc comment exists to
/// prevent.
pub const CONTEXT_FIREWALL_REDUCTION_PURPOSE: &str = "context-firewall-reduction";

/// [`CONTEXT_FIREWALL_REDUCTION_PURPOSE`]'s other outcome: one row per
/// eligible result the firewall passed through unreduced, `route` carrying
/// [`crate::firewall::BypassReason::as_str`]'s word so a later reader can
/// count bypasses by reason.
pub const CONTEXT_FIREWALL_BYPASS_PURPOSE: &str = "context-firewall-bypass";

/// Map line 1988: one row per `context-firewall show` call — a raw-result
/// expansion request, the primary recall signal design-decisions.md's
/// Phase 57 section names. Not one of the packet's original two constants;
/// added beside them because line 1988 is its own box and an expansion is
/// neither a reduction nor a bypass — folding it into either purpose would
/// make expansion volume unreadable from reduction volume. `route` carries
/// `"found"` or `"not-found"`; `quota_context` carries the stored entry's
/// tool name when the id resolved, `None` otherwise.
pub const CONTEXT_FIREWALL_EXPANSION_PURPOSE: &str = "context-firewall-expansion";

/// How far back [`EvidenceLedger::classification_record`] and the routing
/// economics readers look — seven days, the same window the shell's
/// route-evidence view already uses, so a routing model's record and the
/// route table beside it agree on what "recent" means.
pub const CLASSIFICATION_EVIDENCE_WINDOW_SECONDS: i64 = 7 * 24 * 60 * 60;

/// What a row written by `main.rs`'s failover-prevention sink says: a
/// gateway failover was steered off a route whose failures **correlate**
/// with the failed backend's — capability map line 1852's measurement, one
/// row per steered failover, counted back by purpose and never as an
/// exchange.
///
/// Spelled here beside [`CLASSIFICATION_PURPOSE`] for the reason
/// [`ROUTING_LATENCY_PURPOSE`] gives: one producer, one reader, one word.
///
/// **A row under this purpose is not an exchange and not a model call.** It
/// carries no outcome, no failure class and no tokens, so every reader keyed
/// on `outcome` ([`FailureClassCounts::record`]) ignores it by construction,
/// [`RoutingOverhead::from_consumption`] skips it by name, and
/// [`correlate_routes`] — which would otherwise read its own consequence
/// back as evidence — excludes it explicitly.
pub const CORRELATION_PURPOSE: &str = "route-correlation";

/// How far apart two exchanges' windows may sit and still be *the same
/// moment* for [`correlate_routes`] — capability map line 1370's
/// "temporally overlapping", with the tolerance named rather than assumed.
///
/// Sixty seconds. The overlap this reader most needs to see is the one a
/// failover produces on its own: the failed backend's exchange ends, and the
/// route it failed over to starts its first exchange seconds later. Those
/// two windows never literally intersect — one ends before the other begins
/// — and a tolerance of zero would make every failover's most informative
/// pair of rows invisible. A minute covers that gap with room for a slow
/// client; an hour would fold two separate incidents into one. The
/// conservative error is to see *fewer* overlaps: a missed overlap leaves a
/// pair at [`CorrelationVerdict::InsufficientEvidence`] — no correlation,
/// line 1378's safe side — while an invented one penalises a route that did
/// nothing wrong.
pub const CORRELATION_OVERLAP_TOLERANCE_SECONDS: i64 = 60;

/// How many informative failure events a pair of routes needs before
/// [`RouteCorrelation::verdict`] reports a confidence at all — line 1376's
/// "sufficient overlapping observations."
///
/// The same five as [`MIN_SAMPLE_FOR_SUMMARY`], on purpose: this ledger has
/// one answer to "how many observations before a figure is trusted", and a
/// second number here would make a correlation trustworthy at a count a
/// failure rate computed from the same rows is not.
pub const MIN_CORRELATION_SAMPLE: usize = MIN_SAMPLE_FOR_SUMMARY;

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

    /// Whether a failure of this class on one route says anything about
    /// another route failing at the same moment — the classes
    /// [`correlate_routes`] matches on, capability map line 1373.
    ///
    /// Two and only two. An `Upstream5xx` is the provider's own
    /// infrastructure answering that it is broken, and two front doors
    /// answering so together is the strongest signal this ledger holds that
    /// they are one door. A `Throttle` is a limiter firing, and two limiters
    /// firing together is the "matching serving behaviour" the line names.
    /// Everything else is about the credential, the request, or a transport
    /// this build cannot attribute to either side — a `CredentialFailure` on
    /// two routes at once is two bad keys, not one shared upstream.
    pub fn is_correlatable(self) -> bool {
        matches!(self, Self::Upstream5xx | Self::Throttle)
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

    /// The wall-clock span this exchange occupied, as `(start_unix,
    /// end_unix)` — the same shape [`AggregateReading::window`] and
    /// [`ObservedIdentity::window`] return — and the interval
    /// [`correlate_routes`] tests for overlap.
    ///
    /// `dispatched_at` and `completed_at` when the producer recorded them
    /// (the gateway always does); `observed_at` stands in for either end a
    /// producer left absent, so a row that recorded only when it was written
    /// is a point in time rather than no interval at all. An end before its
    /// start — the case [`Self::duration_ms`] answers `None` to — is clamped
    /// to the start rather than producing a negative span.
    pub fn window(&self) -> (i64, i64) {
        let start = self.dispatched_at_unix.unwrap_or(self.observed_at_unix);
        let end = self.completed_at_unix.unwrap_or(self.observed_at_unix);
        (start, end.max(start))
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

/// One route as [`correlate_routes`] tells routes apart: the `provider` and
/// `model` already on every [`RoutingObservation`] — capability map line
/// 1373's "provider metadata", and nothing fetched from anywhere.
///
/// `model` is part of the identity because line 1373 asks for
/// *model-specific* 5xx events: two providers whose `claude-x` both fail at
/// once may share an upstream for that model and nothing else, and a
/// correlation keyed on provider alone would carry that pair's evidence to
/// models it was never observed on. The ledger's `route` column (the wire
/// protocol) is deliberately **not** part of it: the question is whether two
/// front doors lead to one room, and the protocol spoken at the door does
/// not change what is behind it.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct RouteIdentity {
    pub provider: String,
    pub model: String,
}

impl RouteIdentity {
    pub fn new(provider: impl Into<String>, model: impl Into<String>) -> Self {
        Self {
            provider: provider.into(),
            model: model.into(),
        }
    }
}

impl std::fmt::Display for RouteIdentity {
    /// `provider/model` — what every explanation and report prints.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}/{}", self.provider, self.model)
    }
}

/// What [`RouteCorrelation::verdict`] answers — capability map line 1376.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum CorrelationVerdict {
    /// Fewer than [`MIN_CORRELATION_SAMPLE`] informative events — line
    /// 1376's refusal, carrying the count so a reader prints *2 of 5* rather
    /// than *unknown*. **A consumer treats this exactly as no correlation.**
    InsufficientEvidence { sample_size: usize, required: usize },
    /// Enough events to say something, and what they say: the share of them
    /// in which the other route failed the same way at the same moment.
    Measured { confidence: f64, sample_size: usize },
}

/// What this project's ledger has observed about whether two routes fail
/// together — capability map lines 1370, 1373, 1374 and 1376, as one value.
///
/// # What is counted
///
/// An **informative failure event** is a correlatable failure
/// ([`FailureClass::is_correlatable`]) on one route during which the other
/// route was *observed at all* — had an exchange with a recorded outcome
/// whose window overlaps the failure's within
/// [`CORRELATION_OVERLAP_TOLERANCE_SECONDS`]. A failure while the other
/// route was idle says nothing about the pair and is counted nowhere: line
/// 1370's "measured, never assumed" cuts both ways, and treating an
/// unobserved route as having survived would manufacture independence.
///
/// Of the informative events, `overlaps` are those where the other route
/// failed with the **same class** inside the tolerance, and `lone` are those
/// where it was observed and did not. Each failure event is matched at most
/// once, so a burst of five on each side is ten events and not twenty-five
/// pairs.
///
/// # Why the confidence moves both ways (line 1374)
///
/// [`Self::confidence`] is `overlaps / (overlaps + lone)`. A new overlapping
/// failure raises it; a new failure the other route sat out lowers it.
/// Nothing here is a stored flag: the value is recomputed from the rows on
/// every read and never persisted, because the rows are the claim and the
/// rows keep arriving.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RouteCorrelation {
    routes: (RouteIdentity, RouteIdentity),
    overlaps: usize,
    lone: usize,
}

impl RouteCorrelation {
    /// A pair nothing has been observed about — zero events, which
    /// [`Self::verdict`] reports as insufficient with a count of zero.
    pub fn unmeasured(a: RouteIdentity, b: RouteIdentity) -> Self {
        let routes = if a <= b { (a, b) } else { (b, a) };
        Self {
            routes,
            overlaps: 0,
            lone: 0,
        }
    }

    /// The two routes, in a fixed order so `(a, b)` and `(b, a)` are the
    /// same pair.
    pub fn routes(&self) -> (&RouteIdentity, &RouteIdentity) {
        (&self.routes.0, &self.routes.1)
    }

    /// Failure events the other route failed the same way during.
    pub fn overlaps(&self) -> usize {
        self.overlaps
    }

    /// Failure events the other route was observed during and did not
    /// fail the same way.
    pub fn lone(&self) -> usize {
        self.lone
    }

    /// Every informative failure event — the denominator, and the count
    /// line 1376 requires beside any confidence.
    pub fn sample_size(&self) -> usize {
        self.overlaps + self.lone
    }

    /// Line 1376: a confidence only once [`MIN_CORRELATION_SAMPLE`] events
    /// exist, and otherwise the count that fell short.
    pub fn verdict(&self) -> CorrelationVerdict {
        let sample_size = self.sample_size();
        if sample_size < MIN_CORRELATION_SAMPLE {
            return CorrelationVerdict::InsufficientEvidence {
                sample_size,
                required: MIN_CORRELATION_SAMPLE,
            };
        }
        CorrelationVerdict::Measured {
            confidence: self.overlaps as f64 / sample_size as f64,
            sample_size,
        }
    }

    /// [`Self::verdict`]'s confidence, or `None` below the minimum — the
    /// shape a consumer composes with, where absent contributes nothing.
    pub fn confidence(&self) -> Option<f64> {
        match self.verdict() {
            CorrelationVerdict::Measured { confidence, .. } => Some(confidence),
            CorrelationVerdict::InsufficientEvidence { .. } => None,
        }
    }
}

/// Every pair of routes [`correlate_routes`] found anything about, looked
/// up by either ordering of the pair. [`Default`] is the empty set — every
/// pair unmeasured — which is what a caller with no ledger passes.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RouteCorrelations {
    pairs: std::collections::BTreeMap<(RouteIdentity, RouteIdentity), RouteCorrelation>,
}

impl RouteCorrelations {
    /// What is known about `a` and `b` failing together — never `None`: a
    /// pair with no rows is [`RouteCorrelation::unmeasured`], so "nothing
    /// observed" and "too little observed" reach a consumer as the same
    /// verdict rather than as two shapes to handle.
    pub fn between(&self, a: &RouteIdentity, b: &RouteIdentity) -> RouteCorrelation {
        let key = if a <= b {
            (a.clone(), b.clone())
        } else {
            (b.clone(), a.clone())
        };
        self.pairs
            .get(&key)
            .cloned()
            .unwrap_or_else(|| RouteCorrelation::unmeasured(key.0, key.1))
    }

    /// Every pair with at least one informative event, in route order.
    pub fn iter(&self) -> impl Iterator<Item = &RouteCorrelation> {
        self.pairs.values()
    }

    pub fn is_empty(&self) -> bool {
        self.pairs.is_empty()
    }
}

/// Whether two windows touch, or come within `tolerance` seconds of it.
fn overlaps_within(a: (i64, i64), b: (i64, i64), tolerance: i64) -> bool {
    a.0 <= b.1.saturating_add(tolerance) && b.0 <= a.1.saturating_add(tolerance)
}

/// Fold every correlatable failure in `failing` into `into`, judged against
/// what `other` was doing at the time — see [`RouteCorrelation`] for the
/// three outcomes an event can have.
fn count_failures_against(
    failing: &[&RoutingObservation],
    other: &[&RoutingObservation],
    into: &mut RouteCorrelation,
) {
    for failure in failing {
        let Some(class) = failure
            .failure_class
            .filter(|class| class.is_correlatable())
        else {
            continue;
        };
        let window = failure.window();
        let mut observed = false;
        let mut matched = false;
        for row in other {
            if !overlaps_within(window, row.window(), CORRELATION_OVERLAP_TOLERANCE_SECONDS) {
                continue;
            }
            observed = true;
            if row.failure_class == Some(class) {
                matched = true;
                break;
            }
        }
        match (observed, matched) {
            (false, _) => {}
            (true, true) => into.overlaps += 1,
            (true, false) => into.lone += 1,
        }
    }
}

/// Capability map lines 1370, 1373, 1374 and 1376 as one pure function over
/// raw rows, so every decision in it — the tolerance, the class match, the
/// route identity, the minimum — is reachable by a test with no database.
/// [`EvidenceLedger::route_correlations`] is the one door that feeds it.
///
/// Rows with no recorded outcome never inform a pair (an exchange nobody
/// judged is not evidence the route was up), and rows written under
/// [`CORRELATION_PURPOSE`] are this function's own consequence and are never
/// read back as its cause.
pub fn correlate_routes(observations: &[RoutingObservation]) -> RouteCorrelations {
    let mut by_route: std::collections::BTreeMap<RouteIdentity, Vec<&RoutingObservation>> =
        Default::default();
    for row in observations {
        if row.outcome.is_none() || row.purpose.as_deref() == Some(CORRELATION_PURPOSE) {
            continue;
        }
        by_route
            .entry(RouteIdentity::new(&row.provider, &row.model))
            .or_default()
            .push(row);
    }
    let routes: Vec<&RouteIdentity> = by_route.keys().collect();
    let mut pairs = std::collections::BTreeMap::new();
    for (index, a) in routes.iter().enumerate() {
        for b in &routes[index + 1..] {
            let mut correlation = RouteCorrelation::unmeasured((*a).clone(), (*b).clone());
            count_failures_against(&by_route[*a], &by_route[*b], &mut correlation);
            count_failures_against(&by_route[*b], &by_route[*a], &mut correlation);
            if correlation.sample_size() > 0 {
                pairs.insert(((*a).clone(), (*b).clone()), correlation);
            }
        }
    }
    RouteCorrelations { pairs }
}

/// Capability map line 1317: whether a throttle on one route reads as this
/// provider's own cadence limiter firing everywhere, or as one model's own
/// limit — computed, never stored, from the same rows and the same overlap
/// [`correlate_routes`] measures, restricted to [`FailureClass::Throttle`]
/// and to one provider's own models rather than every route in the ledger.
///
/// # One of the map line's four scopes is still not here
///
/// Line 1317 names four: provider-wide, model-specific, account-specific,
/// request-pool-specific. Three now have a producer in this build.
/// **Account-specific** gained its key with Phase 56A: every gateway
/// exchange row carries the serving credential's label in
/// [`RoutingObservation::quota_context`]
/// (`crate::gateway::session` stamps `credential().label()` on every
/// observation), so a second account of one provider is now something the
/// rows can tell apart — the earlier note here that *"no row carries an
/// account identity"* described the build before that column had its
/// producer. The variant is still emitted only when the evidence permits:
/// rows without a `quota_context` contribute nothing to it, and a ledger
/// with one account's rows classifies exactly as it always did.
/// **Request-pool-specific** still has neither a producer nor a consumer:
/// `routing::free::is_request_pool` has no production caller, and the one
/// production allowance read asks only `is_exhausted`, which a pooled and a
/// token-priced credential both answer the same way (refusal register, row
/// 531). Fabricating it would be exactly the invention line 1317's own
/// "when evidence permits" refuses.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ThrottleScope {
    /// A throttle on this route overlapped, within
    /// [`CORRELATION_OVERLAP_TOLERANCE_SECONDS`], a throttle on another model
    /// of the same provider — direct evidence the limiter reached more than
    /// one model, and outweighs any number of windows where it did not.
    ProviderWide,
    /// Every informative throttle on this route overlapped a sibling model
    /// of the same provider recording a **non-throttle** outcome — evidence
    /// the limiter is scoped to this model alone.
    ModelSpecific,
    /// A throttle on this route overlapped sibling-model throttles of the
    /// **same account** while a **different account** of the same provider
    /// (another [`RoutingObservation::quota_context`]) recorded a
    /// non-throttle outcome in the same window — the limiter reached more
    /// than one of this account's models, and another account kept serving
    /// through it, which refutes provider-wide without claiming
    /// model-specific. Never emitted from rows that carry no
    /// `quota_context`: with no account key the sibling-model overlap still
    /// reads [`ThrottleScope::ProviderWide`], exactly as before the key
    /// existed.
    AccountSpecific,
    /// Fewer than [`MIN_CORRELATION_SAMPLE`] informative throttle events for
    /// this route — line 1376's own refusal shape, reused rather than given
    /// a second minimum: this ledger keeps one answer to *how many
    /// observations before a figure is trusted*.
    Unknown { sample_size: usize, required: usize },
}

/// [`classify_throttle_scope`]'s per-event judgement: whether a throttle on
/// `route` was, within [`CORRELATION_OVERLAP_TOLERANCE_SECONDS`], observed
/// against a sibling model of the same provider, and whether that sibling
/// was throttled too — the same three-way outcome
/// [`count_failures_against`] folds into a [`RouteCorrelation`], specialised
/// to one provider's own models and to [`FailureClass::Throttle`] alone.
fn count_throttles_against_siblings(
    failing: &[&RoutingObservation],
    siblings: &[&RoutingObservation],
) -> (usize, usize) {
    let mut overlaps = 0usize;
    let mut lone = 0usize;
    for failure in failing {
        if failure.failure_class != Some(FailureClass::Throttle) {
            continue;
        }
        let window = failure.window();
        let mut observed = false;
        let mut matched = false;
        for row in siblings {
            if !overlaps_within(window, row.window(), CORRELATION_OVERLAP_TOLERANCE_SECONDS) {
                continue;
            }
            observed = true;
            if row.failure_class == Some(FailureClass::Throttle) {
                matched = true;
                break;
            }
        }
        match (observed, matched) {
            (false, _) => {}
            (true, true) => overlaps += 1,
            (true, false) => lone += 1,
        }
    }
    (overlaps, lone)
}

/// The account axis of [`classify_throttle_scope`]: for each informative
/// throttle on the route that carries a [`RoutingObservation::quota_context`],
/// whether a row of a **different** account of the same provider (any model,
/// a different `quota_context`) was observed within
/// [`CORRELATION_OVERLAP_TOLERANCE_SECONDS`] — and whether that other
/// account was throttled too. Rows without a context contribute nothing on
/// either side: an account this column cannot name is not evidence about
/// accounts.
///
/// Returns `(cross_throttle, cross_served)`: throttles during which another
/// account was also throttled, and throttles during which another account
/// recorded a non-throttle outcome.
fn count_throttles_against_other_accounts(
    failing: &[&RoutingObservation],
    provider_rows: &[&RoutingObservation],
) -> (usize, usize) {
    let mut cross_throttle = 0usize;
    let mut cross_served = 0usize;
    for failure in failing {
        if failure.failure_class != Some(FailureClass::Throttle) {
            continue;
        }
        let Some(account) = failure.quota_context.as_deref() else {
            continue;
        };
        let window = failure.window();
        let mut served = false;
        let mut throttled = false;
        for row in provider_rows {
            let Some(other) = row.quota_context.as_deref() else {
                continue;
            };
            if other == account {
                continue;
            }
            if !overlaps_within(window, row.window(), CORRELATION_OVERLAP_TOLERANCE_SECONDS) {
                continue;
            }
            if row.failure_class == Some(FailureClass::Throttle) {
                throttled = true;
                break;
            }
            served = true;
        }
        if throttled {
            cross_throttle += 1;
        } else if served {
            cross_served += 1;
        }
    }
    (cross_throttle, cross_served)
}

/// Line 1317, as a pure function over raw rows — the same shape
/// [`correlate_routes`] takes, restricted to `route`'s own provider's other
/// models rather than every other route in the ledger: line 1317 asks
/// whether a throttle is provider-wide **within one provider**, not whether
/// it correlates with an unrelated one.
///
/// An informative event is a throttle on `route` during which a sibling
/// model of the same provider was observed at all, within
/// [`CORRELATION_OVERLAP_TOLERANCE_SECONDS`] — rows with no recorded outcome
/// and this reader's own [`CORRELATION_PURPOSE`] rows are excluded on both
/// sides, the same rule [`correlate_routes`] applies. Below
/// [`MIN_CORRELATION_SAMPLE`] informative events, [`ThrottleScope::Unknown`]
/// with the count, exactly line 1376's shape. At or above it,
/// [`ThrottleScope::ProviderWide`] if any sibling model was throttled at the
/// same moment, else [`ThrottleScope::ModelSpecific`].
pub fn classify_throttle_scope(
    observations: &[RoutingObservation],
    route: &RouteIdentity,
) -> ThrottleScope {
    let informative = |row: &&RoutingObservation| {
        row.outcome.is_some() && row.purpose.as_deref() != Some(CORRELATION_PURPOSE)
    };
    let failing: Vec<&RoutingObservation> = observations
        .iter()
        .filter(|row| row.provider == route.provider && row.model == route.model)
        .filter(informative)
        .collect();
    let siblings: Vec<&RoutingObservation> = observations
        .iter()
        .filter(|row| row.provider == route.provider && row.model != route.model)
        .filter(informative)
        .collect();
    // The account axis reads every informative row of the provider, the
    // failing route's own model included: another account running the *same*
    // model is still another account.
    let provider_rows: Vec<&RoutingObservation> = observations
        .iter()
        .filter(|row| row.provider == route.provider)
        .filter(informative)
        .collect();

    let (overlaps, lone) = count_throttles_against_siblings(&failing, &siblings);
    let (cross_throttle, cross_served) =
        count_throttles_against_other_accounts(&failing, &provider_rows);
    let sample_size = overlaps + lone;
    if sample_size < MIN_CORRELATION_SAMPLE {
        return ThrottleScope::Unknown {
            sample_size,
            required: MIN_CORRELATION_SAMPLE,
        };
    }
    if cross_throttle > 0 {
        // Two accounts throttled in one window: the limiter provably
        // reached past any single account, whatever the models said.
        ThrottleScope::ProviderWide
    } else if overlaps > 0 {
        if cross_served > 0 {
            // This account's sibling models throttled together while a
            // different account kept serving — see the variant's own doc.
            ThrottleScope::AccountSpecific
        } else {
            ThrottleScope::ProviderWide
        }
    } else {
        ThrottleScope::ModelSpecific
    }
}

/// Every route [`classify_throttle_scope`] has anything to say about — at
/// least one throttle, in the window queried — looked up by route. The same
/// relationship [`RouteCorrelations`] has to a single pair: one query builds
/// every entry at once, and a caller with one route in mind still asks this
/// type rather than the database again.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ThrottleScopes {
    routes: std::collections::BTreeMap<RouteIdentity, ThrottleScope>,
}

impl ThrottleScopes {
    /// What this reader knows about `route`'s own throttles — never a bare
    /// absence: a route with no recorded throttle is
    /// [`ThrottleScope::Unknown`] with a count of zero, the same "nothing
    /// observed and too little observed read as one verdict" rule
    /// [`RouteCorrelations::between`] keeps.
    pub fn for_route(&self, route: &RouteIdentity) -> ThrottleScope {
        self.routes
            .get(route)
            .copied()
            .unwrap_or(ThrottleScope::Unknown {
                sample_size: 0,
                required: MIN_CORRELATION_SAMPLE,
            })
    }

    /// Every route with at least one recorded throttle, in route order.
    pub fn iter(&self) -> impl Iterator<Item = (&RouteIdentity, &ThrottleScope)> {
        self.routes.iter()
    }

    pub fn is_empty(&self) -> bool {
        self.routes.is_empty()
    }
}

/// [`classify_throttle_scope`] for every route that recorded a throttle in
/// `observations`, rather than one asked about by name.
pub fn classify_throttle_scopes(observations: &[RoutingObservation]) -> ThrottleScopes {
    let routes: std::collections::BTreeSet<RouteIdentity> = observations
        .iter()
        .filter(|row| row.failure_class == Some(FailureClass::Throttle))
        .map(|row| RouteIdentity::new(&row.provider, &row.model))
        .collect();
    let routes = routes
        .into_iter()
        .map(|route| {
            let scope = classify_throttle_scope(observations, &route);
            (route, scope)
        })
        .collect();
    ThrottleScopes { routes }
}

/// Map line 1965's recent-throttling facet, counted from raw rows: how many
/// informative throttles the window's observations record against
/// `provider`, and whether that count could honestly be narrowed to one
/// account.
///
/// `account_narrowed` is `true` only when **every** throttle row of the
/// provider carries a [`RoutingObservation::quota_context`] and a
/// `credential_label` was given to narrow by — then `throttled` counts that
/// account's own rows alone. Any context-less throttle row makes the whole
/// reading provider-wide instead: a throttle no row attributes to an account
/// cannot be subtracted from one, so the honest count is the provider's
/// total, shared by every entitlement of that provider. Zero rows are a
/// provider-wide zero for the same reason — "none observed" is an
/// observation about the provider's rows, not about one account's.
///
/// The same informative-row rule as [`classify_throttle_scope`]: rows with
/// no recorded outcome and the correlation reader's own
/// [`CORRELATION_PURPOSE`] rows are not evidence.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CredentialThrottles {
    /// Informative throttles counted — the account's own when
    /// `account_narrowed`, the provider's total otherwise.
    pub throttled: usize,
    /// Whether `throttled` is the named credential's own count rather than
    /// the provider-wide total.
    pub account_narrowed: bool,
}

/// See [`CredentialThrottles`]. `credential_label` is the
/// [`crate::routing::CredentialId::label`] shape the gateway stamps into
/// [`RoutingObservation::quota_context`]; `None` — an entitlement with no
/// credential of its own — always yields the provider-wide count.
pub fn recent_credential_throttles(
    observations: &[RoutingObservation],
    provider: &str,
    credential_label: Option<&str>,
) -> CredentialThrottles {
    let throttles: Vec<&RoutingObservation> = observations
        .iter()
        .filter(|row| row.provider == provider)
        .filter(|row| row.failure_class == Some(FailureClass::Throttle))
        .filter(|row| row.outcome.is_some() && row.purpose.as_deref() != Some(CORRELATION_PURPOSE))
        .collect();
    let every_row_names_its_account =
        !throttles.is_empty() && throttles.iter().all(|row| row.quota_context.is_some());
    match credential_label {
        Some(label) if every_row_names_its_account => CredentialThrottles {
            throttled: throttles
                .iter()
                .filter(|row| row.quota_context.as_deref() == Some(label))
                .count(),
            account_narrowed: true,
        },
        _ => CredentialThrottles {
            throttled: throttles.len(),
            account_narrowed: false,
        },
    }
}

/// Token spend recorded against one account inside a queried window — map
/// line 1971's *"spend ceilings"* half, read from the rows this ledger
/// actually holds.
///
/// # Why tokens, and why that is not this reader's own decision
///
/// `routing_observations.cost_micro_usd` has **no producer in this build**
/// — see [`NewObservation::with_tokens`], which records why — so a reader
/// that answered in money would answer `None` forever, and a ceiling that
/// can never be reached is a rule the broker can never be held to. Map line
/// 1465's reader already settled the same question the same way, in
/// production, in [`RoutingOverhead`]'s own words: *"'Spend' is tokens,
/// input plus output as the provider reported them, because that is the only
/// currency this ledger holds."* This reader is that sentence applied per
/// account. Cached input tokens are excluded for line 1465's reason too:
/// providers disagree on whether they are already inside `input_tokens`, and
/// a sum that might double-count is worse than one that names what it omits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CredentialSpend {
    /// Input plus output tokens summed over the rows that carried a count —
    /// the account's own when `account_narrowed`, the provider's total
    /// otherwise. `None` when **no** row carried a count at all, which is
    /// *unknown* and is not `Some(0)`: the columns are nullable so those two
    /// facts stay apart, and a spend ceiling may only be judged reached
    /// against a reading that exists.
    pub tokens: Option<u64>,
    /// Whether `tokens` is the named credential's own sum rather than the
    /// provider-wide total.
    pub account_narrowed: bool,
    /// How many rows contributed a count to `tokens`. `0` exactly when
    /// `tokens` is `None`.
    pub sample_count: usize,
}

/// See [`CredentialSpend`]. `credential_label` is the
/// [`crate::routing::CredentialId::label`] shape the gateway stamps into
/// [`RoutingObservation::quota_context`]; `None` — an entitlement with no
/// credential of its own — always yields the provider-wide sum.
///
/// The narrowing rule is [`recent_credential_throttles`]'s, deliberately
/// verbatim: the reading is the account's own only when **every** counted
/// row of that provider names an account, because one contextless row means
/// the ledger holds spend nobody can attribute, and a sum that quietly
/// dropped it would under-report the very number a ceiling is checked
/// against. Under-reporting is the direction that lets a ceiling be
/// exceeded, so this reader widens rather than narrows when it is unsure.
///
/// [`CORRELATION_PURPOSE`] rows are excluded for the reason that constant
/// gives — they are this ledger's own bookkeeping and not exchanges — and
/// rows with no outcome are excluded because an exchange that never
/// completed reported no usage to sum.
pub fn recent_credential_spend(
    observations: &[RoutingObservation],
    provider: &str,
    credential_label: Option<&str>,
) -> CredentialSpend {
    let counted: Vec<&RoutingObservation> = observations
        .iter()
        .filter(|row| row.provider == provider)
        .filter(|row| row.outcome.is_some() && row.purpose.as_deref() != Some(CORRELATION_PURPOSE))
        .filter(|row| row.input_tokens.is_some() || row.output_tokens.is_some())
        .collect();
    let every_row_names_its_account =
        !counted.is_empty() && counted.iter().all(|row| row.quota_context.is_some());
    let account_narrowed = match credential_label {
        Some(_) => every_row_names_its_account,
        None => false,
    };
    let rows: Vec<&&RoutingObservation> = match (account_narrowed, credential_label) {
        (true, Some(label)) => counted
            .iter()
            .filter(|row| row.quota_context.as_deref() == Some(label))
            .collect(),
        _ => counted.iter().collect(),
    };
    let sample_count = rows.len();
    let tokens = if sample_count == 0 {
        None
    } else {
        Some(rows.iter().fold(0u64, |sum, row| {
            let input = row.input_tokens.unwrap_or(0).max(0) as u64;
            let output = row.output_tokens.unwrap_or(0).max(0) as u64;
            sum.saturating_add(input).saturating_add(output)
        }))
    };
    CredentialSpend {
        tokens,
        account_narrowed,
        sample_count,
    }
}

/// How recent a throttle must be to read as still-live pressure rather than
/// history the window happens to still hold, and how close a reset must sit
/// to count as imminent relief — map line 1245's "recency", one horizon for
/// both questions rather than a second invented number: an hour is the
/// shortest cadence window this project's own throttle producers actually
/// observe (`crate::gateway::session`'s own per-window limiters), so a
/// throttle or a reset outside it says nothing about the account's *current*
/// pressure.
pub const RECENT_SIGNAL_HORIZON_SECONDS: i64 = 3_600;

/// Map line 1249's second horizon — pressure that persists well past the
/// short window rather than a single accident. Three days, not a week or a
/// month: the one production caller queries rows only
/// [`CLASSIFICATION_EVIDENCE_WINDOW_SECONDS`] deep (seven days), and setting
/// this horizon at or past that bound would make
/// [`LongWindowPressure::NoPressure`] structurally unreachable — no query
/// could ever cover it, so the honest answer would always collapse to
/// [`LongWindowPressure::Undistinguished`]. Three days leaves the full
/// window room to actually prove an absence.
pub const LONG_SIGNAL_HORIZON_SECONDS: i64 = 3 * 24 * 3_600;

/// Map line 1248's anecdote guard: fewer than this many observed
/// throttle→success recoveries in window, and no reset window is learned at
/// all. Two is the floor at which a single unlucky pairing — a throttle
/// immediately followed, by coincidence, by an unrelated success — cannot be
/// the whole story behind the learned value.
pub const MIN_LEARNED_RESET_RECOVERIES: usize = 2;

/// Map line 1248 — whose reset reading, if any, informed this estimate's
/// "is a reset imminent" term. Kept off [`HeadroomBand`] itself (1250/1251's
/// own rule: no numeric field, no invented precision) and reported here
/// instead, so a consumer can label an inferred reading as what it is rather
/// than letting it render identically to the provider's own stated word.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResetBasis {
    /// No reset behaviour — stated or inferred — entered this estimate.
    Unknown,
    /// The caller's own authoritative reading: the provider's stated word,
    /// read from the gateway-quota cache. Never displaced by a learned
    /// value — see [`estimate_subscription_headroom`].
    Stated,
    /// No stated reading existed. Inferred from
    /// [`MIN_LEARNED_RESET_RECOVERIES`] or more throttle→success recoveries
    /// already in window.
    Learned,
}

/// Map line 1249 — whether the rows behind this estimate reach back far
/// enough to say anything about pressure beyond
/// [`RECENT_SIGNAL_HORIZON_SECONDS`], out to
/// [`LONG_SIGNAL_HORIZON_SECONDS`]. A third state, not a bucket guessed from
/// thin evidence: two rows an hour apart cannot tell a multi-hour window
/// from a monthly one, and the honest answer there is
/// [`Self::Undistinguished`] rather than a guessed [`Self::NoPressure`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LongWindowPressure {
    /// No informative row reached back far enough to say anything about the
    /// longer window — absence of evidence, not evidence of absence.
    Undistinguished,
    /// Coverage reached the long horizon and no throttle fell inside it.
    NoPressure,
    /// A throttle fell inside the long horizon, outside the short one:
    /// pressure the short window alone would miss entirely.
    Present,
}

/// Map lines 1244/1245/1246/1250/1251/1254's estimator output: never a bare
/// number.
///
/// # Why a band, never a percentage
///
/// [`crate::provider::quota::Percentage`] already refuses to label an
/// inferred capacity figure as exact (capability map line 1234); this type
/// goes one step further and carries no number at all, because none of its
/// inputs — accepted-request counts, throttle recency, session history — has
/// a natural denominator to divide by. A computed percentage would be a real
/// number glued to an invented scale, exactly what line 1251 forbids for
/// opaque token counts and what this type refuses to make representable for
/// the same reason.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HeadroomBand {
    /// A throttle inside [`RECENT_SIGNAL_HORIZON_SECONDS`] of `now`, with no
    /// reset imminent to relieve it.
    Exhausted,
    /// A throttle fell inside the window — recently, with a reset close
    /// behind it to soften the reading, or earlier and not repeated since.
    Low,
    /// Neither pressure nor activity was observed. A reset reading with
    /// nothing else behind it lands exactly here: real evidence the account
    /// is quota-bound, and none at all that it is under pressure right now.
    Moderate,
    /// Requests were accepted, or this project's own session history served
    /// this account, and no throttle fell in the window.
    Ample,
}

/// What kind of row [`estimate_subscription_headroom`] actually had to work
/// with — carried on the returned value so an opaque-limit account (map line
/// 1244: no token budget its provider will ever publish) and an account
/// whose rows happen to carry a token count render differently, without
/// either claiming more than the estimate has.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HeadroomBasis {
    /// No scoped row carried a token count. Accepted-request counts, throttle
    /// recency, reset behavior and session history are exactly what an
    /// opaque-limit account can supply, and this estimator asks nothing more
    /// of it.
    RequestActivity,
    /// At least one scoped row carried a token count. Recorded as a label
    /// only: map line 1251 forbids turning a raw count into a fictitious
    /// exact figure with no stated ceiling to divide it by, and this
    /// estimator does not duplicate the ceiling check
    /// [`crate::routing::Entitlement::spend_constraint`] already makes — a
    /// carried token count changes this label alone, never the band.
    TokenUsage,
}

/// Map line 1245's estimate, in full: a [`HeadroomBand`], the confidence it
/// is worth, what it was built from, and whose reading it is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SubscriptionHeadroomEstimate {
    pub band: HeadroomBand,
    /// Always [`Confidence::Low`] today — every signal behind this estimate
    /// is Glasshouse's own inference over its own recorded activity, never
    /// the provider's stated word. That is [`Confidence::Low`]'s own
    /// definition: *"derived, with no measurement of this quantity behind it
    /// at all."*
    pub confidence: Confidence,
    pub basis: HeadroomBasis,
    /// Map line 1246's keying rule, reused verbatim from
    /// [`recent_credential_throttles`]: `true` only when every informative
    /// row this estimate drew from named its own account; widened to
    /// provider scope the moment one does not.
    pub account_narrowed: bool,
    /// Map line 1248 — whose reset reading, if any, fed this estimate.
    pub reset_basis: ResetBasis,
    /// Map line 1249 — whether evidence separates short-window pressure
    /// from pressure that persists into the longer horizon.
    pub long_window_pressure: LongWindowPressure,
}

/// Map line 1245's estimator, and lines 1244/1246/1250/1251/1254 with it —
/// see [`SubscriptionHeadroomEstimate`] and [`HeadroomBand`] for the type's
/// own honesty rules. No new table, no migration, no persisted estimator
/// state: every call re-derives the estimate from rows the caller already
/// holds, the same "today's history IS the ledger's own rows in window"
/// premise every other reader in this module keeps.
///
/// Reads five things, none of them queried here:
///
/// - **accepted-request counts** and **throttle events with their
///   recency**, from `observations` — this provider's own informative rows
///   (`outcome.is_some()`, excluding [`CORRELATION_PURPOSE`] rows, the same
///   filter [`classify_throttle_scope`] applies), narrowed to
///   `credential_label` by the widen-when-unsure rule
///   [`recent_credential_throttles`] and [`recent_credential_spend`] already
///   apply: only when **every** informative row names its account does the
///   count narrow, and one contextless row widens the whole estimate to
///   provider scope rather than silently dropping it (map line 1246);
/// - **token usage where rows carry it** — never turned into a figure, only
///   recorded on the returned value's [`HeadroomBasis`] (line 1251);
/// - **reset behavior**, as `seconds_until_reset` — the caller's own
///   gateway-quota-cache reading, already computed for the provider-wide
///   capacity facet and handed in rather than re-read. Map line 1248: when
///   this is `None`, a fallback is learned from `scoped`'s own
///   throttle→success recoveries rather than left unused — see
///   [`ResetBasis`]. The learned value never displaces a real reading;
///   [`SubscriptionHeadroomEstimate::reset_basis`] says which one applied;
/// - **historical sessions**, as `recent_session_count` — this project's own
///   count of sessions charged to this account (`sessions.entitlement`,
///   migration 22), read by the caller and handed in: this function stays a
///   pure read over values already fetched, the shape every other reader in
///   this module keeps.
///
/// `None` — unknown — when nothing at all is available: no informative row,
/// no session count, no reset reading. An account this genuinely unmeasured
/// is not "exhausted" and not "ample"; it is unmeasured, the 32B line-1239
/// discipline every other facet on `ResolvedEntitlement` already keeps.
pub fn estimate_subscription_headroom(
    observations: &[RoutingObservation],
    provider: &str,
    credential_label: Option<&str>,
    now_unix: i64,
    seconds_until_reset: Option<i64>,
    recent_session_count: Option<usize>,
) -> Option<SubscriptionHeadroomEstimate> {
    let informative: Vec<&RoutingObservation> = observations
        .iter()
        .filter(|row| row.provider == provider)
        .filter(|row| row.outcome.is_some() && row.purpose.as_deref() != Some(CORRELATION_PURPOSE))
        .collect();

    let every_row_names_its_account =
        !informative.is_empty() && informative.iter().all(|row| row.quota_context.is_some());
    let account_narrowed = credential_label.is_some() && every_row_names_its_account;

    let scoped: Vec<&RoutingObservation> = if account_narrowed {
        informative
            .into_iter()
            .filter(|row| row.quota_context.as_deref() == credential_label)
            .collect()
    } else {
        informative
    };

    let accepted = scoped
        .iter()
        .filter(|row| row.outcome == Some(Outcome::Succeeded))
        .count();
    let most_recent_throttle_age = scoped
        .iter()
        .filter(|row| row.failure_class == Some(FailureClass::Throttle))
        .map(|row| now_unix.saturating_sub(row.observed_at_unix))
        .min();
    let carried_tokens = scoped
        .iter()
        .any(|row| row.input_tokens.is_some() || row.output_tokens.is_some());

    let session_count = recent_session_count.unwrap_or(0);

    if scoped.is_empty() && session_count == 0 && seconds_until_reset.is_none() {
        return None;
    }

    let recent_pressure =
        most_recent_throttle_age.is_some_and(|age| age <= RECENT_SIGNAL_HORIZON_SECONDS);
    let any_pressure = most_recent_throttle_age.is_some();
    let has_activity = accepted > 0 || session_count > 0;

    // Map line 1248: a stated reading is authoritative and is never
    // recomputed; only its absence opens the door to a learned fallback,
    // and even then only past the anecdote guard.
    let (effective_seconds_until_reset, reset_basis) = match seconds_until_reset {
        Some(seconds) => (Some(seconds), ResetBasis::Stated),
        None => match learn_reset_window_seconds(&scoped) {
            Some(window) => (Some(window), ResetBasis::Learned),
            None => (None, ResetBasis::Unknown),
        },
    };
    let reset_imminent = effective_seconds_until_reset
        .is_some_and(|seconds| (0..=RECENT_SIGNAL_HORIZON_SECONDS).contains(&seconds));

    // Map line 1249: positive evidence of long-window pressure needs no
    // full coverage of the horizon — one throttle out there is real
    // evidence regardless of how far back the rest of `scoped` reaches.
    // Its *absence* does, or the honest answer is "we did not look that
    // far", not "nothing happened".
    let long_window_pressure = {
        let present = scoped
            .iter()
            .filter(|row| row.failure_class == Some(FailureClass::Throttle))
            .map(|row| now_unix.saturating_sub(row.observed_at_unix))
            .any(|age| age > RECENT_SIGNAL_HORIZON_SECONDS && age <= LONG_SIGNAL_HORIZON_SECONDS);
        if present {
            LongWindowPressure::Present
        } else {
            let deepest_age = scoped
                .iter()
                .map(|row| now_unix.saturating_sub(row.observed_at_unix))
                .max();
            match deepest_age {
                Some(age) if age >= LONG_SIGNAL_HORIZON_SECONDS => LongWindowPressure::NoPressure,
                _ => LongWindowPressure::Undistinguished,
            }
        }
    };

    let band = match (recent_pressure, any_pressure, reset_imminent, has_activity) {
        (true, _, true, _) => HeadroomBand::Low,
        (true, _, false, _) => HeadroomBand::Exhausted,
        (false, true, _, _) => HeadroomBand::Low,
        (false, false, _, true) => HeadroomBand::Ample,
        (false, false, _, false) => HeadroomBand::Moderate,
    };

    Some(SubscriptionHeadroomEstimate {
        band,
        confidence: Confidence::Low,
        basis: if carried_tokens {
            HeadroomBasis::TokenUsage
        } else {
            HeadroomBasis::RequestActivity
        },
        account_narrowed,
        reset_basis,
        long_window_pressure,
    })
}

/// Map line 1248's fallback window: the interval between a `Throttle` row
/// and the next `Succeeded` row after it in `scoped`, averaged across every
/// such recovery — `None` below [`MIN_LEARNED_RESET_RECOVERIES`] of them,
/// the anecdote rule stated in the packet this shipped from. Only ever
/// consulted by [`estimate_subscription_headroom`] when the caller supplied
/// no real `seconds_until_reset` at all.
fn learn_reset_window_seconds(scoped: &[&RoutingObservation]) -> Option<i64> {
    let mut ordered: Vec<&RoutingObservation> = scoped.to_vec();
    ordered.sort_by_key(|row| row.observed_at_unix);

    let mut recoveries = Vec::new();
    for (index, row) in ordered.iter().enumerate() {
        if row.failure_class != Some(FailureClass::Throttle) {
            continue;
        }
        if let Some(success) = ordered[index + 1..]
            .iter()
            .find(|later| later.outcome == Some(Outcome::Succeeded))
        {
            let recovery = success
                .observed_at_unix
                .saturating_sub(row.observed_at_unix);
            if recovery > 0 {
                recoveries.push(recovery);
            }
        }
    }

    if recoveries.len() < MIN_LEARNED_RESET_RECOVERIES {
        return None;
    }
    let sum: i64 = recoveries.iter().sum();
    Some(sum / recoveries.len() as i64)
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
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RoutingOverhead {
    /// Rows whose `purpose` is [`CLASSIFICATION_PURPOSE`].
    pub classification_requests: usize,
    pub classification_tokens: Option<i64>,
    /// Every other row the ledger holds in the window — gateway exchanges,
    /// memory extraction, anything a later producer stamps with another
    /// purpose.
    ///
    /// **This stays the line-1466 denominator and keeps its meaning**, and
    /// the four fields below are its breakdown rather than a partition that
    /// replaces it: `extraction + routing_latency + tier_movement + coding_agent +
    /// unstamped == task_requests` exactly, by construction.
    pub task_requests: usize,
    pub task_tokens: Option<i64>,
    /// Rows whose `purpose` is [`EXTRACTION_PURPOSE`] — capability map line
    /// 1832's *"memory-extraction cost, separately from interactive coding
    /// cost"*. Stamped from the build this constant landed in; earlier
    /// extraction rows are in [`Self::unstamped_requests`] and are never
    /// moved here.
    pub extraction_requests: usize,
    pub extraction_tokens: Option<i64>,
    /// Rows whose `purpose` is [`ROUTING_LATENCY_PURPOSE`] — line 1833's
    /// *request consumption* half for the routing model's own decision
    /// timing. These carry no tokens by construction, so a token figure here
    /// is honestly absent rather than zero.
    pub routing_latency_requests: usize,
    pub routing_latency_tokens: Option<i64>,
    /// Rows whose `purpose` is [`TIER_ESCALATION_PURPOSE`] or
    /// [`TIER_DOWNGRADE_PURPOSE`] — line 1566's record of the session
    /// router moving the tier it prefers. No tokens by construction, for
    /// [`ROUTING_LATENCY_PURPOSE`]'s reason.
    pub tier_movement_requests: usize,
    pub tier_movement_tokens: Option<i64>,
    /// Rows whose `purpose` is [`ENTITLEMENT_FALLBACK_EXHAUSTED_PURPOSE`]
    /// or [`ENTITLEMENT_FALLBACK_THROTTLED_PURPOSE`] — line 1970's record
    /// of the broker leaving an account. No tokens by construction, for
    /// [`ROUTING_LATENCY_PURPOSE`]'s reason.
    pub entitlement_fallback_requests: usize,
    pub entitlement_fallback_tokens: Option<i64>,
    /// Rows whose `purpose` is [`CONTEXT_FIREWALL_REDUCTION_PURPOSE`],
    /// [`CONTEXT_FIREWALL_BYPASS_PURPOSE`], or
    /// [`CONTEXT_FIREWALL_EXPANSION_PURPOSE`] — map lines 1987 and 1988's
    /// telemetry. No tokens by construction, for the reason
    /// [`CONTEXT_FIREWALL_REDUCTION_PURPOSE`]'s own doc comment gives: this
    /// purpose's producer never writes an estimate into a column documented
    /// as a provider's own report.
    pub context_firewall_requests: usize,
    pub context_firewall_tokens: Option<i64>,
    /// Rows no producer stamped that **did** name a harness — the gateway
    /// relay, and today nothing else. This is *"interactive coding cost"* as
    /// lines 1832 and 1833 use the phrase, and it is the one side of the
    /// separation this build cannot count in tokens:
    /// `crate::gateway::ingress` relays a body it is designed never to
    /// parse, so every one of these rows leaves all three token columns
    /// `NULL`. The request count is real; the token figure is absent, and
    /// must render as absent.
    pub coding_agent_requests: usize,
    pub coding_agent_tokens: Option<i64>,
    /// Everything none of the four named buckets claims — today exactly the
    /// rows written before this build stamped a purpose (no `purpose`, no
    /// harness), which is every memory-extraction call the previous builds
    /// recorded.
    ///
    /// **Its own bucket precisely so those rows are neither re-labelled nor
    /// silently counted as somebody else's spend.** A `purpose` a later
    /// build writes and this one does not know would also land here, which
    /// is visible degradation rather than a wrong attribution.
    pub unstamped_requests: usize,
    pub unstamped_tokens: Option<i64>,
}

/// Fold one group's counts into one bucket, keeping an absent token count
/// absent.
///
/// `Some(0)` and `None` are different facts here — the whole reason
/// [`PurposeConsumption`]'s token fields are `Option` — so a bucket only
/// becomes counted once a group that carried a count reaches it.
fn add_consumption(bucket: (&mut usize, &mut Option<i64>), requests: usize, tokens: Option<i64>) {
    let (count, total) = bucket;
    *count += requests;
    if let Some(tokens) = tokens {
        *total = Some(total.unwrap_or(0) + tokens);
    }
}

impl RoutingOverhead {
    pub fn from_consumption(groups: &[PurposeConsumption]) -> Self {
        let mut overhead = Self::default();
        for group in groups {
            let tokens = match (group.input_tokens, group.output_tokens) {
                (None, None) => None,
                (input, output) => Some(input.unwrap_or(0) + output.unwrap_or(0)),
            };
            // The named bucket this group belongs to. `harness_recorded` is
            // what tells the two `NULL`-purpose producers apart — see
            // [`PurposeConsumption`]'s own doc comment — so an unstamped row
            // that named a harness is the coding agent's, and one that named
            // none is a row written before this build stamped a purpose.
            let named = match group.purpose.as_deref() {
                Some(CLASSIFICATION_PURPOSE) => (
                    &mut overhead.classification_requests,
                    &mut overhead.classification_tokens,
                ),
                Some(EXTRACTION_PURPOSE) => (
                    &mut overhead.extraction_requests,
                    &mut overhead.extraction_tokens,
                ),
                // Line 1852's rows: one per steered failover, no tokens and
                // no request to any model. Not spend on either side of line
                // 1466's comparison, so neither a bucket nor the denominator
                // — see `CORRELATION_PURPOSE`'s own doc comment.
                Some(CORRELATION_PURPOSE) => continue,
                Some(ROUTING_LATENCY_PURPOSE) => (
                    &mut overhead.routing_latency_requests,
                    &mut overhead.routing_latency_tokens,
                ),
                Some(TIER_ESCALATION_PURPOSE | TIER_DOWNGRADE_PURPOSE) => (
                    &mut overhead.tier_movement_requests,
                    &mut overhead.tier_movement_tokens,
                ),
                Some(
                    ENTITLEMENT_FALLBACK_EXHAUSTED_PURPOSE | ENTITLEMENT_FALLBACK_THROTTLED_PURPOSE,
                ) => (
                    &mut overhead.entitlement_fallback_requests,
                    &mut overhead.entitlement_fallback_tokens,
                ),
                Some(
                    CONTEXT_FIREWALL_REDUCTION_PURPOSE
                    | CONTEXT_FIREWALL_BYPASS_PURPOSE
                    | CONTEXT_FIREWALL_EXPANSION_PURPOSE,
                ) => (
                    &mut overhead.context_firewall_requests,
                    &mut overhead.context_firewall_tokens,
                ),
                None if group.harness_recorded => (
                    &mut overhead.coding_agent_requests,
                    &mut overhead.coding_agent_tokens,
                ),
                _ => (
                    &mut overhead.unstamped_requests,
                    &mut overhead.unstamped_tokens,
                ),
            };
            add_consumption(named, group.sample_count, tokens);
            // Line 1466's denominator is *everything that is not the routing
            // model*, and it keeps that meaning: the four buckets above,
            // minus classification, sum to exactly this.
            if group.purpose.as_deref() != Some(CLASSIFICATION_PURPOSE) {
                add_consumption(
                    (&mut overhead.task_requests, &mut overhead.task_tokens),
                    group.sample_count,
                    tokens,
                );
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

/// One [`EvidenceLedger::request_stats_by_harness`] row — map line 1951's
/// token/wall-clock/request-count half for one harness.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HarnessRequestStats {
    pub harness: String,
    /// Every `routing_observations` row this harness produced in the
    /// window, whether or not it carries timing or token data.
    pub requests: i64,
    /// `None` when no row in this window carries both `dispatched_at` and
    /// `completed_at` — never a fabricated zero.
    pub wall_clock: Option<WallClockSummary>,
    /// Rows carrying an `input_tokens` count — the relay path's rows never
    /// do (refusal register P1b), so this is `0` there, not [`Self::requests`].
    pub token_rows_present: i64,
    /// `input_tokens` summed over exactly [`Self::token_rows_present`] rows.
    /// A caller must print *"not exposed on `requests -
    /// token_rows_present` of `requests` exchanges"* rather than this sum
    /// alone whenever `token_rows_present < requests` (map line 1951's own
    /// mutation: printing `0` for an all-`NULL` group is refused).
    pub input_tokens_sum: i64,
    pub output_tokens_sum: i64,
}

impl HarnessRequestStats {
    fn from_rows(harness: String, rows: &[RoutingObservation]) -> Self {
        let durations: Vec<i64> = rows
            .iter()
            .filter_map(RoutingObservation::duration_ms)
            .collect();
        let wall_clock = (!durations.is_empty()).then(|| WallClockSummary {
            sample_count: durations.len() as i64,
            sum_ms: durations.iter().sum(),
            median_ms: median(durations.clone()),
        });
        let with_tokens: Vec<&RoutingObservation> = rows
            .iter()
            .filter(|observation| observation.input_tokens.is_some())
            .collect();
        Self {
            harness,
            requests: rows.len() as i64,
            wall_clock,
            token_rows_present: with_tokens.len() as i64,
            input_tokens_sum: with_tokens.iter().filter_map(|o| o.input_tokens).sum(),
            output_tokens_sum: with_tokens.iter().filter_map(|o| o.output_tokens).sum(),
        }
    }
}

/// [`HarnessRequestStats::wall_clock`] — `completed_at - dispatched_at`
/// over exactly the rows that carry both, matching
/// [`RoutingObservation::duration_ms`]'s own gap: neither timestamp is
/// invented for a row missing one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WallClockSummary {
    pub sample_count: i64,
    pub sum_ms: i64,
    pub median_ms: i64,
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

    /// **Map line 1629**'s reader: the most recent observations, newest
    /// first, whose `purpose` is [`CLASSIFICATION_PURPOSE`] or
    /// [`EXTRACTION_PURPOSE`] — *"which resource performed important memory
    /// extraction or classification for debugging"* — across every
    /// `(provider, model, route, harness)` identity at once.
    ///
    /// **Not [`Self::recent`].** That method requires the caller to already
    /// name one identity via [`ObservationQuery`], and the question this
    /// line asks is the opposite: which identity performed the work,
    /// unknown in advance. A purpose-filtered sibling fits where `recent`'s
    /// exact-identity shape does not.
    pub fn recent_support_work(
        &self,
        limit: usize,
    ) -> Result<Vec<RoutingObservation>, EvidenceLedgerError> {
        let conn = self.lock();
        let mut statement = conn
            .prepare(
                "SELECT * FROM routing_observations
                 WHERE project_id = ?1 AND purpose IN (?2, ?3)
                 ORDER BY observed_at DESC
                 LIMIT ?4",
            )
            .map_err(sql_err("read support-work routing observations"))?;
        let rows = statement
            .query_map(
                params![
                    self.project_id,
                    CLASSIFICATION_PURPOSE,
                    EXTRACTION_PURPOSE,
                    limit as i64
                ],
                row_to_observation,
            )
            .map_err(sql_err("read support-work routing observations"))?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row.map_err(sql_err("read a support-work routing observation"))??);
        }
        Ok(out)
    }

    /// **Map line 1951**'s token/wall-clock/request-count half, grouped by
    /// harness alone. `routing_observations.harness` is written directly by
    /// every producer
    /// (`crate::gateway::session::record_routing_observation`'s
    /// `.with_harness(...)`, and `main.rs`'s five `with_purpose` call
    /// sites), so this needs no join to `sessions` — unlike
    /// [`crate::evaluation::EvaluationObservations::outcomes_by_tier_and_harness`]'s
    /// outcome half, which has no harness of its own to read and joins
    /// `sessions.harness` instead.
    ///
    /// Reads the raw rows and folds them in Rust rather than aggregating in
    /// SQL — the same choice [`Self::route_correlations`] and
    /// [`Self::throttle_scopes`] make and for the same reason: the
    /// wall-clock median and the "rows without token data" split are
    /// decisions worth testing without a database, not SQL to get right
    /// once and never examine again.
    pub fn request_stats_by_harness(
        &self,
        from: i64,
        to: i64,
    ) -> Result<Vec<HarnessRequestStats>, EvidenceLedgerError> {
        let observations = {
            let conn = self.lock();
            let mut statement = conn
                .prepare(
                    "SELECT * FROM routing_observations
                     WHERE project_id = ?1 AND observed_at >= ?2 AND observed_at <= ?3
                     ORDER BY harness IS NULL, harness ASC, observed_at ASC",
                )
                .map_err(sql_err("read routing observations by harness"))?;
            let rows = statement
                .query_map(params![self.project_id, from, to], row_to_observation)
                .map_err(sql_err("read routing observations by harness"))?;
            let mut observations = Vec::new();
            for row in rows {
                observations.push(row.map_err(sql_err("read a routing observation"))??);
            }
            observations
        };

        let mut by_harness: std::collections::BTreeMap<String, Vec<RoutingObservation>> =
            std::collections::BTreeMap::new();
        for observation in observations {
            let harness = observation
                .harness
                .clone()
                .unwrap_or_else(|| UNKNOWN_HARNESS.to_owned());
            by_harness.entry(harness).or_default().push(observation);
        }

        Ok(by_harness
            .into_iter()
            .map(|(harness, rows)| HarnessRequestStats::from_rows(harness, &rows))
            .collect())
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

    /// Every pair of routes this project has observed failing or serving at
    /// the same moments, over the window ending at `now_unix` — lines 1370,
    /// 1373, 1374 and 1376's reader, and the one door
    /// `crate::gateway::session::SessionRouting::observe_exchange` reaches
    /// [`correlate_routes`] through.
    ///
    /// Reads every outcome-carrying row in the window in one pass and hands
    /// them to the pure function rather than joining in SQL: the overlap
    /// tolerance, the class match and the minimum are decisions, and a
    /// decision belongs where a test reaches it without a database. Rows
    /// with no outcome never inform a pair (see [`RouteCorrelation`]), so
    /// the query leaves them on disk.
    ///
    /// Called once per provider failure, not per exchange: a failover is a
    /// small minority of exchanges, and a full-window read at that moment
    /// costs less than keeping a correlation warm across every exchange that
    /// moved nothing.
    pub fn route_correlations(
        &self,
        now_unix: i64,
        window_seconds: i64,
    ) -> Result<RouteCorrelations, EvidenceLedgerError> {
        let earliest = now_unix.saturating_sub(window_seconds);
        let observations = {
            let conn = self.lock();
            let mut statement = conn
                .prepare(
                    "SELECT * FROM routing_observations
                     WHERE project_id = ?1
                       AND observed_at >= ?2 AND observed_at <= ?3
                       AND outcome IS NOT NULL
                     ORDER BY observed_at ASC",
                )
                .map_err(sql_err("read routing observations for correlation"))?;
            let rows = statement
                .query_map(
                    params![self.project_id, earliest, now_unix],
                    row_to_observation,
                )
                .map_err(sql_err("read routing observations for correlation"))?;
            let mut observations = Vec::new();
            for row in rows {
                observations.push(row.map_err(sql_err("read a routing observation"))??);
            }
            observations
        };
        Ok(correlate_routes(&observations))
    }

    /// Capability map line 1317's reader: [`classify_throttle_scopes`], fed
    /// every outcome-carrying row in the window ending at `now_unix` — the
    /// same query shape [`Self::route_correlations`] runs, for the same
    /// reason: the tolerance, the class match and the minimum are decisions,
    /// and a decision belongs where a test reaches it without a database.
    pub fn throttle_scopes(
        &self,
        now_unix: i64,
        window_seconds: i64,
    ) -> Result<ThrottleScopes, EvidenceLedgerError> {
        Ok(classify_throttle_scopes(
            &self.observations_in_window(now_unix, window_seconds)?,
        ))
    }

    /// Every outcome-carrying observation in the window ending at `now_unix`
    /// — the exact row set [`Self::throttle_scopes`] and
    /// [`Self::route_correlations`] classify, exposed for a caller that
    /// needs the rows themselves: map line 1965's entitlement telemetry
    /// resolver narrows them by provider and
    /// [`RoutingObservation::quota_context`]
    /// ([`recent_credential_throttles`]).
    pub fn observations_in_window(
        &self,
        now_unix: i64,
        window_seconds: i64,
    ) -> Result<Vec<RoutingObservation>, EvidenceLedgerError> {
        let earliest = now_unix.saturating_sub(window_seconds);
        let conn = self.lock();
        let mut statement = conn
            .prepare(
                "SELECT * FROM routing_observations
                 WHERE project_id = ?1
                   AND observed_at >= ?2 AND observed_at <= ?3
                   AND outcome IS NOT NULL
                 ORDER BY observed_at ASC",
            )
            .map_err(sql_err("read routing observations in a window"))?;
        let rows = statement
            .query_map(
                params![self.project_id, earliest, now_unix],
                row_to_observation,
            )
            .map_err(sql_err("read routing observations in a window"))?;
        let mut observations = Vec::new();
        for row in rows {
            observations.push(row.map_err(sql_err("read a routing observation"))??);
        }
        Ok(observations)
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

    /// Capability map line 1564's producer: the [`FailureClass`] of the
    /// **most recent** exchange this project recorded against `(provider,
    /// model)` within the window — `Ok(None)` when nothing was recorded, or
    /// the latest row carried no class (it succeeded, or a producer wrote a
    /// verdict without a kind).
    ///
    /// The latest row and not a count: line 1564 says *after* a clearly
    /// attributable failure, and "the last thing that happened on this
    /// backend" is the attribution this ledger can honestly make — rows
    /// carry no session id, so a count over the window would mix in every
    /// other session's exchanges. `main.rs`'s task-boundary `route` path
    /// reads it for the destination the work is on and hands it to
    /// `SessionRouter::with_retry_after`, which promotes one tier on a
    /// [`FailureClass::RequestIncompatibility`] or
    /// [`FailureClass::EmptyCompletion`] and on nothing else.
    ///
    /// Scoped to this ledger's `project_id`, like [`Self::observed_identities`].
    pub fn latest_failure_class_for_model(
        &self,
        provider: &str,
        model: &str,
        now_unix: i64,
        window_seconds: i64,
    ) -> Result<Option<FailureClass>, EvidenceLedgerError> {
        let earliest = now_unix.saturating_sub(window_seconds);
        let stored: Option<Option<String>> = {
            let conn = self.lock();
            conn.query_row(
                "SELECT failure_class
                 FROM routing_observations
                 WHERE project_id = ?1 AND provider = ?2 AND model = ?3
                   AND observed_at >= ?4 AND observed_at <= ?5
                 ORDER BY observed_at DESC, seq DESC
                 LIMIT 1",
                params![self.project_id, provider, model, earliest, now_unix],
                |row| row.get(0),
            )
            .optional()
            .map_err(sql_err("find the most recent failure class for a model"))?
        };
        match stored.flatten() {
            None => Ok(None),
            Some(text) => FailureClass::from_stored(&text).map(Some).ok_or(
                EvidenceLedgerError::UnknownAggregateValue {
                    column: "failure_class",
                    value: text,
                },
            ),
        }
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

    /// Line 1564's producer: the **latest** row decides, a succeeded latest
    /// row answers `None` even after earlier failures, and a pair nobody
    /// recorded answers `None` rather than borrowing a neighbour's history.
    #[test]
    fn the_latest_failure_class_is_the_most_recent_rows_and_nothing_older() {
        let tmp = tempfile::tempdir().unwrap();
        let fixture = Fixture::new(tmp.path(), "alpha");
        let ledger = fixture.ledger();
        let record = |at: i64, outcome: Outcome, class: Option<FailureClass>| {
            ledger
                .record(
                    observation("alpha", "mid")
                        .with_timing(Some(at), Some(at))
                        .with_outcome(outcome)
                        .with_failure_class(class),
                    at,
                )
                .unwrap();
        };

        assert_eq!(
            ledger
                .latest_failure_class_for_model("alpha", "mid", 1_000, 600)
                .unwrap(),
            None
        );
        record(900, Outcome::Failed, Some(FailureClass::Throttle));
        record(950, Outcome::Failed, Some(FailureClass::EmptyCompletion));
        assert_eq!(
            ledger
                .latest_failure_class_for_model("alpha", "mid", 1_000, 600)
                .unwrap(),
            Some(FailureClass::EmptyCompletion),
            "the most recent row, not the first or the most frequent"
        );
        record(980, Outcome::Succeeded, None);
        assert_eq!(
            ledger
                .latest_failure_class_for_model("alpha", "mid", 1_000, 600)
                .unwrap(),
            None,
            "a success after a failure is not a failure to promote on"
        );
        assert_eq!(
            ledger
                .latest_failure_class_for_model("alpha", "other-model", 1_000, 600)
                .unwrap(),
            None,
            "another model's history is not this one's"
        );
        assert_eq!(
            ledger
                .latest_failure_class_for_model("alpha", "mid", 2_000, 600)
                .unwrap(),
            None,
            "outside the window there is no history"
        );
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

/// Capability map lines 1370, 1373, 1374 and 1376 on the pure function,
/// with no database — each test here is the named killer of one of the
/// packet's four mutations, and the helpers build rows the way the gateway
/// producer writes them (a window, an outcome, a class when it failed).
#[cfg(test)]
mod correlation_tests {
    use super::*;

    fn row(
        provider: &str,
        model: &str,
        start: i64,
        end: i64,
        class: Option<FailureClass>,
    ) -> RoutingObservation {
        RoutingObservation {
            seq: 0,
            project_id: "project".to_owned(),
            observed_at_unix: end,
            provider: provider.to_owned(),
            model: model.to_owned(),
            route: Some("anthropic-messages".to_owned()),
            quota_context: None,
            harness: Some("claude-code".to_owned()),
            purpose: None,
            dispatched_at_unix: Some(start),
            first_byte_at_unix: None,
            first_token_at_unix: None,
            first_tool_call_at_unix: None,
            completed_at_unix: Some(end),
            input_tokens: None,
            output_tokens: None,
            cached_input_tokens: None,
            cost: None,
            tool_rounds: None,
            retries: None,
            repairs: None,
            failovers: None,
            outcome: Some(if class.is_some() {
                Outcome::Failed
            } else {
                Outcome::Succeeded
            }),
            failure_class: class,
            context_state: ContextState::Unknown,
        }
    }

    fn five_xx(provider: &str, start: i64) -> RoutingObservation {
        row(
            provider,
            "the-model",
            start,
            start + 5,
            Some(FailureClass::Upstream5xx),
        )
    }

    fn served(provider: &str, start: i64) -> RoutingObservation {
        row(provider, "the-model", start, start + 5, None)
    }

    fn route(provider: &str) -> RouteIdentity {
        RouteIdentity::new(provider, "the-model")
    }

    /// Line 1370 — kills *drop the overlap test*. Two 5xx thirty seconds
    /// apart are one moment; two 5xx sixty-one seconds apart (measured from
    /// the first window's end) are two, and the second one, with the other
    /// route serving in between, is a lone failure rather than an overlap.
    #[test]
    fn an_overlap_is_measured_within_the_tolerance_and_not_beyond_it() {
        let rows = vec![
            five_xx("a", 0),
            five_xx("b", 30),
            five_xx("a", 1_000),
            served("b", 1_010),
            five_xx("b", 1_005 + CORRELATION_OVERLAP_TOLERANCE_SECONDS + 1),
        ];
        let pair = correlate_routes(&rows).between(&route("a"), &route("b"));
        assert_eq!(
            (pair.overlaps(), pair.lone()),
            (2, 1),
            "a's first failure and b's answer to it are one overlap each way; a's second \
             failure saw b serving and b's late failure saw nobody: {pair:?}"
        );
    }

    /// Line 1373 — kills *match on class only* in its provider-metadata
    /// half: the identity is `(provider, model)`, so `b/x` failing beside
    /// `a/x` says nothing about `b/y`, which was serving at the time.
    #[test]
    fn a_correlation_is_model_specific_not_provider_wide() {
        let rows = vec![
            five_xx("a", 0),
            five_xx("b", 10),
            row("b", "other-model", 10, 15, None),
        ];
        let correlations = correlate_routes(&rows);
        let same_model = correlations.between(&route("a"), &route("b"));
        assert_eq!((same_model.overlaps(), same_model.lone()), (2, 0));
        let other_model =
            correlations.between(&route("a"), &RouteIdentity::new("b", "other-model"));
        assert_eq!(
            (other_model.overlaps(), other_model.lone()),
            (0, 1),
            "the other model on the same provider was observed serving through a's failure, \
             and that is evidence against it sharing a's failure domain: {other_model:?}"
        );
    }

    /// Line 1373 — kills *match on class only* in its serving-behaviour
    /// half: a credential failure beside a 5xx, or a throttle beside a 5xx,
    /// is the other route being observed and **not** failing the same way.
    #[test]
    fn a_different_failure_class_at_the_same_moment_is_not_a_match() {
        let rows = vec![
            five_xx("a", 0),
            row(
                "b",
                "the-model",
                10,
                15,
                Some(FailureClass::CredentialFailure),
            ),
            row("a", "the-model", 100, 105, Some(FailureClass::Throttle)),
            five_xx("b", 110),
        ];
        let pair = correlate_routes(&rows).between(&route("a"), &route("b"));
        assert_eq!(
            (pair.overlaps(), pair.lone()),
            (0, 3),
            "a's 5xx saw a bad key, a's throttle saw a 5xx, b's 5xx saw a throttle — three \
             observed failures, none matched: {pair:?}"
        );
    }

    /// Line 1374 — kills *freeze the confidence*: the same pair read three
    /// times as rows arrive goes 1.00, then down to 0.50, then up to 0.75.
    #[test]
    fn new_rows_move_the_confidence_both_ways() {
        let mut rows = Vec::new();
        for i in 0..5 {
            rows.push(five_xx("a", i * 1_000));
            rows.push(five_xx("b", i * 1_000 + 10));
        }
        let first = correlate_routes(&rows).between(&route("a"), &route("b"));
        assert_eq!(first.confidence(), Some(1.0), "{first:?}");

        for i in 0..10 {
            rows.push(five_xx("a", 100_000 + i * 1_000));
            rows.push(served("b", 100_000 + i * 1_000 + 10));
        }
        let second = correlate_routes(&rows).between(&route("a"), &route("b"));
        assert_eq!(second.confidence(), Some(0.5), "{second:?}");

        for i in 0..10 {
            rows.push(five_xx("a", 200_000 + i * 1_000));
            rows.push(five_xx("b", 200_000 + i * 1_000 + 10));
        }
        let third = correlate_routes(&rows).between(&route("a"), &route("b"));
        assert_eq!(third.confidence(), Some(0.75), "{third:?}");
        assert_eq!(third.sample_size(), 40);
    }

    /// Line 1376 — kills *ignore the minimum*: four informative events is
    /// insufficient, says so with both numbers, and yields no confidence;
    /// the fifth makes it a measurement.
    #[test]
    fn below_the_minimum_sample_the_verdict_is_insufficient_and_says_the_count() {
        let mut rows = vec![
            five_xx("a", 0),
            five_xx("b", 10),
            five_xx("a", 1_000),
            five_xx("b", 1_010),
        ];
        let short = correlate_routes(&rows).between(&route("a"), &route("b"));
        assert_eq!(
            short.verdict(),
            CorrelationVerdict::InsufficientEvidence {
                sample_size: 4,
                required: MIN_CORRELATION_SAMPLE,
            }
        );
        assert_eq!(short.confidence(), None);

        rows.push(five_xx("a", 2_000));
        rows.push(served("b", 2_010));
        let enough = correlate_routes(&rows).between(&route("a"), &route("b"));
        assert_eq!(
            enough.verdict(),
            CorrelationVerdict::Measured {
                confidence: 0.8,
                sample_size: 5,
            }
        );
    }

    /// Line 1370's other half: a failure while the other route was idle is
    /// not evidence of independence, and a pair nobody has observed together
    /// is unmeasured rather than absent.
    #[test]
    fn a_failure_while_the_other_route_was_idle_informs_nothing() {
        let rows = vec![five_xx("a", 0), served("b", 10_000)];
        let correlations = correlate_routes(&rows);
        assert!(correlations.is_empty());
        let pair = correlations.between(&route("b"), &route("a"));
        assert_eq!(pair.sample_size(), 0);
        assert_eq!(
            pair.routes(),
            (&route("a"), &route("b")),
            "either order is the same pair"
        );
    }

    /// The reader never feeds on its own output or on rows nobody judged:
    /// a `CORRELATION_PURPOSE` row and an outcome-less row beside a failure
    /// leave that failure uninformative.
    #[test]
    fn a_correlation_row_and_an_unjudged_row_are_not_evidence() {
        let mut steer = served("b", 10);
        steer.purpose = Some(CORRELATION_PURPOSE.to_owned());
        let mut unjudged = served("b", 20);
        unjudged.outcome = None;
        let rows = vec![five_xx("a", 0), steer, unjudged];
        assert!(correlate_routes(&rows).is_empty());
    }

    /// Line 1852's rows are not spend on either side of line 1466.
    #[test]
    fn from_consumption_leaves_correlation_rows_out_of_every_bucket() {
        let groups = [
            PurposeConsumption {
                purpose: Some(CORRELATION_PURPOSE.to_owned()),
                harness_recorded: false,
                sample_count: 3,
                input_tokens: None,
                output_tokens: None,
                cached_input_tokens: None,
                first_byte_sample_count: 0,
                mean_time_to_first_byte_ms: None,
            },
            PurposeConsumption {
                purpose: Some("a-purpose-this-build-does-not-know".to_owned()),
                harness_recorded: false,
                sample_count: 2,
                input_tokens: None,
                output_tokens: None,
                cached_input_tokens: None,
                first_byte_sample_count: 0,
                mean_time_to_first_byte_ms: None,
            },
        ];
        let overhead = RoutingOverhead::from_consumption(&groups);
        assert_eq!(
            (overhead.task_requests, overhead.unstamped_requests),
            (2, 2),
            "the unknown purpose still degrades visibly into unstamped; the correlation rows \
             are nowhere: {overhead:?}"
        );
    }

    #[test]
    fn a_window_falls_back_to_observed_at_and_never_runs_backwards() {
        let mut point = served("a", 100);
        point.dispatched_at_unix = None;
        point.completed_at_unix = None;
        point.observed_at_unix = 42;
        assert_eq!(point.window(), (42, 42));
        let mut backwards = served("a", 100);
        backwards.completed_at_unix = Some(50);
        assert_eq!(backwards.window(), (100, 100));
    }
}

#[cfg(test)]
mod throttle_scope_tests {
    use super::*;

    fn row(
        provider: &str,
        model: &str,
        start: i64,
        end: i64,
        class: Option<FailureClass>,
    ) -> RoutingObservation {
        RoutingObservation {
            seq: 0,
            project_id: "project".to_owned(),
            observed_at_unix: end,
            provider: provider.to_owned(),
            model: model.to_owned(),
            route: Some("anthropic-messages".to_owned()),
            quota_context: None,
            harness: Some("claude-code".to_owned()),
            purpose: None,
            dispatched_at_unix: Some(start),
            first_byte_at_unix: None,
            first_token_at_unix: None,
            first_tool_call_at_unix: None,
            completed_at_unix: Some(end),
            input_tokens: None,
            output_tokens: None,
            cached_input_tokens: None,
            cost: None,
            tool_rounds: None,
            retries: None,
            repairs: None,
            failovers: None,
            outcome: Some(if class.is_some() {
                Outcome::Failed
            } else {
                Outcome::Succeeded
            }),
            failure_class: class,
            context_state: ContextState::Unknown,
        }
    }

    fn throttle(provider: &str, model: &str, start: i64) -> RoutingObservation {
        row(
            provider,
            model,
            start,
            start + 5,
            Some(FailureClass::Throttle),
        )
    }

    fn served(provider: &str, model: &str, start: i64) -> RoutingObservation {
        row(provider, model, start, start + 5, None)
    }

    fn route(provider: &str, model: &str) -> RouteIdentity {
        RouteIdentity::new(provider, model)
    }

    /// Line 1317, its provider-wide half — kills *collapse provider-wide
    /// into model-specific*: five throttles on `x` each overlapped by a
    /// throttle on sibling model `y` of the same provider is direct evidence
    /// the limiter reached both.
    #[test]
    fn overlapping_throttles_on_sibling_models_read_as_provider_wide() {
        let rows: Vec<RoutingObservation> = (0..5)
            .flat_map(|i| {
                let at = i * 1_000;
                [throttle("a", "x", at), throttle("a", "y", at + 10)]
            })
            .collect();
        assert_eq!(
            classify_throttle_scope(&rows, &route("a", "x")),
            ThrottleScope::ProviderWide,
            "every throttle on x overlapped a throttle on y of the same provider"
        );
    }

    /// Line 1317, its model-specific half — kills *ignore the sibling
    /// model's success*: five throttles on `x`, each overlapped by `y`
    /// serving normally, is evidence the limiter never reached `y`.
    #[test]
    fn a_throttle_overlapped_by_a_sibling_models_success_reads_as_model_specific() {
        let rows: Vec<RoutingObservation> = (0..5)
            .flat_map(|i| {
                let at = i * 1_000;
                [throttle("a", "x", at), served("a", "y", at + 10)]
            })
            .collect();
        assert_eq!(
            classify_throttle_scope(&rows, &route("a", "x")),
            ThrottleScope::ModelSpecific,
            "every throttle on x was observed against a sibling that kept serving"
        );
    }

    /// A single provider-wide instance outweighs any number of
    /// model-specific ones — the scope answers "did the limiter ever reach
    /// another model", not a majority vote.
    #[test]
    fn one_overlapping_throttle_among_many_lone_ones_still_reads_as_provider_wide() {
        let mut rows: Vec<RoutingObservation> = (0..4)
            .flat_map(|i| {
                let at = i * 1_000;
                [throttle("a", "x", at), served("a", "y", at + 10)]
            })
            .collect();
        rows.push(throttle("a", "x", 100_000));
        rows.push(throttle("a", "y", 100_010));
        assert_eq!(
            classify_throttle_scope(&rows, &route("a", "x")),
            ThrottleScope::ProviderWide
        );
    }

    /// Line 1317 — kills *drop the minimum sample*: four informative
    /// throttle events is insufficient and says so with both numbers; the
    /// fifth makes it a verdict.
    #[test]
    fn below_the_minimum_sample_the_scope_is_unknown_and_says_the_count() {
        let mut rows: Vec<RoutingObservation> = (0..4)
            .flat_map(|i| {
                let at = i * 1_000;
                [throttle("a", "x", at), served("a", "y", at + 10)]
            })
            .collect();
        assert_eq!(
            classify_throttle_scope(&rows, &route("a", "x")),
            ThrottleScope::Unknown {
                sample_size: 4,
                required: MIN_CORRELATION_SAMPLE,
            }
        );

        rows.push(throttle("a", "x", 5_000));
        rows.push(served("a", "y", 5_010));
        assert_eq!(
            classify_throttle_scope(&rows, &route("a", "x")),
            ThrottleScope::ModelSpecific
        );
    }

    /// A throttle observed against no sibling at all is uninformative, same
    /// as [`correlate_routes`]'s own rule — it does not count toward the
    /// sample and does not make the scope provider-wide by default.
    #[test]
    fn a_throttle_with_no_sibling_observed_is_uninformative() {
        let rows = vec![throttle("a", "x", 0)];
        assert_eq!(
            classify_throttle_scope(&rows, &route("a", "x")),
            ThrottleScope::Unknown {
                sample_size: 0,
                required: MIN_CORRELATION_SAMPLE,
            }
        );
    }

    /// Only [`FailureClass::Throttle`] counts, not every correlatable class:
    /// an `Upstream5xx` on `x` says nothing about line 1317's question even
    /// when a sibling model failed the same way at the same moment.
    #[test]
    fn an_upstream_5xx_is_not_a_throttle_and_contributes_nothing() {
        let rows: Vec<RoutingObservation> = (0..5)
            .flat_map(|i| {
                let at = i * 1_000;
                [
                    row("a", "x", at, at + 5, Some(FailureClass::Upstream5xx)),
                    row("a", "y", at + 10, at + 15, Some(FailureClass::Upstream5xx)),
                ]
            })
            .collect();
        assert_eq!(
            classify_throttle_scope(&rows, &route("a", "x")),
            ThrottleScope::Unknown {
                sample_size: 0,
                required: MIN_CORRELATION_SAMPLE,
            },
            "5xx rows are not throttles and do not inform this scope"
        );
    }

    /// A different provider's model is not a sibling: `b/x` throttling
    /// beside `a/x` says nothing about `a`'s own other models.
    #[test]
    fn a_different_providers_model_is_not_a_sibling() {
        let rows: Vec<RoutingObservation> = (0..5)
            .flat_map(|i| {
                let at = i * 1_000;
                [throttle("a", "x", at), throttle("b", "x", at + 10)]
            })
            .collect();
        assert_eq!(
            classify_throttle_scope(&rows, &route("a", "x")),
            ThrottleScope::Unknown {
                sample_size: 0,
                required: MIN_CORRELATION_SAMPLE,
            }
        );
    }

    /// [`classify_throttle_scopes`] finds every throttled route and nothing
    /// else, and [`ThrottleScopes::for_route`] answers a route it never saw
    /// with an honest zero rather than a panic or a default guess.
    #[test]
    fn classify_throttle_scopes_covers_every_throttled_route_and_no_others() {
        let mut rows: Vec<RoutingObservation> = (0..5)
            .flat_map(|i| {
                let at = i * 1_000;
                [throttle("a", "x", at), throttle("a", "y", at + 10)]
            })
            .collect();
        rows.push(served("c", "z", 999_999));
        let scopes = classify_throttle_scopes(&rows);

        assert_eq!(
            scopes.for_route(&route("a", "x")),
            ThrottleScope::ProviderWide
        );
        assert_eq!(
            scopes.for_route(&route("a", "y")),
            ThrottleScope::ProviderWide
        );
        assert_eq!(
            scopes.for_route(&route("c", "z")),
            ThrottleScope::Unknown {
                sample_size: 0,
                required: MIN_CORRELATION_SAMPLE,
            },
            "c/z never throttled, so it is unmeasured rather than absent"
        );
        assert_eq!(
            scopes.iter().count(),
            2,
            "only the two throttled routes are stored"
        );
    }

    /// `row` with the account key line 1965's facets read —
    /// [`RoutingObservation::quota_context`], the credential label the
    /// gateway stamps on every exchange.
    fn account_row(
        provider: &str,
        model: &str,
        account: &str,
        start: i64,
        class: Option<FailureClass>,
    ) -> RoutingObservation {
        let mut observation = row(provider, model, start, start + 5, class);
        observation.quota_context = Some(account.to_owned());
        observation
    }

    /// Line 1317's account-specific scope, now that the key exists: five
    /// windows where account A's sibling models `x` and `y` throttled
    /// together while account B of the same provider kept serving. Without
    /// the account key this exact shape reads provider-wide (the sibling
    /// models overlapped) — the other account serving through it is what
    /// refutes that.
    #[test]
    fn sibling_throttles_beside_another_account_serving_read_as_account_specific() {
        let rows: Vec<RoutingObservation> = (0..5)
            .flat_map(|i| {
                let at = i * 1_000;
                [
                    account_row("a", "x", "a/KEY_A", at, Some(FailureClass::Throttle)),
                    account_row("a", "y", "a/KEY_A", at + 10, Some(FailureClass::Throttle)),
                    account_row("a", "x", "a/KEY_B", at + 20, None),
                ]
            })
            .collect();
        assert_eq!(
            classify_throttle_scope(&rows, &route("a", "x")),
            ThrottleScope::AccountSpecific,
            "account A's models throttled together while account B kept serving"
        );
    }

    /// The refuting evidence for account-specificity: the *other account*
    /// throttled in the same window too, so the limiter provably reached
    /// past one account and the verdict stays provider-wide.
    #[test]
    fn a_throttle_shared_by_two_accounts_stays_provider_wide() {
        let rows: Vec<RoutingObservation> = (0..5)
            .flat_map(|i| {
                let at = i * 1_000;
                [
                    account_row("a", "x", "a/KEY_A", at, Some(FailureClass::Throttle)),
                    account_row("a", "y", "a/KEY_A", at + 10, Some(FailureClass::Throttle)),
                    account_row("a", "x", "a/KEY_B", at + 20, Some(FailureClass::Throttle)),
                ]
            })
            .collect();
        assert_eq!(
            classify_throttle_scope(&rows, &route("a", "x")),
            ThrottleScope::ProviderWide,
            "two accounts throttled in one window is the limiter reaching past either"
        );
    }

    /// Rows with no account key classify exactly as they did before the key
    /// existed — the account axis is evidence-permitting, never inferred:
    /// the same five sibling-throttle windows with no `quota_context`
    /// anywhere still read provider-wide even when a context-less row was
    /// serving beside them.
    #[test]
    fn contextless_rows_never_produce_an_account_specific_verdict() {
        let rows: Vec<RoutingObservation> = (0..5)
            .flat_map(|i| {
                let at = i * 1_000;
                [
                    throttle("a", "x", at),
                    throttle("a", "y", at + 10),
                    served("a", "z", at + 20),
                ]
            })
            .collect();
        assert_eq!(
            classify_throttle_scope(&rows, &route("a", "x")),
            ThrottleScope::ProviderWide,
            "no row names an account, so nothing may claim an account boundary"
        );
    }
}

#[cfg(test)]
mod credential_throttle_tests {
    use super::*;

    fn row(
        provider: &str,
        account: Option<&str>,
        class: Option<FailureClass>,
    ) -> RoutingObservation {
        RoutingObservation {
            seq: 0,
            project_id: "project".to_owned(),
            observed_at_unix: 1_000,
            provider: provider.to_owned(),
            model: "m".to_owned(),
            route: Some("anthropic-messages".to_owned()),
            quota_context: account.map(str::to_owned),
            harness: Some("claude-code".to_owned()),
            purpose: None,
            dispatched_at_unix: Some(995),
            first_byte_at_unix: None,
            first_token_at_unix: None,
            first_tool_call_at_unix: None,
            completed_at_unix: Some(1_000),
            input_tokens: None,
            output_tokens: None,
            cached_input_tokens: None,
            cost: None,
            tool_rounds: None,
            retries: None,
            repairs: None,
            failovers: None,
            outcome: Some(if class.is_some() {
                Outcome::Failed
            } else {
                Outcome::Succeeded
            }),
            failure_class: class,
            context_state: ContextState::Unknown,
        }
    }

    const THROTTLE: Option<FailureClass> = Some(FailureClass::Throttle);

    /// Map line 1965's per-account narrowing: every throttle row of the
    /// provider names its account, so each credential is counted its own
    /// rows and no other's — and another provider's throttles are not this
    /// provider's however many there are.
    #[test]
    fn every_row_naming_its_account_narrows_the_count_to_the_credential() {
        let rows = vec![
            row("alpha", Some("alpha/KEY_A"), THROTTLE),
            row("alpha", Some("alpha/KEY_A"), THROTTLE),
            row("alpha", Some("alpha/KEY_B"), THROTTLE),
            row("beta", None, THROTTLE),
            row("beta", None, THROTTLE),
        ];
        let counted = recent_credential_throttles(&rows, "alpha", Some("alpha/KEY_A"));
        assert_eq!(
            counted,
            CredentialThrottles {
                throttled: 2,
                account_narrowed: true,
            },
            "KEY_A's own rows, not KEY_B's and not beta's"
        );
        let sibling = recent_credential_throttles(&rows, "alpha", Some("alpha/KEY_B"));
        assert_eq!(sibling.throttled, 1);
        assert!(sibling.account_narrowed);
    }

    /// One context-less throttle row makes the whole reading provider-wide:
    /// a throttle no row attributes to an account cannot be subtracted from
    /// one, so the honest count is the provider's total.
    #[test]
    fn a_contextless_throttle_row_widens_the_reading_to_provider_scope() {
        let rows = vec![
            row("alpha", Some("alpha/KEY_A"), THROTTLE),
            row("alpha", None, THROTTLE),
        ];
        let counted = recent_credential_throttles(&rows, "alpha", Some("alpha/KEY_A"));
        assert_eq!(
            counted,
            CredentialThrottles {
                throttled: 2,
                account_narrowed: false,
            }
        );
    }

    /// Zero rows are a provider-wide zero — "none observed" is a statement
    /// about the provider's rows, never a per-account claim — and rows that
    /// are not informative throttles (a served exchange, a correlation
    /// probe's own row, a row with no outcome) contribute nothing.
    #[test]
    fn only_informative_throttles_count_and_zero_is_provider_wide() {
        let mut probe = row("alpha", Some("alpha/KEY_A"), THROTTLE);
        probe.purpose = Some(CORRELATION_PURPOSE.to_owned());
        let mut outcomeless = row("alpha", Some("alpha/KEY_A"), THROTTLE);
        outcomeless.outcome = None;
        let rows = vec![row("alpha", Some("alpha/KEY_A"), None), probe, outcomeless];
        let counted = recent_credential_throttles(&rows, "alpha", Some("alpha/KEY_A"));
        assert_eq!(
            counted,
            CredentialThrottles {
                throttled: 0,
                account_narrowed: false,
            }
        );
    }
}

/// Map line 1971's spend reader — [`recent_credential_spend`].
#[cfg(test)]
mod credential_spend_tests {
    use super::*;

    fn row(
        provider: &str,
        account: Option<&str>,
        tokens: Option<(i64, i64)>,
    ) -> RoutingObservation {
        RoutingObservation {
            seq: 0,
            project_id: "project".to_owned(),
            observed_at_unix: 1_000,
            provider: provider.to_owned(),
            model: "m".to_owned(),
            route: Some("anthropic-messages".to_owned()),
            quota_context: account.map(str::to_owned),
            harness: Some("claude-code".to_owned()),
            purpose: None,
            dispatched_at_unix: Some(995),
            first_byte_at_unix: None,
            first_token_at_unix: None,
            first_tool_call_at_unix: None,
            completed_at_unix: Some(1_000),
            input_tokens: tokens.map(|(input, _)| input),
            output_tokens: tokens.map(|(_, output)| output),
            cached_input_tokens: Some(9_999),
            cost: None,
            tool_rounds: None,
            retries: None,
            repairs: None,
            failovers: None,
            outcome: Some(Outcome::Succeeded),
            failure_class: None,
            context_state: ContextState::Unknown,
        }
    }

    /// Every counted row names its account, so the sum is this account's own
    /// — and it is input plus output and **not** the cached-input column,
    /// which providers disagree about.
    #[test]
    fn every_row_naming_its_account_narrows_the_sum_to_the_credential() {
        let rows = vec![
            row("alpha", Some("alpha/KEY_A"), Some((100, 20))),
            row("alpha", Some("alpha/KEY_A"), Some((5, 5))),
            row("alpha", Some("alpha/KEY_B"), Some((900, 900))),
            row("beta", Some("beta/KEY_A"), Some((1_000, 1_000))),
        ];
        assert_eq!(
            recent_credential_spend(&rows, "alpha", Some("alpha/KEY_A")),
            CredentialSpend {
                tokens: Some(130),
                account_narrowed: true,
                sample_count: 2,
            },
            "KEY_A's own rows on this provider, input plus output, and nothing else"
        );
    }

    /// One contextless counted row means the ledger holds spend nobody can
    /// attribute. The reading widens to provider scope rather than quietly
    /// dropping it: under-reporting is the direction that would let a
    /// ceiling be exceeded.
    #[test]
    fn a_contextless_counted_row_widens_the_reading_to_provider_scope() {
        let rows = vec![
            row("alpha", Some("alpha/KEY_A"), Some((100, 20))),
            row("alpha", None, Some((7, 3))),
        ];
        assert_eq!(
            recent_credential_spend(&rows, "alpha", Some("alpha/KEY_A")),
            CredentialSpend {
                tokens: Some(130),
                account_narrowed: false,
                sample_count: 2,
            }
        );
    }

    /// A row that carried no token count at all is not a zero. With no
    /// counted row anywhere the reading is `None` — unknown — which is what
    /// keeps a stated ceiling from being judged reached by a build that
    /// measured nothing.
    #[test]
    fn no_counted_row_reads_unknown_and_never_zero() {
        let rows = vec![
            row("alpha", Some("alpha/KEY_A"), None),
            row("alpha", Some("alpha/KEY_A"), None),
        ];
        assert_eq!(
            recent_credential_spend(&rows, "alpha", Some("alpha/KEY_A")),
            CredentialSpend {
                tokens: None,
                account_narrowed: false,
                sample_count: 0,
            }
        );

        // And an account with no rows of its own, beside a sibling that has
        // them, reads unknown rather than zero for the same reason.
        let rows = vec![row("alpha", Some("alpha/KEY_B"), Some((10, 10)))];
        assert_eq!(
            recent_credential_spend(&rows, "alpha", Some("alpha/KEY_A")).tokens,
            None
        );
    }

    /// This ledger's own bookkeeping is not spend, and neither is an
    /// exchange that never completed.
    #[test]
    fn correlation_rows_and_unfinished_exchanges_are_not_spend() {
        let mut correlation = row("alpha", Some("alpha/KEY_A"), Some((100, 100)));
        correlation.purpose = Some(CORRELATION_PURPOSE.to_owned());
        let mut unfinished = row("alpha", Some("alpha/KEY_A"), Some((100, 100)));
        unfinished.outcome = None;
        let rows = vec![
            correlation,
            unfinished,
            row("alpha", Some("alpha/KEY_A"), Some((1, 2))),
        ];
        assert_eq!(
            recent_credential_spend(&rows, "alpha", Some("alpha/KEY_A")).tokens,
            Some(3)
        );
    }
}
