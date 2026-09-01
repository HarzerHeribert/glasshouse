# Capability evidence — phase 33C

Split from the single evidence ledger this project used to keep at the repository root (see `docs/product/evidence/README.md` for the full index and the entry template / evidence rules that used to precede it).

### Phase 33C — failure, quota, and route correlation

Contract: Given a gateway-backed interactive session whose provider has just
failed, when Glasshouse ranks the compatible failover candidates, it scores a
candidate sharing the failed backend's failure domain below one that does not,
and records on every assignment change which domain actually moved — while
never treating a merely-different provider as *proven* independent, and never
letting a rate-limit on one credential become a claim about another upstream.

State: **COMPLETE** for map lines 1371, 1372, 1375, 1377 and 1378 — five of
seventeen. **NOT STARTED** for the other twelve, each with its missing source
named below.

Production evidence:

- `src/routing/domain.rs::FailureDomain` — three-valued (`Shared`, `Unknown`,
  `Independent`), and `FailureDomain::between` is the **only** producer of one
  in the crate. It returns `Shared` when two backends share a provider and
  `Unknown` otherwise; it can never return `Independent`.
- `src/routing/interactive.rs::failure_domain_contribution` →
  pushed into every candidate's `RoutingExplanation` inside
  `InteractiveRouting::on_provider_failure`'s per-candidate loop, before
  `best()` sums the magnitudes and ranks. **`gateway/session.rs:426` is the
  production caller** — the gateway accept loop's own `observe_exchange`, once
  per connection after the exchange is over.
- `src/routing/interactive.rs::AssignmentChange::benefit()` → `RoutingBenefit`,
  wired into `RoutingRecord::note`'s tracing line as `benefit = %change.benefit()`,
  so every recorded failover *and* every credential rotation carries it.
- The quota domain deliberately has **no new type**: it is `routing::CredentialId`,
  whose own `PartialEq` already answers "is this the same allowance", and which
  `routing::free::FreePool` is already keyed by. A wrapper would only ever
  compare the same way.

Regression evidence:

- `routing::domain::tests::two_credentials_of_one_provider_are_two_quota_domains_and_one_failure_domain`
  — line 1371 asserted on the types themselves.
- `routing::domain::tests::a_different_provider_is_an_unknown_failure_domain_not_a_shared_one`
- `routing::domain::tests::between_can_never_construct_independent` — line 1378,
  structural.
- `routing::interactive::tests::on_provider_failure_prefers_a_different_failure_domain_over_a_shared_one`
  — the load-bearing ranking test.
- `routing::interactive::tests::a_cross_provider_candidate_is_scored_unknown_not_independence`
- `tests/routing_policy.rs::failure_domain::a_candidate_sharing_the_failed_backends_own_provider_loses_to_a_diverse_one`
  — the same claim at the public-API boundary (§35).
- `gateway::session::tests::observe_exchange_records_a_credential_rotation_as_a_different_queue_not_independent_failure_handling`
  — lines 1372 and 1377, driving a **real `429`** through
  `SessionRouting::observe_exchange`, the accept loop's own function.

Failure/isolation evidence:

- **The integrator's own mutation, and it is sharper than the two the packet
  asked for.** The worker ran `remove-guard` (delete the contribution) and
  `invert-condition` (flip its sign); both kill. Neither distinguishes "the
  contribution decides" from "the candidate order decides", because both make
  the *wrong* candidate win outright. So the integrator **neutralised** the
  constant instead — `SHARED_FAILURE_DOMAIN_PENALTY: -1.0 → 0.0` — which makes
  the two candidates tie, and `best()` prefers the first on a tie. The test
  lists the shared-domain candidate **first**, so a tie returns the wrong
  answer and the test fails. It did, in **both** targets: the lib test and
  `tests/routing_policy.rs`'s, each read from its own result line (§5).
  Restored byte-identical.
- **Line 1378's structural test was re-verified under CRLF by the integrator,
  in both directions** (§14/§15). It uses `include_str!` and searches the
  multi-line literal `"\n    pub fn "` — the exact idiom that took Windows CI
  red once — so reasoning about it was not enough. A CRLF copy of `domain.rs`
  passes all three domain tests; the same CRLF copy **with the `alter-boundary`
  mutation applied fails**, and the assertion message printed the extracted
  function body, proving the scan located the real region rather than an empty
  one. The literal survives CRLF because `\r\n    pub fn ` contains
  `\n    pub fn `. Restored byte-identical, 0 CRLF bytes confirmed.
- **A behavioral test cannot prove line 1378, and that is by design.**
  `FailureDomain::Unknown` and `FailureDomain::Independent` score identically
  (both `0.0`, both rendering "independence is not established"), because
  rewarding *any* unproven independence is precisely what 1378 forbids. So the
  `alter-boundary` mutation **survives** the behavioral test and is killed only
  by the structural one. The worker checked whether this was §41's "weak
  mutation, weak test in the same direction" and correctly concluded it is not:
  the indistinguishability is the requirement, so only a structural proof can
  ever kill this class.

Gates run by the integrator on the integrated tree, not taken from the report:
`cargo fmt --all -- --check` clean; `cargo clippy --workspace --all-targets
--all-features -- -D warnings` zero diagnostics; `cargo doc -p glasshouse
--no-deps` clean; routing lib 79 passed, gateway lib 74 passed,
`--test routing_policy` 25 passed, 0 failed.

**Why twelve stay open, grouped by the kind of missing thing.**

- **Needs a response body or stream framing Glasshouse cannot read** — 1364
  (the nine-way failure classification). `classify` in `gateway/session.rs`
  already distinguishes credential rejection (`401..=403`), throttle (`429`),
  upstream 5xx and unreachable, which is four of the nine. `timeout`,
  `stream abort` and `empty completion` need the response stream's framing, and
  `gateway::ingress` is structurally incapable of carrying a body by its own
  module doc — the identical blocker holding Phase 33A lines 1331–1334.
- **Needs temporal correlation across the evidence ledger** — 1370, 1373, 1374,
  1376. Phase 33A's ledger now exists and records per-exchange observations, so
  the *store* is there; what is missing is a component that measures temporally
  overlapping failures between routes and attaches a sample size. Line 1378 —
  now closed — explicitly makes this optional for V1, which is why the round
  closed the fail-closed default first rather than the analysis.
- **~~Needs a cadence signal that is deliberately not collected~~ — STALE as of
  2026-09-01, corrected at integration.** This said
  `WorkloadOutcome::RateLimited { retry_after }` is built with
  `retry_after: None` at `gateway/session.rs:564`. **Line 1319 closed that.**
  The signal now exists and travels the whole way:
  `session::stated_retry_after` (`gateway/session.rs:775`) is called by the
  accept loop (`gateway/mod.rs:632`), reaches
  `WorkloadOutcome::RateLimited { retry_after }` (`gateway/session.rs:835`),
  and `ResourceHealth::fail` turns a declared wait into
  `cooling_down_until = now + declared` (`routing/free.rs:345`), which
  `FreePool::is_available` (`routing/free.rs:495`) already reads.

  **What this unblocks, and what it does not.** 1368 is packaged on this chain
  (GH-PACED-RETRY): the gap is that `is_available` has **no caller anywhere in
  `src/gateway/`**, so when `observe_exchange` finds no sibling to rotate to and
  returns `Unchanged`, the accept loop forwards the *next* request to the same
  cooling-down credential — retrying a paced route in place. 1366 and 1367 are
  **not** unblocked by this: 1366 needs a cadence *learner* distinct from the
  declared remainder, and 1367 needs a reservation model across concurrent
  consumers — `routing/free.rs` has no reservation, lease or in-flight
  machinery at all (verified, not assumed). Neither has an established Phase -1;
  do not package either on the strength of this correction.
- **Needs a probe budget model** — 1369.
- **Needs the route-topology record** — 1377 is closed for the two categories
  this build can honestly produce; see the note below.

**Line 1377, and why it is closed with two categories rather than four.**

The map line names four: *independent capacity, independent quota, independent
failure handling, or merely a different queue onto the same upstream.*
`RoutingBenefit` produces `DifferentQueueSameUpstream` (the fourth, exactly) and
`UnconfirmedFailureDomainChange`, plus `NoChange` which `benefit()`'s match must
answer for completeness.

- **Independent quota is not missing — it is carried.** A credential change *is*
  the quota-domain change, and both producible variants encode it: a rotation
  changes the quota domain and not the failure domain, and a provider change
  changes both.
- **Independent capacity has no producer anywhere in this build.** Phase 32G
  (request-cost estimation) is 0/10 and Phase 33 (resource health) is **7/15**
  (corrected 2026-08-29, batch 47; this sentence read "0/15" and was six ticks
  stale). The correction does not change the claim: none of Phase 33's seven
  closed lines is a *capacity* producer — they track availability, quota state
  and degraded/recovered health, a different axis — so independent capacity
  still has no producer.
- **Independent failure handling may never be asserted**, because line 1378 —
  in this same phase — forbids treating absent evidence as independence, and
  nothing here establishes it.

The honest user question (§33) is *can Glasshouse record whether a routing
change bought real resilience or merely a different queue onto the same
upstream?* It can, on every recorded change, and it refuses to claim what it
cannot establish. Two unreachable enum variants were offered by the worker and
**declined**: `FailureDomain::Independent` is unreachable-but-present because a
structural test proves nothing constructs it, which is load-bearing; a
`RoutingBenefit::IndependentCapacity` nothing constructs and no test needs would
be decoration.


---

## Orchestrator ruling — map line 1377, 2026-08-29 (batch 47)

**Inherited unresolved through three orchestrators. Ruling: the tick stands,
and this question is now closed — do not re-open it without new evidence.**

The line asks Glasshouse to record *which* of four things a routing benefit
came from. `RoutingBenefit` (`routing/interactive.rs:787`) has exactly two
variants — `UnconfirmedFailureDomainChange` and `DifferentQueueSameUpstream` —
and `grep -rn IndependentCapacity crates/glasshouse/src/` returns **nothing**:
the identifier is not constructed, not matched, and not named anywhere in
production source. Verified directly by the orchestrator, not taken from a
report.

That is the line satisfied, not evaded. The four categories are alternatives,
and "merely a different queue onto the same upstream" is one of them and is
recorded. Refusing to assert independent capacity or independent failure
handling is required behaviour here — line 1378, in this same phase, forbids
treating absent evidence as independence. A build that named a category it
cannot establish would breach 1378 to satisfy 1377.

**What the "PARTIAL" language in this entry meant** was that two of the four
categories are unproducible. That is a true statement about the build and a
false reason to doubt the tick, which is why it survived three readings without
resolving. It is ruling 3 of the sweep's three options: the tick is right and
the entry describes a clause the box does not require.


---

# `GH-FAILURE-TAXONOMY` — 2026-08-31: framing is not content, and the relay now says what kind of failure it saw

Fable specialist at xhigh. **Five lines against one ruling**: *"Phase 33: framing
is not content — the relay may count and timestamp what it never reads"*
(`design-decisions.md`). Cluster L's boundary moves exactly as far as that
ruling says: the relay observes the status line, the headers it already
forwards, the byte count it already keeps to relay the body, and how the stream
ended — and **never a byte of body content**, which a structural scan
(`an_exchange_has_nowhere_to_put_a_body`, now also covering `Framing` and
`StreamEnd`) enforces.

Contract: Given a gateway-backed exchange, when the provider answers or fails
to, Glasshouse records **what kind** of failure it was — throttle, exhausted
quota, upstream 5xx, timeout, stream abort, empty completion, credential
failure, request incompatibility, or unknown — so rate-limit responses are
counted apart from transport and model failures and cadence throttling apart
from a spent window, while preserving that the relay never reads, buffers or
interprets response content.

**Production.** `gateway::ingress::forward` gained `Framing { declared, relayed,
ended: Complete | Truncated | Aborted | ClientClosed }` on `Exchange`, filled by
a `Counted<R>` reader that sees each `read`'s byte count and never its buffer
(`relayed = None` when no body was permitted — HEAD/204/304 — so "nothing
arrived" and "nothing was allowed" stay two facts). `gateway::session::failure_class`
maps status + `RateLimitHeaders` + framing to `routing::evidence::FailureClass`
(`classify`, the routing verdict, is untouched — the record is a sibling, and
takes the headers the verdict deliberately narrows away). **Migration 18**
(`ALTER TABLE routing_observations ADD COLUMN failure_class TEXT`, nullable, no
`CHECK`, no index; `database::FAILURE_CLASSES` pinned by a test);
`record_routing_observation` takes an `ExchangeReading` and writes the class,
`failovers` (1 for this exchange's own `ChangeCause::Failover`, 0 for a
credential rotation — the code's own vocabulary keeps them apart) and
`retries = 0` (a count: `forward` calls `Agent::run` once and `ureq` 3.4.0 has
no transparent retry, verified in its source). A `2xx` whose stream was cut or
whose permitted body never came now records `outcome = failed`. Readers:
`EvidenceLedger::failure_classes_by_provider`, `FailureClassCounts` (**with no
`failures()` total on purpose** — 1365's three figures cannot be summed),
`provider/resources.rs::render_failure_classes` beside the health line;
`main.rs::resources_report` gathers the counts (the one line the worker handed
over, applied at integration).

Gates on the integrated tree: fmt, clippy `-D warnings`, rustdoc clean;
`gateway_failure_taxonomy` 7; `gateway_retry_after` 2; `routing_evidence` 13;
`gateway_degrade` 3; blast radius and the full `--lib` run recorded in
`.agent-runtime/blast-ft.log` (the worker's own runs were red only on the
schema-bump ripple in two files outside its scope, whose verified patches were
applied at integration — `session::store::tests` 64/64, `session_context`
18/18 with them). Eight mutations, eight KILLED.

## 1364 — CLOSED

*"Classify failures at least as throttle, exhausted quota, upstream 5xx,
timeout, stream abort, empty completion, credential failure, request
incompatibility, or unknown."* Nine classes decided at one site from
status/headers/framing; eight driven live through a real `TcpStream` and the
production entry point (`each_failure_class_is_recorded_from_status_headers_and_framing_alone`);
`Timeout` unit-tested at the classifier because **the shipped agent sets no
timeout by its own documented decision** (a streaming response may go minutes
between events) — recorded as a limit, and `upstream.rs`'s `timeout_connect`
named as the one-line producer. Limits: a close-delimited response cut
mid-stream reads `Complete` (no served protocol answers that way); a `200`
whose body describes a model error is served — the body is what the relay
cannot read.

## 1365 — CLOSED

Throttle vs exhausted quota is *read*, never guessed: `429` is `ExhaustedQuota`
iff `remaining == Some(0)` **and** the window reopens ≥
`EXHAUSTED_QUOTA_HORIZON_SECONDS = 300` after `first_byte_at` (reset field,
else `retry-after`), else `Throttle`; `402` is `ExhaustedQuota` (the 9H live
finding); provider health is the third bucket by construction
(`FailureClass::is_provider_health`: 5xx, timeout, stream abort, empty
completion, unknown). `resources` renders *cadence throttled N, quota exhausted
N, provider unhealthy N — of N exchange(s), N served*. Killed:
`throttle_and_exhausted_quota_are_told_apart_by_headers_not_guessed`;
`resources_renders_cadence_quota_and_health_as_three_figures_with_denominators`.
Limit: the 300-second horizon is a documented judgement, one constant to lift.

## 1316 — CLOSED at integration (the worker reported *partial*)

*"Track recent rate-limit responses separately from transport or model
failures."* Every exchange's class is recorded in production; the per-provider
"recent, by class" reader and rendering were built and tested through
`GatheredTelemetry::report()`; the worker left it *partial* by §35 because
`main.rs::resources_report` did not yet call `gather_failure_classes` — the
one-line patch it handed over is applied on the integrated tree, so the reader
has its production caller. **Ruling: COMPLETE**, with the honest note that the
call site itself is exercised by the existing `provider_discovery`
shipped-binary tests of `glasshouse resources` and by the blast radius, not by a
test the worker could write from its files.

## 1318 — CLOSED

*"Feed rate-limit events back into the unified capacity estimator."* The loop
was intact and unevidenced: a relayed `429` carrying `X-RateLimit-Limit: 300`,
`-Remaining: 0`, `-Reset: 3600` through a gateway started with a
`GatewayQuotaCache` moves the band `observed_capacity` reports from `unknown`
to `exhausted`, `capacity 0%`
(`a_rate_limited_response_changes_the_capacity_band_the_estimator_reports`);
mutating the `cache.store(..)` call away kills it (§35).

## 1334 — OPEN, on exactly two quantities

`failovers` and `retries` are now written honestly and the outcome proxy is
improved; **`tool_rounds` and `repairs`** need a turn structure and a body this
layer cannot see — a tool round spans several connections and only the harness
or the session above the gateway can count it; a repair is a concept nothing
in the tree holds. `outcome` remains a transport-level proxy for the
user-visible verdict.

Packet errors the worker recorded, all accepted: migration 16 and 17 were
already taken (this is 18); the packet's field names would have failed the
relay's own structural scan (`body_bytes_relayed` → `Framing.relayed`);
`ended` needed a fourth variant (`ClientClosed`) for the two inbound-hop close
paths; `Timeout` has no live producer (kept because the map names it and the
mapping is real code on the path — in tension with `database.rs`'s "variants
follow producers" rule, and said so).

### Lines 1370, 1373, 1374, 1376 — temporal route correlation, with a sample size

Package `GH-ROUTE-CORRELATION`, 2026-08-31, Fable 5 at xhigh. The reader `phase-33c.md:101` said was missing now exists: `correlate_routes` over `RoutingObservation` rows joins routes by overlapping failure windows and matching class, and yields `RouteCorrelations` with a `CorrelationVerdict` per pair. `CORRELATION_OVERLAP_TOLERANCE_SECONDS = 60` is argued from the conservative side — a missed overlap lands on `InsufficientEvidence`, line 1378's safe side, while an invented one penalises a route that did nothing wrong. `MIN_CORRELATION_SAMPLE` is deliberately `MIN_SAMPLE_FOR_SUMMARY` (5): the ledger keeps one answer to *how many observations before a figure is trusted*. `CORRELATION_PURPOSE` rows are excluded from the reader's own input so it cannot read its consequence back as evidence. The production caller is `gateway/session.rs:604` (`observe_exchange`, the only caller of `on_provider_failure`), which the packet's EXPECTED FILES had omitted and the worker added with its reason. Eleven mutations, eleven killed; 61-target blast radius, exit 0.

### Measure temporally overlapping failures between routes rather than assuming different front doors are independent providers. (line 1370)

Contract: Given routing observations on two or more routes, when Glasshouse asks whether two routes fail together, it counts a failure on one route against what the other route was observed doing within CORRELATION_OVERLAP_TOLERANCE_SECONDS of it — overlap, lone, or uninformative when the other route was idle — while preserving that an unobserved route is never read as independent and that a same-provider pair is judged by identity, not by overlap.

State: COMPLETE — ruled 2026-08-31 by the orchestrator from the report's five artifacts and the diff of the decision.

Production evidence:
- `src/routing/evidence.rs` — `correlate_routes`
- `src/routing/evidence.rs` — `overlaps_within`
- `src/routing/evidence.rs` — `CORRELATION_OVERLAP_TOLERANCE_SECONDS`
- `src/routing/evidence.rs` — `RoutingObservation::window`
- `src/routing/evidence.rs` — `EvidenceLedger::route_correlations`
- `src/gateway/session.rs` — `SessionRouting::observe_exchange`

Regression evidence:
- `routing::evidence::correlation_tests::an_overlap_is_measured_within_the_tolerance_and_not_beyond_it`
- `routing::evidence::correlation_tests::a_failure_while_the_other_route_was_idle_informs_nothing`
- `gateway::session::tests::observe_exchange_steers_a_real_failover_off_a_route_the_ledger_shows_failing_with_it`
- `route_correlation::correlations_are_read_from_a_real_ledger_with_their_sample_size_and_window`

| mutation | vocabulary | result | killed by |
|---|---|---|---|
| if !overlaps_within(window, row.window(), CORRELATION_OVERLAP_TOLERANCE_SECONDS) { -> if false { | `remove-guard` | **killed** | `routing::evidence::correlation_tests::an_overlap_is_measured_within_the_tolerance_and_not_beyond_it` |
| explanation.push(contribution); -> let _ = contribution; | `skip-state-update` | **killed** | `routing::interactive::tests::on_provider_failure_steers_off_a_measured_correlation_and_names_the_route` |
| gateway/session.rs: &correlations, -> &RouteCorrelations::default(), | `skip-state-update` | **killed** | `gateway::session::tests::observe_exchange_steers_a_real_failover_off_a_route_the_ledger_shows_failing_with_it` |

> remove-guard observed: assertion `left == right` failed: ... RouteCorrelation { overlaps: 4, lone: 0 } (expected (2, 1))

> skip-state-update observed: a route observed failing with the failed backend every time must lose to one with no such record, even though it is configured first

> skip-state-update observed: a route the ledger shows failing at the same moments as the failed backend must lose the failover to one it shows serving through them, even though it is configured first

Recorded scope limits — stated by the worker, not discovered later:
- Throttle overlap is proven in the pure function only; the gateway-level test uses Upstream5xx
- No Windows leg


---


### Treat correlated model-specific 5xx events, matching provider metadata, or matching serving behavior as evidence of a shared failure domain. (line 1373)

Contract: Given two failures at the same moment, when Glasshouse decides whether they are one shared-domain event, it requires the same failure class (Upstream5xx or Throttle) and keys the pair by the provider and model already on the rows, fetching nothing — while preserving that a different class beside a failure is evidence the other route did not share it, and that the wire-protocol `route` column plays no part.

State: COMPLETE — ruled 2026-08-31 by the orchestrator from the report's five artifacts and the diff of the decision.

Production evidence:
- `src/routing/evidence.rs` — `FailureClass::is_correlatable`
- `src/routing/evidence.rs` — `RouteIdentity`
- `src/routing/evidence.rs` — `count_failures_against`
- `src/routing/interactive.rs` — `route_correlation_contribution`

Regression evidence:
- `routing::evidence::correlation_tests::a_correlation_is_model_specific_not_provider_wide`
- `routing::evidence::correlation_tests::a_different_failure_class_at_the_same_moment_is_not_a_match`
- `routing::interactive::tests::a_same_provider_candidate_carries_no_correlation_term`

| mutation | vocabulary | result | killed by |
|---|---|---|---|
| RouteIdentity::new(&row.provider, &row.model) -> RouteIdentity::new(&row.provider, "") | `alter-boundary` | **killed** | `routing::evidence::correlation_tests::a_correlation_is_model_specific_not_provider_wide` |
| if row.failure_class == Some(class) { -> if row.failure_class.is_some() { | `invert-condition` | **killed** | `routing::evidence::correlation_tests::a_different_failure_class_at_the_same_moment_is_not_a_match` |

> alter-boundary observed: assertion `left == right` failed at evidence.rs:3750 — the same-model pair read back unmeasured

> invert-condition observed: three observed failures, none matched: RouteCorrelation { overlaps: 3, lone: 0 }

Recorded scope limits — stated by the worker, not discovered later:
- The line's third clause, `matching serving behavior`, is read as same-class-at-the-same-moment; no latency or framing signature is compared


---


### Preserve route-topology claims as confidence-weighted observations that can change when new evidence arrives. (line 1374)

Contract: Given a pair of routes, when Glasshouse states how likely they are to share a failure domain, it states a confidence in [0,1] recomputed from the rows on every read that new overlapping failures raise and new lone failures lower, and the router weighs it as that share of a shared provider's penalty — while preserving that nothing is stored as a same-provider flag and that FailureDomain::between still never returns Independent.

State: COMPLETE — ruled 2026-08-31 by the orchestrator from the report's five artifacts and the diff of the decision.

Production evidence:
- `src/routing/evidence.rs` — `RouteCorrelation::verdict`
- `src/routing/evidence.rs` — `RouteCorrelation::confidence`
- `src/routing/interactive.rs` — `correlation_penalty`
- `src/routing/interactive.rs` — `route_correlation_contribution`

Regression evidence:
- `routing::evidence::correlation_tests::new_rows_move_the_confidence_both_ways`
- `routing::interactive::tests::on_provider_failure_steers_off_a_measured_correlation_and_names_the_route`

| mutation | vocabulary | result | killed by |
|---|---|---|---|
| confidence: self.overlaps as f64 / sample_size as f64, -> confidence: 1.0, | `freeze-value` | **killed** | `routing::evidence::correlation_tests::new_rows_move_the_confidence_both_ways` |

> freeze-value observed: assertion `left == right` failed: RouteCorrelation { overlaps: 10, lone: 10 } (expected Some(0.5))

Recorded scope limits — stated by the worker, not discovered later:
- The confidence is a plain ratio; no recency weighting inside the window


---


### Require sufficient overlapping observations and expose sample size before presenting a route correlation as meaningful. (line 1376)

Contract: Given fewer than MIN_CORRELATION_SAMPLE informative failure events for a pair, when Glasshouse reports or weighs that pair, it reports InsufficientEvidence with the count and the requirement, contributes exactly 0.0 to the ranking, and `glasshouse route` prints the sample size before any confidence — while preserving that a measured pair prints its overlaps of its sample size beside the confidence.

State: COMPLETE — ruled 2026-08-31 by the orchestrator from the report's five artifacts and the diff of the decision.

Production evidence:
- `src/routing/evidence.rs` — `MIN_CORRELATION_SAMPLE`
- `src/routing/evidence.rs` — `CorrelationVerdict`
- `src/routing/interactive.rs` — `route_correlation_contribution`
- `src/main.rs` — `route_correlations_section`

Regression evidence:
- `routing::evidence::correlation_tests::below_the_minimum_sample_the_verdict_is_insufficient_and_says_the_count`
- `routing::interactive::tests::on_provider_failure_treats_insufficient_correlation_evidence_exactly_as_none`
- `route_correlation::the_route_command_prints_every_pairs_sample_size_before_any_correlation`

| mutation | vocabulary | result | killed by |
|---|---|---|---|
| if sample_size < MIN_CORRELATION_SAMPLE { -> if sample_size < 1 { | `alter-boundary` | **killed** | `routing::evidence::correlation_tests::below_the_minimum_sample_the_verdict_is_insufficient_and_says_the_count` |
| same change, judged at the router | `alter-boundary` | **killed** | `routing::interactive::tests::on_provider_failure_treats_insufficient_correlation_evidence_exactly_as_none` |

> alter-boundary observed: assertion `left == right` failed at evidence.rs:3827 — four events reported as Measured

> alter-boundary observed: assertion `left == right` failed: configuration order still decides

Recorded scope limits — stated by the worker, not discovered later:
- The `glasshouse route` test drives the shipped binary on macOS only

