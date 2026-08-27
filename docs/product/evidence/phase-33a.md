# Capability evidence — phase 33A

### Phase 33A — the routing evidence ledger, and the honest shape of what the gateway can see

**The finding first, because it decides most of the fourteen lines.** The
packet's hypothesis — *"`gateway/session.rs` sees enough of a turn to record
a real observation"* — is **partly true and partly false**, and the false
half is structural, not a gap this round ran out of time for.

`crate::gateway::ingress` is deliberately incapable of reading a response
body: its own module documentation says the `Exchange` value that reaches
`tracing` "holds an outcome, two statuses, a byte count and two names" and is
"structurally incapable of carrying a body," because a proxy that parsed
response bytes to find the first real token would be a parser of the payload
it exists to be unable to read. That single design fact answers five of the
fifteen lines by itself:

- **`first_byte_at`, `first_token_at`, `first_tool_call_at` (line 1331):
  never supplied.** Not merely outside this round's partition — `ingress.rs`
  is forbidden to this package, but even inside it, finding "the first real
  token" needs body parsing the module's own design forbids. **These stay
  open.**
- **Line 1332 (do not mistake a keepalive for the first token) is therefore
  moot for this producer**: it never attempts to find a first token, so it
  cannot get the distinction wrong. Recorded as open rather than vacuously
  ticked.
- **`input_tokens`, `output_tokens`, `cached_input_tokens`, `cost_micro_usd`
  (line 1333): never supplied**, same reason.
- **`tool_rounds`, `retries`, `repairs`, `failovers` (line 1334, half of
  it): never supplied.** The gateway serves one HTTP request per connection
  and has no notion of a *turn* spanning several of them.
- **`outcome` (line 1334, the other half): a transport-level proxy, not the
  user-visible verdict the line asks for.** A `200` whose body describes a
  model error is indistinguishable from a real success to this producer,
  because the body is exactly what it cannot read.

What the gateway genuinely can supply, once a launch profile has bound an
assignment (`SessionRouting::bind`): `provider`, `model`, `harness`, `route`
(the wire-protocol slug), `quota_context` (the credential's own safe label),
an accurate `completed_at`, and an approximate `dispatched_at` — the instant
the accept loop handed the connection to `ingress::serve`, not the true
dispatch instant inside `ingress::forward`, which is outside this round's
partition. `context_state` is always `unknown` from this producer; the
gateway has no cache-state signal of its own.

**Whether an observation is written by production code.** Yes, inside the
gateway module's own accept loop — proven with a real socket and mutation-
killed (see below) — but **not yet reached from the shipped binary**, for
the same reason `crate::provider::telemetry::GatewayQuotaCache` was not
reached until `QUOTA-LIVE`: `crates/glasshouse/src/main.rs` is this
package's `FORBIDDEN FILES`, and it is the only caller of
`crate::gateway::start_if_required_with_quota_cache` today. This package
adds an additive sibling, `start_if_required_with_telemetry`, that `main.rs`
does not call yet. See `PATCHES ANOTHER PACKAGE MUST APPLY` in the report
for the three-line change that wires a real
`crate::routing::evidence::EvidenceLedger::open` into both of `main.rs`'s
gateway launch sites.

Contract: Given a project database at schema version 11, when a real
gateway exchange reaches a provider on a session a launch profile has
bound, Glasshouse appends one `routing_observations` row carrying that
exchange's honest, transport-level subset of identity, timing and outcome,
while preserving every prior row unedited and computing every rolling
summary on read rather than replacing the raw rows that produced it.

State: **COMPLETE** for map lines 1329, 1330, 1337, 1338, 1342 and 1343 — six
of fifteen. **NOT STARTED, blocked** for the other nine.

> **Orchestrator's ruling, and the line it draws.** A box closes here when the
> **recording** path runs in the shipped binary, which it now does: `main.rs`
> hands a real `EvidenceLedger` to every gateway it starts, and
> `gateway/mod.rs`'s accept loop appends a row per forwarded exchange —
> mutation-proven against a real socket by the package, and re-proven by the
> integrator.
>
> **The nine that stay open split into two kinds, and the difference matters
> for whoever picks this up.** Four (1331, 1332, 1333, 1334) are blocked
> because the gateway is *structurally* unable to read a response body — no
> amount of work inside `gateway/**` reaches them, and the component that
> could is one that reads the response stream's own framing. Five (1335, 1336,
> 1339, 1340, 1341) are the aggregate half: `summarize` is real, tested and
> called **only from tests**. That is `evaluate_reserve_spend`'s exact
> position in Phase 32F, refused there in this same batch, and refused here
> for the same reason.
>
> **Integrator's finding on its own wiring, recorded because it was mine.** The
> first version passed `EvidenceLedger::open(runtime)?` — evaluated on every
> launch whether or not a gateway is required, with `?` turning a telemetry
> failure into a failed session. A read-only data directory would have meant
> "glasshouse will not start". Now `evidence_ledger(runtime)` warns and
> returns `None`; telemetry is the one subsystem here whose failure must
> always be survivable.
>
> **And the wiring was invisible to the tests.** Removing the ledger from both
> call sites left the entire suite green, so
> `main.rs::tests::every_gateway_the_binary_starts_is_given_the_evidence_ledger`
> now scans the production source for it — `main.rs`'s own existing
> `production_code` idiom, the same one
> `a_single_attempt_loses_the_race_that_the_bound_wins` uses. It fails on
> exactly the edit that previously went unnoticed. **It proves structure, not
> behaviour, and no box above closes on it** — what it prevents is a future
> edit silently dropping the ledger back to `None`.

Original package's assessment:

State: **PARTIALLY VERIFIED.** The ledger itself — schema, storage,
rolling summaries, decay, context-state separation, project isolation — is
built, tested, and reached from real production code inside the gateway
module. The two things not yet true in the shipped binary are (a) `main.rs`
constructing and passing a real `EvidenceLedger` into gateway startup, and
(b) any consumer downstream of `crate::config::pairing::ObservationSource`
actually asking this ledger a routing question — `routing-score`'s own
package, running concurrently this round, is what would supply that
consumer.

Production evidence:
- `crates/glasshouse/src/database.rs` — migration 11: `routing_observations`,
  its index, and its two project-isolation triggers (migration 4's own pair,
  copied verbatim).
- `crates/glasshouse/src/routing/evidence.rs` — `EvidenceLedger::{open,
  record, recent, summarize}`, `NewObservation`, `RoutingObservation`,
  `AggregateReading<T>`, `RoutingSummary`, `ObservedEvidenceSource` (this
  package's `ObservationSource` implementation).
- `crates/glasshouse/src/gateway/session.rs` —
  `SessionRouting::record_routing_observation`, the one place identity
  (provider/model/harness/quota_context) and the honest outcome proxy are
  assembled from a real, bound `Assignment` and a real `Exchange`.
- `crates/glasshouse/src/gateway/mod.rs` — the accept loop's connection
  thread: `dispatched_at`/`completed_at` stamped around `ingress::serve`,
  and the call to `record_routing_observation`, additive exactly like
  `GatewayQuotaCache`'s own wiring (`Gateway::start_with_telemetry`,
  `start_if_required_with_telemetry`). `Gateway::start` and
  `start_with_quota_cache` are unaffected — every existing caller, including
  every other conformance test, behaves exactly as before this package.

Regression evidence:
- `routing::evidence::tests::*` (11) — round-trip of every field a producer
  can supply; append-only (two records never collapse into one edited row);
  project isolation; a summary below `MIN_SAMPLE_FOR_SUMMARY` is `None`, at
  it is a real number with the right sample count and confidence; decay
  excludes an out-of-window row from a summary while `recent` still reads it
  raw; warm and cold observations never share one summary; a cost always
  carries its confidence; token volume and cost never move the failure-rate
  or duration aggregates; `ObservedEvidenceSource` answers from the same
  data `summarize` does, and answers `None` for a first-party route this
  ledger's producer never records.
- `database::tests::{a_version_ten_database_migrates_forward_keeping_its_memories,
  migration_eleven_rejects_a_routing_observation_from_a_foreign_project,
  migration_eleven_refuses_a_cost_with_no_confidence_label}` — the three
  migration proofs the packet named, plus the pre-existing
  `a_version_nine_database_migrates_forward_keeping_its_memories` updated to
  drop `routing_observations` on its own rollback (it now creates that table
  too, and previously did not account for it).
- `gateway::conformance::a_real_forwarded_exchange_reaches_the_routing_evidence_ledger`
  — a real `Gateway`, a real accept loop, a real socket: an unbound exchange
  is not recorded, a bound one is, with a real `dispatched_at`/`completed_at`
  and the credential's safe label as `quota_context`.
- `crates/glasshouse/tests/routing_evidence.rs` (4, external) — the same
  properties driven from outside the crate, through `glasshouse::` only,
  catching anything left `pub(crate)` that a caller needs public: an
  observation survives the process that recorded it; two projects never
  share one; a summary's sample count and window match exactly what was
  recorded; `ObservedEvidenceSource` is reachable and correct from outside
  the crate.

Failure/isolation evidence:
- `migration_eleven_rejects_a_routing_observation_from_a_foreign_project` —
  migration 4's isolation trigger, applied to the new table, really aborts.
- `a_ledger_never_sees_another_projects_observations` and
  `two_projects_never_share_a_routing_observation` — the same property one
  layer up, through `EvidenceLedger` rather than raw SQL.
- `migration_eleven_refuses_a_cost_with_no_confidence_label` — the `CHECK`
  pairing `cost_micro_usd` with `cost_confidence`; the paired case is also
  asserted to succeed, so the failure is about the missing label and not
  about the column existing at all.
- `no_aggregate_changes_when_only_token_volume_or_cost_changes` — line
  1342's own negative: two batches differing only in token volume and cost
  produce byte-identical failure-rate and duration aggregates.

Mutation evidence (practice §41, §35 for the call rather than the callee),
each `ok` before, `FAILED` mutated, `ok` after restore, in a private
`CARGO_TARGET_DIR` with every source `touch`ed before each build (§16):

- **The §35 proof.** `gateway/mod.rs`'s
  `routing.record_routing_observation(ledger, &exchange, dispatched_at,
  completed_at)` call in the accept loop's connection thread, disabled
  (`if false && let Some(ledger) = ...`) → `FAILED` at
  `a_real_forwarded_exchange_reaches_the_routing_evidence_ledger`, which
  timed out waiting for a row that never arrived. Restored, `ok`. This is
  the proof the packet asked for by name: delete the gateway's `record`
  call and confirm a named test goes red.

Platform/external evidence:
- `cargo doc -p glasshouse --no-deps` (practice §60 addendum): clean.
  Caught six broken intra-doc links on the first pass — every one a doc
  comment naming a private item (`gateway::ingress::Exchange`, private
  `memory::store` paths not re-exported the way `crate::memory::` re-exports
  them) — fixed before the gate, not discovered by it.
- `cargo check -p glasshouse --lib --tests`: clean.
- `cargo clippy -p glasshouse --all-targets --all-features -- -D warnings`
  (macOS, this worktree, local clippy 0.1.96 — older than CI's 1.98.0, so
  provisional per the packet): clean.
- `cargo test -p glasshouse --all-features` (macOS, this worktree, run alone
  per practice §40): every target green **except** the five pre-existing
  `session::store` tests the packet predicted — see the report's `PATCHES
  ANOTHER PACKAGE MUST APPLY`. Re-run after `touch`ing every source file to
  force a rebuild; the same five and only five fail on the unmodified tree's
  own schema-version bump, reproducing the packet's own "four for four"
  history one time more (five, this time, because migration 11 adds a whole
  table rather than only columns on an existing one).
- **The local gate (`scripts/ci-local.sh`) was not run** — another worker
  (`routing-score`) was live this round in the same `routing/**` tree per
  §40; running the full container gate beside a second live worker would
  attribute its load to this package's code. The Linux, Windows and MSRV
  legs are therefore unproven for this change.
- **No provider key or network request anywhere in this package.** Every
  test constructs its own fixture provider or writes only inside its own
  `tempfile::tempdir()`.

Missing evidence:
- `main.rs` does not yet call `start_if_required_with_telemetry`, so no real
  Glasshouse session writes a routing observation today. See the report.
- No consumer downstream of `ObservedEvidenceSource` exists in the shipped
  binary yet — `routing-score`'s own package, concurrent with this one, is
  what would supply the caller that makes design decision 6 real rather
  than reachable-but-unused.
- `first_byte_at`, `first_token_at`, `first_tool_call_at`, `tool_rounds`,
  `retries`, `repairs`, `failovers`, and every token/cost column: no
  producer in this build can supply them. See this entry's own opening
  finding for why, and the report's leading section for what would have to
  change.

---

## Per-line disposition

The criterion is practice §33's: *ask the box as a question a user would
ask, and see whether the honest answer is yes in the shipped binary.*

**Store project-local routing observations as an append-oriented evidence
ledger rather than only maintaining current aggregate counters.**
**Mechanism CLOSED, shipped-binary reach OPEN.** `routing_observations` is
append-only by schema (no `UPDATE` path anywhere) and by this store's own
method list (`record`, `recent`, `summarize` — no editor). Proven with a
real gateway write, mutation-killed. Not yet reached from `main.rs`.

**Record provider, route, model identity, authenticated quota context,
harness, request purpose, and observation timestamp for each measurable
turn.** **PARTIAL.** Provider, model, route, harness and quota context are
all captured once a session is bound, proven through a real exchange.
`purpose` is never supplied by this producer — nothing in the gateway's
partition knows why a turn was made — and stays `NULL` on every row this
build writes. Open on `purpose` alone; the rest closes with the wiring
patch.

**Record dispatch time, first-byte time, time to first real token, time to
first tool call, and completion time when the protocol exposes them.**
**OPEN.** `completed_at` is accurate; `dispatched_at` is an honest upper-
bound proxy (see this entry's opening finding); the other three are
structurally unavailable to a pass-through gateway and are not attempted.

**Do not treat whitespace padding, transport keepalives, or reasoning-only
deltas as the first generated token.** **OPEN, moot.** This producer never
computes a first-token time, so it cannot violate this line, but it also
does not satisfy it — there is no correct behaviour to credit yet.

**Record input tokens, output tokens, cached-input tokens, and monetary
cost only when they are actually exposed or can be estimated with an
explicit confidence label.** **Mechanism CLOSED, no producer OPEN.** The
schema stores all four, nullable, with `cost_confidence` unforgeable via a
`CHECK` migration-proofed in both directions (a bare cost is refused, a
paired one is accepted). No producer in this build ever supplies a value —
reading them means parsing a response body this module cannot read.

**Record successful tool rounds, retries, repairs, and failovers, and the
final user-visible outcome separately.** **PARTIAL.** The four counters are
separate nullable columns by schema (never folded into a rate); no producer
supplies any of them today, because the gateway has no notion of a turn
spanning more than one HTTP exchange. `outcome` is supplied, but only as a
transport-level proxy — see this entry's opening finding for exactly what
that means and does not mean.

**Preserve raw observations alongside rolling aggregates so a routing
decision can be audited and aggregation logic can be recalibrated.**
**CLOSED.** `summarize` never deletes or mutates a row; `recent` reads the
same rows raw. Tested directly: an observation outside a summary's window
is excluded from the aggregate and still readable through `recent`.

**Compute robust rolling summaries such as median, tail latency,
exponentially weighted averages, failure rates, and sample counts where
useful.** **CLOSED.** Median and p95 exchange duration, an EWMA of
duration, and a failure rate, each carrying its own sample count — computed
on read from raw rows.

**Separate warm-context, cold-context, and unknown-context observations
instead of averaging away cache effects.** **CLOSED.** `context_state` is
`NOT NULL DEFAULT 'unknown'` by schema; `summarize` takes it as a required
filter rather than an optional one, so a caller cannot accidentally blend
buckets. Tested: warm and cold observations of the same identity never
share one summary. This producer always writes `unknown`, honestly, since
the gateway has no cache-state signal of its own.

**Keep metrics distinct for materially different model versions,
quantizations, routes, or changing stealth-model identities.** **CLOSED by
construction.** `provider`, `model` and `route` are exact-match identity in
every read; a different model string — including one a stealth-routing
change produces — is a different key with no normalization step that could
collapse two into one.

**Attach source, observation window, sample size, freshness, and confidence
to every aggregate used for routing.** **CLOSED.** `AggregateReading<T>`
wraps `crate::provider::quota::Reading` (source, observed-at) with the two
things that type does not carry — sample count and window — plus
`freshness()` and `confidence()` accessors. Follows design decision 2's
named precedent rather than inventing a parallel vocabulary.

**Apply conservative priors or keep a metric unknown when the sample is too
small to support a routing decision.** **CLOSED.** `MIN_SAMPLE_FOR_SUMMARY`
(5, matching `crate::config::pairing::CONFIDENT_AT_OBSERVATIONS` — the same
underlying question, not a coincidence). Below it, every field of
`RoutingSummary` is `None`; tested at both `MIN_SAMPLE_FOR_SUMMARY - 1` and
exactly at it.

**Decay or expire stale operational evidence without deleting durable raw
observations prematurely.** **CLOSED.** `summarize`'s window parameter
excludes anything older than `now - window_seconds` from every aggregate;
nothing is ever deleted. Tested: an old failed observation outside the
window does not pull a summary's failure rate down, and is still present in
`recent`'s own output.

**Treat token volume, request count, context size, and spend as resource
telemetry rather than evidence of quality or progress.** **CLOSED.** No
field of `RoutingSummary` reads `input_tokens`, `output_tokens`,
`cached_input_tokens` or `cost_micro_usd` — a test that varies only those
fields between two ledgers asserts identical failure-rate and duration
aggregates, the shape the packet's own design decision 5 asked for.

**Keep the evidence ledger physically project-scoped and require explicit
export before observations leave the project.** **CLOSED on the physical
half, vacuously true on the export half.** `EvidenceLedger::open` reaches a
database file only through `crate::database::open(runtime)`, the same door
`ProjectMemory` and `ProjectCheckpoints` use — there is no second
constructor that accepts a path, a project id, or another project's
connection. Proven twice: in-crate and from outside the crate, two sibling
projects sharing one data root never see each other's rows. No export
function exists at all in this package, so there is no implicit path for an
observation to leave a project either — the requirement is satisfied by
absence rather than by a built, deliberately-named export operation, which
is the honest state to report rather than invent a mechanism nothing calls.

### Phase 33A after batch 37 — the five aggregate lines close

State: **COMPLETE** for 1335, 1336, 1339, 1340 and 1341, in addition to the six
already closed. **Nine of fifteen.**

All five were open for exactly one reason: `summarize` had no production
consumer. It has one now. `gateway/session.rs::observe_exchange` builds an
`ObservedEvidenceSource` over the ledger and hands it to
`InteractiveRouting::on_provider_failure`, reached from the accept loop —
mutation-proven by replacing `evidence_ledger.as_deref()` with `None`, which
turns `a_real_provider_failure_with_recorded_evidence_prefers_the_stronger_candidate_over_order`
red. Re-run independently by the integrator.

That test exercises the whole set at once: rolling summaries over real rows
(1336), sample counts above `MIN_SAMPLE_FOR_SUMMARY` deciding whether an
aggregate is usable at all (1339, 1340), raw observations preserved beside the
summary that reads them (1335), and a bounded observation window (1341).

**Still open: 1331–1334**, because the gateway cannot read a response body by
design — unchanged by this round, and unreachable from `gateway/**` at all.
