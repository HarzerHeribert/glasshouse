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
//! - **`first_byte_at`, `first_token_at`, `first_tool_call_at`: not
//!   supplied, at all, by this producer.** Not merely unavailable to this
//!   round's partition — structurally unavailable to the ingress design
//!   itself. `crate::gateway::ingress`'s own module documentation is explicit
//!   that `crate::gateway::ingress::Exchange` (private to that module) "holds an outcome, two
//!   statuses, a byte count and two names" and is "structurally incapable of
//!   carrying a body," because a pass-through gateway that parsed response
//!   bytes to find the first real token would be a parser of the payload it
//!   exists to be unable to read. Line 1332's warning against treating
//!   "whitespace padding, transport keepalives, or reasoning-only deltas" as
//!   the first generated token is consequently moot for this producer: it
//!   never attempts to find one, so it cannot get it wrong, and it leaves the
//!   column `NULL` rather than fabricate a value. **These boxes stay open.**
//!   A component that reads the response stream's own framing (the harness
//!   adapter, or a body-aware layer this project has not built) is what would
//!   have to supply them.
//! - **`tool_rounds`, `retries`, `repairs`, `failovers`: not supplied.** The
//!   gateway serves one HTTP request per connection
//!   (`crate::gateway::ingress::serve`'s own "why one request per
//!   connection") and has no notion of a *turn* spanning several of them; a
//!   harness may make several exchanges for what a user experiences as one
//!   turn, and only something above the gateway — the harness, or the
//!   session it belongs to — can count rounds across that boundary.
//! - **`input_tokens`, `output_tokens`, `cached_input_tokens`,
//!   `cost_micro_usd`: not supplied.** Same reason as the timing columns
//!   above: reading them means parsing a response body this module is
//!   forbidden to parse.
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

    pub fn with_timing(
        mut self,
        dispatched_at_unix: Option<i64>,
        completed_at_unix: Option<i64>,
    ) -> Self {
        self.dispatched_at_unix = dispatched_at_unix;
        self.completed_at_unix = completed_at_unix;
        self
    }

    pub fn with_outcome(mut self, outcome: Outcome) -> Self {
        self.outcome = Some(outcome);
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
                context_state
            ) VALUES (
                ?1, ?2,
                ?3, ?4, ?5, ?6, ?7, ?8,
                ?9, ?10, ?11, ?12, ?13,
                ?14, ?15, ?16,
                ?17, ?18,
                ?19, ?20, ?21, ?22, ?23,
                ?24
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
        context_state,
    }))
}

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
        let reliable_observation_count = summary
            .failure_rate
            .as_ref()
            .map(AggregateReading::sample_count)
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
}
