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

State: **COMPLETE** for map lines 1329, 1337, 1338, 1342 and 1343 — five
of fifteen. **1330 was listed here as COMPLETE and is now re-opened** — see
*"1330 re-opened"* at the end of this entry; the summary disagreed with this
same document's own per-line disposition below, which has always read PARTIAL. **NOT STARTED, blocked** for the other nine.

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
turn.** **COMPLETE as of 2026-09-02** — see *"Line 1330 — CLOSED"* at the end
of this file. Provider, model, route, harness and quota context were captured
from the start, proven through a real exchange; `purpose` was the one gap and
`GH-TURN-PURPOSE` closed it by stamping `HARNESS_TURN_PURPOSE` in
`record_routing_observation`, mutation KILLED.

*The disposition this replaces read:* **PARTIAL** — *"`purpose` is never
supplied by this producer — nothing in the gateway's partition knows why a turn
was made — and stays `NULL` on every row this build writes. Open on `purpose`
alone; the rest closes with the wiring patch."* That was accurate when written
and it is what correctly held the box open through the 2026-08-29 re-open. The
"wiring patch" it predicted is exactly what landed.

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

---

### 1330 re-opened, 2026-08-29 — the summary disagreed with this file's own body

**The box was ticked from this entry's summary line while this entry's own
per-line disposition said `PARTIAL`.** Both sentences were in the same document
the whole time. The disposition, unchanged since it was written, reads:

> *"`purpose` is never supplied by this producer — nothing in the gateway's
> partition knows why a turn was made — and stays `NULL` on every row this build
> writes. **Open on `purpose` alone**; the rest closes with the wiring patch."*

Confirmed independently by a read-only audit and by the orchestrator:

- `NewObservation.purpose` is declared at `routing/evidence.rs:247` and defaults
  to `None` at `:279`.
- **There is no `with_purpose` builder anywhere on the type.** `evidence.rs:296-329`
  lists every builder that exists: `with_route`, `with_quota_context`,
  `with_harness`, `with_timing`, `with_outcome`, `with_context_state`. A field
  with no setter cannot be set by any caller, production **or test** — which is
  a stronger statement than the usual "no production caller".
- The sole production writer, `gateway/session.rs::record_routing_observation`
  (`:278-322`), calls five of those builders and never touches `purpose`.
- The gateway could not supply one anyway: `Exchange` (`gateway/ingress.rs:117`)
  carries `outcome`, `status`, `provider`, `protocol` and `host`, and `protocol`
  is already stored as `route`. Nothing purpose-shaped reaches this writer.

**The standard that decides it is the project's own, applied twice already in
the same phase family.** Map lines **1542** (`ObservedEvidence::reliability` is
`None` on 100% of real rows) and **1545** (`ContextState` is `Unknown` on 100% of
real rows) are both refused and unticked, with the reasoning recorded in
`phase-35b.md`. `purpose` is absent by a wider margin than either. Leaving 1330
ticked would make that standard something this project applies to open boxes and
suspends for closed ones.

**Six of the seven facts the line names are genuinely recorded and remain
proven** — provider, route, model, quota context, harness and timestamp, through
`a_real_forwarded_exchange_reaches_the_routing_evidence_ledger`. Nothing about
that evidence is withdrawn. The line simply names seven things and the build
records six.

**What closing it now requires:** a producer, not a builder. Adding
`with_purpose` is five minutes; the missing part is that no purpose-bearing fact
reaches `record_routing_observation`. Whoever closes this must first answer what
distinguishes one gateway turn's purpose from another's in the current build —
and today the honest answer may be "nothing", in which case the line waits for
disposable-job routing to actually traverse the gateway.

### Two more PARTIAL ticks found by the same audit — flagged, deliberately NOT reversed

The audit applied its vocabulary to every ticked line in Phases 33A, 33C and 35B
(27 lines) and returned **24 SOUND**. Besides 1330 it found two more it judged
`PARTIAL`. **Both are left ticked, and the reason is that both were closed
knowingly, with the narrowing written down at the time** — which is a different
situation from 1330, where a summary contradicted its own body.

- **Map line 1377** (four categories of routing benefit) — `RoutingBenefit`
  (`routing/interactive.rs:787-804`) has three variants, and two of the four
  named categories have no producer in the current build. **Already disclosed in
  full at `phase-33c.md:117-142`** as a deliberate two-of-four closure.
- **Map line 1541** (decay scoped to *"the exact harness-profile-model-backend
  combination"*) — the audit sharpened what was known: `launch_profile` is not
  merely unqueried, it is **`String::new()` at the only non-test constructor**
  (`routing/interactive.rs:681`, annotated in-line explaining why), and
  `ObservedEvidenceSource::observed` (`evidence.rs:1141-1156`) never reads it.
  So the decay is scoped to three of the four named dimensions on every real row.
  Disclosed as a narrowing in `phase-35b.md:221-230`.

**The orchestrator's ruling, with its reasoning, per practice §33.** A disclosed
narrowing that a previous integrator weighed and accepted is a judgement, and
reversing it deserves more than one audit at the end of a spent window. 1330 is
not that: its own entry says `PARTIAL` and *"open on `purpose` alone"*, so
un-ticking it restores agreement rather than overturning a decision. **1377 and
1541 are recorded here as live questions for the next round**, with the audit's
sharper evidence attached so that whoever takes them up starts from it rather
than re-deriving it.

**The systemic finding worth more than the three boxes:** in every case the
*evidence files were honest* and the *map tick was more generous than the entry
it rested on*. The gap is between an entry's summary line and its body. Nothing
checks that those two agree — `scripts/check-evidence-coverage.py` verifies an
entry **exists** for a phase, not that its summary matches its dispositions.
That is a cheap gate somebody could write, and it would have caught this one.

---

# Line 1331 — PARTIALLY VERIFIED 2026-08-30, and the box stays ☐

Package `GH-GATEWAY-FIRST-BYTE`.

*"Record dispatch time, first-byte time, time to first real token, time to
first tool call, and completion time when the protocol exposes them."*

State: **PARTIALLY VERIFIED** — three of the five timestamps have real
production evidence; two have no honest producer.

## What now exists

`first_byte_at` had **no production writer and no production reader** before
this package. It has both now:

- **Producer** — `gateway::ingress::forward` takes the clock **once**, when
  upstream's first response byte becomes available, onto `Exchange::first_byte_at`.
  An exchange that never reached upstream (`Unauthenticated`, `Declined`,
  `Unrouted`) records `NULL`, because there was no first byte.
- **Carrier** — `NewObservation::with_first_byte_at`, added **additively**. The
  packet suggested widening `with_timing` to three parameters; that broke two
  call sites outside scope (`routing/interactive.rs`, `shell/mod.rs`), and the
  worker reverted it rather than reach into forbidden files. Correct call.
- **Consumer** — `EvidenceLedger::consumption_by_purpose` returns a first-byte
  sample count and a mean time-to-first-byte per group, and
  `glasshouse routing-cost` prints them.

**The honesty rule the command already enforced for tokens is enforced here
too**: a group with no timed rows prints *"not recorded"*, never `0ms`.
Mutating that to `"0ms"` is **KILLED** by
`an_exchange_that_never_reached_a_provider_records_no_first_byte_and_the_reader_says_so`.
The characteristic mutation — `.with_first_byte_at(exchange.first_byte_at)` →
`.with_first_byte_at(None)` — is **KILLED** on the call.

## Why the box does not tick

`first_token_at` and `first_tool_call_at` are `NULL` on every row, because
finding either boundary means parsing a response body `gateway::ingress` is
designed never to read.

**The qualifier does not rescue it.** *"When the protocol exposes them"* is a
claim about the **protocol's** capability. For a streaming provider the protocol
**does** expose a first-token boundary; Glasshouse declines to look. Reading the
qualifier as *"when Glasshouse chooses to look"* is the same stretch that
un-ticked 1455 and 1456 the same morning, and it is refused here for the same
reason.

**What would close it:** a decision about whether the relay may observe response
**framing** — the boundary between chunks — without reading content. That is
narrower than "parse the body" and may have an honest answer. It is a product
decision, and it belongs with the `ingress` ruling that already blocks the relay
path's usage reader.

## One scope overflow, approved

`gateway/conformance.rs` was not in `EXPECTED FILES`. It held
`a_real_forwarded_exchange_reaches_the_routing_evidence_ledger`, which asserted
`first_byte_at_unix == None, "this producer never supplies it"` — a fact this
package's whole purpose is to make false. The worker replaced the negative
assertion with a positive ordering check
(`dispatched_at <= first_byte_at <= completed_at`) on the same already-wired
exchange. **Leaving a known-stale failing assertion in place would have been
worse**, and the alternative — reporting and stopping — would have shipped a red
target. Approved.

## Limits

- `dispatched_at` remains the pre-existing accept-loop handoff instant, an
  honest upper-bound proxy rather than the true dispatch inside
  `ingress::forward`. Unchanged by this package, and it predates it.
- `mean_time_to_first_byte_ms` is a **mean**, not a median, computed in SQL over
  rows carrying both `first_byte_at` and `dispatched_at`.
- Says nothing about the disposable path, which does not go through the gateway.


---

## From `GH-FAILURE-TAXONOMY` (2026-08-31) — 1334 stays OPEN on two quantities

`failovers` (this exchange's own `ChangeCause::Failover`, 0/1) and `retries` (0 — the gateway forwards exactly once, verified in `ureq` 3.4.0's source) are now written on every row, and a 2xx whose stream was cut or whose permitted body never came records `outcome = failed`. **`tool_rounds` and `repairs`** still need a turn structure and a body this layer cannot see. 1331–1333 are unchanged: first real token, padding-vs-token and token counts require reading content, which the ruling does not permit. Full entry: `phase-33c.md`.

---

# Line 1330 — CLOSED 2026-09-02 (`GH-TURN-PURPOSE`), and the re-open was right to happen

The 2026-08-29 re-open above is vindicated, not overturned: it held this line
open on `purpose` alone, and `purpose` is exactly what this package supplied.
**Its stated reason had since gone stale, and that is worth recording.** It
said:

> *"There is no `with_purpose` builder anywhere on the type. A field with no
> setter cannot be set by any caller, production **or test**."*

`NewObservation::with_purpose` now exists (`routing/evidence.rs:792`) with
**ten** production call sites in `main.rs` — each naming why *Glasshouse
itself* called a provider (`CORRELATION_PURPOSE`, `CLASSIFICATION_PURPOSE`,
`EXTRACTION_PURPOSE`, `ROUTING_LATENCY_PURPOSE`, the three
`CONTEXT_FIREWALL_*`). What remained true is narrower and is the actual gap
this package closed: `SessionRouting::record_routing_observation`
(`gateway/session.rs:425`), the **only** production writer of a real forwarded
turn, called seven builders and not that one.

**What ships.** `HARNESS_TURN_PURPOSE = "harness-turn"`
(`routing/evidence.rs`), stamped in `record_routing_observation`'s builder
chain. It is a *recording*, not an inference: the gateway already knows the row
came from relaying a harness request — that is the precondition the function
documents for writing a row at all (the exchange reached the provider **and**
`assignment` is `Some`).

The other six facts were already proven; the seventh joins them in the same
test.

Regression: `gateway::conformance::a_real_forwarded_exchange_reaches_the_routing_evidence_ledger`.

| mutation | vocabulary | result | killed by |
|---|---|---|---|
| delete `.with_purpose(Some(HARNESS_TURN_PURPOSE))` from `record_routing_observation` | `forwarded-turn-records-no-purpose` | **killed** | `gateway::conformance::a_real_forwarded_exchange_reaches_the_routing_evidence_ledger` |

> observed: ``assertion `left == right` failed`` — `left: None`, `right: Some("harness-turn")` (`conformance.rs:836`)

## The orchestrator's Phase −1 was wrong, and the worker caught it

The packet's FEASIBILITY asserted: *"Verified safe: no consumer anywhere treats
`purpose IS NULL` as meaningful — every `purpose: None` in the tree is a struct
construction or query default, not a filter."*

**False.** `RoutingOverhead::from_consumption` (`routing/evidence.rs`) matched
`None if group.harness_recorded =>` into
`coding_agent_requests`/`coding_agent_tokens` — `harness_recorded` being what
tells the two `NULL`-purpose producers apart, `true` only for gateway rows,
which is precisely the traffic this box stamps. The orchestrator's grep looked
for `purpose.is_none()`, `purpose == None` and `purpose: None`, and **a match
arm is none of those shapes.**

Stamping the purpose without touching that match would have dropped every
future gateway row through to the catch-all `_ => unstamped_*` arm, silently
zeroing `coding_agent_requests` — map lines 1464/1832/1833's *"interactive
coding cost"* — from this build forward. The existing test
`from_consumption_leaves_correlation_rows_out_of_every_bucket` proves that
fall-through is real and intended for *unknown* purposes, which is exactly what
the new constant would have been.

The worker added `Some(HARNESS_TURN_PURPOSE) | None if group.harness_recorded`
so the bucket spans the stamped/unstamped boundary as one fact rather than two,
and updated the two adjacent doc comments. **The integrator read that diff and
accepted it**: the guard binds the whole or-pattern, and a gateway row always
records a harness, so a stamped row that somehow lacked one still falls back
conservatively.

**Lesson, recorded because the check is cheap and the miss was not:** a Phase −1
consumer search must cover **match arms**, not only method calls and struct
literals. `grep 'purpose'` in the consuming module would have found it; three
narrower greps did not.

## Owed follow-up — named so it does not evaporate

`from_consumption`'s new arm is **production code with no direct test**. It is
covered only by the pre-existing suite continuing to pass (54/54, including the
correlation-bucket test). It protects **1464/1832/1833**, not 1330, so it does
not hold this line — but it is exactly the unwatched-production shape behind
all ten of this project's historical un-tickings.

~~**Successor: one Green package adding a `from_consumption` assertion …**~~
**DONE the same night** (`GH-CONSUMPTION-ARM`, 2026-09-02) — dispatched rather
than filed, because an owed follow-up that becomes a note is this process's
most common trap.

`routing::evidence::correlation_tests::from_consumption_routes_harness_turn_rows_across_the_stamped_boundary`
asserts all three cases the arm spans: a `Some(HARNESS_TURN_PURPOSE)` row and a
`None`-with-`harness_recorded` row land in the **same** `coding_agent_*` bucket,
and a row with some other purpose still falls through to `unstamped_*`. No
production line changed; the existing
`from_consumption_leaves_correlation_rows_out_of_every_bucket` was not touched.

| mutation | vocabulary | result | killed by |
|---|---|---|---|
| remove `Some(HARNESS_TURN_PURPOSE) \| ` from the arm, leaving `None if group.harness_recorded` | `harness-turn-falls-through-to-unstamped` | **killed** | the test above |

> observed: ``assertion `left == right` failed`` — `coding_agent_requests` 2 not 5, `coding_agent_tokens` `Some(15)` not `Some(165)` (`routing/evidence.rs:5164`)

That mutation **is** the regression the arm was added to prevent, so lines
1464/1832/1833 are now watched at the consumer rather than only at the producer.

## Recorded limits

- Proves the builder call is load-bearing for one row shape (a bound,
  provider-reaching exchange). The call is unconditional before the `outcome`
  match's early returns, so no variant-specific behaviour was possible to
  introduce, but no per-variant mutation was run.
- The named test lives in `src/gateway/conformance.rs:749`, **not**
  `tests/routing_evidence.rs` as the packet claimed — that file's own module doc
  says it deliberately does not re-prove the gateway's production wiring.
  `conformance.rs` was outside the packet's `EXPECTED FILES` and is recorded as
  justified `scope_overflow`.

**Phase 33A now stands at 11/15.** 1331-1334 remain blocked on the relay-path
`ingress` reader — the same blocker as line 1263 — and nothing here changes that.

---

## Censused again 2026-09-02 (`GH-RECON-33A-32G`) — one disposition was stale in the direction that keeps the phase shut

- **1333 — the ledger's *"no producer in this build ever supplies a value"* is STALE.** A translated gateway exchange (`translate::place` → `translate::serve`, a real path whenever the harness's protocol differs from the provider's and a supported pair exists) decodes the canonical response, and `tokens_of` (`gateway/translate/mod.rs:825-831`) hands its `usage` to `record_routing_observation`'s `.with_tokens(...)` (`gateway/session.rs:478-488`) — all three columns, cached included. The relayed majority still writes `NULL`, by design. Nothing proves the translated chain end to end: `conformance.rs` proves the relay's row only. **Successor dispatched: `GH-TRANSLATED-USAGE-PROOF`** (Green, two socket tests: the translated row carries the fixture's exact counts; the relayed row is `NULL` even when the body carried a usage object). **Reading call, made now:** the line says *only when they are actually exposed*; a translated exchange exposes them and a relayed one does not, so the pair of tests — counts where exposed, `NULL` where not — is the line's own claim, and 1333 ticks on it. The production module doc at `routing/evidence.rs:89-93` repeats the stale sentence and is corrected with that package.
- **1331, 1332 — refused, unchanged, word for word.** `first_token_at`/`first_tool_call_at` still have no builder; the new `Framing` type carries byte counts and stream-end state, not SSE event boundaries, so it cannot say *first real token* without the content read the ingress design forbids. Register row P1b.
- **1334 — the ledger's PARTIAL is accurate.** `retries` (`gateway/session.rs:474`, a true `0`: `ureq` 3 retries nothing) and `failovers` (`:473`, from `ExchangeEffect`) are written; `tool_rounds` and `repairs` have no concept anywhere in the tree. No successor.

---

## 1333 — CLOSED 2026-09-02 (`GH-TRANSLATED-USAGE-PROOF`, Green, tests only)

The census above said no test joined a translated exchange to the ledger; the
worker found one already did (`gateway_translate.rs:802-821`, since
`bbd8103`) and said so before writing anything — the recon grepped the file
it had and still reported no hits. What was genuinely missing was the
**restraint half**: nothing asserted a relayed exchange writes `NULL`. The new
file proves both through a real socket, and the relay-path mutation
(fabricate `Some(Tokens { input: 1, .. })` at the one `Exchange` default,
`ingress.rs:1057`) is the one that proves the line's *only when*. The stale
sentence at `routing/evidence.rs:89-93` is corrected in the same commit.

### Record input tokens, output tokens, cached-input tokens, and monetary cost only when they are actually exposed or can be estimated with an explicit confidence label. (line 1333)

Contract: Given a translated gateway exchange with provider-stated usage, when it completes, Glasshouse records input, output and cached-input tokens on its routing row equal to the provider's own numbers; given a relayed exchange whose body it never reads, the same row records NULL for all three, while preserving zero production behaviour change.

State: **COMPLETE** — ruled 2026-09-02. The worker returned `open`, deferring the *whole line* reading to the orchestrator; the reading was made in the census entry above and holds: the line says *only when they are actually exposed*, a translated exchange exposes the three token counts and a relayed one does not, and the pair of socket tests proves counts where exposed and `NULL` where not, with both mutations KILLED. The monetary-cost half is line 1307's (☑, `with_cost` carrying `cost_confidence` — an explicit confidence label, which is the line's other condition), not this package's to re-prove. The worker's third limit stands as scope: one protocol pair, non-streaming; the same `tokens_of` → `with_tokens` chain serves the other pairs and the streaming path.

Production evidence:
- `gateway/session.rs` — `record_routing_observation (.with_tokens at :478-488, unchanged by this package)`
- `gateway/ingress.rs` — `exchange() (:1045-1058, tokens: None default for every relay-path Exchange, unchanged by this package)`
- `gateway/translate/mod.rs` — `tokens_of/finish (:629-639, :825-831, unchanged by this package)`

Regression evidence:
- `gateway_translate_evidence::a_translated_exchanges_stated_usage_reaches_the_routing_row`
- `gateway_translate_evidence::a_relayed_exchange_invents_no_usage_even_though_its_body_has_some`
- `gateway_translate::a_claude_code_request_is_translated_to_chat_completions_and_the_answer_back_with_ids_preserved (pre-existing, same claim, independently confirmed still passing)`

| mutation | vocabulary | result | killed by |
|---|---|---|---|
| gateway/session.rs:478-489 .with_tokens(exchange.tokens...) -> .with_tokens(None, None, None) | `skip-state-update` | **killed** | `gateway_translate_evidence::a_translated_exchanges_stated_usage_reaches_the_routing_row` |
| gateway/ingress.rs:1057 tokens: None -> tokens: Some(Tokens { input: 1, output: 1, cached: None }) | `fabricate-value` | **killed** | `gateway_translate_evidence::a_relayed_exchange_invents_no_usage_even_though_its_body_has_some` |

> skip-state-update observed: assertion `left == right` failed: prompt_tokens (40) minus cached_tokens (8) is Anthropic's input_tokens

> fabricate-value observed: assertion `left == right` failed: a relayed exchange's body is never read; nothing may be invented for it

Recorded scope limits — stated by the worker, not discovered later:
- does not authorise ticking 1333 as a whole line: the relayed majority stays NULL by design, and whether that satisfies the line's words is an orchestrator reading call
- the monetary-cost (cost_micro_usd) half of 1333 has no producer traced or proven here
- only the anthropic-messages<->openai-chat pair and non-streaming responses are exercised; the other two supported pairs and the streaming path share the same producer/writer but are not separately driven by this package

---

---

# Lines 1331 and 1332 — CLOSED 2026-09-02 (`GH-STREAM-FIRST-EVENTS`, Amber, Sonnet high): the two timestamps the seam can honestly supply

The 2026-08-30 ruling above kept 1331 PARTIALLY VERIFIED because the relay
declines to parse the body that carries a first-token boundary, and *"when
the protocol exposes them"* is about the protocol, not about what Glasshouse
chooses to look at. Both halves still hold. What changed is the path: since
Phase 56 a **translated** exchange is decoded by the seam in order to be
re-encoded, and the row already carries `tokens`, `effort` and `turn_shape`
from exactly that decoding. `design-decisions.md` (*First real token and
first tool call on the translated path*) ruled the two instants the same kind
of fact on the same path, and wrote 1332's rule on the canonical vocabulary.

**Contract (1331).** Given a translated gateway exchange, when the provider's
response passes through the seam, Glasshouse records on the exchange's
routing-observation row the instant of the first text delta carrying a
non-whitespace character (`first_token_at`) and the instant of the first
tool-use block start (`first_tool_call_at`), and `glasshouse routing-cost`
prints a mean time-to-first-token and time-to-first-tool-call per group
beside its time-to-first-byte — while a relayed exchange writes `NULL` for
both, as it does for `first_byte_at`'s siblings, and a document response
records both as its own `first_byte_at` only when the document contains a
qualifying block.

**Contract (1332).** The first real token is never a whitespace-only delta,
an SSE transport comment, a provider keepalive event, or a reasoning-only
delta.

**Production evidence.** `gateway/translate/mod.rs::FirstEvents::note` — one
rule on `StreamEvent`: the first `Delta::Text` with a non-whitespace
character stamps `first_token_at` once, the first `BlockStart::ToolUse`
stamps `first_tool_call_at` once, nothing else touches either and no text is
retained; called in the streamed loop before each event is encoded;
`FirstEvents::of_document` runs the same rule over `Response::as_events()`
with a clock that answers `first_byte_at`, for `deliver_document` and the
stream-requested-but-document-answered branch (the worker's one deviation
from the packet's shape, provably equivalent because `canonical::accumulate`
concatenates deltas without dropping characters — recorded as a packet error
by the worker, accepted). `gateway/ingress.rs::Exchange` carries both beside
`first_byte_at`, `None` on every relayed and refused path;
`gateway/session.rs::record_routing_observation` writes them through
`NewObservation::with_first_token_at` / `with_first_tool_call_at`
(`routing/evidence.rs`, beside `with_first_byte_at`, into the two columns
migration 11 created and nothing had written); `consumption_by_purpose`
computes the two `COUNT`/`AVG(CASE …)` pairs beside the first-byte pair;
`main.rs::render_routing_cost` prints four lines through
`render_time_to_first_byte`, *not recorded* for an untimed group. The 1332
exclusions at the vocabulary level: an SSE comment is dropped by `SseReader`;
`ping` decodes to no event (`anthropic.rs`); a thinking or signature delta is
refused at decode — so `note` can never see a reasoning-only delta, and the
whitespace check is the one exclusion done by content.

**Regression evidence** (`tests/gateway_first_events.rs`, a raw-socket fake
upstream and a real `Gateway`, the `anthropic-messages → openai-chat` pair
`gateway_translate.rs` drives, 5):
`a_translated_streamed_exchange_notes_first_token_and_first_tool_call_in_order`
(a keep-alive comment, a whitespace-only delta, a 1.2 s pause, real text, a
pause, a tool-use block — the three instants ordered with gaps of at least a
second), `a_translated_stream_with_text_and_no_tool_use_records_no_first_tool_call`,
`a_relayed_exchange_records_no_first_token_or_first_tool_call`,
`a_translated_document_with_text_and_a_tool_call_records_both_as_first_byte_at`,
`a_translated_stream_whose_only_text_is_whitespace_records_no_first_token`;
unit `gateway::translate::tests::{first_events_note_stamps_only_a_real_token_and_a_tool_use_and_never_twice,
first_events_of_document_uses_first_byte_at_as_the_only_clock_reading}`;
`routing_cost::a_row_carrying_first_token_and_first_tool_call_prints_real_figures_and_an_untimed_group_says_so_twice`.

**Mutations** (worker, four, all KILLED, restored byte-identical):
`whitespace-counts` (the non-whitespace check → `true`) by the whitespace
test — *a whitespace-only text delta must never count as the first real
token*; `never-stamped` (the streamed loop's `note` call removed) by the
ordered test — `first_token_at` was `None`; `tool-call-at-any-block` (the
`ToolUse` arm widened to any block start) by the no-tool-use test — the
worker's first attempt dropped the arm's `..` and produced a non-compiling
mutant that `mutate.sh` reported as a false KILLED (§80's fourth way), re-run
with the pattern intact; `relay-stamps` (the relay's success `Exchange` given
`first_token_at: first_byte_at`) by the relayed test — *nothing may be
invented for it*.

Gates: `gateway_first_events` 5/5, `--lib gateway` **whole** 210/210 (the
module's source scans run only there), `routing::evidence` 60/60,
`gateway_translate` 9/9, `gateway_translate_evidence` 2/2, `routing_cost`
9/9, `blast-radius.sh --targeted` exit 0. Scope overflow, mechanical:
`tests/routing_economics.rs`'s one `PurposeConsumption` literal gained the
four fields.

**Recorded limits.** Unix-second resolution, like every timestamp on the
row — at that resolution *time to first token* is nearly always zero or one,
honest and nearly useless for comparison; the millisecond offsets Phase 33B's
TTFC family needs are a schema decision (Cluster G) and the named successor
**`GH-STREAM-TIMING-MS`** (Red). The socket tests drive one pair; the
mechanism is pair-agnostic by construction. The `ping` and reasoning-delta
exclusions hold by construction (decoder facts read, not driven through the
fixture, whose wire has neither). A mid-stream client disconnect keeps the
instants already noted rather than discarding them (a fact that happened,
unlike usage) — untested either way. The stream-requested-but-document branch
shares `of_document` and has no dedicated test.

State: **COMPLETE** for 1331 (promoted from PARTIALLY VERIFIED) and 1332.
Phase 33A stands at 14 of 15; 1334 stays open on `tool_rounds` and `repairs`
(the translated response's tool-use blocks are now countable at the same
seam — a Green successor, `GH-TOOL-ROUNDS-ON-TRANSLATED`, once the ms
question is settled or independently of it).

# Line 1334 — CLOSED 2026-09-02 (`GH-TOOL-ROUNDS-ON-TRANSLATED`, Amber, Sonnet high): Phase 33A complete

`GH-FAILURE-TAXONOMY` left this line open on `tool_rounds` and `repairs`, *a
turn structure and a body this layer cannot see*. The relay still cannot; the
translated seam decodes both halves of every exchange, and
`design-decisions.md` (*Tool rounds and repairs on the translated path*)
defined the two counts on what it decodes.

**Contract.** Given a translated gateway exchange, when it is recorded,
Glasshouse writes the number of tool-use blocks in the response as
`tool_rounds` and the number of `is_error` tool results in the request as
`repairs` — `0` when the seam looked and found none — beside the `retries`,
`failovers` and `outcome` already recorded, each its own column; while a
relayed or refused exchange writes `NULL` for both and nothing here judges
success beyond the block counts the protocol names (*successful* is the
reader's subtraction, rounds begun minus repairs, never a stored column).

**Production evidence.** `translate/canonical.rs::Request::error_tool_results`
beside the `turn_shape` derivation; `translate/mod.rs::FirstEvents` gains
`tool_uses`, incremented on every `BlockStart::ToolUse` in `note` (the
instant is stamped once, the count grows; `of_document` inherits it),
`serve` computes `repairs` where the request is decoded, `finish` carries
`tool_rounds: Some(first.tool_uses)`; `ingress.rs::Exchange` carries both,
`None` on the relay and on refusal; `gateway/session.rs::record_routing_observation`
writes them through `NewObservation::with_tool_rounds` / `with_repairs`
(`routing/evidence.rs`, beside `with_retries` — the builders were the gap;
the columns date from migration 11 and the header's *not supplied* bullet is
rewritten).

**Regression evidence** (`tests/gateway_tool_rounds.rs`, shipped `Gateway`
through a raw-socket fake upstream, 4):
`a_translated_stream_with_two_tool_calls_and_one_error_result_counts_both`,
`a_translated_document_with_one_tool_call_and_no_error_result_counts_both`,
`a_translated_stream_with_no_tool_use_counts_zero`,
`a_relayed_exchange_records_no_tool_rounds_or_repairs`.

**Mutations** (worker, restored byte-identical): `rounds-never-counted` (the
increment removed) KILLED by the two counting tests; `errors-not-counted`
(`is_error: true` → `false` in the request count) KILLED — *the one
is_error: true tool result must be counted*; `relay-counts` (the relay's
`Exchange` given `Some(0)`) KILLED — *nothing may be invented for it*.

**Recorded limits.** An exchange whose request decoded but never reached a
response writes `repairs: Some` with `tool_rounds: None` — handled by
construction (the sums are per column), not driven through a socket; one
protocol pair drives the tests, as for the first-events package.

State: **COMPLETE**. **Phase 33A stands at 15 of 15.**
