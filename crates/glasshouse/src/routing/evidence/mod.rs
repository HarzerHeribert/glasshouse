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
//! Two producers write these rows, each honest about what it can see:
//! [`crate::gateway::session::SessionRouting`] relays a harness's own request
//! and so leaves most timing, token and outcome columns `NULL` unless a
//! translated exchange's own seam already decoded them; `crate::memory::extract`
//! builds and decodes its own request and so can fill the token columns the
//! gateway cannot. A column this build cannot honestly read stays `NULL` —
//! "the build that wrote this row recorded nothing here" — never a guessed or
//! interpolated value.
// History: design-decisions.md, "Trims: routing/evidence/mod.rs", module doc.

use std::sync::Mutex;

use rusqlite::{Connection, Row};

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

/// A turn Glasshouse relayed on a harness's behalf, as opposed to the
/// calls Glasshouse makes for its own bookkeeping. See the other
/// `*_PURPOSE` constants in this module: each names why *Glasshouse*
/// called a provider; this one names the case where it did not.
pub const HARNESS_TURN_PURPOSE: &str = "harness-turn";

/// A client-relayed look a supervisor made on its own behalf, not the task
/// it is watching — pane's `supervisor.md` §3.
pub const SUPERVISOR_PURPOSE: &str = "supervisor";

/// A client may name a purpose only from this list; everything else is
/// [`HARNESS_TURN_PURPOSE`]. The gateway strips the `x-glasshouse-purpose`
/// request header before forwarding regardless of whether its value
/// appears here, so an unrecognised name never reaches a provider and
/// never reaches the ledger either.
pub const CLIENT_NAMEABLE_PURPOSES: &[&str] = &[SUPERVISOR_PURPOSE];

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

/// The four-word effort ladder a translated exchange's row records —
/// migration 24's `routing_observations.effort_level`. The *stored*
/// vocabulary: this module may not depend on `crate::gateway`, so it is
/// declared here rather than reusing `crate::gateway::translate::canonical`'s
/// *wire* `EffortLevel`, and pinned against it exhaustively by that module's
/// own `every_wire_effort_level_stores_and_reads_back_as_the_same_word` test.
///
/// [`Self::from_stored`] answers [`None`] for a word this build does not
/// know, kept as `None` rather than an error — migration 24's own doc
/// comment has the reason.
// History: design-decisions.md, "Trims: routing/evidence/mod.rs", `EffortLevel` doc.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum EffortLevel {
    Minimal,
    Low,
    Medium,
    High,
}

impl EffortLevel {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Minimal => "minimal",
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
        }
    }

    pub fn from_stored(value: &str) -> Option<Self> {
        match value {
            "minimal" => Some(Self::Minimal),
            "low" => Some(Self::Low),
            "medium" => Some(Self::Medium),
            "high" => Some(Self::High),
            _ => None,
        }
    }
}

/// What shape the turn a translated exchange carried was — migration 24's
/// `routing_observations.turn_shape`.
///
/// Two words, and unlike [`EffortLevel`] there is no second vocabulary
/// anywhere for this to drift from: no wire spells a turn shape, and
/// [`crate::gateway::translate::canonical::Request::turn_shape`] derives it
/// from the decoded request alone. So it is declared once, here, where the
/// column it is stored in lives.
///
/// [`Self::from_stored`] answers [`None`] for an unrecognised word, on
/// [`EffortLevel`]'s reasoning.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TurnShape {
    /// The last user message carried nothing but tool results: the harness
    /// is handing back what a tool returned, not writing a new prompt.
    ToolResume,
    /// Everything else, a turn with no user message at all included.
    Prompt,
}

impl TurnShape {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ToolResume => "tool-resume",
            Self::Prompt => "prompt",
        }
    }

    pub fn from_stored(value: &str) -> Option<Self> {
        match value {
            "tool-resume" => Some(Self::ToolResume),
            "prompt" => Some(Self::Prompt),
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
/// No rule deciding a value reads a byte of the body — see
/// `crate::gateway::session`'s `failure_class`, beside `classify`, and
/// `docs/product/design-decisions.md`'s *"Phase 33: framing is not content"*.
// History: design-decisions.md, "Trims: routing/evidence/mod.rs", `FailureClass` doc.
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
// History: design-decisions.md, "Trims: routing/evidence/mod.rs", `FailureClassCounts` doc.
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

    /// Milliseconds from the instant the upstream request was sent to the
    /// first response byte — `crate::database` migration 25, and never an
    /// absolute instant. See [`Self::with_first_byte_ms`].
    pub first_byte_ms: Option<i64>,
    /// [`Self::first_byte_ms`]'s sibling for the first real generated token.
    pub first_token_ms: Option<i64>,
    /// [`Self::first_byte_ms`]'s sibling for the first tool-use block start.
    pub first_tool_call_ms: Option<i64>,
    /// [`Self::first_byte_ms`]'s sibling for the end of the exchange.
    pub completed_ms: Option<i64>,

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
    /// Which class of work this request was, when the producer classified one
    /// — see [`super::request::TaskClass`] and `crate::database` migration
    /// 23.
    pub task_class: Option<super::request::TaskClass>,
    /// The Glasshouse session this exchange belonged to, when the producer
    /// was told one — migration 24. See [`Self::with_session_id`].
    pub session_id: Option<String>,
    /// The effort the request carried, on a translated exchange — migration
    /// 24. See [`Self::with_effort_level`].
    pub effort_level: Option<EffortLevel>,
    /// The shape of the turn the request carried, on a translated exchange
    /// — migration 24. See [`Self::with_turn_shape`].
    pub turn_shape: Option<TurnShape>,

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
            first_byte_ms: None,
            first_token_ms: None,
            first_tool_call_ms: None,
            completed_ms: None,
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
            task_class: None,
            session_id: None,
            effort_level: None,
            turn_shape: None,
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

    /// Line 1331's other two timing columns [`Self::with_timing`] does not
    /// carry — the instant the first real generated token passed the seam.
    /// Supplied only by a **translated** exchange, whose seam already decodes
    /// every canonical event in order to re-encode it for the harness; a
    /// relayed exchange never enters a codec and passes [`None`], exactly
    /// like [`Self::with_first_byte_at`]'s own relayed case. See
    /// `docs/product/design-decisions.md`'s *"first real token and first
    /// tool call on the translated path — the 1331/1332 ruling"*.
    pub fn with_first_token_at(mut self, first_token_at_unix: Option<i64>) -> Self {
        self.first_token_at_unix = first_token_at_unix;
        self
    }

    /// [`Self::with_first_token_at`]'s sibling: the instant the first
    /// tool-use block started, under the same rule and the same `None` case.
    pub fn with_first_tool_call_at(mut self, first_tool_call_at_unix: Option<i64>) -> Self {
        self.first_tool_call_at_unix = first_tool_call_at_unix;
        self
    }

    /// Migration 25's first offset: milliseconds from the instant the
    /// upstream request was **sent** to the instant the provider's status
    /// and headers were in hand.
    ///
    /// Not a duration derived from the columns above. Those are unix
    /// seconds, and their zero — `dispatched_at` — is the instant the
    /// gateway handed a connection to `ingress::serve`, which is earlier
    /// than the send by however long reading and rebuilding the request
    /// took. This offset's zero is the send itself, and it is read from a
    /// monotonic `std::time::Instant` rather than from two wall-clock
    /// readings subtracted, so a clock step cannot make it negative. The
    /// column's own `CHECK` refuses a negative value if one ever arrives
    /// anyway. See `docs/product/design-decisions.md`'s *"Millisecond
    /// offsets on the routing row — Cluster G's second column set"*.
    ///
    /// A separate builder rather than a parameter on [`Self::with_timing`],
    /// for exactly [`Self::with_first_byte_at`]'s reason: only the producer
    /// that holds the dispatch `Instant` can supply it, and every other
    /// producer's existing call stays untouched.
    pub fn with_first_byte_ms(mut self, first_byte_ms: Option<i64>) -> Self {
        self.first_byte_ms = first_byte_ms;
        self
    }

    /// [`Self::with_first_byte_ms`]'s sibling for the first real generated
    /// token — supplied only by a **translated** exchange, whose seam
    /// decodes the canonical events, exactly like
    /// [`Self::with_first_token_at`]'s own relayed `None`.
    pub fn with_first_token_ms(mut self, first_token_ms: Option<i64>) -> Self {
        self.first_token_ms = first_token_ms;
        self
    }

    /// [`Self::with_first_token_ms`]'s sibling for the first tool-use block
    /// start.
    pub fn with_first_tool_call_ms(mut self, first_tool_call_ms: Option<i64>) -> Self {
        self.first_tool_call_ms = first_tool_call_ms;
        self
    }

    /// [`Self::with_first_byte_ms`]'s sibling for the end of the exchange —
    /// supplied on both the relayed and the translated path, since both know
    /// when they stopped moving bytes. [`RoutingObservation::duration_ms`]
    /// prefers it over the seconds difference precisely because this one was
    /// measured rather than subtracted.
    pub fn with_completed_ms(mut self, completed_ms: Option<i64>) -> Self {
        self.completed_ms = completed_ms;
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

    /// Which class of work this request was — capability map line 1276, and
    /// the missing link between `super::request::RouterAnswer::task_class`
    /// (which has existed since Phase 34C) and any reader of history.
    ///
    /// `None` is "this producer did not classify", the same honest absence
    /// every other nullable column carries, and it is what every gateway row
    /// carries: the gateway relays a turn and never runs the classifier.
    /// [`super::burn::task_class_request_rates`] counts only rows that name
    /// a class, so an absent one lowers no average rather than joining a
    /// bucket it did not earn.
    pub fn with_task_class(mut self, task_class: Option<super::request::TaskClass>) -> Self {
        self.task_class = task_class;
        self
    }

    /// Which Glasshouse session this exchange served — capability map line
    /// 2019's *per-session* clause and migration 24's first column.
    ///
    /// The value is `crate::session::SessionId`'s own string and nothing
    /// else: never the harness's `metadata.user_id`, never a native session
    /// id, never a credential. `docs/product/design-decisions.md`'s *A
    /// session identity on the routing evidence rows* argues each of those
    /// three exclusions.
    ///
    /// `None` is *this producer was never told which session it serves* —
    /// the same honest absence every other nullable column on this type
    /// carries — and it is what a gateway nothing has called
    /// [`crate::gateway::session::SessionRouting::serve_session`] on writes,
    /// never an invented id. `main.rs::record_routing_latency`'s row keeps
    /// it too, deliberately: that row is about a routing decision taken
    /// before any session record existed.
    pub fn with_session_id(mut self, session_id: Option<impl Into<String>>) -> Self {
        self.session_id = session_id.map(Into::into);
        self
    }

    /// The effort the harness asked for on this exchange — migration 24's
    /// second column, and half of what capability map line 2039's shadow
    /// measurement joins.
    ///
    /// A fact of the *request*, read at the one seam that holds a decoded
    /// one (`crate::gateway::translate::serve`). `None` on every relayed
    /// exchange, whose body this gateway never reads, and on a translated
    /// request that asked for no thinking at all — the same absence to this
    /// column, with the row's own `route` telling the two apart.
    pub fn with_effort_level(mut self, effort_level: Option<EffortLevel>) -> Self {
        self.effort_level = effort_level;
        self
    }

    /// Whether this exchange's turn handed back tool results or wrote a new
    /// prompt — migration 24's third column, and the other half of what line
    /// 2039's shadow measurement selects on.
    ///
    /// [`Self::with_effort_level`]'s rule for `None`, for the same reason: a
    /// relayed exchange has no decoded request to derive a shape from.
    pub fn with_turn_shape(mut self, turn_shape: Option<TurnShape>) -> Self {
        self.turn_shape = turn_shape;
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

    /// Line 1334's last two quantities. How many tool-use blocks this
    /// exchange's response requested — the rounds this exchange *began* —
    /// supplied only by a **translated** exchange, whose seam already
    /// decodes the response to re-encode it. `None` is "this producer never
    /// decoded a response to count from," as for every other nullable
    /// column; `Some(0)` is "it counted and found none," which is not the
    /// same fact and must not be confused with it. See
    /// `docs/product/design-decisions.md`'s *"Tool rounds and repairs on the
    /// translated path"*.
    pub fn with_tool_rounds(mut self, tool_rounds: Option<u32>) -> Self {
        self.tool_rounds = tool_rounds.map(i64::from);
        self
    }

    /// [`Self::with_tool_rounds`]'s sibling: how many `is_error: true`
    /// tool-result blocks this exchange's request carried — the harness's
    /// own report that a previous round failed. Supplied only by a
    /// **translated** exchange whose request decoded, under the same
    /// `None`-vs-`Some(0)` rule.
    pub fn with_repairs(mut self, repairs: Option<u32>) -> Self {
        self.repairs = repairs.map(i64::from);
        self
    }

    pub fn with_context_state(mut self, context_state: ContextState) -> Self {
        self.context_state = context_state;
        self
    }

    /// Map line 1307: the estimated cost the routing decision this
    /// observation records actually used, carried in from
    /// `crate::routing::session::Routed::cost` rather than recomputed here.
    /// `None` — the default — means unknown size or unknown price, and this
    /// row then leaves `cost_micro_usd` `NULL` exactly like every other
    /// producer's absent reading, never a fabricated zero.
    pub fn with_cost(mut self, cost: Option<ObservedCost>) -> Self {
        self.cost = cost;
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

    /// Milliseconds from the send to the first response byte — migration 25,
    /// and `None` for every row written before it as well as for every
    /// exchange whose request never left. See
    /// [`NewObservation::with_first_byte_ms`] for why this is an offset from
    /// the send rather than from `dispatched_at`.
    pub first_byte_ms: Option<i64>,
    /// [`Self::first_byte_ms`]'s sibling for the first real generated token
    /// — additionally `None` on every relayed exchange.
    pub first_token_ms: Option<i64>,
    /// [`Self::first_token_ms`]'s sibling for the first tool-use block start.
    pub first_tool_call_ms: Option<i64>,
    /// [`Self::first_byte_ms`]'s sibling for the end of the exchange — the
    /// figure [`Self::duration_ms`] prefers over the seconds difference.
    pub completed_ms: Option<i64>,

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
    /// `None` for a row whose producer ran no classifier, for every row
    /// written before migration 23, **and** for a row whose stored word this
    /// build does not recognise — see migration 23's own doc comment for why
    /// the third case is not an error the way an unknown `failure_class` is.
    pub task_class: Option<super::request::TaskClass>,
    /// The Glasshouse session this exchange served, `None` for a row whose
    /// producer was never told one and for every row written before
    /// migration 24 — see [`NewObservation::with_session_id`].
    pub session_id: Option<String>,
    /// `None` for a relayed exchange, for a translated request that asked
    /// for no thinking, for every row written before migration 24, **and**
    /// for a row whose stored word this build does not recognise — see
    /// [`EffortLevel`].
    pub effort_level: Option<EffortLevel>,
    /// [`Self::effort_level`]'s four cases, for [`TurnShape`]'s two words.
    pub turn_shape: Option<TurnShape>,

    pub context_state: ContextState,
}

impl RoutingObservation {
    /// How long this exchange took, in milliseconds — [`Self::completed_ms`]
    /// when the producer measured it, and the second-resolution difference
    /// `completed_at - dispatched_at` otherwise.
    ///
    /// The preference is the point, and it is silent: every consumer of this
    /// method — [`EvidenceLedger::classification_record`],
    /// [`EvidenceLedger::support_work_latency`] and the medians they compute
    /// — improves from a figure that was zero or one second to one that was
    /// actually measured, without any of them changing. A row written before
    /// migration 25, or by a producer holding no dispatch `Instant`, keeps
    /// the fallback and reads exactly as it always did.
    ///
    /// The two are not the same span, and the difference is smaller than the
    /// resolution the fallback has: `completed_ms` is measured from the
    /// instant the upstream request was **sent**, and the fallback from the
    /// instant the connection was handed to the gateway's ingress. See
    /// [`NewObservation::with_first_byte_ms`].
    pub fn duration_ms(&self) -> Option<i64> {
        if let Some(completed_ms) = self.completed_ms {
            return Some(completed_ms);
        }
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

fn median(mut values: Vec<i64>) -> i64 {
    values.sort_unstable();
    values[values.len() / 2]
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

/// `pub(crate)`: [`crate::evaluation`]'s map-line-1845 join reads
/// `routing_observations` directly (the same database file, a second
/// connection — see that module's own doc comment) and reuses this row
/// decoder rather than re-deriving [`RoutingObservation`]'s parsing.
pub(crate) fn row_to_observation(
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

    // Migration 23, and deliberately not `failure_class`'s shape above: an
    // unrecognised word is `None`, not an `UnknownValue`. See the migration's
    // own doc comment -- a class is a bucketing input to an average, and a
    // future build's sixth class must not break an older build's burn rate.
    let task_class_text: Option<String> = row.get("task_class")?;
    let task_class = task_class_text
        .as_deref()
        .and_then(super::request::TaskClass::from_stored);

    // Migration 24, and `task_class`'s arm above rather than
    // `failure_class`'s, for the reason that migration's own doc comment
    // gives: both stored words are bucketing inputs to a ratio, so a word a
    // future build invents must lower no reader here rather than failing the
    // whole row for an older build. `session_id` needs no arm of its own —
    // it is an opaque identifier with no vocabulary to fail against.
    let effort_level_text: Option<String> = row.get("effort_level")?;
    let effort_level = effort_level_text
        .as_deref()
        .and_then(EffortLevel::from_stored);

    let turn_shape_text: Option<String> = row.get("turn_shape")?;
    let turn_shape = turn_shape_text.as_deref().and_then(TurnShape::from_stored);

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
        // Migration 25. No vocabulary to fail against and no arm of their
        // own: an integer column reads back as the integer it holds, and
        // `NULL` is the *this producer did not measure* every other optional
        // column on this row already means.
        first_byte_ms: row.get("first_byte_ms")?,
        first_token_ms: row.get("first_token_ms")?,
        first_tool_call_ms: row.get("first_tool_call_ms")?,
        completed_ms: row.get("completed_ms")?,
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
        task_class,
        session_id: row.get("session_id")?,
        effort_level,
        turn_shape,
        context_state,
    }))
}

impl EvidenceLedger {
    fn lock(&self) -> std::sync::MutexGuard<'_, Connection> {
        self.conn
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

mod joins;
mod ledger;
mod readers;
mod signals;
#[cfg(test)]
mod tests;

pub use joins::{
    EffortShadow, EffortShadowRow, HeadroomBand, HeadroomBasis, HeadroomReplayCounts,
    LONG_SIGNAL_HORIZON_SECONDS, LongWindowPressure, MIN_LEARNED_RESET_RECOVERIES,
    OutputEstimateAccuracy, RECENT_SIGNAL_HORIZON_SECONDS, ResetBasis, RouteResponsiveness,
    SeparationMeasure, SeparationReport, SubscriptionHeadroomEstimate,
    estimate_subscription_headroom,
};
pub use readers::{
    ClassificationRecord, HarnessRequestStats, LatencyRecord, ObservationQuery,
    ObservedEvidenceSource, ObservedIdentity, PurposeConsumption, RoutingOverhead, RoutingSummary,
    SessionTranslationSavings, TranslationSavings, WallClockSummary,
};
pub use signals::{
    CorrelationVerdict, CredentialCost, CredentialSpend, CredentialThrottles, RouteCorrelation,
    RouteCorrelations, RouteIdentity, ThrottleScope, ThrottleScopes, classify_throttle_scope,
    classify_throttle_scopes, correlate_routes, estimated_context_tokens, recent_credential_cost,
    recent_credential_spend, recent_credential_throttles,
};
